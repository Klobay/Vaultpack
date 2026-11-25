use crate::format::{ATTR_HIDDEN, ATTR_READONLY, ATTR_SYSTEM};
use std::fs::Metadata;
use std::path::Path;
use std::time::SystemTime;

pub fn nanos(t: std::io::Result<SystemTime>) -> i64 {
    match t {
        Ok(t) => match t.duration_since(std::time::UNIX_EPOCH) {
            Ok(d) => d.as_nanos() as i64,
            Err(e) => -(e.duration().as_nanos() as i64),
        },
        Err(_) => 0,
    }
}

pub fn attrs_of(m: &Metadata) -> u32 {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        let a = m.file_attributes();
        let mut out = 0;
        if a & 0x1 != 0 {
            out |= ATTR_READONLY;
        }
        if a & 0x2 != 0 {
            out |= ATTR_HIDDEN;
        }
        if a & 0x4 != 0 {
            out |= ATTR_SYSTEM;
        }
        return out;
    }
    #[cfg(not(windows))]
    {
        let _ = (ATTR_HIDDEN, ATTR_SYSTEM);
        if m.permissions().readonly() {
            ATTR_READONLY
        } else {
            0
        }
    }
}

pub fn set_times(path: &Path, modified: i64, accessed: i64) {
    let f = |n: i64| filetime::FileTime::from_unix_time(n.div_euclid(1_000_000_000), n.rem_euclid(1_000_000_000) as u32);
    let _ = filetime::set_file_times(path, f(accessed), f(modified));
}

pub fn set_attrs(path: &Path, attrs: u32) {
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        use windows_sys::Win32::Storage::FileSystem::{GetFileAttributesW, SetFileAttributesW, FILE_ATTRIBUTE_HIDDEN, FILE_ATTRIBUTE_READONLY, FILE_ATTRIBUTE_SYSTEM, INVALID_FILE_ATTRIBUTES};
        let wide: Vec<u16> = path.as_os_str().encode_wide().chain(std::iter::once(0)).collect();
        unsafe {
            let cur = GetFileAttributesW(wide.as_ptr());
            if cur == INVALID_FILE_ATTRIBUTES {
                return;
            }
            let mut a = cur & !(FILE_ATTRIBUTE_HIDDEN | FILE_ATTRIBUTE_READONLY | FILE_ATTRIBUTE_SYSTEM);
            if attrs & ATTR_READONLY != 0 {
                a |= FILE_ATTRIBUTE_READONLY;
            }
            if attrs & ATTR_HIDDEN != 0 {
                a |= FILE_ATTRIBUTE_HIDDEN;
            }
            if attrs & ATTR_SYSTEM != 0 {
                a |= FILE_ATTRIBUTE_SYSTEM;
            }
            SetFileAttributesW(wide.as_ptr(), a);
        }
    }
    #[cfg(not(windows))]
    {
        if attrs & ATTR_READONLY != 0 {
            if let Ok(m) = std::fs::metadata(path) {
                let mut p = m.permissions();
                p.set_readonly(true);
                let _ = std::fs::set_permissions(path, p);
            }
        }
    }
}

pub fn clear_readonly(path: &Path) {
    if let Ok(m) = std::fs::metadata(path) {
        let mut p = m.permissions();
        if p.readonly() {
            #[allow(clippy::permissions_set_readonly_false)]
            p.set_readonly(false);
            let _ = std::fs::set_permissions(path, p);
        }
    }
}

pub fn free_space(path: &Path) -> Option<u64> {
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        use windows_sys::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;
        let mut probe = path.to_path_buf();
        while !probe.exists() {
            if !probe.pop() {
                return None;
            }
        }
        let wide: Vec<u16> = probe.as_os_str().encode_wide().chain(std::iter::once(0)).collect();
        let mut free: u64 = 0;
        let mut total: u64 = 0;
        let mut total_free: u64 = 0;
        let ok = unsafe { GetDiskFreeSpaceExW(wide.as_ptr(), &mut free, &mut total, &mut total_free) };
        if ok != 0 {
            Some(free)
        } else {
            None
        }
    }
    #[cfg(not(windows))]
    {
        let _ = path;
        None
    }
}

pub fn is_dir_link(m: &Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        m.file_attributes() & 0x10 != 0 && m.file_type().is_symlink()
    }
    #[cfg(not(windows))]
    {
        let _ = m;
        false
    }
}
