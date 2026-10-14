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
            let millis = self.collect_pass_timings(schedule, output, recorder)?;
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
            }
        };

        let mut json = serde_json::to_string_pretty(&dump)
            .expect("timing structs are plain data and always serialize");
        json.push('\n');
        Ok(json)
    }

    /// Runs the graph with per-pass timestamp writes and returns each pass's GPU time in
    /// milliseconds, in scheduled order. Only called when timestamps are supported.
    fn collect_pass_timings(
        &self,
        schedule: &Schedule,
        output: ResourceId,
        recorder: Option<&dyn PassRecorder>,
    ) -> Result<Vec<f64>, Diagnostic> {
        let pass_count = schedule.order_names().len();
        let query_count = (pass_count * 2) as u32;

        let query_set = self.device.create_query_set(&wgpu::QuerySetDescriptor {
            label: Some("ludion-render-graph timings"),
            ty: wgpu::QueryType::Timestamp,
            count: query_count,
        });
        let resolve_buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("ludion-render-graph timing resolve"),
            size: u64::from(query_count) * 8,
            usage: wgpu::BufferUsages::QUERY_RESOLVE | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        let map_buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("ludion-render-graph timing readback"),
            size: u64::from(query_count) * 8,
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

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
        encoder.resolve_query_set(&query_set, 0..query_count, &resolve_buffer, 0);
        encoder.copy_buffer_to_buffer(
            &resolve_buffer,
            0,
            &map_buffer,
            0,
            u64::from(query_count) * 8,
        );
        self.queue.submit([encoder.finish()]);

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
        let millis = {
            let mapped = map_buffer.slice(..).get_mapped_range().map_err(|error| {
                timing_failure(format!("could not access mapped bytes: {error}"))
            })?;
            let stamps: Vec<u64> = mapped
                .chunks_exact(8)
                .map(|c| u64::from_le_bytes(c.try_into().expect("chunks_exact(8) yields 8 bytes")))
                .collect();
            (0..pass_count)
                .map(|i| {
                    // Saturating: some backends report equal or slightly out-of-order stamps.
                    let delta = stamps[i * 2 + 1].saturating_sub(stamps[i * 2]);
                    (delta as f64) * period_ns / 1.0e6
                })
                .collect()
        };
        map_buffer.unmap();

        Ok(millis)
    }
}

fn timing_failure(detail: String) -> Diagnostic {
    Diagnostic::new(
        code::RENDER_READBACK_FAILED,
        format!("could not read GPU timings back: {detail}"),
    )
    .with_help("timestamp readback failed; retry, and check GPU driver stability")
}
