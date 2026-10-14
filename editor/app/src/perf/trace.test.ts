import { describe, expect, it } from 'vitest';
import { emptyRing, pushWindow, type PerfWindow, type SampleColumns } from './model';
import { chromeTrace, LANE_GPU, LANE_HOST, LANE_SIM, LANE_TRANSPORT, traceFromReport, traceFromRing, type TraceEvent } from './trace';

/** Two frames, every lane populated, with numbers chosen so every span boundary is exact. */
const SAMPLES: SampleColumns = {
  frames: [100, 101],
  frame_total_ms: [4, 5],
  extract_ms: [1, null],
  submit_ms: [0.5, 0.5],
  system_names: ['enemy_ai', 'move_projectiles'],
  system_ms: [
    [3, 4],
    [0.5, 0.5],
  ],
  pass_names: ['scene', 'composite'],
  pass_ms: [
    [2, 2],
    [0.25, null],
  ],
};

function spans(events: readonly TraceEvent[], tid: number): TraceEvent[] {
  return events.filter((e) => e.ph === 'X' && e.tid === tid);
}

describe('chromeTrace', () => {
  const trace = chromeTrace([{ samples: SAMPLES }], 'district-plaza');

  it('labels only the lanes it filled', () => {
    const threads = trace.traceEvents.filter((e) => e.ph === 'M' && e.name === 'thread_name');
    expect(threads.map((e) => e.tid)).toEqual([LANE_SIM, LANE_HOST, LANE_GPU]);
    expect(threads.map((e) => e.args?.name)).toEqual(['sim systems', 'host', 'GPU passes']);
    const process = trace.traceEvents.find((e) => e.name === 'process_name');
    expect(process?.args?.name).toBe('ludion · district-plaza');
    expect(trace.displayTimeUnit).toBe('ms');
  });

  it('categorises every span by its lane, as the CLI export does', () => {
    expect(spans(trace.traceEvents, LANE_SIM).every((e) => e.cat === 'sim')).toBe(true);
    expect(spans(trace.traceEvents, LANE_HOST).every((e) => e.cat === 'host')).toBe(true);
    expect(spans(trace.traceEvents, LANE_GPU).every((e) => e.cat === 'gpu')).toBe(true);
  });

  it('butt-joins the schedule inside its own frame, in registration order', () => {
    const sim = spans(trace.traceEvents, LANE_SIM);
    expect(sim.map((e) => e.name)).toEqual(['enemy_ai', 'move_projectiles', 'enemy_ai', 'move_projectiles']);
    // Frame 100 starts at 0; 3 ms then 0.5 ms, in microseconds.
    expect(sim[0]).toMatchObject({ ts: 0, dur: 3000, args: { frame: 100 } });
    expect(sim[1]).toMatchObject({ ts: 3000, dur: 500 });
  });

  it('extends the frame by the host spans measured beside the tick', () => {
    const host = spans(trace.traceEvents, LANE_HOST);
    expect(host.map((e) => e.name)).toEqual(['frame_total', 'extract', 'submit', 'frame_total', 'submit']);
    expect(host[0]).toMatchObject({ ts: 0, dur: 4000 });
    expect(host[1]).toMatchObject({ ts: 4000, dur: 1000 });
    expect(host[2]).toMatchObject({ ts: 5000, dur: 500 });
    // Frame 101 opens at 4 + 1 + 0.5 = 5.5 ms, and carries no extract (it was not rendered).
    expect(host[3]).toMatchObject({ ts: 5500, dur: 5000, args: { frame: 101 } });
    expect(host[4]).toMatchObject({ ts: 10500, dur: 500 });
  });

  it('never overlaps two spans on one lane', () => {
    for (const tid of [LANE_SIM, LANE_HOST, LANE_GPU, LANE_TRANSPORT]) {
      const lane = spans(trace.traceEvents, tid);
      for (let i = 1; i < lane.length; i += 1) {
        const previousEnd = (lane[i - 1].ts ?? 0) + (lane[i - 1].dur ?? 0);
        expect(lane[i].ts ?? 0).toBeGreaterThanOrEqual(previousEnd);
      }
    }
  });

  it('files GPU passes under their own frame and queues them behind the previous frame', () => {
    const gpu = spans(trace.traceEvents, LANE_GPU);
    expect(gpu.map((e) => e.name)).toEqual(['scene', 'composite', 'scene']);
    expect(gpu[0]).toMatchObject({ ts: 0, dur: 2000, args: { frame: 100 } });
    expect(gpu[1]).toMatchObject({ ts: 2000, dur: 250 });
    // Frame 101's origin (5.5 ms) is later than the GPU cursor (2.25 ms), so it wins.
    expect(gpu[2]).toMatchObject({ ts: 5500, args: { frame: 101 } });
  });

  it('queues a GPU lane that cannot keep up instead of overlapping it', () => {
    const slow = chromeTrace([
      {
        samples: {
          ...SAMPLES,
          pass_names: ['scene'],
          pass_ms: [[9, 9]],
        },
      },
    ]);
    const gpu = spans(slow.traceEvents, LANE_GPU);
    expect(gpu[0]).toMatchObject({ ts: 0, dur: 9000 });
    // The second frame's own origin is 5.5 ms, but the GPU was still busy until 9 ms.
    expect(gpu[1]?.ts).toBe(9000);
  });

  it('drops the GPU lane entirely on an adapter that cannot time passes', () => {
    const degraded = chromeTrace([{ samples: { ...SAMPLES, pass_names: ['scene'], pass_ms: [[null, null]] } }]);
    expect(spans(degraded.traceEvents, LANE_GPU)).toEqual([]);
    expect(degraded.traceEvents.some((e) => e.ph === 'M' && e.tid === LANE_GPU)).toBe(false);
  });

  it('emits the transport lane only from the per-frame columns the server filled', () => {
    const withTransport = chromeTrace([
      {
        samples: SAMPLES,
        transport: {
          encode_median_ms: 12,
          send_median_ms: 0.4,
          sampled_frames: 2,
          encode_ms: [12, null],
          send_ms: [0.4, null],
        },
      },
    ]);
    const lane = spans(withTransport.traceEvents, LANE_TRANSPORT);
    expect(lane.map((e) => e.name)).toEqual(['encode', 'send']);
    expect(lane[0]).toMatchObject({ ts: 0, dur: 12000 });
    expect(lane[1]).toMatchObject({ ts: 12000, dur: 400 });
  });

  it('lays several windows end to end on one axis', () => {
    const two = chromeTrace([{ samples: SAMPLES }, { samples: SAMPLES }]);
    const host = spans(two.traceEvents, LANE_HOST).filter((e) => e.name === 'frame_total');
    expect(host).toHaveLength(4);
    // The second window opens where the first ended: 5.5 + 5.5 = 11 ms.
    expect(host[2].ts).toBe(11000);
  });

  it('rounds to nanoseconds so the JSON carries no float noise', () => {
    const odd = chromeTrace([
      {
        samples: {
          frames: [0],
          frame_total_ms: [1 / 3],
          extract_ms: [null],
          submit_ms: [null],
          system_names: [],
          system_ms: [],
          pass_names: [],
          pass_ms: [],
        },
      },
    ]);
    expect(spans(odd.traceEvents, LANE_HOST)[0].dur).toBe(333.333);
  });

  it('answers a valid, empty trace for no sections at all', () => {
    const empty = chromeTrace([]);
    expect(empty.traceEvents.filter((e) => e.ph === 'X')).toEqual([]);
    expect(empty.traceEvents[0]?.name).toBe('process_name');
    expect(() => JSON.stringify(empty)).not.toThrow();
  });
});

