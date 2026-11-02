import { describe, expect, it } from 'vitest';
import {
  columnAt,
  emptyRing,
  extractSeries,
  foldSeries,
  formatMs,
  FRAME_BUDGET_MS,
  laneSeries,
  latestBreakdown,
  layoutTimeline,
  MAX_WINDOWS,
  median,
  niceCeil,
  OTHER_KEY,
  pushWindow,
  readColumn,
  seriesKey,
  TOTAL_KEY,
  type PerfWindow,
  type SampleColumns,
  type Series,
  type SeriesSet,
} from './model';

/** A window whose columns are consistent with its head — the shape the wire guarantees. */
function makeWindow(overrides: Partial<PerfWindow> & { first_frame: number }): PerfWindow {
  const first = overrides.first_frame;
  const count = overrides.frames ?? 3;
  const frames = Array.from({ length: count }, (_, i) => first + i);
  const samples: SampleColumns = {
    frames,
    frame_total_ms: frames.map(() => 4),
    extract_ms: frames.map(() => 1),
    submit_ms: frames.map(() => 0.5),
    system_names: ['enemy_ai', 'move_projectiles'],
    system_ms: [frames.map(() => 3), frames.map(() => 0.5)],
    pass_names: ['scene', 'composite'],
    pass_ms: [frames.map(() => 2), frames.map(() => 0.4)],
    ...(overrides.samples ?? {}),
  };
  return {
    capture: false,
    last_frame: first + count - 1,
    frames: count,
    frame_total_median_ms: 4,
    threads: 8,
    systems: [
      { name: 'enemy_ai', median_ms: 3 },
      { name: 'move_projectiles', median_ms: 0.5 },
    ],
    sequential: [
      { name: 'extract', median_ms: 1 },
      { name: 'submit', median_ms: 0.5 },
    ],
    gpu: {
      passes: [
        { name: 'scene', median_ms: 2 },
        { name: 'composite', median_ms: 0.4 },
      ],
      sampled_frames: count,
      timestamps_supported: true,
    },
    ...overrides,
    samples,
  };
}

describe('the window ring', () => {
  it('keeps windows in arrival order and caps at MAX_WINDOWS, dropping the oldest', () => {
    let ring = emptyRing();
    for (let i = 0; i < MAX_WINDOWS + 5; i += 1) {
      ring = pushWindow(ring, makeWindow({ first_frame: i * 3 }));
    }
    expect(ring.windows).toHaveLength(MAX_WINDOWS);
    expect(ring.windows[0].first_frame).toBe(5 * 3);
    expect(ring.windows.at(-1)?.first_frame).toBe((MAX_WINDOWS + 4) * 3);
  });

  it('refuses a capture window — it measures frames the rolling stream already measured', () => {
    const ring = pushWindow(emptyRing(), makeWindow({ first_frame: 0, capture: true }));
    expect(ring.windows).toHaveLength(0);
  });

  it('refuses a duplicate or reordered window: the frame axis only means anything ascending', () => {
    let ring = pushWindow(emptyRing(), makeWindow({ first_frame: 30 }));
    ring = pushWindow(ring, makeWindow({ first_frame: 30 }));
    ring = pushWindow(ring, makeWindow({ first_frame: 27 }));
    expect(ring.windows).toHaveLength(1);
    ring = pushWindow(ring, makeWindow({ first_frame: 33 }));
    expect(ring.windows.map((w) => w.first_frame)).toEqual([30, 33]);
  });

  it('does not mutate the ring it was given', () => {
    const first = emptyRing();
    const second = pushWindow(first, makeWindow({ first_frame: 0 }));
    expect(first.windows).toHaveLength(0);
    expect(second).not.toBe(first);
  });
});

describe('median', () => {
  it('ignores nulls, averages an even count, and answers 0 for nothing measured', () => {
    expect(median([1, 2, 3])).toBe(2);
    expect(median([1, null, 3, 5])).toBe(3);
    expect(median([2, 4])).toBe(3);
    expect(median([])).toBe(0);
    expect(median([null, null])).toBe(0);
  });
});

