// Typed wrappers over the Generation view's host commands (`src-tauri/src/generation.rs`).
// Same posture as `shell/host.ts` and `terminal/pty.ts`: everything Tauri-specific funnels through
// one module, so components never touch `invoke`.
//
// THE HOST RUNS THE CLI; IT IS NOT A SECOND IMPLEMENTATION. `genInvoke` spawns
// `ludion gen <verb> … --json` in the game project and hands back the one object it printed
// (`Plan/comfy-generation.md` decision 15). Nothing here — and nothing host-side — speaks HTTP to a
// ComfyUI server, so the view has no capability a shell does not.
//
// THE ADDRESS IS PER-SESSION AND NEVER PERSISTED. `url` is what the human typed into the view for
// this session; omit it and the child inherits `LUDION_COMFY_URL` from the host's environment, which
// is where the address belongs (`ludion docs generation`). It is never written to a recipe, a
// run record or `localStorage`.

import { invoke } from '@tauri-apps/api/core';

let ticketCounter = 0;

/** A fresh name for one run, so `genCancel` can find its child. */
export function nextGenTicket(): string {
  ticketCounter += 1;
  return `gen-${ticketCounter}`;
}

/** The verbs the host will run — the allowlist in `generation::GEN_VERBS`, mirrored. */
export type GenVerb = 'nodes' | 'check' | 'run' | 'status' | 'fetch' | 'sheet' | 'accept';

/** One invocation's outcome (`generation::GenResult`). */
export interface GenResult {
  /** The child exited 0. A verb that ran and refused is `ok: false` — the promise still resolves. */
  readonly ok: boolean;
  /** The one JSON object the verb printed. A failing verb still prints it (`ok: false`, diagnostics
   *  inside); `null` only when nothing parseable was printed — clap's exit-2 argv refusal, a crash. */
  readonly json: unknown;
  /** stderr verbatim: the diagnostics, and cargo's own output above them. */
  readonly stderr: string;
  readonly exitCode: number | null;
}

/** Run one `ludion gen` verb with `projectDir` as its working directory.
 *
 *  `args` are the verb's own positional arguments and flags; `--json` is appended host-side and
 *  `--url` is this call's `url` (not sent to `sheet` and `accept`, which never contact a server), so
 *  passing either inside `args` is refused. Rejects only when the
 *  process could not be run at all (no cargo, a bad project directory, a verb outside the
 *  allowlist) — a failing verb resolves with `ok: false`.
 *
 *  A `gen run` takes minutes; the host runs it off the UI thread and this promise waits it out.
 *  Pass a `ticket` from {@link nextGenTicket} to make the run cancellable with {@link genCancel}. */
export function genInvoke(
  verb: GenVerb,
  args: readonly string[],
  projectDir: string,
  url?: string,
  ticket?: string,
): Promise<GenResult> {
  return invoke('gen_invoke', { verb, args, projectDir, url: url ?? null, ticket: ticket ?? null });
}

/** Stop the run started under `ticket`: its process tree is killed and its `genInvoke` resolves
 *  `ok: false` with a cancel note on stderr. `false` if nothing runs under it any more. Jobs the run
 *  already queued on the ComfyUI server are not cancelled there. */
export function genCancel(ticket: string): Promise<boolean> {
  return invoke('gen_cancel', { ticket });
}

/** Read an image file as a `data:` url, for the candidate grid.
 *
 *  The webview has no file access of its own, so a candidate reaches the page as bytes in a string —
 *  the shape the Art Director view already renders a capture from. Rejects for a non-image
 *  extension, a missing file, or one over the host's inline size limit. */
export function readEditorImage(path: string): Promise<string> {
  return invoke('read_editor_image', { path });
}

/** A file's size in bytes — the one fact the grid can state about a mesh candidate before it is
 *  accepted (`gen sheet` refuses a mesh run, so there is no thumbnail to show instead). */
export function editorFileSize(path: string): Promise<number> {
  return invoke('editor_file_size', { path });
}
