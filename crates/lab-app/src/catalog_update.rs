//! Background Thorlabs catalog update: fetches search pages through `curl.exe` (at most one request
//! per 30 s), caches them in `%LOCALAPPDATA%\LabSetup\catalog-update\` for resume, and swaps the
//! result into the catalog database in one transaction.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use lab_catalog::db::{CatalogDb, UpdateError};
use lab_catalog::thorlabs::{self, Planner, EXCLUDE, GRAPHQL_URL, SECTIONS};

const GAP_S: i64 = 30; // robots.txt Crawl-delay
const RETRY_WAIT_S: u64 = 60;
const TRIES: usize = 3;
const CACHE_MAX_AGE_S: i64 = 7 * 86_400;
const LOCK_STALE_S: i64 = 5 * 60;
const TICK: Duration = Duration::from_millis(250);
const LOCK: &str = ".lock";
const STARTED: &str = "started";
const SITE_CHANGED: &str =
    "Update stopped: Thorlabs search results are no longer in the expected format. The old parts are kept.";

static RUNNING: AtomicBool = AtomicBool::new(false);
static CANCEL: AtomicBool = AtomicBool::new(false);

/// Sent from the worker thread. `Notice` is shown until the next `Progress`; a cancelled update sends nothing.
#[allow(dead_code)] // Task 6 wires the Catalog window to this module
pub enum UpdateEvent {
    Progress { section: String, done: usize, total: usize },
    Notice(String),
    Finished(usize),
    Stopped(String),
}

/// Starts the update on a background thread; false if this or another LabSetup process is already updating.
#[allow(dead_code)]
pub fn start(on_event: impl Fn(UpdateEvent) + Send + 'static) -> bool {
    if RUNNING.swap(true, Ordering::SeqCst) {
        return false;
    }
    let Some(dir) = cache_dir().filter(|d| take_lock(d)) else {
        RUNNING.store(false, Ordering::SeqCst);
        return false;
    };
    CANCEL.store(false, Ordering::SeqCst);
    std::thread::spawn(move || {
        // Released before the last event, so `running()` is already false when the UI hears of the end.
        let result = {
            let _release = Release(dir.clone());
            run(&dir, &on_event)
        };
        match result {
            Ok(n) => on_event(UpdateEvent::Finished(n)),
            Err(Halt::Stop(text)) => on_event(UpdateEvent::Stopped(text)),
            Err(Halt::Cancelled) => {}
        }
    });
    true
}

#[allow(dead_code)]
pub fn cancel() {
    CANCEL.store(true, Ordering::SeqCst);
}

#[allow(dead_code)]
pub fn running() -> bool {
    RUNNING.load(Ordering::SeqCst)
}

enum Halt {
    Cancelled,
    Stop(String),
}

fn generic(reason: impl std::fmt::Display) -> Halt {
    Halt::Stop(format!("Update stopped: {reason}. Press Update to continue."))
}

/// Removes the lock (and the cache dir if it is now empty) and clears `RUNNING`, also on a panic.
struct Release(PathBuf);

impl Drop for Release {
    fn drop(&mut self) {
        let _ = fs::remove_file(self.0.join(LOCK));
        let _ = fs::remove_dir(&self.0);
        RUNNING.store(false, Ordering::SeqCst);
    }
}

fn run(dir: &Path, on_event: &dyn Fn(UpdateEvent)) -> Result<usize, Halt> {
    let db_path = lab_catalog::catalog_db_path().ok_or_else(|| generic("LOCALAPPDATA is not set"))?;
    let mut db = CatalogDb::open(&db_path).map_err(generic)?;

    let started = fs::read_to_string(dir.join(STARTED)).ok().and_then(|s| s.trim().parse::<i64>().ok());
    match started {
        Some(t) if !cache_expired(t, now_unix()) => {}
        other => {
            // Pages are fetched by offset, so old pages would mix with shifted new ones.
            clear_cache(dir);
            if other.is_some() {
                on_event(UpdateEvent::Notice("Starting over: the saved progress is more than a week old.".into()));
            }
            fs::write(dir.join(STARTED), now_unix().to_string()).map_err(generic)?;
        }
    }

    db.stage_begin().map_err(generic)?;
    let mut planner = Planner::new(&SECTIONS);
    while let Some(req) = planner.next() {
        if CANCEL.load(Ordering::SeqCst) {
            return Err(Halt::Cancelled);
        }
        let file = dir.join(thorlabs::cache_name(&req.query, req.offset));
        let (json, fresh) = match fs::read_to_string(&file) {
            Ok(json) => (json, false),
            Err(_) => (fetch(dir, &db, &thorlabs::request_body(&req.query, req.offset))?, true),
        };
        let page = thorlabs::parse_page(&json, &req.section, &EXCLUDE).map_err(|_| Halt::Stop(SITE_CHANGED.into()))?;
        if fresh {
            // Cached only once it parses, and via a rename, so a resume never meets a broken page.
            let tmp = file.with_extension("tmp");
            fs::write(&tmp, &json).and_then(|_| fs::rename(&tmp, &file)).map_err(generic)?;
        }
        db.stage_insert(&req.section, &page.items).map_err(generic)?;
        planner.accept(&page);
        let (section, done, total) = planner.progress();
        on_event(UpdateEvent::Progress { section, done, total });
    }

    match db.finish_update(&SECTIONS, now_unix()) {
        Ok(n) => {
            clear_cache(dir);
            Ok(n)
        }
        Err(UpdateError::Drop(d)) => {
            clear_cache(dir); // the refused result is not kept: the next Update fetches fresh data
            Err(Halt::Stop(format!(
                "Update stopped: {} returned {} parts, last time {}. The site may have changed; the old parts are kept.",
                d.section,
                thousands(d.got),
                thousands(d.last)
            )))
        }
        Err(UpdateError::Db(e)) => Err(generic(e)),
    }
}

