// Tests for the pure selection/commit logic behind the shared `variant` widget (Phase 8 Wave 2:
// `Mesh.shape`/`MeshSource`; generalized to N object variants in Phase 10 Wave 5 when `Voxel(String)`
// joined `Asset(String)`). `VariantInput` is a thin React wrapper with no logic of its own — this
// repo has no DOM/component-rendering test setup (`vite.config.ts` runs vitest with
// `environment: 'node'` and only collects `*.test.ts`), so, matching the rest of this codebase's style,
// the widget's actual decision logic is factored into small exported pure functions and tested directly
// here rather than through DOM rendering.

import { describe, expect, it } from 'vitest';
import {
  channelName,
  currentObjectOption,
  isSliderValue,
  variantCommitValue,
  variantObjectTag,
  variantTag,
} from './FieldWidgets';
import type { VariantOption } from './inspect-fields';

const MESH_SOURCE_OPTIONS: readonly VariantOption[] = [
  { kind: 'unit', value: 'cube' },
  { kind: 'unit', value: 'plane' },
  { kind: 'unit', value: 'sphere' },
  { kind: 'asset', property: 'asset' },
];

// Phase 10 Wave 5: `Mesh.shape` gains a second data-carrying variant (`Voxel(String)`, `{ voxel: … }`)
// beside `Asset(String)` (`{ asset: … }`) — the shape `deriveVariantOptions` now supports with N object
// variants instead of one.
const MESH_SHAPE_WITH_VOXEL_OPTIONS: readonly VariantOption[] = [
  { kind: 'unit', value: 'cube' },
  { kind: 'unit', value: 'plane' },
  { kind: 'unit', value: 'sphere' },
  { kind: 'asset', property: 'asset' },
  { kind: 'asset', property: 'voxel' },
];

// Phase 12 Wave 5: `Collider.body` is the first *unit-only* variant field (`deriveVariantOptions`
// no longer requires an object branch) — the dropdown renders with no text input, and the same
// selection/commit logic drives it.
const BODY_OPTIONS: readonly VariantOption[] = [
  { kind: 'unit', value: 'static' },
  { kind: 'unit', value: 'dynamic' },
];

describe('unit-only variant field (Collider.body)', () => {
  it('selects the current unit value and never shows an object text input', () => {
    expect(variantTag(BODY_OPTIONS, 'static')).toBe('static');
    expect(variantTag(BODY_OPTIONS, 'dynamic')).toBe('dynamic');
    expect(currentObjectOption(BODY_OPTIONS, 'static')).toBeUndefined();
  });

  it('commits a dropdown choice as the bare variant string (the component.set value)', () => {
    expect(variantCommitValue(BODY_OPTIONS, 'dynamic')).toBe('dynamic');
    expect(variantCommitValue(BODY_OPTIONS, 'static')).toBe('static');
  });

  it('shows an unrecognised value as unselected, never throwing', () => {
    expect(variantTag(BODY_OPTIONS, 'kinematic')).toBe('');
    expect(variantTag(BODY_OPTIONS, { aabb: {} })).toBe('');
  });
});

describe('isSliderValue', () => {
  it('accepts the finite numbers a handle can sit at, including the ends of a track', () => {
    for (const value of [0, 1, -0.2, 0.0137, 5.5]) {
      expect(isSliderValue(value)).toBe(true);
    }
  });

  it('rejects anything a handle would have to invent a position for', () => {
    // A field the entity does not carry, or one whose value disagrees with its schema: the widget
    // falls back to the plain number box rather than drawing a handle at a made-up place.
    for (const value of [undefined, null, '0.5', Number.NaN, Number.POSITIVE_INFINITY, [0.5], {}]) {
      expect(isSliderValue(value)).toBe(false);
    }
  });
});

describe('channelName', () => {
  it('names a 3-wide vector R/G/B — the width a ranged vector actually has (lift/gamma/gain)', () => {
    expect([0, 1, 2].map((i) => channelName(i, 3))).toEqual(['R', 'G', 'B']);
  });

  it('names a 4-wide vector R/G/B/A', () => {
    expect([0, 1, 2, 3].map((i) => channelName(i, 4))).toEqual(['R', 'G', 'B', 'A']);
  });

  it('falls back to 1-based indices at any other width', () => {
    expect([0, 1].map((i) => channelName(i, 2))).toEqual(['1', '2']);
    expect([0, 4].map((i) => channelName(i, 5))).toEqual(['1', '5']);
    expect(channelName(0, 1)).toBe('1');
  });

  it('never yields an empty tag for an index outside the vector', () => {
    // A row can only ever be drawn for an in-range index, but a name is a label: it degrades to the
    // index rather than to nothing.
    expect(channelName(4, 3)).toBe('5');
    expect(channelName(-1, 3)).toBe('0');
  });
});

