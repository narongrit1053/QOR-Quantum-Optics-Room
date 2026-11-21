//! Wire types for the live control channel: one JSON object per line each way.
//! Units are millimetres and degrees (ZYX Euler `[z, y, x]`).

use lab_core::ConstraintKind;
use serde::{Deserialize, Deserializer};
use serde_json::{json, Map, Value};
use std::path::PathBuf;

pub const MAX_LINE: usize = 1 << 20;

#[derive(Debug, PartialEq)]
pub struct Envelope {
    pub v: u32,
    pub doc: Option<String>,
    pub request: Request,
}

#[derive(Debug, PartialEq)]
pub enum Request {
    Read(ReadRequest),
    Write(WriteRequest),
}

#[derive(Debug, PartialEq, Deserialize)]
#[serde(tag = "cmd", content = "args", rename_all = "snake_case")]
pub enum ReadRequest {
    Status,
    List { platform: Option<u64> },
    Platforms,
    Holes { platform: u64 },
    Part { id: String },
    Catalog { search: String, limit: Option<usize> },
    Features { node: u64 },
    Beams,
    Snapshot { path: Option<PathBuf>, width: Option<u32>, height: Option<u32> },
}

#[derive(Debug, PartialEq, Deserialize)]
#[serde(tag = "cmd", content = "args", rename_all = "snake_case")]
pub enum WriteRequest {
    Place(PlaceArgs),
    Stack(StackArgs),
    Constrain(ConstrainArgs),
    Move { node: u64, position: Option<[f64; 3]>, rotation: Option<[f64; 3]> },
    SetParams { node: u64, params: Map<String, Value> },
    Rename { node: u64, name: String },
    Note { node: u64, text: String },
    Remove { nodes: Vec<u64> },
    Undo,
    Redo,
}

#[derive(Debug, PartialEq, Deserialize)]
pub struct PlaceArgs {
    pub part: String,
    #[serde(flatten)]
    pub at: PlaceAt,
    pub name: Option<String>,
    pub rotate_z: Option<f64>,
}

/// Untagged: the variant is told apart by which keys are present.
#[derive(Debug, PartialEq, Deserialize)]
#[serde(untagged)]
pub enum PlaceAt {
    Hole { platform: u64, hole: HoleArg },
    Mate { on: u64, conn: String, parent_conn: String, slide: Option<f64>, twist: Option<f64> },
    Free { parent: Option<ParentArg>, position: [f64; 3], rotation: Option<[f64; 3]> },
}

#[derive(Debug, PartialEq, Deserialize)]
#[serde(untagged)]
pub enum HoleArg {
    Label(String),
    Index([i32; 2]),
}

/// `"room"`, `{"platform":1}` or `{"node":5}`.
#[derive(Debug, PartialEq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ParentArg {
    Room,
    Platform(u64),
    Node(u64),
}

#[derive(Debug, PartialEq, Deserialize)]
pub struct StackArgs {
    pub platform: u64,
    pub hole: HoleArg,
    pub holder: String,
    pub post: String,
    pub mount: String,
    pub base: Option<String>,
    pub optic: Option<String>,
    pub axis_height: Option<f64>,
    pub slide: Option<f64>,
    pub rotate_z: Option<f64>,
}

#[derive(Debug, PartialEq, Deserialize)]
pub struct ConstrainArgs {
    pub node: u64,
    pub to: ParentArg,
    pub constraints: Vec<ConstraintArg>,
}

#[derive(Debug, PartialEq, Deserialize)]
pub struct ConstraintArg {
    pub child: String,
    pub parent: String,
    #[serde(default, deserialize_with = "kind_lower")]
    pub kind: Option<ConstraintKind>,
    pub offset: Option<f64>,
    pub angle: Option<f64>,
    pub flip: Option<bool>,
    pub lock_rotation: Option<bool>,
}

