// Runs this engine's `ludion` CLI: `node scripts/ludion.mjs <verb> [args]`.
// It builds into this project's own `target/ludion-cli`, never the engine checkout's `target/`, and
// always builds the engine that the game's path dependency names, so the CLI matches the game.
import { spawnSync } from 'node:child_process';
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const engineRoot = {{engine_root}};
const targetDir =path.join(root, 'target', 'ludion-cli');
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
    `${engineRoot}/Cargo.toml`,
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
