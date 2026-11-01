// Runs this engine's `ludion` CLI: `node scripts/ludion.mjs <verb> [args]`.
// It builds into this project's own `target/ludion-cli`, never the engine checkout's `target/`, and
// always builds the engine that the game's path dependency names, so the CLI matches the game.
import { spawnSync } from 'node:child_process';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const result = spawnSync(
  'cargo',
  [
    'run',
    '-q',
    '--manifest-path',
    '{{engine_root}}/Cargo.toml',
    '-p',
    'ludion-cli',
    '--target-dir',
    path.join(root, 'target', 'ludion-cli'),
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
