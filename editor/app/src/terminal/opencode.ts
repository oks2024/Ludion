// Assembles the `pty_spawn` options for an opencode instance talking to a local llama.cpp
// server — pure, so the exact spawn is unit-tested. opencode reads its whole config from
// `OPENCODE_CONFIG_CONTENT` (model, provider, mcp, permissions); the caller composes the persona
// text (via `systemPromptText`, shared with the claude spawn) and writes it to
// `opencodePersonaPath`, this module only deals in paths/config.

import type { PtySpawnOpts } from './pty';

export interface LlamaCppSettings {
  readonly baseUrl: string;
  readonly modelId: string;
  readonly contextTokens: number;
}

export const DEFAULT_LLAMACPP: LlamaCppSettings = {
  baseUrl: 'http://127.0.0.1:8080/v1',
  modelId: 'local-model',
  contextTokens: 65536,
};

/** Fold stored (localStorage) strings into settings; null/invalid fields fall to defaults per-field. */
export function resolveLlamaSettings(stored: {
  baseUrl: string | null;
  modelId: string | null;
  contextTokens: string | null;
}): LlamaCppSettings {
  const baseUrl =
    stored.baseUrl !== null && stored.baseUrl.trim() !== '' ? stored.baseUrl : DEFAULT_LLAMACPP.baseUrl;
  const modelId =
    stored.modelId !== null && stored.modelId.trim() !== '' ? stored.modelId : DEFAULT_LLAMACPP.modelId;
  const parsedContext = stored.contextTokens !== null ? Number(stored.contextTokens) : NaN;
  const contextTokens =
    Number.isInteger(parsedContext) && parsedContext > 0 ? parsedContext : DEFAULT_LLAMACPP.contextTokens;
  return { baseUrl, modelId, contextTokens };
}

/** Model ids opencode's base-system-prompt selector matches on (opencode source:
 *  packages/opencode/src/session/system.ts) — a local model whose id contains one of these would
 *  silently get that provider's tuned base prompt instead of the default one. */
const RESERVED_MODEL_ID_SUBSTRINGS = ['gpt', 'claude', 'gemini', 'codex', 'kimi', 'trinity', 'o1', 'o3'];

/** Human-readable rejection, or null when valid. */
export function validateLlamaSettings(s: LlamaCppSettings): string | null {
  try {
    const url = new URL(s.baseUrl);
    if (url.protocol !== 'http:' && url.protocol !== 'https:') {
      return `Base URL must be http or https, got "${url.protocol}"`;
    }
  } catch {
    return `Base URL "${s.baseUrl}" is not a valid URL`;
  }

  if (s.modelId.trim() === '') {
    return 'Model id must not be empty';
  }

  const lower = s.modelId.toLowerCase();
  const hit = RESERVED_MODEL_ID_SUBSTRINGS.find((needle) => lower.includes(needle));
  if (hit !== undefined) {
    return `Model id "${s.modelId}" contains "${hit}" — opencode picks its base system prompt by model-id substring match, so this would silently swap in another provider's prompt`;
  }

  if (!Number.isInteger(s.contextTokens) || s.contextTokens <= 0) {
    return `Context tokens must be a positive integer, got ${s.contextTokens}`;
  }

  return null;
}

export function opencodePersonaPath(projectDir: string, viewId: string): string {
  return `${projectDir}/.ludion/opencode-persona-${viewId}.md`;
}

