import type { Command, ResponseBody } from '@ludion/protocol';
import { describe, expect, it } from 'vitest';
import {
  type CallFn,
  type EntityId,
  type UndoEntry,
  UndoHistory,
  executeRedo,
  executeUndo,
  inverseCommand,
  redoCommand,
} from './undo';

const CORE_REFS = new Map([
  ['Parent', ['']],
  ['PrefabInstance', ['/root']],
]);

// --- test helpers ------------------------------------------------------------------------------

/** A recording `CallFn`: remembers every command sent and returns whatever `respond` yields. */
function recordingCaller(respond: (cmd: Command) => ResponseBody | Promise<ResponseBody>): {
  call: CallFn;
  calls: Command[];
} {
  const calls: Command[] = [];
  const call: CallFn = async (cmd) => {
    calls.push(cmd);
    return respond(cmd);
  };
  return { call, calls };
}

const ACK = { kind: 'ack' } as ResponseBody;

/** A `spawned` reply carrying the fresh id the ECS handed back. */
function spawned(entity: EntityId): ResponseBody {
  return { kind: 'spawned', entity } as ResponseBody;
}

/** The `method` of a recorded command (the discriminant). */
function methodOf(cmd: Command): string {
  return (cmd as { method: string }).method;
}

/** The `params` of a recorded command, read structurally past the index-signature. */
function paramsOf(cmd: Command): {
  entity?: EntityId;
  component?: string;
  value?: unknown;
  components?: Record<string, unknown>;
} {
  return (cmd as { params: { entity?: EntityId; component?: string; value?: unknown; components?: Record<string, unknown> } }).params;
}

const E = (index: number, generation: number): EntityId => ({ index, generation });

// --- command derivation ------------------------------------------------------------------------

describe('inverseCommand', () => {
  it('inverts a fresh-insert set (before === null) with component.remove', () => {
    const entry: UndoEntry = { kind: 'set', entity: E(1, 0), component: 'Sprite', before: null, after: { tint: 1 } };
    const cmd = inverseCommand(entry);
    expect(methodOf(cmd)).toBe('component.remove');
    expect(paramsOf(cmd)).toEqual({ entity: E(1, 0), component: 'Sprite' });
  });

  it('inverts an overwriting set with a restoring component.set of the before value', () => {
    const entry: UndoEntry = { kind: 'set', entity: E(2, 0), component: 'Transform', before: { x: 1 }, after: { x: 9 } };
    const cmd = inverseCommand(entry);
    expect(methodOf(cmd)).toBe('component.set');
    expect(paramsOf(cmd)).toEqual({ entity: E(2, 0), component: 'Transform', value: { x: 1 } });
  });

  it('inverts a remove with a restoring component.set of the before value', () => {
    const entry: UndoEntry = { kind: 'remove', entity: E(3, 0), component: 'Light', before: { lux: 5 } };
    const cmd = inverseCommand(entry);
    expect(methodOf(cmd)).toBe('component.set');
    expect(paramsOf(cmd)).toEqual({ entity: E(3, 0), component: 'Light', value: { lux: 5 } });
  });

  it('inverts a destroy with entity.spawn of the captured components (Name left in the map)', () => {
    const components = { Name: 'crate', Transform: { x: 4 } };
    const entry: UndoEntry = { kind: 'destroy', entity: E(4, 0), components };
    const cmd = inverseCommand(entry);
    expect(methodOf(cmd)).toBe('entity.spawn');
    expect(paramsOf(cmd)).toEqual({ components });
  });

  it('inverts a spawn with entity.destroy of the entity', () => {
    const entry: UndoEntry = { kind: 'spawn', entity: E(5, 0), components: { Transform: {} } };
    const cmd = inverseCommand(entry);
    expect(methodOf(cmd)).toBe('entity.destroy');
    expect(paramsOf(cmd)).toEqual({ entity: E(5, 0) });
  });
});

