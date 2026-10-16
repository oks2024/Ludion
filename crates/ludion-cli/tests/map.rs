//! Integration tests for `ludion generate-map` and `ludion map-check` (map-authoring Wave 2), driven
//! against the built binary. The generator's per-code and diff-locality coverage lives in
//! `ludion-scene`'s Wave 1 unit tests; these tests cover the CLI surface: file-level determinism,
//! default output naming, a representative pair of error exits, the snapshot-tested `map-check`
//! report, and the cross-district gate-link (`L1027`) check.

use std::path::PathBuf;
use std::process::{Command as ProcCommand, Output};

fn fixtures() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

/// A stable (not per-process) scratch dir, cleared each run — same rationale as `tests/import.rs`.
fn scratch(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(name);
    if let Err(e) = std::fs::remove_dir_all(&dir) {
        assert!(
            e.kind() == std::io::ErrorKind::NotFound,
            "stale scratch dir {dir:?} could not be removed: {e}"
        );
    }
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn run_ludion(args: &[&str]) -> Output {
    ProcCommand::new(env!("CARGO_BIN_EXE_ludion"))
        .args(args)
        .output()
        .expect("failed to run the ludion binary")
}

/// The report's first line is `map-check <absolute, OS-specific path>`; pin it to a token so the
/// golden is machine-independent.
fn normalize_report(stdout: &str) -> String {
    match stdout.split_once('\n') {
        Some((_first, rest)) => format!("map-check <MAP>\n{rest}"),
        None => stdout.to_string(),
    }
}

// --- generate-map --------------------------------------------------------------------------------

/// The fixture map + kit + fragment compiles, and two independent runs write byte-identical scenes
/// (file-level determinism — the property the drift gate leans on). The generated scene carries its
/// `generated_from:` provenance and merges the fragment's entities first.
#[test]
fn generate_map_is_byte_deterministic() {
    let map = fixtures().join("district.map.yaml");
    let out = scratch("generate_determinism");
    let a = out.join("a.yaml");
    let b = out.join("b.yaml");

    for dest in [&a, &b] {
        let output = run_ludion(&[
            "generate-map",
            map.to_str().unwrap(),
            "--out",
            dest.to_str().unwrap(),
        ]);
        assert!(
            output.status.success(),
            "generate-map failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    let first = std::fs::read(&a).unwrap();
    let second = std::fs::read(&b).unwrap();
    assert_eq!(first, second, "two runs must produce byte-identical scenes");

    let text = String::from_utf8(first).unwrap();
    assert!(
        text.contains("generated_from: district.map.yaml\n"),
        "the generated scene must carry its provenance: {text}"
    );
    // The fragment (singletons.yaml) is merged verbatim first: its camera precedes any grid entity.
    let camera = text.find("name: camera").expect("fragment camera present");
    let floor = text
        .find("name: floor-")
        .expect("a grid floor entity present");
    assert!(camera < floor, "fragment entities must be emitted first");
}

/// With no `--out`, the scene is written beside the map with the `.map` suffix dropped
/// (`district.map.yaml` → `district.yaml`), and matches an explicit `--out` generation byte-for-byte.
#[test]
fn generate_map_default_output_naming() {
    let out = scratch("generate_default_naming");
    for f in ["kit.yaml", "singletons.yaml", "district.map.yaml"] {
        std::fs::copy(fixtures().join(f), out.join(f)).unwrap();
    }
    let map = out.join("district.map.yaml");

    let output = run_ludion(&["generate-map", map.to_str().unwrap()]);
    assert!(
        output.status.success(),
        "generate-map failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let default_out = out.join("district.yaml");
    assert!(
        default_out.exists(),
        "the default output `district.yaml` must be written beside the map"
    );

    let explicit = out.join("explicit.yaml");
    run_ludion(&[
        "generate-map",
        map.to_str().unwrap(),
        "--out",
        explicit.to_str().unwrap(),
    ]);
    assert_eq!(
        std::fs::read(&default_out).unwrap(),
        std::fs::read(&explicit).unwrap(),
        "default-named output must equal the explicit `--out` output"
    );
}

/// A map naming a kit that does not exist exits non-zero with `L1015` (IO) on stderr and writes no
/// scene.
#[test]
fn generate_map_missing_kit_is_l1015() {
    let map = fixtures().join("missing-kit.map.yaml");
    let out = scratch("generate_missing_kit").join("out.yaml");
    let output = run_ludion(&[
        "generate-map",
        map.to_str().unwrap(),
        "--out",
        out.to_str().unwrap(),
    ]);
    assert!(!output.status.success(), "a missing kit must fail");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("error[L1015]"), "expected L1015: {stderr}");
    assert!(!out.exists(), "a failed generate must not write a scene");
}

/// A marker naming a kind the kit does not define exits non-zero with `L1024` and its suggestion.
#[test]
fn generate_map_bad_marker_kind_is_l1024() {
    let map = fixtures().join("bad-marker-kind.map.yaml");
    let out = scratch("generate_bad_marker").join("out.yaml");
    let output = run_ludion(&[
        "generate-map",
        map.to_str().unwrap(),
        "--out",
        out.to_str().unwrap(),
    ]);
    assert!(!output.status.success(), "an unknown marker kind must fail");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("error[L1024]"), "expected L1024: {stderr}");
    assert!(
        stderr.contains("nearest match is `well`"),
        "L1024 must suggest the nearest kind: {stderr}"
    );
    assert!(!out.exists(), "a failed generate must not write a scene");
}

// --- map-check -----------------------------------------------------------------------------------

/// The full annotated report is snapshot-tested against a committed text golden. The fixture is built
/// so every section has content: blocked walls, an unreachable pocket, two markers (a collider-less
/// torch and the anchor well), and one intended-walkable cell blocked by the well's prop collider.
#[test]
fn map_check_report_matches_golden() {
    let map = fixtures().join("district.map.yaml");
    let output = run_ludion(&["map-check", map.to_str().unwrap()]);
    assert!(
        output.status.success(),
        "map-check exited non-zero: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let actual = normalize_report(&String::from_utf8_lossy(&output.stdout));
    let golden = std::fs::read_to_string(fixtures().join("report.txt")).unwrap();
    assert_eq!(
        actual, golden,
        "map-check report drifted from tests/fixtures/report.txt"
    );
}

/// A gate whose `to` resolves to no sibling map or scene exits non-zero with `L1027` on stderr, while
/// the report still prints on stdout.
#[test]
fn map_check_dangling_gate_is_l1027() {
    let map = fixtures().join("gate-dangling.map.yaml");
    let output = run_ludion(&["map-check", map.to_str().unwrap()]);
    assert!(!output.status.success(), "a dangling gate must fail");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("error[L1027]"), "expected L1027: {stderr}");
    assert!(
        String::from_utf8_lossy(&output.stdout).contains("gate links: 1 checked, 1 dangling"),
        "the report still prints on stdout"
    );
}

/// A gate whose `to` resolves to a sibling scene carrying the named anchor entity exits 0.
#[test]
fn map_check_healthy_gate_exits_zero() {
    let map = fixtures().join("gate-healthy.map.yaml");
    let output = run_ludion(&["map-check", map.to_str().unwrap()]);
    assert!(
        output.status.success(),
        "a resolving gate must pass: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains("gate links: 1 checked, 0 dangling"),);
}

// --- map-check: --at query -----------------------------------------------------------------------

/// `--at x,z` appends a focused block: the cell the point falls in, that cell's walkable/blocked
/// verdict (from the same occupancy probe), and the nearest marker by Euclidean distance.
#[test]
fn map_check_at_reports_cell_verdict_and_nearest() {
    let map = fixtures().join("district.map.yaml");

    // A blocked cell: the well marker's own cell (7, 11) -> cell (c3,r5), blocked by its collider.
    let blocked = run_ludion(&["map-check", map.to_str().unwrap(), "--at", "7,11"]);
    let out = String::from_utf8_lossy(&blocked.stdout);
    assert!(
        out.contains("at (7, 11): cell (c3,r5)  blocked  nearest marker `town-well` 0.0u away"),
        "expected the blocked-cell --at line, got:\n{out}"
    );

    // A walkable cell: interior floor at (3, 3) -> cell (c1,r1); nearest marker is north-torch (6u).
    let walkable = run_ludion(&["map-check", map.to_str().unwrap(), "--at", "3,3"]);
    let out = String::from_utf8_lossy(&walkable.stdout);
    assert!(
        out.contains("at (3, 3): cell (c1,r1)  walkable  nearest marker `north-torch` 6.0u away"),
        "expected the walkable-cell --at line, got:\n{out}"
    );
}

// --- map-check: placement lints (L1028 / L1029) --------------------------------------------------

/// A `Gate` the anchor cannot reach (it stands in a second room, walled off from the first marker's)
/// raises `L1028` as a warning, without changing the exit code (the resolving gate keeps L1027 quiet).
#[test]
fn map_check_sealed_gate_is_l1028_warning() {
    let map = fixtures().join("lint-sealed-gate.map.yaml");
    let output = run_ludion(&["map-check", map.to_str().unwrap()]);
    assert!(
        output.status.success(),
        "L1028 is a warning - the command still exits zero: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("warning[L1028]") && stderr.contains("walled-gate"),
        "expected a warning[L1028] naming the unreachable gate: {stderr}"
    );
}

/// A marker whose spawn point sits inside *another* prop's collider raises `L1029`; the prop that
/// blocks its own cell (the well) does not (self-exclusion). A warning - exit unchanged.
#[test]
fn map_check_marker_on_foreign_collider_is_l1029_warning() {
    let map = fixtures().join("lint-blocked-marker.map.yaml");
    let output = run_ludion(&["map-check", map.to_str().unwrap()]);
    assert!(
        output.status.success(),
        "L1029 is a warning - exit stays zero"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("warning[L1029]") && stderr.contains("buried"),
        "expected a warning[L1029] naming the buried marker: {stderr}"
    );
    assert!(
        !stderr.contains("obstacle"),
        "the prop blocking its OWN cell must be excluded (self-collider): {stderr}"
    );
}

/// A marker on a walkable cell the reachability anchor can never reach (a sealed interior pocket)
/// raises `L1029`. A warning - exit unchanged.
#[test]
fn map_check_marker_in_sealed_pocket_is_l1029_warning() {
    let map = fixtures().join("lint-pocket.map.yaml");
    let output = run_ludion(&["map-check", map.to_str().unwrap()]);
    assert!(
        output.status.success(),
        "L1029 is a warning - exit stays zero"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("warning[L1029]")
            && stderr.contains("stranded")
            && stderr.contains("pocket"),
        "expected a warning[L1029] for the stranded pocket marker: {stderr}"
    );
}

/// A clean map - a building marker on its own collider, a torch on clear floor, and a gate on a real
/// perimeter gap - raises neither `L1028` nor `L1029`.
#[test]
fn map_check_clean_map_has_no_lints() {
    let map = fixtures().join("lint-clean.map.yaml");
    let output = run_ludion(&["map-check", map.to_str().unwrap()]);
    assert!(output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !stderr.contains("warning[L1028]")
            && !stderr.contains("warning[L1029]")
            && !stderr.contains("warning[L1040]"),
        "a clean map must raise no placement or seal lints: {stderr}"
    );
}

/// A 0.5 walker fits `lint-clean`'s 2 m door, which a collider (`door-cap`) closes behind its gate:
/// sealed. The same map without the cap and a 3.0 trigger covering the whole door still leaks: a game
/// may let a player pass a trigger without crossing it, so a trigger never seals its opening.
#[test]
fn map_check_a_gate_trigger_never_seals_its_opening() {
    let (status, out, err) = map_check("lint-clean.map.yaml");
    assert!(status.success(), "{err}");
    assert!(
        out.contains("seal: sealed from first marker `town-well`"),
        "{out}"
    );
    assert!(!out.contains("gate triggers"), "{out}");

    let (status, out, err) = map_check("lint-leaky-gate.map.yaml");
    assert!(status.success(), "{err}");
    assert!(err.contains("warning[L1040]"), "{err}");
    assert!(err.contains("the walker can leave the map"), "{err}");
    assert!(out.contains("seal: LEAKS — warning L1040"), "{out}");
}

/// `seal-gap`: a wall gap 1.1 wide (z 2.55-3.65) between two plugs. A 0.5 walker fits with 0.05 to
/// spare (centre line z 3.05-3.15), but the reachability lattice (9 samples a cell, step 2/9, at
/// z 3.0 and 3.222 around it) has no sample in that band, so it reads the gap as closed and the
/// pocket west of it as `outside`. The seal flood (step 0.125, shrunk radius 0.41, samples at z 3.0625
/// and 3.1875 in the band) finds the way out, and the full-radius flood does too (a wide leak); the
/// walk first enters a blocked reachability sample at the gap mouth, near (4.2, 3.1).
#[test]
fn map_check_seal_finds_a_gap_the_reachability_lattice_misses() {
    let (status, out, err) = map_check("seal-gap.map.yaml");
    assert!(status.success(), "{err}");
    assert!(
        out.contains("(blank) outside"),
        "the r-walk should read the gap as closed:
{out}"
    );
    assert!(out.contains("seal: LEAKS — warning L1040"), "{out}");
    assert!(
        err.contains("warning[L1040]: the walker can leave the map: from first marker `spawn`"),
        "{err}"
    );
    assert!(
        err.contains("a walker-wide way out: it leaves the report's walkable area near (4.2, 3.1)"),
        "a 1.1 gap fits the 1.0 walker, so the full-radius flood leaks too: {err}"
    );
}

/// `seal-narrow`: the same wall with a 0.9 gap (z 2.6125-3.5125), narrower than the 1.0 walker but wider
/// than the 0.82 shrunk one, with a seal sample at its centre line (z 3.0625). The shrunk flood
/// leaks, the full-radius one does not: the warning says the walker may fit.
#[test]
fn map_check_seal_classifies_a_gap_narrower_than_the_walker() {
    let (status, out, err) = map_check("seal-narrow.map.yaml");
    assert!(status.success(), "{err}");
    assert!(out.contains("seal: LEAKS — warning L1040"), "{out}");
    assert!(
        err.contains("it squeezes through a gap near (4.2, 3.1) between 0.82 and about 1.18 wide"),
        "{err}"
    );
}

/// A `Residue`-marked marker with no explicit `name:` raises `L1032` (its identity is the
/// index-derived fallback name, `torch-0`); a second `Residue`-marked marker WITH an explicit
/// `name:` (`memory-anchor`) stays silent. A warning - exit unchanged, and exactly one hit.
#[test]
fn map_check_unnamed_residue_marker_is_l1032_warning() {
    let map = fixtures().join("lint-residue-unnamed.map.yaml");
    let output = run_ludion(&["map-check", map.to_str().unwrap()]);
    assert!(
        output.status.success(),
        "L1032 is a warning - exit stays zero: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("warning[L1032]")
            && stderr.contains("torch-0")
            && stderr.contains("/markers/0"),
        "expected a warning[L1032] naming the unnamed marker's derived name: {stderr}"
    );
    assert!(
        !stderr.contains("memory-anchor"),
        "the NAMED Residue marker must stay silent: {stderr}"
    );
    assert_eq!(
        stderr.matches("warning[L1032]").count(),
        1,
        "only the unnamed marker should raise L1032: {stderr}"
    );
}

// --- map-check: --traverse / --speed / L1031 -----------------------------------------------------

/// `--traverse` measures the shortest walkable gate-to-gate path and prints it as cells → world units
/// → seconds plus the gabarit verdict. The fixture is a 46-cell corridor at `cell_size 4.0`, so the
/// 45-cell path is exactly 180 u — the plan's transition reference, 30.0 s at the default 6 u/s.
#[test]
fn map_check_traverse_reports_cells_units_seconds_and_gabarit() {
    let map = fixtures().join("traverse.map.yaml");
    let output = run_ludion(&["map-check", map.to_str().unwrap(), "--traverse"]);
    assert!(
        output.status.success(),
        "the traverse fixture must pass: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let out = String::from_utf8_lossy(&output.stdout);
    assert!(
        out.contains("traverse: speed 6 u/s, 2 gates, 1 pair\n")
            && out.contains(
                "  gate-east -> gate-west  45 cells  180 u  30.0 s  transition (~30 s)\n"
            ),
        "expected the traverse table, got:\n{out}"
    );
}

/// `--speed` scales the seconds proportionally (the cells and world units never move) and can carry
/// the crossing into another gabarit: 180 u is 30.0 s at 6 u/s, 60.0 s at 3 u/s.
#[test]
fn map_check_traverse_speed_scales_seconds() {
    let map = fixtures().join("traverse.map.yaml");
    let output = run_ludion(&[
        "map-check",
        map.to_str().unwrap(),
        "--traverse",
        "--speed",
        "3",
    ]);
    assert!(output.status.success());
    let out = String::from_utf8_lossy(&output.stdout);
    assert!(
        out.contains("traverse: speed 3 u/s, 2 gates, 1 pair\n")
            && out.contains(
                "  gate-east -> gate-west  45 cells  180 u  60.0 s  standard (60-90 s)\n"
            ),
        "expected the rescaled traverse table, got:\n{out}"
    );
}

/// Two gates separated by a solid wall raise `L1031`, and the traverse row says so instead of a
/// distance. A warning, like the other occupancy-probe heuristics — the exit code stays zero.
#[test]
fn map_check_bisected_map_is_l1031_warning() {
    let map = fixtures().join("traverse-bisected.map.yaml");
    let output = run_ludion(&["map-check", map.to_str().unwrap(), "--traverse"]);
    assert!(
        output.status.success(),
        "L1031 is a warning - exit stays zero"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("warning[L1031]")
            && stderr.contains("gate-east")
            && stderr.contains("gate-west"),
        "expected a warning[L1031] naming both gates: {stderr}"
    );
    assert!(
        String::from_utf8_lossy(&output.stdout)
            .contains("  gate-east -> gate-west  no walkable path (L1031)\n"),
        "the traverse row must name the missing route"
    );
}

/// `L1031` is a correctness lint, not a report section: it fires without `--traverse`, which only
/// decides whether the table prints. The clean corridor stays silent either way.
#[test]
fn map_check_l1031_fires_without_traverse() {
    let bisected = run_ludion(&[
        "map-check",
        fixtures()
            .join("traverse-bisected.map.yaml")
            .to_str()
            .unwrap(),
    ]);
    let stderr = String::from_utf8_lossy(&bisected.stderr);
    assert!(
        stderr.contains("warning[L1031]"),
        "L1031 must fire with no --traverse: {stderr}"
    );
    assert!(
        !String::from_utf8_lossy(&bisected.stdout).contains("traverse:"),
        "the traverse table prints only under --traverse"
    );

    let joined = run_ludion(&[
        "map-check",
        fixtures().join("traverse.map.yaml").to_str().unwrap(),
    ]);
    assert!(
        !String::from_utf8_lossy(&joined.stderr).contains("L1031"),
        "a map whose gates reach each other raises no L1031"
    );
}

/// `--speed` alone is meaningless, so it is a usage error (clap `requires`), not a silent no-op.
#[test]
fn map_check_speed_without_traverse_is_a_usage_error() {
    let map = fixtures().join("traverse.map.yaml");
    let output = run_ludion(&["map-check", map.to_str().unwrap(), "--speed", "3"]);
    assert!(!output.status.success(), "--speed alone must fail");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("--traverse") && stderr.contains("Usage:"),
        "expected a clap usage error naming --traverse: {stderr}"
    );
}

/// A non-positive or non-finite `--speed` would divide the seconds into nonsense; it is refused up
/// front, once for the whole invocation, exactly as a malformed `--at` is.
#[test]
fn map_check_non_positive_speed_is_a_usage_error() {
    let map = fixtures().join("traverse.map.yaml");
    let output = run_ludion(&[
        "map-check",
        map.to_str().unwrap(),
        "--traverse",
        "--speed",
        "0",
    ]);
    assert!(!output.status.success(), "--speed 0 must fail");
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("--speed must be a positive number"),
        "expected the readable --speed message"
    );
}

/// A map with a single gate has no pair to measure; the table says so rather than printing nothing.
#[test]
fn map_check_traverse_with_one_gate_says_so() {
    let map = fixtures().join("gate-healthy.map.yaml");
    let output = run_ludion(&["map-check", map.to_str().unwrap(), "--traverse"]);
    assert!(output.status.success());
    let out = String::from_utf8_lossy(&output.stdout);
    assert!(
        out.contains("traverse: speed 6 u/s, 1 gate, 0 pairs\n")
            && out.contains("(a gate-to-gate crossing needs two or more Gate markers)"),
        "expected the empty-pair note, got:\n{out}"
    );
}

/// `--traverse` applies to every map of a batch invocation, in turn.
#[test]
fn map_check_traverse_applies_to_every_map_of_a_batch() {
    let output = run_ludion(&[
        "map-check",
        fixtures().join("traverse.map.yaml").to_str().unwrap(),
        fixtures()
            .join("traverse-bisected.map.yaml")
            .to_str()
            .unwrap(),
        "--traverse",
    ]);
    assert!(output.status.success());
    let out = String::from_utf8_lossy(&output.stdout);
    assert!(
        out.contains("  gate-east -> gate-west  45 cells  180 u  30.0 s  transition (~30 s)\n")
            && out.contains("  gate-east -> gate-west  no walkable path (L1031)\n"),
        "expected both maps' traverse tables, got:\n{out}"
    );
}

// --- generate-map / map-check batch --------------------------------------------------------------

/// Copies the shared district fixture set (map + kit + fragment) into `dir` under the given map stems,
/// returning the copied map paths - for batch/`--check` tests that must write beside the source.
fn stage_maps(dir: &std::path::Path, stems: &[&str]) -> Vec<PathBuf> {
    std::fs::copy(fixtures().join("kit.yaml"), dir.join("kit.yaml")).unwrap();
    std::fs::copy(
        fixtures().join("singletons.yaml"),
        dir.join("singletons.yaml"),
    )
    .unwrap();
    let src = std::fs::read_to_string(fixtures().join("district.map.yaml")).unwrap();
    stems
        .iter()
        .map(|stem| {
            let path = dir.join(format!("{stem}.map.yaml"));
            std::fs::write(&path, &src).unwrap();
            path
        })
        .collect()
}

/// `generate-map a b` compiles both maps, each to its own default output path.
#[test]
fn generate_map_batch_writes_both_outputs() {
    let dir = scratch("generate_batch");
    let maps = stage_maps(&dir, &["one", "two"]);
    let output = run_ludion(&[
        "generate-map",
        maps[0].to_str().unwrap(),
        maps[1].to_str().unwrap(),
    ]);
    assert!(
        output.status.success(),
        "batch generate failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        dir.join("one.yaml").exists(),
        "first default output written"
    );
    assert!(
        dir.join("two.yaml").exists(),
        "second default output written"
    );
}

/// `map-check a b` prints a report for each input.
#[test]
fn map_check_batch_prints_both_reports() {
    let dir = scratch("mapcheck_batch");
    let maps = stage_maps(&dir, &["alpha", "beta"]);
    let output = run_ludion(&[
        "map-check",
        maps[0].to_str().unwrap(),
        maps[1].to_str().unwrap(),
    ]);
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("alpha.map.yaml") && stdout.contains("beta.map.yaml"),
        "both reports must print: {stdout}"
    );
}

/// A batch where one input fails (a missing kit) exits non-zero, even though the other input is fine.
#[test]
fn generate_map_batch_one_failing_input_is_nonzero() {
    let dir = scratch("generate_batch_fail");
    let good = stage_maps(&dir, &["good"]).remove(0);
    let bad = fixtures().join("missing-kit.map.yaml");
    let output = run_ludion(&[
        "generate-map",
        good.to_str().unwrap(),
        bad.to_str().unwrap(),
    ]);
    assert!(
        !output.status.success(),
        "a batch with any failing input must exit non-zero"
    );
}

/// `--out` names a single destination, so it is a usage error with more than one input map.
#[test]
fn generate_map_out_with_two_inputs_is_usage_error() {
    let dir = scratch("generate_out_conflict");
    let maps = stage_maps(&dir, &["one", "two"]);
    let out = dir.join("out.yaml");
    let output = run_ludion(&[
        "generate-map",
        maps[0].to_str().unwrap(),
        maps[1].to_str().unwrap(),
        "--out",
        out.to_str().unwrap(),
    ]);
    assert!(!output.status.success(), "--out with two inputs must fail");
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("--out"),
        "the usage error must mention --out"
    );
    assert!(!out.exists(), "no output is written on the usage error");
}

