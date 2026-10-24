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
#[cfg(test)]
mod tests;
pub mod types;

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use reqwest::header::{ACCEPT, COOKIE, HeaderMap, HeaderValue, ORIGIN, REFERER, RETRY_AFTER, SET_COOKIE};
use reqwest::{Method, StatusCode, Url};
use serde::de::DeserializeOwned;
use serde_json::Value;
use zeroize::Zeroizing;

use crate::error::{AppError, Result};
use crate::vault::Secret;
use types::{AuthenticatedUser, Presences, Profiles, SocialCounts, Thumbnails, TwoStepConfig, UserDetails, UserPresence};

/// Called with `(account_id, new cookie)` when Roblox rotates a stored account's cookie.
pub type RotationSink = Arc<dyn Fn(i64, Secret) + Send + Sync>;

const USER_AGENT: &str =
    "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/141.0.0.0 Safari/537.36";
const MAX_ATTEMPTS: u32 = 3;
const CSRF: &str = "x-csrf-token";
const CHALLENGE: &str = "rblx-challenge-id";
const HEADSHOT_RETRIES: u32 = 5;
#[cfg(not(test))]
const HEADSHOT_STEP: Duration = Duration::from_millis(500);
#[cfg(test)]
const HEADSHOT_STEP: Duration = Duration::from_millis(1);
/// 2-step `mediaType` names by enum value.
const MEDIA_TYPES: [&str; 8] =
    ["Email", "SMS", "Authenticator", "RecoveryCode", "SecurityKey", "CrossDevice", "Password", "Passkey"];

/// Headers, body and the rotated cookie (if any) of a 2xx answer.
type Reply = (HeaderMap, Vec<u8>, Option<Secret>);

/// What a route's last answer said about its rate-limit window.
struct Window {
    remaining: u64,
    reset_at: Instant,
}

pub struct Client {
    http: std::result::Result<reqwest::Client, String>,
    on_rotate: RotationSink,
    /// Replaces scheme and host of every URL (tests).
    base: Option<String>,
    csrf: Mutex<HashMap<i64, String>>,
    windows: Mutex<HashMap<&'static str, Window>>,
}

impl Client {
    pub fn new(on_rotate: RotationSink) -> Client {
        Client::with_base(on_rotate, None)
    }

