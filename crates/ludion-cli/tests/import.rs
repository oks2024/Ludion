//! Integration tests for `ludion import` (Phase 8 Wave 3), driven against the built binary plus the
//! engine/testkit libraries. The headline is the **Wave 6 mechanized exit gate**: the committed
//! `examples/import/` fixture compiles, validates, renders to its blessed golden, re-exports
//! canonically byte-for-byte, and re-imports byte-identically to the committed expected tree.

use std::path::{Path, PathBuf};
use std::process::{Command as ProcCommand, Output};

use ludion_engine::{Engine, dispatch};
use ludion_protocol::Command;
use ludion_protocol::command::SceneSaveParams;
use ludion_testkit::{GoldenOutcome, GoldenTest, bless_requested};

use crate::common;

fn import_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/import")
}
fn textured_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/import-textured")
}
fn cutout_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/import-cutout")
}
fn tinted_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/import-tinted-emissive")
}
fn node_clips_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/import-node-clips")
}
fn fixture() -> PathBuf {
    import_root().join("scene.gltf")
}
fn expected_dir() -> PathBuf {
    import_root().join("expected")
}
fn golden(platform: &str) -> PathBuf {
    golden_in(&import_root(), platform)
}
fn textured_golden(platform: &str) -> PathBuf {
    golden_in(&textured_root(), platform)
}
fn tinted_golden(platform: &str) -> PathBuf {
    golden_in(&tinted_root(), platform)
}
fn golden_in(root: &Path, platform: &str) -> PathBuf {
    root.join("goldens").join(platform).join("scene.png")
}
thread_local! {
    // A test's scratch locks live until its thread ends, so helpers can hand back a plain path.
    static SCRATCH_LOCKS: std::cell::RefCell<Vec<common::ScratchLock>> = const { std::cell::RefCell::new(Vec::new()) };
}
/// A fresh scratch directory (see `common::stable_scratch`), locked until the calling test ends.
fn scratch(name: &str) -> PathBuf {
    let (dir, lock) = common::stable_scratch(name);
    SCRATCH_LOCKS.with(|locks| locks.borrow_mut().push(lock));
    dir
}
fn run_ludion(args: &[&str]) -> Output {
    ProcCommand::new(env!("CARGO_BIN_EXE_ludion"))
        .args(args)
        .output()
        .expect("failed to run the ludion binary")
}

/// The committed trees' hand-authored wrapper scene: it instances `model.prefab.yaml` beside the
/// camera, and no import writes it.
const WRAPPER_SCENE: &str = "scene.yaml";

/// Copies `gltf` into the fresh scratch `name` and imports it into `<scratch>/expected`, so the
/// prefab's `generated_from` reads as it does in the committed tree. Returns that output
/// directory and the CLI's output.
fn import_beside_copy(name: &str, gltf: &Path, extra: &[&str]) -> (PathBuf, Output) {
    let dir = scratch(name);
    let copy = dir.join("scene.gltf");
    std::fs::copy(gltf, &copy).unwrap();
    let out = dir.join("expected");
    let mut args = vec![
        "import",
        copy.to_str().unwrap(),
        "--out",
        out.to_str().unwrap(),
    ];
    args.extend_from_slice(extra);
    let output = run_ludion(&args);
    (out, output)
}

/// Asserts every file under `expected` (bar the hand-authored wrapper scene) exists under `actual`
/// with byte-identical content, and that `actual` introduces no extra files — a full
/// byte-for-byte tree comparison.
fn assert_trees_identical(expected: &Path, actual: &Path) {
    let mut expected_files = collect_files(expected);
    expected_files.retain(|rel| rel != Path::new(WRAPPER_SCENE));
    let mut actual_files = collect_files(actual);
    expected_files.sort();
    actual_files.sort();
    assert_eq!(
        expected_files, actual_files,
        "output tree file set differs from the committed expected tree"
    );
    for rel in &expected_files {
        let a = std::fs::read(expected.join(rel)).unwrap();
        let b = std::fs::read(actual.join(rel)).unwrap();
        assert_eq!(
            a,
            b,
            "`{}` differs from the committed expected tree",
            rel.display()
        );
    }
}

/// Every file under `root`, as paths relative to `root`.
fn collect_files(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                stack.push(path);
            } else {
                out.push(path.strip_prefix(root).unwrap().to_path_buf());
            }
        }
    }
    out
}

/// The five-map fixture (`examples/import-textured/`) imports in **default mode** — no
/// `--allow-lossy`, no warning — and reproduces its committed tree byte-for-byte: two mesh documents
/// (one with an authored `TANGENT`, one whose frame the importer generated) and five hash-named PNGs
/// from six glTF images, since two of them are pixel-identical.
#[test]
fn textured_import_reproduces_committed_tree_in_default_mode() {
    let (out, output) =
        import_beside_copy("import_textured", &textured_root().join("scene.gltf"), &[]);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "the textured fixture must import with no unsupported feature: {stderr}"
    );
    assert!(
        stderr.is_empty(),
        "a fully supported document warns about nothing: {stderr}"
    );
    assert_trees_identical(&textured_root().join("expected"), &out);

    for rel in [
        "model.prefab.yaml",
        "meshes/tangent-cube.mesh.yaml",
        "meshes/computed-cube.mesh.yaml",
    ] {
        let check = run_ludion(&["check-scene", out.join(rel).to_str().unwrap()]);
        assert!(
            check.status.success(),
            "check-scene failed for {rel}: {}",
            String::from_utf8_lossy(&check.stderr)
        );
    }
}

/// A re-import over a tree holding a file the new import would not write is `L0030` naming it and
/// leaves the tree untouched; an identical re-import over its own output still succeeds.
#[test]
fn a_reimport_over_stale_outputs_is_refused_and_an_identical_one_succeeds() {
    let (out, first) = import_beside_copy(
        "import_stale_outputs",
        &textured_root().join("scene.gltf"),
        &[],
    );
    assert!(first.status.success());
    let input = out.parent().unwrap().join("scene.gltf");
    let reimport = || {
        run_ludion(&[
            "import",
            input.to_str().unwrap(),
            "--out",
            out.to_str().unwrap(),
        ])
    };
    assert!(
        reimport().status.success(),
        "an identical re-import over its own output"
    );

    let stale = out.join("meshes/old.mesh.yaml");
    std::fs::write(&stale, "stale").unwrap();
    let before = collect_files(&out);
    let refused = reimport();
    let stderr = String::from_utf8_lossy(&refused.stderr);
    assert!(!refused.status.success());
    assert!(stderr.contains("L0030"), "{stderr}");
    assert!(stderr.contains("meshes/old.mesh.yaml"), "{stderr}");
    let mut after = collect_files(&out);
    let mut before = before;
    before.sort();
    after.sort();
    assert_eq!(before, after, "the refusal wrote nothing");
    assert_eq!(std::fs::read_to_string(&stale).unwrap(), "stale");
}

