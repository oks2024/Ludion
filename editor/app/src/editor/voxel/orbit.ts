// The stage viewport's camera math: orbit / pan / dolly, the five contact-sheet presets, the
// turntable, and the two framing modes. Pure — the view throttles these poses onto the stage
// camera's `Transform` with `component.set`, and nothing here knows that.
//
// ROTATION CONVENTION — the engine's, and there is exactly one. `Transform.rotation` is Euler XYZ in
// DEGREES, composed as `Quat::from_euler(EulerRot::XYZ, …)`, i.e. the matrix `Rx·Ry·Rz` over column
// vectors, and the camera looks down its local −Z. The authority is
// `crates/ludion-engine/src/model_sheet.rs` (`ViewRig::compute`: `Mat4::look_at_rh(position, target,
// up).inverse()` decomposed with `to_euler(EulerRot::XYZ)`) — that is how the contact sheet frames
// every cell, so matching it is what makes this viewport agree with the sheet. `editor/app/src/
// editor/gizmo/camera.ts` mirrors the same convention on the TS side (`eulerXyzDegToQuat` /
// `quatToEulerXyzDeg`), and the committed `editor/app/fixtures/stage/scenes/model-stage.yaml` shows the FRONT pose
// authored by hand as `translation [0, 2, -7] / rotation [0, 180, 0]` — a camera on −Z looking at
// +Z. All three agree; `lookAtPose` reproduces the stage file's numbers exactly (see orbit.test.ts).
//
// PARITY OF PERCEPTION is the through-line of this arc, so the presets are not invented here: their
// directions and their labels are lifted from `ModelView::direction_and_up` and `ModelView::label`
// in `model_sheet.rs`. `GAME 58` is the vigil district camera's own 58°-down pitch.

import {
  vAdd,
  vCross,
  vLen,
  vNormalize,
  vScale,
  vSub,
  type Vec3,
} from '../gizmo/vecmath';

const DEG2RAD = Math.PI / 180;
const RAD2DEG = 180 / Math.PI;

/** World up for interactive orbiting. The TOP preset overrides it (see `Preset.up`). */
const WORLD_UP: Vec3 = [0, 1, 0];

/** Elevation is clamped just short of the poles: at exactly ±90° the up vector is degenerate and the
 *  camera's roll is undefined, which reads as a snap during a drag. */
export const ELEVATION_LIMIT_DEG = 89.5;

/** Orbit distance bounds, comfortably inside the stage camera's near 0.05 / far 100. */
export const MIN_ORBIT_DISTANCE = 0.1;
export const MAX_ORBIT_DISTANCE = 80;

/** Default drag sensitivities, in degrees of orbit per pixel and dolly factor per wheel step. */
export const ORBIT_DEG_PER_PIXEL = 0.4;
export const DOLLY_PER_STEP = 1.1;

/** The stage camera's vertical field of view — `model-stage.yaml`'s `fov_y_deg`, which is the
 *  contact sheet's `FOV_Y_DEG` too. Every framing formula here assumes it. */
export const STAGE_FOV_Y_DEG = 45;

/** The sheet's framing margin: 10% past the tight bounding-sphere fit (`model_sheet.rs`
 *  `FIT_MARGIN`). */
const FIT_MARGIN = 1.1;

// --- Poses ------------------------------------------------------------------------------------ //

/** A `Transform` payload for `component.set` on the stage camera. */
export interface CameraPose {
  readonly translation: Vec3;
  /** Euler XYZ degrees — the engine's `Transform.rotation`. */
  readonly rotation: Vec3;
}

/** The render size the framing math fits into. */
export interface Viewport {
  readonly width: number;
  readonly height: number;
}

/**
 * The camera pose looking from `position` at `target` with world `up`.
 *
 * This is `ViewRig::compute`'s decomposition, in TS: build the camera basis (`right = forward × up`,
 * `camUp = right × forward`, local +Z = −forward), then read Euler XYZ off the rotation matrix with
 * the same entries `quatToEulerXyzDeg` uses.
 *
 * Euler XYZ has two representations of every non-gimbal rotation (`X+180, 180−Y, Z+180` is the same
 * pose); this returns the one with the smaller |Z|, because an orbit camera has no roll and the
 * roll-free branch is the one a human recognizes — it is what makes FRONT come out as the committed
 * `[0, 180, 0]` rather than the equally correct `[180, 0, 180]`.
 */
