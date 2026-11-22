//! The occluded silhouette: GPU tests over the `silhouette_mask` pass and `composite`'s step 7b.

use super::*;
use crate::scene::{SilhouetteMesh, SilhouettePattern, SilhouetteStyle, Silhouettes};

const PX: u32 = 540;

fn size() -> FrameSize {
    FrameSize::native(PX, PX)
}

const WHITE: SilhouetteStyle = SilhouetteStyle {
    color: [1.0, 1.0, 1.0, 1.0],
    outline_px: 0.0,
    outline_alpha: 1.0,
    bias: 0.05,
    always: false,
    inner_px: 0.0,
    inner_alpha: 0.0,
    pattern: SilhouettePattern::None,
    pattern_period: 0.0,
    pattern_duty: 0.0,
    film: true,
};

/// Meshes on a black sky, no lights, the exact-output post; the camera looks down -Z from the origin.
fn scene(
    meshes: Vec<MeshInstance>,
    styles: Vec<SilhouetteStyle>,
    tagged: &[(u32, u8)],
) -> RenderScene {
    let mut scene = RenderScene::empty((PX, PX));
    scene.camera = origin_camera(PX, PX);
    scene.ambient = [0.0; 3];
    scene.post = Some(exact_post());
    scene.meshes = meshes;
    scene.silhouettes = Silhouettes {
        styles,
        meshes: tagged
            .iter()
            .map(|&(mesh, style)| SilhouetteMesh { mesh, style })
            .collect(),
    };
    scene
}

fn px(frame: &RenderedImage, x: u32, y: u32) -> [u8; 3] {
    let i = ((y * frame.width + x) * 4) as usize;
    [frame.rgba[i], frame.rgba[i + 1], frame.rgba[i + 2]]
}

const C: u32 = PX / 2;

/// A small wall in front of a larger figure: the figure's pixels behind the wall are hidden.
fn wall_and_figure() -> Vec<MeshInstance> {
    vec![
        slab(Vec3::new(0.0, 0.0, -4.0), 1.0, [0.2; 3]),
        slab(Vec3::new(0.0, 0.0, -10.0), 6.0, [0.5; 3]),
    ]
}

/// A big wall in front of a small figure: the figure is wholly hidden and the wall surrounds it.
fn big_wall_small_figure() -> Vec<MeshInstance> {
    vec![
        slab(Vec3::new(0.0, 0.0, -4.0), 4.0, [0.2; 3]),
        slab(Vec3::new(0.0, 0.0, -10.0), 2.0, [0.5; 3]),
    ]
}

/// A W1-shaped style packs lanes 2 and 3 as `(0,0,0,0)` and `(0,1,0,0)`; a pattern style packs its
/// period scaled to the render height, its duty and its kind; an inner outline packs its radius.
#[test]
fn lanes_two_and_three_pack_the_pattern_and_inner_outline() {
    let packed = |style: SilhouetteStyle, height: u32| {
        let mut scene = RenderScene::empty((height, height));
        scene.silhouettes.styles = vec![style];
        crate::render::silhouette::composite_lanes(&scene, height, true).styles
    };
    let plain = packed(WHITE, 540);
    assert_eq!(plain[2], [0.0; 4]);
    assert_eq!(plain[3], [0.0, 1.0, 0.0, 0.0]);
    let patterned = packed(
        SilhouetteStyle {
            pattern: SilhouettePattern::Hatch,
            pattern_period: 8.0,
            pattern_duty: 0.25,
            inner_px: 4.0,
            inner_alpha: 0.5,
            ..WHITE
        },
        540,
    );
    assert_eq!(patterned[2], [2.0, 0.5, 4.0, 0.25]);
    assert_eq!(patterned[3], [2.0, 1.0, 0.0, 0.0]);
    let tiny = packed(
        SilhouetteStyle {
            pattern: SilhouettePattern::Scanlines,
            pattern_period: 2.0,
            pattern_duty: 0.5,
            ..WHITE
        },
        100,
    );
    assert_eq!(tiny[2][2], 2.0, "the render period never drops below 2");
    assert_eq!(tiny[3][0], 1.0);
}

