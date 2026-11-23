import { describe, expect, it } from 'vitest';
import type { Vec3 } from '../gizmo/vecmath';
import {
  clipFilePath,
  parseAssemblyDoc,
  parsePartDoc,
  partAsVoxelDoc,
  partFilePath,
  partRestBox,
  partTree,
  partWorldBox,
  pivotGlobal,
  type AssemblyPart,
  type PartNode,
} from './assembly';

// The tests run against a frozen, hand-authored rig — 11 parts, two clips, four mirrored
// limbs and a model-specific lantern — because every property worth pinning here (the humanoid
// hierarchy, the mirrored pivots, the joint heights the file's own comment block states) only
// exists on a rig somebody authored. The globs name the committed files exactly: the voxel folders
// also hold work an author has not committed, and a test that swept them would go red on someone
// else's half-finished model.
const ASSEMBLY = raw(
  import.meta.glob('../../../fixtures/stage/scenes/voxels/rig.assembly.yaml', {
    query: '?raw',
    import: 'default',
    eager: true,
  }),
);

const PARTS = raw(
  import.meta.glob('../../../fixtures/stage/scenes/voxels/parts/rig-*.part.yaml', {
    query: '?raw',
    import: 'default',
    eager: true,
  }),
);

function raw(loaded: Record<string, unknown>): Record<string, string> {
  return loaded as Record<string, string>;
}

/** One file's source, by name. */
function source(files: Record<string, string>, name: string): string {
  const key = Object.keys(files).find((path) => path.endsWith(`/${name}`));
  expect(key, `${name} is missing from the frozen stage fixture (editor/app/fixtures/stage)`).toBeDefined();
  return files[key as string];
}

const RIG = parseAssemblyDoc(source(ASSEMBLY, 'rig.assembly.yaml'));

/** The part document a named part places, parsed. */
function partDoc(name: string) {
  const part = RIG.parts.find((entry) => entry.name === name);
  expect(part, `${name} is missing from the rig`).toBeDefined();
  const file = (part as AssemblyPart).file.replace('parts/', '');
  return { part: part as AssemblyPart, doc: parsePartDoc(source(PARTS, file)) };
}

/** A part's grid extent, which every box and pivot formula needs. */
function partSize(name: string): Vec3 {
  const size = partDoc(name).doc.size;
  expect(size).not.toBeNull();
  return size as Vec3;
}

describe('parseAssemblyDoc — the stage rig', () => {
  it('reads the header past a leading comment block', () => {
    expect(RIG.name).toBe('rig');
    expect(RIG.size).toEqual([8, 16, 5]);
  });

  it('reads the one palette in doc.ts’s shape, in authored order', () => {
    expect(RIG.palette.map((entry) => entry.glyph)).toEqual(['B', 'F', 'H', 'L', 'M']);
    const lamp = RIG.palette.find((entry) => entry.glyph === 'F');
    expect(lamp?.color.map((c) => c.value)).toEqual([1.0, 0.72, 0.3, 1.0]);
    expect(lamp?.emissive?.value).toBe(4.0);
    expect(RIG.palette.find((entry) => entry.glyph === 'M')?.metallic?.value).toBe(1.0);
  });

  it('reads all 11 parts in authored order — the order /parts/<i> pointers index', () => {
    expect(RIG.parts.map((part) => part.name)).toEqual([
      'torso',
      'head',
      'upper-arm-l',
      'lower-arm-l',
      'lantern',
      'upper-arm-r',
      'lower-arm-r',
      'upper-leg-l',
      'lower-leg-l',
      'upper-leg-r',
      'lower-leg-r',
    ]);
  });

  it('reads a part entry’s file, parent, place and pivot off the dash line and its body', () => {
    const torso = RIG.parts[0];
    expect(torso.file).toBe('parts/rig-torso.part.yaml');
    expect(torso.parent).toBeNull();
    expect(torso.place).toEqual([2, 8, 3]);
    expect(torso.pivot).toEqual([2.0, 3.0, 1.0]);
  });

  it('decodes mirror: x — and only the four -r limbs carry it', () => {
    const mirrored = RIG.parts.filter((part) => part.mirrorX).map((part) => part.name);
    expect(mirrored).toEqual(['upper-arm-r', 'lower-arm-r', 'upper-leg-r', 'lower-leg-r']);
  });

  it('reads both clip bindings', () => {
    expect(RIG.clips).toEqual([
      { name: 'walk', file: 'anims/rig-walk.anim.yaml' },
      { name: 'wave', file: 'anims/rig-wave.anim.yaml' },
    ]);
  });

  it('reads a mapping-form clip entry through its clip field', () => {
    const doc = parseAssemblyDoc(
      'kind: assembly\nclips:\n  walk: a.anim.yaml\n  wave:\n    clip: b.anim.yaml\n    speed: 2\n',
    );
    expect(doc.clips).toEqual([
      { name: 'walk', file: 'a.anim.yaml' },
      { name: 'wave', file: 'b.anim.yaml', speed: 2 },
    ]);
  });

  it('is total: an empty and a half-typed document parse to nothing, never a throw', () => {
    expect(parseAssemblyDoc('')).toEqual({
      name: null,
      size: null,
      palette: [],
      parts: [],
      clips: [],
    });
    const half = parseAssemblyDoc('format: 1\nkind: assembly\nparts:\n  - name: torso\n    fi');
    expect(half.parts).toEqual([
      { name: 'torso', file: '', parent: null, place: [0, 0, 0], pivot: [0, 0, 0], mirrorX: false },
    ]);
  });
});

