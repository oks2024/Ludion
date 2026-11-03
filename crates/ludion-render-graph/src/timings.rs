//! `dump-timings` JSON: per-pass GPU time from wgpu timestamp queries, with a graceful fallback.
//!
//! Where the adapter exposes `TIMESTAMP_QUERY`, each pass is bracketed by beginning/end timestamp
//! writes on its `RenderPassDescriptor` (descriptor-level writes are the wgpu-30 mechanism that
//! needs no extra feature beyond `TIMESTAMP_QUERY`); the deltas are resolved, read back, and scaled
//! by the queue's timestamp period into milliseconds. Where it does not (many software rasterizers),
//! the output degrades to a structured `{"timestamps_supported": false, ...}` shape rather than
//! failing — the JSON *shape* is stable and testable; the values, being wall-clock GPU measurements,
//! are not snapshot-testable.
//!
//! This is the **one** timings machinery for both kinds of graph: built-in pass kinds record through
//! the proving executor, externally-recorded graphs (the render pipeline) through their
//! [`PassRecorder`] — the query set, resolve, readback, and scaling are identical.
//!
//! # The frame span
//!
//! Besides per-pass deltas, a timed frame reports one frame-spanning GPU time: the hull of every
//! written stamp, first pass start to last pass end (`frame_span_ms`). It is reported only where
//! one queue timeline is proved, so the stamps are comparable across passes:
//!
//! 1. *Structural.* Both timed paths ([`Gpu::dump_timings_json`] and the render pipeline's frame
//!    loop) record every pass into one encoder and one submit, with the resolve in its own later
//!    submit after that work completes; recorders get no queue and never submit, and wgpu exposes
//!    one queue per device.
//! 2. *Backend allowlist.* Vulkan, Dx12 and Metal report a span; GL and browser WebGPU (quantized
//!    stamps, unpinned) stay `None`.
//! 3. *Per frame, on the data.* A counter reset or wrap (last stamp not after the first) or a failed
//!    sample (`0` or all-ones at either end) yields `None` for that frame.
//!
//! The hull, not last end minus first begin, because on a tile-based GPU an independent pass may
//! start before the first or end after the last; an unwritten interior stamp is ignored. A pass
//! that draws nothing writes no end stamp on Metal, so a graph whose last pass draws nothing
//! reports no span there.
//! Per-pass deltas may overlap and must not be summed to get frame cost; the span is that number.

use serde::Serialize;

use ludion_core::codes::code;
use ludion_core::diagnostics::Diagnostic;

use crate::execute::{Gpu, PassRecorder};
use crate::resource::ResourceId;
use crate::schedule::Schedule;

#[derive(Serialize)]
struct TimingsDump {
    timestamps_supported: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    timestamp_period_ns: Option<f32>,
    passes: Vec<PassTiming>,
    /// First pass start to last pass end, where one queue timeline is proved and the stamps are sound.
    #[serde(skip_serializing_if = "Option::is_none")]
    frame_gpu_time_ms: Option<f64>,
}

/// One timed frame: its per-pass times and, where provable, the frame-spanning GPU time.
#[derive(Clone, Debug)]
pub struct FrameTiming {
    /// Per-pass times in scheduled order.
    pub passes: Vec<PassTiming>,
    /// Earliest pass start to latest pass end in milliseconds, or `None` without timestamps, on a
    /// backend that cannot prove one queue timeline, or when this frame's stamps are unsound.
    pub frame_gpu_time_ms: Option<f64>,
}

/// Bytes one resolved timestamp query occupies.
pub(crate) const QUERY_BYTES: u64 = 8;

/// The GPU storage one timed frame needs: the query set its passes write, the buffer the queries
/// resolve into, and the mappable buffer the CPU reads. The one sizing rule for the blocking dump
/// and the live ring: two queries per pass, [`QUERY_BYTES`] per query.
pub(crate) struct PassQueries {
    pub(crate) queries: wgpu::QuerySet,
    pub(crate) resolve: wgpu::Buffer,
    pub(crate) map: wgpu::Buffer,
}

