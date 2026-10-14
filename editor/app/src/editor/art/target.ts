// Where a global-look singleton materializes: the one rule for "which entity does the first edit
// land on". The component is a parameter — the Art Director passes `PostProcess`, the Lighter
// `GiVolume` — because the rule is about the *kind* of component, not about either one.
//
// WHY THIS IS A MODULE AND NOT A LINE IN THE VIEW. Most scenes — every one of vigil's districts —
// author no `PostProcess` at all, so the Art Director tab has nothing to select and the Inspector's
// "pick an entity, edit its components" loop has no entry point. The tab shows the *effective* look
// regardless (a scene with no component renders the shipped defaults byte-for-byte) and materializes
// the component on the first field a human touches. Which entity that is has to be decided once:
// the plan's decision 10 carries the answer into the terminal's context file so an agent asked to
// "just add some fog" lands where the UI would have put it, and two implementations of a placement
// rule is two rules.
//
// THE RULE. The entity carrying `Environment`, if one exists, else the lowest `EntityId`.
// `Environment` is the global-look singleton the others pair with — vigil pairs it with
// `PostProcess` in `env-night.yaml` and `examples/gallery/gi-interior.yaml` puts `GiVolume` on the
// same entity — so co-locating them is where an author would look. The fallback matches the engine's
// own resolution — `extract` takes the first such component in the ascending-slot walk, i.e. the
// lowest live `EntityId` — so the component the tab writes is the one the renderer will read.
//
// NEVER SPAWN. `night-sky.yaml` took its `Environment` on an *existing* entity precisely so no
// `EntityId` moved and no golden or replay shifted; a tab that created an entity to hold a look would
// undo that. `pickTarget` returns `null` for an empty world rather than inventing one.
//
// Pure: the wire reader takes a plain `ResponseBody` and everything else takes plain data, so the
// whole rule is unit-tested against object literals (see target.test.ts).

import type { ResponseBody } from '@ludion/protocol';
import type { EntityId } from '../undo';

/** The registry name of the component the Art Director materializes. */
export const POST_PROCESS = 'PostProcess';

/** The registry name of the global-look singleton the rule prefers as a host. */
export const ENVIRONMENT = 'Environment';

/** The `include` list `query.entities` needs to answer the target question for `component`: it and
 *  the host singleton, so one round trip decides the host and whether the component is already
 *  authored. `filter` stays empty — the rule's fallback is over *every* entity, not only the ones
 *  carrying something. */
export function targetInclude(component: string): readonly string[] {
  return [ENVIRONMENT, component];
}

/** One `query.entities` match, narrowed to what the rule reads: the handle and which of the requested
 *  components the entity actually carries. Absent components are omitted from the map by the engine
 *  (`commands/entity.rs`), so a present key is the authoritative "carries it" test. */
export interface TargetCandidate {
  readonly entity: EntityId;
  readonly components: Readonly<Record<string, unknown>>;
}

/** Why an entity was chosen — the UI names it, because a placement a human cannot predict is one they
 *  will not trust. */
export type TargetReason = 'environment' | 'lowest-entity-id';

/** The entity a first edit materializes the component on. */
export interface MaterializationTarget {
  readonly entity: EntityId;
  readonly reason: TargetReason;
  /** Whether the entity already carries the component — false means the first edit is an insert, and
   *  its inverse is a `component.remove` rather than a restoring `component.set`. */
  readonly authored: boolean;
  /** Whether the entity carries `Environment`. Always true when `reason` is `environment`; it can
   *  also be true under the fallback, when the lowest-`EntityId` entity happens to carry one. */
  readonly hasEnvironment: boolean;
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null && !Array.isArray(value);
}

/** Reads a wire entity handle `{index, generation}` out of an unknown value, or `null`. */
function asEntityId(value: unknown): EntityId | null {
  if (!isRecord(value)) {
    return null;
  }
  if (typeof value.index !== 'number' || typeof value.generation !== 'number') {
    return null;
  }
  return { index: value.index, generation: value.generation };
}

/**
 * Reads a `query.entities` `query_result` body (requested with {@link targetInclude}) into
 * candidates.
 *
 * Tolerant of malformed entries — anything that isn't the expected shape is skipped rather than
 * thrown on, matching the other wire readers (`hierarchy.ts`, `entity-ops.ts`).
 */
export function parseTargetCandidates(body: ResponseBody): TargetCandidate[] {
  const b = body as { kind?: unknown; entities?: unknown };
  if (b.kind !== 'query_result' || !Array.isArray(b.entities)) {
    return [];
  }
  const out: TargetCandidate[] = [];
  for (const raw of b.entities) {
    if (!isRecord(raw)) {
      continue;
    }
    const entity = asEntityId(raw.entity);
    if (entity === null) {
      continue;
    }
    out.push({ entity, components: isRecord(raw.components) ? raw.components : {} });
  }
  return out;
}

/** Ascending `EntityId` order: slot first, then generation as a tiebreaker a live world cannot
 *  actually produce (one live entity per slot) but a malformed reply could. */
function lower(a: EntityId, b: EntityId): boolean {
  return a.index !== b.index ? a.index < b.index : a.generation < b.generation;
}

/**
 * Picks the entity a first edit of `component` materializes on: the `Environment` carrier if one
 * exists, else the lowest `EntityId`. `null` for an empty candidate list — never a spawn.
 *
 * With several `Environment` carriers (representable, and resolved by the engine the same way) the
 * lowest `EntityId` among them wins, so the tab writes onto the one the renderer reads.
 */
export function pickTarget(
  entities: readonly TargetCandidate[],
  component: string,
): MaterializationTarget | null {
  let lowest: TargetCandidate | null = null;
  let host: TargetCandidate | null = null;
  for (const candidate of entities) {
    if (lowest === null || lower(candidate.entity, lowest.entity)) {
      lowest = candidate;
    }
    if (ENVIRONMENT in candidate.components && (host === null || lower(candidate.entity, host.entity))) {
      host = candidate;
    }
  }
  const chosen = host ?? lowest;
  if (chosen === null) {
    return null;
  }
  return {
    entity: chosen.entity,
    reason: host === null ? 'lowest-entity-id' : 'environment',
    authored: component in chosen.components,
    hasEnvironment: ENVIRONMENT in chosen.components,
  };
}

/** The live value of `component` on the target, or `{}` when it carries none — what the tabs' own
 *  models read, so an unauthored scene still derives against the shipped defaults. */
export function targetValue(
  candidate: TargetCandidate,
  component: string,
): Readonly<Record<string, unknown>> {
  const value = candidate.components[component];
  return isRecord(value) ? value : {};
}
