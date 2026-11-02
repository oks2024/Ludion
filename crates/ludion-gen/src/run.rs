//! The run pipeline: upload, submit, follow, download, report.
//!
//! A run is scratch. Everything it writes lands under
//! `generation/runs/<recipe-name>/<run-id>/`, which both VCSs ignore; the committed artifacts of
//! this arc are the recipe, the workflow, and whatever `gen accept` later freezes out of a run.
//! `run.json` is the record, written in one fixed key order so a diff between two runs reads.
//!
//! The run id is `<UTC compact timestamp>-<8 hex of the resolved graph hash>`: sortable, and its
//! hash half says at a glance whether two runs submitted the same graph. Two runs of one graph in
//! one second take `-2`, `-3`, …: the folder is created, never reused, and its name is the id.

use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use ludion_core::atomic::write_atomic;
use ludion_core::codes::code;
use ludion_core::diagnostics::Diagnostic;
use ludion_scene::{GENERATION_MAX_COUNT, GenerationDocument, InputValue, ScalarValue};
use serde::{Deserialize, Serialize};

use crate::client::{Client, JobState, OutputFile, upload_name};
use crate::clock::Stamp;
use crate::recipe::{Resolved, content_hash, relative_to, workflow_outside_root};
use crate::workflow::{Graph, OutputKind, SeedUse, apply, graph_hash, with_seed};

/// The `--timeout` a run takes when none is given. A diffusion candidate is seconds; a mesh chain
/// is minutes; ten minutes covers both without a wedged server blocking a loop forever.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(600);

/// The longest a follow waits; a longer `--timeout` waits this long.
pub const MAX_TIMEOUT: Duration = Duration::from_secs(7 * 24 * 60 * 60);

/// The scratch folder every run folder lives under, relative to the project root.
pub const RUNS_DIR: &str = "generation/runs";

/// The report's own filename inside a run folder.
pub const REPORT_FILE: &str = "run.json";

/// What `gen run` was asked for, over and above what the recipe says.
#[derive(Clone, Debug)]
pub struct RunOptions {
    /// Overrides the recipe's `count`.
    pub count: Option<u32>,
    /// Overrides the recipe's base `seed`.
    pub seed: Option<u64>,
    /// A scratch image replacing the *first* declared `images:` entry — what chains a candidate
    /// from a previous run into a mesh run without accepting it.
    pub image: Option<PathBuf>,
    /// How long the follow may take before [`code::GENERATION_RUN_TIMED_OUT`].
    pub timeout: Duration,
    /// `false` submits and returns, leaving `gen status` / `gen fetch` the other half.
    pub wait: bool,
    /// The resolved server address.
    pub url: String,
}

impl Default for RunOptions {
    fn default() -> Self {
        RunOptions {
            count: None,
            seed: None,
            image: None,
            timeout: DEFAULT_TIMEOUT,
            wait: true,
            url: crate::client::DEFAULT_COMFY_URL.to_string(),
        }
    }
}

/// One downloaded candidate.
///
/// In the ordinary one-output-node graph, `index` is the candidate index and `seed` is
/// `seed_base + index`, so a candidate's index and its seed are the same fact — what a contact
/// sheet labels and what an accept records. The identity is per job: several files from one job
/// (`batch_size` above 1, two savers) share its seed and differ by `batch_index`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunOutput {
    /// The candidate's position in the run, and its filename stem.
    pub index: u32,
    /// The seed the graph ran at; `None` when the workflow's seed is fed by a link or its pins
    /// disagree.
    pub seed: Option<u64>,
    /// The file's name inside the run folder.
    pub file: String,
    /// The node that wrote it.
    pub node: String,
    /// The file's slot in its node's output list for the job.
    #[serde(default)]
    pub batch_index: u32,
    /// The job it came from.
    pub prompt_id: String,
}

/// One job that has no output: the server recorded it as failed, or no longer knows it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunFailure {
    /// The job's position in submit order; its seed is the report's `seeds` entry.
    pub job: u32,
    /// The job's id on the server.
    pub prompt_id: String,
    /// [`REASON_ERROR`], [`REASON_LOST`] or [`REASON_EMPTY`].
    pub reason: String,
    /// The node that raised, when the server names one.
    pub node: Option<String>,
    /// That node's class.
    pub class: Option<String>,
    /// The server's own message, or what a lost job's absence means.
    pub message: String,
}

/// `reason: "error"` — the server ran the job and recorded a failure.
pub const REASON_ERROR: &str = "error";

/// `reason: "lost"` — the server has no history entry and no queue place for the job. ComfyUI keeps
/// both in memory, so a restart forgets every job it held; lost is terminal for that job. A `fetch`
/// or `status` that finds *every* job lost refuses with [`code::GENERATION_JOBS_UNKNOWN`] and
/// leaves the run `submitted` (a wrong `--url` looks the same); one lost among found jobs is
/// recorded here.
pub const REASON_LOST: &str = "lost";

/// `reason: "empty"` — the server ran the job, reported success, and it wrote no file.
pub const REASON_EMPTY: &str = "empty";

/// What a lost job's absence means; see [`REASON_LOST`] for when it is recorded.
const LOST_MESSAGE: &str = "the server has neither a history entry nor a queue place for it";

impl RunFailure {
    fn of(job: usize, prompt_id: &str, state: &JobState) -> Option<RunFailure> {
        let (reason, node, class, message) = match state {
            JobState::Queued => return None,
            JobState::Finished(entry) => match &entry.error {
                Some(error) => (
                    REASON_ERROR,
                    error.node.clone(),
                    error.class.clone(),
                    error.message.clone(),
                ),
                None if entry.outputs.is_empty() => (
                    REASON_EMPTY,
                    None,
                    None,
                    format!("finished as `{}` and wrote no file", entry.status),
                ),
                None => return None,
            },
            JobState::Lost => (REASON_LOST, None, None, LOST_MESSAGE.to_string()),
        };
        Some(RunFailure {
            job: job as u32,
            prompt_id: prompt_id.to_string(),
            reason: reason.to_string(),
            node,
            class,
            message,
        })
    }

    /// What went wrong, in one line: the failing node and the server's message, or why the job is
    /// lost.
    pub fn summary(&self) -> String {
        match (&self.node, &self.class) {
            (Some(node), Some(class)) => format!("node {node} ({class}): {}", self.message),
            (Some(node), None) => format!("node {node}: {}", self.message),
            _ => self.message.clone(),
        }
    }

    /// The [`code::GENERATION_RUN_FAILED`] for this job; `seed` is [`RunReport::job_seed`] and
    /// `recipe` is the report's, which a lost job's help names.
    pub fn diagnostic(&self, seed: Option<u64>, recipe: &str) -> Diagnostic {
        let seed = seed_text(seed);
        if self.reason == REASON_LOST {
            return Diagnostic::new(
                code::GENERATION_RUN_FAILED,
                format!(
                    "job {} (`{}`) is lost: {}",
                    self.job, self.prompt_id, self.message
                ),
            )
            .with_help(format!(
                "a ComfyUI restart forgets every job it held; `ludion gen run {recipe}` submits it \
                 again"
            ));
        }
        if self.reason == REASON_EMPTY {
            return Diagnostic::new(
                code::GENERATION_RUN_FAILED,
                format!(
                    "job {} (`{}`, seed {seed}) wrote no file: {}",
                    self.job, self.prompt_id, self.message
                ),
            )
            .with_help(
                "the graph's output node may be muted or bypassed in the exported API graph; \
                 re-export it with the node active",
            );
        }
        Diagnostic::new(
            code::GENERATION_RUN_FAILED,
            format!(
                "job {} (`{}`, seed {seed}) failed on the server: {}",
                self.job,
                self.prompt_id,
                self.summary()
            ),
        )
        .with_help(
            "the server's console log holds the traceback; `ludion gen check <recipe>` re-runs the \
             input validation against the same server offline of a run",
        )
    }
}

