//! Transport-span timing: what a rendered frame costs between the sim thread and the socket, and
//! the injection that puts those numbers into the profile messages before they are serialized.
//!
//! The engine measures its own two layers (the sim schedule and the GPU passes) and ships them as
//! [`ProfileWindowData`](ludion_protocol::perf::ProfileWindowData) with the `transport` section
//! left empty — it cannot fill it, because encode and send happen here, downstream of the frame it
//! produced. This module is the third layer:
//!
//! - the **encode** span brackets the frame's codec calls inside the frame router's blocking closure
//!   ([`delivery::route_frame`](crate::delivery)) — one sample per frame covering *all* of them, so a
//!   frame encoded in two formats at once reports their sum rather than a nondeterministic pick; and
//! - the **send** span brackets the write of one frame's bytes into a connection's WebSocket sink
//!   ([`connection::writer_task`](crate::connection)).
//!
//! Both push `(frame, ms)` into a [`ProfileHub`] ring, and the hub fills `transport` on the way out
//! — on a `profile` window event and on a deferred `profile.capture` reply alike, **server-side,
//! before serialization**. So every client (editor, CLI attach, agent) reads the identical completed
//! message; no client assembles the layers itself, and there is no client-private perf path.
//!
//! # Arming
//!
//! Arming mirrors the engine's rule (`ludion_engine::live::sync_perf_arming`): measure while
//! somebody is watching, pay nothing otherwise. A connection holds a [`ProfileArm`] for each open
//! `profile` subscription and one for each `profile.capture` in flight; the last guard's drop
//! disarms and clears the rings. Disconnect is covered for free — the guards live in the
//! connection's own state and drop with it.
//!
//! When disarmed, recording costs exactly one relaxed atomic load: no lock, no allocation, no clock
//! read.
//!
//! # Locking rule
//!
//! **The hub's mutex is never held across an `.await`.** Every method below takes it, does a bounded
//! synchronous piece of work, and releases it before returning; the async callers time an operation
//! and record *after* it completes. Nothing in this module is `async`, which is the property that
//! makes the rule checkable by reading it.

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use ludion_core::time::FrameIndex;
use ludion_engine::perf_live::PROFILE_WINDOW_FRAMES;
use ludion_protocol::perf::{MAX_CAPTURE_FRAMES, TransportProfile};
use ludion_protocol::{Event, EventMessage, Response, ResponseBody, WireResult};

/// Frames each ring keeps before the oldest is evicted.
///
/// Every frame a capture can measure ([`MAX_CAPTURE_FRAMES`]) plus ten rolling windows of slack for
/// the frames delivered just before arming and a reader that lags, so the transport section always
/// describes the same span as the sim and GPU sections. Capped rather than grown: an armed hub on a
/// long-running host must have a ceiling.
const RING_CAPACITY: usize = MAX_CAPTURE_FRAMES as usize + PROFILE_WINDOW_FRAMES * 10;

/// The shared transport-timing state: the arming count and the two per-frame sample rings.
///
/// One per [`Server`](crate::Server), shared by `Arc` between the delivery routers (which record the
/// encode span and inject into events) and every connection (which records the send span, holds the
/// arming guards, and injects into capture replies).
///
/// Its mutex is never held across an `.await` — see the module docs.
pub(crate) struct ProfileHub {
    /// How many parties want transport timing right now: one per open `profile` subscription, one
    /// per `profile.capture` in flight. Nonzero is armed; the hot path reads this once, relaxed.
    interest: AtomicUsize,
    /// The rings. Only touched while armed, so a disarmed host never takes this lock.
    samples: Mutex<Samples>,
}

/// The two per-frame rings (one entry per frame), oldest-evicted at [`RING_CAPACITY`].
#[derive(Default)]
struct Samples {
    /// `(frame, encode ms)` in record order — one entry per frame, whatever its subscribers' formats
    /// cost together.
    encode: VecDeque<(u64, f64)>,
    /// `(frame, sink-write ms)` in record order — one entry per frame; a frame delivered to several
    /// subscribers keeps its slowest subscriber's time, whatever order the deliveries finished in.
    send: VecDeque<(u64, f64)>,
}

