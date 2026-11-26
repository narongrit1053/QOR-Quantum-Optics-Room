//! Registers `labsetup-cli mcp` in the user-level MCP config of each coding-agent CLI found on
//! this PC, so any agent can drive LabSetup without being told where it lives. Only the
//! `labsetup` entry is ever added, changed or removed; a file is backed up once before its first
//! change, and a file that doesn't parse is left alone.

use std::fmt;
use std::path::{Path, PathBuf};

use serde_json::{json, Map, Value};

const NAME: &str = "labsetup";
const TOML_HEADER: &str = "[mcp_servers.labsetup]";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    /// `{"mcpServers": {"labsetup": {"command": exe, "args": ["mcp"]}}}`
    McpServers,
    /// `mcpServers` with `"type": "stdio"`
    Cursor,
    /// `mcpServers` with `"type": "local"` and `"tools": ["*"]`
    Copilot,
    /// `{"servers": {"labsetup": {"type": "stdio", "command": exe, "args": ["mcp"]}}}`
    VsCode,
    /// `{"mcp": {"servers": {"labsetup": {"type": "local", "command": [exe, "mcp"]}}}}` (opencode 2.x)
    Opencode,
    /// `[mcp_servers.labsetup]` with `command` and `args`
    CodexToml,
    /// `mcp_servers:` mapping in Hermes Agent's `config.yaml`
    HermesYaml,
}

pub struct Agent {
    pub name: &'static str,
    /// Exists when the agent is installed (its config file may not exist yet).
    pub detect: PathBuf,
    pub config: PathBuf,
    pub format: Format,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    Added,
    Updated { was: String },
    AlreadySet,
    Removed,
    NotSet,
    NotFound,
    Failed(String),
}

impl fmt::Display for Outcome {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Self::Added => f.write_str("added"),
            Self::Updated { was } => write!(f, "updated (was {was})"),
            Self::AlreadySet => f.write_str("already set"),
            Self::Removed => f.write_str("removed"),
            Self::NotSet => f.write_str("not set"),
            Self::NotFound => f.write_str("not found"),
            Self::Failed(e) => write!(f, "failed: {e}"),
        }
    }
}

/// `%USERPROFILE%`, `%APPDATA%` and `%LOCALAPPDATA%`.
pub fn home_dirs() -> (PathBuf, PathBuf, PathBuf) {
    let get = |k: &str| std::env::var_os(k).map(PathBuf::from).unwrap_or_default();
    (get("USERPROFILE"), get("APPDATA"), get("LOCALAPPDATA"))
}

/// The agent CLIs and apps LabSetup registers with (paths checked against each tool's docs or
/// its own files, 2026-11).
pub fn known_agents(home: &Path, appdata: &Path, local: &Path) -> Vec<Agent> {
    let a = |name, detect: PathBuf, config: PathBuf, format| Agent { name, detect, config, format };
    let gemini = home.join(".gemini");
    vec![
        a("Claude Code", home.join(".claude"), home.join(".claude.json"), Format::McpServers),
        a("Claude desktop app", appdata.join("Claude"), appdata.join("Claude").join("claude_desktop_config.json"), Format::McpServers),
        a("Codex CLI", home.join(".codex"), home.join(".codex").join("config.toml"), Format::CodexToml),
        a("Gemini CLI", gemini.clone(), gemini.join("settings.json"), Format::McpServers),
        a("Antigravity", gemini.join("antigravity"), gemini.join("config").join("mcp_config.json"), Format::McpServers),
        a("Cursor", home.join(".cursor"), home.join(".cursor").join("mcp.json"), Format::Cursor),
        a("GitHub Copilot CLI", home.join(".copilot"), home.join(".copilot").join("mcp-config.json"), Format::Copilot),
        a("Windsurf", home.join(".codeium").join("windsurf"), home.join(".codeium").join("windsurf").join("mcp_config.json"), Format::McpServers),
        a("Devin CLI", home.join(".config").join("devin"), home.join(".config").join("devin").join("mcp_config.json"), Format::McpServers),
        a("VS Code", appdata.join("Code").join("User"), appdata.join("Code").join("User").join("mcp.json"), Format::VsCode),
        a("Trae", appdata.join("Trae").join("User"), appdata.join("Trae").join("User").join("mcp.json"), Format::McpServers),
        a("opencode", home.join(".config").join("opencode"), home.join(".config").join("opencode").join("opencode.json"), Format::Opencode),
        a("Hermes Agent", local.join("hermes"), local.join("hermes").join("config.yaml"), Format::HermesYaml),
    ]
}

