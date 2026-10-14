# Scheduled bounded fuzz job, Windows half — a generic gate script run on a nightly schedule by
# the private TeamCity configs; running the fuzzers is deliberately absent from
# scripts/check.mjs and .github/workflows/ci.yml (only their compile check is there; ci.yml's
# trailing note says why). Runs each cargo-fuzz target for a
# bounded time budget under the quarantined nightly pinned in fuzz/rust-toolchain.toml
# OUTSIDE the workspace's stable 1.94.0 pin. Coverage-guided fuzzing is
# unbounded work, so it runs on a clock, not on every commit (Plan/phase-9-hardening.md decision 3).
#
# Any target that finds a crash writes the reproducer under fuzz/artifacts/<target>/ and exits
# nonzero; the whole job then fails, and the reproducer should be minimized (`cargo fuzz tmin`) and
# turned into a plain #[test] regression in the owning crate (which the pinned stable CI runs — no
# nightly needed for regressions).
#
# Run from the repository root. Requires the pinned nightly toolchain + cargo-fuzz on the agent:
#   rustup toolchain install <the channel in fuzz/rust-toolchain.toml> --component rust-src
#   cargo install cargo-fuzz

$ErrorActionPreference = 'Stop'

# Per-target wall budget in seconds (override with the FUZZ_SECONDS env var for a longer scheduled
# soak). 300s is the local default; a nightly job may set this to e.g. 1800.
$seconds = if ($env:FUZZ_SECONDS) { $env:FUZZ_SECONDS } else { '300' }

if (-not (Test-Path 'fuzz/Cargo.toml')) {
    Write-Error 'not at the repository root: fuzz/Cargo.toml not found'
    exit 1
}

# The nightly is pinned in one place; both fuzz scripts read it from there.
$channel = if (Test-Path 'fuzz/rust-toolchain.toml') { Select-String -Path 'fuzz/rust-toolchain.toml' -Pattern '^channel\s*=\s*"(.+)"' } else { $null }
if (-not $channel) {
    Write-Error 'fuzz/rust-toolchain.toml has no channel = "..." line'
    exit 1
}
$toolchain = $channel.Matches[0].Groups[1].Value

# The target list is whatever `cargo fuzz list` prints, so a new fuzz_targets/ entry is scheduled
# without touching this script.
$targets = @(cargo "+$toolchain" fuzz list)
if ($LASTEXITCODE -ne 0 -or $targets.Count -eq 0) {
    Write-Host 'cargo fuzz list failed or printed no targets (is cargo-fuzz installed?)'
    exit 1
}

$failed = @()
foreach ($t in $targets) {
    Write-Host "=== fuzzing $t for ${seconds}s ==="
    # -max_total_time bounds the run; -rss_limit_mb (libFuzzer default 2048) aborts on an allocation
    # bomb, which is a finding we WANT surfaced (e.g. an in-the-clear length or a zstd decompression
    # bomb in the replay reader).
    cargo "+$toolchain" fuzz run $t -- "-max_total_time=$seconds"
    if ($LASTEXITCODE -ne 0) {
        Write-Host "FUZZ FINDING in $t (see fuzz/artifacts/$t/)"
        $failed += $t
    }
}

if ($failed.Count -gt 0) {
    Write-Host "fuzz findings in: $($failed -join ', ')"
    exit 1
}
Write-Host 'fuzz: no findings'
exit 0
