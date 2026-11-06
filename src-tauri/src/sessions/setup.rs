//! The one-prompt setup. The main app writes a `SetupPlan` (with the alt users' passwords and
//! control tokens) to a randomly named file in its own data folder and starts the same exe
//! elevated with `--altdock-setup <plan.json>` (`ShellExecuteExW` "runas", like `instances::qos`).
//! That process opens the plan once (never through a link), wipes it as soon as it has read it,
//! does each step (going on past failures where that is safe), writes its result into the same
//! file and exits before Tauri starts. Passwords never appear on a command line.
//!
//! The plan file, its folder and its path are as writable as anything else of the user's, so the
//! elevated side trusts nothing in them it can check: user names must be `altdock-<n>`, ports
//! theirs, tokens hex, the main user a user, `.rdp` files AltDock's own session files next to the
//! plan, and the program it copies is always itself. Everything it writes in `%ProgramData%\AltDock`
//! only administrators can change: the HKLM autostart runs that program for every user.
//!
//! The same entry undoes it all ("Remove from this PC", `SetupAction::Uninstall`): the setup
//! records each registry value's previous state in `restore.json` before it first changes it
//! (`restore`), and the uninstall puts them back from there.

use std::path::{Path, PathBuf};

use super::types::{SessionMode, SetupAction, SetupPlan};
use super::{PORT_BASE, slot_of};

pub const FLAG: &str = "--altdock-setup";

/// The plan's path when this process was started for the elevated setup (pure).
pub fn plan_arg(args: &[std::ffi::OsString]) -> Option<PathBuf> {
    match args {
        [_, flag, plan, ..] if flag == FLAG => Some(PathBuf::from(plan)),
        _ => None,
    }
}

/// The plan is a file the main app named: `<folder>\setup-<32 hex>.json` (pure). The elevated
/// side opens nothing else the command line names.
pub fn plan_name_ok(plan: &Path) -> bool {
    plan.is_absolute()
        && plan.file_name().and_then(|n| n.to_str()).is_some_and(|n| {
            n.strip_prefix("setup-").and_then(|n| n.strip_suffix(".json")).is_some_and(|h| h.len() == 32 && h.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)))
        })
}

/// A `.rdp` file the setup may sign: `<plan's folder>\sessions\<session name>.rdp` (pure).
pub fn rdp_file_ok(file: &Path, plan: &Path) -> bool {
    let name_ok = |n: &str| !n.is_empty() && n.len() <= 32 && n.bytes().all(|b| b.is_ascii_alphanumeric() || b"-_ ".contains(&b));
    plan.parent().is_some_and(|d| file.parent() == Some(d.join("sessions").as_path()))
        && file.file_name().and_then(|n| n.to_str()).and_then(|n| n.strip_suffix(".rdp")).is_some_and(name_ok)
}

/// A user account's SID, `S-1-5-21-...` (digits and dashes only, so it can go into an SDDL
/// string): never a group such as Everyone or Users.
fn is_user_sid(s: &str) -> bool {
    s.len() < 200 && s.strip_prefix("S-1-5-21-").is_some_and(|r| !r.is_empty() && r.bytes().all(|b| b.is_ascii_digit() || b == b'-'))
}

/// Refuses a plan the main app could not have written (pure).
pub fn check_plan(p: &SetupPlan) -> Result<(), String> {
    if !is_user_sid(&p.main_user_sid) {
        return Err(format!("bad main user SID {:?}", p.main_user_sid));
    }
    match p.action {
        SetupAction::Setup if p.delete_users => return Err("deleteUsers is for an uninstall".into()),
        // An uninstall only reads which alt users the main app knows; it writes nothing of the plan's.
        SetupAction::Uninstall if !p.users.is_empty() || !p.remove_users.is_empty() || !p.rdp_files.is_empty() || p.thumbprint.is_some() || p.system => {
            return Err("an uninstall plan names alt users only".into());
        }
        _ => {}
    }
    for name in p.managed_users.iter().chain(&p.remove_users).chain(p.users.iter().map(|u| &u.name)) {
        if slot_of(name).is_none() {
            return Err(format!("not an AltDock user: {name:?}"));
        }
    }
    for u in &p.users {
        if slot_of(&u.name).map(|n| PORT_BASE + n as u16) != Some(u.port) {
            return Err(format!("wrong control port for {}", u.name));
        }
        if u.token.len() != 64 || !u.token.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(format!("bad control token for {}", u.name));
        }
        if u.password.chars().count() < 12 || u.password.chars().any(|c| c.is_control()) {
            return Err(format!("bad password for {}", u.name));
        }
    }
    for f in &p.rdp_files {
        if !f.is_absolute() || !f.extension().is_some_and(|e| e.eq_ignore_ascii_case("rdp")) {
            return Err(format!("not a .rdp file: {}", f.display()));
        }
    }
    if p.thumbprint.as_deref().is_some_and(|t| t.len() != 40 || !t.bytes().all(|b| b.is_ascii_hexdigit())) {
        return Err("bad thumbprint".into());
    }
    Ok(())
}

/// `users.json` (pure).
pub fn users_json(users: &[String]) -> String {
    serde_json::json!({ "users": users }).to_string()
}

/// A control config (`control\<user>.json`) (pure).
pub fn control_json(port: u16, token: &str, mode: SessionMode) -> zeroize::Zeroizing<String> {
    zeroize::Zeroizing::new(serde_json::json!({ "port": port, "token": token, "mode": mode }).to_string())
}

/// Read and execute.
const READ_EXECUTE: &str = "0x1200a9";

/// `%ProgramData%\AltDock` and its `app` folder (pure): owned by administrators, who (with
/// SYSTEM) are the only ones that may change anything in it; the main user and the alt users read
/// and execute. Nothing inherited from ProgramData (where every user may create folders). The
/// HKLM autostart runs `app\altdock.exe` for whoever signs in, administrators too: anyone else
/// able to write there could run code as them.
pub fn shared_sddl(main: &str, alts: &[String]) -> String {
    let mut s = format!("O:BAD:PAI(A;OICI;FA;;;BA)(A;OICI;FA;;;SY)(A;OICI;{READ_EXECUTE};;;{main})");
    for a in alts {
        s.push_str(&format!("(A;OICI;{READ_EXECUTE};;;{a})"));
    }
    s
}

/// The `control` folder (and the setup's own scratch folder) (pure): administrators only; no alt
/// user may list it or read another's token, and the main app keeps its tokens in its vault.
pub fn private_dir_sddl() -> String {
    "O:BAD:PAI(A;OICI;FA;;;BA)(A;OICI;FA;;;SY)".into()
}

/// One control config (pure): its alt user may read it, administrators change it.
pub fn control_file_sddl(alt: &str) -> String {
    format!("O:BAD:PAI(A;;FA;;;BA)(A;;FA;;;SY)(A;;FR;;;{alt})")
}

/// `restore.json` (pure): administrators only, though it sits in the folder everyone may read.
pub fn admin_file_sddl() -> String {
    "O:BAD:PAI(A;;FA;;;BA)(A;;FA;;;SY)".into()
}

/// The alt users a `users.json` names (pure); anything not `altdock-<n>` is ignored.
pub fn users_in_json(bytes: &[u8]) -> Vec<String> {
    let v: serde_json::Value = serde_json::from_slice(bytes).unwrap_or_default();
    v.get("users")
        .and_then(|u| u.as_array())
        .map(|a| a.iter().filter_map(|u| u.as_str()).filter(|u| slot_of(u).is_some()).map(str::to_owned).collect())
        .unwrap_or_default()
}

/// The alt users an uninstall looks at (pure): the main app's, `users.json`'s and those whose
/// sign-in screen entry `restore.json` recorded, each once, `altdock-<n>` only.
pub fn uninstall_users(plan: &[String], listed: &[String], hidden: &[String]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for u in plan.iter().chain(listed).chain(hidden) {
        if slot_of(u).is_some() && !out.iter().any(|o| o.eq_ignore_ascii_case(u)) {
            out.push(u.clone());
        }
    }
    out
}

