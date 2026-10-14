// Parsing the Claude Code statusline payload and deriving the two per-tab readouts the
// mockup shows: the context-usage bar (used / window tokens) and the prompt-cache countdown.
//
// The statusline hook pipes a JSON object to the relay on every assistant message plus a
// refresh interval; the relay writes it verbatim to the sink file the drawer tails. The
// field of interest is `context_window` (Claude Code's shape):
//   { total_input_tokens, context_window_size, used_percentage,
//     current_usage: { cache_creation_input_tokens, cache_read_input_tokens } }
// Everything here is pure and defensive — a missing/renamed field degrades to a "—" readout,
// never a throw (the relay is the single point of repair if the payload shifts).

/** Prompt-cache TTL. Cache has NO direct expiry signal anywhere; the countdown is derived
 *  from the last observed cache write plus this window (see `cacheCountdownMs`). */
export const CACHE_TTL_MS = 5 * 60 * 1000;

export interface StatusPayload {
  readonly usedTokens: number | null;
  readonly contextSize: number | null;
  readonly usedPercent: number | null;
  readonly cacheCreation: number | null;
  readonly cacheRead: number | null;
}

function numberOrNull(value: unknown): number | null {
  return typeof value === 'number' && Number.isFinite(value) ? value : null;
}

/** Extract the fields we render from a parsed statusline JSON object. Unknown shapes yield an
 *  all-null payload rather than throwing. */
export function parseStatusPayload(json: unknown): StatusPayload {
  const root = typeof json === 'object' && json !== null ? (json as Record<string, unknown>) : {};
  const cw =
    typeof root.context_window === 'object' && root.context_window !== null
      ? (root.context_window as Record<string, unknown>)
      : {};
  const usage =
    typeof cw.current_usage === 'object' && cw.current_usage !== null
      ? (cw.current_usage as Record<string, unknown>)
      : {};
  return {
    usedTokens: numberOrNull(cw.total_input_tokens),
    contextSize: numberOrNull(cw.context_window_size),
    usedPercent: numberOrNull(cw.used_percentage),
    cacheCreation: numberOrNull(usage.cache_creation_input_tokens),
    cacheRead: numberOrNull(usage.cache_read_input_tokens),
  };
}

/** Parse the raw sink-file text; returns null when it isn't valid JSON yet (the relay writes
 *  atomically, but the file may not exist before the first assistant message). */
export function parseStatusText(text: string): StatusPayload | null {
  try {
    return parseStatusPayload(JSON.parse(text));
  } catch {
    return null;
  }
}

/** Fraction 0–1 for the context-usage bar, from the explicit percent if present, else derived
 *  from used/size. Null when neither is available. */
export function contextFraction(p: StatusPayload | null): number | null {
  if (p === null) {
    return null;
  }
  if (p.usedPercent !== null) {
    return Math.max(0, Math.min(1, p.usedPercent / 100));
  }
  if (p.usedTokens !== null && p.contextSize !== null && p.contextSize > 0) {
    return Math.max(0, Math.min(1, p.usedTokens / p.contextSize));
  }
  return null;
}

/** Round a token count to a compact `68k` / `950` string. */
function compactTokens(n: number): string {
  return n >= 1000 ? `${Math.round(n / 1000)}k` : String(Math.round(n));
}

/** The `68k/200k` context readout, or `—` before the first payload. */
export function formatContextTokens(p: StatusPayload | null): string {
  if (p === null || p.usedTokens === null) {
    return '—';
  }
  const used = compactTokens(p.usedTokens);
  return p.contextSize === null ? used : `${used}/${compactTokens(p.contextSize)}`;
}

// --- Cache countdown ----------------------------------------------------------------------
//
// The countdown resets each time a NEW cache write is observed. "New" is judged by a signature
// over the fields that move per turn (used tokens + cache-creation tokens): whenever a payload
// with cache_creation_input_tokens > 0 arrives whose signature differs from the last one that
// reset the clock, we stamp `lastWriteAt = now`. The countdown is then `TTL − (now − lastWriteAt)`.

export interface CacheState {
  readonly lastWriteAt: number | null;
  /** Signature of the payload that last reset the clock; guards against re-stamping while the
   *  same turn's cache_creation value keeps showing up across refresh reads. */
  readonly lastSig: string | null;
}

export const INITIAL_CACHE_STATE: CacheState = { lastWriteAt: null, lastSig: null };

function cacheSig(p: StatusPayload): string {
  return `${p.usedTokens ?? ''}:${p.cacheCreation ?? ''}`;
}

/** Fold one observed payload into the cache state, stamping a fresh write timestamp only for a
 *  genuinely new cache-creating turn. */
export function updateCacheState(state: CacheState, p: StatusPayload | null, now: number): CacheState {
  if (p === null || p.cacheCreation === null || p.cacheCreation <= 0) {
    return state;
  }
  const sig = cacheSig(p);
  if (sig === state.lastSig) {
    return state;
  }
  return { lastWriteAt: now, lastSig: sig };
}

/** Remaining cache-warm ms (clamped to ≥0), or null if no cache write has been seen yet. */
export function cacheCountdownMs(state: CacheState, now: number, ttl = CACHE_TTL_MS): number | null {
  if (state.lastWriteAt === null) {
    return null;
  }
  return Math.max(0, state.lastWriteAt + ttl - now);
}

/** `m:ss` for the countdown chip, or `—` when unknown. */
export function formatCountdown(ms: number | null): string {
  if (ms === null) {
    return '—';
  }
  const totalSeconds = Math.floor(ms / 1000);
  const minutes = Math.floor(totalSeconds / 60);
  const seconds = totalSeconds % 60;
  return `${minutes}:${String(seconds).padStart(2, '0')}`;
}
