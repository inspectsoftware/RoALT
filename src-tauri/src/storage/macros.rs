//! AltDock (Stream A): the macros, which macro each account runs, and anti-AFK per account.

use rusqlite::{OptionalExtension, Row, params};

use super::{Db, found, invalid};
use crate::commands::now_ms;
use crate::error::{AppError, Result};
use crate::model::{AfkAction, Macro, MacroAssignment, MacroInput};

/// A macro's name, in characters.
pub const MAX_MACRO_NAME: usize = 60;
/// A macro's body, in bytes.
pub const MAX_MACRO: usize = 256 << 10;

const MACRO_SELECT: &str = "SELECT id, name, body, trusted, hotkey, created_at, updated_at FROM macros";

/// An `account_afk` row: (enabled, interval_sec, action).
pub type AfkRow = (bool, u32, AfkAction);

impl Db {
    /// Macros by name (case-insensitive).
    pub fn macros(&self) -> Result<Vec<Macro>> {
        let c = self.conn();
        let mut st = c.prepare(&format!("{MACRO_SELECT} ORDER BY name, id"))?;
        Ok(st.query_map([], macro_row)?.collect::<rusqlite::Result<_>>()?)
    }

    /// `NotFound` when there is none.
    pub fn macro_by_id(&self, id: i64) -> Result<Macro> {
        self.conn().query_row(&format!("{MACRO_SELECT} WHERE id = ?1"), [id], macro_row).optional()?.ok_or(AppError::NotFound)
    }

    /// The id of the macro called `name` (case-insensitive), if any.
    pub fn macro_named(&self, name: &str) -> Result<Option<i64>> {
        Ok(self.conn().query_row("SELECT id FROM macros WHERE name = ?1", [name.trim()], |r| r.get(0)).optional()?)
    }

    /// Insert (`id: None`) or update. The name is trimmed, 1..=60 chars and unique
    /// (case-insensitive); the body at most `MAX_MACRO` bytes. The hotkey is checked by the caller.
    /// On an update `trusted` false keeps the macro's trust as it was.
    pub fn save_macro(&self, m: &MacroInput, trusted: bool) -> Result<i64> {
        let name = m.name.trim();
        if !(1..=MAX_MACRO_NAME).contains(&name.chars().count()) {
            return Err(invalid(tr!("Macro names need 1 to {max} characters.", max = MAX_MACRO_NAME)));
        }
        check_macro_body(&m.body)?;
        let c = self.conn();
        if c.query_row("SELECT EXISTS (SELECT 1 FROM macros WHERE name = ?1 AND id IS NOT ?2)", params![name, m.id], |r| r.get::<_, bool>(0))? {
            return Err(invalid(tr!("A macro called \"{name}\" already exists.", name = name)));
        }
        let now = now_ms();
        match m.id {
            Some(id) => {
                let sql = "UPDATE macros SET name = ?2, body = ?3, trusted = trusted OR ?4, hotkey = ?5, updated_at = ?6 WHERE id = ?1";
                found(c.execute(sql, params![id, name, m.body, trusted, m.hotkey, now])?)?;
                Ok(id)
            }
            None => Ok(c.query_row(
                "INSERT INTO macros (name, body, trusted, hotkey, created_at, updated_at) VALUES (?1, ?2, ?3, ?4, ?5, ?5) RETURNING id",
                params![name, m.body, trusted, m.hotkey, now],
                |r| r.get(0),
            )?),
        }
    }

    /// An imported file: untrusted, no hotkey, named `name` (cut to fit) or, when that is taken,
    /// `name (2)`, `name (3)`…
    pub fn import_macro(&self, name: &str, body: &str) -> Result<i64> {
        let base = match name.trim() {
            "" => "Macro",
            n => n,
        };
        let fit = |suffix: &str| format!("{}{suffix}", base.chars().take(MAX_MACRO_NAME - suffix.chars().count()).collect::<String>().trim_end());
        let taken = |n: &str| self.conn().query_row("SELECT EXISTS (SELECT 1 FROM macros WHERE name = ?1)", [n], |r| r.get::<_, bool>(0));
        let mut name = fit("");
        let mut n = 2;
        while taken(&name)? {
            name = fit(&format!(" ({n})"));
            n += 1;
        }
        self.save_macro(&MacroInput { name, body: body.to_owned(), ..Default::default() }, false)
    }

    /// The user read its whole source: it may run.
    pub fn trust_macro(&self, id: i64) -> Result<()> {
        found(self.conn().execute("UPDATE macros SET trusted = 1, updated_at = ?2 WHERE id = ?1", params![id, now_ms()])?)
    }

    /// Its account assignments go with it.
    pub fn remove_macro(&self, id: i64) -> Result<()> {
        self.conn().execute("DELETE FROM macros WHERE id = ?1", [id])?;
        Ok(())
    }