#[cfg(windows)]
pub use system::*;

#[cfg(windows)]
mod system {
    use std::fs::File;
    use std::io::{Read, Seek, SeekFrom, Write};
    use std::os::windows::fs::{MetadataExt, OpenOptionsExt};
    use std::os::windows::io::AsRawHandle;
    use std::os::windows::process::CommandExt;
    use std::path::{Path, PathBuf};
    use std::time::Duration;

    use windows::Win32::Foundation::{ERROR_CANCELLED, HANDLE, HLOCAL, LocalFree, WAIT_OBJECT_0};
    use windows::Win32::Security::Authorization::{ConvertStringSecurityDescriptorToSecurityDescriptorW, GetSecurityInfo, SDDL_REVISION_1, SE_FILE_OBJECT};
    use windows::Win32::Security::Cryptography::*;
    use windows::Win32::Security::{
        DACL_SECURITY_INFORMATION, OWNER_SECURITY_INFORMATION, PROTECTED_DACL_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR, PSID, SetKernelObjectSecurity,
    };
    use windows::Win32::Storage::FileSystem::{BY_HANDLE_FILE_INFORMATION, GetFileInformationByHandle};
    use windows::Win32::System::SystemInformation::GetSystemTime;
    use windows::Win32::System::Threading::{GetExitCodeProcess, WaitForSingleObject};
    use windows::Win32::UI::Shell::{SEE_MASK_NOASYNC, SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW, ShellExecuteExW};
    use windows::Win32::UI::WindowsAndMessaging::SW_HIDE;
    use windows::core::{HSTRING, Owned, PCWSTR, PSTR, PWSTR, w};
    use zeroize::Zeroizing;

    use super::*;
    use crate::error::{AppError, Result};
    use crate::sessions::health::{normalize_thumbprint, with_thumbprint};
    use crate::sessions::reg::{self, HKLM};
    use crate::sessions::restore::{self, POLICIES, Prior, RegData, RestoreState, TRUSTED_THUMBPRINTS, Undo, undo, undo_thumbprints};
    use crate::sessions::types::SetupResult;
    use crate::sessions::{APP_EXE, RUN_VALUE, app_exe, firewall, program_data, random, rdp, run_command, users};

    /// How long the elevated setup may take once the prompt is answered.
    const TIMEOUT: Duration = Duration::from_secs(300);
    const CERT_SUBJECT: &str = "CN=AltDock RDP signing";

    fn hex(bytes: &[u8]) -> String {
        bytes.iter().map(|b| format!("{b:02X}")).collect()
    }

    // ---------- main app side ----------

    /// Runs `plan` elevated: Windows asks for admin rights first. Declining the prompt is an error;
    /// failed steps are in the result, which the elevated side writes into the plan's file.
    pub fn run(data_dir: &Path, plan: &SetupPlan) -> Result<SetupResult> {
        let path = data_dir.join(format!("setup-{}.json", hex(&random(16)).to_ascii_lowercase()));
        let json = Zeroizing::new(serde_json::to_vec(plan).map_err(|e| AppError::Internal(e.to_string()))?);
        {
            use std::io::Write;
            let mut f = std::fs::File::options().write(true).create_new(true).open(&path)?;
            f.write_all(&json)?;
        }
        let outcome = elevate(&path);
        // The result, or (the elevated side never ran) the plan, which holds passwords: gone either way.
        let read = std::fs::read(&path);
        let _ = std::fs::remove_file(&path);
        outcome?;
        read.ok()
            .and_then(|b| serde_json::from_slice::<SetupResult>(&b).ok())
            .ok_or_else(|| AppError::Internal(tr!("The setup ended without a result.")))
    }

    fn elevate(plan: &Path) -> Result<()> {
        let exe = std::env::current_exe()?;
        let wide = |s: &std::ffi::OsStr| s.encode_wide().chain([0]).collect::<Vec<u16>>();
        use std::os::windows::ffi::OsStrExt;
        let file = wide(exe.as_os_str());
        let params = wide(format!("{FLAG} \"{}\"", plan.display()).as_ref());
        let mut info = SHELLEXECUTEINFOW {
            cbSize: size_of::<SHELLEXECUTEINFOW>() as u32,
            fMask: SEE_MASK_NOCLOSEPROCESS | SEE_MASK_NOASYNC,
            lpVerb: w!("runas"),
            lpFile: PCWSTR(file.as_ptr()),
            lpParameters: PCWSTR(params.as_ptr()),
            nShow: SW_HIDE.0,
            ..Default::default()
        };
        // SAFETY: `info` is a local with its size set; the strings it points at outlive the call.
        if let Err(e) = unsafe { ShellExecuteExW(&mut info) } {
            return Err(if e.code() == ERROR_CANCELLED.to_hresult() {
                AppError::Invalid(tr!("The session setup needs admin rights; the prompt was declined."))
            } else {
                AppError::Internal(e.message())
            });
        }
        if info.hProcess.is_invalid() {
            return Err(AppError::Internal(tr!("The setup didn't start.")));
        }
        // SAFETY: SEE_MASK_NOCLOSEPROCESS hands us the process handle; Owned closes it.
        let h = unsafe { Owned::new(info.hProcess) };
        let mut code = 1u32;
        // SAFETY: `h` is a live process handle; `code` is a local.
        unsafe {
            if WaitForSingleObject(*h, TIMEOUT.as_millis() as u32) != WAIT_OBJECT_0 {
                return Err(AppError::Internal(tr!("The setup didn't finish in time. Run it again.")));
            }
            GetExitCodeProcess(*h, &mut code).map_err(|e| AppError::Internal(e.message()))?;
        }
        log::info!("session setup exited with {code}");
        Ok(())
    }

    // ---------- elevated side ----------

    const FILE_ATTRIBUTE_DIRECTORY: u32 = 0x10;
    const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
    const FILE_FLAG_BACKUP_SEMANTICS: u32 = 0x0200_0000;
    const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
    const READ_CONTROL: u32 = 0x0002_0000;
    const WRITE_DAC: u32 = 0x0004_0000;
    const WRITE_OWNER: u32 = 0x0008_0000;
    const FILE_READ_ATTRIBUTES: u32 = 0x80;
    const GENERIC_READ: u32 = 0x8000_0000;
    const GENERIC_WRITE: u32 = 0x4000_0000;
    /// Largest plan, and largest `.rdp` file the setup reads.
    const MAX_READ: u64 = 1 << 20;

    /// Opens the file named, never what a link of that name points at; nobody else may open it
    /// meanwhile.
    fn open_own(path: &Path) -> std::result::Result<File, String> {
        let f = File::options()
            .access_mode(GENERIC_READ | GENERIC_WRITE)
            .share_mode(0)
            .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
            .open(path)
            .map_err(|e| format!("{}: {e}", path.display()))?;
        let info = handle_info(&f)?;
        // A link, a folder or a second name (a hard link to someone else's file) isn't the user's own file.
        if info.dwFileAttributes & (FILE_ATTRIBUTE_REPARSE_POINT | FILE_ATTRIBUTE_DIRECTORY) != 0 || info.nNumberOfLinks != 1 {
            return Err(tr!("{file} isn't a plain file.", file = path.display()));
        }
        if f.metadata().map_err(|e| e.to_string())?.len() > MAX_READ {
            return Err(tr!("{file} is too big.", file = path.display()));
        }
        Ok(f)
    }

    fn handle_info(f: &File) -> std::result::Result<BY_HANDLE_FILE_INFORMATION, String> {
        let mut info = BY_HANDLE_FILE_INFORMATION::default();
        // SAFETY: `f` is an open handle; `info` is a local.
        unsafe { GetFileInformationByHandle(HANDLE(f.as_raw_handle()), &mut info) }.map_err(|e| e.message())?;
        Ok(info)
    }

    /// Replaces the whole content of an open file.
    fn rewrite(f: &mut File, bytes: &[u8]) -> std::io::Result<()> {
        f.seek(SeekFrom::Start(0))?;
        f.set_len(0)?;
        f.write_all(bytes)?;
        f.flush()
    }

