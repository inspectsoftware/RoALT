//! The alt Windows users (`altdock-<n>`) and SIDs. Reading needs no rights; creating, repairing
//! and deleting run in the elevated setup only. Remote Desktop Users is found by its SID
//! (`S-1-5-32-555`): its name depends on Windows' language.

use windows::Win32::Foundation::{ERROR_MEMBER_IN_ALIAS, HANDLE, HLOCAL, LocalFree};
use windows::Win32::NetworkManagement::NetManagement::{
    LOCALGROUP_MEMBERS_INFO_3, LOCALGROUP_USERS_INFO_0, LG_INCLUDE_INDIRECT, NERR_UserNotFound, NetApiBufferFree,
    NetLocalGroupAddMembers, NetUserAdd, NetUserDel, NetUserGetInfo, NetUserGetLocalGroups, NetUserSetInfo, UF_DONT_EXPIRE_PASSWD,
    UF_PASSWD_CANT_CHANGE, UF_SCRIPT, USER_INFO_1, USER_INFO_1003, USER_INFO_1008, USER_PRIV_USER,
};
use windows::Win32::Security::Authorization::{ConvertSidToStringSidW, ConvertStringSidToSidW};
use windows::Win32::Security::{GetTokenInformation, LookupAccountNameW, LookupAccountSidW, PSID, SID_NAME_USE, TOKEN_QUERY, TOKEN_USER, TokenUser};
use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};
use windows::Win32::UI::Shell::DeleteProfileW;
use windows::core::{HSTRING, Owned, PCWSTR, PWSTR};
use zeroize::Zeroizing;

/// Remote Desktop Users, whatever Windows calls it.
const REMOTE_DESKTOP_USERS: &str = "S-1-5-32-555";
const NERR_SUCCESS: u32 = 0;

fn net_error(code: u32) -> String {
    windows::core::Error::from(windows::core::HRESULT::from_win32(code)).message()
}

/// A local user of that name exists.
pub fn exists(name: &str) -> bool {
    let mut buf: *mut u8 = std::ptr::null_mut();
    // SAFETY: level 0 hands back one USER_INFO_0 we only free.
    let r = unsafe { NetUserGetInfo(PCWSTR::null(), &HSTRING::from(name), 0, &mut buf) };
    if !buf.is_null() {
        // SAFETY: allocated by NetUserGetInfo.
        unsafe { NetApiBufferFree(Some(buf.cast())) };
    }
    r == NERR_SUCCESS
}

/// A SID as `S-1-...`.
fn sid_string(sid: PSID) -> Option<String> {
    let mut s = PWSTR::null();
    // SAFETY: `sid` is a valid SID; the string is LocalAlloc'd and freed below.
    unsafe { ConvertSidToStringSidW(sid, &mut s) }.ok()?;
    // SAFETY: `s` is the NUL-terminated string just made.
    let out = unsafe { s.to_string() }.ok();
    // SAFETY: allocated by ConvertSidToStringSidW.
    unsafe { LocalFree(Some(HLOCAL(s.0.cast()))) };
    out
}

/// The name of the account with SID `sid` (`S-1-...`).
pub fn name_of_sid(sid: &str) -> Option<String> {
    let mut psid = PSID::default();
    // SAFETY: the SID is LocalAlloc'd and freed below.
    unsafe { ConvertStringSidToSidW(&HSTRING::from(sid), &mut psid) }.ok()?;
    let (mut name, mut domain) = (vec![0u16; 256], vec![0u16; 256]);
    let (mut name_len, mut domain_len) = (name.len() as u32, domain.len() as u32);
    let mut kind = SID_NAME_USE::default();
    // SAFETY: the buffers hold the lengths given.
    let found = unsafe {
        LookupAccountSidW(PCWSTR::null(), psid, Some(PWSTR(name.as_mut_ptr())), &mut name_len, Some(PWSTR(domain.as_mut_ptr())), &mut domain_len, &mut kind)
    };
    // SAFETY: allocated by ConvertStringSidToSidW.
    unsafe { LocalFree(Some(HLOCAL(psid.0))) };
    found.ok()?;
    Some(String::from_utf16_lossy(&name[..name_len as usize]))
}

