//! Integration tests for `ludion render --dump-voxels` (`Plan/gi-wave0-voxelizer.md` §11, WP-E),
//! driven against the built binary.
//!
//! Unlike the ordinary `render` tests in `render.rs`, none of these skip on a missing GPU adapter:
//! `--dump-voxels` is pure CPU end to end (`ludion_engine::render_voxel_sheet`), and when `--out` is
//! omitted the command never requests an adapter at all — these tests are exactly the proof that it
//! runs to completion regardless.

use std::path::PathBuf;
use std::process::{Command, Output};

fn out_path(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(name)
}

fn gallery_scene(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../examples/gallery")
        .join(name)
}

fn run_ludion(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_ludion"))
        .args(args)
        .output()
        .expect("failed to run the ludion binary")
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

/// A scratch scene authoring a `GiVolume` (entity 0) and one colored cube (entity 1) sitting on the
/// origin — small enough that the grid gets upscaled for legibility, and simple enough that its one
/// occupied cell's color is exactly the cube's authored albedo.
fn write_gi_cube_scene(path: &std::path::Path, cell_size: f32) {
    let yaml = format!(
        "format: 1\n\
         entities:\n\
         \x20\x20- name: gi\n\
         \x20\x20\x20\x20components:\n\
         \x20\x20\x20\x20\x20\x20GiVolume:\n\
         \x20\x20\x20\x20\x20\x20\x20\x20cell_size: {cell_size}\n\
         \x20\x20- name: cube\n\
         \x20\x20\x20\x20components:\n\
         \x20\x20\x20\x20\x20\x20Transform:\n\
         \x20\x20\x20\x20\x20\x20\x20\x20translation:\n\
         \x20\x20\x20\x20\x20\x20\x20\x20\x20\x20- 0.0\n\
         \x20\x20\x20\x20\x20\x20\x20\x20\x20\x20- 0.5\n\
         \x20\x20\x20\x20\x20\x20\x20\x20\x20\x20- 0.0\n\
         \x20\x20\x20\x20\x20\x20Mesh:\n\
         \x20\x20\x20\x20\x20\x20\x20\x20shape: cube\n\
         \x20\x20\x20\x20\x20\x20\x20\x20color:\n\
         \x20\x20\x20\x20\x20\x20\x20\x20\x20\x20- 0.8\n\
         \x20\x20\x20\x20\x20\x20\x20\x20\x20\x20- 0.2\n\
         \x20\x20\x20\x20\x20\x20\x20\x20\x20\x20- 0.1\n\
         \x20\x20\x20\x20\x20\x20\x20\x20\x20\x20- 1.0\n"
    );
    std::fs::write(path, yaml).expect("scratch scene writes");
}

/// The `"grid ExEyEz, N occupied cell(s)"` clause of `run_dump_voxels`'s summary line, split into
/// the extent text (`"ExEyEz"`) and the occupied count.
fn grid_summary(stdout: &str) -> (String, usize) {
    let after_grid = stdout
        .split("grid ")
        .nth(1)
        .unwrap_or_else(|| panic!("summary line names the grid: {stdout}"));
    let mut parts = after_grid.splitn(2, ", ");
    let extent = parts.next().unwrap().to_string();
    let count = parts
        .next()
        .unwrap_or_else(|| panic!("summary line names the occupied count: {stdout}"))
        .split(' ')
        .next()
        .unwrap();
    (extent, count.parse().expect("occupied count is a number"))
}

/// The sRGB8 encoding `voxel_sheet::srgb8` computes for a linear channel — duplicated here through
/// the same pure-Rust `libm` `powf`, with a 1-ULP-scale tolerance in the pixel search below so the
/// test does not couple to an exact last-bit match.
fn expected_albedo_srgb(linear: [f32; 3]) -> [u8; 3] {
    let encode =
        |c: f32| (ludion_core::math::powf(c.clamp(0.0, 1.0), 1.0 / 2.2) * 255.0).round() as u8;
    [encode(linear[0]), encode(linear[1]), encode(linear[2])]
}

/// A near-match search: is there any pixel within 2 of `target` on every channel? Guards against a
/// possible last-bit gamma difference between the test's encoding and the engine's, while still being
/// a real assertion that the cube's color reached the sheet.
fn any_pixel_near(rgba: &[u8], target: [u8; 3]) -> bool {
    rgba.chunks_exact(4)
        .any(|px| (0..3).all(|c| (i32::from(px[c]) - i32::from(target[c])).abs() <= 2))
}

/// A tiny scene with a `GiVolume` and one cube produces a PNG whose dimensions match the printed
/// summary, with at least one occupied cell and a pixel carrying the cube's own albedo — the filled
/// pixel the spec asks for. Run with **no `--out`**, so this also proves the command completes with
/// no GPU adapter ever requested.
#[test]
fn dump_voxels_produces_a_png_with_the_cube_filled() {
    let scene = out_path("gi_cube_scene.yaml");
    write_gi_cube_scene(&scene, 0.5);
    let voxels_out = out_path("gi_cube_voxels.png");

    let output = run_ludion(&[
        "render",
        "--scene",
        scene.to_str().unwrap(),
        "--dump-voxels",
        voxels_out.to_str().unwrap(),
    ]);
    assert!(
        output.status.success(),
        "dump-voxels failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    let (_, occupied) = grid_summary(&stdout);
    assert!(
        occupied > 0,
        "the cube must occupy at least one cell: {stdout}"
    );

    let (w, h, rgba) = decode_png(&voxels_out);
    assert!(w > 0 && h > 0, "the sheet must not be empty");
    assert!(
        stdout.contains(&format!("voxel sheet {w}x{h}")),
        "the summary line must name the PNG's actual dimensions: {stdout}"
    );

    let expected = expected_albedo_srgb([0.8, 0.2, 0.1]);
    assert!(
        any_pixel_near(&rgba, expected),
        "expected a pixel near the cube's albedo {expected:?} somewhere in the sheet"
    );
}

/// Two runs over the same scene write byte-identical PNGs — the pure-CPU determinism contract.
#[test]
fn dump_voxels_is_deterministic() {
    let scene = out_path("gi_cube_scene_det.yaml");
    write_gi_cube_scene(&scene, 0.5);
    let first = out_path("gi_cube_voxels_1.png");
    let second = out_path("gi_cube_voxels_2.png");

    let run = |out: &std::path::Path| {
        run_ludion(&[
            "render",
            "--scene",
            scene.to_str().unwrap(),
            "--dump-voxels",
            out.to_str().unwrap(),
        ])
    };

    let a = run(&first);
    assert!(a.status.success(), "{}", String::from_utf8_lossy(&a.stderr));
    let b = run(&second);
    assert!(b.status.success(), "{}", String::from_utf8_lossy(&b.stderr));

    assert_eq!(
        std::fs::read(&first).unwrap(),
        std::fs::read(&second).unwrap(),
        "two dump-voxels runs over the same scene must write byte-identical PNGs",
    );
    // The two invocations write to different filenames by construction, so the summary lines'
    // paths differ; the grid/occupied-cell clause after the path must still match exactly.
    let (a_extent, a_occupied) = grid_summary(&String::from_utf8_lossy(&a.stdout));
    let (b_extent, b_occupied) = grid_summary(&String::from_utf8_lossy(&b.stdout));
    assert_eq!(
        (a_extent, a_occupied),
        (b_extent, b_occupied),
        "the reported grid must also be deterministic"
    );
}

/// A scene with no `GiVolume` reports `L2104` — readable, not a panic, and no PNG left behind (a
/// silent empty file would look like the dump ran and found nothing, which is a different claim).
#[test]
fn dump_voxels_without_a_gi_volume_is_a_readable_error() {
    let scene = gallery_scene("meshes.yaml");
    let voxels_out = out_path("gi_missing_voxels.png");
    let _ = std::fs::remove_file(&voxels_out);

    let output = run_ludion(&[
        "render",
        "--scene",
        scene.to_str().unwrap(),
        "--dump-voxels",
        voxels_out.to_str().unwrap(),
    ]);
    assert!(!output.status.success(), "expected a non-zero exit");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("error[L2104]"), "{stderr}");
    assert!(
        stderr.contains("authors no `GiVolume`"),
        "the message must explain what's missing: {stderr}"
    );
    assert!(
        !voxels_out.exists(),
        "a failed dump must not leave a PNG behind"
    );
}

/// A `GiVolume` whose only mesh is `GiDynamic` builds no grid: `L2111`, not `L2104`, and no PNG left
/// behind.
#[test]
fn dump_voxels_of_an_empty_gi_volume_is_a_readable_error() {
    let scene = out_path("gi_empty_volume.scene.yaml");
    std::fs::write(
        &scene,
        concat!(
            "format: 1
",
            "entities:
",
            "  - name: gi
",
            "    components:
",
            "      GiVolume:
",
            "        cell_size: 0.5
",
            "  - name: camera
",
            "    components:
",
            "      Transform: {}
",
            "      Camera: {}
",
            "  - name: crate
",
            "    components:
",
            "      Transform: {}
",
            "      Mesh:
",
            "        shape: cube
",
            "      GiDynamic: {}
",
        ),
    )
    .expect("scratch scene writes");
    let voxels_out = out_path("gi_empty_voxels.png");
    let _ = std::fs::remove_file(&voxels_out);

    let output = run_ludion(&[
        "render",
        "--scene",
        scene.to_str().unwrap(),
        "--dump-voxels",
        voxels_out.to_str().unwrap(),
    ]);
    assert!(!output.status.success(), "expected a non-zero exit");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("error[L2111]"), "{stderr}");
    assert!(stderr.contains("1 `GiDynamic`"), "{stderr}");
    assert!(
        !voxels_out.exists(),
        "a failed dump must not leave a PNG behind"
    );
}

