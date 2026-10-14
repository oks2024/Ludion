// Shared by the Node gate scripts in ci/scripts/ (shader-drift, schema-drift, denylist-canary).
// Node built-ins only. On Windows every native tool (cargo, npm) goes through the shell so `.cmd`
// shims and PATH resolve as they would interactively; the command is one string because Node warns
// (DEP0190) about an args array under `shell: true`. Elsewhere tools spawn directly. Mirrors
// `spawnTool` in scripts/check.mjs.

import { spawn } from 'node:child_process';

const quote = (t) => (/\s/.test(t) ? `"${t}"` : t);

function spawnTool(cmd, args, options) {
  return process.platform === 'win32'
    ? spawn([cmd, ...args].map(quote).join(' '), { ...options, shell: true })
    : spawn(cmd, args, options);
}

// Runs a tool with stdio inherited; resolves to its exit code (1 when it cannot be spawned).
export function run(cmd, args, { cwd, env } = {}) {
  return new Promise((resolve) => {
    const child = spawnTool(cmd, args, { cwd, env: { ...process.env, ...env }, stdio: 'inherit' });
    child.on('error', () => resolve(1));
    child.on('close', (code) => resolve(code ?? 1));
  });
}

// Runs a tool with stdout and stderr merged into one string; resolves to `{ code, output }`.
export function capture(cmd, args, { cwd, env } = {}) {
  return new Promise((resolve) => {
    const child = spawnTool(cmd, args, { cwd, env: { ...process.env, ...env } });
    const chunks = [];
    child.stdout.on('data', (d) => chunks.push(d));
    child.stderr.on('data', (d) => chunks.push(d));
    child.on('error', (err) => chunks.push(Buffer.from(`[spawn error] ${err.message}\n`)));
    child.on('close', (code) => resolve({ code: code ?? 1, output: Buffer.concat(chunks).toString('utf8') }));
  });
}