describe('redoCommand', () => {
  it('re-applies a set forward with the after value', () => {
    const entry: UndoEntry = { kind: 'set', entity: E(1, 0), component: 'Transform', before: { x: 1 }, after: { x: 9 } };
    const cmd = redoCommand(entry);
    expect(methodOf(cmd)).toBe('component.set');
    expect(paramsOf(cmd)).toEqual({ entity: E(1, 0), component: 'Transform', value: { x: 9 } });
  });

  it('re-applies a fresh-insert set forward with the after value (still component.set)', () => {
    const entry: UndoEntry = { kind: 'set', entity: E(1, 0), component: 'Sprite', before: null, after: { tint: 2 } };
    const cmd = redoCommand(entry);
    expect(methodOf(cmd)).toBe('component.set');
    expect(paramsOf(cmd)).toEqual({ entity: E(1, 0), component: 'Sprite', value: { tint: 2 } });
  });

  it('re-applies a remove forward with component.remove', () => {
    const entry: UndoEntry = { kind: 'remove', entity: E(3, 0), component: 'Light', before: { lux: 5 } };
    const cmd = redoCommand(entry);
    expect(methodOf(cmd)).toBe('component.remove');
    expect(paramsOf(cmd)).toEqual({ entity: E(3, 0), component: 'Light' });
  });

  it('re-applies a destroy forward with entity.destroy', () => {
    const entry: UndoEntry = { kind: 'destroy', entity: E(4, 0), components: { Name: 'crate' } };
    const cmd = redoCommand(entry);
    expect(methodOf(cmd)).toBe('entity.destroy');
    expect(paramsOf(cmd)).toEqual({ entity: E(4, 0) });
  });

  it('re-applies a spawn forward with entity.spawn of the captured components', () => {
    const components = { Transform: { x: 0 } };
    const entry: UndoEntry = { kind: 'spawn', entity: E(5, 0), components };
    const cmd = redoCommand(entry);
    expect(methodOf(cmd)).toBe('entity.spawn');
    expect(paramsOf(cmd)).toEqual({ components });
  });
});

// --- reparent entries ----------------------------------------------------------------------------

describe('reparent entries', () => {
  const reparent: UndoEntry = { kind: 'reparent', entity: E(1, 0), before: E(7, 0), after: null };
  const reparentParams = (cmd: Command): unknown => (cmd as { params: unknown }).params;

  it('inverts to entity.reparent back to the old parent, keeping local', () => {
    const cmd = inverseCommand(reparent);
    expect(methodOf(cmd)).toBe('entity.reparent');
    expect(reparentParams(cmd)).toEqual({ entity: E(1, 0), parent: E(7, 0), keep: 'local' });
  });

  it('redoes to entity.reparent onto the new parent (null = root), keeping local', () => {
    const cmd = redoCommand(reparent);
    expect(methodOf(cmd)).toBe('entity.reparent');
    expect(reparentParams(cmd)).toEqual({ entity: E(1, 0), parent: null, keep: 'local' });
  });

  it('remap rewrites the entity and both parent refs', async () => {
    const h = new UndoHistory();
    h.push({ kind: 'reparent', entity: E(1, 0), before: E(7, 0), after: E(7, 0) });
    h.remap(E(7, 0), E(9, 1));
    const { call, calls } = recordingCaller(() => ACK);
    await executeUndo(h, call);
    expect(reparentParams(calls[0]!)).toEqual({ entity: E(1, 0), parent: E(9, 1), keep: 'local' });
  });

  it('a [reparent, set Transform] group restores the exact bits both ways, in order', async () => {
    const h = new UndoHistory();
    h.pushGroup([
      reparent,
      { kind: 'set', entity: E(1, 0), component: 'Transform', before: { t: 'old' }, after: { t: 'new' } },
    ]);
    const { call, calls } = recordingCaller(() => ACK);

    await executeUndo(h, call);
    expect(calls.map(methodOf)).toEqual(['component.set', 'entity.reparent']);
    expect(paramsOf(calls[0]!).value).toEqual({ t: 'old' });
    expect(reparentParams(calls[1]!)).toEqual({ entity: E(1, 0), parent: E(7, 0), keep: 'local' });

    calls.length = 0;
    await executeRedo(h, call);
    expect(calls.map(methodOf)).toEqual(['entity.reparent', 'component.set']);
    expect(reparentParams(calls[0]!)).toEqual({ entity: E(1, 0), parent: null, keep: 'local' });
    expect(paramsOf(calls[1]!).value).toEqual({ t: 'new' });
  });
});

// --- UndoHistory stack semantics ---------------------------------------------------------------

describe('UndoHistory', () => {
  it('starts empty', () => {
    const h = new UndoHistory();
    expect(h.canUndo).toBe(false);
    expect(h.canRedo).toBe(false);
  });

  it('push makes an entry undoable', () => {
    const h = new UndoHistory();
    h.push({ kind: 'remove', entity: E(1, 0), component: 'X', before: {} });
    expect(h.canUndo).toBe(true);
    expect(h.canRedo).toBe(false);
  });

  it('clear empties both stacks', async () => {
    const h = new UndoHistory();
    h.push({ kind: 'set', entity: E(1, 0), component: 'X', before: { a: 0 }, after: { a: 1 } });
    const { call } = recordingCaller(() => ACK);
    await executeUndo(h, call); // moves the entry to the redo stack
    expect(h.canRedo).toBe(true);
    h.clear();
    expect(h.canUndo).toBe(false);
    expect(h.canRedo).toBe(false);
  });
});

