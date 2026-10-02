//! MCP server (Model Context Protocol) over stdio: JSON-RPC 2.0, one
//! message per line.
//!
//! Every agent command is a tool named `ferrum_<command>` (spaces become
//! underscores, e.g. `ferrum_view_slice`) with its JSON Schema as input
//! schema. A tool result carries the same `ferrum-agent/1` envelope as the
//! command line (as text and as structured content); renders add the PNG as
//! image content. Loaded series stay in memory between calls.

use std::io::{BufRead, Write};

use base64::Engine as _;
use serde_json::{json, Value};

use crate::agent::Agent;
use crate::schema::command_schema;
use crate::study::{generator, VolumeCache};

/// Protocol revisions this server speaks, newest first.
pub const PROTOCOL_VERSIONS: [&str; 3] = ["2025-06-18", "2025-03-26", "2024-11-05"];

/// Guidance sent to the client at initialisation.
pub const INSTRUCTIONS: &str = "FERRUM medical imaging tools. Not a medical device: results are measurements and \
proposals for review by a qualified person. Open a study with ferrum_study_open, look with ferrum_view_slice, and take \
every number from ferrum_probe, ferrum_stats or ferrum_measure_* (never from image grey values). Report values with \
unit and method, never state a diagnosis, flag research-only engines, and end with the items that need review \
(ferrum_review_list).";

/// Tool name of a command.
pub fn tool_name(command: &str) -> String {
    format!("ferrum_{}", command.replace(' ', "_"))
}

/// Command of a tool name.
pub fn command_of(tool: &str) -> Option<&'static str> {
    Agent::commands().find(|c| tool_name(c) == tool)
}

/// An MCP session: the agent and its cache of loaded series.
pub struct McpServer {
    agent: Agent,
    cache: VolumeCache,
}

impl McpServer {
    /// A server running commands under `agent`'s operator configuration.
    pub fn new(agent: Agent) -> Self {
        Self { agent, cache: VolumeCache::default() }
    }

    /// Series currently kept in memory.
    pub fn cached_series(&self) -> usize {
        self.cache.len()
    }

    /// Serves until `input` ends. Each line is one JSON-RPC message.
    pub fn serve(&mut self, input: impl BufRead, mut output: impl Write) -> std::io::Result<()> {
        for line in input.lines() {
            let line = line?;
            if line.trim().is_empty() {
                continue;
            }
            let reply = match serde_json::from_str::<Value>(&line) {
                Ok(msg) => self.handle(&msg),
                Err(e) => Some(error(Value::Null, -32700, &format!("parse error: {e}"))),
            };
            if let Some(r) = reply {
                writeln!(output, "{r}")?;
                output.flush()?;
            }
        }
        Ok(())
    }

    /// Answers one message; notifications get no answer.
    pub fn handle(&mut self, msg: &Value) -> Option<Value> {
        let id = msg.get("id").cloned();
        let method = msg["method"].as_str().unwrap_or_default();
        let Some(id) = id else {
            return None; // notifications (initialized, cancelled, …) need no reply
        };
        if msg["jsonrpc"] != "2.0" || method.is_empty() {
            return Some(error(id, -32600, "invalid request"));
        }
        let params = &msg["params"];
        Some(match method {
            "initialize" => {
                let asked = params["protocolVersion"].as_str().unwrap_or_default();
                let version = PROTOCOL_VERSIONS.iter().find(|v| **v == asked).unwrap_or(&PROTOCOL_VERSIONS[0]);
                result(
                    id,
                    json!({
                        "protocolVersion": version,
                        "capabilities": { "tools": { "listChanged": false } },
                        "serverInfo": { "name": "ferrum", "version": env!("CARGO_PKG_VERSION"), "title": generator() },
                        "instructions": INSTRUCTIONS,
                    }),
                )
            }
            "ping" => result(id, json!({})),
            "tools/list" => result(id, json!({ "tools": tools() })),
            "tools/call" => self.call(id, params),
            other => error(id, -32601, &format!("method not found: {other}")),
        })
    }

    fn call(&mut self, id: Value, params: &Value) -> Value {
        let name = params["name"].as_str().unwrap_or_default();
        let Some(command) = command_of(name) else {
            return error(id, -32602, &format!("unknown tool {name:?}"));
        };
        let args = params.get("arguments").cloned().filter(|a| !a.is_null()).unwrap_or_else(|| json!({}));
        let envelope = self.agent.run_cached(&mut self.cache, command, &args);
        let ok = envelope["ok"] == true;
        let text = serde_json::to_string_pretty(&envelope).unwrap_or_else(|_| envelope.to_string());
        let mut content = vec![json!({ "type": "text", "text": text })];
        if let Some(png) = envelope["data"]["image"].as_str().filter(|_| ok) {
            match std::fs::read(png) {
                Ok(bytes) => content.push(json!({
                    "type": "image",
                    "data": base64::engine::general_purpose::STANDARD.encode(bytes),
                    "mimeType": "image/png",
                })),
                Err(e) => log::warn!("render {png}: {e}"),
            }
        }
        result(id, json!({ "content": content, "structuredContent": envelope, "isError": !ok }))
    }
}

