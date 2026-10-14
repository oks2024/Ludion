import { describe, expect, it } from 'vitest';
import {
  IDENTITY4,
  mat4FromColumns,
  mat4FromRotationTranslation,
  mat4Inverse,
  mat4Multiply,
  mat4MulVec4,
  quatConjugate,
  quatFromAxisAngle,
  quatMul,
  quatNormalize,
  quatRotate,
  vCross,
  vDot,
  vNormalize,
  type Mat4,
  type Quat,
  type Vec3,
} from './vecmath';

/** Asserts two flat matrices agree componentwise to `digits` decimal places. */
function expectMat(a: Mat4, b: Mat4, digits = 9): void {
  expect(a).toHaveLength(16);
  for (let i = 0; i < 16; i++) {
    expect(a[i]).toBeCloseTo(b[i], digits);
  }
}

describe('mat4Multiply', () => {
  it('is identity-neutral', () => {
    const m = mat4FromColumns([2, 0, 0, 0], [0, 3, 0, 0], [0, 0, 4, 0], [5, 6, 7, 1]);
    expectMat(mat4Multiply(IDENTITY4, m), m);
    expectMat(mat4Multiply(m, IDENTITY4), m);
  });

  it('composes translations (apply-b-first order)', () => {
    const t1 = mat4FromColumns([1, 0, 0, 0], [0, 1, 0, 0], [0, 0, 1, 0], [2, 3, 4, 1]);
    const t2 = mat4FromColumns([1, 0, 0, 0], [0, 1, 0, 0], [0, 0, 1, 0], [1, 1, 1, 1]);
    // Transforming the origin through t1*t2 applies t2 then t1: (0)->(1,1,1)->(3,4,5).
    expect(mat4MulVec4(mat4Multiply(t1, t2), [0, 0, 0, 1])).toEqual([3, 4, 5, 1]);
  });
});

describe('mat4MulVec4', () => {
  it('applies a scale+translation column-major matrix', () => {
    const m = mat4FromColumns([2, 0, 0, 0], [0, 3, 0, 0], [0, 0, 4, 0], [10, 20, 30, 1]);
    expect(mat4MulVec4(m, [1, 1, 1, 1])).toEqual([12, 23, 34, 1]);
  });
});

describe('mat4Inverse', () => {
  it('inverts the identity to itself', () => {
    expectMat(mat4Inverse(IDENTITY4)!, IDENTITY4);
  });

  it('inverts a rotation+translation so M * M⁻¹ = I', () => {
    const q = quatFromAxisAngle(vNormalize([1, 2, 3]), 0.7);
    const m = mat4FromRotationTranslation(q, [4, -5, 6]);
    const inv = mat4Inverse(m);
    expect(inv).not.toBeNull();
    expectMat(mat4Multiply(m, inv!), IDENTITY4, 6);
    expectMat(mat4Multiply(inv!, m), IDENTITY4, 6);
  });

  it('inverts a scale+translation matrix', () => {
    const m = mat4FromColumns([2, 0, 0, 0], [0, 4, 0, 0], [0, 0, 5, 0], [1, 2, 3, 1]);
    const inv = mat4Inverse(m)!;
    expect(mat4MulVec4(inv, mat4MulVec4(m, [7, 8, 9, 1]))).toEqual([
      expect.closeTo(7, 9),
      expect.closeTo(8, 9),
      expect.closeTo(9, 9),
      1,
    ]);
  });

  it('returns null for a singular (zero) matrix', () => {
    expect(mat4Inverse(new Array<number>(16).fill(0))).toBeNull();
  });

  it('returns null for a matrix with a zero column (rank-deficient)', () => {
    const m = mat4FromColumns([0, 0, 0, 0], [0, 1, 0, 0], [0, 0, 1, 0], [0, 0, 0, 1]);
    expect(mat4Inverse(m)).toBeNull();
  });
});

