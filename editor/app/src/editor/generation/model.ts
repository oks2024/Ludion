// The `ludion gen … --json` objects, read into the shapes the view renders.
//
// ONE SOURCE OF TRUTH, AND IT IS THE CLI. The host runs the verb and hands back whatever it printed
// (`Plan/comfy-generation.md` decision 15: a pipeline verb is a CLI command, and the editor is a
// client of it like any shell). So nothing here recomputes a verdict — it reads the `offline`/`live`
// words the verb chose, the diagnostics it emitted, the run record it wrote. A field the verb did
// not print is `null` here, never a default the view could mistake for the tool's answer.
//
// TOTAL AND DEFENSIVE. Every reader takes `unknown` and returns `null` rather than throwing: the
// object on the other side is a subprocess's stdout, and a verb that died mid-print must degrade to
// "no object" with its stderr on screen, not to a white page.
//
// THE RUN RECORD IS THE SAME SHAPE TWICE. `gen run`/`gen fetch` print `run.json` plus a `dir` key,
// and the run folder holds that same file — so `runModel` reads both, and the view uses it to list
// an older run it never submitted itself (`ludion docs generation-runs`).
//
// Pure: no React, no Tauri, no DOM.

import { type DiagnosticModel, num, readDiagnostics, record, str } from '../../shell/cliJson';
import type { GenResult } from '../../shell/genHost';
import type { DirEntry } from '../../shell/host';

export type { DiagnosticModel };

/** What a run produces: images to look at, or one mesh to import. */
export type OutputKind = 'image' | 'mesh';

/** `gen check --json`. */
export interface CheckModel {
  readonly recipe: string;
  readonly workflow: string | null;
  /** Read from the graph's output nodes, never declared by the recipe. `null` when the offline pass
   *  could not determine it (`L1171`). */
  readonly kind: OutputKind | null;
  readonly offline: 'ok' | 'error';
  /** `skipped` is an unreachable server (`L1167`), downgraded to a warning by the verb itself. */
  readonly live: 'ok' | 'error' | 'skipped';
  readonly diagnostics: readonly DiagnosticModel[];
}

/** One candidate in a run. A candidate's index and its seed are the same fact. */
export interface CandidateModel {
  readonly index: number;
  /** `null` when the run could not know it (a pinned or linked seed input). */
  readonly seed: number | null;
  /** The file's name inside the run folder (`0003.png`, `0000.glb`). */
  readonly file: string;
}

/** One job that failed on the server or that the server no longer knows. */
export interface FailedJobModel {
  readonly job: number;
  readonly promptId: string;
  readonly reason: 'error' | 'lost';
  readonly node: string | null;
  readonly class: string | null;
  readonly message: string;
}

/** A run record — `gen run`/`gen fetch --json`, or a `run.json` read off disk. */
export interface RunModel {
  readonly runId: string;
  /** The run folder, project-relative. `null` for a `run.json` read off disk, which does not carry
   *  its own folder (see `ludion docs generation-runs`); the caller knows where it read it from. */
  readonly dir: string | null;
  readonly recipe: string;
  readonly kind: OutputKind | null;
  readonly count: number;
  readonly seedBase: number;
  /** `submitted` while jobs are still on the server, `ok` once every output is downloaded,
   *  `partial`/`failed` when some or all jobs failed or were lost. */
  readonly status: string;
  readonly candidates: readonly CandidateModel[];
  readonly failed: readonly FailedJobModel[];
  readonly durationMs: number;
}

/** One mesh document an accept produced. */
export interface AcceptedMesh {
  readonly path: string;
  readonly vertices: number;
  readonly triangles: number;
}

/** `gen accept --json`. `sidecar` is the image path's `.origin.yaml`; a mesh carries its provenance
 *  inside each document instead, so it is `null` there. */
export interface AcceptModel {
  readonly source: string;
  readonly destination: string;
  readonly sidecar: string | null;
  readonly meshes: readonly AcceptedMesh[];
  readonly textures: readonly string[];
}

/** `gen status <run> --json`: the run record plus the server's verdict on it. */
export interface StatusModel {
  readonly dir: string | null;
  readonly run: RunModel | null;
  /** Every job has a verdict, so a fetch completes the run. */
  readonly ready: boolean;
  readonly pending: readonly string[];
  readonly failed: readonly FailedJobModel[];
}

