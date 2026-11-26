//! `labsetup-cli`: headless read/inspect/export of `.labsetup` projects and the part catalog.
//! No window, GPU, Slint or Bevy. Exit codes: 0 ok, 1 problems found (`validate`), 2 usage or I/O error.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use lab_app::bom_rows::{rows, to_csv};
use lab_catalog::Catalog;
use lab_core::glam::EulerRot;
use lab_core::model::{Attach, Document, Parent, Shape};
use lab_core::LoadReport;
use serde_json::json;

const USAGE: &str = "\
usage:
  labsetup-cli --help | --version
  labsetup-cli info     <project> [--json]
  labsetup-cli list     <project> [--json]
  labsetup-cli bom      <project> [-o out.csv]
  labsetup-cli validate <project>
  labsetup-cli catalog  [--search <text>]
  labsetup-cli live     <command> [--pid N] [--doc TOKEN] [--json ARGS] [--arg value ...]
                        (drives the running app; see 'labsetup-cli live --help')
  labsetup-cli mcp      MCP server on stdin/stdout for AI agents (the live commands as tools)
  labsetup-cli agents   install | uninstall | status
                        (registers 'labsetup-cli mcp' with the coding-agent CLIs on this PC)";

#[derive(Debug, PartialEq)]
enum Cmd {
    Help,
    Version,
    Info { path: PathBuf, json: bool },
    List { path: PathBuf, json: bool },
    Bom { path: PathBuf, out: Option<PathBuf> },
    Validate { path: PathBuf },
    Catalog { search: Option<String> },
    Live(Live),
    Mcp,
    Agents(AgentsCmd),
}

#[derive(Debug, PartialEq)]
enum AgentsCmd {
    Install,
    Uninstall,
    Status,
}

#[derive(Debug, PartialEq)]
enum Live {
    Help,
    Call { pid: Option<u32>, line: String },
}

/// Keys whose protocol type is a string: never turned into numbers (`--name 5` is "5").
const STRING_KEYS: &[&str] =
    &["part", "id", "name", "text", "search", "hole", "holder", "post", "mount", "base", "optic", "conn", "parent_conn", "path"];

/// String keys stay text (a `hole` may still be a `[col,row]` array), `flip`/`lock_rotation` take
/// true/false, anything else goes out as JSON if it is a number, array or object, else as a string
/// (`--to room`, `--parent room`).
fn live_value(key: &str, s: &str) -> serde_json::Value {
    let json = serde_json::from_str::<serde_json::Value>(s).ok();
    match json {
        Some(v) if key == "hole" && v.is_array() => v,
        _ if STRING_KEYS.contains(&key) => serde_json::Value::String(s.to_owned()),
        Some(v @ serde_json::Value::Bool(_)) if matches!(key, "flip" | "lock_rotation") => v,
        Some(v) if v.is_number() || v.is_array() || v.is_object() => v,
        _ => serde_json::Value::String(s.to_owned()),
    }
}

fn parse_live(rest: &[String]) -> Result<Live, String> {
    let (cmd, rest) = rest.split_first().ok_or("'live' needs a command (try 'labsetup-cli live --help')")?;
    if matches!(cmd.as_str(), "--help" | "-h" | "help") {
        return Ok(Live::Help);
    }
    let (mut pid, mut doc, mut args, mut over) = (None, None, serde_json::Map::new(), None);
    let mut it = rest.iter();
    while let Some(a) = it.next() {
        if a == "--help" || a == "-h" {
            return Ok(Live::Help);
        }
        let name = a.strip_prefix("--").filter(|n| !n.is_empty()).ok_or_else(|| format!("unexpected argument '{a}'"))?;
        let val = it.next().ok_or_else(|| format!("{a} needs a value"))?;
        match name {
            "pid" => pid = Some(val.parse().map_err(|_| format!("--pid needs a number, got '{val}'"))?),
            "doc" => doc = Some(val.clone()),
            "json" => match serde_json::from_str(val) {
                Ok(serde_json::Value::Object(m)) => over = Some(m),
                _ => return Err("--json needs a JSON object".into()),
            },
            _ => {
                let key = name.replace('-', "_");
                let v = live_value(&key, val);
                args.insert(key, v);
            }
        }
    }
    args.extend(over.unwrap_or_default());
    Ok(Live::Call { pid, line: lab_app::live::tools::request_line(cmd, args, doc.as_deref()) })
}

