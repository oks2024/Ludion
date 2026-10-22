//! Integration tests for `ludion render` and `ludion dump-graph`, driven against the built binary.
//!
//! The headline `render` test is the Phase 2 exit criterion at the CLI level: two *separate OS
//! processes* rendering the same scene write byte-identical PNGs. It skips gracefully when the
//! machine has no GPU adapter (printing why), matching the established cross-process test style in
//! `sim_hash.rs`. `dump-graph` needs no GPU and is always exercised — including as the CPU-only way
//! to verify what `--scale` does to the graph.

use std::path::PathBuf;
use std::process::{Command, Output};

fn empty_scene() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/scenes/empty.yaml")
}

fn gallery_scene(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../examples/gallery")
        .join(name)
}

/// Decodes a PNG file to RGBA8 (width, height, bytes).
fn decode_png(path: &std::path::Path) -> (u32, u32, Vec<u8>) {
    let file = std::fs::File::open(path).expect("PNG exists");
    let decoder = png::Decoder::new(std::io::BufReader::new(file));
    let mut reader = decoder.read_info().expect("valid PNG");
    let mut buf = vec![0u8; reader.output_buffer_size().unwrap()];
    let info = reader.next_frame(&mut buf).expect("frame decodes");
    buf.truncate(info.buffer_size());
    (info.width, info.height, buf)
}

fn out_path(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(name)
}

fn run_ludion(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_ludion"))
        .args(args)
        .output()
        .expect("failed to run the ludion binary")
}

/// Whether a failed `render` was due to a missing GPU adapter (L2003), in which case the test skips.
fn is_no_adapter(output: &Output) -> bool {
    ludion_core::require_gpu::skip_no_adapter(
        String::from_utf8_lossy(&output.stderr).contains("error[L2003]"),
    )
}

/// **Phase 2 cross-process render determinism.** Two invocations of the binary write byte-identical
/// PNGs. Skips when no GPU adapter is available.
#[test]
fn needs_gpu_two_processes_render_identical_png() {
    let scene = empty_scene();
    let first_out = out_path("cli_render_1.png");
    let second_out = out_path("cli_render_2.png");

    let args1 = [
        "render",
        "--scene",
        scene.to_str().unwrap(),
        "--out",
        first_out.to_str().unwrap(),
        "--width",
        "256",
        "--height",
        "144",
    ];
    let first = run_ludion(&args1);
    if !first.status.success() {
        if is_no_adapter(&first) {
            eprintln!("SKIPPED two_processes_render_identical_png: no GPU adapter");
            return;
        }
        panic!("render failed: {}", String::from_utf8_lossy(&first.stderr));
    }

    let args2 = [
        "render",
        "--scene",
        scene.to_str().unwrap(),
        "--out",
        second_out.to_str().unwrap(),
        "--width",
        "256",
        "--height",
        "144",
    ];
    let second = run_ludion(&args2);
    assert!(
        second.status.success(),
        "second render failed: {}",
        String::from_utf8_lossy(&second.stderr)
    );

    let a = std::fs::read(&first_out).expect("first PNG exists");
    let b = std::fs::read(&second_out).expect("second PNG exists");
    assert_eq!(a, b, "cross-process render determinism is broken");
    assert_eq!(&a[1..4], b"PNG", "output is a PNG");
}

/// **Phase 2 cross-process determinism on real scene content.** Two invocations rendering a gallery
/// scene write byte-identical PNGs, and the image is not a single flat color (non-triviality guard).
/// Skips when no GPU adapter is available.
#[test]
fn needs_gpu_two_processes_render_gallery_scene_identically_and_non_trivially() {
    let scene = gallery_scene("meshes.yaml");
    let first_out = out_path("cli_gallery_1.png");
    let second_out = out_path("cli_gallery_2.png");

    let args = |out: &str| {
        vec![
            "render".to_string(),
            "--scene".to_string(),
            scene.to_str().unwrap().to_string(),
            "--out".to_string(),
            out.to_string(),
            "--width".to_string(),
            "192".to_string(),
            "--height".to_string(),
            "108".to_string(),
        ]
    };

    let first = run_ludion(
        &args(first_out.to_str().unwrap())
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>(),
    );
    if !first.status.success() {
        if is_no_adapter(&first) {
            eprintln!("SKIPPED gallery render: no GPU adapter");
            return;
        }
        panic!("render failed: {}", String::from_utf8_lossy(&first.stderr));
    }
    let second = run_ludion(
        &args(second_out.to_str().unwrap())
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>(),
    );
    assert!(second.status.success());

    let a = std::fs::read(&first_out).unwrap();
    let b = std::fs::read(&second_out).unwrap();
    assert_eq!(
        a, b,
        "cross-process render of a gallery scene must be byte-identical"
    );

    // Non-triviality: the decoded image is not one flat color.
    let (_, _, pixels) = decode_png(&first_out);
    let first_px = &pixels[0..4];
    assert!(
        pixels.chunks_exact(4).any(|px| px != first_px),
        "gallery render is a single flat color — likely an all-black regression",
    );
}

/// **Cross-process determinism with ambient occlusion on** (`Plan/post-process-pipeline.md` wave 7).
/// SSAO is the arc's first stochastic-looking estimator, and decision 5 gave up the temporal
/// accumulation that would normally hide a non-reproducible one — so it has to be reproducible.
/// Nothing in the pass reads a clock, a frame index or an RNG: the kernel is a closed-form sequence
/// and the rotation is a function of the pixel, which two separate OS processes must agree on to the
/// byte. AO is driven through `--set` at both ends — the scene authors `ssao: 1.0`, so the
/// non-triviality baseline has to ask for `0` explicitly or it would compare two AO frames and prove
/// nothing about whether the pass ran. Skips when no GPU adapter is available.
#[test]
fn needs_gpu_two_processes_render_ambient_occlusion_identically() {
    let scene = gallery_scene("post-full.yaml");
    let first_out = out_path("cli_ssao_1.png");
    let second_out = out_path("cli_ssao_2.png");

    let args = |out: &str| {
        vec![
            "render".to_string(),
            "--scene".to_string(),
            scene.to_str().unwrap().to_string(),
            "--out".to_string(),
            out.to_string(),
            "--width".to_string(),
            "192".to_string(),
            "--height".to_string(),
            "108".to_string(),
            "--set".to_string(),
            "/entities/0/components/PostProcess/ssao=0.8".to_string(),
        ]
    };
    let run = |out: &std::path::Path| {
        run_ludion(
            &args(out.to_str().unwrap())
                .iter()
                .map(String::as_str)
                .collect::<Vec<_>>(),
        )
    };

    let first = run(&first_out);
    if !first.status.success() {
        if is_no_adapter(&first) {
            eprintln!("SKIPPED ssao render: no GPU adapter");
            return;
        }
        panic!("render failed: {}", String::from_utf8_lossy(&first.stderr));
    }
    let second = run(&second_out);
    assert!(
        second.status.success(),
        "second render failed: {}",
        String::from_utf8_lossy(&second.stderr)
    );

    assert_eq!(
        std::fs::read(&first_out).unwrap(),
        std::fs::read(&second_out).unwrap(),
        "cross-process ambient-occlusion render must be byte-identical",
    );

    // And AO must actually have done something: the same scene with the pass switched off is a
    // different image. Otherwise the equality above would be a determinism proof about a pass that
    // never ran.
    let baseline_out = out_path("cli_ssao_baseline.png");
    let baseline = run_ludion(&[
        "render",
        "--scene",
        scene.to_str().unwrap(),
        "--out",
        baseline_out.to_str().unwrap(),
        "--width",
        "192",
        "--height",
        "108",
        "--set",
        "/entities/0/components/PostProcess/ssao=0.0",
    ]);
    assert!(baseline.status.success());
    assert_ne!(
        std::fs::read(&first_out).unwrap(),
        std::fs::read(&baseline_out).unwrap(),
        "ambient occlusion must change the image it is proved deterministic on",
    );
}

