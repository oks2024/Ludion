// The UE5 binding table as pure data: which camera gesture a pointer's held buttons and modifiers
// mean, what a wheel notch means, and which fly action a key is (`Plan/editor-stage-camera.md`
// decision 5). Every view's camera reads its bindings from here, so the table exists once and is
// pinned by tests rather than by a sitting.
//
// KEYS ARE PHYSICAL — `KeyboardEvent.code`, never `key`. WASD is a shape under the left hand, not
// four letters: `code` keeps that shape on an AZERTY board (where it reads ZQSD) with no rebind, and
// keeps a dead key or an IME from swallowing a movement key.
//
// THE RESOLUTION IS TOTAL. Every button/modifier combination maps to exactly one gesture, including
// the ones nobody presses on purpose — a drag that resolves to nothing while the human is still
// holding the mouse down is the bug this shape removes. Alt is checked first because in UE every Alt
// combination is a pivot gesture and none of them is a fly.
//
// Pure: no DOM types, no React, no engine.

import type { FlyKeys } from './fly';

// --- Buttons ---------------------------------------------------------------------------------- //

/** `PointerEvent.buttons` bits — the buttons held *after* the event, which is what a gesture is a
 *  function of. `button` (singular) names only the one that just changed and cannot tell LMB+RMB
 *  from RMB alone. */
export const LEFT_BUTTON = 1;
export const RIGHT_BUTTON = 2;
export const MIDDLE_BUTTON = 4;

/** What a drag does. `pick` is "not the camera's": the view keeps its own meaning for a plain left
 *  drag (pick, marquee, gizmo), and `none` is a combination the camera ignores. */
export type DragGesture = 'fly-look' | 'orbit' | 'dolly' | 'pan' | 'pick' | 'none';

export interface DragInput {
  /** `PointerEvent.buttons`. */
  readonly buttons: number;
  readonly alt: boolean;
  /** Whether a plain left drag orbits here. True on a stage — there is nothing to select, so left
   *  drag orbits as UE's own asset preview does; false in a viewport where left belongs to picking
   *  and the gizmo (decision 5's one binding not copied from UE). */
  readonly plainLeftOrbits: boolean;
}

/** The gesture a drag means. */
export function resolveDrag(input: DragInput): DragGesture {
  const left = (input.buttons & LEFT_BUTTON) !== 0;
  const right = (input.buttons & RIGHT_BUTTON) !== 0;
  const middle = (input.buttons & MIDDLE_BUTTON) !== 0;
  if (input.alt) {
    // Alt is the pivot modifier: LMB tumbles, RMB zooms, MMB tracks. Middle wins over the others so
    // a stray left still pans while the human is tracking; Alt+LMB+RMB lands on dolly, which is
    // what UE does with that pair.
    if (middle) {
      return 'pan';
    }
    if (right) {
      return 'dolly';
    }
    return left ? 'orbit' : 'none';
  }
  if (middle || (left && right)) {
    return 'pan';
  }
  if (right) {
    return 'fly-look';
  }
  if (left) {
    return input.plainLeftOrbits ? 'orbit' : 'pick';
  }
  return 'none';
}

/** Whether a gesture is the camera's to drive — i.e. everything but a view-owned `pick` and the
 *  ignored combinations. */
export function isCameraDrag(gesture: DragGesture): boolean {
  return gesture !== 'pick' && gesture !== 'none';
}

// --- Wheel ------------------------------------------------------------------------------------ //

/** The wheel changes fly speed while the look is held, and dollies otherwise. */
export interface WheelResolution {
  readonly kind: 'fly-speed' | 'dolly';
  /** Notches, in the sense the matching math takes: positive dollies *out* (`orbit.ts`'s `dollyBy`)
   *  and positive speeds *up* (`fly.ts`'s `adjustFlySpeed`) — which is why the two carry opposite
   *  signs for the same wheel direction. Rolling the wheel away from the hand (`deltaY < 0`) pulls
   *  the camera in and speeds the fly up, both of which read as "more". */
  readonly steps: number;
}

/**
 * What one wheel event means. Only the SIGN of `deltaY` is used: browsers disagree wildly on its
 * magnitude and unit (pixels, lines, pages), and both consumers are multiplicative per step anyway.
 */
export function resolveWheel(deltaY: number, flying: boolean): WheelResolution {
  const notch = deltaY > 0 ? 1 : deltaY < 0 ? -1 : 0;
  // The two consumers read a positive step as opposite directions (see `steps`), so the fly-speed
  // notch is the negated one — written out at 0 rather than negated, to keep a `-0` out of the
  // result.
  if (flying) {
    return { kind: 'fly-speed', steps: notch === 0 ? 0 : -notch };
  }
  return { kind: 'dolly', steps: notch };
}

// --- Keys ------------------------------------------------------------------------------------- //

/** One thing a key does while the look is held. The six movement actions are `FlyKeys`' own field
 *  names, so the mapping below is a rename-free lookup. */
export type FlyAction = keyof FlyKeys | 'fov-narrow' | 'fov-widen';

const FLY_ACTIONS: Readonly<Record<string, FlyAction>> = {
  KeyW: 'forward',
  KeyS: 'back',
  KeyA: 'left',
  KeyD: 'right',
  KeyQ: 'down',
  KeyE: 'up',
  KeyZ: 'fov-narrow',
  KeyC: 'fov-widen',
};

/** The fly action a physical key code is, or `null`. */
export function flyAction(code: string): FlyAction | null {
  return FLY_ACTIONS[code] ?? null;
}

/** A camera command that stands on its own — no mouse button held, no drag in flight. */
export type ViewCommand = 'frame' | 'home' | 'toggle-game';

const VIEW_COMMANDS: Readonly<Record<string, ViewCommand>> = {
  KeyF: 'frame',
  Home: 'home',
  KeyG: 'toggle-game',
};

/** The camera command a physical key code is, or `null`. */
export function viewCommand(code: string): ViewCommand | null {
  return VIEW_COMMANDS[code] ?? null;
}

/** No movement at all — the state a released look hands back. */
export const NO_FLY_KEYS: FlyKeys = {
  forward: false,
  back: false,
  left: false,
  right: false,
  down: false,
  up: false,
};

/** The movement keys a set of held key codes amounts to. Codes that are not movement are ignored,
 *  so the caller may hand over everything it is holding. */
export function flyKeysFrom(held: ReadonlySet<string>): FlyKeys {
  const keys = { ...NO_FLY_KEYS };
  for (const code of held) {
    const action = flyAction(code);
    if (action !== null && action !== 'fov-narrow' && action !== 'fov-widen') {
      keys[action] = true;
    }
  }
  return keys;
}

/**
 * The FOV direction `fly.ts`'s `adjustFov` takes: −1 narrowing (Z), +1 widening (C), 0 for neither —
 * and for both, which cancel exactly as opposite movement keys do.
 */
export function fovDirection(held: ReadonlySet<string>): -1 | 0 | 1 {
  let narrow = false;
  let widen = false;
  for (const code of held) {
    const action = flyAction(code);
    narrow ||= action === 'fov-narrow';
    widen ||= action === 'fov-widen';
  }
  if (narrow === widen) {
    return 0;
  }
  return narrow ? -1 : 1;
}
