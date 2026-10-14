// The camera projection kit: world ⇄ screen the exact way the engine does it, so gizmo handles land
// on the entity to the pixel. Every formula here mirrors `crates/ludion-engine/src/extract.rs`
// (`build_camera`, `euler_quat`) and `crates/ludion-engine/src/bounds.rs` (`project`) against
// `glam` 0.30 — see vecmath.ts for the matrix/quaternion conventions this builds on.
//
// Two coordinate spaces are in play. WORLD space is right-handed metres; the camera sits at its
// `Transform` and looks down its local −Z. SCREEN space is engine pixels: origin top-left, +Y down,
// sized to the engine render target (which equals the canvas's intrinsic size — the same "engine
// pixels" `picking.ts` speaks). NDC (clip/w) is the wgpu/DX convention with 0..1 depth (glam's `_rh`
// matrices, not `_rh_gl`). A point is "behind the camera" when its clip `w ≤ 1e-6` — identical to
// `bounds.rs`'s `BEHIND_CAMERA_W`, and for orthographic (`w == 1`) nothing is ever behind.
//
// Parsers at the bottom take `unknown` wire component values (the wiring task feeds them
// `component.get`/`query.entities` bodies) and return typed, fully-defaulted data. Every field of
// both `Transform` and `Camera` has a serde default, so an absent field is normal, not an error.

import type { Point } from '../../viewer/picking';
import {
  IDENTITY4,
  mat4FromRotationTranslation,
  mat4FromColumns,
  mat4Inverse,
  mat4Multiply,
  mat4MulVec4,
  quatFromAxisAngle,
  quatMul,
  quatNormalize,
  vAdd,
  vLen,
  vNormalize,
  vSub,
  type Mat4,
  type Quat,
  type Vec3,
} from './vecmath';

/** A clip `w` at or below this is treated as behind the camera (matches `bounds.rs`). */
const BEHIND_CAMERA_W = 1e-6;

/** The engine render size, in pixels — the screen space every projection here targets. */
export interface Viewport {
  readonly width: number;
  readonly height: number;
}

/** A parsed `Transform` component value: translation, Euler-XYZ-degree rotation, per-axis scale. */
export interface Transform {
  readonly translation: Vec3;
  /** Euler XYZ angles in DEGREES (intrinsic), matching the `Transform.rotation` field. */
  readonly rotation: Vec3;
  readonly scale: Vec3;
}

/** The camera's projection, discriminated the way the wire enum is (`type: perspective|orthographic`). */
export type Projection =
  | { readonly kind: 'perspective'; readonly fovYDeg: number }
  | { readonly kind: 'orthographic'; readonly height: number };

/** A parsed `Camera` component value. */
export interface CameraParams {
  readonly projection: Projection;
  readonly near: number;
  readonly far: number;
}

/** A picking ray in world space: `origin` on the near plane, `dir` a unit vector toward `far`. */
export interface Ray {
  readonly origin: Vec3;
  readonly dir: Vec3;
}

/**
 * Everything the overlay needs from the camera, built once per frame. `view`/`viewProj` project
 * world→clip; `invViewProj` unprojects screen→world; `camRight`/`camUp`/`camForward` are the camera's
 * world-space basis (columns of the camera model matrix) — used for constant-apparent-size sizing and
 * screen-facing handles. `camForward` points where the camera looks (its local −Z in world).
 */
export interface CameraKit {
  readonly view: Mat4;
  readonly viewProj: Mat4;
  readonly invViewProj: Mat4;
  readonly cameraPos: Vec3;
  readonly camRight: Vec3;
  readonly camUp: Vec3;
  readonly camForward: Vec3;
}

const DEG2RAD = Math.PI / 180;
const RAD2DEG = 180 / Math.PI;

// --- Euler ⇄ quaternion (must match glam `Quat::from_euler(EulerRot::XYZ, …)`) ----------------- //

/**
 * A quaternion from Euler XYZ angles in degrees, intrinsic — glam's `EulerRot::XYZ`, whose matrix is
 * `Rx·Ry·Rz` (column vectors), i.e. the quaternion product `qx * qy * qz`. This may differ in overall
 * sign from glam's internal Shoemake result, but `q` and `−q` are the same rotation, so the derived
 * rotation matrix (all this feeds) is identical.
 */