/// One image the run uploaded: the recipe's `images:` entry as resolved for *this* run, `--image`
/// override included. The hash is of the bytes that went up, which is what makes a later edit to the
/// source image detectable from an accepted asset's provenance.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunImage {
    /// The recipe's name for the image, which an `image:` override reference spells.
    pub name: String,
    /// The file, relative to the project root.
    pub path: String,
    /// A content hash of the uploaded bytes.
    pub hash: String,
    /// The overrides that feed it to the graph. Defaulted on read, so a report written before the
    /// field existed still parses.
    #[serde(default)]
    pub bound_to: Vec<RunBinding>,
}

/// One override that binds an image: the node (by `_meta.title`) and the input it sets.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunBinding {
    /// The node's `_meta.title`.
    pub node: String,
    /// The input name on that node.
    pub input: String,
}

/// `run.json`. Field order is the file's key order — the canonical-writer discipline applied to a
/// scratch report, so two runs diff line by line.
///
/// **The server address is not a field**, deliberately: a machine address is machine state and
/// would be wrong on every other machine, and this file is read by whoever picks the run up next.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunReport {
    /// `<UTC compact timestamp>-<8 hex of the graph hash>`.
    pub run_id: String,
    /// The recipe, relative to the project root.
    pub recipe: String,
    /// A content hash of the recipe file.
    pub recipe_hash: String,
    /// The workflow, relative to the project root.
    pub workflow: String,
    /// A content hash of the workflow file.
    pub workflow_hash: String,
    /// A content hash of the resolved graph at the base seed.
    pub graph_hash: String,
    /// The base seed the run asked for; job `i` was asked to run at `seed_base + i`.
    pub seed_base: u64,
    /// How many graphs were submitted.
    pub count: u32,
    /// `image` or `mesh`, read from the graph's output nodes.
    pub kind: String,
    /// The server's version string.
    pub comfy_version: String,
    /// Every model checkpoint the resolved graph loaded.
    pub checkpoints: Vec<String>,
    /// Every image the run uploaded. Defaulted on read, so a report written before the field
    /// existed still parses.
    #[serde(default)]
    pub images: Vec<RunImage>,
    /// The prompt text the submitted graph carried ([`prompt_text`]); what `gen accept` records.
    /// `None` only in a report written before the snapshot existed.
    #[serde(default)]
    pub prompt: Option<String>,
    /// The recipe's `inputs:` as the run read them ([`inputs_to_json`]); what `gen accept` records.
    /// `None` only in a report written before the snapshot existed.
    #[serde(default)]
    pub inputs: Option<serde_json::Value>,
    /// The job ids, in submit order.
    pub prompt_ids: Vec<String>,
    /// The seed each job ran at, in submit order; `None` where it could not be known. Defaulted on
    /// read, so a report written before the field existed still parses.
    #[serde(default)]
    pub seeds: Vec<Option<u64>>,
    /// The downloaded candidates.
    pub outputs: Vec<RunOutput>,
    /// The jobs that produced none. Defaulted on read, so a report written before the field
    /// existed still parses.
    #[serde(default)]
    pub failed: Vec<RunFailure>,
    /// UTC ISO-8601 instant the run was submitted.
    pub started: String,
    /// UTC ISO-8601 instant the run finished, empty while it has not.
    pub finished: String,
    /// Wall time from submit to finish.
    pub duration_ms: u64,
    /// `submitted` while the jobs are on the server or still being submitted; once every job has a verdict, `ok` (no
    /// failure), `partial` (some candidates, some failures) or `failed` (no candidates).
    pub status: String,
    /// The folder this report was read from or written to. Not a field of the file: the folder's
    /// own path is not a fact the file should be able to disagree with.
    #[serde(skip)]
    pub dir: PathBuf,
}

/// `status: "submitted"` — the jobs are on the server (or still being submitted) and nothing has
/// been downloaded.
pub const STATUS_SUBMITTED: &str = "submitted";

/// `status: "ok"` — every output is downloaded and the report is complete.
pub const STATUS_OK: &str = "ok";

/// `status: "partial"` — some jobs failed or were lost, the rest are downloaded.
pub const STATUS_PARTIAL: &str = "partial";

/// `status: "failed"` — every job failed or was lost; nothing was downloaded.
pub const STATUS_FAILED: &str = "failed";

impl RunReport {
    /// One [`code::GENERATION_RUN_FAILED`] per failed or lost job.
    pub fn failure_diagnostics(&self) -> Vec<Diagnostic> {
        self.failed
            .iter()
            .map(|failure| failure.diagnostic(self.job_seed(failure.job as usize), &self.recipe))
            .collect()
    }

    /// The seed job `job` ran at. A report written before `seeds` existed has none recorded, and
    /// every job in it ran at `seed_base + job`.
    pub fn job_seed(&self, job: usize) -> Option<u64> {
        if self.seeds.is_empty() {
            return Some(self.seed_base.wrapping_add(job as u64));
        }
        self.seeds.get(job).copied().flatten()
    }
}

/// A seed as human text: the number, or `?` when the run could not know it.
pub fn seed_text(seed: Option<u64>) -> String {
    seed.map_or_else(|| "?".to_string(), |seed| seed.to_string())
}

/// The [`code::GENERATION_IDENTICAL_CANDIDATES`] warning when a run of `count` jobs would submit
/// the same graph every time: no seed input takes the run's seed. Offline; `None` for a run that
/// varies, a single job, or a recipe whose overrides do not apply (which `gen check` reports).
pub fn identical_candidates(resolved: &Resolved, count: u32) -> Option<Diagnostic> {
    if count <= 1 {
        return None;
    }
    let placeholders: BTreeMap<String, String> = resolved
        .doc
        .images
        .iter()
        .map(|(name, _)| (name.clone(), name.clone()))
        .collect();
    let applied = apply(&resolved.graph, &resolved.doc, &placeholders).ok()?;
    let cause = match with_seed(&applied, 0).1 {
        SeedUse::Written(_) => return None,
        SeedUse::Pinned(seed) => format!("the recipe sets every seed input to {seed}"),
        SeedUse::Unknown => "no seed input takes the run's seed".to_string(),
    };
    Some(
        Diagnostic::new(
            code::GENERATION_IDENTICAL_CANDIDATES,
            format!("a run of {count} candidates would submit identical graphs: {cause}"),
        )
        .with_help(
            "drop the recipe's seed pin, expose a literal `seed` or `noise_seed` input on the \
             sampler, or run with `--count 1`",
        ),
    )
}

