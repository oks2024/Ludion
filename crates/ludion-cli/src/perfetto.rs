//! Chrome trace-event JSON export for a [`ProfileReport`] — openable at ui.perfetto.dev.
//!
//! [`chrome_trace`] is a pure function over a report already in hand: it adds no capability the wire
//! protocol doesn't already offer, so it stays a client-side serializer rather than a wire method,
//! exactly as `Plan/engine-profiler.md`'s design decisions require. Both
//! `ludion profile` (offline) and `ludion profile --attach` (live) funnel through the same
//! `--perfetto <PATH>` flag in `profile.rs`.
//!
//! # Timeline construction (a synthetic timeline, not wall-clock truth)
//!
//! A [`ProfileReport`]'s raw per-frame numbers ([`ProfileSamples`]) are *durations*, never real
//! timestamps — nothing in the report says when frame 0 started. This module manufactures a stable
//! timeline from them, by the same rule the editor's TS export uses
//! (`editor/app/src/perf/trace.ts` — the two exports are kept byte-comparable on purpose):
//!
//! 1. **A frame's origin is the sum of every earlier frame's measured wall time**, and a frame's wall
//!    time is `frame_total + extract + submit` (absent `extract`/`submit` count as zero). The two host
//!    spans are measured *beside* the tick, not inside it ([`SimProfile::sequential`] on the offline
//!    side, the live tick's own sequential spans on the other), so they extend the frame rather than
//!    fitting inside it — advancing the cursor by `frame_total` alone would leave them overlapping the
//!    next frame's `frame_total` on the host lane, which is not a busy trace, it's a malformed one (a
//!    viewer nests spans on a thread; two that straddle each other on one `tid` don't nest). Gaps
//!    between frames — the host sleeping to hold the tick rate — are NOT represented: the trace is a
//!    picture of the work, not of the wall clock.
//! 2. **Sim spans butt-join within their frame**, because the schedule genuinely did run strictly
//!    sequentially (`ludion-ecs` has no per-system parallelism — see `CLAUDE.md`'s determinism rules).
//!    The schedule cannot outrun its own frame total, so a frame's systems never spill into the next.
//! 3. **GPU and transport are aligned by frame index, and their lanes are monotonic.** A pass or
//!    transport sample did not literally happen inside its sim frame's wall window — a live capture
//!    pipelines GPU timing 1-3 frames late and transport happens on a different thread entirely — but
//!    each sample is *filed* by frame index, so a frame's GPU/transport spans start at that frame's
//!    synthetic origin *or* when the previous frame's same-lane work ended, whichever is later — so a
//!    lane that can't keep up reads as a queue (which is what it is) rather than as overlapping spans.
//!    Read these two lanes as "what frame N's own samples were, laid out in relative order" rather
//!    than as literally accurate wall time.
//!
//! # Lanes
//!
//! One process (`pid` 1), up to four thread lanes named via `thread_name` metadata events:
//! 1. **sim systems** (`tid` 1) — per-system spans, butt-joined within each frame. Absent when the
//!    profiled engine registered no systems (the built-in registry, offline).
//! 2. **host** (`tid` 2) — `frame_total`, then `extract`, then `submit` (the latter two present iff
//!    that frame sampled them), per frame. Always present when there is at least one frame.
//! 3. **GPU passes** (`tid` 3) — per-pass spans, butt-joined within each frame. Absent when no frame
//!    carried a GPU sample (no adapter, or timestamps unsupported) — never an error. The lane is
//!    schematic: it joins per-pass durations, not stamps, so it shows no overlap and no gaps; frame
//!    cost is `gpu.frame_median_ms`.
//! 4. **transport** (`tid` 4) — `encode` then `send`, per frame. Absent when the report carries no
//!    transport columns (every offline report, or a live report no frame was ever delivered from).
//!
//! # Samples required
//!
//! [`chrome_trace`] needs [`ProfileReport::samples`] — medians alone cannot draw a timeline. Both
//! producers fill it whenever `--perfetto` is requested (the offline driver always does now; a live
//! `--attach` capture asks for it), so this is refused only for a report that genuinely carries none —
//! a live capture taken without `include_samples`, or an older committed report JSON predating the
//! samples section. The refusal names the fix rather than silently drawing a one-frame stand-in from
//! medians: a "trace" built from a median would look like real per-frame data and mislead exactly the
//! reader this export exists to help.

