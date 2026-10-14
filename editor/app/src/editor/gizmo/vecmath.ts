// A minimal column-major 3D math kit — vectors, quaternions, and 4x4 matrices — reproducing the
// exact conventions of `glam` 0.30 (the engine's math crate) so the editor's gizmo overlay projects
// world geometry to screen with byte-for-byte the same formulas the renderer uses. Matching glam
// matters: a handle drawn even one convention off (row/column major, a flipped quaternion sign, the
// wrong NDC depth range) would sit beside the entity instead of on it. There is deliberately no npm
// math dependency — the whole surface is small, and owning it lets us pin the conventions in tests
// against hand-computed and glam-derived fixtures (see vecmath.test.ts).
//
// Conventions, all mirrored from glam:
//   - Matrices are COLUMN-MAJOR, stored flat: `m[col*4 + row]`. `Mat4::from_cols(c0,c1,c2,c3)` lays
//     the four columns end to end, so `mat4FromColumns` takes columns, not rows.
//   - Points/vectors are column vectors; `M * v` is `mat4MulVec4`. Composition `A * B` means "apply B
//     first" (`mat4Multiply(a, b)`), matching glam operator order.
//   - Quaternions are `[x, y, z, w]`. `quatMul` is the Hamilton product with glam's exact sign
//     layout, so `quatMul(a, b)` rotates a vector as `a * (b * v)` — b applied first.
//   - Right-handed throughout. No perspective divide happens here: `mat4MulVec4` returns the raw
//     homogeneous vec4 and the caller (camera.ts) divides, exactly as `bounds.rs` does.

/** A 3D vector or point `[x, y, z]` (tuple to match `picking.ts`'s `parseVec3` output). */
export type Vec3 = readonly [number, number, number];

/** A homogeneous 4D vector `[x, y, z, w]`. */
export type Vec4 = readonly [number, number, number, number];

/** A quaternion `[x, y, z, w]` (glam's storage order — `w` last). */
export type Quat = readonly [number, number, number, number];

/** A 4x4 matrix, column-major, flat length-16: element `(row r, col c)` is `m[c*4 + r]`. */
export type Mat4 = readonly number[];

/** The 4x4 identity. */
export const IDENTITY4: Mat4 = [1, 0, 0, 0, 0, 1, 0, 0, 0, 0, 1, 0, 0, 0, 0, 1];

// --- Vec3 ------------------------------------------------------------------------------------- //

export function vAdd(a: Vec3, b: Vec3): Vec3 {
  return [a[0] + b[0], a[1] + b[1], a[2] + b[2]];
}

export function vSub(a: Vec3, b: Vec3): Vec3 {
  return [a[0] - b[0], a[1] - b[1], a[2] - b[2]];
}

export function vScale(a: Vec3, s: number): Vec3 {
  return [a[0] * s, a[1] * s, a[2] * s];
}

export function vDot(a: Vec3, b: Vec3): number {
  return a[0] * b[0] + a[1] * b[1] + a[2] * b[2];
}

export function vCross(a: Vec3, b: Vec3): Vec3 {
  return [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]];
}

export function vLen(a: Vec3): number {
  return Math.hypot(a[0], a[1], a[2]);
}

/** Normalizes `a`; returns `[0,0,0]` for a (near-)zero vector rather than `NaN` — callers that care
 *  about degeneracy test `vLen` first (see the drag solvers). */
export function vNormalize(a: Vec3): Vec3 {
  const len = vLen(a);
  return len > 1e-12 ? [a[0] / len, a[1] / len, a[2] / len] : [0, 0, 0];
}

/** Whether every component of a Vec3 is a finite number (the finiteness gate for drag output). */
export function vFinite(a: Vec3): boolean {
  return Number.isFinite(a[0]) && Number.isFinite(a[1]) && Number.isFinite(a[2]);
}

// --- Mat4 ------------------------------------------------------------------------------------- //

/** Builds a Mat4 from its four columns (each a Vec4), matching `glam::Mat4::from_cols`. */
export function mat4FromColumns(c0: Vec4, c1: Vec4, c2: Vec4, c3: Vec4): Mat4 {
  return [...c0, ...c1, ...c2, ...c3];
}

