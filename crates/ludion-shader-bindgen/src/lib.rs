//! WGSL shader reflection for the Ludion engine's shader bindgen.
//!
//! This crate parses and validates WGSL with naga, extracts memory layouts with naga's Layouter
//! (so reflected offsets are by construction the ones `create_shader_module` computes), and produces
//! a **wgpu-free** [`Reflection`] model — plain Rust structs describing struct layouts, resource
//! bindings, hoisted scalar consts, and entry-point vertex inputs. The [`emit`] function turns that
//! model into the renderer's committed `shader_gen.rs` (Wave 3), and the crate's thin bin
//! (`cargo run -p ludion-shader-bindgen`) reflects the repo's shaders and rewrites that file.
//!
//! # Compose-then-reflect
//!
//! The unit of reflection is a [`ShaderUnit`] — an optional preamble prepended to a file's source —
//! so the reflector sees exactly the string the pipeline composes. The decision-16 snippet dedup uses
//! this seam: the shared `Camera`/`Light`/`Lights`/`LIGHT_COUNT` declarations and the fullscreen VS
//! live in `shaders/snippets/*.wgsl`, composed ahead of each shader's body per the [`COMPOSITIONS`]
//! registry (`mesh.wgsl` = `camera.wgsl` + `lights.wgsl` + its body, and so on).
//!
//! ```
//! use ludion_shader_bindgen::{reflect, ShaderUnit};
//!
//! let unit = ShaderUnit::new(
//!     "u.wgsl",
//!     r#"
//!     struct Camera { view_proj: mat4x4<f32>, cam_pos: vec4<f32> };
//!     @group(0) @binding(0) var<uniform> camera: Camera;
//!     @vertex fn vs_main(@location(0) p: vec3<f32>) -> @builtin(position) vec4<f32> {
//!         return camera.view_proj * vec4<f32>(p, 1.0);
//!     }
//!     "#,
//! );
//! let reflection = reflect(&[unit]).unwrap();
//! let camera = reflection.struct_named("Camera").unwrap();
//! assert_eq!(camera.size, 80); // mat4x4 (64) + vec4 (16)
//! ```
//!
//! # Unification
//!
//! Same-named structs, resource variables, and consts are unified across all reflected units and a
//! disagreement is a hard error (decisions 4, 13). The bind-group unification key is the **resource
//! variable name**: WGSL bind groups have no names, but the global resource variables do
//! (`camera`, `lights`, `sprite_tex`, …), and that name is the cross-shader identity the plan
//! appeals to. Two units declaring a resource of the same name must agree on group index, binding
//! index, and kind; a differently-named resource reusing the same `(group, binding)` slot in
//! another pipeline is *not* a conflict (e.g. `lights` and `atlas_tex` both at `@group(1)
//! @binding(0)`), so the key is the name, not the slot. Visibility is merged across units.
//!
//! Unification needs no address-space clause: WGSL has one layout algorithm, and the uniform
//! address space only adds *validation constraints* (16-byte member/stride rounding) — it never
//! re-lays a type out. So a struct reached through a uniform in one shader and a
//! `var<storage, read>` array in another unifies to one entry with one layout, and a genuine
//! disagreement is the same `StructConflict` as anywhere else.

mod emit;
mod error;
mod model;
mod reflect;

use std::path::Path;

/// The renderer's reflected shaders (files under `crates/ludion-renderer/src/shaders/`), sorted so
/// the generated header's source list is stable. `tests/real_shaders.rs` checks it against the
/// directory, so a shader cannot be left out silently.
pub const RENDERER_SHADERS: &[&str] = &[
    "blit.wgsl",
    "bloom_down.wgsl",
    "bloom_up.wgsl",
    "box_blur.wgsl",
    "composite.wgsl",
    "cull.wgsl",
    "debug_line.wgsl",
    "decal_lit.wgsl",
    "decal_receiver.wgsl",
    "dof.wgsl",
    "dof_coc.wgsl",
    "gi_bake.wgsl",
    "gi_inject.wgsl",
    "ground_marker.wgsl",
    "hiz_reduce.wgsl",
    "hiz_seed.wgsl",
    "ibl.wgsl",
    "lens_down.wgsl",
    "lens_up.wgsl",
    "local_shadow_debug.wgsl",
    "medium_scatter.wgsl",
    "medium_scatter_blur.wgsl",
    "mesh.wgsl",
    "particle.wgsl",
    "prepass.wgsl",
    "probe_debug.wgsl",
    "probe_face.wgsl",
    "probe_relight.wgsl",
    "projector.wgsl",
    "rain.wgsl",
    "reflections.wgsl",
    "resolve.wgsl",
    "shadow.wgsl",
    "shadow_cutout.wgsl",
    "shafts.wgsl",
    "silhouette_mask.wgsl",
    "sky.wgsl",
    "smaa_blend.wgsl",
    "smaa_edges.wgsl",
    "smaa_weights.wgsl",
    "sprite.wgsl",
    "ssao.wgsl",
    "ssr_blend.wgsl",
    "ssr_color.wgsl",
    "ssr_debug.wgsl",
    "ssr_trace.wgsl",
    "text.wgsl",
    "threshold.wgsl",
    "ui_polygon.wgsl",
    "ui_rect.wgsl",
    "ui_shape.wgsl",
    "ui_text.wgsl",
    "upscale.wgsl",
];

/// WGSL global names of `texture_2d<f32>` bindings that are **not** filterable (an `R32Float`
/// or `Rgba32Float` source read with `textureLoad`). WGSL cannot express it, so reflection takes
/// it from this list; binding names are workspace-unique, so a name is a safe key.
pub const UNFILTERABLE_TEXTURES: &[&str] = &[
    "hiz_src",
    "trace_hiz",
    "probe_live",
    "proj_receivers",
    "lit_receivers",
];

/// Every [`UNFILTERABLE_TEXTURES`] entry must name a reflected `texture_2d<f32>` binding. Reflection
/// itself cannot enforce it (its unit tests reflect tiny units that bind none of the names), so the
/// generator calls this on the real shader set: a renamed binding would otherwise turn filterable
/// without a sound.
pub fn check_unfilterable(reflection: &Reflection) -> Result<(), ReflectError> {
    check_unfilterable_names(reflection, UNFILTERABLE_TEXTURES)
}