use ludion_engine::ProfileReport;
use serde_json::{Value, json};

/// The `sim systems` lane.
const TID_SIM: u64 = 1;
/// The `host` lane (frame total, extract, submit).
const TID_HOST: u64 = 2;
/// The `GPU passes` lane.
const TID_GPU: u64 = 3;
/// The `transport` lane (encode, send).
const TID_TRANSPORT: u64 = 4;
/// The one synthetic process every trace event is filed under.
const PID: u64 = 1;

/// Builds a Chrome trace-event document (`{"traceEvents": [...]}`) from `report`, openable at
/// ui.perfetto.dev.
///
/// See the module docs for the timeline construction and lane assignment. Every span's `ts`/`dur` is
/// in **microseconds** (Chrome trace-event convention), converted from the report's milliseconds.
///
/// # Errors
/// A human-readable `String` when `report.samples` is `None` — there is no per-frame data to build a
/// timeline from (see the module docs' "Samples required" section).
pub fn chrome_trace(report: &ProfileReport) -> Result<Value, String> {
    let samples = report.samples.as_ref().ok_or_else(|| {
        "this report carries no per-frame samples to build a timeline from (`samples` is absent) — \
         a fresh `ludion profile --scene ...` or `--attach <url> --perfetto <out>` capture always \
         requests them, so this is either an older committed report JSON, or a `profile.capture` \
         reply taken without `include_samples`; re-run the capture with `--perfetto` set to get one \
         that carries them"
            .to_string()
    })?;

    let frame_count = samples.frames.len();

    // Frame N's synthetic origin: the cumulative sum of every earlier frame's `frame_total + extract +
    // submit` (rule 1 in the module docs) — not `frame_total` alone, which would leave a frame's own
    // `extract`/`submit` overlapping the next frame's `frame_total` on the host lane.
    let mut frame_start_us = Vec::with_capacity(frame_count);
    let mut elapsed_us = 0.0f64;
    for (i, &total_ms) in samples.frame_total_ms.iter().enumerate() {
        frame_start_us.push(elapsed_us);
        let extract_ms = samples.extract_ms.get(i).copied().flatten().unwrap_or(0.0);
        let submit_ms = samples.submit_ms.get(i).copied().flatten().unwrap_or(0.0);
        elapsed_us += ms_to_us(total_ms + extract_ms + submit_ms);
    }

    let has_sim_lane = !samples.system_names.is_empty();
    let gpu_frame_has_sample = |i: usize| -> bool {
        samples
            .pass_ms
            .iter()
            .any(|row| row.get(i).is_some_and(|v| v.is_some()))
    };
    let has_gpu_lane = (0..frame_count).any(gpu_frame_has_sample);
    let transport_frame_has_sample = |i: usize| -> bool {
        report.transport.as_ref().is_some_and(|t| {
            t.encode_ms.get(i).is_some_and(|v| v.is_some())
                || t.send_ms.get(i).is_some_and(|v| v.is_some())
        })
    };
    let has_transport_lane = (0..frame_count).any(transport_frame_has_sample);

    let mut events = Vec::new();
    if has_sim_lane {
        events.push(thread_name(TID_SIM, "sim systems"));
    }
    if frame_count > 0 {
        events.push(thread_name(TID_HOST, "host"));
    }
    if has_gpu_lane {
        events.push(thread_name(TID_GPU, "GPU passes"));
    }
    if has_transport_lane {
        events.push(thread_name(TID_TRANSPORT, "transport"));
    }

    // Monotonic across frames (rule 3): a lane's cursor never rewinds to a later frame's earlier
    // origin, so a lane that can't keep up queues instead of overlapping.
    let mut gpu_cursor_us = 0.0f64;
    let mut transport_cursor_us = 0.0f64;

    for (i, &base) in frame_start_us.iter().enumerate() {
        if has_sim_lane {
            let mut cursor = base;
            for (sys_idx, name) in samples.system_names.iter().enumerate() {
                let dur_ms = samples
                    .system_ms
                    .get(sys_idx)
                    .and_then(|row| row.get(i))
                    .copied()
                    .unwrap_or(0.0);
                let dur_us = ms_to_us(dur_ms);
                events.push(span(name, "sim", TID_SIM, cursor, dur_us));
                cursor += dur_us;
            }
        }

        // Host: frame_total, then extract, then submit — butt-joined in that fixed order.
        let total_us = ms_to_us(samples.frame_total_ms[i]);
        events.push(span("frame_total", "host", TID_HOST, base, total_us));
        let mut cursor = base + total_us;
        if let Some(extract_ms) = samples.extract_ms.get(i).copied().flatten() {
            let dur_us = ms_to_us(extract_ms);
            events.push(span("extract", "host", TID_HOST, cursor, dur_us));
            cursor += dur_us;
        }
        if let Some(submit_ms) = samples.submit_ms.get(i).copied().flatten() {
            let dur_us = ms_to_us(submit_ms);
            events.push(span("submit", "host", TID_HOST, cursor, dur_us));
        }

        if has_gpu_lane {
            let mut cursor = base.max(gpu_cursor_us);
            for (pass_idx, name) in samples.pass_names.iter().enumerate() {
                let Some(pass_ms) = samples
                    .pass_ms
                    .get(pass_idx)
                    .and_then(|row| row.get(i).copied().flatten())
                else {
                    continue;
                };
                let dur_us = ms_to_us(pass_ms);
                events.push(span(name, "gpu", TID_GPU, cursor, dur_us));
                cursor += dur_us;
            }
            gpu_cursor_us = cursor;
        }

        if has_transport_lane && let Some(transport) = report.transport.as_ref() {
            let mut cursor = base.max(transport_cursor_us);
            if let Some(encode_ms) = transport.encode_ms.get(i).copied().flatten() {
                let dur_us = ms_to_us(encode_ms);
                events.push(span("encode", "transport", TID_TRANSPORT, cursor, dur_us));
                cursor += dur_us;
            }
            if let Some(send_ms) = transport.send_ms.get(i).copied().flatten() {
                let dur_us = ms_to_us(send_ms);
                events.push(span("send", "transport", TID_TRANSPORT, cursor, dur_us));
                cursor += dur_us;
            }
            transport_cursor_us = cursor;
        }
    }

    Ok(json!({ "traceEvents": events }))
}