    /// Called first thing in `main`: Some(exit code) when this process is the elevated setup,
    /// which has then done its work.
    pub fn elevated_entry() -> Option<i32> {
        let args: Vec<std::ffi::OsString> = std::env::args_os().collect();
        let plan_path = plan_arg(&args)?;
        crate::i18n::set("");
        if !plan_name_ok(&plan_path) {
            return Some(2);
        }
        let Ok(mut file) = open_own(&plan_path) else { return Some(2) };
        let mut bytes = Zeroizing::new(Vec::new());
        let read = file.read_to_end(&mut bytes).map_err(|e| e.to_string());
        // Wiped before anything else happens: it holds passwords.
        let _ = rewrite(&mut file, &vec![0u8; bytes.len()]).and_then(|()| file.set_len(0));
        let mut result = SetupResult::default();
        let plan = read
            .and_then(|_| serde_json::from_slice::<SetupPlan>(&bytes).map_err(|e| e.to_string()))
            .and_then(|p| check_plan(&p).map(|()| p));
        drop(bytes);
        match plan {
            Ok(plan) => execute(&plan, &plan_path, &mut result),
            Err(e) => result.push("plan", Err(e)),
        }
        let written = serde_json::to_vec(&result).ok().is_some_and(|j| rewrite(&mut file, &j).is_ok());
        Some(if written { 0 } else { 1 })
    }

    fn execute(plan: &SetupPlan, plan_path: &Path, r: &mut SetupResult) {
        match plan.action {
            SetupAction::Setup => set_up(plan, plan_path, r),
            SetupAction::Uninstall => uninstall(plan, r),
        }
    }

    fn set_up(plan: &SetupPlan, plan_path: &Path, r: &mut SetupResult) {
        let main = plan.main_user_sid.as_str();
        let root = program_data();
        // Before any value changes: where its previous state goes.
        let existing: Vec<String> = plan.managed_users.iter().filter_map(|u| users::sid_of(u)).collect();
        let mut record = Recorder::open(&root, main, &existing);
        if plan.system {
            r.push("firewall", firewall::add());
            policies(&mut record, r);
        }
        for u in &plan.users {
            r.push(format!("user:{}", u.name), users::create_or_repair(&u.name, &u.password));
            r.push(format!("remoteDesktopUsers:{}", u.name), users::add_to_remote_desktop_users(&u.name));
            r.push(format!("hide:{}", u.name), record.set(reg::USER_LIST, &u.name, RegData::Dword(0)).map(|()| String::new()));
        }
        // Their control configs go with `control_configs` (no longer managed).
        for name in &plan.remove_users {
            let _ = reg::delete(HKLM, reg::USER_LIST, name);
            r.push(format!("remove:{name}"), users::delete(name));
        }
        // Alt users that don't exist (a failed create) can't be in an ACL.
        let alts: Vec<String> = plan.managed_users.iter().filter_map(|u| users::sid_of(u)).collect();
        let shared = secure_dir(&root, &shared_sddl(main, &alts));
        r.push("sharedFolder", shared.clone().map(|()| root.display().to_string()));
        r.push(
            "usersJson",
            shared.clone().and_then(|()| write_new(&root.join("users.json"), users_json(&plan.managed_users).as_bytes(), None)).map(|()| String::new()),
        );
        if shared.is_ok() {
            control_configs(plan, r);
        }
        if !plan.system || shared.is_err() {
            return;
        }
        r.push("programCopy", copy_program(main, &alts));
        r.push("autostart", reg::set_string(HKLM, reg::RUN, RUN_VALUE, &run_command()).map(|()| run_command()));
        match certificate(plan.thumbprint.as_deref()) {
            Ok((sha1, sha256)) => {
                r.thumbprint = Some(sha1.clone());
                r.push("certificate", Ok(sha1.clone()));
                r.push("trustedThumbprints", record.trust(&sha1).map(|()| String::new()));
                for f in &plan.rdp_files {
                    r.push(format!("sign:{}", f.file_stem().unwrap_or_default().to_string_lossy()), sign(f, plan_path, &sha256));
                }
            }
            Err(e) => r.push("certificate", Err(e)),
        }
    }

    /// `restore::POLICIES`, one result per step (its first failure).
    fn policies(record: &mut Recorder, r: &mut SetupResult) {
        let mut steps: Vec<(&str, std::result::Result<String, String>)> = Vec::new();
        for p in POLICIES {
            if p.only_if_present && reg::prior(HKLM, p.key, p.value) == Prior::Absent {
                continue;
            }
            let outcome = record.set(p.key, p.value, RegData::Dword(p.data)).map(|()| String::new());
            match steps.last_mut() {
                Some((step, done)) if *step == p.step => {
                    if done.is_ok() {
                        *done = outcome;
                    }
                }
                _ => steps.push((p.step, outcome)),
            }
        }
        for (step, outcome) in steps {
            r.push(step, outcome);
        }
    }

    /// `restore.json` while the setup runs: each value's previous state is written there (as an
    /// administrators-only file) before the value first changes; a value whose state can't be
    /// kept isn't changed.
    struct Recorder {
        path: PathBuf,
        state: RestoreState,
        /// A setup from before `restore.json` ran here: values that already say what AltDock sets
        /// have an unknown previous state.
        earlier: bool,
        ready: std::result::Result<(), String>,
    }

    impl Recorder {
        fn open(root: &Path, main: &str, alts: &[String]) -> Recorder {
            let path = root.join(restore::FILE);
            let mut earlier = root.join("users.json").is_file() || reg::string(HKLM, reg::RUN, RUN_VALUE).is_some();
            let mut state = RestoreState::default();
            let ready = secure_dir(root, &shared_sddl(main, alts)).and_then(|()| {
                match read_admin_file(&path)? {
                    Some(bytes) => match serde_json::from_slice::<RestoreState>(&bytes) {
                        Ok(s) => state.merge(s),
                        // Damaged: what AltDock's values say now can't be told from before.
                        Err(e) => {
                            log::warn!("{} is damaged ({e}); starting over", path.display());
                            earlier = true;
                        }
                    },
                    None if path.exists() => earlier = true,
                    None => {}
                }
                Ok(())
            });
            Recorder { path, state, earlier, ready }
        }

        fn save(&self) -> std::result::Result<(), String> {
            let json = serde_json::to_vec_pretty(&self.state).map_err(|e| e.to_string())?;
            // Written aside, then moved over the old one: never a moment without it.
            let new = self.path.with_extension("json.new");
            write_new(&new, &json, Some(&admin_file_sddl()))?;
            std::fs::rename(&new, &self.path).map_err(|e| format!("{}: {e}", self.path.display()))
        }

        /// Sets an HKLM value, its previous state recorded first.
        fn set(&mut self, key: &str, value: &str, data: RegData) -> std::result::Result<(), String> {
            self.ready.clone()?;
            if self.state.record(key, value, reg::prior(HKLM, key, value), &data, self.earlier) {
                self.save()?;
            }
            reg::set(HKLM, key, value, &data)
        }

        /// Adds the signing certificate to `TrustedCertThumbprints`, the list's previous state and
        /// the thumbprint recorded first.
        fn trust(&mut self, sha1: &str) -> std::result::Result<(), String> {
            self.ready.clone()?;
            let now = reg::prior(HKLM, reg::TS_POLICY, TRUSTED_THUMBPRINTS);
            let list = match now.data() {
                Some(RegData::String(s)) => s.clone(),
                _ => String::new(),
            };
            let target = RegData::String(with_thumbprint(&list, sha1));
            let recorded = self.state.record(reg::TS_POLICY, TRUSTED_THUMBPRINTS, now, &target, self.earlier);
            if self.state.add_thumbprint(sha1) || recorded {
                self.save()?;
            }
            reg::set(HKLM, reg::TS_POLICY, TRUSTED_THUMBPRINTS, &target)
        }
    }

