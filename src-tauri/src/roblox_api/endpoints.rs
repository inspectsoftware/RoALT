//! Every Roblox URL in one place (verified 2026-10-24, see docs/DESIGN.md).

pub const USERS_AUTHENTICATED: &str = "https://users.roblox.com/v1/users/authenticated";
pub const LOGIN_PAGE: &str = "https://www.roblox.com/login";
pub const WWW: &str = "https://www.roblox.com/";
/// `Origin` the website sends to auth.roblox.com and apis.roblox.com (`Referer` is `WWW`).
pub const WEB_ORIGIN: &str = "https://www.roblox.com";
