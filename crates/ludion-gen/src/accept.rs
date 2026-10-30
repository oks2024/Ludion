//! Freezing one candidate out of a run into the project, with its provenance.
//!
//! A run folder is scratch that both VCSs ignore; acceptance is the one step that turns a candidate
//! into content. The image is re-encoded rather than copied — [`ludion_assets::decode_png`] then
//! [`ludion_assets::encode_png`], the workspace's single deterministic PNG path, which writes only
//! `IHDR`/`IDAT`/`IEND` — so an accepted asset carries no server-written ancillary chunk and hashes
//! identically to the same pixels arriving through `ludion import`. The assets crate has no
//! separate normalizer: that decode/encode pair *is* the normalization.
//!
//! Provenance rides beside the image in a `kind: origin` sidecar named `<asset>.png.origin.yaml`,
//! because a PNG has no text place for it and a `tEXt` chunk is exactly what the importer strips.
//! An accepted asset is otherwise entirely ordinary — content, hand-editable from this moment on.
//!
//! A mesh candidate takes the same shape through a different carrier: the `.glb` goes to
//! [`ludion_import`] — the one glTF reader, called in-process — and the provenance block lands in
//! each produced `meshes/*.mesh.yaml` under its own `origin:` key, so a generated mesh needs no
//! sidecar. `out` is therefore the import destination *directory* for a mesh and the asset file
//! itself for an image, and neither is ever written over.
//!
//! The import mode is the caller's, exactly as it is for the `import` verb: strict by default, and
//! lossy on request. A generator's writer is not a re-exportable source — there is no going back to
//! fix the graph's `SaveGLB` node — so `L1202`'s "re-export without it" advice has no addressee
//! here, and the lossy mode is the only answer a candidate carrying an unmappable material flag
//! has. Every `L1202` the importer hands back therefore has its help rewritten for this path
//! (`readdress_lossy`), in both directions: the flag while the import is strict, what was left
//! out once it has been passed.
//!
//! Provenance records what the asset was made *from* as well as by: `run.json`'s `images` — the
//! recipe's `images:` entries as resolved for that run, with a hash of the bytes that went up —
//! travel into the block, which is the only field of an image-to-mesh asset's origin that names its
//! source image.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use ludion_core::atomic::write_atomic;
use ludion_core::codes::code;
use ludion_core::diagnostics::Diagnostic;
use ludion_ecs::Registry;
use ludion_scene::{Origin, OriginImage};

use crate::recipe::{self, normalize};
use crate::run::{self, RunOutput, RunReport, STATUS_FAILED, STATUS_OK, STATUS_PARTIAL};
use crate::workflow::{self, OutputKind};

/// The suffix an accepted image's provenance sidecar takes, appended to the whole filename so the
/// two sort together and neither can be mistaken for the other's stem.
pub const ORIGIN_SUFFIX: &str = ".origin.yaml";

/// What one accept wrote.
#[derive(Clone, Debug, PartialEq)]
pub struct Accepted {
    /// The candidate inside the run folder.
    pub source: PathBuf,
    /// The asset written into the project: the file itself for an image, the import directory for a
    /// mesh.
    pub destination: PathBuf,
    /// The provenance sidecar, for a carrier that has no text of its own to hold it.
    pub sidecar: Option<PathBuf>,
    /// The provenance block, whichever carrier holds it.
    pub origin: Origin,
    /// What the import wrote, for a mesh candidate. `None` for an image.
    pub mesh: Option<AcceptedMesh>,
}

/// The tree a mesh accept wrote under its destination, and the counts a density verdict is read
/// from.
#[derive(Clone, Debug, PartialEq)]
pub struct AcceptedMesh {
    /// Every mesh document written, in the importer's own (glTF document) order.
    pub meshes: Vec<AcceptedMeshDocument>,
    /// Every texture written beside them.
    pub textures: Vec<PathBuf>,
    /// The importer's own non-fatal diagnostics, plus one [`code::MESH_VERTEX_BUDGET_EXCEEDED`] per
    /// document over the style budget — the signal that the graph's decimation target is too high,
    /// which is where density is set.
    pub warnings: Vec<Diagnostic>,
}

/// One written mesh document and the geometry it holds.
#[derive(Clone, Debug, PartialEq)]
pub struct AcceptedMeshDocument {
    /// The document's path under the accept's destination directory.
    pub path: PathBuf,
    /// Vertices in the document.
    pub vertices: usize,
    /// Triangles in the document.
    pub triangles: usize,
}

/// Freezes candidate `index` of the run at `run_dir` into `out`, with its provenance.
///
/// For an image, `out` names the asset itself and the provenance sidecar is `<out>` plus
/// [`ORIGIN_SUFFIX`]. For a mesh, `out` names the directory [`ludion_import`] writes its
/// `model.prefab.yaml` + `meshes/*.mesh.yaml` + `textures/*.png` into, and the provenance rides in
/// each mesh document's `origin:` key. Nothing is ever overwritten: an existing file, or a destination
/// directory that already holds anything, is a refusal.
///
/// `registry` is the component registry the imported prefab is emitted through — the built-in set,
/// which lives above this crate. `allow_lossy` is the import mode a mesh candidate is read in and is
/// ignored for an image: `false` refuses an unmappable glTF feature with `L1202`, `true` downgrades
/// it to a warning on [`AcceptedMesh::warnings`] and compiles the subset that maps.
///
/// # Errors
/// [`code::GENERATION_ACCEPT_REFUSED`] for every refusal: no readable `run.json`, a run that is not
/// finalized, an index outside the run, a missing candidate file, a destination that already
/// exists, or a run predating the prompt snapshot whose recipe no longer resolves or whose recipe or
/// workflow hash has changed since. `L1102` for a candidate that is not a decodable
/// PNG, and the importer's own `L1201`/`L1202`/`L1203` for a `.glb` it will not read.
pub fn accept(
    run_dir: &Path,
    index: u32,
    out: &Path,
    registry: &Registry,
    allow_lossy: bool,
) -> Result<Accepted, Vec<Diagnostic>> {
    let report = run::read_report(run_dir)?;
    let output = candidate(&report, index)?;
    let source = report.dir.join(&output.file);
    if !source.is_file() {
        return Err(vec![
            refused(format!(
                "candidate {index} of run `{}` is recorded as `{}`, and no file is there",
                report.run_id, output.file
            ))
            .with_help(
                "a run folder is scratch and may have been cleaned; run the recipe again to \
                 produce the candidate",
            ),
        ]);
    }
    if report.kind == OutputKind::Mesh.as_str() {
        return accept_mesh(&report, output, &source, out, registry, allow_lossy);
    }
    accept_image(&report, output, &source, out)
}

