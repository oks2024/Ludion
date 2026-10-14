// The viewport's latency instrument: what a keypress costs before its effect reaches the screen.
//
// WHY IT IS ITS OWN MODULE. `vite.config.ts` collects `src/**/*.test.ts` under a `node` environment,
// so nothing living inside a `.tsx` widget is reachable by a unit test. Every decision below — the
// inject→frame correspondence, the rolling medians, the duty accumulator, the timestamp guard — is
// about what number to report, so all of it lives here and the surfaces keep only two call sites: a
// keydown and a blit.
//
// WHY THESE FOUR NUMBERS. Decode runs in a worker and the arriving frame that finds it busy is
// dropped rather than queued (`./framePump`), so "frames received vs frames decoded" is not a backlog
// counter — it is the shedding count, and shedding is the fix rather than the symptom. What is worth
// reading:
//
//   - `queuedMs` — how long a key event sat in the main thread's task queue before its handler ran.
//     This is the backlog's shadow on the SEND leg: a keydown cannot reach `ws.send` until whatever
//     the main thread is doing finishes.
//   - `lagMs` — the headline: key event → the first blitted frame that could possibly depict it.
//   - `decodeMs` — what one frame's decode costs, measured wherever the decode runs. It stays large
//     after the work moves off-thread, and that is the point: the cost did not vanish, it stopped
//     being the input leg's problem.
//   - `duty` — the fraction of wall-clock time the MAIN THREAD spends on the frame path over a
//     rolling second. This is the one that says whether input can get out: at 1.0 the main thread is
//     saturated and a keydown waits behind it; near 0 the frame path is not in input's way at all.
//     It is fed by {@link LagProbe.noteMainWork}, deliberately separate from `decodeMs`, so the two
//     can diverge — which is exactly what an off-thread decode looks like.
//
// THE INJECT→FRAME CORRESPONDENCE (the one non-mechanical rule here; see `INJECT_FRAME_LEAD`).
//
// Pure: no React, no DOM, no wire, and no clock — the caller supplies every timestamp, so a test
// drives the whole instrument with plain numbers.

import { DEFAULT_WINDOW_MS } from './fps';

/**
 * How many sim frames past an inject's applied frame the effect can first appear.
 *
 * DERIVATION, from the sources — worth a constant ~16.7 ms of bias at 60 Hz if it is off by one:
 *
 * 1. An `input.inject` reply's envelope `frame` is documented as the frame the command applied at
 *    (`crates/ludion-protocol/src/response.rs`), and is filled as `engine.frame()` sampled *after*
 *    the command ran (`crates/ludion-engine/src/dispatch.rs`, the `Dispatched` construction).
 * 2. `SimDriver::frame` is the frame that has NOT ticked yet: `do_tick` runs the systems with
 *    `frame = self.frame` and only afterwards advances the counter
 *    (`crates/ludion-sim/src/driver.rs`, `do_tick` steps 2–3).
 * 3. The editor sends no explicit frame (`viewer/keymap.ts` emits `{event}` only), so
 *    `inject_input`'s `at = frame.unwrap_or(self.frame).max(self.frame)` is exactly that same
 *    un-ticked frame F, and the event is applied at the START of tick F, before any system runs
 *    (`do_tick` step 1; the test `inject_input_applies_at_the_next_tick` pins both halves).
 * 4. A streamed frame is stamped with `engine.frame()` read at submit time
 *    (`crates/ludion-engine/src/live.rs`, `emit_egress`), and the live loop ticks (step 3) before it
 *    emits egress (step 4). So a frame stamped S depicts the world with ticks 0..S-1 run.
 *
 * Therefore a frame stamped F still shows the world as it was *before* the key was pressed — the
 * input applies during tick F, whose result is stamped F + 1. The first frame that can depict an
 * inject whose reply said F is the first with `frameIndex >= F + 1`.
 */
export const INJECT_FRAME_LEAD = 1;

/** Rolling window for the frame-side figures — one second, the window the fps readout already uses. */
export const FRAME_WINDOW_MS = DEFAULT_WINDOW_MS;

