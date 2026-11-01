//! Fonts by name, past extract (`Plan/font-assets.md` W3): the GPU draw of a baked face's atlas
//! (SDF and bitmap). Skips without an adapter.

use std::path::{Path, PathBuf};

use ludion_engine::ui::resolve_ui_text;
use ludion_engine::{FontLibrary, FrameSize, render_scene};

const VIEWPORT: (u32, u32) = (960, 540);

fn root(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("ludion-font-draw-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("fonts")).unwrap();
    let gallery = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/gallery/fonts");
    for file in [
        "inter.font.yaml",
        "inter.png",
        "silkscreen.font.yaml",
        "silkscreen.png",
    ] {
        std::fs::copy(gallery.join(file), dir.join("fonts").join(file)).unwrap();
    }
    std::fs::write(
        dir.join("fonts.yaml"),
        "format: 1\nkind: fonts\nfonts:\n  body: fonts/inter.font.yaml\n  pixel: fonts/silkscreen.font.yaml\n",
    )
    .unwrap();
    dir
}

const CAMERA: &str = "format: 1\nentities:\n  - name: camera\n    components:\n      Transform:\n        translation:\n          - 0.0\n          - 0.0\n          - 5.0\n      Camera: {}\n";

fn scene_with(texts: &[(&str, &str, [f32; 2], f32)]) -> String {
    let mut scene = CAMERA.to_string();
    for (font, text, offset, size) in texts {
        scene.push_str(&format!(
            "  - components:\n      UiText:\n        text: \"{text}\"\n        font: {font}\n        size: {size}\n        offset:\n          - {}\n          - {}\n",
            offset[0], offset[1]
        ));
    }
    scene
}

/// Pixel bounds `[x0, y0, x1, y1]` (inclusive) where `a` and `b` differ, or `None` if identical.
fn diff_bounds(a: &[u8], b: &[u8], width: u32) -> Option<[u32; 4]> {
    let mut bounds: Option<[u32; 4]> = None;
    for (i, (pa, pb)) in a.chunks(4).zip(b.chunks(4)).enumerate() {
        if pa != pb {
            let (x, y) = (i as u32 % width, i as u32 / width);
            bounds = Some(match bounds {
                None => [x, y, x, y],
                Some([x0, y0, x1, y1]) => [x0.min(x), y0.min(y), x1.max(x), y1.max(y)],
            });
        }
    }
    bounds
}

/// Both a baked SDF face and a baked bitmap face draw: pixels change only inside each block's
/// resolved rect (plus a 2 px fringe). Exact pixels are the gallery golden's job.
#[test]
fn needs_gpu_baked_faces_draw_inside_their_resolved_rects() {
    let dir = root("gpu");
    let render = |name: &str, scene: &str| {
        let path = dir.join(name);
        std::fs::write(&path, scene).unwrap();
        render_scene(&path, FrameSize::native(VIEWPORT.0, VIEWPORT.1), false)
    };
    let base = match render("base.yaml", CAMERA) {
        Ok(outcome) => outcome,
        Err(errors) if ludion_core::require_gpu::is_no_adapter(&errors) => {
            eprintln!("SKIPPED: no GPU adapter available");
            return;
        }
        Err(errors) => panic!("render failed: {errors:?}"),
    };
    let library = FontLibrary::load(ludion_assets::AssetRoot::content(&dir));
    for (font, offset, size) in [("body", [40.0, 60.0], 48.0), ("pixel", [40.0, 300.0], 40.0)] {
        let text = "Hamburg";
        let drawn = render("text.yaml", &scene_with(&[(font, text, offset, size)]))
            .unwrap_or_else(|e| panic!("{font}: render failed: {e:?}"));
        let face = library.face(Some(font));
        let block = resolve_ui_text(
            face,
            &ludion_engine::components::UiText {
                text: text.to_string(),
                offset,
                size,
                ..Default::default()
            },
            VIEWPORT,
        );
        let rect = [block[0], block[1], block[0] + block[2], block[1] + block[3]];
        let [x0, y0, x1, y1] = diff_bounds(&base.image.rgba, &drawn.image.rgba, base.image.width)
            .unwrap_or_else(|| panic!("{font}: the text drew nothing"));
        let (x0, y0, x1, y1) = (x0 as f32, y0 as f32, x1 as f32, y1 as f32);
        assert!(
            x0 >= rect[0] - 2.0
                && y0 >= rect[1] - 2.0
                && x1 <= rect[2] + 2.0
                && y1 <= rect[3] + 2.0,
            "{font}: inked [{x0}, {y0}, {x1}, {y1}] outside the block {rect:?}"
        );
    }
}