/// What `gen status` reports: the run's own record plus where the server says its jobs are.
#[derive(Clone, Debug, PartialEq)]
pub struct RunStatus {
    /// The run folder's `run.json`.
    pub report: RunReport,
    /// Whether every job has a verdict, so `gen fetch` will complete the run.
    pub ready: bool,
    /// The jobs still queued on the server, in submit order.
    pub pending: Vec<String>,
    /// The jobs that failed or are lost, in submit order.
    pub failed: Vec<RunFailure>,
    /// The server's `/queue`, verbatim; `null` for a finalized run, which asks the server nothing.
    pub queue: serde_json::Value,
}

/// Runs a resolved recipe: upload, submit `count` graphs at `seed`, `seed + 1`, …, follow them,
/// download every output, and write `run.json`.
///
/// `run.json` is written as `status: "submitted"` before the first job is posted, again after every
/// accepted job, and complete at the end — so a run that times out or is interrupted still leaves a
/// folder [`status`] and [`fetch`] can pick up. A kill between an accepted POST and its write loses
/// that one job. A job that fails or is lost is an outcome, not an error: the report is `partial` or
/// `failed` and names it in [`RunReport::failed`].
///
/// # Errors
/// Everything the live validation pass reports (`L1164`, `L1165`, `L1166`), the graph's output-kind
/// verdict (`L1171`), and the client's failure taxonomy (`L1167`, `L1168`, `L1170`). A failure
/// before the first job is accepted removes the folder; any later one keeps it, and names
/// `gen fetch <dir>`.
pub fn run(resolved: &Resolved, opts: &RunOptions) -> Result<RunReport, RunError> {
    let count = candidate_count(opts, &resolved.doc).map_err(one)?;
    let unbound = crate::workflow::unbound_images(&resolved.doc);
    if !unbound.is_empty() {
        return Err(unbound.into());
    }
    let client = Client::new(&opts.url);
    let stats = client.system_stats().map_err(one)?;
    let object_info = client.object_info().map_err(one)?;

    let kind = resolved.graph.output_kind().map_err(one)?;
    let diagnostics = crate::workflow::validate(&resolved.graph, &resolved.doc, &object_info);
    if diagnostics.iter().any(|d| d.severity.is_error()) {
        return Err(diagnostics.into());
    }

    let root = resolved.project_root();
    let images = image_sources(resolved, opts.image.as_deref())?;
    let mut uploaded = BTreeMap::new();
    let mut image_records = Vec::with_capacity(images.len());
    let mut kept = Vec::with_capacity(images.len());
    for (name, path) in &images {
        let bytes = std::fs::read(path).map_err(|error| vec![unreadable_image(path, &error)])?;
        image_records.push(image_record(&root, &resolved.doc, name, path, &bytes));
        let upload = upload_name(path, &bytes);
        uploaded.insert(
            name.clone(),
            client.upload_image(&upload, &bytes).map_err(one)?,
        );
        kept.push((upload, bytes));
    }

    let seed_base = opts.seed.unwrap_or(resolved.doc.seed);
    let base_graph = apply(&resolved.graph, &resolved.doc, &uploaded)?;
    let hash = graph_hash(&with_seed(&base_graph, seed_base).0);
    let workflow = relative_to(&root, &resolved.workflow_path)
        .ok_or_else(|| vec![workflow_outside_root(&resolved.doc.workflow)])?;
    let stamp = Stamp::now();
    let (dir, run_id) = create_run_dir(
        &root.join(RUNS_DIR).join(sanitize(&resolved.name())),
        &format!("{}-{}", stamp.compact, &hash[..8]),
    )?;
    // Until the first job is accepted nothing is on the server, so a folder this call created has
    // nothing to recover and every failure in between removes it.
    let discard = |diagnostics: Vec<Diagnostic>| {
        let _ = std::fs::remove_dir_all(&dir);
        RunError::from(diagnostics)
    };

    // Each source under the name it was uploaded as, so two with one file name stay two files.
    let inputs_dir = dir.join("inputs");
    if !kept.is_empty() {
        create_dir(&inputs_dir).map_err(discard)?;
        for (name, bytes) in &kept {
            write_atomic(&inputs_dir.join(name), bytes)
                .map_err(|e| discard(vec![write_failed(&inputs_dir, e)]))?;
        }
    }

    let client_id = run_id.clone();
    // ComfyUI never replays frames to a late socket, and a cached job finishes before one opens.
    let subscription = opts.wait.then(|| client.subscribe(&client_id));

    let started = Instant::now();
    let mut report = RunReport {
        run_id,
        recipe: project_relative(&root, &resolved.recipe_path),
        recipe_hash: resolved.recipe_hash.clone(),
        workflow,
        workflow_hash: resolved.workflow_hash.clone(),
        graph_hash: hash,
        seed_base,
        count: 0,
        kind: kind.as_str().to_string(),
        comfy_version: stats.comfy_version,
        checkpoints: checkpoints(&base_graph),
        images: image_records,
        prompt: Some(prompt_text(&base_graph)),
        inputs: Some(inputs_to_json(&resolved.doc.inputs)),
        prompt_ids: Vec::with_capacity(count as usize),
        seeds: Vec::with_capacity(count as usize),
        outputs: Vec::new(),
        failed: Vec::new(),
        started: stamp.iso.clone(),
        finished: String::new(),
        duration_ms: 0,
        status: STATUS_SUBMITTED.to_string(),
        dir: dir.clone(),
    };
    write_report(&report).map_err(discard)?;

    // The report is rewritten after every accepted job, so a run killed part-way still lists the
    // jobs the server holds. A kill between an accepted POST and its write loses that one job.
    let mut refused = None;
    for index in 0..count {
        let (graph, used) = with_seed(&base_graph, seed_base.wrapping_add(u64::from(index)));
        match client.submit(&graph.to_value(), &client_id) {
            Ok(prompt_id) => {
                report.seeds.push(used.seed());
                report.prompt_ids.push(prompt_id.clone());
                report.count = report.prompt_ids.len() as u32;
                if let Err(mut diagnostics) = write_report(&report) {
                    let help = format!(
                        "job `{prompt_id}` was accepted by the server but is not recorded in \
                         `{REPORT_FILE}`; it is still queued there"
                    );
                    if let Some(first) = diagnostics.first_mut() {
                        *first = first.clone().with_help(match &first.help {
                            Some(existing) => format!("{existing}; {help}"),
                            None => help,
                        });
                    }
                    return Err(RunError {
                        diagnostics,
                        dir: Some(dir.clone()),
                    });
                }
            }
            Err(diagnostic) => {
                refused = Some(diagnostic);
                break;
            }
        }
    }

    if let Some(diagnostic) = refused {
        if report.prompt_ids.is_empty() {
            return Err(discard(one(diagnostic)));
        }
        let help = format!(
            "{} job(s) submitted before it are recorded in `{}`; `ludion gen fetch {}` collects \
             them",
            report.prompt_ids.len(),
            display_path(&report.dir),
            display_path(&report.dir)
        );
        let help = match &diagnostic.help {
            Some(existing) => format!("{existing}; {help}"),
            None => help,
        };
        return Err(RunError {
            diagnostics: vec![diagnostic.with_help(help)],
            dir: Some(dir),
        });
    }

    let Some(subscription) = subscription else {
        return Ok(report);
    };

    let phase_error = |diagnostics: Vec<Diagnostic>| RunError {
        diagnostics: diagnostics
            .into_iter()
            .map(|diagnostic| follow_phase_error(diagnostic, &dir))
            .collect(),
        dir: Some(dir.clone()),
    };
    client
        .follow(subscription, &report.prompt_ids, opts.timeout)
        .map_err(|diagnostic| phase_error(vec![diagnostic]))?;
    // A waited run follows on the URL it just submitted to, so an all-lost verdict here is
    // finalized `failed`, never the wrong-server refusal `fetch` makes.
    let (states, _) = client
        .job_states(&report.prompt_ids)
        .map_err(|diagnostic| phase_error(vec![diagnostic]))?;
    complete(&client, &mut report, &states, started).map_err(phase_error)?;
    Ok(report)
}