/// The tinted-emissive fixture (`examples/import-tinted-emissive/`) imports in **default mode** with
/// no warning and reproduces its committed tree byte-for-byte: a baked map per tinted material, none
/// for the proportional control.
#[test]
fn tinted_emissive_import_reproduces_committed_tree_in_default_mode() {
    let (out, output) = import_beside_copy("import_tinted", &tinted_root().join("scene.gltf"), &[]);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "the tinted fixture must import with no unsupported feature: {stderr}"
    );
    assert!(
        stderr.is_empty(),
        "a fully supported document warns about nothing: {stderr}"
    );
    assert_trees_identical(&tinted_root().join("expected"), &out);

    let mesh =
        |name: &str| std::fs::read_to_string(out.join(format!("meshes/{name}.mesh.yaml"))).unwrap();
    assert!(mesh("card-textured").contains("emissive_texture:"));
    assert!(mesh("card-constant").contains("emissive_texture:"));
    assert!(!mesh("card-proportional").contains("emissive_texture:"));
    for rel in [
        "model.prefab.yaml",
        "meshes/card-textured.mesh.yaml",
        "meshes/card-constant.mesh.yaml",
        "meshes/card-proportional.mesh.yaml",
    ] {
        let check = run_ludion(&["check-scene", out.join(rel).to_str().unwrap()]);
        assert!(
            check.status.success(),
            "check-scene failed for {rel}: {}",
            String::from_utf8_lossy(&check.stderr)
        );
    }
}

/// The gallery hinge prefab is a copy of the fixture's expected tree with only `generated_from`
/// re-pointed, so it cannot drift from what the importer produces.
#[test]
fn the_committed_gallery_hinge_matches_the_node_clips_fixture() {
    let gallery =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/gallery/prefabs/hinge");
    let expected = node_clips_root().join("expected");
    let mut gallery_files = collect_files(&gallery);
    let mut expected_files = collect_files(&expected);
    gallery_files.sort();
    expected_files.sort();
    assert_eq!(
        gallery_files, expected_files,
        "the two trees hold different files"
    );
    let without_origin = |path: PathBuf| {
        std::fs::read_to_string(path)
            .unwrap()
            .lines()
            .filter(|line| !line.starts_with("generated_from:"))
            .collect::<Vec<_>>()
            .join(
                "
",
            )
    };
    for rel in &expected_files {
        assert_eq!(
            without_origin(gallery.join(rel)),
            without_origin(expected.join(rel)),
            "gallery `{}` drifted from the fixture",
            rel.display()
        );
    }
}

/// The hinge fixture (`examples/import-node-clips/`) imports in **default mode** — no
/// `--allow-lossy`, no warning — and reproduces its committed tree byte-for-byte: the prefab lists
/// its two part clips in `clips:`, and the STEP clip keeps `ease: step`.
#[test]
fn node_clips_import_reproduces_committed_tree_in_default_mode() {
    let (out, output) = import_beside_copy(
        "import_node_clips",
        &node_clips_root().join("scene.gltf"),
        &[],
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "the hinge fixture must import with no unsupported feature: {stderr}"
    );
    assert!(
        stderr.is_empty(),
        "a fully supported document warns about nothing: {stderr}"
    );
    assert_trees_identical(&node_clips_root().join("expected"), &out);

    let read = |rel: &str| std::fs::read_to_string(out.join(rel)).unwrap();
    let prefab = read("model.prefab.yaml");
    assert!(prefab.contains(
        "clips:
  open: clips/open.anim.yaml
  snap: clips/snap.anim.yaml
"
    ));
    assert!(read("clips/snap.anim.yaml").contains("ease: step"));
    for rel in [
        "model.prefab.yaml",
        "clips/open.anim.yaml",
        "clips/snap.anim.yaml",
    ] {
        let check = run_ludion(&["check-scene", out.join(rel).to_str().unwrap()]);
        assert!(
            check.status.success(),
            "check-scene failed for {rel}: {}",
            String::from_utf8_lossy(&check.stderr)
        );
    }
}

