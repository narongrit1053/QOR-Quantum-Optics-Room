//! "Import from URL…" (POLA-170): a Thorlabs item page or a direct .step link → a local
//! STEP file. The network is reached only through Windows' own `curl.exe`, so no network
//! crate is linked into the app (DESIGN.md: offline).

use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};

const CURL: &str = r"C:\Windows\System32\curl.exe";
const NOT_SUPPORTED: &str = "Paste a Thorlabs item page (thorlabs.com/item/…) or a direct link to a .step file.";

#[derive(Debug, PartialEq)]
pub enum UrlSource {
    Thorlabs { part: String },
    Direct(String),
}

pub fn classify(url: &str) -> Result<UrlSource, String> {
    let url = url.trim();
    let lower = url.to_ascii_lowercase();
    let rest = lower.strip_prefix("https://").or_else(|| lower.strip_prefix("http://"));
    let bare = rest.unwrap_or(&lower);
    let bare = bare.strip_prefix("www.").unwrap_or(bare);
    if let Some(tail) = bare.strip_prefix("thorlabs.com/item/") {
        // Take the part from the original (case kept), at the same offset from the end.
        let tail = &url[url.len() - tail.len()..];
        let part = tail.split(['?', '#', '/']).next().unwrap_or("");
        let part = percent_decode(part);
        if !part.is_empty() && part.chars().all(|c| c.is_ascii_alphanumeric() || "-_/.+".contains(c)) {
            return Ok(UrlSource::Thorlabs { part });
        }
        return Err(NOT_SUPPORTED.into());
    }
    if rest.is_some() {
        let path = lower.split(['?', '#']).next().unwrap_or("");
        if path.ends_with(".step") || path.ends_with(".stp") {
            return Ok(UrlSource::Direct(url.to_string()));
        }
    }
    Err(NOT_SUPPORTED.into())
}

fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            if let Ok(v) = std::str::from_utf8(&b[i + 1..i + 3]).map_err(|_| ()).and_then(|h| u8::from_str_radix(h, 16).map_err(|_| ())) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// The STEP asset URL of `part` in a Thorlabs GraphQL `products` response.
pub fn step_url_from_graphql(json: &str, part: &str) -> Result<String, String> {
    let v: serde_json::Value = serde_json::from_str(json).map_err(|e| format!("Thorlabs answered with something unexpected ({e})."))?;
    let items = v["data"]["products"]["items"].as_array().cloned().unwrap_or_default();
    let item = items
        .iter()
        .find(|i| i["code"].as_str().is_some_and(|c| c.eq_ignore_ascii_case(part)))
        .ok_or_else(|| format!("Thorlabs has no item {part}."))?;
    let assets = item["assets"].as_array().cloned().unwrap_or_default();
    let is_step_name = |a: &serde_json::Value| {
        a["name"].as_str().is_some_and(|n| {
            let n = n.to_ascii_lowercase();
            n.ends_with(".step") || n.ends_with(".stp")
        })
    };
    assets
        .iter()
        .find(|a| a["group"].as_str() == Some("Step"))
        .or_else(|| assets.iter().find(|a| is_step_name(a)))
        .and_then(|a| a["url"].as_str())
        .map(str::to_string)
        .ok_or_else(|| format!("Thorlabs has no STEP file for {part}."))
}

fn curl(args: &[&str], stdin: Option<&str>) -> Result<Vec<u8>, String> {
    let mut cmd = Command::new(CURL);
    cmd.args(["-sSfL", "--max-time", "120", "-A", "Mozilla/5.0 LabSetup"]).args(args);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    cmd.stdin(if stdin.is_some() { Stdio::piped() } else { Stdio::null() })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = cmd.spawn().map_err(|e| format!("Could not start {CURL}: {e}"))?;
    if let (Some(text), Some(mut pipe)) = (stdin, child.stdin.take()) {
        let _ = pipe.write_all(text.as_bytes());
    }
    let out = child.wait_with_output().map_err(|e| e.to_string())?;
    if !out.status.success() {
        return Err(format!("Download failed: {}", String::from_utf8_lossy(&out.stderr).trim()));
    }
    Ok(out.stdout)
}

