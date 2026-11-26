//! The one list of live commands: drives `labsetup-cli live --help`, the request line the CLI and
//! the MCP server send, and the MCP tool list. A command missing here fails `table_covers_every_command`.

use serde_json::{json, Map, Value};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Group {
    Read,
    Write,
    App,
}

pub struct Tool {
    /// CLI spelling, e.g. `place-bundle`; the wire name is this with `-` turned into `_`.
    pub cmd: &'static str,
    pub group: Group,
    /// The flags column of `live --help`.
    pub usage: &'static str,
    pub summary: &'static str,
    /// A full CLI example line.
    pub example: &'static str,
    pub example_args: fn() -> Value,
    /// JSON Schema of the args object, without `pid`/`doc`.
    pub schema: fn() -> Value,
}

fn obj(props: Value, required: &[&str]) -> Value {
    json!({"type": "object", "properties": props, "required": required})
}
fn int(d: &str) -> Value {
    json!({"type": "integer", "description": d})
}
fn num(d: &str) -> Value {
    json!({"type": "number", "description": d})
}
fn text(d: &str) -> Value {
    json!({"type": "string", "description": d})
}
fn vec3(d: &str) -> Value {
    json!({"type": "array", "items": {"type": "number"}, "minItems": 3, "maxItems": 3, "description": d})
}
fn strings(d: &str) -> Value {
    json!({"type": "array", "items": {"type": "string"}, "description": d})
}
fn none() -> Value {
    obj(json!({}), &[])
}
fn empty() -> Value {
    json!({})
}
// shortcut: no oneOf/anyOf in schemas (some agent CLIs reject them); alternatives are told in descriptions.
fn hole() -> Value {
    text("hole label like C14 (a [col,row] array also works)")
}
fn parent() -> Value {
    json!({"description": "\"room\", {\"platform\": ID} or {\"node\": ID}"})
}