/// Where the servers map sits in a JSON config.
fn section(format: Format) -> &'static [&'static str] {
    match format {
        Format::VsCode => &["servers"],
        Format::Opencode => &["mcp", "servers"],
        _ => &["mcpServers"],
    }
}

fn entry(format: Format, exe: &str) -> Value {
    match format {
        Format::VsCode | Format::Cursor => json!({"type": "stdio", "command": exe, "args": ["mcp"]}),
        Format::Copilot => json!({"type": "local", "command": exe, "args": ["mcp"], "tools": ["*"]}),
        Format::Opencode => json!({"type": "local", "command": [exe, "mcp"]}),
        _ => json!({"command": exe, "args": ["mcp"]}),
    }
}

fn parse_json(text: &str) -> Result<Map<String, Value>, String> {
    if text.trim().is_empty() {
        return Ok(Map::new());
    }
    match serde_json::from_str(text) {
        Ok(Value::Object(m)) => Ok(m),
        Ok(_) => Err("the file is not a JSON object, left unchanged".into()),
        Err(e) => Err(format!("can't read it ({e}), left unchanged")),
    }
}

fn parse_toml(text: &str) -> Result<toml::Table, String> {
    text.parse().map_err(|e| format!("can't read it ({e}), left unchanged"))
}

fn check(text: &str, format: Format) -> Result<(), String> {
    match format {
        Format::CodexToml => parse_toml(text).map(drop),
        // shortcut: YAML isn't parsed (no YAML crate); edit_yaml refuses forms it doesn't know, add a YAML crate if users hit that
        Format::HermesYaml => Ok(()),
        _ => parse_json(text).map(drop),
    }
}

/// The registered command path, if any.
pub fn current(text: &str, format: Format) -> Option<String> {
    let text = strip_bom(text);
    match format {
        Format::CodexToml => {
            let t = parse_toml(text).ok()?;
            return t.get("mcp_servers")?.get(NAME)?.get("command")?.as_str().map(String::from);
        }
        Format::HermesYaml => return yaml_current(text),
        _ => {}
    }
    let m = Value::Object(parse_json(text).ok()?);
    let c = section(format).iter().try_fold(&m, |v, k| v.get(k))?.get(NAME)?.get("command")?;
    c.as_str().or_else(|| c.get(0).and_then(Value::as_str)).map(String::from)
}

/// The servers map inside a JSON config, created when missing.
fn servers_mut(m: &mut Map<String, Value>, format: Format) -> Result<&mut Map<String, Value>, String> {
    let mut cur = m;
    for k in section(format) {
        cur = cur
            .entry(*k)
            .or_insert_with(|| json!({}))
            .as_object_mut()
            .ok_or_else(|| format!("its \"{k}\" is not an object, left unchanged"))?;
    }
    Ok(cur)
}

fn indent(l: &str) -> usize {
    l.len() - l.trim_start().len()
}

/// Lines of the top-level `mcp_servers:` mapping: its key line up to the next top-level key.
fn yaml_section(lines: &[&str]) -> Option<(usize, usize)> {
    let s = lines.iter().position(|l| l.starts_with("mcp_servers:"))?;
    let e = lines[s + 1..]
        .iter()
        .position(|l| !l.trim().is_empty() && indent(l) == 0 && !l.starts_with('#'))
        .map_or(lines.len(), |i| s + 1 + i);
    Some((s, e))
}

/// Indent of the section's server keys, if it has any.
fn yaml_child_indent(lines: &[&str], s: usize, e: usize) -> Option<usize> {
    lines[s + 1..e].iter().find(|l| !l.trim().is_empty() && !l.trim_start().starts_with('#')).map(|l| indent(l))
}