    /// Reads a file only administrators can have made (owned by Administrators, SYSTEM or this
    /// administrator), never through a link. None when there is none, or it is someone else's.
    fn read_admin_file(path: &Path) -> std::result::Result<Option<Vec<u8>>, String> {
        let f = match File::options()
            .access_mode(GENERIC_READ | READ_CONTROL)
            .share_mode(1)
            .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
            .open(path)
        {
            Ok(f) => f,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(format!("{}: {e}", path.display())),
        };
        let info = handle_info(&f)?;
        let plain = info.dwFileAttributes & (FILE_ATTRIBUTE_REPARSE_POINT | FILE_ATTRIBUTE_DIRECTORY) == 0 && info.nNumberOfLinks == 1;
        if !plain || !owned_by_admins(&f) {
            log::warn!("{} isn't the setup's own file; ignored", path.display());
            return Ok(None);
        }
        let mut bytes = Vec::new();
        (&f).take(MAX_READ).read_to_end(&mut bytes).map_err(|e| format!("{}: {e}", path.display()))?;
        Ok(Some(bytes))
    }

    /// Carries out one `Undo` on an HKLM value.
    fn apply(u: Undo, key: &str, value: &str) -> std::result::Result<String, String> {
        match u {
            Undo::Keep => Ok(String::new()),
            Undo::Set(data) => reg::set(HKLM, key, value, &data).map(|()| tr!("Restored.")),
            Undo::Delete => reg::delete(HKLM, key, value).map(|()| tr!("Removed.")),
        }
    }

    // ---------- uninstall ----------

    /// "Remove from this PC": the autostart, the signing certificates and their trust, the
    /// policies and settings (back to what they were), with `delete_users` the alt users and their
    /// profiles, the firewall rule, and `%ProgramData%\AltDock` last (it holds `restore.json`).
    fn uninstall(plan: &SetupPlan, r: &mut SetupResult) {
        let root = program_data();
        let read = |name: &str| read_admin_file(&root.join(name)).ok().flatten();
        let (mut state, have_file) = match read(restore::FILE).map(|b| serde_json::from_slice::<RestoreState>(&b)) {
            Some(Ok(s)) => (s, true),
            _ => (RestoreState::default(), false),
        };
        // Only what the setup itself sets is ever written back.
        state.values.retain(|v| restore::restorable(&v.key, &v.value));
        r.push(
            "restoreState",
            Ok(if have_file {
                root.join(restore::FILE).display().to_string()
            } else {
                tr!("No record of the previous values: only those that still say what AltDock set are removed.")
            }),
        );
        let listed = read("users.json").map(|b| users_in_json(&b)).unwrap_or_default();

        r.push("autostart", reg::delete(HKLM, reg::RUN, RUN_VALUE).map(|()| String::new()));

        let mut ours = state.thumbprints.clone();
        match remove_certificates() {
            Ok(removed) => {
                r.push("certificate", Ok(removed.join(", ")));
                ours.extend(removed);
            }
            Err(e) => r.push("certificate", Err(e)),
        }
        let now = reg::prior(HKLM, reg::TS_POLICY, TRUSTED_THUMBPRINTS);
        let list = match now.data() {
            Some(RegData::String(s)) => Some(s.as_str()),
            _ => None,
        };
        let u = undo_thumbprints(list, &ours, state.prior(reg::TS_POLICY, TRUSTED_THUMBPRINTS));
        r.push("trustedThumbprints", apply(u, reg::TS_POLICY, TRUSTED_THUMBPRINTS));

        let mut steps: Vec<(&str, std::result::Result<String, String>)> = Vec::new();
        for p in POLICIES {
            let now = reg::prior(HKLM, p.key, p.value);
            let fallback = p.fallback.map(RegData::Dword);
            let u = undo(state.prior(p.key, p.value), have_file, now.data(), &RegData::Dword(p.data), fallback.as_ref());
            let outcome = apply(u, p.key, p.value);
            match steps.last_mut() {
                Some((step, done)) if *step == p.step => {
                    if done.is_ok() {
                        *done = outcome;
                    }
                }
                _ => steps.push((p.step, outcome)),
            }
        }
        for (step, outcome) in steps {
            r.push(step, outcome);
        }

        for name in uninstall_users(&plan.managed_users, &listed, &state.hidden_users()) {
            if plan.delete_users {
                let deleted = users::delete(&name);
                if deleted.is_ok() {
                    let _ = reg::delete(HKLM, reg::USER_LIST, &name);
                }
                r.push(format!("remove:{name}"), deleted);
            } else {
                let now = reg::prior(HKLM, reg::USER_LIST, &name);
                let u = undo(state.prior(reg::USER_LIST, &name), have_file, now.data(), &RegData::Dword(0), None);
                r.push(format!("hide:{name}"), apply(u, reg::USER_LIST, &name));
            }
        }
        // Only now that Remote Desktop is back as it was (off, unless it was on before AltDock).
        r.push("firewall", firewall::remove());
        r.push("sharedFolder", remove_shared(&root));
    }

