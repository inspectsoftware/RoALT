//! What each client uses: CPU from `GetProcessTimes` deltas, memory from
//! `K32GetProcessMemoryInfo`, IO from `GetProcessIoCounters`. Each sample opens a short-lived
//! `PROCESS_QUERY_LIMITED_INFORMATION` handle checked against the client's start time; nothing
//! reads the client's memory (no VM_READ).

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use windows::Win32::Foundation::{FILETIME, HANDLE};
use windows::Win32::System::ProcessStatus::{K32GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS, PROCESS_MEMORY_COUNTERS_EX};
use windows::Win32::System::Threading::{
    GetProcessIoCounters, GetProcessTimes, IO_COUNTERS, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SYNCHRONIZE,
};

use super::process::{open, started};
use crate::model::{ClientMetrics, Limits};

/// Samples are reused for this long, so several views polling don't add up.
const CACHE: Duration = Duration::from_secs(2);
const MB: f64 = 1024.0 * 1024.0;

/// One reading of a process.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Reading {
    /// Kernel + user time, 100 ns ticks.
    pub cpu: u64,
    pub working_set: u64,
    pub private: u64,
    pub read: u64,
    pub write: u64,
}

/// The previous reading of each client, for the deltas.
struct Last {
    at: Instant,
    t0: u64,
    r: Reading,
}

pub struct Sampler {
    last: Mutex<HashMap<String, Last>>,
    cache: Mutex<Option<(Instant, Vec<ClientMetrics>)>>,
}

impl Sampler {
    pub fn new() -> Sampler {
        Sampler { last: Mutex::new(HashMap::new()), cache: Mutex::new(None) }
    }

    /// Metrics of the running clients (id, pid, start time, limits as `Tracker::clients`);
    /// a client whose process can't be read is left out.
    pub fn sample(&self, clients: &[(String, u32, u64, Limits)]) -> Vec<ClientMetrics> {
        let mut cache = self.cache.lock().unwrap_or_else(|e| e.into_inner());
        if let Some((at, m)) = cache.as_ref()
            && at.elapsed() < CACHE
            && m.len() == clients.len()
            && m.iter().zip(clients).all(|(m, c)| m.instance_id == c.0 && m.pid == c.1)
        {
            return m.clone();
        }
        let cores = std::thread::available_parallelism().map_or(1, |n| n.get()) as f64;
        let mut last = self.last.lock().unwrap_or_else(|e| e.into_inner());
        last.retain(|id, _| clients.iter().any(|c| c.0 == *id));
        let now = Instant::now();
        let mut out = Vec::new();
        for (id, pid, t0, _) in clients {
            let Some(r) = read(*pid, *t0) else { continue };
            let prev = last.get(id).filter(|l| l.t0 == *t0);
            let secs = prev.map(|l| now.duration_since(l.at).as_secs_f64()).filter(|&s| s > 0.0);
            let rate = |f: fn(&Reading) -> u64| prev.zip(secs).map(|(l, s)| f(&r).saturating_sub(f(&l.r)) as f64 / s);
            out.push(ClientMetrics {
                instance_id: id.clone(),
                pid: *pid,
                // 100 ns ticks per second of one core: 1e7.
                cpu_pct: rate(|r| r.cpu).map(|t| (t / 1e7 / cores * 100.0).min(100.0)),
                working_set_mb: r.working_set as f64 / MB,
                private_mb: r.private as f64 / MB,
                read_bps: rate(|r| r.read),
                write_bps: rate(|r| r.write),
            });
            last.insert(id.clone(), Last { at: now, t0: *t0, r });
        }
        *cache = Some((now, out.clone()));
        out
    }
}

impl Default for Sampler {
    fn default() -> Self {
        Self::new()
    }
}