/// Freezes an image candidate: the PNG through the workspace's one encode path, its provenance in a
/// sidecar beside it.
fn accept_image(
    report: &RunReport,
    output: &RunOutput,
    source: &Path,
    out: &Path,
) -> Result<Accepted, Vec<Diagnostic>> {
    let sidecar = with_suffix(out, ORIGIN_SUFFIX);
    for existing in [out, sidecar.as_path()] {
        if existing.exists() {
            return Err(vec![
                refused(format!("`{}` already exists", existing.display())).with_help(
                    "an accept never overwrites; delete the asset, or name a free path with \
                     `--out`",
                ),
            ]);
        }
    }

    let origin = provenance(report, output)?;

    let bytes = std::fs::read(source).map_err(|error| {
        vec![refused(format!(
            "could not read `{}`: {error}",
            source.display()
        ))]
    })?;
    let (width, height, rgba) = ludion_assets::decode_png(&bytes, &source.display().to_string())
        .map_err(|diagnostic| vec![diagnostic])?;
    let normalized = ludion_assets::encode_png(width, height, &rgba).map_err(|error| {
        vec![Diagnostic::new(
            code::OUTPUT_IO,
            format!("could not re-encode `{}`: {error}", source.display()),
        )]
    })?;

    if let Some(parent) = out.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent).map_err(|error| vec![write_failed(parent, error)])?;
    }
    write_asset_and_sidecar(
        out,
        &normalized,
        &sidecar,
        &ludion_scene::emit_origin_document(&origin),
    )?;

    Ok(Accepted {
        source: source.to_path_buf(),
        destination: out.to_path_buf(),
        sidecar: Some(sidecar),
        origin,
        mesh: None,
    })
}

/// Writes the asset, then its sidecar; a failed sidecar removes the asset again, so a retry is not
/// refused as "already exists".
fn write_asset_and_sidecar(
    out: &Path,
    asset: &[u8],
    sidecar: &Path,
    text: &str,
) -> Result<(), Vec<Diagnostic>> {
    write_atomic(out, asset).map_err(|error| vec![write_failed(out, error)])?;
    write_atomic(sidecar, text).map_err(|error| {
        let _ = std::fs::remove_file(out);
        vec![write_failed(sidecar, error)]
    })
}

/// Freezes a mesh candidate: the `.glb` through [`ludion_import`] in default (non-lossy) mode, its
/// provenance spliced into every mesh document the import produces.
///
/// The provenance goes into the plan before anything is written, so each document is written once,
/// already carrying its `origin:` — there is no read-modify-write pass over a file already on disk.
/// Only the mesh documents are re-validated here: they are the ones this step edits, and the prefab
/// is the importer's own output, checked by the `import` verb and by that crate's tests.
fn accept_mesh(
    report: &RunReport,
    output: &RunOutput,
    source: &Path,
    out_dir: &Path,
    registry: &Registry,
    allow_lossy: bool,
) -> Result<Accepted, Vec<Diagnostic>> {
    if !is_free(out_dir) {
        return Err(vec![
            refused(format!("`{}` already holds files", out_dir.display())).with_help(
                "an accept never overwrites, and an imported mesh tree is content from this moment \
                 on; name an empty or absent directory with `--out`",
            ),
        ]);
    }
    let origin = provenance(report, output)?;

    let bytes = std::fs::read(source).map_err(|error| {
        vec![refused(format!(
            "could not read `{}`: {error}",
            source.display()
        ))]
    })?;
    // A generated `.glb` is self-contained, and the run folder is not a source tree.
    let mut import =
        ludion_import::import(&bytes, ludion_import::ExternalFiles::Refused, allow_lossy)
            .map_err(|diagnostics| readdress_lossy(diagnostics, allow_lossy))?;
    for mesh in &mut import.meshes {
        mesh.doc.origin = Some(origin.clone());
        // ComfyUI's `SaveGLB` flags every material double-sided, and a closed remesh wants culling.
        mesh.doc.double_sided = false;
    }

    let prefab_yaml = ludion_scene::emit_prefab(&import.prefab, registry);
    let mut documents = Vec::with_capacity(import.meshes.len());
    let mut warnings = readdress_lossy(import.warnings.clone(), allow_lossy);
    for mesh in &import.meshes {
        // Re-reading the emitted text is what proves the spliced provenance block and the
        // document's texture references coexist in a form the loader will take.
        let doc = ludion_scene::check_mesh(&ludion_scene::emit_mesh(&mesh.doc))?;
        warnings.extend(ludion_scene::mesh_budget_warning(&doc));
        documents.push(AcceptedMeshDocument {
            path: out_dir.join(&mesh.rel_path),
            vertices: doc.vertex_count(),
            triangles: doc.triangle_count(),
        });
    }
    import.write(out_dir, &prefab_yaml).map_err(|error| {
        vec![
            error.with_help("an accept writes the whole import tree; check the parent is writable"),
        ]
    })?;

    Ok(Accepted {
        source: source.to_path_buf(),
        destination: out_dir.to_path_buf(),
        sidecar: None,
        origin,
        mesh: Some(AcceptedMesh {
            meshes: documents,
            textures: import
                .textures
                .iter()
                .map(|texture| out_dir.join(&texture.rel_path))
                .collect(),
            warnings,
        }),
    })
}

/// Rewrites every `L1202`'s help for the accept path. The importer's own advice addresses someone
/// holding an exportable source, which whoever generated a `.glb` never is: there is no re-export
/// that could drop the feature at the source, so the flag is the only answer while the import is
/// strict — and once it has been passed, repeating it is noise, so the warning says what was left
/// out instead.
fn readdress_lossy(diagnostics: Vec<Diagnostic>, allow_lossy: bool) -> Vec<Diagnostic> {
    diagnostics
        .into_iter()
        .map(|diagnostic| {
            if diagnostic.code != code::IMPORT_UNSUPPORTED_FEATURE {
                return diagnostic;
            }
            if allow_lossy {
                diagnostic.with_help("the mesh was imported without this feature")
            } else {
                diagnostic.with_help(
                    "`ludion gen accept <run> <index> --out <dir> --allow-lossy` imports the \
                     supported subset and leaves this out — a generated `.glb` has no re-export to \
                     fix it at the source",
                )
            }
        })
        .collect()
}