/// Why [`run`] returned no report. `dir` is the run folder once its `run.json` is on disk.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RunError {
    /// Everything the run reported.
    pub diagnostics: Vec<Diagnostic>,
    /// The run folder, once the server holds a job of the run.
    pub dir: Option<PathBuf>,
}

impl From<Vec<Diagnostic>> for RunError {
    fn from(diagnostics: Vec<Diagnostic>) -> Self {
        RunError {
            diagnostics,
            dir: None,
        }
    }
}

/// Reads a run folder's `run.json` and asks the server where its jobs are. A finalized run is
/// final: its record is reported as written, since after a server restart every job would read
/// lost.
///
/// # Errors
/// [`code::GENERATION_ACCEPT_REFUSED`] when the folder has no readable report,
/// [`code::GENERATION_JOBS_UNKNOWN`] when the server knows none of the run's jobs (or it has none),
/// plus the client's failure taxonomy.
pub fn status(run_dir: &Path, url: &str) -> Result<RunStatus, Vec<Diagnostic>> {
    let report = read_report(run_dir)?;
    if report.status != STATUS_SUBMITTED {
        return Ok(RunStatus {
            ready: true,
            pending: Vec::new(),
            failed: report.failed.clone(),
            queue: serde_json::Value::Null,
            report,
        });
    }
    if report.prompt_ids.is_empty() {
        return Err(one(no_jobs_accepted(&report)));
    }
    let client = Client::new(url);
    let (states, queue) = client.job_states(&report.prompt_ids).map_err(one)?;
    if let Some(diagnostic) = server_knows_none(&report, &states) {
        return Err(one(diagnostic));
    }
    let mut pending = Vec::new();
    let mut failed = Vec::new();
    for (job, (id, state)) in report.prompt_ids.iter().zip(&states).enumerate() {
        match state {
            JobState::Queued => pending.push(id.clone()),
            _ => failed.extend(RunFailure::of(job, id, state)),
        }
    }
    Ok(RunStatus {
        ready: pending.is_empty(),
        pending,
        failed,
        queue,
        report,
    })
}

/// Completes a `--no-wait` run: downloads every finished job's outputs, records the failed and lost
/// ones, and finalizes `run.json`. A run that is no longer `submitted` is returned untouched.
///
/// # Errors
/// [`code::GENERATION_ACCEPT_REFUSED`] when the folder has no readable report,
/// [`code::GENERATION_RUN_TIMED_OUT`] when a job is still queued,
/// [`code::GENERATION_JOBS_UNKNOWN`] when the server knows none of the run's jobs (or it has none;
/// the run stays `submitted`), plus the client's failure taxonomy.
pub fn fetch(run_dir: &Path, url: &str) -> Result<RunReport, Vec<Diagnostic>> {
    let mut report = read_report(run_dir)?;
    if report.status != STATUS_SUBMITTED {
        return Ok(report);
    }
    if report.prompt_ids.is_empty() {
        return Err(one(no_jobs_accepted(&report)));
    }
    let client = Client::new(url);
    let (states, _) = client.job_states(&report.prompt_ids).map_err(one)?;
    if let Some(diagnostic) = server_knows_none(&report, &states) {
        return Err(one(diagnostic));
    }
    complete(&client, &mut report, &states, Instant::now())?;
    Ok(report)
}

/// Reads a run folder's `run.json`.
///
/// # Errors
/// [`code::GENERATION_ACCEPT_REFUSED`] when the folder is missing, holds no report, or holds one
/// that will not parse.
pub fn read_report(run_dir: &Path) -> Result<RunReport, Vec<Diagnostic>> {
    let path = run_dir.join(REPORT_FILE);
    let source = std::fs::read_to_string(&path).map_err(|e| {
        vec![
            Diagnostic::new(
                code::GENERATION_ACCEPT_REFUSED,
                format!("could not read `{}`: {e}", path.display()),
            )
            .with_help(
                "a run folder is `generation/runs/<recipe>/<run-id>/`, and `run.json` is written \
                 before its first job is submitted",
            ),
        ]
    })?;
    let mut report: RunReport = serde_json::from_str(&source).map_err(|e| {
        vec![
            Diagnostic::new(
                code::GENERATION_ACCEPT_REFUSED,
                format!("`{}` is not a run report: {e}", path.display()),
            )
            .with_help("delete the run folder and submit the recipe again"),
        ]
    })?;
    report.dir = run_dir.to_path_buf();
    Ok(report)
}

/// The text every positive-side conditioning encoder is fed, joined in node-id order — the one
/// field of a provenance block a human actually reads a year later.
///
/// The positive side is every node reachable upstream from the source of any input named
/// `positive`, following every link except inputs named `negative`: `FluxGuidance`,
/// `ConditioningCombine` and `ControlNetApplyAdvanced` chains count, and a negative encoder routed
/// through a node that also has a positive output does not. A graph with no `positive` input at all
/// (Flux's `BasicGuider`) has no negative side to exclude, so every encoder counts. An encoder is a
/// node whose class contains `TextEncode`; its text is the string inputs `text`, `prompt`, `text_g`
/// and `text_l`, trimmed, non-empty, a value repeated within one node taken once. A prompt fed
/// through a link (a primitive string node) is not recorded.
pub fn prompt_text(graph: &Graph) -> String {
    let positive = positive_side(graph);
    let mut parts = Vec::new();
    for node in graph.nodes() {
        if !node.class_type.contains("TextEncode")
            || positive.as_ref().is_some_and(|set| !set.contains(&node.id))
        {
            continue;
        }
        let mut taken: Vec<&str> = Vec::new();
        for name in PROMPT_INPUTS {
            if let Some(text) = node.inputs.get(name).and_then(serde_json::Value::as_str) {
                let text = text.trim();
                if !text.is_empty() && !taken.contains(&text) {
                    taken.push(text);
                    parts.push(text.to_string());
                }
            }
        }
    }
    parts.join("\n")
}

/// The string inputs a text encoder carries its prompt in, in the order they are recorded.
const PROMPT_INPUTS: [&str; 4] = ["text", "prompt", "text_g", "text_l"];

/// The node ids upstream of any `positive` input, or `None` when the graph has no such input.
fn positive_side(graph: &Graph) -> Option<BTreeSet<String>> {
    let mut pending: Vec<&str> = Vec::new();
    let mut any = false;
    for node in graph.nodes() {
        if let Some(value) = node.inputs.get("positive") {
            any = true;
            pending.extend(link_source(value));
        }
    }
    if !any {
        return None;
    }
    let mut reached = BTreeSet::new();
    while let Some(id) = pending.pop() {
        if !reached.insert(id.to_string()) {
            continue;
        }
        if let Some(node) = graph.node(id) {
            for (name, value) in &node.inputs {
                if name != "negative" {
                    pending.extend(link_source(value));
                }
            }
        }
    }
    Some(reached)
}