describe('executeUndo / executeRedo — nothing to do', () => {
  it('returns null when the undo stack is empty', async () => {
    const h = new UndoHistory();
    const { call, calls } = recordingCaller(() => ACK);
    expect(await executeUndo(h, call)).toBeNull();
    expect(calls).toHaveLength(0);
  });

  it('returns null when the redo stack is empty', async () => {
    const h = new UndoHistory();
    const { call, calls } = recordingCaller(() => ACK);
    expect(await executeRedo(h, call)).toBeNull();
    expect(calls).toHaveLength(0);
  });
});

describe('push clears the redo branch', () => {
  it('drops any redo entry when a new edit is recorded', async () => {
    const h = new UndoHistory();
    h.push({ kind: 'set', entity: E(1, 0), component: 'X', before: { a: 0 }, after: { a: 1 } });
    const { call } = recordingCaller(() => ACK);
    await executeUndo(h, call);
    expect(h.canRedo).toBe(true);
    h.push({ kind: 'set', entity: E(2, 0), component: 'Y', before: { b: 0 }, after: { b: 1 } });
    expect(h.canRedo).toBe(false);
    expect(h.canUndo).toBe(true);
  });
});

describe('undo → redo round-trip', () => {
  it('restores canUndo/canRedo across a full cycle and re-sends the forward command', async () => {
    const h = new UndoHistory();
    h.push({ kind: 'set', entity: E(7, 0), component: 'Transform', before: { x: 1 }, after: { x: 5 } });

    const { call, calls } = recordingCaller(() => ACK);

    const undo = await executeUndo(h, call);
    expect(undo).toEqual({ ok: true, entries: [expect.objectContaining({ kind: 'set' })], remaps: [] });
    expect(h.canUndo).toBe(false);
    expect(h.canRedo).toBe(true);
    expect(methodOf(calls[0]!)).toBe('component.set');
    expect(paramsOf(calls[0]!).value).toEqual({ x: 1 }); // undo sends the BEFORE value

    const redo = await executeRedo(h, call);
    expect(redo).toEqual({ ok: true, entries: [expect.objectContaining({ kind: 'set' })], remaps: [] });
    expect(h.canUndo).toBe(true);
    expect(h.canRedo).toBe(false);
    expect(methodOf(calls[1]!)).toBe('component.set');
    expect(paramsOf(calls[1]!).value).toEqual({ x: 5 }); // redo sends the AFTER value
  });
});

// --- structural change: destroy → undo → respawn → id remap ------------------------------------

describe('destroy → undo respawns and remaps references', () => {
  it('rewrites OTHER entries on both stacks and retargets subsequent undo at the NEW id', async () => {
    const old = E(5, 0);
    const fresh = E(9, 1); // ECS hands back a brand-new index+generation
    const components = { Name: 'crate', Transform: { x: 3 } };

    const h = new UndoHistory();
    // An earlier edit to the same entity sits UNDER the destroy on the undo stack.
    h.push({ kind: 'set', entity: old, component: 'Transform', before: { x: 1 }, after: { x: 3 } });
    h.push({ kind: 'destroy', entity: old, components });

    const { call, calls } = recordingCaller((cmd) =>
      methodOf(cmd) === 'entity.spawn' ? spawned(fresh) : ACK,
    );

    // Undo the destroy -> respawn -> fresh id, remap reported.
    const undoDestroy = await executeUndo(h, call);
    expect(methodOf(calls[0]!)).toBe('entity.spawn');
    expect(paramsOf(calls[0]!).components).toEqual(components);
    expect(undoDestroy).toEqual({
      ok: true,
      entries: [{ kind: 'destroy', entity: fresh, components }],
      remaps: [{ from: old, to: fresh }],
    });

    // The earlier `set` entry still under the undo stack was remapped onto the new id.
    const undoSet = await executeUndo(h, call);
    expect(undoSet).toEqual({
      ok: true,
      entries: [expect.objectContaining({ kind: 'set', entity: fresh })],
      remaps: [],
    });
    expect(methodOf(calls[1]!)).toBe('component.set');
    expect(paramsOf(calls[1]!).entity).toEqual(fresh); // targets the NEW id, not old
  });

  it('redo of the respawned destroy targets the new id (not the original)', async () => {
    const old = E(5, 0);
    const fresh = E(9, 1);
    const components = { Name: 'crate' };

    const h = new UndoHistory();
    h.push({ kind: 'destroy', entity: old, components });

    const { call, calls } = recordingCaller((cmd) =>
      methodOf(cmd) === 'entity.spawn' ? spawned(fresh) : ACK,
    );

    await executeUndo(h, call); // respawn -> destroy entry now carries `fresh`, sits on redo stack

    const redo = await executeRedo(h, call);
    expect(redo).toEqual({ ok: true, entries: [{ kind: 'destroy', entity: fresh, components }], remaps: [] });
    expect(methodOf(calls[1]!)).toBe('entity.destroy');
    expect(paramsOf(calls[1]!).entity).toEqual(fresh); // re-destroys the NEW id
    expect(paramsOf(calls[1]!).entity).not.toEqual(old);
  });
});

