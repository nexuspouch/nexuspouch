//! Session transcript format adapters (SESSION_HISTORY_DESIGN §3).
//!
//! Normalize Claude Code / Codex / ACP / generic JSONL into unified turns,
//! then build FTS body text and ~1k-token embedding chunks.

use serde_json::Value;

/// Approximate chars per embedding chunk (~1000 tokens for CJK/Latin mix).
const CHUNK_CHARS: usize = 3500;
const MAX_CHUNKS: usize = 32;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionTurn {
    pub agent: String,
    pub session_id: String,
    pub seq: u64,
    pub ts_ms: i64,
    pub role: String,
    pub content: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AdapterKind {
    Acp,
    Claude,
    Codex,
    Generic,
}

#[derive(Debug, Clone)]
pub struct AdaptResult {
    pub kind: AdapterKind,
    pub turns: Vec<SessionTurn>,
    /// Plain text for FTS (`role: content` lines).
    pub fts_body: String,
    /// Embedding chunks (message-aware packing).
    pub chunks: Vec<String>,
}

/// Normalize a sessions-space JSONL body using path hints (`agent/session.jsonl`).
pub fn adapt_transcript(raw: &str, path_hint: &str) -> AdaptResult {
    let agent = agent_from_path(path_hint);
    let session_id = session_from_path(path_hint);
    let kind = detect_kind(raw);
    let turns = match kind {
        AdapterKind::Acp => parse_acp(raw, &agent, &session_id),
        AdapterKind::Claude => parse_claude(raw, &agent, &session_id),
        AdapterKind::Codex => parse_codex(raw, &agent, &session_id),
        AdapterKind::Generic => parse_generic(raw, &agent, &session_id),
    };
    let fts_body = turns_to_fts(&turns);
    let chunks = chunk_turns(&turns);
    AdaptResult {
        kind,
        turns,
        fts_body,
        chunks,
    }
}

fn agent_from_path(path: &str) -> String {
    let norm = path.replace('\\', "/");
    let mut parts = norm.split('/').filter(|s| !s.is_empty());
    parts
        .next()
        .unwrap_or("unknown")
        .chars()
        .take(64)
        .collect()
}

fn session_from_path(path: &str) -> String {
    let norm = path.replace('\\', "/");
    let name = norm.rsplit('/').next().unwrap_or("session");
    name.trim_end_matches(".jsonl")
        .chars()
        .take(128)
        .collect()
}

fn detect_kind(raw: &str) -> AdapterKind {
    let mut saw_claude = 0u32;
    let mut saw_codex = 0u32;
    let mut saw_acp = 0u32;
    for line in raw.lines().take(40) {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Ok(v) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        if v.get("role").and_then(|r| r.as_str()).is_some()
            && v.get("content").is_some()
            && v.get("session_id").is_some()
        {
            saw_acp += 1;
        }
        let ty = v.get("type").and_then(|t| t.as_str()).unwrap_or("");
        if matches!(ty, "user" | "assistant") && v.get("message").is_some() {
            saw_claude += 1;
        }
        if ty == "response_item"
            || (ty == "event_msg" && v.get("payload").is_some())
            || v.pointer("/payload/type").and_then(|t| t.as_str()) == Some("message")
        {
            saw_codex += 1;
        }
    }
    if saw_acp > 0 && saw_acp >= saw_claude && saw_acp >= saw_codex {
        return AdapterKind::Acp;
    }
    if saw_claude >= saw_codex && saw_claude > 0 {
        return AdapterKind::Claude;
    }
    if saw_codex > 0 {
        return AdapterKind::Codex;
    }
    AdapterKind::Generic
}

fn parse_acp(raw: &str, agent: &str, session_id: &str) -> Vec<SessionTurn> {
    let mut out = Vec::new();
    let mut seq = 0u64;
    for line in raw.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Ok(v) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        let role = v
            .get("role")
            .and_then(|r| r.as_str())
            .unwrap_or("")
            .to_string();
        if !matches!(role.as_str(), "user" | "assistant" | "tool" | "agent") {
            continue;
        }
        let content = text_value(v.get("content")).unwrap_or_default();
        if content.trim().is_empty() {
            continue;
        }
        seq += 1;
        let role = if role == "agent" {
            "assistant".into()
        } else {
            role
        };
        out.push(SessionTurn {
            agent: v
                .get("agent")
                .and_then(|a| a.as_str())
                .unwrap_or(agent)
                .to_string(),
            session_id: v
                .get("session_id")
                .and_then(|s| s.as_str())
                .unwrap_or(session_id)
                .to_string(),
            seq: v.get("seq").and_then(|s| s.as_u64()).unwrap_or(seq),
            ts_ms: v.get("ts_ms").and_then(|t| t.as_i64()).unwrap_or(0),
            role,
            content: content.trim().to_string(),
        });
    }
    out
}