/** Rolling window for the input-side figures. Far wider than the frame window because a human presses
 *  keys orders of magnitude more rarely than the stream delivers, and a one-second window over key
 *  presses is empty most of the time. */
export const INPUT_WINDOW_MS = 5000;

/** A press whose frame never arrives (the stream stalled, the sim paused, the socket dropped) is
 *  abandoned after this long, so a pending measurement can never pin a growing list. */
export const INJECT_TIMEOUT_MS = 2000;

/** The largest dispatch delay still believable as one. A `timeStamp` on a different time origin
 *  (epoch milliseconds, say) lands far outside this, which is how the guard catches it. */
export const MAX_QUEUED_MS = 10_000;

/** Ring capacity for frame samples: enough for a full second at ~250 fps, which no stream reaches. */
const FRAME_CAPACITY = 256;

/** Ring capacity for input samples: a five-second window of the fastest human key mashing. */
const INPUT_CAPACITY = 64;

/** Most presses awaiting their frame at once. Beyond this the oldest is dropped — a backlog that
 *  deep means the answers stopped coming, not that more of them are worth keeping. */
const PENDING_CAPACITY = 32;

/** The instrument's reading. Every duration is a median over its rolling window; `null` means the
 *  window holds no sample. */
export interface LagSnapshot {
  /** Key event → first blitted frame that could depict it, in milliseconds. */
  readonly lagMs: number | null;
  /** How long the key event waited in the main thread's task queue before its handler ran. */
  readonly queuedMs: number | null;
  /** One frame's decode cost, measured wherever the decode ran (a worker, normally). */
  readonly decodeMs: number | null;
  /** Fraction of the last {@link FRAME_WINDOW_MS} the main thread spent on the frame path — the
   *  share of the time a keypress has to compete for. */
  readonly duty: number;
  /** Presses whose `Event.timeStamp` failed the sanity guard — nonzero means `queuedMs` is not
   *  measurable on this browser and `lagMs` is a lower bound (see {@link LagProbe.noteInject}). */
  readonly rejectedTimeStamps: number;
}

/** A probe that has measured nothing. */
export const NO_LAG: LagSnapshot = {
  lagMs: null,
  queuedMs: null,
  decodeMs: null,
  duty: 0,
  rejectedTimeStamps: 0,
};

/**
 * A bounded ring of timestamped values, summarized over a trailing window.
 *
 * Fixed-size typed arrays because this is written on every frame: pushing costs two stores and no
 * allocation. Order is never needed — a median and a sum both ignore it — so the ring is scanned as a
 * flat array and the write cursor's wrap is invisible.
 */
class SampleRing {
  private readonly at: Float64Array;
  private readonly value: Float64Array;
  private readonly scratch: Float64Array;
  private readonly capacity: number;
  private readonly windowMs: number;
  private count = 0;
  private head = 0;

  constructor(capacity: number, windowMs: number) {
    this.capacity = capacity;
    this.windowMs = windowMs;
    this.at = new Float64Array(capacity);
    this.value = new Float64Array(capacity);
    this.scratch = new Float64Array(capacity);
  }

  push(atMs: number, value: number): void {
    this.at[this.head] = atMs;
    this.value[this.head] = value;
    this.head = (this.head + 1) % this.capacity;
    if (this.count < this.capacity) {
      this.count++;
    }
  }

  /** The median of the in-window values, or `null` when the window is empty. An even count averages
   *  the two middle samples. */
  median(nowMs: number): number | null {
    const cutoff = nowMs - this.windowMs;
    let n = 0;
    for (let i = 0; i < this.count; i++) {
      if ((this.at[i] as number) > cutoff) {
        this.scratch[n++] = this.value[i] as number;
      }
    }
    if (n === 0) {
      return null;
    }
    const sorted = this.scratch.subarray(0, n).sort();
    const mid = n >> 1;
    return n % 2 === 1 ? (sorted[mid] as number) : ((sorted[mid - 1] as number) + (sorted[mid] as number)) / 2;
  }