/// **Cross-process determinism with volumetric light shafts on** (`Plan/post-process-pipeline.md`
/// wave 8). The march is the arc's second dithered estimator and, like SSAO, has no temporal
/// accumulation to hide a non-reproducible one (decision 5). Nothing in it reads a clock, a frame
/// index or an RNG: the step offset is `BAYER4` indexed by the fragment's pixel position, which two
/// separate OS processes must agree on to the byte. Shafts are driven through `--set` at both ends —
/// the scene authors `shafts: 0.02`, so the non-triviality baseline has to ask for `0` explicitly or
/// it would compare two marched frames. Skips when no GPU adapter is available.
#[test]
fn needs_gpu_two_processes_render_light_shafts_identically() {
    // post-full carries the gallery's only `DirectionalLight`, and shafts need one to march.
    let scene = gallery_scene("post-full.yaml");
    let first_out = out_path("cli_shafts_1.png");
    let second_out = out_path("cli_shafts_2.png");

    let args = |out: &str| {
        vec![
            "render".to_string(),
            "--scene".to_string(),
            scene.to_str().unwrap().to_string(),
            "--out".to_string(),
            out.to_string(),
            "--width".to_string(),
            "192".to_string(),
            "--height".to_string(),
            "108".to_string(),
            "--set".to_string(),
            "/entities/0/components/PostProcess/shafts=1.0".to_string(),
        ]
    };
    let run = |out: &std::path::Path| {
        run_ludion(
            &args(out.to_str().unwrap())
                .iter()
                .map(String::as_str)
                .collect::<Vec<_>>(),
        )
    };

    let first = run(&first_out);
    if !first.status.success() {
        if is_no_adapter(&first) {
            eprintln!("SKIPPED shafts render: no GPU adapter");
            return;
        }
        panic!("render failed: {}", String::from_utf8_lossy(&first.stderr));
    }
    let second = run(&second_out);
    assert!(
        second.status.success(),
        "second render failed: {}",
        String::from_utf8_lossy(&second.stderr)
    );

    assert_eq!(
        std::fs::read(&first_out).unwrap(),
        std::fs::read(&second_out).unwrap(),
        "cross-process light-shaft render must be byte-identical",
    );

    // And the march must actually have run: the same scene with it switched off is a different
    // image. Otherwise the equality above would be a determinism proof about a pass that never ran.
    let baseline_out = out_path("cli_shafts_baseline.png");
    let baseline = run_ludion(&[
        "render",
        "--scene",
        scene.to_str().unwrap(),
        "--out",
        baseline_out.to_str().unwrap(),
        "--width",
        "192",
        "--height",
        "108",
        "--set",
        "/entities/0/components/PostProcess/shafts=0.0",
    ]);
    assert!(baseline.status.success());
    assert_ne!(
        std::fs::read(&first_out).unwrap(),
        std::fs::read(&baseline_out).unwrap(),
        "light shafts must change the image they are proved deterministic on",
    );
}

/// **Cross-process determinism with film grain on** (`Plan/post-process-pipeline.md` wave 10). The
/// grain is a noise field, and a noise field is the one thing in this pipeline that could plausibly
/// have reached for `fract(sin(dot(...)))` — whose last bits are not guaranteed across backends or
/// driver revisions. It is an integer hash instead (Wellons' `triple32` over the cell's coordinates),
/// and this is the test that would break if it ever stopped being one: wrapping `u32` multiplies and
/// shifts are specified to the bit, a transcendental is not. Nothing else in the step reads a clock,
/// a frame index or an RNG — the pattern is static by decision — so two separate OS processes must
/// agree to the byte. Grain is switched on through `--set` rather than in the scene, so no committed
/// file moves. Skips when no GPU adapter is available.
#[test]
fn needs_gpu_two_processes_render_grain_identically() {
    // night-sky is the scene grain exists for: a low-luminance authored gradient, and at 108 lines
    // the cell floors to one render pixel, so every pixel of the frame takes its own hash.
    let scene = gallery_scene("night-sky.yaml");
    let first_out = out_path("cli_grain_1.png");
    let second_out = out_path("cli_grain_2.png");

    let args = |out: &str| {
        vec![
            "render".to_string(),
            "--scene".to_string(),
            scene.to_str().unwrap().to_string(),
            "--out".to_string(),
            out.to_string(),
            "--width".to_string(),
            "192".to_string(),
            "--height".to_string(),
            "108".to_string(),
            "--set".to_string(),
            "/entities/0/components/PostProcess/grain=0.02".to_string(),
        ]
    };
    let run = |out: &std::path::Path| {
        run_ludion(
            &args(out.to_str().unwrap())
                .iter()
                .map(String::as_str)
                .collect::<Vec<_>>(),
        )
    };

    let first = run(&first_out);
    if !first.status.success() {
        if is_no_adapter(&first) {
            eprintln!("SKIPPED grain render: no GPU adapter");
            return;
        }
        panic!("render failed: {}", String::from_utf8_lossy(&first.stderr));
    }
    let second = run(&second_out);
    assert!(
        second.status.success(),
        "second render failed: {}",
        String::from_utf8_lossy(&second.stderr)
    );

    assert_eq!(
        std::fs::read(&first_out).unwrap(),
        std::fs::read(&second_out).unwrap(),
        "cross-process grain render must be byte-identical",
    );

    // And the grain must actually have run: the same scene without the override is a different
    // image. Otherwise the equality above would be a determinism proof about a step that was skipped
    // — which, since the step is gated on `grain != 0`, is exactly what a broken override would
    // produce.
    let baseline_out = out_path("cli_grain_baseline.png");
    let baseline = run_ludion(&[
        "render",
        "--scene",
        scene.to_str().unwrap(),
        "--out",
        baseline_out.to_str().unwrap(),
        "--width",
        "192",
        "--height",
        "108",
    ]);
    assert!(baseline.status.success());
    assert_ne!(
        std::fs::read(&first_out).unwrap(),
        std::fs::read(&baseline_out).unwrap(),
        "grain must change the image it is proved deterministic on",
    );
}

/// `dump-graph` prints stable JSON — two runs are byte-identical — and needs no GPU.
#[test]
fn dump_graph_is_deterministic() {
    let scene = empty_scene();
    let args = ["dump-graph", "--scene", scene.to_str().unwrap()];
    let a = run_ludion(&args);
    let b = run_ludion(&args);
    assert!(
        a.status.success(),
        "dump-graph failed: {}",
        String::from_utf8_lossy(&a.stderr)
    );
    assert!(b.status.success());
    assert_eq!(
        a.stdout, b.stdout,
        "dump-graph output must be deterministic"
    );

    let value: serde_json::Value =
        serde_json::from_slice(&a.stdout).expect("dump-graph emits valid JSON");
    assert!(value.get("passes").is_some());
    assert!(value.get("aliasing").is_some());
    // Default dimensions are 1920x1080.
    assert_eq!(value["resources"][0]["width"], 1920);
}