/// One reading of `pid`, only while it is still the process started at `t0`.
pub fn read(pid: u32, t0: u64) -> Option<Reading> {
    let h = open(pid, PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_SYNCHRONIZE)?;
    if started(*h) != Some(t0) {
        return None;
    }
    let cpu = cpu_ticks(*h)?;
    let mut mem = PROCESS_MEMORY_COUNTERS_EX { cb: size_of::<PROCESS_MEMORY_COUNTERS_EX>() as u32, ..Default::default() };
    let mut io = IO_COUNTERS::default();
    // SAFETY: `h` is live with PROCESS_QUERY_LIMITED_INFORMATION (all these need); `mem` is a
    // local PROCESS_MEMORY_COUNTERS_EX whose `cb` says its size, and starts with the plain
    // PROCESS_MEMORY_COUNTERS; `io` is a local.
    unsafe {
        K32GetProcessMemoryInfo(*h, (&raw mut mem).cast::<PROCESS_MEMORY_COUNTERS>(), mem.cb).ok().ok()?;
        GetProcessIoCounters(*h, &mut io).ok()?;
    }
    Some(Reading {
        cpu,
        working_set: mem.WorkingSetSize as u64,
        private: mem.PrivateUsage as u64,
        read: io.ReadTransferCount,
        write: io.WriteTransferCount,
    })
}

fn cpu_ticks(h: HANDLE) -> Option<u64> {
    let (mut created, mut exited, mut kernel, mut user) =
        (FILETIME::default(), FILETIME::default(), FILETIME::default(), FILETIME::default());
    // SAFETY: `h` is live with PROCESS_QUERY_LIMITED_INFORMATION; the out pointers are locals.
    unsafe { GetProcessTimes(h, &mut created, &mut exited, &mut kernel, &mut user) }.ok()?;
    let ticks = |f: FILETIME| (u64::from(f.dwHighDateTime) << 32) | u64::from(f.dwLowDateTime);
    Some(ticks(kernel) + ticks(user))
}

/// How many 10 s memory samples in a row a client has been over the limit, after one more
/// sample of `private_mb` (pure). 0 when the limit is off.
pub fn mem_strikes(prev: u32, private_mb: u64, limit_mb: u32) -> u32 {
    if limit_mb > 0 && private_mb > u64::from(limit_mb) { prev.saturating_add(1) } else { 0 }
}

/// Samples over the limit in a row that restart a client.
pub const MEM_STRIKES: u32 = 3;

#[cfg(test)]
mod tests {
    use std::os::windows::process::CommandExt;
    use std::process::{Command, Stdio};

    use super::*;
    use crate::instances::process::start_time;

    #[test]
    fn reads_a_running_process() {
        let root = std::env::var_os("SystemRoot").unwrap_or_else(|| r"C:\Windows".into());
        let mut child = Command::new(std::path::Path::new(&root).join(r"System32\PING.EXE"))
            .args(["-n", "30", "127.0.0.1"])
            .stdout(Stdio::null())
            .creation_flags(0x0800_0000)
            .spawn()
            .unwrap();
        let (pid, t0) = (child.id(), start_time(child.id()).unwrap());
        let r = read(pid, t0).unwrap();
        assert!(r.working_set > 0 && r.private > 0, "{r:?}");
        assert_eq!(read(pid, t0 - 10), None, "a reused pid is not the client");

        let s = Sampler::new();
        let clients = [("i".to_owned(), pid, t0, Limits::default())];
        let first = s.sample(&clients);
        assert_eq!((first.len(), first[0].cpu_pct, first[0].read_bps), (1, None, None), "no delta yet");
        assert_eq!(s.sample(&clients), first, "cached for 2 s");
        *s.cache.lock().unwrap() = None;
        std::thread::sleep(Duration::from_millis(50));
        let second = s.sample(&clients);
        assert!(second[0].cpu_pct.is_some_and(|c| (0.0..=100.0).contains(&c)), "{second:?}");
        assert!(second[0].working_set_mb > 0.0);
        child.kill().unwrap();
        child.wait().unwrap();
        *s.cache.lock().unwrap() = None;
        assert!(s.sample(&clients).is_empty(), "gone");
    }

    #[test]
    fn memory_strikes() {
        assert_eq!(mem_strikes(0, 5_000, 4_096), 1);
        assert_eq!(mem_strikes(2, 5_000, 4_096), MEM_STRIKES);
        assert_eq!(mem_strikes(2, 4_096, 4_096), 0, "at the limit is not over it");
        assert_eq!(mem_strikes(2, 99_999, 0), 0, "off");
        assert_eq!(mem_strikes(u32::MAX, 5_000, 1), u32::MAX);
    }
}
