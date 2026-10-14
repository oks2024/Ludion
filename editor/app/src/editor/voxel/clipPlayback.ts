// The clip strip as pure data: which clip is staged, whether the session is running, which tick is
// posed — and the exact wire writes each control owes, as intents, so the arithmetic is tested
// without a client.
//
// THE COMPONENT'S OWN `speed` NEVER MOVES. `Animation.speed` is written 1.0 always and the speed
// control is `sim.set_speed`, the Effects strip's mechanism (`editor/effects/time.ts`) — one way to
// slow a stage down, not two. It is also what keeps the bar honest: with the component at 1.0 a sim
// frame is a clip tick whatever the multiplier, so `phaseTick` reads the posed tick straight off the
// session's frame and a ×0.25 pass names the same ticks as a ×1 pass, only slower.
//
// SCRUBBING PAUSES FIRST. `started = now − k` poses tick `k` only while `now` holds still, so a
// scrub stops the sim before it writes. `started` is an unsigned frame index, so a session younger
// than the tick asked for is stepped forward first (`stepsNeeded`) — without that the deep end of a
// clip is simply unreachable on a cold stage.
//
// A STAMP IS A PREDICTION UNTIL IT LANDS. Every `set-animation` intent carries the TICK it means
// beside the `started` computed from the frame the transition was handed, because a running session
// moves between the decision and the write: the executor re-derives `started` at the frame the write
// actually lands on (`restamped`) and feeds that back through `withStamp`. Nothing here may be the
// authority on the session's clock.
//
// REST IS A REMOVAL, and it hands the session back the way it found it. Selecting rest — or losing
// the assembly — removes `Animation` and resumes at ×1, so the next model is never staged into a
// frozen sim. `component.remove` is `L3006` on an entity that carries no such component, which is why
// the removal is emitted only when this machine believes it put one there (`clip !== null`).
//
// A CHARACTER CLIP'S OVERLAY SPEED IS A RATE. Engine clip time on a character is
// `elapsed × Animation.speed × binding.speed`, so the component still writes 1 and `ctx.rate` (the
// staged binding's `speed`, default 1) divides every tick offset into sim ticks: one subtraction per
// stamp, however the clip is sped up.
//
// A CROSSFADE IS THE SAME STAMP WITH THREE OFFSETS. `started` and `fade_started` move together
// (the incoming clip and its blend weight both begin at its tick 0), and `from_started` sits
// `fromElapsed` sim ticks earlier — so a scrub inside the fade window is still one subtraction, and
// `scrubTo` steps forward far enough for the deepest of the three.
//
// Pure: no React, no protocol client, no DOM.

import { loopDue } from '../effects/time';
import { phaseTick, scrubStarted, simTicks, stepsNeeded, tickOf } from './anim';

/** The registry name of the component every write here lands on. */
export const ANIMATION = 'Animation';

/** `Animation.speed`, always — see the header. */
export const COMPONENT_SPEED = 1;

/** What the transitions need to know about the session and the selected clip. `clipTicks` is
 *  `durationTicks(doc.duration)` and `loops` the clip's own `loop:`; both are 0/false while no clip
 *  is loaded, which collapses every tick computation to 0 rather than to a guess. */
export interface ClipContext {
  /** The latest frame the session reported. */
  readonly frame: number;
  readonly clipTicks: number;
  readonly loops: boolean;
  /** The staged binding's overlay `speed` (clip ticks per sim tick); absent is 1. */
  readonly rate?: number;
}

/** A blend from the clip that was staged into the one that is. */
export interface Crossfade {
  /** The outgoing clip's short name. */
  readonly from: string;
  /** Sim ticks the outgoing clip had run when the fade began — how far before `started` its own
   *  `from_started` sits. */
  readonly fromElapsed: number;
  /** The blend's length, as the engine's `Animation.fade` takes it. */
  readonly seconds: number;
}