describe('extractSeries', () => {
  it('concatenates windows into one frame axis with one series per measured name', () => {
    let ring = pushWindow(emptyRing(), makeWindow({ first_frame: 0 }));
    ring = pushWindow(ring, makeWindow({ first_frame: 3 }));
    const set = extractSeries(ring);

    expect(set.frames).toEqual([0, 1, 2, 3, 4, 5]);
    const names = set.series.map((s) => s.key);
    expect(names).toContain(TOTAL_KEY);
    expect(names).toContain(seriesKey('system', 'enemy_ai'));
    expect(names).toContain(seriesKey('sequential', 'extract'));
    expect(names).toContain(seriesKey('pass', 'scene'));
    const ai = set.series.find((s) => s.key === seriesKey('system', 'enemy_ai'));
    expect(ai?.values).toEqual([3, 3, 3, 3, 3, 3]);
    expect(ai?.medianMs).toBe(3);
  });

  it('pads a name that only some windows carry, so its own gap stays visible', () => {
    let ring = pushWindow(emptyRing(), makeWindow({ first_frame: 0 }));
    ring = pushWindow(
      ring,
      makeWindow({
        first_frame: 3,
        samples: {
          frames: [3, 4, 5],
          frame_total_ms: [4, 4, 4],
          extract_ms: [1, 1, 1],
          submit_ms: [0.5, 0.5, 0.5],
          system_names: ['enemy_ai', 'quest_clock'],
          system_ms: [
            [3, 3, 3],
            [0.2, 0.2, 0.2],
          ],
          pass_names: [],
          pass_ms: [],
        },
      }),
    );
    const set = extractSeries(ring);

    // Present only in the second window: null across the first.
    expect(set.series.find((s) => s.key === seriesKey('system', 'quest_clock'))?.values).toEqual([
      null,
      null,
      null,
      0.2,
      0.2,
      0.2,
    ]);
    // Present only in the first: padded on the tail.
    expect(set.series.find((s) => s.key === seriesKey('pass', 'scene'))?.values).toEqual([2, 2, 2, null, null, null]);
    // Every series is the same length as the frame axis.
    for (const s of set.series) {
      expect(s.values).toHaveLength(set.frames.length);
    }
  });

  it('carries transport only when the server filled per-frame columns', () => {
    const withoutColumns = pushWindow(
      emptyRing(),
      makeWindow({
        first_frame: 0,
        transport: { encode_median_ms: 12, send_median_ms: 0.4, sampled_frames: 3 },
      }),
    );
    expect(extractSeries(withoutColumns).series.some((s) => s.kind === 'transport')).toBe(false);

    const withColumns = pushWindow(
      emptyRing(),
      makeWindow({
        first_frame: 0,
        transport: {
          encode_median_ms: 12,
          send_median_ms: 0.4,
          sampled_frames: 3,
          encode_ms: [12, null, 11],
          send_ms: [0.4, null, 0.5],
        },
      }),
    );
    const encode = extractSeries(withColumns).series.find((s) => s.key === seriesKey('transport', 'encode'));
    expect(encode?.values).toEqual([12, null, 11]);
    expect(encode?.medianMs).toBe(11.5);
  });

  it('answers an empty set for an empty ring', () => {
    expect(extractSeries(emptyRing())).toEqual({ frames: [], series: [] });
  });
});

describe('lanes and folding', () => {
  const set = extractSeries(pushWindow(emptyRing(), makeWindow({ first_frame: 0 })));

  it('stacks only same-kind series, ranked by median', () => {
    // enemy_ai 3 · extract 1 · then the 0.5 ms tie broken by key, so `sequential:submit` leads.
    expect(laneSeries(set, 'sim').map((s) => s.name)).toEqual(['enemy_ai', 'extract', 'submit', 'move_projectiles']);
    expect(laneSeries(set, 'gpu').map((s) => s.name)).toEqual(['scene', 'composite']);
    expect(laneSeries(set, 'transport')).toEqual([]);
  });

  it('folds past the palette into one Other band carrying the sum', () => {
    const many: Series[] = Array.from({ length: 5 }, (_, i) => ({
      key: `system:s${i}`,
      name: `s${i}`,
      kind: 'system' as const,
      values: [i + 1, null],
      medianMs: i + 1,
    })).reverse();
    const folded = foldSeries(many, 3);
    expect(folded.map((s) => s.key)).toEqual(['system:s4', 'system:s3', OTHER_KEY]);
    // 3 + 2 + 1 for the first column; every folded value null leaves the fold null, not zero.
    expect(folded[2].values).toEqual([6, null]);
    expect(folded[2].name).toBe('Other (3)');
  });

  it('leaves a lane that fits the palette untouched', () => {
    const lane = laneSeries(set, 'sim');
    expect(foldSeries(lane, 8)).toBe(lane);
  });
});

describe('niceCeil', () => {
  it('rounds up to a 1/2/5 decade so the axis label is a number, not a measurement', () => {
    expect(niceCeil(FRAME_BUDGET_MS)).toBe(20);
    expect(niceCeil(0.8)).toBe(1);
    expect(niceCeil(1.4)).toBe(2);
    expect(niceCeil(3)).toBe(5);
    expect(niceCeil(23)).toBe(50);
    expect(niceCeil(0.03)).toBe(0.05);
    expect(niceCeil(0)).toBe(1);
  });
});

