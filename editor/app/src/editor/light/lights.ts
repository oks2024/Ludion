// The Lighter's two light queries, read into what the panel shows: which entity is the sun, and
// which entities are the point lights a human can select.
//
// WHY THE LIST EXISTS AT ALL. A bare `PointLight` entity carries no mesh, so it has no AABB in
// `render.bounds` and the viewport cannot pick it — a torch's flame is a separate entity from its
// light. Without a list, the only lights reachable in this tab would be the ones that happen to sit
// on something visible. So the list is not a convenience view of the viewport's selection; it is the
// only way to reach most of what the tab edits.
//
// WHICH TERM A LIGHT LIVES IN IS PART OF ITS IDENTITY HERE. `GiDynamic` is a field-less marker, so
// its presence in the query's `include` map is the whole reading — and it decides whether the light
// is baked into the volume (moving it re-voxelizes and re-bakes) or injected per frame (moving it
// costs nothing). The list carries it because that is the property a human is trying to see across
// every light at once, and the include costs one more key on a reply the tab already asks for.
//
// THE SUN IS THE LOWEST `EntityId`, not "the first one the query returned". A scene may author
// several `DirectionalLight`s (nothing forbids it) and the renderer takes the first in its ascending
// slot walk, so the tab must resolve the same one or it would edit a sun the frame is not lit by —
// the rule `pickTarget` encodes for the look singletons, applied to the light the shadow pass reads.
//
// Pure: the wire reader takes a plain `ResponseBody`, everything else takes plain data (see
// lights.test.ts).

import type { ResponseBody } from '@ludion/protocol';
import type { EntityId } from '../undo';
import { parseTargetCandidates } from '../art/target';

/** The registry name of the sun. */
export const DIRECTIONAL_LIGHT = 'DirectionalLight';

/** The registry name of a placed light. */
export const POINT_LIGHT = 'PointLight';

/** The registry name of a placed cone light. It feeds no GI, so it takes no `GiDynamic` marker. */
export const SPOT_LIGHT = 'SpotLight';

/** The registry name of the field-less marker that moves a light out of the bake and into the
 *  per-frame injection. Authored `GiDynamic: {}`; presence is the whole value. */
export const GI_DYNAMIC = 'GiDynamic';

/** The reserved name component every entity may carry. */
const NAME = 'Name';

/** The `include` list the lights queries need: the `Name` a row is labelled by, the marker that
 *  decides which GI term the light lives in, and `SpotLight` to tell a cone from a point. The
 *  `filter` is `[PointLight]` or `[SpotLight]` — an unmarked light must still match, so `GiDynamic`
 *  is asked for and never required. */
export function lightInclude(): readonly string[] {
  return [NAME, GI_DYNAMIC, SPOT_LIGHT];
}

/** One light the query matched: its handle, the `Name` it carries if any, whether it is marked
 *  `GiDynamic`, and whether it is a cone. */
export interface LightEntry {
  readonly entity: EntityId;
  /** `null` when the entity carries no `Name` — most spawned lights do not. */
  readonly name: string | null;
  /** Whether the entity carries `GiDynamic`. The engine omits absent components from the reply's
   *  map, so a present key is the authoritative "is marked" test. */
  readonly dynamic: boolean;
  /** Whether the entity carries a `SpotLight`, read by presence like `dynamic`. */
  readonly spot: boolean;
}

/** Ascending `EntityId`: slot first, generation as the tiebreaker. */
function lower(a: EntityId, b: EntityId): boolean {
  return a.index !== b.index ? a.index < b.index : a.generation < b.generation;
}

/**
 * Reads filtered `query.entities` replies (requested with `include: ['Name']`, or with the light
 * component beside it) into one list of entries, in ascending `EntityId` order. Several replies —
 * the point-light and spot-light queries — merge into one list.
 *
 * The order is imposed here rather than trusted from the reply: it is what makes {@link pickSun}'s
 * answer the renderer's, and what keeps the list from reshuffling under the pointer if the engine's
 * walk ever changes. Malformed entries are skipped, matching the other wire readers.
 */
export function parseLightEntries(...bodies: readonly ResponseBody[]): readonly LightEntry[] {
  const entries = bodies.flatMap((body) =>
    parseTargetCandidates(body).map((candidate) => {
      const name = candidate.components[NAME];
      return {
        entity: candidate.entity,
        name: typeof name === 'string' && name !== '' ? name : null,
        dynamic: GI_DYNAMIC in candidate.components,
        spot: SPOT_LIGHT in candidate.components,
      };
    }),
  );
  return entries.sort((a, b) => (lower(a.entity, b.entity) ? -1 : lower(b.entity, a.entity) ? 1 : 0));
}

/** The sun: the lowest `EntityId` among the entries, or `null` when the scene has no directional
 *  light. Never a spawn — a scene without a sun is a scene to be told about, not one to change. */
export function pickSun(entries: readonly LightEntry[]): EntityId | null {
  return entries.length === 0 ? null : entries[0].entity;
}

/** What a list row calls a light. An unnamed one is not a defect — the handle beside it identifies
 *  it — so this says so plainly rather than inventing a name from the component's values. */
export function lightLabel(entry: LightEntry): string {
  return entry.name ?? 'unnamed';
}

/**
 * Whether a light card carries the `GiDynamic` toggle.
 *
 * Only a `PointLight` can be injected: the sun is a `DirectionalLight`, which the bake folds into
 * the volume unconditionally, and a `SpotLight` feeds no GI at all, so marking either would change
 * nothing the renderer reads. The rule is a
 * predicate rather than an `if` in the panel because the card must state {@link SUN_ALWAYS_BAKES}
 * where the control would have been — a control that is simply missing is indistinguishable from a
 * control that is broken.
 */
export function offersGiDynamicToggle(component: string): boolean {
  return component === POINT_LIGHT;
}

/** Why a spot light's card has no toggle. Shown, not implied. */
export const SPOT_FEEDS_NO_GI =
  'A SpotLight feeds no GI and casts no shadows, so there is no GiDynamic marker to set here.';

/** Why the sun's card has no toggle. Shown, not implied. */
export const SUN_ALWAYS_BAKES =
  'The sun is always baked into the volume — only a PointLight can be injected per frame, so there is no GiDynamic marker to set here.';
