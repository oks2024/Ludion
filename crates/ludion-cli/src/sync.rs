//! Implementation of `ludion sync [--project <dir>] [--json]` — keeps a project's derived import
//! trees fresh.
//!
//! Each `[[import]]` table in `ludion.toml` names glTF sources (`from`, a glob) and a derived root
//! (`to`); a match `<dir>/<stem>.gltf` compiles into `<to>/<stem>/` exactly as `ludion import` would.
//! An entry is fresh when the content of its glTF's directory and the importer itself (this
//! binary's bytes) are unchanged since the last sync and every file it wrote is still there,
//! recorded in `<to>/.ludion-sync.json`. The record is also ownership: a `to` holding files without
//! it, or a record that does not parse, is refused (`L1245`) and the sync only ever deletes
//! directories under a root that carries it. An import compiles into `<to>/.sync-<stem>` and is
//! swapped in whole: never a partially written tree, though for an instant during the swap none at
//! all. The sync holds `<to>/.ludion-sync.lock`; a second sync waits for it (saying so on stderr),
//! then re-reads the record and re-decides ownership under the lock. A provisional empty record is
//! written before the first swap, so a first sync killed midway leaves a root the next run recovers
//! from. After the import stage, still under the locks, the cook stage (`cook.rs`) compresses the
//! imported meshes' maps into `.ludion/cooked/`; a cook failure never undoes an import.

use std::collections::BTreeMap;
use std::fs::{File, TryLockError};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::atomic::{AtomicUsize, Ordering};

use ludion_assets::content_hash;
use ludion_assets::import_sync::{
    LOCK_FILE, RECORD_FILE, SyncRecord as Record, SyncRecordEntry as RecordEntry, collect_files,
    directory_hash, expand_import_sources, hex,
};
use ludion_core::atomic::write_atomic;
use ludion_core::codes::code::{IMPORT_OUTPUT_IO, IMPORT_SYNC_REFUSED};
use ludion_core::diagnostics::Diagnostic;

use crate::cook::{CookOutcome, CookRequest, CookTree, cook_project};
use crate::diagnostic;
use crate::import::import_gltf;
use ludion_assets::cooked::CookTarget;
use ludion_engine::manifest::{self, ImportSection};

use crate::bake_probes::{self, Warmed};

/// One glTF a table's `from` matched.
#[derive(Debug, Clone)]
struct Entry {
    stem: String,
    source: String,
    hash: String,
}

#[derive(Debug, Default, PartialEq, Eq)]
struct Plan {
    import: Vec<String>,
    skip: Vec<String>,
    remove: Vec<String>,
}

/// What to do per stem: import what is new, changed, or built by another importer; skip what is
/// fresh and complete (`present` says every recorded output exists); remove what the record holds
/// that is no longer found. `found` is sorted by stem.
fn plan(
    found: &[Entry],
    previous: Option<&Record>,
    importer: &str,
    present: impl Fn(&str, &[String]) -> bool,
) -> Plan {
    let mut plan = Plan::default();
    let same_importer = previous.is_some_and(|record| record.importer == importer);
    for entry in found {
        let fresh = same_importer
            && previous
                .and_then(|record| record.entries.get(&entry.stem))
                .is_some_and(|old| {
                    old.hash == entry.hash
                        && old.source == entry.source
                        && !old.outputs.is_empty()
                        && present(&entry.stem, &old.outputs)
                });
        if fresh {
            plan.skip.push(entry.stem.clone());
        } else {
            plan.import.push(entry.stem.clone());
        }
    }
    if let Some(record) = previous {
        plan.remove = record
            .entries
            .keys()
            .filter(|stem| !found.iter().any(|entry| &entry.stem == *stem))
            .cloned()
            .collect();
    }
    plan
}

/// An entry's import: its warnings and written files, or the diagnostics that failed it.
type ImportResult = Result<(Vec<Diagnostic>, Vec<String>), Vec<Diagnostic>>;

/// One table after discovery and the refusal checks.
struct Table {
    to: PathBuf,
    found: Vec<Entry>,
    /// Read under the lock, after discovery.
    previous: Option<Record>,
}

