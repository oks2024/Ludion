// Normalizes any thrown value into the structured, displayable diagnostic the UI shows.
//
// A rejected `call()` is usually a `ProtocolError` (the engine's structured `{code, message, help}`
// diagnostic — the "errors are the UX budget" pillar reaching the human), but it can also be a
// transport `ProtocolClientError` or, defensively, anything else. This flattens all three into one
// shape so a single `<DiagnosticBanner>` renders them non-modally without knowing the source.

import type { LogEvent, WireErrorSchema } from '@ludion/protocol';
import { ProtocolError } from '../protocol/client';

/** A displayable diagnostic: an optional L-code, a message, an optional help line, and the
 *  diagnostics that accompany it (a failed `scene.load` reports its later errors as `related`). */
export interface DiagnosticView {
  readonly code: string | null;
  readonly message: string;
  readonly help: string | null;
  readonly related?: readonly DiagnosticView[];
}

/** Lifts a wire diagnostic into a {@link DiagnosticView}, recursing over `related`. */
export function fromWire(d: WireErrorSchema): DiagnosticView {
  return { code: d.code, message: d.message, help: d.help ?? null, related: (d.related ?? []).map(fromWire) };
}

/** Coerces an unknown thrown value into a {@link DiagnosticView}. */
export function toDiagnostic(err: unknown): DiagnosticView {
  if (err instanceof ProtocolError) {
    return {
      code: err.code,
      message: err.diagnosticMessage,
      help: err.help,
      related: err.related.map(fromWire),
    };
  }
  if (err instanceof Error) {
    return { code: null, message: err.message, help: null };
  }
  return { code: null, message: String(err), help: null };
}

/** A {@link DiagnosticView} as plain text for a one-line note: `L1234: message — help`. */
export function diagnosticText(d: DiagnosticView): string {
  const head = d.code === null ? d.message : `${d.code}: ${d.message}`;
  return d.help === null ? head : `${head} — ${d.help}`;
}

/** Log levels worth interrupting the human for. Info/debug/trace are normal chatter (e.g. the
 *  `scene loaded` note) and would only add banner noise; only warnings and errors surface. */
const SURFACED_LEVELS: ReadonlySet<LogEvent['level']> = new Set<LogEvent['level']>(['warn', 'error']);

/** The fallback for a log line without the structured `diagnostic` field (an older engine): the
 *  first line of a diagnostic's rustc-style `Display` (`error[L1101]: message`, or
 *  `warning[L1121]: …`) and its `  help: …` continuation line. Matching it lets the L-code render in
 *  the banner's code chip instead of being buried in the message text. The banner's own
 *  error/warning styling comes from the log event's `level`, so only the code and message are
 *  lifted out here. */
const DIAGNOSTIC_HEAD = /^(?:error|warning)\[(L\d{4})\]:\s*(.*)$/;
const HELP_PREFIX = 'help: ';

/**
 * Turns a `log`-channel event into a displayable diagnostic, or `null` when its level is below the
 * surfacing threshold. The engine's live loop reports a render failure — a missing texture (`L1101`),
 * a bad extract — as an *error*-level log, so this is what turns an otherwise silent "waiting for
 * frames…" stall into a visible banner. The event's structured `diagnostic` is used when present;
 * otherwise the rendered text is parsed. A non-diagnostic message is surfaced verbatim with no code.
 */
export function logEventToDiagnostic(event: LogEvent): DiagnosticView | null {
  if (!SURFACED_LEVELS.has(event.level)) {
    return null;
  }
  if (event.diagnostic) {
    return fromWire(event.diagnostic);
  }
  const lines = event.message.split('\n');
  const helpLine = lines.find((line) => line.trimStart().startsWith(HELP_PREFIX));
  const help = helpLine !== undefined ? helpLine.trimStart().slice(HELP_PREFIX.length) : null;
  const head = lines[0]?.match(DIAGNOSTIC_HEAD) ?? null;
  if (head !== null) {
    return { code: head[1] as string, message: head[2] as string, help };
  }
  return { code: null, message: event.message, help };
}
