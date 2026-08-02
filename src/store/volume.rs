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

/// Windows: `GetDiskFreeSpaceExW` on the store root (or its volume).
#[cfg(windows)]
pub fn probe_volume(path: &Path) -> Option<(i64, i64)> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;

    let mut wide: Vec<u16> = path.as_os_str().encode_wide().collect();
    if wide.is_empty() {
        return None;
    }
    wide.push(0);

    let mut avail: u64 = 0;
    let mut total: u64 = 0;
    let mut free: u64 = 0;
    let ok = unsafe {
        GetDiskFreeSpaceExW(
            wide.as_ptr(),
            &mut avail,
            &mut total,
            &mut free,
        )
    };
    if ok == 0 || total == 0 {
        return None;
    }
    // Prefer caller-available free (quota-aware) when present.
    let free_i = if avail > 0 { avail } else { free };
    Some((total as i64, free_i as i64))
}

#[cfg(all(not(unix), not(windows)))]
pub fn probe_volume(_path: &Path) -> Option<(i64, i64)> {
    None
}
