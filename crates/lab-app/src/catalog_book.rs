//! "Catalog…" (POLA-173, POLA-193): browse the catalog database (`lab_catalog::db`), keep a pick
//! list in `%APPDATA%\LabSetup\catalog-list.json`, download & import the list in one batch, and
//! run the in-app Thorlabs update (`catalog_update`).

use std::cell::{Cell, RefCell};
use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use lab_catalog::book::BookItem;
use lab_catalog::db::CatalogDb;
use lab_ui::{AppWindow, CatalogRow};
use slint::{ComponentHandle, Model, ModelRc, VecModel};

use crate::app_state::AppState;
use crate::catalog_update::{self, UpdateEvent, thousands};

const MAX_ROWS: usize = 500;
const DURATION_NOTE: &str = "A full update takes one to two hours.";
const START_FAILED: &str =
    "Update stopped: another LabSetup window is updating, or the update folder is not writable. Press Update to continue.";

#[derive(Default)]
struct State {
    query: String,
    /// Index into `categories()`; 0 is "All".
    category: usize,
    selected: Option<String>,
    list: Vec<String>,
}

thread_local! {
    static STATE: RefCell<State> = RefCell::new(State::default());
    /// Decoded thumbnails by part, so no image is decoded twice in a session.
    static THUMBS: RefCell<HashMap<String, slint::Image>> = RefCell::new(HashMap::new());
    /// Search debounce: restarted on every keystroke.
    static SEARCH_TIMER: slint::Timer = slint::Timer::default();
    /// The UI thread's own connection; `None` until the startup sync is done (WAL lets it read
    /// while the updater writes).
    static DB: RefCell<Option<CatalogDb>> = const { RefCell::new(None) };
    /// For refreshes that start outside `connect`'s callback (startup sync, update events).
    static APP: RefCell<Option<Rc<RefCell<AppState>>>> = const { RefCell::new(None) };
    /// Bumped on every Update and Cancel; events of an older update are ignored.
    static UPDATE_GEN: Cell<u64> = const { Cell::new(0) };
    /// Watches `catalog_update::running()` while an update runs (a panic sends no event).
    static UPDATE_POLL: slint::Timer = slint::Timer::default();
}

/// `f` on the UI connection; the default (empty) result while the database is not ready.
fn with_db<T: Default>(f: impl FnOnce(&CatalogDb) -> T) -> T {
    DB.with(|d| d.borrow().as_ref().map(f).unwrap_or_default())
}

/// "All", then the distinct second-level categories, sorted.
fn categories() -> Vec<String> {
    std::iter::once("All".to_string()).chain(with_db(CatalogDb::categories)).collect()
}

// shortcut: items are keyed by part number alone, so two vendors sharing one show the first; key by (vendor, part) if that happens.
fn find(part: &str) -> Option<BookItem> {
    with_db(|db| db.find(part))
}

fn list_path() -> Option<PathBuf> {
    std::env::var_os("APPDATA").map(|d| PathBuf::from(d).join("LabSetup").join("catalog-list.json"))
}

fn load_list() -> Vec<String> {
    list_path().and_then(|p| std::fs::read_to_string(p).ok()).and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
}

fn save_list(list: &[String]) {
    let Some(path) = list_path() else { return };
    let written = path
        .parent()
        .map_or(Ok(()), std::fs::create_dir_all)
        .and_then(|_| std::fs::write(&path, serde_json::to_string_pretty(list).unwrap_or_default()));
    if let Err(error) = written {
        eprintln!("catalog list not saved: {error}");
    }
}

/// Creates the catalogs folder if missing and opens it in Explorer.
fn open_catalogs_folder() {
    let Some(dir) = lab_catalog::catalogs_dir() else { return };
    if let Err(error) = std::fs::create_dir_all(&dir) {
        eprintln!("could not create {}: {error}", dir.display());
        return;
    }
    if let Err(error) = std::process::Command::new("explorer.exe").arg(&dir).spawn() {
        eprintln!("could not open {}: {error}", dir.display());
    }
}

/// Opens `url` in the default browser; only http(s) links are opened.
pub fn open_url(url: &str) {
    if !(url.starts_with("https://") || url.starts_with("http://")) {
        return;
    }
    // rundll32 passes the URL through untouched (`cmd /c start` would split it at '&').
    let mut cmd = std::process::Command::new("rundll32.exe");
    cmd.args(["url.dll,FileProtocolHandler", url]);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    if let Err(error) = cmd.spawn() {
        eprintln!("could not open {url}: {error}");
    }
}

