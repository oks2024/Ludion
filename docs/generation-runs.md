---
description: The ludion gen verbs, the run folder, and what gen accept freezes — image or mesh
---

# Generation runs

**Scope:** running a `kind: generation` recipe — the `ludion gen` verbs, the scratch run folder they
write, and what `gen accept` freezes into content: an image, or now a mesh through `ludion import`
in-process. The recipe document itself, the workflow file and the loop that drives this is
`ludion docs generation`; which prompts and graph settings produce something worth accepting is
`ludion docs generation-craft`; what an accepted mesh's maps do is `ludion docs materials`.

## The verbs

Every verb takes `--json`; every verb but `sheet` and `accept` (which never contact a server) takes
`--url <u>`. **`--json` prints exactly one compact object on one stdout line, on success and on
failure**: `{ok, …the verb's fields, diagnostics}`, `ok` true exactly when the exit
code is 0, `diagnostics` what went to stderr, a field the verb could not produce `null`. `run` and
`fetch` carry the `run.json` keys whenever the folder holds one, plus `dir` (`null` if the run was
refused before its folder existed), so a timed-out run names the folder for `gen fetch <dir>`. An
argument clap cannot parse exits 2 with clap's text and no object.

| verb | does | `--json` |
|---|---|---|
| `gen nodes [filter] [--class C]` | Typed node listing. `filter` is a case-insensitive substring over class name and category; `--class` prints one class's full input table. | `{nodes: [{class, category, required, optional, outputs}]}` |
| `gen check <recipe>` | The offline passes, then the live pass. The **offline** result decides the exit code, so this runs in CI with no server; an unreachable server degrades to an `L1167` warning. | `{recipe, workflow, kind, overrides, offline, live}` |
| `gen run <recipe> [--count N] [--seed S] [--image P] [--timeout S] [--no-wait]` | Check, upload `images:`, submit `count` graphs, follow, download, write `run.json`. A job that fails or is lost does not abort the rest: it is recorded in `failed`, one `L1169` per job on stderr, the report still prints, and the exit code is 1 unless the run is `ok` (or a `--no-wait` submit). `--image` replaces the *first* declared `images:` entry with a scratch path — how a candidate chains into the next run without being accepted. `--timeout` defaults to 600 s, at most a week. | the `run.json` object plus `dir` |
| `gen status [<run-dir>]` | With a run folder, its record plus the server's verdict on every job (queued, finished, failed or lost); a finalized run is reported as written, never re-asked (`queue` is `null`). Failures are listed and exit 0. `L1391` when the server knows none of a `submitted` run's jobs (usually a wrong `--url`), or the run lists none. Bare, the queue plus the most recent run of every recipe. | `{dir, report, ready, pending, failed, queue}` (`report` is `null` when `run.json` is unreadable) or `{queue, runs}` |
| `gen fetch <run-dir>` | Completes a `--no-wait` run: downloads every finished job's output, records the failed and lost jobs, finalizes `run.json`. `L1170` if a job is still queued. `L1391` if the server knows none of the jobs (a wrong `--url` or a restarted server) or the run lists none; `run.json` stays `submitted`, so a fetch against the right server still collects it. A run that is no longer `submitted` is returned untouched. Exits as `gen run` does. | the `run.json` object plus `dir` |
| `gen sheet <run-dir> [--cols N] [--out P]` | Composes the candidate grid, one cell per output labelled `#<index> seed <n>`, cells letterboxed to a common size. A label wider than its cell shrinks, then drops to `#<index>`, then is omitted; an unknown seed shows `?`. Defaults to `sheet.png` in the run folder. A mesh run has nothing to compose: `L1171`, pointed at the model stage. | `{run_id, sheet, cells}`, where `cells` is a count |
| `gen accept <run-dir> <index> --out <path> [--allow-lossy]` | Freezes one candidate with its provenance: a re-encoded `.png` for an image, an `ludion import` tree for a mesh (`--out` names the import **directory**, which must be absent or empty). `--allow-lossy` is the import mode, as on `ludion import`. Never overwrites (`L1172`). | `source`, `destination`, `sidecar`, `origin`, plus `meshes`/`textures` for a mesh |

There is no `gen new`, `gen delete` or `gen ls`: a recipe is a dozen lines with `gen check` as its
entry, and a run folder is a folder.

## The run folder

```
generation/runs/<recipe-name>/<run-id>/
  run.json          the record
  inputs/           every uploaded source, under its upload name `ludion-<hash>.<ext>`
  0000.png          candidate 0 (0000.glb for a mesh graph)
  0001.png
  sheet.png         written by gen sheet
```

`<run-id>` is a compact UTC timestamp plus 8 hex of the resolved graph hash — sortable, its hash
half saying at a glance whether two runs submitted the same graph, and covering the uploaded image
bytes. It is unique: two runs of one graph in one second take `-2`, `-3`, …. **The whole tree is
ignored by both version-control systems:** it is scratch, and megabytes of it per iteration.

**The sheet's cell label, not its position, is the index.** Cells fill row-major from `#0` top-left,
but that order is inferable only because every cell is labelled — the label is what a human and a
model both actually read.

