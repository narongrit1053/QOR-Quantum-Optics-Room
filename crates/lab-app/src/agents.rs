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
    /// `[mcp_servers.labsetup]` with `command` and `args`
    CodexToml,
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

/// `%USERPROFILE%` and `%APPDATA%`.
pub fn home_dirs() -> (PathBuf, PathBuf) {
    let get = |k: &str| std::env::var_os(k).map(PathBuf::from).unwrap_or_default();
    (get("USERPROFILE"), get("APPDATA"))
}

/// The agent CLIs LabSetup registers with (paths checked against each tool's docs, 2026-11).
/// opencode is left out: its docs disagree on the entry shape and it rejects unknown config.
pub fn known_agents(home: &Path, appdata: &Path) -> Vec<Agent> {
    let a = |name, detect: PathBuf, config: PathBuf, format| Agent { name, detect, config, format };
    let gemini = home.join(".gemini");
    vec![
        a("Claude Code", home.join(".claude"), home.join(".claude.json"), Format::McpServers),
        a("Codex CLI", home.join(".codex"), home.join(".codex").join("config.toml"), Format::CodexToml),
        a("Gemini CLI", gemini.clone(), gemini.join("settings.json"), Format::McpServers),
        a("Antigravity", gemini.join("antigravity"), gemini.join("config").join("mcp_config.json"), Format::McpServers),
        a("Cursor", home.join(".cursor"), home.join(".cursor").join("mcp.json"), Format::Cursor),
        a("GitHub Copilot CLI", home.join(".copilot"), home.join(".copilot").join("mcp-config.json"), Format::Copilot),
        a("Windsurf", home.join(".codeium").join("windsurf"), home.join(".codeium").join("windsurf").join("mcp_config.json"), Format::McpServers),
        a("Devin CLI", home.join(".config").join("devin"), home.join(".config").join("devin").join("mcp_config.json"), Format::McpServers),
        a("VS Code", appdata.join("Code").join("User"), appdata.join("Code").join("User").join("mcp.json"), Format::VsCode),
    ]
}

fn section(format: Format) -> &'static str {
    match format {
        Format::VsCode => "servers",
        Format::CodexToml => "mcp_servers",
        _ => "mcpServers",
    }
}

fn entry(format: Format, exe: &str) -> Value {
    match format {
        Format::VsCode | Format::Cursor => json!({"type": "stdio", "command": exe, "args": ["mcp"]}),
        Format::Copilot => json!({"type": "local", "command": exe, "args": ["mcp"], "tools": ["*"]}),
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
    if format == Format::CodexToml {
        parse_toml(text).map(drop)
    } else {
        parse_json(text).map(drop)
    }
}

/// The registered command path, if any.
pub fn current(text: &str, format: Format) -> Option<String> {
    let text = strip_bom(text);
    if format == Format::CodexToml {
        let t = parse_toml(text).ok()?;
        return t.get(section(format))?.get(NAME)?.get("command")?.as_str().map(String::from);
    }
    let m = parse_json(text).ok()?;
    m.get(section(format))?.get(NAME)?.get("command")?.as_str().map(String::from)
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
    let new = if format == Format::CodexToml {
        let block = format!("{TOML_HEADER}\ncommand = {}\nargs = [\"mcp\"]\n", toml::Value::String(exe.to_string()));
        edit_toml(text, Some(&block))?
    } else {
        let mut m = parse_json(text)?;
        let s = m.entry(section(format)).or_insert_with(|| json!({}));
        let s = s.as_object_mut().ok_or_else(|| format!("its \"{}\" is not an object, left unchanged", section(format)))?;
        s.insert(NAME.into(), entry(format, &exe));
        json_text(m)
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
    let new = if format == Format::CodexToml {
        edit_toml(text, None)?
    } else {
        let mut m = parse_json(text)?;
        if let Some(s) = m.get_mut(section(format)).and_then(Value::as_object_mut) {
            s.remove(NAME);
        }
        json_text(m)
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
        let names: Vec<_> = known_agents(Path::new(r"C:\Users\u"), Path::new(r"C:\Users\u\AppData\Roaming")).iter().map(|a| a.name).collect();
        for n in ["Claude Code", "Codex CLI", "Gemini CLI", "Antigravity", "Cursor", "GitHub Copilot CLI", "VS Code"] {
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