fn check_unfilterable_names(reflection: &Reflection, names: &[&str]) -> Result<(), ReflectError> {
    for &name in names {
        let is_float_texture = matches!(
            reflection.binding_named(name).map(|b| &b.kind),
            Some(BindingKind::Texture {
                sample: TextureSample::Float { .. },
                ..
            })
        );
        if !is_float_texture {
            return Err(ReflectError::UnfilterableNotFound { name: name.into() });
        }
    }
    Ok(())
}

/// Units whose bindings must not reach an `atomic<…>`: the GPU cull compacts survivors in input
/// order with a scan, so an atomic append would make draw order depend on thread timing.
pub const ATOMIC_FREE_UNITS: &[&str] = &["cull.wgsl"];

/// No binding of an [`ATOMIC_FREE_UNITS`] unit may reach an `atomic<…>` through arrays or struct members.
pub fn check_atomic_free(reflection: &Reflection) -> Result<(), ReflectError> {
    check_atomic_free_units(reflection, ATOMIC_FREE_UNITS)
}

fn check_atomic_free_units(reflection: &Reflection, units: &[&str]) -> Result<(), ReflectError> {
    for binding in &reflection.bindings {
        if !matches!(binding.kind, BindingKind::Buffer { atomic: true, .. }) {
            continue;
        }
        if let Some(unit) = binding.units.iter().find(|u| units.contains(&u.as_str())) {
            return Err(ReflectError::AtomicInAtomicFreeUnit {
                unit: unit.clone(),
                binding: binding.name.clone(),
            });
        }
    }
    Ok(())
}

/// The render graph's reflected shaders (files under `crates/ludion-render-graph/src/shaders/`).
pub const RENDER_GRAPH_SHADERS: &[&str] = &["gradient.wgsl", "vignette.wgsl", "window_blit.wgsl"];

/// The renderer shaders built outside `ShaderId`, with auto-derived layouts (`layout: None`); the
/// renderer's `shader_ids_cover_the_bindgen_list` test keeps this and `ShaderId::all()` disjoint and
/// together equal to [`RENDERER_SHADERS`]. The renderer's `ComputeShader::ALL` lists the same names in
/// the same order, which a renderer test pins.
pub const COMPUTE_SHADERS: &[&str] = &[
    "cull.wgsl",
    "gi_bake.wgsl",
    "gi_inject.wgsl",
    "ibl.wgsl",
    "probe_relight.wgsl",
];

