//! RoALT's online counter, switched off in AltDock: RoALT pinged its own Cloudflare Worker with a
//! launch id and how many clients ran. AltDock has no counter and sends nothing; `ping` stays so the
//! command and its binding keep their shape, and always answers None (the UI hides the count).

use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// A counter's answer, passed on to the UI as it is.
#[derive(Serialize, Deserialize, TS, Clone, Debug, PartialEq, Eq)]
#[ts(export)]
pub struct Online {
    /// Apps running now.
    pub online: u32,
    /// Their Roblox clients added up; None from a counter older than this field.
    #[serde(default)]
    pub playing: Option<u32>,
}

/// No counter: never sends anything, always None.
pub async fn ping(_playing: usize) -> Option<Online> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn sends_nothing() {
        assert_eq!(ping(3).await, None);
    }
}