/// `%LOCALAPPDATA%\LabSetup\thumbs\<vendor>_<part, / → _>.<ext>` for an item with an image.
fn thumb_path(item: &BookItem) -> Option<PathBuf> {
    let url = item.image.as_deref()?;
    let ext = url.split('?').next()?.rsplit('.').next()?.to_lowercase();
    if !["jpg", "jpeg", "png", "webp"].contains(&ext.as_str()) {
        return None;
    }
    let dir = PathBuf::from(std::env::var_os("LOCALAPPDATA")?).join("LabSetup").join("thumbs");
    Some(dir.join(format!("{}_{}.{ext}", item.vendor, item.part).replace(['/', '\\'], "_")))
}

/// The cached thumbnail, or an empty image (the placeholder shows through) while missing or failed.
fn thumb(item: &BookItem) -> slint::Image {
    if let Some(image) = THUMBS.with(|t| t.borrow().get(&item.part).cloned()) {
        return image;
    }
    let Some(image) = thumb_path(item).filter(|p| p.exists()).and_then(|p| load_thumb(&p)) else { return Default::default() };
    THUMBS.with(|t| t.borrow_mut().insert(item.part.clone(), image.clone()));
    image
}

/// Slint's image loader has no webp, so webp is decoded here (POLA-180).
fn load_thumb(path: &Path) -> Option<slint::Image> {
    if path.extension().is_none_or(|e| e != "webp") {
        return slint::Image::load_from_path(path).ok();
    }
    let file = std::io::BufReader::new(std::fs::File::open(path).ok()?);
    let mut decoder = image_webp::WebPDecoder::new(file).ok()?;
    let (w, h) = decoder.dimensions();
    let mut buf = vec![0; decoder.output_buffer_size()?];
    decoder.read_image(&mut buf).ok()?;
    if decoder.has_alpha() {
        let mut pixels = slint::SharedPixelBuffer::<slint::Rgba8Pixel>::new(w, h);
        pixels.make_mut_bytes().copy_from_slice(&buf);
        Some(slint::Image::from_rgba8(pixels))
    } else {
        let mut pixels = slint::SharedPixelBuffer::<slint::Rgb8Pixel>::new(w, h);
        pixels.make_mut_bytes().copy_from_slice(&buf);
        Some(slint::Image::from_rgb8(pixels))
    }
}

/// Download queue (popped from the back), parts that failed this session, worker flag.
static QUEUE: Mutex<Vec<(String, String, PathBuf)>> = Mutex::new(Vec::new());
static FAILED: Mutex<BTreeSet<String>> = Mutex::new(BTreeSet::new());
static RUNNING: AtomicBool = AtomicBool::new(false);

/// Replaces the queue with the missing thumbnails of `items` (in order) and starts the single
/// worker thread if it is not running; the worker exits when the queue is empty.
fn fetch_thumbs(ui: slint::Weak<AppWindow>, items: &[BookItem]) {
    let failed = FAILED.lock().unwrap().clone();
    let mut jobs: Vec<_> = items
        .iter()
        .filter(|i| !failed.contains(&i.part))
        .filter_map(|i| Some((i.part.clone(), i.image.clone()?, thumb_path(i)?)))
        .filter(|(_, _, p)| !p.exists())
        .collect();
    jobs.reverse();
    let empty = jobs.is_empty();
    *QUEUE.lock().unwrap() = jobs;
    if empty || RUNNING.swap(true, Ordering::SeqCst) {
        return;
    }
    std::thread::spawn(move || loop {
        let job = QUEUE.lock().unwrap().pop();
        let Some((part, url, path)) = job else {
            RUNNING.store(false, Ordering::SeqCst);
            // A search may have queued work between the pop and the store.
            if QUEUE.lock().unwrap().is_empty() || RUNNING.swap(true, Ordering::SeqCst) {
                return;
            }
            continue;
        };
        let tmp = path.with_extension("part");
        let done = path
            .parent()
            .map_or(Ok(()), std::fs::create_dir_all)
            .map_err(|e| e.to_string())
            .and_then(|_| crate::import_url::curl(&[&url], None))
            .and_then(|bytes| std::fs::write(&tmp, bytes).and_then(|_| std::fs::rename(&tmp, &path)).map_err(|e| e.to_string()));
        if let Err(error) = done {
            eprintln!("thumbnail {part}: {error}");
            FAILED.lock().unwrap().insert(part);
            continue;
        }
        let _ = ui.upgrade_in_event_loop(move |ui| set_thumb(&ui, &part, &path));
    });
}

