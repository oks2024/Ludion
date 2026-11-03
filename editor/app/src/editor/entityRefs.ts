// Which component fields hold entity references, read from the engine's own component schemas.
//
// Mirrors `entity_ref_fields` in `crates/ludion-ecs/src/entity_ref.rs`: the engine marks every
// entity-ref field with the `x-entity-ref` schema keyword, and `engine.describe` serves each
// component's schema, so the editor needs no list of component names. The engine refuses a marked
// reference inside a list, map or tuple at registration (L1392), so none reaches this walk.

import type { CallFn, EntityId } from './undo';

/** Component name → JSON Pointer suffixes of its entity-ref fields (`""` is the value itself). */
export type EntityRefIndex = ReadonlyMap<string, readonly string[]>;

const ENTITY_REF_KEYWORD = 'x-entity-ref';
const MAX_DEPTH = 12;

type Json = Record<string, unknown>;

function isObject(value: unknown): value is Json {
  return typeof value === 'object' && value !== null && !Array.isArray(value);
}

function escapeSegment(key: string): string {
  return key.replaceAll('~', '~0').replaceAll('/', '~1');
}

function unescapeSegment(raw: string): string {
  return raw.replaceAll('~1', '/').replaceAll('~0', '~');
}

/** Resolves a `#/…` JSON Pointer against `root`, decoding `~1` and `~0`. */
function resolvePointer(root: unknown, ref: string): unknown {
  let node: unknown = root;
  for (const raw of ref.replace(/^#/, '').split('/').slice(1)) {
    if (!isObject(node) && !Array.isArray(node)) return undefined;
    const key = unescapeSegment(raw);
    node = (node as Record<string, unknown>)[key];
  }
  return node;
}

function walk(root: unknown, node: unknown, suffix: string, depth: number, found: Set<string>): void {
  if (depth > MAX_DEPTH || !isObject(node)) return;
  if (ENTITY_REF_KEYWORD in node) {
    found.add(suffix);
    return;
  }
  if (typeof node.$ref === 'string') {
    walk(root, resolvePointer(root, node.$ref), suffix, depth + 1, found);
  }
  if (isObject(node.properties)) {
    for (const [key, property] of Object.entries(node.properties)) {
      walk(root, property, `${suffix}/${escapeSegment(key)}`, depth + 1, found);
    }
  }
  for (const combinator of ['anyOf', 'oneOf', 'allOf']) {
    const branches = node[combinator];
    if (!Array.isArray(branches)) continue;
    for (const branch of branches) walk(root, branch, suffix, depth + 1, found);
  }
}

/** The sorted, deduplicated suffixes of every entity-ref field in one component's JSON Schema. */
export function entityRefSuffixes(schema: unknown): string[] {
  const found = new Set<string>();
  walk(schema, schema, '', 0, found);
  return [...found].sort();
}

/** The ref-bearing components of an `engine.describe` description; an odd shape yields an empty map. */
export function entityRefIndex(description: unknown): EntityRefIndex {
  const index = new Map<string, readonly string[]>();
  const components = isObject(description) ? description.components : undefined;
  if (!isObject(components)) return index;
  for (const [name, schema] of Object.entries(components)) {
    const suffixes = entityRefSuffixes(schema);
    if (suffixes.length > 0) index.set(name, suffixes);
  }
  return index;
}

function sameId(value: unknown, id: EntityId): boolean {
  return isObject(value) && value.index === id.index && value.generation === id.generation;
}

function remapAt(value: unknown, keys: readonly string[], from: EntityId, to: EntityId): unknown {
  if (keys.length === 0) {
    return sameId(value, from) ? { index: to.index, generation: to.generation } : value;
  }
  if (!isObject(value)) return value;
  const [key, ...rest] = keys;
  const child = value[key!];
  if (child === undefined) return value;
  const next = remapAt(child, rest, from, to);
  return next === child ? value : { ...value, [key!]: next };
}

/**
 * `value` with each ref at `suffixes` that equals `from` rewritten to a fresh plain `to`. Returns the
 * same object when nothing changed, and copies only along the changed paths.
 */
export function remapRefsAt(value: unknown, suffixes: readonly string[], from: EntityId, to: EntityId): unknown {
  let out = value;
  for (const suffix of suffixes) {
    out = remapAt(out, suffix.split('/').slice(1).map(unescapeSegment), from, to);
  }
  return out;
}

/** Fetches the entity-ref index from the connected engine (`engine.describe`). */
export async function loadEntityRefs(call: CallFn): Promise<EntityRefIndex> {
  const body = await call({ method: 'engine.describe' });
  const b = body as { kind?: unknown; description?: unknown };
  if (b.kind !== 'engine_description') {
    throw new Error(`engine.describe answered ${String(b.kind)}, not engine_description`);
  }
  return entityRefIndex(b.description);
}
