//! Implementation of `ludion check-scene <FILE>`, plus the read → parse → validate pipeline and
//! the engine-construction/recording helpers it shares with `ludion sim-hash` and `ludion serve`.

use std::fs::File;
use std::path::Path;
use std::process::ExitCode;

use crate::placement::{self, PlacementOptions};
use crate::{clip_report, diagnostic};
use ludion_core::codes::code;
use ludion_core::diagnostics::{Diagnostic, pointer_token};
use ludion_ecs::Registry;
use ludion_engine::Engine;
use ludion_scene::Scene;

/// The voxel authoring-view flags (`--mirror-x`, `--slice`, `--column`, `--exposure`). They apply
/// only to `kind: voxel` and `kind: assembly` documents; passing any against another input is a
/// readable usage error.
#[derive(Debug, Default, Clone)]
pub struct VoxelViews {
    /// `--mirror-x`: report X-mirror symmetry (informational; never changes the exit code).
    pub mirror_x: bool,
    /// `--slice <y>`: print one annotated layer, or `all` for every layer bottom-up.
    pub slice: Option<String>,
    /// `--column <x,z>`: print the vertical profile at a single column.
    pub column: Option<String>,
    /// `--exposure`: report what of the model is visible — the surface/fully-enclosed split per
    /// glyph, and each emissive glyph's exposed faces by direction.
    pub exposure: bool,
    /// `--from <view>`: which viewpoint `--exposure` judges emissive faces from. Meaningful only
    /// alongside `exposure` (clap enforces that), and defaults to the direction-only report.
    pub from: ludion_scene::ExposureView,
}

impl VoxelViews {
    /// Whether any voxel-view flag was requested.
    fn any(&self) -> bool {
        self.mirror_x || self.slice.is_some() || self.column.is_some() || self.exposure
    }
}

/// The model authoring-view flags (`--bounds`, `--json`). `--bounds` applies to `kind: sculpt`
/// documents and to a `kind: mesh` document too, where it prints the document's own model-space box
/// (`Plan/mesh-texture-editing.md` — placing a region primitive is that question exactly; a
/// skinned mesh adds its joint tree), and to a scene, where it prints the resolved collision-layer
/// matrix (`--json` adds the same fields to the summary object).
/// Passing it against another kind is a readable usage error, the [`VoxelViews`] shape.
#[derive(Debug, Default, Clone)]
pub struct SculptViews {
    /// `--bounds`: print per-part and per-bone world-unit bounds on a sculpt, the vertex AABB on a
    /// mesh (plus the joint tree of a skinned one), or the resolved layer matrix on a scene.
    pub bounds: bool,
    /// `--json`: with `--bounds`, add a scene's layer matrix or a mesh's box and joints to the
    /// summary object; alone, an animation clip's numeric report. Any other document refuses it.
    pub json: bool,
}

impl SculptViews {
    /// Whether any sculpt-view flag was requested.
    fn any(&self) -> bool {
        self.bounds
    }
}

/// One `check-scene` run's report: text on stdout/stderr as always, or, under `--json`, the one
/// summary object `{path, kind, ok, …the kind's fields, diagnostics}` on stdout. The text path is
/// byte-identical either way; diagnostics still render on stderr in both modes.
pub(crate) struct Reporter<'a> {
    pub(crate) path: &'a Path,
    kind: Option<String>,
    json: bool,
    diagnostics: Vec<Diagnostic>,
}

impl<'a> Reporter<'a> {
    pub(crate) fn new(path: &'a Path, kind: Option<String>, json: bool) -> Self {
        Self {
            path,
            kind,
            json,
            diagnostics: Vec::new(),
        }
    }

    /// Prints a warning to stderr and keeps it for the summary object.
    pub(crate) fn warn(&mut self, warning: Diagnostic) {
        diagnostic::print(&warning);
        self.diagnostics.push(warning);
    }

    /// Reports a failed check: the diagnostics render to stderr, the failure object (no kind
    /// fields) goes to stdout under `--json`.
    pub(crate) fn fail(&mut self, diagnostics: Vec<Diagnostic>) -> ExitCode {
        debug_assert!(
            diagnostics.iter().any(|d| d.severity.is_error()),
            "fail needs at least one error"
        );
        diagnostic::report(self.path, &diagnostics);
        self.diagnostics.extend(diagnostics);
        self.emit(false, serde_json::Value::Null);
        ExitCode::FAILURE
    }

    /// Reports success: under `--json` the summary object carrying `fields` (a JSON object), else
    /// `text` prints the usual lines.
    pub(crate) fn ok(self, fields: serde_json::Value, text: impl FnOnce()) -> ExitCode {
        if self.json {
            self.emit(true, fields);
        } else {
            text();
        }
        ExitCode::SUCCESS
    }

    fn emit(&self, ok: bool, fields: serde_json::Value) {
        if !self.json {
            return;
        }
        let mut object = serde_json::Map::new();
        object.insert("path".into(), self.path.display().to_string().into());
        object.insert("kind".into(), self.kind.clone().into());
        object.extend(crate::diagnostic::json_envelope(
            ok,
            fields,
            &self.diagnostics,
        ));
        println!("{}", serde_json::Value::Object(object));
    }
}

/// Runs `ludion check-scene <path> [--mirror-x] [--slice <y|all>] [--column <x,z>] [--exposure]
/// [--bounds [--json]] [--placement]`. `placement` (scenes only, a usage error on any other kind)
/// adds the placement lint's warnings and summary line.
///
/// Dispatches on the document's top-level `kind:` key through the `ludion_scene::KINDS` table: a
/// document declaring `kind: mesh`/`voxel`/`part`/`animation`/`palette`/`atlas-source`/`sculpt`/`prefab` is
/// validated as that single-file document family with a rich summary, any other kind the table
/// says `ludion-scene` checks (`mesh-edit`, `voxel-ops`, `generation`, `origin`) is validated with a
/// bare `OK`, and one checked elsewhere (a map, a profile) is a usage error naming the verb that
/// does; `kind: assembly` runs the full multi-file pass (the assembly
/// document plus every part file it names, meshed), `kind: atlas` the two-file one (the document plus
/// the packed image its regions address), and `kind: sprite` is single-file until it carries a
/// shared-palette include, which is read and merged before the sprite is validated; a document with no
/// `kind` key at all is a scene, validated as today; a `kind` in no row is `L1119`. Scene documents
/// never declare `kind` (the scene format's own unknown-key rule, `L1010`, keeps it out of the scene
/// skeleton), so these document families never collide.
///
/// The voxel-view flags in `views` apply to `kind: voxel` documents (printed before the closing
/// `OK (…)` summary) and to `kind: assembly` documents (printed after the assembly's own report,
/// running over the parts *composed* into one assembly-global grid at rest pose —
/// `Plan/voxel-animation.md` decision 11); the sculpt-view flags in `sculpt_views` apply to
/// `kind: sculpt` documents, and `--bounds` also to `kind: mesh` documents and scenes, printed the
/// same way. Passing either family against a kind it does not answer to is a readable usage error.
///
/// Either path prints a one-line success summary on success, or every diagnostic (rustc-style:
/// L-code, message, JSON Pointer, suggestion) to stderr and a non-zero exit code on failure.
pub fn run(
    path: &Path,
    views: &VoxelViews,
    sculpt_views: &SculptViews,
    placement: Option<PlacementOptions>,
) -> ExitCode {
    let source = match std::fs::read_to_string(path) {
        Ok(source) => source,
        Err(error) => {
            let diagnostic = Diagnostic::new(
                code::SCENE_IO,
                format!("could not read `{}`: {error}", path.display()),
            )
            .with_help("check the path (relative to the current directory) and permissions");
            return Reporter::new(path, None, sculpt_views.json).fail(vec![diagnostic]);
        }
    };
    let kind = ludion_scene::peek_kind(&source);
    if let (Some(_), Some(kind)) = (&placement, &kind) {
        return diagnostic::usage_error(format_args!(
            "--placement applies only to scenes; this document is a {kind}"
        ));
    }
    if sculpt_views.json {
        match kind.as_deref() {
            Some("voxel" | "assembly") if views.any() => {
                return diagnostic::usage_error(
                    "--json prints the summary object; --mirror-x, --slice, --column and --exposure are text views",
                );
            }
            // The bounds fields exist only on a scene or a mesh.
            Some(other) if sculpt_views.bounds && other != "mesh" => {
                return diagnostic::usage_error(format_args!(
                    "--json applies only to --bounds on a scene or a mesh document; this document is a {other}"
                ));
            }
            _ => {}
        }
    }
    let mut reporter = Reporter::new(
        path,
        Some(kind.clone().unwrap_or_else(|| "scene".to_string())),
        sculpt_views.json,
    );
    match kind {
        Some(kind) if kind == "voxel" && !sculpt_views.any() => run_voxel(reporter, &source, views),
        // `kind: assembly` runs the same authoring views over its *composed* grid
        // (`Plan/voxel-animation.md` decision 11) — checked before the catch-all usage error below,
        // so this is the one other kind the voxel-view flags are legal against.
        Some(kind) if kind == "assembly" && !sculpt_views.any() => {
            run_assembly(reporter, &source, views)
        }
        // `kind: sculpt` is the one kind the sculpt-view flags are legal against; a voxel-view flag
        // here falls through to the catch-all below, which already names "sculpt" correctly.
        Some(kind) if kind == "sculpt" && !views.any() => {
            run_sculpt(reporter, &source, sculpt_views)
        }
        // `--bounds` is the one sculpt-view flag a mesh document answers: it has vertices, so it has
        // a box, and "where is this model in its own space" is what an author placing anything on it
        // asks first.
        Some(kind) if kind == "mesh" && !views.any() => run_mesh(reporter, &source, sculpt_views),
        // `--bounds` on a scene prints the resolved collision-layer matrix; the voxel flags still
        // fall through to the usage error below.
        None if sculpt_views.bounds && !views.any() => {
            run_scene(reporter, &source, true, placement.as_ref())
        }
        other if views.any() || sculpt_views.any() => {
            // `--mirror-x`/`--slice`/`--column`/`--exposure` are voxel/assembly-only and `--bounds`
            // is sculpt/mesh/scene-only; on the wrong kind either is a usage error, named the same
            // way for a voxel, an unknown kind, and an assembly.
            let kind = other.as_deref().unwrap_or("scene").to_string();
            if views.any() {
                diagnostic::usage_error(voxel_flags_on_non_voxel(&kind));
            }
            if sculpt_views.any() {
                diagnostic::usage_error(sculpt_flags_on_non_sculpt(&kind));
            }
            ExitCode::FAILURE
        }
        Some(kind) if kind == "part" => run_part(reporter, &source),
        Some(kind) if kind == "animation" => clip_report::run(reporter, &source),
        Some(kind) if kind == "sprite" => run_sprite(reporter, &source),
        Some(kind) if kind == "palette" => run_palette(reporter, &source),
        Some(kind) if kind == "atlas-source" => run_atlas_source(reporter, &source),
        Some(kind) if kind == "atlas" => run_atlas(reporter, &source),
        Some(kind) if kind == "font-source" => run_font_source(reporter, &source),
        Some(kind) if kind == "font" => run_font(reporter, &source),
        Some(kind) if kind == "character" => run_character(reporter, &source),
        Some(kind) if kind == "prefab" => run_prefab(reporter, &source),
        Some(other) => match ludion_scene::kind_entry(&other) {
            Some(entry) => match entry.check {
                ludion_scene::KindCheck::Here(check) => {
                    run_checked(reporter, &source, entry, check)
                }
                ludion_scene::KindCheck::Elsewhere(verb) => {
                    let diagnostic = Diagnostic::new(
                        code::KIND_CHECKED_ELSEWHERE,
                        format!(
                            "check-scene does not validate `kind: {}` documents ({})",
                            entry.name, entry.noun
                        ),
                    )
                    .with_path("/kind")
                    .with_help(verb);
                    reporter.fail(vec![diagnostic])
                }
            },
            None => reporter.fail(vec![unrecognized_kind(&other)]),
        },
        None => run_scene(reporter, &source, false, placement.as_ref()),
    }
}

