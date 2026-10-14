import { describe, expect, it } from 'vitest';
import { buildSpawnConfig, mcpConfigJson, settingsJson, systemPromptText } from './spawnConfig';

const base = {
  termId: 'level-designer-abc',
  url: 'ws://127.0.0.1:9411',
  projectDir: 'D:/Repos/Ludion/games/vigil',
  personaText: '# Persona: Level Designer\n\nYou are a level-design domain expert.',
  modelId: 'opus',
  view: 'level-designer',
  scenePath: 'district-plaza.yaml',
  relayScript: 'D:/Repos/Ludion/editor/terminal/statusline-relay.mjs',
  repoRoot: 'D:/Repos/Ludion',
  sinkPath: 'D:/Repos/Ludion/games/vigil/.ludion/claude-status-level-designer-abc.json',
  cols: 80,
  rows: 24,
};

describe('mcpConfigJson', () => {
  it('injects a ludion-mcp server dialing the given url, pinned to the repo workspace', () => {
    const cfg = JSON.parse(mcpConfigJson('ws://host:1', 'D:/repo'));
    expect(cfg.mcpServers.ludion.command).toBe('cargo');
    expect(cfg.mcpServers.ludion.args).toEqual([
      'run',
      '-q',
      '--manifest-path',
      'D:/repo/Cargo.toml',
      '-p',
      'ludion-mcp',
      '--',
      '--url',
      'ws://host:1',
    ]);
  });
});

describe('settingsJson', () => {
  it('wires the relay as a command statusline', () => {
    const s = JSON.parse(settingsJson('C:/a b/relay.mjs'));
    expect(s.statusLine.type).toBe('command');
    expect(s.statusLine.command).toBe('node "C:/a b/relay.mjs"');
  });

  it('pre-approves the whole ludion surface — mcp server plus the CLI spellings', () => {
    const allow = JSON.parse(settingsJson('r.mjs')).permissions.allow;
    expect(allow).toContain('mcp__ludion');
    expect(allow).toContain('Bash(ludion:*)');
    expect(allow).toContain('Bash(ludion *)');
    expect(allow).toContain('Bash(cargo run -p ludion-cli:*)');
  });

  it('keeps scene.load, engine.restore and engine.shutdown on ask — the calls the human must see', () => {
    const permissions = JSON.parse(settingsJson('r.mjs')).permissions;
    expect(permissions.ask).toEqual([
      'mcp__ludion__scene_load',
      'mcp__ludion__engine_restore',
      'mcp__ludion__engine_shutdown',
    ]);
    // Unset on purpose: anything outside ludion still prompts.
    expect(permissions.defaultMode).toBeUndefined();
  });
});

describe('systemPromptText', () => {
  it('keeps the persona verbatim and appends the session section', () => {
    const t = systemPromptText(base);
    expect(t.startsWith(base.personaText)).toBe(true);
    expect(t).toContain('## Editor session (injected at spawn)');
    expect(t).toContain('level-designer');
    expect(t).toContain('D:/Repos/Ludion/games/vigil');
    expect(t).toContain('district-plaza.yaml');
    expect(t).toContain('ws://127.0.0.1:9411');
    expect(t).toContain('.ludion/editor-context.json');
    expect(t).toContain('pointer');
  });

  // The system temp dir sits outside the project tree, so a `cd` there is snapped back by the
  // harness and `$TEMP` is an unresolvable backslash path in Git Bash. Point scratch work in-tree.
  it('names the in-project scratch dir', () => {
    expect(systemPromptText(base)).toContain('.ludion/scratch/');
  });

  it('reports (none) when no scene is loaded', () => {
    expect(systemPromptText({ ...base, scenePath: '' })).toContain('Scene: (none)');
  });
});

describe('buildSpawnConfig', () => {
  it('assembles the model/system-prompt/mcp/settings flags, cwd, and env in order', () => {
    const opts = buildSpawnConfig(base);
    expect(opts.id).toBe('level-designer-abc');
    expect(opts.program).toBe('claude');
    expect(opts.cwd).toBe(base.projectDir);
    expect(opts.env).toEqual([['LUDION_STATUS_SINK', base.sinkPath]]);

    const args = opts.args;
    expect(args[0]).toBe('--model');
    expect(args[1]).toBe(base.modelId);
    expect(args[2]).toBe('--append-system-prompt');
    expect(args[3]).toBe(systemPromptText(base));
    expect(args[4]).toBe('--mcp-config');
    expect(JSON.parse(args[5]).mcpServers.ludion.args).toContain('ludion-mcp');
    expect(args[6]).toBe('--settings');
    const settings = JSON.parse(args[7]);
    expect(settings.statusLine.command).toContain('statusline-relay.mjs');
    expect(settings.permissions.allow).toContain('mcp__ludion');
  });

  it('passes the persona by text only and carries no positional prompt', () => {
    const args = buildSpawnConfig(base).args;
    // The CLI rejects `--append-system-prompt` and `--append-system-prompt-file` together.
    expect(args).not.toContain('--append-system-prompt-file');
    expect(args).toHaveLength(8);
  });
});
