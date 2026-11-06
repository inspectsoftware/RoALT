//! Which alt users are signed in (`WTSEnumerateSessionsW`). Signing a session out goes only
//! through its control API (`POST /v1/logoff`): the AltDock there closes its clients first.

use windows::Win32::System::RemoteDesktop::{
    WTS_CONNECTSTATE_CLASS, WTS_CURRENT_SERVER_HANDLE, WTS_SESSION_INFOW, WTSActive, WTSConnectQuery, WTSConnected, WTSDisconnected,
    WTSEnumerateSessionsW, WTSFreeMemory, WTSQuerySessionInformationW, WTSShadow, WTSUserName,
};
use windows::core::PWSTR;

use super::types::WtsState;

/// One Windows session: its user (may be empty) and state.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Seen {
    pub id: u32,
    pub user: String,
    pub state: WtsState,
}

/// A session's state as the Sessions page shows it (pure); listeners and idle slots are None.
fn state(s: WTS_CONNECTSTATE_CLASS) -> WtsState {
    match s {
        x if x == WTSActive || x == WTSConnected || x == WTSConnectQuery || x == WTSShadow => WtsState::Active,
        x if x == WTSDisconnected => WtsState::Disconnected,
        _ => WtsState::None,
    }
}

/// Every Windows session with a user.
pub fn sessions() -> Vec<Seen> {
    let mut out = Vec::new();
    let mut list: *mut WTS_SESSION_INFOW = std::ptr::null_mut();
    let mut count = 0u32;
    // SAFETY: the list is allocated by WTS and freed below.
    if let Err(e) = unsafe { WTSEnumerateSessionsW(Some(WTS_CURRENT_SERVER_HANDLE), 0, 1, &mut list, &mut count) } {
        log::debug!("could not list Windows sessions: {e}");
        return out;
    }
    // SAFETY: `list` holds `count` entries until WTSFreeMemory.
    for s in unsafe { std::slice::from_raw_parts(list, count as usize) } {
        let mut name = PWSTR::null();
        let mut len = 0u32;
        // SAFETY: the name is allocated by WTS and freed below.
        if unsafe { WTSQuerySessionInformationW(Some(WTS_CURRENT_SERVER_HANDLE), s.SessionId, WTSUserName, &mut name, &mut len) }.is_ok() {
            // SAFETY: a NUL-terminated string from WTS.
            let user = unsafe { name.to_string() }.unwrap_or_default();
            // SAFETY: allocated by WTSQuerySessionInformationW.
            unsafe { WTSFreeMemory(name.0.cast()) };
            if !user.is_empty() {
                out.push(Seen { id: s.SessionId, user, state: state(s.State) });
            }
        }
    }
    // SAFETY: allocated by WTSEnumerateSessionsW.
    unsafe { WTSFreeMemory(list.cast()) };
    out
}

/// `user`'s state among `seen` (pure): active beats disconnected beats signed out.
pub fn state_of(seen: &[Seen], user: &str) -> WtsState {
    let mine = || seen.iter().filter(|s| s.user.eq_ignore_ascii_case(user));
    if mine().any(|s| s.state == WtsState::Active) {
        WtsState::Active
    } else if mine().any(|s| s.state == WtsState::Disconnected) {
        WtsState::Disconnected
    } else {
        WtsState::None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn states_per_user() {
        let seen = |id, user: &str, state| Seen { id, user: user.into(), state };
        let list = [
            seen(1, "you", WtsState::Active),
            seen(2, "altdock-1", WtsState::Disconnected),
            seen(3, "AltDock-2", WtsState::Active),
            seen(4, "altdock-2", WtsState::Disconnected),
        ];
        assert_eq!(state_of(&list, "altdock-1"), WtsState::Disconnected);
        assert_eq!(state_of(&list, "altdock-2"), WtsState::Active);
        assert_eq!(state_of(&list, "altdock-3"), WtsState::None);
        assert_eq!(state(WTSDisconnected), WtsState::Disconnected);
        assert_eq!(state(WTSActive), WtsState::Active);
        assert_eq!(state(windows::Win32::System::RemoteDesktop::WTSListen), WtsState::None);
    }
}
