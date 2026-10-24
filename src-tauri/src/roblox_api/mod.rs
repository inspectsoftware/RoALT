//! The one typed client for every Roblox web API RoALT uses.
//!
//! - One `reqwest::Client`, no cookie jar: `.ROBLOSECURITY` is set per request.
//! - CSRF: the real request is sent with the cached token for that account; a 403 carrying
//!   `x-csrf-token` stores the new token and retries once (any host).
//! - Rate limits: per-route next-slot throttle fed by `x-ratelimit-*`; 429 waits for
//!   `x-ratelimit-reset` (not `retry-after`), exponential backoff from 1 s, max 3 tries.
//! - Cookie rotation: a `Set-Cookie: .ROBLOSECURITY=` on any response goes to the sink
//!   (stored accounts) and is returned to the caller.
//! - Errors: 401 → `CookieInvalid` (`9002/2` → `BoundSessionRequired`), 403 with
//!   `rblx-challenge-*` → `Challenge`, exhausted 429 → `RateLimited`, else `Roblox(..)`.

pub mod endpoints;
pub mod types;

use std::sync::Arc;

use crate::error::Result;
use crate::vault::Secret;
use types::AuthenticatedUser;

/// Called with `(account_id, new cookie)` when Roblox rotates a stored account's cookie.
pub type RotationSink = Arc<dyn Fn(i64, Secret) + Send + Sync>;

pub struct Client {
    on_rotate: RotationSink,
}

impl Client {
    pub fn new(on_rotate: RotationSink) -> Client {
        let _ = &on_rotate;
        todo!("roblox_api: new")
    }

    /// `GET users.roblox.com/v1/users/authenticated`, the only reliable cookie check.
    /// `account_id` 0 = cookie not stored yet (no sink call). Returns the user and, if Roblox
    /// rotated the cookie during the call, the new value.
    pub async fn authenticated_user(&self, account_id: i64, cookie: &Secret) -> Result<(AuthenticatedUser, Option<Secret>)> {
        let _ = (account_id, cookie, &self.on_rotate);
        todo!("roblox_api: authenticated_user")
    }
}