impl PassQueries {
    pub(crate) fn new(device: &wgpu::Device, pass_count: usize) -> PassQueries {
        let count = (pass_count * 2) as u32;
        let size = u64::from(count) * QUERY_BYTES;
        PassQueries {
            queries: device.create_query_set(&wgpu::QuerySetDescriptor {
                label: Some("ludion-render-graph pass timing"),
                ty: wgpu::QueryType::Timestamp,
                count,
            }),
            resolve: device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("ludion-render-graph pass timing resolve"),
                size,
                usage: wgpu::BufferUsages::QUERY_RESOLVE | wgpu::BufferUsages::COPY_SRC,
                mapped_at_creation: false,
            }),
            map: device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("ludion-render-graph pass timing readback"),
                size,
                usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            }),
        }
    }
}

/// Whether `backend` is one where the timestamps of a single submission share one timeline.
pub(crate) fn backend_proves_one_timeline(backend: wgpu::Backend) -> bool {
    matches!(
        backend,
        wgpu::Backend::Vulkan | wgpu::Backend::Dx12 | wgpu::Backend::Metal
    )
}

/// The frame span in milliseconds from resolved `stamps` (begin0, end0, begin1, end1, … in
/// scheduled order), or `None` when they cannot be trusted: fewer than two, an unwritten (`0` or
/// all-ones) first or last stamp, or a last stamp not after the first (counter reset or wrap).
/// Otherwise the hull of the written stamps; an unwritten interior stamp is ignored.
pub(crate) fn frame_span_ms(stamps: &[u64], period_ns: f64) -> Option<f64> {
    let written = |s: u64| s != 0 && s != u64::MAX;
    let (&first, &last) = (stamps.first()?, stamps.last()?);
    if stamps.len() < 2 || !written(first) || !written(last) || last <= first {
        return None;
    }
    let min = stamps.iter().copied().filter(|&s| written(s)).min()?;
    let max = stamps.iter().copied().filter(|&s| written(s)).max()?;
    Some((max - min) as f64 * period_ns / 1.0e6)
}

/// One pass's GPU time. `gpu_time_ms` is `None` when the adapter cannot time passes at all —
/// the pass is still named, so a report's shape does not depend on the adapter.
///
/// Shared by the blocking [`Gpu::dump_timings_json`] dump and the pipelined
/// [`PassTimingRing`](crate::PassTimingRing).
#[derive(Clone, Debug, Serialize)]
pub struct PassTiming {
    /// The pass's name, as scheduled.
    pub name: String,
    /// GPU time in milliseconds, or `None` without `TIMESTAMP_QUERY`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub gpu_time_ms: Option<f64>,
}

impl Gpu {
    /// Renders `schedule` (targeting `output`) once and returns per-pass GPU timings as JSON.
    ///
    /// `recorder` selects how passes are recorded: `None` runs the built-in pass kinds (the proving
    /// graph); `Some` dispatches every pass to the external recorder (the render pipeline). Either
    /// way the timing machinery — query set, per-pass descriptor timestamp writes, resolve,
    /// readback, scaling — is this one method.
    ///
    /// If the adapter lacks timestamp support, returns the fallback shape without timings — never an
    /// error. The returned string is pretty-printed with a trailing newline, matching `dump-graph`.
    ///
    /// # Errors
    /// `L2005` only if timestamp readback (a buffer map) fails on a timestamp-capable adapter;
    /// `L2010` / `L2011` if the GPU reported a fault (see [`Gpu::take_fault`]).
    pub fn dump_timings_json(
        &self,
        schedule: &Schedule,
        output: ResourceId,
        recorder: Option<&dyn PassRecorder>,
    ) -> Result<String, Diagnostic> {
        self.guarded(|| self.dump_timings_json_unguarded(schedule, output, recorder))
    }

