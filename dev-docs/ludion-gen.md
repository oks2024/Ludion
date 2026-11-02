---
description: The ComfyUI client crate: recipe resolution, API-graph validation, the run pipeline and accept
---

# ludion-gen

**Scope:** the crate between the `kind: generation` document and a running ComfyUI server — path
resolution, API-graph validation, the submit/follow/download pipeline, contact-sheet composition and
the accept step. The document layer (parse/validate/emit, `GenerationDocument`, `Origin`) belongs to
`ludion-scene` and is `ludion devdocs scene-kind-documents`; the CLI shell over this crate is
`ludion devdocs ludion-cli`; the authoring format and the verb surface are `ludion docs generation`.

## Layout

- `crates/ludion-gen/src/recipe.rs` — `resolve`: every path a recipe names, taken relative to the recipe document's own directory (the `shape: voxel` rule), so a recipe folder moves as a unit. `GENERATION_DIR` fixes the project-relative root.
- `crates/ludion-gen/src/workflow.rs` — the API-format graph: parse, the `_meta.title` index, `apply` (leaf writes only), `with_seed` (returns the graph and a `SeedUse`: `Written`, `Pinned` or `Unknown`, which decides the seed a candidate is labelled with), `graph_hash`, `OutputKind`, and the offline + `/object_info` passes.
- `crates/ludion-gen/src/client.rs` — `ureq` over the HTTP endpoints plus a synchronous `tungstenite` progress stream. `resolve_url`, `COMFY_URL_ENV`, `DEFAULT_COMFY_URL`.
- `crates/ludion-gen/src/run.rs` — the pipeline and `RunReport`/`RunOutput`/`RunImage`/`RunStatus`; `run` (failing with a `RunError`: its diagnostics plus the run folder once the server holds a job of the run), `status`, `fetch`, `read_report`, `run_order` (run folders oldest to newest: base, then the `-2`, `-3` attempt numerically).
- `crates/ludion-gen/src/sheet.rs` — `layout` (pure geometry), `fitted_label` (the label and glyph scale that fit a cell: full, then `#<index>`, then none) and `compose` (the IO half), plus the crate-private glyph table the labels are drawn with.
- `crates/ludion-gen/src/accept.rs` — freezing one candidate: the PNG re-encode, the `Origin` fill and the `.origin.yaml` sidecar for an image; for a mesh, an in-process `ludion_import::import` call (`ExternalFiles::Refused`: only embedded sources) and the `origin:` splice into every produced mesh document.
- `crates/ludion-gen/src/clock.rs` — `Stamp`, three spellings of one UTC instant from `SystemTime` alone. The workspace carries no date crate and one stamp per run does not justify acquiring one.
- `crates/ludion-gen/tests/mock.rs` — the mock-server suite; `tests/common/mock_server.rs` the server itself, shared with `ludion-cli`'s `gen` tests by `#[path]`; `tests/fixtures/` its canned graphs and `/object_info` capture.

## Dataflow

One run, end to end: `recipe::resolve` (document → resolved paths) → `workflow::parse` + `apply`
(overrides by title) → `Client::upload_image` for each `images:` entry, under `upload_name` →
`run::create_run_dir` (the folder is the run id) → `workflow::with_seed` per candidate →
`Client::subscribe` → `Client::submit` → `Client::follow` → `Client::job_states` →
`Client::view` per output of each finished job → `run.json`. `run.json` is written as
`status: "submitted"` before the first job is posted and after each accepted one, then `ok` /
`partial` / `failed` once every job has a verdict (`failed` lists the ones with no output). A run
that times out or is killed therefore leaves a folder `status` and `fetch` can resume from — that
write discipline is the whole reason `--no-wait` needs no extra state.

The endpoint map, the write discipline of `run.json`, `RunReport::images` and the rules for follow, verdicts and the HTTP client (subscribe-before-submit, `/queue` before `/history`, no TLS, the download cap): `ludion devdocs gen-run-protocol`.

Accept's dataflow is shorter and makes no server call and no recipe read: `run::read_report` → the
candidate's source file → `accept::accept`. An image re-encodes through `ludion_assets`; a mesh calls
`ludion_import::import` in-process — this crate depends on `ludion-import` directly, the same `.glb`
reader `ludion import` calls on a file from disk, but with `ExternalFiles::Refused`, so a candidate
with an external buffer or image is `L1204` — then splices `Origin` into every `MeshDocument` in
the returned plan **before** `Import::write` touches disk, so each document is written once, already
carrying its provenance; there is no read-modify-write pass over a file already on disk.
`accept::accept` takes a `&Registry` parameter the CLI supplies, because the built-in component set
lives engine-side (`ludion-engine`) and this crate must not depend on it — the same reason
`ludion-import` itself takes no registry.

## Rules