/// Whether `out_dir` is a destination a mesh accept may write into: absent, or present and empty.
/// Anything else — a file of that name, a directory holding something, a directory that cannot be
/// read — counts as occupied, since a refusal is the safe verdict.
fn is_free(out_dir: &Path) -> bool {
    match std::fs::read_dir(out_dir) {
        Ok(mut entries) => entries.next().is_none(),
        Err(error) => error.kind() == std::io::ErrorKind::NotFound,
    }
}

/// The candidate `index` names, once the run is in a state that has one.
fn candidate(report: &RunReport, index: u32) -> Result<&RunOutput, Vec<Diagnostic>> {
    if report.status == STATUS_FAILED {
        return Err(vec![
            refused(format!(
                "run `{}` failed: every job failed or was lost, so it has no candidate",
                report.run_id
            ))
            .with_help(
                "`run.json`'s `failed` names each job and why; `ludion gen run <recipe>` submits \
                 it again",
            ),
        ]);
    }
    if report.status != STATUS_OK && report.status != STATUS_PARTIAL {
        return Err(vec![
            refused(format!(
                "run `{}` is `{}`, and only a finalized run has candidates to accept",
                report.run_id, report.status
            ))
            .with_help(format!(
                "`ludion gen status {dir}` says where its jobs are; `ludion gen fetch {dir}` \
                 completes it once they finish",
                dir = run::display_path(&report.dir)
            )),
        ]);
    }
    report
        .outputs
        .iter()
        .find(|output| output.index == index)
        .ok_or_else(|| {
            vec![
                refused(format!(
                    "run `{}` has {} candidates, and {index} is not one of them",
                    report.run_id,
                    report.outputs.len()
                ))
                .with_help(match report.outputs.len() {
                    0 => "the run downloaded nothing".to_string(),
                    count => format!("the indices are 0 to {}", count - 1),
                }),
            ]
        })
}

/// The provenance block an accepted candidate carries, read from `run.json` alone.
///
/// A report written by this version holds the prompt and the `inputs:` snapshot as the run read
/// them, so editing, renaming or deleting the recipe afterwards changes nothing recorded here. A
/// report written before the snapshot existed re-resolves the recipe, which is only the prompt that
/// ran when the recipe's and workflow's hashes still match the report's; a drift is refused.
fn provenance(report: &RunReport, output: &RunOutput) -> Result<Origin, Vec<Diagnostic>> {
    let (prompt, inputs, checkpoints, images) = match (&report.prompt, &report.inputs) {
        (Some(prompt), Some(inputs)) => {
            let inputs = run::inputs_from_json(inputs).map_err(|cause| {
                vec![
                    refused(format!(
                        "run `{}`'s `run.json` has an unreadable inputs snapshot: {cause}",
                        report.run_id
                    ))
                    .with_help("run the recipe again; the new run records a fresh snapshot"),
                ]
            })?;
            (
                prompt.clone(),
                inputs,
                report.checkpoints.clone(),
                recorded_images(report),
            )
        }
        _ => reread_recipe(report)?,
    };

    Ok(Origin {
        tool: "comfyui".to_string(),
        comfy_version: report.comfy_version.clone(),
        recipe: report.recipe.clone(),
        recipe_hash: Some(report.recipe_hash.clone()),
        run_id: report.run_id.clone(),
        date: report
            .started
            .split('T')
            .next()
            .unwrap_or_default()
            .to_string(),
        seed: output.seed,
        workflow: report.workflow.clone(),
        workflow_hash: report.workflow_hash.clone(),
        checkpoints,
        images,
        prompt,
        inputs,
    })
}

/// What `reread_recipe` returns: the prompt, inputs, checkpoints and images.
type Rereads = (String, run::InputTable, Vec<String>, Vec<OriginImage>);

/// The images a report recorded, as provenance spells them.
fn recorded_images(report: &RunReport) -> Vec<OriginImage> {
    report
        .images
        .iter()
        .map(|image| OriginImage {
            name: image.name.clone(),
            path: image.path.clone(),
            hash: image.hash.clone(),
        })
        .collect()
}

/// The prompt, inputs, checkpoints and images of a report that predates the snapshot, re-read from
/// the recipe — but only when the recipe and workflow are the bytes the run recorded.
fn reread_recipe(report: &RunReport) -> Result<Rereads, Vec<Diagnostic>> {
    let root = recipe::project_root(&report.dir);
    let recipe_path = normalize(&root.join(&report.recipe));
    let resolved = recipe::resolve(&recipe_path).map_err(|causes| {
        vec![
            refused(format!(
                "run `{}` predates the prompt snapshot, so its prompt must be re-read from the \
                 recipe `{}`, which no longer resolves: {}",
                report.run_id,
                report.recipe,
                causes
                    .iter()
                    .map(|cause| cause.message.clone())
                    .collect::<Vec<_>>()
                    .join("; ")
            ))
            .with_help(
                "restore the recipe and its workflow, or run the recipe again and accept from the \
                 new run, which records its own prompt",
            ),
        ]
    })?;

    if resolved.recipe_hash != report.recipe_hash || resolved.workflow_hash != report.workflow_hash
    {
        return Err(vec![
            refused(format!(
                "run `{}` predates the prompt snapshot, and `{}` or `{}` has changed since it ran \
                 (recipe hash {} then, {} now; workflow hash {} then, {} now), so the prompt it \
                 would record is not the one that ran",
                report.run_id,
                report.recipe,
                report.workflow,
                report.recipe_hash,
                resolved.recipe_hash,
                report.workflow_hash,
                resolved.workflow_hash,
            ))
            .with_help(
                "restore the file to the hash the run recorded, or run the recipe again and \
                 accept from the new run, which records its own prompt",
            ),
        ]);
    }

    // An image override feeds only a `LoadImage`, which neither the prompt nor the checkpoints read.
    let placeholders: BTreeMap<String, String> = resolved
        .doc
        .images
        .iter()
        .map(|(name, _)| (name.clone(), name.clone()))
        .collect();
    let graph =
        workflow::apply(&resolved.graph, &resolved.doc, &placeholders).map_err(|causes| {
            vec![refused(format!(
            "run `{}` predates the prompt snapshot, and its recipe's overrides no longer apply: {}",
            report.run_id,
            causes
                .iter()
                .map(|cause| cause.message.clone())
                .collect::<Vec<_>>()
                .join("; ")
        ))
        .with_help(
            "restore the recipe and its workflow, or run the recipe again and accept from the \
             new run, which records its own prompt",
        )]
        })?;
    let images = if report.images.is_empty() {
        legacy_images(report, &root, &resolved)?
    } else {
        recorded_images(report)
    };
    Ok((
        run::prompt_text(&graph),
        resolved.doc.inputs.clone(),
        run::checkpoints(&graph),
        images,
    ))
}

