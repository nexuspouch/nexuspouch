use super::{io_err, Local, OpError};
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use std::fs;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MasterPointer {
    pub master: String,
    pub epoch: i64,
}

pub fn load_pointer(local: &Local) -> Result<MasterPointer, OpError> {
    let path = local.pointer_path();
    if !path.exists() {
        return Ok(MasterPointer {
            master: local.device_id.clone(),
            epoch: 0,
        });
    }
    let raw = fs::read_to_string(&path).map_err(io_err)?;
    let mut p: MasterPointer =
        serde_json::from_str(&raw).map_err(|e| OpError::new("internal", e.to_string()))?;
    if !crate::protocol::is_valid_device_id(&p.master) {
        p.master = local.device_id.clone();
    }
    Ok(p)
}

pub fn save_pointer(local: &Local, p: &MasterPointer) -> Result<(), OpError> {
    let path = local.pointer_path();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(io_err)?;
    }
    let raw = serde_json::to_string_pretty(p).map_err(|e| OpError::new("internal", e.to_string()))?;
    let tmp = path.with_extension("tmp");
    fs::write(&tmp, &raw).map_err(io_err)?;
    fs::rename(&tmp, &path).map_err(io_err)?;
    Ok(())
}

pub fn pointer_query(local: &Local) -> Result<Map<String, Value>, OpError> {
    let p = load_pointer(local)?;
    let mut m = Map::new();
    m.insert("master".into(), json!(p.master));
    m.insert("epoch".into(), json!(p.epoch));
    Ok(m)
}

pub fn pointer_apply(local: &Local, frame: &crate::protocol::Frame) -> Result<Map<String, Value>, OpError> {
    let master = frame.payload.get("master").and_then(|v| v.as_str()).unwrap_or("");
    let epoch = super::any_to_i64(frame.payload.get("epoch").unwrap_or(&Value::Null)).unwrap_or(0);
    if !crate::protocol::is_valid_device_id(master) || epoch <= 0 {
        return Err(OpError::new("bad_op", "invalid master pointer"));
    }
    let cur = load_pointer(local)?;
    if epoch < cur.epoch {
        return Ok(Map::from_iter([
            ("applied".into(), json!(false)),
            ("reason".into(), json!("stale")),
        ]));
    }
    if epoch == cur.epoch {
        if cur.master == master {
            return Ok(Map::from_iter([
                ("applied".into(), json!(false)),
                ("reason".into(), json!("unchanged")),
            ]));
        }
        return Ok(Map::from_iter([
            ("applied".into(), json!(false)),
            ("reason".into(), json!("same_epoch_conflict")),
        ]));
    }
    save_pointer(
        local,
        &MasterPointer {
            master: master.to_string(),
            epoch,
        },
    )?;
    Ok(Map::from_iter([
        ("applied".into(), json!(true)),
        ("master".into(), json!(master)),
        ("epoch".into(), json!(epoch)),
    ]))
}

pub fn migrate(local: &Local, _frame: &crate::protocol::Frame) -> Result<Map<String, Value>, OpError> {
    let cur = load_pointer(local)?;
    let mut epoch = cur.epoch + 1;
    if epoch < 1 {
        epoch = 1;
    }
    let next = MasterPointer {
        master: local.device_id.clone(),
        epoch,
    };
    save_pointer(local, &next)?;
    let cursors = local.cursors().all()?;
    let mut cursor_map = Map::new();
    for (k, v) in cursors {
        cursor_map.insert(k, json!(v));
    }
    Ok(Map::from_iter([
        ("master".into(), json!(next.master)),
        ("epoch".into(), json!(next.epoch)),
        ("old_master_reachable".into(), json!(false)),
        ("cursors".into(), Value::Object(cursor_map)),
        ("seeded_files".into(), json!(0)),
        (
            "hash_gate".into(),
            json!({
                "ran": false,
                "ok": true,
                "devices": [],
                "mismatches": [],
                "mismatch_count": 0
            }),
        ),
    ]))
}
