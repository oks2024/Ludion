//! Pipelined per-pass GPU timing: a 3-deep ring of timestamp query sets for the *live* frame loop.
//!
//! [`timings`](crate::timings) answers "how long would this graph's passes take" by rendering one
//! synthetic frame and **blocking** on the timestamp readback. That is the right shape for
//! `dump-timings`, and the wrong shape for a profiler watching the frames the engine is already
//! rendering. [`PassTimingRing`] is the non-blocking counterpart, built on the same three pieces
//! ([`Gpu::render_streamed`] attaches the descriptor-level timestamp writes, resolves into a buffer,
//! and `map_async`'s it) but pipelined exactly like [`StreamReadback`](crate::StreamReadback): up to
//! [`PassTimingRing::DEPTH`] frames' query sets are in flight at once, and
//! [`try_collect_timings`](PassTimingRing::try_collect_timings) drains whichever is ready.
//!
//! A sample is resolved only after its frame's GPU work has finished: Metal has not written the
//! last pass's end stamp before then. The frame files a completion callback; the next collect that
//! sees it fire submits the resolve and requests the readback map, so a sample arrives a little
//! later and nothing ever waits.
//!
//! Two deliberate divergences from the image ring:
//!
//! - **A full ring skips the frame's timing; it never blocks.** The image ring blocks on its oldest
//!   readback because a dropped frame is a visible defect; a dropped *measurement* is not. A
//!   profiler that stalls the render loop changes the numbers it reports, so skipping is the only
//!   honest option.
//! - **Samples arrive 1–3 frames late**, so each carries the caller's stamp for the frame it
//!   actually measured — never the frame that happened to collect it.
//!
//! On an adapter without `TIMESTAMP_QUERY` the ring allocates no query sets at all and reports the
//! same degradation `dump_timings_json` does: every pass present by name with `gpu_time_ms: None`.
//! A graph with more passes than a slot holds ([`PassTimingRing::MAX_PASSES`]) files the same
//! named, untimed sample, so an over-cap graph is visible rather than silent.
//! Timing never returns an error — losing a sample must never cost a frame.
//!
//! Each collected sample also carries the frame span ([`FrameTiming`]), under the proofs in
//! [`timings`](crate::timings).

use std::collections::VecDeque;
use std::sync::mpsc::{Receiver, TryRecvError};

use crate::execute::Gpu;
use crate::timings::{FrameTiming, PassQueries, PassTiming, QUERY_BYTES, frame_span_ms};

/// A persistent 3-deep ring of timestamp query sets for non-blocking per-pass GPU timing.
///
/// Created once per [`Gpu`] ([`Gpu::new_pass_timing_ring`]) and reused across frames — unlike the
/// image ring it is independent of the render size, and a graph may change shape between frames as
/// long as it stays within [`MAX_PASSES`](PassTimingRing::MAX_PASSES); a larger one is named and
/// untimed. `S` is a caller-chosen stamp
/// (a [`FrameIndex`](ludion_core::time::FrameIndex) for the live loop) carried through each pending
/// sample so a late-arriving measurement is filed against the frame it describes.
///
/// Pass it to [`Gpu::render_streamed`] as `Some(&mut ring)` to time a frame; `None` there is the
/// untimed path and costs nothing.
pub struct PassTimingRing<S> {
    /// One query set + resolve + map buffer per in-flight frame. **Empty** when the adapter has no
    /// `TIMESTAMP_QUERY` — that emptiness is the degraded mode, not a separate flag.
    slots: Vec<PassQueries>,
    /// Filed samples, oldest first.
    pending: VecDeque<Pending<S>>,
    /// The slot the next timed frame writes (round-robin), free whenever the ring is not full.
    next_slot: usize,
    /// The queue's timestamp period, captured once at construction; `None` in degraded mode.
    period_ns: Option<f32>,
    /// Whether this backend proves one queue timeline, so a sample may carry a frame span.
    span_proved: bool,
}

/// A filed sample awaiting collection.
struct Pending<S> {
    stamp: S,
    /// Pass names in scheduled order, captured when the frame was recorded — a later frame may
    /// schedule a different graph, so the names cannot be re-derived at collection time.
    names: Vec<String>,
    /// The in-flight readback, or `None` for an untimed sample (ready the moment it is filed).
    mapped: Option<MappedSlot>,
}