/// The node id a `[node_id, slot]` link names.
fn link_source(value: &serde_json::Value) -> Option<&str> {
    match value.as_array()?.as_slice() {
        [serde_json::Value::String(id), slot] if slot.is_u64() => Some(id),
        _ => None,
    }
}

/// File extensions that make a `*_name` input a model file rather than a sampler or config name.
const MODEL_EXTENSIONS: [&str; 6] = [".safetensors", ".ckpt", ".pt", ".pth", ".bin", ".gguf"];

/// Every model file the resolved graph loads: the string value of any `*_name` input on any node
/// that ends in a model extension (`config_name: v1.yaml` and `sampler_name: euler` are not models),
/// in node-id order, deduplicated. These are public facts — a committed workflow names them and
/// provenance records them.
pub fn checkpoints(graph: &Graph) -> Vec<String> {
    let mut names: Vec<String> = Vec::new();
    for node in graph.nodes() {
        for (input, value) in &node.inputs {
            if !input.ends_with("_name") {
                continue;
            }
            if let Some(text) = value.as_str()
                && is_model_file(text)
                && !names.iter().any(|seen| seen == text)
            {
                names.push(text.to_string());
            }
        }
    }
    names
}

fn is_model_file(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    MODEL_EXTENSIONS.iter().any(|ext| lower.ends_with(ext))
}

/// A recipe's `inputs:` — `[(node title, [(input name, value)])]`.
pub type InputTable = Vec<(String, Vec<(String, InputValue)>)>;

/// An [`InputTable`] as `{title: {input: value}}` — the shape a recipe's `inputs:`,
/// `run.json`'s snapshot and an accepted asset's `origin.inputs` all render as. Both levels are
/// sorted by key.
pub fn inputs_to_json(inputs: &[(String, Vec<(String, InputValue)>)]) -> serde_json::Value {
    let mut titles: Vec<&(String, Vec<(String, InputValue)>)> = inputs.iter().collect();
    titles.sort_by(|a, b| a.0.cmp(&b.0));
    let mut out = serde_json::Map::new();
    for (title, entries) in titles {
        let mut entries: Vec<&(String, InputValue)> = entries.iter().collect();
        entries.sort_by(|a, b| a.0.cmp(&b.0));
        let mut fields = serde_json::Map::new();
        for (input, value) in entries {
            fields.insert(input.clone(), input_value_to_json(value));
        }
        out.insert(title.clone(), serde_json::Value::Object(fields));
    }
    serde_json::Value::Object(out)
}

fn input_value_to_json(value: &InputValue) -> serde_json::Value {
    use serde_json::Value;
    match value {
        InputValue::Scalar(ScalarValue::Bool(b)) => Value::from(*b),
        InputValue::Scalar(ScalarValue::Integer(i)) => Value::from(*i),
        InputValue::Scalar(ScalarValue::Number(n)) => {
            serde_json::Number::from_f64(*n).map_or(Value::Null, Value::Number)
        }
        InputValue::Scalar(ScalarValue::Text(text)) => Value::from(text.clone()),
        InputValue::ImageRef(name) => serde_json::json!({ "image": name }),
    }
}

/// The inverse of [`inputs_to_json`], for the snapshot `run.json` carries.
///
/// # Errors
/// A message naming the title and input of the first value that is not a bool, number, string or
/// `{"image": "<name>"}`.
pub fn inputs_from_json(value: &serde_json::Value) -> Result<InputTable, String> {
    use serde_json::Value;
    let Value::Object(titles) = value else {
        return Err("the inputs snapshot is not an object of nodes".to_string());
    };
    let mut out = Vec::with_capacity(titles.len());
    for (title, fields) in titles {
        let Value::Object(fields) = fields else {
            return Err(format!("`{title}` is not an object of inputs"));
        };
        let mut entries = Vec::with_capacity(fields.len());
        for (input, field) in fields {
            let parsed = match field {
                Value::Bool(b) => InputValue::Scalar(ScalarValue::Bool(*b)),
                Value::Number(n) => match (n.as_i64(), n.as_f64()) {
                    (Some(i), _) => InputValue::Scalar(ScalarValue::Integer(i)),
                    (None, Some(f)) => InputValue::Scalar(ScalarValue::Number(f)),
                    (None, None) => {
                        return Err(format!("`{title}`.`{input}` is not a usable number"));
                    }
                },
                Value::String(text) => InputValue::Scalar(ScalarValue::Text(text.clone())),
                Value::Object(map) if map.len() == 1 => match map.get("image") {
                    Some(Value::String(name)) => InputValue::ImageRef(name.clone()),
                    _ => return Err(format!("`{title}`.`{input}` is not a recognised value")),
                },
                _ => return Err(format!("`{title}`.`{input}` is not a recognised value")),
            };
            entries.push((input.clone(), parsed));
        }
        entries.sort_by(|a, b| a.0.cmp(&b.0));
        out.push((title.clone(), entries));
    }
    out.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(out)
}

/// The refusal for a `submitted` run that lists no job: its submit was interrupted before the
/// server accepted one, so there is nothing to collect.
fn no_jobs_accepted(report: &RunReport) -> Diagnostic {
    Diagnostic::new(
        code::GENERATION_JOBS_UNKNOWN,
        format!(
            "run `{}` lists no jobs: its submit was interrupted before the server accepted one",
            report.run_id
        ),
    )
    .with_help(format!(
        "there is nothing to collect; delete `{}` and run `ludion gen run {}` again",
        display_path(&report.dir),
        report.recipe
    ))
}

/// The refusal for a `submitted` run whose jobs the server all reads as lost (a wrong `--url`
/// reads the same as a restart). The run stays `submitted`, so the right server still collects it.
/// `None` when at least one job is known.
fn server_knows_none(report: &RunReport, states: &[JobState]) -> Option<Diagnostic> {
    if !states.iter().all(|state| *state == JobState::Lost) {
        return None;
    }
    Some(
        Diagnostic::new(
            code::GENERATION_JOBS_UNKNOWN,
            format!(
                "this server knows none of run `{}`'s {} jobs",
                report.run_id,
                report.prompt_ids.len()
            ),
        )
        .with_help(format!(
            "is `--url` / `LUDION_COMFY_URL` the server that took the run? `{REPORT_FILE}` keeps the \
             run `submitted`, so a fetch against the right server still collects it; if the server \
             restarted or its history was cleared, `ludion gen run {}` submits it again",
            report.recipe
        )),
    )
}

