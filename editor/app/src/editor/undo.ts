// Undo/redo core for the editor.
//
// Design (Plan/phase-6-editing.md, decision #2): undo lives entirely in the client and is
// *response-powered*. There is no engine-side stack and no `undo.*` protocol method — every undo
// or redo is an ordinary protocol command derived from data the mutation's own response carried
// back (`component.set` → `{previous}`, `component.remove` → `{previous}`, `entity.destroy` →
// `{components}`). Parity holds by construction: any client (the agent included) gets the same
// affordance for free, because the inverse is just another public command.
//
// The one structural wrinkle is destroy→undo. Undoing a destroy respawns the entity, and the ECS
// hands back a *fresh* `EntityId` (a new generation, possibly a new index) — the old id is dead
// forever. So on respawn we `remap` every other stack entry that still referenced the old id onto
// the new one, and hand the caller the remap so it can fix its selection. Redoing a spawn has the
// same shape (a fresh id), so it is handled identically.
//
// This module is deliberately transport-free: the executors take an injected `CallFn`, so the
// whole state machine is unit-testable without a WebSocket (see undo.test.ts).

import type { Command, ResponseBody } from '@ludion/protocol';
import { componentSet } from '../protocol/commands';
import { stringifyWire } from '../protocol/wireJson';
import { type EntityRefIndex, remapRefsAt } from './entityRefs';

const NO_SUFFIXES: readonly string[] = [];
const NO_REFS: EntityRefIndex = new Map();

/** An entity handle as it appears on the wire. */
export type EntityId = { index: number; generation: number };

/**
 * One reversible edit, carrying exactly the data needed to invert *and* replay it.
 *
 * - `set` with `before === null` means the set was a fresh insert (the component was absent), so its
 *   inverse is a `component.remove`, not a restoring `component.set`.
 * - `destroy`/`spawn` carry the full component map so undo can reconstruct the entity.
 * - `reparent` moves the entity with `keep: 'local'` both ways; the local `Transform` bits ride a
 *   sibling `set` entry in the same group (a `keep: 'world'` round trip would not restore them exactly).
 */
export type UndoEntry =
  | { kind: 'set'; entity: EntityId; component: string; before: unknown | null; after: unknown }
  | { kind: 'remove'; entity: EntityId; component: string; before: unknown }
  | { kind: 'destroy'; entity: EntityId; components: Record<string, unknown> }
  | { kind: 'spawn'; entity: EntityId; components: Record<string, unknown> }
  | { kind: 'reparent'; entity: EntityId; before: EntityId | null; after: EntityId | null };

/** An injected command transport, so the executors below need no real socket in tests. */
export type CallFn = (cmd: Command) => Promise<ResponseBody>;

/** A single old→new id substitution produced when a respawn hands back a fresh handle. */
export type Remap = { from: EntityId; to: EntityId };

/**
 * The outcome of one undo/redo step (a *group* of one or more entries).
 *
 * On success `entries` is the retargeted group as it was moved to the opposite stack (applied order),
 * and `remaps` lists every respawn substitution the step performed, in the order they happened — the
 * caller follows its selection through them. On failure the offending `entry` and `error` are
 * reported and the whole group is dropped from history.
 */
export type StepResult =
  | null // nothing to undo/redo
  | { ok: true; entries: UndoEntry[]; remaps: Remap[] }
  | { ok: false; entry: UndoEntry; error: unknown };

/** True when two ids name the same live handle (index *and* generation must match). */
function sameEntity(a: EntityId, b: EntityId): boolean {
  return a.index === b.index && a.generation === b.generation;
}

/** A copy of `entry` retargeted at `entity` (kind and payload preserved). */
function withEntity(entry: UndoEntry, entity: EntityId): UndoEntry {
  return { ...entry, entity };
}

/** The component's entity-ref field suffixes, or none when the engine marks no field in it. */
function suffixesOf(refs: EntityRefIndex, component: string): readonly string[] {
  return refs.get(component) ?? NO_SUFFIXES;
}

/** `components` with every marked entity-ref field equal to `from` rewritten to `to`. */
function remapComponents(
  components: Record<string, unknown>,
  refs: EntityRefIndex,
  from: EntityId,
  to: EntityId,
): Record<string, unknown> {
  let out = components;
  for (const [name, suffixes] of refs) {
    const value = out[name];
    if (value === undefined) continue;
    const remapped = remapRefsAt(value, suffixes, from, to);
    if (remapped !== value) out = { ...out, [name]: remapped };
  }
  return out;
}

