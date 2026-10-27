//! "Catalog…" (POLA-173, POLA-193): browse the user's catalog books, keep a pick list in
//! `%APPDATA%\LabSetup\catalog-list.json`, and download & import the list in one batch.

use std::cell::RefCell;
use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

use lab_catalog::book::{self, BookItem};
use lab_ui::{AppWindow, CatalogRow};
use slint::{ComponentHandle, Model, ModelRc, VecModel};

use crate::app_state::AppState;

const MAX_ROWS: usize = 500;

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
}

/// "All", then the distinct second-level categories, sorted.
fn categories() -> Vec<String> {
    let set: BTreeSet<&str> = book::books().iter().filter_map(|i| i.category.get(1)).map(String::as_str).collect();
    std::iter::once("All").chain(set).map(String::from).collect()
}

/// The book items matching `query` (case-insensitive substring of part, name or description)
/// in second-level category `category` (`None`: all).
#[cfg_attr(not(test), allow(dead_code))] // the app searches through `search_book`
pub fn filter<'a>(items: &'a [BookItem], query: &str, category: Option<&str>) -> Vec<&'a BookItem> {
    filter_indexed(items, &items.iter().map(haystack).collect::<Vec<_>>(), query, category)
}

/// Part, name and description, lower-cased and joined by newlines (a trimmed query cannot span them).
fn haystack(i: &BookItem) -> String {
    format!("{}\n{}\n{}", i.part, i.name, i.description).to_lowercase()
}

/// `filter` with precomputed `haystacks` (one per item).
fn filter_indexed<'a>(items: &'a [BookItem], haystacks: &[String], query: &str, category: Option<&str>) -> Vec<&'a BookItem> {
    let query = query.trim().to_lowercase();
    items
        .iter()
        .zip(haystacks)
        .filter(|(i, _)| category.is_none_or(|c| i.category.get(1).is_some_and(|ic| ic == c)))
        .filter(|(_, h)| query.is_empty() || h.contains(&query))
        .map(|(i, _)| i)
        .collect()
}

/// `filter` over the book, with its search index built once.
fn search_book(query: &str, category: Option<&str>) -> Vec<&'static BookItem> {
    static INDEX: std::sync::OnceLock<Vec<String>> = std::sync::OnceLock::new();
    let items = book::books();
    filter_indexed(items, INDEX.get_or_init(|| items.iter().map(haystack).collect()), query, category)
}

// shortcut: items are keyed by part number alone, so two vendors sharing one show the first; key by (vendor, part) if that happens.
fn find(part: &str) -> Option<&'static BookItem> {
    book::books().iter().find(|i| i.part == part)
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
fn fetch_thumbs(ui: slint::Weak<AppWindow>, items: &[&BookItem]) {
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
        let hits = search_book(&s.query, category);
        let selected = |i: &BookItem| s.selected.as_deref() == Some(i.part.as_str());
        let rows: Vec<CatalogRow> = hits.iter().take(MAX_ROWS).map(|i| row(i, &library, selected(i))).collect();
        ui.set_catalog_rows(ModelRc::new(VecModel::from(rows)));
        fetch_thumbs(ui.as_weak(), &hits[..hits.len().min(MAX_ROWS)]);
        ui.set_catalog_count(
            if hits.len() > MAX_ROWS { format!("showing {MAX_ROWS} of {}", hits.len()) } else { format!("{} parts", hits.len()) }
                .into(),
        );
        let list: Vec<CatalogRow> = s.list.iter().filter_map(|p| find(p)).map(|i| row(i, &library, selected(i))).collect();
        ui.set_catalog_list(ModelRc::new(VecModel::from(list)));
        detail(ui, &s);
    });
}

/// The detail pane for the selected part.
fn detail(ui: &AppWindow, s: &State) {
    let item = s.selected.as_deref().and_then(find);
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
    ui.set_catalog_categories(ModelRc::new(VecModel::from(categories().into_iter().map(Into::into).collect::<Vec<_>>())));
    ui.set_catalog_empty(book::books().is_empty());
    ui.set_catalog_category(STATE.with(|s| s.borrow().category) as i32);
    refresh(ui, app);
    ui.set_catalog_open(true);
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
                let items: Vec<&'static BookItem> = STATE.with(|s| {
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

#[cfg(test)]
mod tests {
    use super::*;

    fn item(part: &str, name: &str, description: &str, category: &str) -> BookItem {
        BookItem {
            vendor: "Example Optics".into(),
            part: part.into(),
            name: name.into(),
            description: description.into(),
            category: vec!["Optomechanics".into(), category.into()],
            url: format!("https://www.thorlabs.com/item/{part}"),
            cad_pdf: None,
            step: None,
            image: None,
        }
    }

    #[test]
    fn catalog_book_filter() {
        let items = [
            item("KM100", "Kinematic Mirror Mount", "for Ø1\" Optics", "Mirror Mounts"),
            item("SM1L05", "Lens Tube", "SM1 Lens Tube, 0.5\" Thread Depth", "Lens Tubes"),
            item("TR50/M", "Post", "Ø12.7 mm Optical Post", "Posts"),
        ];
        let parts = |v: Vec<&BookItem>| v.iter().map(|i| i.part.clone()).collect::<Vec<_>>();
        assert_eq!(parts(filter(&items, "km100", None)), ["KM100"]);
        assert_eq!(parts(filter(&items, " LENS ", None)), ["SM1L05"]);
        assert_eq!(parts(filter(&items, "optic", None)), ["KM100", "TR50/M"]);
        assert_eq!(parts(filter(&items, "", Some("Posts"))), ["TR50/M"]);
        assert_eq!(parts(filter(&items, "mount", Some("Posts"))), Vec::<String>::new());
        assert_eq!(filter(&items, "", None).len(), 3);
    }
}