    fn with_base(on_rotate: RotationSink, base: Option<String>) -> Client {
        let accept = HeaderValue::from_static("application/json, text/plain, */*");
        let http = reqwest::Client::builder()
            .user_agent(USER_AGENT)
            .default_headers(HeaderMap::from_iter([(ACCEPT, accept)]))
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(20))
            .build()
            .map_err(|e| e.to_string());
        Client { http, on_rotate, base, csrf: Mutex::default(), windows: Mutex::default() }
    }

    /// `GET users.roblox.com/v1/users/authenticated`, the only reliable cookie check.
    /// `account_id` 0 = cookie not stored yet (no sink call). Returns the user and, if Roblox
    /// rotated the cookie during the call, the new value.
    pub async fn authenticated_user(&self, account_id: i64, cookie: &Secret) -> Result<(AuthenticatedUser, Option<Secret>)> {
        let url = endpoints::USERS_AUTHENTICATED;
        let (_, body, rotated) = self.request(Method::GET, url, url, None, account_id, Some(cookie)).await?;
        Ok((json(&body)?, rotated))
    }

    /// `GET users.roblox.com/v1/users/{id}` (public). Sent with the account's cookie when
    /// one is given (higher limit than anonymous: 30/min).
    pub async fn user_details(&self, account_id: i64, cookie: Option<&Secret>, user_id: i64) -> Result<UserDetails> {
        self.get(endpoints::USER, &endpoints::fill(endpoints::USER, user_id), account_id, cookie).await
    }

    /// Avatar headshot URLs, 150x150 PNG, no cookie. ≤100 ids per call (chunks larger
    /// lists); re-requests only the `Pending` ids up to 5 times (0.5 s, 1 s, … steps).
    /// Ids that end Blocked / Error / still Pending, or with an empty `imageUrl`, are absent.
    pub async fn headshots(&self, user_ids: &[i64]) -> Result<HashMap<i64, String>> {
        let mut out = HashMap::new();
        for chunk in valid_ids(user_ids).chunks(100) {
            let mut pending = chunk.to_vec();
            for retry in 0..=HEADSHOT_RETRIES {
                if retry > 0 {
                    tokio::time::sleep(HEADSHOT_STEP * retry).await;
                }
                let url = endpoints::fill(endpoints::HEADSHOTS, csv(&pending));
                let page: Thumbnails = self.get(endpoints::HEADSHOTS, &url, 0, None).await?;
                pending = sort_headshots(page, &mut out);
                if pending.is_empty() {
                    break;
                }
            }
        }
        Ok(out)
    }

    /// `GET economy.roblox.com/v1/user/currency` → `robux` of the cookie's own account.
    pub async fn robux(&self, account_id: i64, cookie: &Secret) -> Result<i64> {
        let url = endpoints::CURRENCY;
        field(self.get(url, url, account_id, Some(cookie)).await?, "robux")
    }

    /// Legacy Premium: `GET premiumfeatures.roblox.com/v1/users/{id}/validate-membership`
    /// (body is a bare JSON boolean). Marked for removal by Roblox; callers treat errors as unknown.
    pub async fn premium(&self, account_id: i64, cookie: &Secret, user_id: i64) -> Result<bool> {
        self.get(endpoints::PREMIUM, &endpoints::fill(endpoints::PREMIUM, user_id), account_id, Some(cookie)).await
    }

    /// Roblox Plus via `POST apis.roblox.com/user-profile-api/v1/user/profiles/get-profiles`
    /// `{userIds, fields:["hasRobloxSubscription"]}`, no cookie, ≤200 ids per call. A row
    /// without the field (unknown user, renamed field) is absent: never guess `false`.
    pub async fn plus(&self, user_ids: &[i64]) -> Result<HashMap<i64, bool>> {
        let (url, mut out) = (endpoints::PROFILES, HashMap::new());
        for chunk in valid_ids(user_ids).chunks(200) {
            let body = serde_json::json!({ "userIds": chunk, "fields": ["hasRobloxSubscription"] });
            let (_, bytes, _) = self.request(Method::POST, url, url, Some(&body), 0, None).await?;
            let page: Profiles = json(&bytes)?;
            out.extend(page.profile_details.into_iter().filter_map(|p| Some((p.user_id, p.has_roblox_subscription?))));
        }
        Ok(out)
    }

    /// `friends.roblox.com/v1/users/{id}/{friends,followers,followings}/count` (public).
    pub async fn social_counts(&self, user_id: i64) -> Result<SocialCounts> {
        let count = move |route: &'static str| async move {
            field::<i64>(self.get(route, &endpoints::fill(route, user_id), 0, None).await?, "count")
        };
        let (friends, followers, following) = tokio::try_join!(
            count(endpoints::FRIENDS_COUNT),
            count(endpoints::FOLLOWERS_COUNT),
            count(endpoints::FOLLOWINGS_COUNT)
        )?;
        Ok(SocialCounts { friends, followers, following })
    }

    /// `POST presence.roblox.com/v1/presence/users {userIds}` as the viewer (its cookie),
    /// ≤50 ids per call (chunks), ids ≤ 0 dropped and duplicates removed before sending.
    pub async fn presence(&self, account_id: i64, cookie: &Secret, user_ids: &[i64]) -> Result<Vec<UserPresence>> {
        let (url, mut out) = (endpoints::PRESENCE, Vec::new());
        for chunk in valid_ids(user_ids).chunks(50) {
            let body = serde_json::json!({ "userIds": chunk });
            let (_, bytes, _) = self.request(Method::POST, url, url, Some(&body), account_id, Some(cookie)).await?;
            out.extend(json::<Presences>(&bytes)?.user_presences);
        }
        Ok(out)
    }

    /// `GET accountsettings.roblox.com/v1/email` → `verified`. The address is not kept.
    pub async fn email_verified(&self, account_id: i64, cookie: &Secret) -> Result<bool> {
        let url = endpoints::EMAIL;
        field(self.get(url, url, account_id, Some(cookie)).await?, "verified")
    }

    /// `GET twostepverification.roblox.com/v1/users/{id}/configuration` → names of the
    /// enabled methods ("Email", "SMS", "Authenticator", "SecurityKey", "Passkey", …).
    /// `mediaType` may arrive as a number or a string.
    pub async fn two_step_methods(&self, account_id: i64, cookie: &Secret, user_id: i64) -> Result<Vec<String>> {
        let url = endpoints::fill(endpoints::TWO_STEP, user_id);
        let config: TwoStepConfig = self.get(endpoints::TWO_STEP, &url, account_id, Some(cookie)).await?;
        Ok(enabled_methods(config))
    }

    /// A GET whose JSON answer is `T`.
    async fn get<T: DeserializeOwned>(
        &self,
        route: &'static str,
        url: &str,
        account_id: i64,
        cookie: Option<&Secret>,
    ) -> Result<T> {
        let (_, body, _) = self.request(Method::GET, route, url, None, account_id, cookie).await?;
        json(&body)
    }

    /// One API call with throttle, CSRF retry, 429 backoff and cookie rotation.
    /// `route` keys the throttle (one per endpoint, not per URL). No cookie: no `Cookie`
    /// header, CSRF cached as account 0, no rotation.
    async fn request(
        &self,
        method: Method,
        route: &'static str,
        url: &str,
        body: Option<&Value>,
        account_id: i64,
        cookie: Option<&Secret>,
    ) -> Result<Reply> {
        let account_id = if cookie.is_some() { account_id } else { 0 };
        let http = self.http.as_ref().map_err(|e| AppError::Internal(format!("HTTP client unavailable: {e}")))?;
        let (url, web) = self.target(url)?;
        let (mut rotated, mut csrf_retried, mut attempts) = (None::<Secret>, false, 0);
        loop {
            self.wait_turn(route).await;
            let current = rotated.as_ref().or(cookie);
            let mut req = http.request(method.clone(), url.clone());
            if let Some(current) = current {
                req = req.header(COOKIE, cookie_header(current)?);
            }
            if web {
                req = req.header(REFERER, endpoints::WWW).header(ORIGIN, endpoints::WEB_ORIGIN);
            }
            let token = lock(&self.csrf).get(&account_id).cloned();
            if (method != Method::GET || csrf_retried)
                && let Some(token) = token
            {
                req = req.header(CSRF, token);
            }
            if let Some(body) = body {
                req = req.json(body);
            }

            let resp = req.send().await.map_err(network)?;
            let (status, headers) = (resp.status(), resp.headers().clone());
            let bytes = resp.bytes().await.map_err(network)?;
            let limits = RateLimit::parse(&headers);
            if let (Some(remaining), Some(reset)) = (limits.remaining, limits.reset_secs()) {
                self.set_window(route, remaining, reset);
            }
            if let Some(current) = current
                && let Some(new) = rotated_cookie(&headers).filter(|new| new.expose() != current.expose())
            {
                if account_id != 0 {
                    (self.on_rotate)(account_id, new.clone());
                }
                rotated = Some(new);
            }

            if status.is_success() {
                return Ok((headers, bytes.to_vec(), rotated));
            }
            if status == StatusCode::TOO_MANY_REQUESTS {
                attempts += 1;
                let wait = limits.reset.or(limits.retry_after).unwrap_or(1 << (attempts - 1)).max(1);
                self.set_window(route, 0, wait);
                if attempts >= MAX_ATTEMPTS {
                    return Err(AppError::RateLimited(wait));
                }
                log::info!("rate limited on {route}, retrying in {wait} s");
                continue;
            }
            if status == StatusCode::FORBIDDEN
                && !csrf_retried
                && !headers.contains_key(CHALLENGE)
                && let Some(token) = headers.get(CSRF).and_then(|v| v.to_str().ok())
            {
                lock(&self.csrf).insert(account_id, token.to_owned());
                csrf_retried = true;
                continue;
            }
            return Err(classify(status, &headers, &bytes));
        }
    }

    /// The URL to call and whether it gets the website's `Referer` / `Origin`.
    fn target(&self, url: &str) -> Result<(Url, bool)> {
        let bad = |_| AppError::Internal("bad Roblox URL".into());
        let mut u = Url::parse(url).map_err(bad)?;
        let web = matches!(u.host_str(), Some("auth.roblox.com" | "apis.roblox.com"));
        if let Some(base) = &self.base {
            u = Url::parse(&format!("{base}{}", &u[url::Position::BeforePath..])).map_err(bad)?;
        }
        Ok((u, web))
    }

    /// Waits while `route`'s window is used up, else takes one request from it.
    async fn wait_turn(&self, route: &'static str) {
        loop {
            let wait = {
                let mut windows = lock(&self.windows);
                let now = Instant::now();
                match windows.get_mut(route) {
                    Some(w) if now < w.reset_at && w.remaining == 0 => w.reset_at - now,
                    Some(w) if now < w.reset_at => {
                        w.remaining -= 1;
                        return;
                    }
                    _ => return,
                }
            };
            tokio::time::sleep(wait).await;
        }
    }

    fn set_window(&self, route: &'static str, remaining: u64, reset_secs: u64) {
        let reset_at = Instant::now() + Duration::from_secs(reset_secs);
        lock(&self.windows).insert(route, Window { remaining, reset_at });
    }
}

