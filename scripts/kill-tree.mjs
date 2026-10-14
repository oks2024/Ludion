// Spawning a tool and killing its whole process tree — shared by check.mjs and its test
// (ci/scripts/gate-scripts.test.mjs).
//
// On Windows every native tool (cargo, npx, npm, pwsh) goes through the shell so `.cmd`/`.bat`
// launcher shims and PATH resolve the way an interactive shell would; the command is passed as one
// string because Node warns (DEP0190) about an args array under `shell: true`; a token with a space
// (a prebuilt binary's path) is quoted. Elsewhere tools spawn directly, `detached` into their own
// process group so `killTree` can reach every descendant (nextest, the test binaries it runs).

import { spawn } from 'node:child_process';

const IS_WINDOWS = process.platform === 'win32';

export function spawnTool(cmd, args, options) {
  const quote = (t) => (/\s/.test(t) ? `"${t}"` : t);
  return IS_WINDOWS
    ? spawn([cmd, ...args].map(quote).join(' '), { ...options, shell: true })
    : spawn(cmd, args, { ...options, detached: true });
}

// Under `shell: true` a plain kill only reaches `cmd.exe`; `taskkill /T` takes the tree. Elsewhere
// the child leads its own group, and a negative pid signals the group.
export function killTree(child) {
  if (IS_WINDOWS) spawn('taskkill', ['/pid', String(child.pid), '/T', '/F'], { stdio: 'ignore' });
  else {
    try {
      process.kill(-child.pid, 'SIGKILL');
    } catch {
      child.kill('SIGKILL');
    }
  }
}