/** What every `gen … --json` object carries, success or failure. */
export interface Envelope {
  readonly ok: boolean;
  readonly diagnostics: readonly DiagnosticModel[];
}

/** What to show under a verb: its diagnostics, or its stderr when it printed no object to read them
 *  from. */
export interface VerbMessages {
  readonly diagnostics: readonly DiagnosticModel[];
  readonly stderr: string | null;
}

// --- readers ---------------------------------------------------------------------------------- //

function kindOf(value: unknown): OutputKind | null {
  return value === 'image' || value === 'mesh' ? value : null;
}

/** The envelope of a verb's printed object, or `null` when it is not one — a crash or an argv
 *  refusal printed no object. */
export function envelope(value: unknown): Envelope | null {
  const row = record(value);
  if (row === null || typeof row.ok !== 'boolean') {
    return null;
  }
  return { ok: row.ok, diagnostics: readDiagnostics(row.diagnostics) };
}

/** What the view shows for a finished verb, or `null` when there is nothing to say. */
export function verbMessages(
  result: Pick<GenResult, 'ok' | 'json' | 'stderr' | 'exitCode'>,
): VerbMessages | null {
  const stderr = result.stderr.trim();
  const exited = `the verb exited ${String(result.exitCode)}`;
  const env = envelope(result.json);
  if (env === null) {
    return { diagnostics: [], stderr: stderr === '' ? `${exited} and printed no object` : stderr };
  }
  if (env.diagnostics.length > 0) {
    return { diagnostics: env.diagnostics, stderr: null };
  }
  if (result.ok) {
    return null;
  }
  return { diagnostics: [], stderr: stderr === '' ? `${exited} and said nothing` : stderr };
}

/** `gen check --json` → the status line's model, or `null` for anything that is not one. */
export function checkModel(value: unknown): CheckModel | null {
  const row = record(value);
  const recipe = row === null ? null : str(row.recipe);
  if (row === null || recipe === null) {
    return null;
  }
  return {
    recipe,
    workflow: str(row.workflow),
    kind: kindOf(row.kind),
    offline: row.offline === 'ok' ? 'ok' : 'error',
    live: row.live === 'ok' ? 'ok' : row.live === 'skipped' ? 'skipped' : 'error',
    diagnostics: readDiagnostics(row.diagnostics),
  };
}

/** A run record → the grid's model, or `null`. Reads both `gen run --json` and a `run.json`. */
export function runModel(value: unknown): RunModel | null {
  const row = record(value);
  const runId = row === null ? null : str(row.run_id);
  if (row === null || runId === null) {
    return null;
  }
  const candidates: CandidateModel[] = [];
  if (Array.isArray(row.outputs)) {
    for (const entry of row.outputs) {
      const output = record(entry);
      const index = output === null ? null : num(output.index);
      const file = output === null ? null : str(output.file);
      if (output === null || index === null || file === null) {
        continue;
      }
      candidates.push({ index, seed: num(output.seed), file });
    }
    // Row-major from `#0`, whatever order the downloads finished in: the label is the index, and a
    // grid whose cells are not in index order makes that label a puzzle.
    candidates.sort((a, b) => a.index - b.index);
  }
  return {
    runId,
    dir: str(row.dir),
    recipe: str(row.recipe) ?? '',
    kind: kindOf(row.kind),
    count: num(row.count) ?? candidates.length,
    seedBase: num(row.seed_base) ?? 0,
    status: str(row.status) ?? '',
    candidates,
    failed: readFailed(row.failed),
    durationMs: num(row.duration_ms) ?? 0,
  };
}

function readFailed(value: unknown): FailedJobModel[] {
  const out: FailedJobModel[] = [];
  if (!Array.isArray(value)) {
    return out;
  }
  for (const entry of value) {
    const row = record(entry);
    const job = row === null ? null : num(row.job);
    if (row === null || job === null) {
      continue;
    }
    out.push({
      job,
      promptId: str(row.prompt_id) ?? '',
      reason: row.reason === 'lost' ? 'lost' : 'error',
      node: str(row.node),
      class: str(row.class),
      message: str(row.message) ?? '',
    });
  }
  return out;
}