/// The `x-ratelimit-*` and `retry-after` headers of one answer.
#[derive(Debug, Default, PartialEq)]
struct RateLimit {
    /// Window length in seconds, the `w=` of `x-ratelimit-limit: 20, 20;w=60`.
    window: Option<u64>,
    remaining: Option<u64>,
    /// Seconds until the window resets.
    reset: Option<u64>,
    retry_after: Option<u64>,
}

impl RateLimit {
    fn parse(h: &HeaderMap) -> RateLimit {
        let get = |name: &str| h.get(name).and_then(|v| v.to_str().ok());
        RateLimit {
            window: get("x-ratelimit-limit").and_then(|v| v.split_once("w=")).and_then(|(_, w)| first_number(w)),
            remaining: get("x-ratelimit-remaining").and_then(first_number),
            reset: get("x-ratelimit-reset").and_then(first_number),
            retry_after: get(RETRY_AFTER.as_str()).and_then(first_number),
        }
    }

    /// `x-ratelimit-reset`, else the next window boundary (Roblox aligns windows to the clock).
    fn reset_secs(&self) -> Option<u64> {
        self.reset.or_else(|| {
            let w = self.window.filter(|&w| w > 0)?;
            let now = SystemTime::now().duration_since(UNIX_EPOCH).ok()?.as_secs();
            Some(w - now % w)
        })
    }
}

