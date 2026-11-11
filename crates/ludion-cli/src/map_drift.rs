//! Implementation of `ludion map-drift [--root <dir>] [--json] [--deny-warnings] [--allow <code>]
//! [<scan dir>...]`: the map half of the drift gate, in process, for any game project.
//!
//! Discovery is source-driven: every `*.map.yaml` under the scan dirs is recompiled and
//! byte-compared against its default output, and every scene or prefab whose head names a map under
//! `generated_from:` (with the `--kit`/`--seed`/`--mirror` flags it was compiled with) is recompiled
//! the same way. Every map the checks reach must also seal (`map-check`'s walk), which the engine
//! otherwise keeps a warning. Nothing is written.
//!
//! A map's default output may be absent when at least one scene or prefab claims the map: a room map
//! compiled only into prefabs has none. A claim naming a map that is not discovered under the scan
//! dirs is still checked as an override claimant; a plain claim of an undiscovered map is ignored.
//! Scenes claiming a script or a Rust test stay with the engine's JS gate.

use std::collections::BTreeMap;
use std::io::Read;
use std::path::{Component, Path, PathBuf};
use std::process::ExitCode;

use ludion_core::codes::code;
use ludion_core::diagnostics::Diagnostic;
use ludion_scene::{MapOverrides, Mirror};

use crate::generate_map::{CompileError, Fault, default_out, generate_yaml};
use crate::map_check::{self, Options};

/// Document kinds that name their own sources and are gated elsewhere, or are sources themselves.
const EXCLUDED_SUFFIXES: [&str; 7] = [
    ".voxel.yaml",
    ".atlas.yaml",
    ".font.yaml",
    ".mesh.yaml",
    ".anim.yaml",
    ".map.yaml",
    ".path.yaml",
];

/// Only the head of a document is read: derived meshes are megabytes.
const HEAD_BYTES: u64 = 8192;

/// A scene or prefab naming a map under `generated_from:`.
struct Claimant {
    scene: PathBuf,
    /// `None`: a plain claim of the map's default output.
    overrides: Option<MapOverrides>,
    is_prefab: bool,
}

/// Diagnostics grouped by the file (root-relative, `/`-separated) they belong to.
#[derive(Default)]
struct Findings {
    files: BTreeMap<String, Vec<Diagnostic>>,
}

impl Findings {
    fn add(&mut self, file: &str, diagnostic: Diagnostic) {
        self.files
            .entry(file.to_owned())
            .or_default()
            .push(diagnostic);
    }

    fn errors(&self) -> usize {
        self.files
            .values()
            .flatten()
            .filter(|d| d.severity.is_error())
            .count()
    }
}

struct Gate<'a> {
    root: PathBuf,
    deny_warnings: bool,
    allow: &'a [(String, Option<String>)],
    findings: Findings,
    outputs: usize,
}