describe('layoutTimeline', () => {
  const set: SeriesSet = {
    frames: [10, 11, 12, 13],
    series: [
      { key: 'system:a', name: 'a', kind: 'system', values: [10, 10, 10, 10], medianMs: 10 },
      { key: 'system:b', name: 'b', kind: 'system', values: [2, null, 2, 2], medianMs: 2 },
    ],
  };
  const lane = set.series;

  it('scales to a nice max that always contains the 16.6 ms guide', () => {
    const layout = layoutTimeline(set, lane, { width: 8, height: 100, minColumnPx: 2 });
    expect(layout.maxMs).toBe(20);
    expect(layout.guideY).toBeCloseTo(100 - (FRAME_BUDGET_MS / 20) * 100, 6);
  });

  it('stacks bands from the baseline up, leaving a hairline between segments', () => {
    const layout = layoutTimeline(set, lane, { width: 8, height: 100, minColumnPx: 2 });
    const [a, b] = layout.bands;
    // a: 10 ms of a 20 ms scale over 100 px = the bottom half, minus the 1 px separator.
    expect(a.top[0]).toBe(50);
    expect(a.bottom[0]).toBe(99);
    // b sits on top of a.
    expect(b.top[0]).toBe(40);
    expect(b.bottom[0]).toBe(49);
  });

  it('draws nothing for a column a series did not measure', () => {
    const layout = layoutTimeline(set, lane, { width: 8, height: 100, minColumnPx: 2 });
    const b = layout.bands[1];
    expect(b.top[1]).toBe(b.bottom[1]);
    expect(b.top[1]).toBe(50);
  });

  it('scrolls the oldest columns off when the width cannot hold them all', () => {
    const layout = layoutTimeline(set, lane, { width: 4, height: 100, minColumnPx: 2 });
    expect(layout.x).toHaveLength(2);
    expect(layout.offset).toBe(2);
    expect(layout.firstFrame).toBe(12);
    expect(layout.lastFrame).toBe(13);
    expect(layout.columnWidth).toBe(2);
  });

  it('tracks the frame-total series as a line, independent of the stack', () => {
    const withTotal: SeriesSet = {
      frames: set.frames,
      series: [...set.series, { key: TOTAL_KEY, name: 'frame', kind: 'total', values: [4, 4, null, 40], medianMs: 4 }],
    };
    const layout = layoutTimeline(withTotal, lane, { width: 8, height: 100, minColumnPx: 2 });
    // The total's own peak raises the scale above the stack's.
    expect(layout.maxMs).toBe(50);
    expect(layout.totalY[2]).toBeNull();
    expect(layout.totalY[3]).toBeCloseTo(100 - (40 / 50) * 100, 6);
  });

  it('degrades to an empty layout with no columns rather than throwing', () => {
    const layout = layoutTimeline({ frames: [], series: [] }, [], { width: 100, height: 40 });
    expect(layout.x).toEqual([]);
    expect(layout.bands).toEqual([]);
    expect(layout.guideY).not.toBeNull();
    expect(layoutTimeline(set, lane, { width: 0, height: 0 }).x).toEqual([]);
  });
});

describe('hover readout', () => {
  const set: SeriesSet = {
    frames: [10, 11],
    series: [
      { key: 'system:a', name: 'a', kind: 'system', values: [10, 1], medianMs: 5.5 },
      { key: 'system:b', name: 'b', kind: 'system', values: [2, 8], medianMs: 5 },
      { key: TOTAL_KEY, name: 'frame', kind: 'total', values: [13, null], medianMs: 13 },
    ],
  };
  const lane = set.series.slice(0, 2);
  const layout = layoutTimeline(set, lane, { width: 20, height: 100, minColumnPx: 10 });

  it('maps an x to a column and refuses one outside the plot', () => {
    expect(columnAt(layout, 0)).toBe(0);
    expect(columnAt(layout, 15)).toBe(1);
    expect(columnAt(layout, -1)).toBeNull();
    expect(columnAt(layout, 20)).toBeNull();
  });

  it('reads a column back as its frame and the values in it, biggest first', () => {
    const readout = readColumn(set, lane, layout, 1);
    expect(readout?.frame).toBe(11);
    expect(readout?.entries.map((e) => e.name)).toEqual(['b', 'a']);
    expect(readout?.stackedMs).toBe(9);
    expect(readout?.totalMs).toBeNull();
    expect(readColumn(set, lane, layout, 9)).toBeNull();
  });
});