/// The wire spells constraint kinds in lower case; `lab_core` serialises PascalCase.
fn kind_lower<'de, D: Deserializer<'de>>(d: D) -> Result<Option<ConstraintKind>, D::Error> {
    Option::<String>::deserialize(d)?
        .map(|s| match s.as_str() {
            "mate" => Ok(ConstraintKind::Mate),
            "flush" => Ok(ConstraintKind::Flush),
            "concentric" => Ok(ConstraintKind::Concentric),
            "angle" => Ok(ConstraintKind::Angle),
            _ => Err(serde::de::Error::custom(format!(
                "unknown constraint kind `{s}`, expected mate, flush, concentric or angle"
            ))),
        })
        .transpose()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ErrorCode {
    BadRequest,
    NotFound,
    Refused,
    Busy,
    DocChanged,
    Disabled,
}

impl ErrorCode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::BadRequest => "bad_request",
            Self::NotFound => "not_found",
            Self::Refused => "refused",
            Self::Busy => "busy",
            Self::DocChanged => "doc_changed",
            Self::Disabled => "disabled",
        }
    }
}

#[derive(Debug, PartialEq)]
pub struct LiveError {
    pub code: ErrorCode,
    pub message: String,
}

impl LiveError {
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        Self { code, message: message.into() }
    }
    fn bad(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::BadRequest, message)
    }
}

/// Deserialize `{cmd, args}`. Unit commands may reject an `args` key and struct ones need
/// it, so when args is absent or empty try both shapes.
fn decode<T: for<'a> Deserialize<'a>>(cmd: &Value, args: Option<&Value>) -> Result<T, serde_json::Error> {
    let with = |a: Option<Value>| {
        let mut m = Map::new();
        m.insert("cmd".into(), cmd.clone());
        if let Some(a) = a {
            m.insert("args".into(), a);
        }
        serde_json::from_value::<T>(Value::Object(m))
    };
    let empty = args.map_or(true, |a| a.as_object().is_some_and(|o| o.is_empty()));
    match with(args.cloned()) {
        Err(_) if empty => with(None).or_else(|_| with(Some(json!({})))),
        r => r,
    }
}

pub fn parse_line(line: &str) -> Result<Envelope, LiveError> {
    let val: Value = serde_json::from_str(line).map_err(|e| LiveError::bad(format!("invalid JSON: {e}")))?;
    let obj = val.as_object().ok_or_else(|| LiveError::bad("request must be a JSON object"))?;
    if obj.get("v").and_then(Value::as_u64) != Some(1) {
        return Err(LiveError::bad("unsupported protocol version: field \"v\" must be 1"));
    }
    let doc = match obj.get("doc") {
        None | Some(Value::Null) => None,
        Some(Value::String(s)) => Some(s.clone()),
        Some(_) => return Err(LiveError::bad("field \"doc\" must be a string")),
    };
    let cmd = obj.get("cmd").ok_or_else(|| LiveError::bad("missing field \"cmd\""))?;
    let args = obj.get("args");
    let request = match decode::<ReadRequest>(cmd, args) {
        Ok(r) => Request::Read(r),
        Err(re) => match decode::<WriteRequest>(cmd, args) {
            Ok(w) => Request::Write(w),
            // Each enum rejects the other's commands as unknown; report the one that knew it.
            Err(we) => {
                let unknown = |e: &serde_json::Error| e.to_string().contains("unknown variant");
                let e = if unknown(&we) && !unknown(&re) { re } else { we };
                return Err(LiveError::bad(e.to_string()));
            }
        },
    };
    Ok(Envelope { v: 1, doc, request })
}

// Built by hand because serde_json's map sorts keys; the wire order is ok, doc, result/error.
pub fn ok_line(doc: &str, result: Value) -> String {
    format!("{{\"ok\":true,\"doc\":{},\"result\":{result}}}\n", Value::from(doc))
}

