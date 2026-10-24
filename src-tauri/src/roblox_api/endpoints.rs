//! Every Roblox URL in one place (verified 2026-10-24, see docs/DESIGN.md).

pub const USERS_AUTHENTICATED: &str = "https://users.roblox.com/v1/users/authenticated";
pub const LOGIN_PAGE: &str = "https://www.roblox.com/login";
pub const WWW: &str = "https://www.roblox.com/";
/// `Origin` the website sends to auth.roblox.com and apis.roblox.com (`Referer` is `WWW`).
pub const WEB_ORIGIN: &str = "https://www.roblox.com";

// Account info. `{}` is filled by `fill`; each template is also its endpoint's throttle key.
pub const USER: &str = "https://users.roblox.com/v1/users/{}";
pub const HEADSHOTS: &str =
    "https://thumbnails.roblox.com/v1/users/avatar-headshot?userIds={}&size=150x150&format=Png&isCircular=false";
pub const CURRENCY: &str = "https://economy.roblox.com/v1/user/currency";
pub const PREMIUM: &str = "https://premiumfeatures.roblox.com/v1/users/{}/validate-membership";
pub const PROFILES: &str = "https://apis.roblox.com/user-profile-api/v1/user/profiles/get-profiles";
pub const FRIENDS_COUNT: &str = "https://friends.roblox.com/v1/users/{}/friends/count";
pub const FOLLOWERS_COUNT: &str = "https://friends.roblox.com/v1/users/{}/followers/count";
pub const FOLLOWINGS_COUNT: &str = "https://friends.roblox.com/v1/users/{}/followings/count";
pub const PRESENCE: &str = "https://presence.roblox.com/v1/presence/users";
pub const EMAIL: &str = "https://accountsettings.roblox.com/v1/email";
pub const TWO_STEP: &str = "https://twostepverification.roblox.com/v1/users/{}/configuration";

/// `template` with its `{}` replaced by `value`.
pub fn fill(template: &str, value: impl std::fmt::Display) -> String {
    template.replacen("{}", &value.to_string(), 1)
}
