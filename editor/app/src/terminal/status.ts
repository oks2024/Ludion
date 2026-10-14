// The terminal tab's status chip, as a pure function of observable facts. Deliberately
// simple: a session that emitted PTY output recently is `working`, one that's been quiet is
// `idle`, one whose child exited is `exited`. "Waiting-for-input" detection is intentionally
// not attempted here (it needs prompt-shape heuristics that overfit); it can come later.

export type TermStatus = 'working' | 'idle' | 'exited';

/** How long after the last output chunk a session is still shown as `working`. Terminal
 *  output during a Claude turn is bursty but frequent; ~1.5 s of silence reads as idle. */
export const IDLE_AFTER_MS = 1500;

export interface StatusInput {
  /** ms timestamp of the last `pty://output` chunk, or null before any output. */
  readonly lastOutputAt: number | null;
  /** True once `pty://exit` has fired for this session. */
  readonly exited: boolean;
  readonly now: number;
}

export function terminalStatus({ lastOutputAt, exited, now }: StatusInput): TermStatus {
  if (exited) {
    return 'exited';
  }
  if (lastOutputAt !== null && now - lastOutputAt < IDLE_AFTER_MS) {
    return 'working';
  }
  return 'idle';
}
