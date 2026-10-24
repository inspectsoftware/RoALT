use serde::{Serialize, Serializer};
use ts_rs::TS;

/// Every error the UI can see. Messages are user-facing and must never contain a cookie,
/// ticket, link code or anything else from the vault.
#[derive(Debug, thiserror::Error)]
pub enum AppError {
    #[error("The vault is locked.")]
    Locked,
    #[error("Wrong password.")]
    WrongPassword,
    #[error("This cookie is not valid or has expired. Sign in again.")]
    CookieInvalid,
    #[error("Roblox wants a human check for this account. Open its session window and complete it.")]
    Challenge,
    #[error("Roblox is limiting requests. Try again in {0} s.")]
    RateLimited(u64),
    #[error("Roblox now requires a bound session for this account.")]
    BoundSessionRequired,
    #[error("Roblox answered unexpectedly ({0}).")]
    Roblox(String),
    #[error("Network error: {0}")]
    Network(String),
    #[error("Not found.")]
    NotFound,
    #[error("Cancelled.")]
    Cancelled,
    #[error("{0}")]
    Invalid(String),
    #[error("Storage error: {0}")]
    Storage(String),
    #[error("Encryption failed.")]
    Crypto,
    #[error("{0}")]
    Internal(String),
}

impl AppError {
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Locked => "locked",
            Self::WrongPassword => "wrongPassword",
            Self::CookieInvalid => "cookieInvalid",
            Self::Challenge => "challenge",
            Self::RateLimited(_) => "rateLimited",
            Self::BoundSessionRequired => "boundSessionRequired",
            Self::Roblox(_) => "roblox",
            Self::Network(_) => "network",
            Self::NotFound => "notFound",
            Self::Cancelled => "cancelled",
            Self::Invalid(_) => "invalid",
            Self::Storage(_) => "storage",
            Self::Crypto => "crypto",
            Self::Internal(_) => "internal",
        }
    }
}

/// What a rejected `invoke` carries to the UI.
#[derive(Serialize, TS)]
#[ts(export)]
pub struct ErrorPayload {
    pub kind: String,
    pub message: String,
}

impl Serialize for AppError {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        ErrorPayload { kind: self.kind().into(), message: self.to_string() }.serialize(s)
    }
}

impl From<rusqlite::Error> for AppError {
    fn from(e: rusqlite::Error) -> Self {
        match e {
            rusqlite::Error::QueryReturnedNoRows => Self::NotFound,
            e => Self::Storage(e.to_string()),
        }
    }
}

impl From<std::io::Error> for AppError {
    fn from(e: std::io::Error) -> Self {
        Self::Internal(e.to_string())
    }
}

impl From<tauri::Error> for AppError {
    fn from(e: tauri::Error) -> Self {
        Self::Internal(e.to_string())
    }
}

pub type Result<T, E = AppError> = std::result::Result<T, E>;