/// Remote Desktop Users' name in this Windows' language.
pub fn remote_desktop_users() -> Option<String> {
    name_of_sid(REMOTE_DESKTOP_USERS)
}

/// The user is in Remote Desktop Users (directly or through a group).
pub fn in_remote_desktop_users(name: &str) -> bool {
    let Some(group) = remote_desktop_users() else { return false };
    let mut buf: *mut u8 = std::ptr::null_mut();
    let (mut read, mut total) = (0u32, 0u32);
    // SAFETY: level 0 hands back `read` LOCALGROUP_USERS_INFO_0 entries, freed below.
    let r = unsafe { NetUserGetLocalGroups(PCWSTR::null(), &HSTRING::from(name), 0, LG_INCLUDE_INDIRECT, &mut buf, u32::MAX, &mut read, &mut total) };
    let mut found = false;
    if r == NERR_SUCCESS && !buf.is_null() {
        // SAFETY: `buf` holds `read` entries.
        let groups = unsafe { std::slice::from_raw_parts(buf as *const LOCALGROUP_USERS_INFO_0, read as usize) };
        // SAFETY: each name is a NUL-terminated string inside `buf`.
        found = groups.iter().any(|g| unsafe { g.lgrui0_name.to_string() }.is_ok_and(|n| n.eq_ignore_ascii_case(&group)));
    }
    if !buf.is_null() {
        // SAFETY: allocated by NetUserGetLocalGroups.
        unsafe { NetApiBufferFree(Some(buf.cast())) };
    }
    found
}

/// The SID (`S-1-...`) of a local account.
pub fn sid_of(name: &str) -> Option<String> {
    let mut sid = vec![0u8; 256];
    let mut domain = vec![0u16; 256];
    let (mut sid_len, mut domain_len) = (sid.len() as u32, domain.len() as u32);
    let mut kind = SID_NAME_USE::default();
    // SAFETY: the buffers hold the lengths given.
    unsafe {
        LookupAccountNameW(
            PCWSTR::null(),
            &HSTRING::from(name),
            Some(PSID(sid.as_mut_ptr().cast())),
            &mut sid_len,
            Some(PWSTR(domain.as_mut_ptr())),
            &mut domain_len,
            &mut kind,
        )
    }
    .ok()?;
    sid_string(PSID(sid.as_mut_ptr().cast()))
}

/// The SID of the user this process runs as.
pub fn current_user_sid() -> Option<String> {
    let mut token = HANDLE::default();
    // SAFETY: the current process's pseudo handle; the token is closed by Owned.
    unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) }.ok()?;
    // SAFETY: `token` was just opened.
    let token = unsafe { Owned::new(token) };
    let mut len = 0u32;
    // SAFETY: asks for the size only.
    let _ = unsafe { GetTokenInformation(*token, TokenUser, None, 0, &mut len) };
    // u64s keep the TOKEN_USER aligned.
    let mut buf = vec![0u64; (len as usize).div_ceil(8)];
    // SAFETY: `buf` holds `len` bytes.
    unsafe { GetTokenInformation(*token, TokenUser, Some(buf.as_mut_ptr().cast()), len, &mut len) }.ok()?;
    // SAFETY: the call filled a TOKEN_USER at the start of `buf`, which outlives this use.
    let user = unsafe { &*(buf.as_ptr() as *const TOKEN_USER) };
    sid_string(user.User.Sid)
}

const FLAGS: windows::Win32::NetworkManagement::NetManagement::USER_ACCOUNT_FLAGS =
    windows::Win32::NetworkManagement::NetManagement::USER_ACCOUNT_FLAGS(UF_SCRIPT.0 | UF_DONT_EXPIRE_PASSWD.0 | UF_PASSWD_CANT_CHANGE.0);