describe('quatMul', () => {
  it('has the identity quaternion as unit', () => {
    const q: Quat = quatFromAxisAngle([0, 0, 1], 0.9);
    expect(quatMul([0, 0, 0, 1], q)).toEqual(q);
    expect(quatMul(q, [0, 0, 0, 1])).toEqual(q);
  });

  it('composes two 90° axis rotations (glam Hamilton layout)', () => {
    const qx = quatFromAxisAngle([1, 0, 0], Math.PI / 2);
    const qy = quatFromAxisAngle([0, 1, 0], Math.PI / 2);
    const r = quatMul(qx, qy);
    // Hand-computed: [0.5, 0.5, 0.5, 0.5].
    expect(r[0]).toBeCloseTo(0.5, 9);
    expect(r[1]).toBeCloseTo(0.5, 9);
    expect(r[2]).toBeCloseTo(0.5, 9);
    expect(r[3]).toBeCloseTo(0.5, 9);
  });

  it('rotates a vector as a*(b*v) — b applied first', () => {
    // qz(90) then qx(90) applied to +X: qx*qz. Verify via the rotation matrix columns.
    const qx = quatFromAxisAngle([1, 0, 0], Math.PI / 2);
    const qz = quatFromAxisAngle([0, 0, 1], Math.PI / 2);
    const m = mat4FromRotationTranslation(quatMul(qx, qz), [0, 0, 0]);
    // Applying to +X: qz sends X->Y, then qx sends Y->Z. Result column0 ≈ (0,0,1).
    expect(mat4MulVec4(m, [1, 0, 0, 1])).toEqual([
      expect.closeTo(0, 6),
      expect.closeTo(0, 6),
      expect.closeTo(1, 6),
      1,
    ]);
  });
});

describe('quatFromAxisAngle / quatNormalize', () => {
  it('builds the expected half-angle quaternion', () => {
    const q = quatFromAxisAngle([0, 0, 1], Math.PI / 2);
    expect(q[0]).toBeCloseTo(0, 9);
    expect(q[1]).toBeCloseTo(0, 9);
    expect(q[2]).toBeCloseTo(Math.SQRT1_2, 9);
    expect(q[3]).toBeCloseTo(Math.SQRT1_2, 9);
  });

  it('normalizes a zero quaternion to the identity rather than NaN', () => {
    expect(quatNormalize([0, 0, 0, 0])).toEqual([0, 0, 0, 1]);
  });
});

describe('vec3 helpers', () => {
  it('cross and dot follow the right-hand rule', () => {
    expect(vCross([1, 0, 0], [0, 1, 0])).toEqual([0, 0, 1]);
    expect(vDot([1, 2, 3], [4, 5, 6])).toBe(32);
  });

  it('normalizes a zero vector to zero rather than NaN', () => {
    expect(vNormalize([0, 0, 0])).toEqual([0, 0, 0]);
  });

  it('produces a unit vector', () => {
    const n = vNormalize([3, 4, 0] as Vec3);
    expect(Math.hypot(n[0], n[1], n[2])).toBeCloseTo(1, 12);
  });
});

describe('quatRotate / quatConjugate', () => {
  it('rotates +X to +Y about Z by 90°', () => {
    const v = quatRotate(quatFromAxisAngle([0, 0, 1], Math.PI / 2), [1, 0, 0]);
    expect(v[0]).toBeCloseTo(0, 12);
    expect(v[1]).toBeCloseTo(1, 12);
    expect(v[2]).toBeCloseTo(0, 12);
  });

  it('agrees with the matrix form of the same quaternion', () => {
    const q = quatNormalize([0.3, -0.5, 0.2, 0.7]);
    const m = mat4FromRotationTranslation(q, [0, 0, 0]);
    const viaMat = mat4MulVec4(m, [1, 2, 3, 0]);
    const v = quatRotate(q, [1, 2, 3]);
    for (let i = 0; i < 3; i++) expect(v[i]).toBeCloseTo(viaMat[i]!, 12);
  });

  it('the conjugate undoes the rotation', () => {
    const q = quatNormalize([0.3, -0.5, 0.2, 0.7]);
    const v = quatRotate(quatConjugate(q), quatRotate(q, [4, -1, 2]));
    expect(v[0]).toBeCloseTo(4, 12);
    expect(v[1]).toBeCloseTo(-1, 12);
    expect(v[2]).toBeCloseTo(2, 12);
  });
});
