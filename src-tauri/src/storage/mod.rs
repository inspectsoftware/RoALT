//! SQLite storage. One connection behind a mutex; every query is short.
// ponytail: queries run on the calling (tokio) thread; move to spawn_blocking if any grows slow.

use std::path::Path;
use std::sync::Mutex;

use rusqlite::Connection;

use crate::error::Result;
use crate::model::{AccountPatch, AccountStatus, AccountSummary, Group, GroupInput, Settings};
use crate::vault::VaultRecord;

/// What `upsert_account` writes. `cookie` is the encrypted blob.
pub struct NewAccount<'a> {
    pub user_id: i64,
    pub username: &'a str,
    pub display_name: &'a str,
    pub cookie: &'a [u8],
    pub status: AccountStatus,
    pub checked_at: Option<i64>,
}

/// User-editable details, set wholesale by a backup import.
pub struct AccountMeta {
    pub alias: String,
    pub notes: String,
    pub color: Option<String>,
    pub favourite: bool,
}

pub struct Db {
    conn: Mutex<Connection>,
}

impl Db {
    /// Opens (creating if needed) and migrates. WAL, foreign keys on.
    pub fn open(path: &Path) -> Result<Db> {
        let _ = path;
        todo!("storage: open")
    }

    pub fn open_in_memory() -> Result<Db> {
        todo!("storage: open_in_memory")
    }

    pub fn vault_record(&self) -> Result<Option<VaultRecord>> {
        todo!("storage: vault_record")
    }

    pub fn save_vault_record(&self, rec: &VaultRecord) -> Result<()> {
        let _ = rec;
        todo!("storage: save_vault_record")
    }

    /// Ordered by `sort_order`, then `id`.
    pub fn accounts(&self) -> Result<Vec<AccountSummary>> {
        todo!("storage: accounts")
    }

    /// `NotFound` if missing.
    pub fn account(&self, id: i64) -> Result<AccountSummary> {
        let _ = id;
        todo!("storage: account")
    }

    pub fn account_id_for_user(&self, user_id: i64) -> Result<Option<i64>> {
        let _ = user_id;
        todo!("storage: account_id_for_user")
    }

    /// Inserts, or updates username / display name / cookie / status of the row with this
    /// `user_id` (keeping alias, notes, colour, favourite, groups, order). New rows go last.
    /// Returns the account id.
    pub fn upsert_account(&self, a: &NewAccount) -> Result<i64> {
        let _ = a;
        todo!("storage: upsert_account")
    }

    pub fn set_meta(&self, id: i64, meta: &AccountMeta) -> Result<()> {
        let _ = (id, meta);
        todo!("storage: set_meta")
    }

    /// Applies a UI patch. `color` must be `#rrggbb` or `""` (clear); alias ≤ 64 chars,
    /// notes ≤ 2000 chars; `group_ids` must exist and replace the current set.
    /// Bad input → `AppError::Invalid`, unknown id → `NotFound`.
    pub fn update_account(&self, id: i64, patch: &AccountPatch) -> Result<()> {
        let _ = (id, patch);
        todo!("storage: update_account")
    }

    /// `(user_id, encrypted cookie)`.
    pub fn account_cookie(&self, id: i64) -> Result<(i64, Vec<u8>)> {
        let _ = id;
        todo!("storage: account_cookie")
    }

    pub fn set_cookie(&self, id: i64, blob: &[u8]) -> Result<()> {
        let _ = (id, blob);
        todo!("storage: set_cookie")
    }

    /// Also refreshes username / display name when given.
    pub fn set_status(
        &self,
        id: i64,
        status: AccountStatus,
        checked_at: i64,
        username: Option<&str>,
        display_name: Option<&str>,
    ) -> Result<()> {
        let _ = (id, status, checked_at, username, display_name);
        todo!("storage: set_status")
    }

    /// `ids` get sort_order 0..n in that order; accounts not listed keep their relative
    /// order after them. One transaction.
    pub fn reorder_accounts(&self, ids: &[i64]) -> Result<()> {
        let _ = ids;
        todo!("storage: reorder_accounts")
    }

    pub fn remove_accounts(&self, ids: &[i64]) -> Result<()> {
        let _ = ids;
        todo!("storage: remove_accounts")
    }

    pub fn groups(&self) -> Result<Vec<Group>> {
        todo!("storage: groups")
    }

    /// Name trimmed, 1..=40 chars, unique (case-insensitive) → else `Invalid`.
    /// Colour `#rrggbb` or none.
    pub fn save_group(&self, g: &GroupInput) -> Result<Group> {
        let _ = g;
        todo!("storage: save_group")
    }

    pub fn remove_group(&self, id: i64) -> Result<()> {
        let _ = id;
        todo!("storage: remove_group")
    }

    /// Existing group id for this name (case-insensitive), or a new group.
    pub fn group_by_name_or_create(&self, name: &str, color: Option<&str>) -> Result<i64> {
        let _ = (name, color);
        todo!("storage: group_by_name_or_create")
    }

    /// Replaces the account's group set.
    pub fn set_account_groups(&self, account_id: i64, group_ids: &[i64]) -> Result<()> {
        let _ = (account_id, group_ids);
        todo!("storage: set_account_groups")
    }

    /// Defaults when nothing is stored yet (key `settings`, JSON).
    pub fn settings(&self) -> Result<Settings> {
        todo!("storage: settings")
    }

    pub fn save_settings(&self, s: &Settings) -> Result<()> {
        let _ = s;
        todo!("storage: save_settings")
    }
}