/// What one sync did, per `[[import]]` entry stem.
#[derive(Default)]
struct Outcome {
    /// The cook stage's result; `None` when it did not run.
    cook: Option<CookOutcome>,
    imported: Vec<String>,
    skipped: Vec<String>,
    removed: Vec<String>,
    failed: Vec<(String, Vec<Diagnostic>)>,
    /// What the successful imports warned about, in print order.
    warnings: Vec<Diagnostic>,
    /// The manifest has no `[[import]]` table: nothing was looked at.
    no_tables: bool,
}

/// Syncs every table. `Err` covers what aborts before any write: the manifest, a refused table, an
/// unreadable binary. A per-entry import failure stays in `Outcome::failed`.
fn sync_project(project: &Path, cook: &CookRequest) -> Result<Outcome, Vec<Diagnostic>> {
    let manifest = manifest::load(project).map_err(|diagnostic| vec![diagnostic])?;
    if manifest.imports.is_empty() {
        return Ok(Outcome {
            no_tables: true,
            ..Outcome::default()
        });
    }
    let root = std::path::absolute(project).map_err(|error| {
        vec![Diagnostic::new(
            IMPORT_OUTPUT_IO,
            format!("could not resolve `{}`: {error}", project.display()),
        )]
    })?;
    let importer = importer_fingerprint().map_err(|error| {
        vec![Diagnostic::new(
            IMPORT_OUTPUT_IO,
            format!("could not fingerprint the running `ludion` binary: {error}"),
        )]
    })?;

    let mut tables = Vec::new();
    let mut refusals = Vec::new();
    for section in &manifest.imports {
        match discover(&root, section) {
            Ok(table) => tables.push(table),
            Err(diagnostic) => refusals.push(diagnostic),
        }
    }
    if !refusals.is_empty() {
        return Err(refusals);
    }

    // Wait, never refuse: the check gate, `ludion dev` and the editor's game slot sync the same
    // project concurrently, and a waiter reads the record and re-decides ownership under the lock,
    // so it finds what the first sync wrote fresh. The provisional record, written before any swap,
    // is what makes a killed first run recoverable. Manifest order is the same in every process
    // and nested roots are refused, so the locks cannot deadlock.
    let mut locks = Vec::new();
    for table in &tables {
        locks.push(lock_root(&table.to).map_err(|failure| vec![failure])?);
    }
    let mut refusals = Vec::new();
    for table in &mut tables {
        match claim(&table.to) {
            Ok(previous) => table.previous = previous,
            Err(diagnostic) => refusals.push(diagnostic),
        }
    }
    if !refusals.is_empty() {
        return Err(refusals);
    }
    for table in tables.iter().filter(|table| table.previous.is_none()) {
        let provisional = Record {
            importer: importer.clone(),
            entries: BTreeMap::new(),
        };
        write_record(&table.to, &provisional)
            .map_err(|error| vec![io_failure("write", &table.to.join(RECORD_FILE), &error)])?;
    }

    let mut outcome = Outcome::default();
    for table in &tables {
        sync_table(&root, table, &importer, &mut outcome);
    }
    let cook_textures = manifest
        .cook
        .as_ref()
        .is_none_or(|section| section.textures);
    if cook.enabled && cook_textures {
        let mut targets = manifest.cook_targets();
        for extra in &cook.extra_targets {
            if !targets.contains(extra) {
                targets.push(*extra);
            }
        }
        targets.sort_by_key(|target| matches!(target, CookTarget::Ios));
        let trees: Vec<CookTree> = tables
            .iter()
            .zip(&manifest.imports)
            .map(|(table, section)| CookTree {
                to: table.to.clone(),
                stems: table.found.iter().map(|entry| entry.stem.clone()).collect(),
                max_texture_size: section.max_texture_size,
            })
            .collect();
        outcome.cook = Some(cook_project(&root, &trees, &targets));
    }
    drop(locks);
    Ok(outcome)
}

/// Opens `<to>/.ludion-sync.lock` and blocks until this process holds it, saying on stderr when it
/// has to wait.
fn lock_root(to: &Path) -> Result<File, Diagnostic> {
    std::fs::create_dir_all(to).map_err(|error| io_failure("create", to, &error))?;
    let path = to.join(LOCK_FILE);
    let file = File::options()
        .create(true)
        .write(true)
        .truncate(false)
        .open(&path)
        .map_err(|error| io_failure("open", &path, &error))?;
    match file.try_lock() {
        Ok(()) => {}
        Err(TryLockError::WouldBlock) => {
            eprintln!(
                "waiting for the lock on {}: another sync or export is using it",
                to.display()
            );
            file.lock()
                .map_err(|error| io_failure("lock", &path, &error))?;
        }
        Err(TryLockError::Error(error)) => return Err(io_failure("lock", &path, &error)),
    }
    Ok(file)
}

