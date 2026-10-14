// Where the Generation view's files live, and which spelling of a path each consumer wants.
//
// TWO SPELLINGS, AND THE DIFFERENCE MATTERS. The host's file commands (`readEditorFile`,
// `listEditorDir`, `readEditorImage`) take an ABSOLUTE path; `ludion gen` takes a PROJECT-RELATIVE
// one, because the host runs it with the game project as its working directory and every path a
// verb prints back — the run folder, an accepted destination — is project-relative too
// (`ludion docs generation-runs`). So this module has a pair for each: `…FilePath` for the host,
// `…ProjectPath` for the CLI.
//
// THE PROJECT ROOT IS THE FOLDER HOLDING `generation/`, which is the game project directory the
// shell already tracks — the same anchor `gen status`'s bare form walks up to find.
//
// Pure: no React, no Tauri, no DOM.

import { join } from '../voxel/stage';

/** The recipe document's double extension — what the asset browser lists. */
export const RECIPE_EXT = '.gen.yaml';

/** The committed recipe folder, project-relative. */
export const RECIPES_DIR = 'generation/recipes';

/** The scratch run folder, project-relative. Ignored by both version-control systems. */
export const RUNS_DIR = 'generation/runs';

/** The directory the asset browser lists. `''` with no project directory yet. */
export function recipesRoot(projectDir: string): string {
  return projectDir === '' ? '' : join(projectDir, RECIPES_DIR);
}

/** A recipe's absolute path, from its `recipesRoot`-relative path — what `readEditorFile` takes. */
export function recipeFilePath(projectDir: string, relPath: string): string {
  return join(recipesRoot(projectDir), relPath);
}

/** A recipe's project-relative path — the argument `ludion gen check|run` takes. */
export function recipeProjectPath(relPath: string): string {
  return join(RECIPES_DIR, relPath);
}

/** A recipe's run-folder name: the file stem with `.gen.yaml` stripped, nested folders dropped.
 *
 *  It is the file stem rather than the document's own `name:` key on purpose — the two agree in
 *  every committed recipe, and reading the key would make the folder the view looks in depend on a
 *  value the human can mistype mid-edit. */
export function recipeName(relPath: string): string {
  const base = relPath.slice(relPath.lastIndexOf('/') + 1);
  return base.endsWith(RECIPE_EXT) ? base.slice(0, -RECIPE_EXT.length) : base;
}

/** The absolute folder holding one recipe's runs. `''` with no project directory. */
export function runsRoot(projectDir: string, recipe: string): string {
  return projectDir === '' ? '' : join(projectDir, RUNS_DIR, recipe);
}

/** One run's project-relative folder — the argument `gen sheet|status|fetch|accept` takes. */
export function runProjectPath(recipe: string, runId: string): string {
  return join(RUNS_DIR, recipe, runId);
}

/** An absolute path for anything a verb printed project-relative (a candidate, a destination). */
export function projectFilePath(projectDir: string, projectRelative: string): string {
  return join(projectDir, projectRelative);
}