    fn dump_timings_json_unguarded(
        &self,
        schedule: &Schedule,
        output: ResourceId,
        recorder: Option<&dyn PassRecorder>,
    ) -> Result<String, Diagnostic> {
        let names: Vec<String> = schedule
            .order_names()
            .into_iter()
            .map(str::to_owned)
            .collect();

        let dump = if self.timestamps_supported {
            let (millis, frame_gpu_time_ms) =
                self.collect_pass_timings(schedule, output, recorder)?;
            TimingsDump {
                timestamps_supported: true,
                timestamp_period_ns: Some(self.queue.get_timestamp_period()),
                passes: names
                    .into_iter()
                    .zip(millis)
                    .map(|(name, ms)| PassTiming {
                        name,
                        gpu_time_ms: Some(ms),
                    })
                    .collect(),
                frame_gpu_time_ms,
            }
        } else {
            TimingsDump {
                timestamps_supported: false,
                timestamp_period_ns: None,
                passes: names
                    .into_iter()
                    .map(|name| PassTiming {
                        name,
                        gpu_time_ms: None,
                    })
                    .collect(),
                frame_gpu_time_ms: None,
            }
        };

        let mut json = serde_json::to_string_pretty(&dump)
            .expect("timing structs are plain data and always serialize");
        json.push('\n');
        Ok(json)
    }

    /// Runs the graph with per-pass timestamp writes and returns each pass's GPU time in
    /// milliseconds, in scheduled order, with the frame span. Only called when timestamps are
    /// supported.
    fn collect_pass_timings(
        &self,
        schedule: &Schedule,
        output: ResourceId,
        recorder: Option<&dyn PassRecorder>,
    ) -> Result<(Vec<f64>, Option<f64>), Diagnostic> {
        let pass_count = schedule.order_names().len();
        let query_count = (pass_count * 2) as u32;

        let PassQueries {
            queries: query_set,
            resolve: resolve_buffer,
            map: map_buffer,
        } = PassQueries::new(&self.device, pass_count);

        let frame = self.allocate_physicals(schedule);
        // Reference `output` so a caller can't misroute the target; the timing run renders the same
        // graph the image path would.
        let _ = &frame[schedule.aliasing.assignment[output.index()]];

        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("ludion-render-graph timing frame"),
            });
        match recorder {
            None => self.record_passes(schedule, &frame, &mut encoder, Some(&query_set)),
            Some(recorder) => {
                self.record_external_passes(
                    schedule,
                    &frame,
                    &mut encoder,
                    recorder,
                    Some(&query_set),
                );
            }
        }
        self.queue.submit([encoder.finish()]);
        // Metal has not written the last pass's end stamp until the frame's command buffer completes.
        self.device
            .poll(wgpu::PollType::wait_indefinitely())
            .map_err(|error| timing_failure(format!("device poll failed: {error}")))?;

        let mut resolve = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("ludion-render-graph timing resolve"),
            });
        resolve.resolve_query_set(&query_set, 0..query_count, &resolve_buffer, 0);
        resolve.copy_buffer_to_buffer(
            &resolve_buffer,
            0,
            &map_buffer,
            0,
            u64::from(query_count) * QUERY_BYTES,
        );
        self.queue.submit([resolve.finish()]);

        let (tx, rx) = std::sync::mpsc::channel();
        map_buffer
            .slice(..)
            .map_async(wgpu::MapMode::Read, move |result| {
                let _ = tx.send(result);
            });
        self.device
            .poll(wgpu::PollType::wait_indefinitely())
            .map_err(|error| timing_failure(format!("device poll failed: {error}")))?;
        match rx.recv() {
            Ok(Ok(())) => {}
            other => {
                return Err(timing_failure(format!(
                    "timestamp buffer map did not complete: {other:?}"
                )));
            }
        }

        let period_ns = f64::from(self.queue.get_timestamp_period());
        let (millis, span) = {
            let mapped = map_buffer.slice(..).get_mapped_range().map_err(|error| {
                timing_failure(format!("could not access mapped bytes: {error}"))
            })?;
            let stamps: Vec<u64> = mapped
                .chunks_exact(QUERY_BYTES as usize)
                .map(|c| u64::from_le_bytes(c.try_into().expect("chunks_exact(8) yields 8 bytes")))
                .collect();
            let millis: Vec<f64> = (0..pass_count)
                .map(|i| {
                    // Saturating: some backends report equal or slightly out-of-order stamps.
                    let delta = stamps[i * 2 + 1].saturating_sub(stamps[i * 2]);
                    (delta as f64) * period_ns / 1.0e6
                })
                .collect();
            let span = if self.frame_span_proved() {
                frame_span_ms(&stamps, period_ns)
            } else {
                None
            };
            (millis, span)
        };
        map_buffer.unmap();

        Ok((millis, span))
    }

    /// Whether this adapter's backend proves one queue timeline, so a frame span may be reported.
    pub(crate) fn frame_span_proved(&self) -> bool {
        backend_proves_one_timeline(self.adapter.get_info().backend)
    }
}

