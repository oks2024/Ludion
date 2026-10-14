// UE5-style fly navigation for the stage camera: hold RMB to look; while held, W A S D move, Q/E
// down/up, Z/C narrow/widen FOV, wheel changes fly speed, Shift is fast (`Plan/editor-stage-camera.md`
// decision 5, the fly half). Pure — a view reads held keys and pointer deltas each frame and calls
// these; nothing here knows about DOM events, timers, or React.
//
// ROTATION CONVENTION: shared with `../voxel/orbit.ts` exactly, so a pose can be handed between the
// orbit rig and this one with no jump. `CameraPose.rotation` is Euler XYZ degrees, `Rx·Ry·Rz` over
// column vectors (see `orbit.ts`'s header for the full derivation against the engine's
// `ViewRig::compute`). A fly pose is always roll-free, meaning its HORIZON IS LEVEL (the camera's
// right axis has no world-Y component) — which is NOT `rotation[2] === 0`: under `Rx·Ry·Rz` a level
// camera that is both yawed and pitched carries a non-zero Z angle. Mouse-look therefore emits its
// pose through `lookAtPose` with world up, the same path every orbit pose takes, so a handed-over
// pose survives untouched until the first mouse-look re-levels it.

import { eulerXyzDegToQuat } from '../gizmo/camera';
import { mat4FromRotationTranslation, vAdd, vNormalize, vScale, type Vec3 } from '../gizmo/vecmath';
import { ELEVATION_LIMIT_DEG, STAGE_FOV_Y_DEG, lookAtPose, type CameraPose } from '../voxel/orbit';

const DEG2RAD = Math.PI / 180;
const RAD2DEG = 180 / Math.PI;

/** World up — Q/E and mouse-look both anchor to this, never to the camera's own up (see
 *  `advance`). */
const WORLD_UP: Vec3 = [0, 1, 0];

// --- Speed ------------------------------------------------------------------------------------ //

/** Default fly speed, in world units (metres, at stage scale) per second. */
export const DEFAULT_FLY_SPEED = 4;

/** Fly-speed bounds: fast enough to cross the stage, slow enough to place a part by hand. */
export const MIN_FLY_SPEED = 0.25;
export const MAX_FLY_SPEED = 64;

/** Wheel sensitivity while RMB is held, in the same multiplicative-per-step idiom as `orbit.ts`'s
 *  `DOLLY_PER_STEP`. */
export const FLY_SPEED_PER_STEP = 1.1;

/** Shift multiplies fly speed rather than replacing it, so the wheel-adjusted speed still matters
 *  while running. */
export const SHIFT_SPEED_MULTIPLIER = 3;

// --- Look --------------------------------------------------------------------------------------//

/** Mouse-look sensitivity, in degrees of yaw/pitch per pixel of drag. */
export const LOOK_DEG_PER_PIXEL = 0.15;

// --- FOV --------------------------------------------------------------------------------------- //

/** How fast Z/C narrow or widen the FOV, in degrees per second while held. */
export const FOV_DELTA_PER_SECOND = 40;

/** How far Z/C can push the FOV off its base, in degrees. */
export const MIN_FOV_DELTA_DEG = -30;
export const MAX_FOV_DELTA_DEG = 30;

/** Absolute FOV bounds, regardless of `baseFovYDeg` and the held delta — a view too narrow or too
 *  wide to be usable. */
export const MIN_FOV_Y_DEG = 5;
export const MAX_FOV_Y_DEG = 130;

// --- State --------------------------------------------------------------------------------------//

/** Which movement keys are currently held, named for the action rather than the physical key so a
 *  future rebind doesn't touch this module. */
export interface FlyKeys {
  readonly forward: boolean;
  readonly back: boolean;
  readonly left: boolean;
  readonly right: boolean;
  readonly down: boolean;
  readonly up: boolean;
}

/**
 * The fly camera's held-input state for one frame: which movement keys are down, whether Shift is
 * held, the current (wheel-adjusted) fly speed, and the current Z/C FOV delta from the base.
 */
export interface FlyState {
  readonly keys: FlyKeys;
  readonly shift: boolean;
  readonly speedMetersPerSecond: number;
  readonly fovDeltaDeg: number;
}

// --- Movement ------------------------------------------------------------------------------------ //

/** The rotation's world-space right and forward axes — the columns `gizmo/camera.ts`'s
 *  `buildViewProj` reads off the same model matrix, computed here without needing a translation or a
 *  viewport. */
function basisOf(rotationDeg: Vec3): { readonly right: Vec3; readonly forward: Vec3 } {
  const model = mat4FromRotationTranslation(eulerXyzDegToQuat(rotationDeg), [0, 0, 0]);
  return {
    right: vNormalize([model[0], model[1], model[2]]),
    forward: vNormalize([-model[8], -model[9], -model[10]]),
  };
}

