// Module-level terminal registry: the PTY-backed xterm instances live here, NOT in React state,
// so they survive the drawer unmounting/remounting (a view switch) and keep their scrollback and
// live child. The React component reads this registry and re-parents the active tab's element into
// its container on mount; the PTYs and xterms never restart on a remount.
//
// Sessions are grouped by view id (only `level-designer` exists today, but the per-view grouping is
// the settled lifecycle: switching views swaps which tab set is shown, killing nothing). The global
// `pty://output` / `pty://exit` listeners are attached once and route by session id.

import { Terminal } from '@xterm/xterm';
import { FitAddon } from '@xterm/addon-fit';
import '@xterm/xterm/css/xterm.css';
import { onPtyExit, onPtyOutput, ptyKill, ptyResize, ptySpawn, ptyWrite, type PtySpawnOpts } from './pty';
import { INITIAL_CACHE_STATE, type CacheState, type StatusPayload } from './cache';

export interface TermSession {
  readonly id: string;
  readonly viewId: string;
  readonly label: string;
  /** The model this tab was spawned with — a restart reuses it instead of re-asking. */
  readonly modelId: string;
  /** The project this session was spawned in — a restart respawns HERE, never in whatever project
   *  is current when Restart is clicked, so the tab's own history stays true across a switch. */
  readonly projectDir: string;
  /** The spawning project's name, for the tab label; `null` if `project_info` hadn't resolved one. */
  readonly projectName: string | null;
  readonly term: Terminal;
  readonly fit: FitAddon;
  /** The persistent element xterm renders into; re-parented into the drawer container on mount. */
  readonly host: HTMLDivElement;
  lastOutputAt: number | null;
  exited: boolean;
  exitCode: number | null;
  /** Latest parsed statusline payload (context/cache), refreshed by the drawer's sink poll. */
  status: StatusPayload | null;
  cache: CacheState;
}

const byView = new Map<string, TermSession[]>();
const byId = new Map<string, TermSession>();

let listenersReady = false;
function ensureListeners(): void {
  if (listenersReady) {
    return;
  }
  listenersReady = true;
  // App-lifetime subscriptions; never torn down.
  void onPtyOutput((id, bytes) => {
    const session = byId.get(id);
    if (session === undefined) {
      return;
    }
    session.lastOutputAt = Date.now();
    session.term.write(bytes);
  });
  void onPtyExit((id, code) => {
    const session = byId.get(id);
    if (session === undefined) {
      return;
    }
    session.exited = true;
    session.exitCode = code;
    session.term.write(`\r\n\x1b[2m[process exited${code === null ? '' : ` · code ${code}`}]\x1b[0m\r\n`);
  });
}

export function listSessions(viewId: string): readonly TermSession[] {
  return byView.get(viewId) ?? [];
}

export function hasSession(id: string): boolean {
  return byId.has(id);
}

/** A fresh, collision-proof session id for a view (also the PTY id and the sink-file suffix). */
export function nextTermId(viewId: string): string {
  return `${viewId}-${Date.now().toString(36)}-${Math.floor(Math.random() * 1e6).toString(36)}`;
}

function createXterm(): { term: Terminal; fit: FitAddon; host: HTMLDivElement } {
  const host = document.createElement('div');
  host.className = 'term-host';
  const term = new Terminal({
    fontFamily: '"Cascadia Code", "Consolas", ui-monospace, monospace',
    fontSize: 12.5,
    cursorBlink: true,
    // Match the mockup's terminal body.
    theme: { background: '#0b0c0f', foreground: '#edeef2', cursor: '#9ba0ab' },
  });
  const fit = new FitAddon();
  term.loadAddon(fit);
  // Open into the detached host BEFORE spawn: xterm's default DSR responder must be live so the
  // child doesn't stall on ConPTY's initial `\x1b[6n` cursor-position query.
  term.open(host);
  return { term, fit, host };
}

export interface SpawnSessionArgs {
  readonly viewId: string;
  readonly label: string;
  readonly modelId: string;
  readonly opts: PtySpawnOpts;
  readonly projectDir: string;
  readonly projectName: string | null;
  /** When set, dispose this session and take its slot (restart-fresh). */
  readonly replaceId?: string;
}

/** Create an xterm, wire its input to the PTY, and spawn. Rejects if the host spawn fails (the
 *  caller surfaces the error and the just-created xterm is disposed). */
export async function spawnSession({
  viewId,
  label,
  modelId,
  opts,
  projectDir,
  projectName,
  replaceId,
}: SpawnSessionArgs): Promise<TermSession> {
  ensureListeners();
  const { term, fit, host } = createXterm();
  term.onData((data) => void ptyWrite(opts.id, data));

  const session: TermSession = {
    id: opts.id,
    viewId,
    label,
    modelId,
    projectDir,
    projectName,
    term,
    fit,
    host,
    lastOutputAt: null,
    exited: false,
    exitCode: null,
    status: null,
    cache: INITIAL_CACHE_STATE,
  };

  try {
    await ptySpawn(opts);
  } catch (err) {
    term.dispose();
    throw err;
  }

  const tabs = byView.get(viewId) ?? [];
  if (replaceId !== undefined) {
    const index = tabs.findIndex((t) => t.id === replaceId);
    disposeSession(replaceId);
    const after = byView.get(viewId) ?? [];
    if (index >= 0) {
      after.splice(index, 0, session);
    } else {
      after.push(session);
    }
    byView.set(viewId, after);
  } else {
    tabs.push(session);
    byView.set(viewId, tabs);
  }
  byId.set(opts.id, session);
  return session;
}

/** Kill the child (async, fire-and-forget) and drop the session + xterm from the registry. */
export function disposeSession(id: string): void {
  const session = byId.get(id);
  if (session === undefined) {
    return;
  }
  void ptyKill(id).catch(() => {
    // Already exited / never spawned — nothing to reap.
  });
  session.term.dispose();
  session.host.remove();
  byId.delete(id);
  const tabs = byView.get(session.viewId);
  if (tabs !== undefined) {
    byView.set(
      session.viewId,
      tabs.filter((t) => t.id !== id),
    );
  }
}

/** Fit the terminal to its current container size and push the new dimensions to the PTY. Safe to
 *  call only once the host is in the DOM with a nonzero box. */
export function fitSession(session: TermSession): void {
  try {
    session.fit.fit();
    void ptyResize(session.id, session.term.cols, session.term.rows).catch(() => {
      // The session may have exited between fit and resize; harmless.
    });
  } catch {
    // Container not laid out yet (zero size) — the next resize/tick retries.
  }
}
