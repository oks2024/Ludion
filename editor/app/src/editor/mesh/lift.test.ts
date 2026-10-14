// Fixture: a frozen stage tree (`editor/app/fixtures/stage`); its `.mesh.yaml`
// bodies are copies of one small plate, listed here and never read. The lift exists to read what
// `ludion import` and
// `ludion mesh apply` actually wrote — a hand-built fixture would only pin this test's idea of
// that, and the imported plate is the asset the view is being built for.
//
// Uses the shared Node shims in `../../test-node-shims.d.ts` to read files under vitest's
// `environment: 'node'` (see `vite.config.ts`).

import { readFileSync, readdirSync, statSync } from 'node:fs';
import { join as joinPath } from 'node:path';
import { fileURLToPath } from 'node:url';
import { describe, expect, it } from 'vitest';
import type { DirEntry } from '../../shell/host';
import { candidatePlacements, isPlacementCandidate, liftMesh } from './lift';

const SCENES_ROOT = fileURLToPath(new URL('../../../fixtures/stage/scenes/', import.meta.url));

const PRISTINE = 'meshes/plate-imported/meshes/mesh-0.mesh.yaml';
const FILL = 'meshes/plate-imported/meshes/plate-fill/mesh.mesh.yaml';
const KEY = 'meshes/plate-imported/meshes/plate-key/mesh.mesh.yaml';
const LIT = 'meshes/plate-imported/meshes/plate-lit/mesh.mesh.yaml';
const SCENE = 'meshes/plate-imported/scene.yaml';
const PREFAB = 'meshes/plate-imported/model.prefab.yaml';
const SCENE_FILL = 'meshes/plate-imported/scene-fill.yaml';
const SCENE_KEY = 'meshes/plate-imported/scene-key.yaml';
const SCENE_LIT = 'meshes/plate-imported/scene-lit.yaml';

/** `list_editor_dir`'s contract (`src-tauri/src/files.rs`): every directory, plus the files whose
 *  name ends with `ext`, as `/`-separated paths relative to `root`. */
function listing(ext: string): DirEntry[] {
  const out: DirEntry[] = [];
  const walk = (dir: string, prefix: string): void => {
    for (const name of readdirSync(dir).sort()) {
      const full = joinPath(dir, name);
      const relPath = prefix === '' ? name : `${prefix}/${name}`;
      if (statSync(full).isDirectory()) {
        out.push({ relPath, name, isDir: true });
        walk(full, relPath);
      } else if (name.toLowerCase().endsWith(ext)) {
        out.push({ relPath, name, isDir: false });
      }
    }
  };
  walk(SCENES_ROOT, '');
  return out;
}

function read(relPath: string): string {
  return readFileSync(joinPath(SCENES_ROOT, relPath), 'utf8');
}

describe('the listing split', () => {
  const entries = listing('.yaml');

  it('finds every mesh document of the plate tree', () => {
    const meshes = entries.filter((entry) => !entry.isDir && entry.relPath.endsWith('.mesh.yaml'));
    expect(meshes.map((entry) => entry.relPath)).toEqual([PRISTINE, FILL, KEY, LIT]);
  });

  it('keeps every document kind but the prefab out of the placement candidates', () => {
    expect(isPlacementCandidate('mesh-0.mesh.yaml')).toBe(false);
    expect(isPlacementCandidate('plate-lit.mesh-edit.yaml')).toBe(false);
    expect(isPlacementCandidate('brazier.voxel.yaml')).toBe(false);
    expect(isPlacementCandidate('courtyard.map.yaml')).toBe(false);
    expect(isPlacementCandidate('textures/474b5f55a5ea3109.png')).toBe(false);
    expect(isPlacementCandidate('model.prefab.yaml')).toBe(true);
    expect(isPlacementCandidate('scene.yaml')).toBe(true);
    expect(isPlacementCandidate('scene-lit.yaml')).toBe(true);
  });
});

describe('candidate placements', () => {
  const entries = listing('.yaml');

  // Every plate mesh answers with the scenes its importer/edits left beside it, and with
  // nothing from `scenes/` itself — the stage scene up there are exactly what must not be read.
  it('walks ancestor directories nearest first, never the library root', () => {
    expect(candidatePlacements(PRISTINE, entries)).toEqual([PREFAB, SCENE_FILL, SCENE_KEY, SCENE_LIT, SCENE]);
    expect(candidatePlacements(LIT, entries)).toEqual([PREFAB, SCENE_FILL, SCENE_KEY, SCENE_LIT, SCENE]);
  });
});

describe('the lift', () => {
  it('carries the material the importer authored, not the component defaults', () => {
    const lift = liftMesh(read(SCENE), SCENE, PRISTINE);
    expect(lift).not.toBeNull();
    expect(lift?.mesh.metallic).toBe(1.0);
    expect(lift?.mesh.roughness).toBe(1.0);
    expect(lift?.scale).toEqual([1, 1, 1]);
    expect(lift?.meshNodes).toBe(1);
  });

  it('lifts the importer prefab, material included', () => {
    const lift = liftMesh(read(PREFAB), PREFAB, PRISTINE);
    expect(lift).not.toBeNull();
    expect(lift?.mesh.metallic).toBe(1);
    expect(lift?.scale).toEqual([1, 1, 1]);
  });

  it('lifts a mesh placed only by a nested prefab part', () => {
    const text = [
      'format: 1',
      'kind: prefab',
      'entities:',
      '  - name: root',
      '    components: {}',
      '  - name: arm',
      '    components:',
      '      Parent: root',
      '      Mesh: { shape: { asset: m/arm.mesh.yaml }, metallic: 0.5 }',
      '',
    ].join('\n');
    expect(liftMesh(text, 'p/x.prefab.yaml', 'p/m/arm.mesh.yaml')?.mesh.metallic).toBe(0.5);
  });

  it('carries the emissive the mesh edit placed on the derived tree', () => {
    const lift = liftMesh(read(SCENE_LIT), SCENE_LIT, LIT);
    expect(lift).not.toBeNull();
    expect(lift?.mesh.emissive).toBe(3.0);
    expect(lift?.mesh.shape).toEqual({ asset: 'meshes/plate-lit/mesh.mesh.yaml' });
  });

  // The reference is resolved against the SCENE's own directory. Matching on the bare string would
  // pass both of these and put the wrong plate on the stage.
  it('does not cross the two placements', () => {
    expect(liftMesh(read(SCENE), SCENE, LIT)).toBeNull();
    expect(liftMesh(read(SCENE_LIT), SCENE_LIT, PRISTINE)).toBeNull();
  });

  it('answers null for a scene that places nothing', () => {
    expect(liftMesh('format: 1\nentities: []\n', SCENE, PRISTINE)).toBeNull();
    expect(liftMesh('kind: mesh-edit\nops: []\n', SCENE, PRISTINE)).toBeNull();
  });
});
