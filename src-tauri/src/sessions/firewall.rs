//! "Remote Desktop only from this PC": the setup turns Remote Desktop on for the whole machine, so
//! it also adds a Windows Firewall rule that blocks inbound TCP 3389 from every address but
//! loopback; the uninstall removes it. Through the firewall's COM API (`INetFwPolicy2`), never a
//! command line. Reading the rules (the health check) needs no rights; changing them, admin rights.
//!
//! Loopback: Microsoft's pages on firewall rules and filter origins don't say that loopback
//! traffic skips Windows Firewall (checked 2026-11; they only document the AppContainer loopback
//! block), so the rule doesn't rely on it. Its remote addresses are every IPv4 and IPv6 address
//! except 127.0.0.0/8 and ::1, as the documented range syntax of
//! `INetFwRule::put_RemoteAddresses` allows ("An IPv4 address range in the format of 'start
//! address - end address'", likewise IPv6:
//! https://learn.microsoft.com/en-us/windows/win32/api/netfw/nf-netfw-inetfwrule-put_remoteaddresses).
//! The sessions connect to 127.0.0.<n>, so they never match it. A block rule wins over Remote
//! Desktop's own allow rules ("Explicit block rules take precedence over any conflicting allow
//! rules": https://learn.microsoft.com/windows/security/operating-system-security/network-security/windows-firewall/rules).

use windows::Win32::Foundation::{VARIANT_FALSE, VARIANT_TRUE};
use windows::Win32::NetworkManagement::WindowsFirewall::{
    INetFwPolicy2, INetFwRule, NET_FW_ACTION_BLOCK, NET_FW_IP_PROTOCOL_TCP, NET_FW_PROFILE_TYPE2, NET_FW_PROFILE2_ALL, NET_FW_RULE_DIR_IN, NetFwPolicy2,
    NetFwRule,
};
use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx, CoUninitialize};
use windows::core::BSTR;

use super::FIREWALL_RULE;

/// Remote Desktop's port.
const PORT: &str = "3389";
/// Every address but loopback (127.0.0.0/8, ::1); `::` is never a sender.
const REMOTE: &str = "0.0.0.0-126.255.255.255,128.0.0.0-255.255.255.255,::2-ffff:ffff:ffff:ffff:ffff:ffff:ffff:ffff";
/// At most this many rules of our name are removed (the firewall allows twins).
const MAX_TWINS: usize = 16;

/// COM for this thread while it lives (whatever mode a caller set up first is fine).
struct Com(bool);

impl Com {
    fn init() -> Self {
        // SAFETY: no arguments that outlive the call; balanced in Drop when it succeeded.
        Com(unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) }.is_ok())
    }
}

impl Drop for Com {
    fn drop(&mut self) {
        if self.0 {
            // SAFETY: CoInitializeEx succeeded on this thread.
            unsafe { CoUninitialize() };
        }
    }
}

fn policy() -> windows::core::Result<INetFwPolicy2> {
    // SAFETY: COM is initialized by the caller (`Com`).
    unsafe { CoCreateInstance(&NetFwPolicy2, None, CLSCTX_INPROC_SERVER) }
}

/// Our rule says what `add` makes it say (the addresses aside, which Windows may write its own way).
fn is_ours(rule: &INetFwRule) -> bool {
    // SAFETY: plain property reads of a live rule.
    unsafe {
        rule.Enabled().is_ok_and(|e| e == VARIANT_TRUE)
            && rule.Action().is_ok_and(|a| a == NET_FW_ACTION_BLOCK)
            && rule.Direction().is_ok_and(|d| d == NET_FW_RULE_DIR_IN)
            && rule.Protocol().is_ok_and(|p| p == NET_FW_IP_PROTOCOL_TCP.0)
            && rule.LocalPorts().is_ok_and(|p| p.to_string().trim() == PORT)
            && rule.RemoteAddresses().is_ok_and(|a| a != "*" && !a.to_string().contains("127.0.0.1"))
    }
}

/// What the health check shows: Some(firewall on for the current networks) when our rule is
/// there, enabled and blocking; None when it isn't.
pub fn status() -> Option<bool> {
    let _com = Com::init();
    let policy = policy().ok()?;
    // SAFETY: property reads of live COM objects.
    unsafe {
        let rule = policy.Rules().ok()?.Item(&BSTR::from(FIREWALL_RULE)).ok()?;
        if !is_ours(&rule) {
            return None;
        }
        let current = policy.CurrentProfileTypes().unwrap_or(0);
        let on = [1, 2, 4]
            .into_iter()
            .filter(|p| current & p != 0)
            .all(|p| policy.get_FirewallEnabled(NET_FW_PROFILE_TYPE2(p)).is_ok_and(|e| e != VARIANT_FALSE));
        Some(on)
    }
}

/// Removes every rule of our name; true when there was one.
fn remove_all(policy: &INetFwPolicy2) -> windows::core::Result<bool> {
    let name = BSTR::from(FIREWALL_RULE);
    // SAFETY: calls on live COM objects with a live BSTR.
    unsafe {
        let rules = policy.Rules()?;
        let mut found = false;
        for _ in 0..MAX_TWINS {
            if rules.Item(&name).is_err() {
                break;
            }
            rules.Remove(&name)?;
            found = true;
        }
        Ok(found)
    }
}

/// Adds the block rule (elevated), in place of any rule of that name.
pub fn add() -> std::result::Result<String, String> {
    let _com = Com::init();
    let made = (|| -> windows::core::Result<()> {
        let policy = policy()?;
        remove_all(&policy)?;
        // SAFETY: COM is initialized; the rule is a fresh object we set up and hand to Add.
        unsafe {
            let rule: INetFwRule = CoCreateInstance(&NetFwRule, None, CLSCTX_INPROC_SERVER)?;
            rule.SetName(&BSTR::from(FIREWALL_RULE))?;
            rule.SetDescription(&BSTR::from("AltDock's background sessions connect over loopback; Remote Desktop from other computers stays blocked until \"Remove from this PC\"."))?;
            rule.SetGrouping(&BSTR::from("AltDock"))?;
            rule.SetDirection(NET_FW_RULE_DIR_IN)?;
            rule.SetProtocol(NET_FW_IP_PROTOCOL_TCP.0)?;
            rule.SetLocalPorts(&BSTR::from(PORT))?;
            rule.SetRemoteAddresses(&BSTR::from(REMOTE))?;
            rule.SetProfiles(NET_FW_PROFILE2_ALL.0)?;
            rule.SetAction(NET_FW_ACTION_BLOCK)?;
            rule.SetEnabled(VARIANT_TRUE)?;
            policy.Rules()?.Add(&rule)?;
        }
        Ok(())
    })();
    made.map(|()| FIREWALL_RULE.to_owned()).map_err(|e| e.message())
}

/// Removes the block rule (elevated); none there is fine.
pub fn remove() -> std::result::Result<String, String> {
    let _com = Com::init();
    match policy().and_then(|p| remove_all(&p)) {
        Ok(true) => Ok(FIREWALL_RULE.to_owned()),
        Ok(false) => Ok(String::new()),
        Err(e) => Err(e.message()),
    }
}
