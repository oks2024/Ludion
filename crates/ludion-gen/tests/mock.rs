//! The generation client against a mock ComfyUI server (`common/mock_server.rs`, shared with
//! `ludion-cli`'s `gen` tests).
//!
//! The harness binds a `TcpListener` on port `0` and serves the six endpoints plus `/ws` from
//! committed fixtures, so every failure mode this crate has a code for is reproducible with no GPU,
//! no Python and no network. Two cases cover the happy path end to end — the image family and the
//! mesh one, whose files the server reports under a different `outputs` key; one covers each of
//! `L1162`–`L1171`; two cover the two paths through `follow` and the submit/fetch split. The mock
//! routes a job's frames only to the socket registered under its client id at submit and never
//! replays them, the way ComfyUI does; two cases pin the socket-before-submit order and the
//! `/history` sweep. `fail_jobs`, `forget` and `restart` make jobs fail, vanish and be forgotten
//! the way a ComfyUI error and a restart do. `/upload/image` keeps a real input store, parsed
//! byte-exactly from the multipart body: `overwrite` replaces, identical bytes reuse the name,
//! different bytes take the next free `stem (i).ext`. `/view` serves `truncated.png` as a body cut
//! off after ten of a promised thousand bytes.
//!
//! The **live smoke** is the `#[ignore]`d `live_smoke_against_a_real_server` at the bottom: the
//! manual lane, gated on `LUDION_COMFY_URL`, never in hosted CI.

use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::time::Duration;

use ludion_core::diagnostics::Diagnostic;
use ludion_gen::client::{Client, FollowVia, resolve_url};
use ludion_gen::recipe::Resolved;
use ludion_gen::run::RunOptions;
use serde_json::Value;

#[path = "common/mock_server.rs"]
mod mock_server;
use mock_server::*;

// --- Project scaffolding --------------------------------------------------------------------

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

fn read_fixture(name: &str) -> Vec<u8> {
    std::fs::read(fixtures().join(name)).expect("a committed fixture")
}

/// A throwaway project tree: `generation/workflows/<workflow>` beside
/// `generation/recipes/<name>.gen.yaml`, with the run folder landing under the same root.
struct Project {
    root: PathBuf,
    recipe: PathBuf,
}

