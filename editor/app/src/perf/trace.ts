// Chrome trace-event export — the profile window, openable at ui.perfetto.dev.
//
// WHY THIS IS CLIENT-SIDE AND PURE. Export adds no capability: everything below is a reshaping of
// wire JSON the client already has, so it must not be a wire method (`Plan/engine-profiler.md`). The
// Rust CLI carries the same function over the same JSON; neither is the other's client. Lane ids,
// lane names, span names and categories are held identical to `crates/ludion-cli/src/perfetto.rs` on
// purpose, so the two exports are comparable side by side. THE TIMELINE RULE BELOW IS SHARED, not
// approximated twice: the CLI used to diverge on both points (advancing a frame's cursor by
// `frame_total` alone, and restarting the GPU cursor at each frame's own origin), which let a frame's
// host/GPU spans overlap the next frame's on the same `tid` — a malformed trace, not a busy one, since
// a viewer nests spans on a thread. The integration-fix wave ported this file's rule into the CLI
// (`crates/ludion-cli/src/perfetto.rs`'s own module docs carry the same numbered list) rather than the
// other way — this file's tests were the ones already pinning the non-overlapping shape.
//
// THE TIME AXIS IS RECONSTRUCTED, AND HERE IS EXACTLY HOW. The samples carry durations, never
// timestamps — the engine measures spans, it does not keep a shared clock — so a timeline has to be
// laid out. The rules, each an approximation stated rather than hidden:
//
//   1. **A frame's origin is the sum of every earlier frame's measured wall time**, and a frame's
//      wall time is `frame_total + extract + submit`. The two host spans are measured *beside* the
//      tick, not inside it (`SimProfile::sequential`), so they extend the frame rather than fitting in
//      it — advancing by `frame_total` alone would leave a frame's own `extract`/`submit` overlapping
//      the next frame's `frame_total` on the host lane. Gaps between frames — the host sleeping to
//      hold the tick rate — are NOT represented: the trace is a picture of the work, not of the wall
//      clock.
//   2. **Sim spans butt-join within their frame**, because the schedule is strictly sequential. The
//      schedule cannot outrun its own frame total, so a frame's systems never spill into the next.
//   3. **GPU and transport are aligned by frame index, and their lanes are monotonic.** A pass sample
//      is filed under the frame it measured, but it was collected one to three frames later, and the
//      work itself ran concurrently with a later CPU frame — the approximation with the most in it. A
//      frame's GPU/transport work starts at its own origin *or* when the previous frame's same-lane
//      work ended, whichever is later, so a lane that can't keep up reads as a queue (which is what it
//      is) instead of as overlapping spans a trace viewer cannot nest.
//
// A real difference that remains, deliberately: this export always annotates a span with its source
// `args: { frame }` and rounds `ts`/`dur` to nanosecond resolution to keep the JSON free of float
// noise; the CLI's spans carry neither (nor does it emit this file's `process_name`/`otherData`
// metadata). Both are shape choices for each export's own reader, not timeline-rule divergences, so
// they are not reconciled here.
//
// Pure: JSON in, JSON out. No DOM, no wire, no file system — the caller decides where the bytes go.

import type { SampleColumns, TransportSection, WindowRing } from './model';

/** One `ph:"X"` complete event or `ph:"M"` metadata record. */
export interface TraceEvent {
  readonly name: string;
  readonly ph: 'X' | 'M';
  readonly pid: number;
  readonly tid: number;
  readonly ts?: number;
  readonly dur?: number;
  /** The viewer's grouping category — `sim` · `host` · `gpu` · `transport`, matching the CLI's. */
  readonly cat?: string;
  readonly args?: Record<string, unknown>;
}

export interface ChromeTrace {
  readonly traceEvents: readonly TraceEvent[];
  readonly displayTimeUnit: 'ms';
  /** Free-form, ignored by viewers, read by humans opening the file in an editor. */
  readonly otherData: Record<string, string>;
}

/** One measured window: the columns, plus the delivery section the server may have filled. */
export interface TraceSection {
  readonly samples: SampleColumns;
  readonly transport?: TransportSection | null;
}

/** The lanes, fixed so a saved trace always reads the same way. */
export const LANE_SIM = 1;
export const LANE_HOST = 2;
export const LANE_GPU = 3;
export const LANE_TRANSPORT = 4;

const PID = 1;

/** Lane ids, display names and categories — kept byte-identical to `ludion-cli`'s `perfetto.rs`, so a
 *  trace saved here and one saved by `ludion profile --perfetto` read the same way in the viewer. */
const LANES: readonly (readonly [number, string, string])[] = [
  [LANE_SIM, 'sim systems', 'sim'],
  [LANE_HOST, 'host', 'host'],
  [LANE_GPU, 'GPU passes', 'gpu'],
  [LANE_TRANSPORT, 'transport', 'transport'],
];

const CATEGORY: Readonly<Record<number, string>> = Object.fromEntries(LANES.map(([tid, , cat]) => [tid, cat]));

/** Milliseconds to trace microseconds, at nanosecond resolution — enough for a 3 µs system, and it
 *  keeps the JSON free of float noise like `1666.6666666666667`. */
