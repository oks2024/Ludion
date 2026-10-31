//! Discovering and running a directory's whole golden-test suite — what `ludion test` drives.
//!
//! # Layout convention
//!
//! For a project directory `<dir>` (e.g. `examples/gallery`), every `*.yaml`/`*.yml` scene file
//! directly inside `<dir>` (not recursing into subdirectories like `assets/` or `goldens/`) is one
//! golden test, paired with the golden image `<dir>/goldens/<os>/<scene-stem>.png`, where `<os>`
//! is the host platform's pinned-tier subdirectory ([`crate::golden::platform_dir`]: `windows` or
//! `macos`; any other OS fails discovery with `L2205`). This is the simplest layout that makes
//! "one folder of scenes = one golden suite" true with zero configuration: no manifest is required
//! to add a new gallery scene, just drop the `.yaml` file next to the others and `ludion test
//! --bless` once per pinned platform.
//!
//! An optional `<dir>/goldens/manifest.json` — at the `goldens/` root, **not** inside a per-OS
//! subdirectory, because resolution/threshold/frames describe the scene's content, not a platform —
//! overrides the suite's default resolution/threshold per scene stem, for the rare test that needs
//! something other than the suite default:
//!
//! ```json
//! { "breakout-mock": { "ssim_threshold": 0.97 }, "sprite-flipbook": { "frames": 23 },
//!   "collider-wireframes": { "frames": 2, "debug_draw": ["colliders"] } }
//! ```
//!
//! Every field (`width`, `height`, `ssim_threshold`, `frames`, `debug_draw` — a list of debug-draw
//! category names, as `ludion render --debug-draw` takes — and `anti_aliasing`, a mode name as
//! `ludion render --aa` takes) is optional and falls back to the
//! suite default (`frames` defaults to `0`, the scene at rest) when absent or when the manifest
//! itself is absent. The manifest is strict (`L2208`): no other fields, `ssim_threshold` in
//! `(0, 1]`, `width`/`height` above 0, and every key must be the stem of a scene in `<dir>`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use ludion_core::codes::code;
use ludion_core::diagnostics::Diagnostic;
use serde::Deserialize;

use crate::golden::{GoldenOutcome, GoldenTest};

/// Per-scene overrides read from `goldens/manifest.json`. Every field is optional; an absent
/// manifest is equivalent to an empty one. An unknown field is refused, so a misspelled override
/// cannot silently fall back to the default.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct ManifestEntry {
    width: Option<u32>,
    height: Option<u32>,
    ssim_threshold: Option<f64>,
    frames: Option<u64>,
    debug_draw: Option<Vec<String>>,
    anti_aliasing: Option<String>,
}

/// Suite-wide defaults, overridable per scene by the manifest.
#[derive(Clone, Copy, Debug)]
pub struct SuiteDefaults {
    /// Default render width.
    pub width: u32,
    /// Default render height.
    pub height: u32,
    /// Default minimum passing SSIM.
    pub ssim_threshold: f64,
}

impl Default for SuiteDefaults {
    fn default() -> Self {
        SuiteDefaults {
            width: crate::golden::DEFAULT_WIDTH,
            height: crate::golden::DEFAULT_HEIGHT,
            ssim_threshold: crate::golden::DEFAULT_SSIM_THRESHOLD,
        }
    }
}

/// One discovered test: its scene name (the file stem) and the [`GoldenTest`] to run.
pub struct DiscoveredTest {
    /// The scene's file stem (e.g. `sprites` for `sprites.yaml`) — the test's display name.
    pub name: String,
    /// The golden test itself.
    pub test: GoldenTest,
}

/// The `fonts.yaml` table (`kind: fonts`) sits at a content root beside its scenes but has no
/// entities to render, so the directory walk leaves it out.
fn is_fonts_table(path: &Path) -> bool {
    std::fs::read_to_string(path).is_ok_and(|text| text.lines().any(|line| line == "kind: fonts"))
}

