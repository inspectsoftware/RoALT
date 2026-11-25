//! On-demand JPEG encoding of RoALT's existing `Windows.Graphics.Capture` thumbnails.
//!
//! Only runs while at least one remote browser has an active preview subscription.
//! Zero extra capture work when idle, minimized, or while Windows is locked.

use std::collections::{HashMap, VecDeque};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use jpeg_encoder::{ColorType, Encoder};

use crate::instances::window::{self, Shot};

pub const MAX_FPS: u8 = 30;
pub const FPS_BUDGET: u32 = 90;
/// Pictures per second the PC grabs for all of its viewers together.
pub const PC_FPS_BUDGET: u32 = 120;
const IN_FLIGHT: usize = 3;
const ACK_TIMEOUT: Duration = Duration::from_secs(1);
const MAX_ACK_TIMEOUT: Duration = Duration::from_secs(8);
const MAX_BACKOFF: u32 = 4;
const SENT_KEPT: usize = 16;
const IDLE_GAP: Duration = Duration::from_secs(1);
/// No preview JPEG is bigger: sealed, it stays well inside the relay's frame limit.
pub const MAX_JPEG: usize = 200 * 1024;
const ENCODED_KEPT: usize = 32;
/// Instance ids are UUIDs (36 characters).
pub const MAX_INSTANCE_ID: usize = 64;

/// Pacing of one preview stream to one viewer: at most `IN_FLIGHT` pictures the viewer hasn't
/// acknowledged, so a slow phone gets fewer, fresh frames instead of a growing queue. A picture
/// not acknowledged within twice the measured ack time (`ACK_TIMEOUT` to `MAX_ACK_TIMEOUT`) counts
/// as lost: each loss halves the window and the rate (down to 1/2^`MAX_BACKOFF`), each ack wins
/// one step back. Tries keep their cadence when a timer wakes late (Windows timers tick every
/// 15.6 ms), so 30 a second stays 30, not 21.
#[derive(Default)]
pub struct Flow {
    pub last_seq: u32,
    due: Option<Instant>,
    in_flight: VecDeque<(u32, Instant)>,
    sent_at: VecDeque<(u32, Instant)>,
    rtt: Option<Duration>,
    backoff: u32,
}

impl Flow {
    fn ack_timeout(&self) -> Duration {
        self.rtt.map_or(ACK_TIMEOUT, |rtt| (rtt * 2).clamp(ACK_TIMEOUT, MAX_ACK_TIMEOUT))
    }

    fn window(&self) -> usize {
        (IN_FLIGHT >> self.backoff).max(1)
    }

    fn expire(&mut self, now: Instant) {
        let timeout = self.ack_timeout();
        let before = self.in_flight.len();
        while self.in_flight.front().is_some_and(|&(_, at)| now.duration_since(at) >= timeout) {
            self.in_flight.pop_front();
        }
        if self.in_flight.len() < before {
            self.backoff = (self.backoff + 1).min(MAX_BACKOFF);
        }
    }

    pub fn next_at(&mut self, now: Instant) -> Instant {
        self.expire(now);
        let paced = self.due.unwrap_or(now);
        match self.in_flight.front() {
            Some(&(_, oldest)) if self.in_flight.len() >= self.window() => paced.max(oldest + self.ack_timeout()),
            _ => paced,
        }
    }

    pub fn tried(&mut self, now: Instant, fps: u8) {
        let gap = Duration::from_millis((1000 << self.backoff) / u64::from(fps.max(1)));
        let on_time = self.due.filter(|&due| now.saturating_duration_since(due) < gap).unwrap_or(now);
        self.due = Some(on_time + gap);
    }

    pub fn sent(&mut self, seq: u32, picture: bool, now: Instant) {
        if picture {
            self.last_seq = seq;
            self.in_flight.push_back((seq, now));
            if self.sent_at.len() >= SENT_KEPT {
                self.sent_at.pop_front();
            }
            self.sent_at.push_back((seq, now));
        } else {
            self.due = Some(now + IDLE_GAP);
        }
    }