fn project(name: &str, workflow: &str, recipe_body: &str) -> Project {
    let root = std::env::temp_dir().join(format!("ludion-gen-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let workflows = root.join("generation/workflows");
    let recipes = root.join("generation/recipes");
    std::fs::create_dir_all(&workflows).expect("the workflow folder");
    std::fs::create_dir_all(&recipes).expect("the recipe folder");
    std::fs::copy(fixtures().join(workflow), workflows.join(workflow)).expect("the workflow");
    let recipe = recipes.join(format!("{name}.gen.yaml"));
    std::fs::write(
        &recipe,
        recipe_body.replace("{workflow}", &format!("../workflows/{workflow}")),
    )
    .expect("the recipe");
    Project { root, recipe }
}

fn resolved(project: &Project) -> Resolved {
    ludion_gen::recipe::resolve(&project.recipe).expect("the recipe resolves")
}

fn options(url: &str) -> RunOptions {
    RunOptions {
        url: url.to_string(),
        timeout: Duration::from_secs(20),
        ..Default::default()
    }
}

fn codes(diagnostics: &[Diagnostic]) -> Vec<&str> {
    diagnostics
        .iter()
        .map(|diagnostic| diagnostic.code.as_str())
        .collect()
}

fn joined(diagnostics: &[Diagnostic]) -> String {
    diagnostics
        .iter()
        .map(|diagnostic| {
            format!(
                "{} {} {}",
                diagnostic.code.as_str(),
                diagnostic.message,
                diagnostic.help.clone().unwrap_or_default()
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

const TXT2IMG_RECIPE: &str = "\
format: 1
kind: generation
name: idol
workflow: \"{workflow}\"
seed: 7
count: 2
inputs:
  Empty Latent:
    width: 1024
    height: 1024
  Positive Prompt:
    text: \"a weathered stone idol on a low plinth\"
";

// --- The happy path -------------------------------------------------------------------------

#[test]
fn a_run_submits_follows_downloads_and_writes_its_report() {
    let mock = Mock::start(MockConfig::default());
    let project = project("happy", "txt2img.json", TXT2IMG_RECIPE);
    let resolved = resolved(&project);

    let report = ludion_gen::run::run(&resolved, &options(&mock.url))
        .unwrap_or_else(|d| panic!("the run succeeds:\n{}", joined(&d.diagnostics)));

    assert_eq!(report.count, 2, "the recipe's own `count` drives the run");
    assert_eq!(report.kind, "image");
    assert_eq!(report.status, "ok");
    assert_eq!(report.comfy_version, "0.3.99");
    assert_eq!(report.checkpoints, vec!["flux2-klein-4b.safetensors"]);
    assert_eq!(report.outputs.len(), 2);
    assert_eq!(report.outputs[0].file, "0000.png");
    assert_eq!(report.outputs[0].seed, Some(7));
    assert_eq!(report.outputs[1].file, "0001.png");
    assert_eq!(
        report.outputs[1].seed,
        Some(8),
        "candidate `i` runs at `seed + i`"
    );
    assert_eq!(report.outputs[0].node, "7");

    for output in &report.outputs {
        let path = report.dir.join(&output.file);
        assert_eq!(
            std::fs::read(&path).expect("the candidate is on disk"),
            read_fixture("candidate.png"),
            "the download is byte-identical to what `/view` served"
        );
    }

    // The run id is the sortable timestamp plus the graph hash's first eight hex.
    let (stamp, hash) = report
        .run_id
        .split_once('-')
        .expect("the run id has two halves");
    assert_eq!(stamp.len(), 16, "`YYYYMMDDTHHMMSSZ`");
    assert_eq!(hash, &report.graph_hash[..8]);
    assert!(report.dir.ends_with(&report.run_id));
    assert!(
        report
            .dir
            .starts_with(project.root.join("generation/runs/idol")),
        "a run folder is scratch under the project root: {}",
        report.dir.display()
    );

    // The recipe's base seed reaches the sampler, and `--count` moves only the seed.
    let submitted = mock.state.prompts.lock().unwrap().clone();
    assert_eq!(submitted.len(), 2);
    let seed_of = |graph: &Value| graph["5"]["inputs"]["seed"].as_u64().unwrap();
    assert_eq!(seed_of(&submitted[0].1), 7);
    assert_eq!(seed_of(&submitted[1].1), 8);
    assert_eq!(submitted[0].1["4"]["inputs"]["width"].as_u64(), Some(1024));
    assert_eq!(
        submitted[0].1["2"]["inputs"]["text"].as_str(),
        Some("a weathered stone idol on a low plinth"),
        "an override reaches the node its `_meta.title` names"
    );
    assert_eq!(
        mock.state.client_ids.lock().unwrap().as_slice(),
        &[report.run_id.clone(), report.run_id.clone()],
        "both jobs are submitted under one client id, which is the run id"
    );

    // `run.json`: the plan's key order, and no server address anywhere in it.
    let text = std::fs::read_to_string(report.dir.join("run.json")).expect("run.json");
    let parsed: Value = serde_json::from_str(&text).expect("run.json is JSON");
    let keys: Vec<&str> = parsed
        .as_object()
        .expect("an object")
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(
        keys,
        vec![
            "run_id",
            "recipe",
            "recipe_hash",
            "workflow",
            "workflow_hash",
            "graph_hash",
            "seed_base",
            "count",
            "kind",
            "comfy_version",
            "checkpoints",
            "images",
            "prompt",
            "inputs",
            "prompt_ids",
            "seeds",
            "outputs",
            "failed",
            "started",
            "finished",
            "duration_ms",
            "status",
        ]
    );
    assert_eq!(
        parsed["recipe"].as_str(),
        Some("generation/recipes/happy.gen.yaml"),
        "paths are recorded relative to the project root, in forward slashes"
    );
    assert_eq!(
        parsed["workflow"].as_str(),
        Some("generation/workflows/txt2img.json")
    );
    let host = mock.url.trim_start_matches("http://");
    assert!(
        !text.contains(host) && !text.contains("http"),
        "the server address is never a field of a run report"
    );

    let output_keys: Vec<&str> = parsed["outputs"][0]
        .as_object()
        .expect("an output entry")
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(
        output_keys,
        vec!["index", "seed", "file", "node", "batch_index", "prompt_id"]
    );
}

/// Provenance comes from `run.json` alone: a recipe edited or renamed after the run changes nothing
/// an accept of that run records.
#[test]
fn an_accept_records_the_run_s_prompt_not_the_recipe_as_it_is_now() {
    let mock = Mock::start(MockConfig::default());
    let project = project("snapshot", "txt2img.json", TXT2IMG_RECIPE);
    let report = ludion_gen::run::run(&resolved(&project), &options(&mock.url))
        .unwrap_or_else(|d| panic!("the run succeeds:\n{}", joined(&d.diagnostics)));

    let edited = TXT2IMG_RECIPE
        .replace("{workflow}", "../workflows/txt2img.json")
        .replace("a weathered stone idol on a low plinth", "a chrome teapot");
    std::fs::write(&project.recipe, edited).expect("the recipe is edited");

    let registry = ludion_ecs::Registry::new();
    let first = ludion_gen::accept(
        &report.dir,
        1,
        &project.root.join("assets/one.png"),
        &registry,
        false,
    )
    .unwrap_or_else(|d| panic!("the accept succeeds:\n{}", joined(&d)));
    let literal = "a weathered stone idol on a low plinth";
    assert_eq!(first.origin.prompt, literal);
    assert_eq!(
        first
            .origin
            .inputs
            .iter()
            .find(|(title, _)| title == "Positive Prompt"),
        Some(&(
            "Positive Prompt".to_string(),
            vec![(
                "text".to_string(),
                ludion_scene::InputValue::Scalar(ludion_scene::ScalarValue::Text(
                    literal.to_string()
                ))
            )]
        ))
    );
    assert_eq!(
        first.origin.recipe_hash.as_deref(),
        Some(report.recipe_hash.as_str())
    );
    assert_eq!(
        first.origin.date,
        report.started[..10],
        "the run's date, not today's"
    );

    let moved = project.recipe.with_file_name("renamed.gen.yaml");
    std::fs::rename(&project.recipe, &moved).expect("the recipe is renamed");
    ludion_gen::accept(
        &report.dir,
        0,
        &project.root.join("assets/two.png"),
        &registry,
        false,
    )
    .unwrap_or_else(|d| {
        panic!(
            "a renamed recipe no longer blocks an accept:\n{}",
            joined(&d)
        )
    });
}

/// A mesh graph runs the same pipeline to a different file family: the `3d` entry of the server's
/// `/history`, a `.glb` on disk, `kind: "mesh"` in the report — and the mesh-info node's `text`
/// entry in the same reply is ignored rather than counted or missed.
#[test]
fn a_mesh_run_downloads_its_glb_and_records_kind_mesh() {
    let mock = Mock::start(MockConfig::default());
    let project = project(
        "mesh",
        "img2mesh.json",
        "\
format: 1
kind: generation
name: idol-mesh
workflow: \"{workflow}\"
seed: 11
count: 1
",
    );

    let report = ludion_gen::run::run(&resolved(&project), &options(&mock.url))
        .unwrap_or_else(|d| panic!("the mesh run succeeds:\n{}", joined(&d.diagnostics)));

    assert_eq!(report.kind, "mesh");
    assert_eq!(report.status, "ok");
    assert_eq!(
        report.outputs.len(),
        1,
        "the `text` entry beside the file is not an output: {:?}",
        report.outputs
    );
    assert_eq!(report.outputs[0].file, "0000.glb");
    assert_eq!(report.outputs[0].node, "12");
    assert_eq!(
        report.outputs[0].seed, None,
        "the fixture graph has no seed input, so the run cannot know its seed"
    );
    assert_eq!(
        std::fs::read(report.dir.join("0000.glb")).expect("the candidate is on disk"),
        MESH_BODY,
        "the download is byte-identical to what `/view` served"
    );

    let parsed: Value = serde_json::from_str(
        &std::fs::read_to_string(report.dir.join("run.json")).expect("run.json"),
    )
    .expect("run.json is JSON");
    assert_eq!(parsed["kind"].as_str(), Some("mesh"));
    assert_eq!(parsed["outputs"][0]["file"].as_str(), Some("0000.glb"));
}

#[test]
fn a_declared_image_is_uploaded_and_its_server_name_reaches_the_node() {
    let mock = Mock::start(MockConfig::default());
    let recipe = "\
format: 1
kind: generation
name: portrait
workflow: \"{workflow}\"
seed: 3
images:
  subject: \"subject.png\"
inputs:
  Load Subject:
    image:
      image: subject
";
    let project = project("images", "img2img.json", recipe);
    std::fs::copy(
        fixtures().join("candidate.png"),
        project.root.join("generation/recipes/subject.png"),
    )
    .expect("the committed source image");

    let resolved = resolved(&project);
    let report = ludion_gen::run::run(&resolved, &options(&mock.url))
        .unwrap_or_else(|d| panic!("the run succeeds:\n{}", joined(&d.diagnostics)));

    let source = project.root.join("generation/recipes/subject.png");
    let bytes = std::fs::read(&source).expect("the source image");
    let uploaded = format!("ludion-{}.png", ludion_gen::recipe::content_hash(&bytes));
    assert_eq!(
        mock.state.uploads.lock().unwrap().as_slice(),
        std::slice::from_ref(&uploaded),
        "the declared image goes up before the submit, named after its bytes"
    );
    let submitted = mock.state.prompts.lock().unwrap().clone();
    assert_eq!(
        submitted[0].1["2"]["inputs"]["image"].as_str(),
        Some(uploaded.as_str()),
        "the node holds the name the server gave the upload, not the recipe's path"
    );
    assert_eq!(
        std::fs::read(report.dir.join("inputs").join(&uploaded)).expect("the kept source"),
        bytes,
        "every uploaded source is kept in the run folder under its upload name"
    );
    assert_eq!(
        report.images,
        vec![ludion_gen::RunImage {
            name: "subject".to_string(),
            path: "generation/recipes/subject.png".to_string(),
            hash: ludion_gen::recipe::content_hash(&bytes),
            bound_to: vec![ludion_gen::RunBinding {
                node: "Load Subject".to_string(),
                input: "image".to_string(),
            }],
        }],
        "the report says which image went up, where it sits, what its bytes hashed to, and which \
         override feeds it"
    );

    // A `--image` override replaces the first entry, and a scratch file outside the project root is
    // recorded by file name alone — an absolute machine path never reaches a committed file.
    let scratch_name = format!("ludion-gen-scratch-subject-{}.png", std::process::id());
    let scratch = std::env::temp_dir().join(&scratch_name);
    std::fs::copy(&source, &scratch).expect("the scratch candidate");
    let overridden = ludion_gen::run::run(
        &resolved,
        &RunOptions {
            image: Some(scratch.clone()),
            ..options(&mock.url)
        },
    )
    .unwrap_or_else(|d| panic!("the override run succeeds:\n{}", joined(&d.diagnostics)));
    assert_eq!(overridden.images[0].name, "subject");
    assert_eq!(overridden.images[0].path, scratch_name);
    assert_eq!(overridden.images[0].hash, report.images[0].hash);
}

/// ComfyUI keeps an upload under its file name, so two scratch candidates both called `0003.png`
/// must not share one stored name: each graph has to load the bytes its own run sent.
#[test]
fn uploads_with_one_file_name_keep_their_own_bytes() {
    let mock = Mock::start(MockConfig::default());
    let recipe = "\
format: 1
kind: generation
name: portrait
workflow: \"{workflow}\"
seed: 3
images:
  subject: \"subject.png\"
inputs:
  Load Subject:
    image:
      image: subject
";
    let project = project("same-name-uploads", "img2img.json", recipe);
    std::fs::copy(
        fixtures().join("candidate.png"),
        project.root.join("generation/recipes/subject.png"),
    )
    .expect("the committed source image");
    let resolved = resolved(&project);

    let mut sources = Vec::new();
    let mut hashes = Vec::new();
    for (index, tag) in ["a", "b"].into_iter().enumerate() {
        let directory =
            std::env::temp_dir().join(format!("ludion-gen-same-name-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&directory);
        std::fs::create_dir_all(&directory).expect("the scratch folder");
        let scratch = directory.join("0003.png");
        let mut bytes = read_fixture("candidate.png");
        bytes.extend_from_slice(format!("candidate {index}").as_bytes());
        std::fs::write(&scratch, &bytes).expect("the scratch candidate");
        let report = ludion_gen::run::run(
            &resolved,
            &RunOptions {
                image: Some(scratch),
                wait: false,
                ..options(&mock.url)
            },
        )
        .unwrap_or_else(|d| panic!("the run succeeds:\n{}", joined(&d.diagnostics)));
        hashes.push(report.graph_hash);
        sources.push(bytes);
    }

    let files = mock.state.files.lock().unwrap().clone();
    let submitted = mock.state.prompts.lock().unwrap().clone();
    // Two runs of `count` 1, one graph each.
    assert_eq!(submitted.len(), 2);
    for (graph, bytes) in submitted.iter().map(|(_, graph)| graph).zip(&sources) {
        let name = graph["2"]["inputs"]["image"].as_str().unwrap();
        assert_eq!(
            files.get(name),
            Some(bytes),
            "the graph loads the bytes its own run uploaded (`{name}`)"
        );
    }
    assert_ne!(
        hashes[0], hashes[1],
        "the graph hash covers the image bytes, not just their file name"
    );
}

/// Two committed sources that share a file name used to land on one `inputs/` entry, the second
/// overwriting the first.
#[test]
fn two_sources_with_one_file_name_are_both_kept_in_the_run_folder() {
    let mock = Mock::start(MockConfig::default());
    let recipe = "\
format: 1
kind: generation
name: pair
workflow: \"{workflow}\"
images:
  mask: \"a/ref.png\"
  subject: \"b/ref.png\"
inputs:
  Load Mask:
    image:
      image: mask
  Load Subject:
    image:
      image: subject
";
    let project = project("same-name-sources", "two-images.json", recipe);
    for (folder, tag) in [("a", "mask"), ("b", "subject")] {
        let directory = project.root.join("generation/recipes").join(folder);
        std::fs::create_dir_all(&directory).expect("the source folder");
        let mut bytes = read_fixture("candidate.png");
        bytes.extend_from_slice(tag.as_bytes());
        std::fs::write(directory.join("ref.png"), bytes).expect("the committed source");
    }
    let resolved = resolved(&project);
    let report = ludion_gen::run::run(&resolved, &options(&mock.url))
        .unwrap_or_else(|d| panic!("the run succeeds:\n{}", joined(&d.diagnostics)));

    assert_eq!(report.images.len(), 2);
    for image in &report.images {
        let source = std::fs::read(project.root.join(&image.path)).expect("the source image");
        let kept = report
            .dir
            .join("inputs")
            .join(format!("ludion-{}.png", image.hash));
        assert_eq!(
            std::fs::read(&kept).unwrap_or_else(|e| panic!("{}: {e}", kept.display())),
            source,
            "`inputs/` holds the bytes of `{}`",
            image.path
        );
    }
    assert_eq!(
        std::fs::read_dir(report.dir.join("inputs"))
            .unwrap()
            .count(),
        2,
        "one file per source"
    );
}

#[test]
fn two_runs_of_one_graph_in_one_second_get_two_folders() {
    let mock = Mock::start(MockConfig::default());
    // The stamp has one-second resolution; a pair straddling a tick is retried in a fresh project,
    // so an earlier attempt's folders cannot take a suffix.
    let stamp =
        |report: &ludion_gen::RunReport| report.run_id.split('-').next().unwrap().to_string();
    let (a, b) = (0..5)
        .find_map(|attempt| {
            let project = project(
                &format!("same-second-{attempt}"),
                "txt2img.json",
                TXT2IMG_RECIPE,
            );
            let resolved = resolved(&project);
            let run = || {
                ludion_gen::run::run(&resolved, &options(&mock.url))
                    .unwrap_or_else(|d| panic!("the run succeeds:\n{}", joined(&d.diagnostics)))
            };
            let (a, b) = (run(), run());
            (stamp(&a) == stamp(&b)).then_some((a, b))
        })
        .expect("two runs land in one second within five tries");

    assert_eq!(b.run_id, format!("{}-2", a.run_id), "the second takes `-2`");
    assert_ne!(a.dir, b.dir);
    assert!(b.dir.ends_with(&b.run_id), "the run id is the folder name");
    let read = ludion_gen::run::read_report(&a.dir).expect("the first run's report");
    assert_eq!(
        read.prompt_ids, a.prompt_ids,
        "the second run did not overwrite the first's report"
    );
}

// --- One case per code ----------------------------------------------------------------------

#[test]
fn l1162_refuses_a_workflow_outside_the_project_root() {
    let sibling = std::env::temp_dir().join(format!(
        "ludion-gen-elsewhere-workflow-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&sibling);
    std::fs::create_dir_all(&sibling).expect("the sibling folder");
    std::fs::copy(
        fixtures().join("txt2img.json"),
        sibling.join("txt2img.json"),
    )
    .expect("the outside workflow");
    let project = project(
        "outside-workflow",
        "txt2img.json",
        &format!(
            "format: 1\nkind: generation\nworkflow: \"../../../{}/txt2img.json\"\n",
            sibling.file_name().unwrap().to_string_lossy()
        ),
    );

    let diagnostics =
        ludion_gen::recipe::resolve(&project.recipe).expect_err("the workflow is outside");
    assert_eq!(codes(&diagnostics), vec!["L1162"]);
    assert_eq!(diagnostics[0].path.as_deref(), Some("/workflow"));
    assert!(
        joined(&diagnostics).contains("outside the project root"),
        "{}",
        joined(&diagnostics)
    );
}

#[test]
fn l1162_names_a_workflow_path_that_does_not_resolve() {
    let root = std::env::temp_dir().join(format!(
        "ludion-gen-missing-workflow-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    let recipes = root.join("generation/recipes");
    std::fs::create_dir_all(&recipes).expect("the recipe folder");
    let recipe = recipes.join("ghost.gen.yaml");
    std::fs::write(
        &recipe,
        "format: 1\nkind: generation\nworkflow: \"../workflows/nothing.json\"\n",
    )
    .expect("the recipe");

    let diagnostics = ludion_gen::recipe::resolve(&recipe).expect_err("the path does not resolve");
    assert_eq!(codes(&diagnostics), vec!["L1162"]);
    assert_eq!(diagnostics[0].path.as_deref(), Some("/workflow"));
}

#[test]
fn l1162_names_an_images_entry_that_does_not_resolve() {
    let project = project(
        "missing-image",
        "img2img.json",
        "\
format: 1
kind: generation
workflow: \"{workflow}\"
images:
  subject: \"renders/ghost.png\"
",
    );
    let diagnostics =
        ludion_gen::recipe::resolve(&project.recipe).expect_err("the image does not resolve");
    assert_eq!(codes(&diagnostics), vec!["L1162"]);
    assert_eq!(diagnostics[0].path.as_deref(), Some("/images/subject"));
}

/// An `images:` entry no override binds would run the workflow's baked-in image: `apply` (the
/// offline half of `gen check`) and `run` (before any upload) both refuse it with `L1217`.
#[test]
fn l1217_refuses_a_declared_image_no_override_binds() {
    let mock = Mock::start(MockConfig::default());
    let project = project(
        "unbound-image",
        "img2img.json",
        "format: 1
kind: generation
workflow: \"{workflow}\"
images:
  subject: \"subject.png\"
",
    );
    std::fs::copy(
        fixtures().join("candidate.png"),
        project.root.join("generation/recipes/subject.png"),
    )
    .expect("the committed source image");
    let resolved = resolved(&project);

    let diagnostics =
        ludion_gen::workflow::apply(&resolved.graph, &resolved.doc, &Default::default())
            .expect_err("the unbound image is an offline error");
    assert_eq!(codes(&diagnostics), vec!["L1217"]);
    assert_eq!(diagnostics[0].path.as_deref(), Some("/images/subject"));
    assert!(
        !diagnostics[0].message.contains("  ")
            && !diagnostics[0]
                .help
                .as_deref()
                .unwrap_or_default()
                .contains("  "),
        "no run of spaces from a joined literal: {}",
        joined(&diagnostics)
    );
    assert!(
        joined(&diagnostics).contains("baked-in"),
        "{}",
        joined(&diagnostics)
    );

    let diagnostics = ludion_gen::run::run(&resolved, &options(&mock.url))
        .expect_err("a run refuses before submitting")
        .diagnostics;
    assert_eq!(codes(&diagnostics), vec!["L1217"]);
    assert!(
        mock.state.uploads.lock().unwrap().is_empty(),
        "nothing is uploaded for a refused recipe"
    );
}

#[test]
fn l1163_refuses_a_ui_workflow_and_names_the_export_menu() {
    let project = project(
        "ui-format",
        "ui-workflow.json",
        "format: 1\nkind: generation\nworkflow: \"{workflow}\"\n",
    );
    let diagnostics =
        ludion_gen::recipe::resolve(&project.recipe).expect_err("a UI export is not submittable");
    assert_eq!(codes(&diagnostics), vec!["L1163"]);
    let text = joined(&diagnostics);
    assert!(text.contains("UI (litegraph)"), "{text}");
    assert!(text.contains("Export (API)"), "{text}");
}

#[test]
fn l1164_names_an_unknown_class_and_its_nearest_installed_match() {
    let mock = Mock::start(MockConfig::default());
    let project = project(
        "unknown-node",
        "unknown-node.json",
        "format: 1\nkind: generation\nworkflow: \"{workflow}\"\n",
    );
    let diagnostics = ludion_gen::run::run(&resolved(&project), &options(&mock.url))
        .expect_err("the server has no such class")
        .diagnostics;
    assert_eq!(codes(&diagnostics), vec!["L1164"]);
    let text = joined(&diagnostics);
    assert!(text.contains("EmptyLatentImag"), "{text}");
    assert!(
        text.contains("nearest installed class is `EmptyLatentImage`"),
        "{text}"
    );
}

#[test]
fn l1165_names_a_required_input_the_graph_never_sets() {
    let mock = Mock::start(MockConfig::default());
    let project = project(
        "missing-input",
        "missing-input.json",
        "format: 1\nkind: generation\nworkflow: \"{workflow}\"\n",
    );
    let diagnostics = ludion_gen::run::run(&resolved(&project), &options(&mock.url))
        .expect_err("`steps` is neither set nor linked")
        .diagnostics;
    assert_eq!(codes(&diagnostics), vec!["L1165"]);
    let text = joined(&diagnostics);
    assert!(text.contains("`steps`"), "{text}");
    assert!(text.contains("`Sampler` (KSampler)"), "{text}");
}

#[test]
fn l1165_names_an_override_whose_type_the_input_refuses() {
    let mock = Mock::start(MockConfig::default());
    let project = project(
        "type-mismatch",
        "txt2img.json",
        "\
format: 1
kind: generation
workflow: \"{workflow}\"
inputs:
  Empty Latent:
    width: \"wide\"
  Sampler:
    scheduler: \"kerrus\"
",
    );
    let diagnostics = ludion_gen::run::run(&resolved(&project), &options(&mock.url))
        .expect_err("neither override fits its input")
        .diagnostics;
    assert_eq!(codes(&diagnostics), vec!["L1165", "L1165"]);
    assert_eq!(
        diagnostics[0].path.as_deref(),
        Some("/inputs/Empty Latent/width")
    );
    assert_eq!(
        diagnostics[1].path.as_deref(),
        Some("/inputs/Sampler/scheduler")
    );
    let text = joined(&diagnostics);
    assert!(text.contains("expects an integer (`INT`)"), "{text}");
    assert!(
        text.contains("is a picklist and does not offer `kerrus`"),
        "the newer `[\"COMBO\", {{options}}]` spelling is read too: {text}"
    );
    assert!(text.contains("nearest option is `karras`"), "{text}");
}

#[test]
fn l1166_names_an_absent_title_and_lists_the_graph_s_own() {
    let mock = Mock::start(MockConfig::default());
    let project = project(
        "absent-title",
        "txt2img.json",
        "\
format: 1
kind: generation
workflow: \"{workflow}\"
inputs:
  Postive Prompt:
    text: \"an idol\"
",
    );
    let diagnostics = ludion_gen::run::run(&resolved(&project), &options(&mock.url))
        .expect_err("no node carries that title")
        .diagnostics;
    assert_eq!(codes(&diagnostics), vec!["L1166"]);
    let text = joined(&diagnostics);
    assert!(text.contains("`Checkpoint`, `Decode`"), "{text}");
    assert!(
        text.contains("nearest title is `Positive Prompt`"),
        "{text}"
    );
}

#[test]
fn l1166_refuses_a_title_two_nodes_share() {
    let mock = Mock::start(MockConfig::default());
    let project = project(
        "shared-title",
        "duplicate-title.json",
        "\
format: 1
kind: generation
workflow: \"{workflow}\"
inputs:
  Positive Prompt:
    text: \"an idol\"
",
    );
    let diagnostics = ludion_gen::run::run(&resolved(&project), &options(&mock.url))
        .expect_err("the title is ambiguous")
        .diagnostics;
    assert_eq!(codes(&diagnostics), vec!["L1166"]);
    let text = joined(&diagnostics);
    assert!(text.contains("2 nodes"), "{text}");
    assert!(text.contains("ids 2, 3"), "{text}");
}

#[test]
fn l1167_reports_a_server_that_is_not_there() {
    // Bind and drop: the port is known free, so the connection is refused rather than hanging.
    let closed = TcpListener::bind("127.0.0.1:0").expect("a port");
    let url = format!("http://{}", closed.local_addr().expect("an address"));
    drop(closed);

    let project = project(
        "unreachable",
        "txt2img.json",
        "format: 1\nkind: generation\nworkflow: \"{workflow}\"\n",
    );
    let error = ludion_gen::run::run(&resolved(&project), &options(&url))
        .expect_err("nothing answers there");
    assert_eq!(
        error.dir, None,
        "no run folder exists before the server answers"
    );
    let diagnostics = error.diagnostics;
    assert_eq!(codes(&diagnostics), vec!["L1167"]);
    let text = joined(&diagnostics);
    assert!(text.contains("LUDION_COMFY_URL"), "{text}");
    assert!(text.contains("--url"), "{text}");
}

#[test]
fn l1168_reprints_a_prompt_400_node_by_node() {
    let mock = Mock::start(MockConfig::default());
    let project = project(
        "rejected",
        "fail-me.json",
        "format: 1\nkind: generation\nworkflow: \"{workflow}\"\n",
    );
    let diagnostics = ludion_gen::run::run(&resolved(&project), &options(&mock.url))
        .expect_err("the server refuses the graph")
        .diagnostics;
    assert_eq!(codes(&diagnostics), vec!["L1168"]);
    let text = joined(&diagnostics);
    assert!(text.contains("node 8 (FailMe)"), "{text}");
    assert!(text.contains("input `filename_prefix`"), "{text}");
    assert!(text.contains("Value not in list"), "{text}");
}

#[test]
fn l1169_names_the_node_that_failed_on_the_server() {
    let mock = Mock::start(MockConfig::default());
    let project = project(
        "exploded",
        "explode.json",
        "format: 1\nkind: generation\nworkflow: \"{workflow}\"\n",
    );
    let report =
        ludion_gen::run::run(&resolved(&project), &options(&mock.url)).unwrap_or_else(|d| {
            panic!(
                "a failed job is an outcome, not an abort:\n{}",
                joined(&d.diagnostics)
            )
        });
    assert_eq!(report.status, "failed");
    assert!(report.outputs.is_empty());
    assert_eq!(report.failed.len(), 1);
    let diagnostics = report.failure_diagnostics();
    assert_eq!(codes(&diagnostics), vec!["L1169"]);
    let text = joined(&diagnostics);
    assert!(text.contains("node 7 (SaveImage)"), "{text}");
    assert!(text.contains("CUDA out of memory"), "{text}");

    let registry = ludion_ecs::Registry::new();
    let refused = ludion_gen::accept(
        &report.dir,
        0,
        &project.root.join("assets/none.png"),
        &registry,
        false,
    )
    .expect_err("a failed run has no candidate");
    assert_eq!(codes(&refused), vec!["L1172"]);
}

#[test]
fn one_failed_job_is_one_outcome_and_the_rest_download() {
    let mock = Mock::start(MockConfig {
        fail_jobs: &[1],
        ..Default::default()
    });
    let project = project("partial", "txt2img.json", TXT2IMG_RECIPE);
    let mut opts = options(&mock.url);
    opts.count = Some(3);
    let report = ludion_gen::run::run(&resolved(&project), &opts).unwrap_or_else(|d| {
        panic!(
            "one failed job does not abort the run:\n{}",
            joined(&d.diagnostics)
        )
    });

    assert_eq!(report.status, "partial");
    assert_eq!(report.outputs.len(), 2);
    assert_eq!(
        (report.outputs[0].seed, report.outputs[0].prompt_id.as_str()),
        (Some(7), "job-0")
    );
    assert_eq!(
        (report.outputs[1].seed, report.outputs[1].prompt_id.as_str()),
        (Some(9), "job-2")
    );
    assert_eq!(
        report.failed,
        vec![ludion_gen::RunFailure {
            job: 1,
            prompt_id: "job-1".to_string(),
            reason: "error".to_string(),
            node: Some("7".to_string()),
            class: Some("SaveImage".to_string()),
            message: "CUDA out of memory".to_string(),
        }]
    );

    let reread = ludion_gen::run::read_report(&report.dir).expect("run.json");
    assert_eq!(reread.status, "partial");
    assert_eq!(reread.failed, report.failed, "run.json records the failure");

    let diagnostics = report.failure_diagnostics();
    assert_eq!(codes(&diagnostics), vec!["L1169"]);
    let text = joined(&diagnostics);
    assert!(
        text.contains("job 1") && text.contains("job-1") && text.contains("seed 8"),
        "{text}"
    );
    assert!(text.contains("CUDA out of memory"), "{text}");

    ludion_gen::sheet::compose(&report.dir, None, None)
        .unwrap_or_else(|d| panic!("a partial run composes:\n{}", joined(&d)));
    let registry = ludion_ecs::Registry::new();
    let accepted = ludion_gen::accept(
        &report.dir,
        1,
        &project.root.join("assets/second.png"),
        &registry,
        false,
    )
    .unwrap_or_else(|d| panic!("a partial run accepts:\n{}", joined(&d)));
    assert_eq!(
        accepted.origin.seed,
        Some(9),
        "output 1 is job 2, at seed + 2"
    );
}

#[test]
fn a_failed_job_is_one_outcome_when_polling_too() {
    let mock = Mock::start(MockConfig {
        websocket: false,
        fail_jobs: &[0],
        ..Default::default()
    });
    let project = project("partial-polling", "txt2img.json", TXT2IMG_RECIPE);
    let report =
        ludion_gen::run::run(&resolved(&project), &options(&mock.url)).unwrap_or_else(|d| {
            panic!(
                "polling sees a failed job as an outcome:\n{}",
                joined(&d.diagnostics)
            )
        });
    assert_eq!(report.status, "partial");
    assert_eq!(report.outputs.len(), 1);
    assert_eq!(report.outputs[0].prompt_id, "job-1");
    assert_eq!(report.failed.len(), 1);
}

#[test]
fn a_job_that_wrote_no_file_is_a_recorded_failure() {
    let mock = Mock::start(MockConfig {
        batch: 0,
        ..Default::default()
    });
    let project = project("empty-jobs", "txt2img.json", TXT2IMG_RECIPE);
    let report = ludion_gen::run::run(&resolved(&project), &options(&mock.url))
        .unwrap_or_else(|d| panic!("empty jobs are an outcome:\n{}", joined(&d.diagnostics)));

    assert_eq!(report.status, "failed");
    assert!(report.outputs.is_empty());
    assert_eq!(
        report
            .failed
            .iter()
            .map(|failure| failure.reason.as_str())
            .collect::<Vec<_>>(),
        vec!["empty", "empty"]
    );
    assert_eq!(codes(&report.failure_diagnostics()), vec!["L1169", "L1169"]);
}

#[test]
fn a_submit_refused_part_way_records_the_jobs_already_submitted() {
    let mock = Mock::start(MockConfig {
        refuse_jobs: &[1],
        ..Default::default()
    });
    let project = project("refused-late", "txt2img.json", TXT2IMG_RECIPE);
    let mut opts = options(&mock.url);
    opts.count = Some(3);
    let error =
        ludion_gen::run::run(&resolved(&project), &opts).expect_err("the second is refused");

    let dir = error.dir.expect("the submitted job's folder is kept");
    let report = ludion_gen::run::read_report(&dir).expect("run.json");
    assert_eq!(report.prompt_ids, vec!["job-0"]);
    assert_eq!(report.seeds.len(), 1);
    assert_eq!(report.count, 1);
    assert_eq!(report.status, "submitted");
    let text = joined(&error.diagnostics);
    assert!(text.contains("gen fetch"), "{text}");

    let fetched = ludion_gen::run::fetch(&dir, &mock.url)
        .unwrap_or_else(|d| panic!("the recorded job fetches:\n{}", joined(&d)));
    assert_eq!(fetched.status, "ok");
    assert_eq!(fetched.outputs.len(), 1);
}

#[test]
fn a_refused_first_submit_leaves_no_run_folder() {
    let mock = Mock::start(MockConfig {
        refuse_jobs: &[0],
        ..Default::default()
    });
    let project = project("refused-first", "txt2img.json", TXT2IMG_RECIPE);
    let error = ludion_gen::run::run(&resolved(&project), &options(&mock.url))
        .expect_err("the first is refused");

    assert_eq!(error.dir, None);
    let runs = project.root.join("generation/runs/idol");
    let left = std::fs::read_dir(&runs).map_or(0, |entries| entries.count());
    assert_eq!(left, 0, "no run folder under {}", runs.display());
}

#[test]
fn a_huge_timeout_does_not_panic() {
    let mock = Mock::start(MockConfig::default());
    let project = project("huge-timeout", "txt2img.json", TXT2IMG_RECIPE);
    let mut opts = options(&mock.url);
    opts.timeout = Duration::MAX;
    ludion_gen::run::run(&resolved(&project), &opts)
        .unwrap_or_else(|d| panic!("{}", joined(&d.diagnostics)));
}

#[test]
fn l1170_names_the_job_and_the_recovery() {
    let mock = Mock::start(MockConfig {
        stall: true,
        ..Default::default()
    });
    let project = project(
        "stalled",
        "txt2img.json",
        "format: 1\nkind: generation\nworkflow: \"{workflow}\"\n",
    );
    let mut opts = options(&mock.url);
    opts.timeout = Duration::from_millis(400);

    let resolved = resolved(&project);
    let error = ludion_gen::run::run(&resolved, &opts).expect_err("the follow outlives its budget");
    let diagnostics = error.diagnostics;
    assert_eq!(codes(&diagnostics), vec!["L1170"]);
    let text = joined(&diagnostics);
    assert!(text.contains("job-0"), "{text}");
    assert!(text.contains("gen status"), "{text}");
    assert!(text.contains("gen fetch"), "{text}");
    assert!(
        text.contains("generation/runs/stalled/"),
        "the help names the run folder the recovery verbs take: {text}"
    );

    // A timed-out run still leaves a folder the recovery verbs can pick up.
    let runs = project.root.join("generation/runs/stalled");
    let folder = std::fs::read_dir(&runs)
        .expect("the run folder survives")
        .next()
        .expect("one run")
        .expect("a directory entry")
        .path();
    assert_eq!(error.dir.as_deref(), Some(folder.as_path()));
    let report = ludion_gen::run::read_report(&folder).expect("its report was written at submit");
    assert_eq!(report.status, "submitted");
    assert!(report.outputs.is_empty());
}

#[test]
fn l1171_refuses_a_graph_with_no_output_node() {
    let mock = Mock::start(MockConfig::default());
    let project = project(
        "no-output",
        "no-output.json",
        "format: 1\nkind: generation\nworkflow: \"{workflow}\"\n",
    );
    let diagnostics = ludion_gen::run::run(&resolved(&project), &options(&mock.url))
        .expect_err("nothing in the graph writes a file")
        .diagnostics;
    assert_eq!(codes(&diagnostics), vec!["L1171"]);
    assert!(joined(&diagnostics).contains("no output node"));
}

#[test]
fn l1171_refuses_a_graph_that_writes_both_kinds() {
    let mock = Mock::start(MockConfig::default());
    let project = project(
        "both-kinds",
        "both-kinds.json",
        "format: 1\nkind: generation\nworkflow: \"{workflow}\"\n",
    );
    let diagnostics = ludion_gen::run::run(&resolved(&project), &options(&mock.url))
        .expect_err("a recipe produces exactly one kind")
        .diagnostics;
    assert_eq!(codes(&diagnostics), vec!["L1171"]);
    let text = joined(&diagnostics);
    assert!(text.contains("`Save Image` (SaveImage)"), "{text}");
    assert!(text.contains("`Save Mesh` (SaveGLB)"), "{text}");
}

// --- The two paths through `follow`, and the submit/fetch split -------------------------------

#[test]
fn a_job_that_finishes_during_the_submit_is_observed() {
    let mock = Mock::start(MockConfig::default());
    let project = project("early-finish", "txt2img.json", TXT2IMG_RECIPE);
    let report = ludion_gen::run::run(&resolved(&project), &options(&mock.url))
        .unwrap_or_else(|d| panic!("the run succeeds:\n{}", joined(&d.diagnostics)));
    assert_eq!(report.status, "ok");
    assert_eq!(report.outputs.len(), 2);

    // ComfyUI sends a job's frames only to the socket registered under its submit's client id when
    // they are emitted, so an empty `dropped` proves the socket opened before the first submit.
    assert!(
        mock.state.dropped.lock().unwrap().is_empty(),
        "frames found no socket: {:?}",
        mock.state.dropped.lock().unwrap()
    );
    assert!(
        mock.state
            .sockets
            .lock()
            .unwrap()
            .contains_key(&report.run_id),
        "the socket is registered under the run id"
    );
}

#[test]
fn the_follow_sweeps_history_when_no_frame_reaches_the_socket() {
    let mock = Mock::start(MockConfig {
        frames: false,
        history_after: 1,
        ..Default::default()
    });
    let project = project("history-sweep", "txt2img.json", TXT2IMG_RECIPE);
    let mut opts = options(&mock.url);
    opts.timeout = Duration::from_secs(10);
    let report = ludion_gen::run::run(&resolved(&project), &opts)
        .unwrap_or_else(|d| panic!("the sweep finds the jobs:\n{}", joined(&d.diagnostics)));
    assert_eq!(report.status, "ok");
    assert_eq!(report.outputs.len(), 2);
}

#[test]
fn follow_falls_back_to_polling_when_the_websocket_will_not_open() {
    let mock = Mock::start(MockConfig {
        websocket: false,
        history_after: 1,
        ..Default::default()
    });
    let project = project("polling", "txt2img.json", TXT2IMG_RECIPE);
    let report =
        ludion_gen::run::run(&resolved(&project), &options(&mock.url)).unwrap_or_else(|d| {
            panic!(
                "polling reaches the same answer:\n{}",
                joined(&d.diagnostics)
            )
        });
    assert_eq!(report.outputs.len(), 2);
    assert_eq!(report.status, "ok");

    // The same server, driven straight through the client, says which path it took.
    let client = Client::new(&mock.url);
    let graph = serde_json::from_slice::<Value>(&read_fixture("txt2img.json")).expect("the graph");
    let subscription = client.subscribe("probe");
    let prompt_id = client.submit(&graph, "probe").expect("the submit lands");
    let progress = client
        .follow(
            subscription,
            std::slice::from_ref(&prompt_id),
            Duration::from_secs(10),
        )
        .expect("the job finishes");
    assert_eq!(progress.via, FollowVia::Polling);
    assert_eq!(progress.finished, vec![prompt_id]);
}

#[test]
fn no_wait_leaves_a_submitted_run_that_fetch_completes() {
    let mock = Mock::start(MockConfig::default());
    let project = project("no-wait", "txt2img.json", TXT2IMG_RECIPE);
    let resolved = resolved(&project);

    let mut opts = options(&mock.url);
    opts.wait = false;
    let submitted = ludion_gen::run::run(&resolved, &opts).expect("the submit lands");
    assert_eq!(submitted.status, "submitted");
    assert_eq!(submitted.prompt_ids.len(), 2);
    assert!(submitted.outputs.is_empty());
    assert!(submitted.finished.is_empty());
    assert!(!submitted.dir.join("0000.png").exists());
    assert!(
        mock.state.sockets.lock().unwrap().is_empty(),
        "`--no-wait` opens no socket"
    );

    let status = ludion_gen::run::status(&submitted.dir, &mock.url).expect("the status reads");
    assert!(status.ready, "the mock finished both jobs immediately");
    assert!(status.pending.is_empty());
    assert_eq!(status.report.run_id, submitted.run_id);

    let fetched = ludion_gen::run::fetch(&submitted.dir, &mock.url).expect("the fetch completes");
    assert_eq!(fetched.status, "ok");
    assert_eq!(fetched.outputs.len(), 2);
    assert!(!fetched.finished.is_empty());
    assert!(submitted.dir.join("0000.png").is_file());

    let reread = ludion_gen::run::read_report(&submitted.dir).expect("run.json was rewritten");
    assert_eq!(reread.status, "ok");
    assert_eq!(reread.outputs.len(), 2);

    // A finalized run is final: after a restart every job would read lost, and neither verb asks.
    mock.restart();
    let after = ludion_gen::run::status(&submitted.dir, &mock.url).expect("the status reads");
    assert!(after.ready && after.pending.is_empty() && after.failed.is_empty());
    let again = ludion_gen::run::fetch(&submitted.dir, &mock.url).expect("the fetch is a no-op");
    assert_eq!(again.status, "ok");
    assert_eq!(again.outputs.len(), 2);
}

#[test]
fn status_and_fetch_report_a_failed_job_of_a_no_wait_run() {
    let mock = Mock::start(MockConfig {
        fail_jobs: &[0],
        ..Default::default()
    });
    let project = project("no-wait-failed", "txt2img.json", TXT2IMG_RECIPE);
    let mut opts = options(&mock.url);
    opts.wait = false;
    let submitted = ludion_gen::run::run(&resolved(&project), &opts).expect("the submit lands");

    let status = ludion_gen::run::status(&submitted.dir, &mock.url).expect("the status reads");
    assert!(status.ready, "a failed job has its verdict");
    assert!(status.pending.is_empty());
    assert_eq!(status.failed.len(), 1);
    assert_eq!(status.failed[0].job, 0);
    assert_eq!(status.failed[0].reason, "error");

    let fetched = ludion_gen::run::fetch(&submitted.dir, &mock.url).expect("the fetch completes");
    assert_eq!(fetched.status, "partial");
    assert_eq!(fetched.outputs.len(), 1);
    assert_eq!(
        (
            fetched.outputs[0].index,
            fetched.outputs[0].seed,
            fetched.outputs[0].prompt_id.as_str()
        ),
        (0, Some(8), "job-1")
    );
}

#[test]
fn after_a_restart_status_and_fetch_call_the_jobs_lost() {
    let mock = Mock::start(MockConfig {
        stall: true,
        ..Default::default()
    });
    let project = project("restarted", "txt2img.json", TXT2IMG_RECIPE);
    let mut opts = options(&mock.url);
    opts.wait = false;
    let submitted = ludion_gen::run::run(&resolved(&project), &opts).expect("the submit lands");

    let before = ludion_gen::run::status(&submitted.dir, &mock.url).expect("the status reads");
    assert!(!before.ready);
    assert_eq!(before.pending, vec!["job-0", "job-1"]);
    assert!(before.failed.is_empty());

    mock.restart();
    let after = ludion_gen::run::status(&submitted.dir, &mock.url).expect("the status reads");
    assert!(after.ready, "pending after a restart: {:?}", after.pending);
    assert_eq!(after.failed.len(), 2);
    assert!(after.failed.iter().all(|failure| failure.reason == "lost"));

    let fetched = ludion_gen::run::fetch(&submitted.dir, &mock.url).expect("lost is terminal");
    assert_eq!(fetched.status, "failed");
    assert!(fetched.outputs.is_empty());
    let text = joined(&fetched.failure_diagnostics());
    assert!(text.contains("lost"), "{text}");
    assert!(
        text.contains("gen run generation/recipes/") && text.contains(".gen.yaml"),
        "a lost job names the recipe to run again: {text}"
    );
    assert_eq!(
        codes(&fetched.failure_diagnostics()),
        vec!["L1169", "L1169"]
    );

    let again = ludion_gen::run::fetch(&submitted.dir, &mock.url).expect("a finalized run");
    assert_eq!(again, fetched, "a finalized run is returned unchanged");
}

#[test]
fn a_failing_job_is_chosen_by_its_submit_number_after_a_restart() {
    let mock = Mock::start(MockConfig {
        fail_jobs: &[2],
        ..Default::default()
    });
    let first = project("submit-number-a", "txt2img.json", TXT2IMG_RECIPE);
    let report = ludion_gen::run::run(&resolved(&first), &options(&mock.url)).expect("jobs 0, 1");
    assert_eq!(report.status, "ok");

    mock.restart();
    let second = project(
        "submit-number-b",
        "txt2img.json",
        &TXT2IMG_RECIPE.replace("count: 2", "count: 1"),
    );
    let report = ludion_gen::run::run(&resolved(&second), &options(&mock.url)).expect("job 2");
    assert_eq!(report.status, "failed");
    assert_eq!(report.failed[0].prompt_id, "job-2");
}

#[test]
fn a_waited_run_whose_jobs_vanish_ends_before_its_timeout() {
    let mock = Mock::start(MockConfig {
        forget: true,
        ..Default::default()
    });
    let project = project("vanished", "txt2img.json", TXT2IMG_RECIPE);
    let mut opts = options(&mock.url);
    opts.timeout = Duration::from_secs(3);
    let started = std::time::Instant::now();
    let report = ludion_gen::run::run(&resolved(&project), &opts)
        .unwrap_or_else(|d| panic!("lost jobs are an outcome:\n{}", joined(&d.diagnostics)));
    assert!(started.elapsed() < Duration::from_secs(3));
    assert_eq!(report.status, "failed");
    assert_eq!(report.failed.len(), 2);
    assert!(report.failed.iter().all(|failure| failure.reason == "lost"));
}

#[test]
fn the_url_resolves_from_the_flag_then_the_environment_then_comfyui_s_default() {
    assert_eq!(
        resolve_url(Some("http://example.invalid:8188/")).expect("a whole URL"),
        "http://example.invalid:8188",
        "the flag wins, and a trailing slash is trimmed"
    );
    assert_eq!(
        resolve_url(Some("example.invalid:8188"))
            .expect_err("an address is not a URL")
            .code
            .as_str(),
        "L1167"
    );
    let https = resolve_url(Some("https://example.invalid:8188")).expect_err("no TLS");
    assert_eq!(https.code.as_str(), "L1167");
    assert!(
        https.message.contains("TLS"),
        "the refusal says why: {}",
        https.message
    );
}

/// A reply the server promised and the connection dropped is not "nothing answered".
#[test]
fn a_reply_cut_off_mid_body_is_l1168() {
    let mock = Mock::start(MockConfig::default());
    let diagnostic = Client::new(&mock.url)
        .view("truncated.png", "", "output")
        .expect_err("the body is cut off");
    let text = format!(
        "{} {}",
        diagnostic.message,
        diagnostic.help.unwrap_or_default()
    );
    assert_eq!(diagnostic.code.as_str(), "L1168", "{text}");
    assert!(text.contains("/view"), "{text}");
    assert!(!text.contains("nothing answered"), "{text}");
}

/// One job that writes several files labels them all with its seed and tells them apart by
/// `batch_index`; a `PreviewImage`'s temp file is never a candidate.
#[test]
fn a_batch_shares_its_jobs_seed_and_a_preview_is_not_downloaded() {
    let mock = Mock::start(MockConfig {
        batch: 2,
        preview: true,
        ..Default::default()
    });
    let project = project("batch-preview", "txt2img.json", TXT2IMG_RECIPE);

    let report = ludion_gen::run::run(&resolved(&project), &options(&mock.url))
        .unwrap_or_else(|d| panic!("the run succeeds:\n{}", joined(&d.diagnostics)));

    assert_eq!(report.outputs.len(), 4);
    assert_eq!(
        report
            .outputs
            .iter()
            .map(|o| o.batch_index)
            .collect::<Vec<_>>(),
        vec![0, 1, 0, 1]
    );
    assert_eq!(
        report.outputs.iter().map(|o| o.seed).collect::<Vec<_>>(),
        vec![Some(7), Some(7), Some(8), Some(8)]
    );
    let mut files: Vec<String> = std::fs::read_dir(&report.dir)
        .expect("the run folder")
        .map(|entry| entry.unwrap().file_name().to_string_lossy().to_string())
        .collect();
    files.sort();
    assert_eq!(
        files,
        vec!["0000.png", "0001.png", "0002.png", "0003.png", "run.json"],
        "the preview's temp file was never downloaded"
    );
}

/// A recipe that pins the only seed input runs every job at the pin: the report says so, and the
/// offline lint warns that the extra candidates are identical.
#[test]
fn a_pinned_seed_is_recorded_and_linted() {
    let mock = Mock::start(MockConfig::default());
    let recipe = format!(
        "{}  Sampler:\n    seed: 5\n",
        TXT2IMG_RECIPE.replace("count: 2", "count: 3")
    );
    let project = project("pinned-seed", "txt2img.json", &recipe);
    let resolved = resolved(&project);

    let report = ludion_gen::run::run(&resolved, &options(&mock.url))
        .unwrap_or_else(|d| panic!("the run succeeds:\n{}", joined(&d.diagnostics)));

    assert_eq!(report.seeds, vec![Some(5); 3]);
    assert!(report.outputs.iter().all(|o| o.seed == Some(5)));

    let lint = ludion_gen::run::identical_candidates(&resolved, 3).expect("a pinned seed warns");
    assert_eq!(lint.code.as_str(), "L1266");
    assert!(!lint.severity.is_error());
    assert!(ludion_gen::run::identical_candidates(&resolved, 1).is_none());

    let varied = self::resolved(&self::project(
        "varied-seed",
        "txt2img.json",
        TXT2IMG_RECIPE,
    ));
    assert!(ludion_gen::run::identical_candidates(&varied, 2).is_none());
}

// --- The manual lane ------------------------------------------------------------------------

/// The live smoke: run it by hand against a real ComfyUI, never in CI (no GPU there, and a
/// generation run and `check.mjs` are serialized by hand on the reference box).
///
/// ```text
/// LUDION_COMFY_URL=http://<host>:8188 cargo test -p ludion-gen -- --ignored --nocapture
/// ```
#[test]
#[ignore = "needs a running ComfyUI server; LUDION_COMFY_URL selects it"]
fn live_smoke_against_a_real_server() {
    let Ok(url) = std::env::var("LUDION_COMFY_URL") else {
        panic!("set LUDION_COMFY_URL to the server to smoke against");
    };
    let url = resolve_url(Some(&url)).expect("a whole URL");
    let client = Client::new(&url);

    let stats = client.system_stats().expect("the server answers");
    assert!(
        !stats.comfy_version.is_empty(),
        "a real server reports its version"
    );

    let object_info = client.object_info().expect("the node schema");
    let classes = object_info.as_object().expect("an object of classes");
    assert!(
        classes.contains_key("KSampler") && classes.contains_key("SaveImage"),
        "a real ComfyUI ships both core classes"
    );

    let queue = client.queue().expect("the queue");
    assert!(queue.get("queue_pending").is_some());
    println!(
        "live: ComfyUI {} with {} node classes",
        stats.comfy_version,
        classes.len()
    );
}
