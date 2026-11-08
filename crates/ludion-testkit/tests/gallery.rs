//! Integration tests exercising the real golden-image pipeline end to end: the example gallery and
//! its committed goldens (`examples/gallery/goldens/`), and the Phase 2 exit criterion — an
//! intentionally broken scene must produce a diff report an agent can read without an image viewer.
//!
//! Every test here needs a real GPU adapter (it renders through
//! [`ludion_engine::render_scene`]); they skip gracefully (printing why) when none is available,
//! matching the established pattern in `ludion-cli`'s own GPU-dependent tests. `ludion test` itself
//! (the CLI command) does **not** get this skip — only in-repo `cargo test` suites do.

use std::path::{Path, PathBuf};

use ludion_core::diagnostics::Diagnostic;
use ludion_engine::DebugView;
use ludion_testkit::{GoldenOutcome, GoldenTest, SuiteDefaults, bless_requested};

mod common;

fn gallery_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/gallery")
}

fn scratch_dir(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(name);
    std::fs::create_dir_all(&dir).expect("scratch dir creates");
    dir
}

/// Whether a render failure was due to a missing GPU adapter (`L2003`) — the one condition these
/// tests skip on.
fn is_no_adapter(diagnostics: &[Diagnostic]) -> bool {
    ludion_core::require_gpu::is_no_adapter(diagnostics)
}

/// Whether a render failure is one of the documented skips (`L2003` no GPU adapter, `L2205`
/// unpinned platform, `L2210` not the reference adapter).
fn is_skip(diagnostics: &[Diagnostic]) -> bool {
    ludion_testkit::unverifiable(diagnostics).is_some()
}

/// The skip diagnostic's rendering, for the `SKIPPED` line (its code names the reason).
fn skip_note(diagnostics: &[Diagnostic]) -> String {
    ludion_testkit::unverifiable(diagnostics).map_or_else(String::new, ToString::to_string)
}

/// The host's committed per-OS golden set; `GoldenTest::run` skips off the pinned tier.
fn os_goldens() -> PathBuf {
    gallery_dir().join("goldens").join(std::env::consts::OS)
}

/// Copies `scene_stem.yaml` and the gallery's `assets/` directory into a scratch project dir, so
/// tests can mutate a scene without touching the committed gallery. Returns the scratch scene path.
fn copy_scene_into(scratch: &Path, scene_stem: &str) -> PathBuf {
    let gallery = gallery_dir();
    let assets_src = gallery.join("assets");
    let assets_dst = scratch.join("assets");
    std::fs::create_dir_all(&assets_dst).unwrap();
    if assets_src.is_dir() {
        for entry in std::fs::read_dir(&assets_src).unwrap() {
            let entry = entry.unwrap();
            std::fs::copy(entry.path(), assets_dst.join(entry.file_name())).unwrap();
        }
    }
    let scene_dst = scratch.join(format!("{scene_stem}.yaml"));
    std::fs::copy(gallery.join(format!("{scene_stem}.yaml")), &scene_dst).unwrap();
    scene_dst
}

/// The gallery's model-document golden: a 900×600 contact sheet of the existing lantern voxel model
/// (`voxels/lantern.voxel.yaml`, six views → an exact 3×2 grid of 300×300 cells). It points at the
/// existing model doc rather than a duplicate, so this is a **programmatic** suite entry (directory
/// discovery only scans top-level `*.yaml`, and the lantern lives in `voxels/`) — see the module docs
/// for how discovery-driven model docs are picked up elsewhere.
fn lantern_sheet_test() -> GoldenTest {
    let gallery = gallery_dir();
    let mut test = GoldenTest::new(
        gallery.join("voxels").join("lantern.voxel.yaml"),
        os_goldens().join("lantern-sheet.png"),
    );
    test.width = 900;
    test.height = 600;
    test.ssim_threshold = 1.0;
    test
}

/// The gallery's `kind: sculpt` golden (`Plan/mesh-modeling.md` Wave 1): a 900×600 contact sheet of
/// the stone idol (`sculpts/idol.sculpt.yaml`), proving the model-view harness meshes and renders an
/// implicit-surface document the way [`lantern_sheet_test`] renders a voxel one. Programmatic for the
/// same reason: directory discovery only scans top-level `*.yaml`, and the idol lives in `sculpts/`.
fn idol_sheet_test() -> GoldenTest {
    let gallery = gallery_dir();
    let mut test = GoldenTest::new(
        gallery.join("sculpts").join("idol.sculpt.yaml"),
        os_goldens().join("idol-sheet.png"),
    );
    test.width = 900;
    test.height = 600;
    test.ssim_threshold = 1.0;
    test
}

/// The generated idol's contact sheet (`Plan/comfy-generation.md` Wave 2): a 900×600 sheet of
/// `tests/fixtures/idol-generated/meshes/mesh-0.mesh.yaml`, the textured mesh `gen accept`
/// froze out of a TRELLIS.2 run — the A/B twin of [`idol_sheet_test`]'s authored sculpt at the same
/// extent, views and rig, so the two paths to one subject are compared by two images and not by
/// prose.
///
/// The subject is the one in this suite that does not live under `examples/gallery/`: it is a
/// frozen copy of the asset vigil accepted; the game's tree is not public. The golden stays with
/// its siblings.
fn idol_generated_sheet_test() -> GoldenTest {
    let mut test = GoldenTest::new(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/idol-generated/meshes/mesh-0.mesh.yaml"),
        os_goldens().join("idol-generated-sheet.png"),
    );
    test.width = 900;
    test.height = 600;
    test.ssim_threshold = 1.0;
    test
}

/// The gallery's second `kind: sculpt` golden (`Plan/mesh-modeling.md` Wave 3): a 900×600 contact
/// sheet of the quadruped at rest (`sculpts/creature.sculpt.yaml`), the first model authored on
/// the Wave 3 keys — a `deform: taper` on the tail, horns and underside, `noise` skin on the body
/// masses, and one `head`/`tail` bone per limb capsule. Programmatic for the same reason
/// [`idol_sheet_test`] is: directory discovery only scans top-level `*.yaml`.
fn creature_sheet_test() -> GoldenTest {
    let gallery = gallery_dir();
    let mut test = GoldenTest::new(
        gallery.join("sculpts").join("creature.sculpt.yaml"),
        os_goldens().join("creature-sheet.png"),
    );
    test.width = 900;
    test.height = 600;
    test.ssim_threshold = 1.0;
    test
}

/// The gallery's `kind: assembly` golden (`Plan/voxel-animation.md` decision 11): a 900×600 rest-pose
/// contact sheet of `voxels/figure.assembly.yaml` — a torso, a head, and two mirrored arms
/// (`voxels/parts/*.part.yaml`) — proving the model-view harness renders a multi-part assembly the
/// same way it renders a voxel/mesh document, the "one golden per model" precedent
/// [`lantern_sheet_test`] set. Programmatic for the same reason: directory discovery only scans
/// top-level `*.yaml`, and the assembly (with its part files) lives in `voxels/`.
fn figure_sheet_test() -> GoldenTest {
    let gallery = gallery_dir();
    let mut test = GoldenTest::new(
        gallery.join("voxels").join("figure.assembly.yaml"),
        os_goldens().join("figure-sheet.png"),
    );
    test.width = 900;
    test.height = 600;
    test.ssim_threshold = 1.0;
    test
}