/// The shared-snippet composition registry (Plan/shader-bindgen.md decision 16): each renderer shader
/// maps to the ordered snippet file names (under `shaders/snippets/`) prepended before its own body.
/// The composed string a shader reflects = each snippet's text, then the body, joined by `\n` — exactly
/// what `Renderer` composes and `create_shader_module` sees.
///
/// **This is the bindgen half of the mapping; the renderer declares the same mapping at compile time
/// (`ShaderId::snippets` / the `Snippet` enum in `ludion-renderer`'s `render.rs`, and
/// `ComputeShader::snippets` for the compute shaders).** The two cannot be
/// physically merged — the renderer needs compile-time `include_str!` composition and must not depend
/// on this crate, and this crate must not drag wgpu in by depending on the renderer. A drift between
/// the two is caught by renderer tests (`renderer_composition_matches_bindgen_registry` and
/// `compute_composition_matches_bindgen_registry`) that byte-compare both compositions for every
/// shader, so a mismatch fails CI loudly. Shaders absent
/// here (render-graph's `gradient`/`vignette`/`window_blit`, `ui_rect`) compose no snippets.
pub const COMPOSITIONS: &[(&str, &[&str])] = &[
    (
        "mesh.wgsl",
        &[
            "camera.wgsl",
            "transparent_layer.wgsl",
            "skin.wgsl",
            "cluster_grid.wgsl",
            "lights.wgsl",
            "bayer.wgsl",
            "hdr.wgsl",
            "dfg.wgsl",
            "gi_probe.wgsl",
            "view_ray.wgsl",
            "math.wgsl",
            "rain_occlusion.wgsl",
            "mesh_shading.wgsl",
            "mesh_material.wgsl",
        ],
    ),
    (
        "sprite.wgsl",
        &[
            "camera.wgsl",
            "transparent_layer.wgsl",
            "cluster_grid.wgsl",
            "lights.wgsl",
            "billboard_light.wgsl",
            "quad.wgsl",
        ],
    ),
    (
        "text.wgsl",
        &["camera.wgsl", "transparent_layer.wgsl", "glyph.wgsl"],
    ),
    ("ui_text.wgsl", &["glyph.wgsl"]),
    ("blit.wgsl", &["fullscreen_vs.wgsl"]),
    ("ibl.wgsl", &["math.wgsl", "dfg.wgsl", "sky_gradient.wgsl"]),
    ("gi_bake.wgsl", &["math.wgsl", "gi_rays.wgsl"]),
    ("gi_inject.wgsl", &["math.wgsl", "gi_rays.wgsl"]),
    ("bloom_down.wgsl", &["fullscreen_vs.wgsl", "hdr.wgsl"]),
    ("bloom_up.wgsl", &["fullscreen_vs.wgsl", "hdr.wgsl"]),
    ("lens_down.wgsl", &["fullscreen_vs.wgsl", "hdr.wgsl"]),
    ("lens_up.wgsl", &["fullscreen_vs.wgsl", "hdr.wgsl"]),
    ("box_blur.wgsl", &["fullscreen_vs.wgsl"]),
    (
        "medium_scatter.wgsl",
        &["fullscreen_vs.wgsl", "bayer.wgsl", "medium.wgsl"],
    ),
    ("medium_scatter_blur.wgsl", &["fullscreen_vs.wgsl"]),
    (
        "composite.wgsl",
        &[
            "fullscreen_vs.wgsl",
            "bayer.wgsl",
            "medium.wgsl",
            "sky_gradient.wgsl",
            "view_ray.wgsl",
            "hdr.wgsl",
            "srgb.wgsl",
            "output_encode.wgsl",
        ],
    ),
    ("debug_line.wgsl", &["camera.wgsl"]),
    (
        "decal_lit.wgsl",
        &[
            "fullscreen_vs.wgsl",
            "camera.wgsl",
            "cluster_grid.wgsl",
            "lights.wgsl",
            "hdr.wgsl",
            "dfg.wgsl",
            "gi_probe.wgsl",
            "view_ray.wgsl",
            "math.wgsl",
            "mesh_shading.wgsl",
            "decal.wgsl",
        ],
    ),
    (
        "decal_receiver.wgsl",
        &[
            "camera.wgsl",
            "skin.wgsl",
            "bayer.wgsl",
            "mesh_material.wgsl",
        ],
    ),
    ("dof.wgsl", &["fullscreen_vs.wgsl"]),
    (
        "dof_coc.wgsl",
        &["fullscreen_vs.wgsl", "hdr.wgsl", "world_from_depth.wgsl"],
    ),
    (
        "ground_marker.wgsl",
        &["camera.wgsl", "rounded_box.wgsl", "quad.wgsl"],
    ),
    ("hiz_reduce.wgsl", &["fullscreen_vs.wgsl", "footprint.wgsl"]),
    ("hiz_seed.wgsl", &["fullscreen_vs.wgsl", "footprint.wgsl"]),
    (
        "local_shadow_debug.wgsl",
        &["fullscreen_vs.wgsl", "srgb.wgsl"],
    ),
    (
        "particle.wgsl",
        &[
            "camera.wgsl",
            "transparent_layer.wgsl",
            "cluster_grid.wgsl",
            "lights.wgsl",
            "billboard_light.wgsl",
            "quad.wgsl",
            "soft_falloff.wgsl",
        ],
    ),
    (
        "prepass.wgsl",
        &[
            "camera.wgsl",
            "skin.wgsl",
            "bayer.wgsl",
            "mesh_material.wgsl",
        ],
    ),
    (
        "rain.wgsl",
        &[
            "camera.wgsl",
            "transparent_layer.wgsl",
            "cluster_grid.wgsl",
            "lights.wgsl",
            "billboard_light.wgsl",
            "quad.wgsl",
            "rain_occlusion.wgsl",
        ],
    ),
    (
        "reflections.wgsl",
        &[
            "fullscreen_vs.wgsl",
            "view_ray.wgsl",
            "hdr.wgsl",
            "lobe.wgsl",
            "dfg.wgsl",
            "specular_ibl.wgsl",
            "cluster_grid.wgsl",
            "probes.wgsl",
            "gi_probe.wgsl",
        ],
    ),
    (
        "probe_debug.wgsl",
        &[
            "fullscreen_vs.wgsl",
            "cluster_grid.wgsl",
            "probes.wgsl",
            "srgb.wgsl",
        ],
    ),
    ("probe_face.wgsl", &["fullscreen_vs.wgsl", "hdr.wgsl"]),
    (
        "projector.wgsl",
        &[
            "rounded_box.wgsl",
            "decal.wgsl",
            "soft_falloff.wgsl",
            "world_from_depth.wgsl",
        ],
    ),
    (
        "resolve.wgsl",
        &[
            "fullscreen_vs.wgsl",
            "bayer.wgsl",
            "srgb.wgsl",
            "output_encode.wgsl",
        ],
    ),
    (
        "shafts.wgsl",
        &["fullscreen_vs.wgsl", "bayer.wgsl", "medium.wgsl"],
    ),
    ("shadow.wgsl", &["skin.wgsl"]),
    ("shadow_cutout.wgsl", &["skin.wgsl", "mesh_material.wgsl"]),
    (
        "silhouette_mask.wgsl",
        &["camera.wgsl", "skin.wgsl", "mesh_material.wgsl"],
    ),
    ("sky.wgsl", &["view_ray.wgsl", "sky_gradient.wgsl"]),
    (
        "smaa_blend.wgsl",
        &[
            "fullscreen_vs.wgsl",
            "bayer.wgsl",
            "srgb.wgsl",
            "output_encode.wgsl",
        ],
    ),
    ("smaa_edges.wgsl", &["fullscreen_vs.wgsl", "srgb.wgsl"]),
    ("smaa_weights.wgsl", &["fullscreen_vs.wgsl"]),
    (
        "ssao.wgsl",
        &[
            "fullscreen_vs.wgsl",
            "bayer.wgsl",
            "math.wgsl",
            "world_from_depth.wgsl",
        ],
    ),
    (
        "ssr_blend.wgsl",
        &[
            "fullscreen_vs.wgsl",
            "view_ray.wgsl",
            "hdr.wgsl",
            "dfg.wgsl",
            "ssr.wgsl",
        ],
    ),
    (
        "ssr_color.wgsl",
        &["fullscreen_vs.wgsl", "hdr.wgsl", "footprint.wgsl"],
    ),
    ("ssr_debug.wgsl", &["fullscreen_vs.wgsl", "srgb.wgsl"]),
    (
        "ssr_trace.wgsl",
        &[
            "fullscreen_vs.wgsl",
            "view_ray.wgsl",
            "hdr.wgsl",
            "lobe.wgsl",
            "ssr.wgsl",
        ],
    ),
    ("threshold.wgsl", &["fullscreen_vs.wgsl", "hdr.wgsl"]),
    (
        "upscale.wgsl",
        &[
            "fullscreen_vs.wgsl",
            "bayer.wgsl",
            "srgb.wgsl",
            "output_encode.wgsl",
        ],
    ),
];

/// The ordered snippet file names composed ahead of shader `file_name`, or an empty slice if it
/// composes none. Looks the name up in [`COMPOSITIONS`].
pub fn snippets_for(file_name: &str) -> &'static [&'static str] {
    COMPOSITIONS
        .iter()
        .find(|(name, _)| *name == file_name)
        .map(|(_, snippets)| *snippets)
        .unwrap_or(&[])
}

pub use emit::emit;
pub use error::ReflectError;
pub use model::{
    BindGroupLayout, BindingKind, BindingResource, BufferSpace, EntryPoint, FieldLayout,
    Reflection, ScalarConst, ScalarKind, ShaderStage, StorageAccess, StorageFormat, StructLayout,
    TextureDim, TextureSample, TypeRef, VertexFormat, VertexInput,
};
pub use reflect::{ShaderUnit, reflect};

