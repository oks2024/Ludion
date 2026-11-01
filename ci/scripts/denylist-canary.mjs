// Denylist canary — the one copy, run by scripts/check.mjs (--full), the `denylist-canary` job in
// .github/workflows/ci.yml and the private TeamCity configs. VCS-agnostic: it never calls git or
// lore, and it never writes a tracked file.
//
// Proves the determinism denylists in the generated clippy.toml files actually fail a build. Each
// probe is a scratch crate under target/denylist-canary/ carrying a COPY of a real crate's
// clippy.toml (clippy finds it beside the manifest, the same discovery path as linting the real
// crate) and one banned construct; clippy must reject it, and the output must name the banned path
// AND the reason from the denylist — a failure alone is not proof the rule fired. Two scratch crates, four probes: the
// strict tier's type ban (HashMap) and libm ban (f32::sin) in ludion-core, and the sim tier's
// thread-spawn ban and glam Quat/Vec4 bans in ludion-ecs. The scratch crates have their own
// CARGO_TARGET_DIR (and glam, the only dependency, via a probe's `deps`), so the run holds no lock
// on the workspace's target/ and builds in seconds.
//
// Run from the repository root. Exits 0 iff every probe was rejected for the right reason.

import { copyFileSync, existsSync, mkdirSync, rmSync, writeFileSync } from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { capture } from './gate-lib.mjs';

export const PROBES = [
  {
    name: 'strict',
    crate: 'crates/ludion-core',
    source: [
      'pub fn _denylist_canary() { let _m: std::collections::HashMap<u8, u8> = std::collections::HashMap::new(); }',
      'pub fn _libm_canary(x: f32) -> f32 { x.sin() }',
      '',
    ].join('\n'),
    needles: [
      '`std::collections::HashMap`',
      'non-deterministic iteration order',
      '`f32::sin`',
      'platform libm differs across OSes',
    ],
  },
  {
    name: 'sim',
    crate: 'crates/ludion-ecs',
    source: 'pub fn _thread_canary() { let _ = std::thread::spawn(|| {}); }\n',
    needles: ['`std::thread::spawn`', 'sim parallelism only via ludion_core::par'],
  },
  {
    name: 'sim-glam',
    crate: 'crates/ludion-ecs',
    deps: ['glam = { version = "=0.30.10", features = ["libm"] }'],
    source: [
      'use glam::{Mat4, Quat, Vec3A, Vec4};',
      'pub fn _glam_canary(q: Quat, r: Quat, v: Vec4, w: Vec4, a: Vec3A, m: Mat4) {',
      '    let _ = (q.slerp(r, 0.5), q.lerp(r, 0.5), q.normalize(), q.length(), q.length_squared());',
      '    let _ = (q.length_recip(), q.dot(r), q.angle_between(r), q.rotate_towards(r, 0.5));',
      '    let _ = (v.dot(w), v.length(), v.length_squared(), v.length_recip(), v.normalize());',
      '    let _ = (v.normalize_or_zero(), v.try_normalize(), v.distance(w), v.distance_squared(w));',
      '    let _ = (Quat::from_rotation_arc(glam::Vec3::X, glam::Vec3::Y), Quat::from_rotation_arc_colinear(glam::Vec3::X, glam::Vec3::Y), q.is_normalized());',
      '    let _ = (v.normalize_and_length(), v.is_normalized(), v.project_onto(w), v.project_onto_normalized(w), v.reject_from(w));',
      '    let _ = (v.reject_from_normalized(w), v.clamp_length(0.0, 1.0), v.clamp_length_max(1.0), v.clamp_length_min(0.0));',
      '    let _ = (v.move_towards(w, 0.5), v.reflect(w), v.refract(w, 0.5));',
      '    let _ = (a.normalize(), m.inverse(), m.determinant());',
      '}',
      '',
    ].join('\n'),
    needles: [
      ...['slerp', 'lerp', 'normalize', 'length', 'length_squared', 'length_recip', 'dot', 'angle_between', 'rotate_towards', 'from_rotation_arc', 'from_rotation_arc_colinear', 'is_normalized']
        .map((n) => `\`glam::Quat::${n}\``),
      ...['dot', 'length', 'length_squared', 'length_recip', 'normalize', 'normalize_or_zero', 'try_normalize', 'distance', 'distance_squared', 'normalize_and_length', 'is_normalized', 'project_onto', 'project_onto_normalized', 'reject_from', 'reject_from_normalized', 'clamp_length', 'clamp_length_max', 'clamp_length_min', 'move_towards', 'reflect', 'refract']
        .map((n) => `\`glam::Vec4::${n}\``),
      '`glam::Vec3A::normalize`',
      '`glam::Mat4::inverse`',
      '`glam::Mat4::determinant`',
      'round this differently',
    ],
  },
];

export const missingNeedles = (probe, output) => probe.needles.filter((n) => !output.includes(n));

// Writes `<root>/<name>/{Cargo.toml,src/lib.rs,clippy.toml}`; returns the crate directory.
export function writeProbeCrate(root, probe) {
  const dir = path.join(root, probe.name);
  mkdirSync(path.join(dir, 'src'), { recursive: true });
  // The empty [workspace] detaches the crate from the root workspace it sits inside.
  writeFileSync(path.join(dir, 'Cargo.toml'), [
    '[package]',
    `name = "denylist-canary-${probe.name}"`,
    'version = "0.0.0"',
    'edition = "2024"',
    'publish = false',
    '',
    '[workspace]',
    '',
    ...(probe.deps ? ['[dependencies]', ...probe.deps, ''] : []),
  ].join('\n'));
  writeFileSync(path.join(dir, 'src', 'lib.rs'), probe.source);
  copyFileSync(path.join(probe.crate, 'clippy.toml'), path.join(dir, 'clippy.toml'));
  return dir;
}

async function main() {
  const root = path.join('target', 'denylist-canary');
  for (const probe of PROBES) {
    if (!existsSync(path.join(probe.crate, 'clippy.toml'))) {
      console.error(`not at the repository root: ${probe.crate}/clippy.toml not found`);
      return 1;
    }
  }
  for (const probe of PROBES) {
    const dir = writeProbeCrate(root, probe);
    const { code, output } = await capture(
      'cargo',
      ['clippy', '--manifest-path', path.join(dir, 'Cargo.toml'), '--', '-D', 'warnings'],
      { env: { CARGO_TARGET_DIR: path.join(root, 'target') } },
    );
    console.log(output);
    if (code === 0) {
      console.log(`DENYLIST FAILED TO FIRE (${probe.name})`);
      return 1;
    }
    const missing = missingNeedles(probe, output);
    if (missing.length) {
      console.log(`DENYLIST CANARY (${probe.name}): clippy failed but never reported ${missing.join(', ')}`);
      return 1;
    }
  }
  console.log('denylist ok');
  return 0;
}

if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  process.exit(await main());
}
