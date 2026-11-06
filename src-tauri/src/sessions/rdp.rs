//! The `.rdp` file mstsc opens for a session (`<data>\sessions\<name>.rdp`). Pure: building it,
//! reading one back, and telling whether a signed file still says the same.

use std::path::{Path, PathBuf};

use super::types::SessionConfig;

/// `<data folder>\sessions\<name>.rdp`.
pub fn path(data_dir: &Path, name: &str) -> PathBuf {
    data_dir.join("sessions").join(format!("{name}.rdp"))
}

/// The file's settings, one per line (CRLF, as mstsc writes them). `username` is how the
/// Credential Manager entry names the alt user (`creds::user_name`).
pub fn build(c: &SessionConfig, username: &str) -> String {
    let lines = [
        format!("full address:s:{}", c.address),
        format!("username:s:{username}"),
        // A fixed size: the clients' windows must not change size with the mstsc window.
        "screen mode id:i:1".into(),
        "use multimon:i:0".into(),
        format!("desktopwidth:i:{}", c.width),
        format!("desktopheight:i:{}", c.height),
        "session bpp:i:32".into(),
        "smart sizing:i:0".into(),
        "dynamic resolution:i:0".into(),
        // Sound stays in the session (nothing plays here).
        "audiomode:i:2".into(),
        "audiocapturemode:i:0".into(),
        // Nothing of this PC goes into the session.
        "redirectclipboard:i:0".into(),
        "redirectprinters:i:0".into(),
        "redirectdrives:i:0".into(),
        "drivestoredirect:s:".into(),
        "redirectcomports:i:0".into(),
        "redirectsmartcards:i:0".into(),
        "redirectwebauthn:i:0".into(),
        "devicestoredirect:s:".into(),
        "usbdevicestoredirect:s:".into(),
        "camerastoredirect:s:".into(),
        "redirectposdevices:i:0".into(),
        "autoreconnection enabled:i:1".into(),
        // Credentials come from Credential Manager (`TERMSRV/<address>`); never a prompt.
        "prompt for credentials:i:0".into(),
        "promptcredentialonce:i:0".into(),
        "authentication level:i:0".into(),
        "enablecredsspsupport:i:1".into(),
        "negotiate security layer:i:1".into(),
        "connection type:i:7".into(),
        "networkautodetect:i:0".into(),
        "bandwidthautodetect:i:1".into(),
        "displayconnectionbar:i:0".into(),
        "disable wallpaper:i:1".into(),
        "allow font smoothing:i:0".into(),
        "allow desktop composition:i:1".into(),
        "bitmapcachepersistenable:i:1".into(),
        "administrative session:i:0".into(),
    ];
    let mut out = lines.join("\r\n");
    out.push_str("\r\n");
    out
}

/// UTF-16LE with a byte order mark, as mstsc and rdpsign write `.rdp` files.
pub fn encode(text: &str) -> Vec<u8> {
    let mut out = vec![0xFF, 0xFE];
    out.extend(text.encode_utf16().flat_map(u16::to_le_bytes));
    out
}

/// A `.rdp` file's text: UTF-16LE (with its BOM) or UTF-8.
pub fn decode(bytes: &[u8]) -> String {
    match bytes {
        [0xFF, 0xFE, rest @ ..] => {
            String::from_utf16_lossy(&rest.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect::<Vec<_>>())
        }
        [0xEF, 0xBB, 0xBF, rest @ ..] => String::from_utf8_lossy(rest).into_owned(),
        _ => String::from_utf8_lossy(bytes).into_owned(),
    }
}

/// The settings of a `.rdp` text without rdpsign's `signscope`/`signature` lines, one per line,
/// in order, blank lines dropped.
fn settings(text: &str) -> Vec<&str> {
    text.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with("signscope:") && !l.starts_with("signature:"))
        .collect()
}

/// rdpsign signed this text.
pub fn is_signed(text: &str) -> bool {
    text.lines().any(|l| l.trim().starts_with("signature:s:") && l.trim().len() > "signature:s:".len())
}

/// The file on disk (maybe signed since) still says what `built` says: keep it, and its signature.
pub fn same_settings(on_disk: &str, built: &str) -> bool {
    let (mut a, mut b) = (settings(on_disk), settings(built));
    // rdpsign may reorder what it signs; the same settings are what counts.
    a.sort_unstable();
    b.sort_unstable();
    a == b
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sessions::types::SessionMode;

    fn cfg() -> SessionConfig {
        SessionConfig {
            name: "Farm".into(),
            windows_user: "altdock-1".into(),
            address: "127.0.0.2".into(),
            port: 47611,
            mode: SessionMode::Shared,
            account_ids: vec![],
            width: 1280,
            height: 720,
        }
    }

    #[test]
    fn builds_the_session_file() {
        let t = build(&cfg(), r"PC\altdock-1");
        for want in [
            "full address:s:127.0.0.2",
            r"username:s:PC\altdock-1",
            "desktopwidth:i:1280",
            "desktopheight:i:720",
            "session bpp:i:32",
            "smart sizing:i:0",
            "dynamic resolution:i:0",
            "screen mode id:i:1",
            "audiomode:i:2",
            "redirectclipboard:i:0",
            "redirectprinters:i:0",
            "redirectdrives:i:0",
            "autoreconnection enabled:i:1",
            "prompt for credentials:i:0",
            "authentication level:i:0",
        ] {
            assert!(t.lines().any(|l| l == want), "missing {want}");
        }
        assert!(t.ends_with("\r\n") && !t.contains("\n\n"));
        assert!(!t.contains("password"), "never a password in the file");
        assert!(!is_signed(&t));
    }

    #[test]
    fn utf16_round_trip() {
        let t = build(&cfg(), "x");
        let bytes = encode(&t);
        assert_eq!(&bytes[..4], &[0xFF, 0xFE, b'f', 0]);
        assert_eq!(decode(&bytes), t);
        assert_eq!(decode(t.as_bytes()), t);
        assert_eq!(decode(b"\xEF\xBB\xBFa:s:b"), "a:s:b");
    }

    #[test]
    fn a_signed_file_with_the_same_settings_is_kept() {
        let t = build(&cfg(), "x");
        let signed = format!("{t}signscope:s:Full Address,Username\r\nsignature:s:AQABAAEAAAD9\r\n");
        assert!(is_signed(&signed));
        assert!(same_settings(&signed, &t));
        let other = build(&SessionConfig { width: 1920, ..cfg() }, "x");
        assert!(!same_settings(&signed, &other));
        assert!(!is_signed("signature:s:\r\n"));
    }

    #[test]
    fn file_path() {
        assert_eq!(path(Path::new("D"), "Farm"), Path::new("D").join("sessions").join("Farm.rdp"));
    }
}
