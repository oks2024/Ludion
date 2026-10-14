// A dial that finishes after Disconnect, a project switch or a newer dial must be discarded, never
// promoted: the `await` of a handshake can outlive the session that started it.

export interface DialGate {
  /** Starts a dial; supersedes every earlier token. */
  begin(): number;
  isCurrent(token: number): boolean;
  /** Supersedes every outstanding token. */
  invalidate(): void;
}

export function createDialGate(): DialGate {
  let current = 0;
  return {
    begin: () => ++current,
    isCurrent: (token) => token === current,
    invalidate: () => {
      current += 1;
    },
  };
}

export type DialOutcome<H> =
  | { kind: 'connected'; hello: H }
  | { kind: 'failed'; error: unknown }
  | { kind: 'superseded' };

/**
 * Connects `next` to `url`. A dial the gate superseded while it was in flight closes `next` and
 * reports `superseded` (even when it failed); a plain failure closes `next` and reports `failed`.
 */
export async function dial<H>(
  next: { connect(url: string): Promise<H>; close(): void },
  url: string,
  gate: DialGate,
): Promise<DialOutcome<H>> {
  const token = gate.begin();
  let hello: H;
  try {
    hello = await next.connect(url);
  } catch (error) {
    next.close();
    return gate.isCurrent(token) ? { kind: 'failed', error } : { kind: 'superseded' };
  }
  if (!gate.isCurrent(token)) {
    next.close();
    return { kind: 'superseded' };
  }
  return { kind: 'connected', hello };
}
