// The Performance view's domain: profile windows in, a drawable timeline out.
//
// WHY THIS IS A MODULE AND NOT THE VIEW. vitest's `include` is `src/**/*.test.ts`, so a `.tsx` is
// untestable here (the `chain.ts` convention). Everything with a rule in it —
// which windows are kept, how names become series, what a stacked column measures, where the pixels
// go — lives here and is exercised against object literals. `TimelineCanvas.tsx` only draws what
// `layoutTimeline` hands it, and `PerformanceView.tsx` only wires it to the wire.
//
// THE WINDOWS ARE FRAME-DRIVEN, NOT TIME-DRIVEN. A `profile` window closes after
// `PROFILE_WINDOW_FRAMES` sim frames, so a paused sim emits nothing at all and a slow one emits
// slowly. Nothing here interpolates over that gap: the x axis is the frame axis, one column per
// measured frame, and a stall shows up as the timeline simply not advancing.
//
// CPU AND GPU ARE NEVER STACKED TOGETHER. They are concurrent, so their sum measures nothing; the
// view picks one lane and this module stacks within it. (The dual-axis mistake in stacked clothing.)

/**
 * The columnar per-frame block carried by both a `profile` window event and a `profile.capture`
 * report (`ProfileSamples`). Declared structurally rather than imported: the generated types mint a
 * separate interface per response arm (`ProfileSamples` / `ProfileSamples1`) for one wire shape, and
 * every consumer here wants whichever one it was handed.
 */
export interface SampleColumns {
  /** The frame each column entry belongs to, ascending. */
  readonly frames: readonly number[];
  readonly frame_total_ms: readonly number[];
  /** `null` for a frame the host did not render. */
  readonly extract_ms: readonly (number | null)[];
  readonly submit_ms: readonly (number | null)[];
  readonly system_names: readonly string[];
  /** One row per system name, each a per-frame array. */
  readonly system_ms: readonly (readonly number[])[];
  readonly pass_names: readonly string[];
  /** One row per pass name; `null` where that frame carried no GPU sample. */
  readonly pass_ms: readonly (readonly (number | null)[])[];
}

/** One name's median over the measured frames (`SystemTiming`). */
export interface Median {
  readonly name: string;
  readonly median_ms: number;
}

/** A pass median, which is `null` on an adapter without `TIMESTAMP_QUERY` (`PassTiming`). */
export interface PassMedian {
  readonly name: string;
  readonly median_ms?: number | null;
}

/** The window's GPU half (`LiveGpuProfile`). */
export interface GpuSection {
  readonly passes: readonly PassMedian[];
  readonly sampled_frames: number;
  readonly timestamps_supported: boolean;
  /** Median frame-spanning GPU time; `null` where no span could be proved. Absent in older payloads. */
  readonly frame_median_ms?: number | null;
}

/** The server-filled delivery section (`TransportProfile`), absent until a frame reaches a subscriber. */
export interface TransportSection {
  readonly encode_median_ms: number;
  readonly send_median_ms: number;
  readonly sampled_frames: number;
  readonly encode_ms?: readonly (number | null)[];
  readonly send_ms?: readonly (number | null)[];
}

/** One closed window of live frames (`ProfileWindowData`), structurally. */
export interface PerfWindow {
  readonly capture: boolean;
  readonly first_frame: number;
  readonly last_frame: number;
  readonly frames: number;
  readonly frame_total_median_ms: number;
  /** Sim worker threads the host measured on. Absent from a window predating the field, which is
   *  why the readout omits it rather than claiming a count. */
  readonly threads?: number;
  readonly systems: readonly Median[];
  readonly sequential: readonly Median[];
  readonly gpu: GpuSection;
  readonly samples: SampleColumns;
  readonly transport?: TransportSection | null;
}

/**
 * How many windows the ring keeps: 180 windows of 60 frames is 10 800 frames — three minutes at the
 * default tick rate, which is the span a human watching a scene run can actually reason about. Older
 * windows fall off the front; nothing is aggregated on the way out.
 */
export const MAX_WINDOWS = 180;

