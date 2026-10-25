//! Roblox clients RoALT started: the multi-instance lock, process tracking, log watching
//! and closing. Nothing here reads or writes another process's memory or handles.

pub mod logwatch;
pub mod multi;
pub mod process;
mod tracker;

pub use tracker::{Tracker, WRONG_ACCOUNT};