const TOOLS: &[Tool] = &[
    // Reads
    Tool {
        cmd: "status",
        group: Group::Read,
        usage: "",
        summary: "the open project and its doc token (pass it as doc on writes)",
        example: "live status",
        example_args: empty,
        schema: none,
    },
    Tool {
        cmd: "list",
        group: Group::Read,
        usage: "[--platform ID]",
        summary: "parts with ids, names, parents and poses",
        example: "live list --platform 1",
        example_args: || json!({"platform": 1}),
        schema: || obj(json!({"platform": int("only parts on this platform")}), &[]),
    },
    Tool {
        cmd: "platforms",
        group: Group::Read,
        usage: "",
        summary: "breadboards and tables with their ids",
        example: "live platforms",
        example_args: empty,
        schema: none,
    },
    Tool {
        cmd: "holes",
        group: Group::Read,
        usage: "--platform ID",
        summary: "the hole grid of a platform (labels like C14)",
        example: "live holes --platform 1",
        example_args: || json!({"platform": 1}),
        schema: || obj(json!({"platform": int("platform id")}), &["platform"]),
    },
    Tool {
        cmd: "part",
        group: Group::Read,
        usage: "--id CATALOG_ID",
        summary: "one catalog part: size, connectors, parameters",
        example: "live part --id thorlabs:KM100",
        example_args: || json!({"id": "thorlabs:KM100"}),
        schema: || obj(json!({"id": text("catalog id like thorlabs:KM100")}), &["id"]),
    },
    Tool {
        cmd: "catalog",
        group: Group::Read,
        usage: "--search TEXT [--limit N]",
        summary: "parts in the library",
        example: "live catalog --search \"post holder\" --limit 10",
        example_args: || json!({"search": "post holder", "limit": 10}),
        schema: || obj(json!({"search": text("words to match"), "limit": int("most rows to return")}), &["search"]),
    },
    Tool {
        cmd: "features",
        group: Group::Read,
        usage: "--node ID",
        summary: "a placed part's named features (faces, axes) for constrain",
        example: "live features --node 5",
        example_args: || json!({"node": 5}),
        schema: || obj(json!({"node": int("node id")}), &["node"]),
    },
    Tool {
        cmd: "beams",
        group: Group::Read,
        usage: "",
        summary: "traced beam paths",
        example: "live beams",
        example_args: empty,
        schema: none,
    },
    Tool {
        cmd: "bundles",
        group: Group::Read,
        usage: "",
        summary: "saved library bundles (name, group, parts)",
        example: "live bundles",
        example_args: empty,
        schema: none,
    },
    Tool {
        cmd: "snapshot",
        group: Group::Read,
        usage: "[--path FILE] [--width N] [--height N]",
        summary: "render the 3D view to a PNG",
        example: "live snapshot --path view.png",
        example_args: || json!({"path": "view.png"}),
        schema: || {
            obj(
                json!({"path": text("PNG file to write; a temp file when left out"), "width": int("pixels, default 1280"), "height": int("pixels, default 800")}),
                &[],
            )
        },
    },
    // Writes
    Tool {
        cmd: "place",
        group: Group::Write,
        usage: "--part ID, then one of --platform ID --hole C14 | --on NODE --conn NAME --parent-conn NAME [--slide MM] [--twist DEG]\n             | --position \"[x,y,z]\" [--rotation \"[z,y,x]\"]; optional --name TEXT --rotate-z DEG",
        summary: "add a part: in a platform hole (platform+hole), on another part's connector (on+conn+parent_conn), or at a position",
        example: "live place --part thorlabs:KM100 --platform 1 --hole C14 --rotate-z 90 --doc TOKEN",
        example_args: || json!({"part": "thorlabs:KM100", "platform": 1, "hole": "C14", "rotate_z": 90}),
        schema: || {
            obj(
                json!({
                    "part": text("catalog id"),
                    "platform": int("platform id, with hole"),
                    "hole": hole(),
                    "on": int("node to mount on, with conn and parent_conn"),
                    "conn": text("connector of the new part"),
                    "parent_conn": text("connector of the part it goes on"),
                    "slide": num("mm along the connector"),
                    "twist": num("degrees about the connector"),
                    "parent": parent(),
                    "position": vec3("[x,y,z] mm, when placing freely"),
                    "rotation": vec3("[z,y,x] degrees"),
                    "name": text("display name"),
                    "rotate_z": num("degrees about the vertical"),
                }),
                &["part"],
            )
        },
    },
    Tool {
        cmd: "stack",
        group: Group::Write,
        usage: "--platform ID --hole C14 --holder ID --post ID --mount ID [--base ID] [--optic ID] (--axis-height MM | --slide MM) [--rotate-z DEG]",
        summary: "post holder + post + mount (+ optic) in one step, at a beam height",
        example: "live stack --platform 1 --hole C14 --holder H --post P --mount M --axis-height 75 --doc TOKEN",
        example_args: || json!({"platform": 1, "hole": "C14", "holder": "H", "post": "P", "mount": "M", "axis_height": 75}),
        schema: || {
            obj(
                json!({
                    "platform": int("platform id"),
                    "hole": hole(),
                    "holder": text("post holder catalog id"),
                    "post": text("post catalog id"),
                    "mount": text("mount catalog id"),
                    "base": text("base catalog id"),
                    "optic": text("optic catalog id"),
                    "axis_height": num("optical axis height in mm (or give slide)"),
                    "slide": num("post slide in mm (or give axis_height)"),
                    "rotate_z": num("degrees about the vertical"),
                }),
                &["platform", "hole", "holder", "post", "mount"],
            )
        },
    },
    Tool {
        cmd: "place-bundle",
        group: Group::Write,
        usage: "--name NAME --platform ID --hole C14 [--rotate-z DEG]",
        summary: "a saved bundle, its first part on the hole as when dropped, turned about the hole",
        example: "live place-bundle --name \"QWP set\" --platform 1 --hole C14 --rotate-z 90 --doc TOKEN",
        example_args: || json!({"name": "QWP set", "platform": 1, "hole": "C14", "rotate_z": 90}),
        schema: || {
            obj(
                json!({"name": text("bundle name (see bundles)"), "platform": int("platform id"), "hole": hole(), "rotate_z": num("degrees about the hole")}),
                &["name", "platform", "hole"],
            )
        },
    },
    Tool {
        cmd: "constrain",
        group: Group::Write,
        usage: "--node ID, --to room | {\"platform\":ID} | {\"node\":ID}, --constraints [{child,parent,kind?,offset?,angle?,flip?,lock_rotation?}]",
        summary: "attach a part with constraints between its features and another's (read features first)",
        example: "live constrain --node 5 --json '{\"to\":{\"node\":4},\"constraints\":[{\"child\":\"bottom\",\"parent\":\"top\",\"kind\":\"mate\"}]}' --doc TOKEN",
        example_args: || json!({"node": 5, "to": {"node": 4}, "constraints": [{"child": "bottom", "parent": "top", "kind": "mate"}]}),
        schema: || {
            obj(
                json!({
                    "node": int("the part to constrain"),
                    "to": parent(),
                    "constraints": {"type": "array", "items": {"type": "object", "properties": {
                        "child": text("feature of node"),
                        "parent": text("feature of the other part"),
                        "kind": text("mate, flush, concentric or angle"),
                        "offset": num("mm"),
                        "angle": num("degrees"),
                        "flip": {"type": "boolean"},
                        "lock_rotation": {"type": "boolean"},
                    }, "required": ["child", "parent"]}},
                }),
                &["node", "to", "constraints"],
            )
        },
    },
    Tool {
        cmd: "move",
        group: Group::Write,
        usage: "--node ID [--position \"[x,y,z]\"] [--rotation \"[z,y,x]\"]",
        summary: "set a part's position and/or rotation",
        example: "live move --node 5 --position \"[10,0,0]\" --doc TOKEN",
        example_args: || json!({"node": 5, "position": [10, 0, 0]}),
        schema: || obj(json!({"node": int("node id"), "position": vec3("[x,y,z] mm"), "rotation": vec3("[z,y,x] degrees")}), &["node"]),
    },
    Tool {
        cmd: "set-params",
        group: Group::Write,
        usage: "--node ID --params {...}",
        summary: "change a part's parameters",
        example: "live set-params --node 2 --params '{\"beam_wavelength_nm\":532}' --doc TOKEN",
        example_args: || json!({"node": 2, "params": {"beam_wavelength_nm": 532}}),
        schema: || obj(json!({"node": int("node id"), "params": {"type": "object", "description": "parameter name to value"}}), &["node", "params"]),
    },
    Tool {
        cmd: "rename",
        group: Group::Write,
        usage: "--node ID --name TEXT",
        summary: "rename a part",
        example: "live rename --node 5 --name \"Fold mirror\" --doc TOKEN",
        example_args: || json!({"node": 5, "name": "Fold mirror"}),
        schema: || obj(json!({"node": int("node id"), "name": text("new name")}), &["node", "name"]),
    },
    Tool {
        cmd: "note",
        group: Group::Write,
        usage: "--node ID --text TEXT",
        summary: "set a part's note",
        example: "live note --node 5 --text \"Check angle\" --doc TOKEN",
        example_args: || json!({"node": 5, "text": "Check angle"}),
        schema: || obj(json!({"node": int("node id"), "text": text("note text")}), &["node", "text"]),
    },
    Tool {
        cmd: "remove",
        group: Group::Write,
        usage: "--nodes \"[ID,...]\"",
        summary: "delete parts",
        example: "live remove --nodes \"[5,6]\" --doc TOKEN",
        example_args: || json!({"nodes": [5, 6]}),
        schema: || obj(json!({"nodes": {"type": "array", "items": {"type": "integer"}, "description": "node ids"}}), &["nodes"]),
    },
    Tool {
        cmd: "undo",
        group: Group::Write,
        usage: "",
        summary: "undo the last step",
        example: "live undo",
        example_args: empty,
        schema: none,
    },
    Tool {
        cmd: "redo",
        group: Group::Write,
        usage: "",
        summary: "redo the last undone step",
        example: "live redo",
        example_args: empty,
        schema: none,
    },
    // Downloadable catalog and reloading
    Tool {
        cmd: "book",
        group: Group::App,
        usage: "--search TEXT [--limit N]",
        summary: "catalog rows: vendor, part, name, has_step, in_library, listed",
        example: "live book --search PBS122 --limit 5",
        example_args: || json!({"search": "PBS122", "limit": 5}),
        schema: || obj(json!({"search": text("words to match"), "limit": int("most rows to return")}), &["search"]),
    },
    Tool {
        cmd: "book-list",
        group: Group::App,
        usage: "",
        summary: "the download list and the Download & import progress",
        example: "live book-list",
        example_args: empty,
        schema: none,
    },
    Tool {
        cmd: "book-add",
        group: Group::App,
        usage: "--parts '[\"PBS122\"]'",
        summary: "put parts with a STEP file on the download list",
        example: "live book-add --parts '[\"PBS122\"]'",
        example_args: || json!({"parts": ["PBS122"]}),
        schema: || obj(json!({"parts": strings("part numbers")}), &["parts"]),
    },
    Tool {
        cmd: "book-remove",
        group: Group::App,
        usage: "--parts '[\"PBS122\"]'",
        summary: "take parts off the download list",
        example: "live book-remove --parts '[\"PBS122\"]'",
        example_args: || json!({"parts": ["PBS122"]}),
        schema: || obj(json!({"parts": strings("part numbers")}), &["parts"]),
    },
    Tool {
        cmd: "book-import",
        group: Group::App,
        usage: "",
        summary: "Download & import the list, as in the Catalog window; returns at once, poll book-list",
        example: "live book-import",
        example_args: empty,
        schema: none,
    },
    Tool {
        cmd: "reload",
        group: Group::App,
        usage: "",
        summary: "re-read catalog files and the part library, then recompute the beam",
        example: "live reload",
        example_args: empty,
        schema: none,
    },
];