    pub fn acked(&mut self, seq: u32, now: Instant) {
        let Some(&(_, sent)) = self.sent_at.iter().rfind(|&&(s, _)| s <= seq) else { return };
        let sample = now.saturating_duration_since(sent);
        self.rtt = Some(self.rtt.map_or(sample, |rtt| (rtt * 7 + sample) / 8));
        self.backoff = self.backoff.saturating_sub(1);
        self.in_flight.retain(|&(s, _)| s > seq);
    }
}

/// Clamps a viewer's subscription to what the PC serves: `max_w` 160..=640, `fps` 1..=`MAX_FPS`,
/// and all of it within `FPS_BUDGET`; ids no instance could have are dropped.
pub fn clamp_targets(targets: &mut Vec<super::protocol::RemotePreviewTarget>) {
    targets.retain(|t| t.instance_id.len() <= MAX_INSTANCE_ID);
    targets.truncate(16);
    for t in targets.iter_mut() {
        t.max_w = t.max_w.clamp(160, 640);
        t.fps = t.fps.clamp(1, MAX_FPS);
    }
    let total: u32 = targets.iter().map(|t| u32::from(t.fps)).sum();
    if total > FPS_BUDGET {
        for t in targets.iter_mut() {
            t.fps = u8::try_from((u32::from(t.fps) * FPS_BUDGET / total).max(1)).unwrap_or(1);
        }
    }
}

/// A target's rate once all viewers' targets, `total` pictures a second, share `PC_FPS_BUDGET`.
pub fn shared_fps(fps: u8, total: u32) -> u8 {
    if total <= PC_FPS_BUDGET {
        return fps;
    }
    u8::try_from((u32::from(fps) * PC_FPS_BUDGET / total).max(1)).unwrap_or(1)
}

type Grab = (u32, u32, u8, Option<Vec<u8>>);

/// One batch of preview jobs: each (client, width, quality) is captured and encoded once and
/// handed to every viewer that asked for it.
#[derive(Default)]
pub struct Batch {
    grabbed: HashMap<(Option<u32>, u32, bool), Grab>,
}

impl Batch {
    /// As `grab_jpeg`, sharing the picture with the batch's other viewers of that client.
    pub fn grab(&mut self, pid: Option<u32>, max_w: u32, fps: u8, last_seq: u32) -> Grab {
        let key = (pid, max_w.clamp(160, 640), fps >= 10);
        let (seq, age_ms, status, jpeg) = self.grabbed.entry(key).or_insert_with(|| grab_jpeg(pid, max_w, fps, 0)).clone();
        (seq, age_ms, status, if seq == last_seq { None } else { jpeg })
    }
}

/// Encodes an RGBA top-down pixel buffer to JPEG (`quality` 1..=100).
pub fn encode_rgba_jpeg(width: u32, height: u32, rgba: &[u8], quality: u8) -> Option<Vec<u8>> {
    let w = u16::try_from(width).ok()?;
    let h = u16::try_from(height).ok()?;
    if w == 0 || h == 0 || rgba.len() < (width as usize) * (height as usize) * 4 {
        return None;
    }
    let mut out = Vec::with_capacity(16 * 1024);
    let encoder = Encoder::new(&mut out, quality.clamp(35, 90));
    encoder.encode(rgba, w, h, ColorType::Rgba).ok()?;
    Some(out)
}

fn capped_jpeg(width: u32, height: u32, rgba: &[u8], quality: u8, cap: usize) -> Option<Vec<u8>> {
    std::iter::once(quality).chain([45, 35]).filter(|&q| q <= quality).find_map(|q| encode_rgba_jpeg(width, height, rgba, q).filter(|j| j.len() <= cap))
}