/// The leading integer of `"20, 20;w=60"`-style values.
fn first_number(v: &str) -> Option<u64> {
    v.split([',', ';']).next()?.trim().parse().ok()
}

/// A new `.ROBLOSECURITY` among the `Set-Cookie` headers, last one wins. Deletions
/// (empty value, `Max-Age=0`, a 1970 expiry) are not a rotation.
fn rotated_cookie(h: &HeaderMap) -> Option<Secret> {
    let parse = |v: &str| {
        let mut parts = v.split(';');
        let value = parts.next()?.trim().strip_prefix(".ROBLOSECURITY=")?.trim();
        let deleted = value.is_empty()
            || parts.any(|a| {
                let a = a.trim().to_ascii_lowercase();
                a == "max-age=0" || a.starts_with("max-age=-") || (a.starts_with("expires=") && a.contains("1970"))
            });
        (!deleted).then(|| Secret::new(value))
    };
    h.get_all(SET_COOKIE).iter().filter_map(|v| v.to_str().ok()).filter_map(parse).next_back()
}

fn cookie_header(cookie: &Secret) -> Result<HeaderValue> {
    let raw = Zeroizing::new(format!(".ROBLOSECURITY={}", cookie.expose()));
    let mut v = HeaderValue::from_str(&raw).map_err(|_| AppError::CookieInvalid)?;
    v.set_sensitive(true);
    Ok(v)
}