describe('parsePartDoc', () => {
  it('reads a palette-less grid whose layers match its declared size', () => {
    const { doc } = partDoc('lower-leg-l');
    expect(doc.name).toBe('rig-lower-leg');
    expect(doc.size).toEqual([2, 4, 2]);
    expect(doc.layers).toHaveLength(4);
    for (const layer of doc.layers) {
      expect(layer.rows).toHaveLength(2);
      for (const row of layer.rows) {
        expect(row.text).toHaveLength(2);
      }
    }
    expect(doc.layers[0].rows[0].text).toBe('LL');
    // The lamp's hook layer leaves most of its grid empty.
    expect(partDoc('lantern').doc.layers[4].rows[0].text).toBe('...');
  });

  it('carries the grid into the painter’s own type under the assembly’s palette', () => {
    const { doc } = partDoc('lantern');
    const voxel = partAsVoxelDoc(doc, RIG.palette);
    expect(voxel.layers).toBe(doc.layers);
    expect(voxel.size).toEqual(doc.size);
    expect(voxel.palette.map((entry) => entry.glyph)).toEqual(['B', 'F', 'H', 'L', 'M']);
    // A part declares no `scale:` and nothing here may be spliced.
    expect(voxel.scale).toBeNull();
    expect(voxel.source).toBe('');
  });
});

describe('partTree', () => {
  /** `name` → its depth, flattened depth-first. */
  function flatten(nodes: readonly PartNode[]): [string, number][] {
    return nodes.flatMap((node): [string, number][] => [
      [node.part.name, node.depth],
      ...flatten(node.children),
    ]);
  }

  it('is the documented humanoid hierarchy, one root, children in authored order', () => {
    const roots = partTree(RIG.parts);
    expect(roots).toHaveLength(1);
    expect(roots[0].part.name).toBe('torso');
    expect(flatten(roots)).toEqual([
      ['torso', 0],
      ['head', 1],
      ['upper-arm-l', 1],
      ['lower-arm-l', 2],
      ['lantern', 3],
      ['upper-arm-r', 1],
      ['lower-arm-r', 2],
      ['upper-leg-l', 1],
      ['lower-leg-l', 2],
      ['upper-leg-r', 1],
      ['lower-leg-r', 2],
    ]);
  });

  it('a child authored above its parent still nests', () => {
    const roots = partTree([part('hand', 'arm'), part('arm', null)]);
    expect(flatten(roots)).toEqual([
      ['arm', 0],
      ['hand', 1],
    ]);
  });

  it('an unknown parent, a self-parent and a cycle each degrade to a root — never a loop', () => {
    expect(flatten(partTree([part('hand', 'ghost')]))).toEqual([['hand', 0]]);
    expect(flatten(partTree([part('hand', 'hand')]))).toEqual([['hand', 0]]);
    const cycle = partTree([part('a', 'b'), part('b', 'c'), part('c', 'a')]);
    expect(flatten(cycle)).toEqual([
      ['a', 0],
      ['b', 0],
      ['c', 0],
    ]);
  });

  it('a duplicated name resolves to the first entry carrying it', () => {
    const roots = partTree([part('arm', null), part('arm', null), part('hand', 'arm')]);
    expect(roots.map((node) => node.index)).toEqual([0, 1]);
    expect(roots[0].children.map((node) => node.part.name)).toEqual(['hand']);
  });
});

function part(name: string, parent: string | null): AssemblyPart {
  return { name, file: '', parent, place: [0, 0, 0], pivot: [0, 0, 0], mirrorX: false };
}

