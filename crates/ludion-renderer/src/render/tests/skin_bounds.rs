//! The shadow cull's skinned bounds and the persistent box table: GPU tests.

use super::*;

/// A skinned quad whose single joint has an identity inverse bind, so the shadow cull bounds it
/// by its posed palette.
pub(super) fn bounded_skin_quad(hash: u64) -> MeshData {
    let mut quad = skin_quad([1.0, 0.0, 0.0, 0.0], [0, 0, 0, 0], hash);
    quad.skin_joints = vec![crate::scene::SkinJoint {
        name: "root".into(),
        parent: None,
        rest_local: Mat4::IDENTITY,
        rest_translation: Vec3::ZERO,
        rest_rotation: ludion_core::math::glam::Quat::IDENTITY,
        rest_scale: Vec3::ONE,
        inverse_bind: Mat4::IDENTITY,
        rest: Mat4::IDENTITY,
    }];
    quad
}

/// A horizontal 1.5× quad (an exact `Rx(-90°)`, so a GPU skin multiply and a CPU model agree
/// bit for bit) at world `translation`.
fn skin_flat_model(translation: Vec3) -> Mat4 {
    use ludion_core::math::glam::Vec4;
    Mat4::from_cols(
        Vec4::new(1.5, 0.0, 0.0, 0.0),
        Vec4::new(0.0, 0.0, -1.5, 0.0),
        Vec4::new(0.0, 1.5, 0.0, 0.0),
        translation.extend(1.0),
    )
}

/// **A skinned caster is culled by its posed bounds, not its rest pose.** The instance's model
/// puts the rest pose far outside the spot's range; the palette moves it under the light. It must
/// still cast, byte for byte as its unskinned twin placed there by a model matrix does.
#[test]
fn needs_gpu_a_skinned_caster_resting_outside_the_spot_still_casts_from_its_pose() {
    let Some(renderer) = skip_or_renderer("a_skinned_caster_resting_outside_the_spot") else {
        return;
    };
    let (w, h) = (192u32, 108u32);
    let size = FrameSize::native(w, h);
    let mut scene = local_receive_scene(w, h);
    scene
        .mesh_assets
        .push(Arc::new(bounded_skin_quad(0x5C0_0001)));
    scene.spot_lights = vec![casting_spot(Vec3::new(0.0, 8.0, 0.0), 20.0, true)];
    let empty = renderer.render(&scene, size).expect("no caster");

    // Model-space −32 on X is world −48 under the 1.5× model: from x = 48 to the origin.
    let mut posed = scene.clone();
    posed
        .meshes
        .push(skin_instance(skin_flat_model(Vec3::new(48.0, 3.0, 0.0)), 1));
    posed.bone_palette = vec![Mat4::from_translation(Vec3::new(-32.0, 0.0, 0.0))];
    let skinned = renderer.render(&posed, size).expect("posed render");

    let mut twin = scene.clone();
    twin.meshes
        .push(skin_instance(skin_flat_model(Vec3::new(0.0, 3.0, 0.0)), 0));
    let placed = renderer.render(&twin, size).expect("twin render");

    let (probe_x, probe_y) = pixel_of(&scene, Vec3::new(2.2, 0.0, 0.0), w, h);
    assert!(
        luma(&placed, probe_x, probe_y) * 2 < luma(&empty, probe_x, probe_y),
        "the twin shadows the floor under the light",
    );
    assert_eq!(
        skinned.rgba, placed.rgba,
        "the posed instance casts like its twin"
    );
}

/// The cull's box table holds one row per primitive then per asset, is built once per asset list,
/// and a second frame over the same assets reuses the very same buffer.
#[test]
fn needs_gpu_the_cull_bounds_table_is_uploaded_once_per_asset_list() {
    let Some(renderer) = skip_or_renderer("the_cull_bounds_table_is_uploaded_once") else {
        return;
    };
    let size = FrameSize::native(64, 64);
    let mut scene = RenderScene::empty((64, 64));
    let quad = bounded_skin_quad(0x5C0_0002);
    scene.mesh_assets.push(Arc::new(quad.clone()));

    let first = renderer
        .prepare_frame(&scene, size, None)
        .expect("first frame")
        .cull_bounds_buf;
    let second = renderer
        .prepare_frame(&scene, size, None)
        .expect("second frame")
        .cull_bounds_buf;
    assert!(
        first == second,
        "the same asset list keeps the table buffer"
    );

    let want = crate::cull::bounds_table(&[AssetBounds::of_mesh(&quad)]);
    let device = renderer.gpu.device();
    let bytes = (want.len() * std::mem::size_of::<crate::cull::CullBoundsRaw>()) as u64;
    assert_eq!(first.size(), bytes);
    let staging = device.create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size: bytes,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut encoder =
        device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
    encoder.copy_buffer_to_buffer(&first, 0, &staging, 0, bytes);
    renderer.gpu.queue().submit(Some(encoder.finish()));
    let (tx, rx) = std::sync::mpsc::channel();
    staging.slice(..).map_async(wgpu::MapMode::Read, move |r| {
        let _ = tx.send(r);
    });
    device
        .poll(wgpu::PollType::wait_indefinitely())
        .expect("the device polls");
    rx.recv().unwrap().expect("the buffer maps");
    let mapped = staging.slice(..).get_mapped_range().expect("mapped range");
    assert_eq!(&*mapped, bytemuck::cast_slice::<_, u8>(&want));
    drop(mapped);

    scene
        .mesh_assets
        .push(Arc::new(skin_quad([1.0, 0.0, 0.0, 0.0], [0, 0, 0, 0], 3)));
    let grown = renderer
        .prepare_frame(&scene, size, None)
        .expect("grown frame")
        .cull_bounds_buf;
    assert!(grown != first, "a new asset list rebuilds the table");
    assert_eq!(grown.size(), bytes + 32);
}
