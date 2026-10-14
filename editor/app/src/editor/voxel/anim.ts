// A `kind: animation` clip read for VIEWING, plus the tick arithmetic a scrub bar is made of.
//
// TICKS ARE THE UNIT, NOT SECONDS (`ludion docs voxel-animation`). Clip time is
// `(now − started) × seconds_per_tick × speed`, derived from tick counts every frame and never
// accumulated — which is why `Animation.started` is a frame index and why clip time is always a
// whole number of sim ticks. Two things follow, and both are the reason this module exists:
//
//   - **Scrubbing is a subtraction.** With the sim paused, `now` is fixed, so writing
//     `started = now − k` poses the model at tick `k`. `started` is UNSIGNED, so a fresh session
//     whose frame count is below the clip's length has to be stepped forward first — `stepsNeeded`
//     is that guard, and without it the deepest part of a clip is simply unreachable on a cold
//     stage.
//   - **A keyframe authored between ticks never renders.** `0.45` is tick 27 and lands exactly;
//     `0.11` is 6.6 ticks and never samples as authored. The format cannot catch it and the engine
//     does not complain, so the bar flags it — `isOnTick` is that flag.
//
// `tickLabel` reproduces the CLI's phase report (`ludion render --model … --clip … --strip`) so a
// human reading the bar and an agent reading stdout quote the same instant. The CLI prints its
// seconds as an f64 with Rust's shortest round-trip Display, which is character for character what
// JavaScript's own number-to-string gives for the same `tick / 60` — verified against
// `render --strip 7` on `humanoid-walk` (`t 0.43333333333333335 (tick 26)`).
//
// Total, like every reader in this folder: it never throws and never validates. `check-scene`
// through `readout.ts` is the one checker.
//
// Pure: no React, no Tauri, no DOM.

import {
  itemEntries,
  mapEntries,
  seqItems,
  splitLines,
  type RawLine,
  type MapEntryRead,
} from './doc';

/** The engine's fixed tick rate (`TickRate::DEFAULT`, `crates/ludion-core/src/time.rs`) — the clock
 *  every clip is quantized to. */
export const TICK_HZ = 60;

/** The channels a track may animate: voxel clips use the first two, imported skeletal clips add
 *  scale. There is no visibility channel. */
export type Channel = 'rotation' | 'translation' | 'scale';

/** One channel of one part: the keyframe times, in authored order. Values are deliberately absent —
 *  the bar marks WHEN a pose was authored, and what it looks like is the stage's answer. */
export interface AnimTrack {
  /** The part name the track addresses. A name the assembly does not have is simply not applied at
   *  playback, which is what makes one humanoid clip reusable. */
  readonly part: string;
  readonly channel: Channel;
  readonly times: readonly number[];
}

/** A clip document read positionally. Every field is best-effort; see the module doc. */
export interface AnimDoc {
  readonly name: string | null;
  /** Clip length in seconds, or `0` when absent or unreadable. */
  readonly duration: number;
  /** `true` wraps at `duration`; `false` is a one-shot that holds its final pose. */
  readonly loop: boolean;
  /** Tracks in authored order, one entry per part/channel pair. */
  readonly tracks: readonly AnimTrack[];
}

/** Reads a `kind: animation` document. Total: never throws, never validates. */
export function parseAnimDoc(source: string): AnimDoc {
  const raw = splitLines(source);
  let name: string | null = null;
  let duration = 0;
  let loops = false;
  let tracks: readonly AnimTrack[] = [];

  for (const entry of mapEntries(raw, 0, raw.length)) {
    switch (entry.key) {
      case 'name':
        name = entry.inline === null ? null : entry.inline.text;
        break;
      case 'duration':
        duration = finite(entry.inline?.text);
        break;
      case 'loop':
        loops = entry.inline?.text === 'true';
        break;
      case 'tracks':
        tracks = readTracks(raw, entry);
        break;
      default:
        break;
    }
  }
  return { name, duration, loop: loops, tracks };
}

function readTracks(raw: readonly RawLine[], entry: MapEntryRead): AnimTrack[] {
  const out: AnimTrack[] = [];
  for (const part of mapEntries(raw, entry.body[0], entry.body[1])) {
    for (const channel of mapEntries(raw, part.body[0], part.body[1])) {
      if (channel.key !== 'rotation' && channel.key !== 'translation' && channel.key !== 'scale') {
        continue;
      }
      const times = seqItems(raw, channel.body[0], channel.body[1]).flatMap((key) => {
        const t = itemEntries(raw, key).find((field) => field.key === 't');
        return t === undefined ? [] : [finite(t.inline?.text)];
      });
      out.push({ part: part.key, channel: channel.key, times });
    }
  }
  return out;
}