/// One party's interest in transport timing, held for as long as that interest lasts.
///
/// Dropping it releases the interest; the last one to go disarms the hub and clears the rings, so a
/// later arming starts from frames somebody actually asked about. RAII rather than paired
/// arm/disarm calls because the release paths are many — unsubscribe, disconnect, a capture that
/// answered, a capture whose connection died mid-flight — and every one of them ends by dropping
/// the connection state this lives in.
pub(crate) struct ProfileArm {
    hub: Arc<ProfileHub>,
    /// The newest frame the hub had already sampled when this guard armed.
    ///
    /// A capture whose reply carries no per-frame columns has no frame list to align to, so its
    /// transport section is taken over everything sampled *after* this mark — the frames delivered
    /// while the capture was in flight. `None` when the hub was empty (nothing to exclude).
    after_frame: Option<u64>,
}

impl ProfileHub {
    /// A fresh, disarmed hub.
    pub(crate) fn new() -> Arc<ProfileHub> {
        Arc::new(ProfileHub {
            interest: AtomicUsize::new(0),
            samples: Mutex::new(Samples::default()),
        })
    }

    /// Whether anything wants transport timing. The whole cost of the disabled path.
    pub(crate) fn armed(&self) -> bool {
        self.interest.load(Ordering::Relaxed) != 0
    }

    /// Registers one party's interest, arming the hub until the returned guard drops.
    pub(crate) fn arm(self: &Arc<Self>) -> ProfileArm {
        // Read the high-water mark first: `after_frame` must not include a frame this very arming
        // is about to let through.
        let after_frame = self.newest_sampled_frame();
        self.interest.fetch_add(1, Ordering::AcqRel);
        ProfileArm {
            hub: self.clone(),
            after_frame,
        }
    }

    /// Records one frame's encode span. A no-op while disarmed.
    pub(crate) fn record_encode(&self, frame: FrameIndex, elapsed: Duration) {
        if !self.armed() {
            return;
        }
        let mut samples = self.lock();
        push(&mut samples.encode, frame.as_u64(), millis(elapsed));
    }

    /// Records one frame's sink-write span. A no-op while disarmed.
    pub(crate) fn record_send(&self, frame: FrameIndex, elapsed: Duration) {
        if !self.armed() {
            return;
        }
        let mut samples = self.lock();
        push(&mut samples.send, frame.as_u64(), millis(elapsed));
    }

    /// The transport section for an explicit list of frames, with per-frame columns aligned to it.
    ///
    /// `None` when none of those frames was delivered — the "absent" the wire type documents, not a
    /// section full of zeroes.
    pub(crate) fn profile_for_frames(&self, frames: &[u64]) -> Option<TransportProfile> {
        self.profile_for(frames, true)
    }

    /// The transport section for the frames sampled after `after_frame`, medians only.
    ///
    /// The shape a `profile.capture` reply takes when it did not ask for per-frame columns: there is
    /// no frame list to align columns to, so the window is "what was delivered while the capture was
    /// in flight" instead. `limit` is the capture's own frame count — arming here happens a moment
    /// before the sim thread opens the capture's window, so without it a frame delivered in that gap
    /// would make the section describe more frames than the capture measured.
    pub(crate) fn profile_after(
        &self,
        after_frame: Option<u64>,
        limit: u64,
    ) -> Option<TransportProfile> {
        let frames = {
            let samples = self.lock();
            let mut frames: Vec<u64> = samples
                .encode
                .iter()
                .map(|&(frame, _)| frame)
                .filter(|frame| after_frame.is_none_or(|after| *frame > after))
                .collect();
            frames.sort_unstable();
            frames.dedup();
            frames.split_off(frames.len().saturating_sub(limit as usize))
        };
        self.profile_for(&frames, false)
    }

    /// The transport section over `frames`, optionally carrying the raw per-frame columns.
    fn profile_for(&self, frames: &[u64], columns: bool) -> Option<TransportProfile> {
        let (encode, send) = {
            let samples = self.lock();
            // One entry per frame: a frame delivered to several subscribers is timed once per
            // subscriber, and `push` kept the slowest of them.
            let encode: HashMap<u64, f64> = samples.encode.iter().copied().collect();
            let send: HashMap<u64, f64> = samples.send.iter().copied().collect();
            (encode, send)
        };
        let column = |by_frame: &HashMap<u64, f64>| -> Vec<Option<f64>> {
            frames
                .iter()
                .map(|frame| by_frame.get(frame).copied())
                .collect()
        };
        build(column(&encode), column(&send), columns)
    }