/// Read every shader in [`RENDERER_SHADERS`] and [`RENDER_GRAPH_SHADERS`] from the checkout at
/// `workspace` (the workspace root) as [`ShaderUnit`]s, each composed with its snippets exactly as the
/// renderer builds it (decision 16).
pub fn load_units(workspace: &Path) -> std::io::Result<Vec<ShaderUnit>> {
    let renderer_dir = workspace.join("crates/ludion-renderer/src/shaders");
    let mut units = Vec::new();
    for &name in RENDERER_SHADERS {
        // The preamble is the ordered snippet sources joined by `\n`; `with_preamble` then prepends it
        // (+ `\n`) to the body.
        let snippet_names = snippets_for(name);
        let body = read_source(&renderer_dir.join(name))?;
        if snippet_names.is_empty() {
            units.push(ShaderUnit::new(name, body));
        } else {
            let mut parts = Vec::with_capacity(snippet_names.len());
            for &snippet in snippet_names {
                parts.push(read_source(&renderer_dir.join("snippets").join(snippet))?);
            }
            units.push(ShaderUnit::with_preamble(name, parts.join("\n"), body));
        }
    }
    for &name in RENDER_GRAPH_SHADERS {
        let path = workspace
            .join("crates/ludion-render-graph/src/shaders")
            .join(name);
        units.push(ShaderUnit::new(name, read_source(&path)?));
    }
    Ok(units)
}

