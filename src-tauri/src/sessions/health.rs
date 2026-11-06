//! Whether this PC can run background sessions (`sessions_health`). RDP Wrapper is checked first
//! and hardest: one whose `rdpwrap.ini` doesn't know this exact `termsrv.dll` build can take over
//! the user's own session, so `session_start` refuses then (`wrapper_ready`).

use std::path::{Path, PathBuf};

/// `a.b.c.d` from `VS_FIXEDFILEINFO`'s `dwFileVersionMS`/`LS` (pure): the numbers rdpwrap.ini's
/// sections are named after, not the version string (which may say something else).
///
/// Windows hands a program without a Windows 10 `supportedOS` manifest entry 6.2 as the major and
/// minor version of its own system files (measured: termsrv.dll 10.0.26100.6899 read as
/// 6.2.26100.6899). Every build from 10240 on is Windows 10 or 11, i.e. 10.0, so that is restored.
pub fn format_version(ms: u32, ls: u32) -> String {
    let (major, minor, build) = (ms >> 16, ms & 0xFFFF, ls >> 16);
    let (major, minor) = if build >= 10240 && major < 10 { (10, 0) } else { (major, minor) };
    format!("{major}.{minor}.{build}.{}", ls & 0xFFFF)
}

/// TermService's `ServiceDll` is RDP Wrapper's (pure).
pub fn is_rdpwrap_dll(service_dll: &str) -> bool {
    service_dll.trim().trim_matches('"').rsplit(['\\', '/']).next().is_some_and(|f| f.eq_ignore_ascii_case("rdpwrap.dll"))
}

/// `%NAME%` parts of a registry path filled from `lookup` (pure); unknown ones stay as written.
pub fn expand_env(s: &str, lookup: impl Fn(&str) -> Option<String>) -> String {
    let mut out = String::new();
    let mut rest = s;
    while let Some(start) = rest.find('%') {
        let Some(len) = rest[start + 1..].find('%') else { break };
        let name = &rest[start + 1..start + 1 + len];
        out.push_str(&rest[..start]);
        match lookup(name).filter(|_| !name.is_empty()) {
            Some(v) => out.push_str(&v),
            None => out.push_str(&rest[start..start + len + 2]),
        }
        rest = &rest[start + len + 2..];
    }
    out.push_str(rest);
    out
}

/// Where rdpwrap.ini may be (pure): next to rdpwrap.dll, then RDP Wrapper's default folder.
pub fn ini_paths(dll: Option<&Path>, program_files: Option<&Path>) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = dll.and_then(Path::parent).map(|d| d.join("rdpwrap.ini")).into_iter().collect();
    if let Some(pf) = program_files {
        let default = pf.join("RDP Wrapper").join("rdpwrap.ini");
        if !out.contains(&default) {
            out.push(default);
        }
    }
    out
}

/// rdpwrap.ini has a `[<version>]` section for exactly this termsrv.dll build (pure). Sections
/// like `[10.0.26100.7171-SLInit]` don't count on their own.
pub fn ini_has_version(ini: &str, version: &str) -> bool {
    !version.is_empty()
        && ini.lines().any(|l| {
            let l = l.trim();
            l.strip_prefix('[').and_then(|l| l.strip_suffix(']')).is_some_and(|s| s.trim() == version)
        })
}

/// A setting a policy may override: the policy's value when set, else the system's, else
/// Windows' default (pure).
pub fn effective(policy: Option<u32>, system: Option<u32>, default: u32) -> u32 {
    policy.or(system).unwrap_or(default)
}

/// `thumbprint` is in a `TrustedCertThumbprints` list (pure; comma separated, any case/spaces).
pub fn thumbprint_listed(list: &str, thumbprint: &str) -> bool {
    let want = normalize_thumbprint(thumbprint);
    !want.is_empty() && list.split(',').any(|t| normalize_thumbprint(t) == want)
}

pub use super::restore::normalize_thumbprint;

/// The list with `thumbprint` added (pure); the ones there stay, in their order.
pub fn with_thumbprint(list: &str, thumbprint: &str) -> String {
    if thumbprint_listed(list, thumbprint) {
        return list.to_owned();
    }
    let mut parts: Vec<String> = list.split(',').map(|t| t.trim().to_owned()).filter(|t| !t.is_empty()).collect();
    parts.push(normalize_thumbprint(thumbprint));
    parts.join(",")
}