    /// The newest frame either ring holds a sample for.
    fn newest_sampled_frame(&self) -> Option<u64> {
        let samples = self.lock();
        let newest = |ring: &VecDeque<(u64, f64)>| ring.iter().map(|&(frame, _)| frame).max();
        newest(&samples.encode).max(newest(&samples.send))
    }

    /// The one place the rings are locked, so the "never across an `.await`" rule has one seam to
    /// check. A poisoned mutex is a panicked delivery task, which is not a state to limp on.
    fn lock(&self) -> std::sync::MutexGuard<'_, Samples> {
        self.samples.lock().expect("profile hub mutex poisoned")
    }
}

impl ProfileArm {
    /// Fills a deferred `profile.capture` reply's `transport` section, before it is serialized.
    ///
    /// Anything other than a successful `profile.capture` reply passes through untouched.
    pub(crate) fn fill_capture_reply(&self, response: &mut Response) {
        let WireResult::Ok {
            ok: ResponseBody::ProfileReport { report },
            ..
        } = &mut response.result
        else {
            return;
        };
        report.transport = match &report.samples {
            // The capture asked for columns, so its exact frames are known: align to them.
            Some(samples) => self.hub.profile_for_frames(&samples.frames),
            None => self
                .hub
                .profile_after(self.after_frame, report.measured_frames),
        };
    }
}

impl Drop for ProfileArm {
    fn drop(&mut self) {
        if self.hub.interest.fetch_sub(1, Ordering::AcqRel) != 1 {
            return; // somebody else is still watching
        }
        // Last one out: the rings describe frames nobody is asking about any more.
        let mut samples = self.hub.lock();
        if self.hub.interest.load(Ordering::Acquire) == 0 {
            samples.encode.clear();
            samples.send.clear();
        }
    }
}

/// Fills a `profile` window event's `transport` section, before the event is serialized.
///
/// Any other event passes through untouched. The window carries its own per-frame column of frame
/// indices, so the transport columns align to it exactly — a frame the window covers but nobody was
/// delivered reads as `null`, the same convention the GPU columns use for a frame the timing ring
/// skipped.
pub(crate) fn fill_window_event(hub: &ProfileHub, message: &mut EventMessage) {
    if let Event::ProfileWindow(window) = &mut message.event {
        window.transport = hub.profile_for_frames(&window.samples.frames);
    }
}

/// Records one sample, evicting the oldest frame once the ring is full.
///
/// A frame already near the back keeps the larger of the two times rather than gaining a second entry
/// — its slowest subscriber's, independent of delivery order — so the ring holds one entry per frame
/// and eviction counts frames, not subscriber deliveries.
fn push(ring: &mut VecDeque<(u64, f64)>, frame: u64, ms: f64) {
    if let Some(entry) = ring
        .iter_mut()
        .rev()
        .take_while(|entry| entry.0 >= frame)
        .find(|entry| entry.0 == frame)
    {
        entry.1 = entry.1.max(ms);
        return;
    }
    if ring.len() == RING_CAPACITY {
        ring.pop_front();
    }
    ring.push_back((frame, ms));
}

/// Assembles the wire section from two aligned per-frame columns.
///
/// `None` when nothing was encoded: the frames reached no subscriber, which the wire type spells as
/// an absent section rather than zeroes. `columns` decides whether the raw arrays ride along —
/// medians answer what a frame costs, the columns are what a timeline is drawn from.
fn build(
    encode: Vec<Option<f64>>,
    send: Vec<Option<f64>>,
    columns: bool,
) -> Option<TransportProfile> {
    let mut encode_values: Vec<f64> = encode.iter().flatten().copied().collect();
    if encode_values.is_empty() {
        return None;
    }
    let mut send_values: Vec<f64> = send.iter().flatten().copied().collect();
    Some(TransportProfile {
        encode_median_ms: median(&mut encode_values),
        send_median_ms: median(&mut send_values),
        sampled_frames: encode_values.len() as u64,
        encode_ms: if columns { encode } else { Vec::new() },
        send_ms: if columns { send } else { Vec::new() },
    })
}