/** One rendered frame's worth of time at 60 Hz — the line the timeline draws and the stack is read
 *  against. The label rounds to `16.6`; the arithmetic never does. */
export const FRAME_BUDGET_MS = 1000 / 60;

/** The retained windows, oldest first. Immutable: `pushWindow` returns a new ring, so React state
 *  identity changes exactly when the data does. */
export interface WindowRing {
  readonly windows: readonly PerfWindow[];
}

export function emptyRing(): WindowRing {
  return { windows: [] };
}

/**
 * Appends a window, dropping the oldest past {@link MAX_WINDOWS}.
 *
 * Two windows are refused rather than appended:
 *  - a **capture** window, which measures frames the rolling stream also measures — keeping it would
 *    draw the same frames twice, on top of each other;
 *  - one that does not start after the newest kept window, which is a duplicate or a reordered
 *    delivery. The frame axis is only meaningful ascending.
 */
export function pushWindow(ring: WindowRing, window: PerfWindow): WindowRing {
  if (window.capture) {
    return ring;
  }
  const newest = ring.windows.at(-1);
  if (newest !== undefined && window.first_frame <= newest.first_frame) {
    return ring;
  }
  const windows = [...ring.windows, window];
  return { windows: windows.length > MAX_WINDOWS ? windows.slice(windows.length - MAX_WINDOWS) : windows };
}

/** What a series measures. The kinds never share a stack — see the module header. */
export type SeriesKind = 'system' | 'sequential' | 'pass' | 'transport' | 'total';

/** The lanes the timeline can draw, each one stack of same-kind series. */
export type Lane = 'sim' | 'gpu' | 'transport';

/** One measured name across every retained frame. `values` is aligned to {@link SeriesSet.frames};
 *  `null` is "not measured this frame", which is not the same as zero. */
export interface Series {
  readonly key: string;
  readonly name: string;
  readonly kind: SeriesKind;
  readonly values: readonly (number | null)[];
  /** Median over the non-null values — what ranks a series for folding and for the legend. */
  readonly medianMs: number;
}

export interface SeriesSet {
  /** Every retained frame index, ascending. */
  readonly frames: readonly number[];
  readonly series: readonly Series[];
}

/** The key a series is identified by, everywhere: kind-qualified, because a system and a pass may
 *  legitimately share a name. */
export function seriesKey(kind: SeriesKind, name: string): string {
  return `${kind}:${name}`;
}

/** The frame-total series' key — the one series that is a line rather than a band. */
export const TOTAL_KEY = seriesKey('total', 'frame');

interface Accumulator {
  readonly key: string;
  readonly name: string;
  readonly kind: SeriesKind;
  readonly values: (number | null)[];
}

/**
 * Flattens the ring into one column per measured frame and one series per measured name.
 *
 * Names come and go between windows (a pass list changes with the frame graph; a transport section
 * appears only once a frame reaches a subscriber). A name absent from a window contributes `null`
 * for that window's frames, so a series' own gap is visible instead of being closed by neighbours.
 * Series order is first-seen order across the whole ring, which is what makes a colour stable while
 * the ranking underneath it moves.
 */