/// Maps a final non-2xx answer. Never quotes the body: it may echo what was sent.
fn classify(status: StatusCode, headers: &HeaderMap, body: &[u8]) -> AppError {
    match status {
        StatusCode::UNAUTHORIZED if error_code(body) == Some((9002, 2)) => AppError::BoundSessionRequired,
        StatusCode::UNAUTHORIZED => AppError::CookieInvalid,
        StatusCode::FORBIDDEN if headers.contains_key(CHALLENGE) => AppError::Challenge,
        s => AppError::Roblox(format!("HTTP {}", s.as_u16())),
    }
}

/// `(code, subcode)` of `{errors:[{..}]}` or a flat `{code, ..}` body. Bare strings,
/// `{Error: ..}` and empty bodies have none.
fn error_code(body: &[u8]) -> Option<(i64, i64)> {
    let v: Value = serde_json::from_slice(body).ok()?;
    let e = v.get("errors").and_then(|e| e.get(0)).unwrap_or(&v);
    Some((e.get("code")?.as_i64()?, e.get("subcode").and_then(Value::as_i64).unwrap_or(0)))
}

/// A transport error, named by host only (no path or query).
fn network(e: reqwest::Error) -> AppError {
    let what = if e.is_timeout() {
        "timed out"
    } else if e.is_connect() {
        "could not connect"
    } else {
        "connection failed"
    };
    match e.url().and_then(Url::host_str) {
        Some(host) => AppError::Network(format!("{what} ({host})")),
        None => AppError::Network(what.into()),
    }
}

fn json<T: DeserializeOwned>(body: &[u8]) -> Result<T> {
    serde_json::from_slice(body).map_err(|_| unreadable())
}

/// One top-level field of a JSON object answer; missing or mistyped is unreadable.
fn field<T: DeserializeOwned>(mut v: Value, name: &str) -> Result<T> {
    serde_json::from_value(v.get_mut(name).map(Value::take).unwrap_or_default()).map_err(|_| unreadable())
}

fn unreadable() -> AppError {
    AppError::Roblox("unreadable response".into())
}

/// Positive ids, each once (Roblox fails a whole presence batch on one id ≤ 0).
fn valid_ids(ids: &[i64]) -> Vec<i64> {
    let mut ids: Vec<i64> = ids.iter().copied().filter(|&id| id > 0).collect();
    ids.sort_unstable();
    ids.dedup();
    ids
}

fn csv(ids: &[i64]) -> String {
    ids.iter().map(i64::to_string).collect::<Vec<_>>().join(",")
}

/// Keeps Completed rows with a URL in `out`; returns the ids still Pending.
fn sort_headshots(page: Thumbnails, out: &mut HashMap<i64, String>) -> Vec<i64> {
    let mut pending = Vec::new();
    for t in page.data {
        match (t.state.as_str(), t.image_url) {
            ("Completed", Some(url)) if !url.is_empty() => {
                out.insert(t.target_id, url);
            }
            ("Pending", _) => pending.push(t.target_id),
            _ => {}
        }
    }
    pending
}

/// Names of the enabled 2-step methods; recovery codes and the password don't count.
fn enabled_methods(config: TwoStepConfig) -> Vec<String> {
    let name = |v: &Value| match v {
        Value::Number(n) => MEDIA_TYPES.get(usize::try_from(n.as_u64()?).ok()?).copied(),
        Value::String(s) => MEDIA_TYPES.iter().find(|m| m.eq_ignore_ascii_case(s)).copied(),
        _ => None,
    };
    config
        .methods
        .iter()
        .filter(|m| m.enabled)
        .filter_map(|m| name(&m.media_type))
        .filter(|n| !matches!(*n, "RecoveryCode" | "Password"))
        .map(String::from)
        .collect()
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}