/// The usage message for `--mirror-x`/`--slice`/`--column`/`--exposure` passed against a document
/// that is neither a voxel model nor an assembly.
fn voxel_flags_on_non_voxel(kind: &str) -> String {
    format!(
        "--mirror-x, --slice, --column, and --exposure apply only to voxel and assembly documents \
         (kind: voxel, kind: assembly); this document is a {kind}"
    )
}

/// The usage message for `--bounds` passed against a document it does not answer to (not a sculpt, a
/// mesh, or a scene).
fn sculpt_flags_on_non_sculpt(kind: &str) -> String {
    format!(
        "--bounds applies only to sculpt documents (kind: sculpt), and also to mesh documents \
         (kind: mesh) and scenes; this document is a {kind}"
    )
}

/// Validates a scene, including every `SkinBone` part's link to its skin and bone. `bounds` is
/// `--bounds`: the resolved collision-layer matrix, printed before the `OK` line as text, or as the
/// `layers` field of the summary object under `--json`. `placement` is `--placement`: the lint's
/// warnings go to stderr and its summary line to stdout (a `placement` field under `--json`).
fn run_scene(
    mut reporter: Reporter,
    source: &str,
    bounds: bool,
    placement: Option<&PlacementOptions>,
) -> ExitCode {
    let path = reporter.path;
    let registry = builtin_registry();
    let scene = match ludion_scene::check(source, &registry) {
        Ok(scene) => scene,
        Err(mut diagnostics) => {
            add_project_component_help(path, &mut diagnostics);
            return reporter.fail(diagnostics);
        }
    };
    // Prefab instances expand first, so the engine's passes see what loads; each diagnostic is
    // mapped back to the document. The passes read other files, so they are rooted at the scene's
    // own directory, the content root a host boots with: a scene checks exactly as it loads. They
    // live in `ludion-engine` because a game binary's `--check-scene` and the wire `scene.check`
    // run the same list.
    let expanded = match ludion_engine::expand_scene(scene_root(path), &registry, &scene) {
        Ok(expanded) => expanded,
        Err(errors) => return reporter.fail(errors),
    };
    let located = |errors: Vec<Diagnostic>| -> Vec<Diagnostic> {
        errors.into_iter().map(|d| expanded.locate(d)).collect()
    };
    let scene = &expanded.scene;
    let font_faults = ludion_engine::font_table_faults(scene_root(path));
    if !font_faults.is_empty() {
        for (file, fault) in font_faults {
            let fault = [fault];
            diagnostic::report(&scene_root(path).join(file), &fault);
            reporter.diagnostics.extend(fault);
        }
        reporter.emit(false, serde_json::Value::Null);
        return ExitCode::FAILURE;
    }
    let found = ludion_engine::scene_engine_diagnostics(scene_root(path), scene);
    if let Some(exit) = fail_or_warn(&mut reporter, located(found)) {
        return exit;
    }
    let placement_report = placement.map(|options| {
        let report = placement::lint(scene_root(path), &expanded, options);
        for warning in &report.diagnostics {
            reporter.warn(warning.clone());
        }
        (report, options)
    });
    let components: usize = scene
        .entities
        .iter()
        .map(|entity| entity.components.len())
        .sum();
    let entity_count = scene.entities.len();
    let mut fields = serde_json::json!({ "entities": entity_count, "components": components });
    if let Some((report, _)) = &placement_report {
        fields["placement"] =
            serde_json::json!({ "groups": report.groups, "unmeasured": report.unmeasured });
    }
    if bounds {
        fields["layers"] = layers_json(scene);
    }
    reporter.ok(fields, || {
        if bounds {
            print!("{}", render_layer_matrix(scene));
        }
        if let Some((report, options)) = &placement_report {
            println!("{}", report.summary(options));
        }
        println!(
            "{}: OK ({entity_count} entit{}, {components} component{})",
            path.display(),
            if entity_count == 1 { "y" } else { "ies" },
            if components == 1 { "" } else { "s" }
        );
    })
}

/// Prints a pass's diagnostics through the reporter: `Some(FAILURE)` when one is an error, else each
/// is kept as a warning and the caller carries on.
fn fail_or_warn(reporter: &mut Reporter, diagnostics: Vec<Diagnostic>) -> Option<ExitCode> {
    if diagnostics.iter().any(|d| d.severity.is_error()) {
        return Some(reporter.fail(diagnostics));
    }
    for warning in diagnostics {
        reporter.warn(warning);
    }
    None
}

/// Every resolved layer as `(name, ignored names)` in matrix order, `default` first. Ignoring is
/// symmetric, so a layer lists the ones that ignore it as well as the ones it names.
fn resolved_layers(scene: &Scene) -> Vec<(String, Vec<String>)> {
    let Some(physics) = &scene.physics else {
        return vec![(ludion_scene::DEFAULT_LAYER.to_string(), Vec::new())];
    };
    let matrix = physics.layer_matrix();
    let names = matrix.names();
    names
        .iter()
        .enumerate()
        .map(|(i, name)| {
            let ignored = names
                .iter()
                .enumerate()
                .filter(|&(j, _)| !matrix.collides(i, j))
                .map(|(_, other)| other.clone())
                .collect();
            (name.clone(), ignored)
        })
        .collect()
}

/// The `--bounds` layer report for a scene: a `layers:` header and one line per layer, or one line
/// saying none are declared.
fn render_layer_matrix(scene: &Scene) -> String {
    if scene.physics.as_ref().is_none_or(|p| p.layers.is_empty()) {
        return "layers: none declared — every collider collides with every other\n".to_string();
    }
    let mut out = String::from("layers:\n");
    for (name, ignored) in resolved_layers(scene) {
        if ignored.is_empty() {
            out.push_str(&format!("  {name}: collides with every layer\n"));
        } else {
            out.push_str(&format!("  {name}: ignores {}\n", ignored.join(", ")));
        }
    }
    out
}

/// The `layers` field under `--bounds --json`: `[{"name":…,"ignores":[…]}]`, resolved and
/// symmetric, in matrix order.
fn layers_json(scene: &Scene) -> serde_json::Value {
    resolved_layers(scene)
        .into_iter()
        .map(|(name, ignores)| serde_json::json!({ "name": name, "ignores": ignores }))
        .collect()
}

/// The content root a scene's asset paths resolve against: the scene file's own parent directory,
/// exactly as `EngineBuilder::load_scene_file` roots a hosted game.
fn scene_root(path: &Path) -> &Path {
    path.parent().unwrap_or_else(|| Path::new("."))
}

/// Points the first `L1013` (top level, then its `related`) at the game's own `--check-scene` when
/// `scene_path` sits in a project: this CLI links no game code, so it cannot know the game's
/// components. Appends to the help the registry already wrote; later `L1013`s stay as they are.
pub(crate) fn add_project_component_help(scene_path: &Path, diagnostics: &mut [Diagnostic]) {
    let absolute = std::path::absolute(scene_path).unwrap_or_else(|_| scene_path.to_path_buf());
    let Some(root) = ludion_engine::manifest::find(scene_root(&absolute)) else {
        return;
    };
    let relative = absolute
        .strip_prefix(&root)
        .unwrap_or(scene_path)
        .to_string_lossy()
        .replace('\\', "/");
    let sentence = format!(
        "this CLI knows only engine components; for your game's own, run `cargo run -- --scene {relative} --check-scene` from the project"
    );
    for diagnostic in diagnostics.iter_mut() {
        if append_to_first_unknown_component(diagnostic, &sentence) {
            return;
        }
    }
}

/// Appends `sentence` to the first `L1013` in `diagnostic` or its `related`; whether one was found.
fn append_to_first_unknown_component(diagnostic: &mut Diagnostic, sentence: &str) -> bool {
    if diagnostic.code == code::SCENE_UNKNOWN_COMPONENT {
        *diagnostic = diagnostic.clone().append_help(sentence);
        return true;
    }
    diagnostic
        .related
        .iter_mut()
        .any(|related| append_to_first_unknown_component(related, sentence))
}

