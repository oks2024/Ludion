#!/usr/bin/env bash
# Scheduled bounded fuzz job, macOS/Linux half — a generic gate script run on a nightly schedule
# by the private TeamCity configs; running the fuzzers is deliberately absent from
# scripts/check.mjs and .github/workflows/ci.yml (only their compile check is there; ci.yml's
# trailing note says why). See fuzz.ps1 for the full
# rationale; the two scripts must stay behaviorally identical.
#
# Runs each cargo-fuzz target for a bounded time budget under the quarantined nightly pinned in
# fuzz/rust-toolchain.toml, OUTSIDE the workspace's stable 1.94.0 pin. Scheduled (nightly trigger),
# not part of the per-commit `check` gate. A crash writes a reproducer under
# fuzz/artifacts/<target>/ and fails the job; minimize it (`cargo fuzz tmin`) and land it as a
# plain #[test] regression in the owning crate.
#
# Run from the repository root. Requires the pinned nightly toolchain + cargo-fuzz on the agent:
#   rustup toolchain install <the channel in fuzz/rust-toolchain.toml> --component rust-src
#   cargo install cargo-fuzz

set -u

seconds="${FUZZ_SECONDS:-300}"

if [ ! -f 'fuzz/Cargo.toml' ]; then
    echo 'not at the repository root: fuzz/Cargo.toml not found' >&2
    exit 1
fi

# The nightly is pinned in one place; both fuzz scripts read it from there.
toolchain=$(sed -n 's/^channel *= *"\(.*\)"$/\1/p' fuzz/rust-toolchain.toml)
if [ -z "$toolchain" ]; then
    echo 'fuzz/rust-toolchain.toml has no channel = "..." line' >&2
    exit 1
fi

# The target list is whatever `cargo fuzz list` prints, so a new fuzz_targets/ entry is scheduled
# without touching this script. A read loop, not mapfile: macOS ships bash 3.2.
targets=()
while IFS= read -r t; do
    [ -n "$t" ] && targets+=("$t")
done < <(cargo "+$toolchain" fuzz list)
if [ "${#targets[@]}" -eq 0 ]; then
    echo 'cargo fuzz list printed no targets (is cargo-fuzz installed?)' >&2
    exit 1
fi

failed=()
for t in "${targets[@]}"; do
    echo "=== fuzzing $t for ${seconds}s ==="
    # -max_total_time bounds the run; libFuzzer's default -rss_limit_mb=2048 aborts on an allocation
    # bomb, which is a finding we WANT surfaced.
    if ! cargo "+$toolchain" fuzz run "$t" -- "-max_total_time=$seconds"; then
        echo "FUZZ FINDING in $t (see fuzz/artifacts/$t/)"
        failed+=("$t")
    fi
done

if [ "${#failed[@]}" -gt 0 ]; then
    echo "fuzz findings in: ${failed[*]}"
    exit 1
fi
echo 'fuzz: no findings'
exit 0
