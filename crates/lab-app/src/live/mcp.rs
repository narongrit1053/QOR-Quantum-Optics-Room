//! `labsetup-cli mcp`: a Model Context Protocol server over stdio (newline-delimited JSON-RPC 2.0).
//! Each live command is a tool; a call becomes the same pipe request line the CLI sends. The
//! server keeps no state between calls: the doc token stays the agent's guard against editing a
//! project it did not read.

use std::io::{self, BufRead, Write};

use serde_json::{json, Map, Value};

use super::tools::{find, request_line, tools, Group};

/// Sends a request line to the app (pid picks the window) and returns its reply line or an error text.
pub type SendFn<'a> = dyn FnMut(Option<u32>, &str) -> Result<String, String> + 'a;

const VERSIONS: &[&str] = &["2025-06-18", "2025-03-26", "2024-11-05"];

pub const INSTRUCTIONS: &str = "LabSetup lays out optical tables (breadboards, posts, mounts, optics) in 3D. \
These tools drive the LabSetup window the user has open, and the user watches each step.\n\
- Call labsetup_status first and pass its doc token as `doc` on every write. A doc_changed error means \
the user switched projects: read status again.\n\
- Read labsetup_platforms and labsetup_holes before placing in a hole (labels like C14), and labsetup_features \
before labsetup_constrain.\n\
- Prefer labsetup_stack (holder + post + mount at a beam height) and labsetup_place_bundle (saved assemblies) \
over placing part by part. Find parts with labsetup_catalog.\n\
- Units are mm and degrees; rotations are [z,y,x]. Each write is one undo step the user can undo.\n\
- Check your work with labsetup_snapshot and labsetup_list.\n\
- If a call says LabSetup isn't running, ask the user to open LabSetup and turn on \
'Allow AI agents' in Settings. If several windows run, pass `pid`.\n\
Rules:\n\
- Do only what the user asked. Don't move, rename or rearrange parts they didn't mention.\n\
- Ask the user before removing parts, and never undo the user's own edits; undo only your own steps.\n\
- Before a change of many parts, say what you will do and wait for a yes.\n\
- Use only parts from the catalog or library (labsetup_catalog, labsetup_book); create a new item only when the user asks for one.\n\
- Never build a look-alike of a real part from boxes or other parts. If it isn't in the library, find it with labsetup_book and import it.\n\
- Ask before downloading catalog parts (labsetup_book_add, labsetup_book_import).\n\
- Never guess catalog ids or node ids: look them up. When a call fails, read its error before trying again.\n\
- You can't save; the user saves. When done, tell the user what changed.";

fn tool_name(cmd: &str) -> String {
    format!("labsetup_{}", cmd.replace('-', "_"))
}

fn tool_list() -> Value {
    let list: Vec<Value> = tools()
        .iter()
        .map(|t| {
            let mut schema = (t.schema)();
            let props = schema["properties"].as_object_mut().expect("tool schemas are objects");
            props.insert("pid".into(), json!({"type": "integer", "description": "LabSetup process id, when several windows run"}));
            if t.group == Group::Write {
                props.insert("doc".into(), json!({"type": "string", "description": "doc token from labsetup_status"}));
            }
            json!({"name": tool_name(t.cmd), "description": format!("{}\nExample arguments: {}", t.summary, (t.example_args)()), "inputSchema": schema})
        })
        .collect();
    json!({ "tools": list })
}

/// The one MCP prompt; clients show it as a slash command (Claude Code: /mcp__labsetup__labsetup).
fn prompt_list() -> Value {
    json!({"prompts": [{
        "name": "labsetup",
        "title": "LabSetup",
        "description": "Build or change the setup open in LabSetup",
        "arguments": [{"name": "request", "description": "What to build or change", "required": false}],
    }]})
}

/// Repeats the instructions in the message, for clients that don't pass the server's on.
fn get_prompt(params: &Value) -> Result<Value, (i64, String)> {
    let name = params["name"].as_str().unwrap_or_default();
    if name != "labsetup" {
        return Err((-32602, format!("unknown prompt '{name}'")));
    }
    let request = params["arguments"]["request"].as_str().map(str::trim).filter(|r| !r.is_empty());
    let task = request.map_or("Then ask me what to build or change.".to_string(), |r| format!("Then do this: {r}"));
    let body = format!("Work on the project open in LabSetup with the labsetup_* tools. Start with labsetup_status. {task}\n\n{INSTRUCTIONS}");
    Ok(json!({"description": "LabSetup", "messages": [{"role": "user", "content": text(&body)}]}))
}