/// Validates a mesh asset document. `L1112` (the vertex-count style budget) is registered as a
/// warning in the code table, so it renders `warning[L1112]` and never turns success into failure.
///
/// `--bounds` prints the document's own model-space box (and a skinned mesh's joint tree) between
/// the warning and the closing summary, exactly where a sculpt's per-part block goes; with `--json`
/// the box and joints join the summary object.
fn run_mesh(mut reporter: Reporter, source: &str, views: &SculptViews) -> ExitCode {
    let mesh = match ludion_scene::check_mesh(source) {
        Ok(mesh) => mesh,
        Err(diagnostics) => return reporter.fail(diagnostics),
    };
    if let Some(warning) = ludion_scene::mesh_budget_warning(&mesh) {
        reporter.warn(warning);
    }
    let path = reporter.path;
    let fields = if views.bounds {
        mesh_bounds_json(&mesh)
    } else {
        serde_json::json!({
            "vertices": mesh.vertex_count(),
            "triangles": mesh.triangle_count(),
        })
    };
    reporter.ok(fields, || {
        if views.bounds {
            print!("{}", render_mesh_bounds(&mesh));
        }
        println!(
            "{}: OK ({} vertices, {} triangles)",
            path.display(),
            mesh.vertex_count(),
            mesh.triangle_count()
        );
    })
}

/// Validates a voxel model document. `L1115` (the cell-count style budget) and `L1129` (stray
/// components) are the mesh path's `L1112` mechanism exactly (see [`run_mesh`]): table-registered
/// warnings that report and never turn success into failure.
fn run_voxel(mut reporter: Reporter, source: &str, views: &VoxelViews) -> ExitCode {
    let voxel = match ludion_scene::check_voxel(source) {
        Ok(voxel) => voxel,
        Err(diagnostics) => return reporter.fail(diagnostics),
    };
    if let Some(warning) = ludion_scene::voxel_budget_warning(&voxel) {
        reporter.warn(warning);
    }
    // One decode and one flood fill answer both the warning and the summary's component count; the
    // two convenience calls would walk the whole grid twice over.
    let (components, connectivity) = voxel.connectivity();
    if let Some(warning) = connectivity {
        reporter.warn(warning);
    }
    // The authoring views print between the warnings and the closing `OK (…)` summary. A malformed
    // `--slice`/`--column` argument (out of range, unparsable) is a usage error: report it and stop
    // before the summary, leaving the model itself validated.
    if let Err(failure) = print_voxel_views(&voxel, views, None) {
        return failure.exit();
    }
    let [x, y, z] = voxel.size;
    let filled = voxel.filled_count();
    // What the model actually costs the renderer, the way a mesh document reports its own counts.
    // Never behind a flag: a number that exists to inform `budget_exempt:` is useless to an author
    // who does not know to ask for it, and `L1115`'s cell count is not that number — greedy meshing
    // collapses a wall of cells into one quad.
    let mesh = ludion_engine::voxel_mesh_stats(&voxel);
    let fields = serde_json::json!({
        "name": voxel.name,
        "size": voxel.size,
        "filled": filled,
        "components": components,
        "meshed": {
            "quads": mesh.quads,
            "triangles": mesh.triangles,
            "vertices": mesh.vertices,
        },
        "scale": voxel.scale,
        "generated_from": voxel.generated_from,
        "budget_exempt": voxel.budget_exempt == Some(true),
    });
    let path = reporter.path;
    reporter.ok(fields, || {
        // The declared intended world scale is metadata (never engine-applied), so it is reported
        // as `scale <n>` and omitted entirely when the model declares none.
        let scale = match voxel.scale {
            Some(scale) => format!(", scale {scale}"),
            None => String::new(),
        };
        // The two provenance keys are metadata too, and each changes how the file should be
        // treated: `generated` means the script is the source of truth (edit that, not this),
        // `budget-exempt` means the silence of `L1115` was authored rather than earned.
        let generated = match &voxel.generated_from {
            Some(source) => format!(", generated from {source}"),
            None => String::new(),
        };
        let exempt = if voxel.budget_exempt == Some(true) {
            ", budget-exempt"
        } else {
            ""
        };
        println!(
            "{}: OK ({x}x{y}x{z}, {filled} filled voxel{}, {components} component{}, meshed {} \
             quads / {} triangles / {} vertices{scale}{generated}{exempt})",
            path.display(),
            if filled == 1 { "" } else { "s" },
            if components == 1 { "" } else { "s" },
            mesh.quads,
            mesh.triangles,
            mesh.vertices
        );
    })
}

/// Validates a sculpt document (`kind: sculpt`, an implicit-surface model — `Plan/mesh-modeling.md`
/// decision 9): prints the vertex-budget warning (`L1112`, `ludion_engine::sculpt_budget_warning`,
/// the [`run_voxel`] precedent), meshes it and prints the mesher's own `L1158`/`L1159` warnings, then
/// the `--bounds` authoring view, then the closing `OK (…)` summary.
fn run_sculpt(mut reporter: Reporter, source: &str, views: &SculptViews) -> ExitCode {
    let path = reporter.path;
    let doc = match ludion_scene::check_sculpt(source) {
        Ok(doc) => doc,
        Err(diagnostics) => return reporter.fail(diagnostics),
    };
    // A `shape: voxel` part's source is read relative to this document's own directory, so the
    // document is checked exactly where it sits — the `run_assembly` rooting.
    let root = path.parent().unwrap_or_else(|| Path::new("."));
    let file_name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string());
    let sources = match ludion_engine::read_sculpt_sources(root, &file_name, &doc) {
        Ok(sources) => sources,
        Err(diagnostics) => return reporter.fail(diagnostics),
    };
    let (result, elapsed_ms) = timed(|| ludion_sculpt::mesh_with(&doc, &sources));
    let mesh = match result {
        Ok(mesh) => mesh,
        Err(diagnostic) => return reporter.fail(vec![diagnostic]),
    };
    if let Some(warning) = ludion_engine::sculpt_budget_warning(&doc, &mesh.stats) {
        reporter.warn(warning);
    }
    for warning in ludion_sculpt::unresolved_parts(&doc, &sources, &mesh) {
        reporter.warn(warning);
    }
    let positions = flat_positions(&mesh.positions);
    let (_, warning) = ludion_sculpt::connectivity::connectivity(&positions, &mesh.indices);
    if let Some(warning) = warning {
        reporter.warn(warning);
    }

    // The authoring view prints between the mesher's warnings and the closing `OK (…)` summary, the
    // `print_voxel_views` placement.
    let (lo, hi) = mesh.stats.extent;
    let grid: [u32; 3] =
        std::array::from_fn(|axis| ((hi[axis] - lo[axis]) * doc.resolution as f32).round() as u32);
    let fields = serde_json::json!({
        "parts": doc.parts.len(),
        "bones": doc.bones.len(),
        "grid": grid,
        "resolution": doc.resolution,
        "triangles": mesh.stats.triangles_after,
        "triangles_before_decimation": mesh.stats.triangles_before,
        "vertices": mesh.stats.vertices,
        "meshed_ms": elapsed_ms as u64,
    });
    reporter.ok(fields, || {
        print_sculpt_views(&mesh, views);
        println!(
            "{}: OK (sculpt, {} part{}, {} bone{}, grid {}x{}x{} @ {}/unit, {} triangles after \
             decimation from {}, {} vertices, meshed in {elapsed_ms} ms)",
            path.display(),
            doc.parts.len(),
            if doc.parts.len() == 1 { "" } else { "s" },
            doc.bones.len(),
            if doc.bones.len() == 1 { "" } else { "s" },
            grid[0],
            grid[1],
            grid[2],
            doc.resolution,
            mesh.stats.triangles_after,
            mesh.stats.triangles_before,
            mesh.stats.vertices,
        );
    })
}

/// Reinterprets a mesher's flat, 3-per-vertex position list as one `[f32; 3]` per vertex — the shape
/// [`ludion_sculpt::connectivity::connectivity`] takes.
fn flat_positions(positions: &[f32]) -> Vec<[f32; 3]> {
    positions
        .chunks_exact(3)
        .map(|c| [c[0], c[1], c[2]])
        .collect()
}

/// Prints the sculpt `--bounds` authoring view before the closing `OK (…)` summary — the
/// [`print_voxel_views`] shape.
fn print_sculpt_views(mesh: &ludion_sculpt::SculptMesh, views: &SculptViews) {
    if views.bounds {
        print!("{}", render_sculpt_bounds(&mesh.stats));
    }
}

/// The `--bounds` report: one line per part (name, `min..max` in world units, surface vertex count,
/// or "no surface" for a part the decimated mesh contributes nothing to), then one line per bone (name,
/// the union of its parts' bounds, or "no surface").
fn render_sculpt_bounds(stats: &ludion_sculpt::SculptStats) -> String {
    let mut out = String::new();
    out.push_str("bounds:\n");
    for part in &stats.parts {
        out.push_str(&format!(
            "  part {}: {}\n",
            part.name,
            format_bounds(part.bounds)
        ));
        if part.bounds.is_some() {
            out.push_str(&format!(
                "    {} surface {}\n",
                part.surface_vertices,
                if part.surface_vertices == 1 {
                    "vertex"
                } else {
                    "vertices"
                }
            ));
        }
    }
    for bone in &stats.bones {
        out.push_str(&format!(
            "  bone {}: {}\n",
            bone.name,
            format_bounds(bone.bounds)
        ));
        if let (Some(head), Some(tail)) = (bone.head, bone.tail) {
            out.push_str(&format!(
                "    head {} tail {}\n",
                format_point(head),
                format_point(tail)
            ));
        }
    }
    out
}

