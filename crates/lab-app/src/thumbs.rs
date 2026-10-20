//! Library thumbnails (M4-PLAN POLA-61): PNGs rendered offscreen by the viewport into
//! `<user library>/thumbs/`. Built-in parts go there too: the install dir is read-only.

use std::path::PathBuf;

use lab_viewport::ViewportRequest;

use crate::app_state::AppState;

/// `<user library>/thumbs/<id with ':' and '/' → '_'>.png`.
pub fn path(id: &str) -> Option<PathBuf> {
    Some(lab_catalog::user_dir()?.join("thumbs").join(format!("{}.png", id.replace([':', '/', '\\'], "_"))))
}

/// The thumbnail of `id` if it exists.
pub fn image(id: &str) -> Option<slint::Image> {
    path(id).filter(|p| p.is_file()).and_then(|p| slint::Image::load_from_path(&p).ok())
}

/// Queues every catalog part that has no thumbnail file yet.
pub fn render_missing(app: &AppState) {
    for part in app.catalog.all() {
        if let Some(path) = path(&part.id).filter(|p| !p.is_file()) {
            app.send(ViewportRequest::RenderThumbnail { part: part.id.clone(), path });
        }
    }
}

/// Re-renders `id` (after an import or a wizard save).

pub fn render(app: &AppState, id: &str) {
    if let Some(path) = path(id) {
        app.send(ViewportRequest::RenderThumbnail { part: id.to_string(), path });
    }
}
