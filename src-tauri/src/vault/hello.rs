//! Windows Hello confirmation (DPAPI-mode re-authentication).

use windows::Security::Credentials::UI::{
    UserConsentVerificationResult as Outcome, UserConsentVerifier, UserConsentVerifierAvailability,
};
use windows::Win32::Foundation::HWND;
use windows::Win32::System::WinRT::IUserConsentVerifierInterop;
use windows::core::{HSTRING, factory};
use windows_future::IAsyncOperation;

use crate::error::{AppError, Result};

/// Shows the Windows Hello prompt over `window`. `Ok(())` only when the user is verified.
/// Cancelled → `AppError::Cancelled`; not set up / failed → `AppError::Invalid` with a message
/// saying what to do (e.g. "Windows Hello isn't set up. Set a master password to reveal cookies.").
pub async fn windows_hello(window: &tauri::WebviewWindow, message: &str) -> Result<()> {
    // HWND is not Send; its address is.
    let hwnd = window.hwnd()?.0 as isize;
    let message = HSTRING::from(message);
    tauri::async_runtime::spawn_blocking(move || verify(HWND(hwnd as *mut _), &message))
        .await
        .map_err(|e| AppError::Internal(e.to_string()))?
}

/// Whether Windows Hello is set up on this PC.
pub async fn hello_available() -> bool {
    tauri::async_runtime::spawn_blocking(|| {
        UserConsentVerifier::CheckAvailabilityAsync()
            .and_then(|op| op.join())
            .is_ok_and(|a| a == UserConsentVerifierAvailability::Available)
    })
    .await
    .unwrap_or(false)
}

fn verify(hwnd: HWND, message: &HSTRING) -> Result<()> {
    let available = UserConsentVerifier::CheckAvailabilityAsync().and_then(|op| op.join()).map_err(failed)?;
    if available != UserConsentVerifierAvailability::Available {
        return Err(AppError::Invalid(tr!("Windows Hello isn't set up. Set a master password in Settings to reveal cookies.")));
    }
    let interop = factory::<UserConsentVerifier, IUserConsentVerifierInterop>().map_err(failed)?;
    // SAFETY: `hwnd` is the live main window; the call only reads it to parent the prompt.
    let op: IAsyncOperation<Outcome> =
        unsafe { interop.RequestVerificationForWindowAsync(hwnd, message) }.map_err(failed)?;
    match op.join().map_err(failed)? {
        Outcome::Verified => Ok(()),
        Outcome::Canceled => Err(AppError::Cancelled),
        Outcome::DeviceBusy => Err(AppError::Invalid(tr!("Windows Hello is busy. Try again."))),
        Outcome::RetriesExhausted => Err(AppError::Invalid(tr!("Too many failed attempts. Try again later."))),
        _ => Err(AppError::Invalid(tr!("Windows Hello couldn't verify you."))),
    }
}

fn failed(e: windows::core::Error) -> AppError {
    AppError::Invalid(tr!("Windows Hello failed: {why}", why = e.message()))
}