/// Discovers every scene-vs-golden test in `dir`, in deterministic (filename) order, applying
/// `defaults` and any `goldens/manifest.json` overrides. `filter`, if given, keeps only tests whose
/// name contains it as a substring.
///
/// # Errors
/// `L1015` if `dir` cannot be listed or the manifest read; `L2208` if `goldens/manifest.json`
/// exists but is not valid JSON, carries an unknown field, an `ssim_threshold` outside `(0, 1]`, a
/// zero `width`/`height`, or a key matching no scene; `L2205` when the host OS has no pinned golden tier.
pub fn discover(
    dir: &Path,
    defaults: SuiteDefaults,
    filter: Option<&str>,
) -> Result<Vec<DiscoveredTest>, Diagnostic> {
    let manifest = load_manifest(dir)?;

    let mut scene_paths: Vec<PathBuf> = std::fs::read_dir(dir)
        .map_err(|error| {
            Diagnostic::new(
                code::SCENE_IO,
                format!("could not list `{}`: {error}", dir.display()),
            )
            .with_help("check the path (relative to the current directory) and permissions")
        })?
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.path())
        .filter(|path| {
            path.is_file()
                && matches!(
                    path.extension().and_then(|e| e.to_str()),
                    Some("yaml") | Some("yml")
                )
                && !is_fonts_table(path)
        })
        .collect();
    // `read_dir`'s order is platform-defined; sort by filename so suite order — and therefore
    // pass/fail print order — is deterministic across every OS in the matrix.
    scene_paths.sort();

    let scene_stem = |path: &Path| {
        path.file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default()
    };
    // Checked before `filter`, so a filtered run still catches a misspelled key.
    let unmatched: Vec<&str> = manifest
        .keys()
        .map(String::as_str)
        .filter(|key| !scene_paths.iter().any(|scene| scene_stem(scene) == *key))
        .collect();
    if !unmatched.is_empty() {
        return Err(Diagnostic::new(
            code::GOLDEN_MANIFEST_INVALID,
            format!(
                "`{}` has no scene for the manifest key(s) {}",
                dir.join("goldens").join("manifest.json").display(),
                unmatched
                    .iter()
                    .map(|key| format!("`{key}`"))
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        )
        .with_help("rename the key to a scene's file stem, or remove it"));
    }

    let goldens_dir = dir.join("goldens").join(crate::golden::platform_dir()?);
    let mut tests = Vec::new();
    for scene in scene_paths {
        let stem = scene_stem(&scene);
        if let Some(needle) = filter
            && !stem.contains(needle)
        {
            continue;
        }
        let overrides = manifest.get(&stem);
        let golden = goldens_dir.join(format!("{stem}.png"));
        // Start from `new` (which sets the private built-in registry seam), then apply the suite's
        // resolution/threshold overrides. Directory suites are always built-in-registry scenes.
        let mut test = GoldenTest::new(scene, golden);
        test.width = overrides.and_then(|o| o.width).unwrap_or(defaults.width);
        test.height = overrides.and_then(|o| o.height).unwrap_or(defaults.height);
        test.ssim_threshold = overrides
            .and_then(|o| o.ssim_threshold)
            .unwrap_or(defaults.ssim_threshold);
        test.frames = overrides.and_then(|o| o.frames).unwrap_or(0);
        if let Some(names) = overrides.and_then(|o| o.debug_draw.clone()) {
            test.debug_draw = ludion_engine::DebugCategories::from_names(names);
        }
        // Validated at load, so the name always parses.
        if let Some(mode) = overrides
            .and_then(|o| o.anti_aliasing.as_deref())
            .and_then(ludion_engine::AntiAliasing::from_name)
        {
            test.anti_aliasing = mode;
        }
        tests.push(DiscoveredTest { name: stem, test });
    }
    Ok(tests)
}

fn load_manifest(dir: &Path) -> Result<BTreeMap<String, ManifestEntry>, Diagnostic> {
    let path = dir.join("goldens").join("manifest.json");
    if !path.exists() {
        return Ok(BTreeMap::new());
    }
    let source = std::fs::read_to_string(&path).map_err(|error| {
        Diagnostic::new(
            code::SCENE_IO,
            format!("could not read `{}`: {error}", path.display()),
        )
    })?;
    let manifest: BTreeMap<String, ManifestEntry> =
        serde_json::from_str(&source).map_err(|error| {
            Diagnostic::new(
                code::GOLDEN_MANIFEST_INVALID,
                format!(
                    "could not parse `{}` as a golden-test manifest: {error}",
                    path.display()
                ),
            )
            .with_help(
                "a manifest is a JSON object mapping scene stems to optional \
                 {\"width\":.., \"height\":.., \"ssim_threshold\":.., \"frames\":.., \"debug_draw\":[..], \"anti_aliasing\":\"..\"} overrides, \
                 and no other fields; fix the JSON, or delete the file to use the suite defaults",
            )
        })?;
    for (stem, entry) in &manifest {
        validate_entry(&path, stem, entry)?;
    }
    Ok(manifest)
}