/// The `--bounds` report for a `kind: mesh` document: the model-space box its `positions` span, with
/// the centre and the size spelled out — the three numbers a region primitive is placed from, so
/// none of them has to be arithmetic in the author's head.
///
/// A mesh has no parts, so the block is the sculpt report's shape with one entry; a skinned mesh
/// adds its joint tree, each joint's rest head in model space — the author's read of the rig.
fn render_mesh_bounds(mesh: &ludion_scene::MeshDocument) -> String {
    let box_ = mesh_box(mesh).map(|(min, max)| (min.map(|v| v as f32), max.map(|v| v as f32)));
    let mut out = String::from("bounds:\n");
    out.push_str(&format!("  model: {}\n", format_bounds(box_)));
    if let Some((min, max)) = box_ {
        let centre = [0, 1, 2].map(|axis| 0.5 * (min[axis] + max[axis]));
        let size = [0, 1, 2].map(|axis| max[axis] - min[axis]);
        out.push_str(&format!(
            "    centre {} size {}\n",
            format_point(centre),
            format_point(size)
        ));
        out.push_str(&format!(
            "    {} vertices, {} triangles\n",
            mesh.vertex_count(),
            mesh.triangle_count()
        ));
    }
    if let Some(skin) = &mesh.skin {
        let heads = joint_heads(skin);
        let heads_in_parent = joint_heads_in_parent(skin);
        out.push_str(&format!(
            "  joints: {} (rest heads, model space)\n",
            skin.joints.len()
        ));
        let mut children = vec![Vec::new(); skin.joints.len()];
        let mut stack: Vec<(usize, usize)> = Vec::new();
        for (index, joint) in skin.joints.iter().enumerate() {
            match joint.parent {
                Some(parent) => children[parent as usize].push(index),
                None => stack.push((index, 0)),
            }
        }
        // Depth-first, roots and children in joint order: the stack pops the last pushed first.
        stack.reverse();
        while let Some((index, depth)) = stack.pop() {
            let in_parent = match heads_in_parent[index] {
                Some(head) => format!(" in parent {}", format_point(head.map(|v| v as f32))),
                None => String::new(),
            };
            out.push_str(&format!(
                "    {:indent$}{} {}{in_parent}\n",
                "",
                skin.joints[index].name,
                format_point(heads[index].map(|v| v as f32)),
                indent = 2 * depth
            ));
            stack.extend(
                children[index]
                    .iter()
                    .rev()
                    .map(|&child| (child, depth + 1)),
            );
        }
    }
    out
}

/// The summary fields `--bounds --json` adds for a `kind: mesh` document: `{"model":{"min","max"},"vertices",
/// "triangles","joints":[{"name","parent","head","head_in_parent"}]}` — `model` is null with no
/// positions, `joints` in document order and empty for an unskinned mesh, `head_in_parent` null for a
/// root joint.
fn mesh_bounds_json(mesh: &ludion_scene::MeshDocument) -> serde_json::Value {
    let model = mesh_box(mesh).map(|(min, max)| serde_json::json!({ "min": min, "max": max }));
    let joints: Vec<serde_json::Value> = mesh.skin.as_ref().map_or_else(Vec::new, |skin| {
        let heads = joint_heads(skin);
        let heads_in_parent = joint_heads_in_parent(skin);
        skin.joints
            .iter()
            .zip(heads)
            .zip(heads_in_parent)
            .map(|((joint, head), head_in_parent)| {
                serde_json::json!({
                    "name": joint.name,
                    "parent": joint.parent,
                    "head": head,
                    "head_in_parent": head_in_parent,
                })
            })
            .collect()
    });
    serde_json::json!({
        "model": model,
        "vertices": mesh.vertex_count(),
        "triangles": mesh.triangle_count(),
        "joints": joints,
    })
}

/// The model-space box a mesh's `positions` span, `None` when it has none (an `L1111` document,
/// so defensive: report it the way a part that meshed to nothing is rather than printing infinities).
pub(crate) fn mesh_box(mesh: &ludion_scene::MeshDocument) -> Option<([f64; 3], [f64; 3])> {
    let mut min = [f64::INFINITY; 3];
    let mut max = [f64::NEG_INFINITY; 3];
    for vertex in mesh.positions.chunks_exact(3) {
        for axis in 0..3 {
            min[axis] = min[axis].min(vertex[axis]);
            max[axis] = max[axis].max(vertex[axis]);
        }
    }
    min[0].is_finite().then_some((min, max))
}

/// Each joint's rest head in model space, in joint order.
fn joint_heads(skin: &ludion_scene::MeshSkin) -> Vec<[f64; 3]> {
    skin.rest_model_matrices()
        .expect("a validated skin is parent-first")
        .iter()
        .map(|matrix| [matrix[12], matrix[13], matrix[14]])
        .collect()
}

/// Each joint's rest head in its **parent's** local frame, in joint order; `None` for a root.
///
/// `inverse(parent_model) * child_model` always reduces to the child's own local rest matrix
/// (`child_model` is `parent_model * local`, so the parent cancels exactly), and a TRS matrix's
/// translation column is always its own authored translation regardless of its rotation or scale —
/// so this is exactly `joint.rest.translation`, unaffected by the parent's (or the skin's own)
/// scale. Read directly rather than composed, which would only add avoidable floating-point noise.
fn joint_heads_in_parent(skin: &ludion_scene::MeshSkin) -> Vec<Option<[f64; 3]>> {
    skin.joints
        .iter()
        .map(|joint| joint.parent.map(|_| joint.rest.translation))
        .collect()
}

/// One bounds box as `(x, y, z)..(x, y, z)` at 3-decimal precision, or `no surface` for `None`.
fn format_bounds(bounds: Option<([f32; 3], [f32; 3])>) -> String {
    match bounds {
        Some((min, max)) => format!("{}..{}", format_point(min), format_point(max)),
        None => "no surface".to_string(),
    }
}

/// One point as `(x, y, z)` at the `--bounds` report's 3-decimal precision.
fn format_point(p: [f32; 3]) -> String {
    format!("({:.3}, {:.3}, {:.3})", p[0], p[1], p[2])
}

/// The one legitimate wall-clock read in `check-scene`: how long `ludion_sculpt::mesh` took, for the
/// sculpt summary's `meshed in N ms`. `check-scene` is a CLI report, not sim-side, so this carries
/// the sim-side determinism denylist's one documented `allow` (`crates/ludion-cli/clippy.toml`), the
/// `dev.rs` `clock` module's precedent.
#[allow(clippy::disallowed_methods)]
fn timed<T>(f: impl FnOnce() -> T) -> (T, u128) {
    let start = std::time::Instant::now();
    let value = f();
    (value, start.elapsed().as_millis())
}

/// Validates a part document (`kind: part`, one body part of a multi-part `kind: assembly`,
/// `Plan/voxel-animation.md` decisions 3-4). A part has no palette of its own — its glyphs resolve
/// against the owning assembly's, checked only once that assembly is in hand — so there is nothing
/// here for the voxel path's `--exposure`/`--mirror-x` authoring views to compose with; this is a
/// structural/data-consistency check alone, the `run_mesh` shape.
fn run_part(mut reporter: Reporter, source: &str) -> ExitCode {
    let path = reporter.path;
    let part = match ludion_scene::check_part(source) {
        Ok(part) => part,
        Err(diagnostics) => return reporter.fail(diagnostics),
    };
    let [x, y, z] = part.size;
    let mut filled = 0u64;
    part.for_each_cell(|_, _, _, glyph| {
        if glyph.is_some() {
            filled += 1;
        }
    });
    let fields = serde_json::json!({ "name": part.name, "size": part.size, "filled": filled });
    reporter.ok(fields, || {
        let name = match &part.name {
            Some(name) => format!("{name}, "),
            None => String::new(),
        };
        println!(
            "{}: OK ({name}{x}x{y}x{z}, {filled} filled cell{})",
            path.display(),
            if filled == 1 { "" } else { "s" },
        );
    })
}

/// Validates a sprite document (`kind: sprite`, the authored pixel grid the atlas bake reads —
/// `Plan/sprite-engine.md`, "Data and bake"). A single-file structural/semantic check, the
/// [`run_part`] shape: one summary line naming the extent, the palette, and the effect list, then one
/// line per frame. Nothing here bakes — the pixels an effect list produces are `generate-atlas`'s
/// business, and `render --sprite` is where they are looked at.
fn run_sprite(mut reporter: Reporter, source: &str) -> ExitCode {
    let path = reporter.path;
    let base = match ludion_scene::check_sprite(source) {
        Ok(sprite) => sprite,
        Err(diagnostics) => return reporter.fail(diagnostics),
    };
    // A `palette: { from }` include is resolved against the referenced `kind: palette` document (read
    // relative to this sprite's own directory) through the same seam the contact sheet and
    // `generate-atlas` use, so one unreadable palette reads the same everywhere. An unresolvable or
    // wrong-kind reference is `L1147`; a glyph a frame uses that the merged palette still lacks is the
    // ordinary `L1153` from the re-validation below (a non-include document is unchanged here).
    let was_include = base.palette_include.is_some();
    let sprite = match ludion_engine::resolve_sprite_palette(path, base) {
        Ok(resolved) => resolved,
        Err(diagnostics) => return reporter.fail(diagnostics),
    };
    if was_include {
        let diagnostics = ludion_scene::validate_sprite(&sprite);
        if !diagnostics.is_empty() {
            return reporter.fail(diagnostics);
        }
    }
    let frames: Vec<_> = sprite
        .frames
        .iter()
        .map(|frame| {
            let pivot = frame.pivot.unwrap_or_else(|| sprite.default_pivot());
            (frame.name.as_str(), pivot)
        })
        .collect();
    let fields = serde_json::json!({
        "name": sprite.name,
        "size": sprite.size,
        "palette_glyphs": sprite.palette.len(),
        "frames": frames.iter().map(|(name, pivot)| serde_json::json!({ "name": name, "pivot": pivot })).collect::<Vec<_>>(),
        "effects": sprite.effects.len(),
    });
    reporter.ok(fields, || {
        let name = match &sprite.name {
            Some(name) => format!("{name}, "),
            None => String::new(),
        };
        let [w, h] = sprite.size;
        println!(
            "{}: OK ({name}{w}x{h}, {} palette glyph{}, {} frame{}, {} effect{})",
            path.display(),
            sprite.palette.len(),
            if sprite.palette.len() == 1 { "" } else { "s" },
            sprite.frames.len(),
            if sprite.frames.len() == 1 { "" } else { "s" },
            sprite.effects.len(),
            if sprite.effects.len() == 1 { "" } else { "s" },
        );
        for (name, pivot) in &frames {
            println!("  {name}: pivot ({}, {})", pivot[0], pivot[1]);
        }
    })
}

/// Validates a palette document (`kind: palette`, a standalone glyph palette a sprite can reference
/// instead of inlining — `Plan/sprite-engine.md`, "Data and bake"). A single-file structural/semantic
/// check, the [`run_sprite`] shape: one summary line naming the glyph count.
fn run_palette(mut reporter: Reporter, source: &str) -> ExitCode {
    let path = reporter.path;
    let palette = match ludion_scene::check_palette(source) {
        Ok(palette) => palette,
        Err(diagnostics) => return reporter.fail(diagnostics),
    };
    let fields = serde_json::json!({ "name": palette.name, "glyphs": palette.palette.len() });
    reporter.ok(fields, || {
        let name = match &palette.name {
            Some(name) => format!("{name}, "),
            None => String::new(),
        };
        println!(
            "{}: OK (palette, {name}{} glyph{})",
            path.display(),
            palette.palette.len(),
            if palette.palette.len() == 1 { "" } else { "s" },
        );
    })
}

