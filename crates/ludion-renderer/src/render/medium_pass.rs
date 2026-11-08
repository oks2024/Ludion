//! The medium's recorders: the sun's shaft march and the local lights' in-scatter pair (uniform fill,
//! blur, and the selected lights' shader lanes). Kept beside `render.rs` rather than in it.

use bytemuck::Zeroable;
use ludion_core::math::glam::Vec3;

use super::{
    FrameRecorder, LayoutId, OptionalInput, RecordedPass, ShaderId, UNSHADOWED, local_view_gpu,
    target_texel,
};
use crate::local_shadow::LocalShadowPlan;
use crate::scene::RenderScene;
use crate::shader_gen;

impl FrameRecorder<'_> {
    /// The volumetric light-shaft march, at a fraction of the output extent, from the scene pass's
    /// depth and the sun's shadow map (the pass's declared read order, which is also the shader's
    /// binding order). The authored intensity is folded in here so the blurred result *is* the term
    /// the composite tints and adds — the same posture `record_ssao` takes.
    ///
    /// The medium is the composite's: `fog_density` is the extinction the march integrates and
    /// bounds its reach by, `fog_albedo` the scattering fraction it scales the estimate by, and
    /// `fog_anisotropy` its phase asymmetry. `fog_start` / `fog_height` / `fog_height_falloff` carry
    /// the medium's *shape* across too, so the march integrates the same optical depth the haze does.
    /// `shafts` is a pure intensity on top of that.
    pub(super) fn record_shafts(&self, pass: &mut RecordedPass<'_>) {
        let sun = self
            .sun
            .expect("the shafts pass is only scheduled for a scene with a sun");
        let lanes = &self.sun_lanes;
        let shafts = shader_gen::Shafts {
            inv_view_proj: self.inv_view_proj.to_cols_array_2d(),
            light_space: sun.light_space.to_cols_array_2d(),
            cam_pos: self.camera_position.to_array(),
            density: self.post.fog_density,
            sun_dir: sun.direction.to_array(),
            intensity: self.post.shafts,
            albedo: self.post.fog_albedo,
            anisotropy: self.post.fog_anisotropy,
            fog_start: self.post.fog_start,
            fog_height: self.post.fog_height,
            fog_height_falloff: self.post.fog_height_falloff,
            steps: self.r.quality_params.shaft_steps,
            transparent_depth: u32::from(self.has_transparent_depth),
            pad1: 0.0,
            cascade_space: lanes.space,
            cascade_rect: lanes.rect,
            cascade_far: lanes.params.map(|lane| lane[1]),
            view_depth: lanes.view_depth,
            cascade_count: lanes.count,
            cascade_pad0: 0,
            cascade_pad1: 0,
            cascade_pad2: 0,
            view_forward: self.view_forward,
        };
        let optional = [OptionalInput {
            binding: shader_gen::SHAFTS_LAYER_BINDING,
            present: self.has_transparent_depth,
            fallback: &self.r.fallback_hdr.view,
        }];
        self.fullscreen_effect(
            pass,
            ShaderId::Shafts,
            &optional,
            bytemuck::bytes_of(&shafts),
        );
    }

    /// The local-light in-scatter march, at the shafts' extent, from the scene pass's depth (and the
    /// transparent layer when there is one). The medium is the shafts': `fog_*` carry across
    /// unchanged. Steps per light segment are a quarter of the shafts' steps, floored at two, so the
    /// quality preset reaches this pass through the same `shaft_steps`.
    pub(super) fn record_medium_scatter(&self, pass: &mut RecordedPass<'_>) {
        let mut lights = [shader_gen::ScatterLight::zeroed(); crate::scatter::MAX_SCATTER_LIGHTS];
        lights[..self.scatter_lights.len()].copy_from_slice(&self.scatter_lights);
        let mut views = [shader_gen::LocalView::zeroed(); crate::scatter::MAX_SCATTER_LIGHTS];
        views[..self.scatter_views.len()].copy_from_slice(&self.scatter_views);
        let gi = &self.resolved_gi;
        let scatter = shader_gen::MediumScatter {
            inv_view_proj: self.inv_view_proj.to_cols_array_2d(),
            cam_pos: self.camera_position.to_array(),
            density: self.post.fog_density,
            albedo: self.post.fog_albedo,
            anisotropy: self.post.fog_anisotropy,
            fog_start: self.post.fog_start,
            fog_height: self.post.fog_height,
            fog_height_falloff: self.post.fog_height_falloff,
            steps: (self.r.quality_params.shaft_steps / 4).max(2),
            light_count: self.scatter_lights.len() as u32,
            transparent_depth: u32::from(self.has_transparent_depth),
            view_forward: self.view_forward,
            gi_origin: gi.origin,
            gi_scale: gi.scale,
            gi: [
                if self.has_medium_gi {
                    self.gi_strengths.sky_visibility
                } else {
                    0.0
                },
                if self.has_medium_gi {
                    self.gi_strengths.bounce
                } else {
                    0.0
                },
                f32::from(self.has_medium_gi && self.gi_inject.is_some()),
                f32::from(self.has_medium_gi && gi.origin[3] > 0.5),
            ],
            lights,
            views,
        };
        let optional = [
            OptionalInput {
                binding: shader_gen::SCATTER_LAYER_BINDING,
                present: self.has_transparent_depth,
                fallback: &self.r.fallback_hdr.view,
            },
            OptionalInput {
                binding: shader_gen::SCATTER_ATLAS_BINDING,
                present: self.local_shadow.is_some(),
                fallback: &self.r.fallback_shadow.view,
            },
            OptionalInput {
                binding: shader_gen::SCATTER_DYN_BINDING,
                present: self.has_medium_gi && self.gi_inject.is_some(),
                fallback: self.r.gi_inject.fallback(),
            },
        ];
        let volumes = self.gi_volumes_bind_group(pass.device);
        self.fullscreen_effect_with(
            pass,
            ShaderId::MediumScatter,
            &optional,
            bytemuck::bytes_of(&scatter),
            Some(&volumes),
        );
    }

    /// The baked GI volumes the march binds at group 1, in the shader's binding order.
    fn gi_volumes_bind_group(&self, device: &wgpu::Device) -> wgpu::BindGroup {
        let gi = &self.resolved_gi;
        let views = [&gi.probes, &gi.bounce[0], &gi.bounce[1], &gi.bounce[2]];
        let entries: Vec<_> = views
            .into_iter()
            .enumerate()
            .map(|(binding, view)| wgpu::BindGroupEntry {
                binding: binding as u32,
                resource: wgpu::BindingResource::TextureView(view),
            })
            .collect();
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("medium scatter gi bind group"),
            layout: self.r.layouts.get(LayoutId::MediumScatterGi),
            entries: &entries,
        })
    }

    /// The 4×4 box blur over the local-light term's four channels. Source and target share an extent.
    pub(super) fn record_medium_scatter_blur(&self, pass: &mut RecordedPass<'_>) {
        let [u, v] = target_texel(pass);
        let params = shader_gen::ScatterBlurParams {
            texel: [u, v, 0.0, 0.0],
        };
        self.fullscreen_effect(
            pass,
            ShaderId::MediumScatterBlur,
            &[],
            bytemuck::bytes_of(&params),
        );
    }
}

