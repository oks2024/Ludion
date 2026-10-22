//! The screen-space reflection trace, built only when `PostChain::ssr` is on.
//!
//! The `ssr_trace` pass marches one mirror ray per trace texel (output × [`QualityParams::ssr_scale`](crate::QualityParams::ssr_scale))
//! through the `hiz` pyramid and, on a hit, reads `ssr_color` at the cone's footprint level. It
//! writes `vec4(radiance, confidence)` into the `ssr_trace` transient; a miss writes `vec4(0)`. The
//! march is a fixed [`SSR_MAX_ITERS`] iterations with no jitter and no history; a level-0 candidate
//! is resolved against the render pixels the ray crosses, and the first one it is behind decides.
//!
//! **Confidence** is a product of linear ramps ([`confidence`], twin of `ssr_confidence` in
//! `shaders/snippets/ssr.wgsl`): screen edge, ray length, hit thickness, back-facing hits and the
//! roughness fade. Off-screen is exactly 0.

/// The march's iteration cap (the WGSL's `SSR_MAX_ITERS`).
#[cfg_attr(not(test), allow(dead_code))] // the CPU twin of ssr_trace.wgsl; outside tests only the sync test reads it
pub(crate) const SSR_MAX_ITERS: u32 = 128;
/// UV distance from a screen edge over which confidence ramps up.
pub(crate) const EDGE_FADE: f32 = 0.1;
/// Fraction of the maximum distance over which confidence ramps down.
pub(crate) const LENGTH_FADE: f32 = 0.2;
/// Facing (`dot(n_hit, R)`) below which a hit is fully front-facing.
pub(crate) const BACKFACE_FADE: f32 = 0.1;

/// The trace's look parameters.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct SsrLook {
    /// Roughness at which the reflection starts fading and at which it is gone.
    pub fade: [f32; 2],
    /// The ray's world-space length.
    pub max_distance: f32,
    /// World-space depth behind a surface that still counts as a hit.
    pub thickness: f32,
    /// The blend's scale, read by the blend.
    pub intensity: f32,
}

impl SsrLook {
    /// The authored look: the scene's `PostData` SSR fields.
    pub(crate) fn from_post(post: &crate::PostData) -> SsrLook {
        SsrLook {
            fade: post.ssr_fade,
            max_distance: post.ssr_max_distance,
            thickness: post.ssr_thickness,
            intensity: post.ssr,
        }
    }
}

fn saturate(x: f32) -> f32 {
    x.clamp(0.0, 1.0)
}

/// How far to trust a hit, 0..=1. `uv` is the hit's screen position, `hit_distance` the world distance
/// from the origin, `thickness_excess` the hit's depth behind the surface over the look's thickness,
/// `facing` the hit normal against the ray.
#[cfg_attr(not(test), allow(dead_code))] // the CPU twin of ssr.wgsl; outside tests only the sync test reads it
pub(crate) fn confidence(
    uv: [f32; 2],
    hit_distance: f32,
    thickness_excess: f32,
    facing: f32,
    roughness: f32,
    look: &SsrLook,
) -> f32 {
    let edge =
        saturate(uv[0].min(1.0 - uv[0]) / EDGE_FADE) * saturate(uv[1].min(1.0 - uv[1]) / EDGE_FADE);
    let length = saturate((1.0 - hit_distance / look.max_distance) / LENGTH_FADE);
    let thickness = saturate(2.0 * (1.0 - thickness_excess));
    let back_face = saturate(-facing / BACKFACE_FADE);
    let rough = saturate((look.fade[1] - roughness) / (look.fade[1] - look.fade[0]));
    edge * length * thickness * back_face * rough
}

#[cfg(test)]
mod tests {
    use super::*;
    use ludion_render_graph::Gpu;

    fn look() -> SsrLook {
        SsrLook {
            intensity: 1.0,
            ..SsrLook::from_post(&crate::PostData::default())
        }
    }

    /// A hit that is confident on every axis but the one a test varies.
    fn good(uv: [f32; 2], dist: f32, excess: f32, facing: f32, rough: f32) -> f32 {
        confidence(uv, dist, excess, facing, rough, &look())
    }

    #[test]
    fn off_screen_confidence_is_zero() {
        for uv in [
            [-0.1, 0.5],
            [1.1, 0.5],
            [0.5, -0.1],
            [0.5, 1.1],
            [0.0, 0.5],
            [1.0, 0.5],
            [0.5, 0.0],
            [0.5, 1.0],
        ] {
            assert_eq!(good(uv, 1.0, 0.0, -1.0, 0.0), 0.0, "{uv:?}");
        }
        assert_eq!(good([0.5, 0.5], 1.0, 0.0, -1.0, 0.0), 1.0);
    }