/// Tool descriptions for `tools/list`.
pub fn tools() -> Vec<Value> {
    Agent::commands()
        .filter_map(|c| {
            let (description, schema) = command_schema(c)?;
            let read_only = !matches!(
                c,
                "study open"
                    | "view slice"
                    | "view montage"
                    | "view mpr"
                    | "view volume"
                    | "export bundle"
                    | "annotate add"
                    | "annotate rename"
                    | "annotate delete"
                    | "segment threshold"
                    | "segment interactive"
                    | "segment auto"
                    | "segment rename"
                    | "segment delete"
                    | "review confirm"
                    | "review reject"
            );
            Some(json!({
                "name": tool_name(c),
                "title": c,
                "description": description,
                "inputSchema": schema,
                "annotations": { "readOnlyHint": read_only, "destructiveHint": c.ends_with("delete"), "openWorldHint": false },
            }))
        })
        .collect()
}

fn result(id: Value, result: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "result": result })
}

fn error(id: Value, code: i64, message: &str) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn server() -> McpServer {
        McpServer::new(Agent::default())
    }

    #[test]
    fn handshake_and_tool_list() {
        let mut s = server();
        let init = s
            .handle(&json!({ "jsonrpc": "2.0", "id": 1, "method": "initialize", "params": { "protocolVersion": "2025-03-26" } }))
            .unwrap();
        assert_eq!(init["result"]["protocolVersion"], "2025-03-26");
        assert!(init["result"]["instructions"].as_str().unwrap().contains("Not a medical device"));
        let future = s.handle(&json!({ "jsonrpc": "2.0", "id": 2, "method": "initialize", "params": { "protocolVersion": "2099-01-01" } })).unwrap();
        assert_eq!(future["result"]["protocolVersion"], PROTOCOL_VERSIONS[0]);
        assert_eq!(s.handle(&json!({ "jsonrpc": "2.0", "method": "notifications/initialized" })), None);
        assert_eq!(s.handle(&json!({ "jsonrpc": "2.0", "id": "p", "method": "ping" })).unwrap()["result"], json!({}));
        let list = s.handle(&json!({ "jsonrpc": "2.0", "id": 3, "method": "tools/list" })).unwrap();
        let tools = list["result"]["tools"].as_array().unwrap();
        assert_eq!(tools.len(), Agent::commands().count());
        let slice = tools.iter().find(|t| t["name"] == "ferrum_view_slice").unwrap();
        assert_eq!(slice["inputSchema"]["required"], json!(["workspace", "plane"]));
        assert_eq!(slice["annotations"]["readOnlyHint"], false);
        let probe = tools.iter().find(|t| t["name"] == "ferrum_probe").unwrap();
        assert_eq!(probe["annotations"]["readOnlyHint"], true);
        assert_eq!(command_of("ferrum_measure_distance"), Some("measure distance"));
        assert_eq!(command_of("ferrum_nope"), None);
    }

    #[test]
    fn errors() {
        let mut s = server();
        assert_eq!(
            s.handle(&json!({ "jsonrpc": "2.0", "id": 1, "method": "resources/list" })).unwrap()["error"]["code"],
            -32601
        );
        assert_eq!(s.handle(&json!({ "id": 1, "method": "ping" })).unwrap()["error"]["code"], -32600);
        let unknown = s
            .handle(&json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": { "name": "rm" } }))
            .unwrap();
        assert_eq!(unknown["error"]["code"], -32602);
        let call = json!({ "jsonrpc": "2.0", "id": 9, "method": "tools/call", "params": { "name": "ferrum_study_info", "arguments": {} } });
        let r = s.handle(&call).unwrap();
        assert_eq!(r["result"]["isError"], true, "command errors are tool errors, not protocol errors");
        assert_eq!(r["result"]["structuredContent"]["error"]["code"], "bad_request");
        let mut out = Vec::new();
        s.serve("not json\n\n{\"jsonrpc\":\"2.0\",\"id\":4,\"method\":\"ping\"}\n".as_bytes(), &mut out).unwrap();
        let lines: Vec<Value> =
            String::from_utf8(out).unwrap().lines().map(|l| serde_json::from_str(l).unwrap()).collect();
        assert_eq!((lines[0]["error"]["code"].as_i64(), lines[1]["id"].as_i64()), (Some(-32700), Some(4)));
    }
}