/// Validates an atlas source document (`kind: atlas-source`, the bake's hand-authored input list).
/// Structure and semantics only, the [`run_part`] shape: whether the inputs *exist* and whether a
/// flipbook's frames will resolve is the bake's own question, because only the bake has read them.
fn run_atlas_source(mut reporter: Reporter, source: &str) -> ExitCode {
    let path = reporter.path;
    let atlas = match ludion_scene::check_atlas_source(source) {
        Ok(atlas) => atlas,
        Err(diagnostics) => return reporter.fail(diagnostics),
    };
    let fields = serde_json::json!({
        "name": atlas.name,
        "pixels_per_unit": atlas.pixels_per_unit,
        "filter": filter_keyword(atlas.filter),
        "inputs": atlas.inputs,
        "flipbooks": atlas.flipbooks.len(),
    });
    reporter.ok(fields, || {
        let name = match &atlas.name {
            Some(name) => format!("{name}, "),
            None => String::new(),
        };
        println!(
            "{}: OK (atlas source, {name}{} px/unit, {} filter, {} input{}, {} flipbook{})",
            path.display(),
            atlas.pixels_per_unit,
            filter_keyword(atlas.filter),
            atlas.inputs.len(),
            if atlas.inputs.len() == 1 { "" } else { "s" },
            atlas.flipbooks.len(),
            if atlas.flipbooks.len() == 1 { "" } else { "s" },
        );
        for input in &atlas.inputs {
            println!("  input {input}");
        }
    })
}

/// Validates a font source document (`kind: font-source`, the bake's authored input). Structure and
/// semantics only: the face file is the bake's to read.
fn run_font_source(mut reporter: Reporter, source: &str) -> ExitCode {
    let path = reporter.path;
    let doc = match ludion_scene::check_font_source(source) {
        Ok(doc) => doc,
        Err(diagnostics) => return reporter.fail(diagnostics),
    };
    let name = doc.name.as_deref().filter(|name| !name.is_empty());
    let chars = ludion_font::bake::resolve_charset(&doc.charset).len();
    let fields = serde_json::json!({
        "name": name,
        "mode": doc.mode.keyword(),
        "em_px": doc.em_px,
        "spread_px": doc.spread_px,
        "line_height": doc.line_height,
        "chars": chars,
        "file": doc.file,
    });
    reporter.ok(fields, || {
        let stem = name.map_or_else(|| "(file stem)".to_string(), str::to_string);
        let spread = doc
            .spread_px
            .map_or_else(String::new, |spread| format!(", spread {spread}"));
        let line_height = doc
            .line_height
            .map_or_else(String::new, |n| format!(", line height {n}"));
        println!(
            "{}: OK (font source, {stem}, {} {} px{spread}{line_height}, {chars} chars, file {})",
            path.display(),
            doc.mode.keyword(),
            doc.em_px,
            doc.file,
        );
    })
}

/// Validates a font document (`kind: font`, a baked or authored face). The text only: the atlas
/// image is not opened here.
fn run_font(mut reporter: Reporter, source: &str) -> ExitCode {
    let path = reporter.path;
    let doc = match ludion_scene::check_font(source) {
        Ok(doc) => doc,
        Err(diagnostics) => return reporter.fail(diagnostics),
    };
    let fields = serde_json::json!({
        "mode": doc.mode.keyword(),
        "glyphs": doc.glyphs.len(),
        "kern_pairs": doc.kerning.len(),
        "atlas": [doc.atlas.width, doc.atlas.height],
        "missing": doc.missing.len(),
        "generated_from": doc.generated_from,
    });
    reporter.ok(fields, || {
        let origin = doc
            .generated_from
            .as_deref()
            .map_or_else(|| "authored".to_string(), |from| format!("from {from}"));
        println!(
            "{}: OK (font, {}, {} glyphs, {} kern pairs, {}x{} atlas, {} missing, {origin})",
            path.display(),
            doc.mode.keyword(),
            doc.glyphs.len(),
            doc.kerning.len(),
            doc.atlas.width,
            doc.atlas.height,
            doc.missing.len(),
        );
    })
}

/// Validates a generated atlas document (`kind: atlas`) **and the packed image it addresses** — the
/// second `check-scene` path that touches more than the one file it was given.
///
/// `ludion-scene` does no IO, so its own pass deliberately stops at the text: whether the colour map
/// exists, and whether a region's `rect` names texels the image actually has, are answerable only
/// with the PNG in hand. Both are `L1153` — the defect is the same one the document's other checks
/// report, a document that does not agree with itself — pointed at `/maps/color` and
/// `/regions/<name>/rect`.
fn run_atlas(mut reporter: Reporter, source: &str) -> ExitCode {
    let path = reporter.path;
    let atlas = match ludion_scene::check_atlas(source) {
        Ok(atlas) => atlas,
        Err(diagnostics) => return reporter.fail(diagnostics),
    };
    // `maps.color` is written relative to the atlas document's own directory, so the pair moves
    // together — exactly how a `Sprite`'s `atlas:` resolves it at extract.
    let root = path.parent().unwrap_or_else(|| Path::new("."));
    let image = match ludion_assets::load_png(root, &atlas.color_map) {
        Ok(image) => image,
        Err(cause) => {
            let diagnostic = Diagnostic::new(
                code::SPRITE_DOCUMENT_STRUCTURE,
                format!("atlas colour map `{}`: {}", atlas.color_map, cause.message),
            )
            .with_path("/maps/color")
            .with_help(
                "a `kind: atlas` document and its packed image are written together; regenerate \
                 the pair with `ludion generate-atlas`",
            );
            return reporter.fail(vec![diagnostic]);
        }
    };
    let mut diagnostics = Vec::new();
    for (name, region) in &atlas.regions {
        let [x, y, w, h] = region.rect.map(u64::from);
        let (iw, ih) = (u64::from(image.width), u64::from(image.height));
        if x + w > iw || y + h > ih {
            diagnostics.push(
                Diagnostic::new(
                    code::SPRITE_DOCUMENT_STRUCTURE,
                    format!(
                        "region `{name}` covers [{x}, {y}, {w}, {h}], outside the {iw}x{ih} \
                         packed image `{}`",
                        atlas.color_map
                    ),
                )
                .with_path(format!("/regions/{}/rect", pointer_token(name)))
                .with_help(
                    "the document and the image have drifted apart; re-run \
                     `ludion generate-atlas` to write both from the `kind: atlas-source`",
                ),
            );
        }
    }
    if !diagnostics.is_empty() {
        return reporter.fail(diagnostics);
    }
    let fields = serde_json::json!({
        "size": [image.width, image.height],
        "regions": atlas.regions.len(),
        "flipbooks": atlas.flipbooks.len(),
        "pixels_per_unit": atlas.pixels_per_unit,
        "filter": filter_keyword(atlas.filter),
        "generated_from": atlas.generated_from,
    });
    reporter.ok(fields, || {
        println!(
            "{}: OK (atlas, {}x{} px, {} region{}, {} flipbook{}, {} px/unit, {} filter, generated \
             from {})",
            path.display(),
            image.width,
            image.height,
            atlas.regions.len(),
            if atlas.regions.len() == 1 { "" } else { "s" },
            atlas.flipbooks.len(),
            if atlas.flipbooks.len() == 1 { "" } else { "s" },
            atlas.pixels_per_unit,
            filter_keyword(atlas.filter),
            atlas.generated_from,
        );
    })
}

/// The `filter:` keyword an [`ludion_scene::AtlasFilter`] is written as — how a report names it, so
/// a printed value can be pasted straight back into the document.
fn filter_keyword(filter: ludion_scene::AtlasFilter) -> &'static str {
    match filter {
        ludion_scene::AtlasFilter::Nearest => "nearest",
        ludion_scene::AtlasFilter::Linear => "linear",
    }
}