fn read_source(path: &Path) -> std::io::Result<String> {
    std::fs::read_to_string(path)
        .map_err(|e| std::io::Error::new(e.kind(), format!("{}: {e}", path.display())))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reflect_one(source: &str) -> Reflection {
        reflect(&[ShaderUnit::new("test.wgsl", source)]).expect("reflection should succeed")
    }

    #[test]
    fn atomic_free_units_reject_atomics_through_arrays_and_structs() {
        let array = reflect_one(
            r#"
            @group(0) @binding(0) var<storage, read_write> counters: array<atomic<u32>>;
            @compute @workgroup_size(1) fn cs() { atomicAdd(&counters[0], 1u); }
            "#,
        );
        let in_struct = reflect_one(
            r#"
            struct C { n: atomic<u32>, pad: u32 };
            @group(0) @binding(0) var<storage, read_write> counter: C;
            @compute @workgroup_size(1) fn cs() { atomicAdd(&counter.n, 1u); }
            "#,
        );
        let plain = reflect_one(
            r#"
            @group(0) @binding(0) var<storage, read_write> counters: array<u32>;
            @compute @workgroup_size(1) fn cs() { counters[0] = 1u; }
            "#,
        );
        for (r, binding) in [(&array, "counters"), (&in_struct, "counter")] {
            assert_eq!(
                check_atomic_free_units(r, &["test.wgsl"]),
                Err(ReflectError::AtomicInAtomicFreeUnit {
                    unit: "test.wgsl".into(),
                    binding: binding.into()
                })
            );
            assert_eq!(check_atomic_free_units(r, &["cull.wgsl"]), Ok(()));
        }
        assert_eq!(check_atomic_free_units(&plain, &["test.wgsl"]), Ok(()));
        let message = check_atomic_free_units(&array, &["test.wgsl"])
            .unwrap_err()
            .to_string();
        assert!(message.contains("stable"), "{message}");
    }

    #[test]
    fn three_row_matrices_and_vec3_arrays_spell_four_lanes() {
        let r = reflect_one(
            r#"
            struct M { m: mat3x3<f32>, v: array<vec3<f32>, 2> };
            @group(0) @binding(0) var<uniform> u: M;
            @fragment fn fs() -> @location(0) vec4<f32> {
                return vec4<f32>(u.m[0] + u.v[1], 1.0);
            }
            "#,
        );
        let s = r.struct_named("M").unwrap();
        assert_eq!(s.fields[0].size, 48);
        assert_eq!(s.fields[1].size, 32);
        let out = emit(&r);
        assert!(out.contains("pub m: [[f32; 4]; 3],"), "{out}");
        assert!(out.contains("pub v: [[f32; 4]; 2],"), "{out}");
    }

    #[test]
    fn unfilterable_names_must_be_reflected_float_textures() {
        let r = reflect_one(
            r#"
            @group(0) @binding(0) var hiz: texture_2d<f32>;
            @group(0) @binding(1) var depth: texture_depth_2d;
            struct B { x: vec4<f32> };
            @group(0) @binding(2) var<uniform> buf: B;
            @fragment fn fs() -> @location(0) vec4<f32> {
                return textureLoad(hiz, vec2<i32>(0), 0) + buf.x
                    + vec4<f32>(textureLoad(depth, vec2<i32>(0), 0));
            }
            "#,
        );
        assert_eq!(check_unfilterable_names(&r, &["hiz"]), Ok(()));
        for absent in ["gone", "depth", "buf"] {
            let err = check_unfilterable_names(&r, &["hiz", absent]).unwrap_err();
            assert_eq!(
                err,
                ReflectError::UnfilterableNotFound {
                    name: absent.into()
                }
            );
            assert!(err.to_string().contains(&format!("`{absent}`")), "{err}");
        }
    }

    #[test]
    fn trailing_scalar_gets_implicit_pad() {
        // `count: u32` at 0 then `tail: vec4<f32>` (align 16) at 16 leaves a 12-byte gap, and the
        // struct rounds up to its 16-byte alignment: size 32, not the packed 20.
        let r = reflect_one(
            r#"
            struct S { count: u32, tail: vec4<f32> };
            @group(0) @binding(0) var<uniform> s: S;
            @fragment fn fs() -> @location(0) vec4<f32> { return s.tail * f32(s.count); }
            "#,
        );
        let s = r.struct_named("S").unwrap();
        assert_eq!(s.size, 32);
        assert_eq!(s.alignment, 16);
        assert_eq!(s.fields[0].offset, 0);
        assert_eq!(s.fields[0].size, 4);
        assert_eq!(s.fields[1].offset, 16); // padded past the u32
        assert_eq!(s.fields[1].size, 16);
    }

    #[test]
    fn array_stride_and_count() {
        // In uniform address space an `array<vec3<f32>, 4>` element rounds up to 16-byte stride, so
        // stride is 16 (not the 12-byte packed vec3) and the count is exposed.
        let r = reflect_one(
            r#"
            struct S { items: array<vec4<f32>, 4> };
            @group(0) @binding(0) var<uniform> s: S;
            @fragment fn fs() -> @location(0) vec4<f32> { return s.items[0]; }
            "#,
        );
        let s = r.struct_named("S").unwrap();
        let TypeRef::Array {
            count,
            stride,
            element,
        } = &s.fields[0].ty
        else {
            panic!("expected an array field, got {:?}", s.fields[0].ty);
        };
        assert_eq!(*count, Some(4));
        assert_eq!(*stride, 16);
        assert_eq!(
            **element,
            TypeRef::Vector {
                components: 4,
                scalar: ScalarKind::F32
            }
        );
        assert_eq!(s.size, 64);
    }

    #[test]
    fn a_listed_texture_is_unfilterable() {
        let r = reflect_one(
            r#"
            @group(0) @binding(0) var hiz_src: texture_2d<f32>;
            @group(0) @binding(1) var other_src: texture_2d<f32>;
            @fragment fn fs() -> @location(0) vec4<f32> {
                return textureLoad(hiz_src, vec2<i32>(0), 0) + textureLoad(other_src, vec2<i32>(0), 0);
            }
            "#,
        );
        let sample = |name: &str| match &r.binding_named(name).unwrap().kind {
            BindingKind::Texture { sample, .. } => *sample,
            other => panic!("{other:?}"),
        };
        assert_eq!(
            sample("hiz_src"),
            TextureSample::Float { filterable: false }
        );
        assert_eq!(
            sample("other_src"),
            TextureSample::Float { filterable: true }
        );
        assert!(emit(&r).contains("filterable: false"));
    }

    #[test]
    fn nested_struct_layout() {
        let r = reflect_one(
            r#"
            struct Inner { a: vec4<f32>, b: vec4<f32> };
            struct Outer { head: vec4<f32>, inner: Inner };
            @group(0) @binding(0) var<uniform> o: Outer;
            @fragment fn fs() -> @location(0) vec4<f32> { return o.head + o.inner.a; }
            "#,
        );
        // Both nested and outer struct are reflected (both reachable from the uniform).
        let inner = r.struct_named("Inner").unwrap();
        assert_eq!(inner.size, 32);
        let outer = r.struct_named("Outer").unwrap();
        assert_eq!(outer.fields[1].name, "inner");
        assert_eq!(outer.fields[1].offset, 16);
        assert_eq!(outer.fields[1].ty, TypeRef::Struct("Inner".into()));
        assert_eq!(outer.size, 48);
    }

    #[test]
    fn texture_and_sampler_bindings() {
        let r = reflect_one(
            r#"
            @group(0) @binding(0) var tex: texture_2d<f32>;
            @group(0) @binding(1) var samp: sampler;
            @vertex fn vs() -> @builtin(position) vec4<f32> { return vec4<f32>(0.0); }
            @fragment fn fs() -> @location(0) vec4<f32> { return textureSample(tex, samp, vec2<f32>(0.0)); }
            "#,
        );
        let tex = r.binding_named("tex").unwrap();
        assert_eq!((tex.group, tex.binding), (0, 0));
        assert_eq!(
            tex.kind,
            BindingKind::Texture {
                dim: TextureDim::D2,
                sample: TextureSample::Float { filterable: true },
                multisampled: false,
            }
        );
        // Only the fragment stage samples the texture.
        assert_eq!(tex.visibility, vec![ShaderStage::Fragment]);

        let samp = r.binding_named("samp").unwrap();
        assert_eq!(samp.kind, BindingKind::Sampler { comparison: false });
        assert_eq!(samp.visibility, vec![ShaderStage::Fragment]);
    }

    #[test]
    fn uniform_buffer_binding_carries_size() {
        let r = reflect_one(
            r#"
            struct Cam { view_proj: mat4x4<f32>, cam_pos: vec4<f32> };
            @group(0) @binding(0) var<uniform> cam: Cam;
            @vertex fn vs() -> @builtin(position) vec4<f32> { return cam.view_proj[0]; }
            "#,
        );
        let cam = r.binding_named("cam").unwrap();
        assert_eq!(
            cam.kind,
            BindingKind::Buffer {
                address_space: BufferSpace::Uniform,
                min_size: 80,
                atomic: false,
            }
        );
        assert_eq!(cam.visibility, vec![ShaderStage::Vertex]);
    }

    #[test]
    fn storage_array_of_structs_binds_one_element_stride() {
        let r = reflect_one(
            r#"
            struct Light { pos: vec4<f32>, color: vec4<f32> };
            @group(1) @binding(0) var<storage, read> light_list: array<Light>;
            @fragment fn fs() -> @location(0) vec4<f32> { return light_list[0].color; }
            "#,
        );
        let b = r.binding_named("light_list").unwrap();
        assert_eq!((b.group, b.binding), (1, 0));
        assert_eq!(
            b.kind,
            BindingKind::Buffer {
                address_space: BufferSpace::Storage { read_only: true },
                // One `Light`, not zero and not the whole buffer.
                min_size: 32,
                atomic: false,
            }
        );
        // The element struct is reachable only through the storage array, and must still be emitted
        // so the CPU side has a POD to fill the buffer with.
        let light = r.struct_named("Light").unwrap();
        assert_eq!(light.size, 32);
        assert_eq!(light.fields[1].offset, 16);
    }

    #[test]
    fn bare_scalar_and_vector_storage_arrays() {
        // The cluster tables' shapes: no struct to reflect, just the element stride.
        let r = reflect_one(
            r#"
            @group(1) @binding(1) var<storage, read> cluster_table: array<vec2<u32>>;
            @group(1) @binding(2) var<storage, read> cluster_lights: array<u32>;
            @fragment fn fs() -> @location(0) vec4<f32> {
                let slice = cluster_table[0];
                return vec4<f32>(f32(cluster_lights[slice.x + slice.y]));
            }
            "#,
        );
        let min_size = |name: &str| match r.binding_named(name).unwrap().kind {
            BindingKind::Buffer { min_size, .. } => min_size,
            ref other => panic!("expected a buffer binding, got {other:?}"),
        };
        assert_eq!(min_size("cluster_table"), 8);
        assert_eq!(min_size("cluster_lights"), 4);
        assert!(r.structs.is_empty(), "bare arrays declare no struct");
    }

    #[test]
    fn storage_visibility_merges_across_entry_points() {
        let r = reflect_one(
            r#"
            @group(1) @binding(0) var<storage, read> light_list: array<vec4<f32>>;
            @group(1) @binding(1) var<storage, read> cluster_lights: array<u32>;
            @vertex fn vs() -> @builtin(position) vec4<f32> { return light_list[0]; }
            @fragment fn fs() -> @location(0) vec4<f32> {
                return light_list[cluster_lights[0]];
            }
            "#,
        );
        assert_eq!(
            r.binding_named("light_list").unwrap().visibility,
            vec![ShaderStage::Vertex, ShaderStage::Fragment],
            "both entry points read it, so the BGL entry must be visible to both"
        );
        assert_eq!(
            r.binding_named("cluster_lights").unwrap().visibility,
            vec![ShaderStage::Fragment]
        );
    }

    #[test]
    fn a_struct_shared_by_a_uniform_and_a_storage_array_unifies_once() {
        // WGSL lays a type out identically in both address spaces — uniform space only *constrains*
        // which types are legal — so this is a plain unification, not a layout conflict.
        let mesh = ShaderUnit::new(
            "mesh.wgsl",
            r#"
            struct Light { pos: vec4<f32>, color: vec4<f32> };
            struct Lights { ambient: vec4<f32>, items: array<Light, 4> };
            @group(1) @binding(0) var<uniform> lights: Lights;
            @fragment fn fs() -> @location(0) vec4<f32> { return lights.items[0].pos; }
            "#,
        );
        let sprite = ShaderUnit::new(
            "sprite.wgsl",
            r#"
            struct Light { pos: vec4<f32>, color: vec4<f32> };
            @group(1) @binding(1) var<storage, read> light_list: array<Light>;
            @fragment fn fs() -> @location(0) vec4<f32> { return light_list[0].color; }
            "#,
        );
        let r = reflect(&[mesh, sprite]).unwrap();
        let lights: Vec<_> = r.structs.iter().filter(|s| s.name == "Light").collect();
        assert_eq!(lights.len(), 1, "one entry, not one per address space");
        assert_eq!(lights[0].units, vec!["mesh.wgsl", "sprite.wgsl"]);
        assert_eq!(lights[0].size, 32);
    }

    #[test]
    fn same_named_storage_buffer_unifies_across_units() {
        let unit = |name: &'static str| {
            ShaderUnit::new(
                name,
                r#"
                struct Light { pos: vec4<f32>, color: vec4<f32> };
                @group(1) @binding(1) var<storage, read> light_list: array<Light>;
                @fragment fn fs() -> @location(0) vec4<f32> { return light_list[0].pos; }
                "#,
            )
        };
        let r = reflect(&[unit("mesh.wgsl"), unit("sprite.wgsl")]).unwrap();
        let b = r.binding_named("light_list").unwrap();
        assert_eq!(b.units, vec!["mesh.wgsl", "sprite.wgsl"]);
        assert_eq!(
            r.bindings.iter().filter(|b| b.name == "light_list").count(),
            1
        );
    }

    #[test]
    fn same_named_storage_buffer_with_a_different_element_is_a_hard_error() {
        let a = ShaderUnit::new(
            "mesh.wgsl",
            r#"
            @group(1) @binding(1) var<storage, read> cluster_table: array<vec2<u32>>;
            @fragment fn fs() -> @location(0) vec4<f32> {
                return vec4<f32>(f32(cluster_table[0].x));
            }
            "#,
        );
        let b = ShaderUnit::new(
            "sprite.wgsl",
            r#"
            @group(1) @binding(1) var<storage, read> cluster_table: array<u32>;
            @fragment fn fs() -> @location(0) vec4<f32> {
                return vec4<f32>(f32(cluster_table[0]));
            }
            "#,
        );
        let err = reflect(&[a, b]).unwrap_err();
        assert!(
            matches!(err, ReflectError::BindingConflict { .. }),
            "expected a BindingConflict, got {err:?}"
        );
        let msg = err.to_string();
        // The element stride is what disagrees, and the message must carry it.
        assert!(msg.contains("cluster_table"), "names the resource: {msg}");
        assert!(msg.contains("min_size: 8"), "names both sizes: {msg}");
        assert!(msg.contains("min_size: 4"), "names both sizes: {msg}");
    }

    #[test]
    fn the_same_name_in_two_address_spaces_is_a_hard_error() {
        let a = ShaderUnit::new(
            "mesh.wgsl",
            r#"
            struct Light { pos: vec4<f32>, color: vec4<f32> };
            @group(1) @binding(0) var<storage, read> light_list: array<Light>;
            @fragment fn fs() -> @location(0) vec4<f32> { return light_list[0].pos; }
            "#,
        );
        let b = ShaderUnit::new(
            "sprite.wgsl",
            r#"
            struct Light { pos: vec4<f32>, color: vec4<f32> };
            @group(1) @binding(0) var<uniform> light_list: Light;
            @fragment fn fs() -> @location(0) vec4<f32> { return light_list.pos; }
            "#,
        );
        let err = reflect(&[a, b]).unwrap_err();
        assert!(
            matches!(err, ReflectError::BindingConflict { .. }),
            "expected a BindingConflict, got {err:?}"
        );
        assert!(
            err.to_string().contains("Uniform"),
            "the message names the address spaces: {err}"
        );
    }

    #[test]
    fn read_write_storage_reflects_as_writable() {
        let r = reflect_one(
            r#"
            @group(0) @binding(0) var<storage, read_write> counters: array<u32>;
            @compute @workgroup_size(1) fn cs() { counters[0] = 1u; }
            "#,
        );
        let counters = r.binding_named("counters").unwrap();
        assert_eq!(
            counters.kind,
            BindingKind::Buffer {
                address_space: BufferSpace::Storage { read_only: false },
                min_size: 4,
                atomic: false,
            }
        );
        assert_eq!(counters.visibility, vec![ShaderStage::Compute]);
    }

    #[test]
    fn compute_shader_reflects_storage_textures_and_workgroup_size() {
        let r = reflect_one(
            r#"
            @group(0) @binding(0) var volume: texture_storage_3d<rgba16float, write>;
            @group(0) @binding(1) var faces: texture_storage_2d_array<rgba32float, write>;
            @group(0) @binding(2) var cube: texture_cube_array<f32>;
            @compute @workgroup_size(4, 2, 1)
            fn cs(@builtin(global_invocation_id) gid: vec3<u32>) {
                textureStore(volume, vec3<i32>(gid), vec4<f32>(1.0));
                textureStore(faces, vec2<i32>(gid.xy), 0, vec4<f32>(1.0));
                _ = textureNumLayers(cube);
            }
            "#,
        );
        assert_eq!(
            r.binding_named("volume").unwrap().kind,
            BindingKind::StorageTexture {
                dim: TextureDim::D3,
                format: StorageFormat::Rgba16Float,
                access: StorageAccess::WriteOnly,
            }
        );
        assert_eq!(
            r.binding_named("faces").unwrap().kind,
            BindingKind::StorageTexture {
                dim: TextureDim::D2Array,
                format: StorageFormat::Rgba32Float,
                access: StorageAccess::WriteOnly,
            }
        );
        assert_eq!(
            r.binding_named("cube").unwrap().kind,
            BindingKind::Texture {
                dim: TextureDim::CubeArray,
                sample: TextureSample::Float { filterable: true },
                multisampled: false,
            }
        );
        let cs = r.entry_points.iter().find(|e| e.name == "cs").unwrap();
        assert_eq!(cs.stage, ShaderStage::Compute);
        assert_eq!(cs.workgroup_size, [4, 2, 1]);
    }

    #[test]
    fn an_unmapped_storage_format_is_a_named_error() {
        let err = reflect(&[ShaderUnit::new(
            "t.wgsl",
            r#"
            @group(0) @binding(0) var t: texture_storage_2d<rgba8snorm, write>;
            @compute @workgroup_size(1) fn cs() { textureStore(t, vec2<i32>(0), vec4<f32>(0.0)); }
            "#,
        )])
        .unwrap_err();
        assert!(err.to_string().contains("Rgba8Snorm"), "{err}");
    }

    #[test]
    fn padding_gaps_become_explicit_byte_fields() {
        // `count: u32` then a 16-aligned `tail` leaves a 12-byte gap, and the struct rounds up.
        let r = reflect_one(
            r#"
            struct S { count: u32, tail: vec4<f32> };
            @group(0) @binding(0) var<uniform> s: S;
            @fragment fn fs() -> @location(0) vec4<f32> { return s.tail * f32(s.count); }
            "#,
        );
        let out = emit(&r);
        assert!(
            out.contains(
                "    pub count: u32,
    pub _pad0: [u8; 12],
    pub tail: [f32; 4],
"
            ),
            "{out}"
        );
        assert!(
            !out.contains("_pad1"),
            "no trailing gap here:
{out}"
        );

        // A trailing gap: `vec3` (12 bytes, align 16) at the end of a struct rounds it up to 16.
        let r = reflect_one(
            r#"
            struct T { v: vec3<f32> };
            @group(0) @binding(0) var<uniform> t: T;
            @fragment fn fs() -> @location(0) vec4<f32> { return vec4<f32>(t.v, 1.0); }
            "#,
        );
        let out = emit(&r);
        assert!(
            out.contains(
                "    pub v: [f32; 3],
    pub _pad0: [u8; 4],
}"
            ),
            "{out}"
        );
        assert!(
            out.contains("size_of::<T>() == 16"),
            "the size assert is unchanged:
{out}"
        );
    }

    #[test]
    fn a_compute_only_group_emits_no_bgl_but_keeps_its_consts() {
        let r = reflect_one(
            r#"
            struct P { n: u32 };
            @group(0) @binding(0) var<uniform> p: P;
            @group(0) @binding(1) var out_tex: texture_storage_2d<r32float, write>;
            @compute @workgroup_size(8, 8, 1)
            fn cs(@builtin(global_invocation_id) gid: vec3<u32>) {
                textureStore(out_tex, vec2<i32>(gid.xy), vec4<f32>(f32(p.n)));
            }
            "#,
        );
        let out = emit(&r);
        assert!(
            !out.contains("_bgl"),
            "compute pipelines keep auto layouts:
{out}"
        );
        assert!(out.contains("pub const P_BINDING: u32 = 0;"), "{out}");
        assert!(out.contains("pub const OUT_TEX_BINDING: u32 = 1;"), "{out}");
        assert!(
            out.contains("pub const TEST_CS_WORKGROUP_SIZE: [u32; 3] = [8, 8, 1];"),
            "{out}"
        );
    }

    #[test]
    fn a_compute_only_buffer_group_emits_a_bgl() {
        let r = reflect_one(
            r#"
            struct P { n: u32 };
            @group(0) @binding(0) var<uniform> p: P;
            @group(0) @binding(1) var<storage, read> src: array<vec4<f32>>;
            @group(0) @binding(2) var<storage, read_write> dst: array<vec4<f32>>;
            @compute @workgroup_size(64)
            fn cs(@builtin(global_invocation_id) gid: vec3<u32>) {
                if (gid.x < p.n) { dst[gid.x] = src[gid.x]; }
            }
            "#,
        );
        let out = emit(&r);
        assert!(out.contains("_bgl"), "{out}");
        assert!(out.contains("Storage { read_only: false }"), "{out}");
        assert!(out.contains("Storage { read_only: true }"), "{out}");
        assert!(out.contains("ShaderStages::COMPUTE"), "{out}");
        assert!(out.contains("min_binding_size"), "{out}");
        assert!(
            out.contains("min_binding_size: wgpu::BufferSize::new(16)"),
            "{out}"
        );
    }

    #[test]
    fn a_runtime_sized_struct_member_is_rejected() {
        let err = reflect(&[ShaderUnit::new(
            "t.wgsl",
            r#"
            struct Light { pos: vec4<f32>, color: vec4<f32> };
            struct Lights { ambient: vec4<f32>, items: array<Light> };
            @group(1) @binding(0) var<storage, read> lights: Lights;
            @fragment fn fs() -> @location(0) vec4<f32> {
                return lights.ambient + lights.items[0].pos;
            }
            "#,
        )])
        .unwrap_err();
        assert!(
            matches!(err, ReflectError::RuntimeSizedMember { .. }),
            "expected a RuntimeSizedMember error, got {err:?}"
        );
        let msg = err.to_string();
        assert!(msg.contains("Lights") && msg.contains("items"), "{msg}");
    }

    #[test]
    fn storage_bindings_emit_index_consts_and_a_bgl_entry() {
        let r = reflect_one(
            r#"
            struct Light { pos: vec4<f32>, color: vec4<f32> };
            struct ClusterHeader { ambient: vec4<f32>, count: u32 };
            @group(1) @binding(0) var<uniform> cluster_header: ClusterHeader;
            @group(1) @binding(1) var<storage, read> light_list: array<Light>;
            @fragment fn fs() -> @location(0) vec4<f32> {
                return cluster_header.ambient + light_list[cluster_header.count].color;
            }
            "#,
        );
        let out = emit(&r);
        assert!(
            out.contains("pub const LIGHT_LIST_GROUP: u32 = 1;"),
            "{out}"
        );
        assert!(
            out.contains("pub const LIGHT_LIST_BINDING: u32 = 1;"),
            "{out}"
        );
        assert!(
            out.contains(
                "                ty: wgpu::BindingType::Buffer {\n\
                 \x20                   ty: wgpu::BufferBindingType::Storage { read_only: true },\n\
                 \x20                   has_dynamic_offset: false,\n\
                 \x20                   min_binding_size: wgpu::BufferSize::new(32),\n"
            ),
            "the storage entry pins one element's stride:\n{out}"
        );
        // The element struct rides along as a POD with its layout baked in.
        assert!(out.contains("pub struct Light {"), "{out}");
        assert!(
            out.contains("const _: () = assert!(core::mem::size_of::<Light>() == 32);"),
            "{out}"
        );
    }

    #[test]
    fn vertex_inputs_locations_and_formats() {
        let r = reflect_one(
            r#"
            struct VsIn {
                @location(0) position: vec3<f32>,
                @location(1) uv: vec2<f32>,
                @location(2) color: vec4<f32>,
            };
            @vertex fn vs_main(in: VsIn, @builtin(vertex_index) vi: u32) -> @builtin(position) vec4<f32> {
                return vec4<f32>(in.position, 1.0) + vec4<f32>(in.uv, 0.0, 0.0) + in.color * f32(vi);
            }
            "#,
        );
        let vs = r
            .entry_points
            .iter()
            .find(|e| e.stage == ShaderStage::Vertex)
            .unwrap();
        assert_eq!(vs.inputs.len(), 3); // the builtin vertex_index is excluded
        assert_eq!(vs.inputs[0].location, 0);
        assert_eq!(vs.inputs[0].name, "position");
        assert_eq!(vs.inputs[0].format, VertexFormat::Float32x3);
        assert_eq!(vs.inputs[1].format, VertexFormat::Float32x2);
        assert_eq!(vs.inputs[2].format, VertexFormat::Float32x4);
    }

    #[test]
    fn preamble_composition_supplies_a_const() {
        // The array bound `N` exists only in the preamble — reflection must see the composed string.
        let unit = ShaderUnit::with_preamble(
            "u.wgsl",
            "const N: u32 = 3u;",
            r#"
            struct Light { pos: vec4<f32> };
            struct Lights { items: array<Light, N> };
            @group(0) @binding(0) var<uniform> lights: Lights;
            @fragment fn fs() -> @location(0) vec4<f32> { return lights.items[0].pos; }
            "#,
        );
        let r = reflect(&[unit]).unwrap();
        assert_eq!(r.const_value("N"), Some(3));
        let lights = r.struct_named("Lights").unwrap();
        let TypeRef::Array { count, .. } = &lights.fields[0].ty else {
            panic!("expected array");
        };
        assert_eq!(*count, Some(3));
    }

    #[test]
    fn same_name_different_layout_is_a_hard_error() {
        // `Lights` differs only in a pad field, pushing `ambient` from offset 16 to 32.
        let a = ShaderUnit::new(
            "mesh.wgsl",
            r#"
            struct Lights { count: u32, ambient: vec4<f32> };
            @group(0) @binding(0) var<uniform> l: Lights;
            @fragment fn fs() -> @location(0) vec4<f32> { return l.ambient * f32(l.count); }
            "#,
        );
        let b = ShaderUnit::new(
            "sprite.wgsl",
            r#"
            struct Lights { count: u32, pad0: u32, pad1: u32, pad2: u32, ambient: vec4<f32> };
            @group(0) @binding(0) var<uniform> l: Lights;
            @fragment fn fs() -> @location(0) vec4<f32> { return l.ambient * f32(l.count); }
            "#,
        );
        let err = reflect(&[a, b]).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("mesh.wgsl"), "message names first unit: {msg}");
        assert!(
            msg.contains("sprite.wgsl"),
            "message names second unit: {msg}"
        );
        // First unit has `ambient` at 16 (field 1); second has it at 32 (field 4) — but they differ
        // in field count first, so that is what the message reports.
        assert!(msg.contains("Lights"), "message names the struct: {msg}");
        assert!(
            matches!(err, ReflectError::StructConflict { .. }),
            "expected a StructConflict, got {err:?}"
        );
    }

    #[test]
    fn same_name_different_offset_names_the_field() {
        // Same field *count*, but a differently-sized head shifts `ambient`'s offset — exercises the
        // per-field offset branch and the plan's canonical message shape.
        let a = ShaderUnit::new(
            "mesh.wgsl",
            r#"
            struct L { head: u32, ambient: vec4<f32> };
            @group(0) @binding(0) var<uniform> l: L;
            @fragment fn fs() -> @location(0) vec4<f32> { return l.ambient * f32(l.head); }
            "#,
        );
        let b = ShaderUnit::new(
            "sprite.wgsl",
            r#"
            struct L { head: vec4<f32>, ambient: vec4<f32> };
            @group(0) @binding(0) var<uniform> l: L;
            @fragment fn fs() -> @location(0) vec4<f32> { return l.ambient + l.head; }
            "#,
        );
        let err = reflect(&[a, b]).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("field `ambient` at offset 16 vs 16") || msg.contains("field `head`"),
            "unexpected message: {msg}"
        );
    }

    #[test]
    fn shared_group_slot_with_different_resources_is_not_a_conflict() {
        // `lights` and `atlas_tex` both sit at @group(1) @binding(0) in different units — a real
        // pattern in the repo (mesh's lights vs text's atlas). Keyed by name, this must NOT error.
        let mesh = ShaderUnit::new(
            "mesh.wgsl",
            r#"
            struct Lights { ambient: vec4<f32> };
            @group(1) @binding(0) var<uniform> lights: Lights;
            @fragment fn fs() -> @location(0) vec4<f32> { return lights.ambient; }
            "#,
        );
        let text = ShaderUnit::new(
            "text.wgsl",
            r#"
            @group(1) @binding(0) var atlas_tex: texture_2d<f32>;
            @group(1) @binding(1) var atlas_sampler: sampler;
            @fragment fn fs() -> @location(0) vec4<f32> {
                return textureSample(atlas_tex, atlas_sampler, vec2<f32>(0.0));
            }
            "#,
        );
        let r = reflect(&[mesh, text]).unwrap();
        assert!(r.binding_named("lights").is_some());
        assert!(r.binding_named("atlas_tex").is_some());
    }
}