/// The shader lanes of the selected scattering lights (`indices` are light-list positions: points,
/// then spots), in the order given. The bounding sphere is the cluster binner's own; a point light
/// writes `cos_outer = -2`, the "no cone" sentinel the surface shaders use.
pub(super) fn scatter_light_lanes(
    scene: &RenderScene,
    indices: &[u32],
) -> Vec<shader_gen::ScatterLight> {
    let points = scene.lights.len();
    indices
        .iter()
        .map(|&index| {
            let index = index as usize;
            let (position, axis, color, intensity, range, scatter, cos_inner, cos_outer) =
                if index < points {
                    let l = &scene.lights[index];
                    (
                        l.position,
                        Vec3::ZERO,
                        l.color,
                        l.intensity,
                        l.range,
                        l.scatter,
                        0.0,
                        -2.0,
                    )
                } else {
                    let l = &scene.spot_lights[index - points];
                    (
                        l.position,
                        l.direction,
                        l.color,
                        l.intensity,
                        l.range,
                        l.scatter,
                        l.cos_inner,
                        l.cos_outer,
                    )
                };
            let range = range.max(1e-3);
            let (center, radius) = crate::cluster::light_sphere(position, axis, range, cos_outer);
            let radiance = color * (intensity * scatter);
            shader_gen::ScatterLight {
                position: [position.x, position.y, position.z, range],
                radiance: [radiance.x, radiance.y, radiance.z, cos_inner],
                axis: [axis.x, axis.y, axis.z, cos_outer],
                sphere: [center.x, center.y, center.z, radius],
                shadow: [UNSHADOWED, 0, 0, 0],
            }
        })
        .collect()
}