#[cfg(windows)]
pub use system::*;

#[cfg(windows)]
mod system {
    use std::net::{Ipv4Addr, SocketAddr, TcpStream};
    use std::path::{Path, PathBuf};
    use std::time::Duration;

    use windows::Win32::Storage::FileSystem::{FILE_VER_GET_NEUTRAL, GetFileVersionInfoExW, GetFileVersionInfoSizeExW, VS_FIXEDFILEINFO, VerQueryValueW};
    use windows::core::{HSTRING, w};

    use super::*;
    use crate::sessions::reg::{self, HKCU, HKLM};
    use crate::sessions::types::{FixKind, HealthCheck, SessionConfig};
    use crate::sessions::{FIREWALL_RULE, RUN_VALUE, app_exe, creds, firewall, rdp, run_command, users};

    /// A file's numeric version (`a.b.c.d`), from the version resource of the file itself
    /// (`FILE_VER_GET_NEUTRAL`): without it Windows reads the language file (`termsrv.dll.mui`),
    /// whose version lags the DLL's (measured: 6899 read where RDP Wrapper sees 9549).
    pub fn file_version(path: &Path) -> Option<String> {
        let name = HSTRING::from(path.as_os_str());
        let mut handle = 0u32;
        // SAFETY: `name` is a live NUL-terminated path; `handle` outlives the call.
        let size = unsafe { GetFileVersionInfoSizeExW(FILE_VER_GET_NEUTRAL, &name, &mut handle) };
        if size == 0 {
            return None;
        }
        let mut data = vec![0u8; size as usize];
        // SAFETY: `data` has the `size` bytes the call may write.
        unsafe { GetFileVersionInfoExW(FILE_VER_GET_NEUTRAL, &name, None, size, data.as_mut_ptr().cast()) }.ok()?;
        let mut info: *mut core::ffi::c_void = std::ptr::null_mut();
        let mut len = 0u32;
        // SAFETY: `data` holds the version resource; `info` ends up pointing into it.
        if !unsafe { VerQueryValueW(data.as_ptr().cast(), w!("\\"), &mut info, &mut len) }.as_bool()
            || info.is_null()
            || (len as usize) < size_of::<VS_FIXEDFILEINFO>()
        {
            return None;
        }
        // SAFETY: VerQueryValueW("\") points at a VS_FIXEDFILEINFO inside `data`, still alive.
        let fixed = unsafe { std::ptr::read_unaligned(info as *const VS_FIXEDFILEINFO) };
        (fixed.dwSignature == 0xFEEF04BD).then(|| format_version(fixed.dwFileVersionMS, fixed.dwFileVersionLS))
    }

    fn env(name: &str) -> Option<String> {
        std::env::var(name).ok()
    }

    /// What is known about RDP Wrapper on this PC.
    pub struct Wrapper {
        /// rdpwrap.dll, when TermService loads it.
        pub dll: Option<PathBuf>,
        /// The rdpwrap.ini found.
        pub ini: Option<PathBuf>,
        /// termsrv.dll's numeric version.
        pub termsrv: Option<String>,
        /// The ini has a section for that version.
        pub supported: bool,
    }

    pub fn wrapper() -> Wrapper {
        let dll = reg::string(HKLM, reg::TERMSERVICE, "ServiceDll")
            .map(|d| expand_env(&d, env))
            .filter(|d| is_rdpwrap_dll(d))
            .map(|d| PathBuf::from(d.trim().trim_matches('"')));
        let root = std::env::var_os("SystemRoot").map(PathBuf::from).unwrap_or_else(|| r"C:\Windows".into());
        let termsrv = file_version(&root.join(r"System32\termsrv.dll"));
        let pf = std::env::var_os("ProgramFiles").map(PathBuf::from);
        let ini = ini_paths(dll.as_deref(), pf.as_deref()).into_iter().find(|p| p.is_file());
        let supported = match (&ini, &termsrv) {
            (Some(ini), Some(v)) => std::fs::read(ini).is_ok_and(|b| ini_has_version(&rdp::decode(&b), v)),
            _ => false,
        };
        Wrapper { dll: dll.filter(|d| d.is_file()), ini, termsrv, supported }
    }