export function lookAtPose(position: Vec3, target: Vec3, up: Vec3): CameraPose {
  const forward = safeNormalize(vSub(target, position), [0, 0, -1]);
  // A view direction parallel to `up` leaves the roll undefined; fall back to a perpendicular axis
  // rather than emitting NaN.
  const fallbackUp: Vec3 = Math.abs(forward[1]) > 0.999 ? [0, 0, 1] : WORLD_UP;
  const usableUp = vLen(vCross(forward, up)) < 1e-6 ? fallbackUp : up;
  const right = vNormalize(vCross(forward, usableUp));
  const camUp = vCross(right, forward);
  // Columns of the camera's rotation matrix: r0 = right, r1 = camUp, r2 = −forward.
  const r2: Vec3 = [-forward[0], -forward[1], -forward[2]];

  const sinY = clamp(r2[0], -1, 1);
  const y = Math.asin(sinY);
  let x: number;
  let z: number;
  if (Math.abs(sinY) < 1 - 1e-6) {
    x = Math.atan2(-r2[1], r2[2]);
    z = Math.atan2(-camUp[0], right[0]);
  } else {
    // Gimbal lock: X and Z are coupled, so X is pinned to 0 and Z absorbs the combined angle. There
    // is no second branch to prefer here.
    x = 0;
    z = Math.atan2(right[1], camUp[1]);
  }
  return { translation: position, rotation: preferNoRoll(x * RAD2DEG, y * RAD2DEG, z * RAD2DEG) };
}

/** Picks the Euler branch with the smaller |roll|. Both encode the same rotation. */
function preferNoRoll(x: number, y: number, z: number): Vec3 {
  const roll = wrapDeg(z);
  if (Math.abs(roll) <= 90) {
    return [wrapDeg(x), wrapDeg(y), roll];
  }
  return [wrapDeg(x + 180), wrapDeg(180 - y), wrapDeg(roll + 180)];
}

// --- Orbit state ------------------------------------------------------------------------------ //

/**
 * The interactive camera state. The camera sits at
 * `target + distance · (cos el · sin az, sin el, cos el · cos az)` and looks at `target`, with world
 * up — so `az = 0` puts it on +Z, `az = 180` on −Z (the sheet's FRONT), `az = −90` on −X (LEFT), and
 * `el = 90` straight above.
 */
export interface OrbitState {
  readonly target: Vec3;
  readonly distance: number;
  readonly azimuthDeg: number;
  readonly elevationDeg: number;
}

/** The unit direction from the target toward the camera for an orbit state. */
export function orbitDirection(azimuthDeg: number, elevationDeg: number): Vec3 {
  const az = azimuthDeg * DEG2RAD;
  const el = elevationDeg * DEG2RAD;
  const horizontal = Math.cos(el);
  return [horizontal * Math.sin(az), Math.sin(el), horizontal * Math.cos(az)];
}

/** The pose an orbit state puts the camera in. */
export function orbitPose(state: OrbitState): CameraPose {
  const direction = orbitDirection(state.azimuthDeg, state.elevationDeg);
  const position = vAdd(state.target, vScale(direction, state.distance));
  return lookAtPose(position, state.target, WORLD_UP);
}

/** The orbit state that puts the camera in `direction` from `target` — the inverse of
 *  `orbitDirection`, with elevation clamped off the poles. */
export function orbitFromDirection(direction: Vec3, target: Vec3, distance: number): OrbitState {
  const unit = safeNormalize(direction, [0, 0, -1]);
  return {
    target,
    distance: clamp(distance, MIN_ORBIT_DISTANCE, MAX_ORBIT_DISTANCE),
    azimuthDeg: wrapDeg(Math.atan2(unit[0], unit[2]) * RAD2DEG),
    elevationDeg: clamp(Math.asin(clamp(unit[1], -1, 1)) * RAD2DEG, -ELEVATION_LIMIT_DEG, ELEVATION_LIMIT_DEG),
  };
}

/** Orbit by a pointer drag in pixels. Dragging right walks the camera to its own right (so the
 *  subject appears to turn left); dragging down lifts it over the top. */
export function orbitDrag(
  state: OrbitState,
  dx: number,
  dy: number,
  degPerPixel = ORBIT_DEG_PER_PIXEL,
): OrbitState {
  return {
    ...state,
    azimuthDeg: wrapDeg(state.azimuthDeg + dx * degPerPixel),
    elevationDeg: clamp(
      state.elevationDeg + dy * degPerPixel,
      -ELEVATION_LIMIT_DEG,
      ELEVATION_LIMIT_DEG,
    ),
  };
}

/**
 * Pan by a pointer drag in pixels: the target slides in the camera's own plane so the subject tracks
 * the cursor. One pixel is `2·distance·tan(fov/2) / viewport.height` world units — exact at the
 * target's depth, which is where the subject is.
 */