fn set_thumb(ui: &AppWindow, part: &str, path: &Path) {
    let Some(image) = load_thumb(path) else { return };
    THUMBS.with(|t| t.borrow_mut().insert(part.to_string(), image.clone()));
    for model in [ui.get_catalog_rows(), ui.get_catalog_list()] {
        for i in 0..model.row_count() {
            if let Some(mut r) = model.row_data(i).filter(|r| r.part == part) {
                r.thumb = image.clone();
                model.set_row_data(i, r);
            }
        }
    }
}

/// Part numbers of the parts in the user library (ids are `<vendor>:<part>`).
fn in_library(app: &AppState) -> BTreeSet<String> {
    let c = &app.catalog;
    c.all()
        .iter()
        .filter(|d| c.source(&d.id) == Some(lab_catalog::Source::User))
        .filter_map(|d| d.id.split_once(':').map(|(_, p)| p.to_string()))
        .collect()
}

fn row(item: &BookItem, library: &BTreeSet<String>, selected: bool) -> CatalogRow {
    let badge = if library.contains(&item.part) {
        "In library"
    } else if item.step.is_none() {
        "No STEP"
    } else {
        ""
    };
    CatalogRow { part: item.part.as_str().into(), name: format!("{} · {}", item.vendor, item.name).into(), badge: badge.into(), selected, thumb: thumb(item) }
}

fn refresh(ui: &AppWindow, app: &AppState) {
    STATE.with(|s| {
        let s = s.borrow();
        let library = in_library(app);
        let categories = categories();
        let category = categories.get(s.category).filter(|_| s.category > 0).map(String::as_str);
        let (hits, total) = with_db(|db| db.search(&s.query, category, MAX_ROWS));
        let selected = |i: &BookItem| s.selected.as_deref() == Some(i.part.as_str());
        let rows: Vec<CatalogRow> = hits.iter().map(|i| row(i, &library, selected(i))).collect();
        ui.set_catalog_rows(ModelRc::new(VecModel::from(rows)));
        fetch_thumbs(ui.as_weak(), &hits);
        ui.set_catalog_count(
            if total > MAX_ROWS { format!("showing {MAX_ROWS} of {}", thousands(total)) } else { format!("{} parts", thousands(total)) }.into(),
        );
        let list: Vec<CatalogRow> = s.list.iter().filter_map(|p| find(p)).map(|i| row(&i, &library, selected(&i))).collect();
        ui.set_catalog_list(ModelRc::new(VecModel::from(list)));
        detail(ui, &s);
    });
}

/// The detail pane for the selected part.
fn detail(ui: &AppWindow, s: &State) {
    let item = s.selected.as_deref().and_then(find);
    let item = item.as_ref();
    let text = |f: fn(&BookItem) -> Option<&String>| item.and_then(f).map_or_else(Default::default, |t| t.as_str().into());
    ui.set_catalog_part(text(|i| Some(&i.part)));
    ui.set_catalog_name(text(|i| Some(&i.name)));
    let path = |i: &BookItem| std::iter::once(&i.vendor).chain(&i.category).cloned().collect::<Vec<_>>().join(" › ");
    ui.set_catalog_path(item.map_or_else(Default::default, |i| path(i).into()));
    ui.set_catalog_description(text(|i| Some(&i.description)));
    ui.set_catalog_url(text(|i| Some(&i.url)));
    ui.set_catalog_cad_pdf(text(|i| i.cad_pdf.as_ref()));
    ui.set_catalog_step_url(text(|i| i.step.as_ref()));
    ui.set_catalog_listed(item.is_some_and(|i| s.list.contains(&i.part)));
}

/// Moves the `selected` flag in both models and updates the detail pane; nothing is rebuilt.
fn select(ui: &AppWindow, part: &str) {
    for model in [ui.get_catalog_rows(), ui.get_catalog_list()] {
        for i in 0..model.row_count() {
            if let Some(mut r) = model.row_data(i).filter(|r| r.selected != (r.part == part)) {
                r.selected = r.part == part;
                model.set_row_data(i, r);
            }
        }
    }
    STATE.with(|s| detail(ui, &s.borrow()));
}

/// Shows the window; the list is read from disk each time it opens.
pub fn open(ui: &AppWindow, app: &AppState) {
    STATE.with(|s| {
        let mut s = s.borrow_mut();
        s.list = load_list();
        // The search box opens empty, so the filter must too.
        s.query.clear();
    });
    load_meta(ui);
    ui.set_catalog_category(STATE.with(|s| s.borrow().category) as i32);
    refresh(ui, app);
    ui.set_catalog_open(true);
}

