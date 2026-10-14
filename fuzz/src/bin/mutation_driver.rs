//! Deterministic random-mutation fuzz driver — the fallback path when libFuzzer's sanitizer
//! runtime is unavailable (Plan/phase-9-hardening.md decision 3: "fall back to a tiny fuzz/ crate
//! with a deterministic random-mutation driver so the targets at least execute").
//!
//! On this Windows host the coverage-guided `cargo fuzz run` path cannot start: rustc's bundled
//! LLVM (22.x) needs a matching `clang_rt.asan_dynamic` runtime, only an older MSVC one is
//! installed (ABI mismatch → `STATUS_DLL_INIT_FAILED`), and `--sanitizer none` fails to link on
//! COFF (SanitizerCoverage section symbols need a runtime). This driver runs the SAME harness
//! functions (`ludion_fuzz::*`) so the targets execute and findings surface now; the real
//! coverage-guided run is deferred to a Linux/macOS CI agent where the runtime matches.
//!
//! It is NOT a replacement for coverage-guided fuzzing — it has no coverage feedback, just seeded
//! byte mutation of the committed corpus. It is a smoke/execution proof, deterministic (fixed
//! seed) so a finding reproduces.
//!
//! Usage: `cargo build --bin mutation_driver --release` then run with optional
//! `[iterations_per_target]` (default 2_000_000). Any panic is caught, the input written to
//! `fuzz/findings/<target>/`, and the run continues; a nonzero exit means at least one finding.

use std::panic::{self, AssertUnwindSafe};
use std::path::{Path, PathBuf};

/// A small, dependency-free deterministic PRNG (xorshift64*). Determinism matters here for the
/// same reason it does in the sim: a finding must reproduce from the same seed.
struct Rng(u64);
impl Rng {
    fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545F4914F6CDD1D)
    }
    fn below(&mut self, n: usize) -> usize {
        if n == 0 {
            0
        } else {
            (self.next_u64() % n as u64) as usize
        }
    }
}

/// Mutates `seed` into `out` with a handful of byte-level operations (flip, insert, delete,
/// splice, truncate). Deliberately structure-blind — the point is to reach code paths the
/// hand-written tests do not.
fn mutate(seed: &[u8], rng: &mut Rng, out: &mut Vec<u8>) {
    out.clear();
    out.extend_from_slice(seed);
    let rounds = 1 + rng.below(16);
    for _ in 0..rounds {
        if out.is_empty() {
            out.push(rng.next_u64() as u8);
            continue;
        }
        match rng.below(6) {
            0 => {
                // flip a byte
                let i = rng.below(out.len());
                out[i] ^= 1u8 << rng.below(8);
            }
            1 => {
                // set a random byte
                let i = rng.below(out.len());
                out[i] = rng.next_u64() as u8;
            }
            2 => {
                // insert a byte
                let i = rng.below(out.len() + 1);
                out.insert(i, rng.next_u64() as u8);
            }
            3 => {
                // delete a byte
                let i = rng.below(out.len());
                out.remove(i);
            }
            4 => {
                // truncate
                let n = rng.below(out.len());
                out.truncate(n);
            }
            _ => {
                // duplicate a span (grow toward interesting lengths)
                let len = out.len();
                let a = rng.below(len);
                let b = a + rng.below(len - a) + 1;
                let span: Vec<u8> = out[a..b].to_vec();
                let at = rng.below(out.len() + 1);
                out.splice(at..at, span);
            }
        }
    }
}

fn load_corpus(dir: &Path) -> Vec<Vec<u8>> {
    let mut seeds = Vec::new();
    if let Ok(entries) = std::fs::read_dir(dir) {
        for e in entries.flatten() {
            if e.path().is_file()
                && let Ok(bytes) = std::fs::read(e.path())
            {
                seeds.push(bytes);
            }
        }
    }
    if seeds.is_empty() {
        seeds.push(Vec::new()); // still fuzz from empty if no corpus
    }
    seeds
}

fn main() {
    // Silence panic backtraces during the run — we catch and report ourselves.
    panic::set_hook(Box::new(|_| {}));

    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let iterations: u64 = std::env::args()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(2_000_000);

    let mut findings = 0usize;
    for &(target, run) in ludion_fuzz::TARGETS {
        let corpus = load_corpus(&root.join("corpus").join(target));
        let mut rng = Rng(0x9E3779B97F4A7C15 ^ target.len() as u64);
        let mut buf = Vec::new();
        let mut hits = 0usize;
        for i in 0..iterations {
            let seed = &corpus[(i as usize) % corpus.len()];
            mutate(seed, &mut rng, &mut buf);
            let input = buf.clone();
            let result = panic::catch_unwind(AssertUnwindSafe(|| run(&input)));
            if result.is_err() {
                hits += 1;
                findings += 1;
                let dir = root.join("findings").join(target);
                let _ = std::fs::create_dir_all(&dir);
                let name = format!("panic-{:016x}.bin", fnv1a(&input));
                let path = dir.join(&name);
                let _ = std::fs::write(&path, &input);
                eprintln!(
                    "[{}] FINDING #{hits}: input {} bytes -> {}",
                    target,
                    input.len(),
                    path.display()
                );
                if hits >= 25 {
                    eprintln!("[{}] stopping after 25 findings", target);
                    break;
                }
            }
        }
        println!("{:24} {iterations} execs, {hits} finding(s)", target);
    }

    let _ = panic::take_hook();
    if findings > 0 {
        eprintln!("TOTAL findings: {findings} (see fuzz/findings/)");
        std::process::exit(1);
    }
    println!("no findings");
}

/// FNV-1a for stable finding filenames.
fn fnv1a(bytes: &[u8]) -> u64 {
    let mut h = 0xcbf29ce484222325u64;
    for &b in bytes {
        h ^= b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    h
}