/// The global `--quality` reaches the graph: at `low` a scene that authors AO, shafts and DoF builds
/// the same passes as one that authored none of them. `dump-graph` is the CPU-only way to see it, so
/// this is the whole flag proven end to end without a GPU.
#[test]
fn quality_masks_the_authored_effects_out_of_the_graph() {
    let scene = gallery_scene("post-full.yaml");
    let pass_names = |args: &[&str]| -> Vec<String> {
        let output = run_ludion(args);
        assert!(
            output.status.success(),
            "dump-graph failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let value: serde_json::Value =
            serde_json::from_slice(&output.stdout).expect("dump-graph emits valid JSON");
        value["passes"]
            .as_array()
            .expect("a graph has passes")
            .iter()
            .map(|pass| pass["name"].as_str().expect("a pass is named").to_string())
            .collect()
    };

    let full = pass_names(&["dump-graph", "--scene", scene.to_str().unwrap()]);
    let low = pass_names(&[
        "dump-graph",
        "--quality",
        "low",
        "--scene",
        scene.to_str().unwrap(),
    ]);
    assert!(
        full.iter().any(|name| name.starts_with("ssao")),
        "the fixture must author the effects being masked: {full:?}",
    );
    assert!(
        !low.iter().any(|name| name.starts_with("ssao")
            || name.starts_with("shafts")
            || name.starts_with("dof")),
        "`--quality low` must leave every optional effect's passes out: {low:?}",
    );
    assert!(low.len() < full.len());
}

/// A setting that is neither a preset nor a usable file refuses to start (`L0016`), before the
/// subcommand runs — the `LUDION_THREADS`/`L0014` posture, and the reason the knob exists: a
/// silently-ignored value means rendering at a quality nobody chose.
#[test]
fn an_unresolvable_quality_refuses_to_start() {
    let scene = empty_scene();
    let output = run_ludion(&[
        "dump-graph",
        "--quality",
        "medum",
        "--scene",
        scene.to_str().unwrap(),
    ]);
    assert!(!output.status.success(), "expected a non-zero exit");
    assert!(output.stdout.is_empty(), "no JSON on a refused setting");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("L0016"), "{stderr}");
    assert!(stderr.contains("medum"), "{stderr}");
}

/// An extent that is not a `WIDTHxHEIGHT` pixel pair refuses to start (`L0017`), before the
/// subcommand runs — the same posture `--threads`/`L0014` and `--quality`/`L0016` take, and for the
/// same reason: a silently-ignored value means streaming at a resolution nobody chose. `dump-graph`
/// is the vehicle because the chain is resolved globally, on every verb, including the ones that
/// never apply the extent.
#[test]
fn an_unresolvable_render_size_refuses_to_start() {
    let scene = empty_scene();
    // The four ways an extent fails: no separator, the unicode lookalike, a zero axis, and one past
    // the 8K ceiling.
    for bad in ["960", "960×540", "0x0", "9000x540"] {
        let output = run_ludion(&[
            "dump-graph",
            "--render-size",
            bad,
            "--scene",
            scene.to_str().unwrap(),
        ]);
        assert!(
            !output.status.success(),
            "expected a non-zero exit for `{bad}`"
        );
        assert!(output.stdout.is_empty(), "no JSON on a refused extent");
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(stderr.contains("L0017"), "{stderr}");
        assert!(stderr.contains("--render-size"), "{stderr}");
    }
}

/// A well-formed extent is accepted on any verb, and the flag beats the environment — so a host's
/// one-run instruction still works on a machine whose `LUDION_RENDER_SIZE` is malformed. Nothing is
/// asserted about `dump-graph`'s *output* here: the extent is deliberately applied by `serve` alone
/// (a per-machine variable that moved a golden's or a sheet's size would be the bug this design
/// avoids), so the observable property is that the chain resolves and the verb runs.
#[test]
fn a_well_formed_render_size_is_accepted_and_the_flag_beats_the_environment() {
    let scene = empty_scene();
    let args = [
        "dump-graph",
        "--render-size",
        "960x540",
        "--scene",
        scene.to_str().unwrap(),
    ];

    let plain = run_ludion(&args);
    assert!(
        plain.status.success(),
        "a well-formed extent must not fail a verb: {}",
        String::from_utf8_lossy(&plain.stderr),
    );

    let over_bad_env = Command::new(env!("CARGO_BIN_EXE_ludion"))
        .args(args)
        .env("LUDION_RENDER_SIZE", "not-an-extent")
        .output()
        .expect("failed to run the ludion binary");
    assert!(
        over_bad_env.status.success(),
        "the flag wins outright and the environment is not even validated: {}",
        String::from_utf8_lossy(&over_bad_env.stderr),
    );

    // And with no flag to shadow it, that same malformed variable is the `L0017` refusal.
    let env_only = Command::new(env!("CARGO_BIN_EXE_ludion"))
        .args(["dump-graph", "--scene", scene.to_str().unwrap()])
        .env("LUDION_RENDER_SIZE", "not-an-extent")
        .output()
        .expect("failed to run the ludion binary");
    assert!(!env_only.status.success(), "expected a non-zero exit");
    let stderr = String::from_utf8_lossy(&env_only.stderr);
    assert!(stderr.contains("L0017"), "{stderr}");
    assert!(stderr.contains("LUDION_RENDER_SIZE"), "{stderr}");
}

/// A `--frames` strip with no `--out` is a usage error, not a panic — `--dump-voxels` lifts clap's
/// own `--out` requirement, so the strip has to ask for it itself.
#[test]
fn a_frames_strip_without_out_is_a_usage_error() {
    let scene = empty_scene();
    let dump = out_path("strip_no_out_voxels.json");
    let output = run_ludion(&[
        "render",
        "--scene",
        scene.to_str().unwrap(),
        "--frames",
        "0,30",
        "--dump-voxels",
        dump.to_str().unwrap(),
    ]);
    assert_eq!(output.status.code(), Some(2), "a usage error, not a panic");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("--out"), "{stderr}");
    assert!(!stderr.contains("panicked"), "{stderr}");
}

/// A broken scene fails `render`/`dump-graph` before any GPU work, with the scene diagnostics.
#[test]
fn broken_scene_fails_before_gpu() {
    // A scene the parser rejects (mapping where a sequence is required).
    let broken = out_path("broken_scene.yaml");
    std::fs::write(&broken, "format: 1\nentities: {}\n").unwrap();

    let output = run_ludion(&["dump-graph", "--scene", broken.to_str().unwrap()]);
    assert!(!output.status.success(), "expected a non-zero exit");
    assert!(output.stdout.is_empty(), "no JSON on failure");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("error[L"),
        "expected an L-coded diagnostic: {stderr}"
    );
}

/// A `--set` into a real component field changes the rendered pixels and is echoed, and the scene
/// file on disk is untouched. `night-sky.yaml` carries the `PostProcess` bloom knobs the look sweeps
/// tune. Skips when no GPU adapter is available.
#[test]
fn needs_gpu_set_overrides_a_component_field_and_is_echoed() {
    let scene = gallery_scene("night-sky.yaml");
    let before = std::fs::read(&scene).expect("the gallery scene exists");
    let plain_out = out_path("cli_set_plain.png");
    let set_out = out_path("cli_set_override.png");

    let plain = run_ludion(&[
        "render",
        "--scene",
        scene.to_str().unwrap(),
        "--out",
        plain_out.to_str().unwrap(),
        "--width",
        "128",
        "--height",
        "72",
    ]);
    if !plain.status.success() {
        if is_no_adapter(&plain) {
            eprintln!("SKIPPED set_overrides_a_component_field_and_is_echoed: no GPU adapter");
            return;
        }
        panic!("render failed: {}", String::from_utf8_lossy(&plain.stderr));
    }
    assert!(
        !String::from_utf8_lossy(&plain.stdout).contains("set "),
        "a render with no --set must echo nothing"
    );

    let overridden = run_ludion(&[
        "render",
        "--scene",
        scene.to_str().unwrap(),
        "--out",
        set_out.to_str().unwrap(),
        "--width",
        "128",
        "--height",
        "72",
        "--set",
        "/entities/0/components/PostProcess/bloom_strength=0.0",
        "--set",
        "/entities/0/components/PostProcess/bloom_threshold=4.0",
    ]);
    assert!(
        overridden.status.success(),
        "overridden render failed: {}",
        String::from_utf8_lossy(&overridden.stderr)
    );

    let stdout = String::from_utf8_lossy(&overridden.stdout);
    assert!(
        stdout.contains("set /entities/0/components/PostProcess/bloom_strength: 0.12 -> 0.0"),
        "the applied override must be echoed with its before/after: {stdout}"
    );
    assert!(
        stdout.contains("set /entities/0/components/PostProcess/bloom_threshold: 1.0 -> 4.0"),
        "every applied override must be echoed: {stdout}"
    );

    assert_ne!(
        std::fs::read(&plain_out).unwrap(),
        std::fs::read(&set_out).unwrap(),
        "killing bloom must change the image — the override never reached the renderer"
    );
    assert_eq!(
        before,
        std::fs::read(&scene).unwrap(),
        "--set must never write to the scene file"
    );
}

