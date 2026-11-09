//! Implementation of `ludion generate-map <map.yaml> [--out <scene.yaml>]`.
//!
//! A build-time compiler (like `ludion import`), no engine process: it reads a `kind: map` source,
//! resolves the `kit:` path and each `fragments:` entry **relative to the map file's directory**,
//! parses them all, sets [`MapDoc::source`] to the map file name (so the emitted scene carries
//! `generated_from:`), calls the pure, IO-free [`ludion_scene::generate`], and writes the canonical
//! scene through the built-in component registry. Deterministic: the same map input writes a
//! byte-identical scene file — the property the drift gate (`scripts/map-drift.mjs`) leans on.
//!
//! **Why it does NOT `check-scene` the emitted scene:** a generated district legitimately contains
//! game components (`Gate`, `RunState`, …) unknown to `builtin_registry()`, which would fail L1013 —
//! the same reason `games/vigil/tests/golden.rs` validates against the game's registry, not the
//! built-ins. So the self-check is structural instead: re-`parse` the emitted YAML (scene structure
//! only, no component validation) and assert **parse→emit byte-idempotence** before writing — proof
//! the writer produced canonical, re-parseable output, without needing the game's component set.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use crate::diagnostic::report;
use ludion_core::atomic::write_atomic;
use ludion_core::codes::code;
use ludion_core::diagnostics::Diagnostic;
use ludion_ecs::Registry;
use ludion_scene::{MapDoc, MapKit, MapOverrides, Mirror, PrefabDocument, Scene};

/// The fully resolved inputs of a map source: the parsed doc (with `source` set), the parsed kit,
/// and each fragment's `(path, parsed scene)` in `fragments:` order. Shared by `generate-map` and
/// `map-check`, which resolve a map identically.
pub(crate) struct LoadedMap {
    pub doc: MapDoc,
    pub kit: MapKit,
    pub fragments: Vec<(String, Scene)>,
}

/// Reads and parses a map source and everything it references, resolving `kit:` and each
/// `fragments:` entry relative to the map file's own directory.
///
/// The map is parsed first (it names the kit/fragment paths); its parse errors return immediately.
/// The kit and every fragment are then parsed with their diagnostics **accumulated across all
/// files** — one run reports every broken reference, not just the first.
pub(crate) fn load(
    map_path: &Path,
    overrides: &MapOverrides,
) -> Result<LoadedMap, Vec<Diagnostic>> {
    let source = read_file(map_path)?;
    let mut doc = ludion_scene::parse_map(&source)?;
    overrides.apply(&mut doc);
    doc.source = Some(file_name(map_path));

    let dir = map_path.parent().unwrap_or_else(|| Path::new("."));
    let mut diagnostics = Vec::new();

    let kit = match read_file(&dir.join(&doc.kit)) {
        Ok(text) => match ludion_scene::parse_map_kit(&text) {
            Ok(kit) => Some(kit),
            Err(mut errors) => {
                diagnostics.append(&mut errors);
                None
            }
        },
        Err(mut errors) => {
            diagnostics.append(&mut errors);
            None
        }
    };

    let mut fragments = Vec::with_capacity(doc.fragments.len());
    for fragment in &doc.fragments {
        match read_file(&dir.join(fragment)) {
            Ok(text) => match ludion_scene::parse(&text) {
                Ok(scene) => fragments.push((fragment.clone(), scene)),
                Err(mut errors) => diagnostics.append(&mut errors),
            },
            Err(mut errors) => diagnostics.append(&mut errors),
        }
    }

    if !diagnostics.is_empty() {
        return Err(diagnostics);
    }
    Ok(LoadedMap {
        doc,
        kit: kit.expect("kit is Some when no diagnostics were collected"),
        fragments,
    })
}

