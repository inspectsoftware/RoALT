//! Small registry helpers for the session checks and the elevated setup.

use windows::Win32::Foundation::ERROR_SUCCESS;
use windows::Win32::System::Registry::{
    HKEY, REG_DWORD, REG_SZ, RRF_RT_REG_DWORD, RegDeleteKeyValueW, RegGetValueW, RegSetKeyValueW,
};
use windows::core::HSTRING;

pub use windows::Win32::System::Registry::{HKEY_CURRENT_USER as HKCU, HKEY_LOCAL_MACHINE as HKLM};

/// Terminal Services policies (`fSingleSessionPerUser`, `bEnumerateHWBeforeSW`, ...).
pub const TS_POLICY: &str = r"SOFTWARE\Policies\Microsoft\Windows NT\Terminal Services";
/// The Terminal Server settings policies override (`fDenyTSConnections`, ...).
pub const TS_CONTROL: &str = r"SYSTEM\CurrentControlSet\Control\Terminal Server";
pub const OOBE_POLICY: &str = r"SOFTWARE\Policies\Microsoft\Windows\OOBE";
pub const SYSTEM_POLICY: &str = r"SOFTWARE\Microsoft\Windows\CurrentVersion\Policies\System";
pub const TERMSERVICE: &str = r"SYSTEM\CurrentControlSet\Services\TermService\Parameters";
pub const USER_LIST: &str = r"SOFTWARE\Microsoft\Windows NT\CurrentVersion\Winlogon\SpecialAccounts\UserList";
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