    #[test]
    fn roughness_fades_between_the_look_bounds() {
        let at = |r| good([0.5, 0.5], 1.0, 0.0, -1.0, r);
        assert_eq!(at(0.45), 1.0);
        assert_eq!(at(0.0), 1.0);
        assert_eq!(at(0.65), 0.0);
        assert_eq!(at(1.0), 0.0);
        assert!((at(0.55) - 0.5).abs() < 1e-6);
    }

    #[test]
    fn length_fades_towards_the_maximum() {
        let at = |d| good([0.5, 0.5], d, 0.0, -1.0, 0.0);
        assert_eq!(at(look().max_distance), 0.0);
        assert_eq!(at(look().max_distance * 2.0), 0.0);
        assert_eq!(at(look().max_distance * 0.75), 1.0);
        assert_eq!(at(0.0), 1.0);
    }

    #[test]
    fn thickness_fades_between_half_and_one() {
        let at = |e| good([0.5, 0.5], 1.0, e, -1.0, 0.0);
        assert_eq!(at(1.0), 0.0);
        assert_eq!(at(3.0), 0.0);
        assert_eq!(at(0.5), 1.0);
        assert_eq!(at(0.0), 1.0);
    }

    #[test]
    fn back_facing_hits_are_dropped() {
        let at = |f| good([0.5, 0.5], 1.0, 0.0, f, 0.0);
        assert_eq!(at(0.0), 0.0);
        assert_eq!(at(0.5), 0.0);
        assert_eq!(at(-0.1), 1.0);
        assert_eq!(at(-1.0), 1.0);
    }

    #[test]
    fn the_wgsl_declares_the_same_constants() {
        assert_eq!(crate::shader_gen::SSR_MAX_ITERS, SSR_MAX_ITERS);
        let ssr = include_str!("shaders/snippets/ssr.wgsl");
        for (name, v) in [
            ("EDGE", EDGE_FADE),
            ("LENGTH", LENGTH_FADE),
            ("BACKFACE", BACKFACE_FADE),
        ] {
            assert!(
                ssr.contains(&format!("SSR_{name}_FADE: f32 = {v:?};")),
                "{name}"
            );
        }
    }

    /// The WGSL snippet and [`confidence`] are one formula in two languages.
    #[test]
    fn needs_gpu_the_ssr_confidence_snippet_matches_its_rust_twin() {
        let gpu = match Gpu::request() {
            Ok(gpu) => gpu,
            Err(d) if ludion_core::require_gpu::is_no_adapter(std::slice::from_ref(&d)) => {
                eprintln!("SKIPPED the ssr confidence snippet test: no GPU adapter");
                return;
            }
            Err(d) => panic!("gpu request failed: {d:?}"),
        };
        let mut input = Vec::new();
        for ux in [-0.1, 0.0, 0.05, 0.5, 0.95, 1.0, 1.1] {
            for uy in [-0.1, 0.03, 0.5, 1.0, 1.2] {
                for dist in [0.0, 12.0, 25.0, 29.0, 31.0] {
                    for excess in [0.0, 0.7, 1.2] {
                        for facing in [-1.0, -0.05, 0.3] {
                            for rough in [0.0, 0.5, 0.6, 0.9] {
                                input.extend([ux, uy, dist, excess, facing, rough]);
                            }
                        }
                    }
                }
            }
        }
        let n = input.len() / 6;
        let src = format!(
            "{}\n@group(0) @binding(0) var<storage, read> inp: array<f32>;\n\
             @group(0) @binding(1) var<storage, read_write> out: array<f32>;\n\
             @compute @workgroup_size(1) fn main(@builtin(global_invocation_id) id: vec3<u32>) {{\n\
             let i = id.x * 6u;\n\
             out[id.x] = ssr_confidence(vec2<f32>(inp[i], inp[i + 1u]), inp[i + 2u], inp[i + 3u], inp[i + 4u], inp[i + 5u], vec2<f32>({:?}, {:?}), {:?});\n}}\n",
            include_str!("shaders/snippets/ssr.wgsl"),
            look().fade[0],
            look().fade[1],
            look().max_distance,
        );
        let got = crate::lobe::run_f32_kernel(&gpu, &src, &input, n);
        for (k, row) in input.chunks(6).enumerate() {
            let want = confidence([row[0], row[1]], row[2], row[3], row[4], row[5], &look());
            assert!(
                (got[k] - want).abs() <= 1e-5,
                "{row:?}: gpu {} vs {want}",
                got[k]
            );
        }
    }
}