fn parse_claude(raw: &str, agent: &str, session_id: &str) -> Vec<SessionTurn> {
    let mut out = Vec::new();
    let mut seq = 0u64;
    for line in raw.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Ok(v) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        let ty = v.get("type").and_then(|t| t.as_str()).unwrap_or("");
        if !matches!(ty, "user" | "assistant") {
            continue;
        }
        let blocks = v
            .pointer("/message/content")
            .or_else(|| v.get("content"));
        let content = text_value(blocks).unwrap_or_default();
        if content.trim().is_empty() {
            continue;
        }
        seq += 1;
        let role = if ty == "user" {
            "user"
        } else {
            "assistant"
        };
        out.push(SessionTurn {
            agent: agent.to_string(),
            session_id: session_id.to_string(),
            seq,
            ts_ms: parse_ts_ms(v.get("timestamp")),
            role: role.into(),
            content: content.trim().to_string(),
        });
    }
    out
}

fn parse_codex(raw: &str, agent: &str, session_id: &str) -> Vec<SessionTurn> {
    let mut out = Vec::new();
    let mut seq = 0u64;
    for line in raw.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Ok(v) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        let ty = v.get("type").and_then(|t| t.as_str()).unwrap_or("");
        // response_item → payload.message
        if ty == "response_item" {
            let payload = v.get("payload").cloned().unwrap_or(Value::Null);
            let ptype = payload.get("type").and_then(|t| t.as_str()).unwrap_or("");
            if ptype == "message" {
                let role = payload
                    .get("role")
                    .and_then(|r| r.as_str())
                    .unwrap_or("");
                if !matches!(role, "user" | "assistant") {
                    continue;
                }
                let content = text_value(payload.get("content")).unwrap_or_default();
                if content.trim().is_empty() {
                    continue;
                }
                seq += 1;
                out.push(SessionTurn {
                    agent: agent.to_string(),
                    session_id: session_id.to_string(),
                    seq,
                    ts_ms: parse_ts_ms(v.get("timestamp")),
                    role: role.into(),
                    content: content.trim().to_string(),
                });
            }
            continue;
        }
        // event_msg task_complete may carry last_agent_message
        if ty == "event_msg" {
            let payload = v.get("payload").cloned().unwrap_or(Value::Null);
            if let Some(msg) = payload
                .get("last_agent_message")
                .and_then(|m| m.as_str())
            {
                if !msg.trim().is_empty() {
                    seq += 1;
                    out.push(SessionTurn {
                        agent: agent.to_string(),
                        session_id: session_id.to_string(),
                        seq,
                        ts_ms: parse_ts_ms(v.get("timestamp")),
                        role: "assistant".into(),
                        content: msg.trim().to_string(),
                    });
                }
            }
        }
    }
    out
}

fn parse_generic(raw: &str, agent: &str, session_id: &str) -> Vec<SessionTurn> {
    let mut out = Vec::new();
    let mut seq = 0u64;
    for line in raw.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Ok(v) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        let role = v
            .get("role")
            .or_else(|| v.get("type"))
            .and_then(|r| r.as_str())
            .unwrap_or("");
        let role = match role {
            "user" | "human" => "user",
            "assistant" | "agent" | "ai" => "assistant",
            "tool" | "function" => "tool",
            _ => continue,
        };
        let content = text_value(v.get("content"))
            .or_else(|| text_value(v.get("text")))
            .or_else(|| text_value(v.get("message")))
            .unwrap_or_default();
        if content.trim().is_empty() {
            continue;
        }
        seq += 1;
        out.push(SessionTurn {
            agent: agent.to_string(),
            session_id: session_id.to_string(),
            seq,
            ts_ms: parse_ts_ms(v.get("timestamp").or_else(|| v.get("ts_ms"))),
            role: role.into(),
            content: content.trim().to_string(),
        });
    }
    out
}

fn text_value(v: Option<&Value>) -> Option<String> {
    let v = v?;
    match v {
        Value::String(s) => Some(s.clone()),
        Value::Array(arr) => {
            let mut parts = Vec::new();
            for item in arr {
                if let Some(s) = item.as_str() {
                    parts.push(s.to_string());
                    continue;
                }
                if let Some(t) = item.get("text").and_then(|t| t.as_str()) {
                    parts.push(t.to_string());
                    continue;
                }
                if let Some(t) = item.get("content").and_then(|t| t.as_str()) {
                    parts.push(t.to_string());
                }
            }
            if parts.is_empty() {
                None
            } else {
                Some(parts.join("\n"))
            }
        }
        Value::Object(map) => map
            .get("text")
            .and_then(|t| t.as_str())
            .map(|s| s.to_string())
            .or_else(|| {
                map.get("content")
                    .and_then(|c| text_value(Some(c)))
            }),
        _ => None,
    }
}

