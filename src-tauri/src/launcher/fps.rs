//! FPS cap through Roblox's own flag file, `<version folder>\ClientSettings\ClientAppSettings.json`:
//! `DFIntTaskSchedulerTargetFps` (on Roblox's flag allowlist) sets the client's target frame
//! rate. The client reads the file once, at start, so a launch writes its cap just before the
//! client starts and puts the file back as it was `SETTLE` later: Roblox started any other way
//! (the website, another manager) sees the user's own flags. Launches hold `LOCK` from the write
//! to the put-back, so one never reads another's cap. Other keys in the file are kept; a cap of
//! 0 writes nothing. Never under a bootstrapper (Bloxstrap, Fishstrap, ... write this file
//! themselves).

use std::ops::RangeInclusive;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::{Map, Value};

const KEY: &str = "DFIntTaskSchedulerTargetFps";

/// Accepted caps; 0 means "leave Roblox alone".
pub const RANGE: RangeInclusive<u32> = 5..=360;

/// How long a starting client gets to read the file before it is put back.
pub const SETTLE: Duration = Duration::from_secs(3);

/// Held by each launch from `apply` until its client has started (and, after a write, until
/// `Undo::put_back`), so concurrent launches (batch, rejoin, tray) never cross caps.
pub static LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// The file's new text with our key set to `fps`; every other key kept. None when `fps` is 0,
/// nothing changes, or the file isn't a JSON object (it is left alone). (pure)
pub fn merge(text: Option<&str>, fps: u32) -> Option<String> {
    if fps == 0 {
        return None;
    }
    let mut map: Map<String, Value> = match text.map(str::trim) {
        None | Some("") => Map::new(),
        Some(t) => serde_json::from_str(t).ok()?,
    };
    if map.get(KEY) == Some(&fps.into()) {
        return None;
    }
    map.insert(KEY.into(), fps.into());
    serde_json::to_string_pretty(&map).ok()
}

/// What the file was before `apply` wrote it.
pub struct Undo {
    file: PathBuf,
    /// None: there was no file.
    before: Option<String>,
}

impl Undo {
    /// Puts the file back as it was (removed if there was none). Failures are logged.
    pub fn put_back(self) {
        let r = match &self.before {
            Some(text) => write(&self.file, text),
            None => std::fs::remove_file(&self.file),
        };
        if let Err(e) = r {
            log::warn!("FPS cap: could not put {} back: {e}", self.file.display());
        }
    }
}

/// Replaces the file in one step (a starting client never reads half of it).
fn write(file: &Path, text: &str) -> std::io::Result<()> {
    let tmp = file.with_extension("json.roalt");
    std::fs::write(&tmp, text).and_then(|()| std::fs::rename(&tmp, file))
}

/// Writes `fps` (as `merge` says) next to the client at `exe`; what to put back once it has
/// started, None when nothing was written. Failures are logged and never stop the launch.
pub fn apply(exe: &Path, fps: u32) -> Option<Undo> {
    let dir = exe.parent()?.join("ClientSettings");
    let file = dir.join("ClientAppSettings.json");
    let before = std::fs::read_to_string(&file).ok();
    let Some(new) = merge(before.as_deref(), fps) else {
        if fps != 0 && before.as_deref().is_some_and(|t| serde_json::from_str::<Map<String, Value>>(t).is_err() && !t.trim().is_empty()) {
            log::warn!("FPS cap skipped: {} isn't a JSON object", file.display());
        }
        return None;
    };
    match std::fs::create_dir_all(&dir).and_then(|()| write(&file, &new)) {
        Ok(()) => {
            log::info!("FPS cap {fps} written to {}", file.display());
            Some(Undo { file, before })
        }
        Err(e) => {
            log::warn!("FPS cap not written to {}: {e}", file.display());
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::merge;

    #[test]
    fn merges_into_the_users_flags() {
        assert_eq!(merge(None, 0), None, "0 writes nothing");
        let v: serde_json::Value = serde_json::from_str(&merge(None, 15).unwrap()).unwrap();
        assert_eq!(v, serde_json::json!({ "DFIntTaskSchedulerTargetFps": 15 }));
        let own = r#"{"FFlagDebugGraphicsPreferD3D11": "True", "DFIntTaskSchedulerTargetFps": 60}"#;
        let v: serde_json::Value = serde_json::from_str(&merge(Some(own), 144).unwrap()).unwrap();
        assert_eq!(v, serde_json::json!({ "FFlagDebugGraphicsPreferD3D11": "True", "DFIntTaskSchedulerTargetFps": 144 }));
        assert_eq!(merge(Some(own), 0), None, "the user's own cap is kept");
        assert_eq!(merge(Some(own), 60), None, "already set");
        assert!(merge(Some("  "), 30).is_some(), "blank counts as empty");

        for bad in ["not json", "[1, 2]", "{\"a\":"] {
            assert_eq!(merge(Some(bad), 30), None, "{bad}");
        }
    }
}