/**
 * Advances the pose by one frame of held-key movement. W/S move along the view ray (forward/back),
 * A/D strafe on the camera's own right, Q/E move along WORLD up — not the camera's — so vertical fly
 * motion stays level even pitched steeply, matching UE5 rather than a rocket-nose climb. Shift
 * multiplies speed. Opposite keys held together cancel. Rotation is untouched; only translation moves.
 */
export function advance(pose: CameraPose, input: FlyState, dtSeconds: number): CameraPose {
  if (dtSeconds <= 0) {
    return pose;
  }
  const { keys } = input;
  const step = input.speedMetersPerSecond * (input.shift ? SHIFT_SPEED_MULTIPLIER : 1) * dtSeconds;
  const { right, forward } = basisOf(pose.rotation);
  let translation = pose.translation;
  if (keys.forward !== keys.back) {
    translation = vAdd(translation, vScale(forward, keys.forward ? step : -step));
  }
  if (keys.right !== keys.left) {
    translation = vAdd(translation, vScale(right, keys.right ? step : -step));
  }
  if (keys.up !== keys.down) {
    translation = vAdd(translation, vScale(WORLD_UP, keys.up ? step : -step));
  }
  return { ...pose, translation };
}

/**
 * Mouse-look while RMB is held. Dragging right (`dx > 0`) turns the view right, dragging up
 * (`dy < 0`) looks up — the standard FPS sense. Pitch clamps just short of straight up/down
 * (`ELEVATION_LIMIT_DEG`, shared with the orbit camera so neither rig gimbal-flips where the other
 * can't). The output's horizon is always level: any roll inherited from a handed-over orbit pose
 * (only the TOP preset carries one) is dropped here, since a fly camera never rolls.
 */
export function lookDrag(
  pose: CameraPose,
  dx: number,
  dy: number,
  degPerPixel = LOOK_DEG_PER_PIXEL,
): CameraPose {
  // Yaw and pitch are read off the view ray, not off `rotation[0..1]`: under `Rx·Ry·Rz` the X angle
  // turns about WORLD X, so stepping it on a yawed camera rolls the horizon.
  const { forward } = basisOf(pose.rotation);
  const pitch = clamp(
    Math.asin(clamp(forward[1], -1, 1)) * RAD2DEG - dy * degPerPixel,
    -ELEVATION_LIMIT_DEG,
    ELEVATION_LIMIT_DEG,
  );
  const yaw = Math.atan2(-forward[0], -forward[2]) * RAD2DEG - dx * degPerPixel;
  const horizontal = Math.cos(pitch * DEG2RAD);
  const aim: Vec3 = [
    -horizontal * Math.sin(yaw * DEG2RAD),
    Math.sin(pitch * DEG2RAD),
    -horizontal * Math.cos(yaw * DEG2RAD),
  ];
  return lookAtPose(pose.translation, vAdd(pose.translation, aim), WORLD_UP);
}

/** Adjusts fly speed by `steps` of wheel delta while RMB is held — multiplicative, like `orbit.ts`'s
 *  `dollyBy`, so a notch feels the same at any current speed. Positive `steps` speeds up. */
export function adjustFlySpeed(state: FlyState, steps: number, perStep = FLY_SPEED_PER_STEP): FlyState {
  return {
    ...state,
    speedMetersPerSecond: clamp(
      state.speedMetersPerSecond * Math.pow(perStep, steps),
      MIN_FLY_SPEED,
      MAX_FLY_SPEED,
    ),
  };
}

/**
 * Narrows (`direction = -1`, Z) or widens (`direction = 1`, C) the FOV delta while held, at
 * `FOV_DELTA_PER_SECOND` degrees per second, clamped to `[MIN_FOV_DELTA_DEG, MAX_FOV_DELTA_DEG]`.
 * `direction = 0` (neither held, or both — cancelling exactly as opposite movement keys do) is a
 * no-op.
 */
export function adjustFov(state: FlyState, direction: -1 | 0 | 1, dtSeconds: number): FlyState {
  if (direction === 0 || dtSeconds <= 0) {
    return state;
  }
  return {
    ...state,
    fovDeltaDeg: clamp(
      state.fovDeltaDeg + direction * FOV_DELTA_PER_SECOND * dtSeconds,
      MIN_FOV_DELTA_DEG,
      MAX_FOV_DELTA_DEG,
    ),
  };
}

/** FOV resets on RMB release — the plan's own words. */
export function resetFov(state: FlyState): FlyState {
  return { ...state, fovDeltaDeg: 0 };
}

/** The FOV to render at: `baseFovYDeg` plus the held Z/C delta, clamped to a usable absolute range. */
export function currentFovYDeg(state: FlyState, baseFovYDeg = STAGE_FOV_Y_DEG): number {
  return clamp(baseFovYDeg + state.fovDeltaDeg, MIN_FOV_Y_DEG, MAX_FOV_Y_DEG);
}

// --- Small helpers ------------------------------------------------------------------------------ //

function clamp(value: number, lo: number, hi: number): number {
  return Math.min(hi, Math.max(lo, value));
}
