// The engine refuses an unknown params field (`deny_unknown_fields`). tsc cannot see a stray key in
// a non-literal payload; this walks the output of every pure builder that assembles one against the
// wire schema snapshot's `properties`.

import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { describe, expect, it } from 'vitest';
import type { Command } from '@ludion/protocol';
import { componentSet } from './commands';
import { spawnCommand as archetypeSpawn, ARCHETYPES } from '../editor/palette';
import { spawnCommand as prefabSpawn } from '../editor/prefab-place';
import { gizmoParentQuery } from '../editor/gizmo/scene';
import { instanceCommand } from '../editor/prefab-actions';
import { reparentCommand } from '../editor/hierarchy';
import { beginCompare, endCompare } from '../editor/compare';
import { PRESENTATION_BASE, respawnCommand } from '../editor/undo';
import { destroyEntity, duplicateEntity } from '../editor/entity-ops';
import { keyEdgeToInject, HeldKeys } from '../viewer/keymap';
import { mouseButtonInject, mouseMoveInject } from '../viewer/mousemap';
import { viewPoseOf } from '../editor/camera/useViewCamera';

type Schema = Record<string, unknown>;

const SNAPSHOT_PATH = fileURLToPath(
  new URL('../../../../crates/ludion-protocol/tests/protocol_schema.snapshot.json', import.meta.url),
);
const COMMAND = (JSON.parse(readFileSync(SNAPSHOT_PATH, 'utf8')) as { command: Schema }).command;
const DEFS = COMMAND.$defs as Record<string, Schema>;

/** The object-shaped subschemas a schema may stand for (through `$ref`, `anyOf`, `oneOf`, `allOf`). */
function candidates(schema: Schema | undefined): Schema[] {
  if (schema === undefined) return [];
  const ref = schema.$ref;
  if (typeof ref === 'string') return candidates(DEFS[ref.replace('#/$defs/', '')]);
  const out: Schema[] = schema.properties === undefined ? [] : [schema];
  for (const key of ['anyOf', 'oneOf', 'allOf']) {
    for (const member of (schema[key] as Schema[] | undefined) ?? []) out.push(...candidates(member));
  }
  return out;
}

/** JSON-pointer-ish paths of keys in `value` that no candidate schema declares. */
function strays(value: unknown, schema: Schema | undefined, path: string): string[] {
  if (Array.isArray(value)) {
    return value.flatMap((item, i) => strays(item, schema?.items as Schema | undefined, `${path}/${i}`));
  }
  if (typeof value !== 'object' || value === null) return [];
  const objects = candidates(schema);
  if (objects.length === 0) return [];
  const found: string[] = [];
  for (const [key, child] of Object.entries(value)) {
    // A tagged union: the key counts as declared if any member declares it.
    const owners = objects.filter((o) => key in (o.properties as Schema));
    if (owners.length === 0) {
      found.push(`${path}/${key}`);
      continue;
    }
    for (const owner of owners) {
      found.push(...strays(child, (owner.properties as Record<string, Schema>)[key], `${path}/${key}`));
    }
  }
  return found;
}

/** Every key a command's `params` carries that the wire schema does not declare. */
export function strayKeys(command: Command): string[] {
  const member = (COMMAND.oneOf as Schema[]).find(
    (m) => ((m.properties as Record<string, Schema>).method as Schema).const === command.method,
  );
  expect(member, `no schema member for ${command.method}`).toBeDefined();
  const params = 'params' in command ? command.params : undefined;
  return strays(params, (member?.properties as Record<string, Schema>).params, '/params');
}

const E1 = { index: 1, generation: 0 };

describe('strayKeys', () => {
  it('catches a stray, nested or not', () => {
    expect(strayKeys({ method: 'sim.step', params: { ticks: 1, hsh: true } } as unknown as Command)).toEqual(['/params/hsh']);
    expect(
      strayKeys({ method: 'render.view', params: { pose: { translation: [0, 0, 0], rotation: [0, 0, 0], fovv: 1 } } } as unknown as Command),
    ).toEqual(['/params/pose/fovv']);
  });
});