export function panDrag(
  state: OrbitState,
  dx: number,
  dy: number,
  viewport: Viewport,
  fovYDeg = STAGE_FOV_Y_DEG,
): OrbitState {
  if (viewport.height <= 0) {
    return state;
  }
  const worldPerPixel =
    (2 * state.distance * Math.tan(fovYDeg * 0.5 * DEG2RAD)) / viewport.height;
  const direction = orbitDirection(state.azimuthDeg, state.elevationDeg);
  const forward: Vec3 = [-direction[0], -direction[1], -direction[2]];
  const right = vNormalize(vCross(forward, WORLD_UP));
  const camUp = vCross(right, forward);
  const target = vAdd(
    vAdd(state.target, vScale(right, -dx * worldPerPixel)),
    vScale(camUp, dy * worldPerPixel),
  );
  return { ...state, target };
}

/** Dolly by `steps` (positive pulls back), multiplicatively so every step feels the same. */
export function dollyBy(state: OrbitState, steps: number, perStep = DOLLY_PER_STEP): OrbitState {
  const distance = clamp(
    state.distance * Math.pow(perStep, steps),
    MIN_ORBIT_DISTANCE,
    MAX_ORBIT_DISTANCE,
  );
  return { ...state, distance };
}

/** Advance the turntable one tick: azimuth only, so the subject spins under a fixed elevation. */
export function turntableStep(state: OrbitState, degrees: number): OrbitState {
  return { ...state, azimuthDeg: wrapDeg(state.azimuthDeg + degrees) };
}

// --- Presets ---------------------------------------------------------------------------------- //

/** The five viewport presets. Ids are `ModelView::name`'s tokens, so `--views <id>` renders the
 *  matching contact-sheet cell. */
export type PresetId = 'front' | 'left' | 'top' | 'iso' | 'game';

export interface Preset {
  readonly id: PresetId;
  /** The label BURNED INTO the contact sheet's own cell, byte-identical to `ModelView::label` — the
   *  string that makes "the same view" checkable between the sheet and this viewport. */
  readonly label: string;
  /** The same label without its axis hint, for a narrow button row. */
  readonly short: string;
  /** Unit direction from the subject toward the camera (`ModelView::direction_and_up`). */
  readonly direction: Vec3;
  /** The world up the view frames with. TOP's is +Z, not +Y: looking straight down, world up is
   *  degenerate, and +Z makes the authored front (world −Z) read toward the bottom of the frame —
   *  the floor-plan convention the sheet uses. */
  readonly up: Vec3;
}

/** `GAME 58`'s direction, built exactly as `ModelView::Game` does: a 58°-down pitch over a
 *  horizontal aim at the authored front (world −Z) with a slight +X three-quarter. */
function gameDirection(): Vec3 {
  const pitch = 58 * DEG2RAD;
  const horizontal = vNormalize([0.45, 0, -1]);
  return vAdd(vScale(horizontal, Math.cos(pitch)), vScale(WORLD_UP, Math.sin(pitch)));
}

/** The presets, in the order the button row shows them. */
export const PRESETS: readonly Preset[] = [
  // The camera sits on −Z: the mesher lays authored row z = 0 at world −Z, so −Z is what sees the
  // authored front. The label keeps the AUTHORING axis (+Z) because that is the axis the author
  // reasons in — the sheet has the same apparent contradiction, deliberately.
  { id: 'front', label: 'FRONT +Z', short: 'FRONT', direction: [0, 0, -1], up: WORLD_UP },
  { id: 'left', label: 'LEFT -X', short: 'LEFT', direction: [-1, 0, 0], up: WORLD_UP },
  { id: 'top', label: 'TOP Y', short: 'TOP', direction: [0, 1, 0], up: [0, 0, 1] },
  { id: 'iso', label: 'ISO', short: 'ISO', direction: vNormalize([1, 0.75, 1]), up: WORLD_UP },
  { id: 'game', label: 'GAME 58', short: 'GAME 58', direction: gameDirection(), up: WORLD_UP },
];

/** Looks a preset up by id. */
export function preset(id: PresetId): Preset {
  return PRESETS.find((entry) => entry.id === id) ?? PRESETS[0];
}

/** The preset's exact pose, including TOP's +Z framing up — this is the sheet's cell, reproduced. */
export function presetPose(id: PresetId, target: Vec3, distance: number): CameraPose {
  const { direction, up } = preset(id);
  return lookAtPose(vAdd(target, vScale(direction, distance)), target, up);
}

/** The orbit state a preset hands over to the mouse. Y-up, so TOP's floor-plan roll is a property of
 *  the preset pose alone: the first drag after TOP leaves it, which is the honest behaviour — you
 *  cannot orbit through a pole. */
export function presetOrbit(id: PresetId, target: Vec3, distance: number): OrbitState {
  return orbitFromDirection(preset(id).direction, target, distance);
}

// --- Framing ---------------------------------------------------------------------------------- //

/** A world-space bounding sphere — what the camera is asked to fit. */
export interface Sphere {
  readonly center: Vec3;
  readonly radius: number;
}

