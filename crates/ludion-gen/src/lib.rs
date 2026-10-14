//! The ComfyUI generation client: resolve a `kind: generation` recipe, validate its workflow, and
//! drive one run to a folder of candidates.
//!
//! [`ludion_scene`] owns the recipe's text and reads no file. This crate is everything past the
//! document: [`recipe`] resolves the paths a recipe names against its own directory, [`workflow`]
//! parses the API-format graph and applies the recipe's overrides to it, [`client`] speaks HTTP and
//! a WebSocket to an already-running server, and [`run`](mod@run) is the pipeline — upload, submit, follow,
//! download, `run.json`. [`sheet`] reads a finished run's candidates into one grid, and [`accept`](mod@accept)
//! freezes one of them into the project with its provenance — an image through the canonical PNG
//! writer, a mesh through [`ludion_import`], the one glTF reader.
//!
//! **The engine never learns ComfyUI exists.** Nothing here is a wire method, a component or a
//! loader: a run is scratch, and only an explicit accept freezes a candidate into the project as an
//! ordinary asset. The server address resolves from `--url` or `LUDION_COMFY_URL` and no artifact
//! this crate writes records it.
//!
//! **This is a host-side crate with no determinism denylist.** It reads a wall clock (a run id is a
//! UTC timestamp), it may use std collections, and nothing it produces enters a `WorldHash` — the
//! sim-side rules that bind `ludion-sculpt` and `ludion-scene` would be meaningless here. What it
//! writes into the project instead goes through the canonical writers those crates own.
//!
//! # The failure taxonomy
//!
//! Failure is the product in a pipeline whose far end is a machine someone else is using, so each
//! mode has its own code and its own recovery line:
//!
//! | code | raised by | means |
//! |---|---|---|
//! | `L1038` | [`run`](mod@run), [`accept`](mod@accept), [`sheet`] | a run folder, candidate, `run.json`, contact sheet or accepted file could not be written or encoded |
//! | `L1161` | [`recipe`] | the recipe's structure is wrong (a missing key, a wrong type, an unknown key) |
//! | `L1162` | [`recipe`], [`run`](mod@run) | a path the recipe names does not resolve, or lies outside the project root |
//! | `L1163` | [`workflow`] | the workflow file is not an API-format graph (a UI export) |
//! | `L1164` | [`workflow`] | a node class the server does not have |
//! | `L1165` | [`workflow`] | a required input unset, or an override the input's type refuses |
//! | `L1166` | [`workflow`] | an override names a `_meta.title` that is absent or shared |
//! | `L1167` | [`client`] | nothing answered at the resolved address, or an address that is not plain `http://` |
//! | `L1168` | [`client`] | the server answered and refused (a `/prompt` 400 and its `node_errors`), or a reply that could not be read |
//! | `L1169` | [`run`](mod@run) | a job failed on the server, is lost, or finished and wrote no file |
//! | `L1170` | [`client`], [`run`](mod@run) | the follow outlived its budget, or a fetch found a job still queued; `gen status` / `gen fetch` recover |
//! | `L1171` | [`workflow`], [`sheet`] | no output node (or only a preview), both kinds in one graph, or a sheet of a mesh run or a run with no candidate |
//! | `L1172` | [`accept`](mod@accept) | an accept was refused: a failed or unfinished run, an index outside it, a destination that exists, or an old report that cannot be re-read faithfully |
//! | `L1217` | [`workflow`] | an `images:` entry no `inputs:` override binds |
//! | `L1266` | [`run`](mod@run) | a warning: `count` above 1 and no seed input takes the run's seed, so the candidates are identical |
//!
//! # Example
//!
//! ```no_run
//! use std::path::Path;
//!
//! let resolved = ludion_gen::recipe::resolve(Path::new("generation/recipes/idol.gen.yaml"))?;
//! let url = ludion_gen::client::resolve_url(None).map_err(|d| vec![d])?;
//! let report = ludion_gen::run::run(
//!     &resolved,
//!     &ludion_gen::run::RunOptions { url, ..Default::default() },
//! ).map_err(|e| e.diagnostics)?;
//! println!("{} candidates in {}", report.outputs.len(), report.dir.display());
//! # Ok::<(), Vec<ludion_core::diagnostics::Diagnostic>>(())
//! ```

#![forbid(unsafe_code)]
#![warn(missing_docs)]

mod clock;

pub mod accept;
pub mod client;
pub mod recipe;
pub mod run;
pub mod sheet;
pub mod workflow;

pub use accept::{Accepted, AcceptedMesh, AcceptedMeshDocument, accept};
pub use client::{
    COMFY_URL_ENV, Client, DEFAULT_COMFY_URL, FollowVia, HistoryEntry, JobError, JobState,
    OutputFile, Progress, Subscription, SystemStats, resolve_url,
};
pub use recipe::{Resolved, resolve};
pub use run::{
    RunBinding, RunError, RunFailure, RunImage, RunOptions, RunOutput, RunReport, RunStatus, fetch,
    inputs_to_json, run, status,
};
pub use sheet::{Cell, Layout, compose, layout};
pub use workflow::{Graph, GraphNode, OutputKind, SeedUse};