/** Matrix product `a * b` (column-major): `(a*b) * v == a * (b * v)`. */
export function mat4Multiply(a: Mat4, b: Mat4): Mat4 {
  const out = new Array<number>(16);
  for (let c = 0; c < 4; c++) {
    for (let r = 0; r < 4; r++) {
      out[c * 4 + r] = a[r] * b[c * 4] + a[4 + r] * b[c * 4 + 1] + a[8 + r] * b[c * 4 + 2] + a[12 + r] * b[c * 4 + 3];
    }
  }
  return out;
}

/** Transforms a homogeneous vec4 by `m` (no perspective divide — the caller divides). */
export function mat4MulVec4(m: Mat4, v: Vec4): Vec4 {
  return [
    m[0] * v[0] + m[4] * v[1] + m[8] * v[2] + m[12] * v[3],
    m[1] * v[0] + m[5] * v[1] + m[9] * v[2] + m[13] * v[3],
    m[2] * v[0] + m[6] * v[1] + m[10] * v[2] + m[14] * v[3],
    m[3] * v[0] + m[7] * v[1] + m[11] * v[2] + m[15] * v[3],
  ];
}

/**
 * The inverse of `m`, or `null` when `m` is singular (determinant 0). Cofactor expansion in
 * column-major layout — the classic MESA `gluInvertMatrix` indices, which are layout-agnostic
 * (transposing the input transposes the output), so they yield the column-major inverse directly.
 */
export function mat4Inverse(m: Mat4): Mat4 | null {
  const inv = new Array<number>(16);
  inv[0] = m[5] * m[10] * m[15] - m[5] * m[11] * m[14] - m[9] * m[6] * m[15] + m[9] * m[7] * m[14] + m[13] * m[6] * m[11] - m[13] * m[7] * m[10];
  inv[4] = -m[4] * m[10] * m[15] + m[4] * m[11] * m[14] + m[8] * m[6] * m[15] - m[8] * m[7] * m[14] - m[12] * m[6] * m[11] + m[12] * m[7] * m[10];
  inv[8] = m[4] * m[9] * m[15] - m[4] * m[11] * m[13] - m[8] * m[5] * m[15] + m[8] * m[7] * m[13] + m[12] * m[5] * m[11] - m[12] * m[7] * m[9];
  inv[12] = -m[4] * m[9] * m[14] + m[4] * m[10] * m[13] + m[8] * m[5] * m[14] - m[8] * m[6] * m[13] - m[12] * m[5] * m[10] + m[12] * m[6] * m[9];
  inv[1] = -m[1] * m[10] * m[15] + m[1] * m[11] * m[14] + m[9] * m[2] * m[15] - m[9] * m[3] * m[14] - m[13] * m[2] * m[11] + m[13] * m[3] * m[10];
  inv[5] = m[0] * m[10] * m[15] - m[0] * m[11] * m[14] - m[8] * m[2] * m[15] + m[8] * m[3] * m[14] + m[12] * m[2] * m[11] - m[12] * m[3] * m[10];
  inv[9] = -m[0] * m[9] * m[15] + m[0] * m[11] * m[13] + m[8] * m[1] * m[15] - m[8] * m[3] * m[13] - m[12] * m[1] * m[11] + m[12] * m[3] * m[9];
  inv[13] = m[0] * m[9] * m[14] - m[0] * m[10] * m[13] - m[8] * m[1] * m[14] + m[8] * m[2] * m[13] + m[12] * m[1] * m[10] - m[12] * m[2] * m[9];
  inv[2] = m[1] * m[6] * m[15] - m[1] * m[7] * m[14] - m[5] * m[2] * m[15] + m[5] * m[3] * m[14] + m[13] * m[2] * m[7] - m[13] * m[3] * m[6];
  inv[6] = -m[0] * m[6] * m[15] + m[0] * m[7] * m[14] + m[4] * m[2] * m[15] - m[4] * m[3] * m[14] - m[12] * m[2] * m[7] + m[12] * m[3] * m[6];
  inv[10] = m[0] * m[5] * m[15] - m[0] * m[7] * m[13] - m[4] * m[1] * m[15] + m[4] * m[3] * m[13] + m[12] * m[1] * m[7] - m[12] * m[3] * m[5];
  inv[14] = -m[0] * m[5] * m[14] + m[0] * m[6] * m[13] + m[4] * m[1] * m[14] - m[4] * m[2] * m[13] - m[12] * m[1] * m[6] + m[12] * m[2] * m[5];
  inv[3] = -m[1] * m[6] * m[11] + m[1] * m[7] * m[10] + m[5] * m[2] * m[11] - m[5] * m[3] * m[10] - m[9] * m[2] * m[7] + m[9] * m[3] * m[6];
  inv[7] = m[0] * m[6] * m[11] - m[0] * m[7] * m[10] - m[4] * m[2] * m[11] + m[4] * m[3] * m[10] + m[8] * m[2] * m[7] - m[8] * m[3] * m[6];
  inv[11] = -m[0] * m[5] * m[11] + m[0] * m[7] * m[9] + m[4] * m[1] * m[11] - m[4] * m[3] * m[9] - m[8] * m[1] * m[7] + m[8] * m[3] * m[5];
  inv[15] = m[0] * m[5] * m[10] - m[0] * m[6] * m[9] - m[4] * m[1] * m[10] + m[4] * m[2] * m[9] + m[8] * m[1] * m[6] - m[8] * m[2] * m[5];

  const det = m[0] * inv[0] + m[1] * inv[4] + m[2] * inv[8] + m[3] * inv[12];
  if (det === 0 || !Number.isFinite(det)) {
    return null;
  }
  const rcp = 1 / det;
  for (let i = 0; i < 16; i++) {
    inv[i] *= rcp;
  }
  return inv;
}