/// Refuses an override that would make a test vacuous or unrunnable.
fn validate_entry(path: &Path, stem: &str, entry: &ManifestEntry) -> Result<(), Diagnostic> {
    let refuse = |field: &str, value: String, rule: &str| {
        Err(Diagnostic::new(
            code::GOLDEN_MANIFEST_INVALID,
            format!(
                "`{}` sets {field} of `{stem}` to {value}: {rule}",
                path.display()
            ),
        )
        .with_path(format!("/{stem}/{field}"))
        .with_help("fix the value, or delete the field to use the suite default"))
    };
    if let Some(threshold) = entry.ssim_threshold
        && !(threshold > 0.0 && threshold <= 1.0)
    {
        return refuse(
            "ssim_threshold",
            threshold.to_string(),
            "ssim_threshold must be in (0, 1]; 0 would pass any image",
        );
    }
    for (field, value) in [("width", entry.width), ("height", entry.height)] {
        if value == Some(0) {
            return refuse(field, "0".into(), "a render extent must be greater than 0");
        }
    }
    // Categories are free-form (a game names its own), so only a blank name is wrong.
    if let Some(names) = &entry.debug_draw
        && names.iter().any(|name| name.trim().is_empty())
    {
        return refuse(
            "debug_draw",
            format!("{names:?}"),
            "a debug-draw category name must not be blank",
        );
    }
    if let Some(name) = &entry.anti_aliasing
        && ludion_engine::AntiAliasing::from_name(name).is_none()
    {
        let modes = ludion_engine::AntiAliasing::ALL.map(|mode| mode.name());
        return refuse(
            "anti_aliasing",
            format!("`{name}`"),
            &format!("anti_aliasing must be one of {}", modes.join(", ")),
        );
    }
    Ok(())
}

/// One test's outcome within a suite run.
pub struct SuiteTestResult {
    /// The scene's display name.
    pub name: String,
    /// `Ok` for a completed comparison (pass, fail, or bless); `Err` for a hard failure (the scene
    /// itself failed to render, or the golden could not be read).
    pub outcome: Result<GoldenOutcome, Vec<Diagnostic>>,
}

impl SuiteTestResult {
    /// Whether this result counts as passing (a hard error, or a failed comparison, both count as
    /// not passing).
    pub fn passed(&self) -> bool {
        matches!(&self.outcome, Ok(outcome) if outcome.is_ok())
    }
}

