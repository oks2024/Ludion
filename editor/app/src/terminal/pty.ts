// Typed wrappers over the Tauri PTY host (src-tauri/src/pty.rs) plus its event streams.
// Everything Tauri-specific for the terminal drawer funnels through here so the React
// components and the pure logic modules never touch `invoke`/`listen` directly.

import { invoke } from '@tauri-apps/api/core';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';

/** Spawn options — mirrors `pty::SpawnOpts` (env as ordered pairs). */
export interface PtySpawnOpts {
  readonly id: string;
  readonly program: string;
  readonly args: readonly string[];
  readonly cwd: string;
  readonly env: readonly (readonly [string, string])[];
  readonly cols: number;
  readonly rows: number;
}

/** Repo layout resolved host-side (`pty::EditorPaths`). `launchProject` is the process's
 *  `--project` argument, if any. */
export interface EditorPaths {
  readonly repoRoot: string;
  readonly personasDir: string;
  readonly relayScript: string;
  readonly launchProject: string | null;
}

/** A validated game project (`project::ProjectInfo` via `project_info`). */
export interface ProjectInfo {
  readonly dir: string;
  readonly name: string;
  readonly defaultScene: string;
}

/** `pty://output` payload: base64 of the raw PTY bytes (never a lossy string — a read can
 *  split a UTF-8 sequence). */
interface OutputPayload {
  readonly id: string;
  readonly dataB64: string;
}

/** `pty://exit` payload: fired exactly once, session already reaped host-side. */
interface ExitPayload {
  readonly id: string;
  readonly code: number | null;
}

/** Decode a base64 `pty://output` chunk to the raw bytes xterm.write wants. `atob` yields
 *  one code unit per byte; copying its char codes reconstructs the exact byte stream. */
export function decodeOutput(dataB64: string): Uint8Array {
  const binary = atob(dataB64);
  const bytes = new Uint8Array(binary.length);
  for (let i = 0; i < binary.length; i += 1) {
    bytes[i] = binary.charCodeAt(i);
  }
  return bytes;
}

export function ptySpawn(opts: PtySpawnOpts): Promise<void> {
  return invoke('pty_spawn', { opts });
}

export function ptyWrite(id: string, data: string): Promise<void> {
  return invoke('pty_write', { id, data });
}

export function ptyResize(id: string, cols: number, rows: number): Promise<void> {
  return invoke('pty_resize', { id, cols, rows });
}

export function ptyKill(id: string): Promise<void> {
  return invoke('pty_kill', { id });
}

export function ptyList(): Promise<string[]> {
  return invoke('pty_list');
}

export function writeEditorContext(cwd: string, json: string): Promise<void> {
  return invoke('write_editor_context', { cwd, json });
}

export function readEditorFile(path: string): Promise<string> {
  return invoke('read_editor_file', { path });
}

/** A file's modification time in epoch milliseconds (`pty::FileStat`). */
export interface FileStat {
  readonly mtimeMs: number;
}

/** Stat a path for the code panel's change poll. Rejects when the path is missing/unreadable. */
export function statEditorFile(path: string): Promise<FileStat> {
  return invoke('stat_editor_file', { path });
}

/** Whether a path exists on the host — a directory included (`stat_editor_file` is a plain metadata
 *  read). Never rejects: missing and unreadable both read as `false`. */
export function editorPathExists(path: string): Promise<boolean> {
  return statEditorFile(path).then(
    () => true,
    () => false,
  );
}

/** Atomically write a text file and return its new mtime (the caller adopts it as its baseline). */
export function writeEditorFile(path: string, text: string): Promise<FileStat> {
  return invoke('write_editor_file', { path, text });
}

export function editorPaths(): Promise<EditorPaths> {
  return invoke('editor_paths');
}

/** Validates `dir` as a project (`dir/ludion.toml` exists and parses); rejects with the host's
 *  refusal message otherwise. */
export function projectInfo(dir: string): Promise<ProjectInfo> {
  return invoke('project_info', { dir });
}

/** Copies every stage scene missing from `<project>/scenes/` out of the checkout's template dir,
 *  never overwriting an existing file. Resolves to the `scenes/<file>` paths actually written. */
export function addStageScenes(project: string): Promise<string[]> {
  return invoke('add_stage_scenes', { project });
}

/** Subscribe to raw output for every session; the handler decodes and routes by id. */
export function onPtyOutput(handler: (id: string, bytes: Uint8Array) => void): Promise<UnlistenFn> {
  return listen<OutputPayload>('pty://output', (event) => {
    handler(event.payload.id, decodeOutput(event.payload.dataB64));
  });
}

/** Subscribe to session exits (fires once per session). */
export function onPtyExit(handler: (id: string, code: number | null) => void): Promise<UnlistenFn> {
  return listen<ExitPayload>('pty://exit', (event) => {
    handler(event.payload.id, event.payload.code);
  });
}