/// **Tinted-emissive radiance (GPU).** The imported `card-constant` (baked 1×1 map, `Mesh.color` the
/// base, `Mesh.emissive` the intensity) must render what the glTF asked for: `emissiveFactor`.
/// The reference is the same card with no map and the factor spelled directly as `color / emissive`.
#[test]
fn needs_gpu_tinted_emissive_renders_texture_times_factor() {
    let (out, import) = import_beside_copy(
        "import_tinted_radiance_tree",
        &tinted_root().join("scene.gltf"),
        &[],
    );
    assert!(
        import.status.success(),
        "import failed: {}",
        String::from_utf8_lossy(&import.stderr)
    );
    let mesh_text = std::fs::read_to_string(out.join("meshes/card-constant.mesh.yaml")).unwrap();
    let mut baked = ludion_scene::parse_mesh(&mesh_text).expect("the imported mesh parses");
    // Only emission may reach the pixels: no albedo map, no lights, a black sky.
    baked.textures.base_color = None;
    let mut reference = baked.clone();
    reference.textures.emissive = None;
    std::fs::write(
        out.join("meshes/card-baked.mesh.yaml"),
        ludion_scene::emit_mesh(&baked),
    )
    .unwrap();
    std::fs::write(
        out.join("meshes/card-reference.mesh.yaml"),
        ludion_scene::emit_mesh(&reference),
    )
    .unwrap();

    // The `emissiveFactor` the fixture authors on `card-constant`'s material.
    let factor = [0.25_f64, 0.5, 2.0];
    let peak = factor.iter().copied().fold(0.0, f64::max);
    // Scene files take block-style sequences only.
    let seq = |indent: usize, values: &[f64]| -> String {
        values
            .iter()
            .map(|v| {
                format!(
                    "{}- {v:?}
",
                    " ".repeat(indent)
                )
            })
            .collect()
    };
    let scene = |asset: &str, color: [f64; 3], emissive: f64| {
        let origin = [0.0, 0.0, 0.0];
        let unit = [1.0, 1.0, 1.0];
        let rgba = [color[0], color[1], color[2], 1.0];
        format!(
            "format: 1
name: radiance
seed: 0
entities:
  - name: card
    components:
      Transform:
        translation:
{}        rotation:
{}        scale:
{}      Mesh:
        shape:
          asset: {asset}
        color:
{}        roughness: 0.75
        metallic: 0.0
        emissive: {emissive:?}
  - name: sky
    components:
      Environment:
        sky_zenith:
{}        sky_horizon:
{}        sky_ground:
{}  - name: camera
    components:
      Transform:
        translation:
{}        rotation:
{}        scale:
{}      Camera:
        projection:
          type: perspective
          fov_y_deg: 45.836624
        near: 0.1
        far: 100.0
",
            seq(10, &origin),
            seq(10, &origin),
            seq(10, &unit),
            seq(10, &rgba),
            seq(10, &origin),
            seq(10, &origin),
            seq(10, &origin),
            seq(10, &[0.0, 0.0, 2.0]),
            seq(10, &origin),
            seq(10, &unit),
        )
    };
    // The imported instance: the prefab's `Mesh` values for `card-constant`.
    let imported_color = [0.5, 0.5, 1.0];
    let imported_emissive = 2.0;
    let render = |name: &str, text: String| -> Result<Vec<u8>, Output> {
        let scene_path = out.join(format!("{name}.yaml"));
        std::fs::write(&scene_path, text).unwrap();
        let png = scratch(&format!("import_tinted_radiance_{name}")).join("out.png");
        let output = run_ludion(&[
            "render",
            "--scene",
            scene_path.to_str().unwrap(),
            "--width",
            "128",
            "--height",
            "128",
            "--out",
            png.to_str().unwrap(),
        ]);
        if output.status.success() {
            Ok(std::fs::read(&png).unwrap())
        } else {
            Err(output)
        }
    };
    let baked_png = match render(
        "baked",
        scene(
            "meshes/card-baked.mesh.yaml",
            imported_color,
            imported_emissive,
        ),
    ) {
        Ok(png) => png,
        Err(output) if is_no_adapter(&output) => {
            eprintln!("SKIPPED needs_gpu_tinted_emissive: no GPU adapter");
            return;
        }
        Err(output) => panic!("render failed: {}", String::from_utf8_lossy(&output.stderr)),
    };
    let reference_png = render(
        "reference",
        scene(
            "meshes/card-reference.mesh.yaml",
            factor.map(|f| f / peak),
            peak,
        ),
    )
    .unwrap_or_else(|o| panic!("render failed: {}", String::from_utf8_lossy(&o.stderr)));

    let decode = |png: &[u8]| ludion_assets::decode_png(png, "render").unwrap();
    let (width, height, a) = decode(&baked_png);
    let (_, _, b) = decode(&reference_png);
    let mut worst = 0_i32;
    let mut lit = 0;
    for y in height / 3..2 * height / 3 {
        for x in width / 3..2 * width / 3 {
            let i = ((y * width + x) * 4) as usize;
            if a[i..i + 3].iter().any(|&v| v > 8) {
                lit += 1;
            }
            for c in 0..3 {
                worst = worst.max((i32::from(a[i + c]) - i32::from(b[i + c])).abs());
            }
        }
    }
    assert!(lit > 0, "the card must be visibly emissive in the centre");
    assert!(
        worst <= 2,
        "the baked tint differs from the factor by {worst}/255"
    );
}

/// The cutout fixture (`examples/import-cutout/`, exported from Blender) imports in **default
/// mode** and reproduces its committed tree byte-for-byte: a `MASK` grating carrying its cutoff, and
/// three `doubleSided` meshes (flap, tape strip, open lamp shade).
#[test]
fn cutout_import_reproduces_committed_tree_in_default_mode() {
    let (out, output) = import_beside_copy("import_cutout", &cutout_root().join("scene.gltf"), &[]);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "the cutout fixture must import with no unsupported feature: {stderr}"
    );
    assert!(
        stderr.is_empty(),
        "a fully supported document warns about nothing: {stderr}"
    );
    assert_trees_identical(&cutout_root().join("expected"), &out);

    let mesh =
        |name: &str| std::fs::read_to_string(out.join(format!("meshes/{name}.mesh.yaml"))).unwrap();
    let grating = mesh("grating");
    assert!(grating.contains("alpha_mode: mask\n"), "{grating}");
    assert!(grating.contains("alpha_cutoff: 0.25\n"), "{grating}");
    assert!(!grating.contains("double_sided"), "{grating}");
    for name in ["flap", "tape", "shade"] {
        let text = mesh(name);
        assert!(text.contains("double_sided: true\n"), "{name}: {text}");
        assert!(!text.contains("alpha_mode"), "{name}: {text}");
    }

    for rel in [
        "model.prefab.yaml",
        "meshes/grating.mesh.yaml",
        "meshes/flap.mesh.yaml",
        "meshes/tape.mesh.yaml",
        "meshes/shade.mesh.yaml",
    ] {
        let check = run_ludion(&["check-scene", out.join(rel).to_str().unwrap()]);
        assert!(
            check.status.success(),
            "check-scene failed for {rel}: {}",
            String::from_utf8_lossy(&check.stderr)
        );
    }
}

/// Default (non-lossy) mode fails on the fixture's alpha-blended material, naming it, and writes
/// nothing.
#[test]
fn default_mode_fails_with_l1202_and_writes_nothing() {
    let out = scratch("import_default");
    let output = run_ludion(&[
        "import",
        fixture().to_str().unwrap(),
        "--out",
        out.to_str().unwrap(),
    ]);
    assert!(
        !output.status.success(),
        "an alpha-blended material must fail a non-lossy import"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("error[L1202]"), "expected L1202: {stderr}");
    assert!(
        stderr.contains("pedestal-mesh"),
        "L1202 must name the offender: {stderr}"
    );
    assert!(
        !out.join("model.prefab.yaml").exists(),
        "a failed import must not write a partial tree"
    );
}

/// A tint over a base channel that is zero cannot be baked: strict import fails `L1202` naming the
/// material and the channel and writes nothing; `--allow-lossy` imports the part with no emissive.
#[test]
fn refused_zero_base_tint_is_l1202_strict_and_drops_only_the_emissive_when_lossy() {
    let gltf = tinted_root().join("refused-zero-base.gltf");
    let out = scratch("import_refused_zero_base");
    let strict = run_ludion(&[
        "import",
        gltf.to_str().unwrap(),
        "--out",
        out.to_str().unwrap(),
    ]);
    assert!(!strict.status.success());
    let stderr = String::from_utf8_lossy(&strict.stderr);
    assert!(stderr.contains("error[L1202]"), "{stderr}");
    assert!(
        stderr.contains("refused-zero-base-material") && stderr.contains("green"),
        "{stderr}"
    );
    assert!(!out.join("model.prefab.yaml").exists());

    let lossy = run_ludion(&[
        "import",
        gltf.to_str().unwrap(),
        "--out",
        out.to_str().unwrap(),
        "--allow-lossy",
    ]);
    assert!(
        lossy.status.success(),
        "{}",
        String::from_utf8_lossy(&lossy.stderr)
    );
    let prefab = std::fs::read_to_string(out.join("model.prefab.yaml")).unwrap();
    assert!(
        prefab.contains(
            "emissive: 0.0
"
        ),
        "{prefab}"
    );
}