/// Runs (or blesses) every discovered test, in order. Never stops early: one test's hard failure
/// does not prevent the rest of the suite from running, so a caller sees every failure in one pass.
pub fn run_suite(tests: &[DiscoveredTest], out_dir: &Path, bless: bool) -> Vec<SuiteTestResult> {
    tests
        .iter()
        .map(|discovered| {
            let outcome = if bless {
                discovered.test.bless()
            } else {
                discovered.test.run(out_dir)
            };
            SuiteTestResult {
                name: discovered.name.clone(),
                outcome,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Unwraps a discovery that must succeed; `None` (with a printed reason) when the host OS has no
    /// pinned golden tier, where discovery is unverifiable rather than failing.
    fn verifiable(outcome: Result<Vec<DiscoveredTest>, Diagnostic>) -> Option<Vec<DiscoveredTest>> {
        match outcome {
            Ok(tests) => Some(tests),
            Err(error) => {
                let diags = [error];
                let Some(skip) = crate::golden::unverifiable(&diags) else {
                    panic!("discovery failed: {:?}", diags[0]);
                };
                eprintln!("SKIPPED: {}", skip.message);
                None
            }
        }
    }

    #[test]
    fn discover_finds_scenes_sorted_and_pairs_goldens() {
        let dir =
            std::env::temp_dir().join(format!("ludion_testkit_discover_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("b.yaml"), "format: 1\nentities: []\n").unwrap();
        std::fs::write(dir.join("a.yaml"), "format: 1\nentities: []\n").unwrap();
        std::fs::write(dir.join("not-a-scene.txt"), "ignore me").unwrap();

        let Some(tests) = verifiable(discover(&dir, SuiteDefaults::default(), None)) else {
            return;
        };
        let names: Vec<&str> = tests.iter().map(|t| t.name.as_str()).collect();
        assert_eq!(names, vec!["a", "b"], "discovery must sort by filename");
        assert_eq!(
            tests[0].test.golden,
            dir.join("goldens")
                .join(crate::golden::platform_dir().unwrap())
                .join("a.png"),
            "goldens pair per-OS: goldens/<os>/<stem>.png"
        );

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn discover_applies_filter() {
        let dir =
            std::env::temp_dir().join(format!("ludion_testkit_filter_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("sprites.yaml"), "format: 1\nentities: []\n").unwrap();
        std::fs::write(dir.join("lights.yaml"), "format: 1\nentities: []\n").unwrap();

        let Some(tests) = verifiable(discover(&dir, SuiteDefaults::default(), Some("spr"))) else {
            return;
        };
        assert_eq!(tests.len(), 1);
        assert_eq!(tests[0].name, "sprites");

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_malformed_manifest_is_a_manifest_error_not_an_io_error() {
        let dir =
            std::env::temp_dir().join(format!("ludion_testkit_badmanifest_{}", std::process::id()));
        std::fs::create_dir_all(dir.join("goldens")).unwrap();
        std::fs::write(dir.join("goldens").join("manifest.json"), "{ not json").unwrap();

        let Err(error) = discover(&dir, SuiteDefaults::default(), None) else {
            panic!("a malformed manifest must fail discovery");
        };
        assert_eq!(error.code, code::GOLDEN_MANIFEST_INVALID);

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn manifest_overrides_the_suite_default_threshold() {
        let dir =
            std::env::temp_dir().join(format!("ludion_testkit_manifest_{}", std::process::id()));
        std::fs::create_dir_all(dir.join("goldens")).unwrap();
        std::fs::write(dir.join("scene.yaml"), "format: 1\nentities: []\n").unwrap();
        std::fs::write(
            dir.join("goldens").join("manifest.json"),
            r#"{"scene": {"ssim_threshold": 0.5}}"#,
        )
        .unwrap();

        let Some(tests) = verifiable(discover(&dir, SuiteDefaults::default(), None)) else {
            return;
        };
        assert_eq!(tests.len(), 1);
        assert_eq!(tests[0].test.ssim_threshold, 0.5);
        assert_eq!(
            tests[0].test.width,
            SuiteDefaults::default().width,
            "unset fields keep the default"
        );

        std::fs::remove_dir_all(&dir).ok();
    }

    /// Writes `scene_stems` as scenes and `manifest` as the manifest into a fresh scratch dir,
    /// returning `discover`'s outcome.
    fn discover_with_manifest(
        tag: &str,
        scene_stems: &[&str],
        manifest: &str,
    ) -> Result<Vec<DiscoveredTest>, Diagnostic> {
        let dir = std::env::temp_dir().join(format!(
            "ludion_testkit_strict_{tag}_{}",
            std::process::id()
        ));
        std::fs::create_dir_all(dir.join("goldens")).unwrap();
        for stem in scene_stems {
            std::fs::write(
                dir.join(format!("{stem}.yaml")),
                "format: 1\nentities: []\n",
            )
            .unwrap();
        }
        std::fs::write(dir.join("goldens").join("manifest.json"), manifest).unwrap();
        let outcome = discover(&dir, SuiteDefaults::default(), None);
        std::fs::remove_dir_all(&dir).ok();
        outcome
    }

    #[test]
    fn a_strict_manifest_refuses_what_would_pass_vacuously_or_never_apply() {
        for (tag, manifest) in [
            ("typo", r#"{"a": {"ssim_treshold": 0.9}}"#),
            ("zero", r#"{"a": {"ssim_threshold": 0}}"#),
            ("over", r#"{"a": {"ssim_threshold": 1.5}}"#),
            ("width", r#"{"a": {"width": 0}}"#),
            ("height", r#"{"a": {"height": 0}}"#),
            ("blank", r#"{"a": {"debug_draw": ["path", " "]}}"#),
        ] {
            let Err(error) = discover_with_manifest(tag, &["a"], manifest) else {
                panic!("{manifest} must be refused");
            };
            assert_eq!(error.code, code::GOLDEN_MANIFEST_INVALID, "{manifest}");
        }
    }

    #[test]
    fn a_manifest_key_with_no_scene_is_refused_by_name() {
        let Err(error) = discover_with_manifest("ghost", &["a"], r#"{"ghost": {"frames": 3}}"#)
        else {
            panic!("a key matching no scene must be refused");
        };
        assert_eq!(error.code, code::GOLDEN_MANIFEST_INVALID);
        assert!(error.message.contains("ghost"), "{}", error.message);
    }

    #[test]
    fn a_threshold_of_exactly_one_is_valid() {
        let Some(tests) = verifiable(discover_with_manifest(
            "one",
            &["a"],
            r#"{"a": {"ssim_threshold": 1.0}}"#,
        )) else {
            return;
        };
        assert_eq!(tests[0].test.ssim_threshold, 1.0);
    }

    #[test]
    fn manifest_applies_a_frames_override() {
        let dir = std::env::temp_dir().join(format!(
            "ludion_testkit_manifest_frames_{}",
            std::process::id()
        ));
        std::fs::create_dir_all(dir.join("goldens")).unwrap();
        std::fs::write(dir.join("scene.yaml"), "format: 1\nentities: []\n").unwrap();
        std::fs::write(
            dir.join("goldens").join("manifest.json"),
            r#"{"scene": {"frames": 23}}"#,
        )
        .unwrap();

        let Some(tests) = verifiable(discover(&dir, SuiteDefaults::default(), None)) else {
            return;
        };
        assert_eq!(tests.len(), 1);
        assert_eq!(tests[0].test.frames, 23);

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn manifest_applies_a_debug_draw_override() {
        let tests = verifiable(discover_with_manifest(
            "debug_draw",
            &["scene", "plain"],
            r#"{"scene": {"debug_draw": ["colliders"]}}"#,
        ))
        .expect("the manifest is valid");
        let drawn = |stem: &str| {
            let found = tests.iter().find(|t| t.name == stem).unwrap();
            found.test.debug_draw.clone()
        };
        assert_eq!(
            drawn("scene"),
            ludion_engine::DebugCategories::parse("colliders")
        );
        assert_eq!(drawn("plain"), ludion_engine::DebugCategories::None);
    }

    #[test]
    fn manifest_applies_an_anti_aliasing_override() {
        let tests = verifiable(discover_with_manifest(
            "anti_aliasing",
            &["scene", "plain"],
            r#"{"scene": {"anti_aliasing": "smaa"}}"#,
        ))
        .expect("the manifest is valid");
        let mode = |stem: &str| {
            tests
                .iter()
                .find(|t| t.name == stem)
                .unwrap()
                .test
                .anti_aliasing
        };
        assert_eq!(mode("scene"), ludion_engine::AntiAliasing::Smaa);
        assert_eq!(mode("plain"), ludion_engine::AntiAliasing::Off);
    }

    #[test]
    fn manifest_refuses_an_unknown_anti_aliasing_mode() {
        let error = discover_with_manifest(
            "anti_aliasing_bad",
            &["scene"],
            r#"{"scene": {"anti_aliasing": "fxaa"}}"#,
        )
        .err()
        .expect("fxaa is not a mode");
        assert_eq!(error.code, code::GOLDEN_MANIFEST_INVALID);
        assert!(error.message.contains("anti_aliasing"), "{}", error.message);
        assert!(error.message.contains("fxaa"), "{}", error.message);
    }

    #[test]
    fn an_absent_frames_override_extracts_at_rest() {
        let dir = std::env::temp_dir().join(format!(
            "ludion_testkit_manifest_no_frames_{}",
            std::process::id()
        ));
        std::fs::create_dir_all(dir.join("goldens")).unwrap();
        std::fs::write(dir.join("scene.yaml"), "format: 1\nentities: []\n").unwrap();

        let Some(tests) = verifiable(discover(&dir, SuiteDefaults::default(), None)) else {
            return;
        };
        assert_eq!(tests.len(), 1);
        assert_eq!(tests[0].test.frames, 0, "no manifest means tick 0");

        std::fs::remove_dir_all(&dir).ok();
    }
}
