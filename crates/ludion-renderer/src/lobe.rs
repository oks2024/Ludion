//! The roughness → reflection-footprint function every reflection path shares.
//!
//! GGX importance sampling has `tan²θh = α²·ξ/(1−ξ)` with α = roughness²; the half-vector cone
//! holding half the lobe (ξ = ½) is `θh = atan(α)`, and under the prefilter's N = V = R assumption
//! the reflected direction deviates by `2θh`. So `cone_half_angle(r) = 2·atan(r²)`: 0 for a mirror,
//! exactly π/2 at r = 1. Its solid angle is `4π·x²/(1+x²)` (x = r²) — no trig on the lookup path.
//! A cube face of edge `E` has centre-texel solid angle `(2/E)²` (the largest texel). The lobe is
//! matched to K texels, K = 2π/(2/8)² = 32π, so the r = 1 hemisphere fills the [`FLOOR_FACE`]×8 face
//! and the chain stops there. Solving `Ω(r) = K·(2/E)²` with `E = 8·2^(M−lod)` gives [`cube_lod`];
//! [`mip_roughness`] is its exact inverse above mip 0, so the prefilter and the lookup agree by
//! construction. Mip 0 is the unfiltered mirror: below r ≈ 0.149 the lobe is under 1.2° on 0.45°
//! texels, a mirror to the eye. The WGSL twin of `cube_lod` is `shaders/snippets/lobe.wgsl`.

use ludion_core::math::{atan, log2, sqrt, tan};

/// Below this a roughness is a mirror as far as the log in [`cube_lod`] is concerned.
#[cfg_attr(not(test), allow(dead_code))] // the CPU twin of lobe.wgsl; outside tests only the sync test reads it
const MIN_ALPHA: f32 = 1e-6;
/// Keeps `tan` finite in [`pyramid_level`].
const MAX_HALF_ANGLE: f32 = 1.55;

/// Half-angle of the cone holding half of the GGX lobe at `roughness`, in radians.
#[allow(dead_code)] // read by the SSR pass; no caller yet
pub(crate) fn cone_half_angle(roughness: f32) -> f32 {
    let r = roughness.clamp(0.0, 1.0);
    2.0 * atan(r * r)
}

/// The roughness whose [`cone_half_angle`] is `half_angle`.
#[allow(dead_code)] // read by the SSR pass; no caller yet
pub(crate) fn roughness_for_half_angle(half_angle: f32) -> f32 {
    sqrt(tan(half_angle * 0.5).max(0.0))
}

/// The smallest face of a specular chain. A 1×1 face cannot hold a wide lobe over a gradient sky
/// (it collapses to the face average), so the chain stops at 8×8, as Filament's does.
pub(crate) const FLOOR_FACE: u32 = 8;

/// The top lod `M` of a specular chain of base face `size`: its mips run `0..=M`, down to
/// [`FLOOR_FACE`].
pub(crate) fn chain_max_lod(size: u32) -> u32 {
    debug_assert!(size >= 2 * FLOOR_FACE, "a chain needs at least two mips");
    size.ilog2() - FLOOR_FACE.ilog2()
}

/// The specular cube lod for `roughness` on a chain whose top lod is `max_lod` (`M`).
#[cfg_attr(not(test), allow(dead_code))] // the CPU twin of lobe.wgsl; outside tests only the sync test reads it
pub(crate) fn cube_lod(roughness: f32, max_lod: f32) -> f32 {
    let r = roughness.clamp(0.0, 1.0);
    let x = (r * r).max(MIN_ALPHA);
    (max_lod + 0.5 + log2(x / sqrt(1.0 + x * x))).clamp(0.0, max_lod)
}

/// The roughness mip `mip` of a chain with top lod `max_lod` is prefiltered at: the inverse of
/// [`cube_lod`] above mip 0, which is the mirror.
pub(crate) fn mip_roughness(mip: u32, max_lod: u32) -> f32 {
    if mip == 0 {
        return 0.0;
    }
    let q = 0.5 * ludion_core::math::exp2(2.0 * (mip as f32 - max_lod as f32));
    sqrt(sqrt(q / (1.0 - q)))
}

/// The colour-pyramid level whose texels span the cone's diameter at the hit, for a cone of
/// `half_angle` at `hit_distance`; `pixels_per_unit` is the focal length in pixels over the hit's
/// view depth. Read by the SSR pass; no caller in this crate yet.
#[allow(dead_code)]
pub(crate) fn pyramid_level(half_angle: f32, hit_distance: f32, pixels_per_unit: f32) -> f32 {
    let diameter = 2.0 * hit_distance * tan(half_angle.min(MAX_HALF_ANGLE)) * pixels_per_unit;
    log2(diameter.max(1.0))
}

#[cfg(test)]
mod tests {
    use super::*;
    use ludion_render_graph::Gpu;
    use std::f32::consts::FRAC_PI_2;

    #[test]
    fn the_cone_spans_a_mirror_to_the_hemisphere() {
        assert_eq!(cone_half_angle(0.0), 0.0);
        assert!((cone_half_angle(1.0) - FRAC_PI_2).abs() < 1e-5);
    }

    #[test]
    fn the_cone_grows_with_roughness() {
        let mut last = -1.0;
        for i in 0..=100 {
            let a = cone_half_angle(i as f32 / 100.0);
            assert!(a > last, "not monotonic at {i}");
            last = a;
        }
    }

