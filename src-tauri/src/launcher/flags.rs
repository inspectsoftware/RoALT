//! Lighter rendering for the clients RoALT starts (`Settings.lighter_rendering`), through the only
//! local flags Roblox still honours: its allowlist (DevForum "Allowlist for local client
//! configuration via Fast Flags"; any other is logged as "Denied local configuration for: ...")
//! in `<version folder>\ClientSettings\ClientAppSettings.json`. Lowest textures, no anti-aliasing
//! or grass, lowest geometry detail and render quality: less GPU memory and time per client.
//!
//! Every client shares the file and reads it once, as it starts; Roblox never writes it. A launch
//! merges `FLAGS` into the user's own flags and puts the file back exactly as it was once its
//! client has read its settings (inside `launch`'s `client_settings::LOCK` window, so another
//! launch never sees them by mistake). The original waits in a file beside it (`SAVED`), so a quit
//! or crash meanwhile loses nothing: `restore` at start and exit, or the next `apply`, puts it back.
//! Not under a bootstrapper: Bloxstrap and the like copy their own file over this one at every
//! launch.

use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use serde_json::{Map, Value};

use super::client_settings::{retry, set_readonly};

/// The allowlisted flags and their lightest values, as strings like Bloxstrap writes them.
const FLAGS: [(&str, &str); 10] = [
    ("DFFlagTextureQualityOverrideEnabled", "True"),
    ("DFIntTextureQualityOverride", "0"),
    ("FIntDebugForceMSAASamples", "1"),
    ("FIntFRMMinGrassDistance", "0"),
    ("FIntFRMMaxGrassDistance", "0"),
    ("DFIntCSGLevelOfDetailSwitchingDistance", "0"),
    ("DFIntCSGLevelOfDetailSwitchingDistanceL12", "0"),
    ("DFIntCSGLevelOfDetailSwitchingDistanceL23", "0"),
    ("DFIntCSGLevelOfDetailSwitchingDistanceL34", "0"),
    ("DFIntDebugFRMQualityLevelOverride", "1"),
];

/// The file's text with `FLAGS` set and the user's other flags kept (pure); None when it isn't a
/// JSON object (it is left alone then).
fn merge(text: Option<&str>) -> Option<String> {
    let mut flags: Map<String, Value> = match text.map(str::trim) {
        None | Some("") => Map::new(),
        Some(t) => serde_json::from_str(t).ok()?,
    };
    for (k, v) in FLAGS {
        flags.insert(k.into(), v.into());
    }
    serde_json::to_string_pretty(&flags).ok()
}

/// Replaces the file in one step; a read-only one stays read-only.
fn write(file: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let tmp = file.with_extension("json.roalt");
    let readonly = std::fs::metadata(file).is_ok_and(|m| m.permissions().readonly());
    let done = retry(|| std::fs::write(&tmp, bytes)).and_then(|()| {
        if readonly {
            set_readonly(file, false)?;
        }
        retry(|| std::fs::rename(&tmp, file))
    });
    if readonly && let Err(e) = set_readonly(file, true) {
        log::warn!("lighter rendering: couldn't make {} read-only again: {e}", file.display());
    }
    if done.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    done
}

/// The original beside the flag file: `1` then its bytes, or `0` when there was no file.
const SAVED: &str = "ClientAppSettings.json.roalt-before";

/// Puts back the original kept beside `file`, if one is; it stays there until that worked.
fn put_back_at(file: &Path) {
    let saved = file.with_file_name(SAVED);
    let Ok(kept) = std::fs::read(&saved) else { return };
    let done = match kept.split_first() {
        Some((b'1', bytes)) => write(file, bytes),
        _ => retry(|| std::fs::remove_file(file)).or_else(|e| if e.kind() == ErrorKind::NotFound { Ok(()) } else { Err(e) }),
    };
    match done.and_then(|()| retry(|| std::fs::remove_file(&saved))) {
        Ok(()) => {}
        Err(e) => log::warn!("lighter rendering: couldn't put {} back: {e}", file.display()),
    }
}

/// A launch's flags are in the file until this runs.
pub struct Undo {
    file: PathBuf,
}

impl Undo {
    /// Puts the file back exactly as it was (removed if there was none); failures are logged.
    pub fn put_back(self) {
        put_back_at(&self.file);
    }
}