/// Runs `ludion generate-map <maps>... [--out out] [--check]`: compile each map in turn, aggregating
/// the exit code (non-zero if any input fails). `--out` names a single input's destination, so it is
/// a usage error with more than one map. `--check` regenerates in memory and compares against the
/// committed scene instead of writing.
pub fn run(
    maps: &[PathBuf],
    out: Option<&Path>,
    check: bool,
    kit: Option<&Path>,
    seed: Option<u64>,
    mirror: Option<&str>,
    prefab: bool,
) -> ExitCode {
    let mirror = match mirror {
        Some("x") => Mirror::X,
        Some("z") => Mirror::Z,
        _ => Mirror::None,
    };
    if (kit.is_some() || seed.is_some() || mirror != Mirror::None) && out.is_none() {
        return crate::diagnostic::usage_error(
            "--kit, --seed and --mirror need --out: an overridden room cannot take the map's \
             default output, which the drift gate pins to the plain compile",
        );
    }
    let out_is_prefab = out.is_some_and(|out| file_name(out).ends_with(".prefab.yaml"));
    if prefab && out.is_none() {
        return crate::diagnostic::usage_error(
            "--prefab needs --out naming a `.prefab.yaml`: a map prefab has no default output",
        );
    }
    if prefab && !out_is_prefab {
        return crate::diagnostic::usage_error(
            "--prefab writes a `kind: prefab`: name --out `<name>.prefab.yaml`",
        );
    }
    if !prefab && out_is_prefab {
        return crate::diagnostic::usage_error(
            "--out names a `.prefab.yaml` but writes a scene: pass --prefab to write a prefab, \
             or name a different --out",
        );
    }
    if out.is_some() && maps.len() > 1 {
        return crate::diagnostic::usage_error(format_args!(
            "--out names a single output and cannot be used with {} input maps; drop --out \
             to write each map to its own default path",
            maps.len()
        ));
    }

    let mut failed = false;
    for map in maps {
        let overrides = match resolve_overrides(map, kit, seed, mirror) {
            Ok(overrides) => overrides,
            Err(code) => return code,
        };
        let code = if check {
            check_one(map, out, &overrides, prefab)
        } else {
            generate_one(map, out, &overrides, prefab)
        };
        if code == ExitCode::FAILURE {
            failed = true;
        }
    }
    if failed {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}

/// The overrides as `generated_from` records them: the kit is rewritten from the working directory
/// to the map's directory, the same meaning as the map's own `kit:` value.
pub(crate) fn resolve_overrides(
    map_path: &Path,
    kit: Option<&Path>,
    seed: Option<u64>,
    mirror: Mirror,
) -> Result<MapOverrides, ExitCode> {
    let kit = match kit {
        Some(kit) => {
            let dir = map_path.parent().unwrap_or_else(|| Path::new("."));
            let relative = match ludion_import::relative_to(dir, kit, code::OUTPUT_IO) {
                Ok(relative) => relative,
                Err(diagnostic) => return Err(report(map_path, &[diagnostic])),
            };
            if relative.contains(char::is_whitespace) {
                return Err(crate::diagnostic::usage_error(format_args!(
                    "--kit `{relative}` contains whitespace, which `generated_from` cannot record; \
                     rename the kit file or its directory"
                )));
            }
            Some(relative)
        }
        None => None,
    };
    Ok(MapOverrides { kit, seed, mirror })
}

/// Generates one map and writes its canonical scene.
fn generate_one(
    map_path: &Path,
    out: Option<&Path>,
    overrides: &MapOverrides,
    prefab: bool,
) -> ExitCode {
    let out_path = out
        .map(PathBuf::from)
        .unwrap_or_else(|| default_out(map_path));
    if same_path(map_path, &out_path) {
        eprintln!(
            "{}",
            Diagnostic::new(
                code::OUTPUT_IO,
                format!(
                    "--out `{}` is the map source itself; writing there would replace the map \
                     with its generated scene",
                    out_path.display()
                ),
            )
            .with_help("name a different --out, or drop --out to write beside the map")
        );
        return ExitCode::FAILURE;
    }
    let (count, yaml) = match generate_yaml(map_path, &out_path, overrides, prefab) {
        Ok(pair) => pair,
        Err(code) => return code,
    };
    if let Err(error) = write_atomic(&out_path, &yaml) {
        eprintln!(
            "{}",
            Diagnostic::new(
                code::OUTPUT_IO,
                format!("could not write `{}`: {error}", out_path.display()),
            )
            .with_help("check the destination is writable")
        );
        return ExitCode::FAILURE;
    }

    println!(
        "{}: generated {count} entit{} -> {}",
        map_path.display(),
        if count == 1 { "y" } else { "ies" },
        out_path.display(),
    );
    ExitCode::SUCCESS
}

/// Regenerates one map in memory and compares it byte-for-byte against the committed scene at its
/// output path — the mid-loop form of the CI drift gate. Writes nothing; exits non-zero on drift or
/// if the target scene is missing/unreadable.
fn check_one(
    map_path: &Path,
    out: Option<&Path>,
    overrides: &MapOverrides,
    prefab: bool,
) -> ExitCode {
    let out_path = out
        .map(PathBuf::from)
        .unwrap_or_else(|| default_out(map_path));
    let (_count, yaml) = match generate_yaml(map_path, &out_path, overrides, prefab) {
        Ok(pair) => pair,
        Err(code) => return code,
    };

    match std::fs::read(&out_path) {
        Ok(existing) if existing == yaml.as_bytes() => {
            println!(
                "{}: up to date -> {}",
                map_path.display(),
                out_path.display()
            );
            ExitCode::SUCCESS
        }
        Ok(_) => {
            eprintln!(
                "{}",
                Diagnostic::new(
                    code::MAP_SCENE_DRIFT,
                    format!(
                        "DRIFT {}: the committed scene differs from what this map now generates",
                        out_path.display()
                    ),
                )
                .with_help("run `ludion generate-map` (without --check) to regenerate it")
            );
            ExitCode::FAILURE
        }
        Err(error) => {
            eprintln!(
                "{}",
                Diagnostic::new(
                    code::SCENE_IO,
                    format!(
                        "DRIFT {}: could not read the committed scene to compare: {error}",
                        out_path.display()
                    ),
                )
                .with_help("generate it first with `ludion generate-map` (without --check)")
            );
            ExitCode::FAILURE
        }
    }
}

/// Generates the scene and runs `check-scene`'s validation on the components the built-in registry
/// knows (a game's own components would fail `L1013`, so those are skipped), so a kit value the
/// scene check rejects — a negative collider extent — fails here, where the map is, instead of
/// panicking a downstream consumer. Shared by `generate-map` and `map-check`. A scene fault is
/// returned with the fragment file it was written in (`None`: the map), its pointer already
/// rewritten to where it lives in that file; [`report_faults`] prints them.
pub(crate) fn generate_checked(
    loaded: &LoadedMap,
    registry: &Registry,
    prefab: Option<PrefabTarget<'_>>,
) -> Result<Generated, Vec<Fault>> {
    let (scene, origins) =
        ludion_scene::generate_traced(&loaded.doc, &loaded.kit, &loaded.fragments, registry)
            .map_err(|diagnostics| {
                diagnostics
                    .into_iter()
                    .map(|d| (None, d))
                    .collect::<Vec<_>>()
            })?;
    let locate = |mut diagnostic: Diagnostic| {
        let Some((entity, rest)) = entity_pointer(diagnostic.path.as_deref()) else {
            return (None, diagnostic);
        };
        if let Some((path, local)) = fragment_entity(loaded, entity) {
            diagnostic.path = Some(format!("/entities/{local}{rest}"));
            return (Some(path.to_owned()), diagnostic);
        }
        diagnostic.path = origins.get(entity).cloned();
        (None, diagnostic)
    };
    let mut faults: Vec<_> = ludion_scene::validate(&scene, registry)
        .into_iter()
        .filter(|d| d.code != code::SCENE_UNKNOWN_COMPONENT)
        .map(&locate)
        .collect();
    let mut document = None;
    if let Some(target) = prefab {
        if loaded.doc.physics.is_some() {
            faults.push((
                None,
                Diagnostic::new(
                    code::PREFAB_DOCUMENT_STRUCTURE,
                    "a prefab cannot carry a `physics:` block",
                )
                .with_path("/physics")
                .with_help(
                    "a prefab carries no physics: the scene that spawns it owns the block; drop \
                     `physics:` from this map or generate a scene instead",
                ),
            ));
        }
        match prefab_document(&scene, target, registry) {
            Ok((doc, found)) => {
                faults.extend(found.into_iter().map(&locate));
                document = Some(doc);
            }
            Err(diagnostic) => faults.push((None, diagnostic)),
        }
    }
    if faults.is_empty() {
        Ok((scene, document))
    } else {
        Err(faults)
    }
}

/// A generated scene, with its prefab document when one was asked for.
type Generated = (Scene, Option<PrefabDocument>);

/// A fault with the fragment file it was written in (`None`: the map).
type Fault = (Option<String>, Diagnostic);

/// Where a map prefab is written, for rewriting its paths: the map's directory and the prefab's.
#[derive(Clone, Copy)]
pub(crate) struct PrefabTarget<'a> {
    pub map_dir: &'a Path,
    pub out_dir: &'a Path,
}

