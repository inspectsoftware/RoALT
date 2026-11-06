//! Small registry helpers for the session checks and the elevated setup.

use windows::Win32::Foundation::ERROR_SUCCESS;
use windows::Win32::System::Registry::{
    HKEY, REG_DWORD, REG_SZ, REG_VALUE_TYPE, RRF_NOEXPAND, RRF_RT_ANY, RRF_RT_REG_DWORD, RegDeleteKeyValueW, RegGetValueW, RegSetKeyValueW,
};
use windows::core::HSTRING;

use super::restore::{Prior, RegData};

pub use windows::Win32::System::Registry::{HKEY_CURRENT_USER as HKCU, HKEY_LOCAL_MACHINE as HKLM};

pub use super::restore::{OOBE_POLICY, SYSTEM_POLICY, TS_CONTROL, TS_POLICY, USER_LIST};
pub const TERMSERVICE: &str = r"SYSTEM\CurrentControlSet\Services\TermService\Parameters";
pub const RUN: &str = r"SOFTWARE\Microsoft\Windows\CurrentVersion\Run";
pub const RDP_CLIENT: &str = r"Software\Microsoft\Terminal Server Client";

/// A `REG_DWORD`, None when it isn't there (or isn't a DWORD).
pub fn dword(root: HKEY, key: &str, value: &str) -> Option<u32> {
    let mut data = 0u32;
    let mut len = size_of::<u32>() as u32;
    // SAFETY: `data` is a local u32 and `len` says so; the strings live through the call.
    let r = unsafe {
        RegGetValueW(root, &HSTRING::from(key), &HSTRING::from(value), RRF_RT_REG_DWORD, None, Some((&raw mut data).cast()), Some(&mut len))
    };
    (r == ERROR_SUCCESS).then_some(data)
}

/// A string value (expanded when it is `REG_EXPAND_SZ`).
pub fn string(root: HKEY, key: &str, value: &str) -> Option<String> {
    crate::launcher::client::reg_string(root, key, Some(value))
}

/// A value's state as `restore.json` records it: absent, a `REG_DWORD` or `REG_SZ` (never
/// expanded), or Unknown (another type, or unreadable).
pub fn prior(root: HKEY, key: &str, value: &str) -> Prior {
    let (key, value) = (HSTRING::from(key), HSTRING::from(value));
    let flags = RRF_RT_ANY | RRF_NOEXPAND;
    let mut kind = REG_VALUE_TYPE::default();
    let mut len = 0u32;
    // SAFETY: asks for the type and size only; the strings live through the call.
    let r = unsafe { RegGetValueW(root, &key, &value, flags, Some(&mut kind), None, Some(&mut len)) };
    if r == windows::Win32::Foundation::ERROR_FILE_NOT_FOUND {
        return Prior::Absent;
    }
    if r != ERROR_SUCCESS || len > 64 * 1024 {
        return Prior::Unknown;
    }
    // u16s keep a string aligned; one more for a NUL the data may lack.
    let mut buf = vec![0u16; (len as usize).div_ceil(2) + 1];
    let mut len = (buf.len() * 2) as u32;
    // SAFETY: `buf` holds `len` bytes.
    let r = unsafe { RegGetValueW(root, &key, &value, flags, Some(&mut kind), Some(buf.as_mut_ptr().cast()), Some(&mut len)) };
    if r != ERROR_SUCCESS {
        return Prior::Unknown;
    }
    match kind {
        REG_DWORD if len == 4 => Prior::Was { data: RegData::Dword(u32::from(buf[0]) | (u32::from(buf[1]) << 16)) },
        REG_SZ => {
            let units = &buf[..(len as usize / 2).min(buf.len())];
            let end = units.iter().position(|&c| c == 0).unwrap_or(units.len());
            Prior::Was { data: RegData::String(String::from_utf16_lossy(&units[..end])) }
        }
        _ => Prior::Unknown,
    }
}

/// Sets a value to `data`.
pub fn set(root: HKEY, key: &str, value: &str, data: &RegData) -> std::result::Result<(), String> {
    match data {
        RegData::Dword(d) => set_dword(root, key, value, *d),
        RegData::String(s) => set_string(root, key, value, s),
    }
}

fn check(r: windows::Win32::Foundation::WIN32_ERROR, what: &str) -> std::result::Result<(), String> {
    if r == ERROR_SUCCESS { Ok(()) } else { Err(format!("{what}: {}", windows::core::Error::from(r.to_hresult()).message())) }
}

/// Sets a `REG_DWORD`, creating the key when needed.
pub fn set_dword(root: HKEY, key: &str, value: &str, data: u32) -> std::result::Result<(), String> {
    let bytes = data.to_le_bytes();
    // SAFETY: `bytes` is 4 bytes, the size given; the strings live through the call.
    let r = unsafe { RegSetKeyValueW(root, &HSTRING::from(key), &HSTRING::from(value), REG_DWORD.0, Some(bytes.as_ptr().cast()), 4) };
    check(r, value)
}

/// Sets a `REG_SZ`, creating the key when needed.
pub fn set_string(root: HKEY, key: &str, value: &str, data: &str) -> std::result::Result<(), String> {
    let wide: Vec<u16> = data.encode_utf16().chain([0]).collect();
    // SAFETY: `wide` is NUL-terminated and its byte size is given; the strings live through the call.
    let r = unsafe {
        RegSetKeyValueW(root, &HSTRING::from(key), &HSTRING::from(value), REG_SZ.0, Some(wide.as_ptr().cast()), (wide.len() * 2) as u32)
    };
    check(r, value)
}

/// Deletes a value; one that isn't there is fine.
pub fn delete(root: HKEY, key: &str, value: &str) -> std::result::Result<(), String> {
    // SAFETY: the strings live through the call.
    let r = unsafe { RegDeleteKeyValueW(root, &HSTRING::from(key), &HSTRING::from(value)) };
    if r == windows::Win32::Foundation::ERROR_FILE_NOT_FOUND { Ok(()) } else { check(r, value) }
}