describe('latestBreakdown', () => {
  it('reshapes the newest window into ranked groups measured against their own biggest entry', () => {
    let ring = pushWindow(emptyRing(), makeWindow({ first_frame: 0 }));
    ring = pushWindow(ring, makeWindow({ first_frame: 3, frame_total_median_ms: 9 }));
    const breakdown = latestBreakdown(ring, 8);

    expect(breakdown?.firstFrame).toBe(3);
    expect(breakdown?.frameTotalMs).toBe(9);
    // The host spans extend the frame rather than sitting inside it.
    expect(breakdown?.cpuMs).toBe(10.5);
    const systems = breakdown?.groups.find((g) => g.id === 'systems');
    expect(systems?.entries.map((e) => e.name)).toEqual(['enemy_ai', 'move_projectiles']);
    expect(systems?.entries[0].share).toBe(1);
    expect(systems?.entries[1].share).toBeCloseTo(0.5 / 3, 6);
    expect(systems?.totalMs).toBe(3.5);
  });

  it('omits the GPU group entirely when the adapter cannot time passes', () => {
    const ring = pushWindow(
      emptyRing(),
      makeWindow({
        first_frame: 0,
        gpu: { passes: [{ name: 'scene', median_ms: null }], sampled_frames: 0, timestamps_supported: false },
      }),
    );
    const breakdown = latestBreakdown(ring, 8);
    expect(breakdown?.timestampsSupported).toBe(false);
    expect(breakdown?.groups.map((g) => g.id)).toEqual(['systems', 'sequential']);
  });

  it('names the GPU frame in the passes note when a span was measured', () => {
    const gpu = { passes: [{ name: 'scene', median_ms: 1 }], sampled_frames: 2, timestamps_supported: true };
    const withSpan = pushWindow(emptyRing(), makeWindow({ first_frame: 0, gpu: { ...gpu, frame_median_ms: 4.9 } }));
    const passes = latestBreakdown(withSpan, 8)?.groups.find((g) => g.id === 'passes');
    expect(passes?.note).toContain('GPU frame 4.90 ms');
    const without = pushWindow(emptyRing(), makeWindow({ first_frame: 0, gpu: { ...gpu, frame_median_ms: null } }));
    expect(latestBreakdown(without, 8)?.groups.find((g) => g.id === 'passes')?.note).not.toContain('GPU frame');
  });

  it('adds the transport group only once the server has filled it', () => {
    const ring = pushWindow(
      emptyRing(),
      makeWindow({
        first_frame: 0,
        transport: { encode_median_ms: 12, send_median_ms: 0.4, sampled_frames: 3 },
      }),
    );
    const transport = latestBreakdown(ring, 8)?.groups.find((g) => g.id === 'transport');
    expect(transport?.entries.map((e) => e.name)).toEqual(['encode', 'send']);
    expect(transport?.note).toBe('3 of 3 frames delivered');
  });

  it('folds a long group at the limit', () => {
    const ring = pushWindow(
      emptyRing(),
      makeWindow({
        first_frame: 0,
        systems: Array.from({ length: 12 }, (_, i) => ({ name: `s${i}`, median_ms: 12 - i })),
      }),
    );
    const systems = latestBreakdown(ring, 4)?.groups.find((g) => g.id === 'systems');
    expect(systems?.entries).toHaveLength(4);
    expect(systems?.entries.at(-1)?.key).toBe(OTHER_KEY);
    // 9 + 8 + … + 1 for the nine folded entries.
    expect(systems?.entries.at(-1)?.ms).toBe(45);
  });

  it('answers null for an empty ring', () => {
    expect(latestBreakdown(emptyRing(), 8)).toBeNull();
  });

  it('carries the window worker count, and reports null when the window omits it', () => {
    // Every duration on the panel is relative to the count, so a window that did not carry one must
    // read as unknown — never as the 1 a `?? 1` would invent.
    const withCount = pushWindow(emptyRing(), makeWindow({ first_frame: 0, threads: 4 }));
    expect(latestBreakdown(withCount, 8)?.threads).toBe(4);

    const legacy = makeWindow({ first_frame: 0 });
    const { threads: _dropped, ...withoutCount } = legacy;
    expect(latestBreakdown(pushWindow(emptyRing(), withoutCount), 8)?.threads).toBeNull();
  });
});

describe('formatMs', () => {
  it('renders two decimals and an em dash for nothing measured', () => {
    expect(formatMs(4.2069)).toBe('4.21');
    expect(formatMs(null)).toBe('—');
    expect(formatMs(undefined)).toBe('—');
    expect(formatMs(Number.NaN)).toBe('—');
  });
});