  /** The sum of the in-window values (0 when empty). */
  sum(nowMs: number): number {
    const cutoff = nowMs - this.windowMs;
    let total = 0;
    for (let i = 0; i < this.count; i++) {
      if ((this.at[i] as number) > cutoff) {
        total += this.value[i] as number;
      }
    }
    return total;
  }
}

/** A press being measured: what its lag is counted from, and which frame will close it. */
interface PendingInject {
  readonly token: number;
  /** The clock `lagMs` is measured from — the event's own timestamp when it passed the guard, else
   *  the moment the handler ran. */
  readonly originMs: number;
  /** When the handler ran; the timeout sweep works off this, never off `originMs`. */
  readonly handledMs: number;
  /** The first `frameIndex` that can depict this press, once the reply named the applied frame. */
  showAt: number | null;
}

/**
 * One surface's latency instrument. Create one per frame surface (a `useRef` in the owner) — there is
 * no module-level state, so a viewport and a preview panel measure independently and their readings
 * can be compared.
 */
export interface LagProbe {
  /**
   * Opens a measurement for a key press, returning the token its reply must be handed back with.
   *
   * `timeStampMs` is the event's own `DOMHighResTimeStamp` and `nowMs` the `performance.now()` read
   * at the top of the handler; their difference is `queuedMs`. The two share a time origin in
   * Chromium/WebView2, but that is an assumption rather than a guarantee, so it is checked: a
   * timestamp that is not finite, not positive, in the future, or further back than
   * {@link MAX_QUEUED_MS} contributes no `queuedMs` sample and increments
   * {@link LagSnapshot.rejectedTimeStamps}. The press is still measured — from the handler instead,
   * which makes its `lagMs` a lower bound by exactly the queue delay that could not be read.
   */
  noteInject(timeStampMs: number, nowMs: number): number;
  /**
   * Supplies the frame an inject applied at, from its reply's envelope `frame`. `null` abandons the
   * measurement (the command failed, or the reply carried no frame).
   */
  resolveInject(token: number, appliedFrame: number | null): void;
  /** Records one decoded-and-blitted frame: what it depicts, when the blit finished, what the decode
   *  cost. Feeds `decodeMs` and closes any press this frame is the first to depict. */
  noteFrame(frameIndex: number, nowMs: number, decodeMs: number): void;
  /**
   * Records main-thread time spent on the frame path — the only thing `duty` counts.
   *
   * Separate from {@link noteFrame} because the two no longer coincide: the decode's cost belongs to
   * the worker, while the main thread's share is the handoff (and, where the worker cannot own the
   * canvas, the blit). It is also called for frames that were shed before display, since that time
   * was spent whether or not an image resulted.
   */
  noteMainWork(nowMs: number, costMs: number): void;
  /** The current reading. Also expires presses whose frame never came. */
  snapshot(nowMs: number): LagSnapshot;
}