/** `gen status <run> --json` → the verdict, or `null` for anything that is not one. */
export function statusModel(value: unknown): StatusModel | null {
  const row = record(value);
  if (row === null || typeof row.ready !== 'boolean') {
    return null;
  }
  return {
    dir: str(row.dir),
    run: runModel(row.report),
    ready: row.ready,
    pending: Array.isArray(row.pending)
      ? row.pending.filter((id): id is string => typeof id === 'string')
      : [],
    failed: readFailed(row.failed),
  };
}

/** A candidate's cell label. The index is the identity; an unknown seed is `?`, as on the sheet. */
export function candidateLabel(candidate: CandidateModel): string {
  return `#${candidate.index} seed ${candidate.seed ?? '?'}`;
}

/** The longest `--timeout` the field accepts: a week, the CLI's `ludion_gen::run::MAX_TIMEOUT`. */
export const MAX_TIMEOUT_SECONDS = 604_800;

/** The `--timeout` argument for the timeout field: none when empty, `null` when it is not a positive
 *  whole number of seconds within `MAX_TIMEOUT_SECONDS`. */
export function timeoutArgs(text: string): string[] | null {
  const trimmed = text.trim();
  if (trimmed === '') {
    return [];
  }
  const seconds = Number(trimmed);
  return /^\d+$/.test(trimmed) && seconds > 0 && seconds <= MAX_TIMEOUT_SECONDS
    ? ['--timeout', trimmed]
    : null;
}

/** `gen accept --json` → what was written, or `null`. An image accept carries no `meshes`/
 *  `textures`, so those read as empty rather than absent. */
export function acceptModel(value: unknown): AcceptModel | null {
  const row = record(value);
  const destination = row === null ? null : str(row.destination);
  if (row === null || destination === null) {
    return null;
  }
  const meshes: AcceptedMesh[] = [];
  if (Array.isArray(row.meshes)) {
    for (const entry of row.meshes) {
      const mesh = record(entry);
      const path = mesh === null ? null : str(mesh.path);
      if (mesh === null || path === null) {
        continue;
      }
      meshes.push({ path, vertices: num(mesh.vertices) ?? 0, triangles: num(mesh.triangles) ?? 0 });
    }
  }
  const textures = Array.isArray(row.textures)
    ? row.textures.filter((entry): entry is string => typeof entry === 'string')
    : [];
  return {
    source: str(row.source) ?? '',
    destination,
    sidecar: str(row.sidecar),
    meshes,
    textures,
  };
}

// --- the run list ----------------------------------------------------------------------------- //

/** A run id as `[base, attempt]`: a collision appends `-2`, `-3`… to `<timestamp>-<hash8>`. */
function runKey(id: string): [string, number] {
  const match = /^(.*-.*)-(\d+)$/.exec(id);
  return match === null ? [id, 1] : [match[1], Number(match[2])];
}

/** The run ids under one recipe's run folder, newest first.
 *
 *  A run id is a compact UTC timestamp followed by 8 hex of the graph hash, so the base sorts
 *  newest-first lexicographically — no dates to parse and no mtimes to trust — and a collision
 *  suffix breaks a tie numerically (`-10` is newer than `-9`), as the CLI's `run_order` does. Only
 *  the top level counts: the listing is recursive, and `inputs/` inside a run is not a run. */
export function runFolders(entries: readonly DirEntry[]): string[] {
  return entries
    .filter((entry) => entry.isDir && !entry.relPath.includes('/'))
    .map((entry) => entry.relPath)
    .sort((a, b) => {
      const [baseA, attemptA] = runKey(a);
      const [baseB, attemptB] = runKey(b);
      if (baseA !== baseB) {
        return baseA < baseB ? 1 : -1;
      }
      return attemptB - attemptA;
    });
}

/** The destination the accept dialog offers, project-relative.
 *
 *  An image gets a file named for the recipe and the candidate index — the index is the one fact
 *  that tells two candidates of a run apart. A mesh gets a DIRECTORY, because that is what its
 *  accept takes (`ludion docs generation-runs`), and it must be absent or empty. */
export function defaultDestination(recipe: string, index: number, kind: OutputKind | null): string {
  return kind === 'mesh'
    ? `scenes/meshes/${recipe}/`
    : `scenes/assets/${recipe}-${index}.png`;
}