/** One `component.set` of `Animation`. */
export interface SetAnimation {
  readonly kind: 'set-animation';
  readonly clip: string;
  /** The tick this write means to pose — 0 for a play or a restart. The authority on re-stamping. */
  readonly tick: number;
  /** `Animation.started` at the frame the transition was given; see the header. */
  readonly started: number;
  readonly speed: number;
  /** Present while the write is a crossfade; the two stamps below are its derived frames. */
  readonly fade?: Crossfade;
  readonly fromStarted?: number;
  readonly fadeStarted?: number;
}

/** One wire write the strip owes. The executor turns each into exactly one command. */
export type ClipIntent =
  | SetAnimation
  | { readonly kind: 'remove-animation' }
  | { readonly kind: 'pause' }
  | { readonly kind: 'resume' }
  | { readonly kind: 'step'; readonly ticks: number }
  | { readonly kind: 'set-speed'; readonly speed: number };

/** The strip's whole state. */
export interface ClipState {
  /** The staged clip's short name, or `null` for the rest pose — which is also "this machine
   *  believes nothing of its own is on the subject". */
  readonly clip: string | null;
  /** Whether the session is ticking. Not "a clip is moving": a one-shot holds its last pose with the
   *  sim running. */
  readonly playing: boolean;
  /** The `sim.set_speed` multiplier. */
  readonly speed: number;
  /** The tick the bar names while the session is stopped. */
  readonly tick: number;
  /** Whether a one-shot restarts when it runs out. A looping clip wraps in the engine and ignores
   *  this entirely. */
  readonly loopOneShot: boolean;
  /** `Animation.started` as last written, or `null` at rest. */
  readonly started: number | null;
  /** The crossfade the staged clip was entered through, or `null` for a plain cut. */
  readonly fade: Crossfade | null;
}

export interface ClipTransition {
  readonly state: ClipState;
  readonly intents: readonly ClipIntent[];
}

/** Rest pose on a running session at ×1 — the state a freshly staged model is in, because the host
 *  spawns the stage engine without `--paused` (`editor/app/src-tauri/src/stage.rs`), exactly as the
 *  Effects strip's `INITIAL_RUN` records for its own. */
export const INITIAL_CLIP_STATE: ClipState = {
  clip: null,
  playing: true,
  speed: 1,
  tick: 0,
  loopOneShot: false,
  started: null,
  fade: null,
};

/** A tick brought inside `[0, clipTicks]`, rounded. Non-finite input is tick 0. */
export function clampTick(tick: number, clipTicks: number): number {
  if (!Number.isFinite(tick)) {
    return 0;
  }
  return Math.max(0, Math.min(Math.round(tick), Math.max(0, Math.round(clipTicks))));
}

/** The tick a fraction along the bar names. */
export function tickAtRatio(ratio: number, clipTicks: number): number {
  return clampTick(Number.isFinite(ratio) ? ratio * clipTicks : 0, clipTicks);
}

/** Where a tick sits along the bar, in `[0, 1]`. A clip with no length draws everything at its
 *  start rather than dividing by zero. */
export function barRatio(tick: number, clipTicks: number): number {
  return clipTicks <= 0 ? 0 : clampTick(tick, clipTicks) / clipTicks;
}

/** The length of a crossfade's blend band on the bar, in ticks. */
export function fadeTicks(fade: Crossfade): number {
  return tickOf(fade.seconds);
}

function stamp(
  clip: string,
  tick: number,
  frame: number,
  rate: number,
  fade: Crossfade | null,
): SetAnimation {
  const started = scrubStarted(frame, tick, rate);
  const base: SetAnimation = {
    kind: 'set-animation',
    clip,
    tick,
    started,
    speed: COMPONENT_SPEED,
  };
  return fade === null
    ? base
    : {
        ...base,
        fade,
        fromStarted: Math.max(0, started - fade.fromElapsed),
        fadeStarted: started,
      };
}

/** The same write re-derived at the frame it is really landing on. */
export function restamped(intent: SetAnimation, frame: number, rate = 1): SetAnimation {
  return stamp(intent.clip, intent.tick, frame, rate, intent.fade ?? null);
}

/** The `Animation` value a stamp writes — a whole-value `component.set`, the engine's own
 *  `crossfade()` spelled out when the stamp carries one. */
