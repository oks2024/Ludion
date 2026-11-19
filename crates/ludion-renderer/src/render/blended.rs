//! The blended meshes' draw order and draws (`Plan/transparent-depth.md` decisions 5–7).

use super::mesh_batch::{MeshInstanceRaw, instance_raw, is_mirrored};
use super::rain::WetInput;
use super::{
    CAMERA_GROUP, ENV_GROUP, FrameRecorder, LIGHTS_GROUP, MATERIAL_GROUP, MeshAssetGpu,
    MeshVariant, SUN_GROUP, ShaderId, init_buffer,
};
use crate::cull::{Aabb, AssetBounds, local_bounds};
use crate::scene::{BlendMode, MeshShape, RenderScene};
use ludion_core::math::glam::Vec3;

/// One blended instance's draw: its geometry, mode and pipeline variant. Instance `i` of the draw's
/// buffer belongs to item `i`.
struct BlendedItem {
    shape: MeshShape,
    mode: BlendMode,
    variant: MeshVariant,
    /// The uploaded asset, `None` for a primitive.
    asset: Option<MeshAssetGpu>,
}

/// The frame's blended meshes in draw order: one instance buffer, one draw per item.
pub(super) struct BlendedDraw {
    instances: wgpu::Buffer,
    items: Vec<BlendedItem>,
}

/// Sorts `scene.blended` ([`blend_order`]) and uploads it. `None` when nothing is drawable: an asset
/// whose upload is missing or empty is skipped, like the opaque draws.
pub(super) fn build_blended_draw(
    scene: &RenderScene,
    device: &wgpu::Device,
    meshes: &[MeshAssetGpu],
) -> Option<BlendedDraw> {
    if scene.blended.is_empty() {
        return None;
    }
    let bounds: Vec<AssetBounds> = meshes.iter().map(|asset| asset.bounds.clone()).collect();
    let mut raws: Vec<MeshInstanceRaw> = Vec::new();
    let mut items: Vec<BlendedItem> = Vec::new();
    for i in blend_order(scene, &bounds) {
        let part = &scene.blended[i as usize];
        let (asset, double_sided) = match part.instance.shape {
            MeshShape::Asset(index) => match meshes.get(index) {
                Some(mesh) if mesh.vertex_count > 0 && mesh.index_count > 0 => {
                    (Some(mesh.clone()), mesh.double_sided)
                }
                _ => continue,
            },
            _ => (None, false),
        };
        raws.push(instance_raw(&part.instance, &bounds));
        items.push(BlendedItem {
            shape: part.instance.shape,
            mode: part.mode,
            variant: MeshVariant {
                mirrored: is_mirrored(&part.instance.model),
                double_sided,
            },
            asset,
        });
    }
    if items.is_empty() {
        return None;
    }
    let instances = init_buffer(
        device,
        wgpu::BufferUsages::VERTEX,
        bytemuck::cast_slice(&raws),
    );
    Some(BlendedDraw { instances, items })
}