/// Categories, the empty flag and the override hint, from the database.
fn load_meta(ui: &AppWindow) {
    ui.set_catalog_categories(ModelRc::new(VecModel::from(categories().into_iter().map(Into::into).collect::<Vec<_>>())));
    let has_rows = with_db(|db| !db.is_empty());
    ui.set_catalog_empty(!has_rows && !ui.get_catalog_updating());
    let hint = with_db(CatalogDb::overrides).into_iter().next().map(|(file, n)| {
        format!("{file} overrides {} updated Thorlabs parts. Remove it from the catalogs folder to use the update.", thousands(n))
    });
    ui.set_catalog_hint(hint.unwrap_or_default().into());
}

/// `Thorlabs parts updated <date> · A full update takes one to two hours.`
fn idle_status() -> String {
    let updated = with_db(CatalogDb::last_updated).map_or_else(
        || "Thorlabs parts never updated".to_string(),
        |t| {
            let time = std::time::UNIX_EPOCH + Duration::from_secs(t.max(0) as u64);
            let [year, month, day, ..] = crate::files::local_parts(time);
            format!("Thorlabs parts updated {year:04}-{month:02}-{day:02}")
        },
    );
    format!("{updated} · {DURATION_NOTE}")
}

/// Opens the database and imports changed catalog JSON files on a background thread, then gives
/// the UI thread its own connection and shows the rows.
pub fn startup_sync(ui: &AppWindow) {
    ui.set_catalog_loading(true);
    let w = ui.as_weak();
    std::thread::spawn(move || {
        let path = lab_catalog::catalog_db_path();
        match path.as_deref().map(CatalogDb::open) {
            Some(Ok(mut db)) => {
                if let Some(dir) = lab_catalog::catalogs_dir() {
                    for (file, error) in db.sync_json_dir(&dir).skipped {
                        eprintln!("catalog {file} skipped: {error}");
                    }
                }
            }
            Some(Err(error)) => eprintln!("catalog database not opened: {error}"),
            None => eprintln!("catalog database not opened: LOCALAPPDATA is not set"),
        }
        let _ = w.upgrade_in_event_loop(move |ui| {
            let db = path.as_deref().and_then(|p| CatalogDb::open(p).inspect_err(|e| eprintln!("catalog database not opened: {e}")).ok());
            DB.with(|d| *d.borrow_mut() = db);
            ui.set_catalog_loading(false);
            ui.set_catalog_update_status(idle_status().into());
            show_db(&ui);
        });
    });
}

/// After the database changed: meta, and the rows if the window is open.
fn show_db(ui: &AppWindow) {
    load_meta(ui);
    if ui.get_catalog_open()
        && let Some(app) = APP.with(|a| a.borrow().clone())
    {
        refresh(ui, &app.borrow());
    }
}

/// Ends the update state: the button turns back into Update and `status` is shown.
fn update_ended(ui: &AppWindow, status: String) {
    UPDATE_POLL.with(|t| t.stop());
    ui.set_catalog_updating(false);
    ui.set_catalog_update_status(status.into());
    load_meta(ui);
}

fn stopped(reason: &str) -> String {
    format!("Update stopped: {reason}. Press Update to continue.")
}

fn on_update_event(ui: &AppWindow, generation: u64, event: UpdateEvent) {
    if UPDATE_GEN.with(Cell::get) != generation {
        return; // an update the user cancelled
    }
    match event {
        UpdateEvent::Progress { section, done, total } => ui
            .set_catalog_update_status(format!("Updating Thorlabs: {section} {} / {}", thousands(done), thousands(total)).into()),
        UpdateEvent::Notice(text) => ui.set_catalog_update_status(text.into()),
        UpdateEvent::Finished(n) => {
            eprintln!("catalog update: {n} Thorlabs parts stored");
            update_ended(ui, idle_status());
            show_db(ui);
        }
        UpdateEvent::Stopped(text) => update_ended(ui, text),
    }
}

