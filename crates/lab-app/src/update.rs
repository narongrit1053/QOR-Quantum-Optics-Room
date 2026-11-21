//! In-app updates and What's new. A daily (or manual) check asks GitHub for the newest release
//! that ships an `.msi`; installing downloads it and runs it, and the installer's MajorUpgrade
//! replaces this version. All network access goes through `curl.exe` (`import_url::curl`).

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;

use lab_ui::{AppWindow, ChangelogLine};
use slint::{ComponentHandle, ModelRc, VecModel};

use crate::app_state::AppState;

const RELEASES: &str = "https://api.github.com/repos/narongrit1053/QOR-Quantum-Optics-Room/releases?per_page=20";
const CHANGELOG: &str = include_str!("../../../CHANGELOG.md");
const CURRENT: &str = env!("CARGO_PKG_VERSION");
const DAY_S: i64 = 86_400;

#[derive(Clone, Debug, PartialEq)]
pub struct Release {
    pub version: String,
    pub notes: String,
    pub msi_url: String,
}

thread_local! {
    /// The newer release found by the last check.
    static PENDING: RefCell<Option<Release>> = const { RefCell::new(None) };
}

/// "v1.2.3" or "1.2.3" → (1, 2, 3); anything else (pre-release suffixes included) → None.
fn parse_version(s: &str) -> Option<(u32, u32, u32)> {
    let mut parts = s.trim().trim_start_matches('v').split('.').map(|p| p.parse::<u32>().ok());
    let v = (parts.next()??, parts.next()??, parts.next()??);
    parts.next().is_none().then_some(v)
}

/// The newest non-draft release in a GitHub `/releases` response that is newer than `current`
/// and has an `.msi` asset.
fn newest_release(json: &str, current: &str) -> Option<Release> {
    let current = parse_version(current)?;
    let releases: serde_json::Value = serde_json::from_str(json).ok()?;
    releases
        .as_array()?
        .iter()
        .filter(|r| !r["draft"].as_bool().unwrap_or(false))
        .filter_map(|r| {
            let tag = r["tag_name"].as_str()?;
            let version = parse_version(tag).filter(|v| *v > current)?;
            let msi = r["assets"].as_array()?.iter().find(|a| a["name"].as_str().is_some_and(|n| n.to_lowercase().ends_with(".msi")))?;
            let release = Release {
                version: tag.trim_start_matches('v').to_string(),
                notes: r["body"].as_str().unwrap_or("").to_string(),
                msi_url: msi["browser_download_url"].as_str()?.to_string(),
            };
            Some((version, release))
        })
        .max_by_key(|(v, _)| *v)
        .map(|(_, r)| r)
}

/// Markdown to sheet lines: `## x` is a heading, `- x` a bullet; emphasis and code marks dropped.
fn lines(markdown: &str) -> Vec<ChangelogLine> {
    markdown
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with("# "))
        .map(|l| {
            let (text, heading) = match l.strip_prefix("## ") {
                Some(h) => (h.to_string(), true),
                None => (l.strip_prefix("- ").map_or_else(|| l.to_string(), |b| format!("• {b}")), false),
            };
            ChangelogLine { text: text.replace("**", "").replace(['*', '`'], "").into(), heading }
        })
        .collect()
}

fn now() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64)
}

fn save(app: &AppState) {
    if let Err(error) = lab_file::save_settings(&app.settings) {
        eprintln!("settings not saved: {error}");
    }
}

fn show_sheet(ui: &AppWindow, title: &str, lines: Vec<ChangelogLine>, button: &str) {
    ui.set_changelog_title(title.into());
    ui.set_changelog_lines(ModelRc::new(VecModel::from(lines)));
    ui.set_changelog_button(button.into());
    ui.set_changelog_busy(false);
    ui.set_changelog_open(true);
}

fn text(s: &str) -> Vec<ChangelogLine> {
    vec![ChangelogLine { text: s.into(), heading: false }]
}

pub fn show_whats_new(ui: &AppWindow) {
    show_sheet(ui, &format!("What's new in LabSetup {CURRENT}"), lines(CHANGELOG), "");
}

/// After the first frame: What's new once per version, then the daily update check.
pub fn startup(ui: &AppWindow, app: &Rc<RefCell<AppState>>) {
    let (first_of_version, check) = {
        let mut a = app.borrow_mut();
        let first = a.settings.last_seen_version != CURRENT;
        a.settings.last_seen_version = CURRENT.to_string();
        if first {
            save(&a);
        }
        (first && a.recovery.is_empty(), a.settings.check_updates && now() - a.settings.last_update_check >= DAY_S)
    };
    if first_of_version {
        show_whats_new(ui);
    }
    if check {
        check_now(ui, app, false);
    }
}