    /// Every account's macro, by account id.
    pub fn macro_assignments(&self) -> Result<Vec<MacroAssignment>> {
        let c = self.conn();
        let mut st = c.prepare("SELECT account_id, macro_id, autorun FROM account_macros ORDER BY account_id")?;
        Ok(st.query_map([], |r| Ok(MacroAssignment { account_id: r.get(0)?, macro_id: r.get(1)?, autorun: r.get(2)? }))?.collect::<rusqlite::Result<_>>()?)
    }

    pub fn macro_assignment(&self, account_id: i64) -> Result<Option<MacroAssignment>> {
        let sql = "SELECT account_id, macro_id, autorun FROM account_macros WHERE account_id = ?1";
        Ok(self.conn().query_row(sql, [account_id], |r| Ok(MacroAssignment { account_id: r.get(0)?, macro_id: r.get(1)?, autorun: r.get(2)? })).optional()?)
    }

    /// Gives the account `macro_id` (None: none). `NotFound` when the account or the macro isn't there.
    pub fn assign_macro(&self, account_id: i64, macro_id: Option<i64>, autorun: bool) -> Result<()> {
        let c = self.conn();
        if !c.query_row("SELECT EXISTS (SELECT 1 FROM accounts WHERE id = ?1)", [account_id], |r| r.get::<_, bool>(0))? {
            return Err(AppError::NotFound);
        }
        match macro_id {
            None => {
                c.execute("DELETE FROM account_macros WHERE account_id = ?1", [account_id])?;
            }
            Some(m) => {
                if !c.query_row("SELECT EXISTS (SELECT 1 FROM macros WHERE id = ?1)", [m], |r| r.get::<_, bool>(0))? {
                    return Err(AppError::NotFound);
                }
                c.execute(
                    "INSERT INTO account_macros (account_id, macro_id, autorun) VALUES (?1, ?2, ?3) \
                     ON CONFLICT (account_id) DO UPDATE SET macro_id = excluded.macro_id, autorun = excluded.autorun",
                    params![account_id, m, autorun],
                )?;
            }
        }
        Ok(())
    }