/// `--look-at` and its companions move the scene camera and focus the depth of field through
/// ordinary overrides: the framing line and every override are echoed, and negative coordinates and
/// angles parse as written.
#[test]
fn needs_gpu_look_at_frames_the_shot_and_echoes_its_overrides() {
    let scene = gallery_scene("post-full.yaml");
    let out = out_path("cli_look_at.png");
    let output = run_ludion(&[
        "render",
        "--scene",
        scene.to_str().unwrap(),
        "--out",
        out.to_str().unwrap(),
        "--width",
        "128",
        "--height",
        "72",
        "--look-at",
        "-2,1,-15",
        "--azimuth",
        "-15",
        "--pitch",
        "30",
        "--distance",
        "12",
        "--fov",
        "45",
    ]);
    if !output.status.success() {
        if is_no_adapter(&output) {
            eprintln!("SKIPPED look_at_frames_the_shot_and_echoes_its_overrides: no GPU adapter");
            return;
        }
        panic!("render failed: {}", String::from_utf8_lossy(&output.stderr));
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("framing: look-at [-2.0,1.0,-15.0] azimuth -15 pitch 30 distance 12"),
        "{stdout}"
    );
    for echo in [
        "/Transform/translation: ",
        "/Camera/projection: ",
        "/PostProcess/focus_distance: 32.0 -> 12.0",
    ] {
        assert!(stdout.contains(echo), "missing the `{echo}` echo: {stdout}");
    }
}

/// A framing angle without `--look-at` is `L4008` before any GPU is touched.
#[test]
fn framing_flag_without_look_at_is_l4008() {
    let scene = gallery_scene("post-full.yaml");
    let output = run_ludion(&[
        "render",
        "--scene",
        scene.to_str().unwrap(),
        "--out",
        out_path("cli_look_at_missing.png").to_str().unwrap(),
        "--azimuth",
        "10",
    ]);
    assert!(!output.status.success(), "expected a non-zero exit");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("error[L4008]"), "{stderr}");
    assert!(stderr.contains("add --look-at x,y,z"), "{stderr}");
}

/// A pointer that does not resolve is `L4008`, with a help naming the deepest prefix that did
/// resolve plus a nearest match. A *component* is the case that stays unresolvable — absence is
/// meaningful for an opt-in singleton, so an override may not conjure one. No GPU is touched: the
/// failure is in the CPU prologue.
#[test]
fn unresolvable_set_pointer_is_l4008() {
    let scene = gallery_scene("night-sky.yaml");
    let output = run_ludion(&[
        "render",
        "--scene",
        scene.to_str().unwrap(),
        "--out",
        out_path("cli_set_unresolved.png").to_str().unwrap(),
        "--set",
        "/entities/0/components/PostProces/bloom_strength=0.2",
    ]);
    assert!(!output.status.success(), "expected a non-zero exit");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("error[L4008]"), "{stderr}");
    assert!(
        stderr.contains("/entities/0/components"),
        "the help must name the resolving prefix: {stderr}"
    );
    assert!(
        stderr.contains("nearest match is `PostProcess`"),
        "the help must suggest the nearest component: {stderr}"
    );
}

/// A field the scene left at its default is **created**, not refused — otherwise sweeping any knob
/// a file does not spell out would mean overriding the whole component map. The echo says `unset`
/// rather than `null`, because "this scene never mentioned that field" is exactly when an operator
/// wants to re-read the spelling.
#[test]
fn needs_gpu_set_creates_a_field_the_scene_left_at_its_default() {
    let scene = gallery_scene("night-sky.yaml");
    let output = run_ludion(&[
        "render",
        "--scene",
        scene.to_str().unwrap(),
        "--out",
        out_path("cli_set_created.png").to_str().unwrap(),
        "--width",
        "160",
        "--height",
        "90",
        "--set",
        // night-sky authors the two bloom scalars but not the operator.
        "/entities/0/components/PostProcess/tonemap=neutral",
    ]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("set /entities/0/components/PostProcess/tonemap: unset -> \"neutral\""),
        "a created field echoes `unset`, not `null`: {stdout}"
    );
}

/// The grade end to end, through the seam it is meant to be tuned over: `--set` creates a field
/// `night-sky.yaml` never authors, and `saturation: 0.0` comes out the far end as a frame with no
/// chroma left in it — parse, override, validate, extract, uniform and shader all in one assertion.
/// Greyness needs no baseline image, which is what makes it the direction worth checking here; the
/// other three steps are asserted on pixel statistics in the renderer's own tests.
#[test]
fn needs_gpu_set_drives_the_grade_end_to_end() {
    let scene = gallery_scene("night-sky.yaml");
    let out = out_path("cli_set_grade.png");
    let output = run_ludion(&[
        "render",
        "--scene",
        scene.to_str().unwrap(),
        "--out",
        out.to_str().unwrap(),
        "--width",
        "96",
        "--height",
        "54",
        "--set",
        "/entities/0/components/PostProcess/saturation=0.0",
    ]);
    if !output.status.success() {
        if is_no_adapter(&output) {
            eprintln!("SKIPPED set_drives_the_grade_end_to_end: no GPU adapter");
            return;
        }
        panic!("render failed: {}", String::from_utf8_lossy(&output.stderr));
    }
    assert!(
        String::from_utf8_lossy(&output.stdout)
            .contains("set /entities/0/components/PostProcess/saturation: unset -> 0.0"),
        "the created grade field must be echoed: {}",
        String::from_utf8_lossy(&output.stdout),
    );

    let (_, _, rgba) = decode_png(&out);
    for (index, px) in rgba.chunks_exact(4).enumerate() {
        assert!(
            px[0] == px[1] && px[1] == px[2],
            "pixel {index} is {:?}, so saturation 0 never reached the composite",
            &px[..3],
        );
    }
}

/// The safety argument for creating a field: a misspelled one is the schema's own `L1014`, at the
/// same pointer and naming the nearest declared field — the same answer the same typo would get
/// written into the file. So there is still no second rejection path, and a typo can never render a
/// scene nobody authored.
#[test]
fn a_created_field_with_a_typo_is_the_schemas_l1014() {
    let scene = gallery_scene("night-sky.yaml");
    let output = run_ludion(&[
        "render",
        "--scene",
        scene.to_str().unwrap(),
        "--out",
        out_path("cli_set_created_typo.png").to_str().unwrap(),
        "--set",
        "/entities/0/components/PostProcess/bloom_strenght=0.2",
    ]);
    assert!(!output.status.success(), "expected a non-zero exit");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("error[L1014]"), "{stderr}");
    assert!(
        stderr.contains("/entities/0/components/PostProcess/bloom_strenght"),
        "the schema diagnostic must point at the created field: {stderr}"
    );
    assert!(
        stderr.contains("nearest match is `bloom_strength`"),
        "the help must suggest the nearest field: {stderr}"
    );
    assert!(
        !stderr.contains("L4008"),
        "a misspelled field must not also raise the override code: {stderr}"
    );
}

/// A malformed `--set` spec is `L4008` before anything is loaded.
#[test]
fn malformed_set_spec_is_l4008() {
    let scene = gallery_scene("night-sky.yaml");
    for spec in ["bloom_strength", "entities/0/name=x"] {
        let output = run_ludion(&[
            "render",
            "--scene",
            scene.to_str().unwrap(),
            "--out",
            out_path("cli_set_malformed.png").to_str().unwrap(),
            "--set",
            spec,
        ]);
        assert!(
            !output.status.success(),
            "expected a non-zero exit for {spec}"
        );
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(stderr.contains("error[L4008]"), "{spec}: {stderr}");
    }
}

