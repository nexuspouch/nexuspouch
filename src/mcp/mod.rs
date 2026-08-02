//! MCP (Model Context Protocol) server for Nexuspouch.
//!
//! stdio transport with newline-delimited JSON-RPC 2.0 (the framing Claude
//! Code / Codex / Cursor use). All tools call the local `/api/v1` surface via
//! [`crate::sdk::Client`], so the MCP server shares auth and semantics with
//! the HTTP API and the Noise store protocol.
//!
//! Run: `nexuspouch mcp --root ./data [--token <scoped-token>]`

use crate::protocol;
use crate::sdk;
use base64::{engine::general_purpose::STANDARD, Engine as _};
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt};

pub const MCP_PROTOCOL_VERSION: &str = "2025-06-18";
pub const SERVER_NAME: &str = "nexuspouch";
pub const SERVER_VERSION: &str = env!("CARGO_PKG_VERSION");
const MAX_READ_TEXT: usize = 512 * 1024;
const MAX_CHUNK: usize = 65536;

#[derive(Debug, Clone)]
pub struct RpcError {
    pub code: i64,
    pub message: String,
    pub data: Option<Value>,
}

impl RpcError {
    fn with_data(mut self, data: Value) -> Self {
        self.data = Some(data);
        self
    }
}

fn rpc_err(code: i64, message: impl Into<String>) -> RpcError {
    RpcError {
        code,
        message: message.into(),
        data: None,
    }
}

fn invalid_params(message: impl Into<String>) -> RpcError {
    rpc_err(-32602, message)
}

/// Map a store/client error string (`"<code>: <message>"`) to an MCP server
/// error carrying the store code as structured data.
fn map_store_err(e: String) -> RpcError {
    let (code, msg) = e.split_once(": ").unwrap_or(("store_error", &e));
    rpc_err(-32001, msg.to_string()).with_data(json!({"code": code}))
}

pub struct McpServer {
    client: sdk::Client,
    device: String,
}

impl McpServer {
    pub fn new(base: impl Into<String>, token: impl Into<String>, device: impl Into<String>) -> Self {
        Self {
            client: sdk::Client::new(base, token),
            device: device.into(),
        }
    }

    pub fn with_agent(
        base: impl Into<String>,
        token: impl Into<String>,
        device: impl Into<String>,
        agent: impl Into<String>,
    ) -> Self {
        Self {
            client: sdk::Client::new(base, token).with_agent(agent),
            device: device.into(),
        }
    }

    /// Process one line of stdin (newline-delimited JSON-RPC).
    /// Returns the response line, or `None` for notifications / empty input.
    pub fn handle_line(&self, line: &str) -> Option<String> {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.to_ascii_lowercase().starts_with("content-length:") {
            return None;
        }
        match serde_json::from_str::<Value>(trimmed) {
            Ok(msg) => self.handle_message(&msg).map(|v| v.to_string()),
            Err(_) => Some(
                error_response(Value::Null, -32700, "parse error").to_string(),
            ),
        }
    }

    pub fn handle_message(&self, msg: &Value) -> Option<Value> {
        if msg
            .get("jsonrpc")
            .and_then(|v| v.as_str())
            .map(|s| s != "2.0")
            .unwrap_or(true)
        {
            return Some(error_response(Value::Null, -32600, "invalid request: jsonrpc must be 2.0"));
        }
        let id = msg.get("id").cloned();
        if id.is_none() {
            // Notifications never produce a response.
            return None;
        }
        let method = match msg.get("method").and_then(|v| v.as_str()) {
            Some(m) => m.to_string(),
            None => {
                return Some(error_response(
                    id.unwrap_or(Value::Null),
                    -32600,
                    "invalid request: missing method",
                ));
            }
        };
        let method = method.as_str();
        let params = msg.get("params").cloned().unwrap_or(Value::Null);
        let result = match method {
            "initialize" => self.initialize(&params),
            "ping" => Ok(json!({})),
            "tools/list" => Ok(self.tools_list()),
            "tools/call" => self.tools_call(&params),
            "resources/list" => Ok(self.resources_list()),
            _ => Err(rpc_err(-32601, format!("method not found: {method}"))),
        };
        Some(match result {
            Ok(res) => json!({"jsonrpc": "2.0", "id": id, "result": res}),
            Err(e) => {
                let mut err = json!({"code": e.code, "message": e.message});
                if let Some(data) = e.data {
                    err["data"] = data;
                }
                json!({"jsonrpc": "2.0", "id": id, "error": err})
            }
        })
    }