/// Median of a sample set (`0.0` for an empty one). Sorts in place.
///
/// The same statistic the engine's own sections use, so a report's four layers are comparable
/// numbers rather than four different summaries.
fn median(samples: &mut [f64]) -> f64 {
    if samples.is_empty() {
        return 0.0;
    }
    samples.sort_by(|a, b| a.total_cmp(b));
    let mid = samples.len() / 2;
    if samples.len() % 2 == 1 {
        samples[mid]
    } else {
        (samples[mid - 1] + samples[mid]) / 2.0
    }
}

/// A span's length in milliseconds.
fn millis(elapsed: Duration) -> f64 {
    elapsed.as_secs_f64() * 1_000.0
}

#[cfg(test)]
mod tests {
    use super::*;
    use ludion_protocol::perf::{
        LiveGpuProfile, MAX_CAPTURE_FRAMES, ProfileSamples, ProfileWindowData,
    };
    use ludion_protocol::{Seq, SubId};

    fn frame(index: u64) -> FrameIndex {
        FrameIndex::new(index)
    }

    fn ms(value: f64) -> Duration {
        Duration::from_secs_f64(value / 1_000.0)
    }

    fn window(frames: Vec<u64>) -> EventMessage {
        let count = frames.len() as u64;
        let first = frames.first().copied().unwrap_or(0);
        let last = frames.last().copied().unwrap_or(0);
        EventMessage {
            event: Event::ProfileWindow(Box::new(ProfileWindowData {
                first_frame: first,
                last_frame: last,
                frames: count,
                capture: false,
                threads: 1,
                systems: Vec::new(),
                sequential: Vec::new(),
                frame_total_median_ms: 1.0,
                gpu: LiveGpuProfile {
                    timestamps_supported: false,
                    sampled_frames: 0,
                    passes: Vec::new(),
                },
                transport: None,
                samples: ProfileSamples {
                    frames,
                    system_names: Vec::new(),
                    system_ms: Vec::new(),
                    frame_total_ms: Vec::new(),
                    extract_ms: Vec::new(),
                    submit_ms: Vec::new(),
                    pass_names: Vec::new(),
                    pass_ms: Vec::new(),
                },
            })),
            sub: SubId::new(1),
            seq: Seq::new(0),
            frame: None,
        }
    }

    /// The zero-cost-when-off contract: a disarmed hub accumulates nothing, however many frames flow
    /// through it, and has nothing to inject.
    #[test]
    fn a_disarmed_hub_records_nothing() {
        let hub = ProfileHub::new();
        assert!(!hub.armed());
        for index in 0..200 {
            hub.record_encode(frame(index), ms(5.0));
            hub.record_send(frame(index), ms(0.5));
        }
        assert!(hub.lock().encode.is_empty());
        assert!(hub.lock().send.is_empty());
        assert!(hub.profile_for_frames(&[0, 1, 2]).is_none());
    }

    /// Arming is refcounted: the hub stays armed while any party holds a guard, and the last drop
    /// disarms *and* forgets the samples so a later arming does not report stale frames.
    #[test]
    fn the_last_guard_out_disarms_and_clears() {
        let hub = ProfileHub::new();
        let first = hub.arm();
        let second = hub.arm();
        hub.record_encode(frame(1), ms(4.0));
        drop(first);
        assert!(hub.armed(), "one party left still wants timing");
        assert_eq!(hub.lock().encode.len(), 1);

        drop(second);
        assert!(!hub.armed());
        assert!(
            hub.lock().encode.is_empty(),
            "the rings are dropped with the last interest"
        );
    }

    /// Columns align to the frames asked for, `null` where nothing was delivered, and the medians
    /// are over the samples that exist.
    #[test]
    fn a_window_gets_columns_aligned_to_its_frames() {
        let hub = ProfileHub::new();
        let _arm = hub.arm();
        for (index, encode) in [(10, 4.0), (11, 6.0), (13, 8.0)] {
            hub.record_encode(frame(index), ms(encode));
            hub.record_send(frame(index), ms(1.0));
        }

        let mut message = window(vec![10, 11, 12, 13]);
        fill_window_event(&hub, &mut message);
        let Event::ProfileWindow(filled) = &message.event else {
            panic!("the event kind must survive injection");
        };
        let transport = filled
            .transport
            .as_ref()
            .expect("delivered frames were timed");
        assert_eq!(transport.sampled_frames, 3);
        assert_eq!(transport.encode_median_ms, 6.0);
        assert_eq!(transport.send_median_ms, 1.0);
        assert_eq!(transport.encode_ms.len(), 4);
        assert!(
            transport.encode_ms[2].is_none(),
            "a frame nobody was delivered reads as null, not zero"
        );
        assert_eq!(transport.encode_ms[3], Some(8.0));
    }

