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
                        (drives the running app; see 'labsetup-cli live --help')";

const LIVE_HELP: &str = r#"labsetup-cli live <command> [--pid N] [--doc TOKEN] [--json ARGS] [--arg-name value ...]

Read `status` first and pass its doc token with --doc on writes. Use `features <node>` before `constrain`. Units are mm and degrees.

Flags become args one-to-one, with - turned into _ (--axis-height 75 sends axis_height: 75). A value that is
a JSON number or array (--position "[10,0,0]") is sent as one, anything else as a string. String-valued
options (part, id, name, text, search, hole, ...) are always sent as text; use --json for nested values. --json merges a
JSON object of args over the flags. Prints the reply line; exit 0 if ok, 1 on an error reply, 2 on usage or
connection failure. --pid picks the app when several run.

Reads:
  status                                              live status
  list       [--platform ID]                          live list --platform 1
  platforms                                           live platforms
  holes      --platform ID                            live holes --platform 1
  part       --id CATALOG_ID                          live part --id thorlabs:KM100
  catalog    --search TEXT [--limit N]                live catalog --search "post holder" --limit 10
  features   --node ID                                live features --node 5
  beams                                               live beams
  snapshot   [--path FILE] [--width N] [--height N]   live snapshot --path view.png

Writes (each is one undo step, labelled "Agent: ..."):
  place      --part ID, then one of --platform ID --hole C14 | --on NODE --conn NAME --parent-conn NAME [--slide MM] [--twist DEG]
             | --position "[x,y,z]" [--rotation "[z,y,x]"]; optional --name TEXT --rotate-z DEG
             live place --part thorlabs:KM100 --platform 1 --hole C14 --rotate-z 90 --doc TOKEN
  stack      --platform ID --hole C14 --holder ID --post ID --mount ID [--base ID] [--optic ID] (--axis-height MM | --slide MM) [--rotate-z DEG]
             live stack --platform 1 --hole C14 --holder H --post P --mount M --axis-height 75 --doc TOKEN
  constrain  --node ID, --to room | {"platform":ID} | {"node":ID}, --constraints [{child,parent,kind?,offset?,angle?,flip?,lock_rotation?}]
             live constrain --node 5 --json '{"to":{"node":4},"constraints":[{"child":"bottom","parent":"top","kind":"mate"}]}' --doc TOKEN
  move       --node ID [--position "[x,y,z]"] [--rotation "[z,y,x]"]
             live move --node 5 --position "[10,0,0]" --doc TOKEN
  set-params --node ID --params {...}
             live set-params --node 2 --json '{"params":{"wavelength_nm":532}}' --doc TOKEN
  rename     --node ID --name TEXT                    live rename --node 5 --name "Fold mirror" --doc TOKEN
  note       --node ID --text TEXT                    live note --node 5 --text "Check angle" --doc TOKEN
  remove     --nodes "[ID,...]"                       live remove --nodes "[5,6]" --doc TOKEN
  undo                                                live undo
  redo                                                live redo"#;

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
}

#[derive(Debug, PartialEq)]
enum Live {
    Help,
    Call { pid: Option<u32>, line: String },
}

/// Keys whose protocol type is a string: never turned into numbers (`--name 5` is "5").
const STRING_KEYS: &[&str] =
    &["part", "id", "name", "text", "search", "hole", "holder", "post", "mount", "base", "optic", "conn", "parent_conn", "child", "parent", "path"];

/// String keys stay text (a `hole` may still be a `[col,row]` array), `flip`/`lock_rotation` take
/// true/false, anything else goes out as JSON if it is a number or array, else as a string.
fn live_value(key: &str, s: &str) -> serde_json::Value {
    let json = serde_json::from_str::<serde_json::Value>(s).ok();
    match json {
        Some(v) if key == "hole" && v.is_array() => v,
        _ if STRING_KEYS.contains(&key) => serde_json::Value::String(s.to_owned()),
        Some(v @ serde_json::Value::Bool(_)) if matches!(key, "flip" | "lock_rotation") => v,
        Some(v) if v.is_number() || v.is_array() => v,
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
    let mut env = serde_json::Map::new();
    env.insert("v".into(), 1.into());
    env.insert("cmd".into(), cmd.replace('-', "_").into());
    env.insert("args".into(), args.into());
    if let Some(d) = doc {
        env.insert("doc".into(), d.into());
    }
    Ok(Live::Call { pid, line: serde_json::Value::Object(env).to_string() })
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
    let pipes = pipe::find_pipes();
    let name = match (pid, pipes.as_slice()) {
        (Some(p), _) => pipe::pipe_name(p),
        (None, [(_, n)]) => n.clone(),
        (None, []) => return Err("LabSetup isn't running, or 'Allow AI agents' is off in Settings.".into()),
        (None, many) => {
            let list: Vec<String> = many.iter().map(|(p, _)| format!("pid {p}")).collect();
            return Err(format!("several LabSetup windows are running, pick one with --pid:\n{}", list.join("\n")));
        }
    };
    let reply = pipe::call(&name, line, std::time::Duration::from_secs(2)).map_err(|e| format!("live: {e}"))?;
    println!("{reply}");
    let ok = serde_json::from_str::<serde_json::Value>(&reply).is_ok_and(|v| v["ok"] == true);
    Ok(if ok { 0 } else { 1 })
}

#[cfg(not(windows))]
fn run_live(_: Option<u32>, _: &str) -> Result<u8, String> {
    Err("live control needs Windows".into())
}

fn run(cmd: Cmd) -> Result<u8, String> {
    match cmd {
        Cmd::Live(Live::Help) => println!("{LIVE_HELP}"),
        Cmd::Live(Live::Call { pid, line }) => return run_live(pid, &line),
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