fn start_update(ui: &AppWindow) {
    // A cancelled worker notices within a quarter second; give it that before starting a new one.
    wait_until_stopped(Duration::from_secs(2));
    let generation = UPDATE_GEN.with(|g| {
        g.set(g.get() + 1);
        g.get()
    });
    let w = ui.as_weak();
    let started = catalog_update::start(move |event| {
        let _ = w.upgrade_in_event_loop(move |ui| on_update_event(&ui, generation, event));
    });
    if !started {
        return update_ended(ui, START_FAILED.into());
    }
    ui.set_catalog_updating(true);
    ui.set_catalog_empty(false);
    let w = ui.as_weak();
    let misses = Cell::new(0);
    UPDATE_POLL.with(|t| {
        t.start(slint::TimerMode::Repeated, Duration::from_secs(1), move || {
            // The worker clears `running()` just before its last event, so one miss may be that event in flight.
            misses.set(if catalog_update::running() { 0 } else { misses.get() + 1 });
            if misses.get() >= 2
                && let Some(ui) = w.upgrade().filter(|_| UPDATE_GEN.with(Cell::get) == generation)
            {
                update_ended(&ui, stopped("the update ended unexpectedly"));
            }
        })
    });
}

fn cancel_update(ui: &AppWindow) {
    catalog_update::cancel();
    UPDATE_GEN.with(|g| g.set(g.get() + 1));
    update_ended(ui, stopped("cancelled"));
}

fn wait_until_stopped(limit: Duration) {
    let end = Instant::now() + limit;
    while catalog_update::running() && Instant::now() < end {
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// At quit: cancels a running update and waits up to 2 s, so its lock does not block the next launch.
pub fn stop_update() {
    if catalog_update::running() {
        catalog_update::cancel();
        wait_until_stopped(Duration::from_secs(2));
    }
}

/// After a book batch: parts now in the library leave the list; failed ones stay.
pub fn after_import(ui: &AppWindow, app: &AppState) {
    let library = in_library(app);
    STATE.with(|s| {
        let mut s = s.borrow_mut();
        s.list.retain(|p| !library.contains(p));
        save_list(&s.list);
    });
    if ui.get_catalog_open() {
        refresh(ui, app);
    }
}

pub fn connect(ui: &AppWindow, app: &Rc<RefCell<AppState>>) {
    APP.with(|a| *a.borrow_mut() = Some(app.clone()));
    let (w, a) = (ui.as_weak(), app.clone());
    ui.on_catalog_cmd(move |verb, arg| {
        let Some(ui) = w.upgrade() else { return };
        let arg = arg.to_string();
        let changed_list = STATE.with(|s| {
            let mut s = s.borrow_mut();
            match verb.as_str() {
                "search" => s.query = arg.clone(),
                "category" => s.category = arg.parse().unwrap_or(0),
                "select" => s.selected = Some(arg.clone()),
                "add" => {
                    let part = s.selected.clone().filter(|p| find(p).is_some_and(|i| i.step.is_some()));
                    if let Some(part) = part.filter(|p| !s.list.contains(p)) {
                        s.list.push(part);
                        return true;
                    }
                }
                "remove" => {
                    s.list.retain(|p| *p != arg);
                    return true;
                }
                "clear" => {
                    s.list.clear();
                    return true;
                }
                _ => {}
            }
            false
        });
        match verb.as_str() {
            "link" => return open_url(&arg),
            "folder" => return open_catalogs_folder(),
            "update" => return start_update(&ui),
            "cancel-update" => return cancel_update(&ui),
            "close" => {
                ui.set_catalog_open(false);
                QUEUE.lock().unwrap().clear();
                // Closed: drop the rows so nothing is kept or laid out.
                ui.set_catalog_rows(ModelRc::default());
                ui.set_catalog_list(ModelRc::default());
                return;
            }
            "select" => return select(&ui, &arg),
            "search" => {
                let (w, a) = (w.clone(), a.clone());
                SEARCH_TIMER.with(|t| {
                    t.start(slint::TimerMode::SingleShot, std::time::Duration::from_millis(150), move || {
                        if let Some(ui) = w.upgrade().filter(|ui| ui.get_catalog_open()) {
                            refresh(&ui, &a.borrow());
                        }
                    })
                });
                return;
            }
            "import" => {
                let library = in_library(&a.borrow());
                let items: Vec<BookItem> = STATE.with(|s| {
                    s.borrow().list.iter().filter(|p| !library.contains(*p)).filter_map(|p| find(p)).collect()
                });
                if items.is_empty() {
                    // Everything is in the library already: the list is done.
                    after_import(&ui, &a.borrow());
                } else {
                    crate::import::start_book_batch(&ui, items);
                }
                return;
            }
            _ => {}
        }
        if changed_list {
            STATE.with(|s| save_list(&s.borrow().list));
        }
        refresh(&ui, &a.borrow());
    });
}