/// Creates the alt user, or (it exists) sets its password and makes sure it is enabled and its
/// password never expires. Elevated only.
pub fn create_or_repair(name: &str, password: &Zeroizing<String>) -> Result<String, String> {
    let mut wname: Vec<u16> = name.encode_utf16().chain([0]).collect();
    let mut wpass = Zeroizing::new(password.encode_utf16().chain([0]).collect::<Vec<u16>>());
    let mut comment: Vec<u16> = "AltDock background session".encode_utf16().chain([0]).collect();
    if !exists(name) {
        let info = USER_INFO_1 {
            usri1_name: PWSTR(wname.as_mut_ptr()),
            usri1_password: PWSTR(wpass.as_mut_ptr()),
            usri1_priv: USER_PRIV_USER,
            usri1_comment: PWSTR(comment.as_mut_ptr()),
            usri1_flags: FLAGS,
            ..Default::default()
        };
        // SAFETY: `info` and the strings it points at live through the call.
        let r = unsafe { NetUserAdd(PCWSTR::null(), 1, (&raw const info).cast(), None) };
        return if r == NERR_SUCCESS { Ok(tr!("Created.")) } else { Err(net_error(r)) };
    }
    let pass = USER_INFO_1003 { usri1003_password: PWSTR(wpass.as_mut_ptr()) };
    // SAFETY: `pass` and its string live through the call.
    let r = unsafe { NetUserSetInfo(PCWSTR::null(), PCWSTR(wname.as_ptr()), 1003, (&raw const pass).cast(), None) };
    if r != NERR_SUCCESS {
        return Err(net_error(r));
    }
    let flags = USER_INFO_1008 { usri1008_flags: FLAGS };
    // SAFETY: `flags` lives through the call.
    let r = unsafe { NetUserSetInfo(PCWSTR::null(), PCWSTR(wname.as_ptr()), 1008, (&raw const flags).cast(), None) };
    if r == NERR_SUCCESS { Ok(tr!("Repaired.")) } else { Err(net_error(r)) }
}

/// Adds the user to Remote Desktop Users (already in it is fine). Elevated only.
pub fn add_to_remote_desktop_users(name: &str) -> Result<String, String> {
    let group = remote_desktop_users().ok_or_else(|| tr!("Remote Desktop Users wasn't found."))?;
    let mut member: Vec<u16> = name.encode_utf16().chain([0]).collect();
    let info = LOCALGROUP_MEMBERS_INFO_3 { lgrmi3_domainandname: PWSTR(member.as_mut_ptr()) };
    // SAFETY: one LOCALGROUP_MEMBERS_INFO_3 whose string lives through the call.
    let r = unsafe { NetLocalGroupAddMembers(PCWSTR::null(), &HSTRING::from(group.as_str()), 3, (&raw const info).cast(), 1) };
    if r == NERR_SUCCESS || r == ERROR_MEMBER_IN_ALIAS.0 { Ok(group) } else { Err(net_error(r)) }
}

/// Deletes the user and its profile folder (a missing user is fine). Elevated only.
pub fn delete(name: &str) -> Result<String, String> {
    let sid = sid_of(name);
    // SAFETY: the string lives through the call.
    let r = unsafe { NetUserDel(PCWSTR::null(), &HSTRING::from(name)) };
    if r != NERR_SUCCESS && r != NERR_UserNotFound {
        return Err(net_error(r));
    }
    if let Some(sid) = sid {
        // SAFETY: the string lives through the call. No profile (never signed in) fails harmlessly.
        if let Err(e) = unsafe { DeleteProfileW(&HSTRING::from(sid.as_str()), PCWSTR::null(), PCWSTR::null()) } {
            log::debug!("no profile deleted for {name}: {e}");
        }
    }
    Ok(tr!("Deleted."))
}