    fn initialize(&self, params: &Value) -> Result<Value, RpcError> {
        let client_proto = params.get("protocolVersion").and_then(|v| v.as_str());
        let proto = match client_proto {
            Some(p @ ("2024-11-05" | "2025-03-26" | "2025-06-18")) => p,
            _ => MCP_PROTOCOL_VERSION,
        };
        Ok(json!({
            "protocolVersion": proto,
            "capabilities": {
                "tools": {"listChanged": false},
                "resources": {"subscribe": false},
            },
            "serverInfo": {"name": SERVER_NAME, "version": SERVER_VERSION},
        }))
    }

    fn resources_list(&self) -> Value {
        json!({
            "resources": [{
                "uri": "store://",
                "name": "Nexuspouch store",
                "description": "Device-addressed artifact spaces (artifacts/files/attachments/backups). Use store_read / store_list with store:// URIs.",
                "mimeType": "application/json",
            }]
        })
    }

    fn tools_list(&self) -> Value {
        json!({
            "tools": [
                {
                    "name": "store_write",
                    "description": "Write a file to a Nexuspouch space and return its store:// URI (local-first, mirrored to master).",
                    "inputSchema": {
                        "type": "object",
                        "properties": {
                            "filename": {"type": "string", "description": "Relative path (no leading /, no ..)"},
                            "content": {"type": "string"},
                            "space": {"type": "string", "enum": ["artifacts", "files", "attachments", "backups"], "default": "artifacts"},
                            "task": {"type": "string", "description": "Optional task folder prefix"},
                            "context": {"type": "string", "description": "If set (or to_agent), commit via handoff.create (M3)"},
                            "to_agent": {"type": "string", "description": "Optional handoff recipient agent id"}
                        },
                        "required": ["filename", "content"]
                    }
                },
                {
                    "name": "store_read",
                    "description": "Read a file by store:// URI (text or base64). Truncates above max_bytes; use store_read_chunk for large files.",
                    "inputSchema": {
                        "type": "object",
                        "properties": {
                            "uri": {"type": "string"},
                            "max_bytes": {"type": "integer", "default": 524288}
                        },
                        "required": ["uri"]
                    }
                },
                {
                    "name": "store_read_chunk",
                    "description": "Read a byte range of a file by store:// URI. Returns base64 data and eof flag.",
                    "inputSchema": {
                        "type": "object",
                        "properties": {
                            "uri": {"type": "string"},
                            "offset": {"type": "integer", "default": 0},
                            "length": {"type": "integer", "default": 65536}
                        },
                        "required": ["uri"]
                    }
                },
                {
                    "name": "store_meta",
                    "description": "Resolve a store:// URI and return file/dir metadata (kind, size, sha256, mtime).",
                    "inputSchema": {
                        "type": "object",
                        "properties": {"uri": {"type": "string"}},
                        "required": ["uri"]
                    }
                },
                {
                    "name": "store_list",
                    "description": "List a directory by store:// URI or space/device/path.",
                    "inputSchema": {
                        "type": "object",
                        "properties": {
                            "uri": {"type": "string"},
                            "space": {"type": "string"},
                            "device": {"type": "string"},
                            "path": {"type": "string"}
                        }
                    }
                },
                {
                    "name": "store_search",
                    "description": "Full-text search over artifacts/files (SQLite FTS5, phrase match). Returns uri/path/size/state/snippet/score.",
                    "inputSchema": {
                        "type": "object",
                        "properties": {
                            "q": {"type": "string"},
                            "space": {"type": "string"},
                            "limit": {"type": "integer", "default": 50}
                        },
                        "required": ["q"]
                    }
                },
                {
                    "name": "store_watch",
                    "description": "Return recent store events (commit/delete/handoff). M3 adds durable since/resume.",
                    "inputSchema": {
                        "type": "object",
                        "properties": {
                            "prefix": {"type": "string", "description": "Filter by uri/space prefix"},
                            "since": {"type": "integer", "description": "Event seq cursor (M3)"}
                        }
                    }
                },
                {
                    "name": "store_space",
                    "description": "Report space usage: per-device/space bytes, staging, recycle, volume free/warn.",
                    "inputSchema": {"type": "object", "properties": {}}
                }
            ]
        })
    }