export function animationValue(intent: SetAnimation): Record<string, unknown> {
  const value: Record<string, unknown> = {
    clip: intent.clip,
    started: intent.started,
    speed: intent.speed,
  };
  if (intent.fade !== undefined) {
    value.from = intent.fade.from;
    value.from_started = intent.fromStarted ?? intent.started;
    value.from_speed = COMPONENT_SPEED;
    value.fade_started = intent.fadeStarted ?? intent.started;
    value.fade = intent.fade.seconds;
  }
  return value;
}

function rateOf(ctx: ClipContext): number {
  return ctx.rate !== undefined && Number.isFinite(ctx.rate) && ctx.rate > 0 ? ctx.rate : 1;
}

/** The state after a stamp has been written: what the session now holds, not what was predicted. */
export function withStamp(state: ClipState, intent: SetAnimation): ClipState {
  return {
    ...state,
    clip: intent.clip,
    tick: intent.tick,
    started: intent.started,
    fade: intent.fade ?? null,
  };
}

/** The tick posed right now: derived from the session's frame while it runs, and the scrubbed tick
 *  while it is stopped. Never a wall clock. */
export function shownTick(state: ClipState, ctx: ClipContext): number {
  if (state.clip === null) {
    return 0;
  }
  if (!state.playing || state.started === null) {
    return clampTick(state.tick, ctx.clipTicks);
  }
  return phaseTick(ctx.frame, state.started, rateOf(ctx), ctx.clipTicks, ctx.loops);
}

/** The state with its tick re-read off a freshly observed frame. The engine comes to rest a tick or
 *  two past the `sim.pause` that was asked for, so the bar adopts the pose that is really on screen
 *  rather than the one it predicted. No write: nothing here changes the world. */
export function withPosedTick(state: ClipState, ctx: ClipContext): ClipState {
  if (state.clip === null || state.started === null) {
    return state;
  }
  return {
    ...state,
    tick: phaseTick(ctx.frame, state.started, rateOf(ctx), ctx.clipTicks, ctx.loops),
  };
}

/** Back to the rest pose, with the session running at ×1 — see the header. Also the reset a lost or
 *  switched assembly goes through. */
export function toRest(state: ClipState): ClipTransition {
  const intents: ClipIntent[] = [];
  if (state.clip !== null) {
    intents.push({ kind: 'remove-animation' });
  }
  if (!state.playing) {
    intents.push({ kind: 'resume' });
  }
  if (state.speed !== 1) {
    intents.push({ kind: 'set-speed', speed: 1 });
  }
  return {
    state: { ...state, clip: null, started: null, tick: 0, playing: true, speed: 1, fade: null },
    intents,
  };
}

/** Stages a clip at its first tick and lets it run. The empty name and `null` are both the rest
 *  pose — the engine's own spelling of "no clip" and this machine's, kept one meaning. */
export function selectClip(
  state: ClipState,
  clip: string | null,
  ctx: ClipContext,
): ClipTransition {
  if (clip === null || clip === '') {
    return toRest(state);
  }
  const intent = stamp(clip, 0, ctx.frame, rateOf(ctx), null);
  // Stamped before the resume, so the first frame that ticks is already the clip's first.
  const intents: ClipIntent[] = state.playing ? [intent] : [intent, { kind: 'resume' }];
  return { state: { ...withStamp(state, intent), playing: true }, intents };
}

/**
 * Enters `clip` through a blend from the staged one. The outgoing clip's elapsed time is read off the
 * session's frame (valid running or paused), so the blend starts from the pose on screen. A length
 * the engine would not blend over is its hard cut — a plain `selectClip`.
 */
export function crossfadeTo(
  state: ClipState,
  clip: string,
  seconds: number,
  ctx: ClipContext,
): ClipTransition {
  if (state.clip === null) {
    return { state, intents: [] };
  }
  if (!Number.isFinite(seconds) || seconds <= 0) {
    return selectClip(state, clip, ctx);
  }
  const fade: Crossfade = {
    from: state.clip,
    fromElapsed: Math.max(0, ctx.frame - (state.started ?? ctx.frame)),
    seconds,
  };
  const intent = stamp(clip, 0, ctx.frame, rateOf(ctx), fade);
  const intents: ClipIntent[] = state.playing ? [intent] : [intent, { kind: 'resume' }];
  return { state: { ...withStamp(state, intent), playing: true }, intents };
}