/// Gives every job its verdict: downloads a finished job's outputs into the run folder, records a
/// failed or lost one, and finalizes the report. `states` is the server's answer for the report's
/// jobs, in order.
fn complete(
    client: &Client,
    report: &mut RunReport,
    states: &[JobState],
    started: Instant,
) -> Result<(), Vec<Diagnostic>> {
    let queued: Vec<&str> = report
        .prompt_ids
        .iter()
        .zip(states)
        .filter(|(_, state)| **state == JobState::Queued)
        .map(|(id, _)| id.as_str())
        .collect();
    if !queued.is_empty() {
        return Err(vec![
            Diagnostic::new(
                code::GENERATION_RUN_TIMED_OUT,
                format!(
                    "{} of run `{}`'s jobs have not finished: {}",
                    queued.len(),
                    report.run_id,
                    queued.join(", ")
                ),
            )
            .with_help(format!(
                "`ludion gen status {}` says where they are in the queue; fetch again once they \
                 finish",
                display_path(&report.dir)
            )),
        ]);
    }

    let mut outputs = Vec::new();
    let mut failed = Vec::new();
    let mut index = 0u32;
    for (job, (prompt_id, state)) in report.prompt_ids.iter().zip(states).enumerate() {
        let entry = match state {
            JobState::Finished(entry) if entry.error.is_none() && !entry.outputs.is_empty() => {
                entry
            }
            _ => {
                failed.extend(RunFailure::of(job, prompt_id, state));
                continue;
            }
        };
        let seed = report.job_seed(job);
        for file in &entry.outputs {
            let bytes = client
                .view(&file.filename, &file.subfolder, &file.folder_type)
                .map_err(one)?;
            let name = format!("{index:04}.{}", extension(file));
            write_atomic(&report.dir.join(&name), &bytes)
                .map_err(|e| vec![write_failed(&report.dir, e)])?;
            outputs.push(RunOutput {
                index,
                seed,
                file: name,
                node: file.node.clone(),
                batch_index: file.batch_index,
                prompt_id: prompt_id.clone(),
            });
            index += 1;
        }
    }

    let stamp = Stamp::now();
    report.status = if failed.is_empty() {
        STATUS_OK
    } else if outputs.is_empty() {
        STATUS_FAILED
    } else {
        STATUS_PARTIAL
    }
    .to_string();
    report.outputs = outputs;
    report.failed = failed;
    report.finished = stamp.iso;
    report.duration_ms = started.elapsed().as_millis().min(u128::from(u64::MAX)) as u64;
    write_report(report)
}

/// Writes `run.json`: the struct's field order is the file's key order, and the file ends in a
/// newline like every other text this workspace writes.
fn write_report(report: &RunReport) -> Result<(), Vec<Diagnostic>> {
    let mut json = serde_json::to_string_pretty(report).map_err(|e| {
        vec![Diagnostic::new(
            code::OUTPUT_IO,
            format!("could not encode `{REPORT_FILE}`: {e}"),
        )]
    })?;
    json.push('\n');
    write_atomic(&report.dir.join(REPORT_FILE), json)
        .map_err(|e| vec![write_failed(&report.dir, e)])
}

/// What `run.json` records about one uploaded image, read from the file that is about to go up.
fn image_record(
    root: &Path,
    doc: &GenerationDocument,
    name: &str,
    path: &Path,
    bytes: &[u8],
) -> RunImage {
    RunImage {
        name: name.to_string(),
        path: project_relative(root, path),
        hash: content_hash(bytes),
        bound_to: crate::workflow::image_bindings(doc, name)
            .into_iter()
            .map(|(node, input)| RunBinding { node, input })
            .collect(),
    }
}

/// Every path a `gen` verb prints, text or `--json`, in forward-slash spelling on every OS. A path
/// built from a project-relative string joined against a platform path (a run folder, a candidate
/// file) mixes `/` and `\` on Windows; a reader should never have to notice which. `run.json`'s own
/// `recipe`/`workflow` fields and a provenance block's are unaffected — [`relative_to`] already
/// renders those in forward-slash spelling, and this only ever touches what a verb prints.
pub fn display_path(path: &Path) -> String {
    path.display().to_string().replace('\\', "/")
}

/// A follow-phase error with a help that names the run folder `gen status` and `gen fetch` take,
/// which the client, knowing only job ids, cannot. [`code::GENERATION_RUN_TIMED_OUT`] already says
/// so and gets its `<run-dir>` filled in; any other code gains the pointer, its own help kept.
fn follow_phase_error(diagnostic: Diagnostic, dir: &Path) -> Diagnostic {
    let dir = display_path(dir);
    if diagnostic.code == code::GENERATION_RUN_TIMED_OUT {
        return match diagnostic
            .help
            .as_deref()
            .map(|help| help.replace("<run-dir>", &dir))
        {
            Some(help) => diagnostic.with_help(help),
            None => diagnostic,
        };
    }
    let pointer = format!("the run is recorded; `ludion gen fetch {dir}` collects it");
    let help = match &diagnostic.help {
        Some(existing) => format!("{existing}; {pointer}"),
        None => pointer,
    };
    diagnostic.with_help(help)
}

fn unreadable_image(path: &Path, error: &std::io::Error) -> Diagnostic {
    Diagnostic::new(
        code::GENERATION_DATA,
        format!("could not read the image `{}`: {error}", path.display()),
    )
    .with_help("an `images:` entry names a committed file; `--image` takes its path as given")
}

/// A recorded path: project-relative forward-slash when the file sits under the root, else its bare
/// file name. A `--image` scratch candidate may live anywhere, and an absolute machine path is the
/// one spelling a file that gets committed or pasted must never carry.
pub(crate) fn project_relative(root: &Path, path: &Path) -> String {
    relative_to(root, path).unwrap_or_else(|| {
        path.file_name()
            .map(|name| name.to_string_lossy().to_string())
            .unwrap_or_default()
    })
}

/// The declared `images:` entries, with `--image` replacing the first one.
fn image_sources(
    resolved: &Resolved,
    override_path: Option<&Path>,
) -> Result<Vec<(String, PathBuf)>, Vec<Diagnostic>> {
    let mut images = resolved.images.clone();
    let Some(path) = override_path else {
        return Ok(images);
    };
    if images.is_empty() {
        return Err(vec![
            Diagnostic::new(
                code::GENERATION_DATA,
                format!(
                    "`--image {}` was given, and the recipe declares no `images:` entry to replace",
                    path.display()
                ),
            )
            .with_help(
                "add an `images:` entry naming a committed image and an override feeding it to a \
                 `LoadImage` node; `--image` then swaps that entry's file for a scratch one",
            ),
        ]);
    }
    if !path.is_file() {
        return Err(vec![
            Diagnostic::new(
                code::GENERATION_DATA,
                format!("`--image {}` names no file", path.display()),
            )
            .with_help("a `--image` path is taken as given, not relative to the recipe"),
        ]);
    }
    images[0].1 = path.to_path_buf();
    Ok(images)
}

/// The extension a downloaded output keeps: the server's own when it is 1 to 8 ASCII alphanumeric
/// characters — the name came from the server and becomes a local file name — else the output
/// family's default.
fn extension(file: &OutputFile) -> String {
    Path::new(&file.filename)
        .extension()
        .map(|ext| ext.to_string_lossy().to_string())
        .filter(|ext| {
            (1..=8).contains(&ext.len()) && ext.chars().all(|c| c.is_ascii_alphanumeric())
        })
        .unwrap_or_else(|| {
            match file.kind {
                OutputKind::Image => "png",
                OutputKind::Mesh => "glb",
            }
            .to_string()
        })
}