/// Validates an assembly document (`kind: assembly`, the model of record for a multi-part voxel
/// model, `Plan/voxel-animation.md` decisions 3, 4 and 9) — the one `check-scene` path that touches
/// more than the one file it was given.
///
/// `ludion_engine::load_assembly` is the full multi-file pass: it re-reads the assembly plus every
/// part file it names (each resolved relative to the assembly file's own directory), cross-validates
/// them (unknown glyphs, parts that don't fit the grid), and meshes each part — the `root`/`rel` pair
/// it takes is derived from `path` the way `load_engine` derives a scene's project root: the file's
/// parent directory is `root`, its file name is `rel`, so a part's `file:` entry resolves exactly as
/// it would for a game booted from this same directory. Once that full pass is clean, `source` (the
/// text already in hand) is re-checked with `ludion_scene::check_assembly` to recover the authored
/// fields (`file`/`place`/`parent`/`mirror`) the meshed [`ludion_engine::AssemblyData`] does not
/// carry — guaranteed to succeed, since the full pass already validated the same document.
///
/// After that report, the assembled-grid analysis family runs (`Plan/voxel-animation.md` decision
/// 11): `ludion_engine::load_assembly_documents` resolves the same assembly and parts a second time
/// (the `run_assembly`/model-view harness double-parse precedent — the part *documents*, not the
/// meshed geometry, are what the composition needs, and [`ludion_engine::load_assembly`] above
/// discards them once meshed) so [`ludion_scene::assembly_assembled_grid`] can compose every part into
/// one assembly-global grid at rest pose. The composed grid's connectivity (`L1129`) always prints,
/// exactly like a plain voxel document's, followed by the overlap / joint-plug lint (`L1143`,
/// decision 10), unconditional for the same reason and silent on a plugged joint; `views`'
/// authoring flags run over the same composed grid,
/// with a per-cell part-attribution addendum a plain voxel report has no parts to name.
fn run_assembly(mut reporter: Reporter, source: &str, views: &VoxelViews) -> ExitCode {
    let path = reporter.path;
    let root = path.parent().unwrap_or_else(|| Path::new("."));
    let rel = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string());

    let data = match ludion_engine::load_assembly(root, &rel) {
        Ok(data) => data,
        Err(diagnostics) => return reporter.fail(diagnostics),
    };
    let assembly = ludion_scene::check_assembly(source)
        .expect("load_assembly already validated this exact document");

    let (grid_assembly, parts, ..) = ludion_engine::load_assembly_documents(root, &rel)
        .expect("load_assembly already validated this exact document and its parts");
    let grid = ludion_scene::assembly_assembled_grid(&grid_assembly, &parts);
    let owners = Owners {
        assembly: &grid_assembly,
        grid: &grid,
    };
    // Connectivity always runs over the composed grid, exactly as it always runs over a plain voxel
    // document (`run_voxel`) — never gated behind a view flag.
    let (components, connectivity) = grid.voxel.connectivity();
    // The overlap / joint-plug lint (`L1143`) is unconditional for the same reason: a wrong pivot is
    // found by checking the model, not by remembering to pass a flag. A plug at its joint is silent,
    // so a well-authored assembly prints nothing here.
    let overlap = ludion_scene::assembly_overlap_lint(&grid_assembly, &parts, &grid);

    let part_meshes: Vec<(u64, u64, u64)> = data
        .parts
        .iter()
        .map(|part| {
            let vertices = (part.mesh.positions.len() / 3) as u64;
            (vertices / 4, (part.mesh.indices.len() / 3) as u64, vertices)
        })
        .collect();
    let fields = serde_json::json!({
        "name": assembly.name,
        "size": assembly.size,
        "components": components,
        "parts": assembly.parts.iter().zip(&part_meshes).map(|(entry, &(quads, triangles, vertices))| {
            serde_json::json!({
                "name": entry.name,
                "file": entry.file,
                "place": entry.place,
                "parent": entry.parent,
                "mirror_x": entry.mirror_x,
                "meshed": { "quads": quads, "triangles": triangles, "vertices": vertices },
            })
        }).collect::<Vec<_>>(),
        "clips": assembly.clips.iter().map(|clip| {
            serde_json::json!({ "name": clip.name, "file": clip.file })
        }).collect::<Vec<_>>(),
    });

    if reporter.json {
        for warning in connectivity.into_iter().chain(overlap) {
            reporter.warn(warning);
        }
        return reporter.ok(fields, || {});
    }

    let [sx, sy, sz] = assembly.size;
    let name = match &assembly.name {
        Some(name) => format!(" {name}"),
        None => String::new(),
    };
    println!(
        "{}: OK (assembly{name}, {sx}x{sy}x{sz}, {} part{}, {} clip{})",
        path.display(),
        assembly.parts.len(),
        if assembly.parts.len() == 1 { "" } else { "s" },
        assembly.clips.len(),
        if assembly.clips.len() == 1 { "" } else { "s" },
    );
    for (entry, &(quads, triangles, vertices)) in assembly.parts.iter().zip(&part_meshes) {
        let [px, py, pz] = entry.place;
        let parent = entry.parent.as_deref().unwrap_or("none (root)");
        let mirror = if entry.mirror_x { "x" } else { "none" };
        println!(
            "  {}: file {}, place ({px},{py},{pz}), parent {parent}, mirror {mirror}, meshed \
             {quads} quads / {triangles} triangles / {vertices} vertices",
            entry.name, entry.file,
        );
    }
    for clip in &assembly.clips {
        println!("  clip {}: {}", clip.name, clip.file);
    }
    if let Some(warning) = connectivity {
        diagnostic::print(&warning);
        print!("{}", render_connectivity_owners(&owners, &grid));
    }
    for warning in overlap {
        diagnostic::print(&warning);
    }

    if let Err(failure) = print_voxel_views(&grid.voxel, views, Some(&owners)) {
        return failure.exit();
    }
    if views.exposure {
        print!("{}", render_overlap(&owners, &grid));
    }
    ExitCode::SUCCESS
}

/// `kind: prefab`: the scene pipeline over the prefab body, then the two passes that hold inside a
/// prefab (the body rules and the singleton refusal). The passes that read the content root or the
/// spawning scene's `physics:` block (atlas, skin bone, collider layer) run at spawn time.
fn run_prefab(mut reporter: Reporter, source: &str) -> ExitCode {
    let path = reporter.path;
    let doc = match ludion_scene::check_prefab(source, &builtin_registry()) {
        Ok(doc) => doc,
        Err(diagnostics) => return reporter.fail(diagnostics),
    };
    let body = doc.to_scene();
    for errors in [
        ludion_engine::scene_hierarchy_body_diagnostics(&body),
        ludion_engine::prefab_singleton_diagnostics(&body),
    ] {
        if let Some(exit) = fail_or_warn(&mut reporter, errors) {
            return exit;
        }
    }
    let clips = if doc.clips.is_empty() {
        Vec::new()
    } else {
        let root = path.parent().unwrap_or_else(|| Path::new("."));
        let rel = path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| path.display().to_string());
        let data = match ludion_engine::load_prefab_clips(root, &rel) {
            Ok(data) => data,
            Err(diagnostics) => return reporter.fail(diagnostics),
        };
        let part_names = match prefab_part_names(root, &rel) {
            Ok(names) => names,
            Err(diagnostics) => return reporter.fail(diagnostics),
        };
        let unmatched = prefab_clip_target_diagnostics(&data, &part_names);
        if let Some(exit) = fail_or_warn(&mut reporter, unmatched) {
            return exit;
        }
        data.clips
    };
    let components: usize = doc
        .entities
        .iter()
        .map(|entity| entity.components.len())
        .sum();
    let parts = doc.entities.len();
    let fields = serde_json::json!({
        "name": doc.name,
        "parts": parts,
        "components": components,
        "clips": clips.iter().map(|clip| serde_json::json!({
            "name": clip.name,
            "file": clip.file,
            "duration": clip.clip.duration,
            "looped": clip.clip.looped,
            "tracks": clip.clip.tracks.len(),
        })).collect::<Vec<_>>(),
    });
    reporter.ok(fields, || {
        let name = doc
            .name
            .as_ref()
            .map(|name| format!(" {name}"))
            .unwrap_or_default();
        let clip_count = if clips.is_empty() {
            String::new()
        } else {
            format!(
                ", {} clip{}",
                clips.len(),
                if clips.len() == 1 { "" } else { "s" }
            )
        };
        println!(
            "{}: OK (prefab{name}, {parts} part{}, {components} component{}{clip_count})",
            path.display(),
            if parts == 1 { "" } else { "s" },
            if components == 1 { "" } else { "s" }
        );
        for clip in &clips {
            println!(
                "  clip {}: {}, duration {}, loop {}, {} track{}",
                clip.name,
                clip.file,
                clip.clip.duration,
                clip.clip.looped,
                clip.clip.tracks.len(),
                if clip.clip.tracks.len() == 1 { "" } else { "s" },
            );
        }
    })
}

/// The part names an instance of the prefab at `root`/`rel` spawns (`entities[].name`, a nested
/// instance's parts by their composed path) — what playback binds a clip's tracks against.
fn prefab_part_names(root: &Path, rel: &str) -> Result<Vec<String>, Vec<Diagnostic>> {
    let registry = builtin_registry();
    let probe = format!(
        "format: 1
entities:
  - name: subject
    prefab: {}
",
        serde_json::Value::from(rel)
    );
    let scene = ludion_scene::check(&probe, &registry)?;
    let expanded = ludion_engine::expand_scene(root, &registry, &scene)?;
    Ok(expanded
        .instances
        .into_iter()
        .find(|instance| instance.depth == 0)
        .map(|instance| instance.paths)
        .unwrap_or_default())
}

/// One `L1314` per clip track whose `target` is not a part of the prefab.
fn prefab_clip_target_diagnostics(
    data: &ludion_engine::PrefabClipData,
    parts: &[String],
) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();
    for binding in &data.clips {
        for track in &binding.clip.tracks {
            if parts.contains(&track.target) {
                continue;
            }
            let help = match ludion_core::suggest::nearest(
                &track.target,
                parts.iter().map(String::as_str),
            ) {
                Some(candidate) => format!("nearest part is `{candidate}`"),
                None => "a track's target must be a part name (`entities[].name`)".to_string(),
            };
            diagnostics.push(
                Diagnostic::new(
                    code::PREFAB_CLIP_TARGET,
                    format!(
                        "the clip `{}` (`{}`) animates `{}`, which is not a part of this prefab",
                        binding.name, binding.file, track.target
                    ),
                )
                .with_path(format!("/clips/{}", pointer_token(&binding.name)))
                .with_help(help),
            );
        }
    }
    diagnostics
}

/// `kind: character`: loads the document with its skinned mesh and every clip it binds, rooted at
/// the file's own directory (the `run_assembly` rooting), and reports each binding's effective
/// speed and events. `source` is the text already in hand, re-checked only for the authored name.
fn run_character(mut reporter: Reporter, source: &str) -> ExitCode {
    let path = reporter.path;
    let root = path.parent().unwrap_or_else(|| Path::new("."));
    let rel = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string());

    let data = match ludion_engine::load_character(root, &rel) {
        Ok(data) => data,
        Err(diagnostics) => return reporter.fail(diagnostics),
    };
    let name = ludion_scene::check_character(source)
        .expect("load_character already validated this exact document")
        .name;
    let contacts = match ludion_engine::contact_heights(root, &data) {
        Ok(contacts) => contacts,
        Err(diagnostics) => return reporter.fail(diagnostics),
    };
    let fields = serde_json::json!({
        "name": name,
        "mesh": data.mesh,
        "clips": data.clips.iter().zip(&contacts).map(|(clip, contact)| serde_json::json!({
            "name": clip.name,
            "file": clip.file,
            "duration": clip.clip.duration,
            "speed": clip.speed,
            "events": clip.events.len(),
            "contacts": contact.joints.iter().map(|joint| serde_json::json!({
                "joint": joint.joint,
                "min": joint.min,
                "min_t": joint.min_t,
                "max": joint.max,
                "max_t": joint.max_t,
            })).collect::<Vec<_>>(),
        })).collect::<Vec<_>>(),
    });
    reporter.ok(fields, || {
        let name = name
            .as_ref()
            .map(|name| format!(" {name}"))
            .unwrap_or_default();
        println!(
            "{}: OK (character{name}, mesh {}, {} clip{})",
            path.display(),
            data.mesh,
            data.clips.len(),
            if data.clips.len() == 1 { "" } else { "s" },
        );
        for (clip, contact) in data.clips.iter().zip(&contacts) {
            println!(
                "  clip {}: {}, duration {}, speed {}, events {}",
                clip.name,
                clip.file,
                clip.clip.duration,
                clip.speed,
                clip.events.len(),
            );
            for joint in &contact.joints {
                println!(
                    "    contact {}: height min {:.4} @ {:.3} s, max {:.4} @ {:.3} s",
                    joint.joint, joint.min, joint.min_t, joint.max, joint.max_t,
                );
            }
        }
        if contacts.iter().all(|contact| contact.joints.is_empty()) {
            println!("  note: no ground-contact joint found");
        }
    })
}