/// The gallery's clip golden (`Plan/voxel-animation.md` decision 11): a 900×600 **phase strip** of
/// `voxels/figure.assembly.yaml`'s `wave` clip — four evenly spaced phases of a looping 0.8 s clip
/// (`t = 0, 0.2, 0.4, 0.6`; the last phase is not the first repeated) laid out side by side in one
/// cell by the lineup machinery.
///
/// A strip is how a *clip* gets a committed golden at all: the rest sheet
/// ([`figure_sheet_test`]) pins the geometry, and this pins the poses that geometry moves through.
/// Declared programmatically for the same reason its two siblings are — directory discovery only
/// scans top-level `*.yaml`, and clip/strip is a per-test choice no directory layout expresses.
fn figure_wave_strip_test() -> GoldenTest {
    let gallery = gallery_dir();
    let mut test = GoldenTest::new(
        gallery.join("voxels").join("figure.assembly.yaml"),
        os_goldens().join("figure-wave-strip.png"),
    );
    test.width = 900;
    test.height = 600;
    test.clip = Some("wave".to_string());
    test.strip = Some(4);
    test.ssim_threshold = 1.0;
    test
}

/// The gallery's prefab-clip goldens: a 900×600 **phase strip** of `prefabs/hinge/model.prefab.yaml`
/// playing `clip` — four evenly spaced phases laid out side by side in one cell. The hinge is the
/// node-clip import's output, so these pin a prefab's posed render the way the figure strip pins an
/// assembly's: `open` is a smooth LINEAR swing, `snap` the same hinge on stepped keys.
fn hinge_strip_test(clip: &str) -> GoldenTest {
    let gallery = gallery_dir();
    let mut test = GoldenTest::new(
        gallery
            .join("prefabs")
            .join("hinge")
            .join("model.prefab.yaml"),
        os_goldens().join(format!("hinge-{clip}-strip.png")),
    );
    test.width = 900;
    test.height = 600;
    test.clip = Some(clip.to_string());
    test.strip = Some(4);
    test.ssim_threshold = 1.0;
    test
}

/// The mannequin character crossfading `run` into `idle` over eight phases at `hold: 5`: the fade
/// steps on the pose grid, so the first two cells repeat.
fn mannequin_crossfade_hold_test() -> GoldenTest {
    let mut test = GoldenTest::new(
        gallery_dir()
            .join("characters")
            .join("mannequin.character.yaml"),
        os_goldens().join("mannequin-run-idle-crossfade-hold5.png"),
    );
    test.width = 900;
    test.height = 600;
    test.crossfade = Some("run:idle:0.5".to_string());
    test.strip = Some(8);
    test.hold = Some(5);
    test.ssim_threshold = 1.0;
    test
}

/// Renders a 900×600 strip test and compares it against the committed golden at exactly SSIM 1.0,
/// or (re)writes the golden under `LUDION_BLESS`. Skips with no GPU adapter.
fn check_strip(test: GoldenTest, label: &str) {
    if bless_requested() {
        match test.bless() {
            Ok(GoldenOutcome::Blessed { .. }) => {}
            Ok(_) => unreachable!("bless() only ever returns Blessed"),
            Err(diagnostics) if is_skip(&diagnostics) => {
                eprintln!("SKIPPED {label} (bless): no GPU adapter");
            }
            Err(diagnostics) => panic!("blessing the {label} failed: {diagnostics:?}"),
        }
        return;
    }

    match test.run(&scratch_dir(&format!("{label}_out"))) {
        Ok(GoldenOutcome::Passed { report }) => {
            assert_eq!(
                report.ssim,
                Some(1.0),
                "the {label} must render byte-identically to its blessed golden"
            );
            assert_eq!((report.actual_width, report.actual_height), (900, 600));
        }
        Ok(GoldenOutcome::Failed { report, .. }) => {
            panic!("the {label} failed its golden: {}", report.summary)
        }
        Ok(GoldenOutcome::Blessed { .. }) => unreachable!("run() never blesses"),
        Err(diagnostics) if is_skip(&diagnostics) => {
            eprintln!("SKIPPED {label}: {}", skip_note(&diagnostics));
        }
        Err(diagnostics) => panic!("the {label} render failed: {diagnostics:?}"),
    }
}

/// The hinge's `snap` strip: stepped keys hold each pose on twos.
#[test]
fn needs_gpu_hinge_snap_strip_matches_committed_golden_at_exactly_one() {
    check_strip(hinge_strip_test("snap"), "hinge_snap_strip");
}

/// The hinge's `open` strip: the lid swings smoothly across the four phases.
#[test]
fn needs_gpu_hinge_open_strip_matches_committed_golden_at_exactly_one() {
    check_strip(hinge_strip_test("open"), "hinge_open_strip");
}

/// A crossfade at `hold: 5`: the fade steps on the pose grid, so the first two phases repeat.
#[test]
fn needs_gpu_mannequin_crossfade_hold_strip_matches_committed_golden_at_exactly_one() {
    check_strip(
        mannequin_crossfade_hold_test(),
        "mannequin_crossfade_hold_strip",
    );
}

/// The gallery's `ui-hud.yaml` scene rendered at 1920×1080 (the discovery pairing already covers it
/// at 960×540) — same 16:9 aspect, so the canvas is identical and only pixel scale doubles (Phase 14
/// decision 7's "same-aspect x2 proves uniform canvas scaling on real pixels").
fn ui_hud_1080_test() -> GoldenTest {
    let gallery = gallery_dir();
    let mut test = GoldenTest::new(
        gallery.join("ui-hud.yaml"),
        os_goldens().join("ui-hud-1080.png"),
    );
    test.width = 1920;
    test.height = 1080;
    test.ssim_threshold = 1.0;
    test
}

/// The `ui-hud` scene at 1920×1080 compares clean against its committed golden at exactly SSIM 1.0,
/// mirroring the discovery-driven 960×540 pairing. Under `LUDION_BLESS`, (re)writes the committed
/// golden instead. Skips gracefully with no GPU adapter.
#[test]
fn needs_gpu_ui_hud_1080_matches_committed_golden_at_exactly_one() {
    let test = ui_hud_1080_test();

    if bless_requested() {
        match test.bless() {
            Ok(GoldenOutcome::Blessed { .. }) => {}
            Ok(_) => unreachable!("bless() only ever returns Blessed"),
            Err(diagnostics) if is_skip(&diagnostics) => {
                eprintln!(
                    "SKIPPED ui_hud_1080_matches_committed_golden_at_exactly_one (bless): {}",
                    skip_note(&diagnostics)
                );
            }
            Err(diagnostics) => panic!("blessing the ui-hud 1080 golden failed: {diagnostics:?}"),
        }
        return;
    }

    let out_dir = scratch_dir("ui_hud_1080_out");
    match test.run(&out_dir) {
        Ok(GoldenOutcome::Passed { report }) => {
            assert_eq!(
                report.ssim,
                Some(1.0),
                "the ui-hud 1080 render must be byte-identical to its freshly-blessed golden"
            );
            assert_eq!((report.actual_width, report.actual_height), (1920, 1080));
        }
        Ok(GoldenOutcome::Failed { report, .. }) => {
            panic!("the ui-hud 1080 golden failed: {}", report.summary)
        }
        Ok(GoldenOutcome::Blessed { .. }) => unreachable!("run() never blesses"),
        Err(diagnostics) if is_skip(&diagnostics) => {
            eprintln!(
                "SKIPPED ui_hud_1080_matches_committed_golden_at_exactly_one: {}",
                skip_note(&diagnostics)
            );
        }
        Err(diagnostics) => panic!("the ui-hud 1080 render failed: {diagnostics:?}"),
    }
}