export function extractSeries(ring: WindowRing): SeriesSet {
  const frames: number[] = [];
  const order: string[] = [];
  const acc = new Map<string, Accumulator>();

  const at = (kind: SeriesKind, name: string): Accumulator => {
    const key = seriesKey(kind, name);
    let entry = acc.get(key);
    if (entry === undefined) {
      entry = { key, name, kind, values: [] };
      acc.set(key, entry);
      order.push(key);
    }
    return entry;
  };

  // `total` is registered up front so it is never absent from a non-empty set, whatever a window
  // happens to carry.
  if (ring.windows.length > 0) {
    at('total', 'frame');
  }

  for (const window of ring.windows) {
    const s = window.samples;
    const count = s.frames.length;
    const base = frames.length;
    for (const frame of s.frames) {
      frames.push(frame);
    }

    const put = (kind: SeriesKind, name: string, read: (i: number) => number | null): void => {
      const entry = at(kind, name);
      // Pad any window this name missed, then write its own column run.
      while (entry.values.length < base) {
        entry.values.push(null);
      }
      for (let i = 0; i < count; i += 1) {
        entry.values.push(read(i));
      }
    };

    put('total', 'frame', (i) => s.frame_total_ms[i] ?? null);
    s.system_names.forEach((name, row) => {
      const values = s.system_ms[row];
      put('system', name, (i) => values?.[i] ?? null);
    });
    put('sequential', 'extract', (i) => s.extract_ms[i] ?? null);
    put('sequential', 'submit', (i) => s.submit_ms[i] ?? null);
    s.pass_names.forEach((name, row) => {
      const values = s.pass_ms[row];
      put('pass', name, (i) => values?.[i] ?? null);
    });
    const transport = window.transport;
    if (transport != null) {
      const encode = transport.encode_ms;
      const send = transport.send_ms;
      if (encode !== undefined && encode.length > 0) {
        put('transport', 'encode', (i) => encode[i] ?? null);
      }
      if (send !== undefined && send.length > 0) {
        put('transport', 'send', (i) => send[i] ?? null);
      }
    }
  }

  const total = frames.length;
  const series = order.map((key) => {
    const entry = acc.get(key) as Accumulator;
    while (entry.values.length < total) {
      entry.values.push(null);
    }
    return {
      key: entry.key,
      name: entry.name,
      kind: entry.kind,
      values: entry.values,
      medianMs: median(entry.values),
    };
  });

  return { frames, series };
}

/** The median of the non-null values, or `0` when there are none. */
export function median(values: readonly (number | null)[]): number {
  const present: number[] = [];
  for (const v of values) {
    if (v !== null && Number.isFinite(v)) {
      present.push(v);
    }
  }
  if (present.length === 0) {
    return 0;
  }
  present.sort((a, b) => a - b);
  const mid = present.length >> 1;
  return present.length % 2 === 1 ? present[mid] : (present[mid - 1] + present[mid]) / 2;
}

/** The kinds each lane stacks. `sim` is the tick plus the two host spans beside it — the sequential
 *  spans are not part of `frame_total_ms`, so stacking them together is the whole frame's CPU cost. */
export const LANE_KINDS: Readonly<Record<Lane, readonly SeriesKind[]>> = {
  sim: ['system', 'sequential'],
  gpu: ['pass'],
  transport: ['transport'],
};

/** The series a lane stacks, ranked by median (biggest at the bottom of the stack). */
export function laneSeries(set: SeriesSet, lane: Lane): readonly Series[] {
  const kinds = LANE_KINDS[lane];
  return set.series
    .filter((s) => kinds.includes(s.kind) && s.values.some((v) => v !== null))
    .slice()
    .sort((a, b) => b.medianMs - a.medianMs || a.key.localeCompare(b.key));
}

/** The key the folded remainder takes. Its colour is fixed, never a palette slot. */
export const OTHER_KEY = 'other:rest';

/**
 * Caps a lane at `limit` bands by folding everything past the top `limit - 1` into one `Other`
 * series that carries their sum.
 *
 * The cap is the categorical palette's size, not a layout preference: a ninth hue is a hue nobody
 * validated, so the honest ninth band is "everything else" (the data-viz rule — a series past the
 * palette folds, it never gets a generated colour).
 */
export function foldSeries(series: readonly Series[], limit: number): readonly Series[] {
  if (limit < 1 || series.length <= limit) {
    return series;
  }
  const kept = series.slice(0, limit - 1);
  const rest = series.slice(limit - 1);
  const length = series[0]?.values.length ?? 0;
  const values: (number | null)[] = [];
  for (let i = 0; i < length; i += 1) {
    let sum: number | null = null;
    for (const s of rest) {
      const v = s.values[i];
      if (v !== null && v !== undefined) {
        sum = (sum ?? 0) + v;
      }
    }
    values.push(sum);
  }
  return [
    ...kept,
    {
      key: OTHER_KEY,
      name: `Other (${rest.length})`,
      kind: rest[0]?.kind ?? 'system',
      values,
      medianMs: median(values),
    },
  ];
}

