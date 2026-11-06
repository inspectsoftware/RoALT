//! What "Remove from this PC" puts back. The setup changes a few machine-wide registry values;
//! before it first changes one, it records the value's previous state in
//! `%ProgramData%\AltDock\restore.json` (administrators only), and the uninstall puts each one back
//! from there. Everything here is pure: the elevated side (`setup`) reads and writes the registry.

use serde::{Deserialize, Serialize};

/// Terminal Services policies (`fSingleSessionPerUser`, `bEnumerateHWBeforeSW`, ...).
pub const TS_POLICY: &str = r"SOFTWARE\Policies\Microsoft\Windows NT\Terminal Services";
/// The Terminal Server settings policies override (`fDenyTSConnections`, ...).
pub const TS_CONTROL: &str = r"SYSTEM\CurrentControlSet\Control\Terminal Server";
pub const OOBE_POLICY: &str = r"SOFTWARE\Policies\Microsoft\Windows\OOBE";
pub const SYSTEM_POLICY: &str = r"SOFTWARE\Microsoft\Windows\CurrentVersion\Policies\System";
/// `<user> = 0` hides a user from the sign-in screen.
pub const USER_LIST: &str = r"SOFTWARE\Microsoft\Windows NT\CurrentVersion\Winlogon\SpecialAccounts\UserList";
/// The `.rdp` signing certificates Remote Desktop trusts (a comma separated list).
pub const TRUSTED_THUMBPRINTS: &str = "TrustedCertThumbprints";

/// The file's name in `%ProgramData%\AltDock`.
pub const FILE: &str = "restore.json";

/// One HKLM `REG_DWORD` the setup sets.
pub struct Policy {
    /// The setup step it belongs to (`connections`, ...).
    pub step: &'static str,
    pub key: &'static str,
    pub value: &'static str,
    pub data: u32,
    /// Set only when the value is already there (a policy saying no would win over the setting).
    pub only_if_present: bool,
    /// What the uninstall sets when it doesn't know the previous state; None: delete the value.
    pub fallback: Option<u32>,
}

/// Every policy and setting the setup sets, in order.
pub const POLICIES: &[Policy] = &[
    // Windows itself always has this one; 1 (no remote connections) is its default.
    Policy { step: "connections", key: TS_CONTROL, value: "fDenyTSConnections", data: 0, only_if_present: false, fallback: Some(1) },
    Policy { step: "connections", key: TS_POLICY, value: "fDenyTSConnections", data: 0, only_if_present: true, fallback: None },
    Policy { step: "singleSession", key: TS_POLICY, value: "fSingleSessionPerUser", data: 0, only_if_present: false, fallback: None },
    Policy { step: "hardwareGraphics", key: TS_POLICY, value: "bEnumerateHWBeforeSW", data: 1, only_if_present: false, fallback: None },
    Policy { step: "firstLogon", key: OOBE_POLICY, value: "DisablePrivacyExperience", data: 1, only_if_present: false, fallback: None },
    Policy { step: "firstLogon", key: SYSTEM_POLICY, value: "EnableFirstLogonAnimation", data: 0, only_if_present: false, fallback: None },
];

/// A registry value's data, of the two types the setup writes.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(tag = "type", content = "data", rename_all = "camelCase")]
pub enum RegData {
    Dword(u32),
    String(String),
}

/// A value's state before the setup first changed it.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum Prior {
    /// Not there.
    Absent,
    /// There, with this data.
    Was { data: RegData },
    /// Not known: another type, or a setup from before `restore.json` had set it already.
    Unknown,
}

impl Prior {
    /// The data now (a value read just now): None when absent or of another type.
    pub fn data(&self) -> Option<&RegData> {
        match self {
            Prior::Was { data } => Some(data),
            _ => None,
        }
    }
}

/// One recorded value (always under HKLM).
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Saved {
    pub key: String,
    pub value: String,
    pub prior: Prior,
}

/// `restore.json`.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase")]
pub struct RestoreState {
    pub values: Vec<Saved>,
    /// SHA-1 thumbprints the setup added to `TrustedCertThumbprints`.
    pub thumbprints: Vec<String>,
}

fn same(a: &str, b: &str) -> bool {
    a.eq_ignore_ascii_case(b)
}

