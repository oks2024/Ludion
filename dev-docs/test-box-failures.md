---
description: Test failures caused by the machine, not the suite — how to tell them from a flaky test
---

# Test failures that are the box

**Scope:** anyone reading a red `node scripts/check.mjs` or nextest run whose failure moves between
runs. A flaky test is a P0 bug, so rule the machine out first. Harness detail is
`ludion devdocs ludion-testkit`; budget files are `ludion devdocs budget-files`.

## Gotchas

### `os error 1224` on a PNG write — a held file mapping

Any test that rewrites an image under `target/tmp/` can fail with *"the requested operation cannot
be performed on a file with a user-mapped section open"* when another process holds that PNG mapped.

- **Signature:** an I/O error, never an assertion, and the victim rotates between runs — which is
  what makes it read as a flaky test.
- **Incident (2026-09-18):** the confirmed cause was an image left open in the Windows Photos app. A
  viewer maps the file it displays, so one open golden or render output is enough; a thumbnail COM
  surrogate or a search indexer over the build tree does the same.
- **Mitigation:** close the viewer; delete `target/tmp` before a full run; keep the build tree out
  of any indexer or thumbnail preview. If `rustc` itself dies with `STATUS_ACCESS_VIOLATION`, delete
  `target/debug/incremental` too.
- **`no GPU adapter (L2003) on a lane that requires one (LUDION_REQUIRE_GPU=1)`:** a panic, not a flake — the box lost its adapter (driver update, no GPU session). Fix the box; unset the variable only on a GPU-less machine.
- **PNG writes cannot hit 1224:** engine and testkit PNG writes land on a fresh temp sibling and
  rename over the target. A viewer holding the target can still fail the rename (`os error 5`);
  the mitigation above stands.
- **Scratch piles up in the real temp dir:** many tests never remove their pid-suffixed `ludion-*`
  dirs, and none survive a kill. The `check.mjs` test lanes (`test`, `doctest`, `fps-gate`,
  `tauri-test`, the ignored tiers) point `TEMP`/`TMP`/`TMPDIR` at one bounded
  `<temp>/ludion-check-<8 hex of the checkout path>`: wiped at the start of a run, removed at the
  end when those steps all passed, kept for diagnosis otherwise. It is outside `target/` on purpose:
  ludion-cli's `docs.rs` tests need scratch outside any checkout. A targeted `cargo test` still
  writes `ludion-*` dirs to the real temp dir; delete them when no test runs.
- **macOS embed/ffi boot tests fail with `L4022` under load when `target/debug/deps` holds a huge file
  count (~190k):** CoreAudio's `AudioUnitInitialize` → `CSCheckFix` scans the executable's directory on
  every audio boot, and nextest's parallel processes multiply the scan. Sweep or clean the target tree
  (`cargo sweep`); never raise the timeout.
- An older diagnosis blamed `model_sheet.rs` under parallel nextest. It is suspect: same scratch
  directory, same signature.

## Verify

Re-run the one failing test alone after closing viewers and deleting `target/tmp`. A box failure
disappears; a real defect fails the same test the same way.