/// Resolves `url` to a STEP file and downloads it into `%LOCALAPPDATA%\LabSetup\downloads`.
pub fn download(url: &str) -> Result<PathBuf, String> {
    let (step_url, file_name): (String, String) = match classify(url)? {
        UrlSource::Thorlabs { part } => {
            let query = format!(
                r#"{{"query":"{{ products(storeId:\"Thorlabs-Website\", cultureName:\"en-US\", query:\"{part}\", first:5){{ items {{ code assets {{ name url group }} }} }} }}"}}"#
            );
            let json = curl(
                &["-X", "POST", "-H", "Content-Type: application/json", "--data-binary", "@-", "https://www.thorlabs.com/graphql"],
                Some(&query),
            )?;
            let step = step_url_from_graphql(&String::from_utf8_lossy(&json), &part)?;
            (step, format!("{}.step", part.replace('/', "_")))
        }
        UrlSource::Direct(u) => {
            let path = u.split(['?', '#']).next().unwrap_or("");
            let name = percent_decode(path.rsplit('/').next().unwrap_or("download.step"));
            let name: String = name.chars().filter(|c| !r#"\/:*?"<>|"#.contains(*c)).collect();
            (u, name)
        }
    };
    save_step(&file_name, &step_url)
}

/// Downloads catalog part `part`'s STEP file to `%LOCALAPPDATA%\LabSetup\downloads\<part>.step`
/// ('/' written as '_').
pub fn download_step(part: &str, step_url: &str) -> Result<PathBuf, String> {
    save_step(&format!("{}.step", part.replace('/', "_")), step_url)
}

/// Downloads `step_url` into the downloads folder as `file_name` and checks it is a STEP file.
fn save_step(file_name: &str, step_url: &str) -> Result<PathBuf, String> {
    let dir = std::env::var_os("LOCALAPPDATA")
        .map(|d| PathBuf::from(d).join("LabSetup").join("downloads"))
        .ok_or("%LOCALAPPDATA% is not set.")?;
    std::fs::create_dir_all(&dir).map_err(|e| format!("Could not create {}: {e}", dir.display()))?;
    let path = dir.join(file_name);
    let target = path.to_string_lossy().into_owned();
    curl(&["-o", &target, step_url], None)?;
    let head = std::fs::read(&path).unwrap_or_default();
    if head.len() < 1024 || !head.starts_with(b"ISO-10303-21") {
        let _ = std::fs::remove_file(&path);
        return Err(format!("{step_url} did not return a STEP file."));
    }
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn import_url_classify() {
        let t = |p: &str| Ok(UrlSource::Thorlabs { part: p.into() });
        assert_eq!(classify("https://www.thorlabs.com/item/PBS122"), t("PBS122"));
        assert_eq!(classify("thorlabs.com/item/KM100/?tab=x"), t("KM100"));
        assert_eq!(classify("http://www.thorlabs.com/item/SM1L%2F05"), t("SM1L/05"));
        let direct = "https://media.thorlabs.com/globalassets/items/p/pb/pbs/pbs122/24812-e0w.step?v=0116015956";
        assert_eq!(classify(direct), Ok(UrlSource::Direct(direct.into())));
        assert!(classify("https://example.com/page").is_err());
        assert!(classify("ftp://x/y.step").is_err());
    }

    #[test]
    fn import_url_step_url_from_graphql() {
        let json = r#"{"data":{"products":{"items":[{"code":"PBS122","assets":[
            {"name":"PBS122.pdf","url":"https://x/a.pdf","group":"Drawing"},
            {"name":"24812-E0W.step","url":"https://thin01mstroc282prod.dxcloud.episerver.net/globalassets/items/p/pb/pbs/pbs122/24812-e0w.step?v=0116015956","group":"Step"}]}]}}}"#;
        assert!(step_url_from_graphql(json, "pbs122").unwrap().ends_with("24812-e0w.step?v=0116015956"));
        assert!(step_url_from_graphql(json, "KM100").unwrap_err().contains("KM100"));
    }

    #[test]
    #[ignore = "needs the network; run by hand"]
    fn import_url_download_pbs122() {
        let path = download("https://www.thorlabs.com/item/PBS122").unwrap();
        assert!(path.ends_with("PBS122.step"));
        let direct = "https://media.thorlabs.com/globalassets/items/p/pb/pbs/pbs122/24812-e0w.step?v=0116015956";
        assert!(download(direct).unwrap().ends_with("24812-e0w.step"));
    }
}