    /// Err (a message for the user) unless RDP Wrapper is installed and knows this termsrv.dll.
    pub fn wrapper_ready() -> std::result::Result<(), String> {
        let w = wrapper();
        if w.dll.is_none() {
            return Err(tr!("RDP Wrapper isn't installed. Install it first (see the Sessions page)."));
        }
        if !w.supported {
            return Err(tr!(
                "RDP Wrapper's rdpwrap.ini doesn't support termsrv.dll {version}. Starting a session now could take over your own session; update rdpwrap.ini first.",
                version = w.termsrv.as_deref().unwrap_or("?")
            ));
        }
        Ok(())
    }

    /// Something listens for RDP on this PC.
    pub fn listening() -> bool {
        TcpStream::connect_timeout(&SocketAddr::from((Ipv4Addr::LOCALHOST, 3389)), Duration::from_millis(500)).is_ok()
    }

    fn check(id: &str, ok: bool, title: String, detail: String, fix: FixKind) -> HealthCheck {
        HealthCheck { id: id.into(), ok, title, detail, fix: (!ok).then_some(fix) }
    }

    /// Same version and size: the copy is this build.
    fn same_program(copy: &Path, own: &Path) -> bool {
        let len = |p: &Path| std::fs::metadata(p).map(|m| m.len()).ok();
        len(copy).is_some() && len(copy) == len(own) && file_version(copy) == file_version(own)
    }