// --- redo of a spawn also yields a fresh id ----------------------------------------------------

describe('redo of a spawn respawns with a fresh id', () => {
  it('remaps and retargets just like undo-of-destroy', async () => {
    const old = E(2, 0);
    const fresh = E(11, 3);
    const components = { Transform: { x: 0 } };

    const h = new UndoHistory();
    h.push({ kind: 'spawn', entity: old, components });

    const { call, calls } = recordingCaller((cmd) =>
      methodOf(cmd) === 'entity.spawn' ? spawned(fresh) : ACK,
    );

    // Undo the spawn -> entity.destroy(old), moves onto redo stack unchanged.
    const undo = await executeUndo(h, call);
    expect(methodOf(calls[0]!)).toBe('entity.destroy');
    expect(undo).toEqual({ ok: true, entries: [{ kind: 'spawn', entity: old, components }], remaps: [] });

    // Redo the spawn -> entity.spawn -> fresh id, remap reported, entry retargeted onto undo stack.
    const redo = await executeRedo(h, call);
    expect(methodOf(calls[1]!)).toBe('entity.spawn');
    expect(redo).toEqual({
      ok: true,
      entries: [{ kind: 'spawn', entity: fresh, components }],
      remaps: [{ from: old, to: fresh }],
    });
    expect(h.canUndo).toBe(true);
    expect(h.canRedo).toBe(false);
  });
});

// --- failure handling --------------------------------------------------------------------------

describe('a rejected call drops the entry and leaves the rest of the stack intact', () => {
  it('executeUndo drops the failing entry, returns the error, keeps deeper entries', async () => {
    const boom = new Error('entity gone');
    const h = new UndoHistory();
    h.push({ kind: 'set', entity: E(1, 0), component: 'A', before: { v: 0 }, after: { v: 1 } });
    h.push({ kind: 'set', entity: E(2, 0), component: 'B', before: { v: 0 }, after: { v: 1 } });

    let failNext = true;
    const { call } = recordingCaller(() => {
      if (failNext) {
        failNext = false;
        throw boom;
      }
      return ACK;
    });

    const failed = await executeUndo(h, call);
    expect(failed).toEqual({ ok: false, entry: expect.objectContaining({ kind: 'set', entity: E(2, 0) }), error: boom });
    // The failing entry is gone from BOTH stacks (not wedged, not moved to redo).
    expect(h.canRedo).toBe(false);
    // The deeper entry survives and can still be undone.
    expect(h.canUndo).toBe(true);
    const ok = await executeUndo(h, call);
    expect(ok).toEqual({ ok: true, entries: [expect.objectContaining({ entity: E(1, 0) })], remaps: [] });
    expect(h.canRedo).toBe(true);
  });

  it('executeRedo drops the failing entry too', async () => {
    const boom = new Error('nope');
    const h = new UndoHistory();
    h.push({ kind: 'set', entity: E(1, 0), component: 'A', before: { v: 0 }, after: { v: 1 } });

    // Undo succeeds (moves entry to redo), then the redo fails.
    let calls = 0;
    const { call } = recordingCaller(() => {
      calls += 1;
      if (calls === 2) throw boom;
      return ACK;
    });

    await executeUndo(h, call);
    expect(h.canRedo).toBe(true);
    const failed = await executeRedo(h, call);
    expect(failed).toEqual({ ok: false, entry: expect.objectContaining({ kind: 'set' }), error: boom });
    // Dropped: not re-stacked onto undo, not left on redo.
    expect(h.canRedo).toBe(false);
    expect(h.canUndo).toBe(false);
  });
});