/// Our `labsetup:` entry inside the section (start, end, indent), without trailing blank lines.
fn yaml_child(lines: &[&str], s: usize, e: usize) -> Option<(usize, usize, usize)> {
    let ci = yaml_child_indent(lines, s, e)?;
    let c = (s + 1..e).find(|&i| indent(lines[i]) == ci && lines[i].trim() == "labsetup:")?;
    let mut end = (c + 1..e).find(|&i| !lines[i].trim().is_empty() && indent(lines[i]) <= ci).unwrap_or(e);
    while end > c + 1 && lines[end - 1].trim().is_empty() {
        end -= 1;
    }
    Some((c, end, ci))
}

fn yaml_current(text: &str) -> Option<String> {
    let lines: Vec<&str> = text.lines().collect();
    let (s, e) = yaml_section(&lines)?;
    let (c, end, _) = yaml_child(&lines, s, e)?;
    let v = lines[c + 1..end].iter().find_map(|l| l.trim().strip_prefix("command:"))?.trim();
    Some(if let Some(x) = v.strip_prefix('\'').and_then(|x| x.strip_suffix('\'')) {
        x.replace("''", "'")
    } else if let Some(x) = v.strip_prefix('"').and_then(|x| x.strip_suffix('"')) {
        x.replace("\\\\", "\\")
    } else {
        v.to_owned()
    })
}

fn yaml_entry(exe: &str, ci: usize) -> Vec<String> {
    let (pad, inner) = (" ".repeat(ci), " ".repeat(ci * 2));
    vec![
        format!("{pad}labsetup:"),
        format!("{inner}command: '{}'", exe.replace('\'', "''")),
        format!("{inner}args: [mcp]"),
    ]
}

/// A text edit of the `mcp_servers:` mapping that keeps the rest of the file as it is; `None`
/// removes our entry.
fn edit_yaml(text: &str, exe: Option<&str>) -> Result<String, String> {
    let lines: Vec<&str> = text.lines().collect();
    let Some((s, e)) = yaml_section(&lines) else {
        return Ok(match exe {
            None => text.to_owned(),
            Some(x) => {
                let sep = if text.is_empty() || text.ends_with('\n') { "" } else { "\n" };
                format!("{text}{sep}mcp_servers:\n{}\n", yaml_entry(x, 2).join("\n"))
            }
        });
    };
    let head = lines[s]["mcp_servers:".len()..].trim();
    if !matches!(head, "" | "{}" | "~" | "null") && !head.starts_with('#') {
        return Err("its mcp_servers is written in a form LabSetup can't edit; add the entry by hand".into());
    }
    let mut v: Vec<String> = lines.iter().map(|l| l.to_string()).collect();
    match (yaml_child(&lines, s, e), exe) {
        (Some((c, end, ci)), Some(x)) => drop(v.splice(c..end, yaml_entry(x, ci))),
        (Some((c, end, _)), None) => drop(v.drain(c..end)),
        (None, Some(x)) => {
            v[s] = "mcp_servers:".into();
            let ci = yaml_child_indent(&lines, s, e).unwrap_or(2);
            drop(v.splice(s + 1..s + 1, yaml_entry(x, ci)));
        }
        (None, None) => {}
    }
    Ok(v.join("\n") + "\n")
}

// shortcut: key order not kept (serde_json without preserve_order); switch to an order-keeping writer if users complain
fn json_text(m: Map<String, Value>) -> String {
    serde_json::to_string_pretty(&Value::Object(m)).expect("a JSON map serializes") + "\n"
}

/// Line range of the `[mcp_servers.labsetup]` table and its subtables, up to the next other table
/// header; blank and comment lines just above that header stay with it.
fn toml_block(lines: &[&str]) -> Option<(usize, usize)> {
    let start = lines.iter().position(|l| l.trim() == TOML_HEADER)?;
    let ours = |l: &str| l.trim_start().starts_with("[mcp_servers.labsetup.");
    let mut end = lines[start + 1..]
        .iter()
        .position(|l| l.trim_start().starts_with('[') && !ours(l))
        .map_or(lines.len(), |i| start + 1 + i);
    while end > start + 1 && (lines[end - 1].trim().is_empty() || lines[end - 1].trim_start().starts_with('#')) {
        end -= 1;
    }
    Some((start, end))
}