/**
 * `entry` with every reference to `from` rewritten to `to`: its own `entity` field, and the entity
 * refs buried inside component values — every field the engine marks as an entity ref (`refs`;
 * `before`/`after` on `set`/`remove`, the `components` map on `destroy`/`spawn`). Other component
 * values are left alone.
 */
function remapEntry(entry: UndoEntry, refs: EntityRefIndex, from: EntityId, to: EntityId): UndoEntry {
  const e = sameEntity(entry.entity, from) ? withEntity(entry, to) : entry;
  switch (e.kind) {
    case 'set': {
      const suffixes = suffixesOf(refs, e.component);
      if (suffixes.length === 0) return e;
      return {
        ...e,
        before: remapRefsAt(e.before, suffixes, from, to),
        after: remapRefsAt(e.after, suffixes, from, to),
      };
    }
    case 'remove': {
      const suffixes = suffixesOf(refs, e.component);
      if (suffixes.length === 0) return e;
      return { ...e, before: remapRefsAt(e.before, suffixes, from, to) };
    }
    case 'destroy':
    case 'spawn': {
      const components = remapComponents(e.components, refs, from, to);
      return components === e.components ? e : { ...e, components };
    }
    case 'reparent':
      return { ...e, before: remapNullableRef(e.before, from, to), after: remapNullableRef(e.after, from, to) };
    default: {
      const never: never = e;
      return never;
    }
  }
}

/** A typed, nullable entity id with `from` rewritten to a fresh plain `to`. */
function remapNullableRef(id: EntityId | null, from: EntityId, to: EntityId): EntityId | null {
  return id !== null && sameEntity(id, from) ? { index: to.index, generation: to.generation } : id;
}

/** {@link remapEntry} across a whole group. */
function remapGroup(group: UndoEntry[], refs: EntityRefIndex, from: EntityId, to: EntityId): UndoEntry[] {
  return group.map((e) => remapEntry(e, refs, from, to));
}

/** The first id of the engine's presentation range (`ludion_ecs::PRESENTATION_BASE`). */
export const PRESENTATION_BASE = 16777216;

/**
 * An `entity.spawn` that recreates `old` with `components`: presentation entities come back from the
 * presentation range, sim ones as sim. The flag is omitted when false.
 */
export function respawnCommand(old: EntityId, components: Record<string, unknown>): Command {
  return {
    method: 'entity.spawn',
    params: { components, ...(old.index >= PRESENTATION_BASE ? { presentation: true } : {}) },
  };
}

/**
 * The single protocol command that undoes `entry`.
 *
 * `set`(fresh insert) → `component.remove`; `set`(overwrite)/`remove` → restoring `component.set`;
 * `destroy` → `entity.spawn` of the captured components (a `Name` component, if present, is left in
 * the map — the wire accepts writing `Name` directly); `spawn` → `entity.destroy`.
 */
export function inverseCommand(entry: UndoEntry): Command {
  switch (entry.kind) {
    case 'set':
      if (entry.before === null) {
        return {
          method: 'component.remove',
          params: { entity: entry.entity, component: entry.component },
        };
      }
      return componentSet(entry.entity, entry.component, entry.before as object);
    case 'remove':
      return componentSet(entry.entity, entry.component, entry.before as object);
    case 'destroy':
      return respawnCommand(entry.entity, entry.components);
    case 'spawn':
      return {
        method: 'entity.destroy',
        params: { entity: entry.entity },
      };
    case 'reparent':
      return {
        method: 'entity.reparent',
        params: { entity: entry.entity, parent: entry.before, keep: 'local' },
      };
    default: {
      const never: never = entry;
      return never;
    }
  }
}

/**
 * The single protocol command that re-applies `entry` forward — the mirror of {@link inverseCommand}
 * (`set` → `component.set(after)`, `remove` → `component.remove`, `destroy` → `entity.destroy`,
 * `spawn` → `entity.spawn`).
 */
export function redoCommand(entry: UndoEntry): Command {
  switch (entry.kind) {
    case 'set':
      return componentSet(entry.entity, entry.component, entry.after as object);
    case 'remove':
      return {
        method: 'component.remove',
        params: { entity: entry.entity, component: entry.component },
      };
    case 'destroy':
      return {
        method: 'entity.destroy',
        params: { entity: entry.entity },
      };
    case 'spawn':
      return respawnCommand(entry.entity, entry.components);
    case 'reparent':
      return {
        method: 'entity.reparent',
        params: { entity: entry.entity, parent: entry.after, keep: 'local' },
      };
    default: {
      const never: never = entry;
      return never;
    }
  }
}

