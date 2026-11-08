//! The medium's recorders: the sun's shaft march and the local lights' in-scatter pair (uniform fill,
//! blur, and the selected lights' shader lanes). Kept beside `render.rs` rather than in it.

use bytemuck::Zeroable;
use ludion_core::math::glam::Vec3;

use super::{FrameRecorder, OptionalInput, RecordedPass, ShaderId, UNSHADOWED, target_texel};
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
            lights,
        };
        let optional = [OptionalInput {
            binding: shader_gen::SCATTER_LAYER_BINDING,
            present: self.has_transparent_depth,
            fallback: &self.r.fallback_hdr.view,
        }];
        self.fullscreen_effect(
            pass,
            ShaderId::MediumScatter,
            &optional,
            bytemuck::bytes_of(&scatter),
        );
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

/// The frame's scatter lanes: the [`crate::scatter::select_scatter`] selection when the graph
/// scheduled the pair, otherwise none.
pub(super) fn lanes_for(scene: &RenderScene, scheduled: bool) -> Vec<shader_gen::ScatterLight> {
    if scheduled {
        scatter_light_lanes(scene, &crate::scatter::select_scatter(scene))
    } else {
        Vec::new()
    }
}