/// A recipe name reaches the filesystem as a folder, so anything a path separator could read is
/// folded to `-`; a name of only dots, or a Windows device name, cannot come out of it.
fn sanitize(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.' {
                c
            } else {
                '-'
            }
        })
        .collect();
    if cleaned.trim_matches(['-', '.']).is_empty() {
        "recipe".to_string()
    } else {
        ludion_core::paths::unreserved_stem(&cleaned)
    }
}

fn create_dir(path: &Path) -> Result<(), Vec<Diagnostic>> {
    std::fs::create_dir_all(path).map_err(|e| vec![write_failed(path, e)])
}

/// The most run folders one second and one graph can take before the run gives up.
const MAX_RUN_FOLDERS: u32 = 1000;

/// Creates `parent/<base>`, or `parent/<base>-2`, `-3`, … when that folder exists, and returns it
/// with its final name — the run id. `create_dir` is atomic, so two runs of one graph in one
/// second each win a folder of their own.
fn create_run_dir(parent: &Path, base: &str) -> Result<(PathBuf, String), Vec<Diagnostic>> {
    create_dir(parent)?;
    for attempt in 1..=MAX_RUN_FOLDERS {
        let id = if attempt == 1 {
            base.to_string()
        } else {
            format!("{base}-{attempt}")
        };
        let dir = parent.join(&id);
        match std::fs::create_dir(&dir) {
            Ok(()) => return Ok((dir, id)),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(vec![write_failed(&dir, e)]),
        }
    }
    Err(vec![
        Diagnostic::new(
            code::GENERATION_DATA,
            format!(
                "`{}` already holds {MAX_RUN_FOLDERS} runs named `{base}`",
                parent.display()
            ),
        )
        .with_help("a run folder is scratch; delete the old runs of this recipe"),
    ])
}

/// Orders run ids oldest to newest: by the `<stamp>-<hash8>` base, then by the collision attempt
/// numerically, so `…-10` follows `…-9`. A trailing `-<digits>` is an attempt only when a `-`
/// remains in the base; a bare id is attempt 1.
pub fn run_order(a: &str, b: &str) -> Ordering {
    fn split(id: &str) -> (&str, u32) {
        match id.rsplit_once('-') {
            Some((base, n)) if base.contains('-') && !n.is_empty() => {
                match n.bytes().all(|c| c.is_ascii_digit()).then(|| n.parse()) {
                    Some(Ok(attempt)) => (base, attempt),
                    _ => (id, 1),
                }
            }
            _ => (id, 1),
        }
    }
    split(a).cmp(&split(b))
}

fn write_failed(path: &Path, error: std::io::Error) -> Diagnostic {
    Diagnostic::new(
        code::OUTPUT_IO,
        format!("could not write under `{}`: {error}", path.display()),
    )
    .with_help("a run folder is scratch; check the project root is writable and has room")
}

/// How many candidates this run submits: `--count` over the recipe's `count`, at least 1. A
/// `--count` bypasses the document's own validation, so the ceiling is checked here too.
fn candidate_count(opts: &RunOptions, doc: &GenerationDocument) -> Result<u32, Diagnostic> {
    let count = opts.count.unwrap_or(doc.count).max(1);
    if count > GENERATION_MAX_COUNT {
        return Err(Diagnostic::new(
            code::GENERATION_DATA,
            format!(
                "a run asks for {count} candidates, but produces at most {GENERATION_MAX_COUNT}"
            ),
        )
        .with_help(
            "lower `--count` (or the recipe's `count`), or run again with a different `--seed`",
        ));
    }
    Ok(count)
}