/// The gallery's `sun-cascades.yaml` scene under the `sun_cascades` tint view at 960×540: the cascade
/// view's sheet. The discovery pairing covers the scene's normal render.
fn sun_cascades_view_test() -> GoldenTest {
    let gallery = gallery_dir();
    let mut test = GoldenTest::new(
        gallery.join("sun-cascades.yaml"),
        os_goldens().join("sun-cascades-view.png"),
    )
    .with_debug_view(Some(DebugView::SunCascades));
    test.ssim_threshold = 1.0;
    test
}

/// The cascade view's sheet — the Clockout 0011 acceptance item: `sun-cascades` under the
/// `sun_cascades` debug view compares clean against its committed golden at exactly SSIM 1.0. Under
/// `LUDION_BLESS`, (re)writes the committed golden instead. Skips gracefully with no GPU adapter.
#[test]
fn needs_gpu_sun_cascades_view_matches_committed_golden_at_exactly_one() {
    let test = sun_cascades_view_test();

    if bless_requested() {
        match test.bless() {
            Ok(GoldenOutcome::Blessed { .. }) => {}
            Ok(_) => unreachable!("bless() only ever returns Blessed"),
            Err(diagnostics) if is_skip(&diagnostics) => {
                eprintln!(
                    "SKIPPED sun_cascades_view_matches_committed_golden_at_exactly_one (bless): {}",
                    skip_note(&diagnostics)
                );
            }
            Err(diagnostics) => {
                panic!("blessing the sun-cascades view golden failed: {diagnostics:?}")
            }
        }
        return;
    }

    let out_dir = scratch_dir("sun_cascades_view_out");
    match test.run(&out_dir) {
        Ok(GoldenOutcome::Passed { report }) => {
            assert_eq!(
                report.ssim,
                Some(1.0),
                "the sun-cascades view render must be byte-identical to its freshly-blessed golden"
            );
        }
        Ok(GoldenOutcome::Failed { report, .. }) => {
            panic!("the sun-cascades view golden failed: {}", report.summary)
        }
        Ok(GoldenOutcome::Blessed { .. }) => unreachable!("run() never blesses"),
        Err(diagnostics) if is_skip(&diagnostics) => {
            eprintln!(
                "SKIPPED sun_cascades_view_matches_committed_golden_at_exactly_one: {}",
                skip_note(&diagnostics)
            );
        }
        Err(diagnostics) => panic!("the sun-cascades view render failed: {diagnostics:?}"),
    }
}

/// The gallery's `ui-fonts.yaml` scene rendered at 1920×1080 (the discovery pairing covers
/// 960×540): the Silkscreen line's size 30 is three screen pixels per source pixel here, so this
/// golden is where the pixel face's crispness is pinned.
fn ui_fonts_1080_test() -> GoldenTest {
    let gallery = gallery_dir();
    let mut test = GoldenTest::new(
        gallery.join("ui-fonts.yaml"),
        os_goldens().join("ui-fonts-1080.png"),
    );
    test.width = 1920;
    test.height = 1080;
    test.ssim_threshold = 1.0;
    test
}

/// The `ui-fonts` scene at 1920×1080 compares clean against its committed golden at exactly SSIM 1.0,
/// mirroring the discovery-driven 960×540 pairing. Under `LUDION_BLESS`, (re)writes the committed
/// golden instead. Skips gracefully with no GPU adapter.
#[test]
fn needs_gpu_ui_fonts_1080_matches_committed_golden_at_exactly_one() {
    let test = ui_fonts_1080_test();

    if bless_requested() {
        match test.bless() {
            Ok(GoldenOutcome::Blessed { .. }) => {}
            Ok(_) => unreachable!("bless() only ever returns Blessed"),
            Err(diagnostics) if is_skip(&diagnostics) => {
                eprintln!(
                    "SKIPPED ui_fonts_1080_matches_committed_golden_at_exactly_one (bless): {}",
                    skip_note(&diagnostics)
                );
            }
            Err(diagnostics) => panic!("blessing the ui-fonts 1080 golden failed: {diagnostics:?}"),
        }
        return;
    }

    let out_dir = scratch_dir("ui_fonts_1080_out");
    match test.run(&out_dir) {
        Ok(GoldenOutcome::Passed { report }) => {
            assert_eq!(
                report.ssim,
                Some(1.0),
                "the ui-fonts 1080 render must be byte-identical to its freshly-blessed golden"
            );
            assert_eq!((report.actual_width, report.actual_height), (1920, 1080));
        }
        Ok(GoldenOutcome::Failed { report, .. }) => {
            panic!("the ui-fonts 1080 golden failed: {}", report.summary)
        }
        Ok(GoldenOutcome::Blessed { .. }) => unreachable!("run() never blesses"),
        Err(diagnostics) if is_skip(&diagnostics) => {
            eprintln!(
                "SKIPPED ui_fonts_1080_matches_committed_golden_at_exactly_one: {}",
                skip_note(&diagnostics)
            );
        }
        Err(diagnostics) => panic!("the ui-fonts 1080 render failed: {diagnostics:?}"),
    }
}

/// The `character-cycle` scene driven by `common::cycle_builder` to the middle of the run → jump
/// crossfade: the mannequin is half run, half jump, with its idle → run fade long finished. Tick
/// `IDLE_TICKS + RUN_TICKS + 4` is the jump switch (`character_cycle.rs` pins it at tick index 140)
/// plus half the 0.1 s fade. The bare discovered `character-cycle.png` is the rest pose.
fn character_cycle_driven_test() -> GoldenTest {
    let gallery = gallery_dir();
    let mut test = GoldenTest::new(
        gallery.join("character-cycle.yaml"),
        os_goldens().join("character-cycle-driven.png"),
    )
    .with_builder(common::cycle_builder)
    .with_frames(common::IDLE_TICKS + common::RUN_TICKS + 4);
    test.ssim_threshold = 1.0;
    test
}

/// The driven `character-cycle` render compares clean against its committed golden at exactly SSIM
/// 1.0. Under `LUDION_BLESS`, (re)writes the golden instead. Skips gracefully with no GPU adapter.
#[test]
fn needs_gpu_character_cycle_driven_matches_committed_golden_at_exactly_one() {
    let test = character_cycle_driven_test();

    if bless_requested() {
        match test.bless() {
            Ok(GoldenOutcome::Blessed { .. }) => {}
            Ok(_) => unreachable!("bless() only ever returns Blessed"),
            Err(diagnostics) if is_skip(&diagnostics) => {
                eprintln!(
                    "SKIPPED character_cycle_driven_matches_committed_golden_at_exactly_one                      (bless): {}",
                    skip_note(&diagnostics)
                );
            }
            Err(diagnostics) => {
                panic!("blessing the character-cycle golden failed: {diagnostics:?}")
            }
        }
        return;
    }

    let out_dir = scratch_dir("character_cycle_driven_out");
    match test.run(&out_dir) {
        Ok(GoldenOutcome::Passed { report }) => assert_eq!(
            report.ssim,
            Some(1.0),
            "the driven character-cycle render must be byte-identical to its freshly-blessed golden"
        ),
        Ok(GoldenOutcome::Failed { report, .. }) => {
            panic!(
                "the driven character-cycle golden failed: {}",
                report.summary
            )
        }
        Ok(GoldenOutcome::Blessed { .. }) => unreachable!("run() never blesses"),
        Err(diagnostics) if is_skip(&diagnostics) => {
            eprintln!(
                "SKIPPED character_cycle_driven_matches_committed_golden_at_exactly_one: {}",
                skip_note(&diagnostics)
            );
        }
        Err(diagnostics) => panic!("the driven character-cycle render failed: {diagnostics:?}"),
    }
}

