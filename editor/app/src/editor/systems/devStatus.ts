// Pure parsing, seq-gating, and path-matching for `<projectDir>/.ludion/dev-status.json` — the file
// `ludion dev` writes atomically on every build transition (Wave B). The code panel polls it, and
// this module turns its bytes into a typed status, decides whether a poll saw something new (the
// monotonic `seq`), and matches rustc diagnostic file paths against the open file. All pure, so it's
// unit-tested against literals (devStatus.test.ts) with no host or timers.

export type DevState = 'building' | 'ok' | 'failed' | 'crashed' | 'restore_failed';

/** One rustc diagnostic as `ludion dev` records it. `file` is as rustc emits it — typically relative
 *  to the project dir (e.g. `src/movement.rs`), which is why matching is suffix/separator-tolerant. */
export interface DevDiagnostic {
  readonly level: string; // "error" | "warning" | ...
  readonly code: string | null;
  readonly message: string;
  readonly file: string | null;
  readonly line: number | null;
  readonly column: number | null;
  readonly rendered: string | null;
}

export interface DevStatus {
  readonly state: DevState;
  readonly seq: number;
  readonly timestamp: string;
  readonly durationMs: number | null;
  readonly restoredFrame: number | null;
  /** Why the restore failed / where its snapshot was kept (`restore_failed`), or why the session
   *  state was lost (`crashed`, and the `ok` that follows once the fresh child serves). */
  readonly message: string | null;
  readonly snapshot: string | null;
  readonly diagnostics: readonly DevDiagnostic[];
}

function asString(v: unknown): string | null {
  return typeof v === 'string' ? v : null;
}
function asNumber(v: unknown): number | null {
  return typeof v === 'number' && Number.isFinite(v) ? v : null;
}

const STATES: readonly DevState[] = [
  'building',
  'ok',
  'failed',
  'crashed',
  'restore_failed',
];

function parseDiagnostic(raw: unknown): DevDiagnostic | null {
  if (typeof raw !== 'object' || raw === null) {
    return null;
  }
  const o = raw as Record<string, unknown>;
  const message = asString(o.message);
  if (message === null) {
    return null;
  }
  return {
    level: asString(o.level) ?? 'error',
    code: asString(o.code),
    message,
    file: asString(o.file),
    line: asNumber(o.line),
    column: asNumber(o.column),
    rendered: asString(o.rendered),
  };
}

/** Parse the dev-status JSON text defensively. Returns `null` for invalid JSON or a shape missing
 *  the required `state`/`seq` — the caller treats that as "no usable status", never a throw. */
export function parseDevStatus(text: string): DevStatus | null {
  let raw: unknown;
  try {
    raw = JSON.parse(text);
  } catch {
    return null;
  }
  if (typeof raw !== 'object' || raw === null) {
    return null;
  }
  const o = raw as Record<string, unknown>;
  const state = asString(o.state);
  const seq = asNumber(o.seq);
  if (state === null || !STATES.includes(state as DevState) || seq === null) {
    return null;
  }
  const diagnostics = Array.isArray(o.diagnostics)
    ? o.diagnostics.map(parseDiagnostic).filter((d): d is DevDiagnostic => d !== null)
    : [];
  return {
    state: state as DevState,
    seq,
    timestamp: asString(o.timestamp) ?? '',
    durationMs: asNumber(o.duration_ms),
    restoredFrame: asNumber(o.restored_frame),
    message: asString(o.message),
    snapshot: asString(o.snapshot),
    diagnostics,
  };
}

/** True when a freshly-read status is newer than the last one reacted to (monotonic `seq`). A `null`
 *  previous seq (nothing seen yet) is always newer. Lets the poll skip re-rendering an unchanged file. */
export function isNewer(prevSeq: number | null, status: DevStatus): boolean {
  return prevSeq === null || status.seq > prevSeq;
}

/** Normalize a path for tolerant comparison: `\`→`/`, collapse runs, lowercase (Windows paths are
 *  case-insensitive; over-normalizing is safe here since we only compare, never open, these). */
export function normalizePath(p: string): string {
  return p.replace(/\\/g, '/').replace(/\/+/g, '/').toLowerCase();
}

/** Does a diagnostic's `file` refer to `openPath`? Tolerant of `/`-vs-`\\` and relative-vs-absolute:
 *  equal after normalization, or one is a path-segment-boundary suffix of the other (so `movement.rs`
 *  matches `.../src/movement.rs` but NOT `.../enemy_movement.rs`). */
export function pathsMatch(diagFile: string, openPath: string): boolean {
  const a = normalizePath(diagFile);
  const b = normalizePath(openPath);
  if (a === '' || b === '') {
    return false;
  }
  if (a === b) {
    return true;
  }
  const withBoundary = (s: string) => (s.startsWith('/') ? s : `/${s}`);
  return b.endsWith(withBoundary(a)) || a.endsWith(withBoundary(b));
}

/** The diagnostics whose file resolves to `openPath` — the ones rendered as in-editor decorations. */
export function diagnosticsForFile(
  diagnostics: readonly DevDiagnostic[],
  openPath: string,
): DevDiagnostic[] {
  return diagnostics.filter((d) => d.file !== null && pathsMatch(d.file, openPath));
}