    /// Deletes `%ProgramData%\AltDock` when the setup made it (owned by administrators); never
    /// follows a link: one in its place is removed itself, and `std::fs::remove_dir_all` deletes
    /// through handles, removing links (symlinks, junctions) inside without following them.
    fn remove_shared(root: &Path) -> std::result::Result<String, String> {
        let fail = |e: std::io::Error| format!("{}: {e}", root.display());
        match std::fs::symlink_metadata(root) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(String::new()),
            Err(e) => return Err(fail(e)),
            Ok(m) if m.file_type().is_symlink() || m.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 => {
                return remove_entry(root).map(|()| String::new()).map_err(fail);
            }
            Ok(_) => {}
        }
        let left = tr!("{path} wasn't made by AltDock's setup; it was left alone.", path = root.display());
        let dir = File::options()
            .access_mode(READ_CONTROL | FILE_READ_ATTRIBUTES)
            .share_mode(7)
            .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
            .open(root)
            .map_err(fail)?;
        let attributes = handle_info(&dir)?.dwFileAttributes;
        if attributes & FILE_ATTRIBUTE_DIRECTORY == 0 || attributes & FILE_ATTRIBUTE_REPARSE_POINT != 0 || !owned_by_admins(&dir) {
            return Err(left);
        }
        drop(dir);
        std::fs::remove_dir_all(root).map(|()| root.display().to_string()).map_err(fail)
    }

    /// The control configs, each readable by its own alt user only; configs of users AltDock
    /// doesn't manage (any more) go.
    fn control_configs(plan: &SetupPlan, r: &mut SetupResult) {
        let dir = program_data().join("control");
        if let Err(e) = secure_dir(&dir, &private_dir_sddl()) {
            // Without its ACL the folder would let every alt user read every token.
            r.push("controlFolder", Err(e));
            return;
        }
        for e in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
            let name = e.file_name().to_string_lossy().to_lowercase();
            let managed = plan.managed_users.iter().any(|u| format!("{}.json", u.to_lowercase()) == name);
            if !managed || !e.file_type().is_ok_and(|t| t.is_file()) {
                let _ = remove_entry(&e.path());
            }
        }
        for u in &plan.users {
            let path = dir.join(format!("{}.json", u.name));
            let outcome = users::sid_of(&u.name)
                .ok_or_else(|| tr!("{user} doesn't exist.", user = u.name))
                .and_then(|alt| write_new(&path, control_json(u.port, &u.token, u.mode).as_bytes(), Some(&control_file_sddl(&alt))))
                .map(|()| path.display().to_string());
            r.push(format!("control:{}", u.name), outcome);
        }
    }

    /// Removes a file, a folder tree or a link (only the link) of that name.
    fn remove_entry(path: &Path) -> std::io::Result<()> {
        match std::fs::symlink_metadata(path) {
            Ok(m) if m.file_type().is_symlink() || m.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 => {
                std::fs::remove_dir(path).or_else(|_| std::fs::remove_file(path))
            }
            Ok(m) if m.is_dir() => std::fs::remove_dir_all(path),
            Ok(_) => std::fs::remove_file(path),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e),
        }
    }

    /// Writes a new file in place of whatever had that name (so never through an old link), with
    /// its folder's permissions or `sddl`'s, set before anything is in it.
    fn write_new(path: &Path, bytes: &[u8], sddl: Option<&str>) -> std::result::Result<(), String> {
        let fail = |e: std::io::Error| format!("{}: {e}", path.display());
        remove_entry(path).map_err(fail)?;
        let mut f =
            File::options().write(true).access_mode(GENERIC_WRITE | READ_CONTROL | WRITE_DAC | WRITE_OWNER).create_new(true).open(path).map_err(fail)?;
        if let Some(sddl) = sddl {
            apply_sddl(&f, sddl).inspect_err(|_| {
                let _ = std::fs::remove_file(path);
            })?;
        }
        f.write_all(bytes).and_then(|()| f.flush()).map_err(fail)
    }

    /// Makes `path` a folder owned by administrators with exactly `sddl`'s permissions. Refused
    /// when something else has that name (a link, a file) or another user made the folder: they
    /// could still hold it open, or have put things in it.
    fn secure_dir(path: &Path, sddl: &str) -> std::result::Result<(), String> {
        let ours = tr!("{path} wasn't made by AltDock's setup. Delete it, then run the setup again.", path = path.display());
        match std::fs::symlink_metadata(path) {
            Ok(m) if m.is_dir() && m.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT == 0 => {}
            Ok(_) => return Err(ours),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => std::fs::create_dir(path).map_err(|e| format!("{}: {e}", path.display()))?,
            Err(e) => return Err(format!("{}: {e}", path.display())),
        }
        let dir = File::options()
            .access_mode(READ_CONTROL | WRITE_DAC | WRITE_OWNER | FILE_READ_ATTRIBUTES)
            .share_mode(7)
            .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
            .open(path)
            .map_err(|e| format!("{}: {e}", path.display()))?;
        let attributes = handle_info(&dir)?.dwFileAttributes;
        if attributes & FILE_ATTRIBUTE_DIRECTORY == 0 || attributes & FILE_ATTRIBUTE_REPARSE_POINT != 0 || !owned_by_admins(&dir) {
            return Err(ours);
        }
        apply_sddl(&dir, sddl)
    }

    /// The object's owner is Administrators, SYSTEM, or the administrator running this setup.
    fn owned_by_admins(f: &File) -> bool {
        let mut owner = PSID::default();
        let mut sd = PSECURITY_DESCRIPTOR::default();
        // SAFETY: `f` is open with READ_CONTROL; the descriptor is LocalAlloc'd and freed below.
        let r = unsafe { GetSecurityInfo(HANDLE(f.as_raw_handle()), SE_FILE_OBJECT, OWNER_SECURITY_INFORMATION, Some(&mut owner), None, None, None, Some(&mut sd)) };
        if r.is_err() {
            return false;
        }
        let sid = users::sid_string(owner);
        // SAFETY: allocated by GetSecurityInfo; `owner` points into it and isn't used after.
        unsafe { LocalFree(Some(HLOCAL(sd.0))) };
        sid.is_some_and(|s| s == "S-1-5-32-544" || s == "S-1-5-18" || users::current_user_sid().is_some_and(|me| me == s))
    }

    /// Sets the owner and the (protected) DACL `sddl` names on the open object itself: nothing is
    /// propagated by name, so no link below it is followed.
    fn apply_sddl(f: &File, sddl: &str) -> std::result::Result<(), String> {
        let mut sd = PSECURITY_DESCRIPTOR::default();
        // SAFETY: the descriptor is LocalAlloc'd and freed below.
        unsafe { ConvertStringSecurityDescriptorToSecurityDescriptorW(&HSTRING::from(sddl), SDDL_REVISION_1, &mut sd, None) }.map_err(|e| e.message())?;
        let info = OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION;
        // SAFETY: `f` is open with WRITE_DAC and WRITE_OWNER; `sd` is the descriptor just made.
        let r = unsafe { SetKernelObjectSecurity(HANDLE(f.as_raw_handle()), info, sd) }.map_err(|e| e.message());
        // SAFETY: allocated by ConvertStringSecurityDescriptorToSecurityDescriptorW.
        unsafe { LocalFree(Some(HLOCAL(sd.0))) };
        r
    }

    /// Copies this exe (and DLLs beside it) to `%ProgramData%\AltDock\app`, after emptying it:
    /// nothing but these may be there. A running copy (a session still open) is renamed aside
    /// first: Windows allows that, not deleting it.
    fn copy_program(main: &str, alts: &[String]) -> std::result::Result<String, String> {
        let own = std::env::current_exe().map_err(|e| e.to_string())?;
        let target = app_exe();
        let dir = target.parent().ok_or("no app folder")?.to_owned();
        secure_dir(&dir, &shared_sddl(main, alts))?;
        let mut files = vec![(own.clone(), target.clone())];
        if let Some(src) = own.parent() {
            for e in std::fs::read_dir(src).map_err(|e| e.to_string())?.flatten() {
                let p = e.path();
                if p.extension().is_some_and(|x| x.eq_ignore_ascii_case("dll")) && e.file_type().is_ok_and(|t| t.is_file()) {
                    files.push((p, dir.join(e.file_name())));
                }
            }
        }
        for e in std::fs::read_dir(&dir).map_err(|e| e.to_string())?.flatten() {
            let p = e.path();
            if remove_entry(&p).is_err() && !e.file_name().to_string_lossy().contains(".old-") {
                let aside = p.with_file_name(format!("{}.old-{}", e.file_name().to_string_lossy(), hex(&random(4))));
                std::fs::rename(&p, aside).map_err(|e| format!("{}: {e}", p.display()))?;
            }
        }
        for (from, to) in &files {
            std::fs::copy(from, to).map_err(|e| format!("{}: {e}", to.display()))?;
        }
        debug_assert!(target.ends_with(APP_EXE));
        Ok(target.display().to_string())
    }

    // ---------- signing ----------

    struct Store(HCERTSTORE);

    impl Drop for Store {
        fn drop(&mut self) {
            // SAFETY: opened by `open_store`, closed once.
            let _ = unsafe { CertCloseStore(Some(self.0), 0) };
        }
    }

    struct Cert(*const CERT_CONTEXT);

    impl Drop for Cert {
        fn drop(&mut self) {
            // SAFETY: a context from CertCreateSelfSignCertificate or CertFindCertificateInStore.
            let _ = unsafe { CertFreeCertificateContext(Some(self.0)) };
        }
    }

    /// LocalMachine's `MY` or `Root` store.
    fn open_store(name: PCWSTR) -> std::result::Result<Store, String> {
        // SAFETY: `name` is a static wide string.
        unsafe {
            CertOpenStore(
                CERT_STORE_PROV_SYSTEM_W,
                CERT_QUERY_ENCODING_TYPE(0),
                None,
                CERT_OPEN_STORE_FLAGS(CERT_SYSTEM_STORE_LOCAL_MACHINE),
                Some(name.as_ptr().cast()),
            )
        }
        .map(Store)
        .map_err(|e| e.message())
    }

    fn property(cert: &Cert, id: u32) -> Option<Vec<u8>> {
        let mut len = 0u32;
        // SAFETY: asks for the size, then fills a buffer of that size.
        unsafe { CertGetCertificateContextProperty(cert.0, id, None, &mut len) }.ok()?;
        let mut buf = vec![0u8; len as usize];
        unsafe { CertGetCertificateContextProperty(cert.0, id, Some(buf.as_mut_ptr().cast()), &mut len) }.ok()?;
        buf.truncate(len as usize);
        Some(buf)
    }

    /// The signing certificate: the one made before when it is still in LocalMachine\My with its
    /// key, else a new one. Either way it is put in Root too. Its SHA-1 and SHA-256 thumbprints.
    fn certificate(earlier: Option<&str>) -> std::result::Result<(String, String), String> {
        let subject = subject_name()?;
        let my = open_store(w!("MY"))?;
        let cert = match earlier.and_then(|t| find(&my, t, &subject)) {
            Some(c) => c,
            None => {
                let c = make_certificate(subject)?;
                // SAFETY: both are live; the store keeps its own copy.
                unsafe { CertAddCertificateContextToStore(Some(my.0), c.0, CERT_STORE_ADD_REPLACE_EXISTING, None) }.map_err(|e| e.message())?;
                c
            }
        };
        let root = open_store(w!("Root"))?;
        // SAFETY: as above.
        unsafe { CertAddCertificateContextToStore(Some(root.0), cert.0, CERT_STORE_ADD_REPLACE_EXISTING, None) }.map_err(|e| e.message())?;
        let sha1 = property(&cert, CERT_SHA1_HASH_PROP_ID).ok_or("no SHA-1 thumbprint")?;
        let sha256 = property(&cert, CERT_SHA256_HASH_PROP_ID).ok_or("no SHA-256 thumbprint")?;
        Ok((hex(&sha1), hex(&sha256)))
    }

    /// The encoded `CERT_SUBJECT`.
    fn subject_name() -> std::result::Result<Vec<u8>, String> {
        let subject = HSTRING::from(CERT_SUBJECT);
        let mut len = 0u32;
        // SAFETY: asks for the size, then fills a buffer of that size.
        unsafe { CertStrToNameW(X509_ASN_ENCODING, &subject, CERT_X500_NAME_STR, None, None, &mut len, None) }.map_err(|e| e.message())?;
        let mut name = vec![0u8; len as usize];
        unsafe { CertStrToNameW(X509_ASN_ENCODING, &subject, CERT_X500_NAME_STR, None, Some(name.as_mut_ptr()), &mut len, None) }.map_err(|e| e.message())?;
        name.truncate(len as usize);
        Ok(name)
    }

    /// The certificate with that SHA-1 thumbprint, if it is one this setup made (self-signed, our
    /// subject) and its private key is still known. The plan names the thumbprint: any other
    /// certificate of the machine's must never end up in Root through it.
    fn find(store: &Store, thumbprint: &str, subject: &[u8]) -> Option<Cert> {
        let t = normalize_thumbprint(thumbprint);
        let mut bytes: Vec<u8> = (0..t.len() / 2).filter_map(|i| u8::from_str_radix(&t[i * 2..i * 2 + 2], 16).ok()).collect();
        let blob = CRYPT_INTEGER_BLOB { cbData: bytes.len() as u32, pbData: bytes.as_mut_ptr() };
        // SAFETY: `blob` points at `bytes`, alive through the call.
        let found = unsafe { CertFindCertificateInStore(store.0, X509_ASN_ENCODING, 0, CERT_FIND_SHA1_HASH, Some((&raw const blob).cast()), None) };
        if found.is_null() {
            return None;
        }
        let cert = Cert(found);
        if !self_signed_ours(cert.0, subject) {
            return None;
        }
        property(&cert, CERT_KEY_PROV_INFO_PROP_ID).map(|_| cert)
    }

    /// The certificate's subject and issuer are both `subject` (`CERT_SUBJECT`, encoded).
    fn self_signed_ours(cert: *const CERT_CONTEXT, subject: &[u8]) -> bool {
        // SAFETY: a live context; its info and the name blobs live as long as it.
        unsafe {
            let info = &*(*cert).pCertInfo;
            let blob = |b: &CRYPT_INTEGER_BLOB| std::slice::from_raw_parts(b.pbData, b.cbData as usize);
            blob(&info.Subject) == subject && blob(&info.Issuer) == subject
        }
    }

    /// Takes every certificate the setup made (self-signed, `CERT_SUBJECT`) out of LocalMachine's
    /// `MY` and `Root`, with the key container it made for it; their SHA-1 thumbprints. No other
    /// certificate is touched.
    fn remove_certificates() -> std::result::Result<Vec<String>, String> {
        let subject = subject_name()?;
        let mut removed: Vec<String> = Vec::new();
        let mut failed: Vec<String> = Vec::new();
        for (name, keys) in [(w!("MY"), true), (w!("Root"), false)] {
            let store = open_store(name)?;
            let mut prev: *const CERT_CONTEXT = std::ptr::null();
            loop {
                // SAFETY: `prev` is the last context this enumeration returned (freed by the call).
                let c = unsafe { CertEnumCertificatesInStore(store.0, (!prev.is_null()).then_some(prev)) };
                if c.is_null() {
                    break;
                }
                prev = c;
                if !self_signed_ours(c, &subject) {
                    continue;
                }
                // Deleting frees the context it is given: a copy, so the enumeration goes on.
                // SAFETY: `c` is live.
                let copy = Cert(unsafe { CertDuplicateCertificateContext(Some(c)) });
                let thumbprint = property(&copy, CERT_SHA1_HASH_PROP_ID).map(|t| hex(&t)).unwrap_or_default();
                if keys {
                    delete_key(&copy);
                }
                let raw = copy.0;
                std::mem::forget(copy);
                // SAFETY: `raw` is the copy just made; the call frees it, even when it fails.
                match unsafe { CertDeleteCertificateFromStore(raw) } {
                    Ok(()) if !removed.contains(&thumbprint) => removed.push(thumbprint),
                    Ok(()) => {}
                    Err(e) => failed.push(format!("{thumbprint}: {}", e.message())),
                }
            }
        }
        if failed.is_empty() { Ok(removed) } else { Err(failed.join("; ")) }
    }

    /// Deletes the machine key container `make_certificate` made for `cert` (named
    /// `AltDock-RDP-<hex>`); any other key is left alone.
    fn delete_key(cert: &Cert) {
        let Some(buf) = property(cert, CERT_KEY_PROV_INFO_PROP_ID) else { return };
        if buf.len() < size_of::<CRYPT_KEY_PROV_INFO>() {
            return;
        }
        // SAFETY: the property is a CRYPT_KEY_PROV_INFO whose strings point into `buf`, alive here.
        let info = unsafe { std::ptr::read_unaligned(buf.as_ptr().cast::<CRYPT_KEY_PROV_INFO>()) };
        // SAFETY: NUL-terminated strings inside `buf` (or null).
        let container = unsafe { info.pwszContainerName.to_string() }.unwrap_or_default();
        if !container.starts_with("AltDock-RDP-") || info.dwFlags.0 & CRYPT_MACHINE_KEYSET.0 == 0 || info.dwProvType != PROV_RSA_AES {
            return;
        }
        let mut prov = 0usize;
        // SAFETY: the strings live in `buf`; CRYPT_DELETEKEYSET hands back no context to release.
        if let Err(e) = unsafe {
            CryptAcquireContextW(&mut prov, PCWSTR(info.pwszContainerName.0), PCWSTR(info.pwszProvName.0), info.dwProvType, CRYPT_DELETEKEYSET | CRYPT_MACHINE_KEYSET.0)
        } {
            log::debug!("key container {container} not deleted: {}", e.message());
        }
    }

    /// A self-signed code-signing certificate (RSA 2048, SHA-256, 10 years) with its key in a new
    /// machine key container.
    fn make_certificate(mut name: Vec<u8>) -> std::result::Result<Cert, String> {
        // Subject and issuer.
        let name_blob = CRYPT_INTEGER_BLOB { cbData: name.len() as u32, pbData: name.as_mut_ptr() };

        // The key.
        let mut container: Vec<u16> = format!("AltDock-RDP-{}", hex(&random(8))).encode_utf16().chain([0]).collect();
        let mut prov = 0usize;
        // SAFETY: `container` is NUL-terminated; the context is released below.
        unsafe { CryptAcquireContextW(&mut prov, PCWSTR(container.as_ptr()), MS_ENH_RSA_AES_PROV_W, PROV_RSA_AES, CRYPT_NEWKEYSET | CRYPT_MACHINE_KEYSET.0) }
            .map_err(|e| e.message())?;
        let made = (|| {
            let mut key = 0usize;
            // SAFETY: `prov` is the context just acquired; the key handle is destroyed at once (the
            // key stays in the container).
            unsafe {
                CryptGenKey(prov, ALG_ID(AT_SIGNATURE.0), CRYPT_KEY_FLAGS(2048 << 16), &mut key).map_err(|e| e.message())?;
                let _ = CryptDestroyKey(key);
            }

            // Code signing only.
            let mut oid = PSTR(szOID_PKIX_KP_CODE_SIGNING.0.cast_mut());
            let usage = CTL_USAGE { cUsageIdentifier: 1, rgpszUsageIdentifier: &mut oid };
            let mut elen = 0u32;
            // SAFETY: asks for the size, then fills a buffer of that size; `usage` lives through both.
            unsafe { CryptEncodeObjectEx(X509_ASN_ENCODING, X509_ENHANCED_KEY_USAGE, (&raw const usage).cast(), CRYPT_ENCODE_OBJECT_FLAGS(0), None, None, &mut elen) }
                .map_err(|e| e.message())?;
            let mut eku = vec![0u8; elen as usize];
            unsafe {
                CryptEncodeObjectEx(
                    X509_ASN_ENCODING,
                    X509_ENHANCED_KEY_USAGE,
                    (&raw const usage).cast(),
                    CRYPT_ENCODE_OBJECT_FLAGS(0),
                    None,
                    Some(eku.as_mut_ptr().cast()),
                    &mut elen,
                )
            }
            .map_err(|e| e.message())?;
            let mut ext = CERT_EXTENSION {
                pszObjId: PSTR(szOID_ENHANCED_KEY_USAGE.0.cast_mut()),
                fCritical: false.into(),
                Value: CRYPT_INTEGER_BLOB { cbData: elen, pbData: eku.as_mut_ptr() },
            };
            let exts = CERT_EXTENSIONS { cExtension: 1, rgExtension: &mut ext };
            let info = CRYPT_KEY_PROV_INFO {
                pwszContainerName: PWSTR(container.as_mut_ptr()),
                pwszProvName: PWSTR(MS_ENH_RSA_AES_PROV_W.0.cast_mut()),
                dwProvType: PROV_RSA_AES,
                dwFlags: CRYPT_MACHINE_KEYSET,
                dwKeySpec: AT_SIGNATURE.0,
                ..Default::default()
            };
            let alg = CRYPT_ALGORITHM_IDENTIFIER { pszObjId: PSTR(szOID_RSA_SHA256RSA.0.cast_mut()), ..Default::default() };
            // SAFETY: no arguments.
            let mut end = unsafe { GetSystemTime() };
            end.wYear += 10;
            if end.wMonth == 2 && end.wDay > 28 {
                end.wDay = 28;
            }
            // SAFETY: every pointer is to a local that lives through the call.
            let cert = unsafe {
                CertCreateSelfSignCertificate(
                    Some(HCRYPTPROV_OR_NCRYPT_KEY_HANDLE(prov)),
                    &name_blob,
                    CERT_CREATE_SELFSIGN_FLAGS(0),
                    Some(&info),
                    Some(&alg),
                    None,
                    Some(&end),
                    Some(&exts),
                )
            };
            if cert.is_null() { Err(windows::core::Error::from_thread().message()) } else { Ok(Cert(cert)) }
        })();
        // SAFETY: acquired above.
        let _ = unsafe { CryptReleaseContext(prov, 0) };
        made
    }

    /// Signs one of the main app's session files with `rdpsign.exe /sha256 <thumbprint>`: read
    /// through one handle, checked to be a file `rdp::build` made, signed as a copy in a folder
    /// only administrators can write, then written back through the same handle. Nothing else the
    /// user could name gets signed, and nothing of theirs is opened by path twice.
    fn sign(file: &Path, plan: &Path, sha256: &str) -> std::result::Result<String, String> {
        if !rdp_file_ok(file, plan) {
            return Err(tr!("{file} isn't a session file.", file = file.display()));
        }
        let mut f = open_own(file)?;
        let mut bytes = Vec::new();
        f.read_to_end(&mut bytes).map_err(|e| e.to_string())?;
        if !rdp::is_ours(&rdp::decode(&bytes)) {
            return Err(tr!("{file} isn't a session file.", file = file.display()));
        }
        let scratch = program_data().join("sign");
        secure_dir(&scratch, &private_dir_sddl())?;
        let copy = scratch.join(format!("{}.rdp", hex(&random(8))));
        write_new(&copy, &bytes, None)?;
        let root = std::env::var_os("SystemRoot").map(PathBuf::from).unwrap_or_else(|| r"C:\Windows".into());
        let out = std::process::Command::new(root.join(r"System32\rdpsign.exe"))
            .arg("/sha256")
            .arg(sha256)
            .arg(&copy)
            .creation_flags(0x0800_0000)
            .output();
        let signed = std::fs::read(&copy);
        let _ = std::fs::remove_file(&copy);
        let out = out.map_err(|e| e.to_string())?;
        if !out.status.success() {
            let text = String::from_utf8_lossy(&out.stdout).trim().to_owned() + " " + String::from_utf8_lossy(&out.stderr).trim();
            return Err(format!("rdpsign {:?}: {}", out.status.code(), text.trim()));
        }
        rewrite(&mut f, &signed.map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
        Ok(file.display().to_string())
    }
}

