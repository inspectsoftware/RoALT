//! The alt user's password in the main user's Credential Manager (`TERMSRV/<address>`), where
//! mstsc takes it from: never in the `.rdp` file or on a command line. No elevation needed.

use windows::Win32::Foundation::ERROR_NOT_FOUND;
use windows::Win32::Security::Credentials::{CRED_PERSIST_LOCAL_MACHINE, CRED_TYPE_DOMAIN_PASSWORD, CREDENTIALW, CredDeleteW, CredWriteW};
use windows::core::{HSTRING, PWSTR};
use zeroize::Zeroizing;

use crate::error::{AppError, Result};

fn target(address: &str) -> String {
    format!("TERMSRV/{address}")
}

/// How the `.rdp` file and the credential name an alt user: `<computer>\<user>`.
pub fn user_name(windows_user: &str) -> String {
    let computer = std::env::var("COMPUTERNAME").unwrap_or_else(|_| ".".into());
    format!("{computer}\\{windows_user}")
}

/// Stores (or replaces) the credential for `address`.
pub fn write(address: &str, windows_user: &str, password: &str) -> Result<()> {
    let mut target: Vec<u16> = target(address).encode_utf16().chain([0]).collect();
    let mut user: Vec<u16> = user_name(windows_user).encode_utf16().chain([0]).collect();
    // The blob is the password in UTF-16, without its NUL.
    let mut blob = Zeroizing::new(password.encode_utf16().flat_map(u16::to_le_bytes).collect::<Vec<u8>>());
    let cred = CREDENTIALW {
        Type: CRED_TYPE_DOMAIN_PASSWORD,
        TargetName: PWSTR(target.as_mut_ptr()),
        CredentialBlobSize: blob.len() as u32,
        CredentialBlob: blob.as_mut_ptr(),
        Persist: CRED_PERSIST_LOCAL_MACHINE,
        UserName: PWSTR(user.as_mut_ptr()),
        ..Default::default()
    };
    // SAFETY: `cred` and every buffer it points at live through the call.
    unsafe { CredWriteW(&cred, 0) }.map_err(|e| AppError::Internal(tr!("Could not store the session's sign-in: {why}", why = e.message())))
}

/// Removes the credential for `address` (none there is fine).
pub fn delete(address: &str) -> Result<()> {
    // SAFETY: the string lives through the call.
    match unsafe { CredDeleteW(&HSTRING::from(target(address)), CRED_TYPE_DOMAIN_PASSWORD, None) } {
        Err(e) if e.code() != ERROR_NOT_FOUND.to_hresult() => Err(AppError::Internal(e.message())),
        _ => Ok(()),
    }
}