impl FrameRecorder<'_> {
    /// Draws the blended meshes inside a recording pass: the colour pass when `layer` is `None`, else
    /// the transparent layer of `Some(format)`, `Rgba16Float` being the one that carries the medium. One draw
    /// per instance in sorted order, lit by the scene pass's shadows and GI but no ambient occlusion;
    /// `inputs` are the pass's declared reads.
    pub(super) fn record_blended(
        &self,
        rp: &mut wgpu::RenderPass<'_>,
        device: &wgpu::Device,
        inputs: &[&wgpu::TextureView],
        layer: Option<ludion_render_graph::TextureFormat>,
    ) {
        let Some(draw) = &self.blended_draw else {
            return;
        };
        let twins = &self.r.pipelines[ShaderId::Mesh as usize]
            .cascade_view
            .as_ref()
            .expect("the mesh shader has lazy twins")
            .blend;
        let (sun_bg, env_bg) = self.shading_groups(device, inputs, false, WetInput::Mesh(None));
        rp.set_bind_group(CAMERA_GROUP, &self.camera_skin_bg, &[]);
        rp.set_bind_group(LIGHTS_GROUP, &self.lights_bg, &[]);
        rp.set_bind_group(SUN_GROUP, &sun_bg, &[]);
        rp.set_bind_group(ENV_GROUP, &env_bg, &[]);
        rp.set_vertex_buffer(1, draw.instances.slice(..));
        let mut bound: Option<(BlendMode, MeshVariant)> = None;
        for (i, item) in draw.items.iter().enumerate() {
            if bound != Some((item.mode, item.variant)) {
                bound = Some((item.mode, item.variant));
                rp.set_pipeline(match layer {
                    None => twins.color(device, item.mode, item.variant),
                    Some(format) => {
                        let medium = format == ludion_render_graph::TextureFormat::Rgba16Float;
                        twins.layer(device, item.mode, item.variant, medium)
                    }
                });
            }
            let instance = i as u32..i as u32 + 1;
            match &item.asset {
                None => {
                    let mb = self.r.mesh_of(item.shape);
                    rp.set_bind_group(MATERIAL_GROUP, &self.r.default_material_bg, &[]);
                    rp.set_vertex_buffer(0, mb.buffer.slice(..));
                    rp.draw(0..mb.vertex_count, instance);
                }
                Some(mesh) => {
                    rp.set_bind_group(MATERIAL_GROUP, &mesh.material_bg, &[]);
                    rp.set_vertex_buffer(0, mesh.vertices.slice(..));
                    rp.set_index_buffer(mesh.indices.slice(..), wgpu::IndexFormat::Uint32);
                    rp.draw_indexed(0..mesh.index_count, 0, instance);
                }
            }
        }
    }
}