/// A component-free frame and a present-but-fully-visible one are byte-identical. Skips with no GPU.
#[test]
fn needs_gpu_a_silhouette_on_a_visible_mesh_changes_nothing() {
    let Some(renderer) = skip_or_renderer("a_silhouette_on_a_visible_mesh_changes_nothing") else {
        return;
    };
    let cube = || vec![slab(Vec3::new(0.0, 0.0, -5.0), 1.0, [0.4; 3])];
    let none = renderer
        .render(&scene(cube(), vec![], &[]), size())
        .expect("none");
    let tagged = renderer
        .render(&scene(cube(), vec![WHITE], &[(0, 1)]), size())
        .expect("tagged");
    assert_eq!(none.rgba, tagged.rgba);
}

/// Behind a wall the figure's pixel takes the style colour; the same pixel is untinted without the
/// component, and a visible figure pixel is untouched either way. Skips with no GPU.
#[test]
fn needs_gpu_a_hidden_mesh_is_tinted_and_a_visible_one_is_not() {
    let Some(renderer) = skip_or_renderer("a_hidden_mesh_is_tinted_and_a_visible_one_is_not")
    else {
        return;
    };
    let none = renderer
        .render(&scene(wall_and_figure(), vec![], &[]), size())
        .expect("none");
    let on = renderer
        .render(&scene(wall_and_figure(), vec![WHITE], &[(1, 1)]), size())
        .expect("on");
    assert_ne!(px(&none, C, C), [255, 255, 255]);
    assert_eq!(
        px(&on, C, C),
        [255, 255, 255],
        "the hidden centre takes the style colour"
    );
    let visible = (C + 90, C);
    assert_eq!(
        px(&on, visible.0, visible.1),
        px(&none, visible.0, visible.1),
        "a visible figure pixel is not tinted"
    );
    assert_ne!(px(&none, visible.0, visible.1), px(&none, C, C));
}

/// Two silhouetted cubes, the small one in front of the large one, in the open: neither hides the
/// other, so the frame is byte-identical to none. Skips with no GPU.
#[test]
fn needs_gpu_self_occlusion_draws_nothing() {
    let Some(renderer) = skip_or_renderer("self_occlusion_draws_nothing") else {
        return;
    };
    let cubes = || {
        vec![
            slab(Vec3::new(0.0, 0.0, -4.0), 1.0, [0.2; 3]),
            slab(Vec3::new(0.0, 0.0, -10.0), 6.0, [0.5; 3]),
        ]
    };
    let none = renderer
        .render(&scene(cubes(), vec![], &[]), size())
        .expect("none");
    let both = renderer
        .render(&scene(cubes(), vec![WHITE], &[(0, 1), (1, 1)]), size())
        .expect("both");
    assert_eq!(none.rgba, both.rgba);
}

/// `always` tints a visible cube. Skips with no GPU.
#[test]
fn needs_gpu_always_tints_a_visible_mesh() {
    let Some(renderer) = skip_or_renderer("always_tints_a_visible_mesh") else {
        return;
    };
    let cube = || vec![slab(Vec3::new(0.0, 0.0, -5.0), 1.0, [0.4; 3])];
    let always = SilhouetteStyle {
        always: true,
        ..WHITE
    };
    let on = renderer
        .render(&scene(cube(), vec![always], &[(0, 1)]), size())
        .expect("on");
    assert_eq!(px(&on, C, C), [255, 255, 255]);
    let none = renderer
        .render(&scene(cube(), vec![], &[]), size())
        .expect("none");
    assert_eq!(px(&on, 4, 4), px(&none, 4, 4), "the sky is untouched");
}

