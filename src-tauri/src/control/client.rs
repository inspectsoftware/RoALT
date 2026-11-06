//! The main app's side of the control API (`control::server`): one method per endpoint, to
//! `http://127.0.0.1:<port>` with the session's token. Never through a proxy, never follows a
//! redirect; 5 s per request, longer only for the few that wait on work (`SLOW`).

use std::time::Duration;

use base64::Engine;
use reqwest::{Method, RequestBuilder};
use serde::Serialize;
use serde::de::DeserializeOwned;
use zeroize::Zeroizing;

use super::TOKEN_HEADER;
use super::types::{self, *};
use crate::error::{AppError, Result};

const TIMEOUT: Duration = Duration::from_secs(5);
/// Closing waits for the client's grace period (`settings.close_grace_s`), kill all for every
/// Roblox process, an import checks each replaced cookie with Roblox.
const SLOW: Duration = Duration::from_secs(120);

/// A session's control API.
pub struct ControlClient {
    http: reqwest::Client,
    base: String,
    token: Zeroizing<String>,
}

impl std::fmt::Debug for ControlClient {
    /// Never the token.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ControlClient").field("base", &self.base).finish_non_exhaustive()
    }
}

impl ControlClient {
    pub fn new(port: u16, token: Zeroizing<String>) -> Result<ControlClient> {
        let http = reqwest::Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(TIMEOUT)
            .build()
            .map_err(|e| AppError::Internal(format!("control client unavailable: {}", e.without_url())))?;
        Ok(ControlClient { http, base: format!("http://127.0.0.1:{port}"), token })
    }

    fn request(&self, method: Method, path: &str) -> RequestBuilder {
        self.http.request(method, format!("{}{path}", self.base)).header(TOKEN_HEADER, self.token.as_str())
    }

    /// Sends it; a refusal comes back as the session's own error (`error_of`).
    async fn send(&self, req: RequestBuilder) -> Result<reqwest::Response> {
        let r = req.send().await.map_err(|e| AppError::Network(unreachable(&e)))?;
        if r.status().is_success() {
            return Ok(r);
        }
        let status = r.status();
        Err(match r.json::<ControlError>().await {
            Ok(e) => error_of(e),
            Err(_) => AppError::Internal(format!("the session's AltDock answered HTTP {}", status.as_u16())),
        })
    }

    async fn json<T: DeserializeOwned>(&self, req: RequestBuilder) -> Result<T> {
        self.send(req).await?.json().await.map_err(|e| AppError::Internal(format!("the session's AltDock answered unexpectedly: {}", e.without_url())))
    }

    async fn get<T: DeserializeOwned>(&self, path: &str) -> Result<T> {
        self.json(self.request(Method::GET, path)).await
    }

    async fn post<B: Serialize + ?Sized, T: DeserializeOwned>(&self, path: &str, body: &B) -> Result<T> {
        self.json(self.request(Method::POST, path).json(body)).await
    }

    /// A POST whose answer is `{}`.
    async fn act<B: Serialize + ?Sized>(&self, path: &str, body: &B, timeout: Duration) -> Result<()> {
        self.send(self.request(Method::POST, path).json(body).timeout(timeout)).await.map(drop)
    }

    pub async fn health(&self) -> Result<ControlHealth> {
        self.get("/v1/health").await
    }

    pub async fn state(&self) -> Result<Vec<types::ControlClient>> {
        self.get("/v1/state").await
    }

    pub async fn accounts(&self) -> Result<Vec<ControlAccount>> {
        self.get("/v1/accounts").await
    }

    /// Sends a `.roalt` backup (`vault::backup::write`'s bytes) and its password.
    pub async fn import_accounts(&self, backup: &[u8], password: Zeroizing<String>) -> Result<ImportAnswer> {
        let body = ImportRequest { backup: base64::engine::general_purpose::STANDARD.encode(backup), password };
        self.json(self.request(Method::POST, "/v1/accounts/import").json(&body).timeout(SLOW)).await
    }

    /// `target` as typed in the Launch box.
    pub async fn launch(&self, account_ids: Vec<i64>, target: String) -> Result<LaunchAnswer> {
        self.post("/v1/launch", &LaunchRequest { account_ids, target }).await
    }

    pub async fn close(&self, id: String, force: bool) -> Result<()> {
        self.act("/v1/close", &CloseRequest { id, force }, SLOW).await
    }

