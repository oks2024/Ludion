// The scene picker's pure logic (`Plan/editor-open-project.md` decision 7): classifying a scene
// document from its text, listing the scenes under a project, and the discard-confirmation flow
// around loading one. No Tauri, no React — `ScenePicker.tsx` supplies the host calls and dialog.

import type { DirEntry } from './host';

/** Which tags a listed scene carries. Both can be true (a generated stage scene). */
export interface SceneMarks {
  readonly stage: boolean;
  readonly generated: boolean;
}

/** One scene under `<project>/scenes`, ready to render as a picker row. */
export interface SceneEntry {
  readonly relPath: string;
  readonly marks: SceneMarks;
}

const KIND_LINE = /^kind:\s*\S/m;
const GENERATED_LINE = /^generated_from:\s*\S/m;
const STAGE_NAME = /-stage[^/]*\.yaml$/i;
const CLASSIFY_LINES = 40;

/** Classifies a document's text as a plain scene or not, from its first {@link CLASSIFY_LINES}
 *  lines. A column-0 `kind: …` (an indented one inside a component doesn't count) means the
 *  document is some other kind entirely; a column-0 `generated_from: …` marks a scene as
 *  generated. */
export function classifySceneText(text: string): { scene: false } | { scene: true; generated: boolean } {
  const head = text.split(/\r\n|\r|\n/).slice(0, CLASSIFY_LINES).join('\n');
  if (KIND_LINE.test(head)) {
    return { scene: false };
  }
  return { scene: true, generated: GENERATED_LINE.test(head) };
}

const GENERATED_VALUE = /^generated_from:[ \t]*(.*?)[ \t]*$/m;

/** The `generated_from:` value of a scene's text (its generator), or `null` when it has none.
 *  Read from the same head and column-0 rule as {@link classifySceneText}; surrounding quotes are
 *  dropped. */
export function generatorOf(text: string): string | null {
  const head = text.split(/\r\n|\r|\n/).slice(0, CLASSIFY_LINES).join('\n');
  const match = GENERATED_VALUE.exec(head);
  if (match === null || match[1] === '') {
    return null;
  }
  return match[1].replace(/^(["'])(.*)\1$/, '$2');
}

/** What is known of the loaded scene's generator: the file read is pending, the scene has none, it
 *  names one, or the read failed. */
export type GeneratorState =
  | { kind: 'unknown' }
  | { kind: 'none' }
  | { kind: 'generated'; script: string }
  | { kind: 'error'; message: string };

/** Whether Save is available for the loaded scene, and the tooltip explaining a refusal. A generated
 *  scene is never written (`editor/CLAUDE.md`): its source is the generator it names. Save stays off
 *  until the scene is known not to be generated, so a click while the read is pending cannot write. */
export function saveGate(savePath: string, generator: GeneratorState): { canSave: boolean; title: string | undefined } {
  if (savePath.trim() === '') {
    return { canSave: false, title: 'no scene path yet — connect to a served scene' };
  }
  switch (generator.kind) {
    case 'unknown':
      return { canSave: false, title: 'checking the scene…' };
    case 'error':
      return { canSave: false, title: `cannot tell whether the scene is generated — ${generator.message}` };
    case 'generated':
      return { canSave: false, title: `generated scene — edit its source (${generator.script}) and regenerate` };
    case 'none':
      return { canSave: true, title: undefined };
  }
}

/** Whether `relPath`'s basename names a stage scene (`model-stage.yaml`, `effects-stage-night.yaml`).
 *  The hyphen before `stage` is required, so `backstage.yaml` is not one. */
export function isStageScene(relPath: string): boolean {
  return STAGE_NAME.test(relPath);
}

/** Lists the scenes among `entries` (skipping directories), classified by reading each file's text
 *  in parallel via `read`. A rejected read drops that entry; the surviving order matches `entries`. */
export async function listScenes(
  entries: readonly DirEntry[],
  read: (relPath: string) => Promise<string>,
): Promise<SceneEntry[]> {
  const files = entries.filter((entry) => !entry.isDir);
  const results = await Promise.all(
    files.map(async (entry): Promise<SceneEntry | null> => {
      let text: string;
      try {
        text = await read(entry.relPath);
      } catch {
        return null;
      }
      const classified = classifySceneText(text);
      if (!classified.scene) {
        return null;
      }
      return {
        relPath: entry.relPath,
        marks: { stage: isStageScene(entry.relPath), generated: classified.generated },
      };
    }),
  );
  return results.filter((entry): entry is SceneEntry => entry !== null);
}

/** Whether `a` and `b` name the same scene path: `\`→`/` folded and a leading `./` stripped. An
 *  empty side (the fresh-attach `savePath`) never matches anything, itself included. */
export function sameScenePath(a: string, b: string): boolean {
  if (a === '' || b === '') {
    return false;
  }
  const norm = (p: string): string => p.replace(/\\/g, '/').replace(/^\.\//, '');
  return norm(a) === norm(b);
}

/** What loading a picked scene needs from its caller: the current dirty/save state, a discard
 *  confirmation, and the load call itself. */
export interface PickSceneDeps {
  readonly dirty: boolean;
  readonly savePath: string;
  readonly confirm: (message: string) => Promise<boolean>;
  readonly load: (path: string) => Promise<unknown>;
}

/** Loads `relPath`, confirming discard first when `dirty`. Returns `'cancelled'` on a declined
 *  confirmation without loading; otherwise awaits `load` and lets a rejection propagate to the
 *  caller, which turns it into a diagnostic. */
export async function pickScene(relPath: string, deps: PickSceneDeps): Promise<'loaded' | 'cancelled'> {
  if (deps.dirty) {
    const message = deps.savePath === '' ? 'Discard unsaved changes?' : `Discard unsaved changes to ${deps.savePath}?`;
    const proceed = await deps.confirm(message);
    if (!proceed) {
      return 'cancelled';
    }
  }
  await deps.load(relPath);
  return 'loaded';
}