function finite(text: string | undefined): number {
  const value = Number(text);
  return text !== undefined && text !== '' && Number.isFinite(value) ? value : 0;
}

// --- Tick arithmetic --------------------------------------------------------------------------- //

/** A tolerance in ticks, wide enough to absorb `0.45 × 60 = 27.000000000000004` and far narrower
 *  than the half-tick that would make an off-tick keyframe indistinguishable from an on-tick one. */
const TICK_EPSILON = 1e-6;

/** The clip's last tick: the wrap point for a loop, the held pose for a one-shot. Rounds the way the
 *  engine's own `quantize` does, so `0.35` is 21 ticks and not 20. */
export function durationTicks(duration: number): number {
  return tickOf(duration);
}

/** The tick nearest `seconds`. Mirrors `quantize` in `crates/ludion-engine/src/model_sheet.rs`:
 *  non-finite or non-positive input is tick 0, never a saturating cast. */
export function tickOf(seconds: number): number {
  return Number.isFinite(seconds) && seconds > 0 ? Math.round(seconds * TICK_HZ) : 0;
}

/** A tick as seconds. */
export function secondsOf(tick: number): number {
  return tick / TICK_HZ;
}

/** Whether `t` is a whole 60 Hz tick multiple — i.e. whether the pose authored at `t` is a pose that
 *  ever renders. */
export function isOnTick(t: number): boolean {
  if (!Number.isFinite(t)) {
    return false;
  }
  const ticks = t * TICK_HZ;
  return Math.abs(ticks - Math.round(ticks)) <= TICK_EPSILON;
}

/** One authored keyframe time on the scrub bar. */
export interface KeyframeMark {
  /** The time as authored, in seconds. */
  readonly t: number;
  /** The tick it actually samples at. */
  readonly tick: number;
  /** False when `t` falls between ticks — the documented authoring trap, flagged on the bar. */
  readonly onTick: boolean;
}

/** Every authored keyframe time across every track, de-duplicated and ascending. Two tracks keyed at
 *  the same instant are one mark: the bar shows *when* something was authored, not how much. */
export function keyframeMarks(doc: AnimDoc): readonly KeyframeMark[] {
  const times = new Set<number>();
  for (const track of doc.tracks) {
    for (const t of track.times) {
      times.add(t);
    }
  }
  return [...times]
    .sort((a, b) => a - b)
    .map((t) => ({ t, tick: tickOf(t), onTick: isOnTick(t) }));
}

/** The CLI's phase-report wording for one tick: `t 0.4 (tick 24)`. See the module doc for why the
 *  plain number-to-string matches Rust's f64 Display here. */
export function tickLabel(tick: number): string {
  return `t ${secondsOf(tick)} (tick ${tick})`;
}

/** The `Animation.started` value that poses tick `tick` at sim frame `frame`. Clamped at 0 because
 *  `started` is an unsigned frame index — `stepsNeeded` says how far a session must advance before
 *  the clamp stops lying. `rate` is the clip-ticks-per-sim-tick multiplier a character's per-clip
 *  overlay `speed` adds (1 for everything else), so the offset is in sim ticks. */
export function scrubStarted(frame: number, tick: number, rate = 1): number {
  return Math.max(0, frame - simTicks(tick, rate));
}

/** The sim ticks a clip tick takes to play at `rate`. */
export function simTicks(tick: number, rate = 1): number {
  return Math.round(tick / rate);
}

/** How many `sim.step` ticks a session must advance before `scrubStarted(frame, tick)` can reach
 *  `tick` at all. 0 once `frame >= tick`. */
export function stepsNeeded(frame: number, tick: number, rate = 1): number {
  return Math.max(0, simTicks(tick, rate) - frame);
}

/**
 * The tick currently posed: `(frame − started) × speed`, wrapped for a loop and held for a one-shot.
 *
 * A `started` in the future, a zero `speed` and a clip with no length all read as tick 0 — the
 * engine's own answers (`speed: 0.0` freezes on the first instant; a future `started` reads as that
 * same first instant rather than a negative time). Rounding is the display's, not the engine's: at
 * the ordinary speed 1 every value here is already whole, and a fractional multiplier only moves
 * which tick the label names.
 */
export function phaseTick(
  frame: number,
  started: number,
  speed: number,
  clipTicks: number,
  loops: boolean,
): number {
  const elapsed = (frame - started) * speed;
  if (!Number.isFinite(elapsed) || elapsed <= 0 || clipTicks <= 0) {
    return 0;
  }
  if (!loops) {
    return Math.round(Math.min(elapsed, clipTicks));
  }
  // Rounding can land exactly on the wrap point, which is tick 0 of the next stride, not a tick the
  // clip has.
  return Math.round(elapsed % clipTicks) % clipTicks;
}