    /// A window whose frames were never delivered gets no section at all — absent, not zeroed.
    #[test]
    fn a_window_with_no_delivered_frame_stays_absent() {
        let hub = ProfileHub::new();
        let _arm = hub.arm();
        hub.record_encode(frame(99), ms(4.0));

        let mut message = window(vec![10, 11]);
        fill_window_event(&hub, &mut message);
        let Event::ProfileWindow(filled) = &message.event else {
            panic!("the event kind must survive injection");
        };
        assert!(filled.transport.is_none());
    }

    /// A medians-only capture has no frame list to align to, so its guard's high-water mark is the
    /// boundary: frames delivered before the capture armed are excluded.
    #[test]
    fn a_capture_measures_only_the_frames_after_it_armed() {
        let hub = ProfileHub::new();
        let subscription = hub.arm();
        hub.record_encode(frame(1), ms(100.0));
        hub.record_send(frame(1), ms(9.0));

        let capture = hub.arm();
        hub.record_encode(frame(2), ms(4.0));
        hub.record_send(frame(2), ms(1.0));

        let transport = hub
            .profile_after(capture.after_frame, 10)
            .expect("the capture's own frames were timed");
        assert_eq!(transport.sampled_frames, 1);
        assert_eq!(transport.encode_median_ms, 4.0);
        assert!(
            transport.encode_ms.is_empty(),
            "a medians-only capture carries no columns"
        );
        drop((capture, subscription));
    }

    /// The capture's own frame count is the ceiling: arming happens a moment before the sim thread
    /// opens the window, and a frame delivered in that gap must not make the section claim more
    /// frames than the capture measured.
    #[test]
    fn a_medians_only_capture_reports_no_more_frames_than_it_measured() {
        let hub = ProfileHub::new();
        let capture = hub.arm();
        for index in 1..=4 {
            hub.record_encode(frame(index), ms(index as f64));
        }

        let transport = hub
            .profile_after(capture.after_frame, 3)
            .expect("frames were timed");
        assert_eq!(transport.sampled_frames, 3);
        // The newest three, so the extra frame trimmed is the one that predates the window.
        assert_eq!(transport.encode_median_ms, 3.0);
    }

    /// A full-length capture's transport section covers every frame it measured, even with several
    /// subscribers timing each frame's send.
    #[test]
    fn the_rings_cover_a_full_capture() {
        let hub = ProfileHub::new();
        let _arm = hub.arm();
        let total = MAX_CAPTURE_FRAMES + 50;
        for index in 0..total {
            hub.record_encode(frame(index), ms(1.0));
            hub.record_send(frame(index), ms(0.7));
            hub.record_send(frame(index), ms(0.5));
        }

        let transport = hub
            .profile_after(None, 3_600)
            .expect("the capture's frames were timed");
        assert_eq!(transport.sampled_frames, 3_600);
        let first = hub
            .profile_for_frames(&[50])
            .expect("the first measured frame is still in the ring");
        assert_eq!(first.encode_ms, vec![Some(1.0)]);
        assert_eq!(first.send_ms, vec![Some(0.7)], "worst subscriber per frame");
    }

    /// The ring is capped: a long-armed host evicts the oldest sample instead of growing.
    #[test]
    fn the_rings_are_capped() {
        let hub = ProfileHub::new();
        let _arm = hub.arm();
        for index in 0..(RING_CAPACITY as u64 + 50) {
            hub.record_encode(frame(index), ms(1.0));
        }
        let samples = hub.lock();
        assert_eq!(samples.encode.len(), RING_CAPACITY);
        assert_eq!(samples.encode.front().map(|&(frame, _)| frame), Some(50));
    }
}