    /// Every check of the health panel, in order. `thumbprint`: the signing certificate's SHA-1
    /// thumbprint the last setup reported.
    pub fn checks(sessions: &[SessionConfig], data_dir: &Path, thumbprint: Option<&str>) -> Vec<HealthCheck> {
        let mut out = Vec::new();
        let w = wrapper();
        out.push(check(
            "rdpWrapper",
            w.dll.is_some(),
            tr!("RDP Wrapper installed"),
            match &w.dll {
                Some(d) => d.display().to_string(),
                None => tr!("Remote Desktop doesn't load rdpwrap.dll. Install RDP Wrapper by its instructions; AltDock never installs it."),
            },
            FixKind::RdpWrapper,
        ));
        let version = w.termsrv.clone().unwrap_or_else(|| "?".into());
        out.push(check(
            "rdpWrapperIni",
            w.supported,
            tr!("rdpwrap.ini supports this Windows"),
            match (&w.ini, w.supported) {
                (_, true) => tr!("termsrv.dll {version} is listed.", version = version),
                (Some(ini), false) => tr!("{ini} has no section for termsrv.dll {version}.", ini = ini.display(), version = version),
                (None, false) => tr!("No rdpwrap.ini found."),
            },
            FixKind::RdpWrapperIni,
        ));
        let listening = listening();
        out.push(check(
            "listener",
            listening,
            tr!("Remote Desktop listening"),
            if listening { tr!("Port 3389 answers.") } else { tr!("Nothing answers on port 3389.") },
            if w.supported { FixKind::Restart } else { FixKind::RdpWrapperIni },
        ));
        let deny = effective(reg::dword(HKLM, reg::TS_POLICY, "fDenyTSConnections"), reg::dword(HKLM, reg::TS_CONTROL, "fDenyTSConnections"), 1);
        out.push(check("connections", deny == 0, tr!("Remote connections allowed"), "fDenyTSConnections".into(), FixKind::Setup));
        let blocked = firewall::status();
        out.push(check(
            "remoteRdpBlocked",
            blocked.is_some(),
            tr!("Remote Desktop only from this PC"),
            match blocked {
                Some(true) => tr!("Other computers can't reach Remote Desktop here (firewall rule {rule}).", rule = FIREWALL_RULE),
                Some(false) => tr!("The firewall rule {rule} is there, but Windows Firewall is off for the current network, so it blocks nothing.", rule = FIREWALL_RULE),
                None => tr!("Other computers on the network can reach Remote Desktop here: the firewall rule {rule} is missing.", rule = FIREWALL_RULE),
            },
            FixKind::Setup,
        ));
        let single = effective(reg::dword(HKLM, reg::TS_POLICY, "fSingleSessionPerUser"), reg::dword(HKLM, reg::TS_CONTROL, "fSingleSessionPerUser"), 1);
        out.push(check("singleSession", single == 0, tr!("Several sessions allowed"), "fSingleSessionPerUser".into(), FixKind::Setup));
        let suppress = reg::dword(HKCU, reg::RDP_CLIENT, "RemoteDesktop_SuppressWhenMinimized") == Some(2);
        out.push(check(
            "suppressMinimized",
            suppress,
            tr!("Sessions keep drawing when hidden"),
            "RemoteDesktop_SuppressWhenMinimized".into(),
            FixKind::Setup,
        ));
        let hw = reg::dword(HKLM, reg::TS_POLICY, "bEnumerateHWBeforeSW") == Some(1);
        out.push(check("hardwareGraphics", hw, tr!("Graphics card used in sessions"), "bEnumerateHWBeforeSW".into(), FixKind::Setup));
        let privacy = reg::dword(HKLM, reg::OOBE_POLICY, "DisablePrivacyExperience") == Some(1);
        let animation = reg::dword(HKLM, reg::SYSTEM_POLICY, "EnableFirstLogonAnimation") == Some(0);
        out.push(check(
            "firstLogon",
            privacy && animation,
            tr!("No first sign-in screens"),
            "DisablePrivacyExperience, EnableFirstLogonAnimation".into(),
            FixKind::Setup,
        ));
        let copy = app_exe();
        let own = std::env::current_exe().ok();
        let current = own.as_deref().is_some_and(|own| same_program(&copy, own));
        out.push(check(
            "programCopy",
            current,
            tr!("Program copy for sessions up to date"),
            if current {
                copy.display().to_string()
            } else if copy.is_file() {
                tr!("{path} is another version.", path = copy.display())
            } else {
                tr!("{path} is missing.", path = copy.display())
            },
            FixKind::Setup,
        ));
        let autostart = reg::string(HKLM, reg::RUN, RUN_VALUE).is_some_and(|v| v.trim().eq_ignore_ascii_case(&run_command()));
        out.push(check("autostart", autostart, tr!("AltDock starts in sessions"), RUN_VALUE.into(), FixKind::Setup));
        let missing: Vec<&str> =
            sessions.iter().map(|s| s.windows_user.as_str()).filter(|u| !users::exists(u) || !users::in_remote_desktop_users(u)).collect();
        out.push(check(
            "altUsers",
            missing.is_empty(),
            tr!("Session users ready"),
            if sessions.is_empty() {
                tr!("No sessions yet.")
            } else if missing.is_empty() {
                sessions.iter().map(|s| s.windows_user.as_str()).collect::<Vec<_>>().join(", ")
            } else {
                tr!("Missing or not in Remote Desktop Users: {users}", users = missing.join(", "))
            },
            FixKind::Setup,
        ));
        let trusted = thumbprint.is_some_and(|t| reg::string(HKLM, reg::TS_POLICY, "TrustedCertThumbprints").is_some_and(|l| thumbprint_listed(&l, t)));
        let unsigned: Vec<&str> = sessions
            .iter()
            .filter(|s| {
                let built = rdp::build(s, &creds::user_name(&s.windows_user));
                !std::fs::read(rdp::path(data_dir, &s.name)).is_ok_and(|b| {
                    let text = rdp::decode(&b);
                    rdp::is_signed(&text) && rdp::same_settings(&text, &built)
                })
            })
            .map(|s| s.name.as_str())
            .collect();
        out.push(check(
            "rdpSigning",
            trusted && unsigned.is_empty(),
            tr!("Session files signed and trusted"),
            if !trusted {
                tr!("The signing certificate isn't trusted yet.")
            } else if !unsigned.is_empty() {
                tr!("Not signed (or changed since): {sessions}", sessions = unsigned.join(", "))
            } else {
                thumbprint.unwrap_or_default().to_owned()
            },
            FixKind::Setup,
        ));
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_from_the_numeric_parts() {
        assert_eq!(format_version(10 << 16, (26100 << 16) | 7171), "10.0.26100.7171");
        assert_eq!(format_version((6 << 16) | 3, 9600 << 16), "6.3.9600.0");
        // The 6.2 Windows reports to programs without a Windows 10 manifest entry.
        assert_eq!(format_version((6 << 16) | 2, (26100 << 16) | 6899), "10.0.26100.6899");
        assert_eq!(format_version(u32::MAX, u32::MAX), "65535.65535.65535.65535");
    }

    #[test]
    fn ini_sections_must_match_exactly() {
        let ini = "; RDP Wrapper\r\n[Main]\r\nUpdated=2026-10-01\r\n\r\n[10.0.26100.7171]\r\nLocalOnlyPatch.x64=1\r\n[10.0.26100.7171-SLInit]\r\nbInitialized.x64=1\r\n  [10.0.22631.4602]  \r\n";
        assert!(ini_has_version(ini, "10.0.26100.7171"));
        assert!(ini_has_version(ini, "10.0.22631.4602"));
        assert!(!ini_has_version(ini, "10.0.26100.717"));
        assert!(!ini_has_version(ini, "10.0.26100.71711"));
        assert!(!ini_has_version(ini, "10.0.26100.7172"));
        assert!(!ini_has_version("[10.0.26100.7172-SLInit]\n", "10.0.26100.7172"), "only the SLInit section");
        assert!(!ini_has_version(ini, ""));
        assert!(!ini_has_version("10.0.26100.7171\n", "10.0.26100.7171"), "not a section");
        // rdpwrap.ini saved as UTF-16 reads the same.
        let utf16 = crate::sessions::rdp::encode(ini);
        assert!(ini_has_version(&crate::sessions::rdp::decode(&utf16), "10.0.26100.7171"));
    }

    #[test]
    fn finds_rdp_wrapper_and_its_ini() {
        assert!(is_rdpwrap_dll(r"C:\Program Files\RDP Wrapper\rdpwrap.dll"));
        assert!(is_rdpwrap_dll(r#""C:\Program Files\RDP Wrapper\RDPWrap.dll" "#));
        assert!(!is_rdpwrap_dll(r"%SystemRoot%\System32\termsrv.dll"));
        assert!(!is_rdpwrap_dll(""));
        let pf = Path::new(r"C:\Program Files");
        let dll = Path::new(r"D:\Tools\RDPWrap\rdpwrap.dll");
        assert_eq!(ini_paths(Some(dll), Some(pf)), vec![PathBuf::from(r"D:\Tools\RDPWrap").join("rdpwrap.ini"), pf.join("RDP Wrapper").join("rdpwrap.ini")]);
        let default = pf.join("RDP Wrapper").join("rdpwrap.dll");
        assert_eq!(ini_paths(Some(&default), Some(pf)).len(), 1);
        assert_eq!(ini_paths(None, None), Vec::<PathBuf>::new());
    }

    #[test]
    fn expands_environment_names() {
        let lookup = |n: &str| match n {
            "ProgramFiles" => Some(r"C:\Program Files".to_owned()),
            "SystemRoot" => Some(r"C:\Windows".to_owned()),
            _ => None,
        };
        assert_eq!(expand_env(r"%ProgramFiles%\RDP Wrapper\rdpwrap.dll", lookup), r"C:\Program Files\RDP Wrapper\rdpwrap.dll");
        assert_eq!(expand_env(r"%SystemRoot%\System32\%Nope%\x", lookup), r"C:\Windows\System32\%Nope%\x");
        assert_eq!(expand_env("100% sure", lookup), "100% sure");
        assert_eq!(expand_env("%%", lookup), "%%");
        assert_eq!(expand_env("plain", lookup), "plain");
    }

    #[test]
    fn policies_override_settings() {
        assert_eq!(effective(Some(0), Some(1), 1), 0);
        assert_eq!(effective(None, Some(0), 1), 0);
        assert_eq!(effective(None, None, 1), 1);
    }

    #[test]
    fn trusted_thumbprints_are_kept_and_added() {
        let ours = "ab12cd34ef56ab12cd34ef56ab12cd34ef56ab12";
        assert!(!thumbprint_listed("", ours));
        assert!(thumbprint_listed("1111, AB12CD34EF56AB12CD34EF56AB12CD34EF56AB12", ours));
        assert!(!thumbprint_listed("1111", ""));
        assert_eq!(with_thumbprint("", ours), ours.to_ascii_uppercase());
        assert_eq!(with_thumbprint("1111, 2222,", ours), format!("1111,2222,{}", ours.to_ascii_uppercase()));
        assert_eq!(with_thumbprint("1111,AB12CD34EF56AB12CD34EF56AB12CD34EF56AB12", ours), "1111,AB12CD34EF56AB12CD34EF56AB12CD34EF56AB12");
        assert_eq!(normalize_thumbprint("ab 12:cd"), "AB12CD");
    }
}
