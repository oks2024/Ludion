import { describe, expect, it } from 'vitest';
import {
  buildOpencodeSpawnConfig,
  DEFAULT_LLAMACPP,
  opencodeConfigJson,
  opencodePersonaPath,
  resolveLlamaSettings,
  validateLlamaSettings,
} from './opencode';
import { mcpConfigJson } from './spawnConfig';

const llama = {
  baseUrl: 'http://127.0.0.1:8080/v1',
  modelId: 'local-model',
  contextTokens: 131072,
};

const base = {
  termId: 'level-designer-abc',
  url: 'ws://127.0.0.1:9411',
  projectDir: 'D:/Repos/Ludion/games/vigil',
  repoRoot: 'D:/Repos/Ludion',
  personaPath: 'D:/Repos/Ludion/games/vigil/.ludion/opencode-persona-level-designer.md',
  llama,
  windows: false,
  cols: 80,
  rows: 24,
};

describe('opencodeConfigJson', () => {
  it('shapes the llama.cpp provider block, with an explicit context/output limit', () => {
    const parsed = JSON.parse(opencodeConfigJson(base));
    const provider = parsed.provider['llama.cpp'];
    expect(provider.npm).toBe('@ai-sdk/openai-compatible');
    expect(provider.options.baseURL).toBe(llama.baseUrl);
    expect(provider.models[llama.modelId].tool_call).toBe(true);
    expect(provider.models[llama.modelId].limit).toEqual({ context: 131072, output: 16384 });
  });

  it('caps output at 16384 for a large context and halves a small one', () => {
    const large = JSON.parse(opencodeConfigJson({ ...base, llama: { ...llama, contextTokens: 131072 } }));
    expect(large.provider['llama.cpp'].models[llama.modelId].limit.output).toBe(16384);

    const small = JSON.parse(opencodeConfigJson({ ...base, llama: { ...llama, contextTokens: 8192 } }));
    expect(small.provider['llama.cpp'].models[llama.modelId].limit).toEqual({ context: 8192, output: 4096 });
  });

  it('pins both model and small_model to the local provider/model id', () => {
    const parsed = JSON.parse(opencodeConfigJson(base));
    expect(parsed.model).toBe(`llama.cpp/${llama.modelId}`);
    expect(parsed.small_model).toBe(`llama.cpp/${llama.modelId}`);
  });

  it('keeps mcp.ludion.command in lockstep with the claude spawn path', () => {
    // Drift guard: both spawn paths run the same `cargo run -p ludion-mcp` dev invocation,
    // spelled differently (argv array here vs {command, args} for claude's --mcp-config).
    const claudeMcp = JSON.parse(mcpConfigJson(base.url, base.repoRoot)).mcpServers.ludion;
    const expected = [claudeMcp.command, ...claudeMcp.args];
    const parsed = JSON.parse(opencodeConfigJson(base));
    expect(parsed.mcp.ludion.command).toEqual(expected);
  });

  it('sets a generous mcp timeout and carries the persona path as the sole instruction', () => {
    const parsed = JSON.parse(opencodeConfigJson(base));
    expect(parsed.mcp.ludion.timeout).toBe(120000);
    expect(parsed.instructions).toEqual([base.personaPath]);
  });

  it('asks by default and only pre-approves the ludion surface, scene.load excepted', () => {
    const parsed = JSON.parse(opencodeConfigJson(base));
    expect(parsed.permission.edit).toBe('ask');
    expect(parsed.permission.webfetch).toBe('ask');
    expect(parsed.permission.bash).toEqual({
      '*': 'ask',
      'ludion *': 'allow',
      'cargo run -p ludion-cli *': 'allow',
    });
    expect(parsed.permission['ludion_*']).toBe('allow');
    expect(parsed.permission.ludion_scene_load).toBe('ask');
    expect(parsed.permission.ludion_engine_restore).toBe('ask');
    expect(parsed.permission.ludion_engine_shutdown).toBe('ask');

    // Last-match-wins (opencode 1.18.10 packages/opencode/src/permission/index.ts): the specific
    // ask keys must sit after the ludion_* wildcard they override.
    const keys = Object.keys(parsed.permission);
    expect(keys.indexOf('ludion_scene_load')).toBeGreaterThan(keys.indexOf('ludion_*'));
    expect(keys.indexOf('ludion_engine_restore')).toBeGreaterThan(keys.indexOf('ludion_*'));
    expect(keys.indexOf('ludion_engine_shutdown')).toBeGreaterThan(keys.indexOf('ludion_*'));
  });
});