/**
 * `fill-frame` frames the subject alone, so a 4-cell prop and a 40-cell building look the same size
 * — the view for reading SHAPE. `declared-scale` frames the subject together with the stage's
 * `adventurer` reference, so apparent size tracks real size and the model stands beside the figure
 * at its declared scale — the size judgment a contact sheet cannot give, and the reason the format has
 * a `scale` key at all.
 */
export type FramingMode = 'fill-frame' | 'declared-scale';

/**
 * The world bounding sphere of a voxel model of `size` cells placed at `translation` with uniform
 * `scale`.
 *
 * The model-space AABB is the mesher's bottom-center, one-cell-per-unit box
 * (`[-sx/2, sx/2] × [0, sy] × [-sz/2, sz/2]`, `model_sheet.rs`), so the center sits half the scaled
 * height above the placement and the radius is half the scaled diagonal. `model-stage.yaml` derives
 * its light rig from this same number: `0.5·|(9,9,9)|·0.24 = 1.871`.
 */
export function modelSphere(size: Vec3, scale: number, translation: Vec3 = [0, 0, 0]): Sphere {
  const [sx, sy, sz] = [size[0] * scale, size[1] * scale, size[2] * scale];
  return {
    center: [translation[0], translation[1] + sy * 0.5, translation[2]],
    radius: 0.5 * Math.hypot(sx, sy, sz),
  };
}

/**
 * The bounding sphere of a world-space box: its middle, and half its diagonal.
 *
 * The framing formulas take a sphere, and a part of an assembly arrives as a box (`partWorldBox`).
 * Taking `min`/`max` rather than a box type keeps this module free of the assembly reader — it knows
 * about world-space geometry and nothing about documents.
 */
export function boxSphere(min: Vec3, max: Vec3): Sphere {
  const span: Vec3 = [max[0] - min[0], max[1] - min[1], max[2] - min[2]];
  return {
    center: [min[0] + span[0] * 0.5, min[1] + span[1] * 0.5, min[2] + span[2] * 0.5],
    radius: 0.5 * Math.hypot(span[0], span[1], span[2]),
  };
}

/** The smallest sphere containing both. */
export function mergeSpheres(a: Sphere, b: Sphere): Sphere {
  const between = vSub(b.center, a.center);
  const distance = vLen(between);
  if (distance + b.radius <= a.radius) {
    return a;
  }
  if (distance + a.radius <= b.radius) {
    return b;
  }
  const radius = (distance + a.radius + b.radius) * 0.5;
  return { center: vAdd(a.center, vScale(between, (radius - a.radius) / distance)), radius };
}

/**
 * The camera distance that fits a sphere of `radius` in `viewport`, with the sheet's 10% margin.
 *
 * `model_sheet.rs`'s `fit_distance`, scaled out of unit-sphere space: the limiting half-angle is the
 * smaller of the vertical and horizontal half-FOVs, so a wide viewport is limited vertically and a
 * tall one horizontally. Clamped to the orbit range, so a model too large to frame parks at the
 * bound rather than sailing past the stage camera's far plane.
 */
export function fitDistance(radius: number, viewport: Viewport, fovYDeg = STAGE_FOV_Y_DEG): number {
  const halfY = fovYDeg * 0.5 * DEG2RAD;
  const aspect = viewport.height <= 0 ? 1 : viewport.width / viewport.height;
  const halfX = Math.atan(Math.tan(halfY) * aspect);
  const limiting = Math.min(halfY, halfX);
  return clamp(
    (FIT_MARGIN * radius) / Math.sin(limiting),
    MIN_ORBIT_DISTANCE,
    MAX_ORBIT_DISTANCE,
  );
}

/**
 * Where to look and from how far, for a framing mode. `reference` is the stage's scale figure; it is
 * ignored by `fill-frame`, and `declared-scale` falls back to the subject alone when it is `null`.
 */
export function framing(
  mode: FramingMode,
  subject: Sphere,
  reference: Sphere | null,
  viewport: Viewport,
  fovYDeg = STAGE_FOV_Y_DEG,
): { readonly target: Vec3; readonly distance: number } {
  const sphere =
    mode === 'declared-scale' && reference !== null ? mergeSpheres(subject, reference) : subject;
  return { target: sphere.center, distance: fitDistance(sphere.radius, viewport, fovYDeg) };
}

// --- Small helpers ---------------------------------------------------------------------------- //

function clamp(value: number, lo: number, hi: number): number {
  return Math.min(hi, Math.max(lo, value));
}

/** Wraps degrees into `(-180, 180]`, so `-180` normalizes to `180`. */
function wrapDeg(degrees: number): number {
  const wrapped = (((degrees + 180) % 360) + 360) % 360 - 180;
  return wrapped === -180 ? 180 : wrapped;
}

function safeNormalize(v: Vec3, fallback: Vec3): Vec3 {
  return vLen(v) < 1e-9 ? fallback : vNormalize(v);
}