// --- remap precision ---------------------------------------------------------------------------

describe('remap matches on index AND generation', () => {
  it('does not touch an entry whose id shares the index but differs in generation', async () => {
    const h = new UndoHistory();
    // Two edits, same index (5), different generation.
    h.push({ kind: 'set', entity: E(5, 0), component: 'A', before: { v: 1 }, after: { v: 2 } }); // under
    h.push({ kind: 'set', entity: E(5, 9), component: 'B', before: { v: 3 }, after: { v: 4 } }); // top

    h.remap(E(5, 0), E(5, 1));

    const { call, calls } = recordingCaller(() => ACK);

    // Top entry (gen 9) must be untouched by the remap of gen 0.
    await executeUndo(h, call);
    expect(paramsOf(calls[0]!).entity).toEqual(E(5, 9));

    // The under entry (gen 0) was remapped to gen 1.
    await executeUndo(h, call);
    expect(paramsOf(calls[1]!).entity).toEqual(E(5, 1));
  });
});

// --- groups: a multi-entry step undoes/redoes as one, in mirrored order --------------------------

describe('a group undoes its members in reverse applied order and redoes them forward', () => {
  it('sends inverses B→A on undo and forwards A→B on redo', async () => {
    const h = new UndoHistory();
    h.pushGroup([
      { kind: 'set', entity: E(1, 0), component: 'A', before: { x: 1 }, after: { x: 2 } },
      { kind: 'set', entity: E(2, 0), component: 'B', before: { y: 3 }, after: { y: 4 } },
    ]);

    const { call, calls } = recordingCaller(() => ACK);

    const undo = await executeUndo(h, call);
    expect(calls.map((c) => paramsOf(c).component)).toEqual(['B', 'A']); // reverse of applied order
    expect(paramsOf(calls[0]!).value).toEqual({ y: 3 }); // B's before
    expect(paramsOf(calls[1]!).value).toEqual({ x: 1 }); // A's before
    expect(undo).toEqual({ ok: true, entries: expect.any(Array), remaps: [] });
    expect(h.canUndo).toBe(false);
    expect(h.canRedo).toBe(true);

    calls.length = 0;
    await executeRedo(h, call);
    expect(calls.map((c) => paramsOf(c).component)).toEqual(['A', 'B']); // applied order
    expect(paramsOf(calls[0]!).value).toEqual({ x: 2 }); // A's after
    expect(paramsOf(calls[1]!).value).toEqual({ y: 4 }); // B's after
    expect(h.canUndo).toBe(true);
    expect(h.canRedo).toBe(false);
  });

  it('pushGroup ignores an empty array', () => {
    const h = new UndoHistory();
    h.pushGroup([]);
    expect(h.canUndo).toBe(false);
  });
});

// --- deep remap: entity refs inside `Parent` component values are rewritten too ------------------

describe('remap rewrites Parent refs inside component values on both stacks', () => {
  const old = E(1, 0);
  const fresh = E(5, 0);
  const ref = (e: EntityId) => ({ index: e.index, generation: e.generation });

  it('rewrites set/remove before and destroy components for entries on the UNDO stack', async () => {
    const h = new UndoHistory();
    h.setEntityRefs(CORE_REFS);
    h.pushGroup([
      { kind: 'set', entity: E(7, 0), component: 'Parent', before: ref(old), after: ref(E(2, 0)) },
      { kind: 'remove', entity: E(8, 0), component: 'Parent', before: ref(old) },
      { kind: 'destroy', entity: E(9, 0), components: { Parent: ref(old), Name: 'c' } },
    ]);

    h.remap(old, fresh);

    const { call, calls } = recordingCaller(() => ACK);
    await executeUndo(h, call);
    // Reverse applied order: destroy inverse (spawn), then remove inverse (set), then set inverse (set).
    expect(methodOf(calls[0]!)).toBe('entity.spawn');
    expect(paramsOf(calls[0]!).components).toEqual({ Parent: ref(fresh), Name: 'c' });
    expect(methodOf(calls[1]!)).toBe('component.set');
    expect(paramsOf(calls[1]!).value).toEqual(ref(fresh)); // remove's before, rewritten
    expect(methodOf(calls[2]!)).toBe('component.set');
    expect(paramsOf(calls[2]!).value).toEqual(ref(fresh)); // set's before, rewritten (after {2,0} left alone)
  });

  it('rewrites set after and spawn components for entries on the REDO stack', async () => {
    const h = new UndoHistory();
    h.setEntityRefs(CORE_REFS);
    h.push({ kind: 'set', entity: E(7, 0), component: 'Parent', before: ref(old), after: ref(old) });
    h.push({ kind: 'spawn', entity: E(9, 0), components: { Parent: ref(old) } });

    const ack = recordingCaller(() => ACK);
    await executeUndo(h, ack.call); // spawn -> redo
    await executeUndo(h, ack.call); // set  -> redo

    h.remap(old, fresh);

    const { call, calls } = recordingCaller(() => ACK);
    await executeRedo(h, call); // set redo sends its (rewritten) after
    expect(methodOf(calls[0]!)).toBe('component.set');
    expect(paramsOf(calls[0]!).value).toEqual(ref(fresh));
    await executeRedo(h, call); // spawn redo sends its (rewritten) components
    expect(methodOf(calls[1]!)).toBe('entity.spawn');
    expect(paramsOf(calls[1]!).components).toEqual({ Parent: ref(fresh) });
  });
});