export function eulerXyzDegToQuat(eulerDeg: Vec3): Quat {
  const qx = quatFromAxisAngle([1, 0, 0], eulerDeg[0] * DEG2RAD);
  const qy = quatFromAxisAngle([0, 1, 0], eulerDeg[1] * DEG2RAD);
  const qz = quatFromAxisAngle([0, 0, 1], eulerDeg[2] * DEG2RAD);
  return quatMul(quatMul(qx, qy), qz);
}

/**
 * Extracts Euler XYZ degrees from a quaternion. The branch choices need not match glam's; the
 * contract is the round-trip `eulerXyzDegToQuat(quatToEulerXyzDeg(q)) ≈ ±q`, held even at the
 * y = ±90° gimbal singularity (there the X and Z rotations fold together — X is pinned to 0 and Z
 * absorbs the combined angle). Derived from the `Rx·Ry·Rz` matrix entries.
 */
export function quatToEulerXyzDeg(q: Quat): Vec3 {
  const [x, y, z, w] = quatNormalize(q);
  const x2 = x + x;
  const y2 = y + y;
  const z2 = z + z;
  const xx = x * x2;
  const xy = x * y2;
  const xz = x * z2;
  const yy = y * y2;
  const yz = y * z2;
  const zz = z * z2;
  const wx = w * x2;
  const wy = w * y2;
  const wz = w * z2;
  // Rotation matrix entries R[row][col] (from glam's quat_to_axes columns).
  const r00 = 1 - (yy + zz);
  const r01 = xy - wz;
  const r02 = xz + wy;
  const r12 = yz - wx;
  const r22 = 1 - (xx + yy);
  const r10 = xy + wz;
  const r11 = 1 - (xx + zz);
  // sin(pitch) = R[0][2]; clamp against round-off before asin.
  const sb = Math.max(-1, Math.min(1, r02));
  const b = Math.asin(sb); // clamped input: never NaN
  let a: number;
  let c: number;
  if (Math.abs(sb) < 1 - 1e-6) {
    a = Math.atan2(-r12, r22); // roll about X
    c = Math.atan2(-r01, r00); // yaw about Z
  } else {
    // Gimbal lock (pitch ≈ ±90°): X and Z are coupled; pin X = 0 and let Z take the sum/difference.
    a = 0;
    c = Math.atan2(r10, r11);
  }
  return [a * RAD2DEG, b * RAD2DEG, c * RAD2DEG];
}

/**
 * Composes a world-axis rotation of `angleDeg` about `axis` onto a base Euler-XYZ-degree orientation
 * and returns the new Euler degrees: `axisAngleQuat * baseQuat` (world-frame pre-multiply). Used by
 * the rotate gizmo, where the ring's axis is a fixed world axis.
 */
export function applyWorldAxisRotation(baseEulerDeg: Vec3, axis: Vec3, angleDeg: number): Vec3 {
  const base = eulerXyzDegToQuat(baseEulerDeg);
  const delta = quatFromAxisAngle(vNormalize(axis), angleDeg * DEG2RAD);
  return quatToEulerXyzDeg(quatMul(delta, base));
}

// --- Projection kit ---------------------------------------------------------------------------- //

/** `aspect = width/height`, guarding a zero height exactly as `ludion_renderer::aspect_ratio`. */
function aspectRatio(viewport: Viewport): number {
  return viewport.height <= 0 ? 1 : viewport.width / viewport.height;
}

/** `glam::Mat4::perspective_rh(fovYRad, aspect, near, far)` (0..1 depth). */
function perspectiveRh(fovYRad: number, aspect: number, near: number, far: number): Mat4 {
  const h = Math.cos(fovYRad * 0.5) / Math.sin(fovYRad * 0.5);
  const w = h / aspect;
  const r = far / (near - far);
  return mat4FromColumns([w, 0, 0, 0], [0, h, 0, 0], [0, 0, r, -1], [0, 0, r * near, 0]);
}

/** `glam::Mat4::orthographic_rh(l, r, b, t, near, far)` (0..1 depth); here symmetric about origin. */
function orthographicRh(halfW: number, halfH: number, near: number, far: number): Mat4 {
  const rcpW = 1 / (2 * halfW);
  const rcpH = 1 / (2 * halfH);
  const r = 1 / (near - far);
  // left=-halfW,right=halfW,bottom=-halfH,top=halfH ⇒ the (left+right)/(top+bottom) terms vanish.
  return mat4FromColumns(
    [2 * rcpW, 0, 0, 0],
    [0, 2 * rcpH, 0, 0],
    [0, 0, r, 0],
    [0, 0, r * near, 1],
  );
}