// --- generate-map --check ------------------------------------------------------------------------

/// `generate-map --check` against a committed-matching scene prints `up to date`, exits zero, and
/// leaves the file's bytes untouched.
#[test]
fn generate_map_check_up_to_date_writes_nothing() {
    let dir = scratch("check_up_to_date");
    let map = stage_maps(&dir, &["district"]).remove(0);
    // Generate the committed scene first.
    assert!(
        run_ludion(&["generate-map", map.to_str().unwrap()])
            .status
            .success()
    );
    let out = dir.join("district.yaml");
    let before = std::fs::read(&out).unwrap();

    let output = run_ludion(&["generate-map", map.to_str().unwrap(), "--check"]);
    assert!(
        output.status.success(),
        "an up-to-date scene passes --check: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        String::from_utf8_lossy(&output.stdout).contains("up to date"),
        "expected `up to date`"
    );
    assert_eq!(
        std::fs::read(&out).unwrap(),
        before,
        "--check must not write the file"
    );
}

/// `generate-map --check` against a source that no longer regenerates the committed scene prints
/// `DRIFT`, exits non-zero, and writes nothing.
#[test]
fn generate_map_check_drift_is_nonzero_and_writes_nothing() {
    let dir = scratch("check_drift");
    let map = stage_maps(&dir, &["district"]).remove(0);
    assert!(
        run_ludion(&["generate-map", map.to_str().unwrap()])
            .status
            .success()
    );
    let out = dir.join("district.yaml");
    let committed = std::fs::read(&out).unwrap();

    // Drift the source: add a marker so it now generates a different scene than the committed one.
    let mut src = std::fs::read_to_string(&map).unwrap();
    src.push_str("  - name: extra-torch\n    kind: torch\n    x: 5.0\n    z: 5.0\n");
    std::fs::write(&map, &src).unwrap();

    let output = run_ludion(&["generate-map", map.to_str().unwrap(), "--check"]);
    assert!(!output.status.success(), "a drifted source fails --check");
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("DRIFT")
            && String::from_utf8_lossy(&output.stderr).contains("L1039"),
        "expected `DRIFT` and `L1039` on stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        std::fs::read(&out).unwrap(),
        committed,
        "--check must not overwrite the committed scene, even on drift"
    );
}