/// The `camera-rig` scene driven by `common::rig_builder` to the middle of the switch blend: the
/// rail camera is half behind the hero, half side-on, with the `camera` debug category on. The bare
/// discovered `camera-rig.png` is the authored pose at frame 0, before the rig's first tick.
fn camera_rig_driven_test() -> GoldenTest {
    let gallery = gallery_dir();
    let mut test = GoldenTest::new(
        gallery.join("camera-rig.yaml"),
        os_goldens().join("camera-rig-driven.png"),
    )
    .with_builder(common::rig_builder)
    .with_frames(common::SWITCH_TICK + 30)
    .with_debug_draw(ludion_engine::DebugCategories::parse("camera"));
    test.ssim_threshold = 1.0;
    test
}

/// The driven `camera-rig` render compares clean against its committed golden at exactly SSIM 1.0.
/// Under `LUDION_BLESS`, (re)writes the golden instead. Skips gracefully with no GPU adapter.
#[test]
fn needs_gpu_camera_rig_driven_matches_committed_golden_at_exactly_one() {
    let test = camera_rig_driven_test();

    if bless_requested() {
        match test.bless() {
            Ok(GoldenOutcome::Blessed { .. }) => {}
            Ok(_) => unreachable!("bless() only ever returns Blessed"),
            Err(diagnostics) if is_skip(&diagnostics) => {
                eprintln!(
                    "SKIPPED camera_rig_driven_matches_committed_golden_at_exactly_one (bless): {}",
                    skip_note(&diagnostics)
                );
            }
            Err(diagnostics) => {
                panic!("blessing the camera-rig golden failed: {diagnostics:?}")
            }
        }
        return;
    }

    let out_dir = scratch_dir("camera_rig_driven_out");
    match test.run(&out_dir) {
        Ok(GoldenOutcome::Passed { report }) => assert_eq!(
            report.ssim,
            Some(1.0),
            "the driven camera-rig render must be byte-identical to its freshly-blessed golden"
        ),
        Ok(GoldenOutcome::Failed { report, .. }) => {
            panic!("the driven camera-rig golden failed: {}", report.summary)
        }
        Ok(GoldenOutcome::Blessed { .. }) => unreachable!("run() never blesses"),
        Err(diagnostics) if is_skip(&diagnostics) => {
            eprintln!(
                "SKIPPED camera_rig_driven_matches_committed_golden_at_exactly_one: {}",
                skip_note(&diagnostics)
            );
        }
        Err(diagnostics) => panic!("the driven camera-rig render failed: {diagnostics:?}"),
    }
}

/// The lantern model's contact sheet compares clean against its committed golden at exactly SSIM 1.0
/// — the "one golden per model" harness proof (`Plan/roadmap-voxel-mesh-pbr.md`). Under
/// `LUDION_BLESS`, (re)writes the committed golden instead (the suite's bless mechanism, mirroring
/// `ludion test --bless` for a doc that discovery can't reach). Skips gracefully with no GPU adapter.
#[test]
fn needs_gpu_lantern_contact_sheet_matches_committed_golden_at_exactly_one() {
    let test = lantern_sheet_test();

    if bless_requested() {
        match test.bless() {
            Ok(GoldenOutcome::Blessed { .. }) => {}
            Ok(_) => unreachable!("bless() only ever returns Blessed"),
            Err(diagnostics) if is_skip(&diagnostics) => {
                eprintln!(
                    "SKIPPED lantern_contact_sheet_matches_committed_golden_at_exactly_one \
                     (bless): {}",
                    skip_note(&diagnostics)
                );
            }
            Err(diagnostics) => panic!("blessing the lantern sheet failed: {diagnostics:?}"),
        }
        return;
    }

    let out_dir = scratch_dir("lantern_sheet_out");
    match test.run(&out_dir) {
        Ok(GoldenOutcome::Passed { report }) => {
            assert_eq!(
                report.ssim,
                Some(1.0),
                "the lantern sheet must render byte-identically to its freshly-blessed golden"
            );
            assert_eq!(
                (report.actual_width, report.actual_height),
                (900, 600),
                "the sheet is an exact 3x2 grid of 300x300 cells"
            );
        }
        Ok(GoldenOutcome::Failed { report, .. }) => {
            panic!("the lantern sheet failed its golden: {}", report.summary)
        }
        Ok(GoldenOutcome::Blessed { .. }) => unreachable!("run() never blesses"),
        Err(diagnostics) if is_skip(&diagnostics) => {
            eprintln!(
                "SKIPPED lantern_contact_sheet_matches_committed_golden_at_exactly_one: {}",
                skip_note(&diagnostics)
            );
        }
        Err(diagnostics) => panic!("the lantern sheet render failed: {diagnostics:?}"),
    }
}

/// The idol sculpt's contact sheet compares clean against its committed golden at exactly SSIM 1.0
/// — [`lantern_contact_sheet_matches_committed_golden_at_exactly_one`]'s proof, extended to a
/// `kind: sculpt` document. Under `LUDION_BLESS`, (re)writes the committed golden instead. Skips
/// gracefully with no GPU adapter.
#[test]
fn needs_gpu_idol_contact_sheet_matches_committed_golden_at_exactly_one() {
    let test = idol_sheet_test();

    if bless_requested() {
        match test.bless() {
            Ok(GoldenOutcome::Blessed { .. }) => {}
            Ok(_) => unreachable!("bless() only ever returns Blessed"),
            Err(diagnostics) if is_skip(&diagnostics) => {
                eprintln!(
                    "SKIPPED idol_contact_sheet_matches_committed_golden_at_exactly_one (bless): \
                     {}",
                    skip_note(&diagnostics)
                );
            }
            Err(diagnostics) => panic!("blessing the idol sheet failed: {diagnostics:?}"),
        }
        return;
    }

    let out_dir = scratch_dir("idol_sheet_out");
    match test.run(&out_dir) {
        Ok(GoldenOutcome::Passed { report }) => {
            assert_eq!(
                report.ssim,
                Some(1.0),
                "the idol sheet must render byte-identically to its freshly-blessed golden"
            );
            assert_eq!(
                (report.actual_width, report.actual_height),
                (900, 600),
                "the sheet is an exact 3x2 grid of 300x300 cells"
            );
        }
        Ok(GoldenOutcome::Failed { report, .. }) => {
            panic!("the idol sheet failed its golden: {}", report.summary)
        }
        Ok(GoldenOutcome::Blessed { .. }) => unreachable!("run() never blesses"),
        Err(diagnostics) if is_skip(&diagnostics) => {
            eprintln!(
                "SKIPPED idol_contact_sheet_matches_committed_golden_at_exactly_one: {}",
                skip_note(&diagnostics)
            );
        }
        Err(diagnostics) => panic!("the idol sheet render failed: {diagnostics:?}"),
    }
}

