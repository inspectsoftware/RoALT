//! The one-prompt setup. The main app writes a `SetupPlan` (with the alt users' passwords and
//! control tokens) to a randomly named file in its own data folder and starts the same exe
//! elevated with `--altdock-setup <plan.json>` (`ShellExecuteExW` "runas", like `instances::qos`).
//! That process deletes the plan as soon as it has read it, does each step (going on past
//! failures where that is safe), writes `<plan>.result.json` next to it and exits before Tauri
//! starts. Passwords never appear on a command line.
//!
//! The plan file is as writable as anything else of the user's, so the elevated side trusts
//! nothing in it it can check: user names must be `altdock-<n>`, ports theirs, tokens hex, files
//! `.rdp` files, and the program it copies is always itself.

use std::path::{Path, PathBuf};

use super::types::{SessionMode, SetupPlan};
use super::{MAX_SESSIONS, PORT_BASE, USER_PREFIX};

pub const FLAG: &str = "--altdock-setup";

/// The plan's path when this process was started for the elevated setup (pure).
pub fn plan_arg(args: &[std::ffi::OsString]) -> Option<PathBuf> {
    match args {
        [_, flag, plan, ..] if flag == FLAG => Some(PathBuf::from(plan)),
        _ => None,
    }
}

/// Where the elevated side writes its result: `<plan>.result.json` (pure).
pub fn result_path(plan: &Path) -> PathBuf {
    let mut p = plan.as_os_str().to_owned();
    p.push(".result.json");
    PathBuf::from(p)
}

fn user_slot(name: &str) -> Option<u32> {
    name.strip_prefix(USER_PREFIX)?.parse().ok().filter(|n| (1..=MAX_SESSIONS).contains(n))
}

/// A SID as `S-1-...` (digits and dashes only, so it can go into an SDDL string).
fn is_sid(s: &str) -> bool {
    s.len() < 200 && s.strip_prefix("S-1-").is_some_and(|r| !r.is_empty() && r.bytes().all(|b| b.is_ascii_digit() || b == b'-'))
}

