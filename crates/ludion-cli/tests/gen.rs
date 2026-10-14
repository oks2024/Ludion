//! Integration tests for `ludion gen`, driven against the built `ludion` binary — the
//! `sculpt.rs`/`voxel_cmd.rs` pattern.
//!
//! `check`'s offline half needs no server at all: `--url http://127.0.0.1:9` is an address nothing
//! answers on, so the live pass degrades to a skipped warning and the offline result alone decides
//! the exit code. `sheet` and `accept` read a finished run folder, so a hand-built one (a `run.json`
//! written straight through `ludion_gen::run::RunReport`, plus tiny candidate PNGs, or the committed
//! textured glTF for a mesh run) drives them. `nodes`, `run`, `status` and `fetch` run against the
//! mock ComfyUI server of `ludion-gen`'s tests, included by `#[path]`.
//!
//! The `--json` contract is one object per run, on success and on failure, `ok` true exactly when
//! the exit code is 0. Each verb's object is compared, as a type skeleton, with a hand-written
//! sample in the editor's `__fixtures__` folder, which the editor's own tests parse too.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use ludion_gen::run::{RunOutput, RunReport, STATUS_OK};
use serde_json::Value;

#[allow(dead_code)]
#[path = "../../ludion-gen/tests/common/mock_server.rs"]
mod mock_server;
use mock_server::{Mock, MockConfig};