- **`accept` rewrites every `L1202`'s help, and the CLI prints diagnostics before its success
  lines.** The importer's own advice ("re-export without it") addresses someone holding an
  exportable source, which whoever generated a `.glb` is not: `readdress_lossy` names
  `gen accept … --allow-lossy` while the import is strict, and says what was dropped once the flag
  has been passed — never advising a flag that is already on. `allow_lossy` is a plain parameter of
  `accept`, forwarded to `ludion_import::import`, and the warnings it produces are printed by
  `run_accept` *ahead* of the destination lines, so a green accept ends on what it wrote rather than
  on a warning.
- **This is a host-side crate: no determinism denylist.** It reads a wall clock, may use std
  collections, and nothing it produces enters a `WorldHash`. What it writes into the project goes
  through `ludion-scene`'s and `ludion-assets`' canonical writers, which are the parts that must be
  deterministic. Do not copy a sim-side `clippy.toml` in here.
- **The address never reaches a file.** `resolve_url` takes `--url`, then `LUDION_COMFY_URL`, then
  ComfyUI's own loopback default; `RunReport` and `Origin` record the ComfyUI version and the
  checkpoint filenames instead. Adding a URL field to either is a privacy regression, not a feature —
  these files are committed or pasted into logs.
- **A recipe sets leaf inputs; it never rewires.** `apply` writes scalars and resolved image names
  into `inputs` by title. Any other structured value is refused at the document layer. An edge is the
  workflow's business, and the workflow is committed verbatim.

## Gotchas

- **`RunReport::dir` is `#[serde(skip)]`.** A run folder's own path is not a fact the file inside it
  should be able to disagree with, so `--json` re-inserts it at print time. Serializing it into
  `run.json` would make a moved folder self-contradictory.
- **`/object_info` is a multi-megabyte reply.** The committed fixture is a hand-trimmed capture, not
  a full one; when a new node class needs covering, add just that class's entry rather than
  re-capturing.
- **The sheet's labels are the embedded 8×8 face**, stamped by `ludion_font::stamp_label` over the
  decoded `EMBEDDED_PNG`; `ludion-font` is pure, so this crate still does not depend on wgpu. The
  label strip is `8 × label_scale` tall, and `label_scale` is one step per 192 px of cell width;
  `fitted_label` starts there and steps down, so a label never inks past its cell.
- **`layout` is pure and `compose` is the IO half** precisely so the geometry is testable with no
  decoder, no GPU and no golden image. Keep new geometry in `layout`.

## Verify

```
cargo test -p ludion-gen
cargo clippy -p ludion-gen --all-targets -- -D warnings
```

`tests/common/mock_server.rs` binds a `TcpListener` on port `0` in a thread and serves the endpoints
from `tests/fixtures/` (compiled in with `include_bytes!`, so it works from any crate); `tests/mock.rs` drives it: the happy path end to end (submit → progress frames → history → downloads →
`run.json`), one case per code `L1162`–`L1171`, both paths through `follow` (the WebSocket and the
`websocket: false` polling fallback), the `stall` config for the timeout, and the submit/fetch split.
The mock models ComfyUI's routing: a job finishes at submit and its frames go only to the socket
registered under its submit's `client_id` at that moment (recorded in `sockets`), else the prompt id
lands in `dropped` and is never replayed; `frames: false` leaves only `/history` to observe the
jobs, and an `Explode` graph's (or a `fail_jobs` submit number's) history entry has ComfyUI's failed
shape. `/queue` lists every job `/history` has not yet recorded (`stall`, or `history_after` polls
not reached), `forget: true` accepts jobs that are never queued, run or recorded, and
`Mock::restart()` clears them all, as a ComfyUI restart does and `Mock::forget_job` does for one. `queue_failures` makes the first N `/queue` requests 500, `hold_submit` parks one `/prompt` until `Mock::release_submits()`, and `hang_upgrade` accepts `/ws` and never answers. `/upload/image` parses the multipart
body byte-exactly into an input store (`files`) and models ComfyUI's naming: `overwrite` replaces,
identical bytes reuse the name, different bytes take `stem (i).ext`; `/view` serves `truncated.png`
as a body cut off mid-reply.
No GPU, no Python, no network — it runs in hosted CI.

`accept.rs`'s own `#[cfg(test)]` module covers both carriers directly, with no server: the image
happy path and its byte-exact sidecar, the mesh happy path
(`a_mesh_candidate_is_imported_with_its_provenance_in_every_document` — a committed textured glTF
fixture wrapped in a GLB, imported into a temp project, the `origin:` block spliced into every mesh
document ahead of `name:`/`positions:`, the texture references surviving intact), the `images` record
reaching both carriers between `checkpoints` and `prompt`, the `L1202` readdressing in both
directions, and one `L1172` case each for an out-of-range index, an unfinalized run, an existing
image path, and a mesh destination directory that already holds a file.

The **live smoke** is the `#[ignore]`d `live_smoke_against_a_real_server` at the bottom of that file,
gated on `LUDION_COMFY_URL`:

```
LUDION_COMFY_URL=<the server> cargo test -p ludion-gen -- --ignored --nocapture
```

It is a manual lane. It never runs in hosted CI (no GPU there), and it does not run on a box that
blesses goldens while a check suite is running: a generation run takes the whole card, and GPU
contention there is the known cause of device-lost failures in the test lanes.