function us(ms: number): number {
  return Math.round(ms * 1_000_000) / 1000;
}

function span(name: string, tid: number, startMs: number, durationMs: number, frame: number): TraceEvent {
  return { name, ph: 'X', cat: CATEGORY[tid] ?? '', pid: PID, tid, ts: us(startMs), dur: us(durationMs), args: { frame } };
}

/**
 * Builds the trace for one or more measured windows, laid out end to end on one time axis.
 *
 * A section contributes nothing when its columns are empty; a lane's `thread_name` is emitted only
 * when that lane carries at least one span, so an adapter without timestamp support produces a trace
 * with three lanes rather than an empty fourth (the documented degradation, visible as an absence).
 */
export function chromeTrace(sections: readonly TraceSection[], scene?: string | null): ChromeTrace {
  const events: TraceEvent[] = [];
  const used = new Set<number>();
  let cursorMs = 0;
  let gpuCursorMs = 0;
  let transportCursorMs = 0;
  let frames = 0;

  for (const section of sections) {
    const s = section.samples;
    const columns = s.frames.length;
    for (let i = 0; i < columns; i += 1) {
      const frame = s.frames[i];
      const total = s.frame_total_ms[i] ?? 0;
      const extract = s.extract_ms[i] ?? 0;
      const submit = s.submit_ms[i] ?? 0;
      const origin = cursorMs;

      // Lane 1 — the schedule, butt-joined in registration order.
      let simCursor = origin;
      for (let row = 0; row < s.system_names.length; row += 1) {
        const value = s.system_ms[row]?.[i];
        if (value === null || value === undefined || value <= 0) {
          continue;
        }
        events.push(span(s.system_names[row], LANE_SIM, simCursor, value, frame));
        used.add(LANE_SIM);
        simCursor += value;
      }

      // Lane 2 — the frame the schedule ran in, then the two host spans that extend it.
      events.push(span('frame_total', LANE_HOST, origin, total, frame));
      used.add(LANE_HOST);
      if (extract > 0) {
        events.push(span('extract', LANE_HOST, origin + total, extract, frame));
      }
      if (submit > 0) {
        events.push(span('submit', LANE_HOST, origin + total + extract, submit, frame));
      }

      // Lane 3 — the passes this frame's samples were filed under, queued behind the previous
      // frame's GPU work (rule 3).
      let gpuCursor = Math.max(origin, gpuCursorMs);
      for (let row = 0; row < s.pass_names.length; row += 1) {
        const value = s.pass_ms[row]?.[i];
        if (value === null || value === undefined || value <= 0) {
          continue;
        }
        events.push(span(s.pass_names[row], LANE_GPU, gpuCursor, value, frame));
        used.add(LANE_GPU);
        gpuCursor += value;
      }
      gpuCursorMs = gpuCursor;

      // Lane 4 — delivery, same alignment and the same queue rule.
      const transport = section.transport;
      const encode = transport?.encode_ms?.[i] ?? null;
      const send = transport?.send_ms?.[i] ?? null;
      let transportCursor = Math.max(origin, transportCursorMs);
      if (encode !== null && encode > 0) {
        events.push(span('encode', LANE_TRANSPORT, transportCursor, encode, frame));
        used.add(LANE_TRANSPORT);
        transportCursor += encode;
      }
      if (send !== null && send > 0) {
        events.push(span('send', LANE_TRANSPORT, transportCursor, send, frame));
        used.add(LANE_TRANSPORT);
        transportCursor += send;
      }
      transportCursorMs = transportCursor;

      cursorMs = origin + total + extract + submit;
      frames += 1;
    }
  }

  const metadata: TraceEvent[] = [
    { name: 'process_name', ph: 'M', pid: PID, tid: 0, args: { name: scene == null || scene === '' ? 'ludion' : `ludion · ${scene}` } },
    ...LANES.filter(([tid]) => used.has(tid)).map(([tid, name]) => ({
      name: 'thread_name',
      ph: 'M' as const,
      pid: PID,
      tid,
      args: { name },
    })),
  ];

  return {
    traceEvents: [...metadata, ...events],
    displayTimeUnit: 'ms',
    otherData: {
      source: 'ludion editor · Performance view',
      scene: scene == null ? '' : scene,
      frames: String(frames),
      alignment: 'sim spans butt-joined per frame; GPU and transport aligned by frame index and queued monotonically',
    },
  };
}

/** The rolling ring as a trace — every window the view still holds, in frame order. */
export function traceFromRing(ring: WindowRing, scene?: string | null): ChromeTrace {
  return chromeTrace(
    ring.windows.map((w) => ({ samples: w.samples, transport: w.transport })),
    scene,
  );
}

/** A pinned `profile.capture` report as a trace. A report requested without `include_samples` has no
 *  timeline in it, and yields a valid trace with no spans rather than an error. */
export function traceFromReport(
  report: { readonly scene?: string; readonly samples?: SampleColumns | null; readonly transport?: TransportSection | null },
): ChromeTrace {
  const samples = report.samples;
  return chromeTrace(samples == null ? [] : [{ samples, transport: report.transport }], report.scene ?? null);
}