/// The shadow views the selected lights hold this frame, compacted: a granted light's one (spot) or
/// six (cube) views land consecutively in the next free slots, and its lane's `shadow` becomes
/// `(first_slot, view_count, 0, 0)`. A light with no grant keeps `(UNSHADOWED, 0, 0, 0)`. The order
/// follows `selected`. The plan's light indices are the selection's (points, then spots). At most 64
/// slots are used: the atlas holds at most `ATLAS_CELLS²` views.
pub(super) fn scatter_views(
    plan: &LocalShadowPlan,
    selected: &[u32],
) -> (Vec<shader_gen::LocalView>, Vec<[u32; 4]>) {
    let mut views = Vec::new();
    let mut lanes = Vec::with_capacity(selected.len());
    for &light in selected {
        let first = plan.light_first_view[light as usize];
        if first == UNSHADOWED {
            lanes.push([UNSHADOWED, 0, 0, 0]);
            continue;
        }
        let count = plan.views.iter().filter(|v| v.light == light).count() as u32;
        lanes.push([views.len() as u32, count, 0, 0]);
        for view in &plan.views[first as usize..(first + count) as usize] {
            views.push(local_view_gpu(view, plan.stats.atlas_edge, count));
        }
    }
    (views, lanes)
}

/// The frame's scatter lanes and shadow views: the [`crate::scatter::select_scatter`] selection when
/// the graph scheduled the pair, otherwise none. Lights the frame's `plan` granted shadow their scatter.
pub(super) fn lanes_for(
    scene: &RenderScene,
    scheduled: bool,
    plan: Option<&LocalShadowPlan>,
) -> (Vec<shader_gen::ScatterLight>, Vec<shader_gen::LocalView>) {
    if !scheduled {
        return (Vec::new(), Vec::new());
    }
    let selected = crate::scatter::select_scatter(scene);
    let mut lanes = scatter_light_lanes(scene, &selected);
    let Some(plan) = plan else {
        return (lanes, Vec::new());
    };
    let (views, shadow) = scatter_views(plan, &selected);
    for (lane, shadow) in lanes.iter_mut().zip(shadow) {
        lane.shadow = shadow;
    }
    (lanes, views)
}

#[cfg(test)]
mod tests {
    use ludion_core::math::glam::Vec3;

    use super::*;
    use crate::local_shadow::{ATLAS_CELLS, plan_local_shadows};
    use crate::scene::{LocalShadowData, PointLightData, SpotLightData};

    fn data() -> Option<LocalShadowData> {
        Some(LocalShadowData {
            bias: 0.05,
            near: 0.1,
            distance: None,
            fade: 0.0,
        })
    }

    fn point(x: f32, shadow: Option<LocalShadowData>) -> PointLightData {
        PointLightData {
            position: Vec3::new(x, 0.0, 0.0),
            color: Vec3::ONE,
            intensity: 1.0,
            range: 10.0,
            dynamic: false,
            shadow,
            scatter: 1.0,
        }
    }

    fn spot(x: f32, shadow: Option<LocalShadowData>) -> SpotLightData {
        SpotLightData {
            position: Vec3::new(x, 0.0, 0.0),
            direction: Vec3::NEG_Z,
            color: Vec3::ONE,
            intensity: 1.0,
            range: 10.0,
            cos_inner: 0.95,
            cos_outer: 0.9,
            shadow,
            scatter: 1.0,
        }
    }

    #[test]
    fn the_atlas_never_holds_more_views_than_the_uniform_array() {
        assert!((ATLAS_CELLS * ATLAS_CELLS) as usize <= crate::scatter::MAX_SCATTER_LIGHTS);
    }

    /// Points then spots in the light list: a cube takes six consecutive slots, a spot one, an
    /// ungranted light keeps `UNSHADOWED`, and the slots follow the selected order.
    #[test]
    fn scatter_views_compacts_in_selected_order() {
        let mut scene = RenderScene::empty((16, 16));
        scene.lights = vec![point(0.0, data()), point(1.0, None)];
        scene.spot_lights = vec![spot(2.0, data())];
        let plan = plan_local_shadows(&scene, 2048);
        // Light-list indices: 0, 1 points, 2 the spot. Select the spot first.
        let (views, lanes) = scatter_views(&plan, &[2, 1, 0]);
        assert_eq!(views.len(), 7);
        assert_eq!(
            lanes,
            vec![[0, 1, 0, 0], [UNSHADOWED, 0, 0, 0], [1, 6, 0, 0]]
        );
        assert_eq!(views[0].params[2], 1.0);
        assert!(views[1..].iter().all(|v| v.params[2] == 6.0));
    }
}
