---
name: lore-merge
description: Repair a diverged Lore branch by hand — use when `commit.mjs` or `lore push` reports "diverged" / "advanced by another instance", when `lore sync` leaves `<<<<<<< ours` markers or `~base`/`~mine`/`~theirs` sidecars, or when `lore commit` refuses "still in conflict". Load it BEFORE running any lore verb on the merge.
---

# Repairing a diverged Lore branch

Everything here is verified on Lore v0.8.6 (client and server pinned together). The wrapper
(`scripts/commit.mjs`) refuses to help with a divergence; the repair is by hand.

## What happened

Concurrent commits from two machines (or two instances) make `lore sync` a real 3-way merge. It:

- writes `<<<<<<< ours` / `>>>>>>> theirs` markers into conflicted files — a `.rs` with them fails
  to build with "encountered diff marker", which is often how you find them;
- leaves `<path>~base` / `~mine` / `~theirs` sidecars that must be **deleted**, never committed;
- refuses to sync again while a staged state exists.

Sequence the merge for a moment when no subagent or other instance is mid-edit, and tell the other
instances to hold their commits.

## The recipe

1. Edit each conflicted file to the content you want; delete its sidecars.
2. `lore branch merge resolve <paths>` — commits a proper two-parent merge.
3. `lore commit` directly, then `lore push`, then one `node scripts/commit.mjs "<message>" --all`
   to catch the git mirror up.

Do not use `commit.mjs` for the merge commit itself: with paths it re-stages the conflicted file
normally and undoes the merge pairing; path-less it sweeps the whole tree.

## Three verbs that look right and are not

- **`lore stage merge`** resolves from *history*, ignores your edit, and leaves `lore commit`
  refusing "still in conflict" forever.
- **`resolve mine` / `resolve theirs`** pick the wrong side in divergence merges (measured). Trust
  only content you wrote.
- **`lore unstage` mid-merge silently discards the merge pairing.** The commit then records your
  own revision as *both* parents, stays diverged, and later syncs treat the conflicted files as
  already merged and **silently overwrite the other machine's edits**.

## Escapes (both verified)

- **Before** such a commit: `lore branch merge restart <paths>` regenerates the conflict —
  regenerated markers can swap ours/theirs, so the sidecars are authoritative — and
  `lore branch merge abort` backs out entirely.
- **After** one: back up your resolved files first. `lore unstage .` +
  `lore branch reset <remote-head-signature>` both revert working files to remote content (hence
  the backup). Re-apply, stage, commit, push.
- If `lore status --scan` then misreports real edits as clean (seen once), `lore reset <paths>` +
  re-copy unsticks it.

If the permission layer refuses `lore branch reset`, ask the user to run it with the `!` prefix.

## After

`lore branch info main` must show Latest == Remote Latest. Never stack a second commit on an
unpushed one — one divergence becomes two.