fn parse(args: &[String]) -> Result<Cmd, String> {
    let (cmd, rest) = args.split_first().ok_or("no command given")?;
    if cmd == "live" {
        return parse_live(rest).map(Cmd::Live);
    }
    let mut path = None;
    let mut json = false;
    let mut out = None;
    let mut search = None;
    let mut it = rest.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--json" if matches!(cmd.as_str(), "info" | "list") => json = true,
            "-o" | "--output" if cmd == "bom" => out = Some(PathBuf::from(it.next().ok_or("-o needs a file name")?)),
            "--search" if cmd == "catalog" => search = Some(it.next().ok_or("--search needs text")?.clone()),
            s if s.starts_with('-') => return Err(format!("unknown option '{s}' for '{cmd}'")),
            s if path.is_none() && cmd != "catalog" => path = Some(PathBuf::from(s)),
            s => return Err(format!("unexpected argument '{s}'")),
        }
    }
    let need = |p: Option<PathBuf>| p.ok_or_else(|| format!("'{cmd}' needs a project file"));
    Ok(match cmd.as_str() {
        "--help" | "-h" | "help" if rest.is_empty() => Cmd::Help,
        "--version" | "-V" if rest.is_empty() => Cmd::Version,
        "info" => Cmd::Info { path: need(path)?, json },
        "list" => Cmd::List { path: need(path)?, json },
        "bom" => Cmd::Bom { path: need(path)?, out },
        "validate" => Cmd::Validate { path: need(path)? },
        "catalog" => Cmd::Catalog { search },
        "mcp" if rest.is_empty() => Cmd::Mcp,
        "agents" => match rest {
            [a] if a == "install" => Cmd::Agents(AgentsCmd::Install),
            [a] if a == "uninstall" => Cmd::Agents(AgentsCmd::Uninstall),
            [a] if a == "status" => Cmd::Agents(AgentsCmd::Status),
            _ => return Err("'agents' needs install, uninstall or status".into()),
        },
        other => return Err(format!("unknown command '{other}'")),
    })
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let cmd = match parse(&args) {
        Ok(cmd) => cmd,
        Err(e) => {
            eprintln!("labsetup-cli: {e}\n{USAGE}");
            return ExitCode::from(2);
        }
    };
    match run(cmd) {
        Ok(code) => ExitCode::from(code),
        Err(e) => {
            eprintln!("labsetup-cli: {e}");
            ExitCode::from(2)
        }
    }
}

fn load(path: &Path) -> Result<(Document, LoadReport), String> {
    let bytes = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    lab_file::decode_bytes(&bytes).map_err(|e| format!("{}: {e}", path.display()))
}

fn catalog() -> Catalog {
    let (catalog, warnings) = Catalog::load_default();
    for w in &warnings {
        eprintln!("library: {}: {}", w.path.display(), w.message);
    }
    catalog
}

fn constraint_count(doc: &Document) -> usize {
    doc.nodes
        .iter()
        .map(|n| match &n.attach {
            Attach::Constrained { constraints } => constraints.len(),
            _ => 0,
        })
        .sum()
}

fn parent_text(doc: &Document, parent: &Parent) -> String {
    match parent {
        Parent::Room => "room".into(),
        Parent::Platform(id) => doc.platform(*id).map_or_else(|| format!("platform {}", id.0), |p| format!("platform {}", p.name)),
        Parent::Node(id) => format!("node {}", id.0),
    }
}

#[cfg(windows)]
fn run_live(pid: Option<u32>, line: &str) -> Result<u8, String> {
    use lab_app::live::pipe;
    let name = pipe::pick(pid)?;
    let reply = pipe::call(&name, line, std::time::Duration::from_secs(2)).map_err(|e| format!("live: {e}"))?;
    println!("{reply}");
    let ok = serde_json::from_str::<serde_json::Value>(&reply).is_ok_and(|v| v["ok"] == true);
    Ok(if ok { 0 } else { 1 })
}