/// One complete-span (`ph: "X"`) trace event.
fn span(name: &str, category: &str, tid: u64, ts_us: f64, dur_us: f64) -> Value {
    json!({
        "name": name,
        "cat": category,
        "ph": "X",
        "ts": ts_us,
        "dur": dur_us,
        "pid": PID,
        "tid": tid,
    })
}

/// One `thread_name` metadata (`ph: "M"`) event, naming a lane in the Perfetto/Chrome trace viewer.
fn thread_name(tid: u64, name: &str) -> Value {
    json!({
        "name": "thread_name",
        "ph": "M",
        "pid": PID,
        "tid": tid,
        "args": { "name": name },
    })
}

/// Converts a millisecond duration (the report's unit) to microseconds (the trace-event unit).
fn ms_to_us(ms: f64) -> f64 {
    ms * 1_000.0
}

#[cfg(test)]
mod tests {
    use super::*;
    use ludion_engine::{GpuProfile, PassTiming, SimProfile, SystemTiming};
    use ludion_protocol::perf::{ProfileSamples, TransportProfile};

    /// A minimal two-frame report with one sim system, one GPU pass, and transport columns — enough
    /// to exercise every lane at once.
    fn two_frame_report() -> ProfileReport {
        ProfileReport {
            scene: "test".to_string(),
            warmup_frames: 0,
            measured_frames: 2,
            quality: None,
            sim: SimProfile {
                systems: vec![SystemTiming {
                    name: "bump".to_string(),
                    median_ms: 1.0,
                    tail: None,
                }],
                sequential: vec![],
                frame_total_median_ms: 5.0,
                frame_total_tail: None,
                threads: Some(8),
            },
            gpu: GpuProfile {
                available: true,
                timestamps_supported: true,
                passes: vec![PassTiming {
                    name: "scene".to_string(),
                    median_ms: Some(2.0),
                }],
                frame_median_ms: None,
                frame_tail: None,
            },
            transport: Some(TransportProfile {
                encode_median_ms: 1.5,
                send_median_ms: 0.2,
                sampled_frames: 2,
                encode_ms: vec![Some(1.0), Some(1.2)],
                send_ms: vec![Some(0.1), Some(0.2)],
            }),
            // The trace has no cluster lane: the cluster build is one CPU cost per frame with no
            // per-frame column to lay out, and a timeline is drawn from columns.
            cluster: None,
            local_shadows: None,
            sun_cascades: None,
            decals: None,
            load: None,
            probes: None,
            culling: None,
            skin: None,
            extract: None,
            assembly: None,
            exposure: None,
            checkpoints: None,
            samples: Some(ProfileSamples {
                frames: vec![10, 11],
                system_names: vec!["bump".to_string()],
                system_ms: vec![vec![1.0, 1.5]],
                frame_total_ms: vec![5.0, 6.0],
                extract_ms: vec![Some(0.5), Some(0.6)],
                submit_ms: vec![Some(0.3), None],
                pass_names: vec!["scene".to_string()],
                pass_ms: vec![vec![Some(2.0), Some(2.5)]],
            }),
        }
    }