/// The images of a report that predates the `images` record, rebuilt from the run folder's
/// `inputs/` copies. This reads the legacy layout, where a run copied each upload into `inputs/`
/// under its bare file name; a report written since records its images itself and never gets here.
fn legacy_images(
    report: &RunReport,
    root: &Path,
    resolved: &recipe::Resolved,
) -> Result<Vec<OriginImage>, Vec<Diagnostic>> {
    resolved
        .images
        .iter()
        .map(|(name, path)| {
            let file = path
                .file_name()
                .map(|file| file.to_string_lossy().to_string())
                .unwrap_or_default();
            let help = "run the recipe again and accept from the new run, which records its images";
            let bytes = std::fs::read(report.dir.join("inputs").join(&file)).map_err(|error| {
                vec![
                    refused(format!(
                        "run `{}` predates the image record, and its copy of the image `{name}` \
                         (`inputs/{file}`) cannot be read: {error}",
                        report.run_id
                    ))
                    .with_help(help),
                ]
            })?;
            // The run may have fed a `--image` scratch of the same file name instead.
            if std::fs::read(path).ok().as_deref() != Some(bytes.as_slice()) {
                return Err(vec![
                    refused(format!(
                        "run `{}` predates the image record, and its copy of the image `{name}` \
                         (`inputs/{file}`) is not the file the recipe declares (`{}`), which is \
                         missing or differs",
                        report.run_id,
                        run::project_relative(root, path)
                    ))
                    .with_help(help),
                ]);
            }
            Ok(OriginImage {
                name: name.clone(),
                path: run::project_relative(root, path),
                hash: recipe::content_hash(&bytes),
            })
        })
        .collect()
}

/// `path` with `suffix` appended to its whole filename, so `idol.png` gets `idol.png.origin.yaml`
/// rather than losing its extension.
fn with_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.as_os_str().to_os_string();
    name.push(suffix);
    PathBuf::from(name)
}

fn refused(message: String) -> Diagnostic {
    Diagnostic::new(code::GENERATION_ACCEPT_REFUSED, message)
}

fn write_failed(path: &Path, error: std::io::Error) -> Diagnostic {
    Diagnostic::new(
        code::OUTPUT_IO,
        format!("could not write `{}`: {error}", path.display()),
    )
    .with_help("check the folder exists and is writable")
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::run::RunOutput;

    const WORKFLOW: &str = r#"{
  "1": {"class_type": "CheckpointLoaderSimple", "inputs": {"ckpt_name": "flux2-klein-4b.safetensors"}, "_meta": {"title": "Checkpoint"}},
  "2": {"class_type": "CLIPTextEncode", "inputs": {"text": "a placeholder", "clip": ["1", 1]}, "_meta": {"title": "Positive Prompt"}},
  "3": {"class_type": "SaveImage", "inputs": {"images": ["2", 0]}, "_meta": {"title": "Save"}}
}"#;

    const RECIPE: &str = "\
format: 1
kind: generation
name: idol
workflow: \"../workflows/txt2img.json\"
seed: 7
count: 2
inputs:
  Positive Prompt:
    text: \"a weathered stone idol on a low plinth\"
";

    const EXPECTED_SIDECAR: &str = "\
format: 1
kind: origin
tool: comfyui
comfy_version: \"0.3.99\"
recipe: generation/recipes/idol.gen.yaml
recipe_hash: \"0000000012345678\"
run_id: \"20260918T101112Z-1a2b3c4d\"
date: \"2026-09-18\"
seed: 9
workflow: generation/workflows/txt2img.json
workflow_hash: \"00000000deadbeef\"
checkpoints:
  - flux2-klein-4b.safetensors
prompt: a weathered stone idol on a low plinth
inputs:
  Positive Prompt:
    text: a weathered stone idol on a low plinth
