//! Per-client limits, set through Windows rather than in the client: a job object with a hard
//! CPU cap, efficiency mode (EcoQoS with below-normal priority), CPU affinity and a periodic
//! working-set trim (`K32EmptyWorkingSet`: Windows pages the client's memory out, it pages back
//! in as used; nothing is read or written). `plan` adds the settings: the adaptive manager's
//! background limits, the default trim and auto-spread affinity. Roblox has no per-client
//! frame cap RoALT could set without touching the client; a CPU cap is what lowers its load
//! (its frame rate drops with it). Process handles stay short-lived and are checked against
//! the start time as in `process`; only the job handle is kept, so its cap can change later.
//! A job outlives RoALT while its client runs, so a cap set stays until that client closes.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use windows::Win32::Foundation::HANDLE;
use windows::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_CPU_RATE_CONTROL, JOB_OBJECT_CPU_RATE_CONTROL_ENABLE,
    JOB_OBJECT_CPU_RATE_CONTROL_HARD_CAP, JOBOBJECT_CPU_RATE_CONTROL_INFORMATION, JOBOBJECT_CPU_RATE_CONTROL_INFORMATION_0,
    JobObjectCpuRateControlInformation, SetInformationJobObject,
};
use windows::Win32::System::ProcessStatus::K32EmptyWorkingSet;
use windows::Win32::System::Threading::{
    BELOW_NORMAL_PRIORITY_CLASS, GetProcessAffinityMask, SetProcessAffinityMask, NORMAL_PRIORITY_CLASS, PROCESS_ACCESS_RIGHTS, PROCESS_POWER_THROTTLING_CURRENT_VERSION,
    PROCESS_POWER_THROTTLING_EXECUTION_SPEED, PROCESS_POWER_THROTTLING_STATE, PROCESS_QUERY_LIMITED_INFORMATION,
    PROCESS_SET_INFORMATION, PROCESS_SET_QUOTA, PROCESS_SYNCHRONIZE, PROCESS_TERMINATE, ProcessPowerThrottling,
    SetPriorityClass, SetProcessInformation,
};
use windows::core::Owned;

use super::process::{open, started};
use crate::error::{AppError, Result};
use crate::model::{Limits, Settings};

/// Our own job object; never a handle to the client.
struct Job(Owned<HANDLE>);
// SAFETY: a job handle may be used from any thread; it is only used under the Limiter's lock.
unsafe impl Send for Job {}

/// What has been applied to one instance's current process.
struct Applied {
    pid: u32,
    job: Option<Job>,
    limits: Limits,
    /// When it was last trimmed (or its trim interval set).
    trimmed_at: Instant,
    /// Limits that failed to apply to this pid: not retried every poll.
    failed: Option<Limits>,
}

impl Applied {
    fn new(pid: u32) -> Applied {
        Applied { pid, job: None, limits: Limits::default(), trimmed_at: Instant::now(), failed: None }
    }
}

pub struct Limiter {
    applied: Mutex<HashMap<String, Applied>>,
}

impl Limiter {
    pub fn new() -> Limiter {
        Limiter { applied: Mutex::new(HashMap::new()) }
    }

    /// Brings the client `pid` (started at `t0`) of instance `id` to `limits`, changing only
    /// what differs from what was applied to it before.
    pub fn apply(&self, id: &str, pid: u32, t0: u64, limits: Limits) -> Result<()> {
        let mut map = self.applied.lock().unwrap_or_else(|e| e.into_inner());
        let a = map.entry(id.to_owned()).or_insert_with(|| Applied::new(pid));
        if a.pid != pid {
            *a = Applied::new(pid);
        }
        if limits.cpu_cap != a.limits.cpu_cap {
            match &a.job {
                Some(job) => set_cap(job, limits.cpu_cap)?,
                None => {
                    let job = new_job()?;
                    set_cap(&job, limits.cpu_cap)?;
                    let h = client(pid, t0, PROCESS_SET_QUOTA | PROCESS_TERMINATE)?;
                    // SAFETY: both handles are live; the process handle closes on drop.
                    unsafe { AssignProcessToJobObject(*job.0, *h) }.map_err(|e| failed(pid, e))?;
                    a.job = Some(job);
                }
            }
            a.limits.cpu_cap = limits.cpu_cap;
        }
        if limits.eco != a.limits.eco {
            set_eco(pid, t0, limits.eco)?;
            a.limits.eco = limits.eco;
        }
        if limits.affinity != a.limits.affinity {
            set_affinity(pid, t0, limits.affinity)?;
            a.limits.affinity = limits.affinity;
        }
        if limits.trim_every_min != a.limits.trim_every_min {
            a.limits.trim_every_min = limits.trim_every_min;
            a.trimmed_at = Instant::now();
        }
        if let Some(m) = a.limits.trim_every_min.filter(|&m| m > 0)
            && a.trimmed_at.elapsed() >= Duration::from_secs(u64::from(m) * 60)
        {
            a.trimmed_at = Instant::now();
            trim(pid, t0)?;
        }
        Ok(())
    }