// --- map-check: the probe walker (kit `probe:` / --radius / --speed) ------------------------------

/// The walker resolves flag → the kit's `probe:` block → the built-in default, per field, and the
/// report header names where each number came from. The probe fixture's kit asks for radius 1.5 and
/// speed 3, so the default report is measured at the content's own numbers, not at `0.5` / `6`.
#[test]
fn map_check_probe_resolves_flag_then_kit_then_default() {
    let with_probe = fixtures().join("probe.map.yaml");
    let out =
        String::from_utf8_lossy(&run_ludion(&["map-check", with_probe.to_str().unwrap()]).stdout)
            .into_owned();
    assert!(
        out.contains("probe: radius 1.5 (kit), speed 3 u/s (kit)\n"),
        "the kit's probe block must be the default source, got:\n{out}"
    );

    // A flag beats the kit, per field: `--radius` moves only the radius.
    let out = String::from_utf8_lossy(
        &run_ludion(&[
            "map-check",
            with_probe.to_str().unwrap(),
            "--radius",
            "0.25",
        ])
        .stdout,
    )
    .into_owned();
    assert!(
        out.contains("probe: radius 0.25 (flag), speed 3 u/s (kit)\n"),
        "--radius must beat the kit, got:\n{out}"
    );

    // A kit with no `probe:` block falls through to the built-in defaults.
    let without_probe = fixtures().join("traverse.map.yaml");
    let out = String::from_utf8_lossy(
        &run_ludion(&["map-check", without_probe.to_str().unwrap()]).stdout,
    )
    .into_owned();
    assert!(
        out.contains("probe: radius 0.5 (default), speed 6 u/s (default)\n"),
        "a kit with no probe block falls back to the defaults, got:\n{out}"
    );
}