/**
 * Resumes the session.
 *
 * A resume after a scrub needs no re-stamp: `started` already names the scrubbed tick, so the clip
 * carries on from where the human left it rather than from its beginning.
 */
export function play(state: ClipState): ClipTransition {
  return state.playing
    ? { state, intents: [] }
    : { state: { ...state, playing: true }, intents: [{ kind: 'resume' }] };
}

/** Stops the session, freezing the bar on the tick last derived. */
export function pause(state: ClipState, ctx: ClipContext): ClipTransition {
  return state.playing
    ? {
        state: { ...state, playing: false, tick: shownTick(state, ctx) },
        intents: [{ kind: 'pause' }],
      }
    : { state, intents: [] };
}

export function togglePlay(state: ClipState, ctx: ClipContext): ClipTransition {
  return state.playing ? pause(state, ctx) : play(state);
}

/** Re-stamps the clip at its first tick, replaying the crossfade it was entered through. The run
 *  state is untouched — a restart while stopped jumps to the first pose and stays there, which is the
 *  Effects strip's Restart to the letter. */
export function restart(state: ClipState, ctx: ClipContext): ClipTransition {
  if (state.clip === null) {
    return { state, intents: [] };
  }
  const intent = stamp(state.clip, 0, ctx.frame, rateOf(ctx), state.fade);
  return { state: withStamp(state, intent), intents: [intent] };
}

/**
 * Poses one tick: stop the clock, advance it if the session is younger than the tick asked for, and
 * write the subtraction.
 *
 * Over-stepping is the safe side of the guard and the reason `ctx.frame` may be stale: a frame count
 * read a moment ago is only ever lower than the real one, so `stepsNeeded` over-counts at worst,
 * while a count read too high would leave `started` clamped at 0 and the pose silently wrong.
 */
export function scrubTo(state: ClipState, tick: number, ctx: ClipContext): ClipTransition {
  if (state.clip === null) {
    return { state, intents: [] };
  }
  const target = clampTick(tick, ctx.clipTicks);
  const intents: ClipIntent[] = [];
  if (state.playing) {
    intents.push({ kind: 'pause' });
  }
  const rate = rateOf(ctx);
  // The deepest of the three offsets is the outgoing clip's: `from_started` is unsigned too.
  const steps = stepsNeeded(ctx.frame, simTicks(target, rate) + (state.fade?.fromElapsed ?? 0));
  if (steps > 0) {
    intents.push({ kind: 'step', ticks: steps });
  }
  const intent = stamp(state.clip, target, ctx.frame + steps, rate, state.fade);
  intents.push(intent);
  return { state: { ...withStamp(state, intent), playing: false }, intents };
}

/** A `sim.set_speed` write. A non-finite or non-positive multiplier is refused by returning the
 *  state unchanged, so no control value can reach the wire as a frozen or reversed session. */
export function setSpeed(state: ClipState, speed: number): ClipTransition {
  return Number.isFinite(speed) && speed > 0
    ? { state: { ...state, speed }, intents: [{ kind: 'set-speed', speed }] }
    : { state, intents: [] };
}

/** The one-shot loop toggle. Client-side only: nothing is written until the clip actually runs
 *  out. */
export function setLoopOneShot(state: ClipState, on: boolean): ClipTransition {
  return { state: { ...state, loopOneShot: on }, intents: [] };
}

/** Whether a held one-shot is due to be restarted — the Effects strip's `loopDue` over the clip's
 *  own length. A looping clip wraps in the engine and never comes through here. */
export function loopRestartDue(state: ClipState, ctx: ClipContext): boolean {
  return (
    state.clip !== null &&
    state.playing &&
    state.loopOneShot &&
    !ctx.loops &&
    ctx.clipTicks > 0 &&
    loopDue(ctx.frame, state.started, simTicks(ctx.clipTicks, rateOf(ctx)))
  );
}
