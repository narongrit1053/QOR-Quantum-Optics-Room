//! The I/O worker: one thread that runs save, load and autosave jobs in order, so the UI thread
//! never touches the disk. A panic inside a job becomes an `Internal` error result and the
//! worker carries on.

use std::panic::{self, AssertUnwindSafe};
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use lab_core::Document;

use crate::autosave::AutosaveSession;
use crate::project::{LoadedProject, ProjectLocation, SaveOptions, load_project, save_project};
use crate::{FileError, FileErrorKind};

#[derive(Debug)]
pub enum IoJob {
    Save { ticket: u64, doc: Box<Document>, location: ProjectLocation, options: SaveOptions },
    Load { ticket: u64, path: PathBuf },
    Autosave { ticket: u64, doc: Box<Document>, title: String, location: Option<ProjectLocation> },
    ClearAutosave,
    EndSession,
}

#[derive(Debug)]
pub enum IoResult {
    Saved { ticket: u64, location: ProjectLocation, result: Result<(), FileError> },
    Loaded { ticket: u64, result: Result<LoadedProject, FileError> },
    Autosaved { ticket: u64, result: Result<(), FileError> },
}

pub struct IoWorker {
    tx: Sender<IoJob>,
    handle: Option<JoinHandle<()>>,
}

impl IoWorker {
    /// Starts the thread `lab-io`. `on_result` runs on that thread; the app's closure hops to
    /// the UI thread with `slint::invoke_from_event_loop`.
    pub fn start(autosave: Option<AutosaveSession>, on_result: impl Fn(IoResult) + Send + 'static) -> Self {
        let (tx, rx) = mpsc::channel();
        let handle = std::thread::Builder::new()
            .name("lab-io".into())
            .spawn(move || run(rx, autosave, on_result))
            .expect("INVARIANT: the OS can start a thread");
        Self { tx, handle: Some(handle) }
    }

    /// Queues a job. Never blocks.
    pub fn submit(&self, job: IoJob) {
        let _ = self.tx.send(job);
    }

    /// Closes the queue and waits up to `timeout` for the queued jobs to finish. `false` if the
    /// worker was still busy (it is left to finish on its own).
    pub fn shutdown(self, timeout: Duration) -> bool {
        let Self { tx, handle } = self;
        drop(tx);
        let Some(handle) = handle else { return true };
        let deadline = Instant::now() + timeout;
        while !handle.is_finished() {
            if Instant::now() >= deadline {
                return false;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        let _ = handle.join();
        true
    }
}

/// What a panicking job should report.
enum Pending {
    Save { ticket: u64, location: ProjectLocation },
    Load { ticket: u64, path: PathBuf },
    Autosave { ticket: u64 },
    Silent,
}

impl Pending {
    fn of(job: &IoJob) -> Self {
        match job {
            IoJob::Save { ticket, location, .. } => Pending::Save { ticket: *ticket, location: location.clone() },
            IoJob::Load { ticket, path } => Pending::Load { ticket: *ticket, path: path.clone() },
            IoJob::Autosave { ticket, .. } => Pending::Autosave { ticket: *ticket },
            IoJob::ClearAutosave | IoJob::EndSession => Pending::Silent,
        }
    }

    fn panicked(self, message: String) -> Option<IoResult> {
        let error = |path: PathBuf| {
            FileError::new(path, FileErrorKind::Internal(format!("a file operation panicked: {message}")))
        };
        match self {
            Pending::Save { ticket, location } => {
                let result = Err(error(location.path.clone()));
                Some(IoResult::Saved { ticket, location, result })
            }
            Pending::Load { ticket, path } => Some(IoResult::Loaded { ticket, result: Err(error(path)) }),
            Pending::Autosave { ticket } => Some(IoResult::Autosaved { ticket, result: Err(error(PathBuf::new())) }),
            Pending::Silent => None,
        }
    }
}

fn run(rx: Receiver<IoJob>, mut session: Option<AutosaveSession>, on_result: impl Fn(IoResult)) {
    while let Ok(job) = rx.recv() {
        let pending = Pending::of(&job);
        let outcome = panic::catch_unwind(AssertUnwindSafe(|| execute(job, &mut session)));
        let result = match outcome {
            Ok(result) => result,
            Err(payload) => pending.panicked(panic_message(payload.as_ref())),
        };
        if let Some(result) = result {
            on_result(result);
        }
    }
}

fn panic_message(payload: &(dyn std::any::Any + Send)) -> String {
    payload
        .downcast_ref::<&str>()
        .map(|s| (*s).to_string())
        .or_else(|| payload.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "unknown panic".into())
}

fn execute(job: IoJob, session: &mut Option<AutosaveSession>) -> Option<IoResult> {
    match job {
        IoJob::Save { ticket, doc, location, options } => {
            let result = save_project(&doc, &location, &options);
            Some(IoResult::Saved { ticket, location, result })
        }
        IoJob::Load { ticket, path } => {
            #[cfg(test)]
            if path.as_os_str() == "<panic>" {
                panic!("test panic in a load job");
            }
            Some(IoResult::Loaded { ticket, result: load_project(&path) })
        }
        IoJob::Autosave { ticket, doc, title, location } => {
            let result = match session {
                Some(session) => session.write(&doc, &title, location.as_ref()),
                None => Err(FileError::internal(PathBuf::new(), "autosave unavailable")),
            };
            Some(IoResult::Autosaved { ticket, result })
        }
        IoJob::ClearAutosave => {
            if let Some(session) = session {
                let _ = session.clear();
            }
            None
        }
        IoJob::EndSession => {
            if let Some(session) = session.take() {
                let _ = session.end();
            }
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc;

    use super::*;

    /// W1 (panic half): a panicking job reports Internal and the next job still runs.
    #[test]
    fn panicking_job_reports_internal_and_worker_continues() {
        let (tx, rx) = mpsc::channel();
        let tx = std::sync::Mutex::new(tx);
        let worker = IoWorker::start(None, move |r| {
            let _ = tx.lock().unwrap().send(r);
        });
        worker.submit(IoJob::Load { ticket: 1, path: PathBuf::from("<panic>") });
        worker.submit(IoJob::Load { ticket: 2, path: PathBuf::from("does-not-exist.labsetup") });
        let timeout = Duration::from_secs(10);
        match rx.recv_timeout(timeout).unwrap() {
            IoResult::Loaded { ticket: 1, result: Err(e) } => assert!(matches!(e.kind, FileErrorKind::Internal(_))),
            other => panic!("unexpected {other:?}"),
        }
        match rx.recv_timeout(timeout).unwrap() {
            IoResult::Loaded { ticket: 2, result: Err(e) } => assert!(matches!(e.kind, FileErrorKind::NotFound)),
            other => panic!("unexpected {other:?}"),
        }
        assert!(worker.shutdown(Duration::from_secs(5)));
    }
}