describe('buildOpencodeSpawnConfig', () => {
  it('spawns opencode with no argv and the config riding in env', () => {
    const opts = buildOpencodeSpawnConfig(base);
    expect(opts.id).toBe('level-designer-abc');
    expect(opts.program).toBe('opencode');
    expect(opts.args).toEqual([]);
    expect(opts.cwd).toBe(base.projectDir);
    expect(opts.cols).toBe(80);
    expect(opts.rows).toBe(24);

    const env = Object.fromEntries(opts.env);
    expect(() => JSON.parse(env.OPENCODE_CONFIG_CONTENT)).not.toThrow();
    expect(env.OPENCODE_DISABLE_AUTOUPDATE).toBe('1');
    expect(env.LUDION_STATUS_SINK).toBeUndefined();
  });

  it('routes through cmd.exe on Windows, where the npm shim is a batch file', () => {
    const opts = buildOpencodeSpawnConfig({ ...base, windows: true });
    expect(opts.program).toBe('cmd.exe');
    expect(opts.args).toEqual(['/c', 'opencode']);
  });
});

describe('opencodePersonaPath', () => {
  it('places the persona file under the project .ludion dir, keyed by view id', () => {
    expect(opencodePersonaPath('/proj', 'level-designer')).toBe('/proj/.ludion/opencode-persona-level-designer.md');
  });
});

describe('resolveLlamaSettings', () => {
  it('falls back to defaults when everything is null', () => {
    expect(resolveLlamaSettings({ baseUrl: null, modelId: null, contextTokens: null })).toEqual(DEFAULT_LLAMACPP);
  });

  it('keeps stored values when present', () => {
    const resolved = resolveLlamaSettings({
      baseUrl: 'http://203.0.113.5:8080/v1',
      modelId: 'qwen3-30b',
      contextTokens: '32768',
    });
    expect(resolved).toEqual({ baseUrl: 'http://203.0.113.5:8080/v1', modelId: 'qwen3-30b', contextTokens: 32768 });
  });

  it('falls back to the default context on a non-integer or non-positive value', () => {
    for (const bad of ['abc', '-5', '1.5']) {
      expect(resolveLlamaSettings({ baseUrl: null, modelId: null, contextTokens: bad }).contextTokens).toBe(
        DEFAULT_LLAMACPP.contextTokens,
      );
    }
  });
});

describe('validateLlamaSettings', () => {
  it('accepts the defaults', () => {
    expect(validateLlamaSettings(DEFAULT_LLAMACPP)).toBeNull();
  });

  it('rejects an unparseable or non-http(s) base URL', () => {
    expect(validateLlamaSettings({ ...DEFAULT_LLAMACPP, baseUrl: 'not a url' })).not.toBeNull();
    expect(validateLlamaSettings({ ...DEFAULT_LLAMACPP, baseUrl: 'ftp://x' })).not.toBeNull();
  });

  it('rejects an empty or whitespace-only model id', () => {
    expect(validateLlamaSettings({ ...DEFAULT_LLAMACPP, modelId: '' })).not.toBeNull();
    expect(validateLlamaSettings({ ...DEFAULT_LLAMACPP, modelId: '   ' })).not.toBeNull();
  });

  it('rejects a model id matching a reserved provider substring, case-insensitively', () => {
    expect(validateLlamaSettings({ ...DEFAULT_LLAMACPP, modelId: 'gpt-oss-20b' })).not.toBeNull();
    expect(validateLlamaSettings({ ...DEFAULT_LLAMACPP, modelId: 'claude-x' })).not.toBeNull();
    expect(validateLlamaSettings({ ...DEFAULT_LLAMACPP, modelId: 'MyGemini' })).not.toBeNull();
  });

  it('rejects a zero, negative, or non-integer context size', () => {
    expect(validateLlamaSettings({ ...DEFAULT_LLAMACPP, contextTokens: 0 })).not.toBeNull();
    expect(validateLlamaSettings({ ...DEFAULT_LLAMACPP, contextTokens: -1 })).not.toBeNull();
    expect(validateLlamaSettings({ ...DEFAULT_LLAMACPP, contextTokens: 1.5 })).not.toBeNull();
  });
});