describe('the two entry points', () => {
  function window(first: number): PerfWindow {
    return {
      capture: false,
      first_frame: first,
      last_frame: first + 1,
      frames: 2,
      frame_total_median_ms: 4,
      systems: [],
      sequential: [],
      gpu: { passes: [], sampled_frames: 0, timestamps_supported: true },
      samples: { ...SAMPLES, frames: [first, first + 1] },
    };
  }

  it('traces the whole ring in frame order', () => {
    let ring = emptyRing();
    ring = pushWindow(ring, window(100));
    ring = pushWindow(ring, window(102));
    const frames = traceFromRing(ring, 'plaza')
      .traceEvents.filter((e) => e.tid === LANE_HOST && e.name === 'frame_total')
      .map((e) => e.args?.frame);
    expect(frames).toEqual([100, 101, 102, 103]);
  });

  it('traces a pinned capture, and degrades to no spans when it carries no samples', () => {
    expect(traceFromReport({ scene: 'plaza', samples: SAMPLES }).traceEvents.some((e) => e.ph === 'X')).toBe(true);
    const medianOnly = traceFromReport({ scene: 'plaza' });
    expect(medianOnly.traceEvents.some((e) => e.ph === 'X')).toBe(false);
    expect(medianOnly.otherData.frames).toBe('0');
  });
});