    /// After each tracker poll, with `plan`'s limits: brings every running client to them
    /// (only what changed, plus trims that are due) and forgets instances no longer running.
    /// Limits that failed are not tried again on the same process until they change.
    pub fn sync(&self, clients: &[(String, u32, u64, Limits)]) {
        self.applied.lock().unwrap_or_else(|e| e.into_inner()).retain(|id, _| clients.iter().any(|c| c.0 == *id));
        for (id, pid, t0, limits) in clients {
            let failed = {
                let map = self.applied.lock().unwrap_or_else(|e| e.into_inner());
                map.get(id).is_some_and(|a| a.pid == *pid && a.failed == Some(*limits))
            };
            if failed {
                continue;
            }
            let r = self.apply(id, *pid, *t0, *limits);
            if let Err(e) = &r {
                log::warn!("could not apply limits to instance {id}: {e}");
            }
            if let Some(a) = self.applied.lock().unwrap_or_else(|e| e.into_inner()).get_mut(id) {
                a.failed = r.is_err().then_some(*limits);
            }
        }
    }
}

/// The limits each running client gets (`clients` as `Tracker::clients`, with the limits the
/// user set): `effective`, with `foreground` the pid owning the window in front and each
/// client's auto-spread share of `cores`.
pub fn plan(clients: &[(String, u32, u64, Limits)], v: &Settings, foreground: Option<u32>, cores: u32) -> Vec<(String, u32, u64, Limits)> {
    let n = clients.len() as u32;
    clients
        .iter()
        .enumerate()
        .map(|(i, (id, pid, t0, manual))| {
            let share = (v.auto_spread && n > 1).then(|| spread(i as u32, n, cores));
            (id.clone(), *pid, *t0, effective(*manual, foreground == Some(*pid), v, share))
        })
        .collect()
}

/// One client's limits (pure): its own when the user set any (manual limits win), else the
/// adaptive manager's background limits while its window isn't in front; then the default
/// trim and the auto-spread `share` where it says nothing of its own.
pub fn effective(manual: Limits, foreground: bool, v: &Settings, share: Option<u64>) -> Limits {
    let mut l = if manual == Limits::default() && v.adaptive.enabled && !foreground { v.adaptive.background } else { manual };
    if l.trim_every_min.is_none() && v.trim_every_min > 0 {
        l.trim_every_min = Some(v.trim_every_min);
    }
    if l.affinity.is_none() {
        l.affinity = share;
    }
    l
}

/// Client `i` of `n`'s cores out of `cores` (at most 64), round-robin (pure): with at least as
/// many cores as clients each gets every n-th core; with more clients, one core each.
pub fn spread(i: u32, n: u32, cores: u32) -> u64 {
    let cores = cores.clamp(1, 64);
    if n >= cores {
        return 1 << (i % cores);
    }
    (0..cores).filter(|c| c % n == i % n).fold(0, |m, c| m | 1 << c)
}

/// The mask actually set: the cores asked for that exist (`system`); None when none do (pure).
pub fn clamp_affinity(mask: u64, system: u64) -> Option<u64> {
    Some(mask & system).filter(|&m| m != 0)
}

/// Pages the client's working set out now (`K32EmptyWorkingSet`, needs PROCESS_SET_QUOTA plus PROCESS_QUERY_LIMITED_INFORMATION, which `client` adds).
pub fn trim(pid: u32, t0: u64) -> Result<()> {
    let h = client(pid, t0, PROCESS_SET_QUOTA)?;
    // SAFETY: `h` is live with PROCESS_SET_QUOTA | PROCESS_QUERY_LIMITED_INFORMATION.
    unsafe { K32EmptyWorkingSet(*h) }
        .ok()
        .map_err(|e| AppError::Internal(format!("Could not trim Roblox (process {pid}): {}", e.message())))
}

/// Sets the cores the client may use; None: all of them (the system's mask). A mask naming no
/// existing core is refused.
fn set_affinity(pid: u32, t0: u64, mask: Option<u64>) -> Result<()> {
    let h = client(pid, t0, PROCESS_SET_INFORMATION)?;
    let (mut own, mut system) = (0usize, 0usize);
    // SAFETY: `h` is live with PROCESS_SET_INFORMATION | PROCESS_QUERY_LIMITED_INFORMATION; the
    // out pointers are locals.
    unsafe { GetProcessAffinityMask(*h, &mut own, &mut system) }.map_err(|e| failed(pid, e))?;
    let mask = match mask {
        Some(m) => clamp_affinity(m, system as u64).ok_or_else(|| AppError::Invalid("None of those CPU cores exist here.".into()))?,
        None => system as u64,
    };
    // SAFETY: as above; `mask` is a subset of the system mask.
    unsafe { SetProcessAffinityMask(*h, mask as usize) }.map_err(|e| failed(pid, e))
}

