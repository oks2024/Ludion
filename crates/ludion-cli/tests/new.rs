//! Integration tests for `ludion new`, driving the real built binary (`env!("CARGO_BIN_EXE_ludion")`).
//!
//! The scaffold produces the expected file set + key content (and the Node wrapper only in checkout
//! flow); `new` refuses to scaffold inside the engine workspace's member globs (`L0011`); the
//! generated project's own `cargo test` passes out of the box — the real exit criterion. In a
//! project, a stale `LUDION-GUIDE.md` warns `L0036` until `new --refresh-guide`, and `check-scene`
//! names the game's own `--check-scene` for an unknown component.

mod common;

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

/// The `ludion` binary under test.
fn ludion() -> &'static str {
    env!("CARGO_BIN_EXE_ludion")
}

/// The engine workspace root (`.../crates/ludion-cli` -> hop up two).
fn engine_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("workspace root")
        .to_path_buf()
}

/// Runs `ludion <args>` in `base`, asserts it failed with `L0030` led by the code, and that the
/// existing `taken` file is untouched.
fn assert_refused_with_l0030(base: &Path, args: &[&str]) {
    let taken = base.join("taken.yaml");
    let output = Command::new(ludion())
        .args(args)
        .current_dir(base)
        .output()
        .expect("run ludion");
    assert!(!output.status.success(), "{args:?} should refuse");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.starts_with("error[L0030]: "),
        "{args:?}: stderr was: {stderr}"
    );
    assert_eq!(fs::read_to_string(taken).unwrap(), "keep me\n");
}

/// `voxel new` and `sculpt new` refuse an existing file with the same coded diagnostic.
#[test]
fn model_new_verbs_refuse_an_existing_file_with_l0030() {
    let (base, _lock) = common::stable_scratch("model_existing_target");
    fs::write(base.join("taken.yaml"), "keep me\n").unwrap();
    assert_refused_with_l0030(&base, &["voxel", "new", "taken.yaml", "--size", "2,2,2"]);
    assert_refused_with_l0030(&base, &["sculpt", "new", "taken.yaml"]);
}

/// `voxel upscale` refuses an existing `--out` with the same coded diagnostic.
#[test]
fn voxel_upscale_refuses_an_existing_out_with_l0030() {
    let (base, _lock) = common::stable_scratch("upscale_existing_target");
    let made = Command::new(ludion())
        .args(["voxel", "new", "src.voxel.yaml", "--size", "2,2,2"])
        .current_dir(&base)
        .output()
        .expect("run voxel new");
    assert!(
        made.status.success(),
        "{}",
        String::from_utf8_lossy(&made.stderr)
    );
    fs::write(base.join("taken.yaml"), "keep me\n").unwrap();
    assert_refused_with_l0030(
        &base,
        &[
            "voxel",
            "upscale",
            "src.voxel.yaml",
            "--factor",
            "2",
            "--out",
            "taken.yaml",
        ],
    );
}

/// An existing target is refused with `L0030`, led by the code and with no hand-written prefix.
#[test]
fn new_refuses_an_existing_target_with_l0030() {
    let (base, _lock) = common::stable_scratch("existing_target");
    fs::create_dir_all(base.join("taken")).expect("create the taken directory");
    let output = Command::new(ludion())
        .arg("new")
        .arg("taken")
        .current_dir(&base)
        .output()
        .expect("run `ludion new`");
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.starts_with("error[L0030]: "), "stderr was: {stderr}");
    assert!(
        !stderr.lines().any(|line| line.starts_with("error: ")),
        "stderr was: {stderr}"
    );
}

/// Names that would scaffold a project that does not build are refused with `L0035` and a suggested
/// name, before anything is created.
#[test]
fn new_refuses_names_that_scaffold_a_broken_project() {
    let (base, _lock) = common::stable_scratch("bad_names");
    for bad in ["my game", "2048", "a.b", "a\"b", "test", "fn", "con"] {
        let output = Command::new(ludion())
            .arg("new")
            .arg(bad)
            .current_dir(&base)
            .output()
            .expect("run `ludion new`");
        assert!(!output.status.success(), "{bad:?} must be refused");
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(stderr.contains("L0035"), "{bad:?}: stderr was: {stderr}");
        assert!(
            stderr.contains("try `ludion new "),
            "{bad:?}: stderr was: {stderr}"
        );
        assert!(
            fs::read_dir(&base).unwrap().next().is_none(),
            "{bad:?} created something in the scratch directory"
        );
    }
}