fn parse_ts_ms(v: Option<&Value>) -> i64 {
    let Some(v) = v else {
        return 0;
    };
    if let Some(n) = v.as_i64() {
        return if n < 1_000_000_000_000 { n * 1000 } else { n };
    }
    if let Some(n) = v.as_u64() {
        let n = n as i64;
        return if n < 1_000_000_000_000 { n * 1000 } else { n };
    }
    if let Some(s) = v.as_str() {
        if let Ok(n) = s.parse::<i64>() {
            return if n < 1_000_000_000_000 { n * 1000 } else { n };
        }
        // Best-effort ISO: keep 0 if chrono parse unavailable; callers don't require it.
        let _ = s;
    }
    0
}

fn turns_to_fts(turns: &[SessionTurn]) -> String {
    let mut out = String::new();
    for t in turns {
        if !out.is_empty() {
            out.push('\n');
        }
        out.push_str(&t.role);
        out.push_str(": ");
        out.push_str(&t.content);
    }
    out
}

/// Pack turns into embedding chunks without splitting mid-message when possible.
pub fn chunk_turns(turns: &[SessionTurn]) -> Vec<String> {
    let mut chunks = Vec::new();
    let mut buf = String::new();
    for t in turns {
        let piece = format!("{}: {}\n", t.role, t.content);
        if !buf.is_empty() && buf.len() + piece.len() > CHUNK_CHARS {
            chunks.push(std::mem::take(&mut buf));
            if chunks.len() >= MAX_CHUNKS {
                break;
            }
        }
        if piece.len() > CHUNK_CHARS {
            // Oversized single turn: hard-split.
            let mut rest = piece.as_str();
            while !rest.is_empty() && chunks.len() < MAX_CHUNKS {
                let mut end = CHUNK_CHARS.min(rest.len());
                while end < rest.len() && !rest.is_char_boundary(end) {
                    end += 1;
                }
                chunks.push(rest[..end].to_string());
                rest = &rest[end..];
            }
            buf.clear();
            continue;
        }
        buf.push_str(&piece);
    }
    if !buf.trim().is_empty() && chunks.len() < MAX_CHUNKS {
        chunks.push(buf);
    }
    if chunks.is_empty() {
        // Fallback: raw packing already empty.
    }
    chunks
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;

    fn fixtures() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("docs/storage_fixtures")
    }

    #[test]
    fn adapt_acp_fixture() {
        let raw = fs::read_to_string(fixtures().join("session_acp.jsonl")).unwrap();
        let r = adapt_transcript(&raw, "acp-proxy/sess-1.jsonl");
        assert_eq!(r.kind, AdapterKind::Acp);
        assert_eq!(r.turns.len(), 2);
        assert!(r.fts_body.contains("hello"));
        assert!(!r.chunks.is_empty());
    }

    #[test]
    fn adapt_claude_fixture() {
        let raw = fs::read_to_string(fixtures().join("session_claude.jsonl")).unwrap();
        let r = adapt_transcript(&raw, "claude-code/abc.jsonl");
        assert_eq!(r.kind, AdapterKind::Claude);
        assert!(r.turns.iter().any(|t| t.role == "user"));
        assert!(r.turns.iter().any(|t| t.role == "assistant"));
        assert!(r.fts_body.contains("deploy"));
    }

    #[test]
    fn adapt_codex_fixture() {
        let raw = fs::read_to_string(fixtures().join("session_codex.jsonl")).unwrap();
        let r = adapt_transcript(&raw, "codex/rollout.jsonl");
        assert_eq!(r.kind, AdapterKind::Codex);
        assert!(r.turns.len() >= 2);
        assert!(r.fts_body.contains("fix the bug"));
    }

    #[test]
    fn chunk_turns_packs_messages() {
        let turns: Vec<SessionTurn> = (0..20)
            .map(|i| SessionTurn {
                agent: "a".into(),
                session_id: "s".into(),
                seq: i,
                ts_ms: 0,
                role: "user".into(),
                content: "x".repeat(200),
            })
            .collect();
        let chunks = chunk_turns(&turns);
        assert!(chunks.len() >= 2);
        assert!(chunks.len() <= MAX_CHUNKS);
    }
}