/** How many steps back any editor history remembers — bounds memory, not reach. */
export const UNDO_LIMIT = 64;

/**
 * Two stacks of *groups*. Each stack element is an ordered `UndoEntry[]` that undoes/redoes as ONE
 * user-visible step — a lone inspector edit is a group of one; a cascade (e.g. destroying a parent,
 * which first removes each child's `Parent` then destroys) is a group of several. Recording an edit
 * clears the redo branch (the standard linear history model).
 *
 * `remap` keeps ids current on *both* stacks after a respawn: it rewrites each entry's own `entity`
 * field AND the entity refs buried inside component values, in every field the engine marks as an
 * entity ref (see {@link remapEntry}).
 */
export class UndoHistory {
  private undoStack: UndoEntry[][] = [];
  private redoStack: UndoEntry[][] = [];
  private refs: EntityRefIndex | null = null;

  /** The ref-bearing component fields of the connected engine, or `null` until loaded. */
  get entityRefs(): EntityRefIndex | null {
    return this.refs;
  }

  /**
   * Sets the entity-ref index. It belongs to the engine, so {@link clear} keeps it; a new client
   * resets it to `null`.
   */
  setEntityRefs(refs: EntityRefIndex | null): void {
    this.refs = refs;
  }

  /** Drops the oldest groups beyond {@link UNDO_LIMIT}; the redo stack only holds popped undo groups. */
  private cap(): void {
    if (this.undoStack.length > UNDO_LIMIT) {
      this.undoStack.splice(0, this.undoStack.length - UNDO_LIMIT);
    }
  }

  /** Records a single edit as its own one-entry undoable group and drops any redo branch. */
  push(entry: UndoEntry): void {
    this.undoStack.push([entry]);
    this.cap();
    this.redoStack = [];
  }

  /** Records several entries (applied order) as ONE undoable group and drops any redo branch. Empty groups are ignored. */
  pushGroup(entries: UndoEntry[]): void {
    if (entries.length === 0) return;
    this.undoStack.push([...entries]);
    this.cap();
    this.redoStack = [];
  }

  get canUndo(): boolean {
    return this.undoStack.length > 0;
  }

  get canRedo(): boolean {
    return this.redoStack.length > 0;
  }

  /** Empties both stacks (called on `scene.load`). */
  clear(): void {
    this.undoStack = [];
    this.redoStack = [];
  }

  /**
   * Rewrites every reference to `from` into `to` across every entry on *both* stacks (the entry's own
   * `entity`, plus the marked entity-ref fields inside component values). Called after a respawn
   * hands back a fresh id so entries stacked above the destroy keep pointing at the live entity.
   */
  remap(from: EntityId, to: EntityId): void {
    const refs = this.refs ?? NO_REFS;
    this.undoStack = this.undoStack.map((g) => remapGroup(g, refs, from, to));
    this.redoStack = this.redoStack.map((g) => remapGroup(g, refs, from, to));
  }

  /** Removes and returns the top undo group, or `undefined` when empty. */
  popUndo(): UndoEntry[] | undefined {
    return this.undoStack.pop();
  }

  /** Removes and returns the top redo group, or `undefined` when empty. */
  popRedo(): UndoEntry[] | undefined {
    return this.redoStack.pop();
  }

  /** Pushes a group onto the undo stack without clearing redo (used when a redo re-enters history). */
  pushUndo(group: UndoEntry[]): void {
    this.undoStack.push(group);
    this.cap();
  }

  /** Pushes a group onto the redo stack (used when an undo moves a group across). */
  pushRedo(group: UndoEntry[]): void {
    this.redoStack.push(group);
  }
}

function componentsOf(entry: UndoEntry): Record<string, unknown> | null {
  return entry.kind === 'destroy' || entry.kind === 'spawn' ? entry.components : null;
}

/** Reads the discriminant `kind` off a response body, side-stepping the index-signature narrowing hole. */
function spawnedEntity(body: ResponseBody): EntityId | null {
  const b = body as { kind?: unknown; entity?: { index?: unknown; generation?: unknown } };
  if (b.kind !== 'spawned' || b.entity === undefined) return null;
  const { index, generation } = b.entity;
  if (typeof index !== 'number' || typeof generation !== 'number') return null;
  return { index, generation };
}

