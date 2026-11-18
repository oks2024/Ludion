//! The occluded-silhouette mask's draws: only the meshes the scene's [`Silhouettes`] table names,
//! each instance carrying its 1-based style index in the flash lane (`.x`), which the mask shader
//! writes as `index / 255`. Direct draws only: no cull bucket, no statics, no viewmodel.

use super::mesh_batch::{MeshAssetDraw, MeshDraw, MeshInstanceRaw, instance_raw, is_mirrored};
use super::{MeshAssetGpu, init_buffer};
use crate::cull::AssetBounds;
use crate::scene::{MAX_SILHOUETTE_STYLES, MeshShape, RenderScene};
use ludion_core::math::glam::Vec3;

/// The mask's primitive and asset draws over one shared instance buffer; both empty when no mesh
/// resolves a style. A `Viewmodel` mesh in the table is skipped.
pub(super) fn build_silhouette_draws(
    scene: &RenderScene,
    device: &wgpu::Device,
    meshes: &[MeshAssetGpu],
) -> (Vec<MeshDraw>, Vec<MeshAssetDraw>) {
    let mut style_of = vec![0u8; scene.meshes.len()];
    for entry in &scene.silhouettes.meshes {
        if let Some(slot) = style_of.get_mut(entry.mesh as usize) {
            *slot = entry.style;
        }
    }
    let asset_bounds: Vec<AssetBounds> = meshes.iter().map(|asset| asset.bounds.clone()).collect();
    let raw_of = |scene_index: usize| -> MeshInstanceRaw {
        let m = &scene.meshes[scene_index];
        let mut raw = instance_raw(m, &asset_bounds);
        raw.flash = [f32::from(style_of[scene_index]), 0.0, 0.0, 0.0];
        raw
    };
    let selected =
        |scene_index: usize| style_of[scene_index] != 0 && !scene.meshes[scene_index].viewmodel;

    // (shape, mirrored, raws), primitives then assets, each in ascending scene order.
    let mut prims: Vec<(MeshShape, bool, Vec<MeshInstanceRaw>)> = Vec::new();
    for mirrored in [false, true] {
        for shape in [MeshShape::Cube, MeshShape::Plane, MeshShape::Sphere] {
            let sources: Vec<u32> = (0..scene.meshes.len())
                .filter(|&i| {
                    selected(i)
                        && scene.meshes[i].shape == shape
                        && is_mirrored(&scene.meshes[i].model) == mirrored
                })
                .map(|i| i as u32)
                .collect();
            if !sources.is_empty() {
                let raws = sources.iter().map(|&i| raw_of(i as usize)).collect();
                prims.push((shape, mirrored, raws));
            }
        }
    }
    let mut assets: Vec<(usize, bool, Vec<MeshInstanceRaw>)> = Vec::new();
    for mirrored in [false, true] {
        for (index, mesh) in meshes.iter().enumerate() {
            if mesh.vertex_count == 0 || mesh.index_count == 0 {
                continue;
            }
            let sources: Vec<u32> = (0..scene.meshes.len())
                .filter(|&i| {
                    selected(i)
                        && scene.meshes[i].shape == MeshShape::Asset(index)
                        && is_mirrored(&scene.meshes[i].model) == mirrored
                })
                .map(|i| i as u32)
                .collect();
            if !sources.is_empty() {
                let raws = sources.iter().map(|&i| raw_of(i as usize)).collect();
                assets.push((index, mirrored, raws));
            }
        }
    }

    let all: Vec<MeshInstanceRaw> = prims
        .iter()
        .flat_map(|p| p.2.iter())
        .chain(assets.iter().flat_map(|a| a.2.iter()))
        .copied()
        .collect();
    if all.is_empty() {
        return (Vec::new(), Vec::new());
    }
    let buffer = init_buffer(
        device,
        wgpu::BufferUsages::VERTEX,
        bytemuck::cast_slice(&all),
    );
    let mut first = 0u32;
    let draws = prims
        .into_iter()
        .map(|(shape, mirrored, raws)| {
            let draw = MeshDraw {
                shape,
                instances: buffer.clone(),
                count: raws.len() as u32,
                first,
                bucket: None,
                mirrored,
                viewmodel: false,
                sources: Vec::new(),
                statics: None,
            };
            first += raws.len() as u32;
            draw
        })
        .collect();
    let asset_draws = assets
        .into_iter()
        .map(|(index, mirrored, raws)| {
            let draw = MeshAssetDraw {
                mesh: meshes[index].clone(),
                instances: buffer.clone(),
                count: raws.len() as u32,
                first,
                bucket: None,
                mirrored,
                viewmodel: false,
                sources: Vec::new(),
                statics: None,
            };
            first += raws.len() as u32;
            draw
        })
        .collect();
    (draws, asset_draws)
}

/// The composite's silhouette uniform lanes: header `(on, any outline, widest radius px, style count)`,
/// the camera's forward axis, and two vec4 per style `(color)` then
/// `(outline radius in render pixels, outline alpha, bias, always)`. All zero when the frame has no
/// silhouette, which is the composite's frozen path.
#[derive(Clone, Copy, Default)]
pub(super) struct CompositeLanes {
    pub(super) header: [u32; 4],
    pub(super) forward: [f32; 4],
    pub(super) styles: [[f32; 4]; 2 * MAX_SILHOUETTE_STYLES],
}

/// The outline radius in render pixels: `px` is in 1080p pixels and scales with the render height,
/// at least one pixel.
pub(super) fn outline_radius(px: f32, render_height: u32) -> f32 {
    if px <= 0.0 {
        return 0.0;
    }
    (px * render_height as f32 / 1080.0).round().max(1.0)
}

pub(super) fn composite_lanes(scene: &RenderScene, render_height: u32, on: bool) -> CompositeLanes {
    let mut lanes = CompositeLanes::default();
    if !on {
        return lanes;
    }
    let view = &scene.camera.view;
    let forward = -Vec3::new(view.x_axis.z, view.y_axis.z, view.z_axis.z).normalize_or_zero();
    lanes.forward = [forward.x, forward.y, forward.z, 0.0];
    let count = scene.silhouettes.styles.len().min(MAX_SILHOUETTE_STYLES);
    let mut widest = 0.0f32;
    for (i, style) in scene.silhouettes.styles[..count].iter().enumerate() {
        let radius = outline_radius(style.outline_px, render_height);
        widest = widest.max(radius);
        lanes.styles[i * 2] = style.color;
        lanes.styles[i * 2 + 1] = [
            radius,
            style.outline_alpha,
            style.bias,
            f32::from(u8::from(style.always)),
        ];
    }
    lanes.header = [1, u32::from(widest > 0.0), widest as u32, count as u32];
    lanes
}
