//! Library thumbnails (M4-PLAN POLA-61): PNGs rendered offscreen by the viewport into
//! `<user library>/thumbs/`. Built-in parts go there too: the install dir is read-only.

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::PathBuf;
use std::time::SystemTime;

use lab_viewport::ViewportRequest;

use crate::app_state::AppState;

/// `<user library>/thumbs/<id with ':' and '/' → '_'>.png`.
pub fn path(id: &str) -> Option<PathBuf> {
    Some(lab_catalog::user_dir()?.join("thumbs").join(format!("{}.png", id.replace([':', '/', '\\', '*', '?', '"', '<', '>', '|'], "_"))))
}

thread_local! {
    /// Decoded thumbnails by id, with the file's mtime: a re-render replaces the entry.
    static CACHE: RefCell<HashMap<String, (SystemTime, slint::Image)>> = RefCell::default();
}

/// The thumbnail of `id` if it exists.
pub fn image(id: &str) -> Option<slint::Image> {
    let path = path(id)?;
    let mtime = std::fs::metadata(&path).and_then(|m| m.modified()).ok()?;
    if let Some(img) = CACHE.with(|c| c.borrow().get(id).filter(|(t, _)| *t == mtime).map(|(_, i)| i.clone())) {
        return Some(img);
    }
    let (w, h, rgba) = lab_viewport::read_thumbnail(&path)?;
    let img = slint::Image::from_rgba8(slint::SharedPixelBuffer::clone_from_slice(&rgba, w, h));
    CACHE.with(|c| c.borrow_mut().insert(id.to_string(), (mtime, img.clone())));
    Some(img)
}

/// Queues every catalog part that has no thumbnail file yet.
pub fn render_missing(app: &AppState) {
    for part in app.catalog.all() {
        if let Some(path) = path(&part.id).filter(|p| !p.is_file()) {
            app.send(ViewportRequest::RenderThumbnail { part: part.id.clone(), path });
        }
    }
    for b in &app.settings.bundles {
        if path(&crate::library::bundle_key(&b.name)).is_some_and(|p| !p.is_file()) {
            render_bundle(app, b);
        }
    }
}

/// Renders bundle `b`'s whole set.
pub fn render_bundle(app: &AppState, b: &lab_file::Bundle) {
    let key = crate::library::bundle_key(&b.name);
    let parts = crate::library::bundle_set(&app.catalog, b);
    if let Some(path) = path(&key).filter(|_| !parts.is_empty()) {
        app.send(ViewportRequest::RenderSetThumbnail { key, parts, path });
    }
}

/// Removes `id`'s thumbnail file (a deleted or renamed bundle).
pub fn forget(id: &str) {
    if let Some(path) = path(id) {
        let _ = std::fs::remove_file(path);
    }
}

/// Re-renders `id` (after an import or a wizard save).
pub fn render(app: &AppState, id: &str) {
    if let Some(path) = path(id) {
        app.send(ViewportRequest::RenderThumbnail { part: id.to_string(), path });
    }
}
