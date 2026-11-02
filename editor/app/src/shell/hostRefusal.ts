// Recognizes the editor host's path-confinement refusals so a view can show them instead of
// reading them as "no file yet". The fragments are the host's plain-string messages from
// `src-tauri/src/confine.rs` `check_under`; keep the two in step (hostRefusal.test.ts pins them).

const FRAGMENTS = [
  'is not an absolute path; the editor host',
  'climbs with `..`',
  'is outside the engine checkout and every Ludion project',
] as const;

/** Whether `err` is a host confinement refusal (never true for an ordinary I/O error). */
export function isConfinementRefusal(err: unknown): boolean {
  const text = typeof err === 'string' ? err : err instanceof Error ? err.message : '';
  return FRAGMENTS.some((f) => text.includes(f));
}