/// The generated scene's entities as a [`PrefabDocument`] with its paths rewritten, plus the
/// prefab-only faults (`L1227`/`L1228`/`L1226`/`L1036` from the parse, `L1229`, `L1221`/`L1222`),
/// their pointers still into the generated body.
fn prefab_document(
    scene: &Scene,
    target: PrefabTarget<'_>,
    registry: &Registry,
) -> Result<(PrefabDocument, Vec<Diagnostic>), Diagnostic> {
    let mut doc = PrefabDocument {
        name: scene.name.clone(),
        generated_from: scene.generated_from.clone(),
        clips: Vec::new(),
        entities: scene.entities.clone(),
    };
    relativize_for_prefab(&mut doc, target, registry)?;
    let text = ludion_scene::emit_prefab(&doc, registry);
    let doc = match ludion_scene::parse_prefab(&text) {
        Ok(doc) => doc,
        Err(diagnostics) => return Ok((doc, diagnostics)),
    };
    let body = doc.to_scene();
    let found = ludion_scene::validate_prefab(&doc, registry)
        .into_iter()
        .filter(|d| d.code != code::SCENE_UNKNOWN_COMPONENT)
        .chain(ludion_engine::prefab_singleton_diagnostics(&body))
        .chain(ludion_engine::scene_hierarchy_body_diagnostics(&body))
        .filter(|d| d.severity.is_error())
        .collect();
    Ok((doc, found))
}