describe('currentObjectOption', () => {
  it('finds the object variant whose property the value carries', () => {
    expect(currentObjectOption(MESH_SOURCE_OPTIONS, { asset: 'meshes/rock.mesh.yaml' })).toEqual({
      kind: 'asset',
      property: 'asset',
    });
  });

  it('is undefined for a bare unit string', () => {
    expect(currentObjectOption(MESH_SOURCE_OPTIONS, 'cube')).toBeUndefined();
  });

  it('is undefined for an object matching no declared property, an array, null, or undefined', () => {
    expect(currentObjectOption(MESH_SOURCE_OPTIONS, { other: 1 })).toBeUndefined();
    expect(currentObjectOption(MESH_SOURCE_OPTIONS, ['asset'])).toBeUndefined();
    expect(currentObjectOption(MESH_SOURCE_OPTIONS, null)).toBeUndefined();
    expect(currentObjectOption(MESH_SOURCE_OPTIONS, undefined)).toBeUndefined();
  });

  it('is undefined when the options carry no object variant at all', () => {
    const unitOnly: readonly VariantOption[] = [{ kind: 'unit', value: 'cube' }];
    expect(currentObjectOption(unitOnly, { asset: 'x' })).toBeUndefined();
  });

  it('picks the matching object variant out of two, by property', () => {
    expect(
      currentObjectOption(MESH_SHAPE_WITH_VOXEL_OPTIONS, { voxel: 'voxels/lantern.voxel.yaml' }),
    ).toEqual({ kind: 'asset', property: 'voxel' });
    expect(
      currentObjectOption(MESH_SHAPE_WITH_VOXEL_OPTIONS, { asset: 'meshes/rock.mesh.yaml' }),
    ).toEqual({ kind: 'asset', property: 'asset' });
  });
});

describe('variantTag', () => {
  it('maps a recognised unit value to itself', () => {
    expect(variantTag(MESH_SOURCE_OPTIONS, 'plane')).toBe('plane');
  });

  it('maps an asset-shaped value to that variant tag', () => {
    expect(variantTag(MESH_SOURCE_OPTIONS, { asset: 'meshes/rock.mesh.yaml' })).toBe(
      variantObjectTag('asset'),
    );
  });

  it('maps an unrecognised value (schema/value mismatch) to the empty tag, never throwing', () => {
    expect(variantTag(MESH_SOURCE_OPTIONS, 'not-a-real-variant')).toBe('');
    expect(variantTag(MESH_SOURCE_OPTIONS, 42)).toBe('');
    expect(variantTag(MESH_SOURCE_OPTIONS, undefined)).toBe('');
  });

  it('distinguishes two object variants by their own tags', () => {
    expect(variantTag(MESH_SHAPE_WITH_VOXEL_OPTIONS, { asset: 'meshes/rock.mesh.yaml' })).toBe(
      variantObjectTag('asset'),
    );
    expect(variantTag(MESH_SHAPE_WITH_VOXEL_OPTIONS, { voxel: 'voxels/lantern.voxel.yaml' })).toBe(
      variantObjectTag('voxel'),
    );
    expect(variantObjectTag('asset')).not.toBe(variantObjectTag('voxel'));
  });
});

describe('variantCommitValue', () => {
  it('commits a unit choice as the bare string', () => {
    expect(variantCommitValue(MESH_SOURCE_OPTIONS, 'sphere')).toBe('sphere');
  });

  it('commits the asset choice as { [property]: assetValue }', () => {
    expect(
      variantCommitValue(MESH_SOURCE_OPTIONS, variantObjectTag('asset'), 'meshes/rock.mesh.yaml'),
    ).toEqual({ asset: 'meshes/rock.mesh.yaml' });
  });

  it('defaults the asset text to an empty string when switching into the asset choice fresh', () => {
    expect(variantCommitValue(MESH_SOURCE_OPTIONS, variantObjectTag('asset'))).toEqual({ asset: '' });
  });

  it('commits each of two object variants to its own property, independently', () => {
    expect(
      variantCommitValue(MESH_SHAPE_WITH_VOXEL_OPTIONS, variantObjectTag('asset'), 'meshes/rock.mesh.yaml'),
    ).toEqual({ asset: 'meshes/rock.mesh.yaml' });
    expect(
      variantCommitValue(
        MESH_SHAPE_WITH_VOXEL_OPTIONS,
        variantObjectTag('voxel'),
        'voxels/lantern.voxel.yaml',
      ),
    ).toEqual({ voxel: 'voxels/lantern.voxel.yaml' });
  });

  // Round-trip: what the dropdown reads back after a commit must reselect the same choice.
  it('round-trips through variantTag for every option', () => {
    for (const option of MESH_SHAPE_WITH_VOXEL_OPTIONS) {
      const tag = option.kind === 'unit' ? option.value : variantObjectTag(option.property);
      const committed = variantCommitValue(MESH_SHAPE_WITH_VOXEL_OPTIONS, tag, 'some/path.yaml');
      expect(variantTag(MESH_SHAPE_WITH_VOXEL_OPTIONS, committed)).toBe(tag);
    }
  });
});
