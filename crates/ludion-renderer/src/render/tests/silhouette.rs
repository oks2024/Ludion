//! The occluded silhouette: GPU tests over the `silhouette_mask` pass and `composite`'s step 7b.

use super::*;
use crate::scene::{SilhouetteMesh, SilhouetteStyle, Silhouettes};

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
