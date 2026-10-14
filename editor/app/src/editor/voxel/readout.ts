// The model readout: `ludion check-scene --json` output → the structured line the MODEL panel and
// the status bar render.
//
// ONE CHECKER, THE ENGINE'S (`Plan/editor-voxel-artist.md`, contract 2). Size, filled count,
// component count, and every diagnostic come from the CLI, read here. Nothing in this module —
// and nothing anywhere else in the editor — recomputes them from the document; `doc.ts` parses the
// same file only to build the painter's grid, and never to judge it.
//
// THE CONTRACT is the one JSON object `check-scene --json` prints on stdout for a `kind: voxel` or
// `kind: assembly` document (`Reporter` in `check_scene.rs`):
//
//   {path, kind, ok, …the kind's fields, diagnostics}     exit 0, or exit 1 with `ok: false`
//
// A voxel carries `name`, `size`, `filled`, `components`, `meshed`, `scale`; an assembly `name`,
// `size`, `components`, `parts[]`, `clips[]`. `diagnostics` is every diagnostic in emission order,
// errors and warnings mixed; an error omits `severity`. The committed samples in `__fixtures__/`
// are the CLI's own output, pinned from both sides (`tests/check_scene_json.rs`, `readout.test.ts`).
//
// Severity comes from each diagnostic, NEVER from the exit code: `L1129` (stray component) and
// `L1115` (cell budget) are warnings on a *successful* run, and they are exactly what the readout
// exists to surface. A usage error or a dead process prints no object at all; the readout then
// reports a failure carrying the stderr tail.

import { type DiagnosticModel, num, readDiagnostics, record, str } from '../../shell/cliJson';
import type { CheckSceneResult } from '../../shell/host';
import type { Vec3 } from '../gizmo/vecmath';
import type { ClipBinding } from './assembly';

/** One diagnostic as the CLI serialized it; `severity` separates warnings (which do not fail the
 *  check) from errors. `ludion explain <code>` documents the code. */
export type ReadoutDiagnostic = DiagnosticModel;

/** Which document family the object described, or `null` when the run did not produce one.
 *  `other` is anything that validates but is neither — a scene, a mesh, a sprite. */
export type ReadoutKind = 'voxel' | 'assembly' | 'other';

/** The parsed result of one `check-scene` run. */
export interface ModelReadout {
  /** The document validated: the process exited 0 and the object says `ok`. Warnings do not clear
   *  this — a model with a stray component is valid and still renders. */
  readonly ok: boolean;
  /** The document family the object described, or `null` on a failed run. */
  readonly kind: ReadoutKind | null;
  /** The path as the CLI echoed it, or `null` when no object was printed. */
  readonly path: string | null;
  /** `[x, y, z]` grid extent, or `null` (a failed run, or a document that reports none). For an
   *  assembly this is the ASSEMBLED grid — the rest pose's extent. */
  readonly size: Vec3 | null;
  /** Filled (non-empty, palette-named) cell count, or `null`. An assembly reports none. */
  readonly filled: number | null;
  /** 6-connected component count (an assembly's is over its composed grid), or `null`. More than 1
   *  comes with an `L1129` warning. */
  readonly components: number | null;
  /** How many parts an assembly places, or `null` for anything else. */
  readonly parts: number | null;
  /** An assembly's clip bindings in the order the CLI reported them (sorted by short name); empty
   *  for anything else. */
  readonly clips: readonly ClipBinding[];
  /** The model's DECLARED intended world scale, or `null` when the document declares none.
   *  Metadata, never engine-applied. */
  readonly scale: number | null;
  /** Every diagnostic, in emission order. Errors and warnings are mixed; filter by `severity`. */
  readonly diagnostics: readonly ReadoutDiagnostic[];
  /** One line for the status bar: the model's shape when it validated, else `L#### <message>` for
   *  the first error. Engine wording verbatim — the editor does not paraphrase diagnostics. */
  readonly summary: string;
}

/**
 * Reads one `runCheckScene` result.
 *
 * Total: output that is not the summary object yields a failed readout carrying the stderr tail (or
 * the exit code), never a throw — the status bar must always have something to say. A document that
 * is neither a voxel model nor an assembly reports `ok` with `kind: 'other'` and null measurements.
 */