pub fn err_line(doc: &str, e: &LiveError) -> String {
    let err = json!({"code": e.code.as_str(), "message": e.message});
    format!("{{\"ok\":false,\"doc\":{},\"error\":{err}}}\n", Value::from(doc))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(s: &str) -> Request {
        parse_line(s).unwrap_or_else(|e| panic!("{s}: {e:?}")).request
    }
    fn r(x: ReadRequest) -> Request {
        Request::Read(x)
    }
    fn w(x: WriteRequest) -> Request {
        Request::Write(x)
    }

    #[test]
    fn parses_every_command() {
        let env = parse_line(r#"{"v":1,"cmd":"status","doc":"d1"}"#).unwrap();
        assert_eq!((env.v, env.doc.as_deref()), (1, Some("d1")));
        assert_eq!(env.request, r(ReadRequest::Status));
        assert_eq!(p(r#"{"v":1,"cmd":"list"}"#), r(ReadRequest::List { platform: None }));
        assert_eq!(p(r#"{"v":1,"cmd":"list","args":{"platform":2}}"#), r(ReadRequest::List { platform: Some(2) }));
        assert_eq!(p(r#"{"v":1,"cmd":"platforms","args":{}}"#), r(ReadRequest::Platforms));
        assert_eq!(p(r#"{"v":1,"cmd":"holes","args":{"platform":1}}"#), r(ReadRequest::Holes { platform: 1 }));
        assert_eq!(p(r#"{"v":1,"cmd":"part","args":{"id":"KM100"}}"#), r(ReadRequest::Part { id: "KM100".into() }));
        assert_eq!(
            p(r#"{"v":1,"cmd":"catalog","args":{"search":"mirror","limit":5}}"#),
            r(ReadRequest::Catalog { search: "mirror".into(), limit: Some(5) })
        );
        assert_eq!(p(r#"{"v":1,"cmd":"features","args":{"node":3}}"#), r(ReadRequest::Features { node: 3 }));
        assert_eq!(p(r#"{"v":1,"cmd":"beams"}"#), r(ReadRequest::Beams));
        assert_eq!(
            p(r#"{"v":1,"cmd":"snapshot","args":{"path":"a.png","width":640}}"#),
            r(ReadRequest::Snapshot { path: Some("a.png".into()), width: Some(640), height: None })
        );

        let place = |at, name: Option<&str>| {
            w(WriteRequest::Place(PlaceArgs { part: "x".into(), at, name: name.map(Into::into), rotate_z: None }))
        };
        assert_eq!(
            p(r#"{"v":1,"cmd":"place","args":{"part":"x","platform":1,"hole":"C14"}}"#),
            place(PlaceAt::Hole { platform: 1, hole: HoleArg::Label("C14".into()) }, None)
        );
        assert_eq!(
            p(r#"{"v":1,"cmd":"place","args":{"part":"x","platform":1,"hole":[13,2],"name":"m"}}"#),
            place(PlaceAt::Hole { platform: 1, hole: HoleArg::Index([13, 2]) }, Some("m"))
        );
        assert_eq!(
            p(r#"{"v":1,"cmd":"place","args":{"part":"x","on":4,"conn":"a","parent_conn":"b","slide":1.5}}"#),
            place(
                PlaceAt::Mate { on: 4, conn: "a".into(), parent_conn: "b".into(), slide: Some(1.5), twist: None },
                None
            )
        );
        assert_eq!(
            p(r#"{"v":1,"cmd":"place","args":{"part":"x","parent":{"node":5},"position":[1,2,3],"rotation":[0,0,90]}}"#),
            place(
                PlaceAt::Free {
                    parent: Some(ParentArg::Node(5)),
                    position: [1.0, 2.0, 3.0],
                    rotation: Some([0.0, 0.0, 90.0])
                },
                None
            )
        );
        assert_eq!(
            p(r#"{"v":1,"cmd":"place","args":{"part":"x","position":[0,0,0]}}"#),
            place(PlaceAt::Free { parent: None, position: [0.0; 3], rotation: None }, None)
        );
        assert_eq!(
            p(r#"{"v":1,"cmd":"stack","args":{"platform":1,"hole":"A1","holder":"h","post":"p","mount":"m","axis_height":50}}"#),
            w(WriteRequest::Stack(StackArgs {
                platform: 1,
                hole: HoleArg::Label("A1".into()),
                holder: "h".into(),
                post: "p".into(),
                mount: "m".into(),
                base: None,
                optic: None,
                axis_height: Some(50.0),
                slide: None,
                rotate_z: None
            }))
        );
        let c = |kind| ConstraintArg {
            child: "c".into(),
            parent: "p".into(),
            kind,
            offset: None,
            angle: None,
            flip: None,
            lock_rotation: None,
        };
        for (to, want) in [
            (r#""room""#, ParentArg::Room),
            (r#"{"platform":1}"#, ParentArg::Platform(1)),
            (r#"{"node":5}"#, ParentArg::Node(5)),
        ] {
            let line = format!(
                r#"{{"v":1,"cmd":"constrain","args":{{"node":2,"to":{to},"constraints":[{{"child":"c","parent":"p"}},{{"child":"c","parent":"p","kind":"flush"}}]}}}}"#
            );
            assert_eq!(
                p(&line),
                w(WriteRequest::Constrain(ConstrainArgs {
                    node: 2,
                    to: want,
                    constraints: vec![c(None), c(Some(ConstraintKind::Flush))]
                }))
            );
        }
        assert_eq!(
            p(r#"{"v":1,"cmd":"move","args":{"node":1,"position":[1,2,3]}}"#),
            w(WriteRequest::Move { node: 1, position: Some([1.0, 2.0, 3.0]), rotation: None })
        );
        let params = serde_json::from_str::<Map<String, Value>>(r#"{"k":1}"#).unwrap();
        assert_eq!(
            p(r#"{"v":1,"cmd":"set_params","args":{"node":1,"params":{"k":1}}}"#),
            w(WriteRequest::SetParams { node: 1, params })
        );
        assert_eq!(
            p(r#"{"v":1,"cmd":"rename","args":{"node":1,"name":"n"}}"#),
            w(WriteRequest::Rename { node: 1, name: "n".into() })
        );
        assert_eq!(
            p(r#"{"v":1,"cmd":"note","args":{"node":1,"text":"t"}}"#),
            w(WriteRequest::Note { node: 1, text: "t".into() })
        );
        assert_eq!(p(r#"{"v":1,"cmd":"remove","args":{"nodes":[1,2]}}"#), w(WriteRequest::Remove { nodes: vec![1, 2] }));
        assert_eq!(p(r#"{"v":1,"cmd":"undo"}"#), w(WriteRequest::Undo));
        assert_eq!(p(r#"{"v":1,"cmd":"redo"}"#), w(WriteRequest::Redo));
    }

    #[test]
    fn rejects_bad_requests() {
        for (line, needle) in [
            ("", ""),
            ("{", ""),
            (r#"{"v":2,"cmd":"status"}"#, "v"),
            (r#"{"v":1,"cmd":"frobnicate"}"#, "frobnicate"),
            (r#"{"v":1,"cmd":"rename","args":{"node":1}}"#, "name"),
            (
                r#"{"v":1,"cmd":"constrain","args":{"node":1,"to":"room","constraints":[{"child":"a","parent":"b","kind":"weld"}]}}"#,
                "weld",
            ),
        ] {
            let e = parse_line(line).unwrap_err();
            assert_eq!(e.code, ErrorCode::BadRequest, "{line}");
            assert!(e.message.contains(needle), "{line}: {}", e.message);
        }
    }

    #[test]
    fn response_lines() {
        assert_eq!(ok_line("d1", json!({"x":1})), "{\"ok\":true,\"doc\":\"d1\",\"result\":{\"x\":1}}\n");
        let l = err_line("d1", &LiveError::new(ErrorCode::NotFound, "no node 9"));
        assert!(l.contains("\"code\":\"not_found\"") && l.ends_with('\n'), "{l}");
    }
}