/// Refuses a plan the main app could not have written (pure).
pub fn check_plan(p: &SetupPlan) -> Result<(), String> {
    if !is_sid(&p.main_user_sid) {
        return Err(format!("bad main user SID {:?}", p.main_user_sid));
    }
    for name in p.managed_users.iter().chain(&p.remove_users).chain(p.users.iter().map(|u| &u.name)) {
        if user_slot(name).is_none() {
            return Err(format!("not an AltDock user: {name:?}"));
        }
    }
    for u in &p.users {
        if user_slot(&u.name).map(|n| PORT_BASE + n as u16) != Some(u.port) {
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

/// Modify: read, write, execute, delete, without changing permissions.
const MODIFY: &str = "0x1301bf";
/// Read and execute.
const READ_EXECUTE: &str = "0x1200a9";

/// `%ProgramData%\AltDock` and its `app` folder (pure): administrators and SYSTEM full, the main
/// user modify, the alt users read and execute; nothing inherited from ProgramData.
pub fn shared_sddl(main: &str, alts: &[String]) -> String {
    let mut s = format!("D:PAI(A;OICI;FA;;;BA)(A;OICI;FA;;;SY)(A;OICI;{MODIFY};;;{main})");
    for a in alts {
        s.push_str(&format!("(A;OICI;{READ_EXECUTE};;;{a})"));
    }
    s
}

/// The `control` folder (pure): no alt user may list it or read another's token.
pub fn control_dir_sddl(main: &str) -> String {
    format!("D:PAI(A;OICI;FA;;;BA)(A;OICI;FA;;;SY)(A;OICI;FA;;;{main})")
}

/// One control config (pure): its alt user may read it, the main user and administrators all.
pub fn control_file_sddl(main: &str, alt: &str) -> String {
    format!("D:PAI(A;;FA;;;BA)(A;;FA;;;SY)(A;;FA;;;{main})(A;;FR;;;{alt})")
}

#[cfg(windows)]
pub use system::*;

#[cfg(windows)]
mod system {
    use std::os::windows::process::CommandExt;
    use std::path::{Path, PathBuf};
    use std::time::Duration;

    use windows::Win32::Foundation::{ERROR_CANCELLED, HLOCAL, LocalFree, WAIT_OBJECT_0};
    use windows::Win32::Security::Authorization::{
        ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1, SE_FILE_OBJECT, SetNamedSecurityInfoW,
    };
    use windows::Win32::Security::Cryptography::*;
    use windows::Win32::Security::{ACL, DACL_SECURITY_INFORMATION, GetSecurityDescriptorDacl, PROTECTED_DACL_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR};
    use windows::Win32::System::SystemInformation::GetSystemTime;
    use windows::Win32::System::Threading::{GetExitCodeProcess, WaitForSingleObject};
    use windows::Win32::UI::Shell::{SEE_MASK_NOASYNC, SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW, ShellExecuteExW};
    use windows::Win32::UI::WindowsAndMessaging::SW_HIDE;
    use windows::core::{BOOL, HSTRING, Owned, PCWSTR, PSTR, PWSTR, w};
    use zeroize::Zeroizing;

    use super::*;
    use crate::error::{AppError, Result};
    use crate::sessions::health::{normalize_thumbprint, with_thumbprint};
    use crate::sessions::reg::{self, HKLM};
    use crate::sessions::types::SetupResult;
    use crate::sessions::{APP_EXE, RUN_VALUE, app_exe, program_data, random, run_command, users};

    /// How long the elevated setup may take once the prompt is answered.
    const TIMEOUT: Duration = Duration::from_secs(300);
    const CERT_SUBJECT: &str = "CN=AltDock RDP signing";

    fn hex(bytes: &[u8]) -> String {
        bytes.iter().map(|b| format!("{b:02X}")).collect()
    }

    // ---------- main app side ----------

    /// Runs `plan` elevated: Windows asks for admin rights first. Declining the prompt is an error;
    /// failed steps are in the result.
    pub fn run(data_dir: &Path, plan: &SetupPlan) -> Result<SetupResult> {
        let path = data_dir.join(format!("setup-{}.json", hex(&random(16)).to_ascii_lowercase()));
        let result = result_path(&path);
        let json = Zeroizing::new(serde_json::to_vec(plan).map_err(|e| AppError::Internal(e.to_string()))?);
        {
            use std::io::Write;
            let mut f = std::fs::File::options().write(true).create_new(true).open(&path)?;
            f.write_all(&json)?;
        }
        let outcome = elevate(&path);
        // The elevated side deletes it at once; this is for when it never ran.
        let _ = std::fs::remove_file(&path);
        outcome?;
        let read = std::fs::read(&result);
        let _ = std::fs::remove_file(&result);
        let bytes = read.map_err(|_| AppError::Internal(tr!("The setup ended without a result.")))?;
        serde_json::from_slice(&bytes).map_err(|e| AppError::Internal(e.to_string()))
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

    /// Called first thing in `main`: Some(exit code) when this process is the elevated setup,
    /// which has then done its work.
    pub fn elevated_entry() -> Option<i32> {
        let args: Vec<std::ffi::OsString> = std::env::args_os().collect();
        let plan_path = plan_arg(&args)?;
        crate::i18n::set("");
        let read = std::fs::read(&plan_path).map(Zeroizing::new);
        // Gone before anything else happens: it holds passwords.
        let _ = std::fs::remove_file(&plan_path);
        let mut result = SetupResult::default();
        let plan = read
            .map_err(|e| e.to_string())
            .and_then(|b| serde_json::from_slice::<SetupPlan>(&b).map_err(|e| e.to_string()))
            .and_then(|p| check_plan(&p).map(|()| p));
        match plan {
            Ok(plan) => execute(&plan, &mut result),
            Err(e) => result.push("plan", Err(e)),
        }
        let written = serde_json::to_vec(&result).ok().is_some_and(|j| std::fs::write(result_path(&plan_path), j).is_ok());
        Some(if written { 0 } else { 1 })
    }

    fn execute(plan: &SetupPlan, r: &mut SetupResult) {
        if plan.system {
            policies(r);
        }
        for u in &plan.users {
            r.push(format!("user:{}", u.name), users::create_or_repair(&u.name, &u.password));
            r.push(format!("remoteDesktopUsers:{}", u.name), users::add_to_remote_desktop_users(&u.name));
            r.push(format!("hide:{}", u.name), reg::set_dword(HKLM, reg::USER_LIST, &u.name, 0).map(|()| String::new()));
        }
        for name in &plan.remove_users {
            let control = program_data().join("control").join(format!("{name}.json"));
            let _ = std::fs::remove_file(control);
            let _ = reg::delete(HKLM, reg::USER_LIST, name);
            r.push(format!("remove:{name}"), users::delete(name));
        }
        let main = plan.main_user_sid.as_str();
        // Alt users that don't exist (a failed create) can't be in an ACL.
        let alts: Vec<String> = plan.managed_users.iter().filter_map(|u| users::sid_of(u)).collect();
        let root = program_data();
        let shared = std::fs::create_dir_all(&root).map_err(|e| e.to_string()).and_then(|()| set_acl(&root, &shared_sddl(main, &alts)));
        r.push("sharedFolder", shared.clone().map(|()| root.display().to_string()));
        r.push(
            "usersJson",
            shared.clone().and_then(|()| std::fs::write(root.join("users.json"), users_json(&plan.managed_users)).map_err(|e| e.to_string())).map(|()| String::new()),
        );
        if shared.is_ok() {
            control_configs(plan, r);
        }
        if !plan.system {
            return;
        }
        r.push("programCopy", copy_program(main, &alts));
        r.push("autostart", reg::set_string(HKLM, reg::RUN, RUN_VALUE, &run_command()).map(|()| run_command()));
        match certificate(plan.thumbprint.as_deref()) {
            Ok((sha1, sha256)) => {
                r.thumbprint = Some(sha1.clone());
                r.push("certificate", Ok(sha1.clone()));
                let list = reg::string(HKLM, reg::TS_POLICY, "TrustedCertThumbprints").unwrap_or_default();
                r.push("trustedThumbprints", reg::set_string(HKLM, reg::TS_POLICY, "TrustedCertThumbprints", &with_thumbprint(&list, &sha1)).map(|()| String::new()));
                for f in &plan.rdp_files {
                    r.push(format!("sign:{}", f.file_stem().unwrap_or_default().to_string_lossy()), sign(f, &sha256));
                }
            }
            Err(e) => r.push("certificate", Err(e)),
        }
    }

    fn policies(r: &mut SetupResult) {
        let set = |values: &[(&str, &str, u32)]| -> std::result::Result<String, String> {
            for (key, value, data) in values {
                reg::set_dword(HKLM, key, value, *data)?;
            }
            Ok(String::new())
        };
        let mut connections = set(&[(reg::TS_CONTROL, "fDenyTSConnections", 0)]);
        // A policy saying no would win over the setting.
        if connections.is_ok() && reg::dword(HKLM, reg::TS_POLICY, "fDenyTSConnections").is_some() {
            connections = set(&[(reg::TS_POLICY, "fDenyTSConnections", 0)]);
        }
        r.push("connections", connections);
        r.push("singleSession", set(&[(reg::TS_POLICY, "fSingleSessionPerUser", 0)]));
        r.push("hardwareGraphics", set(&[(reg::TS_POLICY, "bEnumerateHWBeforeSW", 1)]));
        r.push("firstLogon", set(&[(reg::OOBE_POLICY, "DisablePrivacyExperience", 1), (reg::SYSTEM_POLICY, "EnableFirstLogonAnimation", 0)]));
    }

    /// The control configs, each readable by its own alt user only.
    fn control_configs(plan: &SetupPlan, r: &mut SetupResult) {
        let dir = program_data().join("control");
        let made = std::fs::create_dir_all(&dir).map_err(|e| e.to_string()).and_then(|()| set_acl(&dir, &control_dir_sddl(&plan.main_user_sid)));
        if let Err(e) = made {
            // Without its ACL the folder would let every alt user read every token.
            r.push("controlFolder", Err(e));
            return;
        }
        for u in &plan.users {
            let path = dir.join(format!("{}.json", u.name));
            let outcome = users::sid_of(&u.name)
                .ok_or_else(|| tr!("{user} doesn't exist.", user = u.name))
                .and_then(|alt| {
                    std::fs::write(&path, control_json(u.port, &u.token, u.mode).as_bytes()).map_err(|e| e.to_string())?;
                    set_acl(&path, &control_file_sddl(&plan.main_user_sid, &alt)).inspect_err(|_| {
                        let _ = std::fs::remove_file(&path);
                    })
                })
                .map(|()| path.display().to_string());
            r.push(format!("control:{}", u.name), outcome);
        }
    }

    /// Copies this exe (and DLLs beside it) to `%ProgramData%\AltDock\app`. A running copy (a
    /// session still open) is renamed aside first: Windows allows that, not overwriting it.
    fn copy_program(main: &str, alts: &[String]) -> std::result::Result<String, String> {
        let own = std::env::current_exe().map_err(|e| e.to_string())?;
        let target = app_exe();
        let dir = target.parent().ok_or("no app folder")?.to_owned();
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        set_acl(&dir, &shared_sddl(main, alts))?;
        // Old copies left by earlier runs, now unused.
        for e in std::fs::read_dir(&dir).map_err(|e| e.to_string())?.flatten() {
            if e.file_name().to_string_lossy().contains(".old-") {
                let _ = std::fs::remove_file(e.path());
            }
        }
        let place = |from: &Path, to: &Path| -> std::result::Result<(), String> {
            if to.exists() && std::fs::remove_file(to).is_err() {
                let aside = to.with_file_name(format!("{}.old-{}", to.file_name().unwrap_or_default().to_string_lossy(), hex(&random(4))));
                std::fs::rename(to, aside).map_err(|e| e.to_string())?;
            }
            std::fs::copy(from, to).map(|_| ()).map_err(|e| format!("{}: {e}", to.display()))
        };
        place(&own, &target)?;
        if let Some(src) = own.parent() {
            for e in std::fs::read_dir(src).map_err(|e| e.to_string())?.flatten() {
                let p = e.path();
                if p.extension().is_some_and(|x| x.eq_ignore_ascii_case("dll")) {
                    place(&p, &dir.join(e.file_name()))?;
                }
            }
        }
        debug_assert!(target.ends_with(APP_EXE));
        Ok(target.display().to_string())
    }

    /// Replaces the DACL of `path` with the one in `sddl`, not inheriting from its parent.
    fn set_acl(path: &Path, sddl: &str) -> std::result::Result<(), String> {
        let mut sd = PSECURITY_DESCRIPTOR::default();
        // SAFETY: the descriptor is LocalAlloc'd and freed below.
        unsafe { ConvertStringSecurityDescriptorToSecurityDescriptorW(&HSTRING::from(sddl), SDDL_REVISION_1, &mut sd, None) }
            .map_err(|e| e.message())?;
        let mut present = BOOL::default();
        let mut defaulted = BOOL::default();
        let mut dacl: *mut ACL = std::ptr::null_mut();
        // SAFETY: `sd` is the descriptor just made; `dacl` points into it until it is freed.
        let outcome = unsafe { GetSecurityDescriptorDacl(sd, &mut present, &mut dacl, &mut defaulted) }.map_err(|e| e.message()).and_then(|()| {
            // SAFETY: as above; the path string lives through the call.
            let r = unsafe {
                SetNamedSecurityInfoW(
                    &HSTRING::from(path.as_os_str()),
                    SE_FILE_OBJECT,
                    DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION,
                    None,
                    None,
                    Some(dacl),
                    None,
                )
            };
            if r.is_ok() { Ok(()) } else { Err(format!("{}: {}", path.display(), windows::core::Error::from(r.to_hresult()).message())) }
        });
        // SAFETY: allocated by ConvertStringSecurityDescriptorToSecurityDescriptorW.
        unsafe { LocalFree(Some(HLOCAL(sd.0))) };
        outcome
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
        let my = open_store(w!("MY"))?;
        let cert = match earlier.and_then(|t| find(&my, t)) {
            Some(c) => c,
            None => {
                let c = make_certificate()?;
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

    /// The certificate with that SHA-1 thumbprint, if its private key is still known.
    fn find(store: &Store, thumbprint: &str) -> Option<Cert> {
        let t = normalize_thumbprint(thumbprint);
        let mut bytes: Vec<u8> = (0..t.len() / 2).filter_map(|i| u8::from_str_radix(&t[i * 2..i * 2 + 2], 16).ok()).collect();
        let blob = CRYPT_INTEGER_BLOB { cbData: bytes.len() as u32, pbData: bytes.as_mut_ptr() };
        // SAFETY: `blob` points at `bytes`, alive through the call.
        let found = unsafe { CertFindCertificateInStore(store.0, X509_ASN_ENCODING, 0, CERT_FIND_SHA1_HASH, Some((&raw const blob).cast()), None) };
        if found.is_null() {
            return None;
        }
        let cert = Cert(found);
        property(&cert, CERT_KEY_PROV_INFO_PROP_ID).map(|_| cert)
    }

    /// A self-signed code-signing certificate (RSA 2048, SHA-256, 10 years) with its key in a new
    /// machine key container.
    fn make_certificate() -> std::result::Result<Cert, String> {
        // Subject and issuer.
        let subject = HSTRING::from(CERT_SUBJECT);
        let mut len = 0u32;
        // SAFETY: asks for the size, then fills a buffer of that size.
        unsafe { CertStrToNameW(X509_ASN_ENCODING, &subject, CERT_X500_NAME_STR, None, None, &mut len, None) }.map_err(|e| e.message())?;
        let mut name = vec![0u8; len as usize];
        unsafe { CertStrToNameW(X509_ASN_ENCODING, &subject, CERT_X500_NAME_STR, None, Some(name.as_mut_ptr()), &mut len, None) }.map_err(|e| e.message())?;
        let name_blob = CRYPT_INTEGER_BLOB { cbData: len, pbData: name.as_mut_ptr() };

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

    /// Signs a `.rdp` file in place with `rdpsign.exe /sha256 <thumbprint>`.
    fn sign(file: &Path, sha256: &str) -> std::result::Result<String, String> {
        if !file.is_file() {
            return Err(tr!("{file} is missing.", file = file.display()));
        }
        let root = std::env::var_os("SystemRoot").map(PathBuf::from).unwrap_or_else(|| r"C:\Windows".into());
        let out = std::process::Command::new(root.join(r"System32\rdpsign.exe"))
            .arg("/sha256")
            .arg(sha256)
            .arg(file)
            .creation_flags(0x0800_0000)
            .output()
            .map_err(|e| e.to_string())?;
        if out.status.success() {
            Ok(file.display().to_string())
        } else {
            let text = String::from_utf8_lossy(&out.stdout).trim().to_owned() + " " + String::from_utf8_lossy(&out.stderr).trim();
            Err(format!("rdpsign {:?}: {}", out.status.code(), text.trim()))
        }
    }
}

#[cfg(test)]
mod tests {
    use zeroize::Zeroizing;

    use super::*;
    use crate::sessions::types::{PlanUser, SetupResult};

    fn plan() -> SetupPlan {
        SetupPlan {
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

    #[test]
    fn setup_arguments_and_files() {
        let args = |a: &[&str]| a.iter().map(std::ffi::OsString::from).collect::<Vec<_>>();
        assert_eq!(plan_arg(&args(&["altdock.exe", FLAG, r"C:\d\setup-1.json"])), Some(PathBuf::from(r"C:\d\setup-1.json")));
        assert_eq!(plan_arg(&args(&["altdock.exe"])), None);
        assert_eq!(plan_arg(&args(&["altdock.exe", "--session"])), None);
        assert_eq!(plan_arg(&args(&["altdock.exe", FLAG])), None);
        assert_eq!(result_path(Path::new(r"C:\d\setup-1.json")), PathBuf::from(r"C:\d\setup-1.json.result.json"));
        assert_eq!(users_json(&["altdock-1".into()]), r#"{"users":["altdock-1"]}"#);
        let c: serde_json::Value = serde_json::from_str(&control_json(47611, "ab", SessionMode::Shared)).unwrap();
        assert_eq!(c, serde_json::json!({ "port": 47611, "token": "ab", "mode": "shared" }));
    }

    #[test]
    fn acls() {
        let main = "S-1-5-21-1-2-3-1001";
        assert_eq!(
            shared_sddl(main, &["S-1-5-21-1-2-3-1002".into()]),
            "D:PAI(A;OICI;FA;;;BA)(A;OICI;FA;;;SY)(A;OICI;0x1301bf;;;S-1-5-21-1-2-3-1001)(A;OICI;0x1200a9;;;S-1-5-21-1-2-3-1002)"
        );
        assert!(!control_dir_sddl(main).contains("1002"));
        assert!(control_file_sddl(main, "S-1-5-21-1-2-3-1002").ends_with("(A;;FR;;;S-1-5-21-1-2-3-1002)"));
    }
}