";

    /// A throwaway project with a recipe, its workflow, and one finished run holding `count`
    /// candidates.
    struct Fixture {
        root: PathBuf,
        run_dir: PathBuf,
    }

    /// The registry an accept emits the imported prefab through. Empty here: these tests assert what
    /// the accept writes into the mesh documents, and the built-in component set lives above this
    /// crate (`ludion-engine`), which is the CLI's business to pass in.
    fn registry() -> Registry {
        Registry::new()
    }

    /// The committed textured glTF (`examples/import-textured/`, five maps on two cubes) wrapped in
    /// a GLB container, so a mesh candidate is the binary form a `SaveGLB` node writes. Its buffer
    /// and its PNG maps are data URIs, so the container needs no BIN chunk and the bytes are
    /// self-contained wherever the run folder sits.
    fn glb_candidate() -> Vec<u8> {
        let chunk = std::fs::read(
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/import-textured/scene.gltf"),
        )
        .expect("the committed textured glTF fixture");
        glb_of(chunk)
    }

    /// A GLB container around one JSON chunk.
    fn glb_of(mut chunk: Vec<u8>) -> Vec<u8> {
        while !chunk.len().is_multiple_of(4) {
            chunk.push(b' ');
        }
        let mut glb = Vec::with_capacity(chunk.len() + 20);
        glb.extend_from_slice(b"glTF");
        glb.extend_from_slice(&2u32.to_le_bytes());
        glb.extend_from_slice(&(20 + chunk.len() as u32).to_le_bytes());
        glb.extend_from_slice(&(chunk.len() as u32).to_le_bytes());
        glb.extend_from_slice(&0x4E4F_534Au32.to_le_bytes());
        glb.extend_from_slice(&chunk);
        glb
    }

    fn fixture(name: &str, status: &str, kind: &str, candidates: u32) -> Fixture {
        let root =
            std::env::temp_dir().join(format!("ludion-gen-accept-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("generation/workflows")).expect("the workflow folder");
        std::fs::create_dir_all(root.join("generation/recipes")).expect("the recipe folder");
        std::fs::write(root.join("generation/workflows/txt2img.json"), WORKFLOW)
            .expect("the workflow");
        std::fs::write(root.join("generation/recipes/idol.gen.yaml"), RECIPE).expect("the recipe");

        let run_id = "20260918T101112Z-1a2b3c4d";
        let run_dir = root.join("generation/runs/idol").join(run_id);
        std::fs::create_dir_all(&run_dir).expect("the run folder");

        let extension = if kind == "mesh" { "glb" } else { "png" };
        let outputs: Vec<RunOutput> = (0..candidates)
            .map(|index| RunOutput {
                index,
                seed: Some(7 + u64::from(index)),
                file: format!("{index:04}.{extension}"),
                node: "3".to_string(),
                batch_index: 0,
                prompt_id: format!("job-{index}"),
            })
            .collect();
        for output in &outputs {
            let bytes = if kind == "mesh" {
                glb_candidate()
            } else {
                let pixels: Vec<u8> = (0..2 * 2 * 4).map(|byte| byte as u8).collect();
                ludion_assets::encode_png(2, 2, &pixels).expect("the candidate encodes")
            };
            std::fs::write(run_dir.join(&output.file), bytes).expect("the candidate");
        }

        let report = RunReport {
            run_id: run_id.to_string(),
            recipe: "generation/recipes/idol.gen.yaml".to_string(),
            recipe_hash: "0000000012345678".to_string(),
            workflow: "generation/workflows/txt2img.json".to_string(),
            workflow_hash: "00000000deadbeef".to_string(),
            graph_hash: "1a2b3c4d5e6f7080".to_string(),
            seed_base: 7,
            count: candidates,
            kind: kind.to_string(),
            comfy_version: "0.3.99".to_string(),
            checkpoints: vec!["flux2-klein-4b.safetensors".to_string()],
            images: Vec::new(),
            prompt: Some("a weathered stone idol on a low plinth".to_string()),
            inputs: Some(serde_json::json!({
                "Positive Prompt": {"text": "a weathered stone idol on a low plinth"}
            })),
            prompt_ids: outputs.iter().map(|o| o.prompt_id.clone()).collect(),
            seeds: outputs.iter().map(|o| o.seed).collect(),
            outputs,
            failed: Vec::new(),
            started: "2026-09-18T10:11:12Z".to_string(),
            finished: "2026-09-18T10:11:40Z".to_string(),
            duration_ms: 28_000,
            status: status.to_string(),
            dir: run_dir.clone(),
        };
        let mut json = serde_json::to_string_pretty(&report).expect("the report serializes");
        json.push('\n');
        std::fs::write(run_dir.join("run.json"), json).expect("run.json");

        Fixture { root, run_dir }
    }

    fn codes(diagnostics: &[Diagnostic]) -> Vec<&str> {
        diagnostics
            .iter()
            .map(|diagnostic| diagnostic.code.as_str())
            .collect()
    }

    #[test]
    fn an_accepted_image_is_written_with_a_byte_exact_sidecar() {
        let fixture = fixture("happy", "ok", "image", 3);
        let out = fixture.root.join("assets/idol.png");

        let accepted = accept(&fixture.run_dir, 2, &out, &registry(), false)
            .unwrap_or_else(|d| panic!("the accept succeeds: {:?}", codes(&d)));

        assert_eq!(accepted.destination, out);
        assert_eq!(accepted.source, fixture.run_dir.join("0002.png"));
        assert_eq!(
            accepted.sidecar.as_deref(),
            Some(fixture.root.join("assets/idol.png.origin.yaml").as_path())
        );
        assert_eq!(
            accepted.origin.seed,
            Some(9),
            "candidate `i` ran at `seed + i`"
        );

        let sidecar = std::fs::read_to_string(fixture.root.join("assets/idol.png.origin.yaml"))
            .expect("the sidecar is on disk");
        assert_eq!(sidecar, EXPECTED_SIDECAR);

        let bytes = std::fs::read(&out).expect("the asset is on disk");
        let (width, height, _) =
            ludion_assets::decode_png(&bytes, "idol.png").expect("the asset is a PNG");
        assert_eq!((width, height), (2, 2));
        assert_eq!(
            bytes,
            ludion_assets::encode_png(
                2,
                2,
                &(0..2 * 2 * 4).map(|byte| byte as u8).collect::<Vec<u8>>()
            )
            .expect("the canonical encode"),
            "an accepted image is re-encoded through the workspace's one PNG writer"
        );
    }

    #[test]
    fn an_unknown_seed_leaves_the_sidecar_without_one() {
        let fixture = fixture("no-seed", "ok", "image", 3);
        let mut report = run::read_report(&fixture.run_dir).expect("the fixture's report reads");
        report.outputs[2].seed = None;
        let mut json = serde_json::to_string_pretty(&report).expect("the report serializes");
        json.push('\n');
        std::fs::write(fixture.run_dir.join("run.json"), json).expect("run.json");
        let out = fixture.root.join("assets/idol.png");

        let accepted = accept(&fixture.run_dir, 2, &out, &registry(), false)
            .unwrap_or_else(|d| panic!("the accept succeeds: {:?}", codes(&d)));

        assert_eq!(accepted.origin.seed, None);
        let sidecar = std::fs::read_to_string(fixture.root.join("assets/idol.png.origin.yaml"))
            .expect("the sidecar is on disk");
        assert!(!sidecar.contains("seed:"), "{sidecar}");
        let origin = ludion_scene::check_origin_document(&sidecar).expect("the sidecar checks");
        assert_eq!(origin.seed, None);
    }

    /// Records one uploaded image in a fixture's `run.json` — what a run conditioned on a reference
    /// image writes, and the only place provenance can learn what an asset was made from.
    fn record_image(fixture: &Fixture) {
        let mut report = run::read_report(&fixture.run_dir).expect("the fixture's report reads");
        report.images.push(crate::run::RunImage {
            name: "subject".to_string(),
            path: "generation/references/idol-reference.png".to_string(),
            hash: "0123456789abcdef".to_string(),
            bound_to: Vec::new(),
        });
        let mut json = serde_json::to_string_pretty(&report).expect("the report serializes");
        json.push('\n');
        std::fs::write(fixture.run_dir.join("run.json"), json).expect("run.json");
    }

    #[test]
    fn an_uploaded_image_reaches_the_sidecar_between_checkpoints_and_prompt() {
        let fixture = fixture("images", "ok", "image", 3);
        record_image(&fixture);
        let out = fixture.root.join("assets/idol.png");

        let accepted = accept(&fixture.run_dir, 2, &out, &registry(), false)
            .unwrap_or_else(|d| panic!("the accept succeeds: {:?}", codes(&d)));

        assert_eq!(accepted.origin.images.len(), 1);
        let sidecar = std::fs::read_to_string(accepted.sidecar.expect("the sidecar path"))
            .expect("the sidecar is on disk");
        assert_eq!(
            sidecar,
            EXPECTED_SIDECAR.replace(
                "prompt:",
                "images:\n  - name: subject\n    path: \
                 generation/references/idol-reference.png\n    hash: \"0123456789abcdef\"\nprompt:",
            ),
            "the images block is written right after `checkpoints`"
        );
    }

    const IMAGE_WORKFLOW: &str = r#"{
  "1": {"class_type": "LoadImage", "inputs": {"image": "idol-reference.png"}, "_meta": {"title": "Load Subject"}},
  "2": {"class_type": "SaveImage", "inputs": {"images": ["1", 0]}, "_meta": {"title": "Save"}}
}"#;

    const IMAGE_RECIPE: &str = "format: 1
kind: generation
name: idol
workflow: \"../workflows/txt2img.json\"
seed: 7
count: 1
images:
  subject: ../references/idol-reference.png
inputs:
  Load Subject:
    image:
      image: subject
";

    /// A run from before the `images` record: the recipe declares an image and the run folder holds
    /// its byte copy under `inputs/`, but `run.json` has no snapshot of the prompt, inputs or images.
    fn legacy_image_run(name: &str) -> Fixture {
        let fixture = fixture(name, "ok", "image", 1);
        let root = &fixture.root;
        std::fs::write(
            root.join("generation/workflows/txt2img.json"),
            IMAGE_WORKFLOW,
        )
        .expect("the workflow");
        std::fs::write(root.join("generation/recipes/idol.gen.yaml"), IMAGE_RECIPE)
            .expect("the recipe");
        std::fs::create_dir_all(root.join("generation/references")).expect("the references folder");
        std::fs::write(
            root.join("generation/references/idol-reference.png"),
            b"reference",
        )
        .expect("the reference");
        std::fs::create_dir_all(fixture.run_dir.join("inputs")).expect("the inputs folder");
        std::fs::write(
            fixture.run_dir.join("inputs/idol-reference.png"),
            b"reference",
        )
        .expect("the copy");

        let resolved = recipe::resolve(&root.join("generation/recipes/idol.gen.yaml"))
            .expect("the recipe resolves");
        let mut report = run::read_report(&fixture.run_dir).expect("the fixture's report reads");
        report.recipe_hash = resolved.recipe_hash;
        report.workflow_hash = resolved.workflow_hash;
        report.prompt = None;
        report.inputs = None;
        let mut json = serde_json::to_string_pretty(&report).expect("the report serializes");
        json.push('\n');
        std::fs::write(fixture.run_dir.join("run.json"), json).expect("run.json");
        fixture
    }

    #[test]
    fn a_report_without_an_images_record_rebuilds_it_from_the_run_folders_copies() {
        let fixture = legacy_image_run("legacy-images");
        let out = fixture.root.join("assets/idol.png");

        let accepted = accept(&fixture.run_dir, 0, &out, &registry(), false)
            .unwrap_or_else(|d| panic!("the accept succeeds: {:?}", codes(&d)));

        assert_eq!(accepted.origin.images.len(), 1);
        let image = &accepted.origin.images[0];
        assert_eq!(image.name, "subject");
        assert_eq!(image.path, "generation/references/idol-reference.png");
        assert_eq!(image.hash, recipe::content_hash(b"reference"));
    }

    #[test]
    fn a_report_without_an_images_record_refuses_when_the_copy_is_gone() {
        let fixture = legacy_image_run("legacy-images-gone");
        std::fs::remove_file(fixture.run_dir.join("inputs/idol-reference.png")).expect("the copy");
        let out = fixture.root.join("assets/idol.png");

        let refusal = accept(&fixture.run_dir, 0, &out, &registry(), false)
            .expect_err("the accept is refused");

        assert_eq!(codes(&refusal), ["L1172"]);
        assert!(
            refusal[0].message.contains("`subject`"),
            "{}",
            refusal[0].message
        );
    }

    #[test]
    fn a_report_without_an_images_record_refuses_when_the_copy_differs_from_the_declared_file() {
        let fixture = legacy_image_run("legacy-images-differs");
        std::fs::write(
            fixture.run_dir.join("inputs/idol-reference.png"),
            b"scratch",
        )
        .expect("the copy");
        let out = fixture.root.join("assets/idol.png");

        let refusal = accept(&fixture.run_dir, 0, &out, &registry(), false)
            .expect_err("the accept is refused");

        assert_eq!(codes(&refusal), ["L1172"]);
        assert!(
            refusal[0].message.contains("`subject`"),
            "{}",
            refusal[0].message
        );
    }

    #[test]
    fn a_mesh_accepts_provenance_naming_the_image_it_was_made_from() {
        let fixture = fixture("mesh-images", "ok", "mesh", 1);
        record_image(&fixture);
        let out = fixture.root.join("assets/idol");

        let accepted = accept(&fixture.run_dir, 0, &out, &registry(), false)
            .unwrap_or_else(|d| panic!("the mesh accept succeeds: {:?}", codes(&d)));

        let mesh = accepted
            .mesh
            .as_ref()
            .expect("a mesh accept reports its tree");
        for document in &mesh.meshes {
            let text = std::fs::read_to_string(&document.path).expect("the document is on disk");
            assert!(text.contains("    - name: subject\n"), "{text}");
            assert!(
                text.contains("      path: generation/references/idol-reference.png\n"),
                "{text}"
            );
            let parsed = ludion_scene::check_mesh(&text).expect("the written document checks");
            assert_eq!(
                parsed.origin.expect("provenance").images[0].hash,
                "0123456789abcdef"
            );
        }
    }

    #[test]
    fn a_double_sided_candidate_accepts_strict_and_is_written_culled() {
        let fixture = fixture("mesh-double-sided", "ok", "mesh", 1);
        let chunk = std::fs::read(
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/import-textured/scene.gltf"),
        )
        .expect("the committed textured glTF fixture");
        let patched = String::from_utf8(chunk)
            .expect("the fixture is text")
            .replace(
                "\"pbrMetallicRoughness\"",
                "\"doubleSided\":true,\"pbrMetallicRoughness\"",
            );
        assert!(patched.contains("\"doubleSided\""));
        std::fs::write(
            fixture.run_dir.join("0000.glb"),
            glb_of(patched.into_bytes()),
        )
        .expect("the patched candidate");
        record_image(&fixture);
        let out = fixture.root.join("assets/idol");

        let accepted = accept(&fixture.run_dir, 0, &out, &registry(), false)
            .unwrap_or_else(|d| panic!("a double-sided mesh accepts strict: {:?}", codes(&d)));

        let mesh = accepted
            .mesh
            .as_ref()
            .expect("a mesh accept reports its tree");
        assert!(!mesh.meshes.is_empty());
        for document in &mesh.meshes {
            let text = std::fs::read_to_string(&document.path).expect("the document is on disk");
            assert!(!text.contains("double_sided"), "{text}");
            let parsed = ludion_scene::check_mesh(&text).expect("the written document checks");
            assert!(!parsed.double_sided);
        }
    }

    #[test]
    fn an_l1202_on_the_accept_path_is_readdressed_to_the_flag_then_to_what_was_dropped() {
        let unsupported = Diagnostic::new(
            code::IMPORT_UNSUPPORTED_FEATURE,
            "a `BLEND` alpha mode on mesh 0 has no Ludion mapping",
        )
        .with_help("re-export without it, or pass `--allow-lossy`");

        let strict = readdress_lossy(vec![unsupported.clone()], false);
        let help = strict[0].help.as_deref().unwrap_or_default();
        assert!(help.contains("gen accept"), "{help}");
        assert!(help.contains("--allow-lossy"), "{help}");

        let lossy = readdress_lossy(vec![unsupported], true);
        let help = lossy[0].help.as_deref().unwrap_or_default();
        assert!(
            !help.contains("--allow-lossy"),
            "a warning past the flag never advises passing it: {help}"
        );
        assert!(help.contains("without this feature"), "{help}");
    }

    /// Rewrites a fixture's `run.json` through `edit`.
    fn edit_report(fixture: &Fixture, edit: impl FnOnce(&mut RunReport)) {
        let mut report = run::read_report(&fixture.run_dir).expect("the fixture's report reads");
        edit(&mut report);
        let mut json = serde_json::to_string_pretty(&report).expect("the report serializes");
        json.push('\n');
        std::fs::write(fixture.run_dir.join("run.json"), json).expect("run.json");
    }

    #[test]
    fn an_old_report_with_matching_hashes_re_reads_the_recipe_and_the_graphs_checkpoints() {
        let fixture = fixture("old-match", "ok", "image", 1);
        edit_report(&fixture, |report| {
            report.prompt = None;
            report.inputs = None;
            report.checkpoints = vec!["written-by-the-old-rule".to_string()];
            report.recipe_hash = recipe::content_hash(RECIPE.as_bytes());
            report.workflow_hash = recipe::content_hash(WORKFLOW.as_bytes());
        });
        let accepted = accept(
            &fixture.run_dir,
            0,
            &fixture.root.join("assets/idol.png"),
            &registry(),
            false,
        )
        .unwrap_or_else(|d| panic!("the accept succeeds: {:?}", codes(&d)));
        assert_eq!(
            accepted.origin.prompt,
            "a weathered stone idol on a low plinth"
        );
        assert_eq!(
            accepted.origin.checkpoints,
            vec!["flux2-klein-4b.safetensors"]
        );
    }

    #[test]
    fn an_old_report_whose_recipe_hash_drifted_is_l1172_naming_both_hashes() {
        let fixture = fixture("old-drift", "ok", "image", 1);
        edit_report(&fixture, |report| {
            report.prompt = None;
            report.inputs = None;
            report.recipe_hash = "0000000012345678".to_string();
            report.workflow_hash = recipe::content_hash(WORKFLOW.as_bytes());
        });
        let diagnostics = accept(
            &fixture.run_dir,
            0,
            &fixture.root.join("assets/idol.png"),
            &registry(),
            false,
        )
        .expect_err("a drifted recipe is refused");
        assert_eq!(codes(&diagnostics), vec!["L1172"]);
        let message = &diagnostics[0].message;
        assert!(message.contains("0000000012345678"), "{message}");
        assert!(
            message.contains(&recipe::content_hash(RECIPE.as_bytes())),
            "{message}"
        );
    }

    #[test]
    fn a_snapshotted_report_accepts_after_its_recipe_is_deleted() {
        let fixture = fixture("snapshot-gone", "ok", "image", 1);
        std::fs::remove_file(fixture.root.join("generation/recipes/idol.gen.yaml"))
            .expect("the recipe goes");
        let accepted = accept(
            &fixture.run_dir,
            0,
            &fixture.root.join("assets/idol.png"),
            &registry(),
            false,
        )
        .unwrap_or_else(|d| panic!("the accept succeeds: {:?}", codes(&d)));
        assert_eq!(
            accepted.origin.prompt,
            "a weathered stone idol on a low plinth"
        );
    }

    #[test]
    fn l1172_refuses_an_index_outside_the_run() {
        let fixture = fixture("range", "ok", "image", 2);
        let diagnostics = accept(
            &fixture.run_dir,
            7,
            &fixture.root.join("assets/idol.png"),
            &registry(),
            false,
        )
        .expect_err("index 7 of a two-candidate run is refused");
        assert_eq!(codes(&diagnostics), vec!["L1172"]);
        assert!(
            diagnostics[0].message.contains("has 2 candidates"),
            "{}",
            diagnostics[0].message
        );
    }

    #[test]
    fn l1172_refuses_a_run_that_is_not_finalized() {
        let fixture = fixture("submitted", "submitted", "image", 2);
        let diagnostics = accept(
            &fixture.run_dir,
            0,
            &fixture.root.join("assets/idol.png"),
            &registry(),
            false,
        )
        .expect_err("a submitted run has nothing to accept");
        assert_eq!(codes(&diagnostics), vec!["L1172"]);
        assert!(
            diagnostics[0]
                .help
                .as_deref()
                .unwrap_or_default()
                .contains("gen fetch"),
            "the refusal names the recovery"
        );
    }

    #[test]
    fn a_mesh_candidate_with_an_external_source_is_refused_and_writes_nothing() {
        let fixture = fixture("mesh-external", "ok", "mesh", 1);
        let out = fixture.root.join("assets/idol");
        let json = br#"{"asset":{"version":"2.0"},"buffers":[{"uri":"tri.bin","byteLength":36}]}"#;
        std::fs::write(fixture.run_dir.join("0000.glb"), glb_of(json.to_vec())).expect("the glb");
        std::fs::write(fixture.run_dir.join("tri.bin"), [0u8; 36]).expect("the buffer beside it");

        let diagnostics = accept(&fixture.run_dir, 0, &out, &registry(), false)
            .expect_err("an external buffer is refused even when the file is there");

        assert_eq!(codes(&diagnostics), vec!["L1204"]);
        assert!(!out.exists(), "a refused accept writes nothing");
    }

    #[test]
    fn a_mesh_candidate_is_imported_with_its_provenance_in_every_document() {
        let fixture = fixture("mesh", "ok", "mesh", 1);
        let out = fixture.root.join("assets/idol");

        let accepted = accept(&fixture.run_dir, 0, &out, &registry(), false)
            .unwrap_or_else(|d| panic!("the mesh accept succeeds: {:?}", codes(&d)));

        assert_eq!(accepted.destination, out, "`out` is the import directory");
        assert_eq!(accepted.source, fixture.run_dir.join("0000.glb"));
        assert_eq!(
            accepted.sidecar, None,
            "a mesh document holds its own provenance, so there is no sidecar"
        );
        let mesh = accepted
            .mesh
            .as_ref()
            .expect("a mesh accept reports its tree");
        assert_eq!(
            mesh.meshes.len(),
            2,
            "the fixture's two cubes are two documents"
        );
        assert!(mesh.warnings.is_empty(), "{:?}", codes(&mesh.warnings));
        assert!(
            out.join("model.prefab.yaml").is_file(),
            "the whole import tree is written"
        );

        for document in &mesh.meshes {
            let text = std::fs::read_to_string(&document.path).expect("the document is on disk");
            let parsed = ludion_scene::check_mesh(&text).expect("the written document checks");
            assert_eq!(
                ludion_scene::emit_mesh(&parsed),
                text,
                "provenance and texture references round-trip byte-stably"
            );
            assert_eq!(
                parsed.origin.as_ref().map(|origin| origin.seed),
                Some(Some(7))
            );
            assert_eq!(document.vertices, parsed.vertex_count());
            assert_eq!(document.triangles, parsed.triangle_count());
            assert_eq!((document.vertices, document.triangles), (24, 12));

            let name_at = text.find("name:").expect("the document is named");
            let origin_at = text
                .find("origin:")
                .expect("the provenance block is written");
            let positions_at = text.find("positions:").expect("the geometry follows");
            assert!(
                name_at < origin_at && origin_at < positions_at,
                "`origin:` sits right after `name:`:\n{text}"
            );
            assert!(
                text.contains("base_color_texture:") && text.contains("normal_texture:"),
                "the texture references survive the accept:\n{text}"
            );
        }

        for texture in &mesh.textures {
            assert!(texture.is_file(), "{}", texture.display());
        }
        assert!(
            !mesh.textures.is_empty(),
            "the textured fixture writes its maps"
        );
    }

    #[test]
    fn l1172_refuses_a_mesh_destination_that_already_holds_files() {
        let fixture = fixture("mesh-exists", "ok", "mesh", 1);
        let out = fixture.root.join("assets/idol");
        std::fs::create_dir_all(&out).expect("the destination folder");
        std::fs::write(out.join("model.prefab.yaml"), b"already here").expect("the squatter");

        let diagnostics = accept(&fixture.run_dir, 0, &out, &registry(), false)
            .expect_err("a mesh accept never overwrites an import tree");
        assert_eq!(codes(&diagnostics), vec!["L1172"]);
        assert_eq!(
            std::fs::read(out.join("model.prefab.yaml")).expect("the squatter survives"),
            b"already here",
            "the refusal happens before anything is written"
        );
    }

    #[test]
    fn l1172_refuses_an_index_outside_a_mesh_run() {
        let fixture = fixture("mesh-range", "ok", "mesh", 1);
        let diagnostics = accept(
            &fixture.run_dir,
            3,
            &fixture.root.join("assets/idol"),
            &registry(),
            false,
        )
        .expect_err("index 3 of a one-candidate mesh run is refused");
        assert_eq!(codes(&diagnostics), vec!["L1172"]);
        assert!(
            diagnostics[0].message.contains("has 1 candidates"),
            "{}",
            diagnostics[0].message
        );
    }

    #[test]
    fn an_out_under_a_regular_file_is_an_output_io_failure() {
        let fixture = fixture("blocked", "ok", "image", 1);
        let blocker = fixture.root.join("blocker.txt");
        std::fs::write(&blocker, b"a file, not a folder").expect("the blocker");
        let out = blocker.join("idol.png");

        let diagnostics = accept(&fixture.run_dir, 0, &out, &registry(), false)
            .expect_err("a file cannot hold a folder");
        assert_eq!(codes(&diagnostics), vec!["L1038"]);
    }

    #[test]
    fn a_failed_sidecar_write_leaves_no_asset() {
        let dir = std::env::temp_dir().join(format!("ludion-gen-sidecar-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("the scratch dir");
        let blocker = dir.join("blocker.txt");
        std::fs::write(&blocker, b"a file, not a folder").expect("the blocker");
        let out = dir.join("idol.png");

        let diagnostics = write_asset_and_sidecar(
            &out,
            b"png bytes",
            &blocker.join("idol.png.origin.yaml"),
            "origin",
        )
        .expect_err("a file cannot hold a folder");
        let left_behind = out.exists();
        std::fs::remove_dir_all(&dir).expect("the scratch dir is removed");

        assert_eq!(codes(&diagnostics), vec!["L1038"]);
        assert!(!left_behind, "the asset is rolled back with its sidecar");
    }

    #[test]
    fn l1172_never_overwrites_an_existing_asset_or_sidecar() {
        let fixture = fixture("exists", "ok", "image", 1);
        let out = fixture.root.join("assets/idol.png");
        std::fs::create_dir_all(out.parent().expect("a parent")).expect("the asset folder");
        std::fs::write(&out, b"already here").expect("the squatter");

        let diagnostics = accept(&fixture.run_dir, 0, &out, &registry(), false)
            .expect_err("an accept never overwrites");
        assert_eq!(codes(&diagnostics), vec!["L1172"]);
        assert_eq!(
            std::fs::read(&out).expect("the squatter survives"),
            b"already here",
            "the refusal happens before anything is written"
        );
    }
}
