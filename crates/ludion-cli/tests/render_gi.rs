//! Integration tests for `ludion render --dump-gi` (`Plan/gi-wave1-sky-visibility.md` §10, WP-7),
//! driven against the built binary.
//!
//! Unlike `render_voxels.rs`'s `--dump-voxels` tests, the acceptance test here **does** skip
//! gracefully when the machine has no GPU adapter (the `gpu_render.rs`/`render_voxels.rs` pattern for
//! a GPU-touching CLI test): `--dump-gi` reads a *baked* probe volume back from the GPU, which
//! `--dump-voxels` never does. The missing-`GiVolume` case is still CPU-detectable before any adapter
//! is requested, so its test never skips.

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

/// Whether a failed `render` was due to a missing GPU adapter (`L2003`), in which case a GPU-touching
/// test skips rather than fails.
fn is_no_adapter(output: &Output) -> bool {
    ludion_core::require_gpu::skip_no_adapter(
        String::from_utf8_lossy(&output.stderr).contains("error[L2003]"),
    )
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

/// One `write_room_scene` `PointLight`, `GiDynamic`-marked or not — WP-8's own fixture vocabulary
/// (`Plan/gi-wave3-dynamic-injection.md` §11 T11): a marked light is the DYNAMIC block's data, an
/// unmarked one is the split census's other half.
struct LightSpec {
    name: &'static str,
    position: [f32; 3],
    color: [f32; 3],
    intensity: f32,
    range: f32,
    marked: bool,
}

/// A scratch GI scene: a closed room (floor, roof, four walls) with a doorway gap in the north wall,
/// a `GiVolume`, and a bright `Environment` — built here rather than reused from
/// `examples/gallery/gi-interior.yaml` because that calibration scene is being authored in parallel
/// (`Plan/gi-wave1-sky-visibility.md` WP-5) and this test must not depend on it landing first.
///
/// Geometry: an interior footprint `4x4` (X, Z) and `3` tall, walls `0.75` thick so a `0.25` cell size
/// reliably voxelizes them (three cell-widths of margin against lattice-alignment luck), floor/roof
/// `0.3` thick. The north wall (max Z) is split into two segments leaving an `x ∈ [-0.8, 0.8]` gap —
/// the doorway. `Environment` is inert to the bake itself (sky visibility is purely geometric — see
/// `gi_bake.rs`'s module doc), but present anyway to match the calibration scene's shape.
///
/// `bounce` is `GiVolume.bounce` verbatim. At `0.0` (every pre-existing call site) the scene is
/// exactly what it always was: no `bounce` bake, no emissive geometry, so those tests' assertions
/// stay valid unchanged. A `bounce > 0.0` additionally authors a wall-mounted emissive "torch"
/// (`gi-interior.yaml`'s own warm color and `emissive: 4.0`) against the west wall, giving `--dump-gi`
/// WP-6's BOUNCE block and the bounce-lit cross-process render test (`Plan/gi-wave2-emissive-bounce.md`
/// §9 T11/T12) something to bounce — tied to the same switch so a bounce-off scene never carries dead
/// geometry with nothing reading it. `lights` (WP-8, `Plan/gi-wave3-dynamic-injection.md` §11 T11)
/// authors zero or more `PointLight` entities beside the torch — `LightSpec::marked` gives each one a
/// `GiDynamic: {}` sibling or not, so a caller can build the DYNAMIC block's fixtures (a marked light
/// lighting the room, the split census, the two content-driven absent reasons) on this same geometry
/// rather than a second scene writer.
fn write_room_scene(
    path: &std::path::Path,
    cell_size: f32,
    probe_spacing: f32,
    bounce: f32,
    lights: &[LightSpec],
) {
    // The strict-subset scene grammar forbids flow-style sequences (`[1, 2]`), so every vec3/vec4
    // is one block-style item per line — `v3`/`v4`/`cube_entity` below build that shape once instead
    // of repeating it per wall.
    fn v3(v: [f32; 3]) -> String {
        format!(
            "\n            - {}\n            - {}\n            - {}",
            v[0], v[1], v[2]
        )
    }
    fn v4(v: [f32; 4]) -> String {
        format!(
            "\n            - {}\n            - {}\n            - {}\n            - {}",
            v[0], v[1], v[2], v[3]
        )
    }
    fn cube_entity(name: &str, translation: [f32; 3], scale: [f32; 3]) -> String {
        format!(
            "  - name: {name}\n\
             \x20\x20\x20\x20components:\n\
             \x20\x20\x20\x20\x20\x20Transform:\n\
             \x20\x20\x20\x20\x20\x20\x20\x20translation:{}\n\
             \x20\x20\x20\x20\x20\x20\x20\x20scale:{}\n\
             \x20\x20\x20\x20\x20\x20Mesh:\n\
             \x20\x20\x20\x20\x20\x20\x20\x20shape: cube\n\
             \x20\x20\x20\x20\x20\x20\x20\x20color:{}\n",
            v3(translation),
            v3(scale),
            v4([0.6, 0.6, 0.6, 1.0]),
        )
    }
    // `cube_entity`'s twin with an `emissive` line — the room's optional torch, the one entity in this
    // scene whose radiance actually feeds the bounce bake (`gi_bake.rs` §4.2: `albedo * emissive`).
    fn emissive_cube_entity(
        name: &str,
        translation: [f32; 3],
        scale: [f32; 3],
        color: [f32; 4],
        emissive: f32,
    ) -> String {
        format!(
            "  - name: {name}\n\
             \x20\x20\x20\x20components:\n\
             \x20\x20\x20\x20\x20\x20Transform:\n\
             \x20\x20\x20\x20\x20\x20\x20\x20translation:{}\n\
             \x20\x20\x20\x20\x20\x20\x20\x20scale:{}\n\
             \x20\x20\x20\x20\x20\x20Mesh:\n\
             \x20\x20\x20\x20\x20\x20\x20\x20shape: cube\n\
             \x20\x20\x20\x20\x20\x20\x20\x20color:{}\n\
             \x20\x20\x20\x20\x20\x20\x20\x20emissive: {emissive}\n",
            v3(translation),
            v3(scale),
            v4(color),
        )
    }

    let mut yaml = String::new();
    yaml.push_str("format: 1\n");
    yaml.push_str("entities:\n");
    yaml.push_str(&format!(
        "  - name: environment\n\
         \x20\x20\x20\x20components:\n\
         \x20\x20\x20\x20\x20\x20Environment:\n\
         \x20\x20\x20\x20\x20\x20\x20\x20sky_zenith:{}\n\
         \x20\x20\x20\x20\x20\x20\x20\x20sky_horizon:{}\n\
         \x20\x20\x20\x20\x20\x20\x20\x20ibl_intensity: 1.5\n",
        v3([4.0, 4.5, 6.0]),
        v3([3.0, 3.0, 3.0]),
    ));
    yaml.push_str(&format!(
        "  - name: gi\n\
         \x20\x20\x20\x20components:\n\
         \x20\x20\x20\x20\x20\x20GiVolume:\n\
         \x20\x20\x20\x20\x20\x20\x20\x20cell_size: {cell_size}\n\
         \x20\x20\x20\x20\x20\x20\x20\x20probe_spacing: {probe_spacing}\n\
         \x20\x20\x20\x20\x20\x20\x20\x20bounce: {bounce}\n"
    ));
    yaml.push_str(&cube_entity("floor", [0.0, -0.15, 0.0], [5.5, 0.3, 5.5]));
    yaml.push_str(&cube_entity("roof", [0.0, 3.15, 0.0], [5.5, 0.3, 5.5]));
    yaml.push_str(&cube_entity(
        "wall_west",
        [-2.375, 1.5, 0.0],
        [0.75, 3.0, 5.5],
    ));
    yaml.push_str(&cube_entity(
        "wall_east",
        [2.375, 1.5, 0.0],
        [0.75, 3.0, 5.5],
    ));
    yaml.push_str(&cube_entity(
        "wall_south",
        [0.0, 1.5, -2.375],
        [5.5, 3.0, 0.75],
    ));
    yaml.push_str(&cube_entity(
        "wall_north_left",
        [-1.5875, 1.5, 2.375],
        [1.575, 3.0, 0.75],
    ));
    yaml.push_str(&cube_entity(
        "wall_north_right",
        [1.5875, 1.5, 2.375],
        [1.575, 3.0, 0.75],
    ));
    // The torch: wall-mounted against the west wall's interior face (`x = -2.0`), centered on Z so
    // it sits away from both the north doorway and the southeast corner — the room's other far
    // corner, which stays dim by distance/cosine falloff alone with no interior partition needed
    // (`gi-interior.yaml`'s own torch color and strength).
    if bounce > 0.0 {
        yaml.push_str(&emissive_cube_entity(
            "torch",
            [-1.9, 1.6, 0.0],
            [0.3, 0.6, 0.3],
            [1.0, 0.62, 0.28, 1.0],
            4.0,
        ));
    }
    for light in lights {
        yaml.push_str(&format!(
            "  - name: {}\n\
             \x20\x20\x20\x20components:\n\
             \x20\x20\x20\x20\x20\x20Transform:\n\
             \x20\x20\x20\x20\x20\x20\x20\x20translation:{}\n\
             \x20\x20\x20\x20\x20\x20PointLight:\n\
             \x20\x20\x20\x20\x20\x20\x20\x20color:{}\n\
             \x20\x20\x20\x20\x20\x20\x20\x20intensity: {}\n\
             \x20\x20\x20\x20\x20\x20\x20\x20range: {}\n\
             {}",
            light.name,
            v3(light.position),
            v3(light.color),
            light.intensity,
            light.range,
            if light.marked {
                "      GiDynamic: {}\n"
            } else {
                ""
            },
        ));
    }
    // Inside the room, looking at the doorway. Inert for the dump tests (the probe volume is a
    // function of geometry alone), load-bearing for the full-render one.
    yaml.push_str(&format!(
        "  - name: camera\n\
         \x20\x20\x20\x20components:\n\
         \x20\x20\x20\x20\x20\x20Transform:\n\
         \x20\x20\x20\x20\x20\x20\x20\x20translation:{}\n\
         \x20\x20\x20\x20\x20\x20\x20\x20rotation:{}\n\
         \x20\x20\x20\x20\x20\x20Camera:\n\
         \x20\x20\x20\x20\x20\x20\x20\x20projection:\n\
         \x20\x20\x20\x20\x20\x20\x20\x20\x20\x20type: perspective\n\
         \x20\x20\x20\x20\x20\x20\x20\x20\x20\x20fov_y_deg: 62.0\n",
        v3([0.0, 1.6, -1.6]),
        v3([-10.0, 180.0, 0.0]),
    ));

    std::fs::write(path, yaml).expect("scratch scene writes");
}

/// The `"probes AxBxC"` clause of `run_dump_gi`'s summary line.
fn probes_summary(stdout: &str) -> String {
    stdout
        .split("probes ")
        .nth(1)
        .unwrap_or_else(|| panic!("summary line names the probe lattice: {stdout}"))
        .split(|c: char| c.is_whitespace())
        .next()
        .unwrap()
        .trim_end_matches(',')
        .to_string()
}

/// A scene with no `GiVolume` reports `L2104` before any GPU adapter is ever requested — the CPU
/// check runs first, so this never needs to skip. No PNG is left behind.
#[test]
fn dump_gi_without_a_gi_volume_is_a_readable_error() {
    let scene = gallery_scene("meshes.yaml");
    let gi_out = out_path("gi_missing_dump.png");
    let _ = std::fs::remove_file(&gi_out);

    let output = run_ludion(&[
        "render",
        "--scene",
        scene.to_str().unwrap(),
        "--dump-gi",
        gi_out.to_str().unwrap(),
    ]);
    assert!(!output.status.success(), "expected a non-zero exit");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("error[L2104]"), "{stderr}");
    assert!(
        stderr.contains("authors no `GiVolume`"),
        "the message must explain what's missing: {stderr}"
    );
    assert!(
        !gi_out.exists(),
        "a failed dump must not leave a PNG behind"
    );
    // Reused, not a new code: WP-7 adds no diagnostic of its own.
    assert!(
        !stderr.contains("L2106"),
        "no new diagnostic code should exist for this case"
    );
}