/// The kit's `probe.speed` reaches `--traverse`'s seconds (the same 180 u corridor the flag test
/// measures), and the table states the connectivity of its measure.
#[test]
fn map_check_traverse_uses_the_kit_speed_and_states_its_connectivity() {
    let map = fixtures().join("probe.map.yaml");
    let output = run_ludion(&["map-check", map.to_str().unwrap(), "--traverse"]);
    assert!(output.status.success());
    let out = String::from_utf8_lossy(&output.stdout);
    assert!(
        out.contains("traverse: speed 3 u/s, 2 gates, 1 pair\n")
            && out.contains("4-connected path"),
        "expected the kit-speed traverse table with its connectivity note, got:\n{out}"
    );
}

// --- map-check: real geometry (N1 / T4) ----------------------------------------------------------

/// The report's stdout and the lint stderr of one `map-check` run.
fn map_check(fixture: &str) -> (std::process::ExitStatus, String, String) {
    let map = fixtures().join(fixture);
    let output = run_ludion(&["map-check", map.to_str().unwrap()]);
    (
        output.status,
        String::from_utf8_lossy(&output.stdout).into_owned(),
        String::from_utf8_lossy(&output.stderr).into_owned(),
    )
}

/// A prefab instance's collider and a parented collider block the cells they sit on in the world,
/// and the summary counts the expanded colliders. Cell centre = (2c+1, 2r+1) and the 0.5 probe reaches
/// a 0.9 box only in its own cell, so the oracle is hand-derived: the post's collider is local (1,1)
/// under a parent at (4,4) -> world (5,5) = (c2,r2); the instance at (9,1) -> (c4,r0). The `bar`
/// instance is yawed +90 about Y, which carries its arm's local +X to world -Z: the arm sits at world
/// (6,3) with its box *unrotated* (the 2D index ignores rotation), x 4.1-7.9 by z 2.1-3.9, which
/// covers every sample of (c2,r1) and (c3,r1). The 3D box collider is the solver's: counted, not probed.
#[test]
fn map_check_probes_prefab_instances_and_parented_colliders() {
    let (status, out, err) = map_check("prefab-occupancy.map.yaml");
    assert!(status.success(), "{err}");
    for row in ["0  ....#.", "   ..##..", "   ..#...", "   A....."] {
        assert!(
            out.contains(&format!(
                "
{row}
"
            )),
            "missing grid row `{row}` in:
{out}"
        );
    }
    assert!(
        out.contains("blocked 4  walkable 20  unreachable 0"),
        "{out}"
    );
    assert!(
        out.contains(
            "colliders 4, prefab instances 2 (3 parts), 1 not probed (3D or rigid — the physics solver's)"
        ),
        "{out}"
    );
}