fn jpeg_of(shot: &Shot, quality: u8) -> Option<Vec<u8>> {
    type BySeqAndQuality = VecDeque<((u32, u8), Option<Vec<u8>>)>;
    static ENCODED: Mutex<BySeqAndQuality> = Mutex::new(VecDeque::new());
    let key = (shot.seq, quality);
    let cache = || ENCODED.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    if let Some((_, jpeg)) = cache().iter().find(|(k, _)| *k == key) {
        return jpeg.clone();
    }
    let jpeg = capped_jpeg(shot.thumb.width, shot.thumb.height, &shot.thumb.rgba, quality, MAX_JPEG);
    let mut kept = cache();
    if kept.len() >= ENCODED_KEPT {
        kept.pop_front();
    }
    kept.push_back((key, jpeg.clone()));
    jpeg
}

/// Captures (if needed) and encodes a single client preview.
/// Returns `(seq, age_ms, status_code, Option<jpeg_bytes>)`.
/// `jpeg_bytes` is `None` when the frame sequence equals `last_seq`, no shot is available, or it
/// can't be sent: wider than asked, or over `MAX_JPEG` even at the lowest quality.
pub fn grab_jpeg(pid: Option<u32>, max_w: u32, fps: u8, last_seq: u32) -> (u32, u32, u8, Option<Vec<u8>>) {
    let clamped_w = max_w.clamp(160, 640);
    let preview = window::preview(pid, Some(clamped_w), true);
    let status = preview.status as u8;
    let Some(shot) = preview.shot else {
        return (0, 0, status, None);
    };
    let age_ms = u32::try_from(shot.at.elapsed().as_millis()).unwrap_or(u32::MAX);
    if shot.seq == last_seq || shot.thumb.width > clamped_w {
        return (shot.seq, age_ms, status, None);
    }
    let quality = match (clamped_w >= 480, fps >= 10) {
        (true, false) => 74,
        (false, false) => 64,
        (true, true) => 62,
        (false, true) => 52,
    };
    (shot.seq, age_ms, status, jpeg_of(&shot, quality))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::remote::protocol::RemotePreviewTarget;

    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    #[test]
    fn flow_paces_by_fps_and_stops_at_the_in_flight_window() {
        let t0 = Instant::now();
        let mut f = Flow::default();
        assert_eq!(f.next_at(t0), t0);
        let mut now = t0;
        for seq in 1..=3 {
            assert!(f.next_at(now) <= now);
            f.tried(now, 30);
            f.sent(seq, true, now);
            now += ms(40);
        }
        assert_eq!(f.next_at(now), t0 + ACK_TIMEOUT);
        f.acked(2, now);
        assert!(f.next_at(now) <= now);
    }

    #[test]
    fn flow_keeps_its_cadence_through_late_wakeups() {
        let t0 = Instant::now();
        let mut f = Flow::default();
        let mut now = t0;
        let mut tries = 0;
        while now < t0 + Duration::from_secs(1) {
            if f.next_at(now) <= now {
                f.tried(now, 30);
                f.acked(u32::MAX, now);
                tries += 1;
            }
            now += Duration::from_micros(15_625);
        }
        assert!((29..=31).contains(&tries), "{tries} tries in a second");
    }

    #[test]
    fn flow_forgets_frames_the_relay_dropped() {
        let t0 = Instant::now();
        let mut f = Flow::default();
        f.tried(t0, 30);
        for seq in 1..=3 {
            f.sent(seq, true, t0);
        }
        let later = t0 + ACK_TIMEOUT;
        assert_eq!(f.next_at(later), t0 + ms(33));
    }

    #[test]
    fn flow_backs_off_on_a_link_that_acks_two_seconds_late() {
        let t0 = Instant::now();
        let mut f = Flow::default();
        let mut acks: VecDeque<(Instant, u32)> = VecDeque::new();
        let (mut seq, mut late_pictures) = (0, 0);
        let mut now = t0;
        while now < t0 + Duration::from_secs(40) {
            while acks.front().is_some_and(|&(at, _)| at <= now) {
                let (_, s) = acks.pop_front().unwrap();
                f.acked(s, now);
            }
            if f.next_at(now) <= now {
                f.tried(now, 30);
                seq += 1;
                f.sent(seq, true, now);
                acks.push_back((now + Duration::from_secs(2), seq));
                if now >= t0 + Duration::from_secs(30) {
                    late_pictures += 1;
                }
            }
            now += ms(10);
        }
        assert!(late_pictures < 30, "{late_pictures} pictures in 10 s at 2 s per ack");
    }

    #[test]
    fn flow_slows_to_one_a_second_while_there_is_no_picture() {
        let t0 = Instant::now();
        let mut f = Flow::default();
        f.tried(t0, 30);
        f.sent(0, false, t0);
        assert_eq!(f.next_at(t0), t0 + IDLE_GAP);
        let back = t0 + IDLE_GAP;
        f.tried(back, 30);
        f.sent(5, true, back);
        assert_eq!(f.next_at(back), back + ms(33));
        assert_eq!(f.last_seq, 5);
    }

    #[test]
    fn targets_are_clamped_and_kept_within_the_budget() {
        let target = |fps| RemotePreviewTarget { instance_id: "a".into(), max_w: 9999, fps };
        let mut one = vec![target(200)];
        clamp_targets(&mut one);
        assert_eq!((one[0].max_w, one[0].fps), (640, MAX_FPS));
        let mut many = vec![target(30); 12];
        clamp_targets(&mut many);
        assert!(many.iter().map(|t| u32::from(t.fps)).sum::<u32>() <= FPS_BUDGET);
        assert!(many.iter().all(|t| t.fps >= 1));
        let mut long = vec![RemotePreviewTarget { instance_id: "x".repeat(MAX_INSTANCE_ID + 1), max_w: 320, fps: 5 }, target(5)];
        clamp_targets(&mut long);
        assert_eq!(long.len(), 1);
        assert_eq!(long[0].instance_id, "a");
    }

    fn noise(w: u32, h: u32) -> Vec<u8> {
        let mut x = 0x2545_F491_u32;
        (0..w * h * 4)
            .map(|_| {
                x ^= x << 13;
                x ^= x >> 17;
                x ^= x << 5;
                x as u8
            })
            .collect()
    }

    #[test]
    fn a_jpeg_over_the_cap_is_made_smaller_or_not_sent() {
        let rgba = noise(320, 240);
        let full = encode_rgba_jpeg(320, 240, &rgba, 90).unwrap();
        let capped = capped_jpeg(320, 240, &rgba, 90, full.len() - 1).unwrap();
        assert!(capped.len() < full.len());
        assert_eq!(capped_jpeg(320, 240, &rgba, 90, 1000), None);
    }

    #[test]
    fn viewers_of_one_picture_share_its_encoding() {
        let shot = |rgba: Vec<u8>| Shot { thumb: std::sync::Arc::new(window::Thumb { width: 32, height: 16, rgba }), seq: u32::MAX - 7, at: Instant::now() };
        let first = jpeg_of(&shot([200u8, 10, 10, 255].repeat(32 * 16)), 60).unwrap();
        let again = jpeg_of(&shot([10u8, 200, 10, 255].repeat(32 * 16)), 60).unwrap();
        assert_eq!(first, again, "the same picture number is not encoded twice");
    }

    #[test]
    fn all_viewers_together_stay_within_the_pc_budget() {
        assert_eq!(shared_fps(30, PC_FPS_BUDGET), 30);
        let total = 16 * 90;
        let shared: u32 = (0..16 * 3).map(|_| u32::from(shared_fps(30, total))).sum();
        assert!(shared <= PC_FPS_BUDGET, "{shared} a second");
        assert_eq!(shared_fps(1, 10_000), 1);
    }

    #[test]
    fn encodes_valid_jpeg_from_rgba() {
        let rgba = [32u8, 120, 220, 255].repeat(64 * 48);
        let jpg = encode_rgba_jpeg(64, 48, &rgba, 65).unwrap();
        assert!(jpg.len() > 100);
        // JPEG SOI marker 0xFF 0xD8 and EOI marker 0xFF 0xD9.
        assert_eq!(&jpg[..2], &[0xFF, 0xD8]);
        assert_eq!(&jpg[jpg.len() - 2..], &[0xFF, 0xD9]);
    }
}