describe('remap rewrites PrefabInstance.root inside component values', () => {
  const old = E(1, 0);
  const fresh = E(5, 0);
  const ref = (e: EntityId) => ({ index: e.index, generation: e.generation });
  const link = (root: EntityId) => ({ source: 'a.prefab.yaml', part: 'body', root: ref(root) });

  it('follows root in a remove entry and a destroy components map, leaving other fields and components', async () => {
    const h = new UndoHistory();
    h.setEntityRefs(CORE_REFS);
    h.pushGroup([
      { kind: 'remove', entity: E(8, 0), component: 'PrefabInstance', before: link(old) },
      { kind: 'destroy', entity: E(9, 0), components: { PrefabInstance: link(old), Name: 'c' } },
      { kind: 'remove', entity: E(10, 0), component: 'Sprite', before: { root: ref(old) } },
    ]);

    h.remap(old, fresh);

    const { call, calls } = recordingCaller(() => ACK);
    await executeUndo(h, call);
    expect(paramsOf(calls[0]!).value).toEqual({ root: ref(old) }); // unrelated component untouched
    expect(paramsOf(calls[1]!).components).toEqual({ PrefabInstance: link(fresh), Name: 'c' });
    expect(paramsOf(calls[2]!).value).toEqual(link(fresh));
  });
});

describe('respawning an instance root repairs its own PrefabInstance.root', () => {
  const ref = (e: EntityId) => ({ index: e.index, generation: e.generation });

  it('sends a follow-up set with the new id, and stays correct across two undo/redo cycles', async () => {
    const first = E(5, 0);
    const second = E(9, 1);
    const third = E(12, 2);
    const fresh = [second, third];
    let n = 0;
    const h = new UndoHistory();
    h.setEntityRefs(CORE_REFS);
    h.push({
      kind: 'destroy',
      entity: first,
      components: { PrefabInstance: { source: 'a.prefab.yaml', part: 'body', root: ref(first) }, Name: 'a' },
    });
    const { call, calls } = recordingCaller((cmd) => (methodOf(cmd) === 'entity.spawn' ? spawned(fresh[n++]!) : ACK));

    await executeUndo(h, call); // respawn as `second`
    expect(methodOf(calls[1]!)).toBe('component.set');
    expect(paramsOf(calls[1]!).entity).toEqual(second);
    expect(paramsOf(calls[1]!).component).toBe('PrefabInstance');
    expect(paramsOf(calls[1]!).value).toEqual({ source: 'a.prefab.yaml', part: 'body', root: ref(second) });

    await executeRedo(h, call); // destroy second
    await executeUndo(h, call); // respawn as `third`
    expect(paramsOf(calls[3]!).components).toEqual({
      PrefabInstance: { source: 'a.prefab.yaml', part: 'body', root: ref(second) },
      Name: 'a',
    }); // the stored self-ref follows the id, so the spawn is not sent a dead root twice
    expect(paramsOf(calls[4]!).entity).toEqual(third);
    expect(paramsOf(calls[4]!).value).toMatchObject({ root: ref(third) });
  });

  it('adds no follow-up when the root is a live entity outside the group', async () => {
    const h = new UndoHistory();
    h.setEntityRefs(CORE_REFS);
    h.push({
      kind: 'destroy',
      entity: E(6, 0),
      components: { PrefabInstance: { source: 'a.prefab.yaml', part: 'lid', root: ref(E(5, 0)) } },
    });
    const { call, calls } = recordingCaller((cmd) => (methodOf(cmd) === 'entity.spawn' ? spawned(E(9, 1)) : ACK));
    await executeUndo(h, call);
    expect(calls).toHaveLength(1);
  });

  it('repairs parts that respawned before their instance root, over two cycles', async () => {
    // Part `a` is the instance root but a child of `b`, so the group spawns `b` first.
    const a0 = E(1, 0);
    const b0 = E(2, 0);
    const link = (root: EntityId, part: string) => ({ source: 'p.prefab.yaml', part, root: ref(root) });
    const h = new UndoHistory();
    h.setEntityRefs(CORE_REFS);
    h.pushGroup([
      { kind: 'spawn', entity: b0, components: { Name: 'b', PrefabInstance: link(a0, 'b') } },
      { kind: 'spawn', entity: a0, components: { Name: 'a', Parent: ref(b0), PrefabInstance: link(a0, 'a') } },
    ]);
    const world = new Map<string, Record<string, unknown>>();
    let next = 10;
    const key = (e: EntityId) => `${e.index}:${e.generation}`;
    const { call } = recordingCaller((cmd) => {
      const p = paramsOf(cmd);
      if (methodOf(cmd) === 'entity.spawn') {
        const e = E(next++, 0);
        world.set(key(e), { ...p.components });
        return spawned(e);
      }
      if (methodOf(cmd) === 'component.set') {
        world.get(key(p.entity!))![p.component!] = p.value;
      } else if (methodOf(cmd) === 'entity.destroy') {
        world.delete(key(p.entity!));
      }
      return ACK;
    });

    for (let cycle = 0; cycle < 2; cycle++) {
      await executeUndo(h, call); // destroy both
      expect(world.size).toBe(0);
      await executeRedo(h, call); // respawn b, then a
      const live = [...world.values()];
      const a = [...world.entries()].find(([, c]) => c.Name === 'a')![0];
      expect(live).toHaveLength(2);
      for (const c of live) {
        expect(key((c.PrefabInstance as { root: EntityId }).root)).toBe(a);
      }
    }
  });
});

