// Runs this engine's `ludion` CLI: `node scripts/ludion.mjs <verb> [args]`.
// It builds into this project's own `target/ludion-cli`, never the engine checkout's `target/`, and
// always builds the engine that the game's path dependencies name — `.ludion/engine`, a gitignored
// per-machine link (a junction on Windows, a symlink elsewhere) to this machine's engine checkout.
// `node scripts/ludion.mjs link-engine <engine checkout>` (re)creates that link; run it once on a
// fresh clone. `ludion new` has already run it on the scaffolding machine.
import { spawnSync } from 'node:child_process';
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const link = path.join(root, '.ludion', 'engine');
const engineRoot = link;

function fail(code, what, help) {
  console.error(`error[${code}]: ${what}\n  help: ${help}`);
  process.exit(1);
}

if (process.argv[2] === 'link-engine') {
  const args = process.argv.slice(3);
  if (args.length !== 1) {
    fail(
      '{{code_link_target}}',
      'link-engine takes exactly one argument, the engine checkout',
      'node scripts/ludion.mjs link-engine <engine checkout>',
    );
  }
  let target = path.resolve(args[0]);
  const isFile = (p) => fs.existsSync(p) && fs.statSync(p).isFile();
  if (
    !isFile(path.join(target, 'Cargo.toml')) ||
    !isFile(path.join(target, 'crates', 'ludion', 'Cargo.toml'))
  ) {
    fail(
      '{{code_link_target}}',
      `\`${target}\` is not an engine checkout (no Cargo.toml and crates/ludion/Cargo.toml)`,
      'pass the root of the engine checkout',
    );
  }
  target = fs.realpathSync.native(target);
  let existing = null;
  try {
    existing = fs.lstatSync(link);
  } catch {}
  if (existing) {
    if (!existing.isSymbolicLink()) {
      fail(
        '{{code_link_occupied}}',
        '`.ludion/engine` exists and is not a link; it is never deleted',
        'move or delete it, then rerun link-engine',
      );
    }
    fs.unlinkSync(link);
  }
  fs.mkdirSync(path.dirname(link), { recursive: true });
  fs.symlinkSync(target, link, 'junction');
  console.log(`linked .ludion/engine -> ${target}`);
  process.exit(0);
}

if (!fs.existsSync(path.join(engineRoot, 'crates', 'ludion', 'Cargo.toml'))) {
  fail(
    '{{code_link_missing}}',
    '`.ludion/engine` is missing, dangling, or not an engine checkout',
    'node scripts/ludion.mjs link-engine <engine checkout>',
  );
}
const targetDir = path.join(root, 'target', 'ludion-cli');
if (!fs.existsSync(targetDir)) {
  console.error(
    'ludion.mjs: building the engine CLI into target/ludion-cli (the first build takes minutes)…',
  );
}
const result = spawnSync(
  'cargo',
  [
    'run',
    '-q',
    '--manifest-path',
    path.join(engineRoot, 'Cargo.toml'),
    '-p',
    'ludion-cli',
    '--target-dir',
    targetDir,
    '--',
    ...process.argv.slice(2),
  ],
  { stdio: 'inherit' },
);
if (result.error) {
  console.error(`could not run cargo: ${result.error.message}`);
  process.exit(1);
}
process.exit(result.status ?? 1);
