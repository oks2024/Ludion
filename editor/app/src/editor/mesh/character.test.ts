import { describe, expect, it } from 'vitest';
import { clipFilePath } from '../voxel/assembly';
import { ASSEMBLY_EXT } from '../voxel/stage';
import {
  characterAssetRef,
  characterDisplayName,
  isSkinned,
  parseCharacterDoc,
} from './character';

// The committed gallery character: four clips, `run` with an overlay speed and events.
const FILES = import.meta.glob(
  '../../../../../examples/gallery/characters/mannequin.assembly.yaml',
  { query: '?raw', import: 'default', eager: true },
) as Record<string, string>;
const PATH = 'characters/mannequin.assembly.yaml';
const MANNEQUIN = parseCharacterDoc(Object.values(FILES)[0] ?? '');

describe('paths', () => {
  it('names a character the way a scene does — the listing path itself', () => {
    expect(characterAssetRef(PATH)).toBe(PATH);
    expect(characterAssetRef('characters\\mannequin.assembly.yaml')).toBe(PATH);
  });

  it('strips the double extension', () => {
    expect(characterDisplayName(PATH)).toBe('mannequin');
  });

  it('does not match a mesh or a clip document', () => {
    expect('mannequin.mesh.yaml'.endsWith(ASSEMBLY_EXT)).toBe(false);
    expect('run.anim.yaml'.endsWith(ASSEMBLY_EXT)).toBe(false);
  });
});

describe('parseCharacterDoc — the gallery mannequin', () => {
  it('reads the name and the mesh with their quotes stripped', () => {
    expect(MANNEQUIN.name).toBe('mannequin');
    expect(MANNEQUIN.mesh).toBe('../meshes/skin-mannequin.mesh.yaml');
  });

  it('reads the four clips in authored order', () => {
    expect(MANNEQUIN.clips.map((clip) => clip.name)).toEqual(['idle', 'jump', 'run', 'spin']);
    expect(MANNEQUIN.clips[0]).toEqual({ name: 'idle', file: '../clips/idle.anim.yaml' });
  });

  it('carries the overlay speed only where one is authored', () => {
    const speeds = Object.fromEntries(MANNEQUIN.clips.map((clip) => [clip.name, clip.speed]));
    expect(speeds).toEqual({ idle: undefined, jump: undefined, run: 1.2, spin: undefined });
  });

  it('resolves a clip against the character file, not the content root', () => {
    const run = MANNEQUIN.clips.find((clip) => clip.name === 'run');
    expect(clipFilePath(PATH, run?.file ?? '')).toBe('clips/run.anim.yaml');
  });
});

describe('isSkinned', () => {
  it('is true of the mannequin, whose body is a mesh', () => {
    expect(isSkinned(MANNEQUIN)).toBe(true);
  });

  it('is false of a voxel assembly and of an empty mesh', () => {
    const voxel = parseCharacterDoc(
      'kind: assembly\nname: box\nsize: [2, 1, 1]\nparts:\n  - name: body\n    file: body.part.yaml\n',
    );
    expect(isSkinned(voxel)).toBe(false);
    expect(isSkinned(parseCharacterDoc('kind: assembly\nmesh: ""\n'))).toBe(false);
  });
});

describe('parseCharacterDoc — total', () => {
  it('reads an empty document as nothing', () => {
    expect(parseCharacterDoc('')).toEqual({ name: null, mesh: null, clips: [] });
  });

  it('drops a speed that is not finite and positive', () => {
    const doc = parseCharacterDoc(
      'kind: assembly\nclips:\n  a:\n    clip: a.anim.yaml\n    speed: 0\n  b:\n    clip: b.anim.yaml\n    speed: nope\n  c:\n    speed: 2\n',
    );
    expect(doc.clips).toEqual([
      { name: 'a', file: 'a.anim.yaml' },
      { name: 'b', file: 'b.anim.yaml' },
      { name: 'c', file: '', speed: 2 },
    ]);
  });

  it('reads a scalar and a mapping entry side by side', () => {
    const doc = parseCharacterDoc(
      'kind: assembly\nclips:\n  idle: idle.anim.yaml\n  run:\n    clip: run.anim.yaml\n    speed: 1.5\n',
    );
    expect(doc.clips).toEqual([
      { name: 'idle', file: 'idle.anim.yaml' },
      { name: 'run', file: 'run.anim.yaml', speed: 1.5 },
    ]);
  });
});