fn ludion(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_ludion"))
        .args(args)
        .output()
        .expect("failed to run the ludion binary")
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

fn fixture(name: &str) -> String {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/gen")
        .join(name)
        .to_str()
        .unwrap()
        .to_string()
}

/// The unreachable address every test in this file points `--url` at: nothing on this machine
/// answers on port 9 (the historic `discard` service), so the live pass always degrades to a
/// skipped `L1167` warning.
const UNREACHABLE_URL: &str = "http://127.0.0.1:9";

/// A recipe whose offline pass succeeds (a real workflow, titles that resolve, one output kind)
/// reports `offline: ok`, `live: skipped`, and exits zero even though nothing answers `--url`.
#[test]
fn check_offline_ok_with_an_unreachable_server_exits_zero() {
    let recipe = fixture("idol.gen.yaml");
    let output = ludion(&["gen", "check", &recipe, "--url", UNREACHABLE_URL]);
    assert!(output.status.success(), "stderr: {}", stderr(&output));

    let text = stdout(&output);
    assert!(text.contains("kind: image"), "{text}");
    assert!(text.contains("overrides:"), "{text}");
    assert!(text.contains("Empty Latent:"), "{text}");
    assert!(text.contains("Positive Prompt:"), "{text}");
    assert!(text.contains("height: 512"), "{text}");

    let err = stderr(&output);
    assert!(err.contains("L1167"), "{err}");
    assert!(err.contains("warning"), "{err}");
}

/// A `--url` that is no URL is the live verdict (`skipped`), not a quiet swap for ComfyUI's default
/// address: asking `127.0.0.1:8188` about a server the user never named would report whatever
/// happens to listen there.
#[test]
fn check_reports_a_malformed_url_as_the_live_verdict() {
    let recipe = fixture("idol.gen.yaml");
    let output = ludion(&[
        "gen",
        "check",
        &recipe,
        "--url",
        "example.invalid:8188",
        "--json",
    ]);
    assert!(output.status.success(), "stderr: {}", stderr(&output));

    let value: serde_json::Value =
        serde_json::from_str(&stdout(&output)).expect("gen check --json prints one JSON object");
    assert_eq!(value["live"], "skipped");
    let messages: Vec<&str> = value["diagnostics"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|diagnostic| diagnostic["message"].as_str())
        .collect();
    assert!(
        messages
            .iter()
            .any(|message| message.contains("does not give an http://")),
        "{messages:?}"
    );
    for text in [stdout(&output), stderr(&output)] {
        assert!(!text.contains("127.0.0.1:8188"), "{text}");
    }
}

/// The text form's two-line summary says whether the live pass ran at all: `offline: ok`, then
/// `live: skipped (L1167)` when nothing answers `--url` — a live failure and an offline-only check
/// no longer read the same.
#[test]
fn check_text_prints_the_offline_and_live_summary_lines() {
    let recipe = fixture("idol.gen.yaml");
    let output = ludion(&["gen", "check", &recipe, "--url", UNREACHABLE_URL]);
    assert!(output.status.success(), "stderr: {}", stderr(&output));

    let text = stdout(&output);
    assert!(text.contains("offline: ok"), "{text}");
    assert!(text.contains("live: skipped (L1167)"), "{text}");
}

/// None of the top-level render knobs (`--threads`, `--quality`, `--render-size`) can do anything
/// under `gen` — no verb drives a sim or a render pass — so none of their help text should leak into
/// a `gen` subcommand's `--help`.
#[test]
fn gen_verb_help_never_advertises_the_render_knobs() {
    for verb in [
        "nodes", "check", "run", "status", "fetch", "sheet", "accept",
    ] {
        let output = ludion(&["gen", verb, "--help"]);
        assert!(output.status.success(), "{verb}: {}", stderr(&output));
        let text = stdout(&output);
        for flag in ["--threads", "--quality", "--render-size"] {
            assert!(
                !text.contains(flag),
                "`gen {verb} --help` still shows `{flag}`:\n{text}"
            );
        }
    }
}

/// A render knob given to `gen` is refused by name, never swallowed and never a clap panic (the
/// hidden shadow's type once disagreed with the global's), and a malformed knob in the environment
/// blocks no verb that reads none of them.
#[test]
fn a_render_knob_on_gen_is_refused_and_an_env_knob_is_ignored() {
    for args in [
        ["--threads", "4", "gen", "nodes"],
        ["--quality", "low", "gen", "nodes"],
        ["--render-size", "64x64", "gen", "nodes"],
    ] {
        let output = ludion(&args);
        let text = stderr(&output);
        assert!(!output.status.success(), "{args:?} must refuse");
        assert!(text.contains("does not apply"), "{args:?}: {text}");
        assert!(!text.contains("panicked"), "{args:?}: {text}");
    }
    let output = ludion(&["gen", "--threads", "abc", "nodes"]);
    assert!(!output.status.success());
    assert!(!stderr(&output).contains("panicked"), "{}", stderr(&output));

    let output = Command::new(env!("CARGO_BIN_EXE_ludion"))
        .args(["gen", "nodes", "--url", "http://127.0.0.1:1"])
        .env("LUDION_THREADS", "abc")
        .env("LUDION_QUALITY", "nonsense")
        .env("LUDION_RENDER_SIZE", "nonsense")
        .output()
        .unwrap();
    let text = stderr(&output);
    for blocked in ["L0014", "L0016", "L0017"] {
        assert!(!text.contains(blocked), "{blocked} blocked gen: {text}");
    }
}

/// `gen sheet` and `gen accept` never contact a server, so `--url` is an unknown argument there.
#[test]
fn the_offline_verbs_take_no_url() {
    for verb in ["sheet", "accept"] {
        let output = ludion(&["gen", verb, "--url", "http://127.0.0.1:1", "dir"]);
        assert!(!output.status.success());
        assert!(
            stderr(&output).contains("unexpected argument '--url'"),
            "{verb}: {}",
            stderr(&output)
        );
    }
}

/// `--json` on the same recipe prints exactly one JSON object with the checkpoint-1 shape: offline
/// result, live result, the produced kind, and the resolved override table — the schema decision 15
/// rests on.
#[test]
fn check_json_prints_one_object_with_the_documented_shape() {
    let recipe = fixture("idol.gen.yaml");
    let output = ludion(&["gen", "check", &recipe, "--url", UNREACHABLE_URL, "--json"]);
    assert!(output.status.success(), "stderr: {}", stderr(&output));

    let value: serde_json::Value =
        serde_json::from_str(&stdout(&output)).expect("gen check --json prints one JSON object");

    assert!(value["recipe"].as_str().unwrap().ends_with("idol.gen.yaml"));
    assert!(
        value["workflow"]
            .as_str()
            .unwrap()
            .ends_with("txt2img.json")
    );
    assert_eq!(value["kind"], "image");
    assert_eq!(value["offline"], "ok");
    assert_eq!(value["live"], "skipped");
    assert_eq!(
        value["overrides"]["Empty Latent"]["height"],
        serde_json::json!(512)
    );
    assert_eq!(
        value["overrides"]["Positive Prompt"]["text"],
        "a small weathered stone idol"
    );
    let diagnostics = value["diagnostics"]
        .as_array()
        .expect("diagnostics is an array");
    assert!(
        diagnostics.iter().any(|d| d["code"] == "L1167"),
        "{diagnostics:?}"
    );
}

/// A recipe naming a workflow that does not exist fails the offline pass (`L1162`, the path does
/// not resolve) before any server is contacted at all, and never prints an override table.
#[test]
fn check_reports_the_offline_error_and_never_reaches_the_network() {
    // No `--url` at all: if this reached the network it would hang or fail against whatever is (or
    // is not) listening on the machine's default `LUDION_COMFY_URL`. It must not get that far.
    let output = ludion(&["gen", "check", "definitely-not-a-real-recipe.gen.yaml"]);
    assert!(!output.status.success());
    let err = stderr(&output);
    assert!(err.contains("error"), "{err}");
    assert!(stdout(&output).is_empty(), "{}", stdout(&output));
}

/// A recipe with a committed `images:` entry bound into an override (`Load Subject: {image: {image:
/// subject}}`) offline-checks `ok`: the placeholder upload map lets `workflow::apply` resolve the
/// `ImageRef` without a real upload, so title resolution is exercised without a false
/// "never uploaded" failure.
#[test]
fn check_offline_ok_for_a_recipe_with_an_image_bound_override() {
    let recipe = fixture("idol-variation.gen.yaml");
    let output = ludion(&["gen", "check", &recipe, "--url", UNREACHABLE_URL, "--json"]);
    assert!(output.status.success(), "stderr: {}", stderr(&output));

    let value: serde_json::Value =
        serde_json::from_str(&stdout(&output)).expect("gen check --json prints one JSON object");
    assert_eq!(value["offline"], "ok", "{value:#}");
    assert_eq!(value["kind"], "image");
    assert_eq!(
        value["overrides"]["Load Subject"]["image"],
        serde_json::json!({ "image": "subject" })
    );
}

/// The text override table prints an image-bound override as one readable value naming the file it
/// resolves to — never the document's nested spelling, which reached the terminal as
/// `image: image: subject`.
#[test]
fn check_text_prints_an_image_bound_override_as_one_value() {
    let recipe = fixture("idol-variation.gen.yaml");
    let output = ludion(&["gen", "check", &recipe, "--url", UNREACHABLE_URL]);
    assert!(output.status.success(), "stderr: {}", stderr(&output));

    let text = stdout(&output);
    assert!(
        text.contains("    image: images.subject -> subject.png"),
        "{text}"
    );
    assert!(!text.contains("image: image:"), "{text}");
}

/// A frozen copy of vigil's `idol-variation.gen.yaml` (the 84 B `idol-reference.png` is a
/// placeholder; only `is_file()` is checked) — its `images: subject` entry names the accepted
/// generation reference, so the offline pass reads it, resolves every title, and reports the
/// graph's output kind; only the live pass is affected by `--url` pointing nowhere.
#[test]
fn check_offline_ok_for_the_real_idol_variation_recipe() {
    let project = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/idol-generation")
        .canonicalize()
        .expect("the frozen idol generation project");
    let recipe = project.join("generation/recipes/idol-variation.gen.yaml");
    assert!(recipe.is_file(), "{}", recipe.display());

    let output = ludion(&[
        "gen",
        "check",
        recipe.to_str().unwrap(),
        "--url",
        UNREACHABLE_URL,
        "--json",
    ]);
    assert!(output.status.success(), "stderr: {}", stderr(&output));

    let value: serde_json::Value =
        serde_json::from_str(&stdout(&output)).expect("gen check --json prints one JSON object");
    assert_eq!(value["offline"], "ok", "{value:#}");
    assert_eq!(value["live"], "skipped", "{value:#}");
    assert_eq!(value["kind"], "image", "{value:#}");
}

/// An `images:` entry naming a file that is not on disk fails the offline pass with exactly one
/// `L1162` at `/images/<name>` naming the missing path, before title or output-kind checking ever
/// runs — a self-contained recipe under a scratch project root, so the assertion never depends on
/// what is or is not committed elsewhere.
#[test]
fn check_reports_l1162_when_an_images_entry_names_a_missing_file() {
    let root = std::env::temp_dir().join(format!(
        "ludion-cli-gen-missing-image-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("generation/workflows")).expect("the workflow folder");
    std::fs::create_dir_all(root.join("generation/recipes")).expect("the recipe folder");
    std::fs::write(root.join("generation/workflows/img2img.json"), RUN_WORKFLOW)
        .expect("the workflow");
    std::fs::write(
        root.join("generation/recipes/variation.gen.yaml"),
        "format: 1\n\
         kind: generation\n\
         name: variation\n\
         workflow: \"../workflows/img2img.json\"\n\
         seed: 3\n\
         count: 1\n\
         images:\n\
         \x20\x20subject: \"reference.png\"\n\
         inputs:\n\
         \x20\x20Positive Prompt:\n\
         \x20\x20\x20\x20text: \"a weathered stone idol, variation\"\n",
    )
    .expect("the recipe");

    let recipe = root.join("generation/recipes/variation.gen.yaml");
    let output = ludion(&[
        "gen",
        "check",
        recipe.to_str().unwrap(),
        "--url",
        UNREACHABLE_URL,
        "--json",
    ]);
    assert!(
        !output.status.success(),
        "a missing images: file must fail the offline pass"
    );

    let value: serde_json::Value =
        serde_json::from_str(&stdout(&output)).expect("gen check --json prints one JSON object");
    assert_eq!(value["offline"], "error", "{value:#}");
    assert_eq!(value["live"], "skipped", "{value:#}");
    let diagnostics = value["diagnostics"].as_array().expect("an array");
    assert_eq!(diagnostics.len(), 1, "{diagnostics:#?}");
    assert_eq!(diagnostics[0]["code"], "L1162");
    assert_eq!(diagnostics[0]["path"], "/images/subject");
    assert!(
        diagnostics[0]["message"]
            .as_str()
            .unwrap()
            .contains("reference.png"),
        "{diagnostics:#?}"
    );
}

/// A recipe that pins the only seed input while asking for several candidates would submit the same
/// graph every time: `gen check` warns `L1266` offline, and the offline pass stays `ok`.
#[test]
fn check_warns_l1266_when_a_pin_leaves_no_seed_to_vary() {
    let stock = ludion(&[
        "gen",
        "check",
        &fixture("idol.gen.yaml"),
        "--url",
        UNREACHABLE_URL,
        "--json",
    ]);
    let stock: serde_json::Value = serde_json::from_str(&stdout(&stock)).expect("one JSON object");
    assert!(
        !stock["diagnostics"].to_string().contains("L1266"),
        "{stock:#}"
    );

    let root =
        std::env::temp_dir().join(format!("ludion-cli-gen-identical-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("generation/workflows")).expect("the workflow folder");
    std::fs::create_dir_all(root.join("generation/recipes")).expect("the recipe folder");
    std::fs::copy(
        fixture("txt2img.json"),
        root.join("generation/workflows/txt2img.json"),
    )
    .expect("the workflow");
    let recipe = root.join("generation/recipes/pinned.gen.yaml");
    std::fs::write(
        &recipe,
        "format: 1\n\
         kind: generation\n\
         name: pinned\n\
         workflow: \"../workflows/txt2img.json\"\n\
         count: 2\n\
         inputs:\n\
         \x20\x20Sampler:\n\
         \x20\x20\x20\x20seed: 5\n",
    )
    .expect("the recipe");

    let output = ludion(&[
        "gen",
        "check",
        recipe.to_str().unwrap(),
        "--url",
        UNREACHABLE_URL,
        "--json",
    ]);
    assert!(output.status.success(), "stderr: {}", stderr(&output));
    let value: serde_json::Value = serde_json::from_str(&stdout(&output)).expect("one JSON object");
    assert_eq!(value["offline"], "ok", "{value:#}");
    let found: Vec<&serde_json::Value> = value["diagnostics"]
        .as_array()
        .expect("an array")
        .iter()
        .filter(|d| d["code"] == "L1266")
        .collect();
    assert_eq!(found.len(), 1, "{value:#}");
    assert_eq!(found[0]["severity"], "warning");
}

// --- sheet / accept, over a hand-built run folder ------------------------------------------------

/// The workflow and recipe every hand-built run folder's `run.json` names — the `ludion-gen`
/// `accept.rs` test fixture, reused here so `provenance()`'s recipe re-resolution (which `gen
/// accept` exercises through the real binary) has something real to read.
const RUN_WORKFLOW: &str = r#"{
  "1": {"class_type": "CheckpointLoaderSimple", "inputs": {"ckpt_name": "flux2-klein-4b.safetensors"}, "_meta": {"title": "Checkpoint"}},
  "2": {"class_type": "CLIPTextEncode", "inputs": {"text": "a placeholder", "clip": ["1", 1]}, "_meta": {"title": "Positive Prompt"}},
  "3": {"class_type": "SaveImage", "inputs": {"images": ["2", 0]}, "_meta": {"title": "Save"}}
}"#;

/// The mesh twin of [`RUN_WORKFLOW`]: a `SaveGLB` graph, which is what makes a hand-built run's
/// `kind: mesh` report agree with the recipe `gen accept` re-resolves for its provenance.
const RUN_MESH_WORKFLOW: &str = r#"{
  "1": {"class_type": "LoadImage", "inputs": {"image": "subject.png"}, "_meta": {"title": "Load Subject"}},
  "2": {"class_type": "SaveGLB", "inputs": {"mesh": ["1", 0]}, "_meta": {"title": "Save Mesh"}}
}"#;

const RUN_RECIPE: &str = "\
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

const RUN_MESH_RECIPE: &str = "\
format: 1
kind: generation
name: idol
workflow: \"../workflows/img2mesh.json\"
seed: 7
count: 1
";

/// A throwaway project (`generation/{recipes,workflows,runs}/`) holding one finalized run with
/// `candidates` downloaded outputs, ready for `gen sheet`/`gen accept` to read.
struct RunFixture {
    root: PathBuf,
    run_dir: PathBuf,
}

fn build_run(name: &str, candidates: u32) -> RunFixture {
    build_run_of(name, "image", candidates)
}

/// A mesh run: the same folder shape, a `SaveGLB` recipe, and `.glb` candidates the accept hands to
/// the importer.
fn build_mesh_run(name: &str, candidates: u32) -> RunFixture {
    build_run_of(name, "mesh", candidates)
}

/// The committed textured glTF (`examples/import-textured/`, five maps on two cubes) as a mesh
/// candidate. Its buffer and PNG maps are data URIs, so the bytes are self-contained wherever the
/// run folder sits; the importer reads the container from the bytes, and the binary `.glb` form is
/// `ludion-gen`'s own accept test.
fn mesh_candidate_bytes() -> Vec<u8> {
    std::fs::read(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/import-textured/scene.gltf"),
    )
    .expect("the committed textured glTF fixture")
}

fn build_run_of(name: &str, kind: &str, candidates: u32) -> RunFixture {
    let root = std::env::temp_dir().join(format!("ludion-cli-gen-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("generation/workflows")).expect("the workflow folder");
    std::fs::create_dir_all(root.join("generation/recipes")).expect("the recipe folder");
    let mesh = kind == "mesh";
    let workflow = if mesh {
        "generation/workflows/img2mesh.json"
    } else {
        "generation/workflows/txt2img.json"
    };
    std::fs::write(
        root.join(workflow),
        if mesh {
            RUN_MESH_WORKFLOW
        } else {
            RUN_WORKFLOW
        },
    )
    .expect("the workflow");
    std::fs::write(
        root.join("generation/recipes/idol.gen.yaml"),
        if mesh { RUN_MESH_RECIPE } else { RUN_RECIPE },
    )
    .expect("the recipe");

    let run_id = "20260918T101112Z-1a2b3c4d";
    let run_dir = root.join("generation/runs/idol").join(run_id);
    std::fs::create_dir_all(&run_dir).expect("the run folder");

    let outputs: Vec<RunOutput> = (0..candidates)
        .map(|index| RunOutput {
            index,
            seed: Some(7 + u64::from(index)),
            file: format!("{index:04}.{}", if mesh { "glb" } else { "png" }),
            node: if mesh { "2" } else { "3" }.to_string(),
            batch_index: 0,
            prompt_id: format!("job-{index}"),
        })
        .collect();
    for output in &outputs {
        let bytes = if mesh {
            mesh_candidate_bytes()
        } else {
            let pixels: Vec<u8> = (0..4 * 4 * 4).map(|byte| byte as u8).collect();
            ludion_assets::encode_png(4, 4, &pixels).expect("the candidate encodes")
        };
        std::fs::write(run_dir.join(&output.file), bytes).expect("the candidate");
    }

    let report = RunReport {
        run_id: run_id.to_string(),
        recipe: "generation/recipes/idol.gen.yaml".to_string(),
        recipe_hash: "0000000012345678".to_string(),
        workflow: workflow.to_string(),
        workflow_hash: "00000000deadbeef".to_string(),
        graph_hash: "1a2b3c4d5e6f7080".to_string(),
        seed_base: 7,
        count: candidates,
        kind: kind.to_string(),
        comfy_version: "0.3.99".to_string(),
        checkpoints: vec!["flux2-klein-4b.safetensors".to_string()],
        images: Vec::new(),
        prompt: Some(if mesh {
            String::new()
        } else {
            "a weathered stone idol on a low plinth".to_string()
        }),
        inputs: Some(if mesh {
            serde_json::json!({})
        } else {
            serde_json::json!({"Positive Prompt": {"text": "a weathered stone idol on a low plinth"}})
        }),
        prompt_ids: outputs.iter().map(|o| o.prompt_id.clone()).collect(),
        seeds: outputs.iter().map(|o| o.seed).collect(),
        outputs,
        failed: Vec::new(),
        started: "2026-09-18T10:11:12Z".to_string(),
        finished: "2026-09-18T10:11:40Z".to_string(),
        duration_ms: 28_000,
        status: STATUS_OK.to_string(),
        dir: run_dir.clone(),
    };
    let mut json = serde_json::to_string_pretty(&report).expect("the report serializes");
    json.push('\n');
    std::fs::write(run_dir.join("run.json"), json).expect("run.json");

    RunFixture { root, run_dir }
}

/// `gen sheet` composes a PNG in the run folder and reports the candidate count it read from
/// `run.json`.
#[test]
fn sheet_composes_a_png_and_reports_the_cell_count() {
    let fixture = build_run("sheet", 3);
    let output = ludion(&["gen", "sheet", fixture.run_dir.to_str().unwrap()]);
    assert!(output.status.success(), "stderr: {}", stderr(&output));

    let sheet_path = fixture.run_dir.join("sheet.png");
    assert!(sheet_path.is_file(), "{}", stdout(&output));
    assert!(stdout(&output).contains("3 cells"), "{}", stdout(&output));
}

/// `gen sheet --json` reports the run id, the sheet path and the cell count.
#[test]
fn sheet_json_reports_run_id_sheet_and_cells() {
    let fixture = build_run("sheet-json", 2);
    let output = ludion(&["gen", "sheet", fixture.run_dir.to_str().unwrap(), "--json"]);
    assert!(output.status.success(), "stderr: {}", stderr(&output));

    let value: serde_json::Value =
        serde_json::from_str(&stdout(&output)).expect("gen sheet --json prints one JSON object");
    assert_eq!(value["run_id"], "20260918T101112Z-1a2b3c4d");
    assert_eq!(value["cells"], 2);
    assert!(
        value["sheet"].as_str().unwrap().ends_with("sheet.png"),
        "{value:#}"
    );
}

/// `gen sheet`'s run folder is `std::env::temp_dir()` joined against a literal `"generation/runs/…"`
/// component, which is backslash-native on Windows joined with forward-slash text — the exact mix
/// that made a printed path unreadable. Both the text and `--json` forms print forward slashes only.
#[test]
fn sheet_prints_forward_slash_paths_only() {
    let fixture = build_run("sheet-slashes", 2);

    let text_output = ludion(&["gen", "sheet", fixture.run_dir.to_str().unwrap()]);
    assert!(
        text_output.status.success(),
        "stderr: {}",
        stderr(&text_output)
    );
    let text = stdout(&text_output);
    assert!(!text.contains('\\'), "{text}");

    let out = fixture.root.join("sheet-custom.png");
    let json_output = ludion(&[
        "gen",
        "sheet",
        fixture.run_dir.to_str().unwrap(),
        "--out",
        out.to_str().unwrap(),
        "--json",
    ]);
    assert!(
        json_output.status.success(),
        "stderr: {}",
        stderr(&json_output)
    );
    let value: serde_json::Value = serde_json::from_str(&stdout(&json_output))
        .expect("gen sheet --json prints one JSON object");
    let sheet = value["sheet"].as_str().unwrap();
    assert!(!sheet.contains('\\'), "{sheet}");
}

/// `gen accept` writes the re-encoded PNG and its `.origin.yaml` sidecar, then refuses a second
/// accept to the same destination with `L1172`, leaving the first accept's files untouched.
#[test]
fn accept_writes_the_asset_and_sidecar_then_refuses_a_second_accept() {
    let fixture = build_run("accept", 2);
    let out = fixture.root.join("assets/idol.png");
    let sidecar = fixture.root.join("assets/idol.png.origin.yaml");

    let output = ludion(&[
        "gen",
        "accept",
        fixture.run_dir.to_str().unwrap(),
        "0",
        "--out",
        out.to_str().unwrap(),
    ]);
    assert!(output.status.success(), "stderr: {}", stderr(&output));
    assert!(out.is_file(), "the asset was written");
    assert!(sidecar.is_file(), "the sidecar was written");

    let sidecar_text = std::fs::read_to_string(&sidecar).expect("the sidecar reads");
    assert!(sidecar_text.contains("kind: origin"), "{sidecar_text}");
    assert!(sidecar_text.contains("seed: 7"), "{sidecar_text}");
    let asset_bytes_after_first = std::fs::read(&out).expect("the asset reads");

    let second = ludion(&[
        "gen",
        "accept",
        fixture.run_dir.to_str().unwrap(),
        "1",
        "--out",
        out.to_str().unwrap(),
    ]);
    assert!(!second.status.success(), "a second accept must be refused");
    assert!(stderr(&second).contains("L1172"), "{}", stderr(&second));
    assert_eq!(
        std::fs::read(&out).expect("the asset still reads"),
        asset_bytes_after_first,
        "the refused accept must not touch the first candidate's file"
    );
}

/// A `partial` run (two candidates, a third job failed) composes a sheet and accepts a candidate
/// like an `ok` one: both exit 0.
#[test]
fn a_partial_run_sheets_and_accepts() {
    let fixture = build_run("partial", 2);
    let path = fixture.run_dir.join("run.json");
    let mut report: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&path).expect("run.json")).expect("JSON");
    report["status"] = "partial".into();
    report["count"] = 3.into();
    report["prompt_ids"] = serde_json::json!(["job-0", "job-1", "job-2"]);
    report["failed"] = serde_json::json!([{
        "job": 2,
        "prompt_id": "job-2",
        "reason": "error",
        "node": "3",
        "class": "SaveImage",
        "message": "CUDA out of memory"
    }]);
    std::fs::write(&path, report.to_string()).expect("run.json is rewritten");

    let sheet = ludion(&["gen", "sheet", fixture.run_dir.to_str().unwrap()]);
    assert!(sheet.status.success(), "stderr: {}", stderr(&sheet));
    let out = fixture.root.join("assets/idol.png");
    let accept = ludion(&[
        "gen",
        "accept",
        fixture.run_dir.to_str().unwrap(),
        "1",
        "--out",
        out.to_str().unwrap(),
    ]);
    assert!(accept.status.success(), "stderr: {}", stderr(&accept));
    assert!(out.is_file(), "the asset was written");
}

/// `gen accept --json` reports the source, destination, sidecar and the origin block in its own
/// emit key order.
#[test]
fn accept_json_reports_source_destination_sidecar_and_origin() {
    let fixture = build_run("accept-json", 1);
    let out = fixture.root.join("assets/idol.png");

    let output = ludion(&[
        "gen",
        "accept",
        fixture.run_dir.to_str().unwrap(),
        "0",
        "--out",
        out.to_str().unwrap(),
        "--json",
    ]);
    assert!(output.status.success(), "stderr: {}", stderr(&output));

    let value: serde_json::Value =
        serde_json::from_str(&stdout(&output)).expect("gen accept --json prints one JSON object");
    assert!(
        value["destination"].as_str().unwrap().ends_with("idol.png"),
        "{value:#}"
    );
    assert!(
        value["sidecar"]
            .as_str()
            .unwrap()
            .ends_with("idol.png.origin.yaml"),
        "{value:#}"
    );
    assert_eq!(value["origin"]["tool"], "comfyui");
    assert_eq!(value["origin"]["seed"], 7);
    assert_eq!(
        value["origin"]["prompt"],
        "a weathered stone idol on a low plinth"
    );
    let keys: Vec<&String> = value["origin"].as_object().unwrap().keys().collect();
    assert_eq!(
        keys,
        vec![
            "tool",
            "comfy_version",
            "recipe",
            "recipe_hash",
            "run_id",
            "date",
            "seed",
            "workflow",
            "workflow_hash",
            "checkpoints",
            "images",
            "prompt",
            "inputs",
        ],
        "the origin object's key order is the struct's emit order"
    );
}

/// `gen accept`'s `source` (built from the backslash-native run folder) and its `destination`/
/// `sidecar` (whatever separators `--out` was typed with) print forward slashes only, in both the
/// text and `--json` forms.
#[test]
fn accept_prints_forward_slash_paths_only() {
    let fixture = build_run("accept-slashes", 1);
    let out = fixture.root.join("assets/idol.png");

    let text_output = ludion(&[
        "gen",
        "accept",
        fixture.run_dir.to_str().unwrap(),
        "0",
        "--out",
        out.to_str().unwrap(),
    ]);
    assert!(
        text_output.status.success(),
        "stderr: {}",
        stderr(&text_output)
    );
    let text = stdout(&text_output);
    assert!(!text.contains('\\'), "{text}");

    let out_json = fixture.root.join("assets/idol-json.png");
    let json_output = ludion(&[
        "gen",
        "accept",
        fixture.run_dir.to_str().unwrap(),
        "0",
        "--out",
        out_json.to_str().unwrap(),
        "--json",
    ]);
    assert!(
        json_output.status.success(),
        "stderr: {}",
        stderr(&json_output)
    );
    let value: serde_json::Value = serde_json::from_str(&stdout(&json_output))
        .expect("gen accept --json prints one JSON object");
    for key in ["source", "destination", "sidecar"] {
        let path = value[key].as_str().unwrap();
        assert!(!path.contains('\\'), "{key}: {path}");
    }
}

/// A mesh candidate goes through the importer in-process: `--out` is the directory the tree lands
/// in, every mesh document carries the provenance block right after its `name:`, and the text form
/// reports each document with its vertex and triangle counts.
#[test]
fn accept_imports_a_mesh_candidate_into_the_out_dir() {
    let fixture = build_mesh_run("accept-mesh", 1);
    let out = fixture.root.join("assets/idol");

    let output = ludion(&[
        "gen",
        "accept",
        fixture.run_dir.to_str().unwrap(),
        "0",
        "--out",
        out.to_str().unwrap(),
    ]);
    assert!(output.status.success(), "stderr: {}", stderr(&output));

    let text = stdout(&output);
    assert!(text.contains("vertices: 24"), "{text}");
    assert!(text.contains("triangles: 12"), "{text}");
    assert!(!text.contains('\\'), "{text}");
    assert!(out.join("model.prefab.yaml").is_file(), "{text}");

    let document = out.join("meshes/tangent-cube.mesh.yaml");
    let mesh_text = std::fs::read_to_string(&document).expect("the mesh document is on disk");
    let name_at = mesh_text.find("name:").expect("the document is named");
    let origin_at = mesh_text
        .find("origin:")
        .expect("the provenance block is written");
    assert!(name_at < origin_at, "{mesh_text}");
    assert!(
        mesh_text.contains("run_id: \"20260918T101112Z-1a2b3c4d\""),
        "{mesh_text}"
    );
    assert!(mesh_text.contains("base_color_texture:"), "{mesh_text}");

    let textures = std::fs::read_dir(out.join("textures")).expect("the textures folder");
    assert!(textures.count() > 0, "the five maps are written");

    // A second accept into the same tree is refused: an imported mesh is content from now on.
    let second = ludion(&[
        "gen",
        "accept",
        fixture.run_dir.to_str().unwrap(),
        "0",
        "--out",
        out.to_str().unwrap(),
    ]);
    assert!(!second.status.success(), "{}", stdout(&second));
    assert!(stderr(&second).contains("L1172"), "{}", stderr(&second));
}

/// `gen accept --json` on a mesh candidate keeps the image object's four keys and adds the two a
/// mesh import produces: the documents with their counts, and the texture files beside them.
#[test]
fn accept_json_reports_the_mesh_documents_and_their_counts() {
    let fixture = build_mesh_run("accept-mesh-json", 1);
    let out = fixture.root.join("assets/idol");

    let output = ludion(&[
        "gen",
        "accept",
        fixture.run_dir.to_str().unwrap(),
        "0",
        "--out",
        out.to_str().unwrap(),
        "--json",
    ]);
    assert!(output.status.success(), "stderr: {}", stderr(&output));

    let value: serde_json::Value =
        serde_json::from_str(&stdout(&output)).expect("gen accept --json prints one JSON object");
    assert!(
        value["source"].as_str().unwrap().ends_with("0000.glb"),
        "{value:#}"
    );
    assert!(
        value["destination"]
            .as_str()
            .unwrap()
            .ends_with("assets/idol"),
        "{value:#}"
    );
    assert_eq!(
        value["sidecar"],
        serde_json::Value::Null,
        "a mesh document holds its own provenance"
    );
    assert_eq!(value["origin"]["seed"], 7);

    let meshes = value["meshes"].as_array().expect("the documents written");
    assert_eq!(meshes.len(), 2, "{value:#}");
    for mesh in meshes {
        let path = mesh["path"].as_str().unwrap();
        assert!(
            path.ends_with(".mesh.yaml") && !path.contains('\\'),
            "{path}"
        );
        assert_eq!(mesh["vertices"], 24);
        assert_eq!(mesh["triangles"], 12);
    }
    let textures = value["textures"].as_array().expect("the textures written");
    assert!(!textures.is_empty(), "{value:#}");
    for texture in textures {
        let path = texture.as_str().unwrap();
        assert!(path.ends_with(".png") && !path.contains('\\'), "{path}");
    }
}

// --- the --json contract ------------------------------------------------------------------------

/// The one object `--json` printed, with `ok` agreeing with the exit code.
fn one_object(output: &Output) -> Value {
    let value: Value = serde_json::from_str(&stdout(output)).unwrap_or_else(|e| {
        panic!("stdout is not one JSON object ({e}):\n{}", stdout(output));
    });
    assert!(value.is_object(), "{value:#}");
    assert_eq!(
        value["ok"],
        Value::Bool(output.status.success()),
        "ok is true exactly when the exit code is 0 (stderr: {})",
        stderr(output)
    );
    assert!(value["diagnostics"].is_array(), "{value:#}");
    value
}

/// The JSON type skeleton: scalars become type names; arrays and objects keep their structure.
/// Compared serialized, so key order counts.
fn shape(v: &Value) -> Value {
    match v {
        Value::Null => "null".into(),
        Value::Bool(_) => "bool".into(),
        Value::Number(_) => "number".into(),
        Value::String(_) => "string".into(),
        Value::Array(a) => a.iter().map(shape).collect(),
        Value::Object(o) => o.iter().map(|(k, v)| (k.clone(), shape(v))).collect(),
    }
}

/// A diagnostic's skeleton is its `code` and `message`; `severity`, `path` and `help` are optional
/// (ludion-core's `Diagnostic`), not the envelope's to pin.
fn core_diagnostics(v: &Value) -> Value {
    let mut v = v.clone();
    if let Some(list) = v["diagnostics"].as_array_mut() {
        for d in list {
            *d = serde_json::json!({ "code": d["code"], "message": d["message"] });
        }
    }
    v
}

fn sample(name: &str) -> Value {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../editor/app/src/editor/generation/__fixtures__")
        .join(name);
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{name}: {e}"));
    serde_json::from_str(&text).unwrap_or_else(|e| panic!("{name}: {e}"))
}

fn matches_sample(value: &Value, name: &str) {
    let sample = sample(name);
    let pretty = |v: &Value| serde_json::to_string_pretty(&shape(&core_diagnostics(v))).unwrap();
    assert_eq!(
        pretty(value),
        pretty(&sample),
        "update the sample and model.ts together: {name}\ncli:\n{value:#}"
    );
}

fn codes_of(value: &Value) -> Vec<String> {
    value["diagnostics"]
        .as_array()
        .unwrap()
        .iter()
        .map(|d| d["code"].as_str().unwrap().to_string())
        .collect()
}

/// A project whose `idol` recipe (two candidates) runs against the mock server.
fn mock_project(name: &str) -> PathBuf {
    let root =
        std::env::temp_dir().join(format!("ludion-cli-gen-mock-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("generation/workflows")).expect("the workflow folder");
    std::fs::create_dir_all(root.join("generation/recipes")).expect("the recipe folder");
    std::fs::copy(
        fixture("txt2img.json"),
        root.join("generation/workflows/txt2img.json"),
    )
    .expect("the workflow");
    std::fs::write(
        root.join("generation/recipes/idol.gen.yaml"),
        "format: 1\nkind: generation\nname: idol\nworkflow: \"../workflows/txt2img.json\"\nseed: 7\ncount: 2\n\
         inputs:\n  Positive Prompt:\n    text: \"a small weathered stone idol\"\n",
    )
    .expect("the recipe");
    root
}

/// [`mock_project`] plus a declared reference image: the workflow gains an unlinked `LoadImage`
/// titled `Load Subject`, and the recipe binds `subject` to it. The mock never runs a graph.
fn mock_image_project(name: &str) -> PathBuf {
    let root = mock_project(name);
    std::fs::create_dir_all(root.join("generation/references")).expect("the references folder");
    std::fs::copy(
        fixture("subject.png"),
        root.join("generation/references/subject.png"),
    )
    .expect("the reference image");

    let workflow = root.join("generation/workflows/txt2img.json");
    let mut graph: Value =
        serde_json::from_str(&std::fs::read_to_string(&workflow).unwrap()).unwrap();
    graph["20"] = serde_json::json!({
        "class_type": "LoadImage",
        "inputs": { "image": "example.png" },
        "_meta": { "title": "Load Subject" }
    });
    std::fs::write(&workflow, serde_json::to_string_pretty(&graph).unwrap()).unwrap();

    std::fs::write(
        root.join("generation/recipes/idol.gen.yaml"),
        "format: 1\nkind: generation\nname: idol\nworkflow: \"../workflows/txt2img.json\"\nseed: 7\ncount: 2\n\
         images:\n  subject: \"../references/subject.png\"\n\
         inputs:\n  Positive Prompt:\n    text: \"a small weathered stone idol\"\n  \
         Load Subject:\n    image:\n      image: subject\n",
    )
    .expect("the recipe");
    root
}

/// Runs `ludion` with `cwd` as its working directory (always a scratch project).
fn ludion_in(cwd: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_ludion"))
        .args(args)
        .current_dir(cwd)
        .output()
        .expect("failed to run the ludion binary")
}

fn recipe_of(root: &Path) -> String {
    root.join("generation/recipes/idol.gen.yaml")
        .to_str()
        .unwrap()
        .to_string()
}

fn dir_of(value: &Value) -> String {
    value["dir"].as_str().expect("the run folder").to_string()
}

#[test]
fn run_json_is_the_run_report_plus_ok_dir_and_diagnostics() {
    let mock = Mock::start(MockConfig::default());
    let recipe = recipe_of(&mock_image_project("run"));
    let output = ludion(&["gen", "run", &recipe, "--url", &mock.url, "--json"]);
    let value = one_object(&output);
    assert!(output.status.success(), "{}", stderr(&output));
    assert_eq!(value["status"], "ok");
    assert_eq!(value["count"], 2);
    assert_eq!(value["recipe"], "generation/recipes/idol.gen.yaml");
    assert_eq!(value["outputs"].as_array().unwrap().len(), 2);
    matches_sample(&value, "gen-run.json");
}

#[test]
fn a_run_with_a_failed_job_is_a_failed_object_with_the_report() {
    let mock = Mock::start(MockConfig {
        fail_jobs: &[1],
        ..Default::default()
    });
    let recipe = recipe_of(&mock_project("partial"));
    let output = ludion(&["gen", "run", &recipe, "--url", &mock.url, "--json"]);
    let value = one_object(&output);
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(value["status"], "partial");
    assert_eq!(value["failed"][0]["job"], 1);
    assert_eq!(codes_of(&value), vec!["L1169"]);
    matches_sample(&value, "gen-run-partial.json");
}

#[test]
fn a_timed_out_run_prints_its_submitted_report_and_folder() {
    let mock = Mock::start(MockConfig {
        stall: true,
        ..Default::default()
    });
    let recipe = recipe_of(&mock_project("timeout"));
    let output = ludion(&[
        "gen",
        "run",
        &recipe,
        "--url",
        &mock.url,
        "--timeout",
        "1",
        "--json",
    ]);
    let value = one_object(&output);
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(codes_of(&value), vec!["L1170"]);
    assert_eq!(value["status"], "submitted");
    assert!(Path::new(&dir_of(&value)).join("run.json").is_file());
    matches_sample(&value, "gen-run-timed-out.json");
}

#[test]
fn a_run_refused_before_its_folder_exists_has_a_null_dir() {
    let recipe = recipe_of(&mock_project("refused"));
    let output = ludion(&["gen", "run", &recipe, "--url", UNREACHABLE_URL, "--json"]);
    let value = one_object(&output);
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(value["dir"], Value::Null);
    assert_eq!(codes_of(&value), vec!["L1167"]);
    matches_sample(&value, "gen-run-refused.json");
}

#[test]
fn status_and_fetch_of_a_no_wait_run_carry_the_run_keys() {
    let mock = Mock::start(MockConfig::default());
    let recipe = recipe_of(&mock_image_project("nowait"));
    let output = ludion(&[
        "gen",
        "run",
        &recipe,
        "--url",
        &mock.url,
        "--no-wait",
        "--json",
    ]);
    let submitted = one_object(&output);
    assert!(output.status.success(), "{}", stderr(&output));
    assert_eq!(submitted["status"], "submitted");
    let dir = dir_of(&submitted);

    let output = ludion(&["gen", "status", &dir, "--url", &mock.url, "--json"]);
    let status = one_object(&output);
    assert_eq!(status["dir"], Value::String(dir.clone()));
    assert_eq!(status["ready"], true);

    let output = ludion(&["gen", "fetch", &dir, "--url", &mock.url, "--json"]);
    let fetched = one_object(&output);
    assert!(output.status.success(), "{}", stderr(&output));
    assert_eq!(fetched["status"], "ok");
    matches_sample(&fetched, "gen-run.json");
}

#[test]
fn status_of_a_run_with_a_failed_and_a_queued_job_matches_the_sample() {
    let mock = Mock::start(MockConfig {
        fail_jobs: &[0],
        stall_jobs: &[1],
        ..Default::default()
    });
    let recipe = recipe_of(&mock_project("status-mixed"));
    let output = ludion(&[
        "gen",
        "run",
        &recipe,
        "--url",
        &mock.url,
        "--no-wait",
        "--json",
    ]);
    let dir = dir_of(&one_object(&output));

    let output = ludion(&["gen", "status", &dir, "--url", &mock.url, "--json"]);
    let status = one_object(&output);
    assert_eq!(status["ready"], false);
    assert_eq!(status["pending"], serde_json::json!(["job-1"]));
    assert_eq!(status["failed"][0]["prompt_id"], "job-0");
    matches_sample(&status, "gen-status.json");
}

#[test]
fn fetch_of_a_stalled_run_is_the_timed_out_object() {
    let mock = Mock::start(MockConfig {
        stall: true,
        ..Default::default()
    });
    let recipe = recipe_of(&mock_project("stalled-fetch"));
    let output = ludion(&[
        "gen",
        "run",
        &recipe,
        "--url",
        &mock.url,
        "--no-wait",
        "--json",
    ]);
    let dir = dir_of(&one_object(&output));

    let output = ludion(&["gen", "fetch", &dir, "--url", &mock.url, "--json"]);
    let value = one_object(&output);
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(codes_of(&value), vec!["L1170"]);
    matches_sample(&value, "gen-run-timed-out.json");
}

#[test]
fn status_of_a_folder_without_a_run_json_has_a_null_report() {
    let mock = Mock::start(MockConfig::default());
    let dir = std::env::temp_dir().join(format!(
        "ludion-cli-gen-mock-empty-run-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let output = ludion(&[
        "gen",
        "status",
        dir.to_str().unwrap(),
        "--url",
        &mock.url,
        "--json",
    ]);
    let value = one_object(&output);
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(value["report"], Value::Null);
    assert_eq!(
        value["dir"],
        Value::String(dir.to_str().unwrap().replace('\\', "/"))
    );
}

#[test]
fn nodes_json_lists_a_class_and_fails_with_a_null_list() {
    let mock = Mock::start(MockConfig::default());
    let output = ludion(&[
        "gen",
        "nodes",
        "--class",
        "SaveImage",
        "--url",
        &mock.url,
        "--json",
    ]);
    let value = one_object(&output);
    assert!(output.status.success(), "{}", stderr(&output));
    matches_sample(&value, "gen-nodes.json");

    let output = ludion(&[
        "gen", "nodes", "--class", "Nope", "--url", &mock.url, "--json",
    ]);
    let value = one_object(&output);
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(value["nodes"], Value::Null);
    assert_eq!(codes_of(&value), vec!["L1164"]);
}

#[test]
fn check_json_prints_an_object_on_success_and_on_failure() {
    let output = ludion(&[
        "gen",
        "check",
        &fixture("idol.gen.yaml"),
        "--url",
        UNREACHABLE_URL,
        "--json",
    ]);
    let value = one_object(&output);
    assert!(output.status.success(), "{}", stderr(&output));
    assert_eq!(value["diagnostics"][0]["severity"], "warning");
    matches_sample(&value, "gen-check.json");

    let output = ludion(&[
        "gen",
        "check",
        "definitely-not-a-real-recipe.gen.yaml",
        "--json",
    ]);
    let value = one_object(&output);
    assert_eq!(output.status.code(), Some(1));
    matches_sample(&value, "gen-check-failed.json");
}

#[test]
fn sheet_and_accept_json_print_an_object_on_failure_too() {
    let fixture = build_run("contract", 2);
    let run_dir = fixture.run_dir.to_str().unwrap();

    let output = ludion(&["gen", "sheet", run_dir, "--json"]);
    let value = one_object(&output);
    assert!(output.status.success(), "{}", stderr(&output));
    matches_sample(&value, "gen-sheet.json");

    let out = fixture.root.join("assets/idol.png");
    let accept = |out: &Path| {
        ludion(&[
            "gen",
            "accept",
            run_dir,
            "0",
            "--out",
            out.to_str().unwrap(),
            "--json",
        ])
    };
    let output = accept(&out);
    let accepted = one_object(&output);
    assert!(output.status.success(), "{}", stderr(&output));
    matches_sample(&accepted, "gen-accept.json");

    let output = accept(&out);
    let refused = one_object(&output);
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(codes_of(&refused), vec!["L1172"]);
    matches_sample(&refused, "gen-accept-failed.json");

    let keys = |v: &Value| -> Vec<String> { v.as_object().unwrap().keys().cloned().collect() };
    assert_eq!(keys(&accepted), keys(&refused));
}

// --- path spellings and finalized status ---------------------------------------------------------

#[test]
fn check_resolves_a_dot_slash_recipe_from_the_project_root() {
    let root = mock_image_project("spell-check-root");
    let output = ludion_in(
        &root,
        &[
            "gen",
            "check",
            "./generation/recipes/idol.gen.yaml",
            "--url",
            UNREACHABLE_URL,
            "--json",
        ],
    );
    let value = one_object(&output);
    assert_eq!(value["offline"], "ok", "{value:#}");
}

#[test]
fn check_resolves_a_bare_recipe_name_from_inside_generation_recipes() {
    let root = mock_image_project("spell-check-inside");
    let output = ludion_in(
        &root.join("generation/recipes"),
        &[
            "gen",
            "check",
            "idol.gen.yaml",
            "--url",
            UNREACHABLE_URL,
            "--json",
        ],
    );
    let value = one_object(&output);
    assert_eq!(value["offline"], "ok", "{value:#}");
}

#[test]
fn run_records_project_relative_paths_for_a_dot_slash_recipe() {
    let mock = Mock::start(MockConfig::default());
    let root = mock_image_project("spell-run");
    let output = ludion_in(
        &root,
        &[
            "gen",
            "run",
            "./generation/recipes/idol.gen.yaml",
            "--url",
            &mock.url,
            "--json",
        ],
    );
    let value = one_object(&output);
    assert!(output.status.success(), "{}", stderr(&output));
    assert_eq!(value["recipe"], "generation/recipes/idol.gen.yaml");
    assert_eq!(value["workflow"], "generation/workflows/txt2img.json");
    assert_eq!(
        value["images"][0]["path"],
        "generation/references/subject.png"
    );
}

#[test]
fn accept_of_a_legacy_report_through_dot_slash_records_the_declared_image_path() {
    let mock = Mock::start(MockConfig::default());
    let root = mock_image_project("spell-legacy");
    let output = ludion_in(
        &root,
        &[
            "gen",
            "run",
            "./generation/recipes/idol.gen.yaml",
            "--url",
            &mock.url,
            "--json",
        ],
    );
    let run = one_object(&output);
    // A printed `dir` is relative to the cwd the verb ran in.
    let dir = root.join(dir_of(&run));
    let report_path = dir.join("run.json");
    let mut report: Value =
        serde_json::from_str(&std::fs::read_to_string(&report_path).unwrap()).unwrap();
    report["prompt"] = Value::Null;
    report["inputs"] = Value::Null;
    report["images"] = serde_json::json!([]);
    std::fs::write(&report_path, serde_json::to_string_pretty(&report).unwrap()).unwrap();
    std::fs::create_dir_all(dir.join("inputs")).unwrap();
    std::fs::copy(fixture("subject.png"), dir.join("inputs/subject.png")).unwrap();

    let run_dir = format!("./{}", dir_of(&run).trim_start_matches("./"));
    let output = ludion_in(
        &root,
        &[
            "gen",
            "accept",
            &run_dir,
            "0",
            "--out",
            "assets/idol.png",
            "--json",
        ],
    );
    let value = one_object(&output);
    assert!(output.status.success(), "{value:#}");
    assert_eq!(
        value["origin"]["images"][0]["path"],
        "generation/references/subject.png"
    );
}

#[test]
fn status_of_a_finalized_run_asks_the_server_nothing() {
    let fixture = build_run("status-finalized", 1);
    let dir = fixture.run_dir.to_str().unwrap().to_string();
    let output = ludion(&["gen", "status", &dir, "--url", UNREACHABLE_URL, "--json"]);
    let value = one_object(&output);
    assert!(output.status.success(), "{value:#}");
    assert_eq!(value["ready"], true);
    assert_eq!(value["queue"], Value::Null);
    assert_eq!(value["report"]["status"], "ok");
}

#[test]
fn status_of_a_submitted_run_with_no_server_still_carries_its_report() {
    let mock = Mock::start(MockConfig::default());
    let recipe = recipe_of(&mock_project("status-submitted"));
    let output = ludion(&[
        "gen",
        "run",
        &recipe,
        "--url",
        &mock.url,
        "--no-wait",
        "--json",
    ]);
    let dir = dir_of(&one_object(&output));

    let output = ludion(&["gen", "status", &dir, "--url", UNREACHABLE_URL, "--json"]);
    let value = one_object(&output);
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(codes_of(&value), vec!["L1167"]);
    assert_eq!(value["report"]["status"], "submitted");
}
