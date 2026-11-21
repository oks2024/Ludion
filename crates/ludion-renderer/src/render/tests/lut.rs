//! The `.cube` LUT, composite step 7a: indexed by the sRGB-encoded colour, trilinear, mixed by
//! `strength`, uploaded once per content hash.

use super::*;
use crate::scene::LutData;
use ludion_assets::cube::{CubeLut, identity};
use std::sync::Arc;

const PATCH: Vec3 = Vec3::new(0.0, 0.0, -2.0);

/// A patch of one flat emissive colour filling the frame, the imaging chain out of the way (no bloom,
/// no tonemap, no dither) so the centre pixel is the colour, encoded.
fn patch_scene(w: u32, h: u32, color: [f32; 3]) -> RenderScene {
    let mut scene = RenderScene::empty((w, h));
    scene.camera = origin_camera(w, h);
    scene.ambient = [0.0, 0.0, 0.0];
    scene.meshes = vec![film_box(PATCH, Vec3::new(8.0, 8.0, 0.5), color)];
    scene.post = Some(PostData {
        exposure: 1.0,
        tonemap: TonemapOperator::None,
        bloom_strength: 0.0,
        dither: false,
        ..PostData::default()
    });
    scene
}

fn lut_data(lut: CubeLut, hash: u64, strength: f32) -> LutData {
    LutData {
        lut: Arc::new(CubeLut { hash, ..lut }),
        strength,
    }
}

/// Red and blue swapped: `rgb[i] = [b, g, r]` of the identity.
fn channel_swap(size: u32) -> CubeLut {
    let mut lut = identity(size);
    for row in &mut lut.rgb {
        *row = [row[2], row[1], row[0]];
    }
    lut
}

fn centre(renderer: &Renderer, scene: &RenderScene, size: FrameSize) -> [u8; 3] {
    let image = renderer.render(scene, size).expect("render");
    rgb_at(&image, image.width / 2, image.height / 2)
}

fn within(a: [u8; 3], b: [u8; 3], steps: u8) -> bool {
    (0..3).all(|c| a[c].abs_diff(b[c]) <= steps)
}

/// The f16 packer: exact on the lattice values a LUT carries, clamped, and sound at the bottom of the
/// subnormal range.
#[test]
fn unit_f32_to_f16_bits_matches_ieee_half() {
    assert_eq!(unit_f32_to_f16_bits(0.0), 0);
    assert_eq!(unit_f32_to_f16_bits(1.0), 0x3C00);
    assert_eq!(unit_f32_to_f16_bits(0.5), 0x3800);
    assert_eq!(unit_f32_to_f16_bits(2.0), 0x3C00, "clamped to 1");
    assert_eq!(unit_f32_to_f16_bits(-1.0), 0, "clamped to 0");
    assert_eq!(unit_f32_to_f16_bits(f32::NAN), 0);
    // 2^-24 is the smallest subnormal half; half of it ties to even (zero).
    assert_eq!(unit_f32_to_f16_bits(2.0f32.powi(-24)), 1);
    assert_eq!(unit_f32_to_f16_bits(2.0f32.powi(-25)), 0);
    assert_eq!(unit_f32_to_f16_bits(2.0f32.powi(-14)), 0x0400);
    for i in 0..=1000 {
        let v = i as f32 / 1000.0;
        let back = crate::gi_bake::f16_to_f32(unit_f32_to_f16_bits(v));
        assert!((back - v).abs() <= 1.0 / 2048.0 + 1e-7, "{v} -> {back}");
    }
}

/// **A 33³ identity LUT at full strength is invisible** — within one output step per channel.
#[test]
fn needs_gpu_an_identity_lut_leaves_the_image_alone() {
    let Some(renderer) = skip_or_renderer("an_identity_lut_leaves_the_image_alone") else {
        return;
    };
    let size = FrameSize::native(64, 36);
    let mut scene = patch_scene(64, 36, [0.45, 0.2, 0.08]);
    let without = centre(&renderer, &scene, size);
    scene.lut = Some(lut_data(identity(33), 1, 1.0));
    let with = centre(&renderer, &scene, size);
    assert!(within(without, with, 1), "{without:?} -> {with:?}");
}

/// **A channel-swap LUT turns a red patch blue**, and half strength lands between the two.
#[test]
fn needs_gpu_a_channel_swap_lut_turns_red_blue_and_strength_blends() {
    let Some(renderer) = skip_or_renderer("a_channel_swap_lut_turns_red_blue") else {
        return;
    };
    let size = FrameSize::native(64, 36);
    let mut scene = patch_scene(64, 36, [0.6, 0.05, 0.05]);
    let plain = centre(&renderer, &scene, size);
    assert!(
        plain[0] > 150 && plain[2] < 100,
        "fixture is red: {plain:?}"
    );

    scene.lut = Some(lut_data(channel_swap(33), 2, 1.0));
    let swapped = centre(&renderer, &scene, size);
    assert!(
        swapped[2] > 150 && swapped[0] < 100,
        "red patch turns blue: {plain:?} -> {swapped:?}"
    );
    assert!(
        within([plain[2], plain[1], plain[0]], swapped, 3),
        "the swap is the plain colour reversed: {plain:?} -> {swapped:?}"
    );

    scene.lut = Some(lut_data(channel_swap(33), 2, 0.5));
    let half = centre(&renderer, &scene, size);
    assert!(
        half[0] < plain[0] && half[0] > swapped[0] && half[2] > plain[2] && half[2] < swapped[2],
        "half strength lies between: {plain:?} {half:?} {swapped:?}"
    );
}

/// **A wider input domain darkens**: an identity LUT with `DOMAIN_MAX 2` reads the table at half the
/// encoded value, so a mid-grey patch comes out darker.
#[test]
fn needs_gpu_a_wider_domain_darkens_a_mid_grey_patch() {
    let Some(renderer) = skip_or_renderer("a_wider_domain_darkens") else {
        return;
    };
    let size = FrameSize::native(64, 36);
    let mut scene = patch_scene(64, 36, [0.2, 0.2, 0.2]);
    let plain = centre(&renderer, &scene, size);
    let mut wide = identity(33);
    wide.domain_max = [2.0; 3];
    scene.lut = Some(lut_data(wide, 3, 1.0));
    let darker = centre(&renderer, &scene, size);
    assert!(
        (0..3).all(|c| darker[c] + 40 < plain[c]),
        "{plain:?} -> {darker:?}"
    );
}

/// **A new content hash re-uploads**: two LUTs in turn through one renderer give two results, and
/// dropping the LUT returns the plain image.
#[test]
fn needs_gpu_a_changed_hash_reuploads_the_table() {
    let Some(renderer) = skip_or_renderer("a_changed_hash_reuploads") else {
        return;
    };
    let size = FrameSize::native(64, 36);
    let mut scene = patch_scene(64, 36, [0.6, 0.05, 0.05]);
    let plain = centre(&renderer, &scene, size);
    scene.lut = Some(lut_data(channel_swap(17), 10, 1.0));
    let swapped = centre(&renderer, &scene, size);
    scene.lut = Some(lut_data(identity(33), 11, 1.0));
    let identity_again = centre(&renderer, &scene, size);
    assert!(!within(plain, swapped, 20), "{plain:?} {swapped:?}");
    assert!(
        within(plain, identity_again, 1),
        "{plain:?} {identity_again:?}"
    );
    scene.lut = None;
    assert!(within(plain, centre(&renderer, &scene, size), 0));
}