/// The generated idol's contact sheet compares clean against its committed golden at exactly SSIM
/// 1.0 — [`idol_contact_sheet_matches_committed_golden_at_exactly_one`]'s proof, extended to a
/// textured `.mesh.yaml` that arrived through `gen accept`, so the sheet's texture sampling is
/// pinned alongside its geometry. Under `LUDION_BLESS`, (re)writes the committed golden instead.
/// Skips gracefully with no GPU adapter.
#[test]
fn needs_gpu_generated_idol_contact_sheet_matches_committed_golden_at_exactly_one() {
    let test = idol_generated_sheet_test();

    if bless_requested() {
        match test.bless() {
            Ok(GoldenOutcome::Blessed { .. }) => {}
            Ok(_) => unreachable!("bless() only ever returns Blessed"),
            Err(diagnostics) if is_skip(&diagnostics) => {
                eprintln!(
                    "SKIPPED generated_idol_contact_sheet_matches_committed_golden_at_exactly_one \
                     (bless): {}",
                    skip_note(&diagnostics)
                );
            }
            Err(diagnostics) => panic!("blessing the generated idol sheet failed: {diagnostics:?}"),
        }
        return;
    }

    let out_dir = scratch_dir("idol_generated_sheet_out");
    match test.run(&out_dir) {
        Ok(GoldenOutcome::Passed { report }) => {
            assert_eq!(
                report.ssim,
                Some(1.0),
                "the generated idol sheet must render byte-identically to its freshly-blessed \
                 golden"
            );
            assert_eq!(
                (report.actual_width, report.actual_height),
                (900, 600),
                "the sheet is an exact 3x2 grid of 300x300 cells"
            );
        }
        Ok(GoldenOutcome::Failed { report, .. }) => {
            panic!(
                "the generated idol sheet failed its golden: {}",
                report.summary
            )
        }
        Ok(GoldenOutcome::Blessed { .. }) => unreachable!("run() never blesses"),
        Err(diagnostics) if is_skip(&diagnostics) => {
            eprintln!(
                "SKIPPED generated_idol_contact_sheet_matches_committed_golden_at_exactly_one: {}",
                skip_note(&diagnostics)
            );
        }
        Err(diagnostics) => panic!("the generated idol sheet render failed: {diagnostics:?}"),
    }
}

/// The creature sculpt's contact sheet compares clean against its committed golden at exactly SSIM
/// 1.0 — [`idol_contact_sheet_matches_committed_golden_at_exactly_one`]'s proof, extended to the
/// Wave 3 model whose field carries deformers and noise. Under `LUDION_BLESS`, (re)writes the
/// committed golden instead. Skips gracefully with no GPU adapter.
#[test]
fn needs_gpu_creature_contact_sheet_matches_committed_golden_at_exactly_one() {
    let test = creature_sheet_test();

    if bless_requested() {
        match test.bless() {
            Ok(GoldenOutcome::Blessed { .. }) => {}
            Ok(_) => unreachable!("bless() only ever returns Blessed"),
            Err(diagnostics) if is_skip(&diagnostics) => {
                eprintln!(
                    "SKIPPED creature_contact_sheet_matches_committed_golden_at_exactly_one \
                     (bless): {}",
                    skip_note(&diagnostics)
                );
            }
            Err(diagnostics) => panic!("blessing the creature sheet failed: {diagnostics:?}"),
        }
        return;
    }

    let out_dir = scratch_dir("creature_sheet_out");
    match test.run(&out_dir) {
        Ok(GoldenOutcome::Passed { report }) => {
            assert_eq!(
                report.ssim,
                Some(1.0),
                "the creature sheet must render byte-identically to its freshly-blessed golden"
            );
            assert_eq!(
                (report.actual_width, report.actual_height),
                (900, 600),
                "the sheet is an exact 3x2 grid of 300x300 cells"
            );
        }
        Ok(GoldenOutcome::Failed { report, .. }) => {
            panic!("the creature sheet failed its golden: {}", report.summary)
        }
        Ok(GoldenOutcome::Blessed { .. }) => unreachable!("run() never blesses"),
        Err(diagnostics) if is_skip(&diagnostics) => {
            eprintln!(
                "SKIPPED creature_contact_sheet_matches_committed_golden_at_exactly_one: {}",
                skip_note(&diagnostics)
            );
        }
        Err(diagnostics) => panic!("the creature sheet render failed: {diagnostics:?}"),
    }
}

/// The `figure` assembly's rest-pose contact sheet compares clean against its committed golden at
/// exactly SSIM 1.0 — [`lantern_contact_sheet_matches_committed_golden_at_exactly_one`]'s proof,
/// extended to a `kind: assembly` document (`Plan/voxel-animation.md` decision 11). Under
/// `LUDION_BLESS`, (re)writes the committed golden instead. Skips gracefully with no GPU adapter.
#[test]
fn needs_gpu_figure_contact_sheet_matches_committed_golden_at_exactly_one() {
    let test = figure_sheet_test();

    if bless_requested() {
        match test.bless() {
            Ok(GoldenOutcome::Blessed { .. }) => {}
            Ok(_) => unreachable!("bless() only ever returns Blessed"),
            Err(diagnostics) if is_skip(&diagnostics) => {
                eprintln!(
                    "SKIPPED figure_contact_sheet_matches_committed_golden_at_exactly_one (bless): \
                     {}",
                    skip_note(&diagnostics)
                );
            }
            Err(diagnostics) => panic!("blessing the figure sheet failed: {diagnostics:?}"),
        }
        return;
    }

    let out_dir = scratch_dir("figure_sheet_out");
    match test.run(&out_dir) {
        Ok(GoldenOutcome::Passed { report }) => {
            assert_eq!(
                report.ssim,
                Some(1.0),
                "the figure sheet must render byte-identically to its freshly-blessed golden"
            );
            assert_eq!(
                (report.actual_width, report.actual_height),
                (900, 600),
                "the sheet is an exact 3x2 grid of 300x300 cells"
            );
        }
        Ok(GoldenOutcome::Failed { report, .. }) => {
            panic!("the figure sheet failed its golden: {}", report.summary)
        }
        Ok(GoldenOutcome::Blessed { .. }) => unreachable!("run() never blesses"),
        Err(diagnostics) if is_skip(&diagnostics) => {
            eprintln!(
                "SKIPPED figure_contact_sheet_matches_committed_golden_at_exactly_one: {}",
                skip_note(&diagnostics)
            );
        }
        Err(diagnostics) => panic!("the figure sheet render failed: {diagnostics:?}"),
    }
}