/// Locks every table's `to` root, in manifest order (the order every sync takes them, so two
/// holders cannot deadlock). `ludion export` holds these while it copies the derived trees, so it
/// never ships a tree a sync is midway through swapping.
pub(crate) fn lock_imports(
    project: &Path,
    imports: &[ImportSection],
) -> Result<Vec<File>, Diagnostic> {
    imports
        .iter()
        .map(|import| lock_root(&project.join(&import.to)))
        .collect()
}

/// True when `to` holds nothing but the sync's own lock and debris.
fn holds_only_artifacts(to: &Path) -> bool {
    to.read_dir()
        .is_ok_and(|mut entries| entries.all(|e| e.is_ok_and(|e| is_sync_artifact(&e.file_name()))))
}

fn unowned(to: &Path) -> Diagnostic {
    refused(
        format!(
            "`{}` exists and holds files the sync did not write (no `{RECORD_FILE}`)",
            to.display()
        ),
        "delete it by hand if it is derived, or point `[[import]] to` somewhere else",
    )
}

/// Under the lock: the table's record, or `None` for a `to` that holds nothing foreign. A record
/// that does not parse and a record-less `to` with foreign files are `L1245` refusals.
fn claim(to: &Path) -> Result<Option<Record>, Diagnostic> {
    if let Some(record) = read_record(to)? {
        return Ok(Some(record));
    }
    if holds_only_artifacts(to) {
        Ok(None)
    } else {
        Err(unowned(to))
    }
}

fn write_record(to: &Path, record: &Record) -> std::io::Result<()> {
    let text = serde_json::to_string_pretty(record).expect("a record serializes");
    write_atomic(&to.join(RECORD_FILE), text + "\n")
}

/// The lock, or staging debris: what a sync that has not yet written its record leaves in `to`.
fn is_sync_artifact(name: &std::ffi::OsStr) -> bool {
    let name = name.to_string_lossy();
    name == LOCK_FILE
        || name.starts_with(".sync-")
        || name.starts_with(".old-")
        || ludion_core::atomic::is_temp_name_for(&name, RECORD_FILE)
}

/// The table's record: `None` when absent, an `L1245` refusal when it does not parse.
fn read_record(to: &Path) -> Result<Option<Record>, Diagnostic> {
    let path = to.join(RECORD_FILE);
    let Ok(text) = std::fs::read_to_string(&path) else {
        return Ok(None);
    };
    serde_json::from_str::<Record>(&text)
        .map(Some)
        .map_err(|error| {
            refused(
                format!("`{}` is not a sync record ({error})", path.display()),
                &format!(
                    "it is derived: delete `{}` and run `ludion sync` again",
                    to.display()
                ),
            )
        })
}

/// What `ensure_fresh` did, for a caller's one-line report.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct Summary {
    pub imported: usize,
    pub skipped: usize,
    pub removed: usize,
}

/// The in-process entry `dev` and `export` call: brings the project's derived imports and their
/// cooked maps up to date. A project with no `ludion.toml` or no `[[import]]` table is a no-op.
/// Prints each entry's import warnings; a failed entry is an `Err` carrying all its diagnostics. A
/// map that fails to cook is printed and never an `Err`: it still loads from its source PNG.
pub(crate) fn ensure_fresh(project: &Path, cook: &CookRequest) -> Result<Summary, Vec<Diagnostic>> {
    if !project.join("ludion.toml").is_file() {
        return Ok(Summary::default());
    }
    let outcome = sync_project(project, cook)?;
    for failed in outcome.cook.iter().flat_map(|cook| &cook.failed) {
        eprintln!("sync: could not cook {} ({}):", failed.map, failed.target);
        for diagnostic in &failed.diagnostics {
            diagnostic::print(diagnostic);
        }
    }
    if outcome.failed.is_empty() {
        return Ok(Summary {
            imported: outcome.imported.len(),
            skipped: outcome.skipped.len(),
            removed: outcome.removed.len(),
        });
    }
    Err(outcome
        .failed
        .into_iter()
        .flat_map(|(_, diagnostics)| diagnostics)
        .collect())
}