/**
 * An affine matrix from a rotation quaternion and a translation, matching
 * `glam::Mat4::from_rotation_translation` (the quaternion→axes formula is glam's `quat_to_axes`).
 */
export function mat4FromRotationTranslation(q: Quat, t: Vec3): Mat4 {
  const [x, y, z, w] = q;
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
  return mat4FromColumns(
    [1 - (yy + zz), xy + wz, xz - wy, 0],
    [xy - wz, 1 - (xx + zz), yz + wx, 0],
    [xz + wy, yz - wx, 1 - (xx + yy), 0],
    [t[0], t[1], t[2], 1],
  );
}

// --- Quat ------------------------------------------------------------------------------------- //

/** The Hamilton product `a * b`, using glam's exact component layout (`scalar::Quat::mul_quat`). */
export function quatMul(a: Quat, b: Quat): Quat {
  const [x0, y0, z0, w0] = a;
  const [x1, y1, z1, w1] = b;
  return [
    w0 * x1 + x0 * w1 + y0 * z1 - z0 * y1,
    w0 * y1 - x0 * z1 + y0 * w1 + z0 * x1,
    w0 * z1 + x0 * y1 - y0 * x1 + z0 * w1,
    w0 * w1 - x0 * x1 - y0 * y1 - z0 * z1,
  ];
}

/** A unit quaternion for a rotation of `angleRad` about the (assumed unit) `axis`, matching
 *  `glam::Quat::from_axis_angle`. */
export function quatFromAxisAngle(axis: Vec3, angleRad: number): Quat {
  const s = Math.sin(angleRad * 0.5);
  const c = Math.cos(angleRad * 0.5);
  return [axis[0] * s, axis[1] * s, axis[2] * s, c];
}

/** Normalizes a quaternion; returns the identity for a (near-)zero input rather than `NaN`. */
export function quatNormalize(q: Quat): Quat {
  const len = Math.hypot(q[0], q[1], q[2], q[3]);
  return len > 1e-12 ? [q[0] / len, q[1] / len, q[2] / len, q[3] / len] : [0, 0, 0, 1];
}

/** The conjugate (inverse, for a unit quaternion) of `q`. */
export function quatConjugate(q: Quat): Quat {
  return [-q[0], -q[1], -q[2], q[3]];
}

/** Rotates `v` by the (assumed unit) quaternion `q`: `q * v * q⁻¹`, via glam's `mul_vec3` form. */
export function quatRotate(q: Quat, v: Vec3): Vec3 {
  const u: Vec3 = [q[0], q[1], q[2]];
  const t = vScale(vCross(u, v), 2);
  return vAdd(vAdd(v, vScale(t, q[3])), vCross(u, t));
}
