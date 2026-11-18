//! The `UiGroup` clip on the GPU (`Plan/ui-hud.md` decisions 25-30): `ui_hit` must agree with the
//! pixels the clip shader draws. Skips without an adapter.

use ludion_core::time::{SimTime, TickRate};
use ludion_engine::ui::ui_hit;
use ludion_engine::{FrameSize, render_scene};

const VIEWPORT: (u32, u32) = (960, 540);

/// Camera, then (document index): 1 circle group and 2 its filling rect; 3 a rect-clipped group,
/// 4 a rounded-rect group nested in it that overhangs it, and 5 its filling rect.
const SCENE: &str = "\
format: 1
entities:
  - name: camera
    components:
      Transform:
        translation:
          - 0.0
          - 0.0
          - 5.0
      Camera: {}
  - components:
      UiGroup:
        offset:
          - 100.0
          - 100.0
        size:
          - 200.0
          - 200.0
        clip: circle
  - components:
      UiRect:
        fill_group: true
        group:
          index: 1
          generation: 0
  - components:
      UiGroup:
        offset:
          - 400.0
          - 100.0
        size:
          - 300.0
          - 200.0
        clip: rect
  - components:
      UiGroup:
        offset:
          - 50.0
          - 20.0
        size:
          - 300.0
          - 160.0
        clip:
          rounded_rect:
            corner_radius: 40.0
        group:
          index: 3
          generation: 0
  - components:
      UiRect:
        fill_group: true
        group:
          index: 4
          generation: 0
";

fn luma(rgba: &[u8], width: u32, x: u32, y: u32) -> u32 {
    let i = ((y * width + x) * 4) as usize;
    u32::from(rgba[i]) + u32::from(rgba[i + 1]) + u32::from(rgba[i + 2])
}

#[test]
fn needs_gpu_ui_hit_agrees_with_the_drawn_clip() {
    let dir = std::env::temp_dir().join(format!("ludion-ui-clip-draw-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("scene.yaml");
    std::fs::write(&path, SCENE).unwrap();
    let outcome = match render_scene(&path, FrameSize::native(VIEWPORT.0, VIEWPORT.1), false) {
        Ok(outcome) => outcome,
        Err(errors) if ludion_core::require_gpu::is_no_adapter(&errors) => {
            eprintln!("SKIPPED: no GPU adapter available");
            return;
        }
        Err(errors) => panic!("render failed: {errors:?}"),
    };

    let registry = ludion_engine::builtin_registry();
    let checked = ludion_scene::check(SCENE, &registry).expect("the scene validates");
    let world = ludion_scene::instantiate(&checked, &registry).expect("the scene instantiates");
    let ids: Vec<_> = world.entities().collect();
    let fills = [ids[2], ids[5]];
    let now = SimTime::from_ticks(0, TickRate::DEFAULT);
    let hit = |x: f32, y: f32| {
        fills
            .iter()
            .any(|&id| ui_hit(&world, id, VIEWPORT, [x, y], now))
    };

    let image = &outcome.image;
    let background = luma(&image.rgba, image.width, 4, 4);
    // Half coverage of white over the clear: the target blends in linear light and stores sRGB, so
    // half coverage lands near 0.735 of the way from the background to white.
    let half_cover = f64::from(background) + 0.735 * f64::from(765 - background);
    let mut edge = 0u32;
    let mut band_mismatch = 0u32;
    let mut drawn_pixels = 0u32;
    for y in 0..VIEWPORT.1 {
        for x in 0..VIEWPORT.0 {
            // The shapes live in the top-left 760x320; sample the rest sparsely.
            if !(x < 760 && y < 320) && (x % 7 != 0 || y % 7 != 0) {
                continue;
            }
            let centre = [x as f32 + 0.5, y as f32 + 0.5];
            let expected = hit(centre[0], centre[1]);
            let drawn = f64::from(luma(&image.rgba, image.width, x, y)) > half_cover;
            drawn_pixels += u32::from(drawn);
            // A neighbour one pixel away that disagrees puts this pixel inside the 1 px AA band.
            let in_band = [(-1.0, 0.0), (1.0, 0.0), (0.0, -1.0), (0.0, 1.0)]
                .iter()
                .any(|(dx, dy)| hit(centre[0] + dx, centre[1] + dy) != expected);
            edge += u32::from(in_band);
            if expected != drawn {
                assert!(
                    in_band,
                    "pixel ({x}, {y}): ui_hit says {expected}, the frame says {drawn}, away from any edge"
                );
                band_mismatch += 1;
            }
        }
    }
    assert!(
        drawn_pixels > 10_000,
        "the clipped shapes drew ({drawn_pixels} px)"
    );
    assert!(edge > 100, "the shapes have edges ({edge} px)");
    assert!(
        f64::from(band_mismatch) < f64::from(edge) * 0.02,
        "{band_mismatch} of {edge} edge pixels disagree"
    );
}