struct MappedSlot {
    slot: usize,
    queries: u32,
    /// Fires when the frame's GPU work finishes; `None` once the resolve is submitted.
    done: Option<Receiver<()>>,
    /// The readback map, requested only after the resolve.
    rx: Option<Receiver<Result<(), wgpu::BufferAsyncError>>>,
}

/// What a frame's timing does, decided before recording begins.
pub(crate) enum FramePlan {
    /// Write `queries` timestamps into `slot` and resolve them.
    Timed { slot: usize, queries: u32 },
    /// File a sample that names its passes and times none: the adapter has no `TIMESTAMP_QUERY`, or
    /// the graph has more passes than a slot holds.
    Untimed,
    /// Time nothing and file nothing: the ring is full (or there is no ring).
    Skip,
}

impl<S> PassTimingRing<S> {
    /// Maximum frames whose timings may be in flight. A frame arriving with the ring full goes
    /// untimed rather than stalling for a slot.
    pub const DEPTH: usize = 3;

    /// Maximum passes one frame can time. Each pass costs a begin/end pair, so a slot's query set
    /// holds `MAX_PASSES * 2` queries; a graph above this files a sample that names its passes and
    /// times none, rather than reporting a truncated frame. The fullest graph at 8192 schedules
    /// about 75 passes.
    pub const MAX_PASSES: usize = 128;

    fn new(device: &wgpu::Device, period_ns: f32, span_proved: bool) -> PassTimingRing<S> {
        let slots = (0..Self::DEPTH)
            .map(|_| PassQueries::new(device, Self::MAX_PASSES))
            .collect();
        PassTimingRing {
            slots,
            pending: VecDeque::new(),
            next_slot: 0,
            period_ns: Some(period_ns),
            span_proved,
        }
    }

    /// The no-`TIMESTAMP_QUERY` ring: no query sets, no buffers, samples that name their passes and
    /// time none of them.
    fn degraded() -> PassTimingRing<S> {
        PassTimingRing {
            slots: Vec::new(),
            pending: VecDeque::new(),
            next_slot: 0,
            period_ns: None,
            span_proved: false,
        }
    }

    /// Whether this adapter can time passes at all. `false` means every collected sample carries
    /// `gpu_time_ms: None` — the same degradation
    /// [`dump_timings_json`](Gpu::dump_timings_json) reports as `"timestamps_supported": false`.
    pub fn timestamps_supported(&self) -> bool {
        !self.slots.is_empty()
    }

    /// The queue's timestamp period in nanoseconds (the scale applied to raw query deltas), or
    /// `None` in degraded mode.
    pub fn timestamp_period_ns(&self) -> Option<f32> {
        self.period_ns
    }

    /// Non-blocking drain of the oldest completed sample, or `None` if it is still in flight (or
    /// nothing is pending). Advances map callbacks with a non-blocking `poll(Poll)` first.
    ///
    /// Never fails: a poll or map error costs that one sample (it is dropped, its slot freed) and
    /// leaves the render loop untouched.
    pub fn try_collect_timings(&mut self, gpu: &Gpu) -> Option<(S, FrameTiming)> {
        if self.pending.front()?.mapped.is_some() {
            if gpu.device.poll(wgpu::PollType::Poll).is_err() {
                return None;
            }
            self.advance(gpu);
        }
        self.take_ready()
    }

