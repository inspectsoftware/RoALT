//! Per-client limits, set through Windows rather than in the client: a job object with a hard
//! CPU cap, and efficiency mode (EcoQoS with below-normal priority). Roblox has no per-client
//! frame cap RoALT could set without touching the client; a CPU cap is what lowers its load
//! (its frame rate drops with it). Process handles stay short-lived and are checked against
//! the start time as in `process`; only the job handle is kept, so its cap can change later.
//! A job outlives RoALT while its client runs, so a cap set stays until that client closes.

use std::collections::HashMap;
use std::sync::Mutex;

use windows::Win32::Foundation::HANDLE;
use windows::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_CPU_RATE_CONTROL, JOB_OBJECT_CPU_RATE_CONTROL_ENABLE,
    JOB_OBJECT_CPU_RATE_CONTROL_HARD_CAP, JOBOBJECT_CPU_RATE_CONTROL_INFORMATION, JOBOBJECT_CPU_RATE_CONTROL_INFORMATION_0,
    JobObjectCpuRateControlInformation, SetInformationJobObject,
};
use windows::Win32::System::Threading::{
    BELOW_NORMAL_PRIORITY_CLASS, NORMAL_PRIORITY_CLASS, PROCESS_ACCESS_RIGHTS, PROCESS_POWER_THROTTLING_CURRENT_VERSION,
    PROCESS_POWER_THROTTLING_EXECUTION_SPEED, PROCESS_POWER_THROTTLING_STATE, PROCESS_QUERY_LIMITED_INFORMATION,
    PROCESS_SET_INFORMATION, PROCESS_SET_QUOTA, PROCESS_SYNCHRONIZE, PROCESS_TERMINATE, ProcessPowerThrottling,
    SetPriorityClass, SetProcessInformation,
};
use windows::core::Owned;

use super::process::{open, started};
use crate::error::{AppError, Result};
use crate::model::Limits;

/// Our own job object; never a handle to the client.
struct Job(Owned<HANDLE>);
// SAFETY: a job handle may be used from any thread; it is only used under the Limiter's lock.
unsafe impl Send for Job {}

/// What has been applied to one instance's current process.
struct Applied {
    pid: u32,
    job: Option<Job>,
    limits: Limits,
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
        let a = map.entry(id.to_owned()).or_insert(Applied { pid, job: None, limits: Limits::default() });
        if a.pid != pid {
            *a = Applied { pid, job: None, limits: Limits::default() };
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
        Ok(())
    }

    /// After a tracker poll: applies the limits again to a client found under a new pid, and
    /// forgets instances that are no longer running.
    pub fn sync(&self, clients: &[(String, u32, u64, Limits)]) {
        self.applied.lock().unwrap_or_else(|e| e.into_inner()).retain(|id, _| clients.iter().any(|c| c.0 == *id));
        for (id, pid, t0, limits) in clients {
            let moved = {
                let map = self.applied.lock().unwrap_or_else(|e| e.into_inner());
                map.get(id).map_or(*limits != Limits::default(), |a| a.pid != *pid)
            };
            if moved && let Err(e) = self.apply(id, *pid, *t0, *limits) {
                log::warn!("could not apply limits to instance {id}: {e}");
            }
        }
    }
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
        let capped = Limits { cpu_cap: Some(15), eco: true };
        l.apply("i", pid, t0, capped).unwrap();
        l.apply("i", pid, t0, Limits { cpu_cap: Some(50), eco: false }).unwrap(); // same job, new rate
        l.apply("i", pid, t0, Limits::default()).unwrap();
        assert!(l.apply("j", pid, t0 - 10, capped).is_err(), "a reused pid must not be limited");
        l.sync(&[]);
        assert!(l.applied.lock().unwrap().is_empty());
        child.kill().unwrap();
        child.wait().unwrap();
    }
}