/// Rewrites every asset path from relative-to-the-map to relative-to-the-prefab: a scene's paths
/// resolve against the scene file's directory, a prefab's against the prefab file. Components the
/// registry does not know (a game's own) are copied as written.
fn relativize_for_prefab(
    doc: &mut PrefabDocument,
    target: PrefabTarget<'_>,
    registry: &Registry,
) -> Result<(), Diagnostic> {
    let mut failure = None;
    for entity in &mut doc.entities {
        rewrite_components(&mut entity.components, target, registry, &mut failure);
        if let Some(instance) = entity.instance.as_deref_mut() {
            rewrite_path(&mut instance.prefab, target, &mut failure);
            for overrides in instance.overrides.values_mut() {
                rewrite_components(overrides, target, registry, &mut failure);
            }
        }
    }
    failure.map_or(Ok(()), Err)
}

fn rewrite_components(
    components: &mut [(String, serde_json::Value)],
    target: PrefabTarget<'_>,
    registry: &Registry,
    failure: &mut Option<Diagnostic>,
) {
    for (name, value) in components {
        if let Some(info) = registry.get(name) {
            info.for_each_asset_path(value, &mut |_, path| rewrite_path(path, target, failure));
        }
    }
}

fn rewrite_path(path: &mut String, target: PrefabTarget<'_>, failure: &mut Option<Diagnostic>) {
    if path.is_empty() || failure.is_some() {
        return;
    }
    match ludion_import::relative_to(
        target.out_dir,
        &target.map_dir.join(&*path),
        code::OUTPUT_IO,
    ) {
        Ok(relative) => *path = relative,
        Err(diagnostic) => *failure = Some(diagnostic),
    }
}

/// Splits a generated-scene pointer `/entities/{n}{rest}` into `n` and `rest`.
pub(crate) fn entity_pointer(pointer: Option<&str>) -> Option<(usize, &str)> {
    let tail = pointer?.strip_prefix("/entities/")?;
    let digits = tail
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(tail.len());
    Some((tail[..digits].parse().ok()?, &tail[digits..]))
}

/// The fragment (as written in `fragments:`) and fragment-local index of generated entity `index`,
/// when it is a fragment entity (those are copied first, verbatim, in `fragments:` order).
pub(crate) fn fragment_entity(loaded: &LoadedMap, mut index: usize) -> Option<(&str, usize)> {
    for (path, scene) in &loaded.fragments {
        if index < scene.entities.len() {
            return Some((path, index));
        }
        index -= scene.entities.len();
    }
    None
}

/// Prints [`generate_checked`]'s faults, each against the file it belongs to, and returns the exit
/// code.
pub(crate) fn report_faults(
    map_path: &Path,
    faults: Vec<(Option<String>, Diagnostic)>,
) -> ExitCode {
    let dir = map_path.parent().unwrap_or_else(|| Path::new("."));
    let mut groups: Vec<(PathBuf, Vec<Diagnostic>)> = Vec::new();
    for (fragment, diagnostic) in faults {
        let source = fragment.map_or_else(|| map_path.to_path_buf(), |path| dir.join(path));
        match groups.iter_mut().find(|(path, _)| *path == source) {
            Some((_, found)) => found.push(diagnostic),
            None => groups.push((source, vec![diagnostic])),
        }
    }
    for (source, found) in &groups {
        report(source, found);
    }
    ExitCode::FAILURE
}