#[cfg(test)]
mod tests {
    use zeroize::Zeroizing;

    use super::*;
    use crate::sessions::types::{PlanUser, SetupResult};

    fn plan() -> SetupPlan {
        SetupPlan {
            action: SetupAction::Setup,
            main_user_sid: "S-1-5-21-1004336348-1177238915-682003330-1001".into(),
            managed_users: vec!["altdock-1".into(), "altdock-2".into()],
            users: vec![PlanUser {
                name: "altdock-2".into(),
                password: Zeroizing::new("Ab3#Ab3#Ab3#Ab3#Ab3#Ab3#".into()),
                port: 47612,
                token: Zeroizing::new("ab".repeat(32)),
                mode: SessionMode::Dedicated,
            }],
            remove_users: vec!["altdock-3".into()],
            rdp_files: vec![PathBuf::from(r"C:\Users\you\AppData\Local\AltDock\sessions\Farm.rdp")],
            thumbprint: Some("A".repeat(40)),
            system: true,
            delete_users: false,
        }
    }

    #[test]
    fn plans_round_trip() {
        let p = plan();
        let json = serde_json::to_string(&p).unwrap();
        assert!(json.contains("\"mainUserSid\"") && json.contains("\"managedUsers\"") && json.contains("\"rdpFiles\""), "{json}");
        let back: SetupPlan = serde_json::from_str(&json).unwrap();
        assert_eq!(back.users[0].name, "altdock-2");
        assert_eq!(*back.users[0].password, *p.users[0].password);
        assert_eq!(*back.users[0].token, *p.users[0].token);
        assert_eq!(back.users[0].mode, SessionMode::Dedicated);
        assert_eq!(back.remove_users, p.remove_users);
        assert_eq!(back.rdp_files, p.rdp_files);
        assert_eq!(back.thumbprint, p.thumbprint);
        assert!(back.system);
        assert!(check_plan(&back).is_ok());
        // The main app reads the result from the plan's file: a plan never reads as a result.
        assert!(serde_json::from_str::<SetupResult>(&json).is_err());

        let mut r = SetupResult::default();
        r.push("a", Ok("done".into()));
        r.push("b", Err("no".into()));
        r.thumbprint = Some("AB".into());
        assert!(!r.ok());
        let back: SetupResult = serde_json::from_str(&serde_json::to_string(&r).unwrap()).unwrap();
        assert_eq!(back, r);
    }