    fn events(trace: &Value) -> Vec<&Value> {
        trace["traceEvents"].as_array().unwrap().iter().collect()
    }

    fn spans<'a>(events: &'a [&'a Value], tid: u64) -> Vec<&'a Value> {
        events
            .iter()
            .filter(|e| e["ph"] == "X" && e["tid"] == tid)
            .copied()
            .collect()
    }

    #[test]
    fn samples_absent_is_a_clear_error_not_a_synthetic_frame() {
        let mut report = two_frame_report();
        report.samples = None;
        let error = chrome_trace(&report).unwrap_err();
        assert!(error.contains("samples"), "{error}");
        assert!(error.contains("--attach"), "{error}");
    }

    #[test]
    fn lane_and_thread_name_metadata_are_assigned_correctly() {
        let trace = chrome_trace(&two_frame_report()).unwrap();
        let all = events(&trace);
        let meta: Vec<(u64, &str)> = all
            .iter()
            .filter(|e| e["ph"] == "M")
            .map(|e| {
                (
                    e["tid"].as_u64().unwrap(),
                    e["args"]["name"].as_str().unwrap(),
                )
            })
            .collect();
        assert_eq!(
            meta,
            vec![
                (TID_SIM, "sim systems"),
                (TID_HOST, "host"),
                (TID_GPU, "GPU passes"),
                (TID_TRANSPORT, "transport"),
            ]
        );
    }

    #[test]
    fn ms_to_us_is_exact_for_a_known_value() {
        // 1.5 ms is exactly 1500 us — the conversion must not drift under floating point.
        assert_eq!(ms_to_us(1.5), 1500.0);
        assert_eq!(ms_to_us(0.0), 0.0);
        assert_eq!(ms_to_us(16.667), 16_667.0);
    }

    #[test]
    fn sim_spans_are_butt_joined_within_a_frame() {
        let mut report = two_frame_report();
        report.samples.as_mut().unwrap().system_names =
            vec!["a".to_string(), "b".to_string(), "c".to_string()];
        report.samples.as_mut().unwrap().system_ms =
            vec![vec![1.0, 1.0], vec![2.0, 2.0], vec![3.0, 3.0]];
        let trace = chrome_trace(&report).unwrap();
        let all = events(&trace);
        let sim_spans = spans(&all, TID_SIM);
        // Frame 0's three spans: a[0,1000), b[1000,3000), c[3000,6000) in us.
        let frame0 = &sim_spans[0..3];
        assert_eq!(frame0[0]["name"], "a");
        assert_eq!(frame0[0]["ts"], 0.0);
        assert_eq!(frame0[0]["dur"], 1000.0);
        assert_eq!(frame0[1]["name"], "b");
        assert_eq!(frame0[1]["ts"], 1000.0, "system N+1 starts where N ends");
        assert_eq!(frame0[1]["dur"], 2000.0);
        assert_eq!(frame0[2]["name"], "c");
        assert_eq!(frame0[2]["ts"], 3000.0, "system N+1 starts where N ends");
        assert_eq!(frame0[2]["dur"], 3000.0);
    }

    #[test]
    fn frame_n_plus_one_starts_after_frame_ns_full_host_span() {
        // `two_frame_report`'s frame 0: frame_total 5.0ms, extract 0.5ms, submit 0.3ms — 5.8ms total.
        // Frame 1 must open there, not at 5.0ms (the bug this fix closes: advancing by `frame_total`
        // alone left frame 0's own extract/submit overlapping frame 1's `frame_total`).
        let trace = chrome_trace(&two_frame_report()).unwrap();
        let all = events(&trace);
        let host_spans = spans(&all, TID_HOST);
        let frame0_total = host_spans
            .iter()
            .find(|e| e["name"] == "frame_total" && e["ts"] == 0.0)
            .unwrap();
        assert_eq!(frame0_total["dur"], 5000.0);
        assert!(
            !host_spans
                .iter()
                .any(|e| e["name"] == "frame_total" && e["ts"] == 5000.0),
            "must not stay at the old, overlapping origin"
        );
        let frame1_total = host_spans
            .iter()
            .find(|e| e["name"] == "frame_total" && e["ts"] == 5800.0)
            .expect(
                "frame 1's frame_total must start where frame 0's frame_total + extract + submit ends",
            );
        assert_eq!(frame1_total["dur"], 6000.0);
    }

    #[test]
    fn no_span_overlaps_another_on_the_same_lane() {
        // The general form of the bug the previous test pins a specific case of: for every lane, every
        // span must start no earlier than the previous span on that lane ended. Mirrors
        // `editor/app/src/perf/trace.ts`'s own `never overlaps two spans on one lane` test.
        let trace = chrome_trace(&two_frame_report()).unwrap();
        let all = events(&trace);
        for tid in [TID_SIM, TID_HOST, TID_GPU, TID_TRANSPORT] {
            let mut lane = spans(&all, tid);
            lane.sort_by(|a, b| a["ts"].as_f64().partial_cmp(&b["ts"].as_f64()).unwrap());
            for pair in lane.windows(2) {
                let previous_end =
                    pair[0]["ts"].as_f64().unwrap() + pair[0]["dur"].as_f64().unwrap();
                let next_start = pair[1]["ts"].as_f64().unwrap();
                assert!(
                    next_start >= previous_end,
                    "tid {tid}: {:?} overlaps {:?}",
                    pair[0],
                    pair[1]
                );
            }
        }
    }

    #[test]
    fn a_gpu_lane_that_cannot_keep_up_queues_instead_of_overlapping() {
        // Two frames whose GPU work (9ms each) outlasts their sim frame_total (5ms and 6ms) — the
        // shape that would overlap under a per-frame-reset GPU cursor. Frame 1's GPU span must queue
        // behind frame 0's, not restart at frame 1's own (earlier) origin. Mirrors the TS test of the
        // same name.
        let mut report = two_frame_report();
        {
            let samples = report.samples.as_mut().unwrap();
            samples.pass_names = vec!["scene".to_string()];
            samples.pass_ms = vec![vec![Some(9.0), Some(9.0)]];
        }
        let trace = chrome_trace(&report).unwrap();
        let all = events(&trace);
        let gpu_spans = spans(&all, TID_GPU);
        assert_eq!(gpu_spans[0]["ts"], 0.0);
        assert_eq!(gpu_spans[0]["dur"], 9000.0);
        // Frame 1's own origin is 5.8ms, well before the GPU lane frees up at 9ms.
        assert_eq!(gpu_spans[1]["ts"], 9000.0);
    }

    #[test]
    fn multi_frame_concat_lays_out_every_frame_on_every_present_lane() {
        let trace = chrome_trace(&two_frame_report()).unwrap();
        let all = events(&trace);
        // Two frames, one system each -> two sim spans; two GPU pass samples -> two gpu spans.
        assert_eq!(spans(&all, TID_SIM).len(), 2);
        assert_eq!(spans(&all, TID_GPU).len(), 2);
        // Host: frame_total x2, extract x2, submit x1 (frame 1's submit is None) = 5.
        assert_eq!(spans(&all, TID_HOST).len(), 5);
        // Transport: encode x2, send x2 = 4.
        assert_eq!(spans(&all, TID_TRANSPORT).len(), 4);
    }

    #[test]
    fn host_extract_and_submit_are_omitted_per_frame_when_not_sampled() {
        let trace = chrome_trace(&two_frame_report()).unwrap();
        let all = events(&trace);
        let host_spans = spans(&all, TID_HOST);
        let submits: Vec<&&Value> = host_spans
            .iter()
            .filter(|e| e["name"] == "submit")
            .collect();
        assert_eq!(
            submits.len(),
            1,
            "frame 1's submit is None and must not appear"
        );
    }

    #[test]
    fn empty_gpu_degrades_to_an_absent_lane_not_an_error() {
        let mut report = two_frame_report();
        report.samples.as_mut().unwrap().pass_ms = vec![vec![None, None]];
        let trace = chrome_trace(&report).unwrap();
        let all = events(&trace);
        assert!(
            spans(&all, TID_GPU).is_empty(),
            "no GPU sample anywhere must produce no GPU spans"
        );
        assert!(
            !all.iter().any(|e| e["ph"] == "M" && e["tid"] == TID_GPU),
            "no GPU thread_name metadata when the lane is empty"
        );
    }

    #[test]
    fn no_transport_columns_degrades_to_an_absent_lane() {
        let mut report = two_frame_report();
        report.transport = None;
        let trace = chrome_trace(&report).unwrap();
        let all = events(&trace);
        assert!(spans(&all, TID_TRANSPORT).is_empty());
        assert!(
            !all.iter()
                .any(|e| e["ph"] == "M" && e["tid"] == TID_TRANSPORT)
        );
    }

    #[test]
    fn no_sim_systems_degrades_to_an_absent_lane() {
        let mut report = two_frame_report();
        report.samples.as_mut().unwrap().system_names = vec![];
        report.samples.as_mut().unwrap().system_ms = vec![];
        report.sim.systems = vec![];
        let trace = chrome_trace(&report).unwrap();
        let all = events(&trace);
        assert!(spans(&all, TID_SIM).is_empty());
        assert!(!all.iter().any(|e| e["ph"] == "M" && e["tid"] == TID_SIM));
        // The host lane still reports even with no sim systems.
        assert!(!spans(&all, TID_HOST).is_empty());
    }

    #[test]
    fn every_event_carries_the_one_process_id() {
        let trace = chrome_trace(&two_frame_report()).unwrap();
        for event in events(&trace) {
            assert_eq!(event["pid"], PID);
        }
    }
}