/** Creates a {@link LagProbe}. */
export function createLagProbe(): LagProbe {
  const lag = new SampleRing(INPUT_CAPACITY, INPUT_WINDOW_MS);
  const queued = new SampleRing(INPUT_CAPACITY, INPUT_WINDOW_MS);
  const decode = new SampleRing(FRAME_CAPACITY, FRAME_WINDOW_MS);
  const mainWork = new SampleRing(FRAME_CAPACITY, FRAME_WINDOW_MS);
  const pending: PendingInject[] = [];
  let nextToken = 1;
  let rejectedTimeStamps = 0;
  let lastFrameIndex: number | null = null;
  let lastFrameAtMs = 0;

  // In-place compaction rather than `splice`: this runs on every frame, and `splice` allocates the
  // array of removed elements. `onBlit` closes any press the frame satisfies.
  //
  // The timeout is checked FIRST, so a press is dropped even when this very frame would have closed
  // it: a multi-second gap means a paused sim or a stalled stream, and recording it as one key's
  // latency would poison the median with a number that is not about input at all.
  const sweep = (nowMs: number, onBlit: ((entry: PendingInject) => boolean) | null): void => {
    let write = 0;
    for (let read = 0; read < pending.length; read++) {
      const entry = pending[read] as PendingInject;
      if (nowMs - entry.handledMs > INJECT_TIMEOUT_MS) {
        continue;
      }
      if (onBlit !== null && onBlit(entry)) {
        continue;
      }
      pending[write++] = entry;
    }
    pending.length = write;
  };

  return {
    noteInject(timeStampMs: number, nowMs: number): number {
      const delay = nowMs - timeStampMs;
      const usable = Number.isFinite(timeStampMs) && timeStampMs > 0 && delay >= 0 && delay <= MAX_QUEUED_MS;
      if (usable) {
        queued.push(nowMs, delay);
      } else {
        rejectedTimeStamps++;
      }
      const token = nextToken++;
      if (pending.length >= PENDING_CAPACITY) {
        pending.shift();
      }
      pending.push({
        token,
        originMs: usable ? timeStampMs : nowMs,
        handledMs: nowMs,
        showAt: null,
      });
      return token;
    },

    resolveInject(token: number, appliedFrame: number | null): void {
      const index = pending.findIndex((entry) => entry.token === token);
      if (index < 0) {
        return;
      }
      const entry = pending[index] as PendingInject;
      if (appliedFrame === null || !Number.isFinite(appliedFrame)) {
        pending.splice(index, 1);
        return;
      }
      const showAt = appliedFrame + INJECT_FRAME_LEAD;
      // The reply normally beats the frame it predicts (that frame is a tick away, and both travel the
      // one socket), but nothing enforces it — so a reply naming a frame already on screen closes the
      // measurement against that frame's blit, rather than waiting for a later one and overstating.
      if (lastFrameIndex !== null && lastFrameIndex >= showAt) {
        lag.push(lastFrameAtMs, lastFrameAtMs - entry.originMs);
        pending.splice(index, 1);
        return;
      }
      entry.showAt = showAt;
    },

    noteFrame(frameIndex: number, nowMs: number, decodeMs: number): void {
      decode.push(nowMs, decodeMs);
      lastFrameIndex = frameIndex;
      lastFrameAtMs = nowMs;
      if (pending.length === 0) {
        return;
      }
      sweep(nowMs, (entry) => {
        if (entry.showAt === null || frameIndex < entry.showAt) {
          return false;
        }
        lag.push(nowMs, nowMs - entry.originMs);
        return true;
      });
    },

    noteMainWork(nowMs: number, costMs: number): void {
      mainWork.push(nowMs, costMs);
    },

    snapshot(nowMs: number): LagSnapshot {
      if (pending.length > 0) {
        sweep(nowMs, null);
      }
      return {
        lagMs: lag.median(nowMs),
        queuedMs: queued.median(nowMs),
        decodeMs: decode.median(nowMs),
        // A fixed denominator, not the elapsed time since the first frame: the reading is "fraction of
        // the last second the main thread was busy with frames", so a probe younger than its window
        // reads low rather than dividing by a sliver of wall clock and reporting a saturation that
        // never happened.
        duty: Math.min(1, mainWork.sum(nowMs) / FRAME_WINDOW_MS),
        rejectedTimeStamps,
      };
    },
  };
}

/** One duration for the overlay: `—` when unmeasured, one decimal below 10 ms so a sub-millisecond
 *  decode doesn't read as zero. */
function formatMs(value: number | null): string {
  return value === null ? '—' : `${value.toFixed(value < 10 ? 1 : 0)} ms`;
}

/**
 * The overlay's one-line rendering, e.g. `lag 62 ms · q 14 ms · dec 18 ms · duty 0.92`.
 *
 * `q ?` rather than `q —` when every timestamp was rejected: an unmeasurable queue delay and an
 * unpressed key are different states, and only the first says the instrument's assumption failed.
 */
export function formatLag(snapshot: LagSnapshot): string {
  const q = snapshot.queuedMs === null && snapshot.rejectedTimeStamps > 0 ? '?' : formatMs(snapshot.queuedMs);
  return `lag ${formatMs(snapshot.lagMs)} · q ${q} · dec ${formatMs(snapshot.decodeMs)} · duty ${snapshot.duty.toFixed(2)}`;
}
