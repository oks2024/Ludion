// The Effects view's session plumbing, as pure data: the second `ludion serve` it hosts, the
// `subject` entity its emitter tab inspects, and the constant it shares with `voxel/stage.ts`'s
// `stageSlot` state-card derivation rather than re-deriving it.
//
// The view owns its OWN engine (`shell/host.ts`'s stage session, a second slot beside the Voxel
// Artist's), so this touches neither the game session's url/client/scene nor the voxel stage's.
// The content root is the stage scene's own directory, same as the voxel stage: an emitter's asset
// references (should it ever carry one) resolve against `<projectDir>/scenes`.
//
// Pure: no React, no Tauri, no DOM.

import { join, stageSlot, type StageSlot, type StageSlotInput } from '../voxel/stage';

/** The host's stage slot name for this view's session, distinct from the Voxel Artist's. */
export const STAGE_SLOT = 'effects' as const;

/** The stage scene's basename, under `<projectDir>/scenes/`. Unlike the voxel stage there is one
 *  rig, not two — nothing in the effects stage depends on lighting the way a model's shading does. */
export const STAGE_SCENE = 'effects-stage.yaml';

/** The host's default effects-stage address, needed at persona spawn before the stage has
 *  announced its own url over the status channel. */
export const FALLBACK_STAGE_URL = 'ws://127.0.0.1:9413';

/** The stage scene's one entity, the emitter tab reads and writes. */
export const SUBJECT = 'subject';

/** The stage scene's aim marker: the entity an `arc { center: aim }` volley points at, moved from
 *  the pattern tab's target fields. */
export const TARGET = 'target';

/** The registry name of the component the emitter tab inspects on `SUBJECT`. */
export const EMITTER = 'ParticleEmitter';

/** The registry name of the component the pattern tab points at the open document — also on
 *  `SUBJECT`, so one stage entity plays both tabs' material. */
export const PATTERN_PLAYER = 'PatternPlayer';

/** The registry name of the component the target fields move. */
export const TRANSFORM = 'Transform';

/** The `kind:` of the document the pattern tab edits — the key `engine.describe` reports its schema
 *  under, and the `kind:` line a file must carry to appear in the picker. */
export const PATTERN_KIND = 'pattern';

/** How often the inspector polls the subject's component value. */
export const EMITTER_POLL_MS = 300;

/** The stage engine's content root: the directory `ludion serve` was pointed at, which is what every
 *  document key (`PatternPlayer.pattern`, `asset.reload { path }`) is relative to. */
export function contentRoot(projectDir: string): string {
  return join(projectDir, 'scenes');
}

/** The absolute path of the effects stage scene — the voxel stage's own `join`, so the two stages
 *  resolve a project dir identically. */
export function stageScenePath(projectDir: string): string {
  return join(projectDir, 'scenes', STAGE_SCENE);
}

/** The state card for the effects viewport slot — `voxel/stage.ts`'s own `stageSlot` with this
 *  view's name substituted in, so the two views never carry two copies of the same phase→copy
 *  logic. */
export function effectsStageSlot(input: StageSlotInput): StageSlot | null {
  return stageSlot({ ...input, stageName: 'effects stage' });
}
