//! Roblox clients RoALT started: the multi-instance lock, process tracking, log watching
//! and closing. Nothing here reads or writes another process's memory or handles; anti-AFK
//! (`afk`) presses a key the way a keyboard does.

pub mod afk;
pub mod limits;
pub mod logwatch;
pub mod metrics;
pub mod multi;
pub mod process;
pub mod qos;
mod tracker;

pub mod rejoin;
pub use tracker::{CHALLENGED, HUNG, Tracker, Transition, UNEXPECTED, WRONG_ACCOUNT};