fn text(s: &str) -> Value {
    json!({"type": "text", "text": s})
}

fn call_tool(params: &Value, send: &mut SendFn) -> Result<Value, (i64, String)> {
    let name = params["name"].as_str().unwrap_or_default();
    let tool = name.strip_prefix("labsetup_").and_then(find).ok_or((-32602, format!("unknown tool '{name}'")))?;
    let mut args: Map<String, Value> = params["arguments"].as_object().cloned().unwrap_or_default();
    let pid = args.remove("pid").and_then(|v| v.as_u64()).map(|p| p as u32);
    let doc = args.remove("doc");
    let line = request_line(tool.cmd, args, doc.as_ref().and_then(Value::as_str));
    let reply = match send(pid, &line) {
        Ok(r) => r,
        Err(e) => return Ok(json!({"content": [text(&e)], "isError": true})),
    };
    let reply = reply.trim_end();
    let parsed: Value = serde_json::from_str(reply).unwrap_or(Value::Null);
    if parsed["ok"] != true {
        return Ok(json!({"content": [text(reply)], "isError": true}));
    }
    if tool.cmd == "snapshot" {
        if let Some(Ok(png)) = parsed["result"]["path"].as_str().map(std::fs::read) {
            return Ok(json!({"content": [{"type": "image", "mimeType": "image/png", "data": base64(&png)}, text(reply)]}));
        }
    }
    Ok(json!({"content": [text(reply)]}))
}

/// One JSON-RPC message in, its response out; `None` for notifications.
pub fn handle(msg: &Value, send: &mut SendFn) -> Option<Value> {
    let id = msg.get("id").filter(|v| !v.is_null())?.clone();
    let result = match msg["method"].as_str().unwrap_or_default() {
        "initialize" => {
            let asked = msg["params"]["protocolVersion"].as_str().unwrap_or_default();
            let version = VERSIONS.iter().find(|v| **v == asked).unwrap_or(&VERSIONS[0]);
            Ok(json!({
                "protocolVersion": version,
                "capabilities": {"tools": {}, "prompts": {}},
                "serverInfo": {"name": "labsetup", "version": env!("CARGO_PKG_VERSION")},
                "instructions": INSTRUCTIONS,
            }))
        }
        "ping" => Ok(json!({})),
        "tools/list" => Ok(tool_list()),
        "tools/call" => call_tool(&msg["params"], send),
        "prompts/list" => Ok(prompt_list()),
        "prompts/get" => get_prompt(&msg["params"]),
        other => Err((-32601, format!("method not found: {other}"))),
    };
    Some(match result {
        Ok(r) => json!({"jsonrpc": "2.0", "id": id, "result": r}),
        Err((code, message)) => json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message}}),
    })
}

/// Reads messages line by line until EOF and writes one response line per request.
pub fn serve(input: impl BufRead, mut output: impl Write, send: &mut SendFn) -> io::Result<()> {
    for line in input.lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let reply = match serde_json::from_str::<Value>(&line) {
            Ok(msg) => handle(&msg, send),
            Err(e) => Some(json!({"jsonrpc": "2.0", "id": null, "error": {"code": -32700, "message": format!("parse error: {e}")}})),
        };
        if let Some(r) = reply {
            writeln!(output, "{r}")?;
            output.flush()?;
        }
    }
    Ok(())
}