/// An out-of-range override comes back as the schema's own `L1014` at the exact JSON Pointer —
/// `--set` deliberately has no second rejection path for a value a component cannot hold.
#[test]
fn ill_typed_set_value_is_the_schemas_l1014() {
    let scene = gallery_scene("night-sky.yaml");
    let output = run_ludion(&[
        "render",
        "--scene",
        scene.to_str().unwrap(),
        "--out",
        out_path("cli_set_illtyped.png").to_str().unwrap(),
        "--set",
        "/entities/0/components/PostProcess/bloom_strength=-1.0",
    ]);
    assert!(!output.status.success(), "expected a non-zero exit");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("error[L1014]"), "{stderr}");
    assert!(
        stderr.contains("/entities/0/components/PostProcess/bloom_strength"),
        "the schema diagnostic must point at the overridden field: {stderr}"
    );
    assert!(
        !stderr.contains("L4008"),
        "a type error must not also raise the override code: {stderr}"
    );
}

/// `--set` and `--sweep` reach `--model` too, with the model's palette as their domain — so pairing
/// them is no longer a clap conflict, and the run gets as far as reading the document. (What the
/// palette domain accepts and refuses is `render_model.rs`.)
#[test]
fn set_and_sweep_are_not_scene_only() {
    for flag in ["--set", "--sweep"] {
        let output = run_ludion(&[
            "render",
            "--model",
            "whatever.voxel.yaml",
            "--views",
            "game",
            flag,
            "/name=x",
        ]);
        assert!(!output.status.success(), "the model file does not exist");
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            !stderr.contains("cannot be used with"),
            "`{flag}` and `--model` must not be a clap conflict: {stderr}"
        );
        assert!(
            stderr.contains("error[L1117]"),
            "expected the missing-model diagnostic, so the flags were accepted: {stderr}"
        );
    }
}

/// `--scale` supersamples: the PNG keeps the requested dimensions, the summary line names the
/// render extent it actually drew at (an unechoed quality setting would be the same footgun an
/// unechoed `--set` is), and the pixels differ from the native render. Skips with no GPU adapter.
#[test]
fn needs_gpu_render_scale_supersamples_without_changing_the_output_size() {
    let scene = gallery_scene("meshes.yaml");
    let native_out = out_path("cli_scale_native.png");
    let ssaa_out = out_path("cli_scale_2x.png");
    let args = |out: &str, scale: &str| {
        vec![
            "render".to_string(),
            "--scene".to_string(),
            scene.to_str().unwrap().to_string(),
            "--out".to_string(),
            out.to_string(),
            "--width".to_string(),
            "192".to_string(),
            "--height".to_string(),
            "108".to_string(),
            "--scale".to_string(),
            scale.to_string(),
        ]
    };
    let run = |out: &str, scale: &str| {
        run_ludion(
            &args(out, scale)
                .iter()
                .map(String::as_str)
                .collect::<Vec<_>>(),
        )
    };

    let native = run(native_out.to_str().unwrap(), "1.0");
    if !native.status.success() {
        if is_no_adapter(&native) {
            eprintln!("SKIPPED render_scale_supersamples_without_changing_the_output_size: no GPU");
            return;
        }
        panic!(
            "native render failed: {}",
            String::from_utf8_lossy(&native.stderr)
        );
    }
    assert!(
        !String::from_utf8_lossy(&native.stdout).contains("supersampled"),
        "scale 1 must not claim to supersample: {}",
        String::from_utf8_lossy(&native.stdout),
    );

    let ssaa = run(ssaa_out.to_str().unwrap(), "2.0");
    assert!(
        ssaa.status.success(),
        "supersampled render failed: {}",
        String::from_utf8_lossy(&ssaa.stderr)
    );
    let stdout = String::from_utf8_lossy(&ssaa.stdout);
    assert!(
        stdout.contains("rendered 192x108 (supersampled from 384x216)"),
        "the summary must name both extents: {stdout}"
    );

    let (nw, nh, native_px) = decode_png(&native_out);
    let (sw, sh, ssaa_px) = decode_png(&ssaa_out);
    assert_eq!((nw, nh), (192, 108));
    assert_eq!(
        (sw, sh),
        (192, 108),
        "supersampling must not change the PNG's dimensions"
    );
    assert_ne!(
        native_px, ssaa_px,
        "supersampling must actually resolve differently"
    );
}

/// An out-of-range `--scale` is `L4011` on stderr, with nothing written — and `dump-graph` rejects
/// it identically, so the flag's validation lives in one place.
#[test]
fn out_of_range_scale_is_l4011() {
    let scene = gallery_scene("meshes.yaml");
    for bad in ["0.1", "0", "9"] {
        let output = run_ludion(&[
            "dump-graph",
            "--scene",
            scene.to_str().unwrap(),
            "--scale",
            bad,
        ]);
        assert!(!output.status.success(), "--scale {bad} must be rejected");
        assert!(output.stdout.is_empty(), "no JSON on failure");
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(stderr.contains("error[L4011]"), "--scale {bad}: {stderr}");
    }
}