/// The order `scene.blended` is drawn in, as indices: groups far to near, parts of a group together
/// and in list order. A group's key is the view distance to the centre of the union of its parts'
/// world boxes (the depth along the view under an orthographic camera); a part whose box is unknown
/// counts as its model's origin. Ties go to the lower group.
///
/// Pure CPU, so the sort is testable without an adapter. [`f32::total_cmp`] keeps it a total order
/// under a NaN, as `particle_order` does.
pub(super) fn blend_order(scene: &RenderScene, assets: &[AssetBounds]) -> Vec<u32> {
    let view = &scene.camera.view;
    let ortho = scene.camera.projection.w_axis.w == 1.0;
    let mut groups: Vec<(u32, Option<Aabb>, usize, usize)> = Vec::new();
    for (i, part) in scene.blended.iter().enumerate() {
        let world = match local_bounds(part.instance.shape, assets) {
            Some(local) => local.transformed(&part.instance.model),
            None => {
                let at = part.instance.model.w_axis.truncate();
                Aabb { min: at, max: at }
            }
        };
        match groups.last_mut() {
            Some((group, bounds, _, end)) if *group == part.group => {
                let b = bounds.get_or_insert(world);
                b.min = b.min.min(world.min);
                b.max = b.max.max(world.max);
                *end = i + 1;
            }
            _ => groups.push((part.group, Some(world), i, i + 1)),
        }
    }
    let mut keyed: Vec<(f32, u32, usize, usize)> = groups
        .into_iter()
        .map(|(group, bounds, start, end)| {
            let b = bounds.expect("a group has a part");
            let center: Vec3 = (b.min + b.max) * 0.5;
            let v = view.transform_point3(center);
            let distance = if ortho { -v.z } else { v.length() };
            (distance, group, start, end)
        })
        .collect();
    // Far first; the group index makes every key distinct.
    keyed.sort_unstable_by(|a, b| b.0.total_cmp(&a.0).then(a.1.cmp(&b.1)));
    keyed
        .into_iter()
        .flat_map(|(_, _, start, end)| (start as u32)..(end as u32))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scene::{BlendMode, BlendedMesh, MeshInstance, MeshShape, RenderCamera};
    use ludion_core::math::glam::{Mat3, Mat4};

    fn part(at: Vec3, group: u32) -> BlendedMesh {
        BlendedMesh {
            instance: MeshInstance {
                medium: 1.0,
                shape: MeshShape::Cube,
                model: Mat4::from_translation(at),
                normal_matrix: Mat3::IDENTITY,
                color: [1.0, 1.0, 1.0, 0.5],
                material: [1.0, 0.0, 0.0],
                use_vertex_material: false,
                fade: 1.0,
                fade_height_min: -f32::MAX,
                flash: 0.0,
                flash_color: [1.0; 3],
                wrap: 0.0,
                scatter_color: [1.0; 3],
                bone_offset: 0,
                cast_shadows: false,
                viewmodel: false,
                rim: [0.0; 4],
            },
            mode: BlendMode::Alpha,
            group,
        }
    }

    fn scene(parts: Vec<BlendedMesh>, eye: Vec3, ortho: bool) -> RenderScene {
        let mut scene = RenderScene::empty((64, 64));
        scene.camera = RenderCamera {
            view: Mat4::look_at_rh(eye, eye - Vec3::Z, Vec3::Y),
            projection: if ortho {
                Mat4::orthographic_rh(-5.0, 5.0, -5.0, 5.0, 0.1, 100.0)
            } else {
                Mat4::perspective_rh(1.0, 1.0, 0.1, 100.0)
            },
            position: eye,
        };
        scene.blended = parts;
        scene
    }

    #[test]
    fn far_draws_before_near() {
        let s = scene(
            vec![
                part(Vec3::new(0.0, 0.0, -2.0), 0),
                part(Vec3::new(0.0, 0.0, -9.0), 1),
            ],
            Vec3::ZERO,
            false,
        );
        assert_eq!(blend_order(&s, &[]), vec![1, 0]);
    }

    #[test]
    fn the_order_follows_the_camera() {
        let parts = vec![
            part(Vec3::new(0.0, 0.0, -2.0), 0),
            part(Vec3::new(0.0, 0.0, -9.0), 1),
        ];
        // From the far side the near one is the other pane.
        let mut s = scene(parts, Vec3::new(0.0, 0.0, -20.0), false);
        s.camera.view = Mat4::look_at_rh(
            Vec3::new(0.0, 0.0, -20.0),
            Vec3::new(0.0, 0.0, -2.0),
            Vec3::Y,
        );
        assert_eq!(blend_order(&s, &[]), vec![0, 1]);
    }

    #[test]
    fn ties_go_to_the_lower_group() {
        let s = scene(
            vec![
                part(Vec3::new(1.0, 0.0, -5.0), 3),
                part(Vec3::new(-1.0, 0.0, -5.0), 1),
            ],
            Vec3::ZERO,
            true,
        );
        assert_eq!(blend_order(&s, &[]), vec![1, 0]);
    }

    #[test]
    fn a_group_stays_together_and_in_order() {
        // Group 0's parts straddle group 1 in depth; its centre is the union's, so it sorts as one.
        let s = scene(
            vec![
                part(Vec3::new(0.0, 0.0, -2.0), 0),
                part(Vec3::new(0.0, 0.0, -12.0), 0),
                part(Vec3::new(0.0, 0.0, -8.0), 1),
            ],
            Vec3::ZERO,
            false,
        );
        // Group 0's centre is at z = -7 (nearer than group 1 at -8): group 1 first.
        assert_eq!(blend_order(&s, &[]), vec![2, 0, 1]);
    }

    #[test]
    fn ortho_sorts_by_depth_along_the_view() {
        // Equal depth, different lateral offset: the perspective distance would differ, ortho's does not.
        let s = scene(
            vec![
                part(Vec3::new(4.0, 0.0, -5.0), 0),
                part(Vec3::new(0.0, 0.0, -5.0), 1),
            ],
            Vec3::ZERO,
            true,
        );
        assert_eq!(blend_order(&s, &[]), vec![0, 1]);
        let p = scene(
            vec![
                part(Vec3::new(4.0, 0.0, -5.0), 0),
                part(Vec3::new(0.0, 0.0, -5.0), 1),
            ],
            Vec3::ZERO,
            false,
        );
        assert_eq!(blend_order(&p, &[]), vec![0, 1]);
    }

    #[test]
    fn a_nan_position_does_not_panic_and_keeps_every_part() {
        let s = scene(
            vec![
                part(Vec3::new(f32::NAN, 0.0, -5.0), 0),
                part(Vec3::new(0.0, 0.0, -3.0), 1),
                part(Vec3::new(0.0, 0.0, -9.0), 2),
            ],
            Vec3::ZERO,
            false,
        );
        let mut order = blend_order(&s, &[]);
        order.sort_unstable();
        assert_eq!(order, vec![0, 1, 2]);
    }

    #[test]
    fn empty_is_empty() {
        assert!(blend_order(&scene(vec![], Vec3::ZERO, false), &[]).is_empty());
    }
}