    #[test]
    fn plans_from_elsewhere_are_refused() {
        let bad = |f: fn(&mut SetupPlan)| {
            let mut p = plan();
            f(&mut p);
            check_plan(&p).is_err()
        };
        assert!(bad(|p| p.main_user_sid = "S-1-5-21-1)(A;;FA;;;WD".into()));
        // Groups (Everyone, Users, Authenticated Users) would get the ACEs meant for one user.
        assert!(bad(|p| p.main_user_sid = "S-1-1-0".into()));
        assert!(bad(|p| p.main_user_sid = "S-1-5-32-545".into()));
        assert!(bad(|p| p.main_user_sid = "S-1-5-11".into()));
        assert!(bad(|p| p.managed_users.push("Administrator".into())));
        assert!(bad(|p| p.remove_users.push("you".into())));
        assert!(bad(|p| p.remove_users.push("altdock-0".into())));
        assert!(bad(|p| p.users[0].name = "altdock-99".into()));
        assert!(bad(|p| p.users[0].port = 3389));
        assert!(bad(|p| p.users[0].token = Zeroizing::new("xyz".into())));
        assert!(bad(|p| p.users[0].password = Zeroizing::new("short".into())));
        assert!(bad(|p| p.rdp_files.push(PathBuf::from(r"C:\Windows\System32\drivers\etc\hosts"))));
        assert!(bad(|p| p.rdp_files.push(PathBuf::from("relative.rdp"))));
        assert!(bad(|p| p.thumbprint = Some("nothex".into())));
    }