/// Asks GitHub in the background. `manual`: show the result (up to date, failure) in the sheet.
pub fn check_now(ui: &AppWindow, app: &Rc<RefCell<AppState>>, manual: bool) {
    {
        let mut a = app.borrow_mut();
        a.settings.last_update_check = now();
        save(&a);
    }
    if manual {
        show_sheet(ui, "Checking for updates…", Vec::new(), "");
    }
    let w = ui.as_weak();
    std::thread::spawn(move || {
        let result = crate::import_url::curl(&["-H", "Accept: application/vnd.github+json", RELEASES], None)
            .map(|bytes| newest_release(&String::from_utf8_lossy(&bytes), CURRENT));
        let _ = w.upgrade_in_event_loop(move |ui| match result {
            Ok(Some(release)) => {
                ui.set_update_version(release.version.as_str().into());
                PENDING.with(|p| *p.borrow_mut() = Some(release));
                if manual {
                    show_update(&ui);
                }
            }
            Ok(None) if manual => show_sheet(&ui, &format!("LabSetup {CURRENT} is up to date"), Vec::new(), ""),
            Err(error) if manual => show_sheet(&ui, "Couldn't check for updates", text(&error), ""),
            Ok(None) => {}
            Err(error) => eprintln!("update check: {error}"),
        });
    });
}

pub fn show_update(ui: &AppWindow) {
    let Some(release) = PENDING.with(|p| p.borrow().clone()) else { return };
    let mut body = lines(&release.notes);
    body.insert(0, ChangelogLine { text: format!("You have {CURRENT}. Installing closes LabSetup; save your work first.").into(), heading: false });
    show_sheet(ui, &format!("LabSetup {} is available", release.version), body, "Download and install");
}

fn download_path(version: &str) -> Option<PathBuf> {
    std::env::var_os("LOCALAPPDATA").map(|d| PathBuf::from(d).join("LabSetup").join("downloads").join(format!("LabSetup-{version}.msi")))
}

/// Downloads the pending release's MSI, starts it, and asks the window to close (which offers
/// to save unsaved changes first).
pub fn install(ui: &AppWindow) {
    let Some(release) = PENDING.with(|p| p.borrow().clone()) else { return };
    let Some(path) = download_path(&release.version) else { return };
    ui.set_changelog_busy(true);
    let w = ui.as_weak();
    std::thread::spawn(move || {
        let tmp = path.with_extension("msi.part");
        let downloaded = path
            .parent()
            .map_or(Ok(()), std::fs::create_dir_all)
            .map_err(|e| e.to_string())
            .and_then(|_| crate::import_url::curl(&["-o", &tmp.to_string_lossy(), &release.msi_url], None))
            .and_then(|_| std::fs::rename(&tmp, &path).map_err(|e| e.to_string()));
        let _ = w.upgrade_in_event_loop(move |ui| {
            ui.set_changelog_busy(false);
            let started = downloaded.and_then(|_| {
                std::process::Command::new("msiexec.exe").arg("/i").arg(&path).spawn().map(drop).map_err(|e| e.to_string())
            });
            match started {
                Ok(()) => {
                    ui.set_changelog_open(false);
                    let _ = ui.window().dispatch_event_with_result(slint::platform::WindowEvent::CloseRequested);
                }
                Err(error) => show_sheet(&ui, "Couldn't download the update", text(&error), "Try again"),
            }
        });
    });
}

/// `app.auto_updates`: turns the daily check on or off.
pub fn toggle_auto(app: &Rc<RefCell<AppState>>) -> bool {
    let mut a = app.borrow_mut();
    a.settings.check_updates = !a.settings.check_updates;
    save(&a);
    a.settings.check_updates
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_parse_and_compare() {
        assert_eq!(parse_version("v0.0.6"), Some((0, 0, 6)));
        assert_eq!(parse_version("1.10.0"), Some((1, 10, 0)));
        assert!(parse_version("v0.1.0") > parse_version("v0.0.10"));
        assert_eq!(parse_version("v0.0.7-rc1"), None);
        assert_eq!(parse_version("v1.2"), None);
        assert_eq!(parse_version("pola-195"), None);
    }

    #[test]
    fn picks_newest_release_with_msi() {
        let json = r##"[
            {"tag_name":"v0.0.9","draft":true,"body":"","assets":[{"name":"LabSetup-0.0.9.msi","browser_download_url":"u9"}]},
            {"tag_name":"v0.0.8","draft":false,"body":"zip only","assets":[{"name":"LabSetup-v0.0.8-windows-x64.zip","browser_download_url":"z8"}]},
            {"tag_name":"v0.0.7","draft":false,"body":"notes 7","assets":[{"name":"LabSetup-0.0.7.MSI","browser_download_url":"u7"}]},
            {"tag_name":"v0.0.5","draft":false,"body":"","assets":[{"name":"LabSetup-0.0.5.msi","browser_download_url":"u5"}]}
        ]"##;
        let r = newest_release(json, "0.0.6").unwrap();
        assert_eq!((r.version.as_str(), r.msi_url.as_str()), ("0.0.7", "u7"));
        assert_eq!(newest_release(json, "0.0.7"), None);
        assert_eq!(newest_release("{\"message\":\"rate limited\"}", "0.0.6"), None);
        assert_eq!(newest_release("not json", "0.0.6"), None);
    }

    #[test]
    fn markdown_becomes_lines() {
        let l = lines("# Changelog\n\n## v0.0.6\n\n- **Faster** `catalog`, *really*\nPlain text.\r\n");
        let got: Vec<(String, bool)> = l.iter().map(|x| (x.text.to_string(), x.heading)).collect();
        assert_eq!(got, [("v0.0.6".into(), true), ("• Faster catalog, really".into(), false), ("Plain text.".into(), false)]);
        assert!(!lines(CHANGELOG).is_empty());
    }
}