fn strip_bom(text: &str) -> &str {
    text.strip_prefix('\u{feff}').unwrap_or(text)
}

/// A text edit, so the user's comments and layout survive; the result is re-parsed as a check.
fn edit_toml(text: &str, block: Option<&str>) -> Result<String, String> {
    let lines: Vec<&str> = text.lines().collect();
    let out = match (toml_block(&lines), block) {
        (Some((s, e)), new) => {
            let mut v: Vec<String> = lines[..s].iter().map(|l| l.to_string()).collect();
            if let Some(b) = new {
                v.extend(b.lines().map(String::from));
            }
            v.extend(lines[e..].iter().map(|l| l.to_string()));
            v.join("\n") + "\n"
        }
        (None, Some(b)) => {
            let sep = if text.is_empty() { "" } else if text.ends_with('\n') { "\n" } else { "\n\n" };
            format!("{text}{sep}{b}")
        }
        (None, None) => text.to_owned(),
    };
    if parse_toml(&out).is_err() {
        return Err("its mcp_servers table is written in a form LabSetup can't edit; add the entry by hand".into());
    }
    Ok(out)
}

/// Adds or updates the `labsetup` entry. Empty text means a new file.
pub fn merge(text: &str, format: Format, exe: &Path) -> Result<(String, Outcome), String> {
    let text = strip_bom(text);
    check(text, format)?;
    let exe = exe.to_string_lossy();
    let outcome = match current(text, format) {
        Some(c) if c == exe => return Ok((text.to_owned(), Outcome::AlreadySet)),
        Some(c) => Outcome::Updated { was: c },
        None => Outcome::Added,
    };
    let new = match format {
        Format::CodexToml => {
            let block = format!("{TOML_HEADER}\ncommand = {}\nargs = [\"mcp\"]\n", toml::Value::String(exe.to_string()));
            edit_toml(text, Some(&block))?
        }
        Format::HermesYaml => edit_yaml(text, Some(&exe))?,
        _ => {
            let mut m = parse_json(text)?;
            servers_mut(&mut m, format)?.insert(NAME.into(), entry(format, &exe));
            json_text(m)
        }
    };
    Ok((new, outcome))
}

/// Removes the `labsetup` entry only.
pub fn remove(text: &str, format: Format) -> Result<(String, Outcome), String> {
    let text = strip_bom(text);
    check(text, format)?;
    if current(text, format).is_none() {
        return Ok((text.to_owned(), Outcome::NotSet));
    }
    let new = match format {
        Format::CodexToml => edit_toml(text, None)?,
        Format::HermesYaml => edit_yaml(text, None)?,
        _ => {
            let mut m = parse_json(text)?;
            servers_mut(&mut m, format)?.remove(NAME);
            json_text(m)
        }
    };
    Ok((new, Outcome::Removed))
}

/// `None` when the agent isn't installed; an empty string for a config file not written yet.
fn read(agent: &Agent) -> Result<Option<String>, String> {
    if !agent.config.exists() && !agent.detect.exists() {
        return Ok(None);
    }
    match std::fs::read_to_string(&agent.config) {
        Ok(t) => Ok(Some(t)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Some(String::new())),
        Err(e) => Err(format!("{}: {e}", agent.config.display())),
    }
}

fn write(agent: &Agent, old: &str, new: &str) -> Result<(), String> {
    let bak = PathBuf::from(format!("{}.labsetup-bak", agent.config.display()));
    if agent.config.exists() && !bak.exists() {
        std::fs::write(&bak, old).map_err(|e| format!("{}: {e}", bak.display()))?;
    }
    if let Some(dir) = agent.config.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    lab_file::atomic_write(&agent.config, new.as_bytes()).map_err(|e| format!("{}: {e}", agent.config.display()))
}

fn apply(agents: &[Agent], edit: impl Fn(&str, Format) -> Result<(String, Outcome), String>) -> Vec<(&'static str, Outcome)> {
    agents
        .iter()
        .map(|a| {
            let outcome = match read(a) {
                Ok(None) => Outcome::NotFound,
                Err(e) => Outcome::Failed(e),
                Ok(Some(old)) => match edit(&old, a.format) {
                    Err(e) => Outcome::Failed(e),
                    Ok((_, o @ (Outcome::AlreadySet | Outcome::NotSet))) => o,
                    Ok((new, o)) => write(a, &old, &new).map_or_else(Outcome::Failed, |()| o),
                },
            };
            (a.name, outcome)
        })
        .collect()
}