/// POSTs one search page: at most one request per 30 s (also across restarts), 3 tries 60 s apart.
fn fetch(dir: &Path, db: &CatalogDb, body: &str) -> Result<String, Halt> {
    let mut error = String::new();
    for attempt in 0..TRIES {
        if attempt > 0 {
            sleep_or_cancel(RETRY_WAIT_S)?;
        }
        let last = db.meta_get("last_request").and_then(|v| v.parse().ok());
        sleep_or_cancel(wait_before_next(last, now_unix()))?;
        let _ = fs::write(dir.join(LOCK), std::process::id().to_string()); // touch: keeps the lock fresh
        db.meta_set("last_request", &now_unix().to_string()).map_err(generic)?;
        match curl_or_cancel(body.to_owned())? {
            Ok(bytes) => return Ok(String::from_utf8_lossy(&bytes).into_owned()),
            Err(e) => error = e,
        }
    }
    Err(generic(error))
}

/// Runs curl on its own thread so Cancel need not wait up to 120 s for it; a cancelled request is
/// left to finish and its result dropped.
fn curl_or_cancel(body: String) -> Result<Result<Vec<u8>, String>, Halt> {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let args = ["-X", "POST", "-H", "Content-Type: application/json", "--data-binary", "@-", GRAPHQL_URL];
        let _ = tx.send(crate::import_url::curl(&args, Some(&body)));
    });
    loop {
        match rx.recv_timeout(TICK) {
            Ok(result) => return Ok(result),
            Err(RecvTimeoutError::Timeout) if CANCEL.load(Ordering::SeqCst) => return Err(Halt::Cancelled),
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => return Ok(Err("the download stopped unexpectedly".into())),
        }
    }
}

fn sleep_or_cancel(secs: u64) -> Result<(), Halt> {
    let end = Instant::now() + Duration::from_secs(secs);
    loop {
        if CANCEL.load(Ordering::SeqCst) {
            return Err(Halt::Cancelled);
        }
        let left = end.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return Ok(());
        }
        std::thread::sleep(left.min(TICK));
    }
}

fn cache_dir() -> Option<PathBuf> {
    Some(lab_catalog::catalog_db_path()?.parent()?.join("catalog-update"))
}

/// shortcut: two processes taking over the same stale lock in the same instant could both run; the
/// window is milliseconds after a 5-minute-old lock, so a real OS lock isn't worth it yet.
fn take_lock(dir: &Path) -> bool {
    let lock = dir.join(LOCK);
    let _ = fs::create_dir_all(dir);
    let create = || fs::OpenOptions::new().write(true).create_new(true).open(&lock).is_ok();
    if create() {
        return true;
    }
    let mtime = fs::metadata(&lock).and_then(|m| m.modified()).map(unix);
    matches!(mtime, Ok(t) if lock_is_stale(t, now_unix())) && fs::remove_file(&lock).is_ok() && create()
}

/// Deletes the cached pages and the `started` stamp, keeping the lock.
fn clear_cache(dir: &Path) {
    for entry in fs::read_dir(dir).into_iter().flatten().flatten() {
        if entry.file_name() != LOCK {
            let _ = fs::remove_file(entry.path());
        }
    }
}

fn unix(t: SystemTime) -> i64 {
    t.duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64)
}

fn now_unix() -> i64 {
    unix(SystemTime::now())
}

/// Seconds to sleep so requests are at least 30 s apart (a clock set back waits at most 30 s).
fn wait_before_next(last_request_unix: Option<i64>, now_unix: i64) -> u64 {
    last_request_unix.map_or(0, |t| (GAP_S - (now_unix - t)).clamp(0, GAP_S) as u64)
}

fn cache_expired(started_unix: i64, now_unix: i64) -> bool {
    now_unix - started_unix > CACHE_MAX_AGE_S
}

fn lock_is_stale(lock_mtime_unix: i64, now_unix: i64) -> bool {
    now_unix - lock_mtime_unix > LOCK_STALE_S
}

/// `5696` → `5,696`.
pub(crate) fn thousands(n: usize) -> String {
    let digits = n.to_string();
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn waits_at_least_30_seconds() {
        assert_eq!(wait_before_next(None, 1000), 0);
        assert_eq!(wait_before_next(Some(990), 1000), 20);
        assert_eq!(wait_before_next(Some(900), 1000), 0);
    }

    #[test]
    fn cache_expires_after_seven_days() {
        assert!(!cache_expired(0, 7 * 86_400));
        assert!(cache_expired(0, 7 * 86_400 + 1));
    }

    #[test]
    fn stale_lock_is_taken_over() {
        assert!(!lock_is_stale(1000, 1000 + 299));
        assert!(lock_is_stale(1000, 1000 + 301));
    }

    #[test]
    fn thousands_separators() {
        assert_eq!(thousands(0), "0");
        assert_eq!(thousands(999), "999");
        assert_eq!(thousands(5696), "5,696");
        assert_eq!(thousands(1_234_567), "1,234,567");
    }
}