/// Prints the requested voxel authoring views (`--mirror-x`, `--slice`, `--column`, `--exposure`, in
/// that order) to stdout. Returns the first failure: a usage error for a malformed
/// `--slice`/`--column` argument, or the coded diagnostic for an out-of-range one; `--mirror-x` and
/// `--exposure` never fail. Every view is rendered — so every argument is validated — before
/// anything prints, so a failure produces the message alone, never a half-rendered view.
///
/// `owners` is `None` for a plain `kind: voxel` document (every block renders exactly as it always
/// has) and `Some` for an assembly's composed grid, where `--mirror-x` and `--exposure` additionally
/// name the part(s) owning each cell they report (`Plan/voxel-animation.md` decision 10).
fn print_voxel_views(
    voxel: &ludion_scene::VoxelDocument,
    views: &VoxelViews,
    owners: Option<&Owners>,
) -> Result<(), ViewFailure> {
    let mut blocks: Vec<String> = Vec::new();
    if views.mirror_x {
        blocks.push(render_mirror_x(voxel, owners));
    }
    if let Some(request) = views.slice.as_deref() {
        match parse_slice(request).map_err(ViewFailure::Usage)? {
            SliceRequest::All => {
                for y in 0..voxel.size[1] {
                    blocks.push(render_slice_block(voxel, y)?);
                }
            }
            SliceRequest::One(y) => blocks.push(render_slice_block(voxel, y)?),
        }
    }
    if let Some(request) = views.column.as_deref() {
        let (x, z) = parse_column(request).map_err(ViewFailure::Usage)?;
        blocks.push(voxel.render_column(x, z).map_err(ViewFailure::Coded)?);
    }
    if views.exposure {
        blocks.push(render_exposure(voxel, views.from, owners));
    }
    // Every argument validated above — now print.
    print!("{}", blocks.concat());
    Ok(())
}

/// Per-cell part attribution for the assembled-grid authoring views (`Plan/voxel-animation.md`
/// decisions 10-11): threaded optionally through the voxel-view renderers so an assembly's report can
/// name the part(s) owning a cell that a plain voxel report names only by coordinate. Plain voxel
/// documents pass `None` everywhere, so their rendered text is exactly what it was before this type
/// existed.
struct Owners<'a> {
    assembly: &'a ludion_scene::AssemblyDocument,
    grid: &'a ludion_scene::AssembledGrid,
}

impl Owners<'_> {
    /// The comma-joined names of every part owning cell `(x, y, z)`, ascending authored order — or
    /// `none` for an unfilled cell (should not occur for a cell a report names, since every such cell
    /// is filled by construction; kept readable rather than panicking if that invariant ever slips).
    fn at(&self, x: u32, y: u32, z: u32) -> String {
        join_part_names(self.assembly, self.grid.owners_at(x, y, z))
    }
}

/// The comma-joined names of the parts at `indices` (ascending, authored order), or `none` if empty.
fn join_part_names(assembly: &ludion_scene::AssemblyDocument, indices: &[usize]) -> String {
    if indices.is_empty() {
        return "none".to_string();
    }
    indices
        .iter()
        .map(|&index| assembly.parts[index].name.as_str())
        .collect::<Vec<_>>()
        .join(", ")
}

/// The parts touching a `--mirror-x` region: every part owning a cell in the region's bounding box,
/// on either the low-x side the box is reported in or its mirrored high-x counterpart.
///
/// [`ludion_scene::MirrorRegion`] itself only carries a bounding box, not the exact (possibly
/// non-convex) cell set a 6-connected flood fill found — so this is deliberately an
/// over-approximation by that same box, consistent with the region's own reported shape, rather than
/// a second flood fill re-deriving the exact cells: "which files to open" never misses a part that
/// touches the feature, at the cost of occasionally naming one that only brushes the box's corner.
fn region_owners(owners: &Owners, region: &ludion_scene::MirrorRegion, width: u32) -> String {
    let mut indices: Vec<usize> = Vec::new();
    for y in region.min[1]..=region.max[1] {
        for z in region.min[2]..=region.max[2] {
            for x in region.min[0]..=region.max[0] {
                for cx in [x, width - 1 - x] {
                    for &owner in owners.grid.owners_at(cx, y, z) {
                        if !indices.contains(&owner) {
                            indices.push(owner);
                        }
                    }
                }
            }
        }
    }
    indices.sort_unstable();
    join_part_names(owners.assembly, &indices)
}

/// The attribution addendum to an assembled grid's `L1129` connectivity warning (always printed
/// alongside it, never behind a view flag — the warning itself is unconditional): which part(s) own
/// each stray component's sample cell and the main-body cell nearest it, so a disconnected assembly
/// names the files to look at, not just the coordinates.
fn render_connectivity_owners(owners: &Owners, grid: &ludion_scene::AssembledGrid) -> String {
    let components = grid.voxel.connectivity_components();
    if components.len() <= 1 {
        return String::new();
    }
    let mut out = String::from("connectivity owners:\n");
    let count = components.len();
    for component in &components[..count - 1] {
        let [x, y, z] = component.sample;
        out.push_str(&format!(
            "  stray sample ({x},{y},{z}) owned by {}",
            owners.at(x, y, z)
        ));
        if let Some(gap) = &component.gap {
            let [mx, my, mz] = gap.main;
            out.push_str(&format!(
                "; nearest main-body cell ({mx},{my},{mz}) owned by {}",
                owners.at(mx, my, mz)
            ));
        }
        out.push('\n');
    }
    out
}

/// The `--exposure` overlap addendum for an assembled grid (`Plan/voxel-animation.md` decision 10):
/// every filled cell more than one part contributed, bounded like [`ExposureReport`]'s own enclosed-
/// cell samples so a joint-plug-heavy model prints a handful of lines, not a wall.
///
/// [`ExposureReport`]: ludion_scene::ExposureReport
fn render_overlap(owners: &Owners, grid: &ludion_scene::AssembledGrid) -> String {
    const MAX_SAMPLES: usize = 8;
    let overlaps = grid.overlaps();
    if overlaps.is_empty() {
        return "overlap: none\n".to_string();
    }
    let mut out = format!(
        "overlap: {} cell{} shared by more than one part\n",
        overlaps.len(),
        if overlaps.len() == 1 { "" } else { "s" }
    );
    for ([x, y, z], indices) in overlaps.iter().take(MAX_SAMPLES) {
        out.push_str(&format!(
            "  ({x},{y},{z}): {}\n",
            join_part_names(owners.assembly, indices)
        ));
    }
    if overlaps.len() > MAX_SAMPLES {
        out.push_str(&format!(
            "  ... and {} more\n",
            overlaps.len() - MAX_SAMPLES
        ));
    }
    out
}

/// A `--slice` request: a single layer index, or every layer bottom-up.
enum SliceRequest {
    All,
    One(u32),
}

/// Parses the `--slice` argument: `all` (case-insensitive) or a single non-negative layer index.
fn parse_slice(value: &str) -> Result<SliceRequest, String> {
    if value.eq_ignore_ascii_case("all") {
        return Ok(SliceRequest::All);
    }
    value
        .trim()
        .parse::<u32>()
        .map(SliceRequest::One)
        .map_err(|_| {
            format!(
                "--slice expects a layer index (a non-negative integer) or `all`, got `{value}`"
            )
        })
}

/// Parses the `--column` argument: a single `x,z` token (one comma-joined argument, like
/// `map-check --at`).
fn parse_column(value: &str) -> Result<(u32, u32), String> {
    let (x, z) = value.split_once(',').ok_or_else(|| {
        format!("--column expects `x,z` (a single comma-joined token), got `{value}`")
    })?;
    let x = x
        .trim()
        .parse::<u32>()
        .map_err(|_| format!("--column x is not a non-negative integer: `{}`", x.trim()))?;
    let z = z
        .trim()
        .parse::<u32>()
        .map_err(|_| format!("--column z is not a non-negative integer: `{}`", z.trim()))?;
    Ok((x, z))
}

/// The `--mirror-x` symmetry report as a string: `symmetric`, or the asymmetric-cell total plus one
/// line per connected region — its cell count, bounding box, and glyph set — largest first.
///
/// A region table, never a cell list: one intended asymmetric feature is thousands of cells, so a
/// sample of them all comes from the same feature and answers nothing. Two tight regions read as
/// deliberate at a glance; forty scattered ones read as a bug.
fn render_mirror_x(voxel: &ludion_scene::VoxelDocument, owners: Option<&Owners>) -> String {
    const MAX_REGIONS: usize = 12;
    let regions = voxel.mirror_x_regions();
    if regions.is_empty() {
        return "mirror-x: symmetric\n".to_string();
    }
    let cells: u64 = regions.iter().map(|region| region.cells).sum();
    // Every coordinate is a low-half one (a mirror pair is reported from its low side), so the
    // header says where the numbers live rather than leaving the reader to infer it.
    let mut out = format!(
        "mirror-x: {cells} asymmetric cell{} in {} region{} (low-half x < {})\n",
        if cells == 1 { "" } else { "s" },
        regions.len(),
        if regions.len() == 1 { "" } else { "s" },
        voxel.size[0] / 2
    );
    let width = regions
        .iter()
        .take(MAX_REGIONS)
        .map(|region| region.cells.to_string().len())
        .max()
        .unwrap_or(1);
    for region in regions.iter().take(MAX_REGIONS) {
        let glyphs: Vec<String> = region.glyphs.iter().map(char::to_string).collect();
        out.push_str(&format!(
            "  {:>width$} cells  x {}..{}  y {}..{}  z {}..{}  glyphs {}",
            region.cells,
            region.min[0],
            region.max[0],
            region.min[1],
            region.max[1],
            region.min[2],
            region.max[2],
            glyphs.join(" ")
        ));
        if let Some(owners) = owners {
            out.push_str(&format!(
                "  parts {}",
                region_owners(owners, region, voxel.size[0])
            ));
        }
        out.push('\n');
    }
    if regions.len() > MAX_REGIONS {
        out.push_str(&format!(
            "  ... and {} more regions\n",
            regions.len() - MAX_REGIONS
        ));
    }
    out
}

