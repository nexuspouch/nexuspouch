use std::path::Path;

#[cfg(unix)]
pub fn probe_volume(path: &Path) -> Option<(i64, i64)> {
    use std::ffi::CString;
    use std::mem::MaybeUninit;

    let cpath = CString::new(path.to_string_lossy().as_bytes()).ok()?;
    let mut st = MaybeUninit::<libc::statvfs>::uninit();
    let rc = unsafe { libc::statvfs(cpath.as_ptr(), st.as_mut_ptr()) };
    if rc != 0 {
        return None;
    }
    let st = unsafe { st.assume_init() };
    let bsize = st.f_frsize as i64;
    if bsize <= 0 {
        return None;
    }
    let total = st.f_blocks as i64 * bsize;
    let free = st.f_bavail as i64 * bsize;
    if total <= 0 {
        return None;
    }
    Some((total, free))
}

#[cfg(not(unix))]
pub fn probe_volume(_path: &Path) -> Option<(i64, i64)> {
    None
}