/**
 * Builds the camera matrices from a parsed `Transform` and `Camera`, mirroring `build_camera`:
 * `model = from_rotation_translation(eulerQuat(rotation), translation)` (camera scale ignored),
 * `view = model⁻¹`, projection per the wire enum. Degenerate inputs (singular matrices) fall back to
 * zero/identity so downstream `worldToScreen`/`screenToRay` return `null` rather than emit `NaN`.
 */
export function buildViewProj(cameraTransform: Transform, camera: CameraParams, viewport: Viewport): CameraKit {
  const quat = eulerXyzDegToQuat(cameraTransform.rotation);
  const model = mat4FromRotationTranslation(quat, cameraTransform.translation);
  const view = mat4Inverse(model) ?? IDENTITY4;
  const aspect = aspectRatio(viewport);
  const projection =
    camera.projection.kind === 'perspective'
      ? perspectiveRh(camera.projection.fovYDeg * DEG2RAD, aspect, camera.near, camera.far)
      : orthographicRh((camera.projection.height * 0.5) * aspect, camera.projection.height * 0.5, camera.near, camera.far);
  const viewProj = mat4Multiply(projection, view);
  const invViewProj = mat4Inverse(viewProj) ?? new Array<number>(16).fill(0);
  // Camera world basis = columns of the model matrix (col2 is +Z; the camera looks down −Z).
  const camRight = vNormalize([model[0], model[1], model[2]]);
  const camUp = vNormalize([model[4], model[5], model[6]]);
  const camForward = vNormalize([-model[8], -model[9], -model[10]]);
  return {
    view,
    viewProj,
    invViewProj,
    cameraPos: cameraTransform.translation,
    camRight,
    camUp,
    camForward,
  };
}

/**
 * Projects a world point to engine-pixel screen space, or `null` when it is behind the camera
 * (`clip.w ≤ 1e-6`). Identical arithmetic to `bounds.rs`'s `project` per corner.
 */
export function worldToScreen(p: Vec3, viewProj: Mat4, size: Viewport): Point | null {
  const clip = mat4MulVec4(viewProj, [p[0], p[1], p[2], 1]);
  if (clip[3] <= BEHIND_CAMERA_W) {
    return null;
  }
  const ndcX = clip[0] / clip[3];
  const ndcY = clip[1] / clip[3];
  return {
    x: (ndcX * 0.5 + 0.5) * size.width,
    y: (1 - (ndcY * 0.5 + 0.5)) * size.height,
  };
}

/** Unprojects a screen pixel at a given NDC depth to a world point, or `null` on a degenerate `w`. */
function unproject(point: Point, ndcZ: number, size: Viewport, invViewProj: Mat4): Vec3 | null {
  if (size.width <= 0 || size.height <= 0) {
    return null;
  }
  const ndcX = (point.x / size.width) * 2 - 1;
  const ndcY = (1 - point.y / size.height) * 2 - 1;
  const h = mat4MulVec4(invViewProj, [ndcX, ndcY, ndcZ, 1]);
  if (Math.abs(h[3]) < 1e-12) {
    return null;
  }
  return [h[0] / h[3], h[1] / h[3], h[2] / h[3]];
}

/**
 * A world-space picking ray through the pixel `point`: unprojects it at NDC depth 0 (near plane) for
 * the origin and depth 1 (far plane) for direction. This is uniform across perspective and
 * orthographic — for orthographic the two points differ by the (parallel) view direction. Returns
 * `null` when either unprojection is degenerate or the two points coincide.
 */
export function screenToRay(point: Point, size: Viewport, invViewProj: Mat4): Ray | null {
  const near = unproject(point, 0, size, invViewProj);
  const far = unproject(point, 1, size, invViewProj);
  if (near === null || far === null) {
    return null;
  }
  const delta = vSub(far, near);
  if (vLen(delta) < 1e-12) {
    return null;
  }
  return { origin: near, dir: vNormalize(delta) };
}

// --- Parsers (unknown → typed, defaults applied) ---------------------------------------------- //
//
// Same discipline as picking.ts: `unknown` in, typed-or-`null` out. Because every component field
// has a serde default, an ABSENT field is filled with its default; a PRESENT-but-malformed field
// fails the whole parse (returns `null`) so bad wire data surfaces rather than silently defaulting.

function asFiniteNumber(v: unknown): number | null {
  return typeof v === 'number' && Number.isFinite(v) ? v : null;
}

