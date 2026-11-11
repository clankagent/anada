//! Newline-delimited JSON-RPC MCP adapter. stdout is exclusively protocol output.
use crate::mcp_types::Permission;
use serde_json::{Value, json};
use std::{
    collections::BTreeSet,
    io::{BufRead, Write},
};
const MAX_FRAME: usize = 1_048_576;
const MAX_DISCARD: usize = 8 * MAX_FRAME;
const LATEST: &str = "2025-11-25";
const MODERN: &str = "2026-07-28";
const SUPPORTED: [&str; 5] = [MODERN, LATEST, "2025-06-18", "2025-03-26", "2024-11-05"];
const VERSIONS: [&str; 4] = [LATEST, "2025-06-18", "2025-03-26", "2024-11-05"];

pub trait Broker {
    fn status(&mut self) -> Result<Value, String>;
    fn call(&mut self, name: &str, args: Value) -> Result<Value, String>;
}
struct LocalBroker;
impl Broker for LocalBroker {
    fn status(&mut self) -> Result<Value, String> {
        crate::mcp_session::local_status()
            .and_then(|s| serde_json::to_value(s).map_err(|e| e.to_string()))
    }
    fn call(&mut self, n: &str, a: Value) -> Result<Value, String> {
        crate::mcp_session::call_local(n, a)
    }
}
pub fn run() -> Result<(), String> {
    serve(
        std::io::stdin().lock(),
        std::io::stdout().lock(),
        &mut LocalBroker,
    )
}
fn error(id: Value, code: i32, message: &str) -> Value {
    json!({"jsonrpc":"2.0","id":id,"error":{"code":code,"message":message}})
}
fn result(id: Value, value: Value) -> Value {
    json!({"jsonrpc":"2.0","id":id,"result":value})
}
fn write_response(out: &mut impl Write, v: Value) -> Result<(), String> {
    let mut bytes = serde_json::to_vec(&v).map_err(|e| e.to_string())?;
    if bytes.len() > MAX_FRAME {
        // Single bounded fallback. Never echo an oversized ID or recurse.
        bytes = serde_json::to_vec(&error(
            Value::Null,
            -32603,
            "Response exceeds the 1 MiB limit",
        ))
        .map_err(|e| e.to_string())?;
    }
    out.write_all(&bytes)
        .and_then(|_| out.write_all(b"\n"))
        .and_then(|_| out.flush())
        .map_err(|e| e.to_string())
}
fn server_info() -> Value {
    json!({"name":"anada","version":env!("CARGO_PKG_VERSION")})
}
fn modern_result(mut reply: Value) -> Value {
    if let Some(r) = reply.get_mut("result") {
        r["resultType"] = json!("complete");
        if r.get("tools").is_some() || r.get("supportedVersions").is_some() {
            r["ttlMs"] = json!(0);
            r["cacheScope"] = json!("private");
        }
        r["_meta"] = json!({"io.modelcontextprotocol/serverInfo":server_info()});
    }
    reply
}
fn connection_tool() -> Value {
    json!({"name":"connection_status","description":"Read whether the owner enabled a timed Anada MCP session and its current grants. Cannot enable or extend a session, launch Anada, or start audio.","inputSchema":{"type":"object","properties":{},"additionalProperties":false}})
}
fn status(b: &mut impl Broker) -> Value {
    b.status().unwrap_or_else(
        |_| json!({"active":false,"remaining_seconds":0,"granted":[],"activity":[]}),
    )
}
fn grants(s: &Value) -> BTreeSet<Permission> {
    if s["active"] != true {
        return BTreeSet::new();
    }
    serde_json::from_value(s["granted"].clone()).unwrap_or_default()
}
/// Processes bounded frames; an oversized frame is discarded up to 8 MiB before
/// closing. This bounds recovery memory and prevents an endless oversized frame.
pub fn serve(
    mut input: impl BufRead,
    mut output: impl Write,
    broker: &mut impl Broker,
) -> Result<(), String> {
    let mut initialized = false;
    let mut ready = false;
    loop {
        let mut frame = Vec::new();
        let mut consumed = 0;
        let mut oversized = false;
        let mut ended = false;
        loop {
            let buf = input.fill_buf().map_err(|e| e.to_string())?;
            if buf.is_empty() {
                break;
            }
            let n = buf
                .iter()
                .position(|b| *b == b'\n')
                .map_or(buf.len(), |p| p + 1);
            ended = buf[n - 1] == b'\n';
            consumed += n;
            if consumed > MAX_FRAME {
                oversized = true;
            }
            if !oversized {
                frame.extend_from_slice(&buf[..n]);
            }
            input.consume(n);
            if ended || consumed > MAX_DISCARD {
                break;
            }
        }
        if consumed == 0 {
            return Ok(());
        }
        if oversized {
            write_response(
                &mut output,
                error(Value::Null, -32700, "MCP frame exceeds the 1 MiB limit"),
            )?;
            if !ended {
                return Err("Oversized MCP frame recovery limit reached".into());
            }
            continue;
        }
        let value: Value = match serde_json::from_slice(&frame) {
            Ok(v) => v,
            Err(_) => {
                write_response(&mut output, error(Value::Null, -32700, "Parse error"))?;
                continue;
            }
        };
        let id = value.get("id").cloned();
        let valid_id = id.as_ref().is_none_or(|i| {
            i.as_str().is_some_and(|s| s.len() <= 1024) || i.is_i64() || i.is_u64()
        });
        if !value.is_object()
            || value["jsonrpc"] != "2.0"
            || !value["method"].is_string()
            || !valid_id
            || value.get("params").is_some_and(|p| !p.is_object())
        {
            write_response(
                &mut output,
                error(
                    if valid_id {
                        id.unwrap_or(Value::Null)
                    } else {
                        Value::Null
                    },
                    -32600,
                    "Invalid Request",
                ),
            )?;
            continue;
        }
        let method = value["method"].as_str().unwrap();
        let params = value.get("params").cloned().unwrap_or_else(|| json!({}));
        if id.is_none() {
            if method == "notifications/initialized" && initialized {
                ready = true;
            }
            continue;
        }
        let id = id.unwrap();
        let meta = &params["_meta"];
        let modern = meta
            .get("io.modelcontextprotocol/protocolVersion")
            .is_some()
            || method == "server/discover";
        if modern {
            let version = meta["io.modelcontextprotocol/protocolVersion"].as_str();
            if version.is_none()
                || !meta["io.modelcontextprotocol/clientCapabilities"].is_object()
                || meta
                    .get("io.modelcontextprotocol/clientInfo")
                    .is_some_and(|i| {
                        !i.is_object() || !i["name"].is_string() || !i["version"].is_string()
                    })
            {
                write_response(
                    &mut output,
                    error(
                        id,
                        -32602,
                        "Modern requests require protocolVersion and clientCapabilities in params._meta",
                    ),
                )?;
                continue;
            }
            if version != Some(MODERN) {
                write_response(
                    &mut output,
                    json!({"jsonrpc":"2.0","id":id,"error":{"code":-32022,"message":"Unsupported protocol version","data":{"supported":SUPPORTED,"requested":version}}}),
                )?;
                continue;
            }
        }
        let reply = match method {
            "server/discover" => {
                if params.as_object().unwrap().keys().any(|k| k != "_meta") {
                    error(id, -32602, "server/discover accepts only request metadata")
                } else {
                    result(
                        id,
                        json!({"supportedVersions":SUPPORTED,"capabilities":{"tools":{"listChanged":false}},"instructions":"The owner enables a timed, permission-scoped session in Anada. connection_status is always available. This adapter never starts the app or audio."}),
                    )
                }
            }
            "initialize" => {
                if initialized {
                    error(id, -32600, "Already initialized")
                } else if !params["protocolVersion"].is_string()
                    || !params["capabilities"].is_object()
                    || !params["clientInfo"].is_object()
                {
                    error(
                        id,
                        -32602,
                        "initialize requires protocolVersion, capabilities and clientInfo",
                    )
                } else {
                    initialized = true;
                    let requested = params["protocolVersion"].as_str().unwrap();
                    let version = if VERSIONS.contains(&requested) {
                        requested
                    } else {
                        LATEST
                    };
                    result(
                        id,
                        json!({"protocolVersion":version,"capabilities":{"tools":{"listChanged":false}},"serverInfo":{"name":"anada","version":env!("CARGO_PKG_VERSION")},"instructions":"The Anada owner must enable a timed permission session in the application. This adapter never launches Anada or audio. connection_status is always available. Tools require the active matching grants."}),
                    )
                }
            }
            "ping" => result(id, json!({})),
            _ if !ready && !modern => error(
                id,
                -32602,
                "Complete initialize and notifications/initialized before using tools",
            ),
            "tools/list" => {
                if params
                    .as_object()
                    .unwrap()
                    .keys()
                    .any(|k| k != "cursor" && k != "_meta")
                    || params.get("cursor").is_some()
                {
                    error(id, -32602, "This server has one tool page; omit cursor")
                } else {
                    let s = status(broker);
                    let mut tools = vec![connection_tool()];
                    tools.extend(crate::mcp_tools::catalog(&grants(&s)));
                    result(id, json!({"tools":tools}))
                }
            }
            "tools/call" => {
                if params
                    .as_object()
                    .unwrap()
                    .keys()
                    .any(|k| !matches!(k.as_str(), "name" | "arguments" | "_meta"))
                    || !params["name"].is_string()
                    || params.get("arguments").is_some_and(|a| !a.is_object())
                {
                    error(
                        id,
                        -32602,
                        "tools/call requires a name and object arguments",
                    )
                } else {
                    let name = params["name"].as_str().unwrap();
                    let args = params
                        .get("arguments")
                        .cloned()
                        .unwrap_or_else(|| json!({}));
                    let s = status(broker);
                    let call = if name == "connection_status" {
                        if args.as_object().unwrap().is_empty() {
                            Ok(s)
                        } else {
                            Err("connection_status accepts no arguments".into())
                        }
                    } else if !crate::mcp_tools::catalog(&grants(&s))
                        .iter()
                        .any(|t| t["name"] == name)
                    {
                        Err("Tool is unknown or its permission session is inactive".into())
                    } else {
                        broker.call(name, args)
                    };
                    match call {
                        Ok(v) => result(
                            id,
                            json!({"content":[{"type":"text","text":serde_json::to_string(&v).unwrap_or_default()}],"structuredContent":{"result":v},"isError":false}),
                        ),
                        Err(e) => result(
                            id,
                            json!({"content":[{"type":"text","text":e}],"isError":true}),
                        ),
                    }
                }
            }
            _ => error(id, -32601, "Method not found"),
        };
        write_response(
            &mut output,
            if modern { modern_result(reply) } else { reply },
        )?;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[derive(Default)]
    struct Disabled {
        calls: usize,
    }
    impl Broker for Disabled {
        fn status(&mut self) -> Result<Value, String> {
            Ok(json!({"active":false,"granted":[]}))
        }
        fn call(&mut self, _: &str, _: Value) -> Result<Value, String> {
            self.calls += 1;
            Err("must not call".into())
        }
    }
    fn exchange(lines: &[Value]) -> (Vec<Value>, usize) {
        let input = lines.iter().map(|v| format!("{v}\n")).collect::<String>();
        let mut out = Vec::new();
        let mut broker = Disabled::default();
        serve(input.as_bytes(), &mut out, &mut broker).unwrap();
        (
            String::from_utf8(out)
                .unwrap()
                .lines()
                .map(|l| serde_json::from_str(l).unwrap())
                .collect(),
            broker.calls,
        )
    }
    fn init() -> Value {
        json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"unknown","capabilities":{},"clientInfo":{"name":"test","version":"1"}}})
    }
    #[test]
    fn handshake_disabled_never_calls_host() {
        let (r, c) = exchange(&[
            init(),
            json!({"jsonrpc":"2.0","method":"notifications/initialized"}),
            json!({"jsonrpc":"2.0","id":2,"method":"tools/list"}),
            json!({"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"stop_host","arguments":{}}}),
        ]);
        assert_eq!(r[0]["result"]["protocolVersion"], LATEST);
        assert_eq!(r[1]["result"]["tools"].as_array().unwrap().len(), 1);
        assert_eq!(r[2]["result"]["isError"], true);
        assert_eq!(c, 0);
    }
    #[test]
    fn lifecycle_and_method_errors() {
        let (r, _) = exchange(&[
            json!({"jsonrpc":"2.0","id":0,"method":"tools/list"}),
            init(),
            json!({"jsonrpc":"2.0","method":"notifications/initialized"}),
            json!({"jsonrpc":"2.0","id":"x","method":"unknown"}),
        ]);
        assert_eq!(r[0]["error"]["code"], -32602);
        assert_eq!(r[2]["error"]["code"], -32601);
    }
    #[test]
    fn malformed_and_oversized_frames_recover() {
        let mut input = b"broken\n".to_vec();
        input.extend(vec![b'x'; MAX_FRAME + 1]);
        input.push(b'\n');
        input.extend(format!("{}\n", init()).as_bytes());
        let mut out = vec![];
        serve(&input[..], &mut out, &mut Disabled::default()).unwrap();
        let r: Vec<Value> = String::from_utf8(out)
            .unwrap()
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect();
        assert_eq!(r.len(), 3);
        assert_eq!(r[0]["error"]["code"], -32700);
        assert_eq!(r[1]["error"]["code"], -32700);
        assert!(r[2]["result"].is_object());
    }
}

#[cfg(test)]
mod modern_tests {
    use super::*;
    struct Disabled;
    impl Broker for Disabled {
        fn status(&mut self) -> Result<Value, String> {
            Ok(json!({"active":false,"granted":[]}))
        }
        fn call(&mut self, _: &str, _: Value) -> Result<Value, String> {
            panic!("disabled adapter called backend")
        }
    }
    fn request(id: u64, method: &str, version: &str) -> Value {
        json!({"jsonrpc":"2.0","id":id,"method":method,"params":{"_meta":{"io.modelcontextprotocol/protocolVersion":version,"io.modelcontextprotocol/clientCapabilities":{},"io.modelcontextprotocol/clientInfo":{"name":"test","version":"1"}}}})
    }
    #[test]
    fn modern_discovery_and_inline_tools_require_no_initialize() {
        let input = format!(
            "{}\n{}\n{}\n",
            request(1, "server/discover", MODERN),
            request(2, "tools/list", MODERN),
            request(3, "server/discover", "2099-01-01")
        );
        let mut output = vec![];
        serve(input.as_bytes(), &mut output, &mut Disabled).unwrap();
        let replies: Vec<Value> = String::from_utf8(output)
            .unwrap()
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect();
        assert_eq!(replies[0]["result"]["resultType"], "complete");
        assert_eq!(replies[0]["result"]["supportedVersions"][0], MODERN);
        assert_eq!(replies[1]["result"]["resultType"], "complete");
        assert_eq!(replies[1]["result"]["ttlMs"], 0);
        assert_eq!(replies[1]["result"]["cacheScope"], "private");
        assert_eq!(replies[1]["result"]["tools"].as_array().unwrap().len(), 1);
        assert_eq!(replies[2]["error"]["code"], -32022);
        assert_eq!(replies[2]["error"]["data"]["requested"], "2099-01-01");
    }
    #[test]
    fn modern_missing_capabilities_is_invalid() {
        let mut r = request(1, "server/discover", MODERN);
        r["params"]["_meta"]
            .as_object_mut()
            .unwrap()
            .remove("io.modelcontextprotocol/clientCapabilities");
        let mut output = vec![];
        serve(format!("{r}\n").as_bytes(), &mut output, &mut Disabled).unwrap();
        let r: Value = serde_json::from_slice(&output).unwrap();
        assert_eq!(r["error"]["code"], -32602);
    }
    #[test]
    fn oversized_ids_and_responses_use_single_bounded_fallback() {
        let id = "x".repeat(MAX_FRAME);
        let mut out = vec![];
        write_response(
            &mut out,
            result(json!(id), json!({"huge":"x".repeat(MAX_FRAME)})),
        )
        .unwrap();
        assert!(out.len() < 1024);
        let r: Value = serde_json::from_slice(&out).unwrap();
        assert!(r["id"].is_null());
        assert_eq!(r["error"]["code"], -32603);
        let r = json!({"jsonrpc":"2.0","id":"x".repeat(2048),"method":"ping"});
        let mut out = vec![];
        serve(format!("{r}\n").as_bytes(), &mut out, &mut Disabled).unwrap();
        let r: Value = serde_json::from_slice(&out).unwrap();
        assert!(r["id"].is_null());
        assert_eq!(r["error"]["code"], -32600);
    }
}