fn new_job() -> Result<Job> {
    // SAFETY: an unnamed job with default security; Owned closes it on drop.
    unsafe { CreateJobObjectW(None, None) }.map(|h| Job(unsafe { Owned::new(h) })).map_err(|e| AppError::Internal(e.message()))
}

/// None turns the cap off (the job stays, with no limit).
fn set_cap(job: &Job, cap: Option<u8>) -> Result<()> {
    let info = JOBOBJECT_CPU_RATE_CONTROL_INFORMATION {
        ControlFlags: match cap {
            Some(_) => JOB_OBJECT_CPU_RATE_CONTROL_ENABLE | JOB_OBJECT_CPU_RATE_CONTROL_HARD_CAP,
            None => JOB_OBJECT_CPU_RATE_CONTROL(0),
        },
        // In 1/100 of a percent of all processors.
        Anonymous: JOBOBJECT_CPU_RATE_CONTROL_INFORMATION_0 { CpuRate: u32::from(cap.unwrap_or(100)) * 100 },
    };
    // SAFETY: `info` is a local of the size passed.
    unsafe {
        SetInformationJobObject(
            *job.0,
            JobObjectCpuRateControlInformation,
            (&raw const info).cast(),
            size_of::<JOBOBJECT_CPU_RATE_CONTROL_INFORMATION>() as u32,
        )
    }
    .map_err(|e| AppError::Internal(format!("Could not set the CPU limit: {}", e.message())))
}

fn set_eco(pid: u32, t0: u64, on: bool) -> Result<()> {
    let h = client(pid, t0, PROCESS_SET_INFORMATION)?;
    let state = PROCESS_POWER_THROTTLING_STATE {
        Version: PROCESS_POWER_THROTTLING_CURRENT_VERSION,
        ControlMask: PROCESS_POWER_THROTTLING_EXECUTION_SPEED,
        StateMask: if on { PROCESS_POWER_THROTTLING_EXECUTION_SPEED } else { 0 },
    };
    // SAFETY: `h` is live with PROCESS_SET_INFORMATION; `state` is a local of the size passed.
    unsafe {
        SetProcessInformation(*h, ProcessPowerThrottling, (&raw const state).cast(), size_of::<PROCESS_POWER_THROTTLING_STATE>() as u32)
            .map_err(|e| failed(pid, e))?;
        SetPriorityClass(*h, if on { BELOW_NORMAL_PRIORITY_CLASS } else { NORMAL_PRIORITY_CLASS }).map_err(|e| failed(pid, e))
    }
}

/// A short-lived handle to the client, only if `pid` is still the process started at `t0`.
fn client(pid: u32, t0: u64, rights: PROCESS_ACCESS_RIGHTS) -> Result<Owned<HANDLE>> {
    let h = open(pid, rights | PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_SYNCHRONIZE)
        .ok_or_else(|| AppError::Internal(format!("Could not reach Roblox (process {pid}).")))?;
    if started(*h) != Some(t0) {
        return Err(AppError::Invalid("That client has closed.".into()));
    }
    Ok(h)
}

fn failed(pid: u32, e: windows::core::Error) -> AppError {
    AppError::Internal(format!("Could not limit Roblox (process {pid}): {}", e.message()))
}

#[cfg(test)]
mod tests {
    use std::os::windows::process::CommandExt;
    use std::process::{Command, Stdio};

    use super::*;
    use crate::instances::process::start_time;

    #[test]
    fn caps_and_frees_a_process() {
        let root = std::env::var_os("SystemRoot").unwrap_or_else(|| r"C:\Windows".into());
        let mut child = Command::new(std::path::Path::new(&root).join(r"System32\PING.EXE"))
            .args(["-n", "30", "127.0.0.1"])
            .stdout(Stdio::null())
            .creation_flags(0x0800_0000)
            .spawn()
            .unwrap();
        let (pid, t0) = (child.id(), start_time(child.id()).unwrap());
        let l = Limiter::new();
        let capped = Limits { cpu_cap: Some(15), eco: true, ..Limits::default() };
        l.apply("i", pid, t0, capped).unwrap();
        l.apply("i", pid, t0, Limits { cpu_cap: Some(50), ..Limits::default() }).unwrap(); // same job, new rate
        // One core, then every core again; trimmed now, and on a schedule from then on.
        l.apply("i", pid, t0, Limits { affinity: Some(1), trim_every_min: Some(5), ..Limits::default() }).unwrap();
        assert_eq!(affinity(pid), 1);
        trim(pid, t0).unwrap();
        assert!(trim(pid, t0 - 10).is_err(), "a reused pid must not be trimmed");
        l.apply("i", pid, t0, Limits::default()).unwrap();
        assert_eq!(affinity(pid), system(), "back to every core");
        if system().count_ones() < 64 {
            assert!(set_affinity(pid, t0, Some(1 << 63)).is_err(), "only cores that don't exist");
        }
        assert!(l.apply("j", pid, t0 - 10, capped).is_err(), "a reused pid must not be limited");
        l.sync(&[]);
        assert!(l.applied.lock().unwrap().is_empty());
        child.kill().unwrap();
        child.wait().unwrap();
    }