/// A prefab path that does not resolve is an expansion error in the *fragment* that instances it, so
/// the diagnostic names that file and carries the fragment-local pointer, not the map and a pointer
/// into the generated scene.
#[test]
fn map_check_missing_prefab_fails_at_the_fragment() {
    let (status, _, err) = map_check("prefab-missing.map.yaml");
    assert!(!status.success(), "a missing prefab fails the check");
    assert!(
        err.contains("no-such.prefab.yaml")
            && err.contains("/entities/1/prefab")
            && err.contains("prefab-missing-frag.yaml"),
        "expected the error at the fragment, got:
{err}"
    );
}

/// A collider whose `body` names no variant fails the scene check (`L1014`) before the probe
/// runs, instead of printing an all-walkable grid.
#[test]
fn map_check_malformed_collider_is_refused_before_the_probe() {
    let (status, out, err) = map_check("unmeasured.map.yaml");
    assert!(!status.success(), "{out}");
    assert!(err.contains("error[L1014]"), "{err}");
    assert!(
        !out.contains("legend:"),
        "no grid for a refused map:
{out}"
    );
}

/// A marker with its own collider is a footprint, not a spawn: standing inside that collider in a
/// region the anchor never reaches (the cell keeps free samples around the post) raises no `L1029`.
#[test]
fn map_check_marker_inside_its_own_collider_is_not_a_sealed_pocket() {
    let (status, _, err) = map_check("lint-own-collider.map.yaml");
    assert!(status.success(), "{err}");
    assert!(
        !err.contains("L1029"),
        "unexpected L1029:
{err}"
    );
}

