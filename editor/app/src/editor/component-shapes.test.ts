// Gates the component shapes the editor reads by hand (gizmo camera/transform parsers, the model
// stages' Mesh/Transform reads) against the committed schema snapshot of the engine's registry
// (`crates/ludion-components/tests/snapshots/component_schemas.json`), and pins which components
// carry 64-bit integers the inspector must treat as `wide`.

import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { describe, expect, it } from 'vitest';
import { deriveFields } from '../viewer/inspect-fields';

type Schema = Record<string, unknown>;

const SNAPSHOT_URL = new URL('../../../../crates/ludion-components/tests/snapshots/component_schemas.json', import.meta.url);
const SCHEMAS = JSON.parse(readFileSync(fileURLToPath(SNAPSHOT_URL)).toString()) as Record<string, Schema>;

function isRecord(v: unknown): v is Schema {
  return typeof v === 'object' && v !== null && !Array.isArray(v);
}

function resolve(node: unknown, root: Schema): Schema | null {
  if (!isRecord(node)) return null;
  const ref = node.$ref;
  if (typeof ref === 'string') {
    const name = ref.replace('#/$defs/', '');
    const defs = root.$defs;
    return isRecord(defs) ? resolve(defs[name], root) : null;
  }
  return node;
}

/** The schemas a property `name` can have under `node`: its own, or any tagged-enum branch's. */
function propertyOf(node: unknown, name: string, root: Schema): Schema[] {
  const resolved = resolve(node, root);
  if (resolved === null) return [];
  const found: Schema[] = [];
  const own = resolved.properties;
  if (isRecord(own) && name in own) {
    const s = resolve(own[name], root);
    if (s !== null) found.push(s);
  }
  for (const key of ['oneOf', 'anyOf']) {
    const branches = resolved[key];
    if (Array.isArray(branches)) {
      for (const b of branches) found.push(...propertyOf(b, name, root));
    }
  }
  return found;
}

function resolvePath(component: string, path: readonly string[]): Schema[] {
  const root = SCHEMAS[component];
  expect(root, `component ${component} is in the snapshot`).toBeDefined();
  let level: Schema[] = [root];
  for (const segment of path) {
    level = level.flatMap((node) => propertyOf(node, segment, root));
  }
  return level;
}

function typeNames(s: Schema): string[] {
  const t = s.type;
  return Array.isArray(t) ? (t as string[]) : typeof t === 'string' ? [t] : [];
}

function vec3(s: Schema): boolean {
  const items = isRecord(s.items) ? s.items : {};
  return typeNames(s).includes('array') && s.minItems === 3 && s.maxItems === 3 && typeNames(items).includes('number');
}

// Every `{ component, path }` the hand-written editor code reads.
const NUMBER = (s: Schema): boolean => typeNames(s).includes('number');
const READS: readonly { readonly where: string; readonly component: string; readonly path: readonly string[]; readonly is: (s: Schema) => boolean }[] = [
  { where: 'gizmo/camera.ts parseTransform', component: 'Transform', path: ['translation'], is: vec3 },
  { where: 'gizmo/camera.ts parseTransform', component: 'Transform', path: ['rotation'], is: vec3 },
  { where: 'gizmo/camera.ts parseTransform', component: 'Transform', path: ['scale'], is: vec3 },
  { where: 'gizmo/camera.ts parseCamera', component: 'Camera', path: ['near'], is: NUMBER },
  { where: 'gizmo/camera.ts parseCamera', component: 'Camera', path: ['far'], is: NUMBER },
  { where: 'gizmo/camera.ts parseProjection', component: 'Camera', path: ['projection', 'type'], is: (s) => s.type === 'string' },
  { where: 'gizmo/camera.ts parseProjection', component: 'Camera', path: ['projection', 'fov_y_deg'], is: NUMBER },
  { where: 'gizmo/camera.ts parseProjection', component: 'Camera', path: ['projection', 'height'], is: NUMBER },
  { where: 'views/ModelsView.tsx, VoxelArtistView.tsx Mesh.shape.voxel', component: 'Mesh', path: ['shape', 'voxel'], is: (s) => s.type === 'string' },
  { where: 'views/ModelsView.tsx Mesh.shape.sculpt', component: 'Mesh', path: ['shape', 'sculpt'], is: (s) => s.type === 'string' },
  { where: 'views/ModelsView.tsx Mesh.shape.assembly', component: 'Mesh', path: ['shape', 'assembly'], is: (s) => s.type === 'string' },
];

describe('hand-read component shapes', () => {
  it.each(READS)('$where reads $component.$path', ({ component, path, is }) => {
    const found = resolvePath(component, path);
    expect(found.length, `${component}.${path.join('.')} resolves in the snapshot`).toBeGreaterThan(0);
    expect(found.some(is), `${component}.${path.join('.')} has the shape the editor expects`).toBe(true);
    if (path.join('.') === 'projection.type') {
      const consts = found.map((s) => s.const).filter((c) => c !== undefined);
      expect(consts).toEqual(expect.arrayContaining(['perspective', 'orthographic']));
    }
  });
});

/** Property paths (array elements folded into their array) whose integer schema is 64-bit. */
function wideIntegerPaths(node: unknown, root: Schema, path: readonly string[], depth: number): (readonly string[])[] {
  const s = resolve(node, root);
  if (s === null || depth > 6) return [];
  const out: (readonly string[])[] = [];
  const isWide = (x: unknown): boolean =>
    isRecord(x) && typeNames(x).includes('integer') && (x.format === 'uint64' || x.format === 'int64');
  if (isWide(s)) out.push(path);
  if (isRecord(s.items) && isWide(resolve(s.items, root))) out.push(path);
  if (isRecord(s.properties)) {
    for (const [k, v] of Object.entries(s.properties)) out.push(...wideIntegerPaths(v, root, [...path, k], depth + 1));
  }
  for (const key of ['oneOf', 'anyOf']) {
    const branches = s[key];
    if (Array.isArray(branches)) for (const b of branches) out.push(...wideIntegerPaths(b, root, path, depth + 1));
  }
  return out;
}

describe('wide integer inventory', () => {
  it('knows the components that carry 64-bit integers', () => {
    const withWide = Object.keys(SCHEMAS).filter((name) => wideIntegerPaths(SCHEMAS[name], SCHEMAS[name], [], 0).length > 0);
    expect(withWide).toEqual(expect.arrayContaining(['Volley', 'ParticleEmitter']));
  });

  it.each(Object.keys(SCHEMAS))('%s: every 64-bit integer is a wide field or sits under raw JSON', (name) => {
    const schema = SCHEMAS[name];
    const fields = deriveFields(schema, {});
    for (const path of wideIntegerPaths(schema, schema, [], 0)) {
      const top = fields.find((f) => (path.length === 0 ? f.path.length === 0 : f.path[0] === path[0]));
      expect(top, `${name}.${path.join('.')} has a field`).toBeDefined();
      if (top === undefined || top.kind === 'json') continue;
      const wide = (top.kind === 'number' || top.kind === 'vector') && top.wide && top.path.length === path.length;
      expect(wide, `${name}.${path.join('.')} is a wide field`).toBe(true);
    }
  });
});
