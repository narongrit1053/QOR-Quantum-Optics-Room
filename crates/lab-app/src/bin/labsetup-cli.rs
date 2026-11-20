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
  labsetup-cli catalog  [--search <text>]";

#[derive(Debug, PartialEq)]
enum Cmd {
    Help,
    Version,
    Info { path: PathBuf, json: bool },
    List { path: PathBuf, json: bool },
    Bom { path: PathBuf, out: Option<PathBuf> },
    Validate { path: PathBuf },
    Catalog { search: Option<String> },
}

fn parse(args: &[String]) -> Result<Cmd, String> {
    let (cmd, rest) = args.split_first().ok_or("no command given")?;
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

fn run(cmd: Cmd) -> Result<u8, String> {
    match cmd {
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