pub fn install(exe: &Path, agents: &[Agent]) -> Vec<(&'static str, Outcome)> {
    apply(agents, |text, f| merge(text, f, exe))
}

pub fn uninstall(agents: &[Agent]) -> Vec<(&'static str, Outcome)> {
    apply(agents, remove)
}

/// Per agent: not found / not set / set / stale: <path> / failed: …
pub fn status(exe: &Path, agents: &[Agent]) -> Vec<(&'static str, String)> {
    let exe = exe.to_string_lossy();
    agents
        .iter()
        .map(|a| {
            let s = match read(a) {
                Ok(None) => "not found".into(),
                Err(e) => format!("failed: {e}"),
                Ok(Some(t)) => match current(&t, a.format) {
                    None => "not set".into(),
                    Some(c) if c == exe => "set".into(),
                    Some(c) => format!("stale: {c}"),
                },
            };
            (a.name, s)
        })
        .collect()
}

/// The Settings result text: one line per agent found, then the switch hint when it is off.
pub fn summary(results: &[(&str, Outcome)], allow_agents: bool) -> String {
    let mut lines: Vec<String> =
        results.iter().filter(|(_, o)| *o != Outcome::NotFound).map(|(n, o)| format!("{n}: {o}")).collect();
    if lines.is_empty() {
        lines.push("No coding-agent CLI found on this PC. Run 'labsetup-cli agents install' to see a config snippet for others.".into());
    }
    if !allow_agents {
        lines.push("Agents can connect once 'Allow AI agents' is On.".into());
    }
    lines.join("\n")
}