/** Parses a fixed-length numeric array, or `null` if present-but-wrong; `undefined` → `fallback`. */
function parseVecN<N extends number>(v: unknown, n: N, fallback: readonly number[]): number[] | null {
  if (v === undefined) {
    return [...fallback];
  }
  if (!Array.isArray(v) || v.length !== n) {
    return null;
  }
  const out: number[] = [];
  for (const e of v) {
    const num = asFiniteNumber(e);
    if (num === null) {
      return null;
    }
    out.push(num);
  }
  return out;
}

/** Reads an optional numeric field: `undefined` → `fallback`, present-but-non-numeric → `null`. */
function parseNumberField(v: unknown, fallback: number): number | null {
  if (v === undefined) {
    return fallback;
  }
  return asFiniteNumber(v);
}

/**
 * Parses a `Transform` component value (`{ translation?, rotation?, scale? }`) with defaults
 * `[0,0,0] / [0,0,0] / [1,1,1]`. `null` if the value is not an object, or any present field is
 * malformed.
 */
export function parseTransform(v: unknown): Transform | null {
  if (typeof v !== 'object' || v === null) {
    return null;
  }
  const o = v as { translation?: unknown; rotation?: unknown; scale?: unknown };
  const translation = parseVecN(o.translation, 3, [0, 0, 0]);
  const rotation = parseVecN(o.rotation, 3, [0, 0, 0]);
  const scale = parseVecN(o.scale, 3, [1, 1, 1]);
  if (translation === null || rotation === null || scale === null) {
    return null;
  }
  return {
    translation: [translation[0], translation[1], translation[2]],
    rotation: [rotation[0], rotation[1], rotation[2]],
    scale: [scale[0], scale[1], scale[2]],
  };
}

/**
 * Parses a `Camera` component value (`{ projection?, near?, far? }`) with defaults 60° perspective,
 * near 0.1, far 1000. The projection is the internally-tagged wire enum
 * (`{ type: 'perspective', fov_y_deg } | { type: 'orthographic', height }`); an absent projection
 * defaults to 60° perspective. `null` on any malformed present field.
 */
export function parseCamera(v: unknown): CameraParams | null {
  if (typeof v !== 'object' || v === null) {
    return null;
  }
  const o = v as { projection?: unknown; near?: unknown; far?: unknown };
  const near = parseNumberField(o.near, 0.1);
  const far = parseNumberField(o.far, 1000);
  if (near === null || far === null) {
    return null;
  }
  const projection = parseProjection(o.projection);
  if (projection === null) {
    return null;
  }
  return { projection, near, far };
}

function parseProjection(v: unknown): Projection | null {
  if (v === undefined) {
    return { kind: 'perspective', fovYDeg: 60 };
  }
  if (typeof v !== 'object' || v === null) {
    return null;
  }
  // Unlike the outer components, the variant payload fields have NO serde default in the Rust enum
  // (`Perspective { fov_y_deg }` / `Orthographic { height }`), so a present projection missing its
  // payload is malformed wire data, not a defaultable absence.
  const o = v as { type?: unknown; fov_y_deg?: unknown; height?: unknown };
  if (o.type === 'perspective') {
    const fovYDeg = asFiniteNumber(o.fov_y_deg);
    return fovYDeg === null ? null : { kind: 'perspective', fovYDeg };
  }
  if (o.type === 'orthographic') {
    const height = asFiniteNumber(o.height);
    return height === null ? null : { kind: 'orthographic', height };
  }
  return null;
}

/** The unit world-space direction of a named axis — shared by the layout and drag modules. */
export function axisVector(axis: 'x' | 'y' | 'z'): Vec3 {
  return axis === 'x' ? [1, 0, 0] : axis === 'y' ? [0, 1, 0] : [0, 0, 1];
}

/**
 * The world length that projects to `pixels` screen pixels at world point `at`, or `null` when `at`
 * is behind the camera or the projection is degenerate. Works for both perspective and orthographic:
 * it offsets `at` by a unit step along the camera-right axis (exactly perpendicular to the view
 * direction, so screen distance is linear in the offset) and measures the projected pixel span.
 */
export function worldLengthForPixels(at: Vec3, pixels: number, kit: CameraKit, size: Viewport): number | null {
  const s0 = worldToScreen(at, kit.viewProj, size);
  const s1 = worldToScreen(vAdd(at, kit.camRight), kit.viewProj, size);
  if (s0 === null || s1 === null) {
    return null;
  }
  const pixelSpan = Math.hypot(s1.x - s0.x, s1.y - s0.y);
  if (pixelSpan < 1e-9) {
    return null;
  }
  return pixels / pixelSpan;
}
