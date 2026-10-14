// The Game Designer view's spine: entity → the systems that drive it. Pure parsing + matching over
// `engine.describe`'s `systems` array, so the whole rule is unit-tested against object literals with
// no client (see match.test.ts). The view is a navigator over this data; nothing here talks to the
// wire.
//
// Match rule (settled 2026-07-19): a system is "related" to a selected
// entity iff at least ONE of its declared queries has ALL its component names present on the entity
// (access is ignored — read vs write doesn't change whether the system would iterate the entity).
// A system that declares no queries (`queries: []`) is *undeclared* and never matches — its behavior
// is invisible to this test by construction (the documented v1 blind spot for stored-EntityId access).

/** One component slot of a declared query: the registry component name and its read/write access. */
export interface SystemQueryComponent {
  readonly name: string;
  readonly access: string;
}

/** One declared `.query::<...>()` shape: the tuple of components it reads/writes. */
export interface SystemQuery {
  readonly components: readonly SystemQueryComponent[];
}

/** One registered system, as `engine.describe` exposes it. `order` is the execution order (systems
 *  run strictly in registration order); `registeredAt` is the `.system(...)` call site, the fallback
 *  when `fnPath` can't be resolved to a file. */
export interface SystemDesc {
  readonly name: string;
  readonly order: number;
  readonly fnPath: string;
  readonly registeredAt: { readonly file: string; readonly line: number } | null;
  readonly queries: readonly SystemQuery[];
}

function asString(value: unknown): string | null {
  return typeof value === 'string' ? value : null;
}

function asNumber(value: unknown): number | null {
  return typeof value === 'number' && Number.isFinite(value) ? value : null;
}

function parseQuery(raw: unknown): SystemQuery {
  if (typeof raw !== 'object' || raw === null) {
    return { components: [] };
  }
  const components = (raw as { components?: unknown }).components;
  if (!Array.isArray(components)) {
    return { components: [] };
  }
  const parsed: SystemQueryComponent[] = [];
  for (const entry of components) {
    if (typeof entry !== 'object' || entry === null) {
      continue;
    }
    const name = asString((entry as { name?: unknown }).name);
    if (name === null) {
      continue;
    }
    parsed.push({ name, access: asString((entry as { access?: unknown }).access) ?? 'read' });
  }
  return { components: parsed };
}

function parseSystem(raw: unknown, index: number): SystemDesc | null {
  if (typeof raw !== 'object' || raw === null) {
    return null;
  }
  const o = raw as { name?: unknown; order?: unknown; fn_path?: unknown; registered_at?: unknown; queries?: unknown };
  const name = asString(o.name);
  const fnPath = asString(o.fn_path);
  if (name === null || fnPath === null) {
    return null;
  }
  let registeredAt: SystemDesc['registeredAt'] = null;
  if (typeof o.registered_at === 'object' && o.registered_at !== null) {
    const ra = o.registered_at as { file?: unknown; line?: unknown };
    const file = asString(ra.file);
    if (file !== null) {
      registeredAt = { file, line: asNumber(ra.line) ?? 0 };
    }
  }
  const queries = Array.isArray(o.queries) ? o.queries.map(parseQuery) : [];
  return { name, order: asNumber(o.order) ?? index, fnPath, registeredAt, queries };
}

/** Pulls the `systems` array out of an `engine.describe` response body (`description.systems`),
 *  defensively — any unexpected shape yields `[]`. Ordered by the wire's execution order. */
export function parseSystems(describeBody: unknown): SystemDesc[] {
  if (typeof describeBody !== 'object' || describeBody === null) {
    return [];
  }
  const description = (describeBody as { description?: unknown }).description;
  if (typeof description !== 'object' || description === null) {
    return [];
  }
  const systems = (description as { systems?: unknown }).systems;
  if (!Array.isArray(systems)) {
    return [];
  }
  const out: SystemDesc[] = [];
  systems.forEach((raw, i) => {
    const parsed = parseSystem(raw, i);
    if (parsed !== null) {
      out.push(parsed);
    }
  });
  return out;
}

/** A system with no declared queries — its behavior is invisible to the match rule (subtle badge in
 *  the UI). Never matches. */
export function isUndeclared(system: SystemDesc): boolean {
  return system.queries.length === 0;
}

/** The match rule: `system` drives an entity carrying `componentNames` iff at least one declared
 *  query's components are ALL present on the entity (access ignored). An empty-component query and an
 *  undeclared system never match. */
export function systemMatchesEntity(system: SystemDesc, componentNames: readonly string[]): boolean {
  if (system.queries.length === 0) {
    return false;
  }
  const have = new Set(componentNames);
  return system.queries.some(
    (query) => query.components.length > 0 && query.components.every((component) => have.has(component.name)),
  );
}

/** Splits the matching systems (query-shape overlaps the selection) from the rest, each preserving
 *  execution order. With no selection, everything is "rest" (the full schedule as table of contents). */
export function partitionBySelection(
  systems: readonly SystemDesc[],
  componentNames: readonly string[] | null,
): { readonly matching: SystemDesc[]; readonly rest: SystemDesc[] } {
  if (componentNames === null) {
    return { matching: [], rest: [...systems] };
  }
  const matching: SystemDesc[] = [];
  const rest: SystemDesc[] = [];
  for (const system of systems) {
    (systemMatchesEntity(system, componentNames) ? matching : rest).push(system);
  }
  return { matching, rest };
}

/** The bare function name at the tail of a `crate::mod::fn_name` path (generics/closure suffix
 *  stripped) — the primary jump target and a compact row label. */
export function fnNameOf(fnPath: string): string {
  const base = fnPath.split('<')[0];
  const segments = base.split('::').filter((s) => s.length > 0);
  return segments.length > 0 ? segments[segments.length - 1] : fnPath;
}

/** The module segment a system's fn lives in (the segment before the fn name), or `''` for a
 *  crate-root fn — the compact "where" shown beside the system name. */
export function moduleOf(fnPath: string): string {
  const base = fnPath.split('<')[0];
  const segments = base.split('::').filter((s) => s.length > 0);
  return segments.length >= 3 ? segments[segments.length - 2] : '';
}