describe('partFilePath / clipFilePath', () => {
  it('resolves against the assembly file’s own directory, not the content root', () => {
    expect(partFilePath('voxels/rig.assembly.yaml', 'parts/rig-torso.part.yaml')).toBe(
      'voxels/parts/rig-torso.part.yaml',
    );
    expect(clipFilePath('voxels/rig.assembly.yaml', 'anims/rig-walk.anim.yaml')).toBe(
      'voxels/anims/rig-walk.anim.yaml',
    );
  });

  it('normalises ./ and ../, and clamps a climb above the root', () => {
    expect(partFilePath('voxels/rigs/x.assembly.yaml', '../shared/arm.part.yaml')).toBe(
      'voxels/shared/arm.part.yaml',
    );
    expect(partFilePath('voxels/x.assembly.yaml', './parts/arm.part.yaml')).toBe(
      'voxels/parts/arm.part.yaml',
    );
    expect(partFilePath('voxels/x.assembly.yaml', '../../../arm.part.yaml')).toBe(
      'arm.part.yaml',
    );
  });

  it('an assembly at the content root resolves its siblings there', () => {
    expect(partFilePath('x.assembly.yaml', 'parts/arm.part.yaml')).toBe('parts/arm.part.yaml');
  });

  it('accepts a backslash-separated assembly path and answers in one shape', () => {
    expect(partFilePath('voxels\\rig.assembly.yaml', 'parts/arm.part.yaml')).toBe(
      'voxels/parts/arm.part.yaml',
    );
  });
});

describe('pivotGlobal', () => {
  /** The assembly-global pivot height of a named part. */
  function height(name: string): number {
    const { part: entry } = partDoc(name);
    return pivotGlobal(entry, partSize(name))[1];
  }

  it('reproduces the joint heights the rig’s own comment block states', () => {
    expect(height('upper-arm-l')).toBe(14.0); // shoulder
    expect(height('upper-leg-l')).toBe(8.0); // hip
    expect(height('lower-leg-l')).toBe(4.0); // knee
    expect(height('head')).toBe(14.0); // neck
  });

  it('a mirrored twin lands mirrored, not at the authored x', () => {
    const left = pivotGlobal(partDoc('upper-arm-l').part, partSize('upper-arm-l'));
    const right = pivotGlobal(partDoc('upper-arm-r').part, partSize('upper-arm-r'));
    // Both entries quote pivot x 2.0; the assembly is 8 wide, so the two shoulders sit symmetric
    // about its centre. Adding the authored x straight onto `place` would put the right one at 8.
    expect(left[0]).toBe(2);
    expect(right[0]).toBe(6);
    expect(left[0] + right[0]).toBe((RIG.size as Vec3)[0]);
    expect(left[1]).toBe(right[1]);
  });
});

describe('partRestBox / partWorldBox', () => {
  it('the rest box is place .. place + size, mirrored parts included', () => {
    expect(partRestBox(partDoc('lower-leg-l').part, partSize('lower-leg-l'))).toEqual({
      min: [2, 0, 3],
      max: [4, 4, 5],
    });
    expect(partRestBox(partDoc('lower-leg-r').part, partSize('lower-leg-r'))).toEqual({
      min: [4, 0, 3],
      max: [6, 4, 5],
    });
  });

  it('world space is bottom-centre, one voxel per unit times the entity scale', () => {
    const size = RIG.size as Vec3;
    const feet = partWorldBox(partRestBox(partDoc('lower-leg-l').part, partSize('lower-leg-l')), size, 1);
    // X and Z are centred on the 8x5 assembly; Y starts at the ground.
    expect(feet).toEqual({ min: [-2, 0, 0.5], max: [0, 4, 2.5] });
  });

  it('the whole assembly at the rig’s own 0.25 scale stands 4 units tall on the ground', () => {
    const size = RIG.size as Vec3;
    const whole = partWorldBox({ min: [0, 0, 0], max: size }, size, 0.25);
    expect(whole.min[1]).toBe(0);
    expect(whole.max[1]).toBe(4);
    expect(whole.min[0]).toBe(-whole.max[0]);
    expect(whole.min[2]).toBe(-whole.max[2]);
  });

  it('authored slice row z = 0 is the most negative Z — the model’s forward face', () => {
    const size = RIG.size as Vec3;
    const front = partWorldBox({ min: [0, 0, 0], max: [1, 1, 1] }, size, 1);
    expect(front.max[2]).toBeLessThan(0);
  });
});
