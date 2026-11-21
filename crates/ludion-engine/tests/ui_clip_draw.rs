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

/// Renders `scene` and returns the frame with a `ui_hit` probe over the entities at `probed`
/// (document indices), or `None` without an adapter.
fn render(
    scene: &str,
) -> Option<(
    ludion_renderer::RenderedImage,
    ludion_ecs::World,
    Vec<ludion_core::ids::EntityId>,
)> {
    let dir = std::env::temp_dir().join(format!(
        "ludion-ui-clip-draw-{}-{}",
        std::process::id(),
        scene.len()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("scene.yaml");
    std::fs::write(&path, scene).unwrap();
    let outcome = match render_scene(&path, FrameSize::native(VIEWPORT.0, VIEWPORT.1), false) {
        Ok(outcome) => outcome,
        Err(errors) if ludion_core::require_gpu::is_no_adapter(&errors) => {
            eprintln!("SKIPPED: no GPU adapter available");
            return None;
        }
        Err(errors) => panic!("render failed: {errors:?}"),
    };
    let registry = ludion_engine::builtin_registry();
    let checked = ludion_scene::check(scene, &registry).expect("the scene validates");
    let world = ludion_scene::instantiate(&checked, &registry).expect("the scene instantiates");
    let ids: Vec<_> = world.entities().collect();
    Some((outcome.image, world, ids))
}

/// Compares `ui_hit` over `fills` with the drawn pixels: every disagreement must sit in the 1 px AA
/// band, and those that do must be a small share of the edge pixels. The `region` is checked at every
/// pixel, the rest of the frame sparsely.
fn assert_hit_agrees_with_pixels(
    image: &ludion_renderer::RenderedImage,
    world: &ludion_ecs::World,
    fills: &[ludion_core::ids::EntityId],
    region: (u32, u32),
    min_drawn: u32,
) {
    let now = SimTime::from_ticks(0, TickRate::DEFAULT);
    let hit = |x: f32, y: f32| {
        fills
            .iter()
            .any(|&id| ui_hit(world, id, VIEWPORT, [x, y], now))
    };
    // Half coverage of white over the clear, exactly: the target blends in linear light and stores sRGB,
    // so each channel of half coverage is encode(0.5 * decode(clear) + 0.5), not a fixed share of the way up.
    let decode = |v: f64| {
        if v <= 0.04045 {
            v / 12.92
        } else {
            ludion_core::math::f64::powf((v + 0.055) / 1.055, 2.4)
        }
    };
    let encode = |v: f64| {
        if v <= 0.003_130_8 {
            v * 12.92
        } else {
            1.055 * ludion_core::math::f64::powf(v, 1.0 / 2.4) - 0.055
        }
    };
    let i = ((4 * image.width + 4) * 4) as usize;
    let half_cover: f64 = (0..3)
        .map(|c| 255.0 * encode(0.5 * decode(f64::from(image.rgba[i + c]) / 255.0) + 0.5))
        .sum();
    let mut edge = 0u32;
    let mut band_mismatch = 0u32;
    let mut drawn_pixels = 0u32;
    for y in 0..VIEWPORT.1 {
        for x in 0..VIEWPORT.0 {
            if !(x < region.0 && y < region.1) && (x % 7 != 0 || y % 7 != 0) {
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
        drawn_pixels > min_drawn,
        "the clipped shapes drew ({drawn_pixels} px)"
    );
    assert!(edge > 100, "the shapes have edges ({edge} px)");
    assert!(
        f64::from(band_mismatch) < f64::from(edge) * 0.02,
        "{band_mismatch} of {edge} edge pixels disagree"
    );
}

#[test]
fn needs_gpu_ui_hit_agrees_with_the_drawn_clip() {
    let Some((image, world, ids)) = render(SCENE) else {
        return;
    };
    // The shapes live in the top-left 760x320.
    assert_hit_agrees_with_pixels(&image, &world, &[ids[2], ids[5]], (760, 320), 10_000);
}

/// A white rect turned 30° inside a rect-clipped group turned 20°: the drawn pixels are the
/// intersection of two differently rotated boxes.
const ROTATED_SCENE: &str = "\
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
          - 300.0
          - 150.0
        size:
          - 360.0
          - 240.0
        clip: rect
        rotation: 20.0
  - components:
      UiRect:
        anchor: center
        size:
          - 500.0
          - 80.0
        rotation: 30.0
        group:
          index: 1
          generation: 0
";

#[test]
fn needs_gpu_ui_hit_agrees_with_a_rotated_draw() {
    let Some((image, world, ids)) = render(ROTATED_SCENE) else {
        return;
    };
    assert_hit_agrees_with_pixels(&image, &world, &[ids[2]], (960, 540), 5_000);
}

/// A white 270-degree ring and a 60-degree wedge, both turned 30 degrees about their origins, inside a
/// rect-clipped group turned 20 degrees: the drawn pixels are each shape cut by the turned clip.
const SHAPES_SCENE: &str = "\
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
          - 300.0
          - 150.0
        size:
          - 360.0
          - 360.0
        clip: rect
        rotation: 20.0
  - components:
      UiShape:
        anchor: center
        group:
          index: 1
          generation: 0
        shape:
          type: ring
          radius: 220.0
          thickness: 100.0
          start_deg: 45.0
          sweep_deg: 270.0
        fill:
          - 1.0
          - 1.0
          - 1.0
          - 1.0
        rotation: 30.0
  - components:
      UiShape:
        anchor: center
        group:
          index: 1
          generation: 0
        shape:
          type: wedge
          radius: 100.0
          start_deg: 0.0
          sweep_deg: 60.0
        fill:
          - 1.0
          - 1.0
          - 1.0
          - 1.0
        rotation: 30.0
";

#[test]
fn needs_gpu_ui_hit_agrees_with_drawn_shapes() {
    let Some((image, world, ids)) = render(SHAPES_SCENE) else {
        return;
    };
    assert_hit_agrees_with_pixels(&image, &world, &[ids[2], ids[3]], (960, 540), 5_000);
}

/// A white concave star turned 25 degrees inside a rect-clipped group turned 20 degrees.
const POLYGONS_SCENE: &str = "format: 1
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
          - 300.0
          - 150.0
        size:
          - 360.0
          - 360.0
        clip: rect
        rotation: 20.0
  - components:
      UiShape:
        anchor: center
        group:
          index: 1
          generation: 0
        shape:
          type: polygon
          points:
          - - 0.0
            - -200.0
          - - 76.41
            - -105.17
          - - 190.21
            - -61.8
          - - 123.64
            - 40.17
          - - 117.56
            - 161.8
          - - 0.0
            - 130.0
          - - -117.56
            - 161.8
          - - -123.64
            - 40.17
          - - -190.21
            - -61.8
          - - -76.41
            - -105.17
        fill:
          - 1.0
          - 1.0
          - 1.0
          - 1.0
        rotation: 25.0
";

#[test]
fn needs_gpu_ui_hit_agrees_with_drawn_polygons() {
    let Some((image, world, ids)) = render(POLYGONS_SCENE) else {
        return;
    };
    assert_hit_agrees_with_pixels(&image, &world, &[ids[2]], (960, 540), 5_000);
}