impl RestoreState {
    /// The recorded state of a value (registry names ignore case).
    pub fn prior(&self, key: &str, value: &str) -> Option<&Prior> {
        self.values.iter().find(|s| same(&s.key, key) && same(&s.value, value)).map(|s| &s.prior)
    }

    /// Records `now`, the value's state just before the setup sets it to `target`, unless an
    /// earlier run recorded it already: the first record is the one from before AltDock. When a
    /// setup without this file ran before (`set_up_before`) and the value already says what
    /// AltDock sets, nobody knows what it said before that. True when something new was recorded.
    pub fn record(&mut self, key: &str, value: &str, now: Prior, target: &RegData, set_up_before: bool) -> bool {
        if self.prior(key, value).is_some() {
            return false;
        }
        let prior = if set_up_before && now.data() == Some(target) { Prior::Unknown } else { now };
        self.values.push(Saved { key: key.into(), value: value.into(), prior });
        true
    }

    /// Records a thumbprint added to `TrustedCertThumbprints`; true when it is new here.
    pub fn add_thumbprint(&mut self, thumbprint: &str) -> bool {
        let t = normalize_thumbprint(thumbprint);
        if t.is_empty() || self.thumbprints.iter().any(|o| normalize_thumbprint(o) == t) {
            return false;
        }
        self.thumbprints.push(t);
        true
    }

    /// The records of `other` (another `restore.json`) this one doesn't have yet; this one's win.
    pub fn merge(&mut self, other: RestoreState) {
        for s in other.values {
            if self.prior(&s.key, &s.value).is_none() {
                self.values.push(s);
            }
        }
        for t in other.thumbprints {
            self.add_thumbprint(&t);
        }
    }

    /// The alt users whose sign-in screen entry was recorded (`altdock-<n>` only).
    pub fn hidden_users(&self) -> Vec<String> {
        self.values.iter().filter(|s| same(&s.key, USER_LIST) && super::slot_of(&s.value).is_some()).map(|s| s.value.clone()).collect()
    }
}

/// A value the setup sets and the uninstall may therefore put back: nothing else in a
/// `restore.json` is ever written.
pub fn restorable(key: &str, value: &str) -> bool {
    POLICIES.iter().any(|p| same(p.key, key) && same(p.value, value))
        || (same(key, TS_POLICY) && same(value, TRUSTED_THUMBPRINTS))
        || (same(key, USER_LIST) && super::slot_of(value).is_some())
}

/// What the uninstall does with one value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Undo {
    Keep,
    Set(RegData),
    Delete,
}

/// How to put back a value the setup set to `ours` (pure). `prior`: what `restore.json` recorded;
/// `have_file`: there is a `restore.json` (without one, the value goes only while it still says
/// what AltDock set); `current`: the value now; `fallback`: what to set instead of deleting it
/// when the previous state is unknown. A value changed since the setup is left alone.
pub fn undo(prior: Option<&Prior>, have_file: bool, current: Option<&RegData>, ours: &RegData, fallback: Option<&RegData>) -> Undo {
    if current != Some(ours) {
        return Undo::Keep;
    }
    let unknown = || fallback.map_or(Undo::Delete, |f| Undo::Set(f.clone()));
    match prior {
        Some(Prior::Absent) => Undo::Delete,
        Some(Prior::Was { data }) if data == ours => Undo::Keep,
        Some(Prior::Was { data }) => Undo::Set(data.clone()),
        Some(Prior::Unknown) => unknown(),
        // The setup never changed it (it recorded every value it set).
        None if have_file => Undo::Keep,
        None => unknown(),
    }
}

/// A thumbprint as compared and stored: uppercase hex, nothing else.
pub fn normalize_thumbprint(t: &str) -> String {
    t.chars().filter(char::is_ascii_hexdigit).collect::<String>().to_ascii_uppercase()
}

/// The list without `ours` (pure); the others stay, in their order.
pub fn without_thumbprints(list: &str, ours: &[String]) -> String {
    let ours: Vec<String> = ours.iter().map(|t| normalize_thumbprint(t)).filter(|t| !t.is_empty()).collect();
    list.split(',').map(str::trim).filter(|t| !t.is_empty() && !ours.contains(&normalize_thumbprint(t))).collect::<Vec<_>>().join(",")
}