fn run_agents(cmd: AgentsCmd) -> Result<u8, String> {
    use lab_app::agents::{home_dirs, install, known_agents, snippet, status, uninstall, Outcome};
    let exe = std::env::current_exe().map_err(|e| format!("can't find this program's path: {e}"))?;
    let (home, appdata, local) = home_dirs();
    let agents = known_agents(&home, &appdata, &local);
    let results = match cmd {
        AgentsCmd::Status => {
            for (name, s) in status(&exe, &agents) {
                println!("{name}: {s}");
            }
            return Ok(0);
        }
        AgentsCmd::Install => install(&exe, &agents),
        AgentsCmd::Uninstall => uninstall(&agents),
    };
    for (name, o) in &results {
        println!("{name}: {o}");
    }
    if cmd == AgentsCmd::Install {
        println!("\nOther MCP-capable agents: add this to their MCP config:\n{}", snippet(&exe));
        println!("Agents can control LabSetup while it runs with 'Allow AI agents' On in Settings.");
    }
    Ok(if results.iter().any(|(_, o)| matches!(o, Outcome::Failed(_))) { 1 } else { 0 })
}

/// Nothing but protocol lines may reach stdout here.
#[cfg(windows)]
fn run_mcp() -> Result<u8, String> {
    use lab_app::live::{mcp, pipe};
    let mut send = |pid: Option<u32>, line: &str| {
        pipe::pick(pid).and_then(|n| pipe::call(&n, line, std::time::Duration::from_secs(2)).map_err(|e| format!("live: {e}")))
    };
    mcp::serve(std::io::stdin().lock(), std::io::stdout().lock(), &mut send).map_err(|e| format!("mcp: {e}"))?;
    Ok(0)
}

#[cfg(not(windows))]
fn run_mcp() -> Result<u8, String> {
    Err("live control needs Windows".into())
}

#[cfg(not(windows))]
fn run_live(_: Option<u32>, _: &str) -> Result<u8, String> {
    Err("live control needs Windows".into())
}