/// Puts back what a launch cut short by a quit or crash left behind (at start, and at exit while a
/// launch may still be waiting for its client).
pub fn restore() {
    if let Ok(exe) = super::client::find()
        && let Some(dir) = exe.path.parent()
    {
        put_back_at(&dir.join("ClientSettings").join("ClientAppSettings.json"));
    }
}

/// Writes `FLAGS` next to the client at `exe` for the launch about to start; what to put back once
/// it has started, None when nothing was written. Failures are logged and never stop the launch.
pub fn apply(exe: &Path) -> Option<Undo> {
    let file = exe.parent()?.join("ClientSettings").join("ClientAppSettings.json");
    // A launch cut short: the user's own file first, or these flags would pass for it.
    put_back_at(&file);
    let before = match retry(|| std::fs::read(&file)) {
        Ok(b) => Some(b),
        Err(e) if e.kind() == ErrorKind::NotFound => None,
        Err(e) => {
            log::warn!("lighter rendering skipped: can't read {}: {e}", file.display());
            return None;
        }
    };
    let Some(text) = merge(before.as_deref().map(String::from_utf8_lossy).as_deref()) else {
        log::warn!("lighter rendering skipped: {} isn't a JSON object", file.display());
        return None;
    };
    let saved = file.with_file_name(SAVED);
    let kept = match &before {
        Some(bytes) => [b"1", bytes.as_slice()].concat(),
        None => b"0".to_vec(),
    };
    let written = file
        .parent()
        .map_or(Ok(()), std::fs::create_dir_all)
        .and_then(|()| std::fs::write(&saved, kept))
        .and_then(|()| write(&file, text.as_bytes()));
    match written {
        Ok(()) => Some(Undo { file }),
        Err(e) => {
            log::warn!("lighter rendering not written to {}: {e}", file.display());
            let _ = std::fs::remove_file(&saved);
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn merges_into_the_users_flags() {
        let v: Value = serde_json::from_str(&merge(None).unwrap()).unwrap();
        assert_eq!(v["DFIntTextureQualityOverride"], "0");
        assert_eq!(v.as_object().unwrap().len(), FLAGS.len());
        let own = r#"{"FFlagDebugGraphicsPreferD3D11": "True", "DFIntTextureQualityOverride": "3"}"#;
        let v: Value = serde_json::from_str(&merge(Some(own)).unwrap()).unwrap();
        assert_eq!(v["FFlagDebugGraphicsPreferD3D11"], "True", "the user's own flag stays");
        assert_eq!(v["DFIntTextureQualityOverride"], "0");
        assert!(merge(Some("  ")).is_some(), "blank counts as empty");
        for bad in ["not json", "[1, 2]", "{\"a\":"] {
            assert_eq!(merge(Some(bad)), None, "{bad}");
        }
    }

    #[test]
    fn puts_the_file_back_exactly() {
        let dir = std::env::temp_dir().join(format!("roalt-test-flags-{}", std::process::id()));
        let exe = dir.join("RobloxPlayerBeta.exe");
        let file = dir.join("ClientSettings").join("ClientAppSettings.json");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        // No file before: written, then removed again.
        let undo = apply(&exe).expect("written");
        assert!(std::fs::read_to_string(&file).unwrap().contains("DFIntDebugFRMQualityLevelOverride"));
        undo.put_back();
        assert!(!file.exists());

        // The user's own file: the very same bytes come back.
        let own = b"{\r\n  \"FFlagDebugSkyGray\": \"True\"\r\n}";
        std::fs::write(&file, own).unwrap();
        apply(&exe).expect("written").put_back();
        assert_eq!(std::fs::read(&file).unwrap(), own);

        // Cut short by a quit: the next launch doesn't mistake the flags for the user's own.
        let _left_behind = apply(&exe).expect("written");
        apply(&exe).expect("written").put_back();
        assert_eq!(std::fs::read(&file).unwrap(), own);
        assert!(!file.with_file_name(SAVED).exists());
        let _left_behind = apply(&exe).expect("written");
        put_back_at(&file);
        assert_eq!(std::fs::read(&file).unwrap(), own, "as `restore` does");

        // Not a JSON object: left alone.
        std::fs::write(&file, b"oops").unwrap();
        assert!(apply(&exe).is_none());
        assert_eq!(std::fs::read(&file).unwrap(), b"oops");
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