/// `--set` cannot create a `GiVolume` the scene lacks — the same `L4008` an unresolvable pointer
/// always gets, not a special-cased success, and no PNG is written.
#[test]
fn set_cannot_create_a_missing_gi_volume() {
    let scene = gallery_scene("meshes.yaml");
    let voxels_out = out_path("gi_set_create_voxels.png");
    let _ = std::fs::remove_file(&voxels_out);

    let output = run_ludion(&[
        "render",
        "--scene",
        scene.to_str().unwrap(),
        "--dump-voxels",
        voxels_out.to_str().unwrap(),
        "--set",
        "/entities/0/components/GiVolume/cell_size=0.25",
    ]);
    assert!(!output.status.success(), "expected a non-zero exit");
    assert!(!voxels_out.exists());
}

/// `--set` **tunes** `cell_size` on a `GiVolume` the scene already authors — the documented
/// workaround for a scene without one authored inline: a finer cell size implies a larger grid
/// extent, visible in the summary line's `grid ExEyEz`.
#[test]
fn set_tunes_cell_size_on_an_existing_gi_volume() {
    let scene = out_path("gi_cube_scene_set.yaml");
    write_gi_cube_scene(&scene, 1.0);
    let voxels_out = out_path("gi_set_voxels.png");

    let coarse = run_ludion(&[
        "render",
        "--scene",
        scene.to_str().unwrap(),
        "--dump-voxels",
        voxels_out.to_str().unwrap(),
    ]);
    assert!(
        coarse.status.success(),
        "{}",
        String::from_utf8_lossy(&coarse.stderr)
    );
    let coarse_stdout = String::from_utf8_lossy(&coarse.stdout).into_owned();

    let fine = run_ludion(&[
        "render",
        "--scene",
        scene.to_str().unwrap(),
        "--dump-voxels",
        voxels_out.to_str().unwrap(),
        "--set",
        "/entities/0/components/GiVolume/cell_size=0.25",
    ]);
    assert!(
        fine.status.success(),
        "{}",
        String::from_utf8_lossy(&fine.stderr)
    );
    let fine_stdout = String::from_utf8_lossy(&fine.stdout).into_owned();

    assert!(
        fine_stdout.contains("set /entities/0/components/GiVolume/cell_size: 1 -> 0.25"),
        "the override must be echoed: {fine_stdout}"
    );
    let (coarse_extent, _) = grid_summary(&coarse_stdout);
    let (fine_extent, _) = grid_summary(&fine_stdout);
    assert_ne!(
        coarse_extent, fine_extent,
        "a finer cell_size must change the reported grid extent: coarse={coarse_stdout} \
         fine={fine_stdout}"
    );
}