/// Loads, generates, and canonically emits a map (or, with `prefab`, its `kind: prefab`), running
/// the parse→emit idempotence self-check. Returns the entity count and the YAML. Shared by
/// `generate_one` (which writes the YAML) and `check_one` (which compares it).
fn generate_yaml(
    map_path: &Path,
    out_path: &Path,
    overrides: &MapOverrides,
    prefab: bool,
) -> Result<(usize, String), ExitCode> {
    let mut loaded = match load(map_path, overrides) {
        Ok(loaded) => loaded,
        Err(diagnostics) => return Err(report(map_path, &diagnostics)),
    };
    // `generated_from` is read from the scene's own directory: beside the map it is the bare file
    // name, an `--out` elsewhere names the map relative to it.
    let out_dir = out_path.parent().unwrap_or_else(|| Path::new("."));
    match ludion_import::relative_to(out_dir, map_path, code::OUTPUT_IO) {
        Ok(source) if source.contains(char::is_whitespace) => {
            return Err(crate::diagnostic::usage_error(format_args!(
                "the map path `{source}` contains whitespace, which `generated_from` cannot \
                 record; rename the map file or its directory"
            )));
        }
        Ok(source) => loaded.doc.source = Some(overrides.provenance(&source)),
        Err(diagnostic) => return Err(report(map_path, &[diagnostic])),
    }
    let registry = ludion_engine::builtin_registry();
    let target = prefab.then(|| PrefabTarget {
        map_dir: map_path.parent().unwrap_or_else(|| Path::new(".")),
        out_dir,
    });
    let (scene, document) = match generate_checked(&loaded, &registry, target) {
        Ok(generated) => generated,
        Err(faults) => return Err(report_faults(map_path, faults)),
    };
    let count = scene.entities.len();
    let yaml = match &document {
        Some(doc) => ludion_scene::emit_prefab(doc, &registry),
        None => ludion_scene::emit(&scene, &registry),
    };
    if let Err(diagnostics) = self_check_idempotent(&yaml, prefab, &registry) {
        return Err(report(map_path, &diagnostics));
    }
    Ok((count, yaml))
}

/// Re-parses the emitted YAML (scene structure only — never `check`, which the game components
/// would fail as L1013) and asserts it re-emits byte-for-byte, proving the generator produced
/// canonical output. Defensive: on a byte mismatch (or an unparseable emit) it returns a diagnostic
/// rather than writing a non-idempotent file the drift gate would then reject.
fn self_check_idempotent(
    yaml: &str,
    prefab: bool,
    registry: &Registry,
) -> Result<(), Vec<Diagnostic>> {
    let reemitted = if prefab {
        ludion_scene::emit_prefab(&ludion_scene::parse_prefab(yaml)?, registry)
    } else {
        ludion_scene::emit(&ludion_scene::parse(yaml)?, registry)
    };
    if reemitted != yaml {
        return Err(vec![
            Diagnostic::new(
                code::MAP_DOCUMENT_STRUCTURE,
                "the generated scene did not survive a parse→emit round-trip (an internal \
                 generator invariant failed); the file was not written",
            )
            .with_help("this is a bug in the map generator — please report it"),
        ]);
    }
    Ok(())
}

/// Whether two paths name the same file: by identity when both exist, else by lexical spelling.
fn same_path(a: &Path, b: &Path) -> bool {
    if let (Ok(a), Ok(b)) = (a.canonicalize(), b.canonicalize()) {
        return a == b;
    }
    lexical(a) == lexical(b)
}

fn lexical(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for part in std::path::absolute(path)
        .unwrap_or_else(|_| path.to_path_buf())
        .components()
    {
        match part {
            std::path::Component::ParentDir => {
                out.pop();
            }
            std::path::Component::CurDir => {}
            other => out.push(other),
        }
    }
    out
}

/// The default output path: the map file with the `.map` component of its `.map.yaml` extension
/// dropped (`district-plaza.map.yaml` → `district-plaza.yaml`).
fn default_out(map_path: &Path) -> PathBuf {
    let name = file_name(map_path);
    let out_name = name.strip_suffix(".map.yaml").map_or_else(
        || match name.strip_suffix(".yaml") {
            // A `.yaml` that is not `.map.yaml`: drop `.yaml`, add `.scene.yaml` so we never
            // overwrite the source in place.
            Some(stem) => format!("{stem}.scene.yaml"),
            None => format!("{name}.yaml"),
        },
        |stem| format!("{stem}.yaml"),
    );
    map_path
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join(out_name)
}

/// Reads a referenced file to a string, mapping an IO error to an `L1015` diagnostic naming it.
fn read_file(path: &Path) -> Result<String, Vec<Diagnostic>> {
    std::fs::read_to_string(path).map_err(|error| {
        vec![
            Diagnostic::new(
                code::SCENE_IO,
                format!("could not read `{}`: {error}", path.display()),
            )
            .with_help("check the path (resolved relative to the map file) and permissions"),
        ]
    })
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}