    fn tools_call(&self, params: &Value) -> Result<Value, RpcError> {
        let name = params
            .get("name")
            .and_then(|v| v.as_str())
            .ok_or_else(|| invalid_params("missing tool name"))?;
        let args = params.get("arguments").cloned().unwrap_or(Value::Null);
        let out = match name {
            "store_write" => self.store_write(&args),
            "store_read" => self.store_read(&args),
            "store_read_chunk" => self.store_read_chunk(&args),
            "store_meta" => self.store_meta(&args),
            "store_list" => self.store_list(&args),
            "store_search" => self.store_search(&args),
            "store_watch" => self.store_watch(&args),
            "store_space" => self.store_space(&args),
            _ => return Err(invalid_params(format!("unknown tool: {name}"))),
        };
        match out {
            Ok(v) => Ok(json!({
                "content": [{"type": "text", "text": serde_json::to_string(&v).unwrap_or_else(|_| "{}".into())}],
                "isError": false,
            })),
            Err(e) => {
                let mut err = json!({"code": e.code, "message": e.message});
                if let Some(data) = e.data {
                    err["data"] = data;
                }
                Ok(json!({
                    "content": [{"type": "text", "text": serde_json::to_string(&err).unwrap_or_default()}],
                    "isError": true,
                }))
            }
        }
    }

    fn store_write(&self, args: &Value) -> Result<Value, RpcError> {
        let filename = str_arg(args, "filename")?;
        let content = str_arg(args, "content")?;
        let space = args
            .get("space")
            .and_then(|v| v.as_str())
            .unwrap_or("artifacts");
        if !protocol::is_valid_space(space) {
            return Err(invalid_params(format!("bad space: {space}")));
        }
        let task = args
            .get("task")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty());
        let rel = match task {
            Some(t) => format!("{t}/{filename}"),
            None => filename.to_string(),
        };
        let path = protocol::normalize_path(&rel)
            .map_err(|e| invalid_params(format!("bad path: {e}")))?;

        let bytes = content.as_bytes();
        let size = bytes.len() as i64;
        let sha = hex::encode(Sha256::digest(bytes));