    fn system() -> u64 {
        let (mut own, mut system) = (0usize, 0usize);
        // SAFETY: the pseudo handle of this process; the out pointers are locals.
        unsafe { GetProcessAffinityMask(windows::Win32::System::Threading::GetCurrentProcess(), &mut own, &mut system) }.unwrap();
        system as u64
    }

    fn affinity(pid: u32) -> u64 {
        let h = open(pid, PROCESS_QUERY_LIMITED_INFORMATION).unwrap();
        let (mut own, mut system) = (0usize, 0usize);
        // SAFETY: `h` is live with PROCESS_QUERY_LIMITED_INFORMATION; the out pointers are locals.
        unsafe { GetProcessAffinityMask(*h, &mut own, &mut system) }.unwrap();
        own as u64
    }

    #[test]
    fn affinity_masks_are_clamped() {
        assert_eq!(clamp_affinity(0b1111, 0b0011), Some(0b0011));
        assert_eq!(clamp_affinity(0b1100, 0b0011), None, "no such core");
        assert_eq!(clamp_affinity(0, u64::MAX), None);
        assert_eq!(clamp_affinity(u64::MAX, u64::MAX), Some(u64::MAX));
    }

    #[test]
    fn auto_spread_shares_cores_round_robin() {
        assert_eq!(spread(0, 2, 8), 0b0101_0101);
        assert_eq!(spread(1, 2, 8), 0b1010_1010);
        assert_eq!(spread(2, 3, 4), 0b0100);
        assert_eq!(spread(0, 3, 4), 0b1001);
        // More clients than cores: one core each, wrapping.
        assert_eq!((spread(0, 6, 4), spread(4, 6, 4), spread(5, 6, 4)), (1, 1, 2));
        assert_eq!(spread(63, 100, 200), 1 << 63, "at most 64 cores");
        // Every core is used, none twice.
        let all: Vec<u64> = (0..3).map(|i| spread(i, 3, 12)).collect();
        assert_eq!(all.iter().fold(0, |m, s| m | s), 0xFFF);
        assert_eq!(all.iter().map(|s| s.count_ones()).sum::<u32>(), 12);
    }

    #[test]
    fn adaptive_limits_background_clients_and_manual_ones_win() {
        let bg = Limits { cpu_cap: Some(25), eco: true, ..Limits::default() };
        let mut v = Settings { adaptive: crate::model::Adaptive { enabled: true, background: bg }, ..Settings::default() };
        let none = Limits::default();
        let mine = Limits { cpu_cap: Some(50), ..Limits::default() };
        assert_eq!(effective(none, false, &v, None), bg, "in the background");
        assert_eq!(effective(none, true, &v, None), none, "in front: lifted");
        assert_eq!(effective(mine, false, &v, None), mine, "manual limits win");
        v.adaptive.enabled = false;
        assert_eq!(effective(none, false, &v, None), none, "off");

        // Defaults fill what the client's own limits leave open.
        v.trim_every_min = 15;
        let filled = effective(none, false, &v, Some(0b11));
        assert_eq!((filled.trim_every_min, filled.affinity), (Some(15), Some(0b11)));
        let own = Limits { trim_every_min: Some(0), affinity: Some(0b100), ..Limits::default() };
        assert_eq!(effective(own, false, &v, Some(0b11)), own, "its own trim (off) and cores");

        // plan: the foreground pid is lifted; auto-spread only with several clients.
        v.adaptive.enabled = true;
        v.auto_spread = true;
        let c = |id: &str, pid| (id.to_owned(), pid, 1, none);
        let p = plan(&[c("a", 10), c("b", 20)], &v, Some(20), 4);
        assert_eq!(p[0].3, Limits { trim_every_min: Some(15), affinity: Some(0b0101), ..bg });
        assert_eq!(p[1].3, Limits { trim_every_min: Some(15), affinity: Some(0b1010), ..none });
        assert_eq!(plan(&[c("a", 10)], &v, None, 4)[0].3.affinity, None);
    }
}
