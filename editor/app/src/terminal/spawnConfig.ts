// Assembles the `pty_spawn` options for a persona `claude` instance — pure, so the exact
// command line is unit-tested. The instance is a peer protocol client: it drives the same live
// `ludion serve` the human's editor is attached to, via `ludion-mcp`, and reports its context/
// cache telemetry back through a statusline relay. Everything the instance needs at spawn arrives
// through flags — there is no positional first prompt, so the first user turn is the designer's.

import type { PtySpawnOpts } from './pty';

/** Everything the drawer resolves at runtime (host paths, the live url, the project dir) before
 *  building a spawn. */
export interface SpawnConfigInput {
  readonly termId: string;
  /** The connected engine url, threaded down from App — the mcp server dials exactly this. */
  readonly url: string;
  /** cwd for the instance = the game project directory. */
  readonly projectDir: string;
  /** The persona file's text, read at spawn time so edits to the versioned file take effect on the
   *  next spawn. Composed into one `--append-system-prompt`: the CLI rejects the by-path and by-text
   *  flags together, and the editor-session section has to ride along. */
  readonly personaText: string;
  /** The `claude --model` id chosen in the drawer's picker. */
  readonly modelId: string;
  /** Active view id and scene path, named in the appended editor-session section. */
  readonly view: string;
  readonly scenePath: string;
  /** Absolute path to the statusline relay script. */
  readonly relayScript: string;
  /** Repo root, for the `ludion-mcp` dev invocation's `--manifest-path`. */
  readonly repoRoot: string;
  /** Absolute path to this terminal's statusline sink file. */
  readonly sinkPath: string;
  readonly cols: number;
  readonly rows: number;
}

/** The `ludion-mcp` MCP-server config, injected inline via `--mcp-config`. Dev invocation:
 *  `cargo run -p ludion-mcp` pinned to the repo's workspace so it resolves regardless of the
 *  instance's cwd. Swap for an installed `ludion-mcp` binary once one is on PATH — the mcp server
 *  connects lazily, so the choice only affects first-call latency. */
export function mcpConfigJson(url: string, repoRoot: string): string {
  return JSON.stringify({
    mcpServers: {
      ludion: {
        command: 'cargo',
        args: ['run', '-q', '--manifest-path', `${repoRoot}/Cargo.toml`, '-p', 'ludion-mcp', '--', '--url', url],
      },
    },
  });
}

/** The per-spawn settings, injected inline via `--settings` (merged over the machine's own
 *  settings, so nothing here touches the human's config).
 *
 *  Statusline: Claude Code pipes its `context_window` payload to this command on every assistant
 *  message; the relay writes it to the sink and echoes a one-line status.
 *
 *  Permissions: the ludion surface is exactly what the drawer exists to run, so prompting on it
 *  asks a designer to adjudicate a tool call they have no way to judge. `mcp__ludion` is the
 *  server-level rule covering every protocol command (`ludion-mcp` names tools
 *  `mcp__ludion__<method with '.' → '_'>`); the Bash prefixes cover the CLI verbs the personas are
 *  told to use, in both the installed and in-repo spellings. Three calls stay on `ask` — the ones
 *  the human must see: `scene.load` can wipe unsaved human *and* agent edits, `engine.restore`
 *  replaces the world under the human's edits, and `engine.shutdown` kills the shared live
 *  session their viewport is attached to. `defaultMode`
 *  is deliberately unset: everything outside ludion still prompts. */
export function settingsJson(relayScript: string): string {
  return JSON.stringify({
    statusLine: {
      type: 'command',
      command: `node "${relayScript}"`,
      padding: 0,
    },
    permissions: {
      allow: ['mcp__ludion', 'Bash(ludion:*)', 'Bash(ludion *)', 'Bash(cargo run -p ludion-cli:*)'],
      ask: ['mcp__ludion__scene_load', 'mcp__ludion__engine_restore', 'mcp__ludion__engine_shutdown'],
    },
  });
}

/** The appended system prompt: the persona, then the spawn-time facts about this editor session.
 *  It rides in the system prompt rather than a first user turn so the instance opens on the
 *  designer's own words. Live state stays in the context file — this only says where to find it. */
export function systemPromptText(input: {
  personaText: string;
  view: string;
  projectDir: string;
  scenePath: string;
  url: string;
}): string {
  const scene = input.scenePath !== '' ? input.scenePath : '(none)';
  return `${input.personaText}

## Editor session (injected at spawn)

- View: ${input.view}
- Project dir (your cwd): ${input.projectDir}
- Scene: ${scene}
- Engine url: ${input.url} — your \`ludion-mcp\` tools already speak to that exact live session; the human's viewport is showing it.
- Scratch files: \`.ludion/scratch/\` **relative to your cwd** (gitignored, beside \`.ludion/sheets/\`). Not the system temp dir: it is outside the project tree, so a \`cd\` there snaps the shell back, and on Windows \`$TEMP\` expands to a backslash path Git Bash cannot resolve. A cwd-relative path works unchanged in both shells.

Live selection ("this/that") and placed pointer ("here") are not in this prompt — read \`.ludion/editor-context.json\` in your cwd (the context bridge above) before acting on any such reference.`;
}

export function buildSpawnConfig(input: SpawnConfigInput): PtySpawnOpts {
  const args = [
    '--model',
    input.modelId,
    '--append-system-prompt',
    systemPromptText(input),
    '--mcp-config',
    mcpConfigJson(input.url, input.repoRoot),
    '--settings',
    settingsJson(input.relayScript),
  ];
  return {
    id: input.termId,
    program: 'claude',
    args,
    cwd: input.projectDir,
    env: [['LUDION_STATUS_SINK', input.sinkPath]],
    cols: input.cols,
    rows: input.rows,
  };
}