/** One stacked band, already in canvas pixels: `top`/`bottom` per drawn column, `top === bottom`
 *  where the column has no sample. */
export interface BandLayout {
  readonly key: string;
  readonly name: string;
  readonly top: readonly number[];
  readonly bottom: readonly number[];
}

/** Everything `TimelineCanvas` needs, in device-independent pixels relative to the plot's own box. */
export interface TimelineLayout {
  /** Left edge of each drawn column. */
  readonly x: readonly number[];
  readonly columnWidth: number;
  /** The y-scale's top, in ms. */
  readonly maxMs: number;
  readonly height: number;
  /** The 16.6 ms line's y, or `null` when the scale does not reach it. */
  readonly guideY: number | null;
  readonly bands: readonly BandLayout[];
  /** The frame-total polyline, `null` per column where the frame was not measured. */
  readonly totalY: readonly (number | null)[];
  /** Index into {@link SeriesSet.frames} of the first drawn column — the rest scrolled off the left. */
  readonly offset: number;
  readonly firstFrame: number | null;
  readonly lastFrame: number | null;
}

export interface LayoutOptions {
  readonly width: number;
  readonly height: number;
  /** Narrowest a column may get before older frames scroll off instead. */
  readonly minColumnPx?: number;
  /** The guide line, in ms. */
  readonly guideMs?: number;
}

/** Below this the stack is drawn solid: a 1px separator inside a 2px band erases the band. */
const SEPARATOR_MIN_PX = 3;

/**
 * Turns a lane's series into pixels.
 *
 * The scale's top is a *nice* number at or above both the tallest stack and the guide, so the guide
 * is always on screen (a budget line you cannot see cannot be missed) and the axis label is a number
 * a human reads rather than the max sample's decimals. Columns fill the width; when there are more
 * frames than the width can hold at `minColumnPx`, the OLDEST scroll off — the newest frame is
 * always the rightmost one.
 */
export function layoutTimeline(
  set: SeriesSet,
  series: readonly Series[],
  options: LayoutOptions,
): TimelineLayout {
  const { width, height } = options;
  const minColumnPx = options.minColumnPx ?? 2;
  const guideMs = options.guideMs ?? FRAME_BUDGET_MS;
  const empty: TimelineLayout = {
    x: [],
    columnWidth: 0,
    maxMs: niceCeil(guideMs),
    height,
    guideY: null,
    bands: [],
    totalY: [],
    offset: 0,
    firstFrame: null,
    lastFrame: null,
  };
  if (width <= 0 || height <= 0 || set.frames.length === 0) {
    return { ...empty, guideY: height > 0 ? height - (guideMs / niceCeil(guideMs)) * height : null };
  }

  const capacity = Math.max(1, Math.floor(width / minColumnPx));
  const columns = Math.min(set.frames.length, capacity);
  const offset = set.frames.length - columns;
  const columnWidth = width / columns;

  let peak = 0;
  for (let i = offset; i < set.frames.length; i += 1) {
    let sum = 0;
    for (const s of series) {
      sum += s.values[i] ?? 0;
    }
    if (sum > peak) {
      peak = sum;
    }
  }
  const totalSeries = set.series.find((s) => s.key === TOTAL_KEY);
  if (totalSeries !== undefined) {
    for (let i = offset; i < set.frames.length; i += 1) {
      const v = totalSeries.values[i];
      if (v !== null && v !== undefined && v > peak) {
        peak = v;
      }
    }
  }
  const maxMs = niceCeil(Math.max(peak, guideMs));
  const scale = height / maxMs;

  const x: number[] = [];
  for (let c = 0; c < columns; c += 1) {
    x.push(c * columnWidth);
  }

  const cursor = new Array<number>(columns).fill(height);
  const bands: BandLayout[] = series.map((s) => {
    const top: number[] = [];
    const bottom: number[] = [];
    for (let c = 0; c < columns; c += 1) {
      const v = s.values[offset + c];
      const base = cursor[c];
      if (v === null || v === undefined || v <= 0) {
        top.push(base);
        bottom.push(base);
        continue;
      }
      const raw = v * scale;
      const y = Math.max(0, base - raw);
      // A hairline of surface between stacked segments, but only where the band can spare it.
      const gap = raw >= SEPARATOR_MIN_PX ? 1 : 0;
      top.push(y);
      bottom.push(Math.max(y, base - gap));
      cursor[c] = y;
    }
    return { key: s.key, name: s.name, top, bottom };
  });

  const totalY: (number | null)[] = [];
  for (let c = 0; c < columns; c += 1) {
    const v = totalSeries?.values[offset + c];
    totalY.push(v === null || v === undefined ? null : clamp(height - v * scale, 0, height));
  }

  return {
    x,
    columnWidth,
    maxMs,
    height,
    guideY: clamp(height - guideMs * scale, 0, height),
    bands,
    totalY,
    offset,
    firstFrame: set.frames[offset] ?? null,
    lastFrame: set.frames.at(-1) ?? null,
  };
}

