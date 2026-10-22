//! Every app action the command palette can run (POLA-65). The handlers own the ids;
//! a new action anywhere must get a row here.

pub struct ActionInfo {
    pub id: &'static str,       // the string sent to `on_action`
    pub title: &'static str,    // "Export PNG…"
    pub group: &'static str,    // "File", "Edit", "View", "Tools", "Platform", "Room"
    pub keywords: &'static str, // extra search words, space separated
    pub shortcut: &'static str, // "Ctrl+S", "" if none
}

const fn a(id: &'static str, title: &'static str, group: &'static str, keywords: &'static str, shortcut: &'static str) -> ActionInfo {
    ActionInfo { id, title, group, keywords, shortcut }
}

pub const ACTIONS: &[ActionInfo] = &[
    a("file.new", "New Project", "File", "create blank", "Ctrl+N"),
    a("file.open", "Open…", "File", "load project", "Ctrl+O"),
    a("file.open_demo", "Open Demo Project", "File", "example sample", ""),
    a("file.save", "Save", "File", "write", "Ctrl+S"),
    a("file.save_as", "Save As…", "File", "write copy", "Ctrl+Shift+S"),
    a("file.save_folder", "Save as Folder…", "File", "write directory", ""),
    a("file.export_png", "Export PNG…", "File", "image screenshot picture render", ""),
    a("file.export_bom", "Export BOM (CSV)…", "File", "bill of materials parts list excel", ""),
    a("import.step", "Import Vendor Part…", "File", "step stp glb mesh cad", ""),
    a("edit.undo", "Undo", "Edit", "revert", "Ctrl+Z"),
    a("edit.redo", "Redo", "Edit", "repeat", "Ctrl+Y"),
    a("selection.delete", "Delete Selection", "Edit", "remove", "Del"),
    a("view.3d", "3D view", "View", "perspective orbit", "P"),
    a("view.top", "Top view", "View", "plan orthographic 2d", "T"),
    a("view.split", "Split view", "View", "side by side 3d top", ""),
    a("view.frame_all", "Frame All", "View", "zoom fit everything", "Shift+F"),
    a("view.frame_selection", "Frame Selection", "View", "zoom fit focus", "F"),
    a("view.isolate", "Isolate Plate", "View", "platform hide others", ""),
    a("view.show_all", "Show All", "View", "unisolate exit isolation", "Esc"),
    a("view.blueprint", "Toggle Blueprint", "View", "drawing lines style", ""),
    a("view.beams", "Show beams", "View", "laser beam path optics trace", ""),
    a("view.theme.dark", "Dark Theme", "View", "appearance colour color", ""),
    a("view.theme.light", "Light Theme", "View", "appearance colour color", ""),
    a("view.theme.contrast", "High Contrast Theme", "View", "appearance accessibility colour color", ""),
    a("palette.open", "Command Palette", "View", "search actions commands", "Ctrl+K"),
    a("tool.select", "Select Tool", "Tools", "pointer pick", ""),
    a("tool.move", "Move Tool", "Tools", "translate drag gizmo", ""),
    a("tool.rotate", "Rotate Tool", "Tools", "turn angle gizmo", ""),
    a("tool.measure", "Measure Tool", "Tools", "distance ruler dimension", "M"),
    a("tool.rotate_left", "Rotate −15°", "Tools", "turn nudge counterclockwise", "Q"),
    a("tool.rotate_right", "Rotate +15°", "Tools", "turn nudge clockwise", "E"),
    a("platform.zone.add_rect", "Add Rectangular Zone", "Platform", "keep out area", ""),
    a("platform.zone.add_circle", "Add Circular Zone", "Platform", "keep out area round", ""),
    a("platform.delete", "Delete Platform", "Platform", "remove table breadboard", ""),
    a("room.door.add", "Add Door", "Room", "opening wall", ""),
    a("room.window.add", "Add Window", "Room", "opening wall", ""),
];

/// Case-insensitive; ranks prefix of title > word prefix > subsequence over title+group+keywords.
/// Empty query returns all, in table order. Pure.
pub fn search(query: &str) -> Vec<&'static ActionInfo> {
    let q = query.trim().to_lowercase();
    let mut hits: Vec<(u8, &'static ActionInfo)> = ACTIONS
        .iter()
        .filter_map(|info| {
            if q.is_empty() {
                return Some((0, info));
            }
            let title = info.title.to_lowercase();
            let all = format!("{} {} {}", title, info.group.to_lowercase(), info.keywords.to_lowercase());
            if title.starts_with(&q) {
                Some((0, info))
            } else if all.split_whitespace().any(|w| w.starts_with(&q)) {
                Some((1, info))
            } else if is_subsequence(&q, &all) {
                Some((2, info))
            } else {
                None
            }
        })
        .collect();
    hits.sort_by_key(|(rank, _)| *rank); // stable: table order within a rank
    hits.into_iter().map(|(_, info)| info).collect()
}

fn is_subsequence(needle: &str, hay: &str) -> bool {
    let mut hay = hay.chars();
    needle.chars().filter(|c| !c.is_whitespace()).all(|c| hay.any(|h| h == c))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn search_ranks_prefix_first_and_empty_returns_all() {
        assert_eq!(search("").len(), ACTIONS.len());
        assert_eq!(search("top")[0].id, "view.top");
        assert_eq!(search("save")[0].id, "file.save");
        // Title prefix ("Select Tool") beats word prefix ("Delete Selection") beats subsequence.
        let hits: Vec<_> = search("sel").iter().map(|a| a.id).collect();
        assert_eq!(&hits[..2], &["tool.select", "selection.delete"]);
        assert!(search("zzzq").is_empty());
    }
}
