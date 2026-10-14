#!/usr/bin/env bash
# macOS deps guard — a generic gate script for a persistent macOS CI agent, run as the first step
# of the private TeamCity `check (macos)` configuration; deliberately absent from scripts/check.mjs
# and .github/workflows/ci.yml (GitHub runners are ephemeral, and a local loop has no CI agent's
# target dir to police). Background: the readdir-stall plan in the private Plan-Private/ folder.
#
# macOS 26 resolves a bare executable's "main bundle" by iterating its parent directory, once per
# process, the first time CoreFoundation is asked (Metal, CoreAudio, a test-binary listing) —
# `_CFBundleGetBundleVersionForURL` → `readdir`. Every test binary lives in `target/debug/deps`, and
# on the agent that directory reached 963,006 entries (2026-08-22): ~90 s per process, a 44-minute
# nextest listing, every Metal/CoreAudio test past its timeout, `check (macos)` red for a week. A
# clean full build set is ~5k entries; the producer of the rest is still unnamed.
#
# So this step, every build: prints the count; above LUDION_DEPS_GUARD_REPORT also prints the
# breakdown (the build that fires the guard names the producer in its own log); above
# LUDION_DEPS_GUARD_MAX cleans the dev profile — cheap on the M4, a cold workspace build is ~3 min —
# and pre-warms a scaffold's dependency set so `ludion-cli::dev`'s 180 s first-`listening` budget
# survives the clean (measured 2026-08-23: a bare clean fails that test once).
#
# Never fails the build: exit 0 on every path. Run from the repository root. Deliberately absent from
# .github/workflows/ci.yml — GitHub runners are ephemeral.

set -u

deps='target/debug/deps'
max="${LUDION_DEPS_GUARD_MAX:-30000}"
report="${LUDION_DEPS_GUARD_REPORT:-12000}"

if [ ! -d "$deps" ]; then
    echo "deps guard: no $deps yet (cold checkout), nothing to do"
    exit 0
fi

count=$(find "$deps" -mindepth 1 -maxdepth 1 | wc -l | tr -d ' ')
echo "deps guard: $deps holds $count entries (breakdown above $report, clean above $max)"

if [ "$count" -gt "$report" ]; then
    echo "deps guard: by crate prefix (top 15):"
    find "$deps" -mindepth 1 -maxdepth 1 | sed -E 's#.*/##; s/-[0-9a-f]{16}.*//' | sort | uniq -c | sort -rn | head -15
    echo "deps guard: by extension (top 8):"
    find "$deps" -mindepth 1 -maxdepth 1 | sed -E 's#.*/##; s/.*\.//' | sort | uniq -c | sort -rn | head -8
    echo "deps guard: entries older than 24 h: $(find "$deps" -mindepth 1 -maxdepth 1 -mtime +0 | wc -l | tr -d ' ')"
fi

if [ "$count" -le "$max" ]; then
    exit 0
fi

echo "deps guard: $count entries is past $max — cleaning the dev profile"
if ! cargo clean --profile dev; then
    echo "deps guard: cargo clean failed; continuing with the bloated tree"
    exit 0
fi

# Pre-warm: the scaffold resolves its own dependency set (no lockfile), so its dep stack is a cold
# build after a clean. Paying it here, in a throwaway scaffold sharing the workspace target dir, is
# exactly the warm reuse crates/ludion-cli/tests/dev.rs budgets around.
warm='target/tmp/deps-guard'
target_abs="$(pwd)/target"
rm -rf "$warm"
mkdir -p "$warm"
if cargo build -p ludion-cli \
    && (cd "$warm" && "$target_abs/debug/ludion" new warm_game) \
    && (cd "$warm/warm_game" && CARGO_TARGET_DIR="$target_abs" cargo build); then
    echo "deps guard: scaffold dependency set pre-warmed"
else
    echo "deps guard: pre-warm failed — expect one slow ludion-cli::dev run"
fi
exit 0
