---
description: ludion-gen run protocol — endpoint map, run.json write discipline, follow/verdict rules
---
# gen-run-protocol

**Scope:** how `ludion-gen` talks to ComfyUI and records a run: the endpoint map, the `RunReport` record, and the rules for deciding a job is finished. Hub: `ludion devdocs ludion-gen`.

## Dataflow

The endpoint map:

| method | route |
|---|---|
| `Client::system_stats` | `GET /system_stats` — the version provenance records |
| `Client::object_info` | `GET /object_info` — the node schema the live pass checks against |
| `Client::queue` | `GET /queue` |
| `Client::upload_image` | `POST /upload/image` (multipart, no `overwrite`) as `ludion-<content hash>.<ext>` → the server-side name; the same name is the run folder's `inputs/` copy |
| `Client::submit` | `POST /prompt` → `prompt_id` |
| `Client::history` | `GET /history/{prompt_id}` — a failure is data (`HistoryEntry::error`), not an error |
| `Client::job_states` | `GET /queue`, then `/history/{id}` per id → `Queued` / `Finished` / `Lost` |
| `Client::view` | `GET /view?filename=&subfolder=&type=` |
| `Client::subscribe` | `ws://…/ws?clientId=` — opened before the first submit |
| `Client::follow` | consumes the subscription; sweeps `job_states` |

**Nothing assumes a shared filesystem.** Inputs go up through `/upload/image` and outputs come back
through `/view` even when the server is on this machine, because the same code must work when it is
not.

`RunReport::images` is the run's own record of what went up: one `RunImage { name, path, hash }` per
resolved `images:` entry, `--image` override included, written between `checkpoints` and
`prompt_ids`. The field is `#[serde(default)]`, so a `run.json` written before it existed still
reads, and `path` is project-relative or — for a scratch override outside the root — the bare file
name, never an absolute one. `accept` copies the three fields straight into `Origin::images`, which
is the only thing an image-to-mesh asset's provenance has to say what it was made *from*. After
`images` the report carries `prompt` and `inputs` (`Option`s, `#[serde(default)]`): the snapshot
`provenance` builds the `Origin` from, so accept reads no recipe for a new run. A report without the
snapshot re-resolves the recipe only when `recipe_hash` and `workflow_hash` still match, else `L1172`;
an empty `images` there is rebuilt from the run folder's `inputs/` copies (`legacy_images`).
`inputs_to_json`/`inputs_from_json` are the one converter between the recipe's input table and JSON
(the CLI's `--json` output uses them too).


## Rules

- **A frame is not a verdict.** A pending job leaves only through `Client::job_states`: in
  `/history` (success or failure, `HistoryEntry::error`) or lost. `execution_success`,
  `execution_error`, `execution_interrupted` and `executing` with a **null** node (never `executed`,
  which a multi-saver graph emits once per output node) only bring the next sweep forward, because
  the frame goes out before ComfyUI writes `/history`.
- **`/queue` before `/history`.** A job absent from the earlier queue read with no history entry is
  *lost* — ComfyUI keeps both in memory, so a restart forgets it — and lost is terminal. The other
  order would call a job that finished between the reads lost. A failed or lost job never aborts the
  follow, `status`, `fetch` or the download: `complete` records each as `RunFailure` (reason
  `error`, `lost`, or `REASON_EMPTY` for a job that finished and wrote no file) and finalizes `ok` /
  `partial` / `failed`. The exception is `fetch` and `status` on a `submitted` run whose jobs are
  *all* lost (or that lists none): `no_jobs_accepted` / `server_knows_none` refuse with `L1391` and `run.json` is left
  as it was, because a wrong `--url` reads the same as a restart. A waited `run` never takes that
  path: it follows on the URL it submitted to, so all-lost finalizes `failed`. A sweep fails on any
  `/queue` or `/history` error: `follow` tolerates `MAX_FAILED_SWEEPS` (3) consecutive failed sweeps
  and returns the third; `run` then appends the `gen fetch <dir>` pointer (`follow_phase_error`).
- **`run.json` is written before the first `POST /prompt` and after every accepted one** (one
  `write_atomic` each, at most 64). Until a job is accepted nothing is on the server, so any failure
  removes the folder; a failed write after an accepted job keeps it and names the unrecorded
  `prompt_id`. A kill between an accepted POST and its write loses that one job.
- **A finalized run is final.** `status` and `fetch` on a run that is no longer `submitted` do not ask
  the server about its jobs (after a restart all would read lost, and `fetch` would overwrite a good
  `ok`).
- **Subscribe before the first submit.** ComfyUI sends a client's frames only to the socket
  registered under its `clientId` when they are emitted and never replays them, and a cached job
  finishes in milliseconds. `run()` therefore opens `/ws` (`Client::subscribe`) before the first
  `POST /prompt` and not at all for `--no-wait`. The stream follow also sweeps `/history` for the
  pending ids on entry and every 2 s, so a job whose frames never reached the socket is still
  observed.
- **A WebSocket that will not open is not an error.** `subscribe` returning an empty subscription
  (a proxy that refuses the upgrade, a server built without it) makes `follow` poll `/history`
  every 500 ms under the *same* deadline (capped at `MAX_TIMEOUT`, a week, so a huge `--timeout`
  cannot overflow), sharing `sweep_history` with the stream path, and `Progress::via` records which
  path ran. A mid-run hangup falls back too: `/history` still holds
  the verdict. The upgrade is bounded: `subscribe_within` puts a 10 s budget on the connect and each
  handshake read or write (name resolution is not bounded), and any failure, a timeout included,
  is "no socket".
- **`ureq` is built with `default-features = false`: no TLS, no gzip.** The consequence is real and
  intended — `resolve_url` refuses `https://` (`L1167`) before anything is tried, so only plain
  `http://` over a private network is reachable.
  Enabling TLS pulls a certificate stack into a crate whose entire job is one LAN hop; if a remote
  server ever needs encryption, it belongs behind a tunnel, not in this dependency tree.
- **Download bodies are capped at 512 MiB, not `ureq`'s 10 MB default.** A `.glb` blows past the
  default. A body that cannot be read to the end (the cap, a dropped connection, a timeout) is
  `L1168`, not "nothing answered"; `/view` gets its own 600 s budget, the rest 60 s.