/// `--dump-voxels --out` still writes the ordinary rendered PNG too — the dump is additive, not a
/// replacement for `--out`. Skips gracefully if the machine has no GPU adapter (the `--out` half
/// alone needs one; `--dump-voxels` itself never does).
#[test]
fn needs_gpu_dump_voxels_alongside_out_writes_both_files() {
    let scene = out_path("gi_cube_scene_both.yaml");
    write_gi_cube_scene(&scene, 0.5);
    let png_out = out_path("gi_cube_both.png");
    let voxels_out = out_path("gi_cube_both_voxels.png");

    let output = run_ludion(&[
        "render",
        "--scene",
        scene.to_str().unwrap(),
        "--out",
        png_out.to_str().unwrap(),
        "--dump-voxels",
        voxels_out.to_str().unwrap(),
        "--width",
        "64",
        "--height",
        "64",
    ]);
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        if ludion_core::require_gpu::skip_no_adapter(stderr.contains("error[L2003]")) {
            eprintln!("SKIPPED dump_voxels_alongside_out_writes_both_files: no GPU adapter");
            return;
        }
        panic!("render failed: {stderr}");
    }
    assert!(
        png_out.exists(),
        "the ordinary --out PNG must still be written"
    );
    assert!(
        voxels_out.exists(),
        "the voxel-sheet PNG must also be written"
    );
}