`run.json` has a fixed key order so a diff between two runs reads: `run_id`, `recipe`, `recipe_hash`,
`workflow`, `workflow_hash`, `graph_hash`, `seed_base`, `count`, `kind`, `comfy_version`,
`checkpoints`, `images` (each `{name, path, hash}` — the recipe's `images:` entries as resolved for
*this* run, `--image` override included, the hash of the bytes that went up), `prompt` and `inputs`
(the snapshot `gen accept` reads: the prompt text the submitted graph carried and the recipe's
resolved overrides), `prompt_ids`, `seeds` (the seed each job ran at, `null` where it could not be
known), `outputs` (each `{index, seed, file, node, batch_index, prompt_id}`; `seed` is `null` when
unknown, and files from one job share its seed), `failed` (each
`{job, prompt_id, reason, node, class, message}`; `reason` is `error`, `lost` or `empty`, the seed
is the job's `seeds` entry), `started`, `finished`, `duration_ms`, `status`. It is written as
`status: "submitted"` before the first job is posted and again after each job the server accepts, then
once every job has a verdict: `ok` (no failure), `partial` (some candidates, some failures) or
`failed` (no candidate) — so a run that times out or is interrupted still leaves a folder `gen status`
and `gen fetch` can pick up. A run killed between an accepted job and its write loses that one job. A
submit refused part-way records only the jobs already submitted; a refused first submit leaves no
folder. A follow that fails after the jobs are recorded keeps the folder and names `gen fetch <dir>`.

A failed job is one outcome among the rest, never an abort. **Lost** means the server has neither a
history entry nor a queue place for the job: ComfyUI keeps both in memory, so a restart forgets every
job it held. Lost is terminal, recorded like an error with `reason: "lost"`; the queue is never
deleted from. If *every* job of a `submitted` run reads lost, `gen fetch` and `gen status` refuse with
`L1391` instead and leave the run `submitted`: a wrong `--url` looks the same as a restart, and the
right server can still collect it. A waited `gen run` follows the server it just submitted to, so
there it finalizes `failed`. `gen sheet` and `gen accept` take an `ok` or `partial` run and refuse a `failed` one.

## Accepting a candidate

`gen accept` freezes one candidate with its provenance: an image is re-encoded through the engine's
deterministic PNG path (`<asset>.png` + a `<asset>.png.origin.yaml` sidecar), a mesh is imported
in-process into a directory (strict by default; a generator's `SaveGLB` writes `doubleSided: true`,
which accept clears to keep the mesh culled). The format,
the `origin` block and its fields: `ludion docs generation-accept`.

## Rules

- **A generation run and `node scripts/check.mjs` never overlap on a GPU that blesses goldens.** A
  run is a multi-minute, full-VRAM job, and GPU contention on that box is the known cause of
  device-lost failures in the test lanes. The two are serialized by hand, by whoever starts either
  one — no scheduler, no lock file.

| code | means |
|---|---|
| `L1038` | A run folder, candidate, `run.json`, contact sheet or accepted file could not be written or encoded. |
| `L1161` | Recipe structure — a missing or wrong-typed key, an unrecognized `kind`, an unknown key with its nearest match. |
| `L1162` | Recipe data — `count < 1`, a negative seed, a non-finite number, an unresolvable `workflow`/`images` path, a reference to an undeclared image, a value that is neither a scalar nor that form, a `workflow` that resolves outside the project root (runs record it project-relative). |
| `L1163` | The workflow is not API format — a UI export. Names **File → Export (API)**. |
| `L1164` | A node class the server does not have; names it and the nearest installed match. |
| `L1165` | A required input unset, or an override the input's declared type or enum refuses. |
| `L1166` | An override names a `_meta.title` that is absent or shared by two nodes; lists every title. |
| `L1167` | Nothing answered at the resolved address (names all three sources), or the address is refused first: `https://` (built without TLS) or not `http://`. |
| `L1168` | The server answered and refused — a `/prompt` 400 with `node_errors` reprinted node by node, any HTTP error, or a reply that could not be read to the end (dropped, timed out, over 512 MiB). |
| `L1169` | A job failed on the server, is lost, or finished and wrote no file (`empty`); one per job, naming the node, its class and the server's message. |
| `L1170` | The follow outlived `--timeout`, or a fetch found a job queued; names the job ids and the `gen status`/`gen fetch` recovery. |
| `L1171` | No output node (or only a preview), both kinds in one graph, or a contact sheet asked of a mesh run or a run with no candidate. |
| `L1172` | Accept refused: a failed or unfinalized run, an index outside it, a missing candidate, a destination that already exists — a file for an image, a directory already holding anything for a mesh — or an older run that cannot be re-read faithfully: recipe or workflow changed (both hashes named), or its image copy is missing or not the declared file. |
| `L1391` | `gen fetch` / `gen status` found a `submitted` run whose jobs the server does not know at all (every one lost, usually a wrong `--url`), or whose submit was interrupted before any job was accepted. The run stays `submitted`. |
| `L1266` | A warning: `count` is above 1 but no seed input takes the run's seed (every seed input is pinned by the recipe, or fed by a link), so every job submits the same graph. Drop the pin, expose a literal `seed`, or use `--count 1`. |

## Gotchas

- **`--count 8` is eight jobs, not one batch of eight.** Each is submitted separately at its own seed
  so the index/seed identity holds and one failure does not lose the other seven. A job that writes
  several files (`batch_size` above 1) is one job: its files share its seed and differ by
  `batch_index`.
- **A duplicated node title is silent in the UI and fatal here.** Two nodes both titled `Prompt` make
  every override on that title `L1166`.
- **A mesh accept's `--out` is a directory, not a file.** Pointing it at a path shaped like an image
  accept's — a bare filename with no existing folder to sit empty in — still works (the import
  creates the directory), but reusing an actual file path from an image accept is `L1172`: a file is
  never an acceptable mesh destination.

## Next

- `ludion docs generation` — the recipe document, the workflow file, the loop.
- `ludion docs generation-accept` — what an accept writes and its `origin` provenance.
- `ludion docs generation-craft` — prompts, sizes, seeds and the image-to-image loop.
- `ludion docs materials` — what an accepted mesh's maps mean to the renderer.