/// The one honest difference from `--dump-voxels` is documented where an agent or human will read it.
#[test]
fn help_text_states_the_gpu_requirement() {
    let output = run_ludion(&["render", "--help"]);
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("--dump-gi"),
        "the flag must be listed: {stdout}"
    );
    // clap wraps help text, so match on a phrase rather than the whole sentence.
    assert!(
        stdout.contains("GPU adapter"),
        "the help text must call out the GPU requirement: {stdout}"
    );
}

/// **The acceptance check (§13's definition of done).** On a closed room with a doorway, the baked
/// VISIBILITY block shows real dynamic range: some probe near the doorway/exterior reads high
/// visibility while some probe deep in the room reads low visibility. Scanned as raw greyscale pixels
/// (`r == g == b`, which the sheet's background color never satisfies — see `voxel_sheet.rs`'s
/// `BACKGROUND`), so the assertion needs no knowledge of the sheet's internal tile layout.
///
/// Skips gracefully with no GPU adapter, matching the `gpu_render.rs`/`render_voxels.rs` pattern for
/// a GPU-touching test.
#[test]
fn needs_gpu_dump_gi_shows_dark_interior_and_bright_doorway_probes() {
    let scene = out_path("gi_room_scene.yaml");
    write_room_scene(&scene, 0.25, 1.0, 0.0, &[]);
    let gi_out = out_path("gi_room_dump.png");

    let output = run_ludion(&[
        "render",
        "--scene",
        scene.to_str().unwrap(),
        "--dump-gi",
        gi_out.to_str().unwrap(),
    ]);
    if !output.status.success() {
        if is_no_adapter(&output) {
            eprintln!(
                "SKIPPED dump_gi_shows_dark_interior_and_bright_doorway_probes: no GPU adapter"
            );
            return;
        }
        panic!(
            "dump-gi failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let counts = probes_summary(&stdout);
    let axes: Vec<u32> = counts.split('x').map(|c| c.parse().unwrap()).collect();
    assert_eq!(
        axes.len(),
        3,
        "the summary names a 3-axis lattice: {stdout}"
    );
    assert!(
        axes.iter().all(|&c| c >= 2),
        "every axis must have at least 2 probes for trilinear sampling: {counts}"
    );

    let (w, h, rgba) = decode_png(&gi_out);
    assert!(
        stdout.contains(&format!("GI sheet {w}x{h}")),
        "the summary line must name the PNG's actual dimensions: {stdout}"
    );

    // The VISIBILITY block is the left half of the sheet (the BENT NORMAL block sits to its right,
    // separated by a gap) — restricting the scan there avoids any chance of reading bent-normal
    // channel data as if it were greyscale.
    let half_w = w / 2;
    let mut min_visibility = 255u8;
    let mut max_visibility = 0u8;
    for row in 0..h {
        for col in 0..half_w {
            let idx = (row as usize * w as usize + col as usize) * 4;
            let px = &rgba[idx..idx + 4];
            // A genuine visibility pixel is grey (r == g == b); the sheet's background and label
            // colors are not (see `voxel_sheet.rs`'s `BACKGROUND` = [24, 26, 30, ..]), so this
            // filter finds only real probe data.
            if px[0] == px[1] && px[1] == px[2] {
                min_visibility = min_visibility.min(px[0]);
                max_visibility = max_visibility.max(px[0]);
            }
        }
    }

    assert!(
        max_visibility > 180,
        "a probe near the doorway/exterior must read high visibility (bright); max was \
         {max_visibility}"
    );
    assert!(
        min_visibility < 80,
        "a probe deep in the sealed room must read low visibility (dark); min was {min_visibility}"
    );
    assert!(
        max_visibility - min_visibility >= 100,
        "interior-dark and doorway-bright must be clearly distinct: min={min_visibility} \
         max={max_visibility}"
    );
}

/// Two runs over the same scene write byte-identical PNGs — the bake is deterministic *per machine*
/// (`gi_bake.rs`'s module doc), and this is the CLI-command-level proof of that for the dump path
/// specifically. Skips gracefully with no GPU adapter.
#[test]
fn needs_gpu_dump_gi_is_deterministic() {
    let scene = out_path("gi_room_scene_det.yaml");
    write_room_scene(&scene, 0.25, 1.0, 0.0, &[]);
    let first = out_path("gi_room_dump_1.png");
    let second = out_path("gi_room_dump_2.png");

    let run = |out: &std::path::Path| {
        run_ludion(&[
            "render",
            "--scene",
            scene.to_str().unwrap(),
            "--dump-gi",
            out.to_str().unwrap(),
        ])
    };

    let a = run(&first);
    if !a.status.success() {
        if is_no_adapter(&a) {
            eprintln!("SKIPPED dump_gi_is_deterministic: no GPU adapter");
            return;
        }
        panic!("{}", String::from_utf8_lossy(&a.stderr));
    }
    let b = run(&second);
    assert!(b.status.success(), "{}", String::from_utf8_lossy(&b.stderr));

    assert_eq!(
        std::fs::read(&first).unwrap(),
        std::fs::read(&second).unwrap(),
        "two dump-gi runs over the same scene must write byte-identical PNGs",
    );
}

/// **Bake stability (`Plan/gi-wave1-sky-visibility.md` §11 test 6).** Two separate OS processes
/// rendering the same GI scene write byte-identical **images** — the full path (voxelize → bake →
/// `mesh.wgsl`'s multiply), not just the dumped volume the test above compares. **Per-machine, not
/// cross-platform**: the bake is GPU float accumulation over a fixed ray order (`gi_bake.rs`'s
/// determinism note), sitting exactly where the IBL prefilter sits, off the bit-exact CPU tier.
///
/// It lives here rather than in `ludion-engine`'s GI fixture file because `CARGO_BIN_EXE_ludion` is
/// only set for this package's tests, and spawning `cargo build` from inside a test races the rest
/// of the parallel suite for the `ludion.exe` link target (observed: `os error 5` mid-suite while a
/// `test_cmd` subprocess held the binary open).
#[test]
fn needs_gpu_two_processes_render_the_same_gi_scene_identically() {
    let scene = out_path("gi_room_scene_render.yaml");
    write_room_scene(&scene, 0.25, 0.5, 0.0, &[]);
    let first = out_path("gi_room_render_1.png");
    let second = out_path("gi_room_render_2.png");

    let run = |out: &std::path::Path| {
        run_ludion(&[
            "render",
            "--scene",
            scene.to_str().unwrap(),
            "--out",
            out.to_str().unwrap(),
            "--width",
            "160",
            "--height",
            "90",
        ])
    };

    let a = run(&first);
    if !a.status.success() {
        if is_no_adapter(&a) {
            eprintln!("SKIPPED two_processes_render_the_same_gi_scene_identically: no GPU adapter");
            return;
        }
        panic!("{}", String::from_utf8_lossy(&a.stderr));
    }
    let b = run(&second);
    assert!(b.status.success(), "{}", String::from_utf8_lossy(&b.stderr));

    let bytes = std::fs::read(&first).unwrap();
    assert_eq!(
        bytes,
        std::fs::read(&second).unwrap(),
        "two separate OS processes baking and rendering the same GI scene must write \
         byte-identical PNGs",
    );
    assert_eq!(&bytes[1..4], b"PNG", "output is a PNG");
}

// --- WP-6 (`Plan/gi-wave2-emissive-bounce.md` §8, §9 T11/T12): the sheet's BOUNCE block, and
// WP-8 (`Plan/gi-wave3-dynamic-injection.md` §11 T11): the sheet's DYNAMIC block and header ---

/// Mirrors `voxel_sheet.rs`'s own layout constant — the sheet is four equal-width blocks
/// (VISIBILITY, BENT NORMAL, BOUNCE, DYNAMIC) separated by three gaps this wide, so a block's pixel
/// columns can be located from the PNG's width alone with no knowledge of the probe count.
const BLOCK_GAP: u32 = 24;
/// Mirrors `voxel_sheet.rs`'s own sheet background / "no data here" color.
const BACKGROUND: [u8; 4] = [24, 26, 30, 255];
/// Mirrors `voxel_sheet.rs`'s own `SHEET_HEADER_H` — the header strip above every block, carrying
/// the split census and the bake-cache status. A pixel scan restricted to `SHEET_HEADER_H..height`
/// can never pick up header-text pixels as if they were probe data, however wide the header's own
/// text runs relative to a block's width.
const SHEET_HEADER_H: u32 = 40;

/// The `[x0, x1)` column range of the `index`-th of four equal-width blocks (`0` = VISIBILITY,
/// `1` = BENT NORMAL, `2` = BOUNCE, `3` = DYNAMIC) within a `--dump-gi` sheet of width `w`, derived
/// from `w = 4*block_w + 3*BLOCK_GAP` the same way `voxel_sheet.rs::build_gi_sheet` derives each
/// block's own `_x0`.
fn block_x_range(w: u32, index: u32) -> (u32, u32) {
    let block_w = (w - 3 * BLOCK_GAP) / 4;
    let x0 = index * (block_w + BLOCK_GAP);
    (x0, x0 + block_w)
}

/// The `"bounce "` clause of `run_dump_gi`'s summary line (`"baked"` or `"not baked"`) — stops at
/// the next comma, since WP-8 grew the line with a `dynamic`/`lights`/`bake` tail beyond it.
fn bounce_summary(stdout: &str) -> String {
    stdout
        .split("bounce ")
        .nth(1)
        .unwrap_or_else(|| panic!("summary line names the bounce state: {stdout}"))
        .split(',')
        .next()
        .unwrap()
        .trim()
        .to_string()
}

/// The `"dynamic "` clause of `run_dump_gi`'s summary line — `"injected"` or
/// `"not injected (<reason>)"` — [`bounce_summary`]'s own convention, stopping at the next comma.
fn dynamic_summary(stdout: &str) -> String {
    stdout
        .split("dynamic ")
        .nth(1)
        .unwrap_or_else(|| panic!("summary line names the dynamic state: {stdout}"))
        .split(',')
        .next()
        .unwrap()
        .trim()
        .to_string()
}

/// **T12, the baked half.** On a bounce-lit scratch scene (one wall-mounted emissive torch), the
/// BOUNCE block reads materially brighter near the torch's own corner of the room than in the room's
/// far corner — the same "did the light actually reach here" read `dump_gi_shows_dark_interior_and_
/// bright_doorway_probes` does for VISIBILITY, applied to the new block. The scan restricts itself to
/// the BOUNCE block's own columns ([`block_x_range`]) below the header strip and skips exact
/// background pixels (gaps and unfilled grid slots), so it needs no knowledge of the sheet's
/// internal tile/label geometry.
///
/// Skips gracefully with no GPU adapter.
#[test]
fn needs_gpu_dump_gi_bounce_block_is_bright_near_the_emitter_and_dark_in_the_far_corner() {
    let scene = out_path("gi_room_scene_bounce.yaml");
    write_room_scene(&scene, 0.25, 0.5, 1.0, &[]);
    let gi_out = out_path("gi_room_bounce_dump.png");

    let output = run_ludion(&[
        "render",
        "--scene",
        scene.to_str().unwrap(),
        "--dump-gi",
        gi_out.to_str().unwrap(),
    ]);
    if !output.status.success() {
        if is_no_adapter(&output) {
            eprintln!(
                "SKIPPED dump_gi_bounce_block_is_bright_near_the_emitter_and_dark_in_the_far_corner: \
                 no GPU adapter"
            );
            return;
        }
        panic!(
            "dump-gi failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert_eq!(
        bounce_summary(&stdout),
        "baked",
        "a bounce > 0.0 scene must report a baked bounce volume: {stdout}"
    );

    let (w, h, rgba) = decode_png(&gi_out);
    let (x0, x1) = block_x_range(w, 2);

    let mut min_bright = 255u8;
    let mut max_bright = 0u8;
    for row in SHEET_HEADER_H..h {
        for col in x0..x1 {
            let idx = (row as usize * w as usize + col as usize) * 4;
            let px: [u8; 4] = rgba[idx..idx + 4].try_into().unwrap();
            if px == BACKGROUND {
                continue;
            }
            let bright = px[0].max(px[1]).max(px[2]);
            min_bright = min_bright.min(bright);
            max_bright = max_bright.max(bright);
        }
    }

    assert!(
        max_bright > 60,
        "a probe near the torch must read a materially bright bounce; max was {max_bright}"
    );
    assert!(
        max_bright - min_bright >= 40,
        "the torch's corner and the room's far corner must read clearly distinct: \
         min={min_bright} max={max_bright}"
    );
}

/// **T12, the not-baked half.** A `bounce: 0.0` scene bakes no bounce volume at all, and the summary
/// line says so plainly — the observable WP-6's brief calls out explicitly, chosen over decoding the
/// PNG a second time because the CLI's own words are the more direct proof that "not baked" is
/// communicated, not just structurally true of the data.
///
/// Skips gracefully with no GPU adapter.
#[test]
fn needs_gpu_dump_gi_summary_reports_bounce_not_baked_at_zero() {
    let scene = out_path("gi_room_scene_no_bounce.yaml");
    write_room_scene(&scene, 0.25, 0.5, 0.0, &[]);
    let gi_out = out_path("gi_room_no_bounce_dump.png");

    let output = run_ludion(&[
        "render",
        "--scene",
        scene.to_str().unwrap(),
        "--dump-gi",
        gi_out.to_str().unwrap(),
    ]);
    if !output.status.success() {
        if is_no_adapter(&output) {
            eprintln!("SKIPPED dump_gi_summary_reports_bounce_not_baked_at_zero: no GPU adapter");
            return;
        }
        panic!(
            "dump-gi failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert_eq!(
        bounce_summary(&stdout),
        "not baked",
        "a bounce: 0.0 scene must report no baked bounce volume: {stdout}"
    );
}

/// **T11.** Two separate OS processes rendering the same **bounce-lit** scene write byte-identical
/// images — `two_processes_render_the_same_gi_scene_identically`'s own proof, extended to the path
/// that now also runs the bounce bake's light/sun uniform, storage buffer, and three extra volume
/// writes. Per-machine, not cross-platform, exactly as its sibling.
///
/// Skips gracefully with no GPU adapter.
#[test]
fn needs_gpu_two_processes_render_the_same_bounce_lit_gi_scene_identically() {
    let scene = out_path("gi_room_scene_bounce_render.yaml");
    write_room_scene(&scene, 0.25, 0.5, 1.0, &[]);
    let first = out_path("gi_room_bounce_render_1.png");
    let second = out_path("gi_room_bounce_render_2.png");

    let run = |out: &std::path::Path| {
        run_ludion(&[
            "render",
            "--scene",
            scene.to_str().unwrap(),
            "--out",
            out.to_str().unwrap(),
            "--width",
            "160",
            "--height",
            "90",
        ])
    };

    let a = run(&first);
    if !a.status.success() {
        if is_no_adapter(&a) {
            eprintln!(
                "SKIPPED two_processes_render_the_same_bounce_lit_gi_scene_identically: no GPU adapter"
            );
            return;
        }
        panic!("{}", String::from_utf8_lossy(&a.stderr));
    }
    let b = run(&second);
    assert!(b.status.success(), "{}", String::from_utf8_lossy(&b.stderr));

    let bytes = std::fs::read(&first).unwrap();
    assert_eq!(
        bytes,
        std::fs::read(&second).unwrap(),
        "two separate OS processes baking and rendering the same bounce-lit GI scene must write \
         byte-identical PNGs",
    );
    assert_eq!(&bytes[1..4], b"PNG", "output is a PNG");
}

// --- WP-7 (`Plan/gi-wave3-dynamic-injection.md`, the injection stress scene section): the ---
// --- injection stress scene ---

/// **T10, the D11 cross-process pin at the cap.** Two separate `render` processes on
/// `examples/gallery/gi-inject-stress.yaml` (300 `GiDynamic`-marked lights - the `MAX_PROJECTILES`
/// cap, a clustered burst patch plus a spread field) write byte-identical PNGs. Mirrors
/// [`two_processes_render_the_same_gi_scene_identically`] exactly, aimed at the built-in gallery
/// scene instead of a scratch fixture, because the claim this proves is specific to the cap regime:
/// the `gi_inject` compute pass has no atomics and no cross-invocation reduction (`gi_inject.wgsl`'s
/// fixed `k`-ascending, bin-list-ascending order, the injection compute pass's own doc section in
/// `Plan/gi-wave3-dynamic-injection.md`), so GPU float accumulation over that fixed order is
/// per-machine deterministic even at 300 lights and a deep worst-case bin list - not just at the
/// handful of lights the smaller room fixtures exercise.
///
/// **Per-machine, not cross-platform** (the twin above's own note, restated because it is the same
/// claim): this is a GPU float-accumulation identity, gated by per-OS goldens, not a bit-exact CPU
/// guarantee. Lives here rather than in `ludion-engine`'s GI fixture files for the same
/// `CARGO_BIN_EXE_ludion` / in-test-`cargo build` race reason as its siblings above.
///
/// Skips gracefully with no GPU adapter.
#[test]
fn needs_gpu_two_processes_render_the_same_gi_inject_stress_scene_identically() {
    let scene = gallery_scene("gi-inject-stress.yaml");
    let first = out_path("gi_inject_stress_render_1.png");
    let second = out_path("gi_inject_stress_render_2.png");

    let run = |out: &std::path::Path| {
        run_ludion(&[
            "render",
            "--scene",
            scene.to_str().unwrap(),
            "--out",
            out.to_str().unwrap(),
            "--width",
            "160",
            "--height",
            "90",
        ])
    };

    let a = run(&first);
    if !a.status.success() {
        if is_no_adapter(&a) {
            eprintln!(
                "SKIPPED two_processes_render_the_same_gi_inject_stress_scene_identically: no GPU \
                 adapter"
            );
            return;
        }
        panic!("{}", String::from_utf8_lossy(&a.stderr));
    }
    let b = run(&second);
    assert!(b.status.success(), "{}", String::from_utf8_lossy(&b.stderr));

    let bytes = std::fs::read(&first).unwrap();
    assert_eq!(
        bytes,
        std::fs::read(&second).unwrap(),
        "two separate OS processes baking, injecting, and rendering the same 300-light GI stress \
         scene must write byte-identical PNGs",
    );
    assert_eq!(&bytes[1..4], b"PNG", "output is a PNG");
}

// --- WP-8 (`Plan/gi-wave3-dynamic-injection.md` §11 T11): the sheet's DYNAMIC block, the split
// census, and the three content-driven absent reasons ---

/// **T11, the injected half, on a mixed scene.** One unmarked `PointLight` (feeds the static bake)
/// beside one `GiDynamic`-marked light near the west wall (the torch's own corner, so this reuses
/// the BOUNCE fixture's proven "materially bright near the source" geometry): the summary reports
/// the split census correctly (`1 baked + 1 injected`) and `dynamic injected`, and the DYNAMIC
/// block's own columns read materially brighter near the marked light than the room's far corner —
/// [`dump_gi_bounce_block_is_bright_near_the_emitter_and_dark_in_the_far_corner`]'s own read, applied
/// to the new block.
///
/// Skips gracefully with no GPU adapter.
#[test]
fn needs_gpu_dump_gi_dynamic_block_is_bright_near_a_marked_light_on_a_mixed_scene() {
    let scene = out_path("gi_room_scene_dynamic.yaml");
    write_room_scene(
        &scene,
        0.25,
        0.5,
        1.0,
        &[
            LightSpec {
                name: "ceiling_lamp",
                position: [0.0, 2.8, -1.5],
                color: [0.8, 0.85, 1.0],
                intensity: 1.5,
                range: 2.5,
                marked: false,
            },
            LightSpec {
                name: "mover",
                position: [-1.7, 1.6, 0.3],
                color: [1.0, 0.62, 0.28],
                intensity: 3.0,
                range: 3.0,
                marked: true,
            },
        ],
    );
    let gi_out = out_path("gi_room_dynamic_dump.png");

    let output = run_ludion(&[
        "render",
        "--scene",
        scene.to_str().unwrap(),
        "--dump-gi",
        gi_out.to_str().unwrap(),
    ]);
    if !output.status.success() {
        if is_no_adapter(&output) {
            eprintln!(
                "SKIPPED dump_gi_dynamic_block_is_bright_near_a_marked_light_on_a_mixed_scene: no \
                 GPU adapter"
            );
            return;
        }
        panic!(
            "dump-gi failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert_eq!(
        dynamic_summary(&stdout),
        "injected",
        "a bouncing GiVolume with a GiDynamic-marked light must report the term injected: {stdout}"
    );
    assert!(
        stdout.contains("lights 1 baked + 1 injected"),
        "the split census must count the unmarked lamp and the marked mover separately: {stdout}"
    );

    let (w, h, rgba) = decode_png(&gi_out);
    let (x0, x1) = block_x_range(w, 3);

    let mut min_bright = 255u8;
    let mut max_bright = 0u8;
    for row in SHEET_HEADER_H..h {
        for col in x0..x1 {
            let idx = (row as usize * w as usize + col as usize) * 4;
            let px: [u8; 4] = rgba[idx..idx + 4].try_into().unwrap();
            if px == BACKGROUND {
                continue;
            }
            let bright = px[0].max(px[1]).max(px[2]);
            min_bright = min_bright.min(bright);
            max_bright = max_bright.max(bright);
        }
    }

    assert!(
        max_bright > 60,
        "a probe near the marked mover must read a materially bright DYNAMIC block; max was \
         {max_bright}"
    );
    assert!(
        max_bright - min_bright >= 40,
        "the mover's corner and the room's far corner must read clearly distinct: \
         min={min_bright} max={max_bright}"
    );
}

/// **T11, the first absent reason: `bounce: 0.0`.** A `GiDynamic`-marked light on an unbounced
/// `GiVolume` still reports the term un-injected and says why — the injected term *is* the bounce
/// term (`Plan/gi-wave3-dynamic-injection.md` R2), so marking a light buys it nothing at `bounce:
/// 0.0`. This also proves the reason's precedence: `bounce: 0.0` wins even though a light is marked.
///
/// Skips gracefully with no GPU adapter.
#[test]
fn needs_gpu_dump_gi_summary_reports_dynamic_absent_because_bounce_is_zero() {
    let scene = out_path("gi_room_scene_dynamic_no_bounce.yaml");
    write_room_scene(
        &scene,
        0.25,
        0.5,
        0.0,
        &[LightSpec {
            name: "mover",
            position: [-1.7, 1.6, 0.3],
            color: [1.0, 0.62, 0.28],
            intensity: 3.0,
            range: 3.0,
            marked: true,
        }],
    );
    let gi_out = out_path("gi_room_dynamic_no_bounce_dump.png");

    let output = run_ludion(&[
        "render",
        "--scene",
        scene.to_str().unwrap(),
        "--dump-gi",
        gi_out.to_str().unwrap(),
    ]);
    if !output.status.success() {
        if is_no_adapter(&output) {
            eprintln!(
                "SKIPPED dump_gi_summary_reports_dynamic_absent_because_bounce_is_zero: no GPU \
                 adapter"
            );
            return;
        }
        panic!(
            "dump-gi failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert_eq!(
        dynamic_summary(&stdout),
        "not injected (bounce: 0.0)",
        "a marked light on an unbounced GiVolume must still report why it did not inject: {stdout}"
    );
    assert!(
        stdout.contains("lights 0 baked + 1 injected"),
        "the census counts the marked light as injected regardless of whether the term actually \
         injected: {stdout}"
    );
}

/// **T11, the second absent reason: no `GiDynamic` lights.** A bouncing `GiVolume` with an unmarked
/// light present (so the static bake has something to do) but nothing marked reports the term
/// un-injected for the other content reason, and the split census counts the light as baked, not
/// injected.
///
/// Skips gracefully with no GPU adapter.
#[test]
fn needs_gpu_dump_gi_summary_reports_dynamic_absent_because_no_gidynamic_lights() {
    let scene = out_path("gi_room_scene_dynamic_unmarked.yaml");
    write_room_scene(
        &scene,
        0.25,
        0.5,
        1.0,
        &[LightSpec {
            name: "lamp",
            position: [-1.7, 1.6, 0.3],
            color: [1.0, 0.62, 0.28],
            intensity: 3.0,
            range: 3.0,
            marked: false,
        }],
    );
    let gi_out = out_path("gi_room_dynamic_unmarked_dump.png");

    let output = run_ludion(&[
        "render",
        "--scene",
        scene.to_str().unwrap(),
        "--dump-gi",
        gi_out.to_str().unwrap(),
    ]);
    if !output.status.success() {
        if is_no_adapter(&output) {
            eprintln!(
                "SKIPPED dump_gi_summary_reports_dynamic_absent_because_no_gidynamic_lights: no \
                 GPU adapter"
            );
            return;
        }
        panic!(
            "dump-gi failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert_eq!(
        dynamic_summary(&stdout),
        "not injected (no GiDynamic lights)",
        "a bouncing GiVolume with nothing marked must report that specific reason: {stdout}"
    );
    assert!(
        stdout.contains("lights 1 baked + 0 injected"),
        "the unmarked lamp must count on the baked side of the census: {stdout}"
    );
}
