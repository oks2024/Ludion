// Shader-bindgen drift gate — the one copy, run by scripts/check.mjs's `shader-drift` step, the
// `check` job in .github/workflows/ci.yml and the private TeamCity configs. VCS-agnostic: it never
// calls git or lore.
//
// Compare-only (decision 12 of Plan/shader-bindgen): `--check` reflects the WGSL and compares it
// against the committed shader_gen.rs, exiting nonzero on a mismatch without ever writing the file.
//
// Run from the repository root. Requires cargo (rust-toolchain.toml pins the channel).

import { existsSync } from 'node:fs';
import { capture } from './gate-lib.mjs';

const generated = 'crates/ludion-renderer/src/shader_gen.rs';
if (!existsSync(generated)) {
  console.error(`not at the repository root: ${generated} not found`);
  process.exit(1);
}

// The bindgen exits 1 for a load error, a reflection error and a stale file alike, and cargo exits
// 101 on a compile failure; only the stale case prints `is STALE`.
const { code, output } = await capture('cargo', ['run', '-q', '-p', 'ludion-shader-bindgen', '--', '--check']);
process.stdout.write(output);
if (code !== 0) {
  if (output.includes('is STALE')) {
    console.log(`SHADER DRIFT: ${generated} no longer matches the WGSL.`);
    console.log('Regenerate it: node scripts/check.mjs --fix (or cargo run -p ludion-shader-bindgen), then commit.');
  } else {
    console.log('shader-drift: ludion-shader-bindgen failed (build or reflection error above) — not a drift');
  }
  process.exit(1);
}
console.log('shader drift: none');