pub fn tools() -> &'static [Tool] {
    TOOLS
}

/// `place-bundle` or `place_bundle`.
pub fn find(cmd: &str) -> Option<&'static Tool> {
    let cmd = cmd.replace('_', "-");
    TOOLS.iter().find(|t| t.cmd == cmd)
}

/// The pipe request line for `cmd` (either spelling).
pub fn request_line(cmd: &str, args: Map<String, Value>, doc: Option<&str>) -> String {
    let mut env = Map::new();
    env.insert("v".into(), 1.into());
    env.insert("cmd".into(), cmd.replace('-', "_").into());
    env.insert("args".into(), args.into());
    if let Some(d) = doc {
        env.insert("doc".into(), d.into());
    }
    Value::Object(env).to_string()
}

const PREAMBLE: &str = r#"labsetup-cli live <command> [--pid N] [--doc TOKEN] [--json ARGS] [--arg-name value ...]

Read `status` first and pass its doc token with --doc on writes. Use `features <node>` before `constrain`. Units are mm and degrees.

Flags become args one-to-one, with - turned into _ (--axis-height 75 sends axis_height: 75). A value that is
a JSON number, array or object (--position "[10,0,0]", --to '{"node":4}') is sent as one, anything else as a
string (--to room). String-valued options (part, id, name, text, search, hole, ...) are always sent as text, except
--hole "[col,row]", which goes as an array. --json merges a
JSON object of args over the flags. Prints the reply line; exit 0 if ok, 1 on an error reply, 2 on usage or
connection failure. --pid picks the app when several run."#;