    #[test]
    fn the_inverse_recovers_the_roughness() {
        for i in 1..=100 {
            let r = i as f32 / 100.0;
            if r > 0.99 {
                continue; // tan blows up at the hemisphere
            }
            assert!(
                (roughness_for_half_angle(cone_half_angle(r)) - r).abs() < 1e-3,
                "r = {r}"
            );
        }
    }

    #[test]
    fn the_lod_round_trips_every_mip_above_the_mirror() {
        for max_lod in [5, 4] {
            for m in 1..=max_lod {
                let lod = cube_lod(mip_roughness(m, max_lod), max_lod as f32);
                assert!(
                    (lod - m as f32).abs() < 1e-3,
                    "M = {max_lod}, m = {m}: {lod}"
                );
            }
        }
    }

    #[test]
    fn the_lod_spans_the_chain() {
        for max_lod in [5.0, 4.0] {
            assert_eq!(cube_lod(0.0, max_lod), 0.0);
            assert_eq!(cube_lod(1.0, max_lod), max_lod);
        }
    }

    #[test]
    fn the_per_mip_roughness_table_matches_the_plan() {
        let want = [0.0, 0.2103, 0.2979, 0.4238, 0.6148, 1.0];
        for (m, w) in want.iter().enumerate() {
            let r = mip_roughness(m as u32, 5);
            assert!((r - w).abs() < 1e-3, "mip {m}: {r} vs {w}");
        }
    }

    #[test]
    fn a_half_size_cube_is_the_full_table_shifted_by_one() {
        assert_eq!(mip_roughness(0, 4), 0.0);
        for m in 1..=4 {
            let big = mip_roughness(m + 1, 5);
            let small = mip_roughness(m, 4);
            assert!((big - small).abs() < 1e-5, "mip {m}");
        }
    }

    #[test]
    fn the_chain_stops_at_the_floor_face() {
        assert_eq!(chain_max_lod(256), 5);
        assert_eq!(chain_max_lod(128), 4);
    }

    #[test]
    fn the_pyramid_level_grows_with_angle_and_distance() {
        assert_eq!(pyramid_level(0.0, 5.0, 100.0), 0.0);
        assert!(pyramid_level(0.3, 5.0, 100.0) > pyramid_level(0.1, 5.0, 100.0));
        assert!(pyramid_level(0.3, 10.0, 100.0) > pyramid_level(0.3, 5.0, 100.0));
        assert!(pyramid_level(FRAC_PI_2, 5.0, 100.0).is_finite());
    }

    /// The WGSL snippet and `cube_lod` are one formula in two languages.
    #[test]
    fn needs_gpu_the_lobe_snippet_matches_its_rust_twin() {
        let gpu = match Gpu::request() {
            Ok(gpu) => gpu,
            Err(d) if ludion_core::require_gpu::is_no_adapter(std::slice::from_ref(&d)) => {
                eprintln!("SKIPPED the lobe snippet test: no GPU adapter");
                return;
            }
            Err(d) => panic!("gpu request failed: {d:?}"),
        };
        let device = gpu.device();
        const N: usize = 65;
        for max_lod in [5.0_f32, 4.0] {
            let src = format!(
                "{}\n@group(0) @binding(0) var<storage, read_write> out: array<f32>;\n\
                 @compute @workgroup_size(1) fn main(@builtin(global_invocation_id) id: vec3<u32>) {{\n\
                 out[id.x] = specular_cube_lod(f32(id.x) / {}.0, {max_lod:.1});\n}}\n",
                include_str!("shaders/snippets/lobe.wgsl"),
                N - 1,
            );
            let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("lobe test"),
                source: wgpu::ShaderSource::Wgsl(src.into()),
            });
            let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some("lobe test"),
                layout: None,
                module: &module,
                entry_point: Some("main"),
                compilation_options: Default::default(),
                cache: None,
            });
            let bytes = (N * 4) as u64;
            let storage = device.create_buffer(&wgpu::BufferDescriptor {
                label: None,
                size: bytes,
                usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
                mapped_at_creation: false,
            });
            let readback = device.create_buffer(&wgpu::BufferDescriptor {
                label: None,
                size: bytes,
                usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: None,
                layout: &pipeline.get_bind_group_layout(0),
                entries: &[wgpu::BindGroupEntry {
                    binding: 0,
                    resource: storage.as_entire_binding(),
                }],
            });
            let mut enc = device.create_command_encoder(&Default::default());
            {
                let mut pass = enc.begin_compute_pass(&Default::default());
                pass.set_pipeline(&pipeline);
                pass.set_bind_group(0, &bind, &[]);
                pass.dispatch_workgroups(N as u32, 1, 1);
            }
            enc.copy_buffer_to_buffer(&storage, 0, &readback, 0, bytes);
            gpu.queue().submit(Some(enc.finish()));
            readback
                .slice(..)
                .map_async(wgpu::MapMode::Read, |r| r.unwrap());
            device
                .poll(wgpu::PollType::wait_indefinitely())
                .expect("device poll succeeds");
            let data = readback.slice(..).get_mapped_range().expect("range maps");
            let got: &[f32] = bytemuck::cast_slice(&data);
            for (i, g) in got.iter().enumerate() {
                let want = cube_lod(i as f32 / (N - 1) as f32, max_lod);
                assert!(
                    (g - want).abs() <= 1e-4,
                    "L = {max_lod}, r = {i}/64: gpu {g} vs {want}"
                );
            }
        }
    }
}