/// The outline is an outer dilation: a pixel just outside the hidden region, within the radius, takes
/// the outline alpha, and one beyond the radius is untouched. Skips with no GPU.
#[test]
fn needs_gpu_the_outline_reaches_its_radius_and_no_further() {
    let Some(renderer) = skip_or_renderer("the_outline_reaches_its_radius_and_no_further") else {
        return;
    };
    // 8 px at 1080 is 4 px at this 540-line extent.
    let outlined = SilhouetteStyle {
        outline_px: 8.0,
        outline_alpha: 0.5,
        ..WHITE
    };
    let none = renderer
        .render(&scene(big_wall_small_figure(), vec![], &[]), size())
        .expect("none");
    let on = renderer
        .render(
            &scene(big_wall_small_figure(), vec![outlined], &[(1, 1)]),
            size(),
        )
        .expect("on");
    // The figure's front face is ~52 px from the centre; the wall around it is style 0.
    let wall = i32::from(px(&none, C + 54, C)[0]);
    let ring = i32::from(px(&on, C + 54, C)[0]);
    // The mix is in linear colour; the target stores it sRGB-encoded.
    let c = wall as f32 / 255.0;
    let linear = if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    };
    let expected = srgb8_from_linear((linear + 1.0) / 2.0);
    assert!(
        (ring as f32 - expected).abs() <= 3.0,
        "an outline pixel is the wall mixed halfway to white: {ring} vs {expected}"
    );
    assert_eq!(
        px(&on, C + 62, C),
        px(&none, C + 62, C),
        "beyond the radius"
    );
    assert_eq!(
        px(&on, C + 30, C),
        [255, 255, 255],
        "inside the hidden region"
    );
}

/// A `Viewmodel` mesh in the table is not drawn into the mask. Skips with no GPU.
#[test]
fn needs_gpu_a_viewmodel_mesh_is_never_in_the_mask() {
    let Some(renderer) = skip_or_renderer("a_viewmodel_mesh_is_never_in_the_mask") else {
        return;
    };
    let always = SilhouetteStyle {
        always: true,
        ..WHITE
    };
    let meshes = || {
        let mut figure = slab(Vec3::new(0.0, 0.0, -10.0), 2.0, [0.5; 3]);
        figure.viewmodel = true;
        vec![slab(Vec3::new(0.0, 0.0, -4.0), 4.0, [0.2; 3]), figure]
    };
    let none = renderer
        .render(&scene(meshes(), vec![], &[]), size())
        .expect("none");
    let tagged = renderer
        .render(&scene(meshes(), vec![always], &[(1, 1)]), size())
        .expect("tagged");
    assert_eq!(none.rgba, tagged.rgba);
}

/// `big_wall_small_figure` with the figure (mesh 1) in `style`, on this renderer.
fn hidden_figure(renderer: &Renderer, style: SilhouetteStyle) -> RenderedImage {
    renderer
        .render(
            &scene(big_wall_small_figure(), vec![style], &[(1, 1)]),
            size(),
        )
        .expect("styled")
}

/// Scanlines are anchored to the screen rows: in a fully hidden region a row on the lit half of the
/// period is the tint and one on the unlit half is the untouched frame. Skips with no GPU.
#[test]
fn needs_gpu_scanlines_tint_alternate_rows() {
    let Some(renderer) = skip_or_renderer("scanlines_tint_alternate_rows") else {
        return;
    };
    // 8 px at 1080 is a 4 px period at this 540-line extent; duty 0.5 lights rows 0 and 1 of each.
    let lines = SilhouetteStyle {
        pattern: SilhouettePattern::Scanlines,
        pattern_period: 8.0,
        pattern_duty: 0.5,
        ..WHITE
    };
    let none = renderer
        .render(&scene(big_wall_small_figure(), vec![], &[]), size())
        .expect("none");
    let on = hidden_figure(&renderer, lines);
    let (lit, unlit) = (268, 270);
    assert_eq!(px(&on, C, lit), [255, 255, 255], "a lit row takes the tint");
    assert_eq!(px(&on, C, lit + 1), [255, 255, 255]);
    assert_eq!(
        px(&on, C, unlit),
        px(&none, C, unlit),
        "an unlit row is untouched"
    );
    assert_eq!(
        px(&on, C + 7, lit),
        [255, 255, 255],
        "the rows run across x"
    );
    assert_eq!(px(&on, C + 7, unlit), px(&none, C + 7, unlit));
}