/// How to take AltDock's certificates out of `TrustedCertThumbprints` (pure): the other
/// thumbprints stay; an emptied list goes when there was none before AltDock's.
pub fn undo_thumbprints(current: Option<&str>, ours: &[String], prior: Option<&Prior>) -> Undo {
    let Some(list) = current else { return Undo::Keep };
    let rest = without_thumbprints(list, ours);
    let before = list.split(',').map(str::trim).filter(|t| !t.is_empty()).collect::<Vec<_>>().join(",");
    if rest == before {
        return Undo::Keep;
    }
    if rest.is_empty() && !matches!(prior, Some(Prior::Was { .. })) { Undo::Delete } else { Undo::Set(RegData::String(rest)) }
}

#[cfg(test)]
mod tests {
    use super::*;

    const D0: RegData = RegData::Dword(0);

    #[test]
    fn the_first_record_wins() {
        let mut s = RestoreState::default();
        assert!(s.record(TS_CONTROL, "fDenyTSConnections", Prior::Was { data: RegData::Dword(1) }, &D0, false));
        // A second setup sees AltDock's own value: not what was there before.
        assert!(!s.record(TS_CONTROL, "fdenytsconnections", Prior::Was { data: D0.clone() }, &D0, false));
        assert_eq!(s.prior(TS_CONTROL, "fDenyTSConnections"), Some(&Prior::Was { data: RegData::Dword(1) }));
        assert!(s.record(TS_POLICY, "fSingleSessionPerUser", Prior::Absent, &D0, false));
        assert_eq!(s.prior(&TS_POLICY.to_lowercase(), "FSINGLESESSIONPERUSER"), Some(&Prior::Absent));
        assert_eq!(s.prior(TS_POLICY, "bEnumerateHWBeforeSW"), None);
        // Set up before restore.json existed: AltDock's value there says nothing about before.
        assert!(s.record(TS_POLICY, "bEnumerateHWBeforeSW", Prior::Was { data: RegData::Dword(1) }, &RegData::Dword(1), true));
        assert_eq!(s.prior(TS_POLICY, "bEnumerateHWBeforeSW"), Some(&Prior::Unknown));
        // ... while another value is still known.
        assert!(s.record(OOBE_POLICY, "DisablePrivacyExperience", Prior::Was { data: RegData::Dword(0) }, &RegData::Dword(1), true));
        assert_eq!(s.prior(OOBE_POLICY, "DisablePrivacyExperience"), Some(&Prior::Was { data: RegData::Dword(0) }));

        assert!(s.add_thumbprint("ab 12"));
        assert!(!s.add_thumbprint("AB12"));
        assert!(!s.add_thumbprint(""));
        assert_eq!(s.thumbprints, ["AB12"]);
    }

    #[test]
    fn merging_keeps_what_was_recorded_first() {
        let mut a = RestoreState::default();
        a.record(TS_CONTROL, "fDenyTSConnections", Prior::Was { data: RegData::Dword(1) }, &D0, false);
        a.add_thumbprint("AA");
        let mut b = RestoreState::default();
        b.record(TS_CONTROL, "fDenyTSConnections", Prior::Absent, &D0, false);
        b.record(USER_LIST, "altdock-2", Prior::Absent, &D0, false);
        b.add_thumbprint("aa");
        b.add_thumbprint("BB");
        a.merge(b);
        assert_eq!(a.prior(TS_CONTROL, "fDenyTSConnections"), Some(&Prior::Was { data: RegData::Dword(1) }));
        assert_eq!(a.prior(USER_LIST, "altdock-2"), Some(&Prior::Absent));
        assert_eq!(a.thumbprints, ["AA", "BB"]);
        assert_eq!(a.hidden_users(), ["altdock-2"]);
    }

