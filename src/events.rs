use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use tokio::sync::broadcast;
use uuid::Uuid;

const RECENT_CAP: usize = 100;
const BROADCAST_CAP: usize = 256;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoreEvent {
    pub id: String,
    pub ts_ms: i64,
    pub kind: String,
    pub device: String,
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
            ts_ms: now_ms(),
            kind: kind.into(),
            device: device.into(),
            space: None,
            path: None,
            uri: None,
            detail: Map::new(),
        }
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
}

impl EventBus {
    pub fn new() -> Arc<Self> {
        let (tx, _) = broadcast::channel(BROADCAST_CAP);
        Arc::new(Self {
            tx,
            recent: Mutex::new(VecDeque::with_capacity(RECENT_CAP)),
        })
    }

    pub fn publish(&self, event: StoreEvent) {
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
    }
}