        let mut begin = Map::new();
        begin.insert("space".into(), json!(space));
        begin.insert("path".into(), json!(path));
        begin.insert("size".into(), json!(size));
        begin.insert("sha256".into(), json!(sha));
        let resp = self
            .client
            .store_op("write.begin", begin)
            .map_err(map_store_err)?;
        let upload_id = resp
            .get("upload_id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| rpc_err(-32001, "write.begin: missing upload_id"))?
            .to_string();

        let mut offset = 0usize;
        while offset < bytes.len() {
            let end = (offset + MAX_CHUNK).min(bytes.len());
            let mut chunk = Map::new();
            chunk.insert("upload_id".into(), json!(upload_id));
            chunk.insert("offset".into(), json!(offset as i64));
            chunk.insert("data".into(), json!(STANDARD.encode(&bytes[offset..end])));
            self.client
                .store_op("write.chunk", chunk)
                .map_err(map_store_err)?;
            offset = end;
        }

        let context = args
            .get("context")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty());
        let to_agent = args
            .get("to_agent")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty());
        let uri = format!("store://{space}/{}/{}", self.device, path);

        if context.is_some() || to_agent.is_some() {
            let mut handoff = Map::new();
            handoff.insert("space".into(), json!(space));
            handoff.insert("upload_ids".into(), json!([upload_id]));
            if let Some(c) = context {
                handoff.insert("context".into(), json!(c));
            }
            if let Some(a) = to_agent {
                handoff.insert("to_agent".into(), json!(a));
            }
            let result = self
                .client
                .store_op("handoff.create", handoff)
                .map_err(map_store_err)?;
            return Ok(json!({
                "uri": result.get("handoff_uri").cloned().unwrap_or(json!(uri)),
                "space": space,
                "path": path,
                "size": size,
                "sha256": sha,
                "state": result.get("state").cloned().unwrap_or(json!("published")),
                "handoff": result,
            }));
        }

        let mut commit = Map::new();
        commit.insert("space".into(), json!(space));
        commit.insert("upload_ids".into(), json!([upload_id]));
        let committed = self.client.store_op("commit", commit).map_err(map_store_err)?;

        Ok(json!({
            "uri": uri,
            "space": space,
            "path": path,
            "size": size,
            "sha256": sha,
            "committed": committed,
        }))
    }

    fn store_read(&self, args: &Value) -> Result<Value, RpcError> {
        let uri = str_arg(args, "uri")?;
        let meta = self.client.resolve(uri).map_err(map_store_err)?;
        let size = meta
            .get("meta")
            .and_then(|m| m.get("size"))
            .and_then(|v| v.as_i64())
            .unwrap_or(0);
        let max = args
            .get("max_bytes")
            .and_then(|v| v.as_u64())
            .unwrap_or(MAX_READ_TEXT as u64) as usize;
        let take = (size as usize).min(max.max(1));
        let data = self
            .client
            .read_bytes(uri, 0, take)
            .map_err(map_store_err)?;
        let truncated = (size as usize) > data.len();
        match String::from_utf8(data.clone()) {
            Ok(s) => Ok(json!({
                "uri": uri,
                "size": size,
                "truncated": truncated,
                "encoding": "text",
                "content": s,
            })),
            Err(_) => Ok(json!({
                "uri": uri,
                "size": size,
                "truncated": truncated,
                "encoding": "base64",
                "content": STANDARD.encode(&data),
            })),
        }
    }

    fn store_read_chunk(&self, args: &Value) -> Result<Value, RpcError> {
        let uri = str_arg(args, "uri")?;
        let offset = args.get("offset").and_then(|v| v.as_i64()).unwrap_or(0);
        let length = args
            .get("length")
            .and_then(|v| v.as_i64())
            .unwrap_or(MAX_CHUNK as i64) as usize;
        if offset < 0 || length == 0 || length > MAX_CHUNK {
            return Err(invalid_params("offset >= 0 and 0 < length <= 65536"));
        }
        let data = self
            .client
            .read_bytes(uri, offset, length)
            .map_err(map_store_err)?;
        Ok(json!({
            "data": STANDARD.encode(&data),
            "size": data.len(),
            "eof": data.len() < length,
        }))
    }

    fn store_meta(&self, args: &Value) -> Result<Value, RpcError> {
        let uri = str_arg(args, "uri")?;
        self.client.resolve(uri).map_err(map_store_err)
    }

    fn store_list(&self, args: &Value) -> Result<Value, RpcError> {
        let uri = match str_arg_opt(args, "uri") {
            Some(u) => u.to_string(),
            None => {
                let space = args
                    .get("space")
                    .and_then(|v| v.as_str())
                    .unwrap_or("files");
                let device = args
                    .get("device")
                    .and_then(|v| v.as_str())
                    .unwrap_or(&self.device);
                let path = args.get("path").and_then(|v| v.as_str()).unwrap_or("");
                format!("store://{space}/{device}/{path}")
            }
        };
        self.client.list(&uri).map_err(map_store_err)
    }

    fn store_search(&self, args: &Value) -> Result<Value, RpcError> {
        let q = str_arg(args, "q")?;
        let space = str_arg_opt(args, "space");
        let limit = args
            .get("limit")
            .and_then(|v| v.as_u64())
            .unwrap_or(50) as usize;
        let out = self
            .client
            .search(q, space, None, None, limit)
            .map_err(map_store_err)?;
        Ok(json!({
            "query": q,
            "total": out.get("total").cloned().unwrap_or(json!(0)),
            "results": out.get("results").cloned().unwrap_or(json!([])),
        }))
    }

    fn store_watch(&self, args: &Value) -> Result<Value, RpcError> {
        let prefix = str_arg_opt(args, "prefix");
        let out = self.client.recent_events(100).map_err(map_store_err)?;
        let events = out.get("events").cloned().unwrap_or(json!([]));
        let filtered: Vec<Value> = events
            .as_array()
            .map(|arr| {
                arr.iter()
                    .filter(|ev| match prefix {
                        Some(p) => {
                            let by_uri = ev
                                .get("uri")
                                .and_then(|v| v.as_str())
                                .map(|u| u.starts_with(&p))
                                .unwrap_or(false);
                            let by_space = ev
                                .get("space")
                                .and_then(|v| v.as_str())
                                .map(|s| s.starts_with(&p))
                                .unwrap_or(false);
                            by_uri || by_space
                        }
                        None => true,
                    })
                    .cloned()
                    .collect()
            })
            .unwrap_or_default();
        Ok(json!({
            "events": filtered,
            "note": "M3 adds durable since/resume; current window is the in-memory recent ring",
        }))
    }

    fn store_space(&self, _args: &Value) -> Result<Value, RpcError> {
        self.client.store_op("stats", Map::new()).map_err(map_store_err)
    }
}