/// The `figure` assembly's `wave` phase strip compares clean against its committed golden at exactly
/// SSIM 1.0 — the rest sheet's proof extended to a *posed* render, which is what pins a clip
/// (`Plan/voxel-animation.md` decision 11). Under `LUDION_BLESS`, (re)writes the committed golden
/// instead. Skips gracefully with no GPU adapter.
#[test]
fn needs_gpu_figure_wave_strip_matches_committed_golden_at_exactly_one() {
    let test = figure_wave_strip_test();

    if bless_requested() {
        match test.bless() {
            Ok(GoldenOutcome::Blessed { .. }) => {}
            Ok(_) => unreachable!("bless() only ever returns Blessed"),
            Err(diagnostics) if is_skip(&diagnostics) => {
                eprintln!(
                    "SKIPPED figure_wave_strip_matches_committed_golden_at_exactly_one (bless): no \
                     GPU adapter"
                );
            }
            Err(diagnostics) => panic!("blessing the figure wave strip failed: {diagnostics:?}"),
        }
        return;
    }

    let out_dir = scratch_dir("figure_wave_strip_out");
    match test.run(&out_dir) {
        Ok(GoldenOutcome::Passed { report }) => {
            assert_eq!(
                report.ssim,
                Some(1.0),
                "the figure wave strip must render byte-identically to its freshly-blessed golden"
            );
            assert_eq!(
                (report.actual_width, report.actual_height),
                (900, 600),
                "a strip is one cell, so the sheet is the requested size"
            );
        }
        Ok(GoldenOutcome::Failed { report, .. }) => {
            panic!(
                "the figure wave strip failed its golden: {}",
                report.summary
            )
        }
        Ok(GoldenOutcome::Blessed { .. }) => unreachable!("run() never blesses"),
        Err(diagnostics) if is_skip(&diagnostics) => {
            eprintln!(
                "SKIPPED figure_wave_strip_matches_committed_golden_at_exactly_one: {}",
                skip_note(&diagnostics)
            );
        }
        Err(diagnostics) => panic!("the figure wave strip render failed: {diagnostics:?}"),
    }
}

/// A voxel model document dispatched through [`GoldenTest`] renders its contact sheet: bless a tiny
/// model, then re-run and get an exact SSIM-1.0 pass — proving the model path is the *same* harness
/// (bless → render → decode → compare) as the scene path, not a second one. Skips with no adapter.
#[test]
fn needs_gpu_voxel_model_document_blesses_then_passes_at_exactly_one() {
    let scratch = scratch_dir("voxel_doc_golden");
    let doc = scratch.join("cube.voxel.yaml");
    std::fs::write(
        &doc,
        "format: 1\nkind: voxel\nsize:\n  - 1\n  - 1\n  - 1\npalette:\n  A:\n    color:\n      \
         - 0.8\n      - 0.3\n      - 0.2\n      - 1.0\nslices:\n  -\n    - \"A\"\n",
    )
    .unwrap();
    let golden = scratch.join("goldens").join("cube.voxel.png");
    let mut test = GoldenTest::new(&doc, &golden);
    test.width = 600;
    test.height = 400;

    match test.bless() {
        Ok(GoldenOutcome::Blessed { .. }) => {}
        Ok(_) => unreachable!("bless() only ever returns Blessed"),
        Err(diagnostics) if is_no_adapter(&diagnostics) => {
            eprintln!(
                "SKIPPED voxel_model_document_blesses_then_passes_at_exactly_one: no adapter"
            );
            return;
        }
        Err(diagnostics) => panic!("blessing the voxel doc failed: {diagnostics:?}"),
    }

    let out_dir = scratch.join("target").join("testkit");
    match test.run(&out_dir) {
        Ok(GoldenOutcome::Passed { report }) => {
            assert_eq!(report.ssim, Some(1.0));
            // 600x400, six views -> 3x2 grid of 200x200 cells.
            assert_eq!((report.actual_width, report.actual_height), (600, 400));
        }
        other => panic!(
            "expected an exact pass after blessing the voxel doc, got {:?}",
            other.map(|o| o.ssim())
        ),
    }
}

/// A structurally-broken voxel model document fails through [`GoldenTest`] with its `L111x`
/// validation diagnostics (surfaced on the CPU, before any GPU adapter is touched) — never a panic,
/// and never masked as a comparison failure. Needs no GPU, so it never skips.
#[test]
fn broken_voxel_model_document_fails_with_diagnostics_not_panic() {
    let scratch = scratch_dir("broken_voxel_doc");
    let doc = scratch.join("broken.voxel.yaml");
    // `kind: voxel` (so it dispatches to the model path) but a slice glyph `Z` that no palette entry
    // defines — a data-consistency error the voxel validator reports as `L1114`.
    std::fs::write(
        &doc,
        "format: 1\nkind: voxel\nsize:\n  - 1\n  - 1\n  - 1\npalette:\n  A:\n    color:\n      \
         - 0.8\n      - 0.3\n      - 0.2\n      - 1.0\nslices:\n  -\n    - \"Z\"\n",
    )
    .unwrap();
    let golden = scratch.join("goldens").join("broken.voxel.png");
    let test = GoldenTest::new(&doc, &golden);

    let diagnostics = match test.bless() {
        Err(diagnostics) => diagnostics,
        Ok(_) => panic!("a broken voxel document must not bless successfully"),
    };
    assert!(
        diagnostics
            .iter()
            .any(|d| d.code.as_str().starts_with("L111")),
        "expected an L111x voxel-validation diagnostic, got {:?}",
        diagnostics
            .iter()
            .map(|d| d.code.as_str())
            .collect::<Vec<_>>()
    );
    assert!(
        !ludion_core::require_gpu::is_no_adapter(&diagnostics),
        "validation must fail before the GPU is ever requested"
    );
}

/// **Phase 2 exit criterion — what `ludion test` runs for real.** Every gallery scene compares
/// clean against its committed golden. Because the golden was blessed on this same machine/adapter,
/// the comparison is not just "above threshold" but **exactly** SSIM 1.0 — proving the whole
/// render → encode → decode → compare chain is bit-exact-deterministic on one machine, not merely
/// perceptually close.
#[test]
fn needs_gpu_all_gallery_scenes_pass_against_committed_goldens_at_exactly_one() {
    let dir = gallery_dir();
    let mut tests = match ludion_testkit::discover(&dir, SuiteDefaults::default(), None) {
        Ok(tests) => tests,
        Err(diagnostic) if is_skip(std::slice::from_ref(&diagnostic)) => {
            eprintln!(
                "SKIPPED all_gallery_scenes_pass_against_committed_goldens_at_exactly_one: {diagnostic}"
            );
            return;
        }
        Err(diagnostic) => panic!("gallery directory lists: {diagnostic}"),
    };
    for entry in &mut tests {
        entry.test.ssim_threshold = 1.0;
    }
    assert!(
        !tests.is_empty(),
        "the gallery must have at least one scene"
    );

    let out_dir = scratch_dir("gallery_suite_out");
    let results = ludion_testkit::run_suite(&tests, &out_dir, false);

    for result in &results {
        match &result.outcome {
            Ok(GoldenOutcome::Passed { report }) => {
                assert_eq!(
                    report.ssim,
                    Some(1.0),
                    "{}: same-machine render vs its freshly-blessed golden must be exactly \
                     1.0 (byte-identical render), got {:?}",
                    result.name,
                    report.ssim
                );
            }
            Ok(GoldenOutcome::Failed { report, .. }) => {
                panic!(
                    "{}: expected a pass, got a failure: {}",
                    result.name, report.summary
                );
            }
            Ok(GoldenOutcome::Blessed { .. }) => {
                unreachable!("run_suite(bless=false) never blesses")
            }
            Err(diagnostics) if is_skip(diagnostics) => {
                eprintln!(
                    "SKIPPED all_gallery_scenes_pass_against_committed_goldens_at_exactly_one: \
                     {}",
                    skip_note(diagnostics)
                );
                return;
            }
            Err(diagnostics) => {
                panic!("{}: render failed: {:?}", result.name, diagnostics);
            }
        }
    }
}