/// The `mcpServers` block for agents LabSetup doesn't know.
pub fn snippet(exe: &Path) -> String {
    let mut m = Map::new();
    m.insert("mcpServers".into(), json!({ NAME: entry(Format::McpServers, &exe.to_string_lossy()) }));
    json_text(m)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{json, Value};
    use std::path::{Path, PathBuf};

    const EXE: &str = r"C:\Program Files\LabSetup\labsetup-cli.exe";
    const OTHER: &str = r"D:\old\labsetup-cli.exe";

    fn j(s: &str) -> Value {
        serde_json::from_str(s).unwrap()
    }

    fn temp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("labsetup-agents-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn merge_into_empty_creates_entry() {
        let (text, out) = merge("", Format::McpServers, Path::new(EXE)).unwrap();
        assert_eq!(out, Outcome::Added);
        assert_eq!(j(&text)["mcpServers"]["labsetup"], json!({"command": EXE, "args": ["mcp"]}));
    }

    #[test]
    fn merge_keeps_other_servers_and_settings() {
        let before = r#"{"theme":"dark","mcpServers":{"other":{"command":"x","args":["y"]}}}"#;
        let (text, out) = merge(before, Format::McpServers, Path::new(EXE)).unwrap();
        assert_eq!(out, Outcome::Added);
        let v = j(&text);
        assert_eq!(v["theme"], "dark");
        assert_eq!(v["mcpServers"]["other"], json!({"command":"x","args":["y"]}));
        assert_eq!(v["mcpServers"]["labsetup"]["command"], EXE);
    }

    #[test]
    fn merge_same_path_is_already_set_new_path_updates() {
        let (text, _) = merge("", Format::McpServers, Path::new(OTHER)).unwrap();
        let (_, again) = merge(&text, Format::McpServers, Path::new(OTHER)).unwrap();
        assert_eq!(again, Outcome::AlreadySet);
        let (moved, out) = merge(&text, Format::McpServers, Path::new(EXE)).unwrap();
        assert_eq!(out, Outcome::Updated { was: OTHER.into() });
        assert_eq!(current(&moved, Format::McpServers).as_deref(), Some(EXE));
    }

    #[test]
    fn jsonc_or_broken_file_is_refused() {
        assert!(merge("{ // mine\n }", Format::VsCode, Path::new(EXE)).is_err());
        assert!(merge("[1,2]", Format::McpServers, Path::new(EXE)).is_err());
        let home = temp("jsonc");
        let file = home.join("mcp.json");
        std::fs::write(&file, "{ // mine\n }").unwrap();
        let agents = [Agent { name: "VS Code", detect: file.parent().unwrap().to_path_buf(), config: file.clone(), format: Format::VsCode }];
        let res = install(Path::new(EXE), &agents);
        assert!(matches!(res[0].1, Outcome::Failed(_)));
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "{ // mine\n }");
        assert!(!home.join("mcp.json.labsetup-bak").exists());
    }

    #[test]
    fn vscode_cursor_and_copilot_shapes() {
        let (t, _) = merge("", Format::VsCode, Path::new(EXE)).unwrap();
        assert_eq!(j(&t)["servers"]["labsetup"], json!({"type":"stdio","command":EXE,"args":["mcp"]}));
        let (t, _) = merge("", Format::Cursor, Path::new(EXE)).unwrap();
        assert_eq!(j(&t)["mcpServers"]["labsetup"], json!({"type":"stdio","command":EXE,"args":["mcp"]}));
        let (t, _) = merge("", Format::Copilot, Path::new(EXE)).unwrap();
        assert_eq!(j(&t)["mcpServers"]["labsetup"], json!({"type":"local","command":EXE,"args":["mcp"],"tools":["*"]}));
        assert_eq!(current(&t, Format::Copilot).as_deref(), Some(EXE));
    }

    #[test]
    fn codex_toml_append_replace_remove_keep_comments() {
        let before = "# mine\n[model]\nx = 1\n";
        let (t, out) = merge(before, Format::CodexToml, Path::new(OTHER)).unwrap();
        assert_eq!(out, Outcome::Added);
        assert!(t.starts_with(before));
        let v: toml::Table = t.parse().unwrap();
        assert_eq!(v["mcp_servers"]["labsetup"]["command"].as_str(), Some(OTHER));
        assert_eq!(v["mcp_servers"]["labsetup"]["args"].as_array().unwrap()[0].as_str(), Some("mcp"));
        let with_after = format!("{t}\n[other]\ny = 2\n");
        let (t2, out) = merge(&with_after, Format::CodexToml, Path::new(EXE)).unwrap();
        assert_eq!(out, Outcome::Updated { was: OTHER.into() });
        assert!(t2.contains("[other]\ny = 2"));
        assert_eq!(current(&t2, Format::CodexToml).as_deref(), Some(EXE));
        let (t3, out) = remove(&t2, Format::CodexToml).unwrap();
        assert_eq!(out, Outcome::Removed);
        assert!(!t3.contains("labsetup"));
        assert!(t3.starts_with("# mine\n[model]\nx = 1\n"));
        assert!(t3.contains("[other]\ny = 2"));
    }

    #[test]
    fn codex_keeps_comment_above_next_table_and_drops_our_subtables() {
        let before = "[mcp_servers.labsetup]\ncommand = 'old'\nargs = ['mcp']\n\n[mcp_servers.labsetup.env]\nX = '1'\n\n# GitHub server\n[mcp_servers.github]\ncommand = 'gh'\n";
        let (t, _) = merge(before, Format::CodexToml, Path::new(EXE)).unwrap();
        assert!(t.contains("# GitHub server\n[mcp_servers.github]"), "{t}");
        let (t, out) = remove(before, Format::CodexToml).unwrap();
        assert_eq!(out, Outcome::Removed);
        assert!(!t.contains("labsetup"), "{t}");
        assert!(t.contains("# GitHub server\n[mcp_servers.github]"), "{t}");
        let v: toml::Table = t.parse().unwrap();
        assert!(v["mcp_servers"].get("labsetup").is_none());
    }

    #[test]
    fn opencode_uses_the_nested_servers_shape_it_writes_itself() {
        let before = r#"{"$schema":"https://opencode.ai/config.json","provider":{"x":1}}"#;
        let (t, out) = merge(before, Format::Opencode, Path::new(EXE)).unwrap();
        assert_eq!(out, Outcome::Added);
        assert_eq!(j(&t)["mcp"]["servers"]["labsetup"], json!({"type":"local","command":[EXE,"mcp"]}));
        assert_eq!(j(&t)["provider"]["x"], 1);
        assert_eq!(current(&t, Format::Opencode).as_deref(), Some(EXE));
        let (t, out) = remove(&t, Format::Opencode).unwrap();
        assert_eq!(out, Outcome::Removed);
        assert!(j(&t)["mcp"]["servers"].get("labsetup").is_none());
    }

    #[test]
    fn hermes_yaml_append_insert_update_remove() {
        let exe = r"C:\Program Files\Lab's\labsetup-cli.exe";
        let before = "model:\n  default: x\ntimezone: Asia/Bangkok\n";
        let (t, out) = merge(before, Format::HermesYaml, Path::new(exe)).unwrap();
        assert_eq!(out, Outcome::Added);
        assert!(t.starts_with(before), "{t}");
        assert!(t.ends_with("mcp_servers:\n  labsetup:\n    command: 'C:\\Program Files\\Lab''s\\labsetup-cli.exe'\n    args: [mcp]\n"), "{t}");
        assert_eq!(current(&t, Format::HermesYaml).as_deref(), Some(exe));

        // Existing section with its own indent and another server: insert as a sibling, keep the rest.
        let with_other = "a: 1\nmcp_servers:\n    fusion:\n        command: python\n        args: [x.py]\nb: 2\n";
        let (t, out) = merge(with_other, Format::HermesYaml, Path::new(OTHER)).unwrap();
        assert_eq!(out, Outcome::Added);
        assert!(t.contains("mcp_servers:\n    labsetup:\n        command: 'D:\\old\\labsetup-cli.exe'\n        args: [mcp]\n    fusion:\n        command: python\n"), "{t}");
        assert!(t.ends_with("b: 2\n"), "{t}");

        let (t2, out) = merge(&t, Format::HermesYaml, Path::new(EXE)).unwrap();
        assert_eq!(out, Outcome::Updated { was: OTHER.into() });
        assert_eq!(current(&t2, Format::HermesYaml).as_deref(), Some(EXE));
        assert_eq!(t2.matches("labsetup:").count(), 1);
        let (_, again) = merge(&t2, Format::HermesYaml, Path::new(EXE)).unwrap();
        assert_eq!(again, Outcome::AlreadySet);

        let (t3, out) = remove(&t2, Format::HermesYaml).unwrap();
        assert_eq!(out, Outcome::Removed);
        assert_eq!(t3, with_other);

        let (t4, _) = merge("mcp_servers: {}\n", Format::HermesYaml, Path::new(EXE)).unwrap();
        assert_eq!(current(&t4, Format::HermesYaml).as_deref(), Some(EXE));
        assert!(merge("mcp_servers: {fusion: {command: x}}\n", Format::HermesYaml, Path::new(EXE)).is_err());
    }

    #[test]
    fn json_with_bom_is_read() {
        let (t, out) = merge("\u{feff}{\"mcpServers\":{\"other\":{\"command\":\"x\"}}}", Format::McpServers, Path::new(EXE)).unwrap();
        assert_eq!(out, Outcome::Added);
        assert_eq!(j(&t)["mcpServers"]["other"]["command"], "x");
    }

    #[test]
    fn install_backs_up_then_uninstall_removes_only_ours() {
        let home = temp("install");
        let file = home.join("settings.json");
        let original = r#"{"mcpServers":{"other":{"command":"x"}}}"#;
        std::fs::write(&file, original).unwrap();
        let agents = [Agent { name: "Gemini CLI", detect: file.parent().unwrap().to_path_buf(), config: file.clone(), format: Format::McpServers }];
        assert_eq!(install(Path::new(EXE), &agents), vec![("Gemini CLI", Outcome::Added)]);
        assert_eq!(std::fs::read_to_string(home.join("settings.json.labsetup-bak")).unwrap(), original);
        assert_eq!(install(Path::new(EXE), &agents), vec![("Gemini CLI", Outcome::AlreadySet)]);
        assert_eq!(uninstall(&agents), vec![("Gemini CLI", Outcome::Removed)]);
        let v = j(&std::fs::read_to_string(&file).unwrap());
        assert!(v["mcpServers"].get("labsetup").is_none());
        assert_eq!(v["mcpServers"]["other"]["command"], "x");
        assert_eq!(uninstall(&agents), vec![("Gemini CLI", Outcome::NotSet)]);
    }

    #[test]
    fn missing_agent_is_not_found_and_nothing_created() {
        let home = temp("missing");
        let file = home.join("nope").join("mcp.json");
        let agents = [Agent { name: "Cursor", detect: file.parent().unwrap().to_path_buf(), config: file.clone(), format: Format::McpServers }];
        assert_eq!(install(Path::new(EXE), &agents), vec![("Cursor", Outcome::NotFound)]);
        assert!(!file.exists());
    }

    #[test]
    fn config_in_home_needs_its_detect_folder() {
        let home = temp("claude");
        let agents = [Agent { name: "Claude Code", detect: home.join(".claude"), config: home.join(".claude.json"), format: Format::McpServers }];
        assert_eq!(install(Path::new(EXE), &agents), vec![("Claude Code", Outcome::NotFound)]);
        assert!(!home.join(".claude.json").exists());
        std::fs::create_dir(home.join(".claude")).unwrap();
        assert_eq!(install(Path::new(EXE), &agents), vec![("Claude Code", Outcome::Added)]);
    }

    #[test]
    fn known_agents_include_the_main_clis() {
        let names: Vec<_> = known_agents(Path::new(r"C:\Users\u"), Path::new(r"C:\Users\u\AppData\Roaming"), Path::new(r"C:\Users\u\AppData\Local")).iter().map(|a| a.name).collect();
        for n in ["Claude Code", "Claude desktop app", "Codex CLI", "Gemini CLI", "Antigravity", "Cursor", "GitHub Copilot CLI", "VS Code", "Trae", "opencode", "Hermes Agent"] {
            assert!(names.contains(&n), "{n}");
        }
    }

    #[test]
    fn agent_folder_without_file_gets_a_new_file() {
        let home = temp("newfile");
        let file = home.join("mcp.json");
        let agents = [Agent { name: "Cursor", detect: file.parent().unwrap().to_path_buf(), config: file.clone(), format: Format::McpServers }];
        assert_eq!(install(Path::new(EXE), &agents), vec![("Cursor", Outcome::Added)]);
        assert_eq!(current(&std::fs::read_to_string(&file).unwrap(), Format::McpServers).as_deref(), Some(EXE));
    }

    #[test]
    fn status_reports_stale_path() {
        let home = temp("status");
        let file = home.join("mcp.json");
        std::fs::write(&file, format!(r#"{{"mcpServers":{{"labsetup":{{"command":{},"args":["mcp"]}}}}}}"#, Value::from(OTHER))).unwrap();
        let agents = [
            Agent { name: "Cursor", detect: file.parent().unwrap().to_path_buf(), config: file, format: Format::McpServers },
            Agent { name: "Gone", detect: home.join("x").join("y.json").parent().unwrap().to_path_buf(), config: home.join("x").join("y.json"), format: Format::McpServers },
        ];
        let s = status(Path::new(EXE), &agents);
        assert_eq!(s[0], ("Cursor", format!("stale: {OTHER}")));
        assert_eq!(s[1], ("Gone", "not found".to_string()));
    }

    #[test]
    fn summary_lists_found_agents_and_the_switch_hint() {
        let r = vec![("Claude Code", Outcome::Added), ("Cursor", Outcome::NotFound), ("Codex CLI", Outcome::Failed("x".into()))];
        assert_eq!(summary(&r, true), "Claude Code: added\nCodex CLI: failed: x");
        assert_eq!(summary(&r, false), "Claude Code: added\nCodex CLI: failed: x\nAgents can connect once 'Allow AI agents' is On.");
        let none = vec![("Cursor", Outcome::NotFound)];
        assert!(summary(&none, true).starts_with("No coding-agent CLI found on this PC."));
    }

    #[test]
    fn snippet_is_valid_mcp_servers_json() {
        assert_eq!(j(&snippet(Path::new(EXE)))["mcpServers"]["labsetup"]["command"], EXE);
    }
}