    /// The account's own anti-AFK row; None: it follows the global setting.
    pub fn afk_config(&self, account_id: i64) -> Result<Option<AfkRow>> {
        let row: Option<(bool, u32, String)> = self
            .conn()
            .query_row("SELECT enabled, interval_sec, action FROM account_afk WHERE account_id = ?1", [account_id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
            .optional()?;
        Ok(row.map(|(enabled, interval, action)| {
            // A row written by a newer AltDock with an action this one doesn't know: F13.
            let action = serde_json::from_str(&action).unwrap_or(AfkAction::F13);
            (enabled, interval, action)
        }))
    }

    /// Validated by the caller. `NotFound` when the account isn't there.
    pub fn set_afk_config(&self, account_id: i64, enabled: bool, interval_sec: u32, action: &AfkAction) -> Result<()> {
        let c = self.conn();
        if !c.query_row("SELECT EXISTS (SELECT 1 FROM accounts WHERE id = ?1)", [account_id], |r| r.get::<_, bool>(0))? {
            return Err(AppError::NotFound);
        }
        let action = serde_json::to_string(action).map_err(|e| AppError::Internal(e.to_string()))?;
        c.execute(
            "INSERT INTO account_afk (account_id, enabled, interval_sec, action) VALUES (?1, ?2, ?3, ?4) \
             ON CONFLICT (account_id) DO UPDATE SET enabled = excluded.enabled, interval_sec = excluded.interval_sec, action = excluded.action",
            params![account_id, enabled, interval_sec, action],
        )?;
        Ok(())
    }
}

fn macro_row(r: &Row) -> rusqlite::Result<Macro> {
    Ok(Macro { id: r.get(0)?, name: r.get(1)?, body: r.get(2)?, trusted: r.get(3)?, hotkey: r.get(4)?, created_at: r.get(5)?, updated_at: r.get(6)? })
}

/// At most `MAX_MACRO` bytes (also what a backup's macros are checked with).
pub fn check_macro_body(body: &str) -> Result<()> {
    if body.len() > MAX_MACRO {
        return Err(invalid(tr!("A macro can be at most {max} KiB.", max = MAX_MACRO >> 10)));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::AccountStatus;
    use crate::storage::NewAccount;

    fn account(db: &Db, user_id: i64) -> i64 {
        let a = NewAccount { user_id, username: "u", display_name: "u", cookie: b"x", status: AccountStatus::Unknown, checked_at: None };
        db.upsert_account(&a).unwrap()
    }

    #[test]
    fn macros_crud_limits_and_import() {
        let db = Db::open_in_memory().unwrap();
        let input = |name: &str, body: &str| MacroInput { name: name.into(), body: body.into(), ..Default::default() };
        let a = db.save_macro(&MacroInput { hotkey: "Ctrl+F1".into(), ..input(" Farm ", "Send 'w'") }, true).unwrap();
        let m = db.macro_by_id(a).unwrap();
        assert_eq!((m.name.as_str(), m.body.as_str(), m.trusted, m.hotkey.as_str()), ("Farm", "Send 'w'", true, "Ctrl+F1"));
        assert_eq!(db.macro_named("FARM").unwrap(), Some(a));

        // Names: 1..=60 chars, unique ignoring case (its own name is fine); bodies at most 256 KiB.
        for bad in ["  ", &"x".repeat(61)] {
            assert!(matches!(db.save_macro(&input(bad, ""), true), Err(AppError::Invalid(_))), "{bad}");
        }
        assert!(matches!(db.save_macro(&input("FARM", ""), true), Err(AppError::Invalid(_))));
        assert!(matches!(db.save_macro(&input("Big", &"x".repeat(MAX_MACRO + 1)), true), Err(AppError::Invalid(_))));
        db.save_macro(&input("Big", &"x".repeat(MAX_MACRO)), true).unwrap();
        db.save_macro(&MacroInput { id: Some(a), ..input("farm", "Send 'a'") }, true).unwrap();
        assert_eq!(db.macro_by_id(a).unwrap().name, "farm");
        assert!(matches!(db.save_macro(&MacroInput { id: Some(999), ..input("x", "") }, true), Err(AppError::NotFound)));

        // Imports start untrusted, without a hotkey, and get a free name.
        let i = db.import_macro("FARM", "Send 'x'").unwrap();
        let i2 = db.import_macro("farm", "").unwrap();
        let long = db.import_macro(&"y".repeat(70), "").unwrap();
        let long2 = db.import_macro(&"y".repeat(70), "").unwrap();
        let blank = db.import_macro("  ", "").unwrap();
        let name = |id| db.macro_by_id(id).unwrap().name;
        assert_eq!((name(i), name(i2), name(blank)), ("FARM (2)".into(), "farm (3)".into(), "Macro".into()));
        assert_eq!((name(long), name(long2)), ("y".repeat(60), format!("{} (2)", "y".repeat(56))));
        let imported = db.macro_by_id(i).unwrap();
        assert!(!imported.trusted && imported.hotkey.is_empty());
        db.trust_macro(i).unwrap();
        assert!(db.macro_by_id(i).unwrap().trusted);
        assert!(matches!(db.trust_macro(999), Err(AppError::NotFound)));
        // Editing keeps its trust either way: only the review trusts an imported one.
        db.save_macro(&MacroInput { id: Some(i2), ..input("farm (3)", "") }, false).unwrap();
        assert!(!db.macro_by_id(i2).unwrap().trusted);
        db.save_macro(&MacroInput { id: Some(i), ..input("FARM (2)", "Send 'y'") }, false).unwrap();
        assert!(db.macro_by_id(i).unwrap().trusted);

        assert_eq!(db.macros().unwrap().first().map(|m| m.name.clone()), Some("Big".into()));
        db.remove_macro(a).unwrap();
        assert!(matches!(db.macro_by_id(a), Err(AppError::NotFound)));
        assert_eq!(db.macros().unwrap().len(), 6);
    }

    #[test]
    fn assignments_follow_their_account_and_macro() {
        let db = Db::open_in_memory().unwrap();
        let (a, b) = (account(&db, 1), account(&db, 2));
        let m = db.save_macro(&MacroInput { name: "Farm".into(), ..Default::default() }, true).unwrap();
        let n = db.save_macro(&MacroInput { name: "Fish".into(), ..Default::default() }, true).unwrap();
        assert!(matches!(db.assign_macro(999, Some(m), true), Err(AppError::NotFound)));
        assert!(matches!(db.assign_macro(a, Some(999), true), Err(AppError::NotFound)));
        db.assign_macro(a, Some(m), true).unwrap();
        db.assign_macro(b, Some(m), false).unwrap();
        db.assign_macro(b, Some(n), true).unwrap();
        assert_eq!(db.macro_assignment(b).unwrap(), Some(MacroAssignment { account_id: b, macro_id: n, autorun: true }));
        db.remove_macro(n).unwrap();
        assert_eq!(db.macro_assignments().unwrap(), [MacroAssignment { account_id: a, macro_id: m, autorun: true }]);
        db.assign_macro(a, None, false).unwrap();
        assert!(db.macro_assignments().unwrap().is_empty());
    }

    #[test]
    fn afk_rows_round_trip() {
        let db = Db::open_in_memory().unwrap();
        let a = account(&db, 1);
        assert_eq!(db.afk_config(a).unwrap(), None);
        db.set_afk_config(a, false, 300, &AfkAction::Key("W".into())).unwrap();
        assert_eq!(db.afk_config(a).unwrap(), Some((false, 300, AfkAction::Key("W".into()))));
        db.set_afk_config(a, true, 60, &AfkAction::JumpTurn).unwrap();
        assert_eq!(db.afk_config(a).unwrap(), Some((true, 60, AfkAction::JumpTurn)));
        assert!(matches!(db.set_afk_config(999, true, 60, &AfkAction::F13), Err(AppError::NotFound)));
    }
}