/// Bless, then immediately compare: the freshly written golden must compare as a pass at exactly
/// SSIM 1.0 — the harness's own round-trip determinism guarantee, independent of any committed
/// golden.
#[test]
fn needs_gpu_bless_then_compare_round_trip_is_exact() {
    let scratch = scratch_dir("bless_round_trip");
    let scene = copy_scene_into(&scratch, "sprites");
    let golden = scratch.join("goldens").join("sprites.png");
    let test = GoldenTest::new(scene, golden);

    match test.bless() {
        Ok(GoldenOutcome::Blessed { .. }) => {}
        Ok(_) => unreachable!("bless() only ever returns Blessed"),
        Err(diagnostics) if is_no_adapter(&diagnostics) => {
            eprintln!("SKIPPED bless_then_compare_round_trip_is_exact: no GPU adapter");
            return;
        }
        Err(diagnostics) => panic!("bless failed: {diagnostics:?}"),
    }

    let out_dir = scratch.join("target").join("testkit");
    match test.run(&out_dir) {
        Ok(GoldenOutcome::Passed { report }) => {
            assert_eq!(report.ssim, Some(1.0));
            assert_eq!(report.mae, Some(0.0));
            assert!(report.damaged_tiles.is_empty());
        }
        other => panic!(
            "expected an exact pass after blessing, got {:?}",
            other.map(|o| o.ssim())
        ),
    }
}

/// **Decision 8's render-inertness proof (Phase 12 Wave 5).** `colliders.yaml` stripped of every
/// `Collider` block renders **byte-identically** to the committed scene: a collider never touches
/// the `RenderScene`, so the gallery golden shows authored layout only — behavioral coverage lives
/// in `ludion-spatial`'s conformance suite, never in this image.
#[test]
fn needs_gpu_collider_components_are_render_inert() {
    let scratch = scratch_dir("collider_render_inert");
    let scene = copy_scene_into(&scratch, "colliders");

    let bless = |golden: &Path| -> bool {
        match GoldenTest::new(&scene, golden).bless() {
            Ok(GoldenOutcome::Blessed { .. }) => true,
            Ok(_) => unreachable!("bless() only ever returns Blessed"),
            Err(diagnostics) if is_no_adapter(&diagnostics) => {
                eprintln!("SKIPPED collider_components_are_render_inert: no GPU adapter");
                false
            }
            Err(diagnostics) => panic!("bless failed: {diagnostics:?}"),
        }
    };

    let golden_with = scratch.join("goldens").join("with-colliders.png");
    if !bless(&golden_with) {
        return;
    }

    // Strip every `Collider:` component block: the key line plus its more-indented payload lines.
    let original = std::fs::read_to_string(&scene).unwrap();
    let mut kept: Vec<&str> = Vec::new();
    let mut skipping = false;
    for line in original.lines() {
        let indent = line.len() - line.trim_start().len();
        if skipping && (line.trim().is_empty() || indent > 6) {
            continue;
        }
        skipping = false;
        if line.trim_end() == "      Collider:" {
            skipping = true;
            continue;
        }
        kept.push(line);
    }
    let stripped = kept.join("\n") + "\n";
    assert_ne!(
        original, stripped,
        "colliders.yaml must actually author Collider blocks for this proof to mean anything"
    );
    std::fs::write(&scene, stripped).unwrap();

    let golden_without = scratch.join("goldens").join("without-colliders.png");
    if !bless(&golden_without) {
        return;
    }

    let with = std::fs::read(&golden_with).unwrap();
    let without = std::fs::read(&golden_without).unwrap();
    assert_eq!(
        with, without,
        "Collider must be render-inert: the same scene with and without its Collider components \
         must render byte-identically"
    );
}

/// **The Phase 2 exit criterion, spelled out as an assertion on JSON contents.** Take a gallery
/// scene, move *and* tint one object relative to its committed golden, render, and compare: the
/// diff report's damaged-region localization must name the region the object actually moved to —
/// checked here on the structured data, not by looking at an image.
#[test]
fn needs_gpu_broken_scene_diff_report_localizes_the_damaged_region() {
    let scratch = scratch_dir("broken_scene_exit_criterion");
    let scene = copy_scene_into(&scratch, "sprites");
    let golden = scratch.join("goldens").join("sprites.png");
    let test = GoldenTest::new(&scene, &golden);

    // First, bless a golden matching the *original* scene.
    match test.bless() {
        Ok(GoldenOutcome::Blessed { .. }) => {}
        Ok(_) => unreachable!(),
        Err(diagnostics) if is_no_adapter(&diagnostics) => {
            eprintln!(
                "SKIPPED broken_scene_diff_report_localizes_the_damaged_region: no GPU adapter"
            );
            return;
        }
        Err(diagnostics) => panic!("bless failed: {diagnostics:?}"),
    }

    // Now break it: move the pink "tinted-sprite" up and to the right, enlarge it, and retint it
    // green. It starts at translation [2.4, 0.0, 0.0] with color [1.0, 0.4, 0.7, 1.0] — the same
    // fixture used for the manual verification pass in the task's own instructions.
    let original = std::fs::read_to_string(&scene).unwrap();
    let broken = original.replace(
        "      Transform:\n        translation:\n          - 2.4\n          - 0.0\n          - 0.0\n      Sprite:\n        size:\n          - 0.9\n          - 0.9\n        color:\n          - 1.0\n          - 0.4\n          - 0.7\n          - 1.0",
        "      Transform:\n        translation:\n          - 2.6\n          - 1.6\n          - 0.0\n      Sprite:\n        size:\n          - 1.3\n          - 1.3\n        color:\n          - 0.0\n          - 1.0\n          - 0.1\n          - 1.0",
    );
    assert_ne!(
        original, broken,
        "the fixture text this test targets must still be present in sprites.yaml"
    );
    std::fs::write(&scene, broken).unwrap();

    let out_dir = scratch.join("target").join("testkit");
    let (report, actual_path, report_path) = match test.run(&out_dir) {
        Ok(GoldenOutcome::Failed {
            report,
            actual_path,
            report_path,
        }) => (report, actual_path, report_path),
        Ok(GoldenOutcome::Passed { report }) => {
            panic!(
                "expected the moved+tinted object to fail the golden comparison, ssim = {:?}",
                report.ssim
            )
        }
        Ok(GoldenOutcome::Blessed { .. }) => unreachable!("run() never blesses"),
        Err(diagnostics) => panic!("render failed: {diagnostics:?}"),
    };

    // --- Assertions on the JSON contents (the actual exit criterion) ---

    assert!(!report.pass);
    assert!(
        report.ssim.unwrap() < report.threshold,
        "report: {}",
        report.to_json()
    );

    // The object moved from (world x=+2.4, y=0) up and to the right: it must land in the upper
    // portion of the right half of the tile grid, and definitely not in the untouched left half
    // (the checker sprite) or the exact center (the brick sprite).
    assert!(
        !report.damaged_tiles.is_empty(),
        "expected at least one damaged tile"
    );
    let worst = report
        .worst_tile
        .as_ref()
        .expect("a failing comparison always has a worst tile");
    assert!(
        worst.col >= report.tile_grid.cols / 2,
        "the moved object is on the right half of the frame; worst tile was col {} of {}",
        worst.col,
        report.tile_grid.cols
    );
    assert!(
        worst.row < report.tile_grid.rows / 2,
        "the moved object is in the upper half of the frame; worst tile was row {} of {}",
        worst.row,
        report.tile_grid.rows
    );
    assert!(
        ["top-right", "top-center", "middle-right", "center"].contains(&worst.region.as_str()),
        "unexpected region name: {}",
        worst.region
    );

    // Every damaged tile's rect must fall within the reported image dimensions — a basic sanity
    // check that the localization coordinates are meaningful, not placeholders.
    for tile in &report.damaged_tiles {
        assert!(tile.rect.x + tile.rect.width <= report.actual_width);
        assert!(tile.rect.y + tile.rect.height <= report.actual_height);
    }

    // The summary line and the on-disk artifacts are the human-facing half of the same report.
    assert!(report.summary.contains("ssim"));
    assert!(
        actual_path.exists(),
        "the actual PNG must be written on failure"
    );
    assert!(
        report_path.exists(),
        "the diff report JSON must be written on failure"
    );
    let on_disk = std::fs::read_to_string(&report_path).unwrap();
    assert_eq!(
        on_disk,
        report.to_json(),
        "the written report must match the returned one"
    );
}