/// The `--exposure` report as a string: a totals line, one line per glyph *present in the model*
/// (all of them, so a detail glyph reading `0 enclosed` is a positive confirmation rather than a
/// silence), then the emissive-face histogram in its fixed axis order.
fn render_exposure(
    voxel: &ludion_scene::VoxelDocument,
    from: ludion_scene::ExposureView,
    owners: Option<&Owners>,
) -> String {
    let report = voxel.exposure(from);
    let mut out = format!(
        "exposure: {} filled, {} surface, {} fully enclosed\n",
        report.filled, report.surface, report.enclosed
    );
    for glyph in &report.glyphs {
        out.push_str(&format!(
            "  glyph {}: {} cells, {} enclosed",
            glyph.glyph, glyph.cells, glyph.enclosed
        ));
        if !glyph.enclosed_samples.is_empty() {
            let samples: Vec<String> = glyph
                .enclosed_samples
                .iter()
                .map(|&[x, y, z]| match owners {
                    Some(owners) => format!("({x},{y},{z}) [{}]", owners.at(x, y, z)),
                    None => format!("({x},{y},{z})"),
                })
                .collect();
            out.push_str(&format!(" — samples {}", samples.join(", ")));
            if glyph.enclosed_overflow > 0 {
                out.push_str(&format!(", ... and {} more", glyph.enclosed_overflow));
            }
        }
        out.push('\n');
    }
    if report.emissive.is_empty() {
        out.push_str("emissive faces: none (no palette entry in use declares emissive > 0)\n");
        return out;
    }
    out.push_str("emissive faces:\n");
    for entry in &report.emissive {
        let histogram: Vec<String> = ludion_scene::EmissiveFaces::FACE_LABELS
            .iter()
            .zip(entry.faces)
            .map(|(label, count)| format!("{label} {count}"))
            .collect();
        out.push_str(&format!(
            "  glyph {} (emissive {}): {} exposed — {}\n",
            entry.glyph,
            entry.emissive,
            entry.total(),
            histogram.join(", ")
        ));
        // The viewpoint half: of the faces light can leave through, how many the player is even
        // pointed at, and how many of those this model's own geometry hides from them.
        if let Some(game) = entry.game {
            out.push_str(&format!(
                "    from game view ({} deg): {} face the camera, {} occluded by own geometry\n",
                ludion_scene::GAME_VIEW_PITCH_DEG,
                game.facing,
                game.occluded
            ));
        }
    }
    out
}

/// One annotated slice under a `-- y=N --` header. Propagates an out-of-range error.
fn render_slice_block(voxel: &ludion_scene::VoxelDocument, y: u32) -> Result<String, ViewFailure> {
    let body = voxel.render_slice(y).map_err(ViewFailure::Coded)?;
    Ok(format!("-- y={y} --\n{body}"))
}

/// Why a voxel view request failed: flag text that does not parse is a usage error; a coordinate
/// the model does not have is a coded diagnostic.
enum ViewFailure {
    Usage(String),
    Coded(Diagnostic),
}

impl ViewFailure {
    fn exit(self) -> ExitCode {
        match self {
            Self::Usage(message) => diagnostic::usage_error(message),
            Self::Coded(diagnostic) => {
                diagnostic::print(&diagnostic);
                ExitCode::FAILURE
            }
        }
    }
}

/// A kind with a `ludion-scene` checker but no rich summary of its own: validates it and says OK.
fn run_checked(
    mut reporter: Reporter,
    source: &str,
    entry: &ludion_scene::KindEntry,
    check: fn(&str) -> Result<(), Vec<Diagnostic>>,
) -> ExitCode {
    let path = reporter.path;
    match check(source) {
        Ok(()) => reporter.ok(serde_json::json!({}), || {
            println!("{}: OK ({})", path.display(), entry.name);
        }),
        Err(diagnostics) => reporter.fail(diagnostics),
    }
}

/// The `kind:` names no engine kind (`L1119`, the code the engine's content-root walk raises for
/// the same mistake).
fn unrecognized_kind(kind: &str) -> Diagnostic {
    let names = || ludion_scene::KINDS.iter().map(|entry| entry.name);
    let help = match ludion_core::suggest::nearest(kind, names()) {
        Some(nearest) => format!("nearest kind is `{nearest}`"),
        None => format!(
            "engine kinds: {}; a game data-doc is validated by the game's own `--check-docs`; \
             remove `kind:` for a scene",
            names()
                .map(|name| format!("`{name}`"))
                .collect::<Vec<_>>()
                .join(", ")
        ),
    };
    Diagnostic::new(
        code::DOC_ENVELOPE_INVALID,
        format!("unrecognized document kind `{kind}`"),
    )
    .with_path("/kind")
    .with_help(help)
}

/// The engine's built-in component registry — the set `ludion_engine::builtin_registry` registers,
/// which is where to read the list. Defined once in `ludion-engine` and shared by every CLI command,
/// so `check-scene`, `sim-hash`, `render`, and `dump-graph` all validate against the same set, as
/// does the in-process `scene.check`.
pub(crate) fn builtin_registry() -> Registry {
    ludion_engine::builtin_registry()
}

/// Constructs a fresh built-in `Engine` rooted at the scene file's own directory and loads
/// `scene_path` into it via `scene.load` — the exact construction path `sim-hash` and `serve` both
/// start from, so a scene fails identically (same diagnostics) wherever it is loaded.
///
/// The rooting/resolution logic lives in `EngineBuilder::load_scene_file` (`ludion-engine`), shared
/// with the `ludion` facade's `run` so a hosted game loads a scene exactly as the CLI does; this
/// wrapper only renders the diagnostic and maps it to an exit code. The engine's project root becomes
/// the scene file's parent directory, so a scene's relative asset references resolve against the
/// scene file rather than the process's cwd.
///
/// The engine registers the engine-owned `kind: pattern` documents (`EngineBuilder::patterns`), so a
/// hosted scene's `PatternPlayer` plays what its content root holds. That turns the content-root walk
/// on; a `kind:` belonging to the game whose scene this is gets skipped, because a built-in engine
/// hosts anyone's scene and cannot know a game's kinds. One aggregated `L1155` is printed here for
/// the whole walk (`Engine::doc_skip_summary`), never one per file.
///
/// On failure, prints the collapsed diagnostic to stderr (rustc-style; a scene with several
/// errors reports each one, chained through the diagnostic's related list) and returns the exit
/// code the command should end with.
pub(crate) fn load_engine(scene_path: &Path) -> Result<Engine, ExitCode> {
    let engine = Engine::builder()
        .patterns()
        .load_scene_file(scene_path)
        .map_err(|mut diagnostic| {
            add_project_component_help(scene_path, std::slice::from_mut(&mut diagnostic));
            eprintln!("{diagnostic}");
            ExitCode::FAILURE
        })?;
    // Registering a kind turns the content-root walk on, and a game's own scene directory holds
    // kinds only that game's binary knows. They are skipped, not fatal — but never silently. One
    // summary line, not one block per file: vigil's scene directory alone skips 28 documents, and
    // 84 lines of warning bury whatever this command says next.
    if let Some(summary) = engine.doc_skip_summary() {
        eprintln!("{summary}");
    }
    Ok(engine)
}

/// Arms recording on `engine` to `record_path`, matching `sim-hash --record`'s pattern: create the
/// sink and arm right after the scene loads, so the session header captures this run's scene
/// reference and seed, and every subsequent command becomes a replayable delta.
///
/// On failure, prints a diagnostic to stderr and returns the exit code the command should end
/// with.
pub(crate) fn arm_recording(engine: &mut Engine, record_path: &Path) -> Result<(), ExitCode> {
    let sink = File::create(record_path).map_err(|error| {
        let diagnostic = Diagnostic::new(
            code::OUTPUT_IO,
            format!(
                "could not create replay session `{}`: {error}",
                record_path.display()
            ),
        )
        .with_help("check that the destination directory exists and is writable");
        eprintln!("{diagnostic}");
        ExitCode::FAILURE
    })?;
    engine.set_recording(sink).map_err(|diagnostic| {
        eprintln!("{diagnostic}");
        ExitCode::FAILURE
    })
}

#[cfg(test)]
mod project_help_tests {
    use super::add_project_component_help;
    use ludion_core::codes::code;
    use ludion_core::diagnostics::Diagnostic;
    use std::path::Path;

    fn unknown(message: &str) -> Diagnostic {
        Diagnostic::new(code::SCENE_UNKNOWN_COMPONENT, message).with_help("registered are: A")
    }

    fn diagnostics() -> Vec<Diagnostic> {
        vec![
            unknown("first"),
            Diagnostic::new(code::SCENE_IO, "other"),
            unknown("second"),
        ]
    }

    #[test]
    fn only_the_first_unknown_component_gains_the_sentence_inside_a_project() {
        let scene =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../games/breakout/scenes/x.yaml");
        let mut found = diagnostics();
        add_project_component_help(&scene, &mut found);
        let help = found[0].help.as_deref().expect("help");
        assert!(help.starts_with("registered are: A; "), "{help}");
        assert!(
            help.contains("--scene scenes/x.yaml --check-scene"),
            "{help}"
        );
        assert_eq!(found[1].help, None);
        assert_eq!(found[2].help.as_deref(), Some("registered are: A"));
    }

    #[test]
    fn a_scene_outside_a_project_is_unchanged() {
        let scene = std::env::temp_dir().join("ludion-no-project-here/x.yaml");
        let mut found = diagnostics();
        add_project_component_help(&scene, &mut found);
        assert_eq!(found, diagnostics());
    }
}