/// Runs `ludion sync`. Exit 1 when the manifest is unusable, a table is refused (nothing is
/// written), or any entry failed to import or cook.
pub(crate) fn run(project: &Path, json: bool, cook: &CookRequest) -> ExitCode {
    let outcome = match sync_project(project, cook) {
        Ok(outcome) => outcome,
        Err(diagnostics) => {
            for diagnostic in &diagnostics {
                diagnostic::print(diagnostic);
            }
            if json {
                print_json(false, &Outcome::default(), &Warmed::default(), &diagnostics);
            }
            return ExitCode::FAILURE;
        }
    };
    if outcome.no_tables {
        if !json {
            println!(
                "sync: no [[import]] tables in {}",
                project.join("ludion.toml").display()
            );
        }
        let warmed = warm_probes(project, json);
        if json {
            print_json(true, &outcome, &warmed, &[]);
        }
        return ExitCode::SUCCESS;
    }
    for (stem, diagnostics) in &outcome.failed {
        eprintln!("sync: could not import `{stem}`:");
        for diagnostic in diagnostics {
            diagnostic::print(diagnostic);
        }
    }
    let cook_failed = outcome.cook.iter().flat_map(|cook| &cook.failed);
    for failed in cook_failed.clone() {
        eprintln!("sync: could not cook {} ({}):", failed.map, failed.target);
        for diagnostic in &failed.diagnostics {
            diagnostic::print(diagnostic);
        }
    }
    let ok = outcome.failed.is_empty() && cook_failed.clone().next().is_none();
    let warmed = warm_probes(project, json);
    if json {
        let rendered: Vec<Diagnostic> = outcome
            .warnings
            .iter()
            .chain(outcome.failed.iter().flat_map(|(_, d)| d))
            .chain(cook_failed.flat_map(|failed| &failed.diagnostics))
            .cloned()
            .collect();
        print_json(ok, &outcome, &warmed, &rendered);
    } else {
        for stem in &outcome.imported {
            println!("imported {stem}");
        }
        for stem in &outcome.skipped {
            println!("skipped {stem} (fresh)");
        }
        for stem in &outcome.removed {
            println!("removed {stem}");
        }
        for (stem, _) in &outcome.failed {
            println!("failed {stem}");
        }
        println!(
            "sync: {} imported, {} skipped, {} removed, {} failed",
            outcome.imported.len(),
            outcome.skipped.len(),
            outcome.removed.len(),
            outcome.failed.len()
        );
        if let Some(cook) = &outcome.cook {
            println!(
                "cook: {} cooked, {} reused, {} failed",
                cook.cooked,
                cook.reused,
                cook.failed.len()
            );
        }
        print_warmed(&warmed);
    }
    if ok {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

/// Warms the probe store for the default scene's directory. Human progress notes print only when
/// `quiet` is false, so `--json` keeps stdout to the one object. A manifest `run` has already loaded
/// is read again here, cheaply.
fn warm_probes(project: &Path, quiet: bool) -> Warmed {
    let Ok(manifest) = manifest::load(project) else {
        return Warmed::default();
    };
    let Some(scene_dir) = manifest.default_scene.parent() else {
        return Warmed::default();
    };
    bake_probes::warm(project, scene_dir, quiet)
}

/// The human summary of a warm, silent when no scene held a probe.
fn print_warmed(warmed: &Warmed) {
    if warmed.scenes > 0 {
        println!(
            "probes: {} captured, {} stored, in {} scenes",
            warmed.captured, warmed.stored, warmed.scenes
        );
    }
    if !warmed.skipped.is_empty() {
        println!(
            "skipped {} scenes the CLI cannot load: bake them with the game's --bake-probes",
            warmed.skipped.len()
        );
    }
}

/// The shared `--json` envelope: the per-stem lists, then `rendered` — every diagnostic the run
/// printed, in print order (a refusal is `ok: false` and `L1245` here).
fn print_json(ok: bool, outcome: &Outcome, warmed: &Warmed, rendered: &[Diagnostic]) {
    let failed: Vec<_> = outcome
        .failed
        .iter()
        .map(|(stem, diagnostics)| serde_json::json!({ "stem": stem, "diagnostics": diagnostics }))
        .collect();
    let cook = outcome.cook.as_ref();
    let cook_failed: Vec<_> = cook
        .iter()
        .flat_map(|cook| &cook.failed)
        .map(|failed| {
            serde_json::json!({
                "map": failed.map,
                "target": failed.target,
                "diagnostics": failed.diagnostics,
            })
        })
        .collect();
    let fields = serde_json::json!({
        "imported": outcome.imported,
        "skipped": outcome.skipped,
        "removed": outcome.removed,
        "failed": failed,
        "cooked": cook.map_or(0, |cook| cook.cooked),
        "cook_reused": cook.map_or(0, |cook| cook.reused),
        "cook_failed": cook_failed,
        "store_bytes": cook.map_or(0, |cook| cook.store_bytes),
        "probes_captured": warmed.captured,
        "probes_stored": warmed.stored,
        "probe_scenes_skipped": warmed.skipped,
    });
    let envelope = serde_json::Value::Object(diagnostic::json_envelope(ok, fields, rendered));
    println!("{envelope}");
}

/// xxh3 of the running binary: never stale, unlike a version constant someone must bump.
fn importer_fingerprint() -> std::io::Result<String> {
    let bytes = std::fs::read(std::env::current_exe()?)?;
    Ok(hex(content_hash(&bytes)))
}

fn refused(message: String, help: &str) -> Diagnostic {
    Diagnostic::new(IMPORT_SYNC_REFUSED, message).with_help(help.to_string())
}

/// Expands a table's `from`, hashes each source, and checks the `to` tree is the sync's to write.
fn discover(root: &Path, section: &ImportSection) -> Result<Table, Diagnostic> {
    let sources = expand_import_sources(root, &section.from);
    if sources.is_empty() {
        return Err(refused(
            format!(
                "`[[import]] from` = `{}` matches no `.gltf` file under `{}`",
                section.from,
                root.display()
            ),
            "check the glob for a typo (`*` matches within one path segment, `**` is not supported)",
        ));
    }
    let mut found: Vec<Entry> = Vec::new();
    for source in sources {
        let stem = Path::new(&source)
            .file_stem()
            .map(|stem| stem.to_string_lossy().into_owned())
            .unwrap_or_default();
        if let Some(first) = found
            .iter()
            .find(|entry| entry.stem.eq_ignore_ascii_case(&stem))
        {
            return Err(refused(
                format!(
                    "`{}` and `{source}` both import as `{stem}` (into `{}/{stem}`)",
                    first.source,
                    section.to.display()
                ),
                "rename one of the sources: each lands in `<to>/<stem>/`, and stems that differ only in case share a directory on Windows and macOS",
            ));
        }
        let dir = root.join(Path::new(&source).parent().unwrap_or(Path::new("")));
        let hash = hex(directory_hash(&dir).map_err(|error| {
            if error.kind() == std::io::ErrorKind::InvalidInput {
                return refused(
                    error.to_string(),
                    "replace the link with the files, or move it out of the glTF's directory",
                );
            }
            refused(
                format!("could not read `{}`: {error}", dir.display()),
                "check the path and permissions",
            )
        })?);
        found.push(Entry { stem, source, hash });
    }
    found.sort_by(|a, b| a.stem.cmp(&b.stem));

    let to = root.join(&section.to);
    // A root that has never seen a sync (no record, no lock) is refused before it gets a lock
    // file; once a lock exists the decision is `claim`'s, under the lock.
    if !to.join(RECORD_FILE).is_file()
        && !to.join(LOCK_FILE).exists()
        && to.exists()
        && !holds_only_artifacts(&to)
    {
        return Err(unowned(&to));
    }
    Ok(Table {
        to,
        found,
        previous: None,
    })
}

/// Applies one table's plan: removals, then parallel imports, then the record.
fn sync_table(root: &Path, table: &Table, importer: &str, outcome: &mut Outcome) {
    let plan = plan(
        &table.found,
        table.previous.as_ref(),
        importer,
        |stem, outputs| {
            outputs
                .iter()
                .all(|file| table.to.join(stem).join(file).is_file())
        },
    );
    let mut removed: Vec<String> = plan.remove.clone();
    // A failed import leaves no record entry; a source deleted after that must still go.
    if let Ok(read) = table.to.read_dir() {
        let mut leftovers: Vec<String> = read
            .filter_map(Result::ok)
            .filter(|entry| entry.path().is_dir())
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .filter(|name| !table.found.iter().any(|entry| &entry.stem == name))
            .filter(|name| !removed.contains(name))
            .collect();
        leftovers.sort();
        removed.extend(leftovers);
        removed.sort();
    }
    // Staging and swap debris from a crashed run is not a stem: swept, never reported.
    let debris: Vec<String> = removed
        .iter()
        .filter(|name| name.starts_with('.'))
        .cloned()
        .collect();
    removed.retain(|name| !name.starts_with('.'));
    if let Err(error) = std::fs::create_dir_all(&table.to) {
        let failure = io_failure("create", &table.to, &error);
        outcome
            .failed
            .push((table.to.display().to_string(), vec![failure]));
        return;
    }
    for name in debris {
        let _ = remove_dir(&table.to.join(name));
    }
    let mut gone = Vec::new();
    for stem in removed {
        match remove_dir(&table.to.join(&stem)) {
            Ok(()) => gone.push(stem),
            Err(failure) => outcome.failed.push((stem, vec![failure])),
        }
    }
    let removed = gone;

    let jobs: Vec<&Entry> = table
        .found
        .iter()
        .filter(|entry| plan.import.contains(&entry.stem))
        .collect();
    let next = AtomicUsize::new(0);
    let workers = std::thread::available_parallelism()
        .map_or(1, std::num::NonZeroUsize::get)
        .min(jobs.len());
    let mut results: Vec<(usize, ImportResult)> = std::thread::scope(|scope| {
        let handles: Vec<_> = (0..workers)
            .map(|_| {
                scope.spawn(|| {
                    let mut done = Vec::new();
                    loop {
                        let index = next.fetch_add(1, Ordering::Relaxed);
                        let Some(entry) = jobs.get(index) else { break };
                        done.push((index, import_entry(root, &table.to, entry)));
                    }
                    done
                })
            })
            .collect();
        handles
            .into_iter()
            .flat_map(|handle| handle.join().expect("import worker panicked"))
            .collect()
    });
    results.sort_by_key(|(index, _)| *index);

    let mut record = Record {
        importer: importer.to_string(),
        entries: BTreeMap::new(),
    };
    let mut imported_ok: Vec<&str> = Vec::new();
    for ((_, result), entry) in results.into_iter().zip(&jobs) {
        match result {
            Ok((warnings, outputs)) => {
                for warning in warnings {
                    diagnostic::print(&warning);
                    outcome.warnings.push(warning);
                }
                record
                    .entries
                    .insert(entry.stem.clone(), record_entry(entry, outputs));
                imported_ok.push(&entry.stem);
            }
            Err(diagnostics) => outcome.failed.push((entry.stem.clone(), diagnostics)),
        }
    }
    for entry in &table.found {
        if plan.skip.contains(&entry.stem) {
            let outputs = table
                .previous
                .as_ref()
                .and_then(|previous| previous.entries.get(&entry.stem))
                .map(|old| old.outputs.clone())
                .unwrap_or_default();
            record
                .entries
                .insert(entry.stem.clone(), record_entry(entry, outputs));
        }
    }
    // Report in sorted stem order, whatever order the workers finished in.
    outcome.imported.extend(
        table
            .found
            .iter()
            .map(|e| e.stem.as_str())
            .filter(|s| imported_ok.contains(s))
            .map(String::from),
    );
    outcome.skipped.extend(plan.skip);
    outcome.removed.extend(removed);

    if let Err(error) = write_record(&table.to, &record) {
        let failure = io_failure("write", &table.to.join(RECORD_FILE), &error);
        outcome
            .failed
            .push((RECORD_FILE.to_string(), vec![failure]));
    }
}

pub(crate) fn io_failure(action: &str, path: &Path, error: &std::io::Error) -> Diagnostic {
    Diagnostic::new(
        IMPORT_OUTPUT_IO,
        format!("could not {action} `{}`: {error}", path.display()),
    )
    .with_help(
        "check that nothing holds the files open (a viewer or indexer) and the disk is writable",
    )
}

/// Removes a derived directory; one that is already gone is fine.
fn remove_dir(path: &Path) -> Result<(), Diagnostic> {
    match std::fs::remove_dir_all(path) {
        Err(error) if error.kind() != std::io::ErrorKind::NotFound => {
            Err(io_failure("remove", path, &error))
        }
        _ => Ok(()),
    }
}

fn record_entry(entry: &Entry, outputs: Vec<String>) -> RecordEntry {
    RecordEntry {
        source: entry.source.clone(),
        hash: entry.hash.clone(),
        outputs,
    }
}

/// Imports into `<to>/.sync-<stem>` (a sibling, so `generated_from` paths match a hand import) and
/// swaps it in; a failed import never touches the live tree.
fn import_entry(root: &Path, to: &Path, entry: &Entry) -> ImportResult {
    let staging = to.join(format!(".sync-{}", entry.stem));
    let dest = to.join(&entry.stem);
    let old = to.join(format!(".old-{}", entry.stem));
    remove_dir(&staging).map_err(|failure| vec![failure])?;
    let imported = match import_gltf(&root.join(&entry.source), &staging, false) {
        Ok(imported) => imported,
        Err(diagnostics) => {
            let _ = remove_dir(&staging);
            return Err(diagnostics);
        }
    };
    let mut outputs = Vec::new();
    if let Err(error) = collect_files(&staging, &staging, &mut outputs) {
        let _ = remove_dir(&staging);
        return Err(vec![io_failure("read", &staging, &error)]);
    }
    outputs.sort();
    if let Err(failure) = swap_in(&staging, &dest, &old) {
        let _ = remove_dir(&staging);
        return Err(vec![failure]);
    }
    Ok((imported.warnings, outputs))
}

fn swap_in(staging: &Path, dest: &Path, old: &Path) -> Result<(), Diagnostic> {
    remove_dir(old)?;
    let had_old = dest.exists();
    if had_old {
        rename_retry(dest, old).map_err(|error| io_failure("rename", dest, &error))?;
    }
    if let Err(error) = rename_retry(staging, dest) {
        if had_old && let Err(restore) = rename_retry(old, dest) {
            return Err(Diagnostic::new(
                IMPORT_OUTPUT_IO,
                format!(
                    "could not rename `{}` ({error}), and could not restore the previous tree from `{}` ({restore})",
                    staging.display(),
                    old.display()
                ),
            )
            .with_help(format!(
                "move `{}` back to `{}`, or run `ludion sync` to re-import",
                old.display(),
                dest.display()
            )));
        }
        return Err(io_failure("rename", staging, &error));
    }
    remove_dir(old)
}

/// Windows refuses to rename a directory while a reader holds a file in it, usually briefly; only
/// those errors are retried, with a growing back-off (about 2 s in all).
fn rename_retry(from: &Path, to: &Path) -> std::io::Result<()> {
    use std::io::ErrorKind;
    let mut attempt = 1;
    let mut delay = 50;
    loop {
        match std::fs::rename(from, to) {
            // 32 is Windows' ERROR_SHARING_VIOLATION, which has no distinct ErrorKind.
            Err(error)
                if attempt < 8
                    && (matches!(
                        error.kind(),
                        ErrorKind::PermissionDenied | ErrorKind::ResourceBusy
                    ) || error.raw_os_error() == Some(32)) =>
            {
                attempt += 1;
                std::thread::sleep(std::time::Duration::from_millis(delay));
                delay = (delay * 2).min(400);
            }
            result => return result,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(stem: &str, hash: &str) -> Entry {
        Entry {
            stem: stem.into(),
            source: format!("k/{stem}.gltf"),
            hash: hash.into(),
        }
    }

    fn record(importer: &str, entries: &[(&str, &str)]) -> Record {
        Record {
            importer: importer.into(),
            entries: entries
                .iter()
                .map(|(stem, hash)| {
                    (
                        (*stem).to_string(),
                        RecordEntry {
                            source: format!("k/{stem}.gltf"),
                            hash: (*hash).into(),
                            outputs: vec!["m.yaml".into()],
                        },
                    )
                })
                .collect(),
        }
    }

    #[test]
    fn an_unchanged_tree_skips_everything() {
        let found = [entry("a", "1"), entry("b", "2")];
        let previous = record("x", &[("a", "1"), ("b", "2")]);
        let plan = plan(&found, Some(&previous), "x", |_, _| true);
        assert_eq!(plan.skip, ["a", "b"]);
        assert!(plan.import.is_empty() && plan.remove.is_empty());
    }

    #[test]
    fn a_new_importer_reimports_everything() {
        let found = [entry("a", "1"), entry("b", "2")];
        let previous = record("x", &[("a", "1"), ("b", "2")]);
        let plan = plan(&found, Some(&previous), "y", |_, _| true);
        assert_eq!(plan.import, ["a", "b"]);
        assert!(plan.skip.is_empty());
    }

    #[test]
    fn a_changed_hash_reimports_only_that_entry() {
        let found = [entry("a", "1"), entry("b", "3")];
        let previous = record("x", &[("a", "1"), ("b", "2")]);
        let plan = plan(&found, Some(&previous), "x", |_, _| true);
        assert_eq!(plan.import, ["b"]);
        assert_eq!(plan.skip, ["a"]);
    }

    #[test]
    fn a_vanished_source_is_removed() {
        let found = [entry("a", "1")];
        let previous = record("x", &[("a", "1"), ("gone", "9")]);
        let plan = plan(&found, Some(&previous), "x", |_, _| true);
        assert_eq!(plan.remove, ["gone"]);
        assert_eq!(plan.skip, ["a"]);
    }

    #[test]
    fn no_record_imports_everything() {
        let plan = plan(&[entry("a", "1")], None, "x", |_, _| true);
        assert_eq!(plan.import, ["a"]);
        assert!(plan.remove.is_empty());
    }

    #[test]
    fn a_missing_output_reimports_that_entry() {
        let found = [entry("a", "1"), entry("b", "2")];
        let previous = record("x", &[("a", "1"), ("b", "2")]);
        let plan = plan(&found, Some(&previous), "x", |stem, _| stem != "a");
        assert_eq!(plan.import, ["a"]);
        assert_eq!(plan.skip, ["b"]);
    }

    #[test]
    fn a_record_without_outputs_reimports() {
        let found = [entry("a", "1")];
        let mut previous = record("x", &[("a", "1")]);
        previous.entries.get_mut("a").unwrap().outputs.clear();
        let plan = plan(&found, Some(&previous), "x", |_, _| true);
        assert_eq!(plan.import, ["a"]);
    }

    #[test]
    fn a_lock_and_staging_dirs_are_the_syncs_own() {
        use std::ffi::OsStr;
        assert!(is_sync_artifact(OsStr::new(".ludion-sync.lock")));
        assert!(is_sync_artifact(OsStr::new(".sync-a")));
        assert!(is_sync_artifact(OsStr::new(".old-a")));
        assert!(is_sync_artifact(OsStr::new("..ludion-sync.json.12.0.tmp")));
        assert!(!is_sync_artifact(OsStr::new("..other.json.12.0.tmp")));
        assert!(!is_sync_artifact(OsStr::new("hand.txt")));
        assert!(!is_sync_artifact(OsStr::new(".hidden")));
    }

    #[test]
    fn a_failed_removal_is_l1203_and_a_missing_directory_is_fine() {
        let dir = std::env::temp_dir().join(format!("ludion-sync-unit-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        assert!(remove_dir(&dir.join("absent")).is_ok());
        let file = dir.join("file.txt");
        std::fs::write(&file, "x").unwrap();
        let failure = remove_dir(&file).unwrap_err();
        assert_eq!(failure.code, IMPORT_OUTPUT_IO);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn ensure_fresh_is_a_no_op_without_a_manifest_or_import_tables() {
        let dir = std::env::temp_dir().join(format!("ludion-sync-fresh-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let cook = CookRequest::default();
        assert_eq!(ensure_fresh(&dir, &cook).unwrap(), Summary::default());
        assert_eq!(dir.read_dir().unwrap().count(), 0);

        std::fs::write(
            dir.join("ludion.toml"),
            "name = \"scratch\"\ndefault_scene = \"scenes/main.yaml\"\n",
        )
        .unwrap();
        assert_eq!(ensure_fresh(&dir, &cook).unwrap(), Summary::default());
        assert_eq!(dir.read_dir().unwrap().count(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