fn base64(bytes: &[u8]) -> String {
    const A: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for c in bytes.chunks(3) {
        let n = (c[0] as u32) << 16 | (*c.get(1).unwrap_or(&0) as u32) << 8 | *c.get(2).unwrap_or(&0) as u32;
        for i in 0..4 {
            out.push(if i <= c.len() { A[(n >> (18 - 6 * i) & 63) as usize] as char } else { '=' });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::live::tools::{request_line, tools};
    use serde_json::json;

    const NOT_RUNNING: &str = "LabSetup isn't running, or 'Allow AI agents' is off in Settings.";

    fn call(name: &str, args: Value, send: &mut SendFn) -> Value {
        let msg = json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":name,"arguments":args}});
        handle(&msg, send).unwrap()["result"].clone()
    }

    fn ok_send(reply: &'static str) -> impl FnMut(Option<u32>, &str) -> Result<String, String> {
        move |_, _| Ok(reply.to_string())
    }

    #[test]
    fn initialize_returns_instructions_and_tools_capability() {
        let msg = json!({"jsonrpc":"2.0","id":0,"method":"initialize","params":{"protocolVersion":"2025-03-26"}});
        let r = handle(&msg, &mut ok_send("")).unwrap();
        assert_eq!(r["id"], 0);
        assert_eq!(r["result"]["instructions"], INSTRUCTIONS);
        assert!(r["result"]["capabilities"]["tools"].is_object());
        assert_eq!(r["result"]["protocolVersion"], "2025-03-26");
        assert_eq!(r["result"]["serverInfo"]["name"], "labsetup");
        let odd = json!({"jsonrpc":"2.0","id":0,"method":"initialize","params":{"protocolVersion":"1999-01-01"}});
        assert_eq!(handle(&odd, &mut ok_send("")).unwrap()["result"]["protocolVersion"], "2025-06-18");
    }

    #[test]
    fn instructions_carry_the_agent_rules() {
        for rule in ["Rules:", "only what the user asked", "Ask the user before removing", "never undo the user's own", "before downloading", "Use only parts from the catalog or library", "Never build a look-alike", "Never guess catalog ids", "tell the user what changed"] {
            assert!(INSTRUCTIONS.contains(rule), "missing: {rule}");
        }
        assert!(INSTRUCTIONS.len() < 2000, "instructions are sent at every connect; keep them short");
    }

    #[test]
    fn labsetup_prompt_is_listed_as_a_slash_command() {
        let init = handle(&json!({"jsonrpc":"2.0","id":0,"method":"initialize","params":{}}), &mut ok_send("")).unwrap();
        assert!(init["result"]["capabilities"]["prompts"].is_object());
        let r = handle(&json!({"jsonrpc":"2.0","id":1,"method":"prompts/list"}), &mut ok_send("")).unwrap();
        let p = &r["result"]["prompts"][0];
        assert_eq!(p["name"], "labsetup");
        assert_eq!(p["arguments"][0]["name"], "request");
        assert_eq!(p["arguments"][0]["required"], false);
    }

    #[test]
    fn labsetup_prompt_carries_the_request_and_the_rules() {
        let get = |args: Value| {
            let msg = json!({"jsonrpc":"2.0","id":2,"method":"prompts/get","params":{"name":"labsetup","arguments":args}});
            handle(&msg, &mut ok_send("")).unwrap()
        };
        let r = get(json!({"request": "add a PBS at C14"}));
        let text = r["result"]["messages"][0]["content"]["text"].as_str().unwrap().to_string();
        assert_eq!(r["result"]["messages"][0]["role"], "user");
        assert!(text.contains("add a PBS at C14"), "{text}");
        assert!(text.contains("labsetup_status"), "{text}");
        assert!(text.contains(INSTRUCTIONS), "{text}");
        let empty = get(json!({}));
        assert!(empty["result"]["messages"][0]["content"]["text"].as_str().unwrap().contains("ask me what to build or change"));
        let bad = handle(&json!({"jsonrpc":"2.0","id":3,"method":"prompts/get","params":{"name":"nope"}}), &mut ok_send("")).unwrap();
        assert_eq!(bad["error"]["code"], -32602);
    }

    #[test]
    fn tools_list_has_one_tool_per_command_with_pid_and_doc() {
        let r = handle(&json!({"jsonrpc":"2.0","id":2,"method":"tools/list"}), &mut ok_send("")).unwrap();
        let list = r["result"]["tools"].as_array().unwrap();
        assert_eq!(list.len(), tools().len());
        let get = |n: &str| list.iter().find(|t| t["name"] == n).unwrap().clone();
        let place = get("labsetup_place");
        assert!(place["inputSchema"]["properties"]["doc"].is_object());
        assert!(place["inputSchema"]["properties"]["pid"].is_object());
        let status = get("labsetup_status");
        assert!(status["inputSchema"]["properties"]["pid"].is_object());
        assert!(status["inputSchema"]["properties"].get("doc").is_none());
        assert!(list.iter().any(|t| t["name"] == "labsetup_place_bundle"));
    }

    #[test]
    fn descriptions_show_json_args_not_cli_flags() {
        let r = handle(&json!({"jsonrpc":"2.0","id":2,"method":"tools/list"}), &mut ok_send("")).unwrap();
        let place = r["result"]["tools"].as_array().unwrap().iter().find(|t| t["name"] == "labsetup_place").unwrap().clone();
        let d = place["description"].as_str().unwrap();
        assert!(d.contains(r#"Example arguments: {"#), "{d}");
        assert!(!d.contains("--"), "{d}");
    }

    #[test]
    fn call_sends_the_cli_request_line() {
        let mut seen = Vec::new();
        let mut send = |pid: Option<u32>, line: &str| {
            seen.push((pid, line.to_string()));
            Ok(r#"{"ok":true,"doc":"T","result":{}}"#.to_string())
        };
        let r = call("labsetup_move", json!({"node":5,"position":[10,0,0],"doc":"T","pid":7}), &mut send);
        assert_ne!(r["isError"], true);
        let mut args = serde_json::Map::new();
        args.insert("node".into(), json!(5));
        args.insert("position".into(), json!([10, 0, 0]));
        assert_eq!(seen, vec![(Some(7), request_line("move", args, Some("T")))]);
    }

    #[test]
    fn error_reply_sets_is_error() {
        let r = call("labsetup_status", json!({}), &mut ok_send(r#"{"ok":false,"doc":"","error":{"code":"refused","message":"no"}}"#));
        assert_eq!(r["isError"], true);
        assert!(r["content"][0]["text"].as_str().unwrap().contains("refused"));
    }

    #[test]
    fn app_not_running_is_a_tool_error_and_server_continues() {
        let input = concat!(
            r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"labsetup_status","arguments":{}}}"#,
            "\n",
            r#"{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"labsetup_beams","arguments":{}}}"#,
            "\n"
        );
        let mut out = Vec::new();
        serve(input.as_bytes(), &mut out, &mut |_, _| Err(NOT_RUNNING.to_string())).unwrap();
        let lines: Vec<Value> = String::from_utf8(out).unwrap().lines().map(|l| serde_json::from_str(l).unwrap()).collect();
        assert_eq!(lines.len(), 2);
        for l in &lines {
            assert_eq!(l["result"]["isError"], true);
            assert_eq!(l["result"]["content"][0]["text"], NOT_RUNNING);
        }
    }

    #[test]
    fn unknown_tool_is_invalid_params() {
        let msg = json!({"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"labsetup_fly","arguments":{}}});
        assert_eq!(handle(&msg, &mut ok_send("")).unwrap()["error"]["code"], -32602);
    }

    #[test]
    fn snapshot_returns_inline_png() {
        let path = std::env::temp_dir().join(format!("labsetup-mcp-test-{}.png", std::process::id()));
        std::fs::write(&path, [1u8, 2, 3]).unwrap();
        let reply = json!({"ok":true,"doc":"T","result":{"path":path}}).to_string();
        let r = call("labsetup_snapshot", json!({}), &mut |_, _| Ok(reply.clone()));
        std::fs::remove_file(&path).ok();
        assert_eq!(r["content"][0]["type"], "image");
        assert_eq!(r["content"][0]["mimeType"], "image/png");
        assert_eq!(r["content"][0]["data"], "AQID");
        assert_eq!(r["content"][1]["type"], "text");
    }

    #[test]
    fn bad_json_line_gets_parse_error_and_notifications_get_no_reply() {
        let input = "not json\n{\"jsonrpc\":\"2.0\",\"method\":\"notifications/initialized\"}\n{\"jsonrpc\":\"2.0\",\"id\":9,\"method\":\"ping\"}\n";
        let mut out = Vec::new();
        serve(input.as_bytes(), &mut out, &mut ok_send("")).unwrap();
        let lines: Vec<Value> = String::from_utf8(out).unwrap().lines().map(|l| serde_json::from_str(l).unwrap()).collect();
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0]["error"]["code"], -32700);
        assert_eq!(lines[1]["id"], 9);
        assert_eq!(lines[1]["result"], json!({}));
    }

    #[test]
    fn unknown_method_is_method_not_found() {
        let msg = json!({"jsonrpc":"2.0","id":4,"method":"resources/list"});
        assert_eq!(handle(&msg, &mut ok_send("")).unwrap()["error"]["code"], -32601);
    }

    #[test]
    fn base64_matches_rfc4648_vectors() {
        for (i, o) in [("", ""), ("f", "Zg=="), ("fo", "Zm8="), ("foo", "Zm9v"), ("foobar", "Zm9vYmFy")] {
            assert_eq!(base64(i.as_bytes()), o);
        }
    }
}