/**
 * Runs one popped `group` as a single step: sends each member's command in `execOrder` over the
 * indices, and moves the retargeted group onto the opposite stack via `pushBack`.
 *
 * Respawns are the reason this is a state machine rather than a loop of independent calls. The moment
 * a member's reply carries a fresh id, `remap` is applied *before the next member runs* to: the
 * remaining not-yet-executed members (a member's own `entity` or its `Parent` ref may point at the id
 * that just changed), the members already executed (they must re-run against the live id on the
 * opposite stack), and both history stacks. Because the working group `wg` holds the entire group and
 * is remapped whole each time, all three fall out of one `remapGroup` + one `history.remap`.
 *
 * A respawn carries the dead ids of entities that have not respawned yet (a part's instance root, a
 * parent's reference to a child), and the new id exists only once that entity has respawned. So once the
 * group has run, a follow-up `component.set` writes the remapped value of every ref-bearing component of
 * every respawned entry whose remaps changed it.
 *
 * A rejected call DROPS the whole group (already popped, never re-stacked, no partial group on the
 * opposite stack) so a stale member — e.g. its entity was destroyed by a running sim — can't wedge
 * the history. Members executed before the failure stay executed; that is accepted (never-wedge > all-or-nothing).
 */
async function runGroup(
  history: UndoHistory,
  call: CallFn,
  group: UndoEntry[],
  toCommand: (entry: UndoEntry) => Command,
  execOrder: 'reverse' | 'forward',
  pushBack: (group: UndoEntry[]) => void,
): Promise<StepResult> {
  let wg = [...group];
  const remaps: Remap[] = [];
  const refs = history.entityRefs ?? NO_REFS;
  const sentRefs = new Map<number, Map<string, string>>();
  const forward = [...wg.keys()];
  const indices = execOrder === 'reverse' ? forward.reverse() : forward;

  for (const i of indices) {
    const entry = wg[i]!;
    let body: ResponseBody;
    try {
      body = await call(toCommand(entry));
    } catch (error) {
      return { ok: false, entry, error };
    }
    const newId = spawnedEntity(body);
    if (newId !== null) {
      const from = entry.entity;
      const sent = new Map<string, string>();
      const components = componentsOf(entry);
      for (const name of refs.keys()) {
        if (components?.[name] !== undefined) sent.set(name, stringifyWire(components[name]));
      }
      sentRefs.set(i, sent);
      wg = remapGroup(wg, refs, from, newId);
      history.remap(from, newId);
      remaps.push({ from, to: newId });
    }
  }

  // A respawned entity was sent the old ids of entities not yet respawned, whose new ids exist only
  // now, so repair every ref-bearing component the remaps changed after it was sent.
  for (const [i, sent] of sentRefs) {
    const entry = wg[i]!;
    const components = componentsOf(entry);
    for (const [name, before] of [...sent].sort(([x], [y]) => (x < y ? -1 : x > y ? 1 : 0))) {
      const value = components?.[name];
      if (value === undefined || stringifyWire(value) === before) continue;
      try {
        await call(componentSet(entry.entity, name, value as object));
      } catch (error) {
        return { ok: false, entry, error };
      }
    }
  }

  pushBack(wg);
  return { ok: true, entries: wg, remaps };
}

/**
 * Undoes the top group: pop it and send each member's {@link inverseCommand} in REVERSE of applied
 * order, then move the retargeted group onto the redo stack. Fresh-id remaps and failure handling
 * are documented on {@link runGroup}.
 */
export async function executeUndo(history: UndoHistory, call: CallFn): Promise<StepResult> {
  const group = history.popUndo();
  if (group === undefined) return null;
  return runGroup(history, call, group, inverseCommand, 'reverse', (g) => history.pushRedo(g));
}

/**
 * Redoes the top undone group: pop it and send each member's {@link redoCommand} in applied order,
 * then move the retargeted group back onto the undo stack. Mirror of {@link executeUndo} — redoing a
 * spawn respawns and yields a new id just as undoing a destroy does.
 */
export async function executeRedo(history: UndoHistory, call: CallFn): Promise<StepResult> {
  const group = history.popRedo();
  if (group === undefined) return null;
  return runGroup(history, call, group, redoCommand, 'forward', (g) => history.pushUndo(g));
}