fn error_response(id: Value, code: i64, message: &str) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": {"code": code, "message": message},
    })
}

fn str_arg<'a>(args: &'a Value, key: &str) -> Result<&'a str, RpcError> {
    args.get(key)
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .ok_or_else(|| invalid_params(format!("missing or invalid argument: {key}")))
}

fn str_arg_opt<'a>(args: &'a Value, key: &str) -> Option<&'a str> {
    args.get(key)
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
}

/// Run the stdio MCP server until stdin closes.
pub async fn run(
    base: String,
    token: String,
    device: String,
    agent: Option<String>,
) -> Result<(), Box<dyn std::error::Error>> {
    let server = match agent {
        Some(a) => McpServer::with_agent(base, token, device, a),
        None => McpServer::new(base, token, device),
    };
    let stdin = tokio::io::stdin();
    let mut lines = tokio::io::BufReader::new(stdin).lines();
    let stdout = tokio::io::stdout();
    let mut out = tokio::io::BufWriter::new(stdout);
    while let Some(line) = lines.next_line().await? {
        if line.trim().is_empty() {
            continue;
        }
        if line.trim().to_ascii_lowercase().starts_with("content-length:") {
            // Header block: the following blank line is skipped by the loop
            // and the body arrives as the next line.
            continue;
        }
        if let Some(resp) = server.handle_line(&line) {
            out.write_all(resp.as_bytes()).await?;
            out.write_all(b"\n").await?;
            out.flush().await?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::{self, ApiState};
    use crate::events::EventBus;
    use crate::store::Local;
    use axum::Router;
    use std::sync::Arc;

    const DEVICE: &str = "aaaaaaaaaaaaaaaa";

    async fn spawn_api() -> String {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(Local::open(dir.path(), DEVICE).unwrap());
        let events = EventBus::new();
        store.set_event_bus(Arc::clone(&events));
        let auth = crate::AuthConfig::new("", None);
        let state = Arc::new(ApiState {
            store,
            auth,
            device: DEVICE.to_string(),
            events,
            mdns: false,
            agents: Arc::new(crate::agents::AgentRegistry::open(dir.path())),
        });
        let app = Router::new().nest("/api/v1", api::router(state));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(
                listener,
                app.into_make_service_with_connect_info::<std::net::SocketAddr>(),
            )
            .await
            .unwrap();
        });
        format!("http://{addr}")
    }

    fn rpc(server: &McpServer, id: u64, method: &str, params: Value) -> Value {
        let msg = json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params});
        let line = msg.to_string();
        let resp = server.handle_line(&line).unwrap();
        serde_json::from_str(&resp).unwrap()
    }

    fn call_tool(server: &McpServer, name: &str, args: Value) -> Value {
        let params = json!({"name": name, "arguments": args});
        let resp = rpc(server, 1, "tools/call", params);
        assert_eq!(resp.get("error").and_then(|e| e.get("code")).cloned(), None, "{resp}");
        let content = resp["result"]["content"][0]["text"].as_str().unwrap();
        serde_json::from_str(content).unwrap()
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn initialize_and_tools_list() {
        let base = spawn_api().await;
        let server = McpServer::new(base, "", DEVICE);
        let init = rpc(
            &server,
            1,
            "initialize",
            json!({"protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "test"}}),
        );
        assert_eq!(init["result"]["serverInfo"]["name"], "nexuspouch");
        assert_eq!(init["result"]["protocolVersion"], "2025-06-18");

        let list = rpc(&server, 2, "tools/list", json!({}));
        let tools = list["result"]["tools"].as_array().unwrap();
        assert_eq!(tools.len(), 8);
        let names: Vec<&str> = tools
            .iter()
            .map(|t| t["name"].as_str().unwrap())
            .collect();
        for want in ["store_write", "store_read", "store_read_chunk", "store_meta", "store_list", "store_search", "store_watch", "store_space"] {
            assert!(names.contains(&want), "missing tool {want}");
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn write_read_roundtrip() {
        let base = spawn_api().await;
        let server = McpServer::new(base, "", DEVICE);

        let written = call_tool(
            &server,
            "store_write",
            json!({"filename": "out.txt", "content": "hello mcp", "space": "artifacts", "task": "t-1"}),
        );
        let uri = written["uri"].as_str().unwrap().to_string();
        assert_eq!(uri, format!("store://artifacts/{DEVICE}/t-1/out.txt"));
        assert_eq!(written["size"], 9);
        assert_eq!(written["sha256"].as_str().unwrap().len(), 64);

        let read = call_tool(&server, "store_read", json!({"uri": uri}));
        assert_eq!(read["encoding"], "text");
        assert_eq!(read["content"], "hello mcp");
        assert_eq!(read["truncated"], false);

        let chunk = call_tool(&server, "store_read_chunk", json!({"uri": uri, "offset": 0, "length": 4}));
        assert_eq!(STANDARD.encode(b"hell"), chunk["data"].as_str().unwrap());
        assert_eq!(chunk["eof"], false);

        let meta = call_tool(&server, "store_meta", json!({"uri": uri}));
        assert_eq!(meta["meta"]["kind"], "file");
        assert_eq!(meta["meta"]["size"], 9);

        let list = call_tool(
            &server,
            "store_list",
            json!({"uri": format!("store://artifacts/{DEVICE}/t-1/")}),
        );
        assert!(list["entries"]
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e["path"].as_str().unwrap().ends_with("out.txt")));

        let space = call_tool(&server, "store_space", json!({}));
        assert!(space.get("devices").is_some());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn error_mapping_and_validation() {
        let base = spawn_api().await;
        let server = McpServer::new(base, "", DEVICE);

        // Missing file -> isError with store code preserved.
        let params = json!({"name": "store_read", "arguments": {
            "uri": format!("store://files/{DEVICE}/nope.txt")
        }});
        let resp = rpc(&server, 1, "tools/call", params);
        assert_eq!(resp["result"]["isError"], true);
        let text = resp["result"]["content"][0]["text"].as_str().unwrap();
        assert!(text.contains("not_found"), "{text}");
        assert!(text.contains("\"code\":-32001"), "{text}");

        // Unknown tool -> invalid params.
        let resp = rpc(&server, 2, "tools/call", json!({"name": "store_nope", "arguments": {}}));
        assert_eq!(resp["error"]["code"], -32602);

        // Missing required argument.
        let resp = rpc(&server, 3, "tools/call", json!({"name": "store_write", "arguments": {}}));
        assert_eq!(resp["result"]["isError"], true);
        let text = resp["result"]["content"][0]["text"].as_str().unwrap();
        assert!(text.contains("missing or invalid argument"), "{text}");

        // Bad path traversal.
        let resp = rpc(&server, 4, "tools/call", json!({"name": "store_write",
            "arguments": {"filename": "../escape.txt", "content": "x"}}));
        assert_eq!(resp["result"]["isError"], true);
        let text = resp["result"]["content"][0]["text"].as_str().unwrap();
        assert!(text.contains("bad path"), "{text}");
    }

    #[test]
    fn notifications_and_parse_errors() {
        let server = McpServer::new("http://127.0.0.1:1", "", DEVICE);
        let notif = r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#;
        assert!(server.handle_line(notif).is_none());

        let parse = server.handle_line("not json").unwrap();
        let v: Value = serde_json::from_str(&parse).unwrap();
        assert_eq!(v["error"]["code"], -32700);
    }
}