    fn uninstall_plan() -> SetupPlan {
        SetupPlan {
            action: SetupAction::Uninstall,
            main_user_sid: "S-1-5-21-1004336348-1177238915-682003330-1001".into(),
            managed_users: vec!["altdock-1".into()],
            delete_users: true,
            ..Default::default()
        }
    }

    #[test]
    fn uninstall_plans_name_alt_users_only() {
        let p = uninstall_plan();
        assert!(check_plan(&p).is_ok());
        let back: SetupPlan = serde_json::from_str(&serde_json::to_string(&p).unwrap()).unwrap();
        assert_eq!(back.action, SetupAction::Uninstall);
        assert!(back.delete_users);
        assert!(serde_json::to_string(&p).unwrap().contains(r#""action":"uninstall""#));
        assert!(check_plan(&SetupPlan { delete_users: false, managed_users: vec![], ..uninstall_plan() }).is_ok());
        let bad = |f: fn(&mut SetupPlan)| {
            let mut p = uninstall_plan();
            f(&mut p);
            check_plan(&p).is_err()
        };
        assert!(bad(|p| p.managed_users.push("Administrator".into())));
        assert!(bad(|p| p.managed_users.push("altdock-33".into())));
        assert!(bad(|p| p.main_user_sid = "S-1-1-0".into()));
        // An uninstall writes nothing the plan brings: no users, files, certificates or setup.
        assert!(bad(|p| p.users = plan().users));
        assert!(bad(|p| p.remove_users.push("altdock-2".into())));
        assert!(bad(|p| p.rdp_files.push(PathBuf::from(r"C:\d\sessions\Farm.rdp"))));
        assert!(bad(|p| p.thumbprint = Some("A".repeat(40))));
        assert!(bad(|p| p.system = true));
        // Deleting every alt user is for an uninstall only.
        let mut setup = plan();
        setup.delete_users = true;
        assert!(check_plan(&setup).is_err());
        // Plans from before the action existed don't read.
        let mut old: serde_json::Value = serde_json::to_value(plan()).unwrap();
        old.as_object_mut().unwrap().remove("action");
        assert!(serde_json::from_value::<SetupPlan>(old).is_err());
    }

    #[test]
    fn uninstall_finds_the_alt_users() {
        assert_eq!(users_in_json(br#"{"users":["altdock-1","Administrator","altdock-0",7,"altdock-2"]}"#), ["altdock-1", "altdock-2"]);
        assert!(users_in_json(b"not json").is_empty());
        assert!(users_in_json(br#"{"users":"altdock-1"}"#).is_empty());
        let s = |v: &[&str]| v.iter().map(|x| x.to_string()).collect::<Vec<_>>();
        assert_eq!(
            uninstall_users(&s(&["altdock-1", "you"]), &s(&["ALTDOCK-1", "altdock-3"]), &s(&["altdock-2", "altdock-99"])),
            ["altdock-1", "altdock-3", "altdock-2"]
        );
    }

    #[test]
    fn setup_arguments_and_files() {
        let args = |a: &[&str]| a.iter().map(std::ffi::OsString::from).collect::<Vec<_>>();
        assert_eq!(plan_arg(&args(&["altdock.exe", FLAG, r"C:\d\setup-1.json"])), Some(PathBuf::from(r"C:\d\setup-1.json")));
        assert_eq!(plan_arg(&args(&["altdock.exe"])), None);
        assert_eq!(plan_arg(&args(&["altdock.exe", "--session"])), None);
        assert_eq!(plan_arg(&args(&["altdock.exe", FLAG])), None);
        let hex = "0123456789abcdef0123456789abcdef";
        assert!(plan_name_ok(&PathBuf::from(format!(r"C:\d\setup-{hex}.json"))));
        for bad in [
            r"C:\d\setup-1.json".to_owned(),
            format!(r"C:\d\setup-{}.json", hex.to_uppercase()),
            format!(r"C:\d\setup-{hex}.json.txt"),
            format!("setup-{hex}.json"),
            r"C:\Windows\System32\config\SAM".to_owned(),
        ] {
            assert!(!plan_name_ok(Path::new(&bad)), "{bad}");
        }
        let plan = PathBuf::from(format!(r"C:\d\setup-{hex}.json"));
        assert!(rdp_file_ok(Path::new(r"C:\d\sessions\Farm 1.rdp"), &plan));
        assert!(!rdp_file_ok(Path::new(r"C:\d\Farm.rdp"), &plan));
        assert!(!rdp_file_ok(Path::new(r"C:\e\sessions\Farm.rdp"), &plan));
        assert!(!rdp_file_ok(Path::new(r"C:\d\sessions\x\Farm.rdp"), &plan));
        assert!(!rdp_file_ok(Path::new(r"C:\d\sessions\Farm.txt"), &plan));
        assert!(!rdp_file_ok(Path::new(r"C:\d\sessions\.rdp"), &plan));
        assert_eq!(users_json(&["altdock-1".into()]), r#"{"users":["altdock-1"]}"#);
        let c: serde_json::Value = serde_json::from_str(&control_json(47611, "ab", SessionMode::Shared)).unwrap();
        assert_eq!(c, serde_json::json!({ "port": 47611, "token": "ab", "mode": "shared" }));
    }

    #[test]
    fn acls() {
        let main = "S-1-5-21-1-2-3-1001";
        // Only administrators and SYSTEM may write what the HKLM autostart runs for everyone.
        assert_eq!(
            shared_sddl(main, &["S-1-5-21-1-2-3-1002".into()]),
            "O:BAD:PAI(A;OICI;FA;;;BA)(A;OICI;FA;;;SY)(A;OICI;0x1200a9;;;S-1-5-21-1-2-3-1001)(A;OICI;0x1200a9;;;S-1-5-21-1-2-3-1002)"
        );
        for sddl in [shared_sddl(main, &["S-1-5-21-1-2-3-1002".into()]), private_dir_sddl(), control_file_sddl("S-1-5-21-1-2-3-1002"), admin_file_sddl()] {
            assert!(sddl.starts_with("O:BAD:P"), "{sddl}");
            // Full control or modify for no one but BA and SY; no CREATOR OWNER, Users or Everyone.
            for ace in sddl.split('(').skip(1) {
                let trustee = ace.trim_end_matches(')').rsplit(';').next().unwrap();
                let rights = ace.split(';').nth(2).unwrap();
                assert!(["BA", "SY"].contains(&trustee) || rights == "0x1200a9" || rights == "FR", "{ace}");
                assert!(!["CO", "BU", "WD", "AU", "IU"].contains(&trustee), "{ace}");
            }
        }
        assert!(!private_dir_sddl().contains(main));
        // restore.json: nobody but administrators reads (or changes) what gets put back.
        assert_eq!(admin_file_sddl(), "O:BAD:PAI(A;;FA;;;BA)(A;;FA;;;SY)");
        assert!(control_file_sddl("S-1-5-21-1-2-3-1002").ends_with("(A;;FR;;;S-1-5-21-1-2-3-1002)"));
    }
}