    /// Submits the resolve and requests the readback map for every pending frame whose GPU work has
    /// finished, oldest first, stopping at the first still in flight.
    fn advance(&mut self, gpu: &Gpu) {
        for index in 0..self.pending.len() {
            let Some(mapped) = self.pending[index].mapped.as_mut() else {
                continue;
            };
            let Some(done) = mapped.done.as_ref() else {
                continue;
            };
            match done.try_recv() {
                Err(TryRecvError::Empty) => return,
                // The callback was dropped unfired; `take_ready` drops the sample.
                Err(TryRecvError::Disconnected) => {
                    mapped.done = None;
                    continue;
                }
                Ok(()) => {}
            }
            mapped.done = None;
            let (slot, queries) = (mapped.slot, mapped.queries);
            let mut encoder = gpu
                .device
                .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                    label: Some("ludion-render-graph pass timing resolve"),
                });
            self.record_resolve(&mut encoder, slot, queries);
            gpu.queue.submit([encoder.finish()]);
            let (tx, rx) = std::sync::mpsc::channel();
            self.slots[slot]
                .map
                .slice(..)
                .map_async(wgpu::MapMode::Read, move |result| {
                    let _ = tx.send(result);
                });
            if let Some(mapped) = self.pending[index].mapped.as_mut() {
                mapped.rx = Some(rx);
            }
        }
    }

    /// Decides how the next frame is timed, given how many passes it schedules.
    pub(crate) fn plan_frame(&self, pass_count: usize) -> FramePlan {
        if self.pending.len() >= Self::DEPTH {
            FramePlan::Skip
        } else if self.slots.is_empty() || pass_count > Self::MAX_PASSES {
            FramePlan::Untimed
        } else {
            FramePlan::Timed {
                slot: self.next_slot,
                queries: (pass_count * 2) as u32,
            }
        }
    }

    /// The query set a [`FramePlan::Timed`] frame's passes write into.
    pub(crate) fn query_set(&self, slot: usize) -> &wgpu::QuerySet {
        &self.slots[slot].queries
    }

    /// Records the resolve + copy for a timed frame, into an encoder submitted after the frame's
    /// work has completed.
    pub(crate) fn record_resolve(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        slot: usize,
        queries: u32,
    ) {
        let slot = &self.slots[slot];
        encoder.resolve_query_set(&slot.queries, 0..queries, &slot.resolve, 0);
        encoder.copy_buffer_to_buffer(
            &slot.resolve,
            0,
            &slot.map,
            0,
            u64::from(queries) * QUERY_BYTES,
        );
    }

    /// Files the frame as pending. Called **after** the frame is submitted; `done` fires when its
    /// GPU work finishes and is `None` only for an untimed sample.
    pub(crate) fn file(
        &mut self,
        plan: FramePlan,
        names: Vec<String>,
        stamp: S,
        done: Option<Receiver<()>>,
    ) {
        let mapped = match plan {
            FramePlan::Skip => return,
            FramePlan::Untimed => None,
            FramePlan::Timed { slot, queries } => {
                self.next_slot = (slot + 1) % Self::DEPTH;
                Some(MappedSlot {
                    slot,
                    queries,
                    done,
                    rx: None,
                })
            }
        };
        self.pending.push_back(Pending {
            stamp,
            names,
            mapped,
        });
    }

    /// Pops the oldest sample if it has completed, converting raw query deltas to milliseconds.
    fn take_ready(&mut self) -> Option<(S, FrameTiming)> {
        match &self.pending.front()?.mapped {
            None => {}
            Some(mapped) => match (&mapped.done, &mapped.rx) {
                // Still in flight, or completed but not yet resolved.
                (Some(_), _) => return None,
                (None, Some(rx)) => match rx.try_recv() {
                    Ok(Ok(())) => {}
                    Err(TryRecvError::Empty) => return None,
                    // A failed map costs this sample and frees its slot; the ring keeps running.
                    Ok(Err(_)) | Err(TryRecvError::Disconnected) => {
                        self.pending.pop_front();
                        return None;
                    }
                },
                // Completion never arrived; a failed sample frees its slot like a failed map.
                (None, None) => {
                    self.pending.pop_front();
                    return None;
                }
            },
        }
        let Pending {
            stamp,
            names,
            mapped,
        } = self.pending.pop_front().expect("front was just inspected");
        let (millis, frame_gpu_time_ms) =
            match mapped.and_then(|m| self.read_slot(m.slot, names.len())) {
                Some((millis, span)) => (Some(millis), span),
                None => (None, None),
            };
        let passes = names
            .into_iter()
            .enumerate()
            .map(|(i, name)| PassTiming {
                name,
                gpu_time_ms: millis.as_ref().map(|m| m[i]),
            })
            .collect();
        Some((
            stamp,
            FrameTiming {
                passes,
                frame_gpu_time_ms,
            },
        ))
    }

    /// Reads `pass_count` begin/end query pairs out of a mapped slot and unmaps it, returning the
    /// per-pass milliseconds and the frame span.
    fn read_slot(&self, slot: usize, pass_count: usize) -> Option<(Vec<f64>, Option<f64>)> {
        let period_ns = f64::from(self.period_ns?);
        let buffer = &self.slots[slot].map;
        let millis = buffer.slice(..).get_mapped_range().ok().map(|mapped| {
            let stamps: Vec<u64> = mapped
                .chunks_exact(QUERY_BYTES as usize)
                .take(pass_count * 2)
                .map(|c| u64::from_le_bytes(c.try_into().expect("chunks_exact(8) yields 8 bytes")))
                .collect();
            let millis: Vec<f64> = (0..pass_count)
                .map(|i| {
                    // Saturating: some backends report equal or slightly out-of-order stamps.
                    let delta = stamps[i * 2 + 1].saturating_sub(stamps[i * 2]);
                    (delta as f64) * period_ns / 1.0e6
                })
                .collect();
            let span = if self.span_proved {
                frame_span_ms(&stamps, period_ns)
            } else {
                None
            };
            (millis, span)
        });
        buffer.unmap();
        millis
    }
}