/// The `projectile-muzzle` scene driven tick by tick: shots fired from the eye leave the viewmodel
/// muzzle and meet the crosshair line within 4 m. The engine is driven by hand (`advance_live` then
/// `extract_present_scene`) because convergence lives in the extract-side store, which a one-shot
/// render never primes. The bare discovered `projectile-muzzle.png` has no shots.
#[test]
fn needs_gpu_projectile_muzzle_shot_matches_committed_golden() {
    let mut engine = common::projectile_builder()
        .load_scene_file(&gallery_dir().join("projectile-muzzle.yaml"))
        .expect("the scene loads");
    for _ in 0..common::SHOT_GOLDEN_TICK {
        assert!(engine.advance_live(), "the sim ticks");
        engine.extract_present_scene().expect("extracts");
    }
    ludion_testkit::check_engine_golden(
        &mut engine,
        gallery_dir().join("goldens"),
        "projectile-muzzle-shot",
        &scratch_dir("projectile_muzzle_shot_out"),
    );
}

/// The `projectile-gibs` room driven to tick 150: a thousand debris shots (asset-mesh and oriented
/// cube looks, one tag fading) have settled on the floor. The bare discovered `projectile-gibs.png`
/// is the empty room.
#[test]
fn needs_gpu_projectile_gibs_settle_matches_committed_golden() {
    let mut engine = common::gib_builder()
        .load_scene_file(&gallery_dir().join("projectile-gibs.yaml"))
        .expect("the scene loads");
    for _ in 0..common::GIB_GOLDEN_TICK {
        assert!(engine.advance_live(), "the sim ticks");
        engine.extract_present_scene().expect("extracts");
    }
    ludion_testkit::check_engine_golden(
        &mut engine,
        gallery_dir().join("goldens"),
        "projectile-gibs-settle",
        &scratch_dir("projectile_gibs_settle_out"),
    );
}

/// The gibs room with `kind: voxel` models as the three looks, driven to the same tick.
#[test]
fn needs_gpu_projectile_gibs_voxel_matches_committed_golden() {
    let mut engine = common::voxel_gib_builder()
        .load_scene_file(&gallery_dir().join("projectile-gibs.yaml"))
        .expect("the scene loads");
    for _ in 0..common::GIB_GOLDEN_TICK {
        assert!(engine.advance_live(), "the sim ticks");
        engine.extract_present_scene().expect("extracts");
    }
    ludion_testkit::check_engine_golden(
        &mut engine,
        gallery_dir().join("goldens"),
        "projectile-gibs-voxel",
        &scratch_dir("projectile_gibs_voxel_out"),
    );
}

/// The same drive without a GPU: by tick 150 nearly every live gib is parked above the floor, and
/// after the longest lifetime (counted from the last throw) every one has expired exactly once.
#[test]
fn projectile_gibs_settle_on_the_floor_and_all_expire() {
    use ludion_engine::Projectiles;
    let mut engine = common::gib_builder()
        .load_scene_file(&gallery_dir().join("projectile-gibs.yaml"))
        .expect("the scene loads");
    let mut expired = 0usize;
    let end = common::GIB_THROW_TICKS + (common::GIB_LIFETIME * 60.0) as u64 + 2;
    for tick in 1..=end {
        assert!(engine.advance_live(), "the sim ticks");
        let res = engine.world().resource::<Projectiles>().unwrap();
        expired += res.events().expired.len();
        if tick == common::GIB_GOLDEN_TICK {
            let pool = res.pool();
            let ids: Vec<_> = pool.ids().collect();
            assert!(!ids.is_empty());
            let parked = ids
                .iter()
                .filter(|&&id| pool.is_parked(id) == Some(true))
                .count();
            assert!(
                parked * 10 >= ids.len() * 9,
                "{parked} of {} gibs parked",
                ids.len()
            );
            for &id in &ids {
                let y = pool.position(id).unwrap()[1];
                assert!(y >= -0.05, "a gib sank to y = {y}");
            }
        }
    }
    let res = engine.world().resource::<Projectiles>().unwrap();
    assert!(res.pool().is_empty(), "every gib has expired");
    assert_eq!(expired, common::GIB_COUNT as usize);
}

/// The `lod-hall` scene (500 skinned figures, a three-level chain) against its committed golden.
/// The all-scenes golden covers it too; this names the LOD proof.
#[test]
fn needs_gpu_lod_hall_matches_committed_golden() {
    let mut engine = ludion_engine::Engine::builder()
        .load_scene_file(&gallery_dir().join("lod-hall.yaml"))
        .expect("the scene loads");
    ludion_testkit::check_engine_golden(
        &mut engine,
        gallery_dir().join("goldens"),
        "lod-hall",
        &scratch_dir("lod_hall_out"),
    );
}

/// **Culling on or off, the hall renders the same bytes.** Turning culling off keeps the cull pass
/// for a chained frame with the frustum test skipped, so level selection, and so the image, is
/// identical. A scene with no chain keeps every existing gallery golden untouched (the all-scenes
/// golden above), and `CullShape.levels == 1` for it (`graph.rs`'s chain tests).
#[test]
fn needs_gpu_lod_hall_renders_identically_with_culling_on_and_off() {
    let mut engine = ludion_engine::Engine::builder()
        .load_scene_file(&gallery_dir().join("lod-hall.yaml"))
        .expect("the scene loads");
    let on = match engine.render_png(640, 360) {
        Ok(png) => png,
        Err(d) if is_no_adapter(std::slice::from_ref(&d)) => {
            eprintln!(
                "SKIPPED lod_hall_renders_identically_with_culling_on_and_off: no GPU adapter"
            );
            return;
        }
        Err(d) => panic!("render failed: {d}"),
    };
    engine.set_frustum_culling(false);
    let off = engine.render_png(640, 360).expect("renders");
    assert!(on == off, "culling off changed the hall's bytes");
}
