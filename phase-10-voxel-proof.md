# Phase 10 — THE test (voxel authoring)

The record of Phase 10 Wave 6's exit-criteria run (2026-07-12). The forcing function, per
`Plan/phase-10-voxel-models.md` decision 9: *a fresh agent, from a one-line prompt ("author a voxel
lantern model and render it in a scene"), with only shipped docs, produces a valid `.voxel.yaml` +
scene + green blessed golden, zero intervention.* The format exists solely to be agent-writable, so
friction in the format is this phase's real output.

## What actually happened

A fresh agent was spawned with the one-line task plus **environment facts only** (the built CLI's
path, a scratch directory outside the checkout), and an explicit prohibition on reading the engine
checkout — no plan, no source, no gallery examples. Every voxel-facing fact came from the shipped
docs: the scaffolded `LUDION-GUIDE.md` (embedding `docs/engine-guide.md`'s new "Voxel models"
chapter) and `ludion docs`.

Result: `ludion new voxel-lantern`, then a hand-authored 5×8×5 lantern (138 filled voxels, two
palette glyphs — dark metal frame, warm amber glass: base plate, frame rings, three glass-body
layers with corner posts, tiered roof, knob finial), referenced from the scene as
`Mesh { shape: { voxel: … } }`, rotated for a 3/4 view, lit warm-key/cool-fill. Full suite green:
gameplay ×3, drift, and a real golden comparison against the blessed `goldens/main.png` (RTX
3090/Vulkan). The orchestrator re-ran the suite independently (5/5 green) and confirmed the golden
reads unmistakably as a lantern.

**The plan's predicted sharp edge never cut.** The risk register named slice-orientation confusion
(rows/z vs columns/x, bottom-up layers) as the likely suspect; the model checked clean on the
*first* `check-scene` run — `OK (5x8x5, 138 filled voxels)` — with the geometry meaning what the
agent intended. The guide's "Orientation — the one thing to get right" section (explicit rule list
plus a worked 2×1×2 example) is the difference between this run and that prediction.

The agent's loop was the designed one: author → `check-scene` the model alone → preview render →
adjust framing once → bless → full suite. One diagnostic was hit in the entire run (`L1101`, below),
and its message was self-explanatory — `ludion explain` was never needed.

## Friction → action

| # | Finding | Verdict | Action |
|---|---------|---------|--------|
| 1 | Orientation rules, palette discipline, origin/sizing all held first-try; the format was authored blind without one grid error | praise | none — the worked-example approach in the guide chapter is load-bearing; keep it |
| 2 | **Where `voxels/` lives was the one real stumble**: the guide said paths are content-root-relative and showed `voxels/*.voxel.yaml`, but the project-anatomy table never placed the directory; the natural first guess (project root, next to `goldens/`) fails `L1101` since the content root is the *scene's* directory (`scenes/voxels/…`) | doc gap | **fixed in-phase** (this wave): the anatomy section now states where asset dirs live in a scaffolded project, and the voxel chapter's referencing section names the exact path and the failure mode |
| 3 | No shipped way to render the *committed* scene when it names game components: CLI `render`/`check-scene` are built-ins-only (`L1013`, documented), so previewing framing meant maintaining a parallel built-ins-only `preview.yaml`, then mirroring tuned values into `main.yaml` and verifying via the blessed golden | UX gap, mild | deferred with trigger recorded: a game-registry one-shot render (`cargo run -- --render-once` or a testkit helper) when a second authoring run trips on it; the bless-then-read-the-golden loop is a workable fallback |
| 4 | Camera framing for a model of known size is guess-and-check (no aim-at helper, no fov/distance guidance); one preview iteration was enough | not a defect | none now — noted as expected render-look-adjust; a look-at authoring nicety is a possible future guide recipe |
| 5 | The guide's PointLight calibration numbers (from the Phase 7 fix) matched reality unchanged | praise | none — calibration sentences in component rows keep earning their place |

## What this proves for the exit gate

The phase's question — *is a voxel grid an authoring representation an agent can use blind?* — is
answered yes on the first attempt: format authored without error, one filesystem-layout stumble
(now documented away), zero human intervention, and a verifiable artifact at the end. The
remaining Phase 10 gate is the companion human pass: one look at the voxel gallery render
(`examples/gallery/goldens/voxel-model.png`).