/// The full `labsetup-cli live --help` text.
pub fn help_text() -> String {
    let mut h = String::from(PREAMBLE);
    for (group, title) in [
        (Group::Read, "Reads:"),
        (Group::Write, "Writes (each is one undo step, labelled \"Agent: ...\"):"),
        (
            Group::App,
            "Downloadable catalog and reloading (no undo step, no --doc needed; book-add, book-remove, book-import\nand reload are refused as busy while LabSetup is busy, importing or updating the catalog):",
        ),
    ] {
        h.push_str("\n\n");
        h.push_str(title);
        for t in TOOLS.iter().filter(|t| t.group == group) {
            h.push_str(format!("\n  {:<12} {}", t.cmd, t.usage).trim_end());
            h.push_str(&format!("\n               {}\n               {}", t.summary, t.example));
        }
    }
    h
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::live::protocol;
    use std::collections::BTreeSet;

    const ALL_CMDS: &[&str] = &[
        "status", "list", "platforms", "holes", "part", "catalog", "features", "beams", "bundles", "snapshot",
        "place", "stack", "place_bundle", "constrain", "move", "set_params", "rename", "note", "remove", "undo", "redo",
        "book", "book_list", "book_add", "book_remove", "book_import", "reload",
    ];

    #[test]
    fn every_tool_example_parses_to_its_own_command() {
        for t in tools() {
            let line = request_line(t.cmd, (t.example_args)().as_object().unwrap().clone(), Some("T"));
            let env = protocol::parse_line(&line).unwrap_or_else(|e| panic!("{}: {e:?}", t.cmd));
            assert_eq!(env.request.cmd_name(), t.cmd.replace('-', "_"));
        }
    }

    #[test]
    fn table_covers_every_command() {
        let names: BTreeSet<String> = tools().iter().map(|t| t.cmd.replace('-', "_")).collect();
        assert_eq!(names.len(), tools().len());
        assert_eq!(names, ALL_CMDS.iter().map(|s| s.to_string()).collect());
    }

    #[test]
    fn schemas_are_objects_listing_required_keys_that_examples_have() {
        for t in tools() {
            let s = (t.schema)();
            assert_eq!(s["type"], "object", "{}", t.cmd);
            for k in s["required"].as_array().into_iter().flatten() {
                assert!((t.example_args)().get(k.as_str().unwrap()).is_some(), "{} example lacks {k}", t.cmd);
            }
        }
    }

    #[test]
    fn help_mentions_every_command_and_example() {
        let h = help_text();
        for t in tools() {
            assert!(h.contains(t.example), "{}", t.cmd);
        }
    }

    #[test]
    fn find_accepts_both_spellings() {
        assert_eq!(find("place-bundle").map(|t| t.cmd), Some("place-bundle"));
        assert_eq!(find("place_bundle").map(|t| t.cmd), Some("place-bundle"));
        assert!(find("nope").is_none());
    }
}