export function parseCheckScene(result: CheckSceneResult): ModelReadout {
  const json = lastObject(result.stdout);
  if (json === null) {
    return { ...EMPTY, summary: failureSummary([], result) };
  }
  const diagnostics = readDiagnostics(json.diagnostics);
  const path = str(json.path);
  if (result.exitCode !== 0 || json.ok !== true) {
    return { ...EMPTY, path, diagnostics, summary: failureSummary(diagnostics, result) };
  }

  const size = vec3(json.size);
  const components = num(json.components);
  if (json.kind === 'voxel' && size !== null) {
    const filled = num(json.filled);
    const scale = num(json.scale);
    return {
      ...EMPTY,
      ok: true,
      kind: 'voxel',
      path,
      size,
      filled,
      components,
      scale,
      diagnostics,
      summary: okSummary(size, filled ?? 0, components ?? 0, scale, diagnostics),
    };
  }
  if (json.kind === 'assembly' && size !== null) {
    const parts = Array.isArray(json.parts) ? json.parts.length : 0;
    const clips = clipBindings(json.clips);
    return {
      ...EMPTY,
      ok: true,
      kind: 'assembly',
      path,
      size,
      components,
      parts,
      clips,
      diagnostics,
      summary: assemblySummary(str(json.name) ?? undefined, size, parts, clips.length, diagnostics),
    };
  }

  // Valid, but neither family — report it honestly rather than inventing measurements.
  const kind = str(json.kind) ?? 'document';
  const warning = diagnostics.find((d) => d.severity === 'warning');
  return {
    ...EMPTY,
    ok: true,
    kind: 'other',
    path,
    diagnostics,
    summary: warning === undefined ? kind : `${kind} · ${headline(warning)}`,
  };
}

/** Every field's "nothing was read" value, so each branch above states only what it actually
 *  learned. */
const EMPTY: ModelReadout = {
  ok: false,
  kind: null,
  path: null,
  size: null,
  filled: null,
  components: null,
  parts: null,
  clips: [],
  scale: null,
  diagnostics: [],
  summary: '',
};

/** The last non-empty stdout line parsed as the summary object (a boolean `ok` marks it), or
 *  `null`. */
function lastObject(stdout: string): Record<string, unknown> | null {
  const line = lines(stdout)
    .filter((l) => l.trim() !== '')
    .at(-1);
  if (line === undefined) {
    return null;
  }
  try {
    const value = record(JSON.parse(line));
    return value !== null && typeof value.ok === 'boolean' ? value : null;
  } catch {
    return null;
  }
}

function vec3(value: unknown): Vec3 | null {
  if (!Array.isArray(value) || value.length !== 3) {
    return null;
  }
  const [x, y, z] = value.map(num);
  return x === null || y === null || z === null ? null : [x, y, z];
}

function clipBindings(value: unknown): ClipBinding[] {
  const out: ClipBinding[] = [];
  for (const entry of Array.isArray(value) ? value : []) {
    const row = record(entry);
    const name = row === null ? null : str(row.name);
    const file = row === null ? null : str(row.file);
    if (name !== null && file !== null) {
      out.push({ name, file });
    }
  }
  return out;
}

/** `9x9x9 · 283 filled · 1 component · scale 0.24`. */
function okSummary(
  size: Vec3,
  filled: number,
  components: number,
  scale: number | null,
  diagnostics: readonly ReadoutDiagnostic[],
): string {
  const parts = [
    `${size[0]}x${size[1]}x${size[2]}`,
    `${filled} filled`,
    `${components} component${components === 1 ? '' : 's'}`,
  ];
  if (scale !== null) {
    parts.push(`scale ${scale}`);
  }
  return withFirstWarning(parts, diagnostics);
}

/** `L#### <first line of the message>` — engine messages can run to several lines; the full text
 *  stays in `diagnostics`. */
function headline(diagnostic: ReadoutDiagnostic): string {
  return `${diagnostic.code} ${diagnostic.message.split('\n')[0]}`;
}

/** Joins the summary's own facts, appending the first warning — a stray component is the readout's
 *  whole reason for existing on a *passing* model. */
function withFirstWarning(parts: string[], diagnostics: readonly ReadoutDiagnostic[]): string {
  const warning = diagnostics.find((d) => d.severity === 'warning');
  if (warning !== undefined) {
    parts.push(headline(warning));
  }
  return parts.join(' · ');
}

/** `assembly player · 13x32x9 · 11 parts · 2 clips`, with the first warning appended — an assembly's
 *  `L1129` and `L1143` are exactly the checks a rig edit is judged by. */
function assemblySummary(
  name: string | undefined,
  size: Vec3,
  parts: number,
  clips: number,
  diagnostics: readonly ReadoutDiagnostic[],
): string {
  const head = name === undefined ? 'assembly' : `assembly ${name}`;
  return withFirstWarning(
    [
      head,
      `${size[0]}x${size[1]}x${size[2]}`,
      `${parts} part${parts === 1 ? '' : 's'}`,
      `${clips} clip${clips === 1 ? '' : 's'}`,
    ],
    diagnostics,
  );
}

function failureSummary(
  diagnostics: readonly ReadoutDiagnostic[],
  result: CheckSceneResult,
): string {
  const error = diagnostics.find((d) => d.severity === 'error') ?? diagnostics[0];
  if (error !== undefined) {
    return headline(error);
  }
  const stderrTail = lines(result.stderr).at(-1);
  if (stderrTail !== undefined && stderrTail !== '') {
    return stderrTail;
  }
  return result.exitCode === null
    ? 'check-scene was killed'
    : `check-scene exited ${result.exitCode}`;
}

/** Splits on either line ending and drops the trailing empty element. */
function lines(text: string): string[] {
  const split = text.split(/\r?\n/);
  if (split.at(-1) === '') {
    split.pop();
  }
  return split;
}