function clamp(v: number, lo: number, hi: number): number {
  return v < lo ? lo : v > hi ? hi : v;
}

/** The smallest 1/2/5×10^k at or above `v` — the y axis' top, so its label is a number and not a
 *  measurement. */
export function niceCeil(v: number): number {
  if (!Number.isFinite(v) || v <= 0) {
    return 1;
  }
  const exponent = Math.floor(Math.log10(v));
  const magnitude = 10 ** exponent;
  const normalized = v / magnitude;
  const step = normalized <= 1 ? 1 : normalized <= 2 ? 2 : normalized <= 5 ? 5 : 10;
  return Number((step * magnitude).toPrecision(12));
}

/** The drawn column under a pointer at `x`, or `null` outside the plot. */
export function columnAt(layout: TimelineLayout, x: number): number | null {
  if (layout.columnWidth <= 0 || layout.x.length === 0) {
    return null;
  }
  const column = Math.floor(x / layout.columnWidth);
  return column >= 0 && column < layout.x.length ? column : null;
}

/** One column's readout for the hover tooltip: the frame it measured and each band's value. */
export interface ColumnReadout {
  readonly frame: number;
  readonly totalMs: number | null;
  readonly entries: readonly { readonly key: string; readonly name: string; readonly ms: number }[];
  readonly stackedMs: number;
}

export function readColumn(
  set: SeriesSet,
  series: readonly Series[],
  layout: TimelineLayout,
  column: number,
): ColumnReadout | null {
  const index = layout.offset + column;
  const frame = set.frames[index];
  if (frame === undefined) {
    return null;
  }
  const entries: { key: string; name: string; ms: number }[] = [];
  let stackedMs = 0;
  for (const s of series) {
    const v = s.values[index];
    if (v !== null && v !== undefined) {
      entries.push({ key: s.key, name: s.name, ms: v });
      stackedMs += v;
    }
  }
  entries.sort((a, b) => b.ms - a.ms);
  const total = set.series.find((s) => s.key === TOTAL_KEY)?.values[index];
  return { frame, totalMs: total ?? null, entries, stackedMs };
}

// --- the latest window's breakdown ---------------------------------------------------------------

export interface BreakdownEntry {
  readonly key: string;
  readonly name: string;
  readonly ms: number;
  /** The entry's share of its own group, `0..1` — the bar's length. */
  readonly share: number;
}

export interface BreakdownGroup {
  readonly id: 'systems' | 'sequential' | 'passes' | 'transport';
  readonly label: string;
  readonly totalMs: number;
  readonly entries: readonly BreakdownEntry[];
  /** A quiet line under the group's head: how much of the window it covers, or why it is empty. */
  readonly note: string | null;
}

export interface Breakdown {
  readonly frames: number;
  readonly firstFrame: number;
  readonly lastFrame: number;
  readonly frameTotalMs: number;
  /** The sim tick plus its sequential spans — what one frame costs the CPU. */
  readonly cpuMs: number;
  /** Sim worker threads the host measured on, or `null` when the window did not carry a count.
   *  Every duration on the panel is relative to it, so two captures at different counts are not
   *  comparable — which is why it is shown rather than assumed. */
  readonly threads: number | null;
  readonly timestampsSupported: boolean;
  readonly groups: readonly BreakdownGroup[];
}