/// Hatch lines follow `x + y`: pixels on one diagonal share a state, and the next diagonal band differs.
/// Skips with no GPU.
#[test]
fn needs_gpu_hatch_follows_the_diagonal() {
    let Some(renderer) = skip_or_renderer("hatch_follows_the_diagonal") else {
        return;
    };
    let hatch = SilhouetteStyle {
        pattern: SilhouettePattern::Hatch,
        pattern_period: 8.0,
        pattern_duty: 0.5,
        ..WHITE
    };
    let none = renderer
        .render(&scene(big_wall_small_figure(), vec![], &[]), size())
        .expect("none");
    let on = hidden_figure(&renderer, hatch);
    // x + y = 536 is 0 mod 4 (lit); 538 is 2 mod 4 (unlit).
    for step in 0..4 {
        assert_eq!(
            px(&on, C + step, 266 - step),
            [255, 255, 255],
            "lit diagonal"
        );
        assert_eq!(
            px(&on, C + step, 268 - step),
            px(&none, C + step, 268 - step),
            "unlit diagonal"
        );
    }
}

/// The inner outline is a band inside the fill edge: the edge pixel and the one inside it take the inner
/// alpha, the centre keeps the fill. Skips with no GPU.
#[test]
fn needs_gpu_the_inner_outline_hugs_the_edge() {
    let Some(renderer) = skip_or_renderer("the_inner_outline_hugs_the_edge") else {
        return;
    };
    let fill = SilhouetteStyle {
        color: [1.0, 1.0, 1.0, 0.5],
        ..WHITE
    };
    // 4 px at 1080 is a 2 px band at this 540-line extent.
    let banded = SilhouetteStyle {
        inner_px: 4.0,
        inner_alpha: 1.0,
        ..fill
    };
    let none = renderer
        .render(&scene(big_wall_small_figure(), vec![], &[]), size())
        .expect("none");
    let plain = hidden_figure(&renderer, fill);
    let on = hidden_figure(&renderer, banded);
    let mut edge = C;
    while px(&plain, edge + 1, C) != px(&none, edge + 1, C) {
        edge += 1;
    }
    assert!(edge > C + 20, "found the figure's right edge at {edge}");
    assert_eq!(px(&on, edge, C), [255, 255, 255], "the edge pixel");
    assert_eq!(px(&on, edge - 1, C), [255, 255, 255], "one pixel inside");
    assert_eq!(
        px(&on, edge - 3, C),
        px(&plain, edge - 3, C),
        "beyond the band"
    );
    assert_eq!(px(&on, C, C), px(&plain, C, C), "the centre keeps the fill");
    assert_ne!(px(&plain, C, C), [255, 255, 255]);
}

/// Pattern and inner-outline parameters on a style that enables neither change no byte. Skips with
/// no GPU.
#[test]
fn needs_gpu_a_plain_style_ignores_unused_pattern_parameters() {
    let Some(renderer) = skip_or_renderer("a_plain_style_ignores_unused_pattern_parameters") else {
        return;
    };
    let noisy = SilhouetteStyle {
        pattern_period: 8.0,
        pattern_duty: 0.25,
        inner_alpha: 0.5,
        ..WHITE
    };
    let a = hidden_figure(&renderer, WHITE);
    let b = hidden_figure(&renderer, noisy);
    assert_eq!(a.rgba, b.rgba);
}

fn fnv(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325, |h, b| {
        (h ^ u64::from(*b)).wrapping_mul(0x0100_0000_01b3)
    })
}

/// The grain-owning passes' bytes on a frame with no silhouette, hashed.
fn frozen_hashes(renderer: &mut Renderer) -> Vec<(&'static str, u64)> {
    use super::transparent_dof::{grain_post, grain_scene};
    use crate::graph::RenderScale;
    let (w, h) = (96u32, 64u32);
    let scale = |factor: f32| FrameSize::scaled(w, h, RenderScale::new(factor).unwrap());
    let owners = [
        ("composite", AntiAliasing::Off, FrameSize::native(w, h)),
        ("smaa_blend", AntiAliasing::Smaa, FrameSize::native(w, h)),
        ("resolve", AntiAliasing::Off, scale(2.0)),
        ("upscale", AntiAliasing::Off, scale(0.5)),
    ];
    owners
        .into_iter()
        .map(|(name, aa, size)| {
            renderer.set_anti_aliasing(aa);
            let mut scene = grain_scene(size.output.0, size.output.1);
            scene.post = Some(grain_post(0.05, 1.0, size.output.1));
            let image = renderer.render(&scene, size).expect("render");
            (name, fnv(&image.rgba))
        })
        .collect()
}

