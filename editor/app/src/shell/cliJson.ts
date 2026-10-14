// Readers for a CLI verb's `--json` stdout: the host runs the verb and hands back whatever it
// printed, so every reader takes `unknown` and returns `null` (or an empty list) rather than
// throwing. Shared by the Generation view's models and the voxel model readout.
//
// Pure: no React, no Tauri, no DOM.

/** One `L####` diagnostic as a verb serializes it. `severity` is omitted for an error, which is why
 *  it is defaulted rather than required. */
export interface DiagnosticModel {
  readonly code: string;
  readonly severity: 'error' | 'warning';
  readonly message: string;
  /** A JSON Pointer into the document the diagnostic refers to, when it has one. */
  readonly path: string | null;
  readonly help: string | null;
}

export function record(value: unknown): Record<string, unknown> | null {
  return typeof value === 'object' && value !== null && !Array.isArray(value)
    ? (value as Record<string, unknown>)
    : null;
}

export function str(value: unknown): string | null {
  return typeof value === 'string' ? value : null;
}

export function num(value: unknown): number | null {
  return typeof value === 'number' && Number.isFinite(value) ? value : null;
}

/** Every well-formed `Diagnostic` in a serialized list; a `related` block reads as part of the same
 *  list. */
export function readDiagnostics(value: unknown): DiagnosticModel[] {
  if (!Array.isArray(value)) {
    return [];
  }
  const out: DiagnosticModel[] = [];
  for (const entry of value) {
    const row = record(entry);
    const code = row === null ? null : str(row.code);
    if (row === null || code === null) {
      continue;
    }
    out.push({
      code,
      severity: row.severity === 'warning' ? 'warning' : 'error',
      message: str(row.message) ?? '',
      path: str(row.path),
      help: str(row.help),
    });
    // A `related` block is context for the one above it, so it reads as part of the same list
    // rather than disappearing.
    out.push(...readDiagnostics(row.related));
  }
  return out;
}
