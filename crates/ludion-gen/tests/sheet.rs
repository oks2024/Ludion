//! The contact sheet over a hand-built run folder.
//!
//! No server and no GPU: a run folder is a `run.json` and its candidate files, so the composition
//! half of [`ludion_gen::sheet`] is testable with two PNGs written by the workspace's own encoder.
//! The geometry half is a pure function with its own unit tests beside it.

use std::path::{Path, PathBuf};

use ludion_gen::run::{RunOutput, RunReport};

/// A solid `size`×`size` RGBA image.
fn solid(size: u32, colour: [u8; 4]) -> Vec<u8> {
    let mut rgba = vec![0u8; (size * size * 4) as usize];
    for pixel in rgba.chunks_exact_mut(4) {
        pixel.copy_from_slice(&colour);
    }
    rgba
}

/// A finished two-candidate image run at `generation/runs/idol/<run-id>/`.
fn run_folder(name: &str, kind: &str, candidates: &[(u32, [u8; 4])]) -> PathBuf {
    let root = std::env::temp_dir().join(format!("ludion-gen-sheet-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let run_dir = root.join("generation/runs/idol/20260918T101112Z-1a2b3c4d");
    std::fs::create_dir_all(&run_dir).expect("the run folder");

    let extension = if kind == "mesh" { "glb" } else { "png" };
    let mut outputs = Vec::new();
    for (index, (size, colour)) in candidates.iter().enumerate() {
        let file = format!("{index:04}.{extension}");
        let bytes = ludion_assets::encode_png(*size, *size, &solid(*size, *colour))
            .expect("the candidate encodes");
        std::fs::write(run_dir.join(&file), bytes).expect("the candidate");
        outputs.push(RunOutput {
            index: index as u32,
            seed: Some(7 + index as u64),
            file,
            node: "9".to_string(),
            batch_index: 0,
            prompt_id: format!("job-{index}"),
        });
    }

    let report = RunReport {
        run_id: "20260918T101112Z-1a2b3c4d".to_string(),
        recipe: "generation/recipes/idol.gen.yaml".to_string(),
        recipe_hash: "0000000012345678".to_string(),
        workflow: "generation/workflows/txt2img.json".to_string(),
        workflow_hash: "00000000deadbeef".to_string(),
        graph_hash: "1a2b3c4d5e6f7080".to_string(),
        seed_base: 7,
        count: candidates.len() as u32,
        kind: kind.to_string(),
        comfy_version: "0.3.99".to_string(),
        checkpoints: vec!["flux2-klein-4b.safetensors".to_string()],
        images: Vec::new(),
        prompt: None,
        inputs: None,
        prompt_ids: outputs.iter().map(|o| o.prompt_id.clone()).collect(),
        seeds: outputs.iter().map(|o| o.seed).collect(),
        outputs,
        failed: Vec::new(),
        started: "2026-09-18T10:11:12Z".to_string(),
        finished: "2026-09-18T10:11:40Z".to_string(),
        duration_ms: 28_000,
        status: "ok".to_string(),
        dir: run_dir.clone(),
    };
    let mut json = serde_json::to_string_pretty(&report).expect("the report serializes");
    json.push('\n');
    std::fs::write(run_dir.join("run.json"), json).expect("run.json");
    run_dir
}

fn decode(path: &Path) -> (u32, u32, Vec<u8>) {
    let bytes = std::fs::read(path).expect("the sheet is on disk");
    ludion_assets::decode_png(&bytes, &path.display().to_string()).expect("the sheet is a PNG")
}

fn pixel(rgba: &[u8], width: u32, x: u32, y: u32) -> [u8; 4] {
    let offset = (y as usize * width as usize + x as usize) * 4;
    [
        rgba[offset],
        rgba[offset + 1],
        rgba[offset + 2],
        rgba[offset + 3],
    ]
}

#[test]
fn a_sheet_is_the_grid_the_layout_describes_with_every_candidate_in_its_cell() {
    let red = [220, 40, 40, 255];
    let green = [40, 200, 90, 255];
    let run_dir = run_folder("grid", "image", &[(64, red), (64, green)]);

    let out = ludion_gen::sheet::compose(&run_dir, None, None).expect("the sheet composes");
    assert_eq!(
        out,
        run_dir.join("sheet.png"),
        "the default output is the run folder's own sheet"
    );

    let layout = ludion_gen::sheet::layout(&[(64, 64), (64, 64)], 2);
    assert_eq!(layout.canvas, (152, 94));

    let (width, height, rgba) = decode(&out);
    assert_eq!((width, height), layout.canvas);

    for (cell, expected) in layout.cells.iter().zip([red, green]) {
        assert_eq!(
            pixel(&rgba, width, cell.x + cell.w / 2, cell.y + cell.h / 2),
            expected,
            "candidate {} fills its cell",
            cell.index
        );
    }
    // The gutter between the two cells stays background, so a cell's own edges are readable.
    assert_eq!(pixel(&rgba, width, 0, 0), [24, 24, 28, 255]);
    assert_eq!(
        pixel(&rgba, width, layout.cells[0].x + layout.cell.0 + 4, 20),
        [24, 24, 28, 255]
    );
}

#[test]
fn cols_and_out_are_honoured() {
    let run_dir = run_folder(
        "cols",
        "image",
        &[(32, [255, 0, 0, 255]), (32, [0, 255, 0, 255])],
    );
    let out = run_dir.parent().expect("a parent").join("contact.png");

    let written =
        ludion_gen::sheet::compose(&run_dir, Some(1), Some(&out)).expect("the sheet composes");

    assert_eq!(written, out);
    let (width, height, _) = decode(&out);
    assert_eq!(
        (width, height),
        ludion_gen::sheet::layout(&[(32, 32), (32, 32)], 1).canvas,
        "one column stacks the two candidates"
    );
}

#[test]
fn l1171_refuses_a_mesh_run_and_points_at_the_model_stage() {
    let run_dir = run_folder("mesh", "mesh", &[(32, [255, 255, 255, 255])]);

    let diagnostics =
        ludion_gen::sheet::compose(&run_dir, None, None).expect_err("a mesh run has no sheet");

    assert_eq!(
        diagnostics
            .iter()
            .map(|d| d.code.as_str())
            .collect::<Vec<_>>(),
        vec!["L1171"]
    );
    assert!(
        diagnostics[0].message.contains("the sheet shows images"),
        "{}",
        diagnostics[0].message
    );
}
