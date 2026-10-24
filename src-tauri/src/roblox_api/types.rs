//! Response types. Lenient: unknown fields are ignored.

use serde::Deserialize;

#[derive(Deserialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AuthenticatedUser {
    pub id: i64,
    pub name: String,
    pub display_name: String,
}