/**
 * The newest window, reshaped into the bars the panel draws.
 *
 * Medians, not sums: the window's own statistic. Each group is ranked by cost and folded at `limit`,
 * so the panel answers "where does the frame go" in reading order — and an entry keeps its colour
 * when the ranking moves, because the colour follows the name (`colors.ts`), never the row.
 *
 * The GPU group is omitted entirely on an adapter without timestamp support: every median there is
 * `null`, and a bar chart of nulls is worse than a sentence saying so.
 */
export function latestBreakdown(ring: WindowRing, limit: number): Breakdown | null {
  const window = ring.windows.at(-1);
  if (window === undefined) {
    return null;
  }
  const sequentialMs = window.sequential.reduce((sum, s) => sum + s.median_ms, 0);
  const groups: BreakdownGroup[] = [
    group('systems', 'Systems', window.systems.map((s) => ({ key: seriesKey('system', s.name), name: s.name, ms: s.median_ms })), limit, null),
    group(
      'sequential',
      'Host spans',
      window.sequential.map((s) => ({ key: seriesKey('sequential', s.name), name: s.name, ms: s.median_ms })),
      limit,
      'measured beside the tick, not inside it',
    ),
  ];

  if (window.gpu.timestamps_supported) {
    groups.push(
      group(
        'passes',
        'GPU passes',
        window.gpu.passes.map((p) => ({ key: seriesKey('pass', p.name), name: p.name, ms: p.median_ms ?? 0 })),
        limit,
        `${window.gpu.sampled_frames} of ${window.frames} frames sampled` +
          (window.gpu.frame_median_ms == null ? '' : ` · GPU frame ${formatMs(window.gpu.frame_median_ms)} ms`),
      ),
    );
  }

  const transport = window.transport;
  if (transport != null) {
    groups.push(
      group(
        'transport',
        'Transport',
        [
          { key: seriesKey('transport', 'encode'), name: 'encode', ms: transport.encode_median_ms },
          { key: seriesKey('transport', 'send'), name: 'send', ms: transport.send_median_ms },
        ],
        limit,
        `${transport.sampled_frames} of ${window.frames} frames delivered`,
      ),
    );
  }

  return {
    frames: window.frames,
    firstFrame: window.first_frame,
    lastFrame: window.last_frame,
    frameTotalMs: window.frame_total_median_ms,
    cpuMs: window.frame_total_median_ms + sequentialMs,
    threads: window.threads ?? null,
    timestampsSupported: window.gpu.timestamps_supported,
    groups,
  };
}

function group(
  id: BreakdownGroup['id'],
  label: string,
  raw: readonly { key: string; name: string; ms: number }[],
  limit: number,
  note: string | null,
): BreakdownGroup {
  const ranked = raw.slice().sort((a, b) => b.ms - a.ms || a.name.localeCompare(b.name));
  const kept = ranked.length > limit ? ranked.slice(0, limit - 1) : ranked;
  const folded = ranked.length > limit ? ranked.slice(limit - 1) : [];
  if (folded.length > 0) {
    kept.push({ key: OTHER_KEY, name: `Other (${folded.length})`, ms: folded.reduce((s, e) => s + e.ms, 0) });
  }
  const totalMs = ranked.reduce((sum, e) => sum + e.ms, 0);
  const peak = kept.reduce((max, e) => Math.max(max, e.ms), 0);
  return {
    id,
    label,
    totalMs,
    // The bar is read against the group's biggest entry, not its sum: shares of a sum turn every
    // group with one dominant entry into a row of stubs.
    entries: kept.map((e) => ({ ...e, share: peak > 0 ? e.ms / peak : 0 })),
    note,
  };
}

/** `4.21` — two decimals is one microsecond of a 16.6 ms frame, which is past what any of these
 *  measurements resolves. */
export function formatMs(ms: number | null | undefined): string {
  return ms === null || ms === undefined || !Number.isFinite(ms) ? '—' : ms.toFixed(2);
}
