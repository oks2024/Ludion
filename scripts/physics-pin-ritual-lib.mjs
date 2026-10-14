// Pure helpers of scripts/physics-pin-ritual.mjs, split out so `physics-pin-ritual.test.mjs` can call them.

import fs from 'node:fs';
import path from 'node:path';

// A nextest status line for a test that did not pass, e.g.
//   `        FAIL [   0.053s] ( 2/12) ludion-physics::pile the_pile_matches_the_committed_trace`
// The `( n/m)` counter is printed by some nextest versions and absent in others.
const FAIL_LINE =
  /^\s*(?:FAIL|SIGSEGV|SIGABRT|SIGBUS|SIGILL|SIGFPE|SIGKILL|SIGTERM|TIMEOUT|LEAK-FAIL|EXEC-FAIL|ABORT)\s+\[[^\]]*\]\s+(?:\(\s*\d+\/\d+\)\s+)?(\S+)\s+(\S+)\s*$/gm;

// The failing tests in a nextest log as sorted `binary::test` keys.
export function parseFailedTests(out) {
  const failed = new Set();
  for (const m of out.matchAll(FAIL_LINE)) {
    const bin = m[1].split('::').pop();
    failed.add(`${bin}::${m[2]}`);
  }
  return [...failed].sort();
}

const SKIP_DIRS = new Set(['target', '.git', 'node_modules', '.build']);
const SKIP_FILES = new Set(['Cargo.lock', 'Cargo.toml', path.join('crates', 'ludion-physics', 'Cargo.toml')]);

// `rel:line: text` for every line naming `version` in a text file under `root` that mentions rapier.
export function scanLeftovers(root, version) {
  const todo = [];
  const walk = (dir) => {
    for (const ent of fs.readdirSync(dir, { withFileTypes: true }).sort((a, b) => (a.name < b.name ? -1 : 1))) {
      const full = path.join(dir, ent.name);
      if (ent.isDirectory()) {
        if (!SKIP_DIRS.has(ent.name)) walk(full);
        continue;
      }
      if (!ent.isFile()) continue;
      const rel = path.relative(root, full);
      if (SKIP_FILES.has(rel)) continue;
      let buf;
      try {
        buf = fs.readFileSync(full);
      } catch {
        continue;
      }
      if (buf.includes(0)) continue;
      let text;
      try {
        text = new TextDecoder('utf-8', { fatal: true }).decode(buf);
      } catch {
        continue;
      }
      if (!/rapier/i.test(text) || !text.includes(version)) continue;
      text.split('\n').forEach((line, i) => {
        if (line.includes(version)) todo.push(`  ${rel.split(path.sep).join('/')}:${i + 1}: ${line.trim().slice(0, 100)}`);
      });
    }
  };
  walk(root);
  return todo;
}
