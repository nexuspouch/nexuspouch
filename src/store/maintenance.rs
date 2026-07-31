use super::{browse, io_err, Local, OpError};
use crate::protocol;
use std::fs;

pub fn purge_device(local: &Local, device_id: &str, self_device_id: &str) -> Result<i64, OpError> {
    if !protocol::is_valid_device_id(device_id) {
        return Err(OpError::new("bad_path", "invalid device id"));
    }
    if device_id == self_device_id {
        return Err(OpError::new("acl_denied", "cannot purge self"));
    }
    let dir = local.root.join(device_id);
    let st = fs::metadata(&dir).map_err(|_| OpError::new("not_found", device_id))?;
    if !st.is_dir() {
        return Err(OpError::new("not_found", device_id));
    }
    let freed = browse::dir_size(&dir, false);
    fs::remove_dir_all(&dir).map_err(io_err)?;
    let _ = local.cursors().remove(device_id);
    Ok(freed)
}

pub fn wipe_self(local: &Local, self_device_id: &str) -> Result<i64, OpError> {
    if !protocol::is_valid_device_id(self_device_id) {
        return Err(OpError::new("bad_path", "invalid device id"));
    }
    if self_device_id != local.device_id {
        return Err(OpError::new("acl_denied", "can only wipe self"));
    }
    let dir = local.root.join(self_device_id);
    let freed = match fs::metadata(&dir) {
        Ok(st) if st.is_dir() => browse::dir_size(&dir, false),
        Ok(_) => return Err(OpError::new("bad_path", "not a device directory")),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(0),
        Err(e) => return Err(io_err(e)),
    };
    fs::remove_dir_all(&dir).map_err(io_err)?;
    fs::create_dir_all(&dir).map_err(io_err)?;
    for sp in ["artifacts", "files", "attachments", "backups"] {
        fs::create_dir_all(dir.join(sp)).map_err(io_err)?;
    }
    Ok(freed)
}