/** The opencode config, injected inline via `OPENCODE_CONFIG_CONTENT`.
 *
 *  `small_model` is pinned to the same local model: unset, opencode routes title/summary/
 *  compaction generation to some other configured provider. `limit` is explicit because a
 *  config-defined provider has no models.dev catalog entry, so without it opencode's context
 *  accounting reads 0. The mcp `timeout` is generous because `ludion-mcp` runs via `cargo run` —
 *  the first call may need to compile.
 *
 *  `mcp.ludion.command` must stay in lockstep with `mcpConfigJson` in spawnConfig.ts (the same
 *  dev invocation, reassembled into an argv array instead of a `{command, args}` pair) —
 *  opencode.test.ts enforces this by reconstructing one from the other.
 *
 *  Permission block: opencode's default agent allows everything (`"*": "allow"`) and user config
 *  merges on top of that, so ask-by-default has to be spelled out here. opencode names MCP tools
 *  `<server>_<tool>` (ludion-mcp emits dot-free names, so `scene.load` → `ludion_scene_load`);
 *  `"ludion *"` also matches bare `ludion` (opencode's wildcard adds `( .*)?`). Key order is
 *  load-bearing — last matching key wins — so the three ask-overrides sit after `ludion_*`:
 *  `ludion_scene_load` can wipe unsaved human and agent edits, `ludion_engine_restore` replaces
 *  the world under the human's edits, and `ludion_engine_shutdown` kills the shared live session
 *  the human's viewport is attached to. */
export function opencodeConfigJson(input: {
  url: string;
  repoRoot: string;
  personaPath: string;
  llama: LlamaCppSettings;
}): string {
  const { url, repoRoot, personaPath, llama } = input;
  const model = `llama.cpp/${llama.modelId}`;
  const output = Math.min(16384, Math.floor(llama.contextTokens / 2));
  return JSON.stringify({
    $schema: 'https://opencode.ai/config.json',
    model,
    small_model: model,
    provider: {
      'llama.cpp': {
        npm: '@ai-sdk/openai-compatible',
        name: 'llama-server (local)',
        options: { baseURL: llama.baseUrl },
        models: {
          [llama.modelId]: {
            name: `${llama.modelId} (local)`,
            tool_call: true,
            limit: { context: llama.contextTokens, output },
          },
        },
      },
    },
    mcp: {
      ludion: {
        type: 'local',
        command: [
          'cargo',
          'run',
          '-q',
          '--manifest-path',
          `${repoRoot}/Cargo.toml`,
          '-p',
          'ludion-mcp',
          '--',
          '--url',
          url,
        ],
        enabled: true,
        timeout: 120000,
      },
    },
    instructions: [personaPath],
    permission: {
      edit: 'ask',
      webfetch: 'ask',
      bash: { '*': 'ask', 'ludion *': 'allow', 'cargo run -p ludion-cli *': 'allow' },
      'ludion_*': 'allow',
      ludion_scene_load: 'ask',
      ludion_engine_restore: 'ask',
      ludion_engine_shutdown: 'ask',
    },
  });
}

export interface OpencodeSpawnInput {
  readonly termId: string;
  readonly url: string;
  readonly projectDir: string;
  readonly repoRoot: string;
  readonly personaPath: string;
  readonly llama: LlamaCppSettings;
  /** npm installs opencode as shims only (`opencode` sh + `opencode.cmd`, no .exe), and Windows'
   *  CreateProcessW can neither exec a shell script (error 193) nor a batch file directly — batch
   *  files must go through `cmd.exe /c`, which then resolves `opencode.cmd` via PATHEXT itself. */
  readonly windows: boolean;
  readonly cols: number;
  readonly rows: number;
}

/** Model/mcp/permissions all ride in `OPENCODE_CONFIG_CONTENT` — opencode's top-priority config
 *  tier, so nothing is written to the human's own opencode config on disk. opencode itself gets no
 *  flags or positional prompt as a result, which leaves the first user turn to the designer, and
 *  keeps the spawn clear of Windows's command-line length limit.
 *  `OPENCODE_DISABLE_MODELS_FETCH` is deliberately not set:
 *  a config-defined provider doesn't need the models.dev catalog, and the fetch degrades
 *  gracefully offline. */
export function buildOpencodeSpawnConfig(input: OpencodeSpawnInput): PtySpawnOpts {
  return {
    id: input.termId,
    program: input.windows ? 'cmd.exe' : 'opencode',
    args: input.windows ? ['/c', 'opencode'] : [],
    cwd: input.projectDir,
    env: [
      ['OPENCODE_CONFIG_CONTENT', opencodeConfigJson(input)],
      ['OPENCODE_DISABLE_AUTOUPDATE', '1'],
    ],
    cols: input.cols,
    rows: input.rows,
  };
}