/// A gate standing in a wall between a sealed pocket (north) and the corridor the anchor walks
/// (south) is measured from the reached side: no `L1031`, no `L1028`, a real distance.
#[test]
fn map_check_gate_approach_prefers_the_reached_side() {
    let map = fixtures().join("gate-approach.map.yaml");
    let output = run_ludion(&["map-check", map.to_str().unwrap(), "--traverse"]);
    let (out, err) = (
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
    assert!(output.status.success(), "{err}");
    assert!(
        !err.contains("L1031") && !err.contains("warning[L1028]"),
        "{err}"
    );
    assert!(
        out.lines()
            .any(|l| l.contains("gate-a -> gate-b") && l.contains(" cells ")),
        "the gate-a -> gate-b row must be measured:
{out}"
    );
}

/// The walker fits a gap the cell centres miss: two wells either side of a 1.6-wide gap on the
/// boundary between the two rows (z 1.2-2.8 free; the walker needs 1.0), so every cell centre in the
/// well columns is blocked yet the rooms join. Reachability floods a sub-cell lattice, so the gap
/// cells print as walkable and the gates reach each other.
#[test]
fn map_check_sub_cell_gap_is_walkable() {
    let (status, out, err) = map_check("sub-cell-gap.map.yaml");
    assert!(status.success(), "{err}");
    assert!(
        !err.contains("L1031") && !err.contains("warning[L1028]"),
        "{err}"
    );
    for row in ["0  A..M..", "   ...M.M"] {
        assert!(
            out.contains(&format!(
                "
{row}
"
            )),
            "missing grid row `{row}` in:
{out}"
        );
    }
    assert!(
        out.contains(
            "blocked 0  walkable 12  unreachable 0
"
        ),
        "{out}"
    );
}

/// A gate in the middle of a map is not sealed just because no edge cell is near it: the anchor
/// walks to it.
#[test]
fn map_check_reachable_interior_gate_is_not_sealed() {
    let (status, out, err) = map_check("lint-interior-gate.map.yaml");
    assert!(status.success(), "{err}");
    assert!(
        !err.contains("warning[L1028]"),
        "unexpected L1028:\n{err}\n{out}"
    );
}

/// A gate on the grid edge that the anchor cannot reach is sealed, edge gap or not.
#[test]
fn map_check_unreachable_edge_gate_is_sealed() {
    let (status, _, err) = map_check("lint-cut-off-gate.map.yaml");
    assert!(status.success(), "L1028 is a warning: {err}");
    assert!(
        err.contains("warning[L1028]") && err.contains("far-gate"),
        "expected a warning[L1028] naming far-gate: {err}"
    );
}

/// Void beyond the walls is *outside*, not walkable and not unreachable; a void pocket walled in on
/// all sides is still a sealed `?`. 45 cells: 17 blocked, 3 reached floor, 1 enclosed void, 24 outside.
#[test]
fn map_check_void_beyond_walls_is_outside() {
    let (status, out, err) = map_check("void-ring.map.yaml");
    assert!(status.success(), "{err}");
    assert!(
        out.contains("blocked 17  walkable 4  unreachable 1  outside 24"),
        "{out}"
    );
    assert!(out.contains("#.A.#?#"), "{out}");
}

/// A kit collider with a negative extent fails both verbs with the scene check's `L1116` — it
/// used to panic `map-check` in the probe — and `generate-map` writes nothing.
#[test]
fn map_with_negative_collider_extent_is_l1116_in_both_verbs() {
    let map = fixtures().join("bad-extent.map.yaml");
    let out = scratch("negative_extent").join("out.yaml");
    let generated = run_ludion(&[
        "generate-map",
        map.to_str().unwrap(),
        "--out",
        out.to_str().unwrap(),
    ]);
    assert!(!generated.status.success());
    let stderr = String::from_utf8_lossy(&generated.stderr);
    assert!(stderr.contains("error[L1116]"), "expected L1116: {stderr}");
    assert!(!out.exists(), "a failed generate must not write a scene");

    let checked = run_ludion(&["map-check", map.to_str().unwrap()]);
    assert!(!checked.status.success());
    let stderr = String::from_utf8_lossy(&checked.stderr);
    assert!(stderr.contains("error[L1116]"), "expected L1116: {stderr}");
    assert!(
        !stderr.contains("panicked"),
        "map-check must not panic: {stderr}"
    );
}

/// A validation fault in a generated entity is reported where it was written: a fragment's against
/// the fragment file at its local pointer, a kit tile's against the map at the row that expanded it —
/// never a pointer into the generated scene, which is not a file.
#[test]
fn generated_scene_faults_are_reported_at_their_source() {
    let fragment = fixtures().join("bad-extent-frag.map.yaml");
    let kit = fixtures().join("bad-extent.map.yaml");
    let out = scratch("fault_origin").join("out.yaml");
    for verb in ["generate-map", "map-check"] {
        let mut args = vec![verb, fragment.to_str().unwrap()];
        if verb == "generate-map" {
            args.extend(["--out", out.to_str().unwrap()]);
        }
        let stderr = String::from_utf8_lossy(&run_ludion(&args).stderr).into_owned();
        assert!(
            stderr.contains("error[L1116]")
                && stderr.contains("--> /entities/1/components/Collider/shape/aabb/half_extents/0")
                && stderr.contains("bad-extent-frag.yaml: 1 error found"),
            "{verb}: expected L1116 against the fragment file:\n{stderr}"
        );

        let stderr = String::from_utf8_lossy(&run_ludion(&[verb, kit.to_str().unwrap()]).stderr)
            .into_owned();
        assert!(
            stderr.contains("error[L1116]")
                && stderr.contains("--> /rows/")
                && !stderr.contains("--> /entities/")
                && stderr.contains("bad-extent.map.yaml:"),
            "{verb}: expected L1116 against the map at a row:\n{stderr}"
        );
    }
}

/// A thin wall splits the middle cell: the anchor reaches its west half, so the cell counts as
/// reached, yet the marker on the east half is in a sealed pocket. Hand-derived: cell 8 wide, probe
/// 0.5, wall x 11.9–12.1 across the full depth, so samples within 0.5 of it (x 11.4–12.6) are blocked
/// and the east half (x 12.6–16) joins nothing the anchor walks.
#[test]
fn map_check_marker_in_a_split_cells_sealed_half_is_l1029_warning() {
    let (status, _, err) = map_check("split-cell.map.yaml");
    assert!(status.success(), "L1029 is a warning: {err}");
    // The open fixture also leaks (L1040, which names the anchor), printed after the placement lints.
    let l1029 = err.split("warning[L1040]").next().unwrap();
    assert!(
        err.contains("warning[L1029]") && err.contains("cut-off") && err.contains("pocket"),
        "expected a warning[L1029] for `cut-off`: {err}"
    );
    assert!(
        !l1029.contains("`divider`") && !l1029.contains("`anchor-torch`"),
        "only the cut-off marker is stranded: {l1029}"
    );
}

/// A `Gate` whose `to` is empty (a built-dark map's unbuilt target) is timed like any other gate and
/// labelled inert; the live pair's row is the one `traverse.map.yaml` prints.
#[test]
fn map_check_traverse_times_inert_gates_and_labels_them() {
    let map = fixtures().join("traverse-inert.map.yaml");
    let output = run_ludion(&["map-check", map.to_str().unwrap(), "--traverse"]);
    assert!(
        output.status.success(),
        "an inert gate is not an error: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let out = String::from_utf8_lossy(&output.stdout);
    let row = |pair: &str| out.lines().find(|l| l.contains(pair)).unwrap_or("");
    assert!(
        out.contains("traverse: speed 6 u/s, 3 gates, 3 pairs\n"),
        "expected three gates and pairs, got:\n{out}"
    );
    let live = row("gate-east -> gate-west");
    assert!(
        live.ends_with("180 u  30.0 s  transition (~30 s)"),
        "the live pair's row must be unchanged, got: {live}"
    );
    for pair in ["gate-east -> gate-mid", "gate-mid  -> gate-west"] {
        let line = row(pair);
        assert!(
            line.contains(" s  ") && line.ends_with("[inert gate: empty `to`]"),
            "the inert gate must be timed and labelled, got: {line}"
        );
    }
}