/// `--dump-timings` without `--out` is a clear clap-level error, not a confusing panic — it needs
/// the full GPU render `--out` produces and has nothing to time without one.
#[test]
fn dump_timings_without_out_is_a_clear_error() {
    let scene = out_path("gi_cube_scene_timings.yaml");
    write_gi_cube_scene(&scene, 0.5);

    let output = run_ludion(&[
        "render",
        "--scene",
        scene.to_str().unwrap(),
        "--dump-timings",
        out_path("gi_cube_timings.json").to_str().unwrap(),
    ]);
    assert!(!output.status.success(), "expected a non-zero exit");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("--out"),
        "the error must name the missing argument: {stderr}"
    );
}

/// `L2104` has a working `ludion explain` entry.
#[test]
fn gi_volume_missing_code_is_explainable() {
    let output = run_ludion(&["explain", "L2104"]);
    assert!(
        output.status.success(),
        "`ludion explain L2104` failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

/// `L2111` — a `GiVolume` that covers nothing — has a working `ludion explain` entry.
#[test]
fn gi_volume_empty_code_is_explainable() {
    let output = run_ludion(&["explain", "L2111"]);
    assert!(
        output.status.success(),
        "`ludion explain L2111` failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

/// `L2105` — the irradiance probe budget — has a working `ludion explain` entry naming the field
/// that raises it.
#[test]
fn gi_volume_probe_budget_code_is_explainable() {
    let output = run_ludion(&["explain", "L2105"]);
    assert!(
        output.status.success(),
        "`ludion explain L2105` failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("probe_spacing"),
        "the explanation must name the authored field: {stdout}"
    );
}