/// A hyphenated name with an underscore and a digit is still accepted.
#[test]
fn new_accepts_a_hyphen_underscore_digit_name() {
    let (base, _lock) = common::stable_scratch("good_name");
    let output = Command::new(ludion())
        .arg("new")
        .arg("my-game_2")
        .current_dir(&base)
        .output()
        .expect("run `ludion new`");
    assert!(output.status.success(), "{:?}", output.stderr);
    assert!(base.join("my-game_2/Cargo.toml").is_file());
}

#[test]
fn new_scaffolds_the_expected_files() {
    // A hyphenated name exercises the crate-name vs Rust-identifier substitution.
    let (base, _lock) = common::stable_scratch("scaffold_check");
    let output = Command::new(ludion())
        .arg("new")
        .arg("scaffold-check")
        .current_dir(&base)
        .output()
        .expect("run `ludion new`");
    assert!(
        output.status.success(),
        "`ludion new` failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let proj = base.join("scaffold-check");
    let read = |rel: &str| fs::read_to_string(proj.join(rel)).unwrap_or_default();

    // The full expected file set exists.
    for rel in [
        "Cargo.toml",
        "clippy.toml",
        "ludion.toml",
        ".gitignore",
        "CLAUDE.md",
        "LUDION-GUIDE.md",
        "src/main.rs",
        "src/lib.rs",
        "src/components.rs",
        "src/systems.rs",
        "scenes/main.yaml",
        "scenes/model-stage.yaml",
        "scenes/model-stage-night.yaml",
        "scenes/effects-stage.yaml",
        "tests/gameplay.rs",
        "tests/golden.rs",
        "tests/drift.rs",
        "scripts/ludion.mjs",
    ] {
        assert!(proj.join(rel).is_file(), "missing generated file: {rel}");
    }
    assert!(proj.join("goldens").is_dir(), "missing goldens/ directory");

    // The lockfile stays committed: the ignore file holds the build output and the per-machine
    // stores, never `Cargo.lock`.
    let ignore = read(".gitignore");
    assert!(ignore.lines().any(|line| line == "/target"), "{ignore}");
    assert!(ignore.lines().any(|line| line == ".ludion/"), "{ignore}");
    assert!(!ignore.lines().any(|line| line == "Cargo.lock"), "{ignore}");

    // The three stage scenes (not `main.yaml` — its `Player` is not in the built-in registry) check
    // clean against the built-in component registry.
    for rel in [
        "scenes/model-stage.yaml",
        "scenes/model-stage-night.yaml",
        "scenes/effects-stage.yaml",
    ] {
        let output = Command::new(ludion())
            .arg("check-scene")
            .arg(proj.join(rel))
            .output()
            .expect("run `ludion check-scene`");
        assert!(
            output.status.success(),
            "`ludion check-scene {rel}` failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    // The wrapper builds this checkout's CLI into the project's own target dir.
    let wrapper = read("scripts/ludion.mjs");
    let root_literal = format!(
        "const engineRoot = {};",
        serde_json::to_string(&engine_root().to_string_lossy().replace('\\', "/")).unwrap()
    );
    assert!(
        wrapper.contains(&root_literal),
        "wrapper lacks the engine root literal:\n{wrapper}"
    );
    assert!(
        wrapper.contains("ludion-cli"),
        "wrapper lacks the ludion-cli target dir"
    );
    assert!(
        !wrapper.contains("{{"),
        "wrapper has an unsubstituted placeholder"
    );

    // Cargo.toml: standalone workspace, engine path deps, name substituted.
    let cargo = read("Cargo.toml");
    assert!(
        cargo.contains("name = \"scaffold-check\""),
        "package name not substituted"
    );
    assert!(
        cargo.contains("[workspace]"),
        "generated project must be a standalone workspace"
    );
    assert!(
        cargo.contains("name = \"scaffold_check\""),
        "lib ident not substituted"
    );
    assert!(
        cargo.contains("/crates/ludion\""),
        "ludion path dep not pointed at the engine checkout"
    );
    // The engine path must resolve to a real directory.
    assert!(
        engine_root().join("crates/ludion/Cargo.toml").is_file(),
        "derived engine root does not contain crates/ludion"
    );

    // main.rs and lib.rs: the ident form is used for the crate reference.
    assert!(read("src/main.rs").contains("ludion::run(scaffold_check::game())"));
    let lib = read("src/lib.rs");
    assert!(lib.contains("pub fn game() -> EngineBuilder"));
    // Definitions live in per-concern modules; the wiring root re-exports them.
    assert!(lib.contains("mod components;") && lib.contains("mod systems;"));
    assert!(read("src/components.rs").contains("struct Player"));
    assert!(read("src/systems.rs").contains("fn move_player"));

    // Scene mixes a builtin visual with the game component.
    let scene = read("scenes/main.yaml");
    assert!(
        scene.contains("Player:") && scene.contains("Camera:") && scene.contains("PointLight:")
    );

    // ludion.toml carries the manifest fields.
    let manifest = read("ludion.toml");
    assert!(manifest.contains("default_scene = \"scenes/main.yaml\""));
    assert!(
        manifest.contains("[ios]")
            && manifest.contains("bundle_id = \"com.example.scaffold-check\"")
    );
    // The iOS host's start symbol and the feature shape the export build relies on.
    assert!(lib.contains("pub unsafe extern \"C\" fn ludion_game_start"));
    let cargo = read("Cargo.toml");
    assert!(
        cargo.contains("default-features = false") && cargo.contains("wire = [\"ludion/wire\"]")
    );

    // The guide is embedded with its docs frontmatter stripped.
    let guide = read("LUDION-GUIDE.md");
    assert!(
        guide.starts_with("# The Ludion engine guide"),
        "guide should start at its H1 heading"
    );
    assert!(
        !guide.starts_with("---"),
        "docs frontmatter must be stripped from LUDION-GUIDE.md"
    );
}

/// Installed-mode scaffolding (`LUDION_NEW_INSTALLED=1`, as if the binary were `cargo install`ed):
/// the generated `Cargo.toml` pins published *version* deps for every engine crate and carries no
/// absolute path into the build machine's checkout. The checkout-mode test above stays the default.
#[test]
fn new_installed_mode_emits_version_deps_without_absolute_paths() {
    let (base, _lock) = common::stable_scratch("installed_mode");
    let output = Command::new(ludion())
        .arg("new")
        .arg("installed-check")
        .current_dir(&base)
        .env("LUDION_NEW_INSTALLED", "1")
        .output()
        .expect("run `ludion new`");
    assert!(
        output.status.success(),
        "`ludion new` (installed mode) failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let cargo = fs::read_to_string(base.join("installed-check").join("Cargo.toml")).unwrap();
    assert!(
        !base.join("installed-check/scripts/ludion.mjs").exists(),
        "an installed CLI is on the PATH; no wrapper is scaffolded"
    );

    // Every engine dependency is a version dep at this binary's own workspace version.
    let version = env!("CARGO_PKG_VERSION");
    for line in [
        format!("ludion = {{ version = \"{version}\", default-features = false }}"),
        format!("ludion-testkit = {{ version = \"{version}\" }}"),
        format!("ludion-engine = {{ version = \"{version}\" }}"),
        format!("ludion-core = {{ version = \"{version}\" }}"),
    ] {
        assert!(
            cargo.contains(&line),
            "missing version dep `{line}` in:\n{cargo}"
        );
    }

    // No dependency path specs (inline `{ path = … }`, distinct from the bin/lib `path = "src/…"`)
    // and no absolute path into the build machine's checkout survive.
    assert!(
        !cargo.contains("{ path ="),
        "installed-mode Cargo.toml must carry no path deps:\n{cargo}"
    );
    assert!(
        !cargo.contains(&engine_root().to_string_lossy().replace('\\', "/")),
        "installed-mode Cargo.toml leaked the build machine's engine root:\n{cargo}"
    );
}

/// Runs `ludion <args>` in `dir`.
fn run_in(dir: &Path, args: &[&str]) -> std::process::Output {
    Command::new(ludion())
        .args(args)
        .current_dir(dir)
        .output()
        .expect("run ludion")
}

/// `ludion docs` warns `L0036` on stderr (stdout and exit code unchanged) when the project's guide
/// differs, until `ludion new --refresh-guide` rewrites only that file.
#[test]
fn a_stale_guide_warns_until_refreshed() {
    let (base, _lock) = common::stable_scratch("stale_guide");
    let made = run_in(&base, &["new", "guide-check"]);
    assert!(
        made.status.success(),
        "{}",
        String::from_utf8_lossy(&made.stderr)
    );
    let proj = base.join("guide-check");
    let fresh = fs::read_to_string(proj.join("LUDION-GUIDE.md")).unwrap();

    let quiet = run_in(&proj, &["docs"]);
    assert!(quiet.status.success());
    assert!(!String::from_utf8_lossy(&quiet.stderr).contains("L0036"));

    let claude = proj.join("CLAUDE.md");
    fs::write(&claude, "my own notes\n").unwrap();
    fs::write(proj.join("LUDION-GUIDE.md"), format!("{fresh}\nan edit\n")).unwrap();
    let heading = fresh
        .lines()
        .next()
        .unwrap()
        .trim_start_matches('#')
        .trim()
        .to_owned();
    for args in [
        vec!["docs"],
        vec!["docs", "engine-guide", "--section", heading.as_str()],
    ] {
        let output = run_in(&proj, &args);
        assert!(output.status.success(), "{args:?}");
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(stderr.contains("warning[L0036]"), "{args:?}: {stderr}");
        assert!(stderr.contains("--refresh-guide"), "{args:?}: {stderr}");
    }

    let refreshed = run_in(&proj.join("scenes"), &["new", "--refresh-guide"]);
    assert!(
        refreshed.status.success(),
        "{}",
        String::from_utf8_lossy(&refreshed.stderr)
    );
    assert_eq!(
        fs::read_to_string(proj.join("LUDION-GUIDE.md")).unwrap(),
        fresh
    );
    assert_eq!(fs::read_to_string(&claude).unwrap(), "my own notes\n");
    assert!(!String::from_utf8_lossy(&run_in(&proj, &["docs"]).stderr).contains("L0036"));

    let outside = run_in(&base, &["new", "--refresh-guide"]);
    assert!(!outside.status.success());
    assert!(String::from_utf8_lossy(&outside.stderr).contains("error[L0023]"));
}

/// An unknown component in a project's scene names the game's own `--check-scene`, once.
#[test]
fn check_scene_in_a_project_names_the_game_check() {
    let (base, _lock) = common::stable_scratch("check_scene_project");
    let made = run_in(&base, &["new", "check-game"]);
    assert!(
        made.status.success(),
        "{}",
        String::from_utf8_lossy(&made.stderr)
    );
    let proj = base.join("check-game");
    let line = "cargo run -- --scene scenes/main.yaml --check-scene";

    let inside = run_in(&proj, &["check-scene", "scenes/main.yaml"]);
    assert!(!inside.status.success());
    let stderr = String::from_utf8_lossy(&inside.stderr);
    assert!(stderr.contains("L1013"), "{stderr}");
    assert_eq!(stderr.matches(line).count(), 1, "{stderr}");

    let loose = base.join("loose");
    fs::create_dir_all(&loose).unwrap();
    fs::copy(proj.join("scenes/main.yaml"), loose.join("main.yaml")).unwrap();
    let outside = run_in(&loose, &["check-scene", "main.yaml"]);
    let stderr = String::from_utf8_lossy(&outside.stderr);
    assert!(stderr.contains("L1013"), "{stderr}");
    assert!(!stderr.contains("--check-scene"), "{stderr}");
}

#[test]
fn new_refuses_inside_the_engine_workspace() {
    // Run with the cwd inside `crates/` so the target lands under a member glob.
    let crates_dir = engine_root().join("crates");
    let probe = crates_dir.join("zz_new_workspace_probe");
    let _ = fs::remove_dir_all(&probe);

    let output = Command::new(ludion())
        .arg("new")
        .arg("zz_new_workspace_probe")
        .current_dir(&crates_dir)
        .output()
        .expect("run `ludion new`");

    assert!(
        !output.status.success(),
        "`new` must refuse a target inside the workspace"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("L0011"),
        "expected the L0011 diagnostic, got: {stderr}"
    );
    assert!(
        !probe.exists(),
        "`new` must not create anything when it refuses"
    );
}

/// The real exit criterion: `ludion new <name>` then `cargo test` inside the generated project is
/// green out of the box.
///
/// Cost note: this compiles and runs the generated project's test suite. `CARGO_TARGET_DIR` is set to
/// this repo's target dir so the engine crates are reused from the workspace build cache rather than
/// cold-built (which would add minutes — observed ~30s warm on a dev box). The generated project is a
/// separate workspace with its own dependency resolution, so it may re-pin a few transitive crates and
/// recompile them into the shared target dir (harmless churn). It is deliberately not `#[ignore]` so
/// the exit criterion is actually enforced.
#[test]
fn generated_project_passes_cargo_test() {
    let platform = match ludion_testkit::platform_dir() {
        Ok(platform) => platform,
        Err(diag) => {
            eprintln!(
                "SKIPPED generated_project_passes_cargo_test:
{diag}"
            );
            return;
        }
    };
    let (base, _lock) = common::stable_scratch("gen_project");
    let status = Command::new(ludion())
        .arg("new")
        .arg("smoke_game")
        .current_dir(&base)
        .status()
        .expect("run `ludion new`");
    assert!(status.success(), "`ludion new smoke_game` failed");

    let project = base.join("smoke_game");
    // An inherited override must win: `CARGO_TARGET_TMPDIR` (and so the scaffold's stable scratch
    // path) follows it, and pointing the nested build elsewhere revives the stale-fingerprint trap
    // documented on `common::stable_scratch` — a same-named scaffold whose old-path dep-info runs
    // stale binaries.
    let target_dir = std::env::var_os("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| engine_root().join("target"));

    // Match this test's own profile so the shared target dir's artifacts are reused, not rebuilt.
    let release = std::env::current_exe()
        .ok()
        .and_then(|exe| {
            exe.parent()
                .and_then(Path::parent)
                .and_then(|p| p.file_name())
                .map(|s| s.to_string_lossy().into_owned())
        })
        .as_deref()
        == Some("release");

    let cargo_test = |bless: bool| {
        let mut cmd = Command::new(env!("CARGO"));
        cmd.arg("test")
            .current_dir(&project)
            .env("CARGO_TARGET_DIR", &target_dir)
            // The knob governs this workspace's tests, not the generated project's own.
            .env_remove(ludion_core::require_gpu::FORBID_GPU_ENV);
        if release {
            cmd.arg("--release");
        }
        if bless {
            // The generated project blesses into its own `goldens/<os>/` on whatever GPU this is.
            cmd.env("LUDION_BLESS", "1")
                .env("LUDION_BLESS_ANY_ADAPTER", "1");
        }
        cmd.output()
            .expect("run `cargo test` in the generated project")
    };

    // Out of the box (no golden committed yet) the full suite is green: the golden test instructs
    // and passes rather than failing or silently blessing.
    let output = cargo_test(false);
    assert!(
        output.status.success(),
        "generated project `cargo test` failed\n--- stdout ---\n{}\n--- stderr ---\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );

    // A generated project's builder and manifest name one default scene, and the host refuses a
    // manifest that moves (`L0038`). Placed before the GPU-dependent half below, which may skip.
    let check_scene = || {
        let mut cmd = Command::new(env!("CARGO"));
        cmd.args(["run", "--quiet"])
            .current_dir(&project)
            .env("CARGO_TARGET_DIR", &target_dir)
            .env_remove(ludion_core::require_gpu::FORBID_GPU_ENV);
        if release {
            cmd.arg("--release");
        }
        cmd.args(["--", "--check-scene"])
            .output()
            .expect("run `cargo run -- --check-scene` in the generated project")
    };
    let checked = check_scene();
    assert!(
        checked.status.success(),
        "generated project `--check-scene` failed\n{}{}",
        String::from_utf8_lossy(&checked.stdout),
        String::from_utf8_lossy(&checked.stderr),
    );
    let manifest_path = project.join("ludion.toml");
    let manifest = fs::read_to_string(&manifest_path).unwrap();
    assert!(manifest.contains("default_scene = \"scenes/main.yaml\""));
    fs::write(
        &manifest_path,
        manifest.replace("scenes/main.yaml", "scenes/model-stage.yaml"),
    )
    .unwrap();
    let moved = check_scene();
    assert!(
        !moved.status.success() && String::from_utf8_lossy(&moved.stderr).contains("L0038"),
        "a manifest that names another scene must be refused with L0038\n{}{}",
        String::from_utf8_lossy(&moved.stdout),
        String::from_utf8_lossy(&moved.stderr),
    );
    fs::write(&manifest_path, manifest).unwrap();

    // The out-of-box golden loop: `LUDION_BLESS=1 cargo test` blesses through the game's own test
    // (the CLI's `ludion test --bless` cannot — directory suites are built-in-registry only), then a
    // plain run compares against the fresh golden. Skipped without a GPU adapter: an asked-for bless
    // is a hard failure there by design, which on this machine just means "nothing to exercise".

    let bless = cargo_test(true);
    let bless_text = format!(
        "{}{}",
        String::from_utf8_lossy(&bless.stdout),
        String::from_utf8_lossy(&bless.stderr)
    );
    if !bless.status.success()
        && ludion_core::require_gpu::skip_no_adapter(bless_text.contains("L2003"))
    {
        eprintln!(
            "SKIPPED bless/compare half of generated_project_passes_cargo_test: no GPU adapter (L2003)"
        );
        return;
    }
    assert!(
        bless.status.success(),
        "generated project `LUDION_BLESS=1 cargo test` failed\n{bless_text}"
    );
    assert!(
        project
            .join("goldens")
            .join(platform)
            .join("main.png")
            .is_file(),
        "blessing must write goldens/<os>/main.png"
    );

    let compare = cargo_test(false);
    assert!(
        compare.status.success(),
        "generated project `cargo test` failed against its freshly blessed golden\n--- stdout ---\n{}\n--- stderr ---\n{}",
        String::from_utf8_lossy(&compare.stdout),
        String::from_utf8_lossy(&compare.stderr),
    );
}
