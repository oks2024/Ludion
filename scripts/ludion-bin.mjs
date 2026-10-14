// How the drift gates invoke the `ludion` CLI.
//
// `scripts/check.mjs` builds `ludion-cli` once and exports the binary's path as LUDION_BIN, so its
// drift gates spawn the binary directly: ~20 ms a call instead of a ~0.8 s `cargo run` that also
// takes cargo's build lock, which lets the gates run beside clippy and the test build. Run on its
// own, a gate has no LUDION_BIN and falls back to `cargo run`.
//
// Zero npm dependencies — Node built-ins only.

import { spawnSync } from 'node:child_process';

export function runLudion(args, options = {}) {
  const bin = process.env.LUDION_BIN;
  if (bin) return spawnSync(bin, args, { encoding: 'utf8', ...options });
  // No shell: the args carry repo paths and must reach ludion verbatim (`cargo` is a real
  // executable on every platform, so it resolves without one).
  return spawnSync('cargo', ['run', '-q', '-p', 'ludion-cli', '--', ...args], { encoding: 'utf8', ...options });
}