    #[test]
    fn the_file_round_trips() {
        let mut s = RestoreState::default();
        s.record(TS_CONTROL, "fDenyTSConnections", Prior::Was { data: RegData::Dword(1) }, &D0, false);
        s.record(TS_POLICY, TRUSTED_THUMBPRINTS, Prior::Was { data: RegData::String("1111".into()) }, &RegData::String("1111,AB".into()), false);
        s.record(USER_LIST, "altdock-1", Prior::Absent, &D0, false);
        s.record(TS_POLICY, "bEnumerateHWBeforeSW", Prior::Unknown, &RegData::Dword(1), false);
        s.add_thumbprint("AB");
        let json = serde_json::to_string(&s).unwrap();
        assert!(json.contains(r#""prior":{"kind":"was","data":{"type":"dword","data":1}}"#), "{json}");
        assert!(json.contains(r#""prior":{"kind":"absent"}"#), "{json}");
        assert_eq!(serde_json::from_str::<RestoreState>(&json).unwrap(), s);
        assert_eq!(serde_json::from_str::<RestoreState>("{}").ok(), None, "both lists are required");
    }

    #[test]
    fn only_what_the_setup_sets_is_restorable() {
        for p in POLICIES {
            assert!(restorable(p.key, p.value), "{}", p.value);
        }
        assert!(restorable(TS_POLICY, TRUSTED_THUMBPRINTS));
        assert!(restorable(USER_LIST, "altdock-3"));
        assert!(!restorable(USER_LIST, "Administrator"));
        assert!(!restorable(USER_LIST, "altdock-0"));
        assert!(!restorable(r"SOFTWARE\Microsoft\Windows\CurrentVersion\Run", "AltDockSession"));
        assert!(!restorable(TS_CONTROL, "fSingleSessionPerUser"));
        assert!(!restorable(r"SYSTEM\CurrentControlSet\Services\TermService\Parameters", "ServiceDll"));
    }

    #[test]
    fn values_go_back_to_what_was_there() {
        let d = RegData::Dword;
        // Recorded.
        assert_eq!(undo(Some(&Prior::Was { data: d(1) }), true, Some(&D0), &D0, Some(&d(1))), Undo::Set(d(1)));
        assert_eq!(undo(Some(&Prior::Absent), true, Some(&D0), &D0, Some(&d(1))), Undo::Delete);
        assert_eq!(undo(Some(&Prior::Was { data: D0 }), true, Some(&D0), &D0, None), Undo::Keep, "it said so before AltDock");
        // Changed since the setup: the user's own choice now.
        assert_eq!(undo(Some(&Prior::Absent), true, Some(&d(5)), &D0, None), Undo::Keep);
        assert_eq!(undo(Some(&Prior::Absent), true, None, &D0, None), Undo::Keep);
        // In the file, but not set by the setup.
        assert_eq!(undo(None, true, Some(&D0), &D0, None), Undo::Keep);
        // Unknown, or no file: only a value that still says what AltDock set goes.
        assert_eq!(undo(Some(&Prior::Unknown), true, Some(&D0), &D0, None), Undo::Delete);
        assert_eq!(undo(None, false, Some(&D0), &D0, None), Undo::Delete);
        assert_eq!(undo(None, false, Some(&D0), &D0, Some(&d(1))), Undo::Set(d(1)), "Windows' own default instead");
        assert_eq!(undo(None, false, Some(&d(1)), &D0, None), Undo::Keep);
        assert_eq!(undo(None, false, None, &D0, None), Undo::Keep);
    }

    #[test]
    fn our_thumbprints_leave_the_trusted_list() {
        let ours = vec!["ab12".to_owned()];
        assert_eq!(without_thumbprints("1111, AB12 ,2222,", &ours), "1111,2222");
        assert_eq!(without_thumbprints("AB12", &ours), "");
        assert_eq!(without_thumbprints("1111", &[]), "1111");
        let was = Prior::Was { data: RegData::String("1111".into()) };
        assert_eq!(undo_thumbprints(Some("1111,AB12"), &ours, Some(&was)), Undo::Set(RegData::String("1111".into())));
        // Added by someone else since: kept.
        assert_eq!(undo_thumbprints(Some("1111,AB12,3333"), &ours, Some(&was)), Undo::Set(RegData::String("1111,3333".into())));
        assert_eq!(undo_thumbprints(Some("AB12"), &ours, Some(&Prior::Absent)), Undo::Delete);
        assert_eq!(undo_thumbprints(Some("AB12"), &ours, None), Undo::Delete);
        assert_eq!(undo_thumbprints(Some("AB12"), &ours, Some(&Prior::Was { data: RegData::String(String::new()) })), Undo::Set(RegData::String(String::new())));
        assert_eq!(undo_thumbprints(Some("1111"), &ours, Some(&Prior::Absent)), Undo::Keep, "ours isn't there");
        assert_eq!(undo_thumbprints(None, &ours, None), Undo::Keep);
    }
}
