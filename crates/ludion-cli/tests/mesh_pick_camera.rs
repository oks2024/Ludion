//! The one test that proves `ludion mesh pick`'s camera IS the picture's camera.
//!
//! Every other pick test asserts against hand-known geometry, which would stay green if `pick` and
//! `render --model` framed the same model differently — the one divergence that would be invisible,
//! and that would make every pick a few pixels off with the author blaming their region. So this
//! one renders a cell and picks against the pixels that came back: where the drawn silhouette ends,
//! the pick must start missing.
//!
//! It lives in a binary of its own because it opens a device and its siblings in `tests/mesh.rs` do
//! not: nextest's `gpu` group (and the `no-gpu` profile's exclusion) works per binary, and the CPU
//! tests must keep running on a GPU-less agent.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

/// A 1×1 quad at `z = 0`, wound to face −Z and coloured by one flat green map — a silhouette a
/// channel comparison can find without guessing at the stage's lighting.
const PLATE_MESH: &str = "\
format: 1
kind: mesh
name: plate
positions:
  - -0.5
  - -0.5
  - 0.0
  - 0.5
  - -0.5
  - 0.0
  - 0.5
  - 0.5
  - 0.0
  - -0.5
  - 0.5
  - 0.0
uvs:
  - 0.0
  - 1.0
  - 1.0
  - 1.0
  - 1.0
  - 0.0
  - 0.0
  - 0.0
base_color_texture:
  path: \"base.png\"
indices:
  - 0
  - 2
  - 1
  - 0
  - 3
  - 2
";

const CELL_W: u32 = 320;
const CELL_H: u32 = 180;

fn scratch() -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("mesh_pick_camera");
    if let Err(error) = std::fs::remove_dir_all(&dir) {
        assert!(
            error.kind() == std::io::ErrorKind::NotFound,
            "stale scratch dir {dir:?} could not be removed: {error}"
        );
    }
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("plate.mesh.yaml"), PLATE_MESH).unwrap();
    let png = ludion_assets::encode_png(8, 8, &[10u8, 200, 40, 255].repeat(8 * 8)).unwrap();
    std::fs::write(dir.join("base.png"), &png).unwrap();
    dir
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

/// Does `pick` find the mesh under this pixel of the rendered cell?
fn hits(mesh: &Path, x: u32, y: u32) -> bool {
    run_ludion(&[
        "mesh",
        "pick",
        mesh.to_str().unwrap(),
        "--view",
        "front",
        "--pixel",
        &format!("{x},{y}"),
        "--width",
        &CELL_W.to_string(),
        "--height",
        &CELL_H.to_string(),
    ])
    .status
    .success()
}

/// Render one `front` cell, find the drawn silhouette's horizontal span down the middle row, and
/// require `pick` to agree with it on both edges.
#[test]
fn needs_gpu_a_pick_hits_exactly_where_the_cell_drew_the_model() {
    let dir = scratch();
    let mesh = dir.join("plate.mesh.yaml");
    let sheet = dir.join("sheet.png");
    let rendered = run_ludion(&[
        "render",
        "--model",
        mesh.to_str().unwrap(),
        "--views",
        "front",
        "--width",
        &CELL_W.to_string(),
        "--height",
        &CELL_H.to_string(),
        "--out",
        sheet.to_str().unwrap(),
    ]);
    if !rendered.status.success() {
        if is_no_adapter(&rendered) {
            eprintln!("SKIPPED a_pick_hits_exactly_where_the_cell_drew_the_model: no GPU adapter");
            return;
        }
        panic!(
            "render --model failed: {}",
            String::from_utf8_lossy(&rendered.stderr)
        );
    }

    let bytes = std::fs::read(&sheet).unwrap();
    let (width, _, rgba) = ludion_assets::decode_png(&bytes, "sheet").expect("the sheet decodes");
    let row = CELL_H / 2;
    // The plate's map is flat green; the stage's ground and sky are neither, whatever the lighting
    // does to them. A one-channel dominance test needs no golden and no tolerance file.
    let green = |x: u32| -> bool {
        let base = ((row * width + x) * 4) as usize;
        let [r, g, b] = [rgba[base], rgba[base + 1], rgba[base + 2]].map(i32::from);
        g > r + 30 && g > b + 30
    };
    let drawn: Vec<u32> = (0..width).filter(|x| green(*x)).collect();
    assert!(
        drawn.len() > 20,
        "the cell drew no recognizable model on row {row}"
    );
    let (left, right) = (drawn[0], drawn[drawn.len() - 1]);
    assert_eq!(
        drawn.len() as u32,
        right - left + 1,
        "the silhouette is not one run: {left}..={right}"
    );

    // Inside the drawn silhouette the pick must hit; outside it must miss. The two-pixel margin is
    // the edge antialiasing the cell's own MSAA leaves, not slack in the agreement.
    assert!(hits(&mesh, left + 2, row), "inside the left edge ({left})");
    assert!(
        hits(&mesh, right - 2, row),
        "inside the right edge ({right})"
    );
    assert!(
        !hits(&mesh, left.saturating_sub(3), row),
        "left of the drawn silhouette ({left})"
    );
    assert!(
        !hits(&mesh, right + 3, row),
        "right of the drawn silhouette ({right})"
    );
}