/// The frozen path: a frame with no silhouette keeps its bytes in every grain-owning pass. Skips with no GPU.
#[test]
fn needs_gpu_frozen_frames_keep_their_bytes_in_every_grain_owner() {
    let Some(mut renderer) =
        skip_or_renderer("frozen_frames_keep_their_bytes_in_every_grain_owner")
    else {
        return;
    };
    let Some(expected) = pinned_for_adapter(
        &renderer,
        "frozen_frames_keep_their_bytes_in_every_grain_owner",
        &FROZEN_HASHES,
    ) else {
        return;
    };
    assert_eq!(frozen_hashes(&mut renderer), expected.to_vec());
}

/// The frozen-path hashes per pinned reference adapter (bytes differ across GPUs). The RTX 3090's
/// were captured before the film weight existed; the Apple M4's were captured after it, on a tree
/// whose committed macOS goldens all re-rendered byte-identically.
const FROZEN_HASHES: [(&str, [(&str, u64); 4]); 2] = [
    (
        "RTX 3090",
        [
            ("composite", 0xbe6f_267a_ad86_8b28),
            ("smaa_blend", 0xd212_fe3b_5a8f_e175),
            ("resolve", 0xd212_fe3b_5a8f_e175),
            ("upscale", 0xd212_fe3b_5a8f_e175),
        ],
    ),
    (
        "Apple M4",
        [
            ("composite", 0x864c_2c6d_8eef_d9d7),
            ("smaa_blend", 0xcffa_b07d_9558_ff60),
            ("resolve", 0xcffa_b07d_9558_ff60),
            ("upscale", 0xcffa_b07d_9558_ff60),
        ],
    ),
];

/// Opaque white fill over a hidden figure, grain 0.05 and vignette 0.8, `film` as given, through the
/// pass that owns the grain.
fn filmed(renderer: &mut Renderer, film: bool, aa: AntiAliasing, size: FrameSize) -> RenderedImage {
    renderer.set_anti_aliasing(aa);
    let mut scene = scene(
        big_wall_small_figure(),
        vec![SilhouetteStyle { film, ..WHITE }],
        &[(1, 1)],
    );
    scene.post = Some(PostData {
        grain: 0.05,
        grain_size: 1.0 / PX as f32,
        vignette: 0.8,
        vignette_radius: 0.0,
        ..exact_post()
    });
    renderer.render(&scene, size).expect("render")
}

/// `film: false` keeps the grain and vignette off an opaque fill, in each pass that owns the grain: the
/// composite, the SMAA blend, and a resample's last pass. Skips with no GPU.
#[test]
fn needs_gpu_film_false_keeps_the_fill_free_of_vignette_and_grain() {
    use crate::graph::RenderScale;
    let Some(mut renderer) =
        skip_or_renderer("film_false_keeps_the_fill_free_of_vignette_and_grain")
    else {
        return;
    };
    let scaled = |factor: f32| FrameSize::scaled(PX, PX, RenderScale::new(factor).unwrap());
    let owners = [
        ("composite", AntiAliasing::Off, size()),
        ("smaa_blend", AntiAliasing::Smaa, size()),
        ("resolve", AntiAliasing::Off, scaled(2.0)),
        ("upscale", AntiAliasing::Off, scaled(0.5)),
    ];
    for (name, aa, frame) in owners {
        let off = filmed(&mut renderer, false, aa, frame);
        let on = filmed(&mut renderer, true, aa, frame);
        let (w, h) = (off.width, off.height);
        let mut differs = false;
        for y in (h / 2 - 12)..(h / 2 + 12) {
            for x in (w / 2 - 12)..(w / 2 + 12) {
                assert_eq!(px(&off, x, y), [255, 255, 255], "{name}: fill at {x},{y}");
                differs |= px(&on, x, y) != [255, 255, 255];
            }
        }
        assert!(
            differs,
            "{name}: the film:true fill carries grain or vignette"
        );
    }
}