/// `--allow-lossy` compiles the fixture and reproduces the committed expected tree byte-for-byte —
/// proving both compiler determinism and that the committed tree is up to date.
#[test]
fn lossy_import_reproduces_committed_tree() {
    let (out, output) = import_beside_copy("import_lossy", &fixture(), &["--allow-lossy"]);
    assert!(
        output.status.success(),
        "lossy import failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("warning[L1202]"),
        "the blend drop must be a printed warning in lossy mode"
    );
    assert_trees_identical(&expected_dir(), &out);
    assert!(
        !out.join(WRAPPER_SCENE).exists(),
        "an import writes a prefab, never a scene"
    );
    let prefab = std::fs::read_to_string(out.join("model.prefab.yaml")).unwrap();
    assert!(
        prefab
            .lines()
            .any(|line| line == r#"generated_from: "../scene.gltf""#),
        "the prefab names its source: {prefab}"
    );
}

/// The committed imported prefab, its wrapper scene and its mesh docs pass `ludion check-scene`.
#[test]
fn imported_tree_passes_check_scene() {
    for rel in [
        "model.prefab.yaml",
        "scene.yaml",
        "meshes/pedestal-mesh.mesh.yaml",
        "meshes/gem-mesh.mesh.yaml",
    ] {
        let path = expected_dir().join(rel);
        let output = run_ludion(&["check-scene", path.to_str().unwrap()]);
        assert!(
            output.status.success(),
            "check-scene failed for {rel}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

/// **Skeletal Animation W1 exit.** The committed skinned mannequin imports clean — no
/// `--allow-lossy` — check-scene passes on the scene and the mesh, and `--bounds` (text and
/// `--json`) reads the rig back as the goblin's 11-joint tree.
#[test]
fn skinned_mannequin_imports_clean() {
    let glb = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../ludion-import/tests/fixtures/skin-mannequin.glb");
    let out = scratch("skinned-mannequin-import");
    let output = run_ludion(&[
        "import",
        glb.to_str().unwrap(),
        "--out",
        out.to_str().unwrap(),
    ]);
    assert!(
        output.status.success() && output.stderr.is_empty(),
        "import must be clean, stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        !out.join(WRAPPER_SCENE).exists(),
        "an import writes a prefab, never a scene"
    );
    let prefab = out.join("model.prefab.yaml");
    let mesh = out.join("meshes/skin-mannequin.mesh.yaml");
    for path in [&prefab, &mesh] {
        let output = run_ludion(&["check-scene", path.to_str().unwrap()]);
        assert!(
            output.status.success(),
            "check-scene failed for {path:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    let output = run_ludion(&["check-scene", mesh.to_str().unwrap(), "--bounds"]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    for line in [
        "joints: 11 (rest heads, model space)",
        "pelvis (0.000, 0.520, 0.000)",
        "head (0.000, 0.860, 0.000)",
    ] {
        assert!(stdout.contains(line), "missing `{line}` in: {stdout}");
    }
    // The root joint carries no parent frame; a child's line carries its head restated in its
    // parent's frame too.
    let pelvis_line = stdout
        .lines()
        .find(|line| line.contains("pelvis ("))
        .expect("a pelvis line");
    assert!(
        !pelvis_line.contains("in parent"),
        "the root joint has no parent frame: {pelvis_line}"
    );
    let head_line = stdout
        .lines()
        .find(|line| line.contains("head ("))
        .expect("a head line");
    assert!(
        head_line.contains("in parent ("),
        "a child joint's line must restate its head in its parent's frame: {head_line}"
    );

    let output = run_ludion(&["check-scene", mesh.to_str().unwrap(), "--bounds", "--json"]);
    assert!(output.status.success());
    let report: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("--bounds --json prints one JSON object");
    let joints = report["joints"].as_array().expect("joints array");
    assert_eq!(joints.len(), 11);
    assert_eq!(joints[0]["name"], "pelvis");
    assert!(joints[0]["parent"].is_null());
    assert!(
        joints[0]["head_in_parent"].is_null(),
        "the root joint has no parent frame"
    );
    let torso = joints.iter().find(|j| j["name"] == "torso").expect("torso");
    assert_eq!(torso["parent"], 0);
    assert!(
        torso["head_in_parent"].is_array(),
        "a child joint's head_in_parent must be present: {torso:?}"
    );
}

/// **Skeletal Animation W3 mesh drift gate.** `examples/gallery/meshes/skin-mannequin.mesh.yaml` (the
/// `ragdoll-skinned` gallery scene's mesh asset) is a straight copy of a fresh import — a fresh
/// `skin-mannequin.glb` import must reproduce it byte for byte.
#[test]
fn the_committed_gallery_skin_mannequin_mesh_matches_a_fresh_import() {
    let glb = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../ludion-import/tests/fixtures/skin-mannequin.glb");
    let out = scratch("skin-mannequin-gallery-drift");
    let output = run_ludion(&[
        "import",
        glb.to_str().unwrap(),
        "--out",
        out.to_str().unwrap(),
    ]);
    assert!(
        output.status.success() && output.stderr.is_empty(),
        "import must be clean, stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let fresh = std::fs::read_to_string(out.join("meshes/skin-mannequin.mesh.yaml"))
        .expect("the fresh import's mesh asset");
    let committed = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../examples/gallery/meshes/skin-mannequin.mesh.yaml");
    let committed = std::fs::read_to_string(&committed).expect("the committed gallery mesh asset");
    assert_eq!(
        fresh, committed,
        "examples/gallery/meshes/skin-mannequin.mesh.yaml has drifted from a fresh import of \
         skin-mannequin.glb; re-copy the fresh import's meshes/skin-mannequin.mesh.yaml over it"
    );
}

/// **Animation Clips W1 clip drift gate.** `examples/gallery/clips/*.anim.yaml` are straight copies
/// of a fresh import of `skin-mannequin-clips.glb`, and each still names that glb as its source.
/// To refresh after re-keying: import the glb into `target/mannequin-clips` (the same depth as
/// `examples/gallery`, so `generated_from` reads identically), then copy its `clips/*.anim.yaml`
/// over `examples/gallery/clips/`.
#[test]
fn the_committed_gallery_mannequin_clips_match_a_fresh_import() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let glb = root.join("crates/ludion-import/tests/fixtures/skin-mannequin-clips.glb");
    let out = scratch("mannequin-clips-drift");
    let output = run_ludion(&[
        "import",
        glb.to_str().unwrap(),
        "--out",
        out.to_str().unwrap(),
    ]);
    assert!(
        output.status.success() && output.stderr.is_empty(),
        "import must be clean, stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let names = |dir: &Path| {
        let mut names: Vec<String> = std::fs::read_dir(dir)
            .expect("a clips directory")
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .filter(|n| n.ends_with(".anim.yaml"))
            .collect();
        names.sort();
        names
    };
    let committed = root.join("examples/gallery/clips");
    let fix = "re-run the import into target/mannequin-clips and re-copy its \
               clips/*.anim.yaml over examples/gallery/clips/";
    assert_eq!(
        names(&out.join("clips")),
        names(&committed),
        "the committed clips are not the glb's Actions; {fix}"
    );
    for name in names(&committed) {
        let clip = committed.join(&name);
        let output = run_ludion(&["check-scene", "--json", clip.to_str().unwrap()]);
        assert!(output.status.success(), "{name} must check clean");
        let report: serde_json::Value =
            serde_json::from_slice(&output.stdout).expect("check-scene --json output");
        assert_eq!(report["source"]["status"], "ok", "{name}: {report}");
        assert_eq!(
            report["source"]["stale"], false,
            "{name} has drifted from a fresh import of skin-mannequin-clips.glb; {fix}"
        );
    }
}

/// Whether a failed `render` was due to a missing GPU adapter (L2003), in which case the test skips
/// — copied from `tests/render.rs`'s idiom rather than shared, since each test binary is its own
/// crate.
fn is_no_adapter(output: &Output) -> bool {
    ludion_core::require_gpu::skip_no_adapter(
        String::from_utf8_lossy(&output.stderr).contains("error[L2003]"),
    )
}

/// **Skeletal Animation W2 exit.** The mannequin skinned at rest renders like the same mesh with its
/// skin stripped (`rest_model × inverse_bind` is identity, so the palette must be a no-op) at SSIM
/// `0.999`; a copy whose root joint's `inverse_bind` is nudged away from identity does **not** pass —
/// the negative control proving the palette actually reaches the GPU on this path. Skips gracefully
/// only when no GPU adapter exists.
#[test]
fn needs_gpu_skinned_mannequin_at_rest_renders_like_its_unskinned_copy() {
    let glb = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../ludion-import/tests/fixtures/skin-mannequin.glb");
    let import_out = scratch("skin-at-rest-import");
    let import = run_ludion(&[
        "import",
        glb.to_str().unwrap(),
        "--out",
        import_out.to_str().unwrap(),
    ]);
    assert!(
        import.status.success(),
        "import must be clean, stderr: {}",
        String::from_utf8_lossy(&import.stderr)
    );
    let mesh_text = std::fs::read_to_string(import_out.join("meshes/skin-mannequin.mesh.yaml"))
        .expect("the imported mesh document exists");

    let skinned_dir = scratch("skin-at-rest-skinned");
    std::fs::write(skinned_dir.join("skin-mannequin.mesh.yaml"), &mesh_text).unwrap();

    let mut stripped_doc = ludion_scene::parse_mesh(&mesh_text).expect("the imported mesh parses");
    stripped_doc.skin = None;
    let stripped_dir = scratch("skin-at-rest-stripped");
    std::fs::write(
        stripped_dir.join("skin-mannequin.mesh.yaml"),
        ludion_scene::emit_mesh(&stripped_doc),
    )
    .unwrap();

    let mut broken_doc = ludion_scene::parse_mesh(&mesh_text).expect("the imported mesh parses");
    broken_doc
        .skin
        .as_mut()
        .expect("the mesh is skinned")
        .joints[0]
        .inverse_bind[12] += 0.3;
    let broken_dir = scratch("skin-at-rest-broken");
    std::fs::write(
        broken_dir.join("skin-mannequin.mesh.yaml"),
        ludion_scene::emit_mesh(&broken_doc),
    )
    .unwrap();

    let render = |dir: &Path, scratch_name: &str| -> Result<Vec<u8>, Output> {
        let out = scratch(scratch_name).join("skin-mannequin.sheet.png");
        let output = run_ludion(&[
            "render",
            "--model",
            dir.join("skin-mannequin.mesh.yaml").to_str().unwrap(),
            "--views",
            "front",
            "--width",
            "256",
            "--height",
            "256",
            "--out",
            out.to_str().unwrap(),
        ]);
        if output.status.success() {
            Ok(std::fs::read(&out).unwrap())
        } else {
            Err(output)
        }
    };

    let skinned_png = match render(&skinned_dir, "skin-at-rest-skinned-out") {
        Ok(png) => png,
        Err(output) if is_no_adapter(&output) => {
            eprintln!(
                "SKIPPED skinned_mannequin_at_rest_renders_like_its_unskinned_copy: no GPU adapter"
            );
            return;
        }
        Err(output) => panic!("render failed: {}", String::from_utf8_lossy(&output.stderr)),
    };
    let stripped_png = render(&stripped_dir, "skin-at-rest-stripped-out")
        .unwrap_or_else(|o| panic!("render failed: {}", String::from_utf8_lossy(&o.stderr)));
    let broken_png = render(&broken_dir, "skin-at-rest-broken-out")
        .unwrap_or_else(|o| panic!("render failed: {}", String::from_utf8_lossy(&o.stderr)));

    let positive = ludion_testkit::compare_png_bytes(&stripped_png, &skinned_png, 0.999)
        .expect("both PNGs decode");
    assert!(
        positive.pass,
        "skinned-at-rest must match its unskinned copy at 0.999: {}",
        positive.summary
    );

    let negative = ludion_testkit::compare_png_bytes(&stripped_png, &broken_png, 0.999)
        .expect("both PNGs decode");
    assert!(
        !negative.pass && negative.damaged_tiles.len() >= 20,
        "a broken inverse bind must change the picture by more than a faint drift — the palette \
         must reach the GPU: {}",
        negative.summary
    );
}

/// **Wave 6 mechanized exit gate (GPU).** The committed fixture: import (`--allow-lossy`) →
/// check-scene green → render matches the blessed golden → `scene.load` then `scene.save` is
/// byte-equal (canonical re-export) → re-running import is byte-identical to the committed expected
/// tree. Skips gracefully only when no GPU adapter exists (matching the gallery golden suite).
#[test]
fn needs_gpu_exit_gate() {
    let platform = match ludion_testkit::platform_dir() {
        Ok(platform) => platform,
        Err(diag) => {
            eprintln!(
                "SKIPPED exit_gate:
{diag}"
            );
            return;
        }
    };
    // 1. Import the fixture fresh and confirm it reproduces the committed expected tree.
    let (out, import) = import_beside_copy("import_exit_gate", &fixture(), &["--allow-lossy"]);
    assert!(
        import.status.success(),
        "import failed: {}",
        String::from_utf8_lossy(&import.stderr)
    );
    assert_trees_identical(&expected_dir(), &out);

    // 2. The freshly imported prefab + meshes pass check-scene.
    for rel in [
        "model.prefab.yaml",
        "meshes/pedestal-mesh.mesh.yaml",
        "meshes/gem-mesh.mesh.yaml",
    ] {
        let output = run_ludion(&["check-scene", out.join(rel).to_str().unwrap()]);
        assert!(
            output.status.success(),
            "check-scene failed for {rel}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    // 3. Render the committed scene and compare against the blessed golden (960×540, SSIM ≥ 0.99).
    if assert_matches_golden(
        &expected_dir().join("scene.yaml"),
        &golden(platform),
        "import_golden_out",
        "exit_gate",
    ) {
        return;
    }

    // 4. scene.load → scene.save is a canonical byte-exact fixed point (the imported scene is
    //    already canonical, so re-exporting it through the engine reproduces it exactly).
    assert_load_save_roundtrip(&expected_dir().join("scene.yaml"), "import_roundtrip");
}

/// **Textured exit gate (GPU).** The `examples/import-textured/` fixture's counterpart to
/// `exit_gate`: import (default mode — the fixture is fully supported, no `--allow-lossy`) →
/// check-scene green on both mesh documents → render matches the blessed golden, both cubes
/// sampling their five texture maps → `scene.load` then `scene.save` is byte-equal. Skips
/// gracefully only when no GPU adapter exists (matching `exit_gate`).
#[test]
fn needs_gpu_textured_exit_gate() {
    let platform = match ludion_testkit::platform_dir() {
        Ok(platform) => platform,
        Err(diag) => {
            eprintln!(
                "SKIPPED textured_exit_gate:
{diag}"
            );
            return;
        }
    };
    // 1. Import the fixture fresh and confirm it reproduces the committed expected tree.
    let (out, import) = import_beside_copy(
        "import_textured_exit_gate",
        &textured_root().join("scene.gltf"),
        &[],
    );
    assert!(
        import.status.success(),
        "import failed: {}",
        String::from_utf8_lossy(&import.stderr)
    );
    assert_trees_identical(&textured_root().join("expected"), &out);

    // 2. The freshly imported prefab + meshes pass check-scene.
    for rel in [
        "model.prefab.yaml",
        "meshes/tangent-cube.mesh.yaml",
        "meshes/computed-cube.mesh.yaml",
    ] {
        let output = run_ludion(&["check-scene", out.join(rel).to_str().unwrap()]);
        assert!(
            output.status.success(),
            "check-scene failed for {rel}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    // 3. Render the committed scene and compare against the blessed golden.
    if assert_matches_golden(
        &textured_root().join("expected").join("scene.yaml"),
        &textured_golden(platform),
        "import_textured_golden_out",
        "textured_exit_gate",
    ) {
        return;
    }

    // 4. scene.load → scene.save is a canonical byte-exact fixed point.
    assert_load_save_roundtrip(
        &textured_root().join("expected").join("scene.yaml"),
        "import_textured_roundtrip",
    );
}

/// **Tinted-emissive exit gate (GPU).** Renders the textured card (ramp × hue) and the constant
/// card (1×1 hue map) beside the proportional control against the blessed golden, then checks
/// `scene.load` → `scene.save` is byte-equal. The import itself is pinned by
/// `tinted_emissive_import_reproduces_committed_tree_in_default_mode`.
#[test]
fn needs_gpu_tinted_emissive_exit_gate() {
    let platform = match ludion_testkit::platform_dir() {
        Ok(platform) => platform,
        Err(diag) => {
            eprintln!(
                "SKIPPED tinted_emissive_exit_gate:
{diag}"
            );
            return;
        }
    };
    let scene = tinted_root().join("expected").join("scene.yaml");
    if assert_matches_golden(
        &scene,
        &tinted_golden(platform),
        "import_tinted_golden_out",
        "tinted_emissive_exit_gate",
    ) {
        return;
    }
    assert_load_save_roundtrip(&scene, "import_tinted_roundtrip");
}

/// Renders `scene` and asserts it matches `golden` at exactly SSIM 1.0 (same-machine render vs its
/// own blessed golden must be byte-exact). Honors `LUDION_BLESS` (`bless_requested`) exactly like
/// `ludion-testkit/tests/gallery.rs`'s per-test bless arms: (re)writes the golden and returns.
/// Returns `true` when the caller should return early — either a fresh bless, or the no-GPU-adapter
/// skip (`L2003`) that matches the gallery golden suite's convention.
fn assert_matches_golden(scene: &Path, golden: &Path, scratch_name: &str, label: &str) -> bool {
    let test = GoldenTest::new(scene.to_path_buf(), golden.to_path_buf());

    if bless_requested() {
        return match test.bless() {
            Ok(GoldenOutcome::Blessed { golden }) => {
                eprintln!("blessed {} — review it, then commit it", golden.display());
                true
            }
            Ok(_) => unreachable!("bless() only ever returns Blessed"),
            Err(diagnostics) if ludion_core::require_gpu::is_no_adapter(&diagnostics) => {
                eprintln!("SKIPPED {label} bless: no GPU adapter");
                true
            }
            Err(diagnostics) => panic!("blessing the {label} golden failed: {diagnostics:?}"),
        };
    }

    let report = match test.run(&scratch(scratch_name)) {
        Ok(GoldenOutcome::Passed { report }) => report,
        Ok(GoldenOutcome::Failed { report, .. }) => {
            panic!("golden comparison failed: {}", report.summary)
        }
        Ok(GoldenOutcome::Blessed { .. }) => unreachable!("run() never blesses"),
        Err(diagnostics) if ludion_core::require_gpu::is_no_adapter(&diagnostics) => {
            eprintln!("SKIPPED {label} render: no GPU adapter");
            return true;
        }
        Err(diagnostics) => panic!("render failed: {diagnostics:?}"),
    };
    assert!(report.pass, "imported scene must match its blessed golden");
    assert_eq!(
        report.ssim,
        Some(1.0),
        "same-machine render vs its blessed golden must be byte-exact"
    );
    false
}

/// Asserts `scene.load` → `scene.save` is a canonical byte-exact fixed point for the already
/// canonical committed `scene_path`, loaded from a copy of its tree so the instance entry resolves.
fn assert_load_save_roundtrip(scene_path: &Path, scratch_name: &str) {
    let scene_text = std::fs::read_to_string(scene_path).unwrap();
    let root = scratch(scratch_name);
    let tree = scene_path.parent().unwrap();
    for rel in collect_files(tree) {
        let target = root.join(&rel);
        std::fs::create_dir_all(target.parent().unwrap()).unwrap();
        std::fs::copy(tree.join(&rel), target).unwrap();
    }
    let mut engine = Engine::with_project_root(&root);
    engine
        .load_inline_scene(&scene_text)
        .expect("the imported scene loads");
    dispatch(
        &mut engine,
        Command::SceneSave(SceneSaveParams {
            path: "out.yaml".into(),
        }),
    )
    .expect("scene.save succeeds");
    let saved = std::fs::read_to_string(root.join("out.yaml")).unwrap();
    assert_eq!(
        saved, scene_text,
        "scene.load → scene.save is not byte-identical"
    );
}

/// An imported node's children land at their parent's world pose, including the fan-out children a
/// multi-primitive mesh gets: the root sits at x = 2 and the child one metre above it.
#[test]
fn imported_children_land_at_their_parents_world_pose() {
    let dir = scratch("import-hierarchy-world-pose");
    let mut bytes = Vec::new();
    for value in [0.0_f32, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0] {
        bytes.extend_from_slice(&value.to_le_bytes());
    }
    std::fs::write(dir.join("tree.bin"), &bytes).unwrap();
    let primitive = r#"{"attributes":{"POSITION":0}}"#;
    let gltf = format!(
        r#"{{"asset":{{"version":"2.0"}},"nodes":[{{"name":"root","translation":[2,0,0],"children":[1]}},{{"name":"child","translation":[0,1,0],"mesh":0}}],"meshes":[{{"name":"tri","primitives":[{primitive},{primitive}]}}],"accessors":[{{"bufferView":0,"componentType":5126,"count":3,"type":"VEC3","min":[0.0,0.0,0.0],"max":[1.0,1.0,0.0]}}],"bufferViews":[{{"buffer":0,"byteOffset":0,"byteLength":36}}],"buffers":[{{"byteLength":36,"uri":"tree.bin"}}]}}"#
    );
    let input = dir.join("tree.gltf");
    std::fs::write(&input, gltf).unwrap();
    let out = dir.join("out");
    let output = run_ludion(&[
        "import",
        input.to_str().unwrap(),
        "--out",
        out.to_str().unwrap(),
    ]);
    assert!(
        output.status.success(),
        "import failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let mut engine = Engine::with_project_root(&out);
    engine
        .load_inline_scene("format: 1\nentities:\n  - name: kit\n    prefab: model.prefab.yaml\n")
        .expect("the wrapper scene loads");
    let world = engine.world_mut();
    let named: Vec<(ludion_core::ids::EntityId, String)> = world
        .query_ids::<(&ludion_engine::Name,)>()
        .map(|(id, (name,))| (id, name.0.clone()))
        .collect();
    let names: Vec<&str> = named.iter().map(|(_, name)| name.as_str()).collect();
    let mut checked = 0;
    for (id, name) in &named {
        let want = if name == "kit/root" {
            [2.0, 0.0, 0.0]
        } else if name.starts_with("kit/") && name.contains("child") {
            [2.0, 1.0, 0.0]
        } else {
            continue;
        };
        let got = ludion_engine::hierarchy::world_transform(world, *id)
            .unwrap()
            .trs
            .translation;
        for (got, want) in got.iter().zip(want) {
            assert!(
                (got - want).abs() < 1e-5,
                "{name}: {got:?} vs {want:?}; entities: {names:?}"
            );
        }
        checked += 1;
    }
    assert!(
        checked >= 3,
        "the root and both fan-out children expected; entities: {names:?}"
    );
}

/// Writes the synthetic slide glTF (`slide.gltf` + `slide.bin`) into `dir` and returns its path: a
/// skinned triangle whose root joint slides 1 m along X in one second, one looping clip.
fn write_slide_gltf(dir: &Path) -> PathBuf {
    let mut bytes = Vec::new();
    for value in [0.0_f32, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0] {
        bytes.extend_from_slice(&value.to_le_bytes());
    }
    bytes.extend([0_u8, 1, 0, 0].repeat(3));
    for value in [0.2_f32, 0.6, 0.0, 0.0].repeat(3) {
        bytes.extend_from_slice(&value.to_le_bytes());
    }
    for value in [0.0_f32, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0] {
        bytes.extend_from_slice(&value.to_le_bytes());
    }
    std::fs::write(dir.join("slide.bin"), &bytes).unwrap();
    let gltf = r#"{"asset":{"version":"2.0"},"nodes":[{"name":"body","mesh":0,"skin":0},{"name":"root","children":[2]},{"name":"tip","translation":[0,1,0]}],"skins":[{"joints":[1,2]}],"meshes":[{"name":"tri","primitives":[{"attributes":{"POSITION":0,"JOINTS_0":1,"WEIGHTS_0":2}}]}],"accessors":[{"bufferView":0,"componentType":5126,"count":3,"type":"VEC3","min":[0.0,0.0,0.0],"max":[1.0,1.0,0.0]},{"bufferView":1,"componentType":5121,"count":3,"type":"VEC4"},{"bufferView":2,"componentType":5126,"count":3,"type":"VEC4"},{"bufferView":3,"componentType":5126,"count":2,"type":"SCALAR"},{"bufferView":4,"componentType":5126,"count":2,"type":"VEC3"}],"bufferViews":[{"buffer":0,"byteOffset":0,"byteLength":36},{"buffer":0,"byteOffset":36,"byteLength":12},{"buffer":0,"byteOffset":48,"byteLength":48},{"buffer":0,"byteOffset":96,"byteLength":8},{"buffer":0,"byteOffset":104,"byteLength":24}],"buffers":[{"byteLength":128,"uri":"slide.bin"}],"animations":[{"name":"slide","channels":[{"sampler":0,"target":{"node":1,"path":"translation"}}],"samplers":[{"input":3,"output":4}],"extras":{"loop":true}}]}"#;
    let input = dir.join("slide.gltf");
    std::fs::write(&input, gltf).unwrap();
    input
}

/// An animated glTF imports as `clips/<name>.anim.yaml` stamped with `generated_from`, and the
/// clip passes `ludion check-scene`. The document is a skinned triangle whose root joint slides 1 m
/// along X in one second; it is written to a scratch directory, which is also `--out`, so the
/// clip's source is one directory up.
#[test]
fn an_animated_gltf_imports_a_clip() {
    let dir = scratch("animated-gltf-import");
    let input = write_slide_gltf(&dir);
    let output = run_ludion(&[
        "import",
        input.to_str().unwrap(),
        "--out",
        dir.to_str().unwrap(),
    ]);
    assert!(
        output.status.success() && output.stderr.is_empty(),
        "import must be clean, stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains("1 clip"));
    let clip = dir.join("clips/slide.anim.yaml");
    let text = std::fs::read_to_string(&clip).expect("the clip is written");
    assert!(
        text.contains(r#"generated_from: "../slide.gltf""#),
        "{text}"
    );
    assert!(text.contains("loop: true"), "{text}");
    let check = run_ludion(&["check-scene", clip.to_str().unwrap()]);
    assert!(
        check.status.success(),
        "check-scene failed: {}",
        String::from_utf8_lossy(&check.stderr)
    );
}

/// Imports the slide glTF into a fresh scratch directory and returns the directory and its clip.
fn import_slide(name: &str) -> (PathBuf, PathBuf) {
    let dir = scratch(name);
    let input = write_slide_gltf(&dir);
    let output = run_ludion(&[
        "import",
        input.to_str().unwrap(),
        "--out",
        dir.to_str().unwrap(),
    ]);
    assert!(output.status.success(), "import failed");
    let clip = dir.join("clips/slide.anim.yaml");
    (dir, clip)
}

/// `check-scene` on a derived clip re-imports its source: before/after counts, the worst
/// model-space error per joint, the model-space seam, and no stale note for a fresh file.
#[test]
fn clip_report_re_imports_the_source() {
    let (_dir, clip) = import_slide("clip-report-source");
    let output = run_ludion(&["check-scene", clip.to_str().unwrap()]);
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("source: ../slide.gltf"), "{stdout}");
    assert!(stdout.contains("(of "), "{stdout}");
    assert!(stdout.contains("root: rotation"), "{stdout}");
    assert!(
        stdout.contains("seam (model space): rotation 0.0000°, translation 1.00000"),
        "{stdout}"
    );
    assert!(!stdout.contains("differs from a fresh import"), "{stdout}");
}

/// `--json` carries the same source report.
#[test]
fn clip_report_json_has_the_source() {
    let (_dir, clip) = import_slide("clip-report-json");
    let output = run_ludion(&["check-scene", clip.to_str().unwrap(), "--json"]);
    assert!(output.status.success());
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).expect("one JSON object");
    assert_eq!(value["source"]["status"], "ok", "{value}");
    assert_eq!(value["source"]["stale"], false, "{value}");
    assert!(!value["source"]["joints"].as_array().unwrap().is_empty());
}

/// A missing source is a note, not an error.
#[test]
fn clip_report_notes_a_missing_source() {
    let (dir, clip) = import_slide("clip-report-missing");
    std::fs::remove_file(dir.join("slide.gltf")).unwrap();
    std::fs::remove_file(dir.join("slide.bin")).unwrap();
    let output = run_ludion(&["check-scene", clip.to_str().unwrap()]);
    assert!(output.status.success());
    assert!(String::from_utf8_lossy(&output.stdout).contains("not found"));
    let json = run_ludion(&["check-scene", clip.to_str().unwrap(), "--json"]);
    assert!(json.status.success());
    let value: serde_json::Value = serde_json::from_slice(&json.stdout).unwrap();
    assert_eq!(value["source"]["status"], "missing", "{value}");
}

/// A clip edited after import is flagged stale.
#[test]
fn clip_report_flags_a_stale_clip() {
    let (_dir, clip) = import_slide("clip-report-stale");
    let text = std::fs::read_to_string(&clip).unwrap();
    assert!(text.contains("loop: true"));
    std::fs::write(&clip, text.replace("loop: true", "loop: false")).unwrap();
    let output = run_ludion(&["check-scene", clip.to_str().unwrap()]);
    assert!(output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stdout).contains("differs from a fresh import"),
        "{}",
        String::from_utf8_lossy(&output.stdout)
    );
    let json = run_ludion(&["check-scene", clip.to_str().unwrap(), "--json"]);
    let value: serde_json::Value = serde_json::from_slice(&json.stdout).unwrap();
    assert_eq!(value["source"]["stale"], true, "{value}");
}

/// An imported prefab placed from a level elsewhere in the project resolves its mesh and texture
/// paths against the prefab's own directory, with no override: `check-scene` accepts the level and a
/// render-side extract reads every file; a renamed mesh is an `L1101` naming the resolved path.
#[test]
fn an_imported_prefab_placed_in_a_subdirectory_finds_its_assets() {
    let dir = scratch("import_prefab_relative");
    let copy = dir.join("scene.gltf");
    std::fs::copy(textured_root().join("scene.gltf"), &copy).unwrap();
    let out = dir.join("models/crate");
    let output = run_ludion(&[
        "import",
        copy.to_str().unwrap(),
        "--out",
        out.to_str().unwrap(),
    ]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let wrapper = std::fs::read_to_string(textured_root().join("expected/scene.yaml")).unwrap();
    let level = wrapper.replace(
        "prefab: model.prefab.yaml",
        "prefab: models/crate/model.prefab.yaml",
    );
    assert_ne!(level, wrapper);
    let level_path = dir.join("level.yaml");
    std::fs::write(&level_path, level).unwrap();

    let check = run_ludion(&["check-scene", level_path.to_str().unwrap()]);
    let stderr = String::from_utf8_lossy(&check.stderr);
    assert!(check.status.success(), "{stderr}");
    assert!(!stderr.contains("L1101"), "{stderr}");

    // `check-scene` does not read an instance's mesh files; a render-side extract does.
    let load = || {
        let mut engine = Engine::with_project_root(&dir);
        engine
            .load_inline_scene(&std::fs::read_to_string(&level_path).unwrap())
            .expect("the level loads");
        ludion_engine::extract(
            engine.world(),
            (320, 180),
            ludion_assets::AssetRoot::content(&dir),
        )
    };
    load().unwrap_or_else(|errors| panic!("every asset resolves: {errors:?}"));

    let mesh = out.join("meshes/tangent-cube.mesh.yaml");
    std::fs::rename(&mesh, out.join("meshes/moved.mesh.yaml")).unwrap();
    let errors = load().unwrap_err();
    let found = errors
        .iter()
        .find(|d| d.code.as_str() == "L1101")
        .unwrap_or_else(|| panic!("{errors:?}"));
    assert!(
        found
            .message
            .contains("models/crate/meshes/tangent-cube.mesh.yaml"),
        "{found:?}"
    );
}