    pub async fn kill_all(&self) -> Result<()> {
        self.act("/v1/kill-all", &serde_json::json!({}), SLOW).await
    }

    /// The bytes `instance_thumbnail` returns: a 24-byte header, then RGBA unless the picture is
    /// `known` already. `w`: at most that many pixels wide (16..=1280).
    pub async fn thumbnail(&self, id: &str, w: u32, known: u32) -> Result<Vec<u8>> {
        let id: String = url::form_urlencoded::byte_serialize(id.as_bytes()).collect();
        let r = self.send(self.request(Method::GET, &format!("/v1/thumbnail/{id}?w={w}&known={known}"))).await?;
        let bytes = r.bytes().await.map_err(|e| AppError::Network(unreachable(&e)))?;
        Ok(bytes.to_vec())
    }

    pub async fn afk(&self, account_id: i64, config: AfkClientConfig) -> Result<()> {
        self.act("/v1/afk", &AfkRequest { account_id, config }, TIMEOUT).await
    }

    pub async fn macros(&self) -> Result<Vec<Macro>> {
        self.get("/v1/macros").await
    }

    /// Saved in the session as trusted (it comes from the main app's own macros).
    pub async fn macro_save(&self, input: &MacroInput) -> Result<Macro> {
        self.post("/v1/macros", input).await
    }

    pub async fn macro_assign(&self, account_id: i64, macro_id: Option<i64>, autorun: bool) -> Result<()> {
        self.act("/v1/macro/assign", &MacroAssignRequest { account_id, macro_id, autorun }, TIMEOUT).await
    }

    pub async fn macro_run(&self, macro_id: i64, instance_ids: Vec<String>) -> Result<()> {
        self.act("/v1/macro/run", &MacroRunRequest { macro_id, instance_ids }, TIMEOUT).await
    }

    pub async fn macro_stop(&self, instance_id: String) -> Result<()> {
        self.act("/v1/macro/stop", &MacroStopRequest { instance_id }, TIMEOUT).await
    }

    /// Starts Roblox's verified install in the session; done once `health` says it is installed.
    pub async fn roblox_install(&self) -> Result<()> {
        self.act("/v1/roblox/install", &serde_json::json!({}), TIMEOUT).await
    }

    /// Signs the session out.
    pub async fn logoff(&self) -> Result<()> {
        self.act("/v1/logoff", &serde_json::json!({}), TIMEOUT).await
    }
}

/// Why the session couldn't be reached (no URL: it holds nothing secret, but says nothing either).
fn unreachable(e: &reqwest::Error) -> String {
    if e.is_timeout() {
        "the session's AltDock didn't answer in time".into()
    } else if e.is_connect() {
        "the session's AltDock isn't running".into()
    } else {
        format!("the session's AltDock: {}", std::error::Error::source(e).map_or_else(|| e.to_string(), |s| s.to_string()))
    }
}

/// The session's error as the main app's own (pure).
fn error_of(e: ControlError) -> AppError {
    match e.kind.as_str() {
        "locked" => AppError::Locked,
        "wrongPassword" => AppError::WrongPassword,
        "notFound" => AppError::NotFound,
        "cancelled" => AppError::Cancelled,
        "cookieInvalid" => AppError::CookieInvalid,
        "challenge" => AppError::Challenge,
        "internal" | "storage" | "crypto" => AppError::Internal(e.message),
        _ => AppError::Invalid(e.message),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_sessions_errors() {
        let e = |kind: &str, message: &str| error_of(ControlError { kind: kind.into(), message: message.into() });
        assert!(matches!(e("locked", "Der Tresor ist gesperrt."), AppError::Locked));
        assert!(matches!(e("notFound", "x"), AppError::NotFound));
        assert!(matches!(e("wrongPassword", "x"), AppError::WrongPassword));
        assert!(matches!(e("invalid", "Pick at least one account."), AppError::Invalid(m) if m == "Pick at least one account."));
        assert!(matches!(e("network", "Network error: x"), AppError::Invalid(m) if m == "Network error: x"));
        assert!(matches!(e("internal", "boom"), AppError::Internal(m) if m == "boom"));
    }

    #[test]
    fn never_shows_the_token() {
        let token = "f".repeat(64);
        let c = ControlClient::new(47611, Zeroizing::new(token.clone())).unwrap();
        assert_eq!(c.base, "http://127.0.0.1:47611");
        assert!(!format!("{c:?}").contains(&token));
    }
}
