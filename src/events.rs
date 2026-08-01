use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::collections::VecDeque;
use std::fs::OpenOptions;
use std::io::{BufRead, Write as _};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use tokio::sync::broadcast;
use uuid::Uuid;

const RECENT_CAP: usize = 100;
const BROADCAST_CAP: usize = 256;
const REPLAY_CAP: usize = 10_000;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoreEvent {
    pub id: String,
    /// Monotonic, persisted event sequence (0 until first publish).
    #[serde(default)]
    pub seq: u64,
    pub ts_ms: i64,
    pub kind: String,
    pub device: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub space: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub uri: Option<String>,
    #[serde(default, skip_serializing_if = "Map::is_empty")]
    pub detail: Map<String, Value>,
}

impl StoreEvent {
    pub fn new(kind: impl Into<String>, device: impl Into<String>) -> Self {
        Self {
            id: Uuid::new_v4().to_string(),
            seq: 0,
            ts_ms: now_ms(),
            kind: kind.into(),
            device: device.into(),
            agent_id: None,
            space: None,
            path: None,
            uri: None,
            detail: Map::new(),
        }
    }

    pub fn with_agent(mut self, agent_id: impl Into<String>) -> Self {
        self.agent_id = Some(agent_id.into());
        self
    }

    pub fn with_uri(mut self, uri: impl Into<String>) -> Self {
        self.uri = Some(uri.into());
        self
    }

    pub fn with_path(mut self, space: impl Into<String>, path: impl Into<String>) -> Self {
        self.space = Some(space.into());
        self.path = Some(path.into());
        self
    }

    pub fn with_detail(mut self, detail: Map<String, Value>) -> Self {
        self.detail = detail;
        self
    }
}

pub struct EventBus {
    tx: broadcast::Sender<StoreEvent>,
    recent: Mutex<VecDeque<StoreEvent>>,
    next_seq: Mutex<u64>,
    log_path: Option<PathBuf>,
    log: Option<Mutex<std::fs::File>>,
}

impl EventBus {
    pub fn new() -> Arc<Self> {
        let (tx, _) = broadcast::channel(BROADCAST_CAP);
        Arc::new(Self {
            tx,
            recent: Mutex::new(VecDeque::with_capacity(RECENT_CAP)),
            next_seq: Mutex::new(0),
            log_path: None,
            log: None,
        })
    }

    /// Open an event bus backed by `<root>/.system/events.jsonl` so watchers can
    /// replay with `since=<seq>` after a disconnect (M3).
    pub fn open(root: impl AsRef<Path>) -> Arc<Self> {
        let dir = root.as_ref().join(".system");
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("events.jsonl");
        let last_seq = last_seq_from(&path);
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .expect("open events.jsonl");
        let (tx, _) = broadcast::channel(BROADCAST_CAP);
        Arc::new(Self {
            tx,
            recent: Mutex::new(VecDeque::with_capacity(RECENT_CAP)),
            next_seq: Mutex::new(last_seq),
            log_path: Some(path),
            log: Some(Mutex::new(file)),
        })
    }

    pub fn publish(&self, mut event: StoreEvent) {
        {
            let mut seq = self.next_seq.lock().unwrap();
            *seq += 1;
            event.seq = *seq;
        }
        if let Some(log) = &self.log {
            if let Ok(mut f) = log.lock() {
                let _ = writeln!(f, "{}", serde_json::to_string(&event).unwrap_or_default());
            }
        }
        {
            let mut buf = self.recent.lock().unwrap();
            if buf.len() >= RECENT_CAP {
                buf.pop_front();
            }
            buf.push_back(event.clone());
        }
        let _ = self.tx.send(event);
    }

    pub fn subscribe(&self) -> broadcast::Receiver<StoreEvent> {
        self.tx.subscribe()
    }

    pub fn recent(&self) -> Vec<StoreEvent> {
        self.recent.lock().unwrap().iter().cloned().collect()
    }

    /// Replay persisted events with `seq > since` (bounded by REPLAY_CAP).
    /// Empty when the bus has no persistence (tests / in-memory).
    pub fn replay(&self, since: u64) -> Vec<StoreEvent> {
        let Some(path) = &self.log_path else {
            return Vec::new();
        };
        let Ok(f) = std::fs::File::open(path) else {
            return Vec::new();
        };
        let mut out = Vec::new();
        for line in std::io::BufReader::new(f).lines().map_while(Result::ok) {
            if let Ok(ev) = serde_json::from_str::<StoreEvent>(&line) {
                if ev.seq > since {
                    out.push(ev);
                    if out.len() >= REPLAY_CAP {
                        break;
                    }
                }
            }
        }
        out
    }
}

fn last_seq_from(path: &Path) -> u64 {
    let Ok(f) = std::fs::File::open(path) else {
        return 0;
    };
    let mut last = 0u64;
    for line in std::io::BufReader::new(f).lines().map_while(Result::ok) {
        if let Ok(ev) = serde_json::from_str::<StoreEvent>(&line) {
            last = ev.seq;
        }
    }
    last
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recent_ring_buffer() {
        let bus = EventBus::new();
        for i in 0..105 {
            bus.publish(StoreEvent::new("commit", format!("d{i:04}")));
        }
        let recent = bus.recent();
        assert_eq!(recent.len(), 100);
        assert_eq!(recent[0].device, "d0005");
        assert_eq!(recent[99].device, "d0104");
    }

    #[test]
    fn subscribe_receives() {
        let bus = EventBus::new();
        let mut rx = bus.subscribe();
        bus.publish(StoreEvent::new("delete", "aaaaaaaaaaaaaaaa"));
        let ev = rx.try_recv().unwrap();
        assert_eq!(ev.kind, "delete");
        assert_eq!(ev.seq, 1);
    }

    #[test]
    fn persisted_seq_and_replay() {
        let dir = tempfile::tempdir().unwrap();
        let bus = EventBus::open(dir.path());
        for i in 1..=5 {
            bus.publish(StoreEvent::new("commit", format!("d{i}")));
        }
        let bus2 = EventBus::open(dir.path());
        // New bus continues the sequence.
        bus2.publish(StoreEvent::new("delete", "d6"));
        let ev = bus2.recent().pop().unwrap();
        assert_eq!(ev.seq, 6);

        let replayed = bus2.replay(3);
        assert_eq!(replayed.len(), 3);
        assert_eq!(replayed[0].seq, 4);
        assert_eq!(replayed[2].seq, 6);

        let replayed = bus2.replay(6);
        assert!(replayed.is_empty());
    }
}