impl Gpu {
    /// Creates a [`PassTimingRing`] for this context, reusable across frames and render sizes.
    ///
    /// On an adapter without `TIMESTAMP_QUERY` this allocates nothing and the ring runs in its
    /// degraded mode (see the module docs).
    pub fn new_pass_timing_ring<S>(&self) -> PassTimingRing<S> {
        if self.timestamps_supported {
            PassTimingRing::new(
                &self.device,
                self.queue.get_timestamp_period(),
                self.frame_span_proved(),
            )
        } else {
            PassTimingRing::degraded()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(count: usize) -> Vec<String> {
        (0..count).map(|i| format!("pass{i}")).collect()
    }

    /// An adapter without `TIMESTAMP_QUERY` still reports every pass, with no timings and no query
    /// writes to attach.
    #[test]
    fn degraded_ring_reports_named_passes_without_timings() {
        let mut ring: PassTimingRing<u64> = PassTimingRing::degraded();
        assert!(!ring.timestamps_supported());
        assert!(ring.timestamp_period_ns().is_none());

        let plan = ring.plan_frame(4);
        assert!(matches!(plan, FramePlan::Untimed));
        ring.file(plan, names(4), 7, None);

        let (stamp, timing) = ring
            .take_ready()
            .expect("a degraded sample is ready at once");
        let passes = timing.passes;
        assert!(timing.frame_gpu_time_ms.is_none());
        assert_eq!(stamp, 7);
        assert_eq!(passes.len(), 4);
        assert!(passes.iter().all(|p| p.gpu_time_ms.is_none()));
        assert_eq!(passes[0].name, "pass0");
        assert!(ring.take_ready().is_none());
    }

    /// The full ring skips rather than blocks, in degraded mode too, and recovers on a drain.
    #[test]
    fn full_ring_skips_and_recovers() {
        let mut ring: PassTimingRing<u64> = PassTimingRing::degraded();
        for stamp in 0..PassTimingRing::<u64>::DEPTH as u64 {
            let plan = ring.plan_frame(2);
            assert!(matches!(plan, FramePlan::Untimed));
            ring.file(plan, names(2), stamp, None);
        }
        assert!(matches!(ring.plan_frame(2), FramePlan::Skip));

        assert_eq!(ring.take_ready().map(|(stamp, _)| stamp), Some(0));
        assert!(matches!(ring.plan_frame(2), FramePlan::Untimed));
    }

    /// A graph with more passes than a slot holds is named and untimed, not dropped and not
    /// truncated; only a full ring skips it.
    #[test]
    fn oversize_graph_is_named_untimed() {
        let max = PassTimingRing::<u64>::MAX_PASSES;
        let mut ring: PassTimingRing<u64> = PassTimingRing::degraded();
        assert!(matches!(ring.plan_frame(max), FramePlan::Untimed));
        let plan = ring.plan_frame(max + 1);
        assert!(matches!(plan, FramePlan::Untimed));

        ring.file(plan, names(max + 1), 3, None);
        let (stamp, timing) = ring
            .take_ready()
            .expect("an untimed sample is ready at once");
        assert_eq!(stamp, 3);
        assert_eq!(timing.passes.len(), max + 1);
        assert!(timing.passes.iter().all(|p| p.gpu_time_ms.is_none()));
        assert_eq!(timing.passes[max].name, format!("pass{max}"));

        for stamp in 0..PassTimingRing::<u64>::DEPTH as u64 {
            let plan = ring.plan_frame(2);
            ring.file(plan, names(2), stamp, None);
        }
        assert!(matches!(ring.plan_frame(max + 1), FramePlan::Skip));
    }
}