describe('presentation respawn', () => {
  it('the flagged and unflagged entity.spawn', () => {
    expect(strayKeys(respawnCommand({ index: PRESENTATION_BASE, generation: 0 }, {}))).toEqual([]);
    expect(strayKeys(respawnCommand(E1, {}))).toEqual([]);
  });
});

describe('builders that assemble a payload carry only declared params', () => {
  it('palette spawn, every archetype', () => {
    for (const archetype of ARCHETYPES) expect(strayKeys(archetypeSpawn(archetype))).toEqual([]);
  });

  it('prefab spawn and instance commands', () => {
    expect(strayKeys(prefabSpawn('prefabs/lamp.prefab.yaml', 'lamp', [1, 2, 3]))).toEqual([]);
    for (const op of ['apply', 'revert', 'unpack'] as const) {
      expect(strayKeys(instanceCommand(op, E1))).toEqual([]);
    }
    expect(strayKeys(instanceCommand('revert', E1, '/bulb'))).toEqual([]);
  });

  it('gizmo parent query', () => {
    const body = { kind: 'query_result', entities: [{ entity: E1, components: { Parent: { index: 2, generation: 0 } } }] };
    const params = gizmoParentQuery(body, E1);
    expect(params).not.toBeNull();
    expect(strayKeys({ method: 'query.entities', params } as unknown as Command)).toEqual([]);
  });

  it('reparent', () => {
    const command = reparentCommand(E1, null, { index: 2, generation: 0 });
    expect(command).not.toBeNull();
    expect(strayKeys(command as Command)).toEqual([]);
  });

  it('compare press and release', () => {
    const start = beginCompare('Light', E1, { intensity: 2 }, { intensity: 0 });
    expect(start).not.toBeNull();
    expect(strayKeys(start!.patch.command)).toEqual([]);
    expect(strayKeys(endCompare(start!.session).command)).toEqual([]);
    const absent = beginCompare('Light', E1, null, { intensity: 0 });
    expect(strayKeys(endCompare(absent!.session).command)).toEqual([]);
  });

  it('component.set', () => {
    expect(strayKeys(componentSet(E1, 'Transform', { translation: [0, 0, 0] }))).toEqual([]);
  });

  it('duplicate and destroy', async () => {
    const sent: Command[] = [];
    const call = async (command: Command) => {
      sent.push(command);
      if (command.method === 'engine.describe') {
        return { kind: 'engine_description', description: { components: { Transform: {} } } };
      }
      if (command.method === 'query.entities') {
        return { kind: 'query_result', entities: [{ entity: E1, components: { Transform: {} } }] };
      }
      return { kind: 'spawned', entity: { index: 9, generation: 0 } };
    };
    await duplicateEntity(E1, call as never);
    await destroyEntity(E1, call as never);
    expect(sent.map((c) => c.method)).toContain('entity.spawn');
    for (const command of sent) expect(strayKeys(command)).toEqual([]);
  });

  it('input.inject builders (params are an open event value, so the envelope is the whole check)', () => {
    const held = new HeldKeys();
    const params = [
      mouseMoveInject({ x: 1, y: 2 }),
      mouseButtonInject('left', 'down'),
      keyEdgeToInject({ code: 'KeyW', type: 'keydown', repeat: false }, held),
    ];
    for (const p of params) {
      expect(p).not.toBeNull();
      expect(strayKeys({ method: 'input.inject', params: p } as unknown as Command)).toEqual([]);
      expect(Object.keys(p!)).toEqual(['event']);
    }
  });

  it('render.view pose carries only ViewPose fields, with and without a fov delta', () => {
    const fly = { keys: {}, shift: false, speedMetersPerSecond: 4, fovDeltaDeg: 5 } as never;
    const pose = { translation: [1, 2, 3], rotation: [0, 0, 0] } as never;
    for (const view of [viewPoseOf(pose, fly), viewPoseOf(pose, { ...(fly as object), fovDeltaDeg: 0 } as never)]) {
      expect(strayKeys({ method: 'render.view', params: { pose: view } } as unknown as Command)).toEqual([]);
    }
  });

  it('stream.subscribe params', () => {
    const command: Command = {
      method: 'stream.subscribe',
      params: { channel: 'frames', mode: { mode: 'latest_wins', max_fps: 30 }, format: 'QOI' },
    };
    expect(strayKeys(command)).toEqual([]);
  });
});