fn timing_failure(detail: String) -> Diagnostic {
    Diagnostic::new(
        code::RENDER_READBACK_FAILED,
        format!("could not read GPU timings back: {detail}"),
    )
    .with_help("timestamp readback failed; retry, and check GPU driver stability")
}

#[cfg(test)]
mod tests {
    use super::*;

    const MS: f64 = 1.0e6;

    #[test]
    fn overlapping_passes_span_less_than_their_sum() {
        let stamps = [100, 200, 150, 250, 200, 300];
        assert_eq!(frame_span_ms(&stamps, 1.0), Some(200.0 / MS));
        let sum: u64 = stamps.chunks(2).map(|p| p[1] - p[0]).sum();
        assert_eq!(sum, 300);
    }

    #[test]
    fn no_stamps_and_one_pass() {
        assert_eq!(frame_span_ms(&[], 1.0), None);
        assert_eq!(frame_span_ms(&[5], 1.0), None);
        assert_eq!(frame_span_ms(&[100, 300], 1.0), Some(200.0 / MS));
    }

    #[test]
    fn period_scales_the_span() {
        assert_eq!(frame_span_ms(&[100, 300], 2.5), Some(500.0 / MS));
    }

    #[test]
    fn a_counter_reset_is_refused() {
        assert_eq!(frame_span_ms(&[300, 400, 100, 200], 1.0), None);
        assert_eq!(frame_span_ms(&[300, 300], 1.0), None);
    }

    #[test]
    fn sentinel_ends_are_refused() {
        assert_eq!(frame_span_ms(&[0, 100, 200, 300], 1.0), None);
        assert_eq!(frame_span_ms(&[100, 200, 300, 0], 1.0), None);
        assert_eq!(frame_span_ms(&[u64::MAX, 200, 300, 400], 1.0), None);
        assert_eq!(frame_span_ms(&[100, 200, 300, u64::MAX], 1.0), None);
    }

    #[test]
    fn an_unwritten_interior_stamp_is_ignored() {
        assert_eq!(frame_span_ms(&[100, 0, 200, 300], 1.0), Some(200.0 / MS));
        assert_eq!(
            frame_span_ms(&[100, u64::MAX, 200, 300], 1.0),
            Some(200.0 / MS)
        );
    }

    #[test]
    fn a_resolve_before_completion_is_refused() {
        let stamps = [
            892_798_226_496_708,
            892_798_226_514_416,
            892_798_226_510_541,
            0,
        ];
        assert_eq!(frame_span_ms(&stamps, 1.0), None);
    }

    #[test]
    fn a_drawless_interior_pass_keeps_the_span() {
        let stamps = [
            892_804_303_481_541,
            0,
            892_804_303_500_333,
            892_804_303_629_208,
            892_804_303_649_750,
        ];
        assert_eq!(frame_span_ms(&stamps, 1.0), Some(168_209.0 / MS));
    }

    #[test]
    fn an_interior_excursion_widens_the_span() {
        assert_eq!(
            frame_span_ms(&[100, 200, 150, 500, 250, 300], 1.0),
            Some(400.0 / MS)
        );
        assert_eq!(
            frame_span_ms(&[100, 200, 50, 250, 250, 300], 1.0),
            Some(250.0 / MS)
        );
    }

    #[test]
    fn a_reversed_interior_pair_still_gives_the_span() {
        assert_eq!(
            frame_span_ms(&[100, 200, 260, 220, 250, 300], 1.0),
            Some(200.0 / MS)
        );
    }

    #[test]
    fn only_vulkan_dx12_and_metal_prove_one_timeline() {
        for backend in [
            wgpu::Backend::Vulkan,
            wgpu::Backend::Dx12,
            wgpu::Backend::Metal,
        ] {
            assert!(backend_proves_one_timeline(backend));
        }
        for backend in [wgpu::Backend::Gl, wgpu::Backend::BrowserWebGpu] {
            assert!(!backend_proves_one_timeline(backend));
        }
    }
}