/// `dump-graph --scale` reports the graph the scale builds, in both directions: `resolve` after
/// `composite` above native and `upscale` below it, the scene target following the render extent
/// either way, and `final` staying the size the caller asked for. This is how an agent verifies a
/// render scale with no GPU at all.
#[test]
fn dump_graph_shows_the_resampling_pass_the_render_scale_calls_for() {
    let scene = empty_scene();
    let dump = |scale: &str| -> serde_json::Value {
        let output = run_ludion(&[
            "dump-graph",
            "--scene",
            scene.to_str().unwrap(),
            "--width",
            "640",
            "--height",
            "360",
            "--scale",
            scale,
        ]);
        assert!(
            output.status.success(),
            "dump-graph --scale {scale} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        serde_json::from_slice(&output.stdout).expect("dump-graph emits valid JSON")
    };

    let native = dump("1.0");
    let passes: Vec<&str> = native["schedule"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    assert!(
        !passes.contains(&"resolve"),
        "scale 1 schedules no resolve: {passes:?}"
    );

    let ssaa = dump("2.0");
    let passes: Vec<&str> = ssaa["schedule"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    assert_eq!(
        passes.last(),
        Some(&"resolve"),
        "resolve runs last on a UI-free supersampled frame: {passes:?}"
    );

    let resource = |dump: &serde_json::Value, name: &str| -> (u64, u64) {
        let r = dump["resources"]
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["name"] == name)
            .unwrap_or_else(|| panic!("the dump declares `{name}`"));
        (r["width"].as_u64().unwrap(), r["height"].as_u64().unwrap())
    };
    assert_eq!(
        resource(&ssaa, "scene"),
        (1280, 720),
        "the scene draws at 2x"
    );
    assert_eq!(resource(&ssaa, "ldr"), (1280, 720), "composite stays at 2x");
    assert_eq!(
        resource(&ssaa, "final"),
        (640, 360),
        "the output is what was asked for"
    );
    assert_eq!(
        resource(&ssaa, "bloom_bright"),
        resource(&native, "bloom_bright"),
        "bloom targets are sized from the output extent, so the scale cannot move them",
    );

    let sub_native = dump("0.5");
    let passes: Vec<&str> = sub_native["schedule"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    assert_eq!(
        passes.last(),
        Some(&"upscale"),
        "below native the tail magnifies instead: {passes:?}"
    );
    assert!(
        !passes.contains(&"resolve"),
        "a magnified frame never also resolves: {passes:?}"
    );
    assert_eq!(
        resource(&sub_native, "scene"),
        (320, 180),
        "the scene draws at half"
    );
    assert_eq!(
        resource(&sub_native, "final"),
        (640, 360),
        "the output is still what was asked for"
    );
    assert_eq!(
        resource(&sub_native, "bloom_bright"),
        resource(&native, "bloom_bright"),
        "the same rule below native: bloom follows the output extent, not the render one",
    );
}

/// **The precedence chain, end to end and with no GPU.** A quality that draws below native supplies
/// the render scale on its own — `--quality low` must dump the `upscale` pass and the smaller scene
/// target with no `--scale` given at all — and an explicit `--scale` beats it, in both directions:
/// back up to native, and past it into supersampling. `dump-graph` is how an agent verifies a render
/// scale on a box with no adapter, so this is where the chain is proved.
#[test]
fn dump_graph_takes_the_render_scale_from_the_quality_unless_a_scale_is_given() {
    let scene = empty_scene();
    let dump = |args: &[&str]| -> serde_json::Value {
        let mut argv = vec![
            "dump-graph",
            "--scene",
            scene.to_str().unwrap(),
            "--width",
            "640",
            "--height",
            "360",
        ];
        argv.extend_from_slice(args);
        let output = run_ludion(&argv);
        assert!(
            output.status.success(),
            "dump-graph {args:?} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        serde_json::from_slice(&output.stdout).expect("dump-graph emits valid JSON")
    };
    let tail = |dump: &serde_json::Value| -> String {
        dump["schedule"]
            .as_array()
            .unwrap()
            .last()
            .unwrap()
            .as_str()
            .unwrap()
            .to_string()
    };
    let scene_extent = |dump: &serde_json::Value| -> (u64, u64) {
        let r = dump["resources"]
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["name"] == "scene")
            .expect("the dump declares `scene`");
        (r["width"].as_u64().unwrap(), r["height"].as_u64().unwrap())
    };

    // 0.75 of 640x360 — the factor `low` ships, applied with nothing but `--quality`.
    let low = dump(&["--quality", "low"]);
    assert_eq!(tail(&low), "upscale");
    assert_eq!(scene_extent(&low), (480, 270));
    assert_eq!(
        scene_extent(&dump(&["--quality", "medium"])),
        (640, 360),
        "the presets that draw every output pixel change no extent",
    );

    // An explicit scale wins over the quality's, both back up to native and past it.
    let native = dump(&["--quality", "low", "--scale", "1.0"]);
    assert_eq!(scene_extent(&native), (640, 360));
    assert_ne!(tail(&native), "upscale", "native resamples through nothing");
    let ssaa = dump(&["--quality", "low", "--scale", "2.0"]);
    assert_eq!(tail(&ssaa), "resolve");
    assert_eq!(
        scene_extent(&ssaa),
        (1280, 720),
        "a machine told to supersample supersamples, whatever quality it renders at",
    );
}

/// A ground plane under `count` point lights on a ring, written to a scratch scene file. The lights
/// are authored one entity each, so document order fixes their `EntityId` order — which is the
/// global light array's order and therefore the per-fragment summation order.
fn write_ring_light_scene(path: &std::path::Path, count: usize) {
    // Strict-subset YAML: block style only, one element per line (a flow sequence is `L1005`).
    let seq = |indent: &str, values: [f32; 3]| {
        values
            .iter()
            .map(|v| format!("{indent}- {v:?}\n"))
            .collect::<String>()
    };
    let mut yaml = String::from("format: 1\nentities:\n");
    yaml.push_str("  - name: camera\n    components:\n      Transform:\n        translation:\n");
    yaml.push_str(&seq("          ", [0.0, 12.0, 14.0]));
    yaml.push_str("        rotation:\n");
    yaml.push_str(&seq("          ", [-38.0, 0.0, 0.0]));
    yaml.push_str(
        "      Camera:\n        projection:\n          type: perspective\n          \
         fov_y_deg: 50.0\n",
    );
    yaml.push_str("  - name: ground\n    components:\n      Transform:\n        scale:\n");
    yaml.push_str(&seq("          ", [24.0, 1.0, 24.0]));
    yaml.push_str("      Mesh:\n        shape: plane\n        color:\n");
    yaml.push_str("          - 0.6\n          - 0.6\n          - 0.62\n          - 1.0\n");
    for i in 0..count {
        let angle = i as f32 * (std::f32::consts::TAU / count as f32);
        yaml.push_str(&format!(
            "  - name: torch-{i}\n    components:\n      Transform:\n        translation:\n"
        ));
        yaml.push_str(&seq(
            "          ",
            [
                8.0 * ludion_core::math::cos(angle),
                1.5,
                8.0 * ludion_core::math::sin(angle),
            ],
        ));
        yaml.push_str("      PointLight:\n        color:\n");
        yaml.push_str("          - 1.0\n          - 0.75\n          - 0.45\n");
        yaml.push_str("        intensity: 2.5\n        range: 7.0\n");
    }
    std::fs::write(path, yaml).expect("scratch scene writes");
}

/// **Cross-process byte identity on a lit scene, well past the deleted 16-light cap**
/// (`Plan/clustered-forward.md` wave 1). Every light now rides a storage buffer and each fragment
/// walks its own cluster cell's slice of an index table, so "the same scene renders the same bytes
/// in two separate OS processes" is a claim about buffers the renderer builds on the CPU each frame,
/// not just about a fixed-size uniform. Twenty-four lights — a population the old path could not
/// express at all. Skips when no GPU adapter is available.
#[test]
fn needs_gpu_two_processes_render_a_scene_past_the_old_light_cap_identically() {
    let scene = out_path("cli_many_lights.yaml");
    write_ring_light_scene(&scene, 24);
    let first_out = out_path("cli_many_lights_1.png");
    let second_out = out_path("cli_many_lights_2.png");

    let render = |out: &std::path::Path| {
        run_ludion(&[
            "render",
            "--scene",
            scene.to_str().unwrap(),
            "--out",
            out.to_str().unwrap(),
            "--width",
            "192",
            "--height",
            "108",
        ])
    };

    let first = render(&first_out);
    if !first.status.success() {
        if is_no_adapter(&first) {
            eprintln!("SKIPPED many-lights render: no GPU adapter");
            return;
        }
        panic!("render failed: {}", String::from_utf8_lossy(&first.stderr));
    }
    let second = render(&second_out);
    assert!(
        second.status.success(),
        "second render failed: {}",
        String::from_utf8_lossy(&second.stderr)
    );

    assert_eq!(
        std::fs::read(&first_out).unwrap(),
        std::fs::read(&second_out).unwrap(),
        "cross-process render of a 24-light scene must be byte-identical",
    );

    // …and the lights are doing real work: the same geometry with none of them is a different image.
    let dark_scene = out_path("cli_no_lights.yaml");
    write_ring_light_scene(&dark_scene, 0);
    let dark_out = out_path("cli_no_lights.png");
    let dark = run_ludion(&[
        "render",
        "--scene",
        dark_scene.to_str().unwrap(),
        "--out",
        dark_out.to_str().unwrap(),
        "--width",
        "192",
        "--height",
        "108",
    ]);
    assert!(
        dark.status.success(),
        "the light-free scene must render (an empty light array still has to bind): {}",
        String::from_utf8_lossy(&dark.stderr)
    );
    assert_ne!(
        std::fs::read(&first_out).unwrap(),
        std::fs::read(&dark_out).unwrap(),
        "the twenty-four lights must change the image, or the equality above proves nothing",
    );
}

/// `--frames N` ticks the built-in schedule before extracting, then renders exactly as a plain
/// `--out` render does — one PNG, still deterministic: two invocations at the same frame count write
/// byte-identical PNGs, and the summary line echoes the tick count. Skips when no GPU adapter is
/// available.
#[test]
fn needs_gpu_frames_single_value_renders_one_deterministic_png() {
    let scene = gallery_scene("meshes.yaml");
    let first_out = out_path("cli_frames_single_1.png");
    let second_out = out_path("cli_frames_single_2.png");

    let args = |out: &str| {
        vec![
            "render".to_string(),
            "--scene".to_string(),
            scene.to_str().unwrap().to_string(),
            "--out".to_string(),
            out.to_string(),
            "--width".to_string(),
            "192".to_string(),
            "--height".to_string(),
            "108".to_string(),
            "--frames".to_string(),
            "3".to_string(),
        ]
    };
    let run = |out: &str| run_ludion(&args(out).iter().map(String::as_str).collect::<Vec<_>>());

    let first = run(first_out.to_str().unwrap());
    if !first.status.success() {
        if is_no_adapter(&first) {
            eprintln!("SKIPPED frames_single_value_renders_one_deterministic_png: no GPU adapter");
            return;
        }
        panic!("render failed: {}", String::from_utf8_lossy(&first.stderr));
    }
    let stdout = String::from_utf8_lossy(&first.stdout);
    assert!(
        stdout.contains("rendered 192x108") && stdout.contains("(frames 3)"),
        "the summary must echo the tick count: {stdout}"
    );

    let second = run(second_out.to_str().unwrap());
    assert!(
        second.status.success(),
        "second render failed: {}",
        String::from_utf8_lossy(&second.stderr)
    );

    let a = std::fs::read(&first_out).unwrap();
    let b = std::fs::read(&second_out).unwrap();
    assert_eq!(
        a, b,
        "two renders at the same --frames must be byte-identical"
    );
    assert_eq!(&a[1..4], b"PNG");
}

/// `--frames a,b,c` stitches a labelled time strip through the sweep sheet's own machinery, forced
/// to one row: an odd requested width proves the cell math ran (integer division truncates to the
/// nearest cell boundary) rather than merely matching an even request by coincidence. Skips when no
/// GPU adapter is available.
#[test]
fn needs_gpu_frames_list_stitches_a_two_cell_strip() {
    let scene = gallery_scene("meshes.yaml");
    let out = out_path("cli_frames_strip.png");
    let output = run_ludion(&[
        "render",
        "--scene",
        scene.to_str().unwrap(),
        "--out",
        out.to_str().unwrap(),
        "--width",
        "201",
        "--height",
        "100",
        "--frames",
        "0,2",
    ]);
    if !output.status.success() {
        if is_no_adapter(&output) {
            eprintln!("SKIPPED frames_list_stitches_a_two_cell_strip: no GPU adapter");
            return;
        }
        panic!("render failed: {}", String::from_utf8_lossy(&output.stderr));
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("frames 0,2: 2 cells,"),
        "the summary must name the frames and cell count: {stdout}"
    );

    // 201 / 2 cols = 100 per cell (integer division), so the stitched sheet is 200 wide — two whole
    // cells, not the 201 that was asked for.
    let (width, height, _) = decode_png(&out);
    assert_eq!(width, 200, "the strip's width must be exactly two cells");
    assert_eq!(
        height, 100,
        "a strip is one row, so the height is unchanged"
    );
}

/// `--frames` values must be strictly ascending, non-negative integers — both a descending list and
/// a non-integer are refused before any scene is touched (clap's own parser for the bad integer,
/// `main.rs`'s own check for the order), and both name the flag.
#[test]
fn frames_rejects_descending_values_and_bad_integers() {
    let scene = gallery_scene("meshes.yaml");
    for frames in ["2,1", "x"] {
        let output = run_ludion(&[
            "render",
            "--scene",
            scene.to_str().unwrap(),
            "--out",
            out_path("cli_frames_bad.png").to_str().unwrap(),
            "--frames",
            frames,
        ]);
        assert!(
            !output.status.success(),
            "--frames {frames} must be rejected"
        );
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            stderr.contains("--frames"),
            "--frames {frames}: the error must mention the flag: {stderr}"
        );
    }
}

/// `--frames a,b,c` needs `--out` exactly like `--sweep` — enforced by the same clap declaration
/// that already requires `--out` on every plain `--scene` render (`out`'s
/// `required_unless_present_any = ["model", "dump_voxels", "dump_gi"]` in `main.rs`; `frames` is not
/// one of the exceptions, so this needs no `--frames`-specific requirement of its own). A clean clap
/// refusal, not a panic in `run_frame_strip`'s `.expect()`.
#[test]
fn frames_list_without_out_is_a_clean_refusal_not_a_panic() {
    let scene = gallery_scene("meshes.yaml");
    let output = run_ludion(&[
        "render",
        "--scene",
        scene.to_str().unwrap(),
        "--frames",
        "0,2",
    ]);
    assert!(
        !output.status.success(),
        "`--frames 0,2` with no `--out` must be refused"
    );
    assert!(output.stdout.is_empty(), "no PNG summary on a refused run");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !stderr.contains("panicked"),
        "must be a clap refusal, never a panic: {stderr}"
    );
    assert!(stderr.contains("--out"), "{stderr}");
    assert!(stderr.contains("--frames"), "{stderr}");
}

/// Every render-domain L-code the commands can surface has a working `ludion explain` entry.
#[test]
fn render_codes_are_explainable() {
    for code in [
        "L2001", "L2002", "L2003", "L2004", "L2005", "L2006", "L4008", "L4011",
    ] {
        let output = run_ludion(&["explain", code]);
        assert!(
            output.status.success(),
            "`ludion explain {code}` failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

/// A scratch project under the OS temp dir holding a square `kind: path` and a scene whose
/// `PathFollower` rides it — the smallest scene the built-in host has engine debug lines for.
fn write_path_project(tag: &str) -> (PathBuf, PathBuf) {
    let dir = std::env::temp_dir().join(format!(
        "ludion-cli-debug-draw-{tag}-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("paths")).unwrap();
    std::fs::write(
        dir.join("paths/square.path.yaml"),
        "format: 1\nkind: path\ninterpolation: linear\nclosed: true\npoints:\n  - - 0.0\n    - 0.0\n    - 0.0\n  - - 4.0\n    - 0.0\n    - 0.0\n  - - 4.0\n    - 0.0\n    - 4.0\n  - - 0.0\n    - 0.0\n    - 4.0\n",
    )
    .unwrap();
    let scene = dir.join("scene.yaml");
    std::fs::write(
        &scene,
        "format: 1\nseed: 1\nentities:\n  - name: camera\n    components:\n      Transform:\n        translation:\n          - 2.0\n          - 8.0\n          - 8.0\n        rotation:\n          - -45.0\n          - 0.0\n          - 0.0\n      Camera: {}\n  - name: platform\n    components:\n      Transform: {}\n      PathFollower:\n        path: paths/square.path.yaml\n        speed: 1.0\n",
    )
    .unwrap();
    (dir, scene)
}

/// `--debug-draw` draws the engine's `path` lines over a follower scene: the PNG differs from the
/// plain render, the summary names the category, and a typo'd category is called out. Skips when no
/// GPU adapter is available.
#[test]
fn needs_gpu_debug_draw_draws_a_followers_path_and_reports_a_typo() {
    let (dir, scene) = write_path_project("path");
    let scene = scene.to_str().unwrap().to_string();
    let plain_out = dir.join("plain.png");
    let drawn_out = dir.join("drawn.png");
    let typo_out = dir.join("typo.png");
    let render = |out: &std::path::Path, extra: &[&str]| {
        let mut args = vec![
            "render",
            "--scene",
            &scene,
            "--out",
            out.to_str().unwrap(),
            "--width",
            "192",
            "--height",
            "108",
            "--frames",
            "10",
        ];
        args.extend_from_slice(extra);
        run_ludion(&args)
    };

    let plain = render(&plain_out, &[]);
    if !plain.status.success() {
        if is_no_adapter(&plain) {
            eprintln!(
                "SKIPPED debug_draw_draws_a_followers_path_and_reports_a_typo: no GPU adapter"
            );
            let _ = std::fs::remove_dir_all(&dir);
            return;
        }
        panic!("render failed: {}", String::from_utf8_lossy(&plain.stderr));
    }
    let drawn = render(&drawn_out, &["--debug-draw", "all"]);
    assert!(
        drawn.status.success(),
        "{}",
        String::from_utf8_lossy(&drawn.stderr)
    );
    let stdout = String::from_utf8_lossy(&drawn.stdout);
    assert!(
        stdout.contains("(debug draw: all)"),
        "summary names the draw: {stdout}"
    );
    assert_ne!(
        decode_png(&plain_out).2,
        decode_png(&drawn_out).2,
        "the follower's path must be drawn over the frame"
    );

    let typo = render(&typo_out, &["--debug-draw", "path,motr"]);
    assert!(
        typo.status.success(),
        "{}",
        String::from_utf8_lossy(&typo.stderr)
    );
    let stdout = String::from_utf8_lossy(&typo.stdout);
    assert!(
        stdout.contains("debug draw: motr,path; nothing drawn for: motr"),
        "a category that drew nothing is named: {stdout}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// `--debug-draw` is refused where it cannot draw: with a multi-value `--frames` strip and with a
/// model sheet. Both refusals happen before any GPU work.
#[test]
fn debug_draw_is_refused_on_a_strip_and_a_model_sheet() {
    let scene = empty_scene();
    let strip = run_ludion(&[
        "render",
        "--scene",
        scene.to_str().unwrap(),
        "--out",
        "unused.png",
        "--frames",
        "0,10",
        "--debug-draw",
        "all",
    ]);
    assert!(!strip.status.success());
    assert!(
        String::from_utf8_lossy(&strip.stderr).contains("--debug-draw"),
        "the refusal names the flag: {}",
        String::from_utf8_lossy(&strip.stderr)
    );

    let model = run_ludion(&[
        "render",
        "--model",
        "unused.voxel.yaml",
        "--debug-draw",
        "all",
    ]);
    assert!(!model.status.success());
    assert!(String::from_utf8_lossy(&model.stderr).contains("--debug-draw"));
}

/// `--debug-view` draws only where the frame traces reflections: with `PostProcess.ssr` authored it
/// names the view on the summary line, without it the normal frame is drawn and the line says so.
#[test]
fn needs_gpu_debug_view_names_what_was_drawn() {
    let scene = gallery_scene("night-sky.yaml");
    let run = |name: &str, extra: &[&str]| {
        let out = out_path(name);
        let mut args = vec![
            "render",
            "--scene",
            scene.to_str().unwrap(),
            "--out",
            out.to_str().unwrap(),
            "--width",
            "192",
            "--height",
            "108",
            "--debug-view",
            "ssr_confidence",
        ];
        args.extend_from_slice(extra);
        run_ludion(&args)
    };
    let drawn = run(
        "cli_debug_view_on.png",
        &["--set", "/entities/0/components/PostProcess/ssr=1.0"],
    );
    if !drawn.status.success() && is_no_adapter(&drawn) {
        eprintln!("SKIPPED debug view render: no GPU adapter");
        return;
    }
    assert!(
        drawn.status.success(),
        "render failed: {}",
        String::from_utf8_lossy(&drawn.stderr)
    );
    let stdout = String::from_utf8_lossy(&drawn.stdout);
    assert!(stdout.contains("debug view: ssr_confidence"), "{stdout}");
    assert!(!stdout.contains("not drawn"), "{stdout}");

    let skipped = run("cli_debug_view_off.png", &[]);
    assert!(skipped.status.success());
    let stdout = String::from_utf8_lossy(&skipped.stdout);
    assert!(
        stdout.contains("debug view: ssr_confidence not drawn"),
        "{stdout}"
    );
}

const PROBE_SCENE: &str = "format: 1
entities:
  - name: camera
    components:
      Transform:
        translation:
          - 0.0
          - 1.5
          - 6.0
      Camera: {}
  - name: environment
    components:
      Environment: {}
  - name: block
    components:
      Transform:
        translation:
          - 0.0
          - 0.5
          - 0.0
      Mesh:
        shape: cube
  - name: left
    components:
      Transform:
        translation:
          - -3.0
          - 1.0
          - 0.0
      ReflectionProbe: {}
  - name: right
    components:
      Transform:
        translation:
          - 3.0
          - 1.0
          - 0.0
      ReflectionProbe:
        shape:
          sphere:
            radius: 4.0
";

fn probe_scene(tag: &str) -> PathBuf {
    let dir = out_path(&format!("cli_dump_probes_{tag}"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let scene = dir.join("scene.yaml");
    std::fs::write(&scene, PROBE_SCENE).unwrap();
    scene
}

/// `--dump-probes` alone writes the manifest and both strips per probe, and with `--out` the frame
/// as well.
#[test]
fn needs_gpu_dump_probes_writes_strips_and_a_manifest() {
    let scene = probe_scene("both");
    let dump = out_path("cli_dump_probes_both_out");
    let _ = std::fs::remove_dir_all(&dump);
    let frame = out_path("cli_dump_probes_both_frame.png");
    let _ = std::fs::remove_file(&frame);
    let output = run_ludion(&[
        "render",
        "--scene",
        scene.to_str().unwrap(),
        "--dump-probes",
        dump.to_str().unwrap(),
    ]);
    if is_no_adapter(&output) {
        return;
    }
    assert!(output.status.success(), "{output:?}");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("2 probes"), "{stdout}");
    assert!(
        stdout.contains("box") && stdout.contains("sphere"),
        "{stdout}"
    );
    assert!(stdout.contains("capture "), "{stdout}");
    for rank in 0..2 {
        for kind in ["captured", "prefiltered"] {
            assert!(dump.join(format!("probe-{rank}-{kind}.png")).is_file());
        }
    }
    let manifest: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(dump.join("probes.json")).unwrap()).unwrap();
    assert_eq!(manifest["probes"].as_array().unwrap().len(), 2);
    assert!(!frame.exists(), "a dump-only run wrote a frame");

    let output = run_ludion(&[
        "render",
        "--scene",
        scene.to_str().unwrap(),
        "--dump-probes",
        dump.to_str().unwrap(),
        "--out",
        frame.to_str().unwrap(),
    ]);
    assert!(output.status.success(), "{output:?}");
    assert!(frame.is_file());
}

/// `--debug-view probe_index` is drawn where the scene has probes and says why when it is not.
#[test]
fn needs_gpu_debug_view_probe_index_names_what_was_drawn() {
    let run = |scene: &std::path::Path, name: &str| {
        let out = out_path(name);
        run_ludion(&[
            "render",
            "--scene",
            scene.to_str().unwrap(),
            "--out",
            out.to_str().unwrap(),
            "--width",
            "192",
            "--height",
            "108",
            "--debug-view",
            "probe_index",
        ])
    };
    let drawn = run(&probe_scene("view"), "cli_debug_view_probe_on.png");
    if !drawn.status.success() && is_no_adapter(&drawn) {
        eprintln!("SKIPPED probe index view render: no GPU adapter");
        return;
    }
    assert!(
        drawn.status.success(),
        "render failed: {}",
        String::from_utf8_lossy(&drawn.stderr)
    );
    let stdout = String::from_utf8_lossy(&drawn.stdout);
    assert!(stdout.contains("debug view: probe_index"), "{stdout}");
    assert!(!stdout.contains("not drawn"), "{stdout}");

    let skipped = run(
        &gallery_scene("night-sky.yaml"),
        "cli_debug_view_probe_off.png",
    );
    assert!(skipped.status.success());
    let stdout = String::from_utf8_lossy(&skipped.stdout);
    assert!(
        stdout.contains("debug view: probe_index not drawn: the scene has no reflection probes"),
        "{stdout}"
    );
}

#[test]
fn dump_probes_is_refused_with_sweep() {
    let scene = probe_scene("sweep");
    let output = run_ludion(&[
        "render",
        "--scene",
        scene.to_str().unwrap(),
        "--dump-probes",
        out_path("cli_dump_probes_sweep_out").to_str().unwrap(),
        "--sweep",
        "/entities/0/components/Transform/translation/0=1,2",
        "--out",
        out_path("cli_dump_probes_sweep.png").to_str().unwrap(),
    ]);
    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("cannot be used with"),
        "{output:?}"
    );
}

#[test]
fn needs_gpu_dump_probes_on_a_probeless_scene_prints_zero() {
    let dump = out_path("cli_dump_probes_none_out");
    let _ = std::fs::remove_dir_all(&dump);
    let output = run_ludion(&[
        "render",
        "--scene",
        gallery_scene("night-sky.yaml").to_str().unwrap(),
        "--dump-probes",
        dump.to_str().unwrap(),
    ]);
    if is_no_adapter(&output) {
        return;
    }
    assert!(output.status.success(), "{output:?}");
    assert!(String::from_utf8_lossy(&output.stdout).contains("0 probes"));
}