/// Runs `ludion map-drift`.
pub fn run(
    root: Option<&Path>,
    dirs: &[PathBuf],
    json: bool,
    deny_warnings: bool,
    allow: &[String],
) -> ExitCode {
    let mut allowed = Vec::new();
    for entry in allow {
        let (code_part, prefix) = match entry.split_once(':') {
            Some((code_part, prefix)) => (code_part, Some(prefix.to_owned())),
            None => (entry.as_str(), None),
        };
        let shaped = code_part.len() == 5
            && code_part.starts_with('L')
            && code_part[1..].bytes().all(|b| b.is_ascii_digit());
        if !shaped {
            return crate::diagnostic::usage_error(format_args!(
                "--allow `{entry}`: expected `L####` or `L####:<message prefix>`"
            ));
        }
        allowed.push((code_part.to_owned(), prefix));
    }

    let root = normalize(root.unwrap_or(Path::new(".")));
    let default_dirs = [PathBuf::from("scenes")];
    let dirs = if dirs.is_empty() { &default_dirs } else { dirs };
    let mut gate = Gate {
        root,
        deny_warnings,
        allow: &allowed,
        findings: Findings::default(),
        outputs: 0,
    };
    let maps = gate.check(dirs);

    let errors = gate.findings.errors();
    let ok = errors == 0;
    if json {
        let files: Vec<_> = gate
            .findings
            .files
            .iter()
            .map(|(file, diagnostics)| {
                serde_json::json!({
                    "file": file,
                    "diagnostics": diagnostics,
                })
            })
            .collect();
        let all: Vec<Diagnostic> = gate.findings.files.values().flatten().cloned().collect();
        let envelope = crate::diagnostic::json_envelope(
            ok,
            serde_json::json!({ "maps": maps, "outputs": gate.outputs, "files": files }),
            &all,
        );
        println!("{}", serde_json::Value::Object(envelope));
    } else {
        for (file, diagnostics) in &gate.findings.files {
            crate::diagnostic::fail_on_errors(Path::new(file), diagnostics);
        }
        if ok {
            println!(
                "map-drift: {maps} map(s), {} output(s) checked, none drifted.",
                gate.outputs
            );
        } else {
            eprintln!(
                "map-drift: {errors} fault(s) in {} file(s)",
                gate.findings
                    .files
                    .values()
                    .filter(|d| d.iter().any(|d| d.severity.is_error()))
                    .count()
            );
        }
    }
    if ok {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

impl Gate<'_> {
    /// Runs every check; returns the number of maps found.
    fn check(&mut self, dirs: &[PathBuf]) -> usize {
        let mut maps = Vec::new();
        let mut scenes = Vec::new();
        for dir in dirs {
            walk(&self.root.join(dir), &mut |path| {
                let name = file_name(path);
                if name.ends_with(".map.yaml") {
                    maps.push(normalize(path));
                } else if (name.ends_with(".yaml") || name.ends_with(".yml"))
                    && !EXCLUDED_SUFFIXES.iter().any(|s| name.ends_with(s))
                {
                    scenes.push(normalize(path));
                }
            });
        }
        maps.sort();
        maps.dedup();
        scenes.sort();
        scenes.dedup();

        if maps.is_empty() {
            let listed: Vec<String> = dirs.iter().map(|d| format!("{}/", slash(d))).collect();
            let label = slash(&dirs[0]);
            self.fault(
                &label,
                format!(
                    "found no *.map.yaml under {} — a gate that checks nothing proves nothing",
                    listed.join(" or ")
                ),
                "pass the directories holding the maps (`ludion map-drift <dir>...`), or run from \
                 the project root with `--root`",
            );
        }

        let claims = self.collect_claims(&scenes);
        // Every distinct compile to seal-check: each map as written, plus each override claimant's.
        let mut seals: Vec<(PathBuf, MapOverrides)> = Vec::new();
        let mut seal_once = |map: &Path, overrides: &MapOverrides| {
            if !seals.iter().any(|(m, o)| m == map && o == overrides) {
                seals.push((map.to_path_buf(), overrides.clone()));
            }
        };

        for map in &maps {
            let claimants = claims.get(map).map(Vec::as_slice).unwrap_or_default();
            let out = default_out(map);
            if out.exists() || claimants.is_empty() {
                self.compare(map, &out, &MapOverrides::default(), false);
            }
            // A scene that names the map but lost its `generated_from` line is caught by the byte
            // compare: the matching output carries the header, so a header-less one differs.
            for claimant in claimants.iter().filter(|c| c.overrides.is_none()) {
                if normalize(&claimant.scene) != normalize(&out) {
                    self.fault(
                        &self.rel(&claimant.scene),
                        format!(
                            "names {} under generated_from but is not the output that source \
                             writes (it writes {}): a moved or orphaned copy — delete it, or \
                             regenerate at the default path",
                            self.rel(map),
                            self.rel(&out)
                        ),
                        &self.fix(map, &out, &MapOverrides::default(), false),
                    );
                }
            }
            seal_once(map, &MapOverrides::default());
        }
        for (map, claimants) in &claims {
            for claimant in claimants {
                if let Some(overrides) = &claimant.overrides {
                    self.compare(map, &claimant.scene, overrides, claimant.is_prefab);
                    seal_once(map, overrides);
                }
            }
        }
        for (map, overrides) in &seals {
            self.seal(map, overrides);
        }
        maps.len()
    }

    /// Reads every scene's head and groups the claimants by the map they name.
    fn collect_claims(&mut self, scenes: &[PathBuf]) -> BTreeMap<PathBuf, Vec<Claimant>> {
        let mut claims: BTreeMap<PathBuf, Vec<Claimant>> = BTreeMap::new();
        for scene in scenes {
            let Some(source) = head_generated_from(scene) else {
                continue;
            };
            let tokens: Vec<&str> = source.split_whitespace().collect();
            let Some(first) = tokens.first().filter(|t| t.ends_with(".map.yaml")) else {
                continue;
            };
            let is_prefab = file_name(scene).ends_with(".prefab.yaml");
            let dir = scene.parent().unwrap_or(Path::new("."));
            let map = normalize(&dir.join(first));
            let rel = self.rel(scene);
            let overrides = if tokens.len() > 1 || is_prefab {
                match parse_flags(&tokens[1..]) {
                    Ok(overrides) => Some(overrides),
                    Err(bad) => {
                        self.fault(
                            &rel,
                            format!("generated_from `{source}`: {bad}"),
                            "write `generated_from: <map> [--kit <k>] [--seed <n>] [--mirror x|z]` \
                             as `ludion generate-map` records it, or regenerate the file",
                        );
                        continue;
                    }
                }
            } else {
                None
            };
            if !map.exists() {
                self.fault(
                    &rel,
                    format!("names map source `{first}`, which does not exist beside it"),
                    "restore the map, or delete this generated file",
                );
                continue;
            }
            claims.entry(map).or_default().push(Claimant {
                scene: scene.clone(),
                overrides,
                is_prefab,
            });
        }
        claims
    }

    /// Recompiles `map` into `out` and byte-compares it against the file committed there.
    fn compare(&mut self, map: &Path, out: &Path, overrides: &MapOverrides, prefab: bool) {
        self.outputs += 1;
        let (map_rel, out_rel) = (self.rel(map), self.rel(out));
        let fix = self.fix(map, out, overrides, prefab);
        let yaml = match generate_yaml(map, out, overrides, prefab) {
            Ok((_, yaml)) => yaml,
            Err(CompileError::Usage(message)) => {
                self.fault(
                    &map_rel,
                    message,
                    "rename the file so its path has no whitespace",
                );
                return;
            }
            Err(CompileError::Faults(faults)) => {
                self.add_faults(map, faults);
                self.fault(
                    &map_rel,
                    format!("`generate-map` for {out_rel} does not compile"),
                    "fix the diagnostics above, then regenerate",
                );
                return;
            }
        };
        match std::fs::read(out) {
            Ok(existing) if existing == yaml.as_bytes() => {}
            Ok(_) => self.findings.add(
                &out_rel,
                Diagnostic::new(
                    code::MAP_SCENE_DRIFT,
                    format!(
                        "DRIFT {out_rel}: the committed file differs from what {map_rel} now \
                         generates"
                    ),
                )
                .with_help(format!("regenerate it with `{fix}` and commit the result")),
            ),
            Err(error) => self.fault(
                &out_rel,
                format!("the output of {map_rel} is not committed: {error}"),
                &format!("generate it with `{fix}` and commit the result"),
            ),
        }
    }

    /// Runs `map-check`'s analysis for one compile and requires it sealed.
    fn seal(&mut self, map: &Path, overrides: &MapOverrides) {
        let map_rel = self.rel(map);
        let analysis = match map_check::analyze(map, &Options::default(), overrides) {
            Ok(analysis) => analysis,
            Err(faults) => {
                self.add_faults(map, faults);
                self.fault(
                    &map_rel,
                    "`map-check` failed on this map".to_owned(),
                    "fix the diagnostics above; `ludion map-check` shows the full report",
                );
                return;
            }
        };
        for gate in &analysis.gate_diagnostics {
            self.findings.add(&map_rel, gate.clone());
        }
        if !analysis.sealed() {
            let how = overrides.provenance(&file_name(map));
            match analysis.seal_lint() {
                Some(lint) => {
                    self.findings.add(&map_rel, lint);
                    self.fault(
                        &map_rel,
                        format!("the walker can leave the map ({how}): the seal check leaks"),
                        "close the opening with a collider behind its gate (L1040 above says \
                         where); `ludion map-check` shows the report",
                    );
                }
                None => self.fault(
                    &map_rel,
                    format!("the seal was not checked ({how}): {}", analysis.seal_line()),
                    "give the map a marker or a reachable anchor so the seal walk can start",
                ),
            }
        }
        if self.deny_warnings {
            for warning in &analysis.warnings {
                // A leak is already a seal fault above.
                if warning.code == code::MAP_EDGE_LEAK || self.allowed(warning) {
                    continue;
                }
                self.findings.add(&map_rel, warning.clone());
                self.fault(
                    &map_rel,
                    format!("denied warning {}: {}", warning.code, warning.message),
                    &format!(
                        "fix the map, or — if it is intended — pass `--allow {}:<message prefix>` \
                         to `ludion map-drift`",
                        warning.code
                    ),
                );
            }
        }
    }

    fn allowed(&self, warning: &Diagnostic) -> bool {
        self.allow.iter().any(|(code, prefix)| {
            code == warning.code.as_str()
                && prefix
                    .as_deref()
                    .is_none_or(|p| warning.message.starts_with(p))
        })
    }

    /// Adds compile faults against the file each was written in (the map, or a fragment).
    fn add_faults(&mut self, map: &Path, faults: Vec<Fault>) {
        let dir = map.parent().unwrap_or(Path::new("."));
        for (fragment, diagnostic) in faults {
            let file = fragment.map_or_else(|| self.rel(map), |f| self.rel(&dir.join(f)));
            self.findings.add(&file, diagnostic);
        }
    }

    fn fault(&mut self, file: &str, message: impl Into<String>, help: &str) {
        self.findings.add(
            file,
            Diagnostic::new(code::MAP_DRIFT_FAULT, message).with_help(help),
        );
    }

    /// The root-relative, `/`-separated spelling of a path.
    fn rel(&self, path: &Path) -> String {
        if self.root == Path::new(".") {
            return slash(path);
        }
        slash(path.strip_prefix(&self.root).unwrap_or(path))
    }

    /// The command that regenerates `out` from `map`, run from the root.
    fn fix(&self, map: &Path, out: &Path, overrides: &MapOverrides, prefab: bool) -> String {
        let mut command = format!("ludion generate-map {}", self.rel(map));
        if *overrides == MapOverrides::default() && !prefab && normalize(out) == default_out(map) {
            return command;
        }
        command.push_str(&format!(" --out {}", self.rel(out)));
        if let Some(kit) = &overrides.kit {
            let dir = map.parent().unwrap_or(Path::new("."));
            command.push_str(&format!(" --kit {}", self.rel(&normalize(&dir.join(kit)))));
        }
        if let Some(seed) = overrides.seed {
            command.push_str(&format!(" --seed {seed}"));
        }
        if let Some(axis) = overrides.mirror.as_str() {
            command.push_str(&format!(" --mirror {axis}"));
        }
        if prefab {
            command.push_str(" --prefab");
        }
        command
    }
}

/// The overrides a `generated_from` flag list records.
fn parse_flags(tokens: &[&str]) -> Result<MapOverrides, String> {
    let mut overrides = MapOverrides::default();
    for pair in tokens.chunks(2) {
        let flag = pair[0];
        let value = pair.get(1).copied();
        let value = match value {
            Some(value)
                if ["--kit", "--seed", "--mirror"].contains(&flag) && !value.starts_with("--") =>
            {
                value
            }
            Some(_) => return Err(format!("`{flag}` is not --kit, --seed or --mirror")),
            None => {
                return Err(if ["--kit", "--seed", "--mirror"].contains(&flag) {
                    format!("`{flag}` has no value")
                } else {
                    format!("`{flag}` is not --kit, --seed or --mirror (and has no value)")
                });
            }
        };
        match flag {
            "--kit" => overrides.kit = Some(value.to_owned()),
            "--seed" => {
                overrides.seed = Some(
                    value
                        .parse()
                        .map_err(|_| format!("`--seed {value}` is not an unsigned integer"))?,
                );
            }
            _ => {
                overrides.mirror = match value {
                    "x" => Mirror::X,
                    "z" => Mirror::Z,
                    _ => return Err(format!("`--mirror {value}` is not `x` or `z`")),
                };
            }
        }
    }
    Ok(overrides)
}

/// The `generated_from:` value among a document's leading column-0 keys, or `None`. Only the head is
/// read, and the scan stops at the first indented line.
fn head_generated_from(path: &Path) -> Option<String> {
    let mut buffer = Vec::new();
    std::fs::File::open(path)
        .ok()?
        .take(HEAD_BYTES)
        .read_to_end(&mut buffer)
        .ok()?;
    for line in String::from_utf8_lossy(&buffer).split('\n') {
        if let Some(rest) = line.strip_prefix("generated_from:") {
            let value = rest.trim();
            if !value.is_empty() {
                let value = value.strip_prefix(['\'', '"']).unwrap_or(value);
                let value = value.strip_suffix(['\'', '"']).unwrap_or(value);
                return Some(value.to_owned());
            }
        }
        if line.starts_with(char::is_whitespace) && !line.trim().is_empty() {
            break;
        }
    }
    None
}

/// Calls `visit` on every file under `dir`, depth-first in name order; a missing dir is skipped.
fn walk(dir: &Path, visit: &mut dyn FnMut(&Path)) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut entries: Vec<_> = entries.flatten().collect();
    entries.sort_by_key(|e| e.file_name());
    for entry in entries {
        let path = entry.path();
        if path.is_dir() {
            walk(&path, visit);
        } else {
            visit(&path);
        }
    }
}

/// A path with `.` segments dropped and each `..` cancelling the segment before it, so two spellings
/// of one file compare equal.
fn normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for part in path.components() {
        match part {
            Component::CurDir => {}
            Component::ParentDir => {
                if !out.pop() {
                    out.push("..");
                }
            }
            other => out.push(other),
        }
    }
    if out.as_os_str().is_empty() {
        out.push(".");
    }
    out
}

fn slash(path: &Path) -> String {
    ludion_core::paths::slash(path)
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}