fn one(diagnostic: Diagnostic) -> Vec<Diagnostic> {
    vec![diagnostic]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_run_folder_name_is_never_dots_or_a_device() {
        for (name, expected) in [
            ("..", "recipe"),
            (".", "recipe"),
            ("-.-", "recipe"),
            ("con", "con_"),
            ("a/b", "a-b"),
            ("idol.v2", "idol.v2"),
        ] {
            assert_eq!(sanitize(name), expected, "{name:?}");
        }
    }

    const GRAPH: &str = r#"{
      "1": {"class_type": "CheckpointLoaderSimple", "inputs": {"ckpt_name": "klein.safetensors"}},
      "3": {"class_type": "CLIPTextEncode", "inputs": {"text": "a stone idol", "clip": ["1", 1]}},
      "2": {"class_type": "CLIPTextEncode", "inputs": {"text": "  blurry  ", "clip": ["1", 1]}},
      "4": {"class_type": "CLIPTextEncode", "inputs": {"text": "", "clip": ["1", 1]}},
      "5": {"class_type": "VAELoader", "inputs": {"vae_name": "klein-vae.safetensors"}},
      "6": {"class_type": "KSampler", "inputs": {"seed": 7, "positive": ["3", 0], "negative": ["2", 0]}}
    }"#;

    #[test]
    fn a_run_folder_under_a_regular_file_is_an_output_io_failure() {
        let blocker =
            std::env::temp_dir().join(format!("ludion-gen-run-blocker-{}.txt", std::process::id()));
        std::fs::write(&blocker, b"a file, not a folder").expect("the blocker");
        let diagnostics = create_run_dir(&blocker, "x").expect_err("a file cannot hold a folder");
        let _ = std::fs::remove_file(&blocker);
        assert_eq!(diagnostics[0].code, code::OUTPUT_IO);
    }

    #[test]
    fn run_ids_order_by_base_then_numeric_attempt() {
        let base = "20261008T120000Z-1a2b3c4d";
        let older = "20261007T120000Z-ffffffff-30";
        let ids = [
            format!("{base}-9"),
            older.to_string(),
            base.to_string(),
            format!("{base}-10"),
            format!("{base}-2"),
        ];
        let newest = ids.iter().max_by(|a, b| run_order(a, b)).unwrap();
        assert_eq!(newest, &format!("{base}-10"));
        let mut sorted: Vec<&str> = ids.iter().map(String::as_str).collect();
        sorted.sort_by(|a, b| run_order(a, b));
        assert_eq!(
            sorted,
            [
                older,
                base,
                &format!("{base}-2")[..],
                &format!("{base}-9")[..],
                &format!("{base}-10")[..]
            ]
        );
    }

    #[test]
    fn the_candidate_count_is_capped_and_at_least_one() {
        let doc = GenerationDocument {
            count: 4,
            ..GenerationDocument::default()
        };
        let with = |count| RunOptions {
            count,
            ..RunOptions::default()
        };
        assert_eq!(candidate_count(&with(None), &doc).unwrap(), 4);
        assert_eq!(candidate_count(&with(Some(0)), &doc).unwrap(), 1);
        assert_eq!(
            candidate_count(&with(Some(GENERATION_MAX_COUNT)), &doc).unwrap(),
            GENERATION_MAX_COUNT
        );
        let err = candidate_count(&with(Some(GENERATION_MAX_COUNT + 1)), &doc).unwrap_err();
        assert_eq!(err.code, code::GENERATION_DATA);
    }

    #[test]
    fn prompt_text_is_the_positive_encoder_alone() {
        let graph = Graph::parse(GRAPH, "test").expect("the graph parses");
        assert_eq!(prompt_text(&graph), "a stone idol");
    }

    fn prompt_of(source: &str) -> String {
        prompt_text(&Graph::parse(source, "test").expect("the graph parses"))
    }

    #[test]
    fn prompt_text_follows_guidance_and_keeps_a_zeroed_negative_out() {
        let prompt = prompt_of(
            r#"{
              "1": {"class_type": "CLIPTextEncode", "inputs": {"text": "a basalt idol"}},
              "2": {"class_type": "ConditioningZeroOut", "inputs": {"conditioning": ["1", 0]}},
              "3": {"class_type": "FluxGuidance", "inputs": {"conditioning": ["1", 0], "guidance": 3.5}},
              "4": {"class_type": "CFGGuider", "inputs": {"positive": ["3", 0], "negative": ["2", 0]}}
            }"#,
        );
        assert_eq!(prompt, "a basalt idol");
    }

    #[test]
    fn prompt_text_takes_only_the_positive_output_of_a_controlnet_node() {
        let prompt = prompt_of(
            r#"{
              "1": {"class_type": "CLIPTextEncode", "inputs": {"text": "an oak door"}},
              "2": {"class_type": "CLIPTextEncode", "inputs": {"text": "smeared"}},
              "3": {"class_type": "ControlNetApplyAdvanced", "inputs": {"positive": ["1", 0], "negative": ["2", 0]}},
              "4": {"class_type": "KSampler", "inputs": {"positive": ["3", 0], "negative": ["3", 1]}}
            }"#,
        );
        assert_eq!(prompt, "an oak door");
    }

    #[test]
    fn prompt_text_takes_a_repeated_sdxl_text_once_and_a_different_one_twice() {
        let encode = |g: &str, l: &str| {
            format!(
                r#"{{
                  "1": {{"class_type": "CLIPTextEncodeSDXL", "inputs": {{"text_g": "{g}", "text_l": "{l}"}}}},
                  "2": {{"class_type": "KSampler", "inputs": {{"positive": ["1", 0]}}}}
                }}"#
            )
        };
        assert_eq!(prompt_of(&encode("a lamp", "a lamp")), "a lamp");
        assert_eq!(prompt_of(&encode("a lamp", "brass")), "a lamp\nbrass");
    }

    #[test]
    fn prompt_text_without_a_positive_input_counts_every_encoder() {
        let prompt = prompt_of(
            r#"{
              "1": {"class_type": "CLIPTextEncode", "inputs": {"text": "a brass key"}},
              "2": {"class_type": "BasicGuider", "inputs": {"conditioning": ["1", 0]}}
            }"#,
        );
        assert_eq!(prompt, "a brass key");
    }

    #[test]
    fn an_image_under_the_project_root_is_recorded_project_relative() {
        let root = Path::new("/project");
        assert_eq!(
            project_relative(root, &root.join("generation/references/idol.png")),
            "generation/references/idol.png"
        );
    }

    #[test]
    fn an_image_outside_the_project_root_is_recorded_by_file_name_alone() {
        assert_eq!(
            project_relative(
                Path::new("/project"),
                Path::new("/elsewhere/scratch/0003.png")
            ),
            "0003.png",
            "a report is committed or pasted; an absolute machine path never belongs in one"
        );
    }

    #[test]
    fn a_report_written_before_images_existed_still_parses() {
        let source = r#"{
          "run_id": "20260918T101112Z-1a2b3c4d",
          "recipe": "generation/recipes/idol.gen.yaml",
          "recipe_hash": "0000000012345678",
          "workflow": "generation/workflows/txt2img.json",
          "workflow_hash": "00000000deadbeef",
          "graph_hash": "1a2b3c4d5e6f7080",
          "seed_base": 7,
          "count": 1,
          "kind": "image",
          "comfy_version": "0.3.99",
          "checkpoints": [],
          "prompt_ids": [],
          "outputs": [],
          "started": "2026-09-18T10:11:12Z",
          "finished": "",
          "duration_ms": 0,
          "status": "submitted"
        }"#;
        let report: RunReport = serde_json::from_str(source).expect("an older report still reads");
        assert!(report.images.is_empty());
        assert!(report.failed.is_empty());
    }

    #[test]
    fn checkpoints_are_model_file_name_inputs_deduplicated() {
        let graph = Graph::parse(GRAPH, "test").expect("the graph parses");
        assert_eq!(
            checkpoints(&graph),
            vec!["klein.safetensors", "klein-vae.safetensors"],
            "a `*_name` string ending in a model extension, in node-id order — never the server address"
        );
    }

    #[test]
    fn checkpoints_are_any_node_s_model_file_by_extension() {
        let graph = Graph::parse(
            r#"{
              "1": {"class_type": "LoadBackgroundRemovalModel", "inputs": {"bg_removal_name": "birefnet.safetensors"}},
              "2": {"class_type": "KSampler", "inputs": {"sampler_name": "euler"}},
              "3": {"class_type": "UnetLoaderGGUF", "inputs": {"unet_name": "flux.GGUF"}},
              "4": {"class_type": "CheckpointLoader", "inputs": {"ckpt_name": "sdxl/base.ckpt", "config_name": "v1.yaml"}}
            }"#,
            "test",
        )
        .expect("the graph parses");
        assert_eq!(
            checkpoints(&graph),
            vec!["birefnet.safetensors", "flux.GGUF", "sdxl/base.ckpt"]
        );
    }

    #[test]
    fn the_inputs_snapshot_round_trips_through_json() {
        let inputs = vec![(
            "Sampler".to_string(),
            vec![
                (
                    "steps".to_string(),
                    InputValue::Scalar(ScalarValue::Integer(5)),
                ),
                (
                    "cfg".to_string(),
                    InputValue::Scalar(ScalarValue::Number(2.0)),
                ),
                (
                    "denoise".to_string(),
                    InputValue::Scalar(ScalarValue::Number(0.6)),
                ),
                (
                    "tiling".to_string(),
                    InputValue::Scalar(ScalarValue::Bool(true)),
                ),
                (
                    "text".to_string(),
                    InputValue::Scalar(ScalarValue::Text("idol".to_string())),
                ),
                (
                    "image".to_string(),
                    InputValue::ImageRef("subject".to_string()),
                ),
            ],
        )];
        let text = serde_json::to_string(&inputs_to_json(&inputs)).expect("serializes");
        let parsed: serde_json::Value = serde_json::from_str(&text).expect("parses");
        let mut expected = inputs.clone();
        expected[0].1.sort_by(|a, b| a.0.cmp(&b.0));
        assert_eq!(inputs_from_json(&parsed).expect("reads back"), expected);
        assert!(inputs_from_json(&serde_json::json!({"N": {"x": [1]}})).is_err());
    }

    #[test]
    fn a_server_extension_is_kept_only_when_plain() {
        let file = |name: &str, kind| OutputFile {
            node: "9".into(),
            filename: name.into(),
            subfolder: String::new(),
            folder_type: "output".into(),
            batch_index: 0,
            kind,
        };
        for (name, kind, expected) in [
            ("a.png", OutputKind::Image, "png"),
            ("a.PNG", OutputKind::Image, "PNG"),
            ("mesh.glb", OutputKind::Mesh, "glb"),
            ("a.png:zone", OutputKind::Image, "png"),
            ("a.abcdefghi", OutputKind::Image, "png"),
            ("a.p g", OutputKind::Image, "png"),
            ("a.pé", OutputKind::Mesh, "glb"),
            ("noext", OutputKind::Mesh, "glb"),
        ] {
            assert_eq!(extension(&file(name, kind)), expected, "{name:?}");
        }
    }
}