// --- mid-group failure drops the whole group ----------------------------------------------------

describe('a failure mid-group aborts the rest and drops the group from history', () => {
  it('reports the offending entry, makes no further calls, and wedges nothing', async () => {
    const boom = new Error('entity gone');
    const h = new UndoHistory();
    h.pushGroup([
      { kind: 'set', entity: E(1, 0), component: 'A', before: { x: 1 }, after: { x: 2 } },
      { kind: 'set', entity: E(2, 0), component: 'B', before: { y: 3 }, after: { y: 4 } },
    ]);

    // Undo runs in reverse: the FIRST call (B's inverse) throws.
    const { call, calls } = recordingCaller(() => {
      throw boom;
    });

    const failed = await executeUndo(h, call);
    expect(failed).toEqual({
      ok: false,
      entry: expect.objectContaining({ kind: 'set', entity: E(2, 0) }),
      error: boom,
    });
    expect(calls).toHaveLength(1); // remaining member (A) never ran
    expect(h.canUndo).toBe(false); // group dropped, not re-stacked
    expect(h.canRedo).toBe(false); // and nothing partial on the redo stack
  });

  it('keeps already-executed members done when a later member fails', async () => {
    const boom = new Error('nope');
    const h = new UndoHistory();
    h.pushGroup([
      { kind: 'set', entity: E(1, 0), component: 'A', before: { x: 1 }, after: { x: 2 } },
      { kind: 'set', entity: E(2, 0), component: 'B', before: { y: 3 }, after: { y: 4 } },
    ]);

    // Reverse order: B succeeds, then A throws.
    let n = 0;
    const { call, calls } = recordingCaller(() => {
      n += 1;
      if (n === 2) throw boom;
      return ACK;
    });

    const failed = await executeUndo(h, call);
    expect(failed).toEqual({ ok: false, entry: expect.objectContaining({ entity: E(1, 0) }), error: boom });
    expect(calls).toHaveLength(2); // B ran (and stays applied), A attempted and failed
    expect(h.canUndo).toBe(false);
    expect(h.canRedo).toBe(false);
  });
});

// --- acceptance: cascade undo (remove child's Parent, destroy parent) ----------------------------