fn run(cmd: Cmd) -> Result<u8, String> {
    match cmd {
        Cmd::Live(Live::Help) => println!("{}", lab_app::live::tools::help_text()),
        Cmd::Live(Live::Call { pid, line }) => return run_live(pid, &line),
        Cmd::Mcp => return run_mcp(),
        Cmd::Agents(a) => return run_agents(a),
        Cmd::Help => println!("{USAGE}"),
        Cmd::Version => println!("labsetup-cli {}", env!("CARGO_PKG_VERSION")),
        Cmd::Info { path, json } => {
            let (doc, report) = load(&path)?;
            let constraints = constraint_count(&doc);
            if json {
                let v = json!({
                    "title": doc.name,
                    "format_version": doc.schema_version,
                    "migrated_from": report.migrated_from,
                    "platforms": doc.platforms.len(),
                    "parts": doc.nodes.len(),
                    "constraints": constraints,
                    "warnings": report.repairs,
                });
                println!("{}", serde_json::to_string_pretty(&v).map_err(|e| e.to_string())?);
            } else {
                println!("title:        {}", doc.name);
                println!("format:       {}", doc.schema_version);
                if let Some(from) = report.migrated_from {
                    println!("migrated from format {from}");
                }
                println!("platforms:    {}", doc.platforms.len());
                println!("parts:        {}", doc.nodes.len());
                println!("constraints:  {constraints}");
                for w in &report.repairs {
                    println!("warning: {w}");
                }
            }
        }
        Cmd::List { path, json } => {
            let (doc, _) = load(&path)?;
            let catalog = catalog();
            let mut items = Vec::new();
            for n in &doc.nodes {
                let (vendor, number) =
                    catalog.get(&n.part).map_or((String::new(), n.part.clone()), |d| (d.vendor.clone(), d.part_number.clone()));
                let (rz, ry, rx) = n.rotation.to_euler(EulerRot::ZYX);
                let rot = [rz.to_degrees(), ry.to_degrees(), rx.to_degrees()];
                let pos = [n.position.x, n.position.y, n.position.z];
                let parent = parent_text(&doc, &n.parent);
                if json {
                    items.push(json!({
                        "id": n.id.0, "name": n.name, "part": n.part, "vendor": vendor, "part_number": number,
                        "parent": parent, "position_mm": pos, "rotation_zyx_deg": rot,
                    }));
                } else {
                    println!(
                        "{}\t{}\t{vendor}/{number}\t{parent}\t{:.2},{:.2},{:.2}\t{:.2},{:.2},{:.2}",
                        n.id.0, n.name, pos[0], pos[1], pos[2], rot[0], rot[1], rot[2]
                    );
                }
            }
            if json {
                println!("{}", serde_json::to_string_pretty(&items).map_err(|e| e.to_string())?);
            }
        }
        Cmd::Bom { path, out } => {
            let (doc, _) = load(&path)?;
            let csv = to_csv(&rows(&doc, &catalog()));
            match out {
                Some(out) => lab_file::atomic_write(&out, csv.as_bytes()).map_err(|e| format!("{}: {e}", out.display()))?,
                None => print!("{csv}"),
            }
        }
        Cmd::Validate { path } => {
            let bytes = std::fs::read(&path).map_err(|e| format!("{}: {e}", path.display()))?;
            let (doc, report) = match lab_file::decode_bytes(&bytes) {
                Ok(loaded) => loaded,
                Err(e) => {
                    eprintln!("{}: {e}", path.display());
                    return Ok(1);
                }
            };
            let catalog = catalog();
            let mut problems = report.repairs.clone();
            for n in &doc.nodes {
                // Box/Cylinder nodes are drawn from their shape; only Part nodes need a catalog def.
                if n.shape == Shape::Part && catalog.get(&n.part).is_none() {
                    problems.push(format!("node {} '{}': part '{}' is not in the catalog", n.id.0, n.name, n.part));
                }
            }
            for p in &problems {
                eprintln!("{}: {p}", path.display());
            }
            if !problems.is_empty() {
                return Ok(1);
            }
            println!("{}: ok", path.display());
        }
        Cmd::Catalog { search } => {
            let catalog = catalog();
            let parts = match &search {
                Some(text) => catalog.search(text, usize::MAX),
                None => catalog.all(),
            };
            for d in parts {
                println!("{}\t{}\t{}", d.vendor, d.part_number, d.name);
            }
        }
    }
    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(s: &str) -> Vec<String> {
        s.split_whitespace().map(String::from).collect()
    }

    #[test]
    fn parses_commands() {
        assert_eq!(
            parse(&args("bom x.labsetup -o y.csv")),
            Ok(Cmd::Bom { path: "x.labsetup".into(), out: Some("y.csv".into()) })
        );
        assert_eq!(parse(&args("info p --json")), Ok(Cmd::Info { path: "p".into(), json: true }));
        assert_eq!(parse(&args("catalog --search km100")), Ok(Cmd::Catalog { search: Some("km100".into()) }));
        assert!(parse(&args("frobnicate x")).is_err());
        assert!(parse(&args("bom")).is_err());
        assert!(parse(&args("bom x -o")).is_err());
        assert!(parse(&args("validate x --json")).is_err());
        assert!(parse(&[]).is_err());
        assert_eq!(parse(&args("mcp")), Ok(Cmd::Mcp));
        assert!(parse(&args("mcp extra")).is_err());
        assert_eq!(parse(&args("agents install")), Ok(Cmd::Agents(AgentsCmd::Install)));
        assert_eq!(parse(&args("agents status")), Ok(Cmd::Agents(AgentsCmd::Status)));
        assert!(parse(&args("agents")).is_err());
        assert!(parse(&args("agents frob")).is_err());
    }

    fn live_line(s: &str) -> serde_json::Value {
        match parse(&args(s)) {
            Ok(Cmd::Live(Live::Call { line, .. })) => serde_json::from_str(&line).unwrap(),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn parses_live_flags() {
        let line = "live place --part thorlabs:KM100 --platform 1 --hole C14 --rotate-z 90 --doc T1";
        assert_eq!(
            live_line(line),
            json!({"v":1,"cmd":"place","doc":"T1","args":{"part":"thorlabs:KM100","platform":1,"hole":"C14","rotate_z":90}})
        );
        let Ok(Cmd::Live(Live::Call { line, .. })) = parse(&args(line)) else { panic!() };
        assert!(lab_app::live::protocol::parse_line(&line).is_ok(), "{line}");
        assert_eq!(live_line("live move --node 5 --position [10,0,0]")["args"], json!({"node":5,"position":[10,0,0]}));
        assert_eq!(live_line("live set-params --node 2 --json {\"params\":{}}")["cmd"], "set_params");
        assert!(matches!(parse(&args("live status --pid 7")), Ok(Cmd::Live(Live::Call { pid: Some(7), .. }))));
    }

    #[test]
    fn string_keys_stay_strings() {
        assert_eq!(live_line("live catalog --search 100")["args"], json!({"search":"100"}));
        assert_eq!(live_line("live rename --node 5 --name 5")["args"], json!({"node":5,"name":"5"}));
        assert_eq!(live_line("live place --part x --hole 3")["args"]["hole"], "3");
        assert_eq!(live_line("live place --part x --hole [3,4]")["args"]["hole"], json!([3, 4]));
    }

    #[test]
    fn object_and_parent_flags() {
        assert_eq!(live_line("live constrain --node 5 --to {\"node\":4}")["args"]["to"], json!({"node":4}));
        assert_eq!(live_line("live constrain --node 5 --to room")["args"]["to"], "room");
        let v = live_line("live set-params --node 2 --params {\"wavelength_nm\":633}");
        assert_eq!(v["args"]["params"], json!({"wavelength_nm":633}));
        assert_eq!(live_line("live place --part x --parent room --position [0,0,0]")["args"]["parent"], "room");
        assert_eq!(live_line("live place --part x --parent {\"node\":5} --position [0,0,0]")["args"]["parent"], json!({"node":5}));
        for line in ["live place --part x --parent room --position [0,0,0]", "live constrain --node 5 --to {\"node\":4} --json {\"constraints\":[]}"] {
            let Ok(Cmd::Live(Live::Call { line: wire, .. })) = parse(&args(line)) else { panic!() };
            assert!(lab_app::live::protocol::parse_line(&wire).is_ok(), "{wire}");
        }
    }

    #[test]
    fn book_add_sends_a_parts_array() {
        let line = r#"live book-add --parts ["PBS122","KM100"]"#;
        let v = live_line(line);
        assert_eq!(v["cmd"], "book_add");
        assert_eq!(v["args"], json!({"parts":["PBS122","KM100"]}));
        let Ok(Cmd::Live(Live::Call { line: wire, .. })) = parse(&args(line)) else { panic!() };
        assert!(lab_app::live::protocol::parse_line(&wire).is_ok(), "{wire}");
    }

    #[test]
    fn bool_keys_parse() {
        assert_eq!(live_line("live constrain --flip true")["args"], json!({"flip":true}));
        assert_eq!(live_line("live constrain --flip yes")["args"], json!({"flip":"yes"}));
    }

    #[test]
    fn json_overrides_flags() {
        let v = live_line("live stack --axis-height 75 --json {\"axis_height\":80,\"holder\":\"h\"}");
        assert_eq!(v["args"], json!({"axis_height":80,"holder":"h"}));
    }

    #[test]
    fn live_usage_errors() {
        assert!(parse(&args("live")).is_err());
        assert!(parse(&args("live status stray")).is_err());
        assert!(parse(&args("live status --doc")).is_err());
        assert!(parse(&args("live status --json [1]")).is_err());
        assert_eq!(parse(&args("live --help")), Ok(Cmd::Live(Live::Help)));
    }

    /// Writes the demo project to `%TEMP%/labsetup-cli-demo.labsetup` (a sample for manual runs) and
    /// checks it reloads.
    #[test]
    fn demo_sample_round_trips() {
        let doc = lab_core::sample::demo_document();
        let bytes = lab_file::encode_archive(&doc).unwrap();
        std::fs::write(std::env::temp_dir().join("labsetup-cli-demo.labsetup"), &bytes).unwrap();
        let (back, _) = lab_file::decode_bytes(&bytes).unwrap();
        assert_eq!(back.nodes.len(), doc.nodes.len());
    }
}
