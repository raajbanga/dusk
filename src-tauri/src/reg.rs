//! Minimal HKCU registry access (advapi32), so we don't need a registry crate.

use std::ffi::c_void;

const HKEY_CURRENT_USER: isize = 0x8000_0001u32 as i32 as isize;
const RRF_RT_ANY: u32 = 0xFFFF;
pub const REG_SZ: u32 = 1;
pub const REG_BINARY: u32 = 3;
pub const REG_DWORD: u32 = 4;

#[link(name = "advapi32")]
unsafe extern "system" {
    fn RegGetValueW(
        key: isize,
        subkey: *const u16,
        value: *const u16,
        flags: u32,
        kind: *mut u32,
        data: *mut c_void,
        len: *mut u32,
    ) -> i32;
    fn RegSetKeyValueW(
        key: isize,
        subkey: *const u16,
        value: *const u16,
        kind: u32,
        data: *const c_void,
        len: u32,
    ) -> i32;
    fn RegDeleteKeyValueW(key: isize, subkey: *const u16, value: *const u16) -> i32;
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}

pub fn get(subkey: &str, value: &str) -> Option<Vec<u8>> {
    let (k, v) = (wide(subkey), wide(value));
    let query = |data: *mut c_void, len: &mut u32| unsafe {
        let null = std::ptr::null_mut();
        RegGetValueW(
            HKEY_CURRENT_USER,
            k.as_ptr(),
            v.as_ptr(),
            RRF_RT_ANY,
            null,
            data,
            len,
        )
    };
    let mut len = 0u32;
    if query(std::ptr::null_mut(), &mut len) != 0 {
        return None;
    }
    let mut buf = vec![0u8; len as usize];
    let rc = query(buf.as_mut_ptr() as _, &mut len);
    buf.truncate(len as usize);
    (rc == 0).then_some(buf)
}

pub fn set(subkey: &str, value: &str, kind: u32, data: &[u8]) -> Result<(), String> {
    let (k, v) = (wide(subkey), wide(value));
    let rc = unsafe {
        RegSetKeyValueW(
            HKEY_CURRENT_USER,
            k.as_ptr(),
            v.as_ptr(),
            kind,
            data.as_ptr() as _,
            data.len() as u32,
        )
    };
    if rc == 0 {
        Ok(())
    } else {
        Err(format!("RegSetKeyValueW failed: {rc}"))
    }
}

pub fn set_string(subkey: &str, value: &str, s: &str) -> Result<(), String> {
    let w = wide(s);
    let bytes: Vec<u8> = w.iter().flat_map(|c| c.to_le_bytes()).collect();
    set(subkey, value, REG_SZ, &bytes)
}

pub fn delete(subkey: &str, value: &str) -> Result<(), String> {
    let (k, v) = (wide(subkey), wide(value));
    match unsafe { RegDeleteKeyValueW(HKEY_CURRENT_USER, k.as_ptr(), v.as_ptr()) } {
        0 | 2 => Ok(()), // 2 = ERROR_FILE_NOT_FOUND: already gone
        rc => Err(format!("RegDeleteKeyValueW failed: {rc}")),
    }
}