describe('acceptance: undoing a [remove-child-Parent, destroy-parent] cascade', () => {
  it('respawns the parent, remaps its fresh id into the pending remove, and reports the remap', async () => {
    const P = E(1, 0);
    const C = E(2, 0);
    const fresh = E(5, 0);
    const pComponents = { Name: 'parent', Transform: { x: 0 } };

    const h = new UndoHistory();
    h.setEntityRefs(CORE_REFS);
    // Applied order: the child's Parent was removed first, then the parent was destroyed.
    h.pushGroup([
      { kind: 'remove', entity: C, component: 'Parent', before: { index: 1, generation: 0 } },
      { kind: 'destroy', entity: P, components: pComponents },
    ]);

    const { call, calls } = recordingCaller((cmd) =>
      methodOf(cmd) === 'entity.spawn' ? spawned(fresh) : ACK,
    );

    const result = await executeUndo(h, call);

    // 1) Destroy inverse first: respawn P from its captured components.
    expect(methodOf(calls[0]!)).toBe('entity.spawn');
    expect(paramsOf(calls[0]!).components).toEqual(pComponents);
    // 2) Then the remove inverse: restore C's Parent — pointing at P's NEW id, not the dead {1,0}.
    expect(methodOf(calls[1]!)).toBe('component.set');
    expect(paramsOf(calls[1]!)).toEqual({ entity: C, component: 'Parent', value: { index: 5, generation: 0 } });
    expect(calls).toHaveLength(2);

    expect(result).toEqual({
      ok: true,
      entries: expect.any(Array),
      remaps: [{ from: P, to: fresh }],
    });
    // The step lands on the redo stack as one group.
    expect(h.canUndo).toBe(false);
    expect(h.canRedo).toBe(true);
  });
});

// --- every marked entity-ref field follows a respawn --------------------------------------------

describe('undo remaps every engine-marked entity-ref field, not just Parent and PrefabInstance', () => {
  const ref = (e: EntityId) => ({ index: e.index, generation: e.generation });
  const P = E(1, 0);
  const C = E(2, 0);
  const P2 = E(11, 1);
  const C2 = E(12, 1);
  const REFS = new Map([
    ['Parent', ['']],
    ['PatternPlayer', ['/target']],
    ['Volley', ['/source']],
    ['Leash', ['/holder']],
  ]);

  it('sends a child PatternPlayer.target the parent id the fake just handed back', async () => {
    const h = new UndoHistory();
    h.setEntityRefs(REFS);
    h.pushGroup([
      { kind: 'destroy', entity: C, components: { Parent: ref(P), PatternPlayer: { target: ref(P), pattern: 'ring' } } },
      { kind: 'destroy', entity: P, components: { Name: 'p' } },
    ]);
    const fresh = [P2, C2];
    let n = 0;
    const { call, calls } = recordingCaller((cmd) => (methodOf(cmd) === 'entity.spawn' ? spawned(fresh[n++]!) : ACK));

    await executeUndo(h, call);

    expect(calls).toHaveLength(2);
    expect(paramsOf(calls[1]!).components).toEqual({
      Parent: ref(P2),
      PatternPlayer: { target: ref(P2), pattern: 'ring' },
    });
  });

  it('repairs a parent field that names a child respawned after it', async () => {
    const h = new UndoHistory();
    h.setEntityRefs(REFS);
    h.pushGroup([
      { kind: 'destroy', entity: C, components: { Parent: ref(P) } },
      { kind: 'destroy', entity: P, components: { Volley: { source: ref(C) } } },
    ]);
    const fresh = [P2, C2];
    let n = 0;
    const { call, calls } = recordingCaller((cmd) => (methodOf(cmd) === 'entity.spawn' ? spawned(fresh[n++]!) : ACK));

    await executeUndo(h, call);

    expect(calls).toHaveLength(3);
    expect(methodOf(calls[2]!)).toBe('component.set');
    expect(paramsOf(calls[2]!)).toEqual({ entity: P2, component: 'Volley', value: { source: ref(C2) } });
  });

  it('follows a game-registered component with no name list in the editor', async () => {
    const h = new UndoHistory();
    h.setEntityRefs(REFS);
    h.pushGroup([
      { kind: 'destroy', entity: C, components: { Leash: { holder: ref(P), slack: 2 } } },
      { kind: 'destroy', entity: P, components: { Name: 'p' } },
    ]);
    const fresh = [P2, C2];
    let n = 0;
    const { call, calls } = recordingCaller((cmd) => (methodOf(cmd) === 'entity.spawn' ? spawned(fresh[n++]!) : ACK));

    await executeUndo(h, call);

    expect(paramsOf(calls[1]!).components).toEqual({ Leash: { holder: ref(P2), slack: 2 } });
  });
});
