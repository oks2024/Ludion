use std::sync::Arc;

use ludion_core::math::glam::{Mat3, Mat4};

use super::*;
use crate::scene::{MeshInstance, RenderScene};

mod blended;
mod gpu_cull;
mod lit_additive;
mod medium_scatter;
mod mesh_medium;
mod rain;
mod rim;
mod shaft_layer;
mod silhouette;
mod skin_bounds;
mod static_batch;
mod transparent_dof;
mod transparent_medium;
mod viewmodel;
mod views;

/// The decision-16 mapping-drift guard: the string this crate composes and compiles for every
/// shader (its snippets + body, from `include_str!`) must be byte-identical to what the bindgen
/// registry (`ludion_shader_bindgen::COMPOSITIONS`) composes by reading the same files from disk.
/// Because the on-disk `.wgsl` bytes *are* the `include_str!` sources, this proves the two snippet
/// mappings agree — reorder or drop a snippet on either side and the composed strings diverge here,
/// so a drift fails loudly (and `shader_gen.rs` is generated from exactly what the GPU sees).
#[test]
fn renderer_composition_matches_bindgen_registry() {
    for id in ShaderId::all() {
        let renderer_side = builtin_composed(id);
        let bindgen_side = crate::compute_shader::registry_composed(id.file_name());

        assert_eq!(
            renderer_side,
            bindgen_side,
            "composition drift for {}: the renderer's `ShaderId::snippets` and the bindgen \
             `COMPOSITIONS` registry disagree — reconcile them so shader_gen.rs is generated from \
             what the GPU compiles",
            id.file_name()
        );
    }
}

/// The bindgen shader list is the renderer's whole shader set: every `ShaderId` file name is
/// unique, none is a compute shader (those are built outside the registry with auto-derived
/// layouts), and together the two equal `RENDERER_SHADERS` — a shader added to one side and not
/// the other fails here instead of silently staying out of `shader_gen.rs`.
#[test]
fn shader_ids_cover_the_bindgen_list() {
    let mut ids: Vec<&str> = ShaderId::all().map(ShaderId::file_name).collect();
    ids.sort_unstable();
    assert!(
        ids.windows(2).all(|pair| pair[0] != pair[1]),
        "`ShaderId` file names must be unique: {ids:?}"
    );
    for compute in ludion_shader_bindgen::COMPUTE_SHADERS {
        assert!(
            !ids.contains(compute),
            "{compute} is a compute shader and must not be a `ShaderId`"
        );
    }
    let mut all: Vec<&str> = ids
        .into_iter()
        .chain(ludion_shader_bindgen::COMPUTE_SHADERS.iter().copied())
        .collect();
    all.sort_unstable();
    assert_eq!(all, ludion_shader_bindgen::RENDERER_SHADERS);
}

/// The shader registry is indexed by its id: row `i` describes the [`ShaderId`] whose
/// discriminant is `i`. [`ShaderId::desc`] and `Renderer::pipeline` index by that discriminant,
/// so a row written out of order would silently hand a shader another's body or another's
/// pipeline. The layout registry asserts the same invariant where it is materialized
/// ([`Layouts::new`]). No GPU needed.
#[test]
fn shader_registry_rows_are_indexed_by_their_id() {
    for (slot, desc) in SHADERS.iter().enumerate() {
        assert_eq!(
            desc.id as usize, slot,
            "SHADERS row {slot} is {:?}",
            desc.id
        );
    }
}

/// [`FrameRecorder::fullscreen_effect`] derives its bindings from the pass's input count —
/// textures at `0..N`, the sampler at `N`, the uniform at `N + 1`. This pins that derivation
/// against the generated binding consts of every fullscreen layout, so a WGSL edit that renumbers
/// or reorders a binding fails here instead of silently mis-binding a frame.
#[test]
fn fullscreen_layouts_follow_the_binding_convention() {
    // One input texture: the bright pass and each pyramid downsample.
    assert_eq!(
        [
            shader_gen::SRC_TEX_BINDING,
            shader_gen::SRC_SAMPLER_BINDING,
            shader_gen::BLOOM_BINDING,
        ],
        [0, 1, 2],
    );
    assert_eq!(
        [
            shader_gen::DOWN_SRC_BINDING,
            shader_gen::DOWN_SAMP_BINDING,
            shader_gen::DOWN_PARAMS_BINDING,
        ],
        [0, 1, 2],
    );
    // Ten input textures: composite mixes scene + bloom, loads the scene depth for fog, samples
    // the blurred shaft term, blends the depth-of-field gather, loads the transparent layer, adds the
    // local-light term, loads the mesh medium mask and the silhouette mask with its depth —
    // the first three in the graph pass's declared read order, and the last seven the arc's
    // *optional* slots, whose bindings are fixed whether or not the scene enables them (see
    // `optional_inputs_hold_their_binding`).
    assert_eq!(
        [
            shader_gen::SCENE_TEX_BINDING,
            shader_gen::BLOOM_TEX_BINDING,
            shader_gen::SCENE_DEPTH_TEX_BINDING,
            shader_gen::SHAFTS_TEX_BINDING,
            shader_gen::DOF_TEX_BINDING,
            shader_gen::TRANSPARENT_DEPTH_TEX_BINDING,
            shader_gen::MEDIUM_SCATTER_TEX_BINDING,
            shader_gen::SCENE_MEDIUM_TEX_BINDING,
            shader_gen::SILHOUETTE_MASK_TEX_BINDING,
            shader_gen::SILHOUETTE_DEPTH_TEX_BINDING,
            shader_gen::SAMP_BINDING,
            shader_gen::COMPOSITE_BINDING,
        ],
        [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11],
    );
    // Three input textures: the depth-of-field prefilter samples the scene, loads its depth, in
    // the graph pass's declared read order, and loads the transparent layer in the *optional* slot
    // behind them. One input: the gather, over that one target.
    assert_eq!(
        [
            shader_gen::COC_SCENE_BINDING,
            shader_gen::COC_DEPTH_BINDING,
            shader_gen::COC_LAYER_BINDING,
            shader_gen::COC_SAMP_BINDING,
            shader_gen::COC_PARAMS_BINDING,
        ],
        [0, 1, 2, 3, 4],
    );
    assert_eq!(
        [
            shader_gen::DOF_SRC_BINDING,
            shader_gen::DOF_SAMP_BINDING,
            shader_gen::DOF_PARAMS_BINDING,
        ],
        [0, 1, 2],
    );
    // Three input textures: the shaft march loads the scene depth and the sun's shadow map, in the
    // graph pass's declared read order, and the transparent layer in the *optional* slot behind
    // them, and declares a sampler it never calls (like `resolve`).
    assert_eq!(
        [
            shader_gen::SHAFTS_DEPTH_BINDING,
            shader_gen::SHAFTS_SHADOW_BINDING,
            shader_gen::SHAFTS_LAYER_BINDING,
            shader_gen::SHAFTS_SAMP_BINDING,
            shader_gen::SHAFTS_BINDING,
        ],
        [0, 1, 2, 3, 4],
    );
    // One input texture: the shared 4×4 box blur, whichever pass names it.
    assert_eq!(
        [
            shader_gen::BOX_SRC_BINDING,
            shader_gen::BOX_SAMP_BINDING,
            shader_gen::BOX_PARAMS_BINDING,
        ],
        [0, 1, 2],
    );
    // Two input textures: a pyramid upsample composites the coarser accumulation and its own
    // level, in the graph pass's declared read order.
    assert_eq!(
        [
            shader_gen::UP_COARSE_BINDING,
            shader_gen::UP_FINE_BINDING,
            shader_gen::UP_SAMP_BINDING,
            shader_gen::UP_PARAMS_BINDING,
        ],
        [0, 1, 2, 3],
    );
    // The resolve declares a sampler it never calls (it fetches exact texels): the convention is
    // the helper's binding derivation, so the slot must exist even when the shader ignores it.
    assert_eq!(
        [
            shader_gen::RESOLVE_SRC_BINDING,
            shader_gen::RESOLVE_SAMP_BINDING,
            shader_gen::RESOLVE_PARAMS_BINDING,
        ],
        [0, 1, 2],
    );
    // The magnifier declares one too, for the same reason and with the same excuse.
    assert_eq!(
        [
            shader_gen::UPSCALE_SRC_BINDING,
            shader_gen::UPSCALE_SAMP_BINDING,
            shader_gen::UPSCALE_PARAMS_BINDING,
        ],
        [0, 1, 2],
    );
    // SMAA: the edge pass reads the composite (its sampler is never called), the weight pass the
    // edges plus the two tables it binds itself at fixed slots, the blend pass the composite and
    // the weights.
    assert_eq!(
        [
            shader_gen::SMAA_EDGES_SRC_BINDING,
            shader_gen::SMAA_EDGES_SAMP_BINDING,
            shader_gen::SMAA_EDGES_PARAMS_BINDING,
        ],
        [0, 1, 2],
    );
    assert_eq!(
        [
            shader_gen::SMAA_WEIGHTS_EDGES_BINDING,
            shader_gen::SMAA_AREA_TEX_BINDING,
            shader_gen::SMAA_SEARCH_TEX_BINDING,
            shader_gen::SMAA_WEIGHTS_SAMP_BINDING,
            shader_gen::SMAA_WEIGHTS_PARAMS_BINDING,
        ],
        [0, 1, 2, 3, 4],
    );
    assert_eq!(
        [
            shader_gen::SMAA_BLEND_SRC_BINDING,
            shader_gen::SMAA_BLEND_WEIGHTS_BINDING,
            shader_gen::SMAA_BLEND_SAMP_BINDING,
            shader_gen::SMAA_BLEND_PARAMS_BINDING,
        ],
        [0, 1, 2, 3],
    );
    // The SSR pyramids: one input texture each (the Hi-Z seed's is scene depth), a sampler they
    // never call, then the uniform.
    assert_eq!(
        [
            shader_gen::HIZ_DEPTH_BINDING,
            shader_gen::HIZ_SAMP_BINDING,
            shader_gen::HIZ_PARAMS_BINDING,
        ],
        [0, 1, 2],
    );
    assert_eq!(shader_gen::HIZ_SRC_BINDING, 0);
    assert_eq!(
        [
            shader_gen::SSR_SRC_BINDING,
            shader_gen::SSR_SAMP_BINDING,
            shader_gen::SSR_COLOR_PARAMS_BINDING,
        ],
        [0, 1, 2],
    );
    assert_eq!(fullscreen_layout(ShaderId::HizSeed), LayoutId::HizSeed);
    assert_eq!(fullscreen_layout(ShaderId::HizReduce), LayoutId::HizReduce);
    assert_eq!(fullscreen_layout(ShaderId::SsrColor), LayoutId::SsrColor);
    // The trace: four inputs (Hi-Z, colour pyramid, normal+roughness, depth), then sampler and
    // uniform.
    assert_eq!(
        [
            shader_gen::TRACE_HIZ_BINDING,
            shader_gen::TRACE_COLOR_BINDING,
            shader_gen::TRACE_NORMAL_BINDING,
            shader_gen::TRACE_DEPTH_BINDING,
            shader_gen::TRACE_SAMP_BINDING,
            shader_gen::TRACE_PARAMS_BINDING,
        ],
        [0, 1, 2, 3, 4, 5],
    );
    assert_eq!(fullscreen_layout(ShaderId::SsrTrace), LayoutId::SsrTrace);
    // The debug view: the trace, sampler, uniform.
    assert_eq!(
        [
            shader_gen::DBG_TRACE_BINDING,
            shader_gen::DBG_SAMP_BINDING,
            shader_gen::DBG_PARAMS_BINDING,
        ],
        [0, 1, 2],
    );
    assert_eq!(fullscreen_layout(ShaderId::SsrDebug), LayoutId::SsrDebug);
    // And each of those shaders binds exactly the one layout the helper fills.
    assert_eq!(fullscreen_layout(ShaderId::Resolve), LayoutId::Resolve);
    assert_eq!(fullscreen_layout(ShaderId::Upscale), LayoutId::Upscale);
    assert_eq!(fullscreen_layout(ShaderId::Threshold), LayoutId::Threshold);
    assert_eq!(fullscreen_layout(ShaderId::Composite), LayoutId::Composite);
    assert_eq!(fullscreen_layout(ShaderId::BloomDown), LayoutId::BloomDown);
    assert_eq!(fullscreen_layout(ShaderId::BloomUp), LayoutId::BloomUp);
    assert_eq!(fullscreen_layout(ShaderId::Shafts), LayoutId::Shafts);
    assert_eq!(fullscreen_layout(ShaderId::BoxBlur), LayoutId::BoxBlur);
    assert_eq!(fullscreen_layout(ShaderId::DofCoc), LayoutId::DofCoc);
    assert_eq!(fullscreen_layout(ShaderId::Dof), LayoutId::Dof);
}

/// **The optional-input rule, pinned.** An optional slot holds its binding whether or not its
/// feature is on, and neither a second optional slot nor an absent neighbour may move it — which
/// is the whole property "append the present inputs and pad the tail" would fail.
///
/// The plan is pure arithmetic, so this needs no GPU: entry `b` says which graph input fills
/// binding `b`, and `None` is where the slot's fallback view goes.
#[test]
fn optional_inputs_hold_their_binding() {
    // No optional slots: every binding is the graph input at the same index — the original
    // shape, and what every pass but `composite` still uses.
    assert_eq!(binding_plan(1, &[]), [Some(0)]);
    assert_eq!(binding_plan(3, &[]), [Some(0), Some(1), Some(2)]);

    // Composite with shafts on and off: three required inputs, one optional at binding 3. The
    // required bindings do not move, and the optional one is filled either way.
    let shafts = shader_gen::SHAFTS_TEX_BINDING;
    assert_eq!(binding_plan(4, &[]), [Some(0), Some(1), Some(2), Some(3)]);
    assert_eq!(
        binding_plan(3, &[shafts]),
        [Some(0), Some(1), Some(2), None]
    );

    // The test the scheme exists to pass: the *second* optional slot, the depth-of-field gather,
    // cannot shift the first, in any combination of present and absent — and the bindings are the
    // WGSL's own, so this is the real pair rather than a hypothetical one.
    let dof = shader_gen::DOF_TEX_BINDING;
    assert_eq!(dof, shafts + 1, "the two optional slots are adjacent");
    assert_eq!(
        binding_plan(5, &[]),
        [Some(0), Some(1), Some(2), Some(3), Some(4)],
        "both optional inputs present",
    );
    assert_eq!(
        binding_plan(4, &[dof]),
        [Some(0), Some(1), Some(2), Some(3), None],
        "the second optional absent must not move the first",
    );
    assert_eq!(
        binding_plan(4, &[shafts]),
        [Some(0), Some(1), Some(2), None, Some(3)],
        "the first optional absent must not move the second",
    );
    assert_eq!(
        binding_plan(3, &[shafts, dof]),
        [Some(0), Some(1), Some(2), None, None],
        "both absent leaves the required inputs exactly where they were",
    );

    // The transparent layer sits behind both, adjacent to the gather, and cannot shift either
    // of them or be shifted by them.
    let layer = shader_gen::TRANSPARENT_DEPTH_TEX_BINDING;
    assert_eq!(layer, dof + 1, "the optional slots are adjacent");
    assert_eq!(
        binding_plan(6, &[]),
        [Some(0), Some(1), Some(2), Some(3), Some(4), Some(5)],
        "all three optional inputs present",
    );
    assert_eq!(
        binding_plan(5, &[layer]),
        [Some(0), Some(1), Some(2), Some(3), Some(4), None],
        "the layer absent must not move the other two",
    );
    assert_eq!(
        binding_plan(5, &[dof]),
        [Some(0), Some(1), Some(2), Some(3), None, Some(4)],
        "the gather absent must not move the layer's declared position past the shafts",
    );
    assert_eq!(
        binding_plan(3, &[shafts, dof, layer]),
        [Some(0), Some(1), Some(2), None, None, None],
        "all absent leaves the required inputs exactly where they were",
    );
}

/// Every renderer shader that composes snippets keeps a body that references them (guards against a
/// snippet extraction that left a dangling declaration behind).
#[test]
fn bodies_are_snippet_free() {
    for id in [ShaderId::Text, ShaderId::UiText] {
        let body = id.body_source();
        assert!(!body.contains("fn glyph_coverage"));
        assert!(!body.contains("atlas_tex: texture_2d"));
    }
    // A snippet body must not itself redeclare a struct the snippet provides.
    assert!(!ShaderId::Mesh.body_source().contains("struct Camera"));
    assert!(!ShaderId::Text.body_source().contains("struct Camera"));
    assert!(!ShaderId::Composite.body_source().contains("fn vs_main"));
    // Both `BAYER4` consumers must read the shared snippet, never redeclare the tile.
    assert!(!ShaderId::Composite.body_source().contains("BAYER4: array"));
    assert!(!ShaderId::Mesh.body_source().contains("BAYER4: array"));
}

/// A minimal asset mesh (one triangle) with an arbitrary content hash.
fn triangle(hash: u64) -> MeshData {
    MeshData {
        positions: vec![0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0],
        normals: vec![0.0, 0.0, 1.0, 0.0, 0.0, 1.0, 0.0, 0.0, 1.0],
        colors: vec![1.0; 12],
        materials: vec![1.0, 0.0, 0.0, 1.0, 0.0, 0.0, 1.0, 0.0, 0.0],
        uvs: vec![0.0; 6],
        tangents: vec![1.0, 0.0, 0.0, 1.0, 1.0, 0.0, 0.0, 1.0, 1.0, 0.0, 0.0, 1.0],
        weights: vec![1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0],
        bone_indices: vec![0; 12],
        skin_joints: Vec::new(),
        textures: None,
        authored_material: false,
        base_color_mean: [1.0; 4],
        rim: None,
        double_sided: false,
        indices: vec![0, 1, 2],
        hash,
        lods: Vec::new(),
    }
}

/// The interleaved [`MeshVertex`] stream from `upload_mesh_asset`'s zip has the documented
/// layout: position, then faceted normal, then per-vertex color, then per-vertex material, then
/// UV, then tangent, then skin weights, then bone indices, contiguous per vertex, and the stride
/// is exactly `3 + 3 + 4 + 3 + 2 + 4 + 4 + 4` lanes (the last two `u32`, the rest `f32`, same
/// size). Every lane rides through in vertex order.
#[test]
fn mesh_vertex_layout_is_position_normal_color_material_uv_tangent() {
    assert_eq!(
        std::mem::size_of::<MeshVertex>(),
        (3 + 3 + 4 + 3 + 2 + 4 + 4 + 4) * std::mem::size_of::<f32>(),
        "MeshVertex is a tightly packed position+normal+color+material+uv+tangent+weights+bones",
    );

    let mesh = MeshData {
        positions: vec![0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0],
        normals: vec![0.0, 0.0, 1.0, 0.0, 0.0, 1.0, 0.0, 0.0, 1.0],
        colors: vec![
            0.1, 0.2, 0.3, 0.4, // v0
            0.5, 0.6, 0.7, 0.8, // v1
            0.9, 1.0, 0.0, 1.0, // v2
        ],
        materials: vec![
            0.25, 0.5, 0.75, // v0
            1.0, 0.0, 2.0, // v1
            0.125, 0.375, 0.625, // v2
        ],
        uvs: vec![
            0.0, 0.0, // v0
            1.0, 0.0, // v1
            0.0, 1.0, // v2
        ],
        tangents: vec![
            1.0, 0.0, 0.0, 1.0, // v0
            0.0, 1.0, 0.0, -1.0, // v1
            0.0, 0.0, 1.0, 1.0, // v2
        ],
        weights: vec![
            1.0, 0.0, 0.0, 0.0, // v0
            0.5, 0.5, 0.0, 0.0, // v1
            0.0, 0.0, 1.0, 0.0, // v2
        ],
        bone_indices: vec![
            0, 0, 0, 0, // v0
            1, 2, 0, 0, // v1
            3, 0, 0, 0, // v2
        ],
        skin_joints: Vec::new(),
        textures: None,
        authored_material: true,
        base_color_mean: [1.0; 4],
        rim: None,
        double_sided: false,
        indices: vec![0, 1, 2],
        hash: 0,
        lods: Vec::new(),
    };
    let vertex_count = mesh.positions.len() / 3;
    let vertices: Vec<MeshVertex> = (0..vertex_count)
        .map(|v| MeshVertex {
            position: [
                mesh.positions[v * 3],
                mesh.positions[v * 3 + 1],
                mesh.positions[v * 3 + 2],
            ],
            normal: [
                mesh.normals[v * 3],
                mesh.normals[v * 3 + 1],
                mesh.normals[v * 3 + 2],
            ],
            color: [
                mesh.colors[v * 4],
                mesh.colors[v * 4 + 1],
                mesh.colors[v * 4 + 2],
                mesh.colors[v * 4 + 3],
            ],
            material: [
                mesh.materials[v * 3],
                mesh.materials[v * 3 + 1],
                mesh.materials[v * 3 + 2],
            ],
            uv: [mesh.uvs[v * 2], mesh.uvs[v * 2 + 1]],
            tangent: [
                mesh.tangents[v * 4],
                mesh.tangents[v * 4 + 1],
                mesh.tangents[v * 4 + 2],
                mesh.tangents[v * 4 + 3],
            ],
            weights: [
                mesh.weights[v * 4],
                mesh.weights[v * 4 + 1],
                mesh.weights[v * 4 + 2],
                mesh.weights[v * 4 + 3],
            ],
            bones: [
                mesh.bone_indices[v * 4],
                mesh.bone_indices[v * 4 + 1],
                mesh.bone_indices[v * 4 + 2],
                mesh.bone_indices[v * 4 + 3],
            ],
        })
        .collect();

    assert_eq!(vertices[0].position, [0.0, 0.0, 0.0]);
    assert_eq!(vertices[0].normal, [0.0, 0.0, 1.0]);
    assert_eq!(vertices[0].color, [0.1, 0.2, 0.3, 0.4]);
    assert_eq!(vertices[0].material, [0.25, 0.5, 0.75]);
    assert_eq!(vertices[1].color, [0.5, 0.6, 0.7, 0.8]);
    assert_eq!(vertices[1].material, [1.0, 0.0, 2.0]);
    assert_eq!(vertices[2].color, [0.9, 1.0, 0.0, 1.0]);
    assert_eq!(vertices[2].material, [0.125, 0.375, 0.625]);
    assert_eq!(vertices[0].uv, [0.0, 0.0]);
    assert_eq!(vertices[0].tangent, [1.0, 0.0, 0.0, 1.0]);
    assert_eq!(vertices[1].uv, [1.0, 0.0]);
    assert_eq!(vertices[1].tangent, [0.0, 1.0, 0.0, -1.0]);
    assert_eq!(vertices[2].uv, [0.0, 1.0]);
    assert_eq!(vertices[2].tangent, [0.0, 0.0, 1.0, 1.0]);
    assert_eq!(vertices[0].weights, [1.0, 0.0, 0.0, 0.0]);
    assert_eq!(vertices[0].bones, [0, 0, 0, 0]);
    assert_eq!(vertices[1].weights, [0.5, 0.5, 0.0, 0.0]);
    assert_eq!(vertices[1].bones, [1, 2, 0, 0]);
    assert_eq!(vertices[2].weights, [0.0, 0.0, 1.0, 0.0]);
    assert_eq!(vertices[2].bones, [3, 0, 0, 0]);
}

/// The pre-bucketing implementation of [`bucket_asset_instances`], kept verbatim as this test's
/// oracle: one filter pass over `scene.meshes` per asset index.
fn bucket_asset_instances_by_refiltering(
    scene: &RenderScene,
    asset_count: usize,
) -> Vec<Vec<MeshInstanceRaw>> {
    (0..asset_count)
        .map(|index| {
            scene
                .meshes
                .iter()
                .filter(|m| m.shape == MeshShape::Asset(index))
                .map(|m| MeshInstanceRaw {
                    medium: [1.0, 0.0, 0.0, 0.0],
                    model: m.model.to_cols_array_2d(),
                    color: m.color,
                    material: instance_material(m),
                    normal: instance_normal_lanes(m, &[]),
                    fade: instance_fade(m),
                    flash: instance_flash(m),
                    rim: m.rim,
                })
                .collect()
        })
        .collect()
}

/// The one bucketing pass emits exactly the bytes, in exactly the order, that the per-asset
/// re-filter did. Draw order decides pixel order under bit-exact pinned-GPU goldens, so the
/// oracle comparison is on the raw instance bytes, not just on counts.
///
/// The scene deliberately interleaves primitives with assets, revisits earlier asset indices out
/// of order, leaves one asset unreferenced (an empty bucket, which the caller skips), and
/// references an out-of-range index (dropped, as before — there is no GPU mesh for it).
#[test]
fn bucketing_asset_instances_preserves_the_refiltered_draw_order() {
    let asset = |index: usize, x: f32| MeshInstance {
        medium: 1.0,
        shape: MeshShape::Asset(index),
        model: Mat4::from_translation(Vec3::new(x, 0.0, 0.0)),
        normal_matrix: Mat3::from_diagonal(Vec3::splat(x + 1.0)),
        color: [x, 0.25, 0.5, 1.0],
        material: [x, 0.7, 2.0],
        use_vertex_material: index.is_multiple_of(2),
        fade: 1.0 - x / 32.0,
        fade_height_min: -f32::MAX,
        flash: 0.0,
        flash_color: [1.0; 3],
        wrap: 0.0,
        scatter_color: [1.0; 3],
        bone_offset: 0,
        cast_shadows: true,
        viewmodel: false,
        rim: [0.0; 4],
    };
    let mut scene = RenderScene::empty((64, 64));
    scene.meshes = vec![
        asset(2, 0.0),
        ground(4.0), // a primitive between assets
        asset(0, 1.0),
        asset(2, 2.0), // index 2 again, after a different index
        cube(Vec3::ZERO, Vec3::ONE, [1.0; 4]),
        asset(0, 3.0),
        asset(9, 4.0), // out of range: no GPU mesh, dropped by both implementations
        asset(2, 5.0),
    ];
    let asset_count = 4; // index 3 is registered but unreferenced: an empty bucket

    let bucketed = bucket_asset_instances(&scene, 0, asset_count, &[], false, false).0;
    let refiltered = bucket_asset_instances_by_refiltering(&scene, asset_count);
    assert_eq!(bucketed.len(), refiltered.len());
    for (index, (new, old)) in bucketed.iter().zip(refiltered.iter()).enumerate() {
        assert_eq!(
            bytemuck::cast_slice::<MeshInstanceRaw, u8>(new),
            bytemuck::cast_slice::<MeshInstanceRaw, u8>(old),
            "asset {index}'s instance buffer must be byte-identical to the re-filtered one",
        );
    }
    // And the oracle itself is exercising something: three instances on 2, two on 0, none on 3.
    assert_eq!(
        refiltered.iter().map(Vec::len).collect::<Vec<_>>(),
        vec![2, 0, 3, 0],
    );
}

/// Every procedural primitive vertex is opaque white, so the per-instance `Mesh.color` tints it
/// exactly (IEEE `x * 1.0 == x`). This is what keeps the existing goldens byte-identical.
#[test]
fn primitive_vertices_are_opaque_white() {
    for shape in [MeshShape::Cube, MeshShape::Plane, MeshShape::Sphere] {
        for vertex in mesh::vertices(shape) {
            assert_eq!(
                vertex.color,
                [1.0, 1.0, 1.0, 1.0],
                "primitive {shape:?} vertices must be opaque white",
            );
            assert_eq!(
                vertex.material,
                mesh::DEFAULT_MATERIAL,
                "primitive {shape:?} vertices carry the default material",
            );
        }
    }
}

/// The per-instance buffer is model + color + material + normal matrix + fade lane + flash lane: a
/// tightly packed `16 + 4 + 4 + 16 + 4 + 4 + 4` floats, and `instance_material` packs the component
/// scalars plus the selection flag (`1.0` when the geometry's own vertex material wins). The fade
/// lane (see [`instance_fade`]) occupies `@location(16)`, the flash lane `@location(17)`.
#[test]
fn instance_layout_carries_material_and_selection_flag() {
    assert_eq!(
        std::mem::size_of::<MeshInstanceRaw>(),
        (16 + 4 + 4 + 16 + 4 + 4 + 4 + 4) * std::mem::size_of::<f32>(),
        "MeshInstanceRaw is model + color + material + normal matrix + fade + flash + rim + medium \
         lanes, tightly packed",
    );
    assert_eq!(std::mem::size_of::<MeshInstanceRaw>(), 224);
    assert_eq!(std::mem::offset_of!(MeshInstanceRaw, medium), 208);

    let base = MeshInstance {
        medium: 1.0,
        shape: MeshShape::Cube,
        model: Mat4::IDENTITY,
        normal_matrix: Mat3::IDENTITY,
        color: [1.0; 4],
        material: [0.3, 0.7, 2.0],
        use_vertex_material: false,
        fade: 1.0,
        fade_height_min: -f32::MAX,
        flash: 0.0,
        flash_color: [1.0; 3],
        wrap: 0.0,
        scatter_color: [1.0; 3],
        bone_offset: 0,
        cast_shadows: true,
        viewmodel: false,
        rim: [0.0; 4],
    };
    assert_eq!(instance_material(&base), [0.3, 0.7, 2.0, 0.0]);
    assert_eq!(
        instance_material(&MeshInstance {
            use_vertex_material: true,
            ..base
        }),
        [0.3, 0.7, 2.0, 1.0],
    );
    // The fade lane packs `(fade, height_min, 0, handedness)`; the default `(1.0, -f32::MAX)` is
    // what every non-fading instance carries.
    assert_eq!(instance_fade(&base), [1.0, -f32::MAX, 0.0, 1.0]);
    assert_eq!(
        instance_fade(&MeshInstance {
            fade: 0.25,
            fade_height_min: 3.0,
            ..base
        }),
        [0.25, 3.0, 0.0, 1.0],
    );
    assert_eq!(
        instance_fade(&MeshInstance {
            model: Mat4::from_scale(Vec3::new(-1.0, 1.0, 1.0)),
            ..base
        }),
        [1.0, -f32::MAX, 0.0, -1.0],
        "a reflection flips the handedness sign",
    );
}

fn wrap_probe(wrap: f32, scatter: [f32; 3]) -> MeshInstance {
    MeshInstance {
        medium: 1.0,
        shape: MeshShape::Cube,
        model: Mat4::from_scale(Vec3::new(2.0, 4.0, 6.0)),
        normal_matrix: Mat3::from_diagonal(Vec3::new(0.5, 0.25, 1.0 / 6.0)),
        color: [1.0; 4],
        material: [0.3, 0.7, 2.0],
        use_vertex_material: true,
        fade: 1.0,
        fade_height_min: -f32::MAX,
        flash: 0.0,
        flash_color: [1.0; 3],
        wrap,
        scatter_color: scatter,
        bone_offset: 0,
        cast_shadows: true,
        viewmodel: false,
        rim: [0.0; 4],
    }
}

/// Wrap off uploads `Mat4::from_mat3`'s exact bytes; a wrapped instance packs its data in lanes the
/// normal product never reads, so the transformed normal is unchanged; nothing non-finite reaches a lane.
#[test]
fn normal_lanes_carry_wrap_without_moving_the_normal() {
    let off = wrap_probe(0.0, [0.2, 0.3, 0.4]);
    let plain = Mat4::from_mat3(off.normal_matrix).to_cols_array_2d();
    assert_eq!(instance_normal_lanes(&off, &[]), plain);
    assert_eq!(
        instance_normal_lanes(&wrap_probe(f32::NAN, [1.0; 3]), &[]),
        plain
    );

    let on = wrap_probe(0.5, [0.2, 0.3, 0.4]);
    let lanes = instance_normal_lanes(&on, &[]);
    assert_eq!(lanes[0][3], 0.5);
    assert_eq!(lanes[3][..3], [0.2, 0.3, 0.4]);
    assert_eq!([lanes[1][3], lanes[2][3], lanes[3][3]], [1.0, 2.0, 3.0]);
    let n = ludion_core::math::glam::Vec4::new(0.3, -0.6, 0.7, 0.0);
    let packed = Mat4::from_cols_array_2d(&lanes) * n;
    let unpacked = Mat4::from_cols_array_2d(&plain) * n;
    assert_eq!(packed.truncate(), unpacked.truncate());

    let hostile = instance_normal_lanes(
        &wrap_probe(f32::INFINITY, [f32::NAN, f32::INFINITY, -1.0]),
        &[],
    );
    assert!(
        hostile.iter().flatten().all(|v| v.is_finite()),
        "{hostile:?}"
    );
    assert_eq!(hostile[0][3], 1.0);
    assert_eq!(hostile[3][..3], [1.0, 1.0, 0.0]);
    let unknown = instance_normal_lanes(
        &MeshInstance {
            shape: MeshShape::Asset(9),
            ..on
        },
        &[],
    );
    assert_eq!([unknown[1][3], unknown[2][3], unknown[3][3]], [0.0; 3]);
}

/// Screen-space quads for `text` at `cell` px from the origin, on the embedded face.
fn test_quads(text: &str, cell: f32) -> Vec<ludion_font::GlyphQuad> {
    let face = ludion_font::FontFace::embedded();
    ludion_font::layout(face, text, cell, 0.0).quads([0.0, 0.0], cell, ludion_font::YAxis::Down)
}

/// An absent clip packs both on-flags as exactly 0.0, a present box sets only its own flag.
#[test]
fn ui_clip_raw_flags_follow_the_boxes() {
    let [_, _, r, t] = ui_clip_raw(&UiClip::NONE);
    assert_eq!((r[2], r[3]), (0.0, 0.0));
    assert_eq!(t, [1.0, 0.0, 1.0, 0.0]);
    let one = UiClipBox {
        rect_px: [1.0, 2.0, 3.0, 4.0],
        radius_px: 1.5,
        radians: 0.0,
    };
    let [a, b, r, _] = ui_clip_raw(&UiClip {
        boxes: [Some(one), None],
    });
    assert_eq!((a, b), ([1.0, 2.0, 3.0, 4.0], [0.0; 4]));
    assert_eq!(r, [1.5, 0.0, 1.0, 0.0]);
}

/// A rotated clip box packs its cosine and sine in its own slot, an unrotated one packs the
/// identity; an instance rotation packs `[cos, sin, pivot]`, and none packs `[1, 0, 0, 0]`.
#[test]
fn ui_rotation_packs_cos_sin_and_pivot() {
    let boxed = |radians| {
        Some(UiClipBox {
            rect_px: [0.0, 0.0, 10.0, 10.0],
            radius_px: 0.0,
            radians,
        })
    };
    let [_, _, _, t] = ui_clip_raw(&UiClip {
        boxes: [boxed(std::f32::consts::FRAC_PI_2), boxed(0.0)],
    });
    assert!((t[0]).abs() < 1e-6 && (t[1] - 1.0).abs() < 1e-6);
    assert_eq!((t[2], t[3]), (1.0, 0.0));
    assert_eq!(ui_rotation_raw(&UiRotation::NONE), [1.0, 0.0, 0.0, 0.0]);
    let raw = ui_rotation_raw(&UiRotation {
        radians: std::f32::consts::PI,
        pivot_px: [3.0, 4.0],
    });
    assert!((raw[0] + 1.0).abs() < 1e-6 && raw[1].abs() < 1e-6);
    assert_eq!((raw[2], raw[3]), (3.0, 4.0));
}

/// One PNG read as a loose texture (premultiplied by the engine) and as an atlas colour map
/// (stored bytes) has one content hash and two pixel streams: the sprite texture cache must hold
/// both rather than serve whichever uploaded first. Skips with no GPU.
#[test]
fn needs_gpu_texture_cache_keys_on_hash_and_premultiplication() {
    let renderer = match Renderer::new() {
        Ok(r) => r,
        Err(d) if ludion_core::require_gpu::is_no_adapter(std::slice::from_ref(&d)) => {
            eprintln!("SKIPPED texture_cache_keys_on_hash_and_premultiplication: no GPU adapter");
            return;
        }
        Err(d) => panic!("renderer init failed: {d:?}"),
    };
    let image = |rgba: [u8; 4], premultiplied_on_load: bool| {
        Arc::new(TextureImage {
            width: 1,
            height: 1,
            rgba: rgba.to_vec(),
            hash: 0xABCD,
            filter_linear: false,
            premultiplied_on_load,
            mean: [1.0; 4],
        })
    };
    let mut scene = RenderScene::empty((64, 64));
    scene.textures = vec![
        image([100, 50, 25, 128], true),
        image([200, 100, 50, 128], false),
    ];

    assert_eq!(
        renderer
            .resolve_textures(&scene, CacheEviction::Evict)
            .len(),
        2
    );
    assert_eq!(
        renderer.texture_cache.borrow().len(),
        2,
        "same hash, different premultiplication: two cache entries"
    );
}

/// UI batching walks the pinned order and never merges across kinds or across a rect texture
/// change: consecutive untextured rects coalesce into one instanced batch, but a text between two
/// rects forces three batches (correctness of draw order beats batch size). Skips with no GPU.
#[test]
fn needs_gpu_ui_batches_respect_kind_and_texture_boundaries() {
    let renderer = match Renderer::new() {
        Ok(r) => r,
        Err(d) if ludion_core::require_gpu::is_no_adapter(std::slice::from_ref(&d)) => {
            eprintln!("SKIPPED ui_batches_respect_kind_and_texture_boundaries: no GPU adapter");
            return;
        }
        Err(d) => panic!("renderer init failed: {d:?}"),
    };
    let device = renderer.gpu.device();

    let rect = |x: f32| UiInstance::Rect {
        rect_px: [x, 0.0, 10.0, 10.0],
        texture: None,
        color: [1.0, 1.0, 1.0, 1.0],
        uv: [0.0, 0.0, 1.0, 1.0],
        clip: UiClip::NONE,
        view: None,
        rotation: crate::UiRotation::NONE,
    };
    let text = UiInstance::Text {
        color: [1.0; 4],
        text: "A".to_string(),
        quads: test_quads("A", 8.0),
        font: None,
        clip: UiClip::NONE,
        outline: None,
        rotation: crate::UiRotation::NONE,
    };

    // Two consecutive untextured rects → one batch of two instances.
    let mut scene = RenderScene::empty((64, 64));
    scene.ui = vec![rect(0.0), rect(20.0)];
    let batches = renderer.build_ui_batches(&scene, device, &[], &[]);
    assert_eq!(batches.len(), 1);
    assert!(
        matches!(batches[0], UiBatch::Rects { count: 2, .. }),
        "consecutive same-texture rects coalesce",
    );

    // rect, text, rect → three batches (a kind change closes each batch).
    scene.ui = vec![rect(0.0), text, rect(20.0)];
    let batches = renderer.build_ui_batches(&scene, device, &[], &[]);
    assert_eq!(batches.len(), 3, "kinds never merge across the boundary");
    assert!(matches!(batches[0], UiBatch::Rects { .. }));
    assert!(matches!(batches[1], UiBatch::Text { .. }));
    assert!(matches!(batches[2], UiBatch::Rects { .. }));

    // Text drawn from different faces never share a batch: [None, None, a, a, None] → 3 batches.
    let font = FontDraw {
        texture: 0,
        sdf: true,
        spread_px: 4,
    };
    let text_with = |font: Option<FontDraw>| UiInstance::Text {
        color: [1.0; 4],
        text: "A".to_string(),
        quads: test_quads("A", 8.0),
        font,
        clip: UiClip::NONE,
        outline: None,
        rotation: crate::UiRotation::NONE,
    };
    scene.ui = vec![
        text_with(None),
        text_with(None),
        text_with(Some(font)),
        text_with(Some(font)),
        text_with(None),
    ];
    let atlas = [GpuTexture {
        texture: renderer.atlas.texture.clone(),
        view: renderer.atlas.view.clone(),
    }];
    let batches = renderer.build_ui_batches(&scene, device, &atlas, &[]);
    assert_eq!(batches.len(), 3, "a face change closes the text batch");
    assert!(batches.iter().all(|b| matches!(
        b,
        UiBatch::Text { count: 12, .. } | UiBatch::Text { count: 6, .. }
    )));

    // World runs group the same way.
    let run = |font: Option<FontDraw>| crate::scene::TextRun {
        position: Vec3::ZERO,
        size: 8.0,
        color: [1.0; 4],
        text: "A".to_string(),
        quads: test_quads("A", 8.0),
        extent: [8.0, 8.0],
        font,
    };
    scene.texts = vec![
        run(None),
        run(None),
        run(Some(font)),
        run(Some(font)),
        run(None),
    ];
    let draws = renderer.build_text_draws(&scene, device, &atlas);
    assert_eq!(draws.len(), 3);
    assert_eq!(
        draws.iter().map(|d| d.count).collect::<Vec<_>>(),
        [12, 12, 6]
    );
}

/// A translucent untextured `UiRect` composites to the straight-alpha result it always has.
///
/// The rect pass is premultiplied — the shader weights the tint by its own alpha and the blend
/// is `One, OneMinusSrcAlpha` — while an untextured rect binds the 1x1 white texture, on which
/// premultiplication is the identity. The fragment is therefore `(C * a, a)` and the blend gives
/// `C * a + dst * (1 - a)`: exactly what `frag.rgb * frag.a + dst * (1 - frag.a)` produced when
/// the shader emitted straight `(C, a)` into an `alpha_blend()` target. This pins that
/// equivalence against the analytic value, so the shipped HUD cannot drift by half a tint. Read
/// in linear light, because the output byte is sRGB-encoded on store. Skips with no GPU.
#[test]
fn needs_gpu_a_translucent_solid_ui_rect_composites_to_the_straight_alpha_result() {
    let renderer = match Renderer::new() {
        Ok(r) => r,
        Err(d) if ludion_core::require_gpu::is_no_adapter(std::slice::from_ref(&d)) => {
            eprintln!(
                "SKIPPED a_translucent_solid_ui_rect_composites_to_the_straight_alpha_result: \
                 no GPU adapter"
            );
            return;
        }
        Err(d) => panic!("renderer init failed: {d:?}"),
    };

    let (w, h) = (64u32, 64u32);
    const ALPHA: f64 = 0.5;
    const TINT: [f64; 3] = [1.0, 0.25, 0.0];

    let bare = RenderScene::empty((w, h));
    let mut with_ui = bare.clone();
    with_ui.ui = vec![UiInstance::Rect {
        rect_px: [0.0, 0.0, 20.0, 20.0],
        texture: None,
        color: [TINT[0] as f32, TINT[1] as f32, TINT[2] as f32, ALPHA as f32],
        uv: [0.0, 0.0, 1.0, 1.0],
        clip: UiClip::NONE,
        view: None,
        rotation: crate::UiRotation::NONE,
    }];

    let bare_img = renderer
        .render(&bare, FrameSize::native(w, h))
        .expect("bare render");
    let ui_img = renderer
        .render(&with_ui, FrameSize::native(w, h))
        .expect("translucent ui render");

    // An interior pixel of the rect, well away from every edge.
    let idx = ((5 * w + 5) * 4) as usize;
    for (channel, tint) in TINT.iter().enumerate() {
        let dst = linear_from_srgb8(bare_img.rgba[idx + channel]);
        let expected = tint * ALPHA + dst * (1.0 - ALPHA);
        let got = linear_from_srgb8(ui_img.rgba[idx + channel]);
        assert!(
            (got - expected).abs() <= 0.006,
            "channel {channel}: premultiplied composite {got} must equal the straight-alpha \
             value {expected} (within the 8-bit sRGB round trip)",
        );
    }
    // Coverage is unchanged too: alpha still leaves the pass as `a`, not `a * a`.
    assert_eq!(
        ui_img.rgba[idx + 3],
        255,
        "the output alpha channel is opaque after compositing over an opaque destination",
    );
}

/// End-to-end overlay smoke test: a UI-carrying scene renders without panic (exercising the three
/// new pipelines' creation + the overlay pass), differs from the same scene with UI stripped, and
/// an opaque corner-anchored red rect paints red pixels where it sits. Skips with no GPU.
#[test]
fn needs_gpu_overlay_draws_ui_over_the_scene() {
    let renderer = match Renderer::new() {
        Ok(r) => r,
        Err(d) if ludion_core::require_gpu::is_no_adapter(std::slice::from_ref(&d)) => {
            eprintln!("SKIPPED overlay_draws_ui_over_the_scene: no GPU adapter");
            return;
        }
        Err(d) => panic!("renderer init failed: {d:?}"),
    };

    let (w, h) = (64u32, 64u32);
    let bare = RenderScene::empty((w, h));
    let mut with_ui = bare.clone();
    // An opaque red 20x20 rect at the top-left pixel corner.
    with_ui.ui = vec![UiInstance::Rect {
        rect_px: [0.0, 0.0, 20.0, 20.0],
        texture: None,
        color: [1.0, 0.0, 0.0, 1.0],
        uv: [0.0, 0.0, 1.0, 1.0],
        clip: UiClip::NONE,
        view: None,
        rotation: crate::UiRotation::NONE,
    }];

    let bare_img = renderer
        .render(&bare, FrameSize::native(w, h))
        .expect("bare render");
    let ui_img = renderer
        .render(&with_ui, FrameSize::native(w, h))
        .expect("ui render");

    assert_ne!(bare_img.rgba, ui_img.rgba, "the overlay changes the image");

    // Interior pixel of the rect (x=5, y=5), top row first: red dominant, ~opaque.
    let idx = ((5 * w + 5) * 4) as usize;
    let (r, g, b) = (ui_img.rgba[idx], ui_img.rgba[idx + 1], ui_img.rgba[idx + 2]);
    assert!(
        r > 200 && g < 60 && b < 60,
        "corner rect pixel is red: rgb=({r},{g},{b})",
    );

    // A pixel far outside the rect (bottom-right) still shows the scene: the blit reproduces the
    // composited image (within the sRGB 8-bit decode/re-encode round-trip, ±a couple of LSBs).
    let far = (((h - 1) * w + (w - 1)) * 4) as usize;
    for c in 0..4 {
        let diff = ui_img.rgba[far + c].abs_diff(bare_img.rgba[far + c]);
        assert!(
            diff <= 2,
            "outside-UI pixel channel {c} drifted {diff} from the bare scene",
        );
    }
}

/// The GPU mesh cache keys on content hash: an asset mesh referenced by two instances uploads
/// **once** (one cache entry), the mesh analog of the texture cache's dedup. Skips with no GPU.
#[test]
fn needs_gpu_same_asset_mesh_referenced_twice_uploads_once() {
    let renderer = match Renderer::new() {
        Ok(r) => r,
        Err(d) if ludion_core::require_gpu::is_no_adapter(std::slice::from_ref(&d)) => {
            eprintln!("SKIPPED same_asset_mesh_referenced_twice_uploads_once: no GPU adapter");
            return;
        }
        Err(d) => panic!("renderer init failed: {d:?}"),
    };

    let mut scene = RenderScene::empty((64, 64));
    scene.mesh_assets.push(Arc::new(triangle(0xABCD)));
    // Two instances of the same asset (index 0) at different transforms.
    for tx in [-1.0_f32, 1.0] {
        scene.meshes.push(MeshInstance {
            medium: 1.0,
            shape: MeshShape::Asset(0),
            model: Mat4::from_translation(Vec3::new(tx, 0.0, 0.0)),
            normal_matrix: Mat3::IDENTITY,
            color: [1.0; 4],
            material: [1.0, 0.0, 0.0],
            use_vertex_material: false,
            fade: 1.0,
            fade_height_min: -f32::MAX,
            flash: 0.0,
            flash_color: [1.0; 3],
            wrap: 0.0,
            scatter_color: [1.0; 3],
            bone_offset: 0,
            cast_shadows: true,
            viewmodel: false,
            rim: [0.0; 4],
        });
    }

    renderer
        .render(&scene, FrameSize::native(64, 64))
        .expect("render succeeds");
    assert_eq!(
        renderer.mesh_asset_cache.borrow().len(),
        1,
        "one unique content hash ⇒ exactly one GPU upload, however many instances reference it",
    );
}

/// A frame uploads only what the previous frame did not already hold: the cumulative counters
/// move by exactly the cache misses, and a repeated frame moves them by nothing. Skips with no GPU.
#[test]
fn needs_gpu_a_frame_uploads_only_what_the_last_frame_lacked() {
    let renderer = match Renderer::new() {
        Ok(r) => r,
        Err(d) if ludion_core::require_gpu::is_no_adapter(std::slice::from_ref(&d)) => {
            eprintln!("SKIPPED a_frame_uploads_only_what_the_last_frame_lacked: no GPU adapter");
            return;
        }
        Err(d) => panic!("renderer init failed: {d:?}"),
    };
    let texture = |hash: u64| {
        Arc::new(TextureImage {
            width: 1,
            height: 1,
            rgba: vec![255, 255, 255, 255],
            hash,
            filter_linear: false,
            premultiplied_on_load: false,
            mean: [1.0; 4],
        })
    };
    let mesh = |hash: u64| {
        let mut mesh = triangle(hash);
        if hash == 11 {
            mesh.textures = Some(crate::scene::MeshTextures {
                base_color: Some(crate::scene::MeshTexture {
                    image: MapImage::Source(texture(0x5EED)),
                    wrap: crate::scene::TextureWrap::Clamp,
                }),
                metallic_roughness: None,
                normal: None,
                occlusion: None,
                emissive: None,
                normal_scale: 1.0,
                occlusion_strength: 1.0,
                alpha_cutoff: None,
            });
        }
        Arc::new(mesh)
    };
    let frame = |textures: [u64; 2], meshes: [u64; 2]| {
        let mut scene = RenderScene::empty((64, 64));
        scene.textures = textures.map(texture).to_vec();
        scene.mesh_assets = meshes.map(mesh).to_vec();
        scene
    };
    let mut last = renderer.uploads();
    let mut delta = |scene: &RenderScene| {
        renderer
            .render(scene, FrameSize::native(64, 64))
            .expect("render succeeds");
        let now = renderer.uploads();
        let moved = GpuUploads {
            textures: now.textures - last.textures,
            mesh_maps: now.mesh_maps - last.mesh_maps,
            mesh_assets: now.mesh_assets - last.mesh_assets,
        };
        last = now;
        moved
    };
    let a = frame([1, 2], [10, 11]);
    let b = frame([2, 3], [11, 12]);
    let expect = |textures, mesh_maps, mesh_assets| GpuUploads {
        textures,
        mesh_maps,
        mesh_assets,
    };
    assert_eq!(delta(&a), expect(2, 1, 2), "first frame uploads everything");
    assert_eq!(
        delta(&b),
        expect(1, 0, 1),
        "only texture 3 and mesh 12 are new"
    );
    assert_eq!(
        delta(&b),
        expect(0, 0, 0),
        "a repeated frame uploads nothing"
    );
}

/// A prewarmed mesh uploads once and stays resident without being drawn: the frame that then
/// draws it uploads nothing, and a frame naming it nowhere evicts it. Skips with no GPU.
#[test]
fn needs_gpu_a_prewarmed_mesh_uploads_once_and_is_evicted_when_unnamed() {
    let renderer = match Renderer::new() {
        Ok(r) => r,
        Err(d) if ludion_core::require_gpu::is_no_adapter(std::slice::from_ref(&d)) => {
            eprintln!("SKIPPED a_prewarmed_mesh_uploads_once: no GPU adapter");
            return;
        }
        Err(d) => panic!("renderer init failed: {d:?}"),
    };
    let m = Arc::new(triangle(0x77));
    let mut last = renderer.uploads().mesh_assets;
    let mut moved = |scene: &RenderScene| {
        renderer
            .render(scene, FrameSize::native(64, 64))
            .expect("render succeeds");
        let now = renderer.uploads().mesh_assets;
        let delta = now - last;
        last = now;
        delta
    };
    let mut warm = RenderScene::empty((64, 64));
    warm.prewarm = vec![m.clone()];
    assert_eq!(moved(&warm), 1, "the prewarm uploads the mesh");
    assert_eq!(moved(&warm), 0, "a kept prewarm uploads nothing more");
    let mut drawn = RenderScene::empty((64, 64));
    drawn.mesh_assets = vec![m];
    assert_eq!(
        moved(&drawn),
        0,
        "the frame that draws it finds it resident"
    );
    let bare = RenderScene::empty((64, 64));
    assert_eq!(moved(&bare), 0, "an empty frame uploads nothing");
    assert_eq!(
        moved(&drawn),
        1,
        "and evicted it, so drawing it uploads again"
    );
}

/// An asset mesh with no vertices and no indices is valid content (a model whose last voxel was
/// erased): it is skipped at draw build rather than sliced as a zero-size buffer, and the other
/// asset in the frame still draws. Skips with no GPU.
#[test]
fn needs_gpu_an_empty_asset_mesh_is_skipped_and_the_rest_still_draws() {
    let renderer = match Renderer::new() {
        Ok(r) => r,
        Err(d) if ludion_core::require_gpu::is_no_adapter(std::slice::from_ref(&d)) => {
            eprintln!("SKIPPED an_empty_asset_mesh_is_skipped: no GPU adapter");
            return;
        }
        Err(d) => panic!("renderer init failed: {d:?}"),
    };
    let instance = |index: usize| MeshInstance {
        medium: 1.0,
        shape: MeshShape::Asset(index),
        model: Mat4::IDENTITY,
        normal_matrix: Mat3::IDENTITY,
        color: [1.0; 4],
        material: [1.0, 0.0, 0.0],
        use_vertex_material: false,
        fade: 1.0,
        fade_height_min: -f32::MAX,
        flash: 0.0,
        flash_color: [1.0; 3],
        wrap: 0.0,
        scatter_color: [1.0; 3],
        bone_offset: 0,
        cast_shadows: true,
        viewmodel: false,
        rim: [0.0; 4],
    };
    let empty = MeshData {
        positions: vec![],
        normals: vec![],
        colors: vec![],
        materials: vec![],
        uvs: vec![],
        tangents: vec![],
        weights: vec![],
        bone_indices: vec![],
        skin_joints: Vec::new(),
        textures: None,
        authored_material: false,
        base_color_mean: [1.0; 4],
        rim: None,
        double_sided: false,
        indices: vec![],
        hash: 0xE0E0,
        lods: Vec::new(),
    };
    let size = FrameSize::native(64, 64);

    let background = renderer
        .render(&RenderScene::empty((64, 64)), size)
        .expect("empty scene renders");

    let mut only_empty = RenderScene::empty((64, 64));
    only_empty.mesh_assets.push(Arc::new(empty.clone()));
    only_empty.meshes.push(instance(0));
    let image = renderer
        .render(&only_empty, size)
        .expect("an empty asset mesh renders without a panic");
    assert_eq!(image, background, "an empty mesh draws nothing");

    let mut both = RenderScene::empty((64, 64));
    both.mesh_assets.push(Arc::new(empty));
    both.mesh_assets.push(Arc::new(triangle(0xABCD)));
    both.meshes.push(instance(0));
    both.meshes.push(instance(1));
    let image = renderer.render(&both, size).expect("both render");
    assert_ne!(
        image, background,
        "the triangle beside the empty mesh still draws"
    );
}

// --- Shadow mapping (Plan/shadow-mapping.md) ------------------------------------------

/// A downward-angled sun with an orthographic frustum wide/deep enough to bracket the test scenes
/// below. Mirrors `ludion_engine::extract`'s `build_sun` math (the renderer never sees the
/// component); `direction` is the travel direction, `color` premultiplied.
fn test_sun(direction: Vec3, color: Vec3, bias: f32) -> crate::scene::SunData {
    test_sun_framed(direction, color, bias, (20.0, 1.0, 40.0))
}

/// [`test_sun`] with its frustum chosen: `(extent, near, far)`.
fn test_sun_framed(
    direction: Vec3,
    color: Vec3,
    bias: f32,
    (extent, near, far): (f32, f32, f32),
) -> crate::scene::SunData {
    let dir = direction.normalize();
    let center = Vec3::ZERO;
    let back = 0.5 * (near + far);
    let eye = center - dir * back;
    let up = if dir.y.abs() > 0.999 {
        Vec3::Z
    } else {
        Vec3::Y
    };
    let view = Mat4::look_to_rh(eye, dir, up);
    let projection = Mat4::orthographic_rh(-extent, extent, -extent, extent, near, far);
    crate::scene::SunData {
        direction: dir,
        color,
        light_space: projection * view,
        bias,
        angular_radius: 0.0,
        cascades: None,
    }
}

/// A large ground plane (`scale` × 1 × `scale`) at the origin, mid-grey, facing +Y.
fn ground(scale: f32) -> MeshInstance {
    MeshInstance {
        medium: 1.0,
        shape: MeshShape::Plane,
        model: Mat4::from_scale(Vec3::new(scale, 1.0, scale)),
        normal_matrix: Mat3::IDENTITY,
        color: [0.6, 0.6, 0.6, 1.0],
        material: [1.0, 0.0, 0.0],
        use_vertex_material: false,
        fade: 1.0,
        fade_height_min: -f32::MAX,
        flash: 0.0,
        flash_color: [1.0; 3],
        wrap: 0.0,
        scatter_color: [1.0; 3],
        bone_offset: 0,
        cast_shadows: true,
        viewmodel: false,
        rim: [0.0; 4],
    }
}

/// A unit cube scaled `size` at `pos`.
fn cube(pos: Vec3, size: Vec3, color: [f32; 4]) -> MeshInstance {
    MeshInstance {
        medium: 1.0,
        shape: MeshShape::Cube,
        model: Mat4::from_translation(pos) * Mat4::from_scale(size),
        normal_matrix: Mat3::IDENTITY,
        color,
        material: [1.0, 0.0, 0.0],
        use_vertex_material: false,
        fade: 1.0,
        fade_height_min: -f32::MAX,
        flash: 0.0,
        flash_color: [1.0; 3],
        wrap: 0.0,
        scatter_color: [1.0; 3],
        bone_offset: 0,
        cast_shadows: true,
        viewmodel: false,
        rim: [0.0; 4],
    }
}

fn skip_or_renderer(name: &str) -> Option<Renderer> {
    match Renderer::new() {
        Ok(r) => Some(r),
        Err(d) if ludion_core::require_gpu::is_no_adapter(std::slice::from_ref(&d)) => {
            eprintln!("SKIPPED {name}: no GPU adapter");
            None
        }
        Err(d) => panic!("renderer init failed: {d:?}"),
    }
}

fn luma(img: &RenderedImage, x: u32, y: u32) -> u32 {
    let i = ((y * img.width + x) * 4) as usize;
    img.rgba[i] as u32 + img.rgba[i + 1] as u32 + img.rgba[i + 2] as u32
}

/// (a) A sun changes the rendered pixels: the same scene with and without a sun differs, because
/// the mesh shader's sun term adds real light (and the sunless path is render-inert by construction
/// — a zeroed `Sun`). (c) The sun scene rendered twice is byte-identical (determinism on a pinned
/// adapter — the shadow pass + PCF are as deterministic as the rest).
#[test]
fn needs_gpu_sun_changes_pixels_and_is_deterministic() {
    let Some(renderer) = skip_or_renderer("sun_changes_pixels_and_is_deterministic") else {
        return;
    };
    let (w, h) = (64u32, 64u32);
    let mut scene = RenderScene::empty((w, h));
    scene.ambient = [0.05, 0.05, 0.05];
    scene.meshes = vec![
        ground(20.0),
        cube(
            Vec3::new(0.0, 1.0, 0.0),
            Vec3::splat(1.5),
            [0.8, 0.2, 0.2, 1.0],
        ),
    ];

    let sunless = renderer
        .render(&scene, FrameSize::native(w, h))
        .expect("sunless render");

    scene.sun = Some(test_sun(
        Vec3::new(0.4, -1.0, 0.3),
        Vec3::new(3.0, 3.0, 3.0),
        0.002,
    ));
    let sunlit = renderer
        .render(&scene, FrameSize::native(w, h))
        .expect("sunlit render");
    let sunlit_again = renderer
        .render(&scene, FrameSize::native(w, h))
        .expect("second sunlit render");

    assert_ne!(sunless.rgba, sunlit.rgba, "the sun changes the image");
    assert_eq!(
        sunlit.rgba, sunlit_again.rgba,
        "the same sun scene renders byte-identically twice",
    );
}

/// `angular_radius` turns the sun's specular point into a soft disc: on a near-mirror plane that
/// reflects the sun, more pixels are bright and the peak does not rise, and two renders agree.
#[test]
fn needs_gpu_sun_angular_radius_widens_the_specular_highlight() {
    let Some(renderer) = skip_or_renderer("sun_angular_radius_widens_the_specular_highlight")
    else {
        return;
    };
    let (w, h) = (64u32, 64u32);
    let size = FrameSize::native(w, h);
    let mut scene = RenderScene::empty((w, h));
    scene.post = Some(exact_post());
    scene.ambient = [0.0; 3];
    // The sun travels toward −z and down, so it sits at +z above the plane and the camera at −z
    // looks straight at its mirror image.
    let eye = Vec3::new(0.0, 3.0, -5.196);
    scene.camera = crate::scene::RenderCamera {
        view: Mat4::look_at_rh(eye, Vec3::ZERO, Vec3::Y),
        projection: Mat4::perspective_rh(40.0_f32.to_radians(), 1.0, 0.1, 100.0),
        position: eye,
    };
    let mut mirror = ground(20.0);
    mirror.color = [1.0, 1.0, 1.0, 1.0];
    mirror.material = [0.05, 1.0, 0.0];
    scene.meshes = vec![mirror];
    let mut sun = test_sun(Vec3::new(0.0, -0.5, -0.866), Vec3::splat(2.0e-5), 0.002);
    let stats = |renderer: &Renderer, scene: &RenderScene| {
        let img = renderer.render(scene, size).expect("render");
        let lumas: Vec<u32> = (0..h)
            .flat_map(|y| (0..w).map(move |x| (x, y)))
            .map(|(x, y)| luma(&img, x, y))
            .collect();
        let peak = lumas.iter().copied().max().unwrap_or(0);
        let bright = lumas.iter().filter(|&&l| l >= 20).count();
        (img.rgba, peak, bright)
    };
    scene.sun = Some(sun);
    let (_, point_peak, point_bright) = stats(&renderer, &scene);
    sun.angular_radius = 2.0_f32.to_radians();
    scene.sun = Some(sun);
    let (disc, disc_peak, disc_bright) = stats(&renderer, &scene);
    let (disc_again, _, _) = stats(&renderer, &scene);
    assert!(
        disc_bright > point_bright,
        "the disc covers more pixels: {point_bright} -> {disc_bright} (peak {point_peak} -> {disc_peak})"
    );
    assert!(
        disc_peak <= point_peak,
        "the disc does not raise the peak: {point_peak} -> {disc_peak}"
    );
    assert_eq!(disc, disc_again, "the disc renders byte-identically twice");
}

/// A top-down camera over a ground, a thin slab `height` metres above it covering everything left of
/// its edge, and a sun tilted so the shadow edge falls into view right of the slab's own image. `size`
/// is the sun's shadow map edge; `cascaded` fits one cascade over the view instead of the anchored box.
/// Returns the scene and the pixel row's first ground pixel past the slab.
fn penumbra_scene(
    height: f32,
    angular_radius: f32,
    cascaded: bool,
    (w, h): (u32, u32),
) -> (RenderScene, u32) {
    let mut scene = RenderScene::empty((w, h));
    scene.post = Some(exact_post());
    scene.ambient = [0.1; 3];
    let eye = Vec3::new(0.0, 40.0, 0.0);
    let aspect = crate::scene::aspect_ratio((w, h));
    scene.camera = crate::scene::RenderCamera {
        view: Mat4::look_to_rh(eye, -Vec3::Y, -Vec3::Z),
        // A 6° horizontal fov: the frame spans 4.2 m of ground.
        projection: Mat4::perspective_rh(
            2.0 * (3.0_f32.to_radians().tan() / aspect).atan(),
            aspect,
            0.1,
            1000.0,
        ),
        position: eye,
    };
    let mut sun = test_sun_framed(
        Vec3::new(0.3, -1.0, 0.0),
        Vec3::splat(3.0),
        0.05 / 48.0,
        (4.0, 1.0, 40.0),
    );
    sun.angular_radius = angular_radius;
    if cascaded {
        // The ground is 40 m from the camera: cascade 1 covers view depth 35–45 m.
        sun.cascades = Some(crate::scene::SunCascadeConfig {
            count: 3,
            shadow_distance: 50.0,
            splits: Some([0.7, 0.9, 1.0]),
            shadow_near: -24.0,
            shadow_far: 24.0,
        });
    }
    scene.sun = Some(sun);
    let edge = -1.05;
    scene.meshes = vec![
        ground(60.0),
        cube(
            Vec3::new(edge - 10.0, height, 0.0),
            Vec3::new(20.0, 0.1, 20.0),
            [0.8; 4],
        ),
    ];
    let (slab_x, _) = project_pixel(&scene, Vec3::new(edge, height + 0.05, 0.0), (w, h));
    (scene, slab_x + 3)
}

/// Transition pixels along the middle row of a [`penumbra_scene`] render: luma strictly between the
/// shadowed level at the slab's edge and the lit level at the right border.
fn penumbra_band(renderer: &Renderer, height: f32, angular_radius: f32, cascaded: bool) -> u32 {
    let (w, h) = (256u32, 64u32);
    let (scene, first) = penumbra_scene(height, angular_radius, cascaded, (w, h));
    let img = renderer
        .render(&scene, FrameSize::native(w, h))
        .expect("render");
    let again = renderer
        .render(&scene, FrameSize::native(w, h))
        .expect("render again");
    assert_eq!(img.rgba, again.rgba, "two renders agree byte for byte");
    let row = h / 2;
    let (dark, lit) = (luma(&img, first, row), luma(&img, w - 1, row));
    assert!(
        dark + 30 < lit,
        "the shadow is in view (dark {dark}, lit {lit})"
    );
    (first..w)
        .filter(|&x| (dark + 1..lit).contains(&luma(&img, x, row)))
        .count() as u32
}

/// Pixel regions of a [`grazing_scene`] frame, `(x0, y0, x1, y1)` inclusive.
struct GrazingProbes {
    /// Open ground well upwind of the caster.
    upwind: (u32, u32, u32, u32),
    /// Open ground beside the shadow, clear of its footprint and penumbra.
    beside: (u32, u32, u32, u32),
    /// Rows that cross the shadow's far edge.
    tip_rows: Vec<u32>,
    /// First column geometrically past the shadow's far edge, penumbra excluded.
    lit_from: u32,
    /// Last column of the frame.
    last: u32,
    /// A pixel deep in the umbra.
    umbra: (u32, u32),
}

/// A top-down camera over a ground and a 1 m cube, lit by a sun 9° above the horizon (a grazing
/// angle, where the receiver's depth slope across a shadow-map footprint is steep). The cube's
/// shadow runs about 6.5 m to the right; the frame is 14 m wide.
fn grazing_scene(
    angular_radius: f32,
    cascaded: bool,
    (w, h): (u32, u32),
) -> (RenderScene, GrazingProbes) {
    let mut scene = RenderScene::empty((w, h));
    scene.post = Some(exact_post());
    scene.ambient = [0.1; 3];
    let eye = Vec3::new(0.0, 40.0, 0.0);
    let aspect = crate::scene::aspect_ratio((w, h));
    scene.camera = crate::scene::RenderCamera {
        view: Mat4::look_to_rh(eye, -Vec3::Y, -Vec3::Z),
        // A 20° horizontal fov: the frame spans about 14 m of ground.
        projection: Mat4::perspective_rh(
            2.0 * (10.0_f32.to_radians().tan() / aspect).atan(),
            aspect,
            0.1,
            1000.0,
        ),
        position: eye,
    };
    let elevation = 9.0_f32.to_radians();
    let mut sun = test_sun_framed(
        Vec3::new(elevation.cos(), -elevation.sin(), 0.15),
        Vec3::splat(3.0),
        0.05 / 48.0,
        (10.0, 1.0, 40.0),
    );
    sun.angular_radius = angular_radius;
    if cascaded {
        sun.cascades = Some(crate::scene::SunCascadeConfig {
            count: 3,
            shadow_distance: 50.0,
            splits: Some([0.7, 0.9, 1.0]),
            shadow_near: -24.0,
            shadow_far: 24.0,
        });
    }
    let dir = sun.direction;
    scene.sun = Some(sun);
    scene.meshes = vec![
        ground(60.0),
        cube(Vec3::new(-2.0, 0.5, 0.0), Vec3::ONE, [0.8; 4]),
    ];
    // The sun ray from a point meets the ground `y / -dir.y` along it: the cube top's far edge, and
    // the cube centre's shadow, deep inside the umbra.
    let at_ground = |from: Vec3| from + dir * (from.y / -dir.y);
    let pixel = |p: Vec3| project_pixel(&scene, Vec3::new(p.x, 0.0, p.z), (w, h));
    let tip_x = at_ground(Vec3::new(-1.5, 1.0, 0.0)).x;
    let umbra = pixel(at_ground(Vec3::new(-2.0, 0.5, 0.0)));
    let (ux0, uy0) = pixel(Vec3::new(-6.8, 0.0, -3.3));
    // A 16 texel filter at this sun reaches about 3 m along the ground, so open ground is only
    // "upwind" once the cube's lit face is out of that reach.
    let (ux1, uy1) = pixel(Vec3::new(-5.5, 0.0, 3.3));
    let (bx0, by0) = pixel(Vec3::new(-6.8, 0.0, -3.3));
    let (bx1, by1) = pixel(Vec3::new(6.8, 0.0, -1.6));
    let tip_rows = [0.7, 0.9, 1.1]
        .map(|z| pixel(Vec3::new(0.0, 0.0, z)).1)
        .to_vec();
    let (lit_from, _) = pixel(Vec3::new(tip_x + 0.25, 0.0, 0.0));
    (
        scene,
        GrazingProbes {
            upwind: (ux0, uy0, ux1, uy1),
            beside: (bx0, by0, bx1, by1),
            tip_rows,
            lit_from,
            last: w - 1,
            umbra,
        },
    )
}

/// With a grazing sun, a penumbra must not dirty lit ground: open ground upwind of and beside the
/// shadow renders the same bytes as the same scene with sun shadows off (same disc specular, no
/// shadow map), and the lit ground just past the shadow's far edge never darkens to the umbra. The
/// hard sun (θ = 0) is held to the same equality as a sanity check on the anchored path only: the
/// cascaded hard PCF is itself acned at a grazing sun (pre-existing). Anchored and cascaded paths.
#[test]
fn needs_gpu_sun_grazing_penumbra_keeps_lit_ground_clean() {
    let Some(mut renderer) = sun_renderer("sun_grazing_penumbra_keeps_lit_ground_clean", 512, 3)
    else {
        return;
    };
    let params = renderer.quality_params;
    let (w, h) = (256u32, 128u32);
    for cascaded in [false, true] {
        let mut render = |angular_radius: f32, shadows: bool| {
            renderer.set_quality(
                Quality {
                    shadows,
                    ..Quality::FULL
                },
                params,
            );
            let (scene, probes) = grazing_scene(angular_radius, cascaded, (w, h));
            let img = renderer
                .render(&scene, FrameSize::native(w, h))
                .expect("render");
            let again = renderer
                .render(&scene, FrameSize::native(w, h))
                .expect("render again");
            assert_eq!(img.rgba, again.rgba, "two renders agree byte for byte");
            (img, probes)
        };
        let soft_angle = 3.0_f32.to_radians();
        let (soft_open, _) = render(soft_angle, false);
        let (soft, probes) = render(soft_angle, true);
        let (hard_open, _) = render(0.0, false);
        let (hard, _) = render(0.0, true);
        let mut pairs = vec![("θ=3°", &soft, &soft_open)];
        if !cascaded {
            pairs.insert(0, ("θ=0", &hard, &hard_open));
        }
        for (label, shadowed, open) in pairs {
            for (name, (x0, y0, x1, y1)) in [("upwind", probes.upwind), ("beside", probes.beside)] {
                let (mut differing, mut drop) = (0, 0);
                let mut bounds = (u32::MAX, u32::MAX, 0, 0);
                for y in y0..=y1 {
                    for x in x0..=x1 {
                        let i = ((y * w + x) * 4) as usize;
                        if shadowed.rgba[i..i + 4] != open.rgba[i..i + 4] {
                            differing += 1;
                            bounds = (
                                bounds.0.min(x),
                                bounds.1.min(y),
                                bounds.2.max(x),
                                bounds.3.max(y),
                            );
                            drop = drop.max(luma(open, x, y).saturating_sub(luma(shadowed, x, y)));
                        }
                    }
                }
                assert_eq!(
                    differing, 0,
                    "cascaded={cascaded} {label}: {differing} {name} ground pixels differ from the unshadowed render (max luma drop {drop}, within {bounds:?} of the region {x0},{y0}..{x1},{y1})"
                );
            }
        }
        let umbra = luma(&soft, probes.umbra.0, probes.umbra.1);
        for &row in &probes.tip_rows {
            for x in probes.lit_from..=probes.last {
                let l = luma(&soft, x, row);
                assert!(
                    l > umbra + 2,
                    "cascaded={cascaded}: lit pixel ({x}, {row}) is umbra-dark: {l} vs umbra {umbra}"
                );
            }
        }
    }
}

/// The sun's penumbra grows with the blocker's distance when it has an angular radius: a slab 6 m up
/// casts a wider transition than one at 1 m, and both are wider than the hard PCF edge of a sun with
/// none. Anchored and cascaded paths.
#[test]
fn needs_gpu_sun_penumbra_widens_with_blocker_distance() {
    let Some(renderer) = sun_renderer("sun_penumbra_widens_with_blocker_distance", 512, 3) else {
        return;
    };
    for cascaded in [false, true] {
        let hard = penumbra_band(&renderer, 6.0, 0.0, cascaded);
        let near = penumbra_band(&renderer, 1.0, 3.0_f32.to_radians(), cascaded);
        let far = penumbra_band(&renderer, 6.0, 3.0_f32.to_radians(), cascaded);
        assert!(
            far > near && near > hard,
            "cascaded={cascaded}: band {hard} (θ=0) < {near} (1 m) < {far} (6 m)"
        );
    }
}

/// At θ = 0 the PCSS tap counts are never read: the original PCF loops run, so any counts render the
/// same bytes.
#[test]
fn needs_gpu_sun_zero_angular_radius_ignores_pcss_taps() {
    let Some(mut renderer) = sun_renderer("sun_zero_angular_radius_ignores_pcss_taps", 512, 3)
    else {
        return;
    };
    let (w, h) = (256u32, 64u32);
    for cascaded in [false, true] {
        let (scene, _) = penumbra_scene(3.0, 0.0, cascaded, (w, h));
        let mut render = |blocker: u32, filter: u32| {
            renderer.set_quality(
                Quality::FULL,
                QualityParams {
                    shadow_map_size: 512,
                    pcss_blocker_taps: blocker,
                    pcss_filter_taps: filter,
                    ..QualityParams::FULL
                },
            );
            renderer
                .render(&scene, FrameSize::native(w, h))
                .expect("render")
                .rgba
        };
        let minimal = render(1, 1);
        let full = render(
            QualityParams::FULL.pcss_blocker_taps,
            QualityParams::FULL.pcss_filter_taps,
        );
        assert_eq!(minimal, full, "cascaded={cascaded}");
    }
}

/// A penumbra narrower than the PCF fallback threshold renders exactly as the hard PCF edge: the
/// classifier and the sentinel fallback both agree with PCF. Anchored and cascaded paths.
#[test]
fn needs_gpu_sun_narrow_penumbra_falls_back_to_pcf() {
    let Some(renderer) = sun_renderer("sun_narrow_penumbra_falls_back_to_pcf", 512, 3) else {
        return;
    };
    let (w, h) = (256u32, 64u32);
    for cascaded in [false, true] {
        let render = |angular_radius: f32| {
            let (mut scene, _) = penumbra_scene(1.0, angular_radius, cascaded, (w, h));
            // A 0.1 m post in the lit part of the frame: a caster thinner than the classifier's gather
            // spacing, whose narrow shadow the PCF fallback must still render as the hard edge does.
            scene.meshes.push(cube(
                Vec3::new(0.6, 0.5, 0.0),
                Vec3::new(0.1, 1.0, 0.1),
                [0.8; 4],
            ));
            renderer
                .render(&scene, FrameSize::native(w, h))
                .expect("render")
                .rgba
        };
        let hard = render(0.0);
        let narrow = render(0.2_f32.to_radians());
        let differing = hard.iter().zip(&narrow).filter(|(a, b)| a != b).count();
        assert_eq!(
            differing, 0,
            "cascaded={cascaded}: {differing} bytes differ"
        );
    }
}

/// A wide sun saturates at `MAX_PENUMBRA_TEXELS`: the transition is at most twice the cap in shadow
/// texels wide, and 30° and 60° suns agree to within a pixel or two.
#[test]
fn needs_gpu_sun_penumbra_is_capped() {
    let Some(renderer) = sun_renderer("sun_penumbra_is_capped", 512, 3) else {
        return;
    };
    for cascaded in [false, true] {
        let wide = penumbra_band(&renderer, 6.0, 30.0_f32.to_radians(), cascaded);
        let wider = penumbra_band(&renderer, 6.0, 60.0_f32.to_radians(), cascaded);
        // A pixel is 4.2 m / 256; a shadow texel is 8 m / 512 anchored, or the fitted cascade's
        // that shades the ground 40 m out.
        let (scene, _) = penumbra_scene(6.0, 0.0, cascaded, (256, 64));
        let texel = if cascaded {
            let fit = fit_sun_cascades(&scene, &renderer.quality_params, false).expect("a fit");
            let ground = fit.cascades[..fit.count as usize]
                .iter()
                .find(|c| c.near <= 40.0 && 40.0 < c.far)
                .expect("a cascade holds the ground");
            ground.texel
        } else {
            8.0 / 512.0
        };
        let bound = (2.0 * 16.0 * texel / (4.2 / 256.0)) as u32 + 4;
        assert!(
            wide <= bound && wider <= bound,
            "cascaded={cascaded}: bands {wide}, {wider} exceed {bound} px"
        );
        assert!(
            wide.abs_diff(wider) <= 3,
            "cascaded={cascaded}: 30° gives {wide} px, 60° gives {wider} px"
        );
    }
}

/// The post settings that make a rendered byte the sRGB encoding of the linear HDR value itself.
fn exact_post() -> PostData {
    PostData {
        bloom_strength: 0.0,
        exposure: 1.0,
        tonemap: TonemapOperator::None,
        dither: false,
        ..PostData::default()
    }
}

/// The sRGB encoding of a linear value, as the `Rgba8UnormSrgb` target stores it.
fn srgb8_from_linear(linear: f32) -> f32 {
    let c = if linear <= 0.003_130_8 {
        linear * 12.92
    } else {
        1.055 * linear.powf(1.0 / 2.4) - 0.055
    };
    c * 255.0
}

/// `Environment.ambient` reaches meshes under IBL: with an all-black sky the irradiance and the
/// prefiltered specular are exactly zero, so a flat dielectric ground reads `ambient × albedo`
/// and nothing else — and a zero ambient leaves it black.
#[test]
fn needs_gpu_environment_ambient_lights_a_mesh_under_ibl() {
    let Some(renderer) = skip_or_renderer("environment_ambient_lights_a_mesh_under_ibl") else {
        return;
    };
    let (w, h) = (64u32, 64u32);
    let size = FrameSize::native(w, h);
    let mut scene = agreement_scene(w, h);
    scene.sun = None;
    scene.post = Some(exact_post());
    scene.meshes = vec![ground(40.0)];
    scene.environment = Some(crate::scene::EnvironmentData {
        sky_zenith: [0.0; 3],
        sky_horizon: [0.0; 3],
        sky_ground: [0.0; 3],
        sun_disc_size: 0.0,
        sun_disc_intensity: 0.0,
        ambient: [0.0; 3],
        ibl_intensity: 1.0,
    });

    let probe = (w / 2, h / 2 + 12);
    let dark = renderer.render(&scene, size).expect("zero-ambient frame");
    assert_eq!(
        rgb_at(&dark, probe.0, probe.1),
        [0, 0, 0],
        "a black sky and zero ambient light nothing",
    );

    scene.ambient = [0.3; 3];
    let lit = renderer.render(&scene, size).expect("ambient frame");
    let want = srgb8_from_linear(0.3 * 0.6);
    for (channel, got) in rgb_at(&lit, probe.0, probe.1).into_iter().enumerate() {
        assert!(
            (f32::from(got) - want).abs() <= 2.0,
            "channel {channel}: ambient × albedo should encode to {want:.1}, got {got}",
        );
    }
}

/// A lit sprite takes the sun's light unshadowed: a sun travelling straight into its face reads
/// the sun colour exactly (ambient 0, no points), even with an opaque cube between the sun and
/// the sprite, and a sunless frame leaves it black.
#[test]
fn needs_gpu_a_lit_sprite_takes_the_sun_unshadowed() {
    let Some(renderer) = skip_or_renderer("a_lit_sprite_takes_the_sun_unshadowed") else {
        return;
    };
    let (w, h) = (64u32, 64u32);
    let size = FrameSize::native(w, h);
    let mut scene = RenderScene::empty((w, h));
    scene.ambient = [0.0; 3];
    scene.camera = origin_camera(w, h);
    scene.post = Some(exact_post());
    scene.sprites = vec![crate::scene::SpriteInstance {
        center: Vec3::new(0.0, 0.0, -4.0),
        right: Vec3::new(3.0, 0.0, 0.0),
        up: Vec3::new(0.0, 3.0, 0.0),
        color: [1.0, 1.0, 1.0, 1.0],
        texture: None,
        uv: crate::scene::SpriteInstance::FULL_UV,
        lit: true,
        layer: 0,
        additive: false,
        flash: 0.0,
        flash_color: [1.0; 3],
        uv_repeat: crate::scene::SpriteInstance::NO_TILING,
        uv_offset: [0.0; 2],
        medium: 1.0,
    }];
    // Behind the camera, so it is never drawn, yet between the sun and the sprite.
    scene.meshes = vec![cube(
        Vec3::new(0.0, 0.0, 1.0),
        Vec3::splat(8.0),
        [0.5, 0.5, 0.5, 1.0],
    )];

    let probe = (w / 2, h / 2);
    let sunless = renderer.render(&scene, size).expect("sunless frame");
    assert_eq!(rgb_at(&sunless, probe.0, probe.1), [0, 0, 0]);

    scene.sun = Some(test_sun(
        Vec3::new(0.0, 0.0, -1.0),
        Vec3::new(0.5, 0.25, 0.1),
        0.002,
    ));
    let sunlit = renderer.render(&scene, size).expect("sunlit frame");
    for (channel, (got, linear)) in rgb_at(&sunlit, probe.0, probe.1)
        .into_iter()
        .zip([0.5, 0.25, 0.1])
        .enumerate()
    {
        let want = srgb8_from_linear(linear);
        assert!(
            (f32::from(got) - want).abs() <= 2.0,
            "channel {channel}: the sun should encode to {want:.1}, got {got}",
        );
    }
}

/// A night-sky `EnvironmentData` for the IBL tests: an authored moonlit gradient with a modest
/// IBL intensity. The disc fields are irrelevant to the prefilter (only the sky colors are).
fn test_env() -> crate::scene::EnvironmentData {
    crate::scene::EnvironmentData {
        sky_zenith: [0.02, 0.03, 0.08],
        sky_horizon: [0.08, 0.11, 0.18],
        sky_ground: [0.01, 0.012, 0.02],
        sun_disc_size: 4.0,
        sun_disc_intensity: 6.0,
        ambient: [0.05, 0.05, 0.06],
        ibl_intensity: 1.9,
    }
}

/// IBL is real and deterministic: (a) an `Environment` changes the rendered pixels versus the same
/// scene without one (metals reflect the prefiltered sky instead of going black under flat
/// ambient), and (b) two **independent** renderers — each running the prefilter compute from
/// scratch, no shared cache — produce byte-identical frames, so the prefilter + sky + IBL path is
/// as deterministic as the rest on a pinned adapter. Skips with no GPU.
#[test]
fn needs_gpu_environment_ibl_changes_pixels_and_prefilter_is_deterministic() {
    let Some(r1) =
        skip_or_renderer("environment_ibl_changes_pixels_and_prefilter_is_deterministic")
    else {
        return;
    };
    // A second, fully independent renderer (its own IBL prefilter run) for the determinism half.
    let r2 = Renderer::new().expect("a second renderer initializes once the first did");

    let (w, h) = (96u32, 64u32);
    let mut scene = RenderScene::empty((w, h));
    scene.meshes = vec![
        ground(20.0),
        // A near-mirror metal sphere — the surface IBL affects most (it reflects the sky).
        MeshInstance {
            medium: 1.0,
            shape: MeshShape::Sphere,
            model: Mat4::from_translation(Vec3::new(0.0, 1.2, 0.0)),
            normal_matrix: Mat3::IDENTITY,
            color: [0.95, 0.8, 0.42, 1.0],
            material: [0.15, 1.0, 0.0],
            use_vertex_material: false,
            fade: 1.0,
            fade_height_min: -f32::MAX,
            flash: 0.0,
            flash_color: [1.0; 3],
            wrap: 0.0,
            scatter_color: [1.0; 3],
            bone_offset: 0,
            cast_shadows: true,
            viewmodel: false,
            rim: [0.0; 4],
        },
    ];

    let without = r1
        .render(&scene, FrameSize::native(w, h))
        .expect("env-less render");
    scene.environment = Some(test_env());
    let with_a = r1
        .render(&scene, FrameSize::native(w, h))
        .expect("env render (renderer 1)");
    let with_b = r2
        .render(&scene, FrameSize::native(w, h))
        .expect("env render (renderer 2)");

    assert_ne!(
        without.rgba, with_a.rgba,
        "an Environment must change the image (sky + IBL are live)"
    );
    assert_eq!(
        with_a.rgba, with_b.rgba,
        "two independent prefilter runs of the same environment must be byte-identical"
    );
}

/// A unit-radius sphere at the origin with the given material and hit-flash amount.
fn reflective_sphere(material: [f32; 3], flash: f32) -> MeshInstance {
    MeshInstance {
        medium: 1.0,
        shape: MeshShape::Sphere,
        model: Mat4::from_scale(Vec3::splat(2.0)),
        normal_matrix: Mat3::IDENTITY,
        color: [0.9, 0.9, 0.9, 1.0],
        material,
        use_vertex_material: false,
        fade: 1.0,
        fade_height_min: -f32::MAX,
        flash,
        flash_color: [0.2, 0.3, 0.4],
        wrap: 0.0,
        scatter_color: [1.0; 3],
        bone_offset: 0,
        cast_shadows: true,
        viewmodel: false,
        rim: [0.0; 4],
    }
}

/// A bright, saturated sky, so a sky reflection cannot be mistaken for anything else.
fn vivid_env(ibl_intensity: f32) -> crate::scene::EnvironmentData {
    crate::scene::EnvironmentData {
        sky_zenith: [0.1, 0.9, 0.2],
        sky_horizon: [0.1, 0.9, 0.2],
        sky_ground: [0.1, 0.9, 0.2],
        sun_disc_size: 0.0,
        sun_disc_intensity: 0.0,
        ambient: [0.0; 3],
        ibl_intensity,
    }
}

fn pixel(img: &RenderedImage, x: u32, y: u32) -> [u8; 4] {
    let i = ((y * img.width + x) * 4) as usize;
    [
        img.rgba[i],
        img.rgba[i + 1],
        img.rgba[i + 2],
        img.rgba[i + 3],
    ]
}

/// The reflections pass is masked at far depth: the sky is drawn later, so a texel no opaque
/// surface covers must not receive the sky's specular term (which a zero F0 still leaves as
/// `prefiltered × ab.y`). A rough metal box in the lower half changes nothing in the upper half.
/// A sky texel's zero normal already yields 0 (`normalize` gives NaN and `hdr_safe` reads it as
/// zero), so this guards the behaviour rather than the mask line itself.
#[test]
fn needs_gpu_reflections_mask_the_sky() {
    let Some(renderer) = skip_or_renderer("reflections_mask_the_sky") else {
        return;
    };
    let (w, h) = (64u32, 64u32);
    let size = FrameSize::native(w, h);
    let mut scene = RenderScene::empty((w, h));
    scene.post = Some(exact_post());
    scene.environment = Some(test_env());
    let eye = Vec3::new(0.0, 1.0, 8.0);
    scene.camera = crate::scene::RenderCamera {
        view: Mat4::look_at_rh(eye, Vec3::new(0.0, 1.0, 0.0), Vec3::Y),
        projection: Mat4::perspective_rh(60.0_f32.to_radians(), 1.0, 0.1, 100.0),
        position: eye,
    };
    let sky_only = renderer.render(&scene, size).expect("sky-only render");
    let mut metal_box = cube(
        Vec3::new(0.0, -0.5, 0.0),
        Vec3::new(2.0, 1.0, 2.0),
        [0.8; 4],
    );
    metal_box.material = [0.5, 1.0, 0.0];
    scene.meshes = vec![metal_box];
    let with_box = renderer.render(&scene, size).expect("box render");

    assert_ne!(
        sky_only.rgba, with_box.rgba,
        "the box must be visible, or the comparison proves nothing"
    );
    for y in 0..h / 2 {
        for x in 0..w {
            assert_eq!(
                pixel(&sky_only, x, y),
                pixel(&with_box, x, y),
                "sky pixel ({x}, {y}) moved when a box entered the lower half",
            );
        }
    }
}

/// A full hit-flash hides the reflection: the flash factor rides `scene_specular.a`, so
/// `mix(lit + s, flash, a)` stays exact. A mirror metal and a rough dielectric flashed to 1.0
/// render the same centre pixel.
#[test]
fn needs_gpu_a_full_flash_hides_the_reflection() {
    let Some(renderer) = skip_or_renderer("a_full_flash_hides_the_reflection") else {
        return;
    };
    let (w, h) = (64u32, 64u32);
    let size = FrameSize::native(w, h);
    let mut scene = RenderScene::empty((w, h));
    scene.post = Some(exact_post());
    scene.environment = Some(vivid_env(1.0));
    scene.meshes = vec![reflective_sphere([0.05, 1.0, 0.0], 1.0)];
    let mirror = renderer.render(&scene, size).expect("mirror render");
    scene.meshes = vec![reflective_sphere([0.9, 0.0, 0.0], 1.0)];
    let rough = renderer.render(&scene, size).expect("rough render");
    assert_eq!(
        pixel(&mirror, w / 2, h / 2),
        pixel(&rough, w / 2, h / 2),
        "a fully flashed pixel shows the flash colour alone"
    );
}

// --- Wrap diffuse (Mesh.wrap) -------------------------------------------------------------

fn wrap_mesh(
    shape: MeshShape,
    model: Mat4,
    wrap: f32,
    scatter: [f32; 3],
    cast: bool,
) -> MeshInstance {
    MeshInstance {
        medium: 1.0,
        shape,
        model,
        normal_matrix: Mat3::from_mat4(model.inverse().transpose()),
        color: [0.9, 0.9, 0.9, 1.0],
        material: [1.0, 0.0, 0.0],
        use_vertex_material: false,
        fade: 1.0,
        fade_height_min: -f32::MAX,
        flash: 0.0,
        flash_color: [1.0; 3],
        wrap,
        scatter_color: scatter,
        bone_offset: 0,
        cast_shadows: cast,
        viewmodel: false,
        rim: [0.0; 4],
    }
}

fn wrap_sphere(wrap: f32, scatter: [f32; 3], cast: bool) -> MeshInstance {
    wrap_mesh(
        MeshShape::Sphere,
        Mat4::from_scale(Vec3::splat(2.0)),
        wrap,
        scatter,
        cast,
    )
}

/// A 64×64 frame of one sphere at the origin seen from +z, lit only from +x, so the left half is
/// past the terminator and a point's screen column follows its `n.x`.
fn wrap_scene(sun: bool) -> RenderScene {
    let mut scene = RenderScene::empty((64, 64));
    scene.ambient = [0.0; 3];
    scene.post = Some(exact_post());
    let eye = Vec3::new(0.0, 0.0, 6.0);
    scene.camera = crate::scene::RenderCamera {
        view: Mat4::look_at_rh(eye, Vec3::ZERO, Vec3::Y),
        projection: Mat4::perspective_rh(40.0_f32.to_radians(), 1.0, 0.1, 100.0),
        position: eye,
    };
    if sun {
        scene.sun = Some(test_sun(Vec3::NEG_X, Vec3::ONE, 0.002));
    }
    scene
}

/// A dark-side pixel at about n.x = -0.3: inside a wrap-0.5 band, past plain Lambert.
const WRAP_BAND_PIXEL: (u32, u32) = (28, 32);

/// A zero wrap is render-identical whatever the scatter colour: the band is never evaluated.
#[test]
fn needs_gpu_wrap_zero_ignores_the_scatter_colour() {
    let Some(renderer) = skip_or_renderer("wrap_zero_ignores_the_scatter_colour") else {
        return;
    };
    let size = FrameSize::native(64, 64);
    let mut scene = wrap_scene(true);
    scene.meshes = vec![wrap_sphere(0.0, [1.0; 3], true)];
    let white = renderer.render(&scene, size).expect("white");
    scene.meshes = vec![wrap_sphere(0.0, [1.0, 0.2, 0.1], true)];
    let red = renderer.render(&scene, size).expect("red");
    assert_eq!(white.rgba, red.rgba);
}

/// Wrap lights the dark side past the terminator, more with more wrap; an authored vertex material
/// wraps too.
#[test]
fn needs_gpu_wrap_softens_the_terminator() {
    let Some(renderer) = skip_or_renderer("wrap_softens_the_terminator") else {
        return;
    };
    let size = FrameSize::native(64, 64);
    let mut scene = wrap_scene(true);
    let at = |scene: &mut RenderScene, mesh: MeshInstance| {
        scene.meshes = vec![mesh];
        let img = renderer.render(scene, size).expect("render");
        luma(&img, WRAP_BAND_PIXEL.0, WRAP_BAND_PIXEL.1)
    };
    let none = at(&mut scene, wrap_sphere(0.0, [1.0; 3], true));
    let half = at(&mut scene, wrap_sphere(0.5, [1.0; 3], true));
    let full = at(&mut scene, wrap_sphere(1.0, [1.0; 3], true));
    assert_eq!(none, 0, "plain Lambert is black past the terminator");
    assert!(half > 0, "wrap 0.5 lights it: {half}");
    assert!(full > half, "wrap 1 lights it more: {half} -> {full}");
    let authored = MeshInstance {
        use_vertex_material: true,
        ..wrap_sphere(1.0, [1.0; 3], true)
    };
    assert!(at(&mut scene, authored) > 0, "an authored material wraps");
}

/// The scatter colour tints the *total* wrapped diffuse per channel, so a zero channel still gets
/// light near the terminator: past it, red scatter gives R > G = B > 0 with G following `x^2`; and the
/// near-lit rim, where the wrapped cosine is almost the plain one, barely moves.
#[test]
fn needs_gpu_wrap_scatter_colour_follows_the_total_diffuse_formula() {
    let Some(renderer) = skip_or_renderer("wrap_scatter_colour_follows_the_total_diffuse_formula")
    else {
        return;
    };
    let size = FrameSize::native(64, 64);
    let mut scene = wrap_scene(true);
    scene.meshes = vec![wrap_sphere(1.0, [1.0, 0.0, 0.0], true)];
    let tinted = renderer.render(&scene, size).expect("tinted");
    scene.meshes = vec![wrap_sphere(0.0, [1.0; 3], true)];
    let plain = renderer.render(&scene, size).expect("plain");
    let band = rgb_at(&tinted, WRAP_BAND_PIXEL.0, WRAP_BAND_PIXEL.1);
    // Past the terminator at wrap 1 the total diffuse per channel is `x * mix(scatter, 1, x)`
    // with `x` the wrapped cosine: red `x`, green and blue `x^2`. A zero scatter channel keeps
    // light, so the band is not pure red.
    let linear = |v: u8| ((f32::from(v) / 255.0 + 0.055) / 1.055).powf(2.4);
    let albedo = 0.9;
    assert!(
        band[0] > band[1] && band[1] == band[2] && band[1] > 0,
        "{band:?}"
    );
    let expected_green = srgb8_from_linear(linear(band[0]).powi(2) / albedo);
    assert!(
        (f32::from(band[1]) - expected_green).abs() <= 3.0,
        "green {} should follow the formula {expected_green}",
        band[1]
    );
    let rim = (45, 32);
    let (a, b) = (rgb_at(&tinted, rim.0, rim.1), rgb_at(&plain, rim.0, rim.1));
    for c in 0..3 {
        assert!(
            a[c].abs_diff(b[c]) <= 12,
            "the lit rim barely moves: {a:?} vs {b:?}"
        );
    }
}

/// The wrap data rides unread lanes of the instance's normal matrix, so a wrapped mesh must shade
/// with the same normals as an unwrapped one under a non-uniform scale (where the normal matrix is
/// not the model's). A `wrap` of 1e-30 takes the band path with the lanes packed but adds exactly
/// zero light (`(n.l + 1e-30) / (1 + 1e-30)` is `n.l` in f32), so the lit frames must be
/// byte-identical to `wrap: 0`. The normal target itself has no debug view; this reads the normal
/// through the sun shading of a metallic-free sphere and a rotated box.
#[test]
fn needs_gpu_wrap_lanes_leave_normals_unchanged_under_non_uniform_scale() {
    let Some(renderer) = skip_or_renderer("wrap_lanes_leave_normals_unchanged") else {
        return;
    };
    let size = FrameSize::native(64, 64);
    let squash = Mat4::from_rotation_y(35.0_f32.to_radians())
        * Mat4::from_rotation_x(20.0_f32.to_radians())
        * Mat4::from_scale(Vec3::new(2.4, 0.7, 1.6));
    for shape in [MeshShape::Sphere, MeshShape::Cube] {
        let render = |wrap: f32| {
            let mut scene = wrap_scene(true);
            scene.ambient = [0.05; 3];
            scene.meshes = vec![wrap_mesh(shape, squash, wrap, [0.2, 0.5, 1.0], true)];
            renderer.render(&scene, size).expect("render")
        };
        let plain = render(0.0);
        let packed = render(1e-30);
        assert!(
            plain.rgba.iter().any(|&b| b != plain.rgba[0]),
            "the shape shows"
        );
        assert_eq!(
            plain.rgba, packed.rgba,
            "{shape:?}: packed lanes moved a pixel"
        );
    }
}

/// Lights for the shadowed-receiver tests: 0 the sun, 1 a shadowed spot, 2 a shadowed point light.
fn wrap_light_scene(kind: u8) -> RenderScene {
    let mut scene = wrap_scene(kind == 0);
    let shadow = Some(crate::scene::LocalShadowData {
        bias: 0.02,
        near: 0.1,
        distance: None,
        fade: 0.0,
    });
    match kind {
        1 => {
            let mut spot = spot_light(Vec3::new(4.0, 0.0, 0.0), Vec3::NEG_X, 6.0, 12.0, 40.0, 60.0);
            spot.shadow = shadow;
            scene.spot_lights = vec![spot];
        }
        2 => {
            let mut light = point_light(Vec3::new(4.0, 0.0, 0.0), 6.0, 12.0);
            light.shadow = shadow;
            scene.lights = vec![light];
        }
        _ => {}
    }
    scene
}

/// The band is not eaten by the object's own shadow: a caster and a non-caster render the same
/// band to within a few levels, for the sun, a shadowed spot and a shadowed point light, on a
/// sphere and a rotated box; and across the sun-lit sphere the luma never notches on the way into
/// the dark.
#[test]
fn needs_gpu_wrap_band_has_no_shadow_acne_or_notch() {
    let Some(renderer) = skip_or_renderer("wrap_band_has_no_shadow_acne_or_notch") else {
        return;
    };
    let size = FrameSize::native(64, 64);
    let box_model =
        Mat4::from_rotation_y(30.0_f32.to_radians()) * Mat4::from_scale(Vec3::splat(1.6));
    for kind in 0..3u8 {
        for boxed in [false, true] {
            let make = |cast: bool| {
                if boxed {
                    wrap_mesh(MeshShape::Cube, box_model, 1.0, [1.0; 3], cast)
                } else {
                    wrap_sphere(1.0, [1.0; 3], cast)
                }
            };
            let mut scene = wrap_light_scene(kind);
            scene.meshes = vec![make(true)];
            let casting = renderer.render(&scene, size).expect("casting");
            scene.meshes = vec![make(false)];
            let free = renderer.render(&scene, size).expect("non-casting");
            let gap = max_channel_diff(&casting, &free, 8..56, 8..56);
            assert!(
                gap <= 6,
                "light {kind} boxed {boxed}: caster differs by {gap}"
            );
            if kind == 0 && !boxed {
                let row: Vec<u32> = (20..45).map(|x| luma(&casting, x, 32)).collect();
                for pair in row.windows(2) {
                    assert!(
                        pair[1] + 6 >= pair[0],
                        "a dark notch in the sun row: {row:?}"
                    );
                }
            }
        }
    }
}

/// A distant caster between the sun and the sphere shades the band: with the wall up, the band is
/// as dark as plain Lambert.
#[test]
fn needs_gpu_wrap_band_is_shadowed_by_a_distant_caster() {
    let Some(renderer) = skip_or_renderer("wrap_band_is_shadowed_by_a_distant_caster") else {
        return;
    };
    let size = FrameSize::native(64, 64);
    let mut scene = wrap_scene(true);
    let wall = cube(
        Vec3::new(6.0, 0.0, 0.0),
        Vec3::new(0.5, 10.0, 10.0),
        [0.5; 4],
    );
    scene.meshes = vec![wrap_sphere(1.0, [1.0; 3], false), wall];
    let img = renderer.render(&scene, size).expect("render");
    let l = luma(&img, WRAP_BAND_PIXEL.0, WRAP_BAND_PIXEL.1);
    assert!(
        l <= 3,
        "the band must be shaded behind a distant caster: {l}"
    );
}

/// The pass adds the term at all: a mirror metal under a vivid sky is clearly brighter at its
/// centre than the same sphere with `ibl_intensity` 0.
#[test]
fn needs_gpu_metal_reflects_the_sky() {
    let Some(renderer) = skip_or_renderer("metal_reflects_the_sky") else {
        return;
    };
    let (w, h) = (64u32, 64u32);
    let size = FrameSize::native(w, h);
    let mut scene = RenderScene::empty((w, h));
    scene.post = Some(exact_post());
    scene.meshes = vec![reflective_sphere([0.05, 1.0, 0.0], 0.0)];
    scene.environment = Some(vivid_env(1.0));
    let lit = renderer.render(&scene, size).expect("reflecting render");
    scene.environment = Some(vivid_env(0.0));
    let dark = renderer.render(&scene, size).expect("ibl-off render");
    let centre = |img: &RenderedImage| {
        let p = pixel(img, w / 2, h / 2);
        u32::from(p[0]) + u32::from(p[1]) + u32::from(p[2])
    };
    assert!(
        centre(&lit) > centre(&dark) + 60,
        "the sky must show in the metal: {} vs {}",
        centre(&lit),
        centre(&dark),
    );
}

/// An orthographic camera at `eye` looking at `target`, `half_h` tall and `aspect` wide.
fn ortho_camera(eye: Vec3, target: Vec3, half_h: f32, aspect: f32) -> crate::scene::RenderCamera {
    crate::scene::RenderCamera {
        view: Mat4::look_at_rh(eye, target, Vec3::Y),
        projection: Mat4::orthographic_rh(
            -half_h * aspect,
            half_h * aspect,
            -half_h,
            half_h,
            0.1,
            20.0,
        ),
        position: eye,
    }
}

/// The largest per-channel gap between any pixel of `img` (RGB) and its pixel at (`x0`, `y0`).
fn max_gap_from(img: &RenderedImage, x0: u32, y0: u32) -> i32 {
    let reference = pixel(img, x0, y0);
    let mut worst = 0;
    for y in 0..img.height {
        for x in 0..img.width {
            let p = pixel(img, x, y);
            for c in 0..3 {
                worst = worst.max((i32::from(p[c]) - i32::from(reference[c])).abs());
            }
        }
    }
    worst
}

fn contrast_env(ibl_intensity: f32) -> crate::scene::EnvironmentData {
    crate::scene::EnvironmentData {
        sky_zenith: [0.9, 0.1, 0.1],
        sky_horizon: [0.1, 0.9, 0.1],
        sky_ground: [0.1, 0.1, 0.9],
        sun_disc_size: 0.0,
        sun_disc_intensity: 0.0,
        ambient: [0.0; 3],
        ibl_intensity,
    }
}

/// Under an orthographic camera every ray is parallel, so a flat plane with a sun mirrored about
/// the view direction has one specular value across the whole frame.
#[test]
fn needs_gpu_ortho_specular_is_uniform_on_a_flat_plane() {
    let Some(renderer) = skip_or_renderer("ortho_specular_is_uniform_on_a_flat_plane") else {
        return;
    };
    let (w, h) = (64u32, 64u32);
    let mut scene = RenderScene::empty((w, h));
    scene.post = Some(exact_post());
    scene.ambient = [0.0; 3];
    scene.camera = ortho_camera(Vec3::new(0.0, 6.0, 6.0), Vec3::ZERO, 8.0, 1.0);
    // The light travels the mirror of the view direction about +Y: the half vector is the normal.
    scene.sun = Some(test_sun(Vec3::new(0.0, -1.0, 1.0), Vec3::splat(1.0), 0.002));
    let mut plane = ground(50.0);
    plane.material = [0.15, 0.0, 0.0];
    plane.color = [0.02, 0.02, 0.02, 1.0];
    scene.meshes = vec![plane];
    let image = renderer
        .render(&scene, FrameSize::native(w, h))
        .expect("ortho specular render");
    let gap = max_gap_from(&image, w / 2, h / 2);
    assert!(
        gap <= 2,
        "the specular term must not vary across an orthographic plane: worst gap {gap} LSB"
    );
}

/// The sky behind an orthographic camera is one colour: its rays do not fan out.
#[test]
fn needs_gpu_ortho_sky_does_not_bend() {
    let Some(renderer) = skip_or_renderer("ortho_sky_does_not_bend") else {
        return;
    };
    let (w, h) = (64u32, 64u32);
    let mut scene = RenderScene::empty((w, h));
    scene.post = Some(exact_post());
    scene.environment = Some(contrast_env(0.0));
    scene.camera = ortho_camera(Vec3::new(0.0, 1.0, 8.0), Vec3::new(0.0, 1.0, 0.0), 8.0, 1.0);
    let image = renderer
        .render(&scene, FrameSize::native(w, h))
        .expect("ortho sky render");
    let gap = max_gap_from(&image, w / 2, 0);
    assert!(
        gap <= 1,
        "an orthographic sky must be one colour: worst gap {gap} LSB"
    );
}

/// A metal plane under an orthographic camera reflects one sky direction everywhere.
#[test]
fn needs_gpu_ortho_sky_reflection_is_uniform_on_a_flat_plane() {
    let Some(renderer) = skip_or_renderer("ortho_sky_reflection_is_uniform_on_a_flat_plane") else {
        return;
    };
    let (w, h) = (64u32, 64u32);
    let mut scene = RenderScene::empty((w, h));
    scene.post = Some(exact_post());
    scene.environment = Some(contrast_env(1.0));
    scene.camera = ortho_camera(Vec3::new(0.0, 6.0, 6.0), Vec3::ZERO, 8.0, 1.0);
    let mut plane = ground(50.0);
    plane.material = [0.2, 1.0, 0.0];
    scene.meshes = vec![plane];
    let image = renderer
        .render(&scene, FrameSize::native(w, h))
        .expect("ortho metal render");
    let gap = max_gap_from(&image, w / 2, h / 2);
    assert!(
        gap <= 2,
        "the sky reflection must not vary across an orthographic plane: worst gap {gap} LSB"
    );
}

/// Under an orthographic camera SSR reflects along one direction, so a wall wide along X is
/// mirrored with a straight edge: the lowest red row is the same in every column.
#[test]
fn needs_gpu_ortho_ssr_reflects_along_one_direction() {
    let Some(renderer) = skip_or_renderer("ortho_ssr_reflects_along_one_direction") else {
        return;
    };
    let size = FrameSize::native(96, 64);
    let mut scene = RenderScene::empty(size.render);
    scene.post = Some(PostData {
        ssr: 1.0,
        ..exact_post()
    });
    scene.environment = Some(test_env());
    scene.camera = ortho_camera(Vec3::new(0.0, 6.0, 6.0), Vec3::ZERO, 8.0, 1.5);
    let mut floor = ground(50.0);
    floor.material = [0.02, 1.0, 0.0];
    let mut wall = cube(
        Vec3::new(0.0, 0.75, -1.0),
        Vec3::new(40.0, 1.5, 0.5),
        [1.0, 0.0, 0.0, 1.0],
    );
    wall.material = [0.5, 0.0, 1.0];
    scene.meshes = vec![floor, wall];
    let chain = PostChain {
        ssr: Some(QualityParams::FULL.ssr_scale),
        ..PostChain::for_scene(&scene).masked(&renderer.quality, &renderer.quality_params)
    };
    let (graph, output) = build_frame_graph(size, chain);
    let scheduled = schedule(&graph).expect("the ssr graph schedules");
    let recorder = renderer.prepare_frame(&scene, size, None).unwrap();
    let image = renderer
        .gpu
        .execute(&scheduled, output, &recorder)
        .expect("the ssr frame renders");
    assert!(renderer.gpu.take_fault().is_none(), "the ssr frame faulted");

    let (w, h) = size.output;
    let mut lowest = Vec::new();
    for x in 8..w - 8 {
        let row = (0..h).rev().find(|&y| {
            let p = pixel(&image, x, y);
            i32::from(p[0]) - i32::from(p[2]) > 40
        });
        lowest.push(row.expect("the wall or its reflection is red in every column"));
    }
    let (lo, hi) = (*lowest.iter().min().unwrap(), *lowest.iter().max().unwrap());
    assert!(
        hi - lo <= 1,
        "the reflected wall's edge must be one row in every column: rows {lo}..{hi}"
    );
}

/// (b) A caster darkens the receiver: a wide slab held between a straight-down sun and the ground
/// shadows the entire visible ground, so a near-ground pixel is measurably darker than the same
/// pixel with the slab removed (both frames keep the sun; only the occluder differs). Proves the
/// depth pass wrote the shadow map and the PCF term sampled it.
#[test]
fn needs_gpu_occluder_shadows_the_receiver() {
    let Some(renderer) = skip_or_renderer("occluder_shadows_the_receiver") else {
        return;
    };
    let (w, h) = (64u32, 64u32);
    let mut scene = RenderScene::empty((w, h));
    scene.ambient = [0.05, 0.05, 0.05];
    // A low camera looking forward and slightly down at the ground in front of it — the slab
    // (high above) stays out of frame while its shadow lands on the visible ground.
    scene.camera = crate::scene::RenderCamera {
        view: Mat4::look_at_rh(Vec3::new(0.0, 3.0, 11.0), Vec3::new(0.0, 0.0, 0.0), Vec3::Y),
        projection: Mat4::perspective_rh(60.0_f32.to_radians(), w as f32 / h as f32, 0.1, 100.0),
        position: Vec3::new(0.0, 3.0, 11.0),
    };
    scene.sun = Some(test_sun(
        Vec3::new(0.0, -1.0, 0.0),
        Vec3::new(3.0, 3.0, 3.0),
        0.002,
    ));

    scene.meshes = vec![ground(20.0)];
    let lit = renderer
        .render(&scene, FrameSize::native(w, h))
        .expect("lit render");

    // A wide, thin slab far above the ground — its straight-down shadow covers all visible ground.
    scene.meshes = vec![
        ground(20.0),
        cube(
            Vec3::new(0.0, 8.0, 0.0),
            Vec3::new(40.0, 0.4, 40.0),
            [0.4, 0.4, 0.4, 1.0],
        ),
    ];
    let shadowed = renderer
        .render(&scene, FrameSize::native(w, h))
        .expect("shadowed render");

    // A near-ground pixel (bottom-centre — the ground just in front of the camera).
    let (px, py) = (w / 2, h - 4);
    let lit_luma = luma(&lit, px, py);
    let shadow_luma = luma(&shadowed, px, py);
    assert!(
        shadow_luma + 40 < lit_luma,
        "the occluder's shadow darkens the ground: lit={lit_luma} shadowed={shadow_luma}",
    );
}

/// A ground marker **draws, stays inside its own footprint, and reads under what stands on it**,
/// from a straight-down camera over a floor: (a) a disc changes the pixels a marker-free frame
/// has, so the analytic coverage is real; (b) a pixel well outside the disc's radius is
/// byte-identical either way — the shape is the SDF's, not the padded quad's; (c) a slab hung
/// over the marker makes the frame byte-identical to the same slab with no marker at all, which
/// is `depth_state(false, Less)` doing exactly its job (occluded by geometry, occluding nothing);
/// (d) the marked scene renders bit-identically twice. Skips with no GPU.
#[test]
fn needs_gpu_a_ground_marker_draws_bounded_and_occluded() {
    let Some(renderer) = skip_or_renderer("a_ground_marker_draws_bounded_and_occluded") else {
        return;
    };
    let (w, h) = (64u32, 64u32);
    let size = FrameSize::native(w, h);
    let eye = Vec3::new(0.0, 12.0, 0.0);

    let mut scene = RenderScene::empty((w, h));
    scene.ambient = [0.4, 0.4, 0.4];
    // Straight down at the origin; +Z is "up" on screen because the view direction is −Y.
    scene.camera = crate::scene::RenderCamera {
        view: Mat4::look_at_rh(eye, Vec3::ZERO, Vec3::Z),
        projection: Mat4::perspective_rh(60.0_f32.to_radians(), w as f32 / h as f32, 0.1, 100.0),
        position: eye,
    };
    scene.meshes = vec![ground(20.0)];
    let bare = renderer.render(&scene, size).expect("marker-free render");

    // A disc well inside the frame: at this framing the visible half-extent on the floor is
    // 12·tan(30°) ≈ 6.93 units, so radius 3 covers the middle and leaves the corners untouched.
    // Kept under the default bloom threshold so (b) measures the marker's own footprint rather
    // than the glow a bright one would legitimately spread.
    scene
        .ground_markers
        .push(crate::scene::GroundMarkerInstance {
            shape: crate::scene::GroundMarkerShape::Disc { radius: 3.0 },
            model: Mat4::from_translation(Vec3::new(0.0, crate::GROUND_MARKER_LIFT, 0.0)),
            color: [0.9, 0.15, 0.1, 1.0],
            inner_color: [0.9, 0.15, 0.1, 1.0],
            style: crate::scene::GroundMarkerStyle::default(),
            flicker_seed: 0,
            clip: None,
        });
    let marked = renderer.render(&scene, size).expect("marked render");

    // (a) the marker draws …
    assert_ne!(
        marked.rgba, bare.rgba,
        "a ground marker must change the pixels it covers",
    );
    // … over the centre specifically, where the disc is.
    assert_ne!(
        luma(&marked, w / 2, h / 2),
        luma(&bare, w / 2, h / 2),
        "the disc must cover the point it is centred on",
    );
    // (b) and nowhere else: the frame corner is ~9.8 units out, far beyond the radius.
    let corner = |img: &RenderedImage| {
        let i = ((2 * img.width + 2) * 4) as usize;
        img.rgba[i..i + 4].to_vec()
    };
    assert_eq!(
        corner(&marked),
        corner(&bare),
        "a pixel outside the disc's radius must be untouched",
    );

    // (c) a slab hung over the disc occludes it whole: wider than the disc, so the marked frame
    // and the unmarked one are byte-identical rather than merely similar.
    let slab = cube(
        Vec3::new(0.0, 2.5, 0.0),
        Vec3::new(8.0, 1.0, 8.0),
        [0.5, 0.5, 0.55, 1.0],
    );
    scene.meshes = vec![ground(20.0), slab];
    let covered_marked = renderer
        .render(&scene, size)
        .expect("covered marked render");
    scene.ground_markers.clear();
    let covered_bare = renderer.render(&scene, size).expect("covered bare render");
    assert_eq!(
        covered_marked.rgba, covered_bare.rgba,
        "a marker under geometry must be fully occluded — depth-tested, never depth-written",
    );

    // (d) determinism: the same marked scene twice.
    scene.meshes = vec![ground(20.0)];
    scene
        .ground_markers
        .push(crate::scene::GroundMarkerInstance {
            shape: crate::scene::GroundMarkerShape::Segment {
                length: 7.0,
                width: 1.5,
            },
            model: Mat4::from_translation(Vec3::new(0.0, crate::GROUND_MARKER_LIFT, 0.0)),
            color: [0.2, 0.6, 0.9, 0.8],
            inner_color: [0.2, 0.6, 0.9, 0.8],
            style: crate::scene::GroundMarkerStyle::default(),
            flicker_seed: 0,
            clip: None,
        });
    let once = renderer.render(&scene, size).expect("segment render");
    let twice = renderer.render(&scene, size).expect("segment render again");
    assert_eq!(once.rgba, twice.rgba, "marker rendering is deterministic");
    assert_ne!(
        once.rgba, bare.rgba,
        "the segment must draw as much as the disc did",
    );
}

/// The per-instance fade lane drives a deterministic screen-door (Bayer) discard on the opaque
/// mesh pass, proven four ways: (a) `fade == 1.0` (the default) is byte-identical to the pre-fade
/// frame — covered by every existing golden staying blessed, so the positive control here is just
/// that a solid cube is visible; (b) `fade == 0.0` discards *every* fragment, so the frame equals
/// omitting the instance entirely (no color and no depth written); (c) `height_min` gates the
/// discard by world height — only fragments above it fade, so a mid-height threshold leaves the
/// lower body opaque and the frame matches neither the solid nor the fully-faded cube; (d) a
/// partial fade renders bit-identically twice (the dither is a pure function of pixel position and
/// fade, with no temporal component). Skips with no GPU.
#[test]
fn needs_gpu_fade_dithers_deterministically_and_gates_by_height() {
    let Some(renderer) = skip_or_renderer("fade_dithers_deterministically_and_gates_by_height")
    else {
        return;
    };
    let (w, h) = (96u32, 96u32);

    // An emissive cube spanning y ∈ [−1, 1] at the origin — bright regardless of lighting, so its
    // pixels are unambiguously distinct from the clear color and the Bayer holes read clearly.
    let make = |fade: f32, height_min: f32| MeshInstance {
        medium: 1.0,
        shape: MeshShape::Cube,
        model: Mat4::from_scale(Vec3::splat(2.0)),
        normal_matrix: Mat3::IDENTITY,
        color: [0.9, 0.3, 0.2, 1.0],
        material: [1.0, 0.0, 1.0],
        use_vertex_material: false,
        fade,
        fade_height_min: height_min,
        flash: 0.0,
        flash_color: [1.0; 3],
        wrap: 0.0,
        scatter_color: [1.0; 3],
        bone_offset: 0,
        cast_shadows: true,
        viewmodel: false,
        rim: [0.0; 4],
    };

    let mut scene = RenderScene::empty((w, h));
    let background = renderer
        .render(&scene, FrameSize::native(w, h))
        .expect("background (no cube) render");

    scene.meshes = vec![make(1.0, -f32::MAX)];
    let solid = renderer
        .render(&scene, FrameSize::native(w, h))
        .expect("solid render");
    assert_ne!(
        background.rgba, solid.rgba,
        "a solid (fade 1.0) cube must be visible"
    );

    scene.meshes = vec![make(0.0, -f32::MAX)];
    let faded = renderer
        .render(&scene, FrameSize::native(w, h))
        .expect("fully-faded render");
    assert_eq!(
        background.rgba, faded.rgba,
        "fade == 0.0 discards every fragment: the frame equals omitting the instance"
    );

    // height_min at the cube's mid-height (y = 0) with fade 0.0: only the upper half fades.
    scene.meshes = vec![make(0.0, 0.0)];
    let gated = renderer
        .render(&scene, FrameSize::native(w, h))
        .expect("height-gated render");
    assert_ne!(
        gated.rgba, solid.rgba,
        "height_min must fade the upper half (≠ solid)"
    );
    assert_ne!(
        gated.rgba, background.rgba,
        "height_min must keep the lower half opaque (≠ background)"
    );

    scene.meshes = vec![make(0.5, -f32::MAX)];
    let a = renderer
        .render(&scene, FrameSize::native(w, h))
        .expect("partial-fade render a");
    let b = renderer
        .render(&scene, FrameSize::native(w, h))
        .expect("partial-fade render b");
    assert_eq!(
        a.rgba, b.rgba,
        "the Bayer dither must be deterministic across renders"
    );
    assert_ne!(
        a.rgba, solid.rgba,
        "fade 0.5 discards some fragments (≠ solid)"
    );
    assert_ne!(
        a.rgba, background.rgba,
        "fade 0.5 keeps some fragments (≠ background)"
    );
}

// --- GPU bone palette (Plan/skeletal-animation.md) ------------------------------------------

/// A single asset-mesh quad (four vertices, two triangles) whose every vertex carries the same
/// skin influence — not a real skinned character, just enough geometry to prove the palette path
/// deforms exactly what a model matrix would. Positions are exact binary fractions so a GPU
/// skin-matrix multiply and a CPU-built model matrix produce bit-identical world positions.
fn skin_quad(weights: [f32; 4], bones: [u32; 4], hash: u64) -> MeshData {
    MeshData {
        positions: vec![
            -1.0, -1.0, 0.0, //
            1.0, -1.0, 0.0, //
            1.0, 1.0, 0.0, //
            -1.0, 1.0, 0.0,
        ],
        normals: [0.0, 0.0, 1.0].repeat(4),
        colors: vec![1.0; 16],
        materials: [1.0, 0.0, 0.0].repeat(4),
        uvs: vec![0.0; 8],
        tangents: [1.0, 0.0, 0.0, 1.0].repeat(4),
        weights: weights.repeat(4),
        bone_indices: bones.repeat(4),
        skin_joints: Vec::new(),
        textures: None,
        authored_material: false,
        base_color_mean: [1.0; 4],
        rim: None,
        double_sided: false,
        indices: vec![0, 1, 2, 0, 2, 3],
        hash,
        lods: Vec::new(),
    }
}

/// An emissive (lighting-independent) asset-mesh instance at `model` with the given
/// `bone_offset`, referencing asset index 0.
fn skin_instance(model: Mat4, bone_offset: u32) -> MeshInstance {
    MeshInstance {
        medium: 1.0,
        shape: MeshShape::Asset(0),
        model,
        normal_matrix: Mat3::IDENTITY,
        color: [1.0; 4],
        material: [1.0, 0.0, 1.0],
        use_vertex_material: false,
        fade: 1.0,
        fade_height_min: -f32::MAX,
        flash: 0.0,
        flash_color: [1.0; 3],
        wrap: 0.0,
        scatter_color: [1.0; 3],
        bone_offset,
        cast_shadows: true,
        viewmodel: false,
        rim: [0.0; 4],
    }
}

/// The GPU bone palette deforms exactly what a model matrix would, and an unskinned instance
/// (`bone_offset == 0`) never reads the palette at all — proven three ways, all with exact
/// power-of-two arithmetic so the comparisons are bit-exact, not merely close. Skips with no GPU.
#[test]
fn needs_gpu_bone_palette_deforms_skinned_instances_and_unskinned_never_reads_it() {
    let Some(renderer) =
        skip_or_renderer("bone_palette_deforms_skinned_instances_and_unskinned_never_reads_it")
    else {
        return;
    };
    let size = FrameSize::native(96, 96);
    let d = Vec3::new(0.5, 0.25, 0.0);

    // (a) A single-bone skin: `bone_offset` 1 into `[T(d)]` (the renderer prepends identity at 0)
    // must deform exactly as a model matrix `T(d)` would, and must differ from the undeformed
    // render.
    let mut scene = RenderScene::empty((96, 96));
    scene
        .mesh_assets
        .push(Arc::new(skin_quad([1.0, 0.0, 0.0, 0.0], [0, 0, 0, 0], 1)));
    scene.meshes = vec![skin_instance(Mat4::IDENTITY, 0)];
    let undeformed = renderer.render(&scene, size).expect("undeformed render");

    scene.meshes = vec![skin_instance(Mat4::IDENTITY, 1)];
    scene.bone_palette = vec![Mat4::from_translation(d)];
    let skinned = renderer.render(&scene, size).expect("skinned render");

    scene.bone_palette = Vec::new();
    scene.meshes = vec![skin_instance(Mat4::from_translation(d), 0)];
    let translated = renderer.render(&scene, size).expect("translated render");

    assert_eq!(
        skinned.rgba, translated.rgba,
        "a palette-driven translation must render exactly as the same model-matrix translation",
    );
    assert_ne!(
        skinned.rgba, undeformed.rgba,
        "the deformed instance must actually move (≠ the undeformed render)",
    );

    // (b) `bone_offset == 0` never reads the palette: a garbage entry (never selected, since this
    // instance's offset is 0) renders byte-identical to an empty palette.
    scene.meshes = vec![skin_instance(Mat4::IDENTITY, 0)];
    scene.bone_palette = vec![Mat4::from_scale(Vec3::ZERO)];
    let garbage_unread = renderer
        .render(&scene, size)
        .expect("garbage-palette render");
    scene.bone_palette = Vec::new();
    let no_palette = renderer.render(&scene, size).expect("no-palette render");
    assert_eq!(
        garbage_unread.rgba, no_palette.rgba,
        "bone_offset 0 must never read the palette, however it is populated",
    );
    assert_eq!(
        garbage_unread.rgba, undeformed.rgba,
        "and must match the same instance's undeformed render above",
    );

    // (c) A two-bone blend `(0.5, 0.5)` between `[I, T(2d)]` at offset 1 is exact arithmetic
    // (`0.5 * I + 0.5 * T(2d)` has translation column `0.5*(0,0,0,1) + 0.5*(2d,1) == (d,1)`
    // bit-for-bit, since `d` is a power-of-two fraction) — it must equal the `T(d)`-model render.
    let mut blended_scene = RenderScene::empty((96, 96));
    blended_scene
        .mesh_assets
        .push(Arc::new(skin_quad([0.5, 0.5, 0.0, 0.0], [0, 1, 0, 0], 2)));
    blended_scene.meshes = vec![skin_instance(Mat4::IDENTITY, 1)];
    blended_scene.bone_palette = vec![Mat4::IDENTITY, Mat4::from_translation(d * 2.0)];
    let blended = renderer
        .render(&blended_scene, size)
        .expect("blended render");
    assert_eq!(
        blended.rgba, translated.rgba,
        "a two-bone blend toward the same net translation must render bit-identically",
    );
}

/// A lit, non-emissive instance of asset 0 with its own normal matrix.
fn lit_skin_instance(model: Mat4, normal_matrix: Mat3, bone_offset: u32) -> MeshInstance {
    MeshInstance {
        material: [1.0, 0.0, 0.0],
        normal_matrix,
        ..skin_instance(model, bone_offset)
    }
}

/// The quad with every normal slanted 45° toward +Y.
fn slanted_quad() -> MeshData {
    let mut quad = skin_quad([1.0, 0.0, 0.0, 0.0], [0, 0, 0, 0], 3);
    let slant = std::f32::consts::FRAC_1_SQRT_2;
    quad.normals = [0.0, slant, slant].repeat(4);
    quad
}

/// Renders the slanted-normal quad under a sun along −Z (so `N·L` is the world normal's `z`),
/// with `instance` and `palette` as given.
fn render_slanted_quad(
    renderer: &Renderer,
    instance: MeshInstance,
    palette: Vec<Mat4>,
) -> RenderedImage {
    render_quad(renderer, slanted_quad(), instance, palette)
}

/// Renders `quad` under the same sun, with `instance` and `palette` as given.
fn render_quad(
    renderer: &Renderer,
    quad: MeshData,
    instance: MeshInstance,
    palette: Vec<Mat4>,
) -> RenderedImage {
    let mut scene = RenderScene::empty((96, 96));
    scene.ambient = [0.0; 3];
    scene.post = Some(exact_post());
    scene.sun = Some(test_sun(Vec3::NEG_Z, Vec3::ONE, 0.002));
    scene.mesh_assets.push(Arc::new(quad));
    scene.meshes = vec![instance];
    scene.bone_palette = palette;
    renderer
        .render(&scene, FrameSize::native(96, 96))
        .expect("slanted-quad render")
}

fn max_channel_gap(a: &RenderedImage, b: &RenderedImage) -> u8 {
    a.rgba
        .iter()
        .zip(&b.rgba)
        .map(|(x, y)| x.abs_diff(*y))
        .max()
        .unwrap_or(0)
}

/// A skinned normal follows the joint's transform the way a model's normal matrix does — the
/// inverse-transpose — under a non-uniform joint scale, where `skin * normal` tilts it the wrong
/// way. The oracle (A) is an unskinned instance under the same scale with glam's inverse-transpose
/// normal matrix; the control (C) uses the old rule and must differ, so the fixture discriminates.
#[test]
fn needs_gpu_skinned_normals_follow_a_non_uniform_joint_scale() {
    let Some(renderer) = skip_or_renderer("skinned_normals_follow_a_non_uniform_joint_scale")
    else {
        return;
    };
    let scale = Mat4::from_scale(Vec3::new(1.0, 3.0, 1.0));
    let inverse_transpose = |m: Mat4| Mat3::from_mat4(m).inverse().transpose();

    let oracle = render_slanted_quad(
        &renderer,
        lit_skin_instance(scale, inverse_transpose(scale), 0),
        Vec::new(),
    );
    let skinned = render_slanted_quad(
        &renderer,
        lit_skin_instance(Mat4::IDENTITY, Mat3::IDENTITY, 1),
        vec![scale],
    );
    let old_rule = render_slanted_quad(
        &renderer,
        lit_skin_instance(scale, Mat3::from_mat4(scale), 0),
        Vec::new(),
    );

    assert!(
        max_channel_gap(&skinned, &oracle) <= 1,
        "a skinned normal must match the inverse-transpose oracle (gap {})",
        max_channel_gap(&skinned, &oracle),
    );
    let centre = (48, 48);
    assert!(
        luma(&old_rule, centre.0, centre.1).abs_diff(luma(&oracle, centre.0, centre.1)) >= 20,
        "the control (old rule) must differ from the oracle, or the fixture proves nothing",
    );

    // A rigid joint (rotation × uniform scale) keeps matching its oracle.
    let rigid = Mat4::from_rotation_x(30f32.to_radians()) * Mat4::from_scale(Vec3::splat(2.0));
    let rigid_oracle = render_slanted_quad(
        &renderer,
        lit_skin_instance(rigid, inverse_transpose(rigid), 0),
        Vec::new(),
    );
    let rigid_skinned = render_slanted_quad(
        &renderer,
        lit_skin_instance(Mat4::IDENTITY, Mat3::IDENTITY, 1),
        vec![rigid],
    );
    assert!(
        max_channel_gap(&rigid_skinned, &rigid_oracle) <= 1,
        "a rigid joint's skinned normal must match its oracle (gap {})",
        max_channel_gap(&rigid_skinned, &rigid_oracle),
    );
}

/// A mirroring joint (negative determinant) keeps its skinned normal: the cofactor of a mirror
/// is the inverse-transpose times a negative determinant, and the snippet's `select` restores
/// the sign. The quad is wound clockwise so the x-mirror turns it toward the camera (the pipeline culls
/// by the model's determinant, and a skinned instance's model is the identity). The oracle is
/// an unskinned twin placed by `diag(-1, 3, -1)`: the same z = 0 vertices with a positive
/// determinant, lit through glam's inverse-transpose. The control negates that normal matrix,
/// which is what an inverted select gives, and must differ.
#[test]
fn needs_gpu_skinned_normals_survive_a_mirroring_joint() {
    let Some(renderer) = skip_or_renderer("skinned_normals_survive_a_mirroring_joint") else {
        return;
    };
    let mut quad = slanted_quad();
    quad.indices = vec![0, 2, 1, 0, 3, 2];
    quad.hash = 4;
    let mirror = Mat4::from_scale(Vec3::new(-1.0, 3.0, 1.0));
    let placement = Mat4::from_scale(Vec3::new(-1.0, 3.0, -1.0));
    let normal_matrix = Mat3::from_mat4(mirror).inverse().transpose();

    let oracle = render_quad(
        &renderer,
        quad.clone(),
        lit_skin_instance(placement, normal_matrix, 0),
        Vec::new(),
    );
    let skinned = render_quad(
        &renderer,
        quad.clone(),
        lit_skin_instance(Mat4::IDENTITY, Mat3::IDENTITY, 1),
        vec![mirror],
    );
    let inverted = render_quad(
        &renderer,
        quad,
        lit_skin_instance(placement, normal_matrix * -1.0, 0),
        Vec::new(),
    );

    assert!(
        max_channel_gap(&skinned, &oracle) <= 1,
        "a mirrored skinned normal must match the inverse-transpose oracle (gap {})",
        max_channel_gap(&skinned, &oracle),
    );
    assert!(
        luma(&inverted, 48, 48).abs_diff(luma(&oracle, 48, 48)) >= 20,
        "the control (negated normal) must differ from the oracle, or the fixture proves nothing",
    );
}

// --- Supersampling (Plan/post-process-pipeline.md) ------------------------------------

/// A scene whose only content is one flat, sub-threshold silhouette: an emissive cube rotated 45°
/// about Z, so every visible face is the *same* colour (no point lights, no sun, no environment —
/// the shading is `ambient × albedo + albedo × emissive`, which is constant per instance) and its
/// screen-space edges are diagonal. Sub-threshold means the bright pass keeps nothing, so bloom
/// contributes exactly zero and every pixel is one of exactly two values.
fn flat_diamond_scene(w: u32, h: u32) -> RenderScene {
    let mut scene = RenderScene::empty((w, h));
    scene.meshes = vec![MeshInstance {
        medium: 1.0,
        shape: MeshShape::Cube,
        model: Mat4::from_rotation_z(std::f32::consts::FRAC_PI_4)
            * Mat4::from_scale(Vec3::splat(2.0)),
        normal_matrix: Mat3::IDENTITY,
        color: [0.8, 0.25, 0.2, 1.0],
        material: [1.0, 0.0, 0.5],
        use_vertex_material: false,
        fade: 1.0,
        fade_height_min: -f32::MAX,
        flash: 0.0,
        flash_color: [1.0; 3],
        wrap: 0.0,
        scatter_color: [1.0; 3],
        bone_offset: 0,
        cast_shadows: true,
        viewmodel: false,
        rim: [0.0; 4],
    }];
    scene
}

/// **The resolve really averages.** At scale 1 the diamond's diagonal edge is a hard staircase:
/// every pixel is either background or silhouette, and *nothing* lies in between (there is no
/// MSAA, and the scene is built so bloom contributes zero). At scale 2 the box filter mixes the
/// four source texels each output pixel covers, so the same edge carries a partial pixel almost
/// everywhere along it (measured: 98 of a ~350-pixel perimeter at 128²) — and the two frames
/// still agree on the flat interior and the flat background, so the difference is confined to
/// the edge. Skips with no GPU.
#[test]
fn needs_gpu_resolving_a_hard_edge_produces_intermediate_pixels() {
    let Some(renderer) = skip_or_renderer("resolving_a_hard_edge_produces_intermediate_pixels")
    else {
        return;
    };
    let (w, h) = (128u32, 128u32);
    let mut scene = flat_diamond_scene(w, h);
    // The output dither ships on and the resolve applies it at the output extent, so a flat area
    // would carry the tile rather than its own value, and the equalities below would read as
    // noise. Off for this fixture only.
    scene.post = Some(crate::scene::PostData {
        dither: false,
        ..crate::scene::PostData::default()
    });

    let native = renderer
        .render(&scene, FrameSize::native(w, h))
        .expect("native render");
    let resolved = renderer
        .render(
            &scene,
            FrameSize::scaled(w, h, crate::RenderScale::new(2.0).unwrap()),
        )
        .expect("supersampled render");
    assert_eq!((resolved.width, resolved.height), (w, h));

    // The two flat values, read where both frames must agree: a corner (background) and the
    // frame centre (the diamond's interior).
    let background = luma(&native, 0, 0);
    let silhouette = luma(&native, w / 2, h / 2);
    assert!(
        silhouette > background + 30,
        "the fixture must actually be a high-contrast silhouette: bg={background} \
         object={silhouette}",
    );
    assert_eq!(background, luma(&resolved, 0, 0), "flat background unmoved");
    // One step per channel, not exact: the half-float seam can land on the other side of an
    // 8-bit rounding boundary. Flat stays flat: every interior pixel equals the centre.
    assert!(
        pixel_drift(&resolved, &native, w / 2, h / 2) <= 1,
        "flat interior moved by more than a rounding step",
    );
    let centre = region(&resolved, w / 2..w / 2 + 1, h / 2..h / 2 + 1)[0];
    assert!(
        region(&resolved, w / 2 - 8..w / 2 + 8, h / 2 - 8..h / 2 + 8)
            .iter()
            .all(|px| *px == centre),
        "flat interior must stay flat",
    );

    // Count pixels strictly between the two flat values, with a couple of LSBs of slack so the
    // sRGB round trip cannot manufacture a hit.
    let intermediates = |img: &RenderedImage| {
        let mut count = 0;
        for y in 0..h {
            for x in 0..w {
                let l = luma(img, x, y);
                if l > background + 2 && l + 2 < silhouette {
                    count += 1;
                }
            }
        }
        count
    };
    assert_eq!(
        intermediates(&native),
        0,
        "at scale 1 a hard edge has no partial pixels at all",
    );
    assert!(
        intermediates(&resolved) > 50,
        "the resolve must average the edge: only {} intermediate pixels",
        intermediates(&resolved),
    );
}

// --- Sub-native rendering (Plan/scalability.md) ----------------------------------------

/// **The magnification interpolates, and that is why it is not the resolve.** At scale 0.5 the
/// diamond's diagonal edge is drawn into a half-extent buffer and then magnified: the bilinear
/// filter mixes the four source texels around each destination pixel's centre, so the edge
/// carries partial pixels almost everywhere along it. The `resolve` pass's area-weighted box
/// would give back a staircase instead — under magnification a destination footprint falls inside
/// a single source texel, so every weight but one is zero and the filter is nearest-neighbour.
///
/// The flat interior and the flat background are unmoved, because four equal taps lerp to their
/// own value: what the scale costs is confined to the edge, exactly as it is above native. Skips
/// with no GPU.
#[test]
fn needs_gpu_magnifying_a_hard_edge_produces_intermediate_pixels() {
    let Some(renderer) = skip_or_renderer("magnifying_a_hard_edge_produces_intermediate_pixels")
    else {
        return;
    };
    let (w, h) = (128u32, 128u32);
    let mut scene = flat_diamond_scene(w, h);
    // Off for the same reason the resolve's twin switches it off: the upscale applies the dither,
    // so a flat area would carry the pattern rather than its own value.
    scene.post = Some(crate::scene::PostData {
        dither: false,
        ..crate::scene::PostData::default()
    });

    let native = renderer
        .render(&scene, FrameSize::native(w, h))
        .expect("native render");
    let magnified = renderer
        .render(
            &scene,
            FrameSize::scaled(w, h, crate::RenderScale::new(0.5).unwrap()),
        )
        .expect("sub-native render");
    assert_eq!((magnified.width, magnified.height), (w, h));

    let background = luma(&native, 0, 0);
    let silhouette = luma(&native, w / 2, h / 2);
    assert!(
        silhouette > background + 30,
        "the fixture must actually be a high-contrast silhouette: bg={background} \
         object={silhouette}",
    );
    assert_eq!(
        background,
        luma(&magnified, 0, 0),
        "flat background unmoved",
    );
    // One step per channel, not exact: the half-float seam can land on the other side of an
    // 8-bit rounding boundary. Flat stays flat: every interior pixel equals the centre.
    assert!(
        pixel_drift(&magnified, &native, w / 2, h / 2) <= 1,
        "flat interior moved by more than a rounding step",
    );
    let centre = region(&magnified, w / 2..w / 2 + 1, h / 2..h / 2 + 1)[0];
    assert!(
        region(&magnified, w / 2 - 8..w / 2 + 8, h / 2 - 8..h / 2 + 8)
            .iter()
            .all(|px| *px == centre),
        "flat interior must stay flat",
    );

    let intermediates = |img: &RenderedImage| {
        let mut count = 0;
        for y in 0..h {
            for x in 0..w {
                let l = luma(img, x, y);
                if l > background + 2 && l + 2 < silhouette {
                    count += 1;
                }
            }
        }
        count
    };
    assert_eq!(
        intermediates(&native),
        0,
        "at scale 1 a hard edge has no partial pixels at all",
    );
    assert!(
        intermediates(&magnified) > 50,
        "the upscale must interpolate the edge, not repeat texels: only {} intermediate pixels",
        intermediates(&magnified),
    );
}

/// **UI stays native-crisp below native**, byte for byte. The overlay runs at the output extent,
/// behind the magnification, so a UI-carrying frame rendered at half scale must put exactly the
/// bytes on the screen that a native one does — every rect edge and every glyph included.
///
/// The background is what makes this a *whole-frame* claim instead of a windowed one: an empty
/// scene composites to a single flat colour, and magnifying a constant returns the constant under
/// any filter, so the comparison is not confined to the pixels the UI covers. That also means the
/// background half of the equality proves nothing about the filter — the interpolation is
/// `magnifying_a_hard_edge_produces_intermediate_pixels`'s claim, and this test is about the
/// overlay and nothing else. The output dither is the one shipped default that would break the
/// flatness (the upscale applies it to the frame the overlay then draws on) and is off for this
/// fixture only.
///
/// The rect and the glyphs are deliberately odd-sized and odd-placed, so none of their edges
/// lands on a source texel: UI rasterized ahead of the magnification could not reproduce these
/// bytes. Skips with no GPU.
#[test]
fn needs_gpu_ui_is_rasterized_at_the_output_extent_below_native() {
    let Some(renderer) = skip_or_renderer("ui_is_rasterized_at_the_output_extent_below_native")
    else {
        return;
    };
    let (w, h) = (128u32, 128u32);
    let mut bare = RenderScene::empty((w, h));
    bare.post = Some(crate::scene::PostData {
        dither: false,
        ..crate::scene::PostData::default()
    });
    let mut with_ui = bare.clone();
    with_ui.ui = vec![
        UiInstance::Rect {
            rect_px: [13.0, 7.0, 41.0, 23.0],
            texture: None,
            color: [1.0, 0.2, 0.1, 1.0],
            uv: [0.0, 0.0, 1.0, 1.0],
            clip: UiClip::NONE,
            view: None,
            rotation: crate::UiRotation::NONE,
        },
        UiInstance::Text {
            color: [1.0; 4],
            text: "crisp".to_string(),
            quads: ludion_font::layout(ludion_font::FontFace::embedded(), "crisp", 11.0, 0.0)
                .quads([9.0, 55.0], 11.0, ludion_font::YAxis::Down),
            font: None,
            clip: UiClip::NONE,
            outline: None,
            rotation: crate::UiRotation::NONE,
        },
    ];

    let half = FrameSize::scaled(w, h, crate::RenderScale::new(0.5).unwrap());
    let native_img = renderer
        .render(&with_ui, FrameSize::native(w, h))
        .expect("native render");
    let half_img = renderer.render(&with_ui, half).expect("sub-native render");
    assert_eq!((half_img.width, half_img.height), (w, h));

    // Without this the equality could hold for a frame carrying no UI at all.
    let bare_half = renderer
        .render(&bare, half)
        .expect("bare sub-native render");
    assert_ne!(
        bare_half.rgba, half_img.rgba,
        "the fixture's UI must actually reach the frame",
    );

    assert_eq!(
        native_img.rgba, half_img.rgba,
        "the overlay draws at the output extent, so a sub-native frame's UI is the native one",
    );
}

// --- The composite pass (Plan/post-process-pipeline.md) --------------------------------

/// A scene whose composite input is a **known** HDR triple, exactly.
///
/// One emissive cube filling the frame centre, with the ambient term zeroed and no lights, no
/// sun and no environment: the mesh shader then computes `albedo * ambient + albedo * emissive`,
/// which with `ambient = 0` and `emissive = 1` is the instance colour verbatim. The caller pairs
/// it with `bloom_strength: 0.0`, and `bloom * 0.0` is exactly zero in IEEE, so the centre
/// pixel's composite input is `color` and nothing else.
///
/// `color`'s components must be exactly representable in f16 — the scene target is
/// `Rgba16Float`, so anything else would be rounded before the composite ever sees it.
fn known_hdr_scene(w: u32, h: u32, color: [f32; 3]) -> RenderScene {
    let mut scene = RenderScene::empty((w, h));
    scene.ambient = [0.0, 0.0, 0.0];
    scene.meshes = vec![MeshInstance {
        medium: 1.0,
        shape: MeshShape::Cube,
        model: Mat4::from_scale(Vec3::splat(4.0)),
        normal_matrix: Mat3::IDENTITY,
        color: [color[0], color[1], color[2], 1.0],
        material: [1.0, 0.0, 1.0],
        use_vertex_material: false,
        fade: 1.0,
        fade_height_min: -f32::MAX,
        flash: 0.0,
        flash_color: [1.0; 3],
        wrap: 0.0,
        scatter_color: [1.0; 3],
        bone_offset: 0,
        cast_shadows: true,
        viewmodel: false,
        rim: [0.0; 4],
    }];
    scene
}

fn centre_rgb(img: &RenderedImage) -> [u8; 3] {
    let i = (((img.height / 2) * img.width + img.width / 2) * 4) as usize;
    [img.rgba[i], img.rgba[i + 1], img.rgba[i + 2]]
}

/// The channel-wise largest difference between the same pixel of two equal-sized images.
fn pixel_drift(a: &RenderedImage, b: &RenderedImage, x: u32, y: u32) -> u8 {
    let i = ((y * a.width + x) * 4) as usize;
    (0..3)
        .map(|c| a.rgba[i + c].abs_diff(b.rgba[i + c]))
        .max()
        .unwrap_or(0)
}

/// Whether `img` has a pixel within `radius` of `(x, y)` that differs from one of its neighbours
/// by more than a few output steps: the neighbourhood of a hard edge.
fn near_edge(img: &RenderedImage, x: u32, y: u32, radius: i32) -> bool {
    let at = |x: i32, y: i32| {
        let x = x.clamp(0, img.width as i32 - 1) as u32;
        let y = y.clamp(0, img.height as i32 - 1) as u32;
        let i = ((y * img.width + x) * 4) as usize;
        [img.rgba[i], img.rgba[i + 1], img.rgba[i + 2]]
    };
    for dy in -radius..=radius {
        for dx in -radius..=radius {
            let (px, py) = (x as i32 + dx, y as i32 + dy);
            let here = at(px, py);
            let right = at(px + 1, py);
            let below = at(px, py + 1);
            let jump = |other: [u8; 3]| (0..3).any(|c| here[c].abs_diff(other[c]) > 8);
            if jump(right) || jump(below) {
                return true;
            }
        }
    }
    false
}

/// **SMAA runs, is deterministic, and touches only the edges.** The diamond's staircase edge is
/// antialiased: the frame differs from `Off`'s on some pixels, every one of them within a few
/// pixels of an `Off` edge, and the flat interior and background stay within one output step of
/// `Off` (the half-float round trip). Two `Smaa` renders agree byte for byte, and switching back
/// to `Off` returns `Off`'s exact bytes.
#[test]
fn needs_gpu_smaa_runs_and_is_deterministic() {
    let Some(mut renderer) = skip_or_renderer("smaa_runs_and_is_deterministic") else {
        return;
    };
    let (w, h) = (96u32, 96u32);
    let scene = flat_diamond_scene(w, h);
    let size = FrameSize::native(w, h);
    let off = renderer.render(&scene, size).expect("off render");

    renderer.set_anti_aliasing(AntiAliasing::Smaa);
    assert_eq!(renderer.anti_aliasing(), AntiAliasing::Smaa);
    let a = renderer.render(&scene, size).expect("smaa render a");
    let b = renderer.render(&scene, size).expect("smaa render b");
    assert_eq!(a.rgba, b.rgba, "two SMAA renders of one frame must agree");

    let mut changed = 0;
    for y in 0..h {
        for x in 0..w {
            let drift = pixel_drift(&a, &off, x, y);
            if drift > 1 {
                changed += 1;
                assert!(
                    near_edge(&off, x, y, 3),
                    "pixel ({x}, {y}) moved by {drift} steps away from any edge",
                );
            }
        }
    }
    assert!(changed > 0, "SMAA must antialias the diamond's edge");

    renderer.set_anti_aliasing(AntiAliasing::Off);
    let back = renderer.render(&scene, size).expect("off again");
    assert_eq!(back.rgba, off.rgba, "Off after Smaa is Off's exact frame");
}

/// **The grain and the dither move into the blend; they are not lost or doubled.** With both on,
/// a flat region of the `Smaa` frame sits within one output step of the `Off` frame per channel
/// (the composite is the only other place they apply, and it is zeroed under SMAA), and differs
/// from a `Smaa` frame with the grain off.
#[test]
fn needs_gpu_smaa_keeps_grain_and_dither() {
    let Some(mut renderer) = skip_or_renderer("smaa_keeps_grain_and_dither") else {
        return;
    };
    let (w, h) = (96u32, 96u32);
    let mut scene = flat_diamond_scene(w, h);
    scene.post = Some(crate::scene::PostData {
        grain: 0.03,
        dither: true,
        ..crate::scene::PostData::default()
    });
    let size = FrameSize::native(w, h);
    let off = renderer.render(&scene, size).expect("off render");

    renderer.set_anti_aliasing(AntiAliasing::Smaa);
    let smaa = renderer.render(&scene, size).expect("smaa render");
    scene.post = Some(crate::scene::PostData {
        grain: 0.0,
        dither: true,
        ..crate::scene::PostData::default()
    });
    let ungrained = renderer.render(&scene, size).expect("smaa without grain");

    // The background corner and the diamond's interior: flat, far from every edge.
    let flat = [(4..28, 4..28), (44..52, 44..52)];
    let mut grained_apart = 0;
    for (xs, ys) in flat {
        for y in ys {
            for x in xs.clone() {
                let drift = pixel_drift(&smaa, &off, x, y);
                assert!(drift <= 1, "({x}, {y}) is {drift} steps from the Off frame");
                if pixel_drift(&smaa, &ungrained, x, y) > 1 {
                    grained_apart += 1;
                }
            }
        }
    }
    assert!(grained_apart > 0, "the grain must reach the SMAA frame");
}

/// The DoF shader's spiral table covers every tap a quality can ask for, and each entry is the
/// unit direction of its golden-angle step.
#[test]
fn dof_spiral_table_matches_the_golden_angle() {
    let body = ShaderId::Dof.body_source();
    let start = body.find("const DOF_SPIRAL").expect("the table");
    let table = &body[start..];
    let table = &table[..table.find(");").expect("the table's end")];
    let numbers: Vec<f32> = table
        .split("vec2<f32>(")
        .skip(1)
        .flat_map(|entry| {
            entry
                .split(')')
                .next()
                .unwrap()
                .split(',')
                .map(|n| n.trim().parse::<f32>().expect("a float literal"))
                .collect::<Vec<_>>()
        })
        .collect();
    assert_eq!(numbers.len() % 2, 0);
    let entries: Vec<[f32; 2]> = numbers.chunks(2).map(|c| [c[0], c[1]]).collect();
    assert!(entries.len() >= QualityParams::FULL.dof_taps as usize);
    let golden = 2.399_963_2_f32;
    for (i, [c, s]) in entries.iter().enumerate() {
        let angle = (i as f32 * golden) as f64;
        assert!((f64::from(*c) - angle.cos()).abs() < 1e-6, "cos at {i}");
        assert!((f64::from(*s) - angle.sin()).abs() < 1e-6, "sin at {i}");
    }
}

/// The pixels of `img` over `xs` x `ys`, as `[r, g, b]` triples in row-major order.
fn region(img: &RenderedImage, xs: std::ops::Range<u32>, ys: std::ops::Range<u32>) -> Vec<[u8; 3]> {
    ys.flat_map(|y| xs.clone().map(move |x| (x, y)))
        .map(|(x, y)| {
            let i = ((y * img.width + x) * 4) as usize;
            [img.rgba[i], img.rgba[i + 1], img.rgba[i + 2]]
        })
        .collect()
}

/// **The dither is applied after the resample, so it survives it.** At scale 2 and 0.5 a flat
/// patch carries the Bayer tile at the output extent: within one step of the native dithered
/// frame, repeating with period 4 exactly in x and y, and not constant.
#[test]
fn needs_gpu_a_resampled_flat_patch_keeps_the_dither_tile() {
    let Some(renderer) = skip_or_renderer("a_resampled_flat_patch_keeps_the_dither_tile") else {
        return;
    };
    let (w, h) = (128u32, 128u32);
    let scene = flat_diamond_scene(w, h);
    let native = renderer
        .render(&scene, FrameSize::native(w, h))
        .expect("native render");
    // The background corner and the diamond's interior: flat, far from every edge.
    let flat = [(4..28u32, 4..28u32), (56..72, 56..72)];
    for factor in [2.0, 0.5] {
        let scaled = renderer
            .render(
                &scene,
                FrameSize::scaled(w, h, crate::RenderScale::new(factor).unwrap()),
            )
            .expect("scaled render");
        for (xs, ys) in flat.clone() {
            for y in ys.clone() {
                for x in xs.clone() {
                    let drift = pixel_drift(&scaled, &native, x, y);
                    assert!(
                        drift <= 1,
                        "{factor}x ({x}, {y}) is {drift} steps off native"
                    );
                }
            }
            let tile = region(&scaled, xs.clone(), ys.clone());
            let side = xs.len();
            let at = |x: usize, y: usize| tile[y * side + x];
            for y in 0..side {
                for x in 0..side - 4 {
                    assert_eq!(
                        at(x, y),
                        at(x + 4, y),
                        "{factor}x period 4 in x at ({x}, {y})"
                    );
                }
            }
            for y in 0..side - 4 {
                for x in 0..side {
                    assert_eq!(
                        at(x, y),
                        at(x, y + 4),
                        "{factor}x period 4 in y at ({x}, {y})"
                    );
                }
            }
            let distinct: std::collections::BTreeSet<_> = tile.iter().collect();
            assert!(
                distinct.len() >= 2,
                "{factor}x: a flat patch with no dither proves nothing",
            );
        }
    }
}

/// **SMAA under a resample still carries the grain and the dither** — now applied by the resample
/// after the blend. Within one step of the `Off` frame at the same scale on flat areas, and apart
/// from a grain-off frame. This also proves the half-float `smaa_blend` pipeline validates.
#[test]
fn needs_gpu_smaa_keeps_grain_and_dither_under_a_resample() {
    let Some(mut renderer) = skip_or_renderer("smaa_keeps_grain_and_dither_under_a_resample")
    else {
        return;
    };
    let (w, h) = (96u32, 96u32);
    let mut scene = flat_diamond_scene(w, h);
    scene.post = Some(crate::scene::PostData {
        grain: 0.03,
        dither: true,
        ..crate::scene::PostData::default()
    });
    let flat = [(4..28u32, 4..28u32), (44..52, 44..52)];
    for factor in [2.0, 0.5] {
        let size = FrameSize::scaled(w, h, crate::RenderScale::new(factor).unwrap());
        scene.post = Some(crate::scene::PostData {
            grain: 0.03,
            dither: true,
            ..crate::scene::PostData::default()
        });
        renderer.set_anti_aliasing(AntiAliasing::Off);
        let off = renderer.render(&scene, size).expect("off render");
        renderer.set_anti_aliasing(AntiAliasing::Smaa);
        let smaa = renderer.render(&scene, size).expect("smaa render");
        scene.post = Some(crate::scene::PostData {
            grain: 0.0,
            dither: true,
            ..crate::scene::PostData::default()
        });
        let ungrained = renderer.render(&scene, size).expect("smaa without grain");

        let mut grained_apart = 0;
        for (xs, ys) in flat.clone() {
            for y in ys {
                for x in xs.clone() {
                    let drift = pixel_drift(&smaa, &off, x, y);
                    assert!(drift <= 1, "{factor}x ({x}, {y}) is {drift} steps from Off");
                    if pixel_drift(&smaa, &ungrained, x, y) > 1 {
                        grained_apart += 1;
                    }
                }
            }
        }
        assert!(
            grained_apart > 0,
            "{factor}x: the grain must reach the frame"
        );
    }
}

/// A debug view has no half-float twin, so under a resample it keeps the 8-bit seam: the frame
/// renders at scale 2 without a wgpu fault.
#[test]
fn needs_gpu_debug_views_render_under_a_resample() {
    let Some(mut renderer) = skip_or_renderer("debug_views_render_under_a_resample") else {
        return;
    };
    renderer.set_quality(
        Quality::FULL,
        QualityParams {
            local_shadow_atlas_size: 256,
            ..QualityParams::FULL
        },
    );
    let (w, h) = (160u32, 120u32);
    let size = FrameSize::scaled(w, h, crate::RenderScale::new(2.0).unwrap());
    let mut scene = local_shadow_fixture(w, h, true);
    scene.debug_view = Some(crate::DebugView::LocalShadowAtlas);
    let image = renderer.render(&scene, size).expect("view render");
    assert_eq!((image.width, image.height), (w, h));
    assert!(renderer.gpu.take_fault().is_none());

    // The SSR trace view: a real full-screen pass with no twin, behind the SSR fixture.
    let size = FrameSize::scaled(96, 64, crate::RenderScale::new(2.0).unwrap());
    let (image, _) = ssr_view_frame_at(&renderer, size, 1.0, Some(crate::DebugView::SsrConfidence));
    assert_eq!((image.width, image.height), (96, 64));
    assert!(renderer.gpu.take_fault().is_none());
}

/// **SMAA blends what the `Off` path would have stored.** A shallow-sloped square at 6.0 against a
/// dark background, tonemap `none`: the composite's half-float output holds the raw value, and the
/// `Off` path's 8-bit store clamps it. Mixing the raw value instead washes the edge pixels out to
/// white, a bright fringe about a third above `Off`'s light. Every `Smaa` pixel stays within the
/// range of the `Off` frame's 3x3 neighbourhood (one step of slack), and the light in the edge band is
/// what `Off`'s is, not a multiple of it.
#[test]
fn needs_gpu_smaa_blends_the_clamped_frame() {
    let Some(mut renderer) = skip_or_renderer("smaa_blends_the_clamped_frame") else {
        return;
    };
    let (w, h) = (96u32, 96u32);
    let mut scene = known_hdr_scene(w, h, [6.0, 6.0, 6.0]);
    scene.meshes[0].model = Mat4::from_rotation_z(0.12) * Mat4::from_scale(Vec3::splat(2.0));
    scene.post = Some(crate::scene::PostData {
        tonemap: TonemapOperator::None,
        bloom_strength: 0.0,
        dither: false,
        ..crate::scene::PostData::default()
    });
    let size = FrameSize::native(w, h);
    let off = renderer.render(&scene, size).expect("off render");
    renderer.set_anti_aliasing(AntiAliasing::Smaa);
    let smaa = renderer.render(&scene, size).expect("smaa render");

    let at = |img: &RenderedImage, x: i32, y: i32, c: usize| {
        let x = x.clamp(0, w as i32 - 1) as u32;
        let y = y.clamp(0, h as i32 - 1) as u32;
        img.rgba[((y * w + x) * 4) as usize + c]
    };
    let to_linear = |v: u8| {
        let s = f64::from(v) / 255.0;
        if s <= 0.04045 {
            s / 12.92
        } else {
            ((s + 0.055) / 1.055).powf(2.4)
        }
    };
    let (mut off_band, mut smaa_band) = (0.0, 0.0);
    for y in 0..h as i32 {
        for x in 0..w as i32 {
            for c in 0..3 {
                let (mut lo, mut hi) = (u8::MAX, u8::MIN);
                for (dx, dy) in (-1..=1).flat_map(|dy| (-1..=1).map(move |dx| (dx, dy))) {
                    let v = at(&off, x + dx, y + dy, c);
                    lo = lo.min(v);
                    hi = hi.max(v);
                }
                let v = at(&smaa, x, y, c);
                assert!(
                    v.saturating_add(1) >= lo && v <= hi.saturating_add(1),
                    "({x}, {y}) channel {c}: {v} outside the Off range {lo}..={hi}",
                );
                if hi > lo.saturating_add(8) {
                    off_band += to_linear(at(&off, x, y, c));
                    smaa_band += to_linear(v);
                }
            }
        }
    }
    assert!(off_band > 0.0, "the fixture must have an edge");
    assert!(
        smaa_band <= off_band * 1.1,
        "SMAA added light along the edge: {smaa_band} against Off's {off_band}",
    );
}

/// A UI overlay renders under SMAA: the graph and the recorder agree on the overlay path, the
/// rect is drawn over the antialiased image, and the pixels outside it match the `Off` frame.
#[test]
fn needs_gpu_smaa_renders_under_a_ui_overlay() {
    let Some(mut renderer) = skip_or_renderer("smaa_renders_under_a_ui_overlay") else {
        return;
    };
    let (w, h) = (96u32, 96u32);
    let mut scene = flat_diamond_scene(w, h);
    scene.ui = vec![UiInstance::Rect {
        rect_px: [4.0, 4.0, 24.0, 24.0],
        texture: None,
        color: [1.0, 1.0, 1.0, 1.0],
        uv: [0.0, 0.0, 1.0, 1.0],
        clip: UiClip::NONE,
        view: None,
        rotation: crate::UiRotation::NONE,
    }];
    let size = FrameSize::native(w, h);
    let off = renderer.render(&scene, size).expect("off render");
    renderer.set_anti_aliasing(AntiAliasing::Smaa);
    let smaa = renderer.render(&scene, size).expect("smaa render with ui");

    let inside = ((16 * w + 16) * 4) as usize;
    assert!(smaa.rgba[inside..inside + 3].iter().all(|&c| c >= 250));
    // The flat background to the right of the rect, away from the diamond.
    for y in 40..56 {
        assert!(pixel_drift(&smaa, &off, 84, y) <= 1);
    }
}

/// **An absent `PostProcess` is the identity, both halves.** A scene with no `PostProcess` and the same scene
/// with a *fully-default* one — every field spelled out at the value it ships with — render
/// byte-identically, and both are the engine's shipped imaging.
///
/// Byte equality, not SSIM: the whole enforcement technique (plan decision 9) is that a neutral
/// step is *skipped* by a uniform branch rather than computed with an identity constant, and only
/// exact bytes can tell those two apart.
#[test]
fn needs_gpu_a_fully_neutral_post_process_is_byte_identical_to_none() {
    let Some(renderer) = skip_or_renderer("a_fully_neutral_post_process_is_byte_identical_to_none")
    else {
        return;
    };
    let (w, h) = (96u32, 96u32);
    let mut scene = flat_diamond_scene(w, h);
    // Something above the bloom threshold, so the bloom mix is live rather than trivially zero.
    scene.meshes.push(cube(
        Vec3::new(1.5, 1.2, 0.0),
        Vec3::splat(0.6),
        [3.0, 2.4, 1.0, 1.0],
    ));

    let absent = renderer
        .render(&scene, FrameSize::native(w, h))
        .expect("render with no PostProcess");

    scene.post = Some(crate::scene::PostData {
        bloom_threshold: 1.0,
        bloom_strength: 0.1,
        exposure: 1.3,
        tonemap: TonemapOperator::Agx,
        dither: true,
        contrast: 1.0,
        saturation: 1.0,
        temperature: 0.0,
        tint: 0.0,
        lift: [0.0; 3],
        gamma: [1.0; 3],
        gain: [1.0; 3],
        fog_density: 0.0,
        fog_color: [0.5, 0.5, 0.5],
        fog_start: 0.0,
        fog_height: 0.0,
        fog_height_falloff: 0.0,
        fog_albedo: 1.0,
        fog_sky: 0.0,
        fog_anisotropy: 0.7,
        vignette: 0.0,
        vignette_radius: 0.5,
        ssao: 0.0,
        ssao_radius: 0.5,
        ssao_bias: 0.02,
        ssao_direct: 0.0,
        shafts: 0.0,
        dof: 0.0,
        focus_distance: 10.0,
        chromatic_aberration: 0.0,
        halation: 0.0,
        grain: 0.0,
        grain_size: 0.002,
        ssr: 0.0,
        ssr_fade: [0.45, 0.65],
        ssr_max_distance: 30.0,
        ssr_thickness: 0.5,
    });
    let neutral = renderer
        .render(&scene, FrameSize::native(w, h))
        .expect("render with a neutral PostProcess");

    assert_eq!(
        absent.rgba, neutral.rgba,
        "a fully-neutral PostProcess must not move a single byte",
    );
    // And the spelled-out neutral above really is what `PostData::default()` says it is, so the
    // renderer's fallback and an authored default cannot drift apart.
    assert_eq!(scene.post, Some(crate::scene::PostData::default()));
}

/// The four operators are four different images, and `agx` — the shipped default — is
/// byte-identical to what a scene with no `PostProcess` at all renders.
#[test]
fn needs_gpu_each_tonemap_operator_is_a_distinct_image_and_agx_is_the_shipped_one() {
    let Some(renderer) =
        skip_or_renderer("each_tonemap_operator_is_a_distinct_image_and_agx_is_the_shipped_one")
    else {
        return;
    };
    let (w, h) = (64u32, 64u32);
    let mut scene = known_hdr_scene(w, h, [1.5, 0.5, 0.125]);
    let frozen = renderer
        .render(&scene, FrameSize::native(w, h))
        .expect("frozen render");

    let mut images = Vec::new();
    for operator in [
        TonemapOperator::Agx,
        TonemapOperator::Aces,
        TonemapOperator::Neutral,
        TonemapOperator::None,
    ] {
        scene.post = Some(crate::scene::PostData {
            tonemap: operator,
            ..crate::scene::PostData::default()
        });
        let image = renderer
            .render(&scene, FrameSize::native(w, h))
            .expect("operator render");
        if operator == TonemapOperator::Agx {
            assert_eq!(
                image.rgba, frozen.rgba,
                "the shipped operator must be what an unauthored scene renders, byte for byte",
            );
        }
        images.push((operator, image.rgba));
    }
    for (i, (a_op, a)) in images.iter().enumerate() {
        for (b_op, b) in &images[i + 1..] {
            assert_ne!(a, b, "{a_op:?} and {b_op:?} rendered the same image");
        }
    }
}

/// **Operator correctness, at the pixel.** A known linear HDR triple is pushed through each
/// operator and the read-back centre pixel is compared against reference values computed
/// independently of this codebase.
///
/// This is the AgX validation gate the plan's risk list asks for: a compact fit is only worth
/// having if it is the *right* fit, and "it renders something plausible" is not a check.
///
/// The references were computed offline from the published formulations — three.js
/// `AgXToneMapping` (itself Filament's, with Missing Deadlines' sigmoid approximation),
/// Narkowicz's ACES fit, and KhronosGroup/ToneMapping's PBR Neutral — evaluated in f64 on the
/// input `[1.5, 0.5, 0.125]` and then sRGB-encoded and quantized the way the
/// `Rgba8UnormSrgb` target does:
///
/// | operator | linear out                     | u8         |
/// |----------|--------------------------------|------------|
/// | `agx`    | 0.731655, 0.436393, 0.252415   | 222,176,138|
/// | `aces`   | 0.876781, 0.616307, 0.170701   | 241,206,115|
/// | `neutral`| 0.938723, 0.342390, 0.118765   | 248,158, 97|
/// | `none`   | 1.0 (clamped), 0.5, 0.125      | 255,188, 99|
///
/// The tolerance is 2 LSB, which covers f32-vs-f64 evaluation and the target's rounding but is
/// far tighter than the gap between any two operators (the closest pair differs by 18 LSB).
/// Measured on the pinned RTX 3090 the agreement is **exact** — all four operators land on the
/// reference u8 in every channel — so the tolerance is headroom for another adapter, not slack
/// this fit needs.
#[test]
fn needs_gpu_tonemap_operators_match_independently_computed_references() {
    let Some(renderer) =
        skip_or_renderer("tonemap_operators_match_independently_computed_references")
    else {
        return;
    };
    let (w, h) = (64u32, 64u32);
    let mut scene = known_hdr_scene(w, h, [1.5, 0.5, 0.125]);
    for (operator, expected) in [
        (TonemapOperator::Agx, [222u8, 176, 138]),
        (TonemapOperator::Aces, [241, 206, 115]),
        (TonemapOperator::Neutral, [248, 158, 97]),
        (TonemapOperator::None, [255, 188, 99]),
    ] {
        scene.post = Some(crate::scene::PostData {
            // Bloom contributes exactly zero, so the composite's input is the fixture's colour.
            bloom_strength: 0.0,
            // The references below are the operators' own curves, so everything the shipped
            // chain does *around* them has to be neutralized: the exposure would scale the
            // input by 1.3 and the dither would move the read-back pixel by a step.
            exposure: 1.0,
            dither: false,
            tonemap: operator,
            ..crate::scene::PostData::default()
        });
        let image = renderer
            .render(&scene, FrameSize::native(w, h))
            .expect("operator render");
        let got = centre_rgb(&image);
        for channel in 0..3 {
            let drift = got[channel].abs_diff(expected[channel]);
            assert!(
                drift <= 2,
                "{operator:?} channel {channel}: got {got:?}, reference {expected:?}",
            );
        }
    }
}

/// Exposure is a linear multiplier on scene + bloom: raising it brightens monotonically and
/// lowering it darkens, and the shipped `1.3` is byte-identical to a scene that never authored
/// the field. It is the one knob on this component whose default is not its neutral value —
/// `1.3` is what puts mid-grey through AgX where the ACES fit put it — so what the equality
/// below pins is that the renderer's fallback and the component's default agree, not a skip.
#[test]
fn needs_gpu_exposure_scales_monotonically_and_the_default_is_what_an_unauthored_scene_renders() {
    let Some(renderer) = skip_or_renderer(
        "exposure_scales_monotonically_and_the_default_is_what_an_unauthored_scene_renders",
    ) else {
        return;
    };
    let (w, h) = (64u32, 64u32);
    let scene = known_hdr_scene(w, h, [0.25, 0.125, 0.0625]);
    let frozen = renderer
        .render(&scene, FrameSize::native(w, h))
        .expect("frozen render");

    let at = |exposure: f32| {
        let mut s = scene.clone();
        s.post = Some(crate::scene::PostData {
            exposure,
            ..crate::scene::PostData::default()
        });
        renderer
            .render(&s, FrameSize::native(w, h))
            .expect("exposed render")
    };

    let neutral = at(1.3);
    assert_eq!(
        neutral.rgba, frozen.rgba,
        "the shipped exposure must be what an unauthored scene renders",
    );

    let dark = at(0.65);
    let bright = at(2.6);
    let mid = luma(&neutral, w / 2, h / 2);
    assert!(
        luma(&dark, w / 2, h / 2) < mid && mid < luma(&bright, w / 2, h / 2),
        "exposure must order the image: {} < {mid} < {}",
        luma(&dark, w / 2, h / 2),
        luma(&bright, w / 2, h / 2),
    );
}

/// Output dither is **on** by default, is what a scene with no `PostProcess` gets, changes the
/// image when switched off, and is deterministic: it is an ordered tile indexed by pixel position
/// with no frame index, so the same frame renders identically twice — and on a flat patch it
/// moves each channel by at most one output step, which is what makes it read as stipple rather
/// than noise.
#[test]
fn needs_gpu_dither_is_on_by_default_and_deterministic() {
    let Some(renderer) = skip_or_renderer("dither_is_on_by_default_and_deterministic") else {
        return;
    };
    let (w, h) = (64u32, 64u32);
    let mut scene = known_hdr_scene(w, h, [0.25, 0.125, 0.0625]);
    let a = renderer
        .render(&scene, FrameSize::native(w, h))
        .expect("default render a");
    let b = renderer
        .render(&scene, FrameSize::native(w, h))
        .expect("default render b");
    assert_eq!(
        a.rgba, b.rgba,
        "the ordered dither carries no frame index: two renders must agree",
    );

    scene.post = Some(crate::scene::PostData::default());
    assert_eq!(
        renderer
            .render(&scene, FrameSize::native(w, h))
            .expect("explicit-default render")
            .rgba,
        a.rgba,
        "a fully-default `PostProcess` is what a scene with none already renders",
    );

    scene.post = Some(crate::scene::PostData {
        dither: false,
        ..crate::scene::PostData::default()
    });
    let undithered = renderer
        .render(&scene, FrameSize::native(w, h))
        .expect("dither-off render");
    assert_ne!(
        a.rgba, undithered.rgba,
        "switching the dither off must change the image",
    );

    // Across the flat cube face, every dithered value sits within one output step of the
    // undithered one — a sub-LSB offset, not a visible change of colour.
    for y in (h / 2 - 8)..(h / 2 + 8) {
        for x in (w / 2 - 8)..(w / 2 + 8) {
            let i = ((y * w + x) * 4) as usize;
            for channel in 0..3 {
                let drift = a.rgba[i + channel].abs_diff(undithered.rgba[i + channel]);
                assert!(
                    drift <= 1,
                    "dither moved ({x},{y}) channel {channel} by {drift}"
                );
            }
        }
    }
}

// --- The grade (Plan/post-process-pipeline.md) -----------------------------------------

/// The mean of one channel over the whole frame — the statistic the direction tests assert on,
/// so a formula that "does something" but the wrong thing cannot pass.
fn mean_channel(img: &RenderedImage, channel: usize) -> f64 {
    let sum: u64 = img
        .rgba
        .chunks_exact(4)
        .map(|px| px[channel] as u64)
        .sum::<u64>();
    sum as f64 / (img.width * img.height) as f64
}

/// The frame's mean distance from encoded mid-grey. The grade's contrast pivots on linear `0.5`,
/// and the sRGB encode is monotone, so pushing contrast up must move every pixel *away* from the
/// encoded image of that pivot and pulling it down must move every pixel toward it.
fn mean_spread_about_pivot(img: &RenderedImage) -> f64 {
    // sRGB-encode(0.5) = 1.055 * 0.5^(1/2.4) - 0.055, quantized the way the target stores it.
    const PIVOT: f64 = 188.0;
    let sum: f64 = img
        .rgba
        .chunks_exact(4)
        .flat_map(|px| px[..3].iter())
        .map(|&v| (v as f64 - PIVOT).abs())
        .sum();
    sum / (img.width * img.height * 3) as f64
}

/// **The identity holds field by field, the third way.** A `PostProcess` whose *grade* fields are all
/// spelled out at their identity values, while the composite-pass fields are deliberately **not** at
/// their defaults, is byte-identical to the same scene with the grade fields left unauthored.
///
/// The first two ways (no component; a fully-neutral one) both prove the grade skips alongside
/// everything else. This one isolates it: exposure, operator and dither are all doing real work,
/// so the only thing the two renders can disagree about is whether the grade computed an
/// identity instead of skipping one.
#[test]
fn needs_gpu_an_identity_grade_is_byte_identical_under_a_working_composite() {
    let Some(renderer) =
        skip_or_renderer("an_identity_grade_is_byte_identical_under_a_working_composite")
    else {
        return;
    };
    let (w, h) = (32u32, 32u32);
    let mut scene = known_hdr_scene(w, h, [1.5, 0.5, 0.125]);
    let working = crate::scene::PostData {
        exposure: 1.4,
        tonemap: TonemapOperator::Agx,
        dither: true,
        ..crate::scene::PostData::default()
    };

    scene.post = Some(working);
    let unauthored = renderer
        .render(&scene, FrameSize::native(w, h))
        .expect("render with the grade left at its defaults");

    scene.post = Some(crate::scene::PostData {
        contrast: 1.0,
        saturation: 1.0,
        temperature: 0.0,
        tint: 0.0,
        lift: [0.0; 3],
        gamma: [1.0; 3],
        gain: [1.0; 3],
        ..working
    });
    let identity = renderer
        .render(&scene, FrameSize::native(w, h))
        .expect("render with every grade field at its identity");

    assert_eq!(
        unauthored.rgba, identity.rgba,
        "an identity grade must be skipped, not computed",
    );
}

/// `saturation: 0.0` collapses **every** pixel onto the grey axis: the mix's two ends are the
/// pixel's Rec.709 luma in all three channels and the pixel itself, and at 0 the result is the
/// luma verbatim — the same float in r, g and b, so the encoded bytes are equal too.
#[test]
fn needs_gpu_zero_saturation_makes_every_pixel_grey() {
    let Some(renderer) = skip_or_renderer("zero_saturation_makes_every_pixel_grey") else {
        return;
    };
    let (w, h) = (32u32, 32u32);
    let mut scene = known_hdr_scene(w, h, [1.5, 0.5, 0.125]);
    scene.post = Some(crate::scene::PostData {
        saturation: 0.0,
        ..crate::scene::PostData::default()
    });
    let image = renderer
        .render(&scene, FrameSize::native(w, h))
        .expect("desaturated render");
    for (index, px) in image.rgba.chunks_exact(4).enumerate() {
        assert!(
            px[0] == px[1] && px[1] == px[2],
            "pixel {index} is {:?}, not grey",
            &px[..3],
        );
    }
}

/// Contrast pivots on mid-grey: above `1.0` it pushes every pixel away from the pivot, below
/// `1.0` it pulls every pixel toward it. Asserted on the frame's mean distance from the pivot, so
/// a formula that merely perturbs the image cannot pass.
#[test]
fn needs_gpu_contrast_widens_and_narrows_the_spread_about_mid_grey() {
    let Some(renderer) = skip_or_renderer("contrast_widens_and_narrows_the_spread_about_mid_grey")
    else {
        return;
    };
    let (w, h) = (32u32, 32u32);
    let scene = known_hdr_scene(w, h, [1.5, 0.5, 0.125]);
    let at = |contrast: f32| {
        let mut s = scene.clone();
        s.post = Some(crate::scene::PostData {
            contrast,
            ..crate::scene::PostData::default()
        });
        mean_spread_about_pivot(
            &renderer
                .render(&s, FrameSize::native(w, h))
                .expect("contrast render"),
        )
    };
    let (low, neutral, high) = (at(0.5), at(1.0), at(1.6));
    assert!(
        low < neutral && neutral < high,
        "contrast must order the spread about mid-grey: {low} < {neutral} < {high}",
    );
}

/// Lift, gamma and gain each brighten when raised — the CDL's three ends of the same curve, and
/// in `gamma`'s case the **reciprocal** convention (a higher gamma brightens midtones) that is
/// the easiest thing in this shader to get backwards.
#[test]
fn needs_gpu_lift_gamma_and_gain_each_brighten_when_raised() {
    let Some(renderer) = skip_or_renderer("lift_gamma_and_gain_each_brighten_when_raised") else {
        return;
    };
    let (w, h) = (32u32, 32u32);
    let scene = known_hdr_scene(w, h, [0.25, 0.125, 0.0625]);
    let mean_of = |post: crate::scene::PostData| {
        let mut s = scene.clone();
        s.post = Some(post);
        let image = renderer
            .render(&s, FrameSize::native(w, h))
            .expect("graded render");
        (0..3).map(|c| mean_channel(&image, c)).sum::<f64>() / 3.0
    };

    let neutral = mean_of(crate::scene::PostData::default());
    for (name, post) in [
        (
            "lift",
            crate::scene::PostData {
                lift: [0.1; 3],
                ..crate::scene::PostData::default()
            },
        ),
        (
            "gamma",
            crate::scene::PostData {
                gamma: [1.8; 3],
                ..crate::scene::PostData::default()
            },
        ),
        (
            "gain",
            crate::scene::PostData {
                gain: [1.5; 3],
                ..crate::scene::PostData::default()
            },
        ),
    ] {
        let raised = mean_of(post);
        assert!(
            raised > neutral,
            "{name} raised must brighten the frame: {raised} <= {neutral}",
        );
    }
}

/// White balance moves the image along its two named axes and nowhere else in sign: a positive
/// `temperature` raises the red/blue ratio (amber) and a negative one lowers it (blue); a
/// positive `tint` raises green against red+blue and a negative one lowers it (magenta).
#[test]
fn needs_gpu_white_balance_moves_the_named_axes_in_the_named_directions() {
    let Some(renderer) =
        skip_or_renderer("white_balance_moves_the_named_axes_in_the_named_directions")
    else {
        return;
    };
    let (w, h) = (32u32, 32u32);
    let scene = known_hdr_scene(w, h, [0.5, 0.5, 0.5]);
    let ratios = |temperature: f32, tint: f32| {
        let mut s = scene.clone();
        s.post = Some(crate::scene::PostData {
            temperature,
            tint,
            ..crate::scene::PostData::default()
        });
        let image = renderer
            .render(&s, FrameSize::native(w, h))
            .expect("white-balanced render");
        let (r, g, b) = (
            mean_channel(&image, 0),
            mean_channel(&image, 1),
            mean_channel(&image, 2),
        );
        (r / b, g / (r + b))
    };

    let (neutral_warmth, neutral_green) = ratios(0.0, 0.0);
    let (warm, _) = ratios(0.4, 0.0);
    let (cool, _) = ratios(-0.4, 0.0);
    assert!(
        cool < neutral_warmth && neutral_warmth < warm,
        "temperature must order the red/blue ratio: {cool} < {neutral_warmth} < {warm}",
    );

    let (_, green) = ratios(0.0, 0.4);
    let (_, magenta) = ratios(0.0, -0.4);
    assert!(
        magenta < neutral_green && neutral_green < green,
        "tint must order green against red+blue: {magenta} < {neutral_green} < {green}",
    );
}

// --- Fog and vignette (Plan/post-process-pipeline.md) ----------------------------------

fn rgb_at(img: &RenderedImage, x: u32, y: u32) -> [u8; 3] {
    let i = ((y * img.width + x) * 4) as usize;
    [img.rgba[i], img.rgba[i + 1], img.rgba[i + 2]]
}

/// A camera at the world origin looking down −Z, with the engine's usual 60° vertical FOV — so a
/// fixture can place a slab at a chosen distance and know where on screen it lands.
fn origin_camera(w: u32, h: u32) -> crate::scene::RenderCamera {
    crate::scene::RenderCamera {
        view: Mat4::look_at_rh(Vec3::ZERO, Vec3::new(0.0, 0.0, -1.0), Vec3::Y),
        projection: Mat4::perspective_rh(
            60.0_f32.to_radians(),
            crate::scene::aspect_ratio((w, h)),
            0.1,
            1000.0,
        ),
        position: Vec3::ZERO,
    }
}

/// A flat emissive slab: ambient-free, light-free shading makes its rendered colour exactly
/// `color`, so any difference between two of them is what the composite did and nothing else.
fn slab(center: Vec3, size: f32, color: [f32; 3]) -> MeshInstance {
    MeshInstance {
        medium: 1.0,
        shape: MeshShape::Cube,
        model: Mat4::from_translation(center) * Mat4::from_scale(Vec3::splat(size)),
        normal_matrix: Mat3::IDENTITY,
        color: [color[0], color[1], color[2], 1.0],
        material: [1.0, 0.0, 1.0],
        use_vertex_material: false,
        fade: 1.0,
        fade_height_min: -f32::MAX,
        flash: 0.0,
        flash_color: [1.0; 3],
        wrap: 0.0,
        scatter_color: [1.0; 3],
        bone_offset: 0,
        cast_shadows: true,
        viewmodel: false,
        rim: [0.0; 4],
    }
}

/// The fog fixture: two identically-coloured slabs subtending the same angle at very different
/// distances (~8 and ~61 world units), each in its own half of the frame, under an authored
/// `Environment` sky that fills every pixel neither of them covers.
///
/// `ibl_intensity: 0.0` keeps the sky *drawn* while contributing no light, so each slab's colour
/// is its emissive verbatim and the near/far comparison is a comparison of fog alone.
///
/// The slab colour and `FOG_COLOR` are opposed in red and blue, so fogging a pixel must lower its
/// red and raise its blue — a signed, per-channel direction rather than "the pixel changed". Both
/// are exactly representable in f16 (the scene target's format).
const SLAB_COLOR: [f32; 3] = [0.5, 0.0625, 0.0625];
const FOG_COLOR: [f32; 3] = [0.0625, 0.0625, 0.5];
/// Where the two slabs and the sky are sampled in the 64×64 fog fixture.
const NEAR_PX: (u32, u32) = (22, 32);
const FAR_PX: (u32, u32) = (41, 32);
const SKY_PX: (u32, u32) = (2, 2);

fn fog_scene(w: u32, h: u32) -> RenderScene {
    let mut scene = RenderScene::empty((w, h));
    scene.camera = origin_camera(w, h);
    scene.environment = Some(crate::scene::EnvironmentData {
        sky_zenith: [0.02, 0.02, 0.08],
        sky_horizon: [0.06, 0.05, 0.10],
        sky_ground: [0.005, 0.005, 0.01],
        sun_disc_size: 0.0,
        sun_disc_intensity: 1.0,
        ambient: [0.0, 0.0, 0.0],
        ibl_intensity: 0.0,
    });
    scene.meshes = vec![
        slab(Vec3::new(-1.5, 0.0, -8.0), 2.0, SLAB_COLOR),
        slab(Vec3::new(11.25, 0.0, -60.0), 15.0, SLAB_COLOR),
    ];
    scene
}

/// The fog fixtures' post. `dither: false` is load-bearing rather than tidy: every assertion
/// below compares two *different* pixels byte for byte (a near slab against a far one, a low
/// slab against a high one), and the shipped ordered dither is a function of pixel position, so
/// it would put a step between any two of them. The fog itself is unaffected either way.
fn fog_post(density: f32, start: f32) -> PostData {
    PostData {
        fog_density: density,
        fog_color: FOG_COLOR,
        fog_start: start,
        dither: false,
        ..PostData::default()
    }
}

/// **The byte-identity claim, the shape that matters most.** A `PostProcess` whose
/// *other* knobs are all doing real work, with fog and vignette left neutral, is byte-identical
/// to the same scene with those two fields unauthored — so the unconditional `scene_depth` read
/// and the two new uniform branches cost exactly nothing.
///
/// The scene is the fog fixture, which is the worst case for the claim: every pixel has a real
/// depth to reconstruct from, and half of them are background the fog step must skip.
#[test]
fn needs_gpu_neutral_fog_and_vignette_are_byte_identical_under_a_working_composite() {
    let Some(renderer) =
        skip_or_renderer("neutral_fog_and_vignette_are_byte_identical_under_a_working_composite")
    else {
        return;
    };
    let (w, h) = (64u32, 64u32);
    let mut scene = fog_scene(w, h);
    let working = PostData {
        exposure: 1.3,
        tonemap: TonemapOperator::Agx,
        contrast: 1.2,
        saturation: 0.8,
        dither: true,
        ..PostData::default()
    };

    scene.post = Some(working);
    let unauthored = renderer
        .render(&scene, FrameSize::native(w, h))
        .expect("render with fog and vignette left at their defaults");

    scene.post = Some(PostData {
        fog_density: 0.0,
        fog_color: FOG_COLOR,
        fog_start: 12.0,
        fog_height: 4.0,
        fog_height_falloff: 0.0,
        vignette: 0.0,
        vignette_radius: 0.2,
        ..working
    });
    let neutral = renderer
        .render(&scene, FrameSize::native(w, h))
        .expect("render with fog and vignette explicitly neutral");

    assert_eq!(
        unauthored.rgba, neutral.rgba,
        "a neutral fog and vignette must be skipped, not computed — and the two gates must be \
         the density and the strength alone, whatever the other fog/vignette knobs say",
    );
}

/// Fog behaves like fog, in four ways at once on one fixture: a **near** surface is less fogged
/// than a **far** one in the same frame; raising `fog_density` moves both toward `fog_color`
/// monotonically (red down, blue up — the fog colour is opposed to the slabs in both channels);
/// a `fog_start` past a surface's distance leaves that surface byte-identical to no fog at all;
/// and a **sky** pixel is untouched at every density, because nothing was drawn there.
#[test]
fn needs_gpu_fog_thickens_with_distance_and_density_and_never_touches_the_sky() {
    let Some(renderer) =
        skip_or_renderer("fog_thickens_with_distance_and_density_and_never_touches_the_sky")
    else {
        return;
    };
    let (w, h) = (64u32, 64u32);
    let mut scene = fog_scene(w, h);
    let render = |scene: &RenderScene| {
        renderer
            .render(scene, FrameSize::native(w, h))
            .expect("fog render")
    };

    scene.post = Some(fog_post(0.0, 0.0));
    let clear = render(&scene);
    // The fixture must actually be two slabs and a sky before anything below means anything.
    assert_eq!(
        rgb_at(&clear, NEAR_PX.0, NEAR_PX.1),
        rgb_at(&clear, FAR_PX.0, FAR_PX.1),
        "unfogged, the two slabs are the same colour",
    );
    assert_ne!(
        rgb_at(&clear, SKY_PX.0, SKY_PX.1),
        rgb_at(&clear, NEAR_PX.0, NEAR_PX.1),
        "the sky sample must not land on a slab",
    );

    // Monotone in density, in both directions, at both distances.
    let mut previous: Option<([u8; 3], [u8; 3])> = None;
    for density in [0.005_f32, 0.02, 0.08] {
        scene.post = Some(fog_post(density, 0.0));
        let image = render(&scene);
        let near = rgb_at(&image, NEAR_PX.0, NEAR_PX.1);
        let far = rgb_at(&image, FAR_PX.0, FAR_PX.1);
        assert!(
            near[0] > far[0] && near[2] < far[2],
            "at density {density} the near slab must be less fogged than the far one: \
             near {near:?} far {far:?}",
        );
        assert_eq!(
            rgb_at(&image, SKY_PX.0, SKY_PX.1),
            rgb_at(&clear, SKY_PX.0, SKY_PX.1),
            "fog must leave the sky alone at density {density}: it is at the far plane, and \
             distance fog would replace an authored gradient outright",
        );
        if let Some((prev_near, prev_far)) = previous {
            for (label, was, now) in [("near", prev_near, near), ("far", prev_far, far)] {
                assert!(
                    now[0] <= was[0] && now[2] >= was[2],
                    "{label}: raising the density to {density} must move the surface toward \
                     the fog colour (red down, blue up): {was:?} -> {now:?}",
                );
            }
        }
        previous = Some((near, far));
    }

    // A start distance between the two slabs fogs only the far one, and leaves the near one
    // byte-identical to the unfogged frame — the segment past `fog_start` is empty there.
    scene.post = Some(fog_post(0.08, 30.0));
    let started = render(&scene);
    assert_eq!(
        rgb_at(&started, NEAR_PX.0, NEAR_PX.1),
        rgb_at(&clear, NEAR_PX.0, NEAR_PX.1),
        "a fog_start past a surface's distance leaves it untouched",
    );
    assert!(
        rgb_at(&started, FAR_PX.0, FAR_PX.1)[2] > rgb_at(&clear, FAR_PX.0, FAR_PX.1)[2],
        "the far slab is still well past fog_start and must still fog",
    );
}

/// Height fog buries what is low and spares what is high — and its `0.0` falloff is the *limit*
/// of the same formula, not a different one.
///
/// Two identical slabs sit at the same distance from the camera, one three units below it and one
/// three units above. At `fog_height_falloff: 0.0` the two must be byte-identical (equal distance,
/// equal fog); raising the falloff about `fog_height: 0.0` must fog the low one strictly harder.
#[test]
fn needs_gpu_height_fog_buries_the_low_slab_and_spares_the_high_one() {
    let Some(renderer) = skip_or_renderer("height_fog_buries_the_low_slab_and_spares_the_high_one")
    else {
        return;
    };
    let (w, h) = (64u32, 64u32);
    let mut scene = RenderScene::empty((w, h));
    scene.camera = origin_camera(w, h);
    scene.ambient = [0.0, 0.0, 0.0];
    scene.meshes = vec![
        slab(Vec3::new(0.0, -3.0, -20.0), 4.0, SLAB_COLOR),
        slab(Vec3::new(0.0, 3.0, -20.0), 4.0, SLAB_COLOR),
    ];
    let (low_px, high_px) = ((32u32, 40u32), (32u32, 23u32));

    let at = |density: f32, falloff: f32| {
        let mut s = scene.clone();
        s.post = Some(PostData {
            fog_height_falloff: falloff,
            ..fog_post(density, 0.0)
        });
        let image = renderer
            .render(&s, FrameSize::native(w, h))
            .expect("height-fog render");
        (
            rgb_at(&image, low_px.0, low_px.1),
            rgb_at(&image, high_px.0, high_px.1),
        )
    };

    let (unfogged_low, unfogged_high) = at(0.0, 0.0);
    assert_eq!(
        unfogged_low, unfogged_high,
        "the two slabs must be identical before any fog: the fixture varies only in height",
    );

    let (flat_low, flat_high) = at(0.03, 0.0);
    assert_eq!(
        flat_low, flat_high,
        "with no height falloff the two slabs are equidistant, so their fog is identical",
    );
    assert!(
        flat_low[2] > unfogged_low[2],
        "the fixture must be fogging at all: {unfogged_low:?} -> {flat_low:?}",
    );

    let (low, high) = at(0.03, 0.3);
    assert!(
        low[2] > high[2] && low[0] < high[0],
        "the lower slab sits in denser fog and must be buried harder: low {low:?} high {high:?}",
    );
    assert!(
        high[2] < flat_high[2],
        "above the reference height the density falls, so the high slab must fog *less* than \
         it did without the falloff: {flat_high:?} -> {high:?}",
    );
}

/// A scene that fills the whole frame with one flat emissive colour: a slab far larger than the
/// view frustum, ambient-free and light-free, so every pixel carries exactly `color` and any
/// variation across the frame is the vignette's.
///
/// Sub-threshold, so the bright pass keeps nothing and bloom contributes exactly zero — which is
/// what lets the same fixture be compared across resolutions.
fn flat_frame_scene(w: u32, h: u32) -> RenderScene {
    let mut scene = RenderScene::empty((w, h));
    scene.ambient = [0.0, 0.0, 0.0];
    scene.meshes = vec![slab(Vec3::new(0.0, 0.0, -20.0), 40.0, [0.5, 0.5, 0.5])];
    scene
}

/// The colour at a **normalized** frame position — the only way to compare a vignette across
/// resolutions, since it is defined in normalized coordinates and nothing else.
fn rgb_at_normalized(img: &RenderedImage, u: f32, v: f32) -> [u8; 3] {
    let x = ((u * img.width as f32) as u32).min(img.width - 1);
    let y = ((v * img.height as f32) as u32).min(img.height - 1);
    rgb_at(img, x, y)
}

/// The vignette darkens the corners and leaves the centre alone, monotonically outward. Inside
/// `vignette_radius` the smoothstep is exactly zero, so the centre is multiplied by exactly
/// `1.0` — byte-identical to no vignette, not merely close.
#[test]
fn needs_gpu_the_vignette_darkens_outward_and_leaves_the_centre_exact() {
    let Some(renderer) =
        skip_or_renderer("the_vignette_darkens_outward_and_leaves_the_centre_exact")
    else {
        return;
    };
    let (w, h) = (64u32, 64u32);
    let mut scene = flat_frame_scene(w, h);
    let plain = renderer
        .render(&scene, FrameSize::native(w, h))
        .expect("un-vignetted render");
    // The fixture is genuinely flat: the vignette is the only thing that can vary across it.
    assert_eq!(
        rgb_at_normalized(&plain, 0.5, 0.5),
        rgb_at_normalized(&plain, 0.02, 0.02),
        "the flat fixture must fill the frame with one colour",
    );

    scene.post = Some(PostData {
        vignette: 0.8,
        vignette_radius: 0.5,
        ..PostData::default()
    });
    let image = renderer
        .render(&scene, FrameSize::native(w, h))
        .expect("vignetted render");

    assert_eq!(
        rgb_at_normalized(&image, 0.5, 0.5),
        rgb_at_normalized(&plain, 0.5, 0.5),
        "inside the radius the falloff is exactly 0, so the centre must not move a byte",
    );
    let centre = luma(&image, w / 2, h / 2);
    let mid = luma(&image, w / 8, h / 2);
    let corner = luma(&image, 0, 0);
    assert!(
        corner < mid && mid < centre,
        "the darkening must grow outward: corner {corner} < mid {mid} < centre {centre}",
    );
}

/// **The screen-relative claim.** The vignette's geometry is a function of normalized screen
/// position and nothing else, so the same scene places it identically at a different render
/// scale *and* at a different resolution.
///
/// The two comparisons are different failures. A `--scale` change would break it if the falloff
/// were computed from anything the render extent feeds (a texel size, a pixel radius); a
/// resolution change would break it if the falloff were computed in pixels at all. The residual
/// is a half-texel of sampling offset at each probe, which is why the tolerance is 3 LSB rather
/// than zero — measured on the pinned RTX 3090 the largest drift is 1.
#[test]
fn needs_gpu_the_vignette_is_placed_identically_at_any_scale_and_resolution() {
    let Some(renderer) =
        skip_or_renderer("the_vignette_is_placed_identically_at_any_scale_and_resolution")
    else {
        return;
    };
    let post = Some(PostData {
        vignette: 0.6,
        vignette_radius: 0.3,
        ..PostData::default()
    });
    let render = |w: u32, h: u32, size: FrameSize| {
        let mut scene = flat_frame_scene(w, h);
        scene.post = post;
        renderer.render(&scene, size).expect("vignette render")
    };

    let reference = render(64, 64, FrameSize::native(64, 64));
    let supersampled = render(
        64,
        64,
        FrameSize::scaled(64, 64, crate::RenderScale::new(2.0).unwrap()),
    );
    let bigger = render(128, 128, FrameSize::native(128, 128));

    for (u, v) in [
        (0.5, 0.5),
        (0.25, 0.25),
        (0.1, 0.1),
        (0.9, 0.9),
        (0.02, 0.5),
        (0.5, 0.98),
    ] {
        let want = rgb_at_normalized(&reference, u, v);
        for (label, image) in [("scale 2", &supersampled), ("128x128", &bigger)] {
            let got = rgb_at_normalized(image, u, v);
            for channel in 0..3 {
                assert!(
                    got[channel].abs_diff(want[channel]) <= 3,
                    "{label} moved the vignette at ({u}, {v}) channel {channel}: \
                     {want:?} -> {got:?}",
                );
            }
        }
    }
    // And the fixture is one the vignette actually acts on, so the agreement above is not the
    // agreement of two flat frames.
    assert!(
        luma(&reference, 0, 0) + 30 < luma(&reference, 32, 32),
        "the probe frame must carry a real vignette",
    );
}

// --- Ambient occlusion (Plan/post-process-pipeline.md) ---------------------------------

/// The SSAO fixture: a ground plane with two tall slabs leaving a one-unit slot down the middle,
/// looked into from above and behind. The slot floor sees almost nothing but the two walls; the
/// open ground past their end sees nothing but sky. A crease *that* enclosed is what makes the
/// probes read a clear signal at a small render size rather than the couple of percent a lone box
/// on a plane produces.
///
/// `emissive` and `light` light the same geometry three different ways, which is how "AO touches
/// only the ambient term" is separated from "the image changed".
fn ao_scene(w: u32, h: u32, ambient: [f32; 3], emissive: f32, light: bool) -> RenderScene {
    let mut scene = RenderScene::empty((w, h));
    // High enough to see down into the slot *and* to keep the open ground in frame.
    let position = Vec3::new(0.0, 6.0, 9.0);
    scene.camera = crate::scene::RenderCamera {
        view: Mat4::look_at_rh(position, Vec3::ZERO, Vec3::Y),
        projection: Mat4::perspective_rh(
            60.0_f32.to_radians(),
            crate::scene::aspect_ratio((w, h)),
            0.1,
            1000.0,
        ),
        position,
    };
    scene.ambient = ambient;
    let mut floor = ground(20.0);
    floor.material = [1.0, 0.0, emissive];
    let wall = |x: f32| {
        cube(
            Vec3::new(x, 1.5, 0.0),
            Vec3::new(1.5, 3.0, 6.0),
            [0.8, 0.8, 0.8, 1.0],
        )
    };
    scene.meshes = vec![floor, wall(-1.25), wall(1.25)];
    if light {
        scene.lights = vec![crate::scene::PointLightData {
            position: Vec3::new(0.0, 4.0, 0.0),
            color: Vec3::new(1.0, 1.0, 1.0),
            intensity: 4.0,
            range: 30.0,
            dynamic: false,
            shadow: None,
            scatter: 0.0,
        }];
    }
    scene
}

fn ao_post(strength: f32, radius: f32) -> PostData {
    PostData {
        ssao: strength,
        ssao_radius: radius,
        ..PostData::default()
    }
}

/// A unit quad facing +Z with UVs over the unit square and a +X tangent, carrying a constant
/// normal-map texel that tilts along both U and V — so a flipped bitangent changes its shading.
fn normal_mapped_quad(hash: u64) -> MeshData {
    let texel = [170u8, 200, 230, 255];
    let normal = crate::scene::MeshTexture {
        image: MapImage::Source(Arc::new(crate::scene::TextureImage {
            width: 4,
            height: 4,
            rgba: texel.repeat(16),
            hash: hash ^ 0x5EED,
            filter_linear: true,
            premultiplied_on_load: false,
            mean: [1.0; 4],
        })),
        wrap: crate::scene::TextureWrap::Repeat,
    };
    MeshData {
        positions: vec![
            -1.0, -1.0, 0.0, //
            1.0, -1.0, 0.0, //
            1.0, 1.0, 0.0, //
            -1.0, 1.0, 0.0,
        ],
        normals: [0.0, 0.0, 1.0].repeat(4),
        colors: vec![1.0; 16],
        materials: [1.0, 0.0, 0.0].repeat(4),
        uvs: vec![0.0, 1.0, 1.0, 1.0, 1.0, 0.0, 0.0, 0.0],
        tangents: [1.0, 0.0, 0.0, 1.0].repeat(4),
        weights: [1.0, 0.0, 0.0, 0.0].repeat(4),
        bone_indices: vec![0; 16],
        skin_joints: Vec::new(),
        textures: Some(crate::scene::MeshTextures {
            base_color: None,
            metallic_roughness: None,
            normal: Some(normal),
            occlusion: None,
            emissive: None,
            normal_scale: 1.0,
            occlusion_strength: 1.0,
            alpha_cutoff: None,
        }),
        authored_material: false,
        base_color_mean: [1.0; 4],
        rim: None,
        double_sided: false,
        indices: vec![0, 1, 2, 0, 2, 3],
        hash,
        lods: Vec::new(),
    }
}

/// A renderer, or `None` (after a skip line) when the box has no GPU adapter.
fn renderer_or_skip(test: &str) -> Option<Renderer> {
    match Renderer::new() {
        Ok(r) => Some(r),
        Err(d) if ludion_core::require_gpu::is_no_adapter(std::slice::from_ref(&d)) => {
            eprintln!("SKIPPED {test}: no GPU adapter");
            None
        }
        Err(d) => panic!("renderer init failed: {d:?}"),
    }
}

/// A 64×64 gradient map: red rises along U, green along V, blue is flat — every channel a
/// compressor must keep, none constant but one.
fn gradient_source(hash: u64) -> ludion_assets::Image {
    sized_gradient_source(64, 64, 4, hash)
}

/// [`gradient_source`] at another size: red and green rise `step` per texel.
fn sized_gradient_source(width: u32, height: u32, step: u32, hash: u64) -> ludion_assets::Image {
    let mut rgba = Vec::with_capacity((width * height * 4) as usize);
    for y in 0..height {
        for x in 0..width {
            rgba.extend_from_slice(&[(x * step) as u8, (y * step) as u8, 128, 255]);
        }
    }
    ludion_assets::Image {
        width,
        height,
        rgba,
        hash,
    }
}

/// Cooks `source` for `target`, round-tripped through the KTX2 writer and reader.
fn cooked_fixture(
    source: &ludion_assets::Image,
    target: CookTarget,
    srgb: bool,
) -> CompressedImage {
    use ludion_assets::cooked::{ColorSpace, CookSettings, read_ktx2};
    let color_space = if srgb {
        ColorSpace::Srgb
    } else {
        ColorSpace::Linear
    };
    let settings = CookSettings {
        target,
        color_space,
        max_texture_size: None,
        alpha_cutoff: None,
    };
    let cooked = ludion_cook::cook(source, &settings).expect("cook");
    read_ktx2(&ludion_cook::write_ktx2(&cooked), target, color_space).expect("read_ktx2")
}

/// A unit quad carrying `source` as its base-colour (sRGB) or occlusion (linear) map, or no map.
fn gradient_quad(source: Option<&ludion_assets::Image>, base_colour: bool) -> MeshData {
    // The mesh cache keys on the quad's own hash, so each variant needs a distinct one.
    let mut quad =
        normal_mapped_quad(0x9A4D + u64::from(source.is_some()) * 2 + u64::from(base_colour));
    quad.textures = source.map(|source| {
        let map = crate::scene::MeshTexture {
            image: MapImage::Source(Arc::new(crate::scene::TextureImage {
                width: source.width,
                height: source.height,
                rgba: source.rgba.clone(),
                hash: source.hash,
                filter_linear: true,
                premultiplied_on_load: false,
                mean: [1.0; 4],
            })),
            wrap: crate::scene::TextureWrap::Clamp,
        };
        let mut textures = crate::scene::MeshTextures {
            base_color: None,
            metallic_roughness: None,
            normal: None,
            occlusion: None,
            emissive: None,
            normal_scale: 1.0,
            occlusion_strength: 1.0,
            alpha_cutoff: None,
        };
        if base_colour {
            textures.base_color = Some(map);
        } else {
            textures.occlusion = Some(map);
        }
        textures
    });
    quad
}

/// The scene both cooked-map renders share: the quad filling the frame, ambient light only.
fn cooked_map_scene(quad: MeshData) -> RenderScene {
    let mut scene = RenderScene::empty((64, 64));
    let position = Vec3::new(0.0, 0.0, 2.2);
    scene.camera = crate::scene::RenderCamera {
        view: Mat4::look_at_rh(position, Vec3::ZERO, Vec3::Y),
        projection: Mat4::perspective_rh(60.0_f32.to_radians(), 1.0, 0.1, 100.0),
        position,
    };
    scene.ambient = [0.8, 0.8, 0.8];
    scene.mesh_assets.push(Arc::new(quad));
    scene.meshes.push(MeshInstance {
        medium: 1.0,
        shape: MeshShape::Asset(0),
        model: Mat4::IDENTITY,
        normal_matrix: Mat3::IDENTITY,
        color: [1.0; 4],
        material: [1.0, 0.0, 0.0],
        use_vertex_material: false,
        fade: 1.0,
        fade_height_min: -f32::MAX,
        flash: 0.0,
        flash_color: [1.0; 3],
        wrap: 0.0,
        scatter_color: [1.0; 3],
        bone_offset: 0,
        cast_shadows: true,
        viewmodel: false,
        rim: [0.0; 4],
    });
    scene
}

/// Mean absolute difference per RGB channel, on the 0–255 scale.
fn mean_abs_rgb(a: &RenderedImage, b: &RenderedImage) -> f64 {
    let total: u64 = a
        .rgba
        .chunks_exact(4)
        .zip(b.rgba.chunks_exact(4))
        .map(|(p, q)| (0..3).map(|c| u64::from(p[c].abs_diff(q[c]))).sum::<u64>())
        .sum();
    total as f64 / (a.rgba.len() / 4 * 3) as f64
}

/// A cooked map seeded under its source map's key renders like the source in both slot colour
/// spaces, and unlike no map at all — so the bind path consumes the compressed texture unchanged
/// and the map is really sampled. Skips with no GPU or no compression support.
#[test]
fn needs_gpu_a_cooked_map_in_the_material_slot_renders_like_its_source() {
    let Some(source_renderer) = renderer_or_skip("a_cooked_map_renders_like_its_source") else {
        return;
    };
    let Some(target) = source_renderer.cook_target() else {
        eprintln!("SKIPPED a_cooked_map_renders_like_its_source: no compression support");
        return;
    };
    let cooked_renderer = Renderer::new().expect("second renderer");
    let size = FrameSize::native(64, 64);
    let source = gradient_source(0xC00C);
    for (base_colour, srgb) in [(true, true), (false, false)] {
        let scene = cooked_map_scene(gradient_quad(Some(&source), base_colour));
        let from_source = source_renderer.render(&scene, size).expect("source render");
        let untextured = cooked_map_scene(gradient_quad(None, base_colour));
        let bare = source_renderer
            .render(&untextured, size)
            .expect("untextured render");

        let cooked = cooked_fixture(&source, target, srgb);
        let uploaded =
            upload_compressed_in(&cooked_renderer.gpu, &cooked, srgb).expect("cooked upload");
        let map = scene.mesh_assets[0]
            .textures
            .as_ref()
            .and_then(|t| material_maps(t)[if base_colour { 0 } else { 3 }].0)
            .expect("the quad's map");
        cooked_renderer
            .mesh_texture_cache
            .borrow_mut()
            .insert(mesh_texture_key(map, srgb, None), uploaded);
        let from_cooked = cooked_renderer.render(&scene, size).expect("cooked render");

        let close = mean_abs_rgb(&from_source, &from_cooked);
        let far = mean_abs_rgb(&bare, &from_cooked);
        eprintln!("cooked-vs-source srgb={srgb}: close={close:.3} far={far:.3}");
        assert!(
            close < 0.5,
            "srgb={srgb}: cooked drifted from source by {close}"
        );
        assert!(far > 12.0, "srgb={srgb}: the map is not sampled ({far})");
    }
}

/// A cooked map's cache key carries its cook key and its block format, so it never aliases the
/// source map it was cooked from.
#[test]
fn a_cooked_maps_texture_key_carries_its_format_and_cook_key() {
    let source = gradient_source(0xAB);
    let cooked = |format: CompressedFormat| MeshTexture {
        image: MapImage::Cooked(Arc::new(crate::scene::CookedMap {
            key: 0xC0DE,
            image: CompressedImage {
                format,
                srgb: true,
                width: 4,
                height: 4,
                levels: vec![vec![0; 16]],
                mean: [1.0; 4],
            },
        })),
        wrap: crate::scene::TextureWrap::Repeat,
    };
    let bc7 = mesh_texture_key(&cooked(CompressedFormat::Bc7), true, None);
    let astc = mesh_texture_key(&cooked(CompressedFormat::Astc4x4), true, None);
    assert_eq!((bc7.hash, bc7.encoding), (0xC0DE, MapEncoding::Bc7));
    assert_eq!((astc.hash, astc.encoding), (0xC0DE, MapEncoding::Astc4x4));
    let from_source = gradient_quad(Some(&source), true);
    let map = from_source
        .textures
        .as_ref()
        .and_then(|t| t.base_color.as_ref());
    let source_key = mesh_texture_key(map.expect("the quad's map"), true, None);
    assert_eq!(
        (source_key.hash, source_key.encoding),
        (0xAB, MapEncoding::Rgba8)
    );
}

/// A mask material's cutoff keys its base-colour map only; an opaque material's key is the
/// pre-mask one, and two cutoffs on one map never share a texture.
#[test]
fn a_mask_cutoff_keys_the_base_colour_map_only() {
    let quad = gradient_quad(Some(&gradient_source(0xAB)), true);
    let mut textures = quad.textures.expect("the quad's textures");
    textures.occlusion = textures.base_color.clone();
    let plain = material_maps(&textures);
    assert!(plain.iter().all(|(_, _, cutoff)| cutoff.is_none()));
    let map = textures.base_color.as_ref().expect("the base colour map");
    assert_eq!(mesh_texture_key(map, true, None).alpha_cutoff, None);

    textures.alpha_cutoff = Some(0.5);
    let masked = material_maps(&textures);
    assert_eq!(masked[0].2, Some(0.5));
    assert!(masked[1..].iter().all(|(_, _, cutoff)| cutoff.is_none()));
    let half = mesh_texture_key(map, true, Some(0.5));
    let third = mesh_texture_key(map, true, Some(0.3));
    assert_ne!(half, third);
    assert_ne!(half, mesh_texture_key(map, true, None));
}

/// A format the device never requested is a typed refusal before any wgpu call, so no fault is
/// raised. Skips with no GPU, or where the device supports every format (the M4).
#[test]
fn needs_gpu_an_unsupported_compressed_format_is_an_error_not_a_fault() {
    let Some(renderer) = renderer_or_skip("an_unsupported_compressed_format") else {
        return;
    };
    let Some(format) = [CompressedFormat::Bc7, CompressedFormat::Astc4x4]
        .into_iter()
        .find(|format| !renderer.supports_compressed(*format))
    else {
        eprintln!("SKIPPED an_unsupported_compressed_format: the device supports every format");
        return;
    };
    let image = CompressedImage {
        format,
        srgb: true,
        width: 4,
        height: 4,
        levels: vec![vec![0; 16]],
        mean: [1.0; 4],
    };
    assert!(matches!(
        upload_compressed_in(&renderer.gpu, &image, true),
        Err(CompressedUploadError::Unsupported { format: f }) if f == format
    ));
    assert!(renderer.gpu.take_fault().is_none());
}

/// A 64×64 cook stores 7 levels down to 1×1; every one uploads (the 2×2 and 1×1 tail copies a whole
/// block) with no device fault. Skips with no GPU or no compression support.
#[test]
fn needs_gpu_every_cooked_mip_level_uploads() {
    let Some(renderer) = renderer_or_skip("every_cooked_mip_level_uploads") else {
        return;
    };
    let Some(target) = renderer.cook_target() else {
        eprintln!("SKIPPED every_cooked_mip_level_uploads: no compression support");
        return;
    };
    for srgb in [true, false] {
        let cooked = cooked_fixture(&gradient_source(0xFEED), target, srgb);
        assert_eq!(cooked.level_count(), 7);
        let uploaded = upload_compressed_in(&renderer.gpu, &cooked, srgb).expect("cooked upload");
        assert_eq!(uploaded.texture.mip_level_count(), 7);
        assert!(renderer.gpu.take_fault().is_none(), "srgb={srgb}");
    }
}

/// Every level read back from the GPU equals the stored block rows, for a square map and a
/// 100×60 one whose rows need padding and whose tail levels are partial blocks.
#[test]
fn needs_gpu_cooked_levels_read_back_byte_exact() {
    let Some(renderer) = renderer_or_skip("cooked_levels_read_back_byte_exact") else {
        return;
    };
    let Some(target) = renderer.cook_target() else {
        eprintln!("SKIPPED cooked_levels_read_back_byte_exact: no compression support");
        return;
    };
    let gpu = &renderer.gpu;
    let sources = [
        gradient_source(0xFEED),
        sized_gradient_source(100, 60, 2, 0xBEEF),
    ];
    for source in &sources {
        let cooked = cooked_fixture(source, target, true);
        let uploaded = upload_compressed_in(gpu, &cooked, true).expect("cooked upload");
        for level in 0..cooked.level_count() {
            let (bw, bh) = cooked.blocks(level);
            let packed = bw * compressed::BLOCK_BYTES;
            let row = packed.div_ceil(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT)
                * wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
            let buffer = gpu.device().create_buffer(&wgpu::BufferDescriptor {
                label: Some("cooked readback"),
                size: u64::from(row * bh),
                usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                mapped_at_creation: false,
            });
            let mut encoder = gpu
                .device()
                .create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
            encoder.copy_texture_to_buffer(
                wgpu::TexelCopyTextureInfo {
                    texture: &uploaded.texture,
                    mip_level: level,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                },
                wgpu::TexelCopyBufferInfo {
                    buffer: &buffer,
                    layout: wgpu::TexelCopyBufferLayout {
                        offset: 0,
                        bytes_per_row: Some(row),
                        rows_per_image: Some(bh),
                    },
                },
                wgpu::Extent3d {
                    width: bw * 4,
                    height: bh * 4,
                    depth_or_array_layers: 1,
                },
            );
            gpu.queue().submit(Some(encoder.finish()));
            let (tx, rx) = std::sync::mpsc::channel();
            buffer.slice(..).map_async(wgpu::MapMode::Read, move |r| {
                let _ = tx.send(r);
            });
            gpu.device()
                .poll(wgpu::PollType::wait_indefinitely())
                .expect("device poll succeeds");
            rx.recv().expect("map completes").expect("map succeeds");
            let mapped = buffer.slice(..).get_mapped_range().expect("mapped range");
            for (y, expected) in cooked.levels[level as usize]
                .chunks_exact(packed as usize)
                .enumerate()
            {
                let at = y * row as usize;
                assert_eq!(
                    &mapped[at..at + packed as usize],
                    expected,
                    "{}x{} level {level} row {y}",
                    cooked.width,
                    cooked.height
                );
            }
        }
    }
    assert!(gpu.take_fault().is_none());
}

/// The asymmetric scene the mirroring test reflects: a rotated cube at +x and a rotated
/// normal-mapped quad at -x, over a symmetric ground, camera on the x = 0 plane, sun in the
/// yz-plane. `mirror` reflects every instance but the ground through x = 0.
fn mirror_scene(w: u32, h: u32, mirror: bool, ssao: bool) -> RenderScene {
    mirror_scene_with(w, h, mirror, ssao, -20.0, false)
}

/// [`mirror_scene`] with the quad turned `quad_degrees` about y and optionally double-sided.
fn mirror_scene_with(
    w: u32,
    h: u32,
    mirror: bool,
    ssao: bool,
    quad_degrees: f32,
    double_sided: bool,
) -> RenderScene {
    let mut scene = RenderScene::empty((w, h));
    let position = Vec3::new(0.0, 3.0, 7.0);
    scene.camera = crate::scene::RenderCamera {
        view: Mat4::look_at_rh(position, Vec3::new(0.0, 0.5, 0.0), Vec3::Y),
        projection: Mat4::perspective_rh(
            60.0_f32.to_radians(),
            crate::scene::aspect_ratio((w, h)),
            0.1,
            1000.0,
        ),
        position,
    };
    scene.ambient = [0.2, 0.2, 0.2];
    scene.sun = Some(test_sun(
        Vec3::new(0.0, -1.0, -0.6),
        Vec3::new(3.0, 3.0, 3.0),
        0.002,
    ));
    if ssao {
        scene.post = Some(ao_post(1.0, 2.0));
    }
    scene.mesh_assets.push(Arc::new(MeshData {
        double_sided,
        ..normal_mapped_quad(0x4D49_5252)
    }));

    let mut cube = cube(Vec3::ZERO, Vec3::ONE, [0.9, 0.3, 0.2, 1.0]);
    cube.model = Mat4::from_translation(Vec3::new(1.2, 0.5, 0.0))
        * Mat4::from_rotation_y(30.0_f32.to_radians());
    cube.normal_matrix = Mat3::from_mat4(cube.model).inverse().transpose();
    let mut quad = skin_instance(
        Mat4::from_translation(Vec3::new(-1.5, 1.0, 0.0))
            * Mat4::from_rotation_y(quad_degrees.to_radians())
            * Mat4::from_scale(Vec3::splat(1.2)),
        0,
    );
    quad.normal_matrix = Mat3::from_mat4(quad.model).inverse().transpose();
    quad.material = [0.6, 0.0, 0.0];
    quad.color = [0.8, 0.8, 0.9, 1.0];

    let flip = Mat4::from_scale(Vec3::new(-1.0, 1.0, 1.0));
    let mut instances = vec![cube, quad];
    if mirror {
        for m in &mut instances {
            m.model = flip * m.model;
            m.normal_matrix = Mat3::from_mat4(flip) * m.normal_matrix;
        }
    }
    scene.meshes = vec![ground(20.0)];
    scene.meshes.extend(instances);
    scene
}

/// Mean absolute channel difference and the fraction of pixels off by more than 8 in any
/// channel, between `mirrored` and `plain` flipped horizontally.
fn mirror_error(mirrored: &RenderedImage, plain: &RenderedImage) -> (f32, f32) {
    let (w, h) = (mirrored.width, mirrored.height);
    let (mut sum, mut bad) = (0u64, 0u32);
    for y in 0..h {
        for x in 0..w {
            let a = ((y * w + x) * 4) as usize;
            let b = ((y * w + (w - 1 - x)) * 4) as usize;
            let worst = (0..3)
                .map(|c| mirrored.rgba[a + c].abs_diff(plain.rgba[b + c]))
                .inspect(|d| sum += *d as u64)
                .max()
                .unwrap();
            bad += (worst > 8) as u32;
        }
    }
    (sum as f32 / (w * h * 3) as f32, bad as f32 / (w * h) as f32)
}

/// A negative-determinant model (a reflection) reverses triangle winding and the tangent frame's
/// handedness; the instance must still render as the exact mirror image of its unmirrored twin —
/// in the shadow, prepass and scene passes, normal maps included.
#[test]
fn needs_gpu_a_mirrored_instance_renders_as_the_mirror_image() {
    let Some(renderer) = skip_or_renderer("a_mirrored_instance_renders_as_the_mirror_image") else {
        return;
    };
    let (w, h) = (96u32, 64u32);
    for ssao in [false, true] {
        let plain = renderer
            .render(&mirror_scene(w, h, false, ssao), FrameSize::native(w, h))
            .expect("plain render");
        let mirrored = renderer
            .render(&mirror_scene(w, h, true, ssao), FrameSize::native(w, h))
            .expect("mirrored render");
        let (mean, frac) = mirror_error(&mirrored, &plain);
        assert!(
            mean <= 0.5 && frac <= 0.005,
            "ssao={ssao}: mirrored scene differs from the flipped plain scene: \
             mean abs diff {mean:.3} (≤ 0.5), fraction > 8: {frac:.4} (≤ 0.005)",
        );
    }
}

// --- Cutout and double-sided meshes (Clockout request 0010) ------------------------------

/// Mean absolute channel difference and the fraction of pixels off by more than 8 in any
/// channel, between two same-size frames.
fn image_error(a: &RenderedImage, b: &RenderedImage) -> (f32, f32) {
    assert_eq!((a.width, a.height), (b.width, b.height));
    let (mut sum, mut bad) = (0u64, 0u32);
    for px in 0..(a.width * a.height) as usize {
        let worst = (0..3)
            .map(|c| a.rgba[px * 4 + c].abs_diff(b.rgba[px * 4 + c]))
            .inspect(|d| sum += *d as u64)
            .max()
            .unwrap();
        bad += (worst > 8) as u32;
    }
    let pixels = (a.width * a.height) as f32;
    (sum as f32 / (pixels * 3.0), bad as f32 / pixels)
}

/// A `normal_mapped_quad` with its material replaced: `textures` as given, `double_sided` as given.
fn sheet(hash: u64, double_sided: bool, textures: Option<crate::scene::MeshTextures>) -> MeshData {
    MeshData {
        textures,
        double_sided,
        ..normal_mapped_quad(hash)
    }
}

/// A material set carrying only a base-colour map and a cutoff.
fn cutout_textures(
    base_color: Option<crate::scene::MeshTexture>,
    alpha_cutoff: Option<f32>,
) -> crate::scene::MeshTextures {
    crate::scene::MeshTextures {
        base_color,
        metallic_roughness: None,
        normal: None,
        occlusion: None,
        emissive: None,
        normal_scale: 1.0,
        occlusion_strength: 1.0,
        alpha_cutoff,
    }
}

/// An 8×2 base-colour map, one orange everywhere; the left half's alpha is `left_alpha`.
fn half_alpha_map(hash: u64, left_alpha: u8) -> crate::scene::MeshTexture {
    alpha_map(hash, left_alpha, 255)
}

/// [`half_alpha_map`] with the right half's alpha chosen too.
fn alpha_map(hash: u64, left_alpha: u8, right_alpha: u8) -> crate::scene::MeshTexture {
    let mut rgba = Vec::new();
    for _ in 0..2 {
        for x in 0..8 {
            let alpha = if x < 4 { left_alpha } else { right_alpha };
            rgba.extend_from_slice(&[200, 120, 60, alpha]);
        }
    }
    crate::scene::MeshTexture {
        image: MapImage::Source(Arc::new(crate::scene::TextureImage {
            width: 8,
            height: 2,
            rgba,
            hash,
            filter_linear: true,
            premultiplied_on_load: false,
            mean: [1.0; 4],
        })),
        wrap: crate::scene::TextureWrap::Clamp,
    }
}

/// An orthographic frame on the origin from `(0, 0, ±8)`. A sun travelling along `d` (negated for
/// the back view) lights the sheet at the origin: negating the normal, light and view together
/// leaves every BRDF lobe unchanged, so the back view of a double-sided sheet must equal the
/// front view mirrored horizontally.
fn sheet_scene(w: u32, h: u32, back: bool, asset: MeshData) -> RenderScene {
    let mut scene = RenderScene::empty((w, h));
    let z = if back { -8.0 } else { 8.0 };
    let position = Vec3::new(0.0, 0.0, z);
    scene.camera = crate::scene::RenderCamera {
        view: Mat4::look_at_rh(position, Vec3::ZERO, Vec3::Y),
        projection: Mat4::orthographic_rh(-2.0, 2.0, -2.0, 2.0, 0.1, 50.0),
        position,
    };
    scene.ambient = [0.2; 3];
    // Output dither is a screen-space pattern the mirror does not carry across.
    scene.post = Some(PostData {
        dither: false,
        ..PostData::default()
    });
    let travel = Vec3::new(0.35, -0.45, -1.0);
    scene.sun = Some(test_sun(
        if back { -travel } else { travel },
        Vec3::splat(3.0),
        0.002,
    ));
    scene.mesh_assets.push(Arc::new(asset));
    let mut instance = skin_instance(Mat4::IDENTITY, 0);
    instance.material = [1.0, 0.0, 0.0];
    scene.meshes = vec![instance];
    scene
}

#[test]
fn needs_gpu_a_double_sided_back_face_shades_with_the_flipped_normal() {
    let Some(renderer) =
        skip_or_renderer("a_double_sided_back_face_shades_with_the_flipped_normal")
    else {
        return;
    };
    let (w, h) = (64u32, 64u32);
    let render = |back: bool, asset: MeshData| {
        renderer
            .render(&sheet_scene(w, h, back, asset), FrameSize::native(w, h))
            .expect("sheet render")
    };
    let plain_front = render(false, sheet(0xB0_01, true, None));
    let plain_back = render(true, sheet(0xB0_01, true, None));
    let mapped_front = render(false, sheet(0xB0_02, true, normal_mapped_quad(0).textures));
    let mapped_back = render(true, sheet(0xB0_02, true, normal_mapped_quad(0).textures));
    for (label, front, back) in [
        ("plain", &plain_front, &plain_back),
        ("normal-mapped", &mapped_front, &mapped_back),
    ] {
        let (mean, frac) = mirror_error(back, front);
        assert!(
            mean <= 0.5 && frac <= 0.005,
            "{label}: the back view must equal the front view mirrored: mean {mean:.3} (≤ 0.5), \
             fraction > 8: {frac:.4} (≤ 0.005)",
        );
    }
    // The fixture bites: the normal map changes the shading, and a single-sided sheet seen from
    // behind is culled down to the background.
    let (map_effect, _) = image_error(&mapped_front, &plain_front);
    assert!(
        map_effect > 1.0,
        "the normal map must act (mean diff {map_effect:.3})"
    );
    let culled = render(true, sheet(0xB0_03, false, None));
    let empty = renderer
        .render(
            &{
                let mut scene = sheet_scene(w, h, true, sheet(0xB0_03, false, None));
                scene.meshes.clear();
                scene
            },
            FrameSize::native(w, h),
        )
        .expect("empty render");
    let centre = ((h / 2 * w + w / 2) * 4) as usize;
    assert_eq!(
        culled.rgba[centre..centre + 4],
        empty.rgba[centre..centre + 4]
    );
    assert_ne!(
        plain_back.rgba[centre..centre + 4],
        empty.rgba[centre..centre + 4],
        "the double-sided sheet must draw from behind",
    );
}

/// A double-sided quad turned to show its back, reflected through x = 0, still renders as the exact
/// mirror image — scene pass and SSAO prepass.
#[test]
fn needs_gpu_a_mirrored_double_sided_back_face_renders_as_the_mirror_image() {
    let Some(renderer) =
        skip_or_renderer("a_mirrored_double_sided_back_face_renders_as_the_mirror_image")
    else {
        return;
    };
    let (w, h) = (96u32, 64u32);
    for ssao in [false, true] {
        let render = |mirror: bool| {
            // A mirrored double-sided draw keeps its own `Cw`, cull-none variant.
            let scene = mirror_scene_with(w, h, mirror, ssao, 160.0, true);
            renderer
                .render(&scene, FrameSize::native(w, h))
                .expect("render")
        };
        let (plain, mirrored) = (render(false), render(true));
        let (mean, frac) = mirror_error(&mirrored, &plain);
        assert!(
            mean <= 0.5 && frac <= 0.005,
            "ssao={ssao}: mean abs diff {mean:.3} (≤ 0.5), fraction > 8: {frac:.4} (≤ 0.005)",
        );
    }
}

/// A vertical plane on a floor; `plane` is asset 0, standing on `y = 0`.
fn cutout_scene(w: u32, h: u32, ssao: bool, plane: MeshData) -> RenderScene {
    let mut scene = RenderScene::empty((w, h));
    let position = Vec3::new(0.0, 2.5, 6.0);
    scene.camera = crate::scene::RenderCamera {
        view: Mat4::look_at_rh(position, Vec3::new(0.0, 1.0, 0.0), Vec3::Y),
        projection: Mat4::perspective_rh(
            50.0_f32.to_radians(),
            crate::scene::aspect_ratio((w, h)),
            0.1,
            1000.0,
        ),
        position,
    };
    scene.ambient = [0.5; 3];
    if ssao {
        scene.post = Some(ao_post(1.0, 2.0));
    }
    scene.mesh_assets.push(Arc::new(plane));
    let mut instance = skin_instance(Mat4::from_translation(Vec3::new(0.0, 1.0, 0.0)), 0);
    instance.material = [1.0, 0.0, 0.0];
    scene.meshes = vec![ground(20.0), instance];
    scene
}

#[test]
fn needs_gpu_a_cutout_discards_in_the_scene_pass_and_the_ssao_prepass() {
    let Some(renderer) =
        skip_or_renderer("a_cutout_discards_in_the_scene_pass_and_the_ssao_prepass")
    else {
        return;
    };
    let (w, h) = (96u32, 64u32);
    // The cutout: left half alpha 0, right half opaque, mask at 0.5.
    let masked = sheet(
        0xC0_01,
        false,
        Some(cutout_textures(Some(half_alpha_map(0xC0_10, 0)), Some(0.5))),
    );
    // The reference: only the opaque right half exists, with no mask.
    let cropped = MeshData {
        positions: vec![
            0.0, -1.0, 0.0, //
            1.0, -1.0, 0.0, //
            1.0, 1.0, 0.0, //
            0.0, 1.0, 0.0,
        ],
        uvs: vec![0.5, 1.0, 1.0, 1.0, 1.0, 0.0, 0.5, 0.0],
        ..sheet(
            0xC0_02,
            false,
            Some(cutout_textures(Some(half_alpha_map(0xC0_11, 255)), None)),
        )
    };
    // The same plane with the mask cleared draws its transparent half as opaque.
    let unmasked = sheet(
        0xC0_03,
        false,
        Some(cutout_textures(Some(half_alpha_map(0xC0_10, 0)), None)),
    );
    for ssao in [false, true] {
        let render = |plane: &MeshData| {
            renderer
                .render(
                    &cutout_scene(w, h, ssao, plane.clone()),
                    FrameSize::native(w, h),
                )
                .expect("cutout render")
        };
        let (reference, cut, full) = (render(&cropped), render(&masked), render(&unmasked));
        let (mean, frac) = image_error(&cut, &reference);
        // Tight on purpose: a prepass that kept the phantom half would darken the floor crease
        // under it by a mean 0.26 and 0.5 % of the frame.
        assert!(
            mean <= 0.1 && frac <= 0.001,
            "ssao={ssao}: the cutout must match the cropped plane: mean {mean:.3} (≤ 0.1), \
             fraction > 8: {frac:.4} (≤ 0.001)",
        );
        let (_, guard) = image_error(&full, &reference);
        assert!(
            guard > 0.02,
            "ssao={ssao}: the unmasked plane must differ from the reference (fraction {guard:.4})",
        );
    }
}

#[test]
fn needs_gpu_a_map_less_mask_cuts_on_the_vertex_and_instance_alpha() {
    let Some(renderer) = skip_or_renderer("a_map_less_mask_cuts_on_the_vertex_and_instance_alpha")
    else {
        return;
    };
    let (w, h) = (96u32, 64u32);
    let plane = sheet(0xD0_01, false, Some(cutout_textures(None, Some(0.5))));
    let with_alpha = |alpha: f32| {
        let mut scene = cutout_scene(w, h, false, plane.clone());
        scene.meshes[1].color = [1.0, 1.0, 1.0, alpha];
        renderer
            .render(&scene, FrameSize::native(w, h))
            .expect("map-less render")
    };
    let mut without = cutout_scene(w, h, false, plane.clone());
    without.meshes.pop();
    let without = renderer
        .render(&without, FrameSize::native(w, h))
        .expect("render without");
    assert_eq!(
        with_alpha(0.2).rgba,
        without.rgba,
        "coverage 0.2 < cutoff 0.5 must discard every fragment",
    );
    assert_ne!(with_alpha(0.8).rgba, without.rgba, "coverage 0.8 must draw");
}

// --- Cutout and double-sided shadow casters ----------------------------------------------

/// A mask quad (cutoff 0.5) whose base map has alpha `left` over its left half and `right` over
/// its right half. `hash` keys the mesh cache, so each distinct content needs its own.
fn mask_quad(hash: u64, left: u8, right: u8) -> MeshData {
    sheet(
        hash,
        false,
        Some(cutout_textures(
            Some(alpha_map(hash ^ 0x005E_EDA1, left, right)),
            Some(0.5),
        )),
    )
}

/// `quad` as the scene's only asset, with one instance at `model` (scalar material, so lit).
fn add_quad(scene: &mut RenderScene, quad: MeshData, model: Mat4) {
    scene.mesh_assets.push(Arc::new(quad));
    let mut instance = skin_instance(model, 0);
    instance.material = [1.0, 0.0, 0.0];
    instance.normal_matrix = Mat3::from_mat4(model).inverse().transpose();
    scene.meshes.push(instance);
}

/// The quad laid flat at height `y`, facing up, spanning `±scale` in x and z.
fn flat_quad_model(y: f32, scale: f32) -> Mat4 {
    Mat4::from_translation(Vec3::new(0.0, y, 0.0))
        * Mat4::from_rotation_x(-std::f32::consts::FRAC_PI_2)
        * Mat4::from_scale(Vec3::splat(scale))
}

/// [`needs_gpu_occluder_shadows_the_receiver`]'s ground and camera under a straight-down sun,
/// with the sun's cascades on when asked.
fn cutout_sun_scene(w: u32, h: u32, cascades: bool) -> RenderScene {
    let mut scene = RenderScene::empty((w, h));
    scene.ambient = [0.05; 3];
    let position = Vec3::new(0.0, 3.0, 11.0);
    scene.camera = crate::scene::RenderCamera {
        view: Mat4::look_at_rh(position, Vec3::ZERO, Vec3::Y),
        projection: Mat4::perspective_rh(60.0_f32.to_radians(), w as f32 / h as f32, 0.1, 100.0),
        position,
    };
    let mut sun = test_sun(Vec3::NEG_Y, Vec3::splat(3.0), 0.002);
    if cascades {
        sun.cascades = Some(crate::scene::SunCascadeConfig {
            count: 4,
            shadow_distance: 60.0,
            splits: None,
            shadow_near: -60.0,
            shadow_far: 60.0,
        });
    }
    scene.sun = Some(sun);
    scene.meshes = vec![ground(20.0)];
    scene
}

/// **A mask mesh casts only its opaque texels into the sun map**, single map and cascades alike.
/// A quad laid flat above the floor, its left half cut and its right half opaque: the floor under
/// the opaque half is dark against the frame without the quad, the floor under the cut half is
/// not. The quad is edge-on to the camera, so only the floor's luma moves.
#[test]
fn needs_gpu_a_cutout_casts_only_its_opaque_texels_into_the_sun_map() {
    let Some(renderer) = skip_or_renderer("a_cutout_casts_only_its_opaque_texels") else {
        return;
    };
    let (w, h) = (128u32, 96u32);
    let size = FrameSize::native(w, h);
    for (n, cascades) in [false, true].into_iter().enumerate() {
        let bare_scene = cutout_sun_scene(w, h, cascades);
        let bare = renderer.render(&bare_scene, size).expect("bare render");
        let mut scene = bare_scene.clone();
        add_quad(
            &mut scene,
            mask_quad(0xF1_00 + n as u64, 0, 255),
            flat_quad_model(3.0, 3.0),
        );
        let shadowed = renderer.render(&scene, size).expect("cutout render");

        let (cx, cy) = pixel_of(&scene, Vec3::new(-1.5, 0.0, 0.0), w, h);
        let (ox, oy) = pixel_of(&scene, Vec3::new(1.5, 0.0, 0.0), w, h);
        assert!(
            luma(&bare, ox, oy) > 60,
            "cascades={cascades}: the floor is lit"
        );
        assert!(
            luma(&shadowed, ox, oy) + 40 < luma(&bare, ox, oy),
            "cascades={cascades}: the floor under the opaque half is dark: {} vs {}",
            luma(&bare, ox, oy),
            luma(&shadowed, ox, oy),
        );
        assert!(
            luma(&shadowed, cx, cy).abs_diff(luma(&bare, cx, cy)) <= 2,
            "cascades={cascades}: the floor under the cut half stays lit: {} vs {}",
            luma(&bare, cx, cy),
            luma(&shadowed, cx, cy),
        );
    }
}

/// **A double-sided sheet casts from either side.** An opaque quad facing the floor, away from a
/// straight-down sun, casts nothing when single-sided (the guard) and a full shadow when
/// double-sided, as a plain mesh and as a mask, and also under a mirroring scale.
#[test]
fn needs_gpu_a_double_sided_sheet_casts_from_its_back() {
    let Some(renderer) = skip_or_renderer("a_double_sided_sheet_casts_from_its_back") else {
        return;
    };
    let (w, h) = (128u32, 96u32);
    let size = FrameSize::native(w, h);
    let bare_scene = cutout_sun_scene(w, h, false);
    let bare = renderer.render(&bare_scene, size).expect("bare render");
    let probes = [Vec3::new(-1.5, 0.0, 0.0), Vec3::new(1.5, 0.0, 0.0)];
    for mask in [false, true] {
        for (double_sided, scale_x) in [(false, 3.0), (true, 3.0), (true, -3.0)] {
            let hash = 0xF2_00
                + u64::from(mask) * 4
                + u64::from(double_sided) * 2
                + u64::from(scale_x < 0.0);
            let textures = mask
                .then(|| cutout_textures(Some(alpha_map(hash ^ 0x005E_EDA2, 255, 255)), Some(0.5)));
            let mut scene = bare_scene.clone();
            add_quad(
                &mut scene,
                sheet(hash, double_sided, textures),
                Mat4::from_translation(Vec3::new(0.0, 3.0, 0.0))
                    * Mat4::from_rotation_x(std::f32::consts::FRAC_PI_2)
                    * Mat4::from_scale(Vec3::new(scale_x, 3.0, 3.0)),
            );
            let image = renderer.render(&scene, size).expect("sheet render");
            for probe in probes {
                let (x, y) = pixel_of(&scene, probe, w, h);
                let (lit, now) = (luma(&bare, x, y), luma(&image, x, y));
                let label = format!("mask={mask} double_sided={double_sided} scale_x={scale_x}");
                if double_sided {
                    assert!(
                        now + 40 < lit,
                        "{label}: the back face casts: {lit} vs {now}"
                    );
                } else {
                    assert!(
                        now.abs_diff(lit) <= 2,
                        "{label}: a culled back face casts nothing: {lit} vs {now}",
                    );
                }
            }
        }
    }
}

/// **The shafts inherit the cutout through the sun map.** A vertical mask quad across the wall's
/// slot, facing the sun: fully cut it changes no byte, fully opaque it closes the beam, and cut on
/// one half only it leaves the beam between the two.
#[test]
fn needs_gpu_the_shafts_inherit_a_cutout_through_the_sun_map() {
    let Some(renderer) = skip_or_renderer("the_shafts_inherit_a_cutout") else {
        return;
    };
    let (w, h) = (96u32, 96u32);
    let size = FrameSize::native(w, h);
    let mut open_scene = shafts_scene(w, h);
    open_scene.post = Some(shafts_post(8.0));
    let open = renderer.render(&open_scene, size).expect("open slot");
    let with_quad = |hash: u64, left: u8, right: u8| {
        let mut scene = open_scene.clone();
        // Facing the sun (-Z), slightly wider than the slot and as tall as the slabs.
        add_quad(
            &mut scene,
            mask_quad(hash, left, right),
            Mat4::from_translation(Vec3::new(0.0, 2.0, -3.0))
                * Mat4::from_rotation_y(std::f32::consts::PI)
                * Mat4::from_scale(Vec3::new(1.02, 12.0, 1.0)),
        );
        renderer.render(&scene, size).expect("quad render")
    };
    let beam = |img: &RenderedImage| column_luma(img, 0, w / 2);

    let cut = with_quad(0xF3_01, 0, 0);
    assert_eq!(cut.rgba, open.rgba, "a fully cut quad changes nothing");
    let closed = with_quad(0xF3_02, 255, 255);
    assert!(
        beam(&closed) + 8.0 < beam(&open),
        "an opaque quad closes the beam: {:.2} vs {:.2}",
        beam(&open),
        beam(&closed),
    );
    let half = with_quad(0xF3_03, 0, 255);
    assert!(
        beam(&closed) < beam(&half) && beam(&half) < beam(&open),
        "a half-cut quad passes half the beam: {:.2} < {:.2} < {:.2}",
        beam(&closed),
        beam(&half),
        beam(&open),
    );
}

/// **An opaque asset caster is unmoved by the cutout pipeline.** A mask at cutoff 0 never
/// discards, so an opaque quad renders byte-identically with `alpha_cutoff: Some(0.0)` (the cutout
/// caster) and `None` (the depth-only one), under the sun map, a granted spot and a drop shadow.
#[test]
fn needs_gpu_an_opaque_caster_is_byte_identical_through_the_cutout_pipeline() {
    let Some(renderer) = skip_or_renderer("an_opaque_caster_is_byte_identical") else {
        return;
    };
    let (w, h) = (128u32, 96u32);
    let size = FrameSize::native(w, h);
    let opaque = |hash: u64, cutoff: Option<f32>| {
        sheet(
            hash,
            false,
            Some(cutout_textures(
                Some(alpha_map(hash ^ 0x005E_EDA4, 255, 255)),
                cutoff,
            )),
        )
    };
    type Build = fn(u32, u32, MeshData) -> RenderScene;
    let cases: [(&str, Build); 3] = [
        ("sun", |w, h, quad| {
            let mut scene = cutout_sun_scene(w, h, false);
            add_quad(&mut scene, quad, flat_quad_model(3.0, 3.0));
            scene
        }),
        ("spot", |w, h, quad| {
            let mut scene = local_receive_scene(w, h);
            add_quad(&mut scene, quad, flat_quad_model(3.0, 3.0));
            scene.spot_lights = vec![casting_spot(Vec3::new(0.0, 8.0, 0.0), 20.0, true)];
            scene
        }),
        ("drop shadow", |w, h, quad| {
            let mut scene = drop_shadow_fixture(w, h, false);
            add_quad(
                &mut scene,
                quad,
                Mat4::from_translation(Vec3::new(1.0, 0.5, 0.0))
                    * Mat4::from_rotation_x(-std::f32::consts::FRAC_PI_2)
                    * Mat4::from_scale(Vec3::new(1.0, 2.0, 1.0)),
            );
            scene.drop_shadows.push(fixture_drop_shadow(Vec::new()));
            scene
        }),
    ];
    for (n, (name, build)) in cases.into_iter().enumerate() {
        let depth_only = renderer
            .render(&build(w, h, opaque(0xF4_00 + 2 * n as u64, None)), size)
            .expect("depth-only render");
        let cutout = renderer
            .render(
                &build(w, h, opaque(0xF4_01 + 2 * n as u64, Some(0.0))),
                size,
            )
            .expect("cutout render");
        assert_eq!(
            depth_only.rgba, cutout.rgba,
            "{name}: a never-discarding cutout caster must match the depth-only one",
        );
    }
}

/// **A granted spot's shadow map holds the cutout too.** A flat mask quad under the light, its
/// left half cut: against the frame with the spot's shadow off, the floor in the opaque half's
/// shadow is dark and the floor beyond the cut half's footprint is untouched.
#[test]
fn needs_gpu_a_local_shadow_casts_only_the_opaque_texels() {
    let Some(renderer) = skip_or_renderer("a_local_shadow_casts_only_the_opaque_texels") else {
        return;
    };
    let (w, h) = (192u32, 108u32);
    let size = FrameSize::native(w, h);
    let mut scene = local_receive_scene(w, h);
    add_quad(
        &mut scene,
        mask_quad(0xF5_01, 0, 255),
        flat_quad_model(3.0, 3.0),
    );
    scene.spot_lights = vec![casting_spot(Vec3::new(0.0, 8.0, 0.0), 20.0, false)];
    let plain = renderer.render(&scene, size).expect("an unshadowed render");
    scene.spot_lights = vec![casting_spot(Vec3::new(0.0, 8.0, 0.0), 20.0, true)];
    let shadowed = renderer.render(&scene, size).expect("a shadowed render");

    let at = |p: Vec3| pixel_of(&scene, p, w, h);
    let (ox, oy) = at(Vec3::new(4.0, 0.0, 0.0));
    let (before, after) = (luma(&plain, ox, oy), luma(&shadowed, ox, oy));
    assert!(before > 40, "the floor beyond the quad is lit: {before}");
    assert!(
        after * 10 < before,
        "the floor in the opaque half's shadow is dark: {before} -> {after}",
    );
    let (cx, cy) = at(Vec3::new(-4.0, 0.0, 0.0));
    assert!(
        luma(&shadowed, cx, cy).abs_diff(luma(&plain, cx, cy)) <= 2,
        "the floor beyond the cut half stays lit: {} vs {}",
        luma(&plain, cx, cy),
        luma(&shadowed, cx, cy),
    );
}

/// **The drop-shadow occlusion pass honours the cutout, and the first-surface rule holds.** A
/// quad laid over half the footprint: fully cut it is no surface at all, so the shadow reaches the
/// floor exactly as in a frame without it; opaque it takes the shadow and spares the floor.
#[test]
fn needs_gpu_a_drop_shadow_sees_through_a_cutout() {
    let Some(renderer) = skip_or_renderer("a_drop_shadow_sees_through_a_cutout") else {
        return;
    };
    let (w, h) = (160u32, 120u32);
    let size = FrameSize::native(w, h);
    let quad_model = Mat4::from_translation(Vec3::new(1.0, 0.5, 0.0))
        * Mat4::from_rotation_x(-std::f32::consts::FRAC_PI_2)
        * Mat4::from_scale(Vec3::new(1.0, 2.0, 1.0));
    let scene_with = |quad: Option<MeshData>, drop: bool| {
        let mut scene = drop_shadow_fixture(w, h, false);
        if let Some(quad) = quad {
            add_quad(&mut scene, quad, quad_model);
        }
        if drop {
            scene.drop_shadows.push(fixture_drop_shadow(Vec::new()));
        }
        scene
    };
    let floor = pixel_of(&scene_with(None, false), Vec3::new(0.8, 0.0, 0.5), w, h);
    let floor_luma = |scene: &RenderScene| {
        luma(
            &renderer.render(scene, size).expect("render"),
            floor.0,
            floor.1,
        )
    };

    let bare = floor_luma(&scene_with(None, false));
    let plateless = floor_luma(&scene_with(None, true));
    assert!(
        plateless * 10 < bare * 6,
        "the guard: {bare} -> {plateless}"
    );
    assert_eq!(
        floor_luma(&scene_with(Some(mask_quad(0xF6_01, 0, 0)), true)),
        plateless,
        "a fully cut quad lets the shadow through to the floor",
    );
    let opaque_bare = floor_luma(&scene_with(Some(mask_quad(0xF6_02, 255, 255)), false));
    assert_eq!(
        floor_luma(&scene_with(Some(mask_quad(0xF6_02, 255, 255)), true)),
        opaque_bare,
        "an opaque quad is the first surface, so the floor beneath keeps its light",
    );
}

/// A closed box of half-extent 1: six outward-facing quads, counter-clockwise from outside.
fn box_asset(hash: u64, double_sided: bool) -> MeshData {
    let mut mesh = MeshData {
        positions: Vec::new(),
        normals: Vec::new(),
        colors: Vec::new(),
        materials: Vec::new(),
        uvs: Vec::new(),
        tangents: Vec::new(),
        weights: Vec::new(),
        bone_indices: Vec::new(),
        indices: Vec::new(),
        ..sheet(hash, double_sided, None)
    };
    for (n, u) in [
        (Vec3::X, Vec3::Y),
        (Vec3::NEG_X, Vec3::Y),
        (Vec3::Y, Vec3::Z),
        (Vec3::NEG_Y, Vec3::Z),
        (Vec3::Z, Vec3::X),
        (Vec3::NEG_Z, Vec3::X),
    ] {
        let v = n.cross(u);
        let base = (mesh.positions.len() / 3) as u32;
        for (su, sv, uv) in [
            (-1.0, -1.0, [0.0, 1.0]),
            (1.0, -1.0, [1.0, 1.0]),
            (1.0, 1.0, [1.0, 0.0]),
            (-1.0, 1.0, [0.0, 0.0]),
        ] {
            mesh.positions.extend((n + u * su + v * sv).to_array());
            mesh.normals.extend(n.to_array());
            mesh.colors.extend([1.0; 4]);
            mesh.materials.extend([1.0, 0.0, 0.0]);
            mesh.uvs.extend(uv);
            mesh.tangents.extend([u.x, u.y, u.z, 1.0]);
            mesh.weights.extend([1.0, 0.0, 0.0, 0.0]);
            mesh.bone_indices.extend([0; 4]);
        }
        mesh.indices
            .extend([base, base + 1, base + 2, base, base + 2, base + 3]);
    }
    mesh
}

/// A clear bit keeps today's arithmetic: a closed box seen from outside, and a normal-mapped
/// opaque quad, render byte-identically through every variant of the new material flags.
#[test]
fn needs_gpu_opaque_meshes_render_identically_with_and_without_the_new_flags() {
    let Some(renderer) =
        skip_or_renderer("opaque_meshes_render_identically_with_and_without_the_new_flags")
    else {
        return;
    };
    let (w, h) = (96u32, 64u32);
    for ssao in [false, true] {
        // Each variant has its own hash: the mesh-asset cache is keyed on it, so a shared hash
        // would render the second frame from the first's cached GPU mesh.
        let box_frame = |double_sided: bool, inside: bool| {
            let hash = 0xE0_01 + 2 * u64::from(double_sided);
            let mut scene = cutout_scene(w, h, ssao, box_asset(hash, double_sided));
            scene.sun = Some(test_sun(
                Vec3::new(0.3, -1.0, -0.5),
                Vec3::splat(3.0),
                0.002,
            ));
            scene.meshes[1].model = Mat4::from_translation(Vec3::new(0.0, 1.0, 0.0))
                * Mat4::from_rotation_y(30.0_f32.to_radians())
                * Mat4::from_scale(Vec3::splat(0.8));
            scene.meshes[1].normal_matrix =
                Mat3::from_mat4(scene.meshes[1].model).inverse().transpose();
            if inside {
                let eye = Vec3::new(0.0, 1.0, 0.0);
                scene.camera.view = Mat4::look_at_rh(eye, eye - Vec3::Z, Vec3::Y);
                scene.camera.position = eye;
            }
            renderer
                .render(&scene, FrameSize::native(w, h))
                .expect("box render")
        };
        assert_eq!(
            box_frame(true, false).rgba,
            box_frame(false, false).rgba,
            "ssao={ssao}: a closed box must not change with double_sided",
        );
        // The pair differs in setup: from inside, only the double-sided box draws its walls.
        assert_ne!(
            box_frame(true, true).rgba,
            box_frame(false, true).rgba,
            "ssao={ssao}: the double-sided box must draw from inside",
        );
        let mapped = |cutoff: Option<f32>| {
            let hash = 0xE0_02 + 2 * u64::from(cutoff.is_some());
            let mut textures = normal_mapped_quad(0).textures.unwrap();
            textures.alpha_cutoff = cutoff;
            renderer
                .render(
                    &cutout_scene(w, h, ssao, sheet(hash, false, Some(textures))),
                    FrameSize::native(w, h),
                )
                .expect("mapped render")
        };
        assert_eq!(
            mapped(Some(0.0)).rgba,
            mapped(None).rgba,
            "ssao={ssao}: a mask at cutoff 0 never discards",
        );
    }
}

/// The pixel a world point projects to, so a probe can name a place in the *scene* rather than a
/// coordinate that silently stops meaning anything when the framing moves.
fn project(camera: &crate::scene::RenderCamera, world: Vec3, w: u32, h: u32) -> (u32, u32) {
    let clip = camera.projection * camera.view * world.extend(1.0);
    let ndc = clip.truncate() / clip.w;
    let x = ((ndc.x * 0.5 + 0.5) * w as f32)
        .round()
        .clamp(0.0, (w - 1) as f32);
    let y = ((0.5 - ndc.y * 0.5) * h as f32)
        .round()
        .clamp(0.0, (h - 1) as f32);
    (x as u32, y as u32)
}

/// The slot floor (walls half a unit away on both sides), and open ground well past their end.
const AO_CREASE: Vec3 = Vec3::new(0.0, 0.0, 0.0);
const AO_OPEN: Vec3 = Vec3::new(0.0, 0.0, 5.0);
/// The radius the fixture's assertions are calibrated at. Large next to the slot's own width
/// because these frames are 128 px: the AO chain runs at half of that and its 4×4 blur is then a
/// meaningful fraction of the slot, which flattens the estimate at a size no real frame renders
/// at. The knob's *direction* is what
/// [`ssao_strength_deepens_and_radius_reaches_further`] pins; this constant only has to make the
/// signal legible.
const AO_RADIUS: f32 = 4.0;

/// **The central assertion** (`Plan/post-process-pipeline.md` decision 8). AO multiplies
/// the ambient + IBL term and nothing else, so:
///
/// - with an ambient term, a crease darkens and open ground barely moves;
/// - with the ambient term at zero, an **emissive** surface is byte-identical with AO on;
/// - with the ambient term at zero, a **directly lit** surface is byte-identical with AO on.
///
/// The last two are whole-image equalities rather than probe comparisons: if AO leaked into
/// emissive or direct light by even one ULP, they would fail. Each is guarded against passing
/// vacuously — the probes must be lit, and the ambient case must genuinely darken.
#[test]
fn needs_gpu_ao_darkens_an_ambient_crease_and_leaves_emissive_and_direct_light_exact() {
    let Some(renderer) =
        skip_or_renderer("ao_darkens_an_ambient_crease_and_leaves_emissive_and_direct_light_exact")
    else {
        return;
    };
    let (w, h) = (128u32, 128u32);
    let size = FrameSize::native(w, h);

    // (1) Ambient-lit: the crease must darken, and open ground must not follow it down.
    let mut ambient = ao_scene(w, h, [0.5, 0.5, 0.5], 0.0, false);
    let off = renderer.render(&ambient, size).expect("ambient, AO off");
    ambient.post = Some(ao_post(1.0, AO_RADIUS));
    let on = renderer.render(&ambient, size).expect("ambient, AO on");

    let crease = project(&ambient.camera, AO_CREASE, w, h);
    let open = project(&ambient.camera, AO_OPEN, w, h);
    let (crease_off, crease_on) = (
        luma(&off, crease.0, crease.1),
        luma(&on, crease.0, crease.1),
    );
    let (open_off, open_on) = (luma(&off, open.0, open.1), luma(&on, open.0, open.1));
    assert!(
        crease_off > 60,
        "the fixture's crease must be lit before AO can darken it: {crease_off}",
    );
    // An eighth, not a sixth. The margin is display-referred, so it is a function of the shipped
    // tonemap operator as much as of the AO factor: AgX is gentler than the Narkowicz ACES fit
    // through this fixture's range, so the same linear multiplier reads as a smaller drop in
    // output bytes (measured 489 -> 408 here, 16.6%). AO's own strength did not change.
    assert!(
        crease_on * 8 < crease_off * 7,
        "AO must take at least an eighth off the crease: {crease_off} -> {crease_on}",
    );
    assert!(
        open_on * 10 > open_off * 9,
        "open ground has nothing above it and must stay within a tenth: {open_off} -> {open_on}",
    );

    // (2) Emissive, ambient at zero: the whole term AO touches is exactly zero, so the image must
    // not move by a single byte.
    let mut emissive = ao_scene(w, h, [0.0, 0.0, 0.0], 0.6, false);
    let emissive_off = renderer.render(&emissive, size).expect("emissive, AO off");
    emissive.post = Some(ao_post(1.0, AO_RADIUS));
    let emissive_on = renderer.render(&emissive, size).expect("emissive, AO on");
    assert!(
        luma(&emissive_off, crease.0, crease.1) > 60,
        "the emissive fixture must actually glow",
    );
    assert_eq!(
        emissive_off.rgba, emissive_on.rgba,
        "ambient occlusion must not touch an emissive surface",
    );

    // (3) Direct point light, ambient at zero: same claim for the light AO must never dim.
    let mut direct = ao_scene(w, h, [0.0, 0.0, 0.0], 0.0, true);
    let direct_off = renderer.render(&direct, size).expect("direct, AO off");
    direct.post = Some(ao_post(1.0, AO_RADIUS));
    let direct_on = renderer.render(&direct, size).expect("direct, AO on");
    assert!(
        luma(&direct_off, crease.0, crease.1) > 60,
        "the direct-light fixture must actually be lit",
    );
    assert_eq!(
        direct_off.rgba, direct_on.rgba,
        "ambient occlusion must not touch a directly lit surface",
    );
}

/// `ssao_direct` is the bounded exception to the fixture above: at `0.0` the ambient-only rule is
/// exact to the byte, above it a *directly lit* crease darkens too, and emissive never moves at any
/// setting. The four claims are what separate "a knob that opens the rule by a stated fraction"
/// from "a knob that multiplies the finished image", which is the failure the split exists to
/// prevent.
///
/// Every case runs with the ambient term at **zero**, so anything that moves here moved through the
/// direct path and nowhere else.
#[test]
fn needs_gpu_ssao_direct_opens_the_ambient_only_rule_by_a_bounded_fraction() {
    let Some(renderer) =
        skip_or_renderer("ssao_direct_opens_the_ambient_only_rule_by_a_bounded_fraction")
    else {
        return;
    };
    let (w, h) = (128u32, 128u32);
    let size = FrameSize::native(w, h);
    let direct_post = |direct: f32| PostData {
        ssao_direct: direct,
        ..ao_post(1.0, AO_RADIUS)
    };
    // The shared fixture's lamp sits four units directly above the slot, which puts the crease
    // near the top of AgX's shoulder — where a real linear attenuation reads as a much smaller
    // byte drop (measured 690 -> 633, 8.3%, for the same factor the ambient fixture shows as
    // 16.6%). Dimming it moves the probe into the operator's linear part, so these assertions
    // measure the AO factor rather than the tonemap's compression.
    let direct_scene = || {
        let mut s = ao_scene(w, h, [0.0, 0.0, 0.0], 0.0, true);
        s.lights[0].intensity = 1.0;
        s
    };

    // (1) Point light, ambient zero. `ssao_direct: 0.0` must be byte-identical to AO off — the
    // field's neutrality, which is what lets it ship without moving a golden.
    let mut scene = direct_scene();
    let off = renderer.render(&scene, size).expect("direct, AO off");
    scene.post = Some(direct_post(0.0));
    let neutral = renderer
        .render(&scene, size)
        .expect("direct, ssao_direct 0");
    assert_eq!(
        off.rgba, neutral.rgba,
        "ssao_direct at zero must leave direct light exactly as the ambient-only rule does",
    );

    // (2) Above zero the crease darkens, and open ground — lit by the same lamp but with nothing
    // above it — must not follow it down. Without the second half a knob that simply dimmed every
    // lit pixel would pass.
    let crease = project(&scene.camera, AO_CREASE, w, h);
    let open = project(&scene.camera, AO_OPEN, w, h);
    let probe = |post: PostData, at: (u32, u32)| {
        let mut s = direct_scene();
        s.post = Some(post);
        luma(&renderer.render(&s, size).expect("AO render"), at.0, at.1)
    };
    let crease_off = luma(&off, crease.0, crease.1);
    let open_off = luma(&off, open.0, open.1);
    assert!(
        crease_off > 60,
        "the fixture's crease must be lit before AO can darken it: {crease_off}",
    );
    let crease_full = probe(direct_post(1.0), crease);
    let open_full = probe(direct_post(1.0), open);
    assert!(
        crease_full * 8 < crease_off * 7,
        "ssao_direct 1.0 must take at least an eighth off a directly lit crease: \
         {crease_off} -> {crease_full}",
    );
    assert!(
        open_full * 10 > open_off * 9,
        "open ground has nothing above it and must stay within a tenth: \
         {open_off} -> {open_full}",
    );

    // (3) It is a fraction, not a switch: a partial value must land strictly between the two ends.
    let crease_part = probe(direct_post(0.35), crease);
    assert!(
        crease_full < crease_part && crease_part < crease_off,
        "a partial ssao_direct must sit between the rule and the full multiply: \
         {crease_off} / {crease_part} / {crease_full}",
    );

    // (4) Emissive is outside the shaded term entirely, so a glowing prop must not dim even at the
    // full multiply — the "AO looks like dirt" failure this field is bounded to avoid.
    let mut emissive = ao_scene(w, h, [0.0, 0.0, 0.0], 0.6, false);
    let emissive_off = renderer.render(&emissive, size).expect("emissive, AO off");
    emissive.post = Some(direct_post(1.0));
    let emissive_on = renderer
        .render(&emissive, size)
        .expect("emissive, ssao_direct 1");
    assert!(
        luma(&emissive_off, crease.0, crease.1) > 60,
        "the emissive fixture must actually glow",
    );
    assert_eq!(
        emissive_off.rgba, emissive_on.rgba,
        "ssao_direct must not touch an emissive surface at any setting",
    );
}

/// The sun is the other direct lobe, and it is a separate multiply in the shader — so it gets its
/// own claim rather than riding the point-light fixture's. Ambient at zero, no point lights: the
/// only light in the frame is the sun, and `ssao_direct` must reach it.
#[test]
fn needs_gpu_ssao_direct_reaches_the_sun_as_well_as_the_point_lights() {
    let Some(renderer) =
        skip_or_renderer("ssao_direct_reaches_the_sun_as_well_as_the_point_lights")
    else {
        return;
    };
    let (w, h) = (128u32, 128u32);
    let size = FrameSize::native(w, h);
    let mut scene = ao_scene(w, h, [0.0, 0.0, 0.0], 0.0, false);
    // Steep enough to reach the slot floor between the two walls, so the probe is sunlit rather
    // than shadowed — a shadowed crease would darken for the wrong reason.
    scene.sun = Some(test_sun(
        Vec3::new(0.0, -1.0, -0.15),
        Vec3::new(1.0, 1.0, 1.0),
        0.002,
    ));
    let off = renderer.render(&scene, size).expect("sun, AO off");
    let crease = project(&scene.camera, AO_CREASE, w, h);
    let crease_off = luma(&off, crease.0, crease.1);
    assert!(
        crease_off > 60,
        "the crease must be sunlit before ssao_direct can darken it: {crease_off}",
    );

    scene.post = Some(PostData {
        ssao_direct: 0.0,
        ..ao_post(1.0, AO_RADIUS)
    });
    let neutral = renderer.render(&scene, size).expect("sun, ssao_direct 0");
    assert_eq!(
        off.rgba, neutral.rgba,
        "ssao_direct at zero must leave the sun exactly as the ambient-only rule does",
    );

    scene.post = Some(PostData {
        ssao_direct: 1.0,
        ..ao_post(1.0, AO_RADIUS)
    });
    let on = renderer.render(&scene, size).expect("sun, ssao_direct 1");
    let crease_on = luma(&on, crease.0, crease.1);
    assert!(
        crease_on * 8 < crease_off * 7,
        "ssao_direct must attenuate the sun in a crease: {crease_off} -> {crease_on}",
    );
}

/// Both knobs move the image the way their names promise: raising `ssao` deepens the crease, and
/// raising `ssao_radius` reaches further, darkening ground a small radius leaves alone. A formula
/// that merely "does something" fails the second half.
#[test]
fn needs_gpu_ssao_strength_deepens_and_radius_reaches_further() {
    let Some(renderer) = skip_or_renderer("ssao_strength_deepens_and_radius_reaches_further")
    else {
        return;
    };
    let (w, h) = (128u32, 128u32);
    let size = FrameSize::native(w, h);
    let mut scene = ao_scene(w, h, [0.5, 0.5, 0.5], 0.0, false);
    let crease = project(&scene.camera, AO_CREASE, w, h);
    // Ground just past the slot's mouth: inside a wide hemisphere, outside a narrow one.
    let reach = project(&scene.camera, Vec3::new(0.0, 0.0, 3.8), w, h);

    let mut probe = |post: PostData, at: (u32, u32)| {
        scene.post = Some(post);
        luma(
            &renderer.render(&scene, size).expect("AO render"),
            at.0,
            at.1,
        )
    };

    let weak = probe(ao_post(0.3, 1.0), crease);
    let strong = probe(ao_post(1.0, 1.0), crease);
    assert!(
        strong < weak,
        "a stronger ssao must darken the crease further: {weak} -> {strong}",
    );

    let narrow = probe(ao_post(1.0, 0.25), reach);
    let wide = probe(ao_post(1.0, 4.0), reach);
    assert!(
        wide < narrow,
        "a wider radius must reach ground a narrow one misses: {narrow} -> {wide}",
    );
}

/// Scale invariance, the shading half of the extent rule: the AO chain is sized from the *output*
/// extent and its radius is in world units, so how much a crease darkens is the same number at
/// scale 1 and scale 2. The comparison is the AO-on/AO-off ratio at each scale, because
/// supersampling legitimately changes the absolute pixel (it is a quality change) but must not
/// change the look.
#[test]
fn needs_gpu_ao_darkening_is_the_same_at_any_render_scale() {
    let Some(renderer) = skip_or_renderer("ao_darkening_is_the_same_at_any_render_scale") else {
        return;
    };
    let (w, h) = (128u32, 128u32);
    let mut scene = ao_scene(w, h, [0.5, 0.5, 0.5], 0.0, false);
    let crease = project(&scene.camera, AO_CREASE, w, h);

    let ratio = |size: FrameSize, scene: &mut RenderScene| {
        scene.post = None;
        let off = renderer.render(scene, size).expect("AO off");
        scene.post = Some(ao_post(1.0, AO_RADIUS));
        let on = renderer.render(scene, size).expect("AO on");
        luma(&on, crease.0, crease.1) as f64 / luma(&off, crease.0, crease.1).max(1) as f64
    };

    let native = ratio(FrameSize::native(w, h), &mut scene);
    let doubled = ratio(
        FrameSize::scaled(w, h, crate::RenderScale::new(2.0).unwrap()),
        &mut scene,
    );
    assert!(
        native < 0.85,
        "the fixture must darken enough for the ratio to mean something: {native}",
    );
    assert!(
        (native - doubled).abs() < 0.05,
        "a scale change is a quality change, never a look change: {native} vs {doubled}",
    );
}

// --- Light shafts (Plan/post-process-pipeline.md) --------------------------------------

/// The light-shaft fixture: a full-frame occluder at `z = -3` split by a two-unit vertical slot at
/// `x ∈ [-1, 1]`, the sun travelling toward the camera and slightly down, and the camera parked
/// **off to the side of the slot's beam** at `x = 4`, looking across it.
///
/// That offset is the whole design. The beam downstream of a slot is a vertical sheet, so a camera
/// sitting inside it would have every ray lit for its first few units — and with the medium's
/// extinction weighting the near end most, every pixel would brighten equally and the fixture
/// would prove nothing. From the side, the rays that swing left cross into the sheet and the rays
/// that swing right never do, so the image carries a lit band and a shadowed one, and it is the
/// shadow map that separates them. A bug adding a constant fails that.
///
/// Because the light travels *toward* the camera, every surface facing it is backlit, so the
/// bands are ambient-dark before the shafts arrive and the measurement is the marched volume
/// alone.
///
/// [`shafts_post`]'s extinction sets the march's `3 / σ_t` reach at fifteen units, which keeps
/// every ray — background ones included — inside `test_sun`'s frustum. Outside it a point counts
/// as lit (the mesh pass's convention), which would swamp the probes.
fn shafts_scene(w: u32, h: u32) -> RenderScene {
    let mut scene = RenderScene::empty((w, h));
    let position = Vec3::new(4.0, 1.2, 7.0);
    scene.camera = crate::scene::RenderCamera {
        view: Mat4::look_at_rh(position, Vec3::new(0.0, 0.5, -1.0), Vec3::Y),
        projection: Mat4::perspective_rh(
            60.0_f32.to_radians(),
            crate::scene::aspect_ratio((w, h)),
            0.1,
            1000.0,
        ),
        position,
    };
    // Travelling toward the camera (+Z) and down, so the view ray sits near the phase function's
    // forward lobe — the geometry a night scene's drama actually comes from.
    scene.sun = Some(test_sun(
        Vec3::new(0.0, -0.35, 1.0),
        Vec3::new(1.0, 1.0, 1.0),
        0.002,
    ));
    // Two slabs leaving a two-unit gap at x ∈ [-1, 1], each tall and wide enough to fill the frame
    // so the only light downstream of them is the slot's.
    let slab = |x: f32| {
        cube(
            Vec3::new(x, 2.0, -3.0),
            Vec3::new(9.0, 24.0, 0.5),
            [0.35, 0.35, 0.4, 1.0],
        )
    };
    scene.meshes = vec![slab(-5.5), slab(5.5)];
    scene
}

/// The fixture's medium at extinction `density`, with `intensity` on its directional term.
///
/// The ambient radiance is **black**, which is what leaves the probes reading the marched sun term
/// alone. It is a physical setting rather than a trick: a night volume with no ambient light to
/// scatter in-scatters nothing isotropically, and the extinction it still applies to the backlit
/// slabs is the very σ_t the march integrates.
fn medium_post(density: f32, intensity: f32) -> PostData {
    PostData {
        fog_density: density,
        fog_color: [0.0; 3],
        shafts: intensity,
        ..PostData::default()
    }
}

/// The fixture's shipped medium and strength. `0.2` puts the march's `3 / σ_t` reach at fifteen
/// world units — long enough for a leftward ray to cross the slot's sheet, short enough to stay
/// inside `test_sun`'s frustum — and the intensity is high because a single-scattering integral
/// over a thin medium is a small number by construction, not because the effect is weak.
fn shafts_post(intensity: f32) -> PostData {
    medium_post(0.2, intensity)
}

/// The mean luma of a full-height column band `x_from..x_to`, so a probe reads a beam rather than
/// a single pixel that a half-texel of sampling could move off.
fn column_luma(img: &RenderedImage, x_from: u32, x_to: u32) -> f64 {
    let mut sum = 0u64;
    for y in 0..img.height {
        for x in x_from..x_to {
            sum += u64::from(luma(img, x, y));
        }
    }
    sum as f64 / f64::from((x_to - x_from) * img.height)
}

/// **The central assertion.** The sun reaches the marched volume only through the wall's
/// slot, so with shafts on the middle of the frame brightens and the shadowed edge barely moves —
/// and raising `shafts` moves the middle monotonically further.
///
/// The edge half is what separates "the effect works" from "something added light everywhere": a
/// march that ignored the shadow map would brighten both bands equally and fail here.
#[test]
fn needs_gpu_shafts_brighten_the_unoccluded_direction_and_scale_with_intensity() {
    let Some(renderer) =
        skip_or_renderer("shafts_brighten_the_unoccluded_direction_and_scale_with_intensity")
    else {
        return;
    };
    let (w, h) = (96u32, 96u32);
    let size = FrameSize::native(w, h);
    let mut scene = shafts_scene(w, h);
    // Leftward rays cross the slot's sheet; rightward ones swing away from it and never enter.
    let beam = |img: &RenderedImage| column_luma(img, 0, w / 2);
    let shadowed = |img: &RenderedImage| column_luma(img, w - 8, w);

    let off = renderer.render(&scene, size).expect("shafts off");
    scene.post = Some(shafts_post(8.0));
    let on = renderer.render(&scene, size).expect("shafts on");

    assert!(
        beam(&on) > beam(&off) + 8.0,
        "the slot's beam must brighten: {:.2} -> {:.2}",
        beam(&off),
        beam(&on),
    );
    assert!(
        shadowed(&on) < shadowed(&off) + 2.0,
        "the shadowed edge must stay put: {:.2} -> {:.2}",
        shadowed(&off),
        shadowed(&on),
    );

    // Monotone in the authored intensity, and still an image the beam dominates.
    scene.post = Some(shafts_post(16.0));
    let stronger = renderer.render(&scene, size).expect("stronger shafts");
    assert!(
        beam(&stronger) > beam(&on),
        "a stronger `shafts` must brighten the beam further: {:.2} -> {:.2}",
        beam(&on),
        beam(&stronger),
    );
}

/// **Byte identity, on bytes, for the sunless case.** A scene asking for the medium's directional
/// term with no `DirectionalLight` has nothing to march, so the chain is not built and the frame
/// must be exactly the same medium's ambient term alone — the composite's optional input is a 1×1
/// stand-in and its uniform flag is zero.
///
/// The reference is the *same medium* with `shafts: 0`, not a scene with no `PostProcess`: the
/// medium's extinction is authored either way and is not what is on trial here. The guard against
/// passing vacuously is the third render: the same strength on the same scene with a sun added does
/// move pixels, so the equality above is a skip and not an effect that never worked.
#[test]
fn needs_gpu_shafts_on_a_sunless_scene_are_byte_identical_to_none() {
    let Some(renderer) = skip_or_renderer("shafts_on_a_sunless_scene_are_byte_identical_to_none")
    else {
        return;
    };
    let (w, h) = (64u32, 64u32);
    let size = FrameSize::native(w, h);
    let mut scene = shafts_scene(w, h);
    scene.sun = None;

    scene.post = Some(shafts_post(0.0));
    let unasked = renderer
        .render(&scene, size)
        .expect("the medium, no shafts");
    scene.post = Some(shafts_post(8.0));
    let asked = renderer.render(&scene, size).expect("shafts, no sun");
    assert_eq!(
        unasked.rgba, asked.rgba,
        "shafts asked for without a sun must not move a single byte",
    );

    let mut sunny = shafts_scene(w, h);
    sunny.post = Some(shafts_post(8.0));
    assert_ne!(
        renderer.render(&sunny, size).expect("shafts, sun").rgba,
        asked.rgba,
        "the same strength with a sun must render differently — else nothing was proved",
    );
}

/// Scale invariance: both shaft targets are a fraction of the *output* extent and the march works
/// in world units, so how much the beam brightens is the same number at scale 1 and scale 2. The
/// comparison is the on/off ratio at each scale, because supersampling legitimately changes the
/// absolute pixel (a quality change) but must never change the look.
#[test]
fn needs_gpu_shaft_brightening_is_the_same_at_any_render_scale() {
    let Some(renderer) = skip_or_renderer("shaft_brightening_is_the_same_at_any_render_scale")
    else {
        return;
    };
    let (w, h) = (96u32, 96u32);
    let mut scene = shafts_scene(w, h);
    let beam = |img: &RenderedImage| column_luma(img, w / 2 - 4, w / 2 + 4);

    let ratio = |size: FrameSize, scene: &mut RenderScene| {
        scene.post = None;
        let off = renderer.render(scene, size).expect("shafts off");
        scene.post = Some(shafts_post(8.0));
        let on = renderer.render(scene, size).expect("shafts on");
        beam(&on) / beam(&off).max(1.0)
    };

    let native = ratio(FrameSize::native(w, h), &mut scene);
    let doubled = ratio(
        FrameSize::scaled(w, h, crate::RenderScale::new(2.0).unwrap()),
        &mut scene,
    );
    assert!(
        native > 1.2,
        "the fixture must brighten enough for the ratio to mean something: {native}",
    );
    assert!(
        (native - doubled).abs() < 0.05,
        "a scale change is a quality change, never a look change: {native} vs {doubled}",
    );
}

// --- One medium: shared extinction, albedo, anisotropy (Plan/volumetric-medium.md) -----

/// **The medium is inert until its extinction is authored — and it is skipped, not computed.** The
/// scene here authors an albedo, an anisotropy *and* a strong `shafts` over a sun, with
/// `fog_density` at zero. Two things have to hold, and the second is why the first is not the
/// interesting half:
///
/// - the frame is byte-identical to no `PostProcess` at all;
/// - `PostChain::for_scene` derives `shafts: false`, so the march and its blur are **not scheduled**.
///
/// Without the gate the pixels would still match — σ_s = α·σ_t is exactly zero and `hdr + 0.0` is
/// exact in IEEE — so a pixel assertion alone would pass over thirty-two marched steps and a 4×4
/// blur spent producing that zero. The arc's rule is that a neutral effect is skipped rather than
/// computed, which is what `ssao` and `dof` already do with their own neutrals, so the chain
/// assertion is the one that pins the behaviour and the pixels merely confirm it.
#[test]
fn needs_gpu_the_medium_is_inert_until_its_extinction_is_authored() {
    let Some(renderer) = skip_or_renderer("the_medium_is_inert_until_its_extinction_is_authored")
    else {
        return;
    };
    let (w, h) = (64u32, 64u32);
    let size = FrameSize::native(w, h);
    let mut scene = shafts_scene(w, h);
    let empty = PostData {
        fog_density: 0.0,
        fog_albedo: 0.25,
        fog_anisotropy: -0.5,
        shafts: 8.0,
        ..PostData::default()
    };

    let absent = renderer.render(&scene, size).expect("no PostProcess");
    scene.post = Some(empty);
    assert!(
        !PostChain::for_scene(&scene).shafts,
        "an empty medium must not schedule the march at all",
    );
    let empty_medium = renderer.render(&scene, size).expect("an empty medium");
    assert_eq!(
        absent.rgba, empty_medium.rgba,
        "an empty medium must not move a single byte, however it is described",
    );

    // Not vacuous: the same description with an extinction schedules the pair and moves the frame,
    // so the two assertions above are the medium being empty rather than the fields never being
    // read.
    scene.post = Some(PostData {
        fog_density: 0.2,
        ..empty
    });
    assert!(
        PostChain::for_scene(&scene).shafts,
        "the same description with an extinction must schedule the march",
    );
    assert_ne!(
        renderer.render(&scene, size).expect("a real medium").rgba,
        empty_medium.rgba,
        "and must render differently",
    );
}

// --- Quality masking: the machine's switch on top of the scene's derived chain ---------------

/// **A masked effect is an unauthored effect, byte for byte.** The fixture authors ambient
/// occlusion, light shafts and depth of field over a sun and a medium. Rendered by a machine at
/// [`Quality::LOW`] it must be indistinguishable from the same scene with those three strengths
/// at zero rendered at [`Quality::FULL`] — the same derived chain, then the same bytes.
///
/// That is the whole claim masking makes: a machine that cannot afford an effect *skips* it and
/// lands on a frame the engine already produces for a scene that never asked for it, rather than
/// on a cheaper variant of the effect that nothing else ever renders.
///
/// The per-effect loop is what keeps the equality from being vacuous: each of the three, authored
/// alone, moves the frame at `FULL` — so the identity above is three effects being skipped, not
/// three fields never being read.
#[test]
fn needs_gpu_a_masked_effect_renders_exactly_as_an_unauthored_one() {
    let Some(mut renderer) =
        skip_or_renderer("a_masked_effect_renders_exactly_as_an_unauthored_one")
    else {
        return;
    };
    let (w, h) = (96u32, 96u32);
    let size = FrameSize::native(w, h);
    // The shaft fixture's sun and medium, plus a cube resting on a ground plane: two isolated
    // slabs give the AO estimate nothing to occlude, and a contact is what it reads.
    let fixture = |post: PostData| {
        let mut scene = shafts_scene(w, h);
        scene.meshes.push(ground(20.0));
        scene.meshes.push(cube(
            Vec3::new(0.0, 0.5, 0.0),
            Vec3::splat(1.0),
            [0.8, 0.8, 0.8, 1.0],
        ));
        scene.post = Some(post);
        scene
    };
    // The medium and the sun stay in both descriptions — only the three effect strengths differ,
    // so nothing but the masked passes can account for a byte.
    let unauthored = PostData {
        focus_distance: 2.0,
        ..shafts_post(0.0)
    };
    let authored = PostData {
        ssao: 1.0,
        shafts: 8.0,
        dof: 0.2,
        ..unauthored
    };
    let frame = |renderer: &Renderer, post: PostData| {
        renderer
            .render(&fixture(post), size)
            .expect("a quality render")
    };

    // `LOW` also masks the sun's shadows, and those are not an effect a scene could have left
    // unauthored — nothing spells "a sunlit frame that casts nothing" — so they are held on
    // here and proved separately by `masking_shadows_removes_the_pass_and_leaves_the_sun_lit`.
    let no_effects = Quality {
        shadows: true,
        ..Quality::LOW
    };
    assert_eq!(
        PostChain::for_scene(&fixture(authored)).masked(&no_effects, &QualityParams::FULL),
        PostChain::for_scene(&fixture(unauthored)),
        "a masked chain must be the chain the same scene derives without those effects",
    );

    let stripped = frame(&renderer, unauthored);
    let full = frame(&renderer, authored);
    renderer.set_quality(no_effects, QualityParams::FULL);
    let masked = frame(&renderer, authored);
    assert_eq!(
        masked.rgba, stripped.rgba,
        "a machine masking these effects must render the frame the scene without them renders",
    );
    assert_ne!(
        full.rgba, masked.rgba,
        "and the fixture must author something for the mask to drop",
    );

    // Each effect alone is worth a frame here, so the identity above is three effects being
    // skipped rather than three fields never being read.
    renderer.set_quality(Quality::FULL, QualityParams::FULL);
    for (name, only) in [
        (
            "ssao",
            PostData {
                ssao: 1.0,
                ..unauthored
            },
        ),
        (
            "shafts",
            PostData {
                shafts: 8.0,
                ..unauthored
            },
        ),
        (
            "dof",
            PostData {
                dof: 0.2,
                ..unauthored
            },
        ),
    ] {
        assert_ne!(
            frame(&renderer, only).rgba,
            stripped.rgba,
            "`{name}` alone must move this fixture's frame",
        );
    }
}

/// **A lowered parameter is a cheaper estimate, and each one reaches exactly its own pass.** The
/// masking test above proves a preset can remove an effect; this proves it can also make one run
/// cheaper, which is the only thing separating `medium` and `high` from `full` (their
/// [`Quality`] toggles are identical, so a test comparing those would pass vacuously).
///
/// Each effect is authored **alone**, so its two passes are the only ones the chain carries and
/// the frames can differ for exactly one reason: that effect's own count. A missed site — a loop
/// bound lowered while its divisor kept a literal — would still show here, because the estimate
/// would move; what it could not do is stay still.
///
/// The scene with none of the three authored is the control: with those passes absent, no count
/// this preset carries is observable, which is the same claim
/// [`QualityParams::LOW`](crate::QualityParams::LOW)'s doc comment makes about the preset that
/// turns all three off.
#[test]
fn needs_gpu_a_lowered_parameter_makes_each_effect_cheaper_and_reaches_nothing_else() {
    let Some(mut renderer) =
        skip_or_renderer("a_lowered_parameter_makes_each_effect_cheaper_and_reaches_nothing_else")
    else {
        return;
    };
    let (w, h) = (96u32, 96u32);
    let size = FrameSize::native(w, h);
    // The masking test's fixture: a sun and a medium over a cube resting on a ground plane, so a
    // contact exists for the AO estimate to find and a beam for the march to resolve.
    let fixture = |post: PostData| {
        let mut scene = shafts_scene(w, h);
        scene.meshes.push(ground(20.0));
        scene.meshes.push(cube(
            Vec3::new(0.0, 0.5, 0.0),
            Vec3::splat(1.0),
            [0.8, 0.8, 0.8, 1.0],
        ));
        scene.post = Some(post);
        scene
    };
    let unauthored = PostData {
        focus_distance: 2.0,
        ..shafts_post(0.0)
    };
    let frame = |renderer: &Renderer, post: PostData| {
        renderer
            .render(&fixture(post), size)
            .expect("a quality render")
    };
    // `MEDIUM` on the three post axes with the sun's two left at the ceiling. The fixture has a
    // sun, so a smaller shadow map would move every frame below and make each assertion pass
    // whether or not its own count was read — the sun's axes have their own test.
    let post_only = QualityParams {
        shadow_map_size: QualityParams::FULL.shadow_map_size,
        pcf: QualityParams::FULL.pcf,
        ..QualityParams::MEDIUM
    };

    for (name, only) in [
        (
            "ssao",
            PostData {
                ssao: 1.0,
                ..unauthored
            },
        ),
        (
            "shafts",
            PostData {
                shafts: 8.0,
                ..unauthored
            },
        ),
        (
            "dof",
            PostData {
                dof: 0.2,
                ..unauthored
            },
        ),
    ] {
        renderer.set_quality(Quality::FULL, QualityParams::FULL);
        let full = frame(&renderer, only);
        renderer.set_quality(Quality::FULL, post_only);
        let lowered = frame(&renderer, only);
        assert_ne!(
            full.rgba, lowered.rgba,
            "`{name}` is the only effect this scene authors, so a lowered count must move it",
        );
    }

    renderer.set_quality(Quality::FULL, QualityParams::FULL);
    let control = frame(&renderer, unauthored);
    renderer.set_quality(Quality::FULL, post_only);
    assert_eq!(
        frame(&renderer, unauthored).rgba,
        control.rgba,
        "with none of the three authored, no count the preset carries can reach the frame",
    );
}

/// A sun over a ground plane and a cube: the shadow the tests below add and remove.
fn shadow_fixture(w: u32, h: u32) -> RenderScene {
    let mut scene = shafts_scene(w, h);
    scene.meshes.push(ground(20.0));
    scene.meshes.push(cube(
        Vec3::new(0.0, 0.5, 0.0),
        Vec3::splat(1.0),
        [0.8, 0.8, 0.8, 1.0],
    ));
    scene
}

/// A sunless floor and an oblique camera low enough to see under a plate and high enough to see
/// its top; `plate` adds the thin plate (mesh index 1) over half of where
/// [`fixture_drop_shadow`] lands.
fn drop_shadow_fixture(w: u32, h: u32, plate: bool) -> RenderScene {
    let mut scene = RenderScene::empty((w, h));
    let position = Vec3::new(0.0, 1.5, 8.0);
    scene.camera = crate::scene::RenderCamera {
        view: Mat4::look_at_rh(position, Vec3::new(0.0, 0.3, 0.0), Vec3::Y),
        projection: Mat4::perspective_rh(
            50.0_f32.to_radians(),
            crate::scene::aspect_ratio((w, h)),
            0.1,
            1000.0,
        ),
        position,
    };
    scene.meshes.push(ground(20.0));
    if plate {
        // Covers x in [0, 2], z in [-2, 2] at y = 0.5.
        scene.meshes.push(cube(
            Vec3::new(1.0, 0.5, 0.0),
            Vec3::new(2.0, 0.02, 4.0),
            [0.8, 0.8, 0.8, 1.0],
        ));
    }
    scene
}

fn fixture_drop_shadow(exclude: Vec<u32>) -> crate::scene::DropShadowInstance {
    crate::scene::DropShadowInstance {
        shape: crate::scene::GroundMarkerShape::Disc { radius: 1.5 },
        anchor: Vec3::new(0.0, 1.0, 0.0),
        yaw: 0.0,
        opacity: 0.8,
        softness: 0.0,
        max_distance: 5.0,
        fade_distance: 1.0,
        exclude,
    }
}

/// The pixel a world point lands on under `scene`'s camera.
fn pixel_of(scene: &RenderScene, point: Vec3, w: u32, h: u32) -> (u32, u32) {
    let clip = (scene.camera.projection * scene.camera.view).project_point3(point);
    (
        ((clip.x * 0.5 + 0.5) * w as f32) as u32,
        ((0.5 - clip.y * 0.5) * h as f32) as u32,
    )
}

/// A lit floor and cube under a spot straight above, seen by `drop_shadow_fixture`'s camera;
/// `cast_shadows` toggles the spot's opt-in.
fn local_shadow_fixture(w: u32, h: u32, cast_shadows: bool) -> RenderScene {
    let mut scene = drop_shadow_fixture(w, h, false);
    scene.meshes.push(cube(
        Vec3::new(0.0, 0.5, 0.0),
        Vec3::ONE,
        [0.8, 0.5, 0.4, 1.0],
    ));
    scene.spot_lights.push(crate::scene::SpotLightData {
        position: Vec3::new(0.0, 4.0, 0.0),
        direction: -Vec3::Y,
        color: Vec3::ONE,
        intensity: 20.0,
        range: 10.0,
        cos_inner: 20.0_f32.to_radians().cos(),
        cos_outer: 40.0_f32.to_radians().cos(),
        shadow: cast_shadows.then_some(crate::scene::LocalShadowData {
            bias: 0.05,
            near: 0.1,
            distance: None,
            fade: 0.0,
        }),
        scatter: 0.0,
    });
    scene
}

/// Reads a `Depth32Float` target back, unpadding the 256-byte row alignment.
/// `texture`'s level 0 as tightly packed texels, through a blocking staging copy.
fn read_texels(
    renderer: &Renderer,
    texture: &wgpu::Texture,
    bytes_per_texel: u32,
    aspect: wgpu::TextureAspect,
) -> Vec<u8> {
    let size = texture.size();
    let (w, h) = (size.width, size.height);
    let row = (w * bytes_per_texel).div_ceil(256) * 256;
    let device = renderer.gpu.device();
    let buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("test readback"),
        size: u64::from(row * h),
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut encoder =
        device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
    encoder.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect,
        },
        wgpu::TexelCopyBufferInfo {
            buffer: &buffer,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(row),
                rows_per_image: Some(h),
            },
        },
        size,
    );
    renderer.gpu.queue().submit(Some(encoder.finish()));
    let (tx, rx) = std::sync::mpsc::channel();
    buffer.slice(..).map_async(wgpu::MapMode::Read, move |r| {
        let _ = tx.send(r);
    });
    device
        .poll(wgpu::PollType::wait_indefinitely())
        .expect("the device polls");
    rx.recv().unwrap().expect("the buffer maps");
    let mapped = buffer.slice(..).get_mapped_range().expect("mapped range");
    (0..h as usize)
        .flat_map(|y| {
            let start = y * row as usize;
            mapped[start..start + (w * bytes_per_texel) as usize].to_vec()
        })
        .collect()
}

fn f32_texels(bytes: &[u8]) -> Vec<f32> {
    bytes
        .chunks_exact(4)
        .map(|texel| f32::from_le_bytes(texel.try_into().unwrap()))
        .collect()
}

fn read_depth32(renderer: &Renderer, texture: &wgpu::Texture) -> Vec<f32> {
    f32_texels(&read_texels(
        renderer,
        texture,
        4,
        wgpu::TextureAspect::DepthOnly,
    ))
}

/// A graph of the frame's depth-only `pass` alone, its target declared as an output: only an
/// output can be copied out, because a later transient may alias any other resource.
fn depth_pass_graph(
    frame: &ludion_render_graph::RenderGraph,
    pass: &'static str,
) -> (
    ludion_render_graph::RenderGraph,
    ludion_render_graph::ResourceId,
) {
    let depth = frame
        .passes()
        .iter()
        .find(|declared| declared.name == pass)
        .and_then(|declared| declared.depth)
        .expect("the graph schedules the depth pass");
    let target = &frame.resources()[depth.index()];
    let mut graph = ludion_render_graph::RenderGraph::new();
    let atlas = graph.output(target.name.clone(), target.desc.clone());
    graph.add_external_pass_with_depth(pass, pass, vec![], vec![], atlas);
    (graph, atlas)
}

/// **The local-shadow pass writes the granted view's tile and nothing else.** The atlas is read
/// back: inside the spot's rect some texel holds scene depth (< 1.0), and every texel outside
/// every granted rect is exactly the 1.0 clear.
#[test]
fn needs_gpu_the_local_shadow_pass_fills_only_the_granted_tiles() {
    let Some(mut renderer) = skip_or_renderer("the_local_shadow_pass_fills_only_the_granted_tiles")
    else {
        return;
    };
    let edge = 256;
    renderer.set_quality(
        Quality::FULL,
        QualityParams {
            local_shadow_atlas_size: edge,
            ..QualityParams::FULL
        },
    );
    let (w, h) = (160u32, 120u32);
    let size = FrameSize::native(w, h);
    let scene = local_shadow_fixture(w, h, true);
    let chain = PostChain::for_scene(&scene).masked(&renderer.quality, &renderer.quality_params);
    assert_eq!(chain.local_shadow_atlas, Some(edge));
    let (frame, _) = build_frame_graph(size, chain);
    let (graph, atlas) = depth_pass_graph(&frame, pass::LOCAL_SHADOW);
    let scheduled = schedule(&graph).expect("the graph schedules");
    let recorder = renderer.prepare_frame(&scene, size, None).unwrap();
    let granted: Vec<_> = recorder
        .local_shadow
        .as_ref()
        .expect("the spot is granted")
        .views
        .iter()
        .map(|view| view.rect)
        .collect();

    let copy = renderer
        .gpu
        .device()
        .create_texture(&wgpu::TextureDescriptor {
            label: Some("local shadow atlas copy"),
            size: wgpu::Extent3d {
                width: edge,
                height: edge,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Depth32Float,
            usage: wgpu::TextureUsages::COPY_DST | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
    renderer
        .gpu
        .execute_into(
            &scheduled,
            &recorder,
            &[(
                atlas,
                wgpu::TexelCopyTextureInfo {
                    texture: &copy,
                    mip_level: 0,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                },
            )],
        )
        .expect("the frame renders");
    let depth = read_depth32(&renderer, &copy);
    let inside = |x: u32, y: u32| {
        granted
            .iter()
            .any(|r| x >= r.x && x < r.x + r.size && y >= r.y && y < r.y + r.size)
    };
    let mut drawn = 0;
    for y in 0..edge {
        for x in 0..edge {
            let d = depth[(y * edge + x) as usize];
            if inside(x, y) {
                drawn += usize::from(d < 1.0);
            } else {
                assert_eq!(d, 1.0, "texel ({x}, {y}) is outside every granted tile");
            }
        }
    }
    assert!(drawn > 0, "the granted tile holds the scene's depth");
}

/// A cascaded sun over a ground plane and cubes spread from 2 m to 50 m ahead of a narrow camera
/// that looks down -Z from `eye`. The sun travels across the view, so a cascade's box reaches
/// laterally only as far as its own slice does. Mesh 0 is the ground, mesh 1 a cube at 3 m and
/// mesh 2 a cube at 45 m.
fn sun_cascade_fixture(w: u32, h: u32, eye: Vec3) -> RenderScene {
    let mut scene = RenderScene::empty((w, h));
    scene.camera = crate::scene::RenderCamera {
        view: Mat4::look_to_rh(eye, -Vec3::Z, Vec3::Y),
        projection: Mat4::perspective_rh(
            20.0_f32.to_radians(),
            crate::scene::aspect_ratio((w, h)),
            0.1,
            1000.0,
        ),
        position: eye,
    };
    let mut sun = test_sun(Vec3::new(1.0, -1.0, 0.0), Vec3::ONE, 0.002);
    sun.cascades = Some(crate::scene::SunCascadeConfig {
        count: 4,
        shadow_distance: 60.0,
        splits: None,
        shadow_near: -60.0,
        shadow_far: 60.0,
    });
    scene.sun = Some(sun);
    scene.meshes.push(ground(400.0));
    for z in [3.0, 45.0, 2.0, 8.0, 18.0, 30.0] {
        scene.meshes.push(cube(
            Vec3::new(0.0, 0.5, -z),
            Vec3::ONE,
            [0.8, 0.8, 0.8, 1.0],
        ));
    }
    scene
}

/// Renders `scene`'s sun pass and reads the shadow atlas back, with the oracle's per-cascade
/// caster lists.
fn render_sun_atlas(
    renderer: &Renderer,
    scene: &RenderScene,
    size: FrameSize,
) -> (Vec<f32>, (u32, u32), Vec<Vec<u32>>) {
    let chain = PostChain::for_scene(scene).masked(&renderer.quality, &renderer.quality_params);
    let (frame, _) = build_frame_graph(size, chain);
    let (graph, atlas) = depth_pass_graph(&frame, pass::SHADOW);
    let desc = &graph
        .resources()
        .iter()
        .find(|r| r.name == "shadow_map")
        .expect("the graph declares the shadow map")
        .desc;
    let extent = (desc.extent.width, desc.extent.height);
    let scheduled = schedule(&graph).expect("the graph schedules");
    let recorder = renderer.prepare_frame(scene, size, None).unwrap();
    let copy = renderer
        .gpu
        .device()
        .create_texture(&wgpu::TextureDescriptor {
            label: Some("sun atlas copy"),
            size: wgpu::Extent3d {
                width: extent.0,
                height: extent.1,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Depth32Float,
            usage: wgpu::TextureUsages::COPY_DST | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
    renderer
        .gpu
        .execute_into(
            &scheduled,
            &recorder,
            &[(
                atlas,
                wgpu::TexelCopyTextureInfo {
                    texture: &copy,
                    mip_level: 0,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                },
            )],
        )
        .expect("the frame renders");
    // The CPU oracle: the GPU cull keeps the casters this lists per cascade.
    let casters = recorder.sun_cascades.as_ref().map_or_else(Vec::new, |fit| {
        let bounds = instance_bounds(&scene.meshes, &[], &scene.bone_palette);
        crate::sun_cascade::sun_cascade_casters(fit, &bounds)
    });
    (read_depth32(renderer, &copy), extent, casters)
}

/// Whether every one of the four tiles of the `tile`-edge atlas holds a drawn texel.
fn every_tile_is_drawn(depth: &[f32], tile: u32) -> bool {
    (0..4).all(|index| {
        let (column, row) = sun_atlas_tile(4, index);
        (0..tile).any(|y| {
            (0..tile)
                .any(|x| depth[((row * tile + y) * tile * 2 + column * tile + x) as usize] < 1.0)
        })
    })
}

fn small_sun_renderer(name: &str) -> Option<Renderer> {
    let mut renderer = skip_or_renderer(name)?;
    renderer.set_quality(
        Quality::FULL,
        QualityParams {
            shadow_map_size: 256,
            ..QualityParams::FULL
        },
    );
    Some(renderer)
}

/// **A sub-texel camera move leaves the cascade atlas unchanged.** The camera moves along the
/// light's lateral axis by a fraction of cascade 0's texel; once both fits agree on every matrix
/// the two atlases are texel-for-texel equal, and every tile holds geometry.
#[test]
fn needs_gpu_sun_cascade_atlas_is_stable_under_a_sub_texel_move() {
    let Some(renderer) = small_sun_renderer("sun_cascade_atlas_is_stable_under_a_sub_texel_move")
    else {
        return;
    };
    let (w, h) = (64u32, 64u32);
    let size = FrameSize::native(w, h);
    let eye = Vec3::new(0.0, 1.5, 0.0);
    let a = sun_cascade_fixture(w, h, eye);
    let fit_a = fit_sun_cascades(&a, &renderer.quality_params, false).expect("a fit");
    let direction = a.sun.as_ref().unwrap().direction;
    let lateral = Mat4::look_to_rh(Vec3::ZERO, direction, Vec3::Y)
        .inverse()
        .x_axis
        .truncate();
    let agree = |x: &SunCascades, y: &SunCascades| {
        x.cascades.iter().zip(&y.cascades).all(|(p, q)| {
            p.light_space
                .to_cols_array()
                .iter()
                .zip(q.light_space.to_cols_array())
                .all(|(m, n)| (m - n).abs() <= 1e-6)
        })
    };
    let (b, fraction) = [0.4, 0.3, 0.2, 0.1, 0.05, -0.4, -0.3, -0.2, -0.1, -0.05]
        .into_iter()
        .find_map(|fraction| {
            let moved = eye + lateral * fraction * fit_a.cascades[0].texel;
            let b = sun_cascade_fixture(w, h, moved);
            let fit_b = fit_sun_cascades(&b, &renderer.quality_params, false)?;
            agree(&fit_a, &fit_b).then_some((b, fraction))
        })
        .expect("some sub-texel move keeps every cascade on its lattice cell");
    let (depth_a, extent, _) = render_sun_atlas(&renderer, &a, size);
    let (depth_b, _, _) = render_sun_atlas(&renderer, &b, size);
    assert_eq!(extent, (512, 512));
    for (index, (p, q)) in depth_a.iter().zip(&depth_b).enumerate() {
        assert!(
            (p - q).abs() <= 1e-6,
            "texel {index} moved by a {fraction}-texel camera move: {p} vs {q}",
        );
    }
    assert!(
        every_tile_is_drawn(&depth_a, 256),
        "every tile holds geometry"
    );
}

/// **Each cascade draws only the casters inside its box.** The near cube is in cascade 0's list
/// and not the last's; the far cube the reverse; the ground spans every box.
#[test]
fn needs_gpu_each_sun_cascade_draws_only_its_casters() {
    let Some(renderer) = small_sun_renderer("each_sun_cascade_draws_only_its_casters") else {
        return;
    };
    let (w, h) = (64u32, 64u32);
    let scene = sun_cascade_fixture(w, h, Vec3::new(0.0, 1.5, 0.0));
    let (depth, _, casters) = render_sun_atlas(&renderer, &scene, FrameSize::native(w, h));
    assert_eq!(casters.len(), 4);
    let (ground, near, far) = (0, 1, 2);
    assert!(casters[0].contains(&near) && !casters[3].contains(&near));
    assert!(casters[3].contains(&far) && !casters[0].contains(&far));
    for (index, list) in casters.iter().enumerate() {
        assert!(list.contains(&ground), "the ground is in cascade {index}");
    }
    assert!(
        every_tile_is_drawn(&depth, 256),
        "every tile holds geometry"
    );
}

/// An unset sun keeps the single map: no cascade count in the chain or the recorder, and the
/// declared descriptor is the square depth texture exactly.
#[test]
fn needs_gpu_an_unset_sun_records_the_single_map() {
    let Some(renderer) = small_sun_renderer("an_unset_sun_records_the_single_map") else {
        return;
    };
    let (w, h) = (64u32, 64u32);
    let mut scene = sun_cascade_fixture(w, h, Vec3::new(0.0, 1.5, 0.0));
    scene.sun.as_mut().unwrap().cascades = None;
    let size = FrameSize::native(w, h);
    let chain = PostChain::for_scene(&scene).masked(&renderer.quality, &renderer.quality_params);
    assert_eq!(chain.sun_cascades, None);
    let recorder = renderer.prepare_frame(&scene, size, None).unwrap();
    assert!(recorder.sun_cascades.is_none());
    let (graph, _) = build_frame_graph(size, chain);
    let map = graph
        .resources()
        .iter()
        .find(|r| r.name == "shadow_map")
        .expect("the graph declares the shadow map");
    assert_eq!(
        map.desc,
        ludion_render_graph::TextureDesc::sampled_depth(256, 256)
    );
}

fn lanes_sun() -> crate::scene::SunData {
    let mut sun = test_sun(Vec3::new(1.0, -1.0, 0.0), Vec3::ONE, 1.0 / 1024.0);
    sun.cascades = Some(crate::scene::SunCascadeConfig {
        count: 4,
        shadow_distance: 100.0,
        splits: None,
        shadow_near: -64.0,
        shadow_far: 64.0,
    });
    sun
}

fn lanes_fit(count: u32) -> SunCascades {
    let mut fit = SunCascades {
        count,
        cascades: [crate::sun_cascade::SunCascade::default(); 4],
    };
    for index in 0..count as usize {
        let step = index as f32;
        fit.cascades[index] = crate::sun_cascade::SunCascade {
            light_space: Mat4::from_scale(Vec3::splat(1.0 + step)),
            near: 1.0 + step * 4.0,
            far: 5.0 + step * 4.0,
            texel: 0.25 * (step + 1.0),
            depth_range: 100.0,
        };
    }
    fit
}

/// No fit packs to all-zero lanes with count 0, whatever the sun and camera are.
#[test]
fn sun_cascade_lanes_without_a_fit_are_zero() {
    let view = Mat4::look_to_rh(Vec3::new(3.0, 2.0, 1.0), Vec3::NEG_Z, Vec3::Y);
    assert_eq!(
        sun_cascade_lanes(None, &lanes_sun(), view),
        SunCascadeLanes::default()
    );
    assert_eq!(SunCascadeLanes::default().count, 0);
}

/// The atlas rects are the row-major tiles of the 1×1, 2×1 and 2×2 grids, in UV.
#[test]
fn sun_cascade_lanes_rects_follow_the_atlas_grid() {
    let sun = lanes_sun();
    let rects = |count: u32| {
        let lanes = sun_cascade_lanes(Some(&lanes_fit(count)), &sun, Mat4::IDENTITY);
        assert_eq!(lanes.count, count);
        lanes.rect
    };
    assert_eq!(rects(1)[0], [0.0, 0.0, 1.0, 1.0]);
    assert_eq!(rects(2)[..2], [[0.0, 0.0, 0.5, 1.0], [0.5, 0.0, 0.5, 1.0]]);
    assert_eq!(
        rects(4),
        [
            [0.0, 0.0, 0.5, 0.5],
            [0.5, 0.0, 0.5, 0.5],
            [0.0, 0.5, 0.5, 0.5],
            [0.5, 0.5, 0.5, 0.5],
        ]
    );
    assert_eq!(rects(3)[3], [0.0; 4], "lanes past the count stay zero");
}

/// `dot(view_depth.xyz, p) + view_depth.w` is the point's `-z` in view space, for a moved and
/// yawed camera.
#[test]
fn sun_cascade_lanes_view_depth_is_the_negated_view_z() {
    let view = Mat4::look_to_rh(
        Vec3::new(3.0, 2.0, -5.0),
        Vec3::new(1.0, 0.2, -1.0).normalize(),
        Vec3::Y,
    );
    let lanes = sun_cascade_lanes(Some(&lanes_fit(2)), &lanes_sun(), view);
    for p in [Vec3::new(4.0, -1.0, 7.0), Vec3::new(-20.0, 3.0, -40.0)] {
        let got = Vec3::from_slice(&lanes.view_depth[..3]).dot(p) + lanes.view_depth[3];
        let want = -view.transform_point3(p).z;
        assert!((got - want).abs() < 1e-3, "{got} vs {want}");
    }
}

/// Each cascade's bias is the authored world bias scaled by its texel over the reference texel,
/// in its own depth range.
#[test]
fn sun_cascade_lanes_bias_holds_the_same_texels_in_every_cascade() {
    let sun = lanes_sun();
    let fit = lanes_fit(3);
    let lanes = sun_cascade_lanes(Some(&fit), &sun, Mat4::IDENTITY);
    let bias_world = sun.bias * 128.0;
    for (index, cascade) in fit.cascades[..3].iter().enumerate() {
        let want = bias_world * cascade.texel / SUN_CASCADE_BIAS_TEXEL / cascade.depth_range;
        assert!((lanes.params[index][2] - want).abs() < 1e-7);
        assert_eq!(lanes.params[index][..2], [cascade.near, cascade.far]);
    }
}

/// With zeroed lanes `build_sun` is the pre-cascade uniform: its first 112 bytes match a `Sun`
/// built from the old fields, and the cascade lanes are zero, shadowed or not.
#[test]
fn build_sun_with_zeroed_lanes_is_the_anchored_uniform() {
    let mut scene = RenderScene::empty((8, 8));
    let sun = test_sun(Vec3::new(0.3, -1.0, 0.2), Vec3::new(1.0, 0.9, 0.8), 0.002);
    scene.sun = Some(sun);
    let pcf_only = QualityParams {
        pcss_blocker_taps: 0,
        pcss_filter_taps: 0,
        ..QualityParams::FULL
    };
    for shadowed in [true, false] {
        let old = Sun {
            light_space: if shadowed {
                sun.light_space.to_cols_array_2d()
            } else {
                [[0.0; 4]; 4]
            },
            direction: [sun.direction.x, sun.direction.y, sun.direction.z, sun.bias],
            color: [sun.color.x, sun.color.y, sun.color.z, 0.0],
            pcf: 3,
            disc_sin: 0.0,
            disc_cos: 1.0,
            disc_spread: 0.0,
            ..Sun::zeroed()
        };
        let built = build_sun(&scene, shadowed, &pcf_only, &SunCascadeLanes::default());
        assert_eq!(
            bytemuck::bytes_of(&built)[..112],
            bytemuck::bytes_of(&old)[..112]
        );
        assert!(bytemuck::bytes_of(&built)[112..].iter().all(|b| *b == 0));
    }
    // A frame that draws no shadow map takes no cascade lanes either.
    let fit = lanes_fit(2);
    let lanes = sun_cascade_lanes(Some(&fit), &lanes_sun(), Mat4::IDENTITY);
    let masked = build_sun(&scene, false, &pcf_only, &lanes);
    assert_eq!(masked.cascade_count, 0);
}

/// The penumbra growth is the map's own geometry: texels per unit of light-NDC depth,
/// `depth_range · tan θ / texel`, for the anchored box and for every fitted cascade, and exactly 0.0
/// (the shader's gate for the PCF loops) at θ = 0 or without a shadow map.
#[test]
fn build_sun_penumbra_scale_matches_the_map_geometry() {
    let (extent, near, far, size) = (20.0_f32, 1.0_f32, 40.0_f32, 512u32);
    let theta = 3.0_f32.to_radians();
    let params = QualityParams {
        shadow_map_size: size,
        ..QualityParams::FULL
    };
    let framed = |angular_radius: f32| {
        let mut scene = RenderScene::empty((8, 8));
        let mut sun = test_sun_framed(
            Vec3::new(0.3, -1.0, 0.2),
            Vec3::ONE,
            0.002,
            (extent, near, far),
        );
        sun.angular_radius = angular_radius;
        scene.sun = Some(sun);
        scene
    };
    let fit = lanes_fit(2);
    let lanes = sun_cascade_lanes(Some(&fit), &lanes_sun(), Mat4::IDENTITY);

    let lit = build_sun(&framed(theta), true, &params, &lanes);
    let want = (far - near) * theta.tan() / (2.0 * extent / size as f32);
    assert!(
        (lit.penumbra_anchored - want).abs() <= want * 1e-5,
        "anchored {} vs {want}",
        lit.penumbra_anchored
    );
    for (i, cascade) in fit.cascades[..2].iter().enumerate() {
        let want = cascade.depth_range * theta.tan() / cascade.texel;
        let got = lit.cascade_params[i][3];
        assert!(
            (got - want).abs() <= want * 1e-5,
            "cascade {i}: {got} vs {want}"
        );
    }

    for (scene, shadowed) in [
        (framed(0.0), true),
        (framed(theta), false),
        (framed(0.0), false),
    ] {
        let sun = build_sun(&scene, shadowed, &params, &lanes);
        assert_eq!(sun.penumbra_anchored, 0.0);
        for lane in sun.cascade_params {
            assert_eq!(lane[3], 0.0);
        }
    }
}

/// The `sun_cascades` view's twins are drawn only with the view asked, a sun, cascades fitted and
/// a shadow map.
#[test]
fn the_cascade_view_needs_the_view_a_sun_fitted_lanes_and_a_shadow_map() {
    let mut scene = RenderScene::empty((8, 8));
    scene.sun = Some(test_sun(Vec3::new(0.3, -1.0, 0.2), Vec3::ONE, 0.002));
    let fit = lanes_fit(2);
    let lanes = sun_cascade_lanes(Some(&fit), &lanes_sun(), Mat4::IDENTITY);
    let none = SunCascadeLanes::default();

    assert!(!sun_cascade_view(&scene, true, &lanes));
    scene.debug_view = Some(crate::DebugView::SunCascades);
    assert!(sun_cascade_view(&scene, true, &lanes));
    assert!(!sun_cascade_view(&scene, true, &none));
    assert!(!sun_cascade_view(&scene, false, &lanes));
    scene.debug_view = Some(crate::DebugView::LocalShadowAtlas);
    assert!(!sun_cascade_view(&scene, true, &lanes));
    scene.debug_view = Some(crate::DebugView::SunCascades);
    scene.sun = None;
    assert!(!sun_cascade_view(&scene, true, &lanes));
}

fn sun_renderer(name: &str, shadow_map_size: u32, pcf: u32) -> Option<Renderer> {
    let mut renderer = skip_or_renderer(name)?;
    renderer.set_quality(
        Quality::FULL,
        QualityParams {
            shadow_map_size,
            pcf,
            ..QualityParams::FULL
        },
    );
    Some(renderer)
}

/// The pixel a world point lands on, asserting it is inside the frame.
fn project_pixel(scene: &RenderScene, p: Vec3, (w, h): (u32, u32)) -> (u32, u32) {
    let clip = scene.camera.projection * scene.camera.view * p.extend(1.0);
    let ndc = clip.truncate() / clip.w;
    let x = ((ndc.x * 0.5 + 0.5) * w as f32).floor();
    let y = ((0.5 - ndc.y * 0.5) * h as f32).floor();
    assert!(
        x >= 0.0 && y >= 0.0 && x < w as f32 && y < h as f32,
        "{p} lands off-frame at ({x}, {y})",
    );
    (x as u32, y as u32)
}

/// **One cascade evaluates the anchored arithmetic.** A hand-built single cascade that reuses the
/// anchored matrix, with a bias chosen to equal the anchored one, shades every byte as the
/// anchored sun does. The anchored box sits well inside the receivers' extent, because a tap
/// that reaches past the box edge clamps in a cascade and reads off-map for the anchored map.
#[test]
fn needs_gpu_sun_cascade_n1_evaluates_the_anchored_arithmetic() {
    let Some(renderer) = sun_renderer("sun_cascade_n1_evaluates_the_anchored_arithmetic", 512, 3)
    else {
        return;
    };
    let (w, h) = (96u32, 96u32);
    let scene_with = |cube_at: Option<Vec3>| {
        let mut scene = RenderScene::empty((w, h));
        let eye = Vec3::new(0.0, 7.0, 9.0);
        scene.camera = crate::scene::RenderCamera {
            view: Mat4::look_at_rh(eye, Vec3::ZERO, Vec3::Y),
            projection: Mat4::perspective_rh(
                50.0_f32.to_radians(),
                crate::scene::aspect_ratio((w, h)),
                0.1,
                200.0,
            ),
            position: eye,
        };
        scene.sun = Some(test_sun(
            Vec3::new(0.4, -1.0, 0.2),
            Vec3::splat(2.0),
            1.0 / 1024.0,
        ));
        scene.meshes.push(ground(24.0));
        if let Some(at) = cube_at {
            scene
                .meshes
                .push(cube(at, Vec3::new(2.0, 3.0, 2.0), [0.8; 4]));
        }
        scene
    };
    let size = FrameSize::native(w, h);
    let anchored_scene = scene_with(Some(Vec3::new(0.0, 1.5, 0.0)));
    let anchored = renderer.render(&anchored_scene, size).expect("anchored");

    let mut cascaded_scene = anchored_scene.clone();
    let sun = cascaded_scene.sun.as_mut().unwrap();
    sun.cascades = Some(crate::scene::SunCascadeConfig {
        count: 1,
        shadow_distance: 100.0,
        splits: None,
        shadow_near: -64.0,
        shadow_far: 64.0,
    });
    let fit = SunCascades {
        count: 1,
        cascades: [
            crate::sun_cascade::SunCascade {
                light_space: sun.light_space,
                near: 0.0,
                far: 1.0e6,
                texel: SUN_CASCADE_BIAS_TEXEL * 2048.0 / 512.0,
                depth_range: 128.0,
            },
            Default::default(),
            Default::default(),
            Default::default(),
        ],
    };
    let lanes = sun_cascade_lanes(Some(&fit), sun, cascaded_scene.camera.view);
    assert_eq!(lanes.params[0][2], sun.bias * 2048.0 / 512.0);
    renderer.sun_cascade_override.set(Some(fit));
    let cascaded = renderer.render(&cascaded_scene, size).expect("cascaded");
    renderer.sun_cascade_override.set(None);
    assert_eq!(anchored.rgba, cascaded.rgba);

    let bare = renderer.render(&scene_with(None), size).expect("no cube");
    assert_ne!(
        anchored.rgba, bare.rgba,
        "the cube's shadow is in the frame"
    );
}

const POST_DEPTHS: [f32; 7] = [2.0, 5.0, 10.0, 20.0, 40.0, 70.0, 95.0];
const POST_HEIGHT: f32 = 4.0;

/// A post's width across the view, which grows with its depth so its shadow stays a few pixels
/// tall under the camera's foreshortening.
fn post_width(depth: f32) -> f32 {
    0.5 + 0.1 * depth
}

/// One wall of height 4 m at the origin, lit by a raking sun that travels +x, so its shadow falls
/// to the right. The camera looks down 25° at the floor point `aim` m right of the wall, `depth`
/// metres along its view axis: that point's view depth is exactly `depth`, which is what picks
/// the cascade. A camera per depth, because a floor pixel's footprint grows with its distance
/// and a 95 m shadow needs a camera that far away to be a few pixels wide in one frame.
fn post_scene(size: (u32, u32), depth: f32, aim: f32, cascaded: bool) -> RenderScene {
    let mut scene = RenderScene::empty(size);
    let pitch = 25.0_f32.to_radians();
    let forward = Vec3::new(0.0, -pitch.sin(), -pitch.cos());
    let eye = post_shadow_point(aim) - forward * depth;
    scene.camera = crate::scene::RenderCamera {
        view: Mat4::look_to_rh(eye, forward, Vec3::Y),
        projection: Mat4::perspective_rh(
            50.0_f32.to_radians(),
            crate::scene::aspect_ratio(size),
            0.1,
            1000.0,
        ),
        position: eye,
    };
    let direction = Vec3::new(1.0, -0.4, 0.0);
    scene.sun = Some(if cascaded {
        let mut sun = test_sun(direction, Vec3::splat(3.0), 0.05 / 200.0);
        sun.cascades = Some(crate::scene::SunCascadeConfig {
            count: 4,
            shadow_distance: 100.0,
            splits: None,
            shadow_near: -100.0,
            shadow_far: 100.0,
        });
        sun
    } else {
        test_sun_framed(
            direction,
            Vec3::splat(3.0),
            0.05 / 199.0,
            (50.0, 1.0, 200.0),
        )
    });
    scene.meshes.push(ground(400.0));
    scene.meshes.push(cube(
        Vec3::new(0.0, POST_HEIGHT * 0.5, 0.0),
        Vec3::new(0.5, POST_HEIGHT, post_width(depth)),
        [0.8; 4],
    ));
    scene
}

/// The floor point inside the wall's shadow, `along` metres right of its face.
fn post_shadow_point(along: f32) -> Vec3 {
    Vec3::new(0.25 + along, 0.0, 0.0)
}

/// The lit floor beside that shadow: past the wall's width in z, away from the camera.
fn post_lit_point(depth: f32, along: f32) -> Vec3 {
    let mut p = post_shadow_point(along);
    p.z -= 0.5 * post_width(depth) + 0.1 * depth + 1.0;
    p
}

/// **Receding posts are shadowed at every depth.** A raking sun and a wall seen from 2 m to 95 m,
/// four cascades over 100 m: at each depth the floor inside its shadow is clearly darker than the
/// lit floor beside it, lit floor stays smooth (no acne), the nearest shadow's edge is sharper
/// than an anchored 100 m box's, and the floor just behind the nearest base is dark (the
/// cascade's bias does not lift the contact shadow).
#[test]
fn needs_gpu_sun_cascade_posts_receding_100m_are_shadowed_at_every_depth() {
    let Some(renderer) = sun_renderer(
        "sun_cascade_posts_receding_100m_are_shadowed_at_every_depth",
        512,
        3,
    ) else {
        return;
    };
    let (w, h) = (128u32, 128u32);
    let size = FrameSize::native(w, h);
    for depth in POST_DEPTHS {
        let scene = post_scene((w, h), depth, 4.0, true);
        let image = renderer.render(&scene, size).expect("cascaded frame");
        let at = |p: Vec3| project_pixel(&scene, p, (w, h));
        let (sx, sy) = at(post_shadow_point(4.0));
        let (lx, ly) = at(post_lit_point(depth, 4.0));
        let (dark, lit) = (luma(&image, sx, sy), luma(&image, lx, ly));
        assert!(
            lit > dark + 30,
            "the post at {depth} m: shadow {dark}, lit {lit}",
        );
        let patch: Vec<u32> = (ly - 2..=ly + 2)
            .flat_map(|y| (lx - 2..=lx + 2).map(move |x| (x, y)))
            .map(|(x, y)| luma(&image, x, y))
            .collect();
        let spread = patch.iter().max().unwrap() - patch.iter().min().unwrap();
        assert!(spread <= 8, "acne on the lit floor at {depth} m: {patch:?}");
    }

    // The nearest wall's two z-edges, as a column of rows: count rows strictly between
    // 20 % and 80 % of the way from lit to dark.
    let nearest = POST_DEPTHS[0];
    let transition_rows = |scene: &RenderScene, image: &RenderedImage| {
        let middle = post_shadow_point(4.0);
        let column = project_pixel(scene, middle, (w, h)).0;
        let rows =
            [0.8 * Vec3::Z, -0.8 * Vec3::Z].map(|dz| project_pixel(scene, middle + dz, (w, h)).1);
        let (top, bottom) = (rows[0].min(rows[1]), rows[0].max(rows[1]));
        let lumas: Vec<u32> = (top..=bottom).map(|y| luma(image, column, y)).collect();
        let (lo, hi) = (*lumas.iter().min().unwrap(), *lumas.iter().max().unwrap());
        let band = (hi - lo) as f32;
        lumas
            .iter()
            .filter(|l| {
                let t = (**l - lo) as f32 / band.max(1.0);
                t > 0.2 && t < 0.8
            })
            .count()
    };
    let scene = post_scene((w, h), nearest, 4.0, true);
    let image = renderer.render(&scene, size).expect("cascaded frame");
    let anchored_scene = post_scene((w, h), nearest, 4.0, false);
    let anchored = renderer
        .render(&anchored_scene, size)
        .expect("anchored frame");
    let (sharp, soft) = (
        transition_rows(&scene, &image),
        transition_rows(&anchored_scene, &anchored),
    );
    assert!(sharp <= 8, "the cascaded edge spans {sharp} rows");
    assert!(sharp < soft, "cascaded {sharp} rows vs anchored {soft}");

    // Contact: two cascade-0 texels past the base is already in shadow.
    let fit = fit_sun_cascades(&scene, &renderer.quality_params, false).expect("a fit");
    let along = 2.0 * fit.cascades[0].texel;
    let base_scene = post_scene((w, h), nearest, along, true);
    let base = renderer.render(&base_scene, size).expect("base frame");
    let (cx, cy) = project_pixel(&base_scene, post_shadow_point(along), (w, h));
    let (lx, ly) = project_pixel(&scene, post_lit_point(nearest, 4.0), (w, h));
    let (contact, lit) = (luma(&base, cx, cy), luma(&image, lx, ly));
    assert!(contact + 30 < lit, "contact {contact} vs lit {lit}");
}

/// **The cascade view tints by cascade.** The same four-cascade scene with the view and without:
/// the near floor leans red (cascade 0), the floor at the 70 m post leans yellow
/// (cascade 3), and the floor past `shadow_distance` is untouched. Another view leaves the frame
/// byte-identical to no view, so the uniform flag is off for it.
#[test]
fn needs_gpu_sun_cascade_view_tints_by_cascade() {
    let Some(renderer) = sun_renderer("sun_cascade_view_tints_by_cascade", 512, 3) else {
        return;
    };
    let (w, h) = (128u32, 128u32);
    let size = FrameSize::native(w, h);
    let rgb = |image: &RenderedImage, (x, y): (u32, u32)| {
        let i = ((y * image.width + x) * 4) as usize;
        [image.rgba[i], image.rgba[i + 1], image.rgba[i + 2]].map(i32::from)
    };
    let render = |depth: f32, view: Option<crate::DebugView>| {
        let mut scene = post_scene((w, h), depth, 4.0, true);
        scene.debug_view = view;
        let image = renderer.render(&scene, size).expect("cascaded frame");
        (scene, image)
    };

    let near = POST_DEPTHS[0];
    let (scene, tinted) = render(near, Some(crate::DebugView::SunCascades));
    let (_, plain) = render(near, None);
    let (_, other) = render(near, Some(crate::DebugView::LocalShadowAtlas));
    assert_eq!(plain.rgba, other.rgba, "another view leaves the flag off");
    let at = project_pixel(&scene, post_lit_point(near, 4.0), (w, h));
    let [r, g, b] = rgb(&tinted, at);
    assert!(
        r > g + 20 && r > b + 20,
        "near floor is not red: {r} {g} {b}"
    );

    let far = POST_DEPTHS[5];
    let (scene, tinted) = render(far, Some(crate::DebugView::SunCascades));
    let at = project_pixel(&scene, post_lit_point(far, 4.0), (w, h));
    let [r, g, b] = rgb(&tinted, at);
    assert!(
        b + 20 < r && b + 20 < g,
        "far floor is not yellow: {r} {g} {b}"
    );

    // The floor at z = -150 is about 206 m from this camera along its view axis, past the 100 m the
    // cascades cover: it is untinted.
    let (_, plain) = render(far, None);
    let at = project_pixel(&scene, Vec3::new(4.25, 0.0, -150.0), (w, h));
    assert_eq!(rgb(&tinted, at), rgb(&plain, at), "past the last cascade");

    // The tint is the `SUN_CASCADE_VIEW` twin's, never the world pipeline's.
    let plain = MeshVariant::PLAIN;
    let world = renderer.variant_pipeline(ShaderId::Mesh, plain);
    let twin = renderer.cascade_view_pipeline(plain, false, false);
    assert!(twin != world, "the twin is its own pipeline");
    let mut scene = post_scene((w, h), near, 4.0, true);
    scene.debug_view = Some(crate::DebugView::SunCascades);
    let recorder = renderer
        .prepare_frame(&scene, size, None)
        .expect("view frame");
    assert!(recorder.scene_mesh_pipeline(plain, false, false) == twin);
    drop(recorder);
    scene.debug_view = None;
    let recorder = renderer
        .prepare_frame(&scene, size, None)
        .expect("plain frame");
    assert!(recorder.scene_mesh_pipeline(plain, false, false) == world);
}

/// A camera 1.5 m up, pitched down, over a floor; the sun travels away from it. With `wall`, a
/// 20 m wall floats at 45 m, above the camera's top ray, so it casts a shadow but is never in the frame
/// (a visible bright wall would shift the whole frame through bloom).
fn tile_wall_scene(size: (u32, u32), wall: bool, angular_radius: f32) -> RenderScene {
    let mut scene = RenderScene::empty(size);
    let eye = Vec3::new(0.0, 1.5, 0.0);
    let pitch = 30.0_f32.to_radians();
    scene.camera = crate::scene::RenderCamera {
        view: Mat4::look_to_rh(eye, Vec3::new(0.0, -pitch.sin(), -pitch.cos()), Vec3::Y),
        projection: Mat4::perspective_rh(
            60.0_f32.to_radians(),
            crate::scene::aspect_ratio(size),
            0.1,
            1000.0,
        ),
        position: eye,
    };
    let mut sun = test_sun(Vec3::new(0.3, -0.6, -1.0), Vec3::splat(3.0), 0.05 / 48.0);
    sun.angular_radius = angular_radius;
    sun.cascades = Some(crate::scene::SunCascadeConfig {
        count: 4,
        shadow_distance: 60.0,
        splits: None,
        shadow_near: -24.0,
        shadow_far: 24.0,
    });
    scene.sun = Some(sun);
    scene.meshes.push(ground(400.0));
    if wall {
        scene.meshes.push(cube(
            Vec3::new(0.0, 9.0, -45.0),
            Vec3::new(20.0, 10.0, 2.0),
            [0.8; 4],
        ));
    }
    scene
}

/// **No PCF tap crosses a tile.** The wall at 45 m is in no cascade-0 caster list, yet it fills
/// other tiles of the atlas. Every pixel that only cascade 0 shades (view depth under its seam
/// band) is byte-identical with and without the wall, with a 5×5 kernel, and again with a 5° sun
/// whose PCSS blocker search and filter read the same tile.
#[test]
fn needs_gpu_sun_cascade_no_pcf_tap_crosses_a_tile() {
    let Some(renderer) = sun_renderer("sun_cascade_no_pcf_tap_crosses_a_tile", 256, 5) else {
        return;
    };
    let (w, h) = (96u32, 96u32);
    let size = FrameSize::native(w, h);
    for angular_radius in [0.0, 5.0_f32.to_radians()] {
        let (bare_scene, wall_scene) = (
            tile_wall_scene((w, h), false, angular_radius),
            tile_wall_scene((w, h), true, angular_radius),
        );

        let (atlas_bare, extent, _) = render_sun_atlas(&renderer, &bare_scene, size);
        let (atlas_wall, _, casters) = render_sun_atlas(&renderer, &wall_scene, size);
        assert!(!casters[0].contains(&1), "cascade 0 does not draw the wall");
        let moved_elsewhere = (0..extent.1)
            .flat_map(|y| (0..extent.0).map(move |x| (x, y)))
            .filter(|&(x, y)| x >= 256 || y >= 256)
            .any(|(x, y)| {
                let i = (y * extent.0 + x) as usize;
                atlas_bare[i] != atlas_wall[i]
            });
        assert!(moved_elsewhere, "the wall reaches another tile");

        let bare = renderer.render(&bare_scene, size).expect("bare");
        let walled = renderer.render(&wall_scene, size).expect("walled");
        let fit = fit_sun_cascades(&wall_scene, &renderer.quality_params, false).expect("a fit");
        let first = fit.cascades[0];
        let seam = first.far - 0.1 * (first.far - first.near);
        let camera = &wall_scene.camera;
        let inverse = (camera.projection * camera.view).inverse();
        let mut compared = 0;
        for y in 0..h {
            for x in 0..w {
                let ndc_x = (x as f32 + 0.5) / w as f32 * 2.0 - 1.0;
                let ndc_y = 1.0 - (y as f32 + 0.5) / h as f32 * 2.0;
                let ray = (inverse.project_point3(Vec3::new(ndc_x, ndc_y, 1.0)) - camera.position)
                    .normalize();
                if ray.y > -1e-4 {
                    continue;
                }
                let hit = camera.position + ray * (-camera.position.y / ray.y);
                if -camera.view.transform_point3(hit).z >= seam - 0.1 {
                    continue;
                }
                compared += 1;
                assert_eq!(pixel(&bare, x, y), pixel(&walled, x, y), "pixel ({x}, {y})");
            }
        }
        assert!(
            compared > 200,
            "only {compared} pixels are cascade 0's alone"
        );
    }
}

/// **The seam band never blends toward a cascade that does not cover the fragment.** A camera
/// with a 1° fov looks at a wall filling the frame, a slab 2 m toward the sun shadowing it. Cascade
/// 0 spans 0.1–1 m with its band at 0.9–1 m, and cascade 1 is a sliver at 1–1.02 m whose box is
/// far smaller than the wall's distance from it. The wall at 0.95 m, inside the band, shades like
/// the wall at 0.85 m: both are in the slab's shadow, not blended toward cascade 1's "lit".
#[test]
fn needs_gpu_sun_cascade_band_never_blends_into_an_uncovering_cascade() {
    let Some(renderer) = sun_renderer(
        "sun_cascade_band_never_blends_into_an_uncovering_cascade",
        512,
        3,
    ) else {
        return;
    };
    let (w, h) = (32u32, 32u32);
    let size = FrameSize::native(w, h);
    let toward_sun = Vec3::new(0.0, 0.5, 1.0).normalize();
    let render = |depth: f32, slab: bool| {
        let mut scene = RenderScene::empty((w, h));
        let eye = Vec3::ZERO;
        scene.camera = crate::scene::RenderCamera {
            view: Mat4::look_to_rh(eye, -Vec3::Z, Vec3::Y),
            projection: Mat4::perspective_rh(
                1.0_f32.to_radians(),
                crate::scene::aspect_ratio((w, h)),
                0.1,
                1000.0,
            ),
            position: eye,
        };
        let mut sun = test_sun(-toward_sun, Vec3::splat(3.0), 0.05 / 48.0);
        sun.cascades = Some(crate::scene::SunCascadeConfig {
            count: 4,
            shadow_distance: 100.0,
            splits: Some([0.01, 0.0102, 0.9]),
            shadow_near: -10.0,
            shadow_far: 10.0,
        });
        scene.sun = Some(sun);
        scene.meshes.push(cube(
            Vec3::new(0.0, 0.0, -depth - 0.05),
            Vec3::new(1.0, 1.0, 0.1),
            [0.8; 4],
        ));
        if slab {
            let on_the_wall = Vec3::new(0.0, 0.0, -depth);
            scene.meshes.push(cube(
                on_the_wall + toward_sun * 2.0,
                Vec3::new(20.0, 20.0, 0.1),
                [0.8; 4],
            ));
        }
        renderer.render(&scene, size).expect("frame")
    };
    let lit = render(0.85, false);
    let before = render(0.85, true);
    let in_band = render(0.95, true);
    let centre = (w / 2, h / 2);
    let (lit_luma, shadow_luma) = (
        luma(&lit, centre.0, centre.1),
        luma(&before, centre.0, centre.1),
    );
    assert!(
        shadow_luma + 30 < lit_luma,
        "the slab shadows the wall: {shadow_luma} vs lit {lit_luma}"
    );
    for y in 0..h {
        for x in 0..w {
            let (a, b) = (luma(&before, x, y), luma(&in_band, x, y));
            assert!(
                a.abs_diff(b) <= 3 * 3,
                "({x}, {y}): {a} at 0.85 m vs {b} in the band"
            );
        }
    }
}

/// **A far occluder darkens the march.** Fog and shafts under a four-cascade sun whose
/// `shadow_distance` is 20 m: a slab overhead at 40 m shadows the air the camera looks through,
/// so the lower half of the frame (air only) is darker than without it.
#[test]
fn needs_gpu_sun_cascade_an_occluder_40m_away_darkens_the_march() {
    let Some(renderer) = sun_renderer("sun_cascade_an_occluder_40m_away_darkens_the_march", 512, 3)
    else {
        return;
    };
    let (w, h) = (64u32, 64u32);
    let size = FrameSize::native(w, h);
    let scene = |slab: bool| {
        let mut scene = RenderScene::empty((w, h));
        let eye = Vec3::new(0.0, 2.0, 0.0);
        scene.camera = crate::scene::RenderCamera {
            view: Mat4::look_to_rh(eye, Vec3::NEG_Z, Vec3::Y),
            projection: Mat4::perspective_rh(
                60.0_f32.to_radians(),
                crate::scene::aspect_ratio((w, h)),
                0.1,
                1000.0,
            ),
            position: eye,
        };
        let mut sun = test_sun(Vec3::new(0.0, -1.0, 0.3), Vec3::ONE, 0.05 / 200.0);
        sun.cascades = Some(crate::scene::SunCascadeConfig {
            count: 4,
            shadow_distance: 20.0,
            splits: None,
            shadow_near: -100.0,
            shadow_far: 100.0,
        });
        scene.sun = Some(sun);
        scene.post = Some(medium_post(0.02, 8.0));
        if slab {
            scene.meshes.push(cube(
                Vec3::new(0.0, 12.0, -40.0),
                Vec3::new(80.0, 1.0, 80.0),
                [0.5; 4],
            ));
        }
        scene
    };
    let lower_half = |image: &RenderedImage| {
        let mut sum = 0u64;
        for y in h / 2..h {
            for x in 0..w {
                sum += u64::from(luma(image, x, y));
            }
        }
        sum as f64 / f64::from(w * (h - h / 2))
    };
    let open = lower_half(&renderer.render(&scene(false), size).expect("open sky"));
    let covered = lower_half(&renderer.render(&scene(true), size).expect("under the slab"));
    assert!(
        covered + 2.0 < open,
        "the slab's shadow must darken the march: {open:.2} -> {covered:.2}",
    );
}

/// A cube face's view in the mesh shader's terms: the rect in UV, the bias scaled up for a
/// smaller atlas, `2 / size` world texels per unit of distance, and the view count passed through.
#[test]
fn local_view_gpu_scales_rect_bias_and_texel() {
    let light = point_light(Vec3::new(1.0, 2.0, 3.0), 1.0, 10.0);
    let mut scene = RenderScene::empty((16, 16));
    scene.lights = vec![crate::scene::PointLightData {
        shadow: Some(crate::scene::LocalShadowData {
            bias: 0.05,
            near: 0.1,
            distance: None,
            fade: 0.0,
        }),
        ..light
    }];
    let plan = crate::local_shadow::plan_local_shadows(&scene, 2048);
    let view = &plan.views[2];
    let gpu = local_view_gpu(view, 2048, 6);
    let size = view.rect.size as f32;
    assert_eq!(gpu.rect[2], size / 2048.0);
    assert_eq!(gpu.rect[3], 1.0 / 2048.0);
    assert_eq!(gpu.params[0], 0.05 * 2.0);
    assert!(
        (gpu.params[1] - 2.0 / size).abs() < 1e-6,
        "{}",
        gpu.params[1]
    );
    assert_eq!(gpu.params[2], 6.0);
    assert_eq!(gpu.params[3], 1.0);
}

/// A top-down view of a large floor with no sun, no environment and no ambient, so the local
/// lights a test adds are the only light.
fn local_receive_scene(w: u32, h: u32) -> RenderScene {
    let mut scene = RenderScene::empty((w, h));
    scene.ambient = [0.0; 3];
    let eye = Vec3::new(0.0, 30.0, 0.01);
    scene.camera = crate::scene::RenderCamera {
        view: Mat4::look_at_rh(eye, Vec3::ZERO, Vec3::Y),
        projection: Mat4::perspective_rh(
            60.0_f32.to_radians(),
            crate::scene::aspect_ratio((w, h)),
            0.1,
            1000.0,
        ),
        position: eye,
    };
    scene.meshes = vec![ground(80.0)];
    // Bloom would spread a lit neighbour into a shadowed pixel and blur the comparisons.
    scene.post = Some(crate::scene::PostData {
        bloom_strength: 0.0,
        ..crate::scene::PostData::default()
    });
    scene
}

/// `spot_light` with the shadow tunables the receive tests share.
fn casting_spot(position: Vec3, range: f32, cast: bool) -> crate::scene::SpotLightData {
    let mut spot = spot_light(position, Vec3::NEG_Y, 6.0, range, 30.0, 45.0);
    spot.shadow = cast.then_some(crate::scene::LocalShadowData {
        bias: 0.02,
        near: 0.1,
        distance: None,
        fade: 0.0,
    });
    spot
}

/// The largest per-channel difference between two renders over a pixel rectangle.
fn max_channel_diff(
    a: &RenderedImage,
    b: &RenderedImage,
    x: std::ops::Range<u32>,
    y: std::ops::Range<u32>,
) -> u8 {
    let mut worst = 0;
    for py in y {
        for px in x.clone() {
            let (p, q) = (rgb_at(a, px, py), rgb_at(b, px, py));
            for c in 0..3 {
                worst = worst.max(p[c].abs_diff(q[c]));
            }
        }
    }
    worst
}

/// **A granted spot shadows what stands under it and nothing else.** The floor in a slab's
/// footprint is dark against the same frame with the opt-in off, the floor in the cone beyond it
/// and the slab's own lit top keep their light (no acne, no over-darkening).
#[test]
fn needs_gpu_a_granted_spot_shadows_the_floor_under_a_slab_and_not_its_top() {
    let Some(renderer) = skip_or_renderer("a_granted_spot_shadows_the_floor_under_a_slab") else {
        return;
    };
    let (w, h) = (192u32, 108u32);
    let size = FrameSize::native(w, h);
    let mut scene = local_receive_scene(w, h);
    scene.meshes.push(cube(
        Vec3::new(0.0, 1.5, 0.0),
        Vec3::new(8.0, 1.0, 8.0),
        [0.8, 0.8, 0.8, 1.0],
    ));
    scene.spot_lights = vec![casting_spot(Vec3::new(0.0, 8.0, 0.0), 20.0, false)];
    let plain = renderer.render(&scene, size).expect("an unshadowed render");
    scene.spot_lights = vec![casting_spot(Vec3::new(0.0, 8.0, 0.0), 20.0, true)];
    let shadowed = renderer.render(&scene, size).expect("a shadowed render");

    let at = |p: Vec3| pixel_of(&scene, p, w, h);
    let (sx, sy) = at(Vec3::new(4.7, 0.0, 0.0));
    let (before, after) = (luma(&plain, sx, sy), luma(&shadowed, sx, sy));
    assert!(before > 40, "the floor beside the slab is lit: {before}");
    assert!(
        after * 10 < before,
        "the floor in the footprint must be dark: {before} -> {after}",
    );
    let (fx, fy) = at(Vec3::new(6.5, 0.0, 0.0));
    assert!(
        luma(&shadowed, fx, fy) * 100 >= luma(&plain, fx, fy) * 97,
        "the floor in the cone outside the footprint stays lit",
    );
    let (tx, ty) = at(Vec3::new(0.0, 2.0, 0.0));
    assert!(
        luma(&shadowed, tx, ty) * 100 >= luma(&plain, tx, ty) * 97,
        "the slab top must not shadow itself",
    );
}

/// **A point light's six faces shadow a room's walls, and the seam between faces shows nothing.**
/// A light inside four tall walls leaves the floor beyond a wall dark, in all four directions;
/// the open floor inside the room, across the boundaries between the cube faces, renders as the
/// unshadowed frame does.
#[test]
fn needs_gpu_a_point_light_in_a_walled_room_lights_only_the_room() {
    let Some(renderer) = skip_or_renderer("a_point_light_in_a_walled_room_lights_only_the_room")
    else {
        return;
    };
    let (w, h) = (192u32, 108u32);
    let size = FrameSize::native(w, h);
    let mut scene = local_receive_scene(w, h);
    let wall = [0.7, 0.7, 0.7, 1.0];
    for (pos, extent) in [
        (Vec3::new(6.0, 3.0, 0.0), Vec3::new(0.4, 6.0, 12.4)),
        (Vec3::new(-6.0, 3.0, 0.0), Vec3::new(0.4, 6.0, 12.4)),
        (Vec3::new(0.0, 3.0, 6.0), Vec3::new(12.4, 6.0, 0.4)),
        (Vec3::new(0.0, 3.0, -6.0), Vec3::new(12.4, 6.0, 0.4)),
    ] {
        scene.meshes.push(cube(pos, extent, wall));
    }
    let mut light = point_light(Vec3::new(0.0, 3.0, 0.0), 8.0, 16.0);
    scene.lights = vec![light];
    let plain = renderer.render(&scene, size).expect("an unshadowed render");
    light.shadow = Some(crate::scene::LocalShadowData {
        bias: 0.02,
        near: 0.1,
        distance: None,
        fade: 0.0,
    });
    scene.lights = vec![light];
    let shadowed = renderer.render(&scene, size).expect("a shadowed render");

    for outside in [
        Vec3::new(9.0, 0.0, 0.0),
        Vec3::new(-9.0, 0.0, 0.0),
        Vec3::new(0.0, 0.0, 9.0),
        Vec3::new(0.0, 0.0, -9.0),
    ] {
        let (x, y) = pixel_of(&scene, outside, w, h);
        let (before, after) = (luma(&plain, x, y), luma(&shadowed, x, y));
        assert!(
            before > 15,
            "the floor at {outside:?} is lit without shadows"
        );
        assert!(
            after * 10 < before,
            "the floor beyond a wall at {outside:?} must be dark: {before} -> {after}",
        );
    }
    let (x0, y0) = pixel_of(&scene, Vec3::new(-4.5, 0.0, -4.5), w, h);
    let (x1, y1) = pixel_of(&scene, Vec3::new(4.5, 0.0, 4.5), w, h);
    assert!(
        max_channel_diff(&plain, &shadowed, x0..x1, y0..y1) <= 3,
        "the open floor crossing every cube-face boundary must match the unshadowed frame",
    );
}

/// **A point light's shadow fades through its `shadow_distance` band and nothing else moves.** A
/// slab shadows the floor at the origin; the camera rises straight above it, so the origin pixel
/// stays central and the view vector near-constant while the footprint distance crosses the band.
/// Before the band the frame equals the one with no distance; past the limit it equals the one
/// with no shadow and the plan has no view; inside, the origin brightens without a jump.
#[test]
fn needs_gpu_a_local_shadow_fades_through_its_band() {
    let Some(renderer) = skip_or_renderer("a_local_shadow_fades_through_its_band") else {
        return;
    };
    let (w, h) = (192u32, 108u32);
    let size = FrameSize::native(w, h);
    let light_at = Vec3::new(-6.0, 6.0, 0.0);
    let build = |eye_y: f32, shadow: Option<(Option<f32>, f32)>| {
        let mut scene = local_receive_scene(w, h);
        let eye = Vec3::new(0.0, eye_y, 0.01);
        scene.camera.view = Mat4::look_at_rh(eye, Vec3::ZERO, Vec3::Y);
        scene.camera.position = eye;
        scene.meshes.push(cube(
            Vec3::new(-3.0, 3.0, 0.0),
            Vec3::new(1.5, 0.4, 1.5),
            [0.8, 0.8, 0.8, 1.0],
        ));
        let mut light = point_light(light_at, 2.0, 16.0);
        light.shadow = shadow.map(|(distance, fade)| crate::scene::LocalShadowData {
            bias: 0.02,
            near: 0.1,
            distance,
            fade,
        });
        scene.lights = vec![light];
        scene
    };
    let origin = |scene: &RenderScene| pixel_of(scene, Vec3::ZERO, w, h);

    // Footprint distance is |eye - light| - 16: about 3 at y = 24, 12 at y = 33.4.
    let mut lumas = Vec::new();
    let mut y = 24.0_f32;
    while y <= 38.0 {
        let scene = build(y, Some((Some(12.0), 6.0)));
        let image = renderer.render(&scene, size).expect("a faded render");
        let (x, py) = origin(&scene);
        lumas.push(luma(&image, x, py));

        if y == 24.0 {
            let twin = build(y, Some((None, 0.0)));
            let twin = renderer.render(&twin, size).expect("a no-distance twin");
            assert_eq!(
                image.rgba, twin.rgba,
                "before the band: equal to no distance"
            );
        }
        y += 0.3;
    }
    assert!(lumas.len() >= 16);
    assert!(
        lumas.windows(2).all(|p| p[0] <= p[1]),
        "the origin must brighten monotonically: {lumas:?}"
    );
    let span = lumas.last().unwrap() - lumas[0];
    assert!(
        span > 50,
        "shadowed to lit spans more than 50 levels: {lumas:?}"
    );
    let worst = lumas.windows(2).map(|p| p[1] - p[0]).max().unwrap();
    assert!(
        worst as f32 <= 0.15 * span as f32,
        "no step above 15% of the span ({worst} of {span}): {lumas:?}"
    );

    let past = build(38.0, Some((Some(12.0), 6.0)));
    let unshadowed = build(38.0, None);
    assert!(
        crate::local_shadow::plan_local_shadows(&past, 4096)
            .views
            .is_empty(),
        "past the limit the light takes no atlas cell"
    );
    let a = renderer.render(&past, size).expect("a past-limit render");
    let b = renderer
        .render(&unshadowed, size)
        .expect("an unshadowed render");
    assert_eq!(a.rgba, b.rgba, "past the limit: equal to no shadow");
}

/// **Two granted spots in neighbouring atlas tiles do not read each other.** Each half of the
/// frame renders as it does with its own spot alone.
#[test]
fn needs_gpu_two_granted_spots_in_adjacent_tiles_do_not_bleed() {
    let Some(renderer) = skip_or_renderer("two_granted_spots_in_adjacent_tiles_do_not_bleed")
    else {
        return;
    };
    let (w, h) = (192u32, 108u32);
    let size = FrameSize::native(w, h);
    let mut scene = local_receive_scene(w, h);
    for x in [-10.0_f32, 10.0] {
        scene.meshes.push(cube(
            Vec3::new(x, 1.0, 0.0),
            Vec3::splat(2.0),
            [0.8, 0.8, 0.8, 1.0],
        ));
    }
    let left = casting_spot(Vec3::new(-10.0, 6.0, 0.0), 9.0, true);
    let right = casting_spot(Vec3::new(10.0, 6.0, 0.0), 9.0, true);
    scene.spot_lights = vec![left, right];
    let both = renderer.render(&scene, size).expect("both spots");
    scene.spot_lights = vec![left];
    let left_alone = renderer.render(&scene, size).expect("the left spot");
    scene.spot_lights = vec![right];
    let right_alone = renderer.render(&scene, size).expect("the right spot");

    assert!(
        max_channel_diff(&both, &left_alone, 0..w / 2, 0..h) <= 1,
        "the left half must not see the right tile",
    );
    assert!(
        max_channel_diff(&both, &right_alone, w / 2..w, 0..h) <= 1,
        "the right half must not see the left tile",
    );
}

/// **An opted-in light that is not granted renders as an unshadowed one.** Sixteen nearer spots
/// take the whole budget; the seventeenth, over an occluder, then renders byte-identically to the
/// same scene with its opt-in off, while granted it would differ.
#[test]
fn needs_gpu_a_light_beyond_the_budget_renders_as_if_unshadowed() {
    let Some(renderer) = skip_or_renderer("a_light_beyond_the_budget_renders_as_if_unshadowed")
    else {
        return;
    };
    let (w, h) = (192u32, 108u32);
    let size = FrameSize::native(w, h);
    let mut scene = local_receive_scene(w, h);
    scene.meshes.push(cube(
        Vec3::new(25.0, 1.0, 0.0),
        Vec3::splat(2.0),
        [0.8, 0.8, 0.8, 1.0],
    ));
    let far = Vec3::new(25.0, 8.0, 0.0);
    let render_with = |filler_casts: bool, far_casts: bool| {
        let mut s = scene.clone();
        s.spot_lights = (0..16)
            .map(|i| {
                let angle = i as f32 * std::f32::consts::TAU / 16.0;
                let position = Vec3::new(angle.cos() * 4.0, 3.0, angle.sin() * 4.0);
                casting_spot(position, 4.0, filler_casts)
            })
            .collect();
        s.spot_lights.push(casting_spot(far, 20.0, far_casts));
        let plan = crate::local_shadow::plan_local_shadows(&s, 4096);
        (renderer.render(&s, size).expect("a render"), plan)
    };
    let (off, _) = render_with(true, false);
    let (starved, plan) = render_with(true, true);
    assert_eq!(
        plan.light_first_view[16],
        crate::local_shadow::UNSHADOWED,
        "the seventeenth light must lose the budget",
    );
    assert_eq!(starved.rgba, off.rgba, "an ungranted light must not shadow");
    let (granted_off, _) = render_with(false, false);
    let (granted, _) = render_with(false, true);
    assert_ne!(
        granted.rgba, granted_off.rgba,
        "the same light, granted, must cast",
    );
}

/// **A skinned caster shadows by its posed shape.** A flat skinned quad over the floor casts at
/// the rest pose's footprint, and a bent pose that moves the quad away moves the shadow with it.
#[test]
fn needs_gpu_a_skinned_caster_shadows_by_its_pose() {
    let Some(renderer) = skip_or_renderer("a_skinned_caster_shadows_by_its_pose") else {
        return;
    };
    let (w, h) = (192u32, 108u32);
    let size = FrameSize::native(w, h);
    let mut scene = local_receive_scene(w, h);
    scene
        .mesh_assets
        .push(Arc::new(skin_quad([1.0, 0.0, 0.0, 0.0], [0, 0, 0, 0], 1)));
    let flat = Mat4::from_translation(Vec3::new(0.0, 3.0, 0.0))
        * Mat4::from_rotation_x(-std::f32::consts::FRAC_PI_2)
        * Mat4::from_scale(Vec3::splat(1.5));
    scene.meshes.push(skin_instance(flat, 1));
    scene.spot_lights = vec![casting_spot(Vec3::new(0.0, 8.0, 0.0), 20.0, true)];

    let (probe_x, probe_y) = pixel_of(&scene, Vec3::new(2.2, 0.0, 0.0), w, h);
    scene.bone_palette = vec![Mat4::IDENTITY];
    let rest = renderer.render(&scene, size).expect("the rest pose");
    scene.bone_palette = vec![Mat4::from_translation(Vec3::new(8.0, 0.0, 0.0))];
    let bent = renderer.render(&scene, size).expect("the bent pose");

    let (resting, moved) = (luma(&rest, probe_x, probe_y), luma(&bent, probe_x, probe_y));
    assert!(
        resting * 3 < moved,
        "the rest pose shadows (2.2, 0, 0) and the moved quad no longer does: {resting} vs {moved}",
    );
}

/// **A drop shadow darkens the floor under its footprint and nothing outside it.** The pixel under
/// the anchor is clearly darker than the same frame without the shadow, a pixel well outside the
/// footprint is byte-identical to it, and a scene without a drop shadow renders the same bytes as
/// one whose list is empty by construction.
#[test]
fn needs_gpu_a_drop_shadow_darkens_the_floor_under_its_footprint() {
    let Some(renderer) = skip_or_renderer("a_drop_shadow_darkens_the_floor_under_its_footprint")
    else {
        return;
    };
    let (w, h) = (160u32, 120u32);
    let size = FrameSize::native(w, h);
    let bare = drop_shadow_fixture(w, h, false);
    let mut shadowed = bare.clone();
    shadowed.drop_shadows.push(fixture_drop_shadow(Vec::new()));
    let lit = renderer.render(&bare, size).expect("a bare render");
    let dark = renderer.render(&shadowed, size).expect("a shadowed render");

    let (x, y) = pixel_of(&bare, Vec3::ZERO, w, h);
    let (before, after) = (luma(&lit, x, y), luma(&dark, x, y));
    assert!(
        after * 10 < before * 6,
        "the floor under the anchor must darken: {before} -> {after}",
    );
    for outside in [Vec3::new(6.0, 0.0, 0.0), Vec3::new(-5.0, 0.0, -4.0)] {
        let (x, y) = pixel_of(&bare, outside, w, h);
        assert_eq!(
            luma(&dark, x, y),
            luma(&lit, x, y),
            "{outside:?} is outside the footprint",
        );
    }
    let again = renderer.render(&bare, size).expect("a second bare render");
    assert_eq!(
        lit.rgba, again.rgba,
        "a drop-shadow-free frame is deterministic"
    );
}

/// Asserts the `cast_shadows` contract on `scene_with(occluder)`, where the occluder is a mesh
/// outside the camera frustum that shadows the visible floor at `probe`: with `cast_shadows` false
/// the frame is byte-identical to the frame without it, and with `true` the probe darkens.
fn assert_shadowless_occluder_is_absent(
    renderer: &Renderer,
    size: FrameSize,
    probe: (u32, u32),
    scene_with: impl Fn(Option<bool>) -> RenderScene,
) {
    let absent = renderer.render(&scene_with(None), size).expect("absent");
    let silent = renderer
        .render(&scene_with(Some(false)), size)
        .expect("shadowless occluder");
    let casting = renderer
        .render(&scene_with(Some(true)), size)
        .expect("casting occluder");
    assert_eq!(
        silent.rgba, absent.rgba,
        "a shadowless occluder must leave the frame byte-identical to no occluder"
    );
    let (lit, dark) = (
        luma(&absent, probe.0, probe.1),
        luma(&casting, probe.0, probe.1),
    );
    assert!(
        dark * 10 < lit * 8,
        "sanity: the same occluder casting must darken the probe: {lit} -> {dark}"
    );
}

/// A floor seen from (0, 3, 11) and, when `slab` is set, a slab behind and above that camera (out
/// of frame) whose shadow a sun travelling toward -Z throws onto the visible floor at z = 6.5.
/// The flag is the slab's `cast_shadows`.
fn shadow_camera_scene(w: u32, h: u32, slab: Option<bool>) -> RenderScene {
    let mut scene = RenderScene::empty((w, h));
    scene.ambient = [0.05, 0.05, 0.05];
    let eye = Vec3::new(0.0, 3.0, 11.0);
    scene.camera = crate::scene::RenderCamera {
        view: Mat4::look_at_rh(eye, Vec3::ZERO, Vec3::Y),
        projection: Mat4::perspective_rh(60.0_f32.to_radians(), w as f32 / h as f32, 0.1, 100.0),
        position: eye,
    };
    scene.meshes = vec![ground(20.0)];
    if let Some(cast) = slab {
        let mut slab = cube(
            Vec3::new(0.0, 8.0, 16.0),
            Vec3::new(40.0, 0.4, 6.0),
            [0.4, 0.4, 0.4, 1.0],
        );
        slab.cast_shadows = cast;
        scene.meshes.push(slab);
    }
    scene
}

/// **`cast_shadows: false` keeps an instance out of the anchored sun map.**
#[test]
fn needs_gpu_a_shadowless_instance_writes_no_anchored_sun_shadow() {
    let Some(renderer) = skip_or_renderer("a_shadowless_instance_anchored_sun") else {
        return;
    };
    let (w, h) = (64u32, 64u32);
    let build = |slab| {
        let mut scene = shadow_camera_scene(w, h, slab);
        scene.sun = Some(test_sun(
            Vec3::new(0.0, -1.0, -1.0),
            Vec3::splat(3.0),
            0.002,
        ));
        scene
    };
    let probe = pixel_of(&build(None), Vec3::new(0.0, 0.0, 6.5), w, h);
    assert_shadowless_occluder_is_absent(&renderer, FrameSize::native(w, h), probe, build);
}

/// **`cast_shadows: false` keeps an instance out of every sun cascade.**
#[test]
fn needs_gpu_a_shadowless_instance_writes_no_cascade_shadow() {
    let Some(renderer) = sun_renderer("a_shadowless_instance_cascades", 512, 3) else {
        return;
    };
    let (w, h) = (64u32, 64u32);
    let build = |slab| {
        let mut scene = shadow_camera_scene(w, h, slab);
        let mut sun = test_sun(Vec3::new(0.0, -1.0, -1.0), Vec3::splat(3.0), 0.002);
        sun.cascades = Some(crate::scene::SunCascadeConfig {
            count: 3,
            shadow_distance: 20.0,
            splits: None,
            shadow_near: -100.0,
            shadow_far: 100.0,
        });
        scene.sun = Some(sun);
        scene
    };
    let probe = pixel_of(&build(None), Vec3::new(0.0, 0.0, 6.5), w, h);
    assert_shadowless_occluder_is_absent(&renderer, FrameSize::native(w, h), probe, build);
}

/// A top-down floor lit by a shadow-granted local light 40 m up, with a slab between the light and
/// the floor but above the camera (so it is behind the near plane). `point` picks a point light
/// over a spot.
fn local_shadow_scene(w: u32, h: u32, slab: Option<bool>, point: bool) -> RenderScene {
    let mut scene = local_receive_scene(w, h);
    let at = Vec3::new(0.0, 40.0, 0.0);
    let shadow = Some(crate::scene::LocalShadowData {
        bias: 0.02,
        near: 0.1,
        distance: None,
        fade: 0.0,
    });
    if point {
        let mut light = point_light(at, 150.0, 60.0);
        light.shadow = shadow;
        scene.lights = vec![light];
    } else {
        let mut spot = casting_spot(at, 60.0, true);
        spot.intensity = 150.0;
        spot.shadow = shadow;
        scene.spot_lights = vec![spot];
    }
    if let Some(cast) = slab {
        let mut slab = cube(
            Vec3::new(0.0, 35.0, 0.0),
            Vec3::new(8.0, 0.4, 8.0),
            [0.4, 0.4, 0.4, 1.0],
        );
        slab.cast_shadows = cast;
        scene.meshes.push(slab);
    }
    scene
}

/// **`cast_shadows: false` keeps an instance out of the local shadow atlas**, for a spot and a
/// point light alike.
#[test]
fn needs_gpu_a_shadowless_instance_writes_no_local_shadow() {
    let Some(renderer) = skip_or_renderer("a_shadowless_instance_local_shadow") else {
        return;
    };
    let (w, h) = (96u32, 96u32);
    for point in [false, true] {
        let probe = pixel_of(&local_shadow_scene(w, h, None, point), Vec3::ZERO, w, h);
        assert_shadowless_occluder_is_absent(&renderer, FrameSize::native(w, h), probe, |slab| {
            local_shadow_scene(w, h, slab, point)
        });
    }
}

/// **`cast_shadows: false` keeps an instance out of drop-shadow occlusion.** The plate no longer
/// shields the floor beneath it, exactly as if it were listed in `exclude`.
#[test]
fn needs_gpu_a_shadowless_plate_does_not_shield_a_drop_shadow() {
    let Some(renderer) = skip_or_renderer("a_shadowless_plate_drop_shadow") else {
        return;
    };
    let (w, h) = (160u32, 120u32);
    let size = FrameSize::native(w, h);
    let render_floor = |cast: bool| {
        let mut bare = drop_shadow_fixture(w, h, true);
        bare.meshes[1].cast_shadows = cast;
        let mut shadowed = bare.clone();
        shadowed.drop_shadows.push(fixture_drop_shadow(Vec::new()));
        let lit = renderer.render(&bare, size).expect("a bare render");
        let dark = renderer.render(&shadowed, size).expect("a shadowed render");
        let (fx, fy) = pixel_of(&bare, Vec3::new(0.8, 0.0, 0.5), w, h);
        (luma(&lit, fx, fy), luma(&dark, fx, fy))
    };
    let (lit, dark) = render_floor(true);
    assert_eq!(dark, lit, "sanity: a casting plate shields the floor");
    let (lit, dark) = render_floor(false);
    assert!(
        dark * 10 < lit * 6,
        "a shadowless plate lets the shadow reach the floor: {lit} -> {dark}"
    );
}

/// Renders the plate fixture bare and with a drop shadow excluding `exclude`, and returns the
/// luma of the floor under the plate and of the plate's top in each.
fn plate_lumas(renderer: &Renderer, exclude: Vec<u32>) -> ((u32, u32), (u32, u32)) {
    let (w, h) = (160u32, 120u32);
    let size = FrameSize::native(w, h);
    let bare = drop_shadow_fixture(w, h, true);
    let mut shadowed = bare.clone();
    shadowed.drop_shadows.push(fixture_drop_shadow(exclude));
    let lit = renderer.render(&bare, size).expect("a bare render");
    let dark = renderer.render(&shadowed, size).expect("a shadowed render");
    let (fx, fy) = pixel_of(&bare, Vec3::new(0.8, 0.0, 0.5), w, h);
    let (px, py) = pixel_of(&bare, Vec3::new(1.0, 0.51, 0.0), w, h);
    let floor = (luma(&lit, fx, fy), luma(&dark, fx, fy));
    let top = (luma(&lit, px, py), luma(&dark, px, py));
    assert!(
        floor.0 > 0 && top.0 > 0,
        "the fixture is lit: {floor:?} {top:?}"
    );
    (floor, top)
}

/// **The shadow lands on the first surface below the anchor.** A thin plate under half the
/// footprint takes the shadow on its top and spares the floor beneath it.
#[test]
fn needs_gpu_a_drop_shadow_lands_on_the_first_surface() {
    let Some(renderer) = skip_or_renderer("a_drop_shadow_lands_on_the_first_surface") else {
        return;
    };
    let (floor, top) = plate_lumas(&renderer, Vec::new());
    assert_eq!(
        floor.1, floor.0,
        "the floor under the plate is not the first surface"
    );
    assert!(
        top.1 * 10 < top.0 * 6,
        "the plate top takes the shadow: {top:?}"
    );
}

/// **A caster's own geometry does not shield its shadow.** Listing the plate's mesh index in
/// `exclude` drops it from the occlusion pass, so the shadow reaches the floor beneath it.
#[test]
fn needs_gpu_a_drop_shadow_skips_its_own_geometry() {
    let Some(renderer) = skip_or_renderer("a_drop_shadow_skips_its_own_geometry") else {
        return;
    };
    let (floor, _) = plate_lumas(&renderer, vec![1]);
    assert!(
        floor.1 * 10 < floor.0 * 6,
        "with the plate excluded the floor beneath it is darkened: {floor:?}",
    );
}

/// **A ramp the caster stands on takes the whole shadow; a flat top above the anchor takes none.**
/// A 15° slab passes under the anchor: its surface 0.6 m uphill and 0.6 m downhill of the
/// anchor darkens alike, because the receiver's plane extended under the anchor sits at the
/// anchor's height. A block whose top is 0.3 m above the anchor stays byte-identical.
#[test]
fn needs_gpu_a_drop_shadow_wraps_a_slope_and_spares_a_step_above() {
    let Some(renderer) = skip_or_renderer("a_drop_shadow_wraps_a_slope_and_spares_a_step_above")
    else {
        return;
    };
    let (w, h) = (160u32, 120u32);
    let size = FrameSize::native(w, h);
    let angle = 15.0_f32.to_radians();
    let surface = |x: f32| 0.5 + 0.05 / angle.cos() + angle.tan() * x;

    let mut bare = drop_shadow_fixture(w, h, false);
    let mut ramp = cube(
        Vec3::new(0.0, 0.5, 0.0),
        Vec3::new(6.0, 0.1, 4.0),
        [0.8, 0.8, 0.8, 1.0],
    );
    ramp.model = Mat4::from_translation(Vec3::new(0.0, 0.5, 0.0))
        * Mat4::from_rotation_z(angle)
        * Mat4::from_scale(Vec3::new(6.0, 0.1, 4.0));
    ramp.normal_matrix = Mat3::from_rotation_z(angle);
    bare.meshes.push(ramp);
    let mut shadowed = bare.clone();
    let mut shadow = fixture_drop_shadow(Vec::new());
    shadow.anchor = Vec3::new(0.0, surface(0.0) + 0.01, 0.0);
    shadowed.drop_shadows.push(shadow);
    let lit = renderer.render(&bare, size).expect("a bare render");
    let dark = renderer.render(&shadowed, size).expect("a shadowed render");
    for x in [-0.6_f32, 0.6] {
        let (px, py) = pixel_of(&bare, Vec3::new(x, surface(x), 0.0), w, h);
        let (before, after) = (luma(&lit, px, py), luma(&dark, px, py));
        assert!(
            after * 10 < before * 8,
            "the ramp at x = {x} must darken: {before} -> {after}",
        );
    }

    let mut bare = drop_shadow_fixture(w, h, false);
    bare.meshes.push(cube(
        Vec3::new(0.8, 1.15, 0.0),
        Vec3::new(1.0, 0.3, 1.0),
        [0.8, 0.8, 0.8, 1.0],
    ));
    let mut shadowed = bare.clone();
    shadowed.drop_shadows.push(fixture_drop_shadow(Vec::new()));
    let lit = renderer.render(&bare, size).expect("a bare render");
    let dark = renderer.render(&shadowed, size).expect("a shadowed render");
    let (px, py) = pixel_of(&bare, Vec3::new(0.8, 1.31, 0.0), w, h);
    assert!(luma(&lit, px, py) > 0, "the block top is lit");
    assert_eq!(
        luma(&dark, px, py),
        luma(&lit, px, py),
        "a flat top above the anchor is never shadowed",
    );
}

/// **A perturbed shading normal does not cut the blob.** A floor whose shading normal is tilted
/// (as a normal map or smooth vertex normals do) but whose geometry is flat takes the whole blob
/// of a caster standing on it, on both sides of the anchor along either axis.
#[test]
fn needs_gpu_a_drop_shadow_ignores_a_perturbed_shading_normal() {
    let Some(renderer) = skip_or_renderer("a_drop_shadow_ignores_a_perturbed_shading_normal")
    else {
        return;
    };
    let (w, h) = (160u32, 120u32);
    let size = FrameSize::native(w, h);
    let tilt = 12.0_f32.to_radians();
    for (normal_matrix, axis) in [
        (Mat3::from_rotation_z(tilt), Vec3::X),
        (Mat3::from_rotation_x(tilt), Vec3::Z),
    ] {
        let mut bare = drop_shadow_fixture(w, h, false);
        bare.meshes[0].normal_matrix = normal_matrix;
        let mut shadowed = bare.clone();
        let mut shadow = fixture_drop_shadow(Vec::new());
        shadow.anchor = Vec3::new(0.0, 0.01, 0.0);
        shadowed.drop_shadows.push(shadow);
        let lit = renderer.render(&bare, size).expect("a bare render");
        let dark = renderer.render(&shadowed, size).expect("a shadowed render");
        for offset in [-0.6_f32, 0.6] {
            let (px, py) = pixel_of(&bare, axis * offset, w, h);
            let (before, after) = (luma(&lit, px, py), luma(&dark, px, py));
            assert!(
                before > 0,
                "the tilted floor is lit at {offset} along {axis:?}"
            );
            assert!(
                after * 10 < before * 8,
                "the floor at {offset} along {axis:?} must darken: {before} -> {after}",
            );
        }
    }
}

/// **A drop shadow at every quality**: the occlusion atlas and the projector are content, not
/// effects, so the lowest preset still darkens the floor.
#[test]
fn needs_gpu_a_drop_shadow_survives_the_lowest_quality() {
    let Some(mut renderer) = skip_or_renderer("a_drop_shadow_survives_the_lowest_quality") else {
        return;
    };
    let (w, h) = (160u32, 120u32);
    let size = FrameSize::native(w, h);
    let bare = drop_shadow_fixture(w, h, false);
    let mut shadowed = bare.clone();
    shadowed.drop_shadows.push(fixture_drop_shadow(Vec::new()));
    renderer.set_quality(Quality::LOW, QualityParams::LOW);
    let lit = renderer.render(&bare, size).expect("a bare render");
    let dark = renderer.render(&shadowed, size).expect("a shadowed render");
    let (x, y) = pixel_of(&bare, Vec3::ZERO, w, h);
    assert!(luma(&dark, x, y) * 10 < luma(&lit, x, y) * 6);
}

/// The drop-shadow fixture's floor under a decal box centred on the origin (`size` metres, so the
/// floor runs through its middle); the plate (mesh 1) is a thin top-facing surface inside it.
fn decal_instance(blend: crate::scene::DecalBlend, size: Vec3) -> crate::scene::DecalInstance {
    let box_to_world = Mat4::from_scale(size);
    crate::scene::DecalInstance {
        box_to_world,
        world_to_box: box_to_world.inverse(),
        blend,
        color: [0.0, 0.0, 0.0, 1.0],
        opacity: 1.0,
        roughness: None,
        metallic: 0.0,
        uv_rect: [0.0, 0.0, 1.0, 1.0],
        angle_fade: [60.0, 80.0],
        layer: 0,
        maps: crate::scene::DecalMaps::default(),
        receivers: crate::scene::DecalReceivers::Exclude(Vec::new()),
    }
}

/// A two-texel map: the left texel is opaque black, the right one fully transparent.
fn decal_half_map() -> crate::scene::MeshTexture {
    crate::scene::MeshTexture {
        image: MapImage::Source(Arc::new(TextureImage {
            width: 2,
            height: 1,
            rgba: vec![0, 0, 0, 255, 0, 0, 0, 0],
            hash: 0xDECA1,
            filter_linear: true,
            premultiplied_on_load: false,
            mean: [0.5; 4],
        })),
        wrap: TextureWrap::Clamp,
    }
}

/// Renders `scene` bare and with `decals`, returning both.
fn decal_pair(
    renderer: &Renderer,
    scene: &RenderScene,
    (w, h): (u32, u32),
    decals: Vec<crate::scene::DecalInstance>,
) -> (RenderedImage, RenderedImage) {
    let size = FrameSize::native(w, h);
    let bare = renderer.render(scene, size).expect("a bare render");
    let mut decorated = scene.clone();
    decorated.decals = decals;
    let painted = renderer.render(&decorated, size).expect("a decal render");
    (bare, painted)
}

/// **A multiply decal darkens inside its box by its coverage, and nowhere outside it.** Opacity
/// 1 paints black, 0.5 lands strictly between, a pixel beyond the box is byte-identical.
#[test]
fn needs_gpu_a_multiply_decal_darkens_only_inside_its_box() {
    let Some(renderer) = skip_or_renderer("a_multiply_decal_darkens_only_inside_its_box") else {
        return;
    };
    let (w, h) = (160u32, 120u32);
    let scene = drop_shadow_fixture(w, h, false);
    let size = Vec3::new(3.0, 1.0, 3.0);
    let full = decal_instance(crate::scene::DecalBlend::Multiply, size);
    let mut half = full.clone();
    half.opacity = 0.5;
    let (bare, black) = decal_pair(&renderer, &scene, (w, h), vec![full]);
    let (_, grey) = decal_pair(&renderer, &scene, (w, h), vec![half]);
    let (x, y) = pixel_of(&scene, Vec3::ZERO, w, h);
    let lit = luma(&bare, x, y);
    assert!(lit > 30, "the fixture floor is lit: {lit}");
    assert!(luma(&black, x, y) <= 3, "opacity 1 paints black");
    let mid = luma(&grey, x, y);
    assert!(
        mid * 10 > lit * 2 && mid * 10 < lit * 8,
        "opacity 0.5 lands between: {lit} -> {mid}"
    );
    for outside in [Vec3::new(4.0, 0.0, 0.0), Vec3::new(-3.0, 0.0, 3.5)] {
        let (x, y) = pixel_of(&scene, outside, w, h);
        assert_eq!(
            luma(&black, x, y),
            luma(&bare, x, y),
            "{outside:?} is outside the box"
        );
    }
}

/// **An add decal brightens by its tint.**
#[test]
fn needs_gpu_an_add_decal_brightens() {
    let Some(renderer) = skip_or_renderer("an_add_decal_brightens") else {
        return;
    };
    let (w, h) = (160u32, 120u32);
    let scene = drop_shadow_fixture(w, h, false);
    let mut glow = decal_instance(crate::scene::DecalBlend::Add, Vec3::new(3.0, 1.0, 3.0));
    glow.color = [0.5, 0.0, 0.0, 1.0];
    let (bare, painted) = decal_pair(&renderer, &scene, (w, h), vec![glow]);
    let (x, y) = pixel_of(&scene, Vec3::ZERO, w, h);
    let (before, after) = (pixel(&bare, x, y), pixel(&painted, x, y));
    assert!(
        after[0] > before[0] + 20,
        "red rises: {before:?} -> {after:?}"
    );
    // The tonemap desaturates, so the other channels move a little; red still leads.
    assert!(
        after[0] - before[0] > 2 * (after[1] - before[1]),
        "red leads: {before:?} -> {after:?}"
    );
}

/// **The angle fade spares a wall and paints the floor.** A thin wall faces the camera inside the
/// box: its normal is 90 degrees off the box's up, beyond the default fade's end.
#[test]
fn needs_gpu_the_angle_fade_leaves_a_wall_clean() {
    let Some(renderer) = skip_or_renderer("the_angle_fade_leaves_a_wall_clean") else {
        return;
    };
    let (w, h) = (160u32, 120u32);
    let mut scene = drop_shadow_fixture(w, h, false);
    scene.meshes.push(cube(
        Vec3::new(0.0, 0.5, -1.0),
        Vec3::new(3.0, 1.0, 0.02),
        [0.8, 0.8, 0.8, 1.0],
    ));
    let paint = decal_instance(crate::scene::DecalBlend::Multiply, Vec3::new(3.0, 1.0, 3.0));
    let (bare, painted) = decal_pair(&renderer, &scene, (w, h), vec![paint]);
    let (wx, wy) = pixel_of(&scene, Vec3::new(0.0, 0.25, -0.98), w, h);
    assert_eq!(
        luma(&painted, wx, wy),
        luma(&bare, wx, wy),
        "the wall is clean"
    );
    let (fx, fy) = pixel_of(&scene, Vec3::new(0.0, 0.0, 0.5), w, h);
    assert!(
        luma(&painted, fx, fy) * 2 < luma(&bare, fx, fy),
        "the floor is painted"
    );
}

/// **`Exclude` and `Only` lists decide the receiver per pixel.** The plate (mesh 1) is spared in
/// both forms while the floor stays painted, but for a rim of two pixels at most.
#[test]
fn needs_gpu_receiver_lists_spare_the_listed_mesh_per_pixel() {
    let Some(renderer) = skip_or_renderer("receiver_lists_spare_the_listed_mesh_per_pixel") else {
        return;
    };
    let (w, h) = (160u32, 120u32);
    let scene = drop_shadow_fixture(w, h, true);
    let decal = |receivers| {
        let mut d = decal_instance(crate::scene::DecalBlend::Multiply, Vec3::new(6.0, 2.0, 6.0));
        d.receivers = receivers;
        d
    };
    let (bare, everywhere) = decal_pair(
        &renderer,
        &scene,
        (w, h),
        vec![decal(crate::scene::DecalReceivers::Exclude(Vec::new()))],
    );
    let (px, py) = pixel_of(&scene, Vec3::new(1.0, 0.51, 0.0), w, h);
    assert!(
        luma(&everywhere, px, py) * 2 < luma(&bare, px, py),
        "with no list the plate top is painted"
    );
    for (name, receivers) in [
        ("exclude", crate::scene::DecalReceivers::Exclude(vec![1])),
        ("only", crate::scene::DecalReceivers::Only(vec![0])),
    ] {
        let (_, listed) = decal_pair(&renderer, &scene, (w, h), vec![decal(receivers)]);
        assert_eq!(
            luma(&listed, px, py),
            luma(&bare, px, py),
            "{name}: the plate top is clean"
        );
        let (fx, fy) = pixel_of(&scene, Vec3::new(-1.5, 0.0, 0.5), w, h);
        assert_eq!(
            luma(&listed, fx, fy),
            luma(&everywhere, fx, fy),
            "{name}: the floor is painted"
        );
        // Along the row through the plate, the pixels the plate owns are where the unlisted and
        // listed renders differ; the floor pixels beside that run must be painted from the third
        // pixel out.
        let row: Vec<bool> = (0..w)
            .map(|x| pixel(&listed, x, py) != pixel(&everywhere, x, py))
            .collect();
        let first = row.iter().position(|&d| d).expect("the plate differs");
        let last = row.iter().rposition(|&d| d).expect("the plate differs");
        for x in [first.saturating_sub(3), (last + 3).min(w as usize - 1)] {
            assert_eq!(
                pixel(&listed, x as u32, py),
                pixel(&everywhere, x as u32, py),
                "{name}: the floor beside the plate is painted (x = {x})"
            );
        }
    }
}

/// **An `Unskinned` decal spares a skinned mesh exactly as `Exclude` of it does.** The plate
/// (mesh 1) is skinned at an identity pose, so the scene pass flags its pixels (the sign of
/// `scene_normal`'s alpha) and no receiver pass is scheduled; the render equals the
/// `Exclude(vec![1])` one byte for byte, unlit and lit, with the floor painted.
#[test]
fn needs_gpu_an_unskinned_decal_spares_a_skinned_mesh() {
    let Some(renderer) = skip_or_renderer("an_unskinned_decal_spares_a_skinned_mesh") else {
        return;
    };
    let (w, h) = (160u32, 120u32);
    let mut scene = drop_shadow_fixture(w, h, true);
    scene.meshes[1].bone_offset = 1;
    scene.bone_palette = vec![Mat4::IDENTITY];
    let (fx, fy) = pixel_of(&scene, Vec3::new(-1.5, 0.0, 0.5), w, h);
    let (px, py) = pixel_of(&scene, Vec3::new(1.0, 0.51, 0.0), w, h);
    for (name, blend) in [
        ("multiply", crate::scene::DecalBlend::Multiply),
        ("lit alpha", crate::scene::DecalBlend::Alpha),
    ] {
        let decal = |receivers| {
            let mut d = decal_instance(blend, Vec3::new(6.0, 2.0, 6.0));
            if blend == crate::scene::DecalBlend::Alpha {
                d.color = [0.7, 0.7, 0.7, 1.0];
            }
            d.receivers = receivers;
            d
        };
        let (bare, unskinned) = decal_pair(
            &renderer,
            &scene,
            (w, h),
            vec![decal(crate::scene::DecalReceivers::Unskinned)],
        );
        let mut all_only = scene.clone();
        all_only.decals = vec![decal(crate::scene::DecalReceivers::Unskinned)];
        assert!(
            !PostChain::for_scene(&all_only).decal_receivers,
            "{name}: an all decal schedules no receiver-id pass"
        );
        let (_, excluded) = decal_pair(
            &renderer,
            &scene,
            (w, h),
            vec![decal(crate::scene::DecalReceivers::Exclude(vec![1]))],
        );
        assert_eq!(
            unskinned.rgba, excluded.rgba,
            "{name}: Unskinned equals Exclude of the skinned mesh"
        );
        assert_ne!(
            pixel(&unskinned, fx, fy),
            pixel(&bare, fx, fy),
            "{name}: the floor is painted"
        );
        assert_pixels_within(
            pixel(&unskinned, px, py),
            pixel(&bare, px, py),
            2,
            &format!("{name}: the skinned plate is clean"),
        );
    }
}

/// **A mask mesh's holes claim no receiver id.** A mask quad 0.5 mm above the floor, cut over its
/// left half, sits inside the depth slack of the floor beneath it. A decal listing only the quad
/// paints the quad's opaque half and leaves the floor under the holes as the bare render has it.
#[test]
fn needs_gpu_a_cutout_receiver_claims_no_pixel_through_its_holes() {
    let Some(renderer) = skip_or_renderer("a_cutout_receiver_claims_no_pixel_through_its_holes")
    else {
        return;
    };
    let (w, h) = (160u32, 120u32);
    let mut scene = drop_shadow_fixture(w, h, false);
    add_quad(
        &mut scene,
        mask_quad(0xF4_00, 0, 255),
        flat_quad_model(0.0005, 3.0),
    );
    let mut decal = decal_instance(crate::scene::DecalBlend::Multiply, Vec3::new(8.0, 2.0, 8.0));
    decal.receivers = crate::scene::DecalReceivers::Only(vec![1]);
    let (bare, painted) = decal_pair(&renderer, &scene, (w, h), vec![decal]);

    let (hx, hy) = pixel_of(&scene, Vec3::new(-1.5, 0.0005, 0.5), w, h);
    let (ox, oy) = pixel_of(&scene, Vec3::new(1.5, 0.0005, 0.5), w, h);
    assert!(luma(&bare, hx, hy) > 30, "the floor under the holes is lit");
    assert!(
        luma(&painted, ox, oy) * 2 < luma(&bare, ox, oy),
        "the opaque half is painted"
    );
    assert_eq!(
        luma(&painted, hx, hy),
        luma(&bare, hx, hy),
        "the floor under the holes is not the quad's to paint"
    );
}

/// **A map's `uv_rect` picks the texels the box shows**: the left texel is opaque black, the
/// right one clear, so a rect inside each half paints the whole box or none of it.
#[test]
fn needs_gpu_a_uv_rect_selects_the_texels_the_box_shows() {
    let Some(renderer) = skip_or_renderer("a_uv_rect_selects_the_texels_the_box_shows") else {
        return;
    };
    let (w, h) = (160u32, 120u32);
    let scene = drop_shadow_fixture(w, h, false);
    let mapped = |rect: [f32; 4]| {
        let mut d = decal_instance(crate::scene::DecalBlend::Multiply, Vec3::new(3.0, 1.0, 3.0));
        d.color = [1.0; 4];
        d.maps.base_color = Some(decal_half_map());
        d.uv_rect = rect;
        d
    };
    let (bare, left) = decal_pair(
        &renderer,
        &scene,
        (w, h),
        vec![mapped([0.0, 0.0, 0.25, 1.0])],
    );
    let (_, right) = decal_pair(
        &renderer,
        &scene,
        (w, h),
        vec![mapped([0.75, 0.0, 1.0, 1.0])],
    );
    let (x, y) = pixel_of(&scene, Vec3::ZERO, w, h);
    assert!(luma(&left, x, y) <= 3, "the black texel paints the box");
    assert_eq!(
        luma(&right, x, y),
        luma(&bare, x, y),
        "the clear texel paints nothing"
    );
}

/// **A drop shadow draws after every decal**, so it darkens an add decal's glow too.
#[test]
fn needs_gpu_a_drop_shadow_darkens_an_add_decal() {
    let Some(renderer) = skip_or_renderer("a_drop_shadow_darkens_an_add_decal") else {
        return;
    };
    let (w, h) = (160u32, 120u32);
    let scene = drop_shadow_fixture(w, h, false);
    let size = FrameSize::native(w, h);
    let mut glow = decal_instance(crate::scene::DecalBlend::Add, Vec3::new(3.0, 1.0, 3.0));
    glow.color = [0.1, 0.1, 0.1, 1.0];
    let mut glowing = scene.clone();
    glowing.decals.push(glow);
    let mut shadowed = glowing.clone();
    shadowed.drop_shadows.push(fixture_drop_shadow(Vec::new()));
    let bright = renderer.render(&glowing, size).expect("the glow");
    let dark = renderer
        .render(&shadowed, size)
        .expect("the glow and the shadow");
    let (x, y) = pixel_of(&scene, Vec3::ZERO, w, h);
    let (lit, shadowed) = (luma(&bright, x, y), luma(&dark, x, y));
    assert!(
        shadowed * 10 < lit * 7,
        "the shadow darkens the glow: {lit} -> {shadowed}"
    );
}

/// The decal fixture lit by a steep sun and the default ambient: a floor (mesh 0) under a wide
/// box, seen by `drop_shadow_fixture`'s camera.
fn lit_decal_fixture(w: u32, h: u32) -> RenderScene {
    let mut scene = drop_shadow_fixture(w, h, false);
    scene.sun = Some(test_sun(Vec3::new(0.3, -1.0, 0.2), Vec3::splat(2.0), 0.002));
    scene
}

/// A lit decal of `blend` over a wide box, opaque, tinted `color`.
fn lit_decal(blend: crate::scene::DecalBlend, color: [f32; 3]) -> crate::scene::DecalInstance {
    let mut decal = decal_instance(blend, Vec3::new(8.0, 2.0, 8.0));
    decal.color = [color[0], color[1], color[2], 1.0];
    decal
}

/// A 1×1 map of `rgba` keyed by `hash`.
fn one_texel_map(rgba: [u8; 4], hash: u64) -> crate::scene::MeshTexture {
    crate::scene::MeshTexture {
        image: MapImage::Source(Arc::new(TextureImage {
            width: 1,
            height: 1,
            rgba: rgba.to_vec(),
            hash,
            filter_linear: true,
            premultiplied_on_load: false,
            mean: [0.5; 4],
        })),
        wrap: TextureWrap::Clamp,
    }
}

fn assert_pixels_within(a: [u8; 4], b: [u8; 4], tolerance: u8, what: &str) {
    for channel in 0..3 {
        assert!(
            a[channel].abs_diff(b[channel]) <= tolerance,
            "{what}: {a:?} vs {b:?} (channel {channel})",
        );
    }
}

/// **An opaque alpha decal is a painted floor.** Paint of colour X over the floor, at the floor's
/// own roughness and metallic, shades like the same floor authored with albedo X.
#[test]
fn needs_gpu_an_opaque_alpha_decal_matches_a_painted_mesh() {
    let Some(renderer) = skip_or_renderer("an_opaque_alpha_decal_matches_a_painted_mesh") else {
        return;
    };
    let (w, h) = (160u32, 120u32);
    let paint = [0.2, 0.5, 0.8];
    let scene = lit_decal_fixture(w, h);
    let mut decal = lit_decal(crate::scene::DecalBlend::Alpha, paint);
    decal.roughness = Some(1.0);
    let (bare, painted) = decal_pair(&renderer, &scene, (w, h), vec![decal]);
    let mut authored = scene.clone();
    authored.meshes[0].color = [paint[0], paint[1], paint[2], 1.0];
    let reference = renderer
        .render(&authored, FrameSize::native(w, h))
        .expect("an authored render");
    let (x, y) = pixel_of(&scene, Vec3::ZERO, w, h);
    assert!(luma(&painted, x, y) > 30 && luma(&painted, x, y) != luma(&bare, x, y));
    assert_pixels_within(
        pixel(&painted, x, y),
        pixel(&reference, x, y),
        2,
        "the paint at the box centre",
    );
}

/// **Shadowed paint is dark, and low quality loses the shadow.** A cube between the sun and the
/// floor shadows part of the paint; at `Quality::LOW` the sun's shadow is gone, so the two spots
/// shade alike.
#[test]
fn needs_gpu_lit_paint_takes_the_suns_shadow_until_low_quality() {
    let Some(mut renderer) = skip_or_renderer("lit_paint_takes_the_suns_shadow_until_low_quality")
    else {
        return;
    };
    let (w, h) = (160u32, 120u32);
    let mut scene = drop_shadow_fixture(w, h, false);
    scene.sun = Some(test_sun(Vec3::new(1.0, -1.0, 0.0), Vec3::splat(3.0), 0.002));
    scene.meshes.push(cube(
        Vec3::new(-1.0, 1.5, 0.0),
        Vec3::ONE,
        [0.8, 0.8, 0.8, 1.0],
    ));
    let paint = lit_decal(crate::scene::DecalBlend::Alpha, [0.7, 0.7, 0.7]);
    let (shadowed, lit) = (Vec3::new(0.5, 0.0, 0.0), Vec3::new(-3.5, 0.0, 0.0));
    let (sx, sy) = pixel_of(&scene, shadowed, w, h);
    let (lx, ly) = pixel_of(&scene, lit, w, h);
    let (_, full) = decal_pair(&renderer, &scene, (w, h), vec![paint.clone()]);
    assert!(
        luma(&full, sx, sy) * 10 < luma(&full, lx, ly) * 8,
        "the paint in the cube's shadow is dark: {} vs {}",
        luma(&full, sx, sy),
        luma(&full, lx, ly),
    );
    renderer.set_quality(Quality::LOW, QualityParams::LOW);
    let (_, low) = decal_pair(&renderer, &scene, (w, h), vec![paint]);
    assert!(
        luma(&low, sx, sy).abs_diff(luma(&low, lx, ly)) <= 6,
        "without the shadow the two spots shade alike: {} vs {}",
        luma(&low, sx, sy),
        luma(&low, lx, ly),
    );
}

/// **A lamp lights the paint.**
#[test]
fn needs_gpu_a_point_light_brightens_lit_paint() {
    let Some(renderer) = skip_or_renderer("a_point_light_brightens_lit_paint") else {
        return;
    };
    let (w, h) = (160u32, 120u32);
    let dim = drop_shadow_fixture(w, h, false);
    let mut lamp = dim.clone();
    lamp.lights
        .push(point_light(Vec3::new(0.0, 1.5, 0.0), 20.0, 8.0));
    let paint = lit_decal(crate::scene::DecalBlend::Alpha, [0.7, 0.7, 0.7]);
    let (_, without) = decal_pair(&renderer, &dim, (w, h), vec![paint.clone()]);
    let (_, with) = decal_pair(&renderer, &lamp, (w, h), vec![paint]);
    let (x, y) = pixel_of(&dim, Vec3::ZERO, w, h);
    assert!(
        luma(&with, x, y) > luma(&without, x, y) + 30,
        "the lamp brightens the paint: {} -> {}",
        luma(&without, x, y),
        luma(&with, x, y),
    );
}

/// **The normal map.** A flat map leaves sunlit paint where it was; a constant tilted map moves it.
#[test]
fn needs_gpu_a_normal_map_tilts_lit_paint_and_a_flat_one_does_not() {
    let Some(renderer) = skip_or_renderer("a_normal_map_tilts_lit_paint_and_a_flat_one_does_not")
    else {
        return;
    };
    let (w, h) = (160u32, 120u32);
    let scene = lit_decal_fixture(w, h);
    let plain = lit_decal(crate::scene::DecalBlend::Alpha, [0.7, 0.7, 0.7]);
    let mut flat = plain.clone();
    flat.maps.normal = Some(one_texel_map([128, 128, 255, 255], 0xF1A7));
    let mut tilted = plain.clone();
    tilted.maps.normal = Some(one_texel_map([220, 128, 200, 255], 0x7117));
    let (_, base) = decal_pair(&renderer, &scene, (w, h), vec![plain]);
    let (_, level) = decal_pair(&renderer, &scene, (w, h), vec![flat]);
    let (_, tilt) = decal_pair(&renderer, &scene, (w, h), vec![tilted]);
    let (x, y) = pixel_of(&scene, Vec3::ZERO, w, h);
    assert_pixels_within(
        pixel(&level, x, y),
        pixel(&base, x, y),
        1,
        "a flat normal map",
    );
    assert!(
        luma(&tilt, x, y).abs_diff(luma(&base, x, y)) > 10,
        "a tilted map changes the shading: {} vs {}",
        luma(&base, x, y),
        luma(&tilt, x, y),
    );
}

/// **The coat.** With no light a white coat is exactly the bare frame; a glossy coat at the sun's
/// mirror angle is brighter than the bare floor; a grey coat away from the highlight darkens it.
#[test]
fn needs_gpu_a_coat_adds_a_lobe_and_tints() {
    let Some(renderer) = skip_or_renderer("a_coat_adds_a_lobe_and_tints") else {
        return;
    };
    let (w, h) = (160u32, 120u32);
    let mut dark = drop_shadow_fixture(w, h, false);
    dark.ambient = [0.0; 3];
    let mut white = lit_decal(crate::scene::DecalBlend::Coat, [1.0, 1.0, 1.0]);
    white.roughness = Some(0.5);
    let (bare, coated) = decal_pair(&renderer, &dark, (w, h), vec![white]);
    assert_eq!(
        bare.rgba, coated.rgba,
        "a white coat with no light is invisible"
    );

    // A 45 degree view whose mirror direction the sun travels along.
    let mut scene = drop_shadow_fixture(w, h, false);
    let position = Vec3::new(0.0, 6.0, 6.0);
    scene.camera.view = Mat4::look_at_rh(position, Vec3::ZERO, Vec3::Y);
    scene.camera.position = position;
    scene.sun = Some(test_sun(Vec3::new(0.0, -1.0, 1.0), Vec3::splat(3.0), 0.002));
    let mut glossy = lit_decal(crate::scene::DecalBlend::Coat, [1.0, 1.0, 1.0]);
    glossy.roughness = Some(0.1);
    let mut grey = lit_decal(crate::scene::DecalBlend::Coat, [0.5, 0.5, 0.5]);
    grey.roughness = Some(1.0);
    let (bare, shiny) = decal_pair(&renderer, &scene, (w, h), vec![glossy]);
    let (_, tinted) = decal_pair(&renderer, &scene, (w, h), vec![grey]);
    let (x, y) = pixel_of(&scene, Vec3::ZERO, w, h);
    assert!(
        luma(&shiny, x, y) > luma(&bare, x, y) + 20,
        "the lobe brightens the highlight: {} -> {}",
        luma(&bare, x, y),
        luma(&shiny, x, y),
    );
    let (ax, ay) = pixel_of(&scene, Vec3::new(3.0, 0.0, 3.0), w, h);
    assert!(
        luma(&tinted, ax, ay) + 40 < luma(&bare, ax, ay),
        "a grey coat darkens away from the highlight: {} -> {}",
        luma(&bare, ax, ay),
        luma(&tinted, ax, ay),
    );
}

/// **A painted mirror is a mirror mesh.** An opaque alpha decal of roughness 0.1 and metallic 1 over
/// a rough floor reflects the sky like a floor authored with that material, with the environment
/// alone and with SSR on: the reflections read the painted normal, roughness and F0.
#[test]
fn needs_gpu_a_painted_mirror_equals_a_mirror_mesh() {
    let Some(renderer) = skip_or_renderer("a_painted_mirror_equals_a_mirror_mesh") else {
        return;
    };
    let (w, h) = (160u32, 120u32);
    let paint = [0.8, 0.8, 0.8];
    for ssr in [0.0, 1.0] {
        let mut scene = lit_decal_fixture(w, h);
        scene.environment = Some(vivid_env(1.0));
        scene.post = Some(PostData {
            ssr,
            ..exact_post()
        });
        let mut decal = lit_decal(crate::scene::DecalBlend::Alpha, paint);
        decal.roughness = Some(0.1);
        decal.metallic = 1.0;
        let (bare, painted) = decal_pair(&renderer, &scene, (w, h), vec![decal]);
        let mut authored = scene.clone();
        authored.meshes[0].color = [paint[0], paint[1], paint[2], 1.0];
        authored.meshes[0].material = [0.1, 1.0, 0.0];
        let reference = renderer
            .render(&authored, FrameSize::native(w, h))
            .expect("an authored render");
        let (x, y) = pixel_of(&scene, Vec3::ZERO, w, h);
        assert!(
            luma(&painted, x, y).abs_diff(luma(&bare, x, y)) > 8,
            "ssr {ssr}: the paint changes the floor: {:?} vs {:?}",
            pixel(&bare, x, y),
            pixel(&painted, x, y),
        );
        assert_pixels_within(
            pixel(&painted, x, y),
            pixel(&reference, x, y),
            3,
            &format!("ssr {ssr}: the painted mirror at the box centre"),
        );
    }
}

/// **Overlap is exact per instance.** A rough decal followed by an opaque glossy one in the same
/// batch renders what the glossy one alone does: the second decal's base and finish both replace
/// the first's colour, normal, roughness and F0.
#[test]
fn needs_gpu_an_opaque_lit_decal_hides_the_one_under_it_exactly() {
    let Some(renderer) = skip_or_renderer("an_opaque_lit_decal_hides_the_one_under_it_exactly")
    else {
        return;
    };
    let (w, h) = (160u32, 120u32);
    let mut scene = lit_decal_fixture(w, h);
    scene.environment = Some(vivid_env(1.0));
    let mut under = lit_decal(crate::scene::DecalBlend::Alpha, [0.9, 0.2, 0.2]);
    under.roughness = Some(1.0);
    let mut over = lit_decal(crate::scene::DecalBlend::Alpha, [0.2, 0.2, 0.9]);
    over.roughness = Some(0.05);
    over.metallic = 0.5;
    let (_, alone) = decal_pair(&renderer, &scene, (w, h), vec![over.clone()]);
    let (_, stacked) = decal_pair(&renderer, &scene, (w, h), vec![under.clone(), over]);
    let (_, under_alone) = decal_pair(&renderer, &scene, (w, h), vec![under]);
    assert_ne!(alone.rgba, under_alone.rgba, "the fixture tells them apart");
    assert_eq!(
        alone.rgba, stacked.rgba,
        "the opaque decal replaces everything under it"
    );
}

/// **A coat over the sky writes nothing.** A coat box reaching past the floor's edge leaves the sky
/// pixels inside it, and the floor beside the edge, exactly as the bare frame has them: a zero
/// normal must not leak NaN into the painted surface the reflections read.
#[test]
fn needs_gpu_a_coat_over_the_sky_leaves_the_frame_alone() {
    let Some(renderer) = skip_or_renderer("a_coat_over_the_sky_leaves_the_frame_alone") else {
        return;
    };
    let (w, h) = (160u32, 120u32);
    let mut scene = drop_shadow_fixture(w, h, false);
    scene.environment = Some(vivid_env(1.0));
    scene.post = Some(PostData {
        ssr: 1.0,
        ..exact_post()
    });
    // The floor ends at z = -3; the box reaches to z = -8 over the sky.
    scene.meshes[0].model = Mat4::from_translation(Vec3::new(0.0, 0.0, 7.0))
        * Mat4::from_scale(Vec3::new(20.0, 1.0, 20.0));
    let mut coat = lit_decal(crate::scene::DecalBlend::Coat, [1.0, 1.0, 1.0]);
    coat.box_to_world = Mat4::from_translation(Vec3::new(0.0, 0.0, -4.0))
        * Mat4::from_scale(Vec3::new(8.0, 4.0, 10.0));
    coat.world_to_box = coat.box_to_world.inverse();
    coat.roughness = Some(1.0);
    let (bare, coated) = decal_pair(&renderer, &scene, (w, h), vec![coat]);
    // Sky inside the box: above the floor edge, at the box's depth.
    let (sx, sy) = pixel_of(&scene, Vec3::new(0.0, 1.0, -6.0), w, h);
    assert_eq!(pixel(&coated, sx, sy), pixel(&bare, sx, sy), "sky pixel");
    for x in 0..w {
        for y in 0..h {
            let (a, b) = (pixel(&coated, x, y), pixel(&bare, x, y));
            if y.abs_diff(sy) < 40 {
                assert!(
                    a[0].abs_diff(b[0]) < 40,
                    "no artefact at ({x}, {y}): {a:?} vs {b:?}"
                );
            }
        }
    }
}

/// **A coat reflects the sky.** A white coat at roughness 0.05 on a rough dark floor under a bright
/// sky brightens the floor; at roughness 1 the gain is smaller.
#[test]
fn needs_gpu_a_coat_reflects_the_sky() {
    let Some(renderer) = skip_or_renderer("a_coat_reflects_the_sky") else {
        return;
    };
    let (w, h) = (160u32, 120u32);
    let mut scene = drop_shadow_fixture(w, h, false);
    scene.environment = Some(vivid_env(1.0));
    scene.meshes[0].color = [0.02, 0.02, 0.02, 1.0];
    let coat = |roughness: f32| {
        let mut coat = lit_decal(crate::scene::DecalBlend::Coat, [1.0, 1.0, 1.0]);
        coat.roughness = Some(roughness);
        coat
    };
    let (bare, glossy) = decal_pair(&renderer, &scene, (w, h), vec![coat(0.05)]);
    let (_, matte) = decal_pair(&renderer, &scene, (w, h), vec![coat(1.0)]);
    let (x, y) = pixel_of(&scene, Vec3::new(0.0, 0.0, 2.0), w, h);
    let green = |image: &RenderedImage| i32::from(pixel(image, x, y)[1]);
    let (base, shiny, dull) = (green(&bare), green(&glossy), green(&matte));
    assert!(
        shiny > base + 5,
        "the glossy coat mirrors the sky: {base} -> {shiny}"
    );
    assert!(
        dull - base < shiny - base,
        "a rough coat gains less: {base} -> {dull} vs {shiny}"
    );
}

/// **A coat shows SSR.** Beside an emissive box, the puddle pixels toward it are brighter with a
/// glossy coat than with a rough one: the trace reads the coat's roughness.
#[test]
fn needs_gpu_a_coat_shows_screen_space_reflections() {
    let Some(renderer) = skip_or_renderer("a_coat_shows_screen_space_reflections") else {
        return;
    };
    let (w, h) = (160u32, 120u32);
    let mut scene = drop_shadow_fixture(w, h, false);
    scene.environment = Some(test_env());
    scene.post = Some(PostData {
        ssr: 1.0,
        ..exact_post()
    });
    scene.meshes[0].color = [0.05, 0.05, 0.05, 1.0];
    let mut lamp = cube(Vec3::new(0.0, 0.5, -1.0), Vec3::ONE, [1.0, 0.3, 0.3, 1.0]);
    lamp.material = [0.5, 0.0, 3.0];
    scene.meshes.push(lamp);
    let coat = |roughness: f32| {
        let mut coat = lit_decal(crate::scene::DecalBlend::Coat, [1.0, 1.0, 1.0]);
        coat.roughness = Some(roughness);
        coat.receivers = crate::scene::DecalReceivers::Only(vec![0]);
        coat
    };
    let (_, glossy) = decal_pair(&renderer, &scene, (w, h), vec![coat(0.05)]);
    let (_, matte) = decal_pair(&renderer, &scene, (w, h), vec![coat(1.0)]);
    let (x, y) = pixel_of(&scene, Vec3::new(0.0, 0.0, 0.8), w, h);
    let red = |image: &RenderedImage| i32::from(pixel(image, x, y)[0]);
    assert!(
        red(&glossy) > red(&matte) + 8,
        "the glossy puddle shows the lamp: {} vs {}",
        red(&matte),
        red(&glossy),
    );
}

/// **Lit decals draw before multiply decals**, whatever their order in the list: a multiply
/// scorch listed first still darkens the paint listed after it.
#[test]
fn needs_gpu_a_multiply_decal_darkens_alpha_paint_listed_after_it() {
    let Some(renderer) = skip_or_renderer("a_multiply_decal_darkens_alpha_paint_listed_after_it")
    else {
        return;
    };
    let (w, h) = (160u32, 120u32);
    let scene = lit_decal_fixture(w, h);
    let scorch = lit_decal(crate::scene::DecalBlend::Multiply, [0.0, 0.0, 0.0]);
    let paint = lit_decal(crate::scene::DecalBlend::Alpha, [0.9, 0.9, 0.9]);
    let (_, painted) = decal_pair(&renderer, &scene, (w, h), vec![paint.clone()]);
    let (_, only) = decal_pair(&renderer, &scene, (w, h), vec![scorch.clone()]);
    let (_, both) = decal_pair(&renderer, &scene, (w, h), vec![scorch, paint]);
    let (x, y) = pixel_of(&scene, Vec3::ZERO, w, h);
    assert!(luma(&painted, x, y) > 3 * luma(&only, x, y));
    assert!(
        luma(&both, x, y).abs_diff(luma(&only, x, y)) <= 3,
        "the scorch covers the paint: {} vs the scorch alone {}",
        luma(&both, x, y),
        luma(&only, x, y),
    );
}

/// **Receivers**: alpha paint excluding the plate leaves it clean and paints the floor.
#[test]
fn needs_gpu_alpha_paint_skips_an_excluded_mesh() {
    let Some(renderer) = skip_or_renderer("alpha_paint_skips_an_excluded_mesh") else {
        return;
    };
    let (w, h) = (160u32, 120u32);
    let mut scene = drop_shadow_fixture(w, h, true);
    scene.sun = Some(test_sun(Vec3::new(0.3, -1.0, 0.2), Vec3::splat(2.0), 0.002));
    let mut paint = lit_decal(crate::scene::DecalBlend::Alpha, [0.9, 0.1, 0.1]);
    paint.receivers = crate::scene::DecalReceivers::Exclude(vec![1]);
    let (bare, painted) = decal_pair(&renderer, &scene, (w, h), vec![paint]);
    let (px, py) = pixel_of(&scene, Vec3::new(1.0, 0.5, 0.0), w, h);
    // Bloom from the painted floor reaches the plate by a level; a painted plate would be red.
    assert_pixels_within(
        pixel(&painted, px, py),
        pixel(&bare, px, py),
        2,
        "the plate is clean",
    );
    let (fx, fy) = pixel_of(&scene, Vec3::new(-1.0, 0.0, 0.5), w, h);
    assert_ne!(
        pixel(&painted, fx, fy),
        pixel(&bare, fx, fy),
        "the floor is painted"
    );
}

/// **A decal at every quality**: it is content, not an effect.
#[test]
fn needs_gpu_a_decal_survives_the_lowest_quality() {
    let Some(mut renderer) = skip_or_renderer("a_decal_survives_the_lowest_quality") else {
        return;
    };
    renderer.set_quality(Quality::LOW, QualityParams::LOW);
    let (w, h) = (160u32, 120u32);
    let scene = drop_shadow_fixture(w, h, true);
    let mut paint = decal_instance(crate::scene::DecalBlend::Multiply, Vec3::new(3.0, 1.0, 3.0));
    paint.receivers = crate::scene::DecalReceivers::Exclude(vec![1]);
    let (bare, painted) = decal_pair(&renderer, &scene, (w, h), vec![paint]);
    let (x, y) = pixel_of(&scene, Vec3::new(-1.0, 0.0, 0.5), w, h);
    assert!(luma(&painted, x, y) * 2 < luma(&bare, x, y));
}

/// **A sunlit slope with nothing above it is fully lit (`Plan/audits/2026-10-03-lighting.md` § O2).**
/// The sun is ~75° off the ground's normal, the grazing case that stripes a surface with acne when
/// the bias is short of the map's texel. The bias is the authored 0.05 world units, converted to
/// light-NDC depth as `extract` does, over vigil's extent (a coarse texel) and a tight one, two
/// depth ranges (the conversion must make them agree) and both map sizes.
#[test]
fn needs_gpu_a_grazing_sunlit_slope_does_not_shadow_itself() {
    let Some(mut renderer) = skip_or_renderer("a_grazing_sunlit_slope_does_not_shadow_itself")
    else {
        return;
    };
    let (w, h) = (160u32, 90u32);
    let size = FrameSize::native(w, h);
    let shadowless = Quality {
        shadows: false,
        ..Quality::FULL
    };
    let grazing = Vec3::new(0.966, -0.259, 0.0);
    for (map, params) in [(2048, QualityParams::FULL), (1024, QualityParams::MEDIUM)] {
        for extent in [10.0_f32, 96.0] {
            for far in [20.0_f32, 200.0] {
                let mut scene = RenderScene::empty((w, h));
                let position = Vec3::new(0.0, 8.0, 6.0);
                scene.camera = crate::scene::RenderCamera {
                    view: Mat4::look_at_rh(position, Vec3::ZERO, Vec3::Y),
                    projection: Mat4::perspective_rh(
                        60.0_f32.to_radians(),
                        crate::scene::aspect_ratio((w, h)),
                        0.1,
                        1000.0,
                    ),
                    position,
                };
                scene.sun = Some(test_sun_framed(
                    grazing,
                    Vec3::ONE,
                    0.05 / (far - 1.0),
                    (extent, 1.0, far),
                ));
                scene.meshes.push(ground(200.0));

                renderer.set_quality(Quality::FULL, params);
                let shadowed = renderer.render(&scene, size).expect("a shadowed render");
                renderer.set_quality(shadowless, params);
                let unshadowed = renderer.render(&scene, size).expect("a shadowless render");
                let (with, without) =
                    (column_luma(&shadowed, 0, w), column_luma(&unshadowed, 0, w));
                assert!(
                    with >= 0.98 * without,
                    "map {map}, extent {extent}, far {far}: a caster-free slope must stay lit, luma {with:.1} vs {without:.1} unshadowed"
                );
            }
        }
    }
}

/// **Masking shadows removes the shadow pass and leaves the sun lighting the scene.** The two
/// meanings `PostChain` used to conflate — "this scene has a sun" and "this frame draws a shadow
/// map" — come apart here, and the second is the only one a machine's quality reaches.
///
/// Three frames pin it. The masked frame must differ from the shadowed one (the shadow really
/// left), must be *brighter* than it (a removed shadow adds light — and this is what fails loudly
/// if the scene pass ever samples the 1×1 fallback depth instead of taking `sun_shadow`'s
/// fully-lit early-out, because that reads as fully shadowed), and must be brighter still than the
/// same scene with no sun at all (the sun kept lighting it, which is the half that makes this a
/// quality change rather than a look change).
#[test]
fn needs_gpu_masking_shadows_removes_the_pass_and_leaves_the_sun_lit() {
    let Some(mut renderer) =
        skip_or_renderer("masking_shadows_removes_the_pass_and_leaves_the_sun_lit")
    else {
        return;
    };
    let (w, h) = (96u32, 96u32);
    let size = FrameSize::native(w, h);
    let scene = shadow_fixture(w, h);
    let shadowless = Quality {
        shadows: false,
        ..Quality::FULL
    };

    // The structural half: the pass and its map leave the graph, and nothing else moves.
    let authored = PostChain::for_scene(&scene);
    let masked = authored.masked(&shadowless, &QualityParams::FULL);
    assert_eq!(
        authored.shadow_map,
        Some(QualityParams::FULL.shadow_map_size)
    );
    assert_eq!(masked.shadow_map, None);
    let pass_names = |chain: PostChain| {
        let (graph, _) = build_frame_graph(size, chain);
        graph
            .passes()
            .iter()
            .map(|declared| declared.name.clone())
            .collect::<Vec<_>>()
    };
    let shadowed_passes = pass_names(authored);
    let masked_passes = pass_names(masked);
    assert!(shadowed_passes.iter().any(|name| name == pass::SHADOW));
    assert_eq!(
        masked_passes,
        shadowed_passes
            .iter()
            .filter(|name| *name != pass::SHADOW)
            .cloned()
            .collect::<Vec<_>>(),
        "masking shadows must remove the shadow pass and leave every other pass in place",
    );

    // The rendered half.
    let shadowed = renderer.render(&scene, size).expect("a shadowed render");
    renderer.set_quality(shadowless, QualityParams::FULL);
    let unshadowed = renderer.render(&scene, size).expect("a shadowless render");
    renderer.set_quality(Quality::FULL, QualityParams::FULL);
    let mut sunless_scene = shadow_fixture(w, h);
    sunless_scene.sun = None;
    let sunless = renderer
        .render(&sunless_scene, size)
        .expect("a sunless render");

    assert_ne!(
        unshadowed.rgba, shadowed.rgba,
        "the shadow must actually leave the frame",
    );
    let (lit, cast, dark) = (
        column_luma(&unshadowed, 0, w),
        column_luma(&shadowed, 0, w),
        column_luma(&sunless, 0, w),
    );
    assert!(
        lit > cast,
        "removing the shadow adds light: shadowless {lit} vs shadowed {cast}",
    );
    assert!(
        lit > dark,
        "the sun still lights the scene: shadowless {lit} vs sunless {dark}",
    );
    assert_ne!(
        unshadowed.rgba, sunless.rgba,
        "masking shadows must not turn the sun off",
    );
}

/// **The sun's two parameter axes each move a shadowed frame, and neither reaches a frame with no
/// shadow map.** A smaller map quantizes the caster's depth differently; a single-tap kernel stops
/// averaging the neighbourhood. Both are counted in the same pass, so the control — the same scene
/// with no sun — is what proves each one is read where it is supposed to be and nowhere else.
#[test]
fn needs_gpu_a_lowered_shadow_map_and_pcf_each_move_a_shadowed_frame() {
    let Some(mut renderer) =
        skip_or_renderer("a_lowered_shadow_map_and_pcf_each_move_a_shadowed_frame")
    else {
        return;
    };
    let (w, h) = (96u32, 96u32);
    let size = FrameSize::native(w, h);
    let scene = shadow_fixture(w, h);
    let mut sunless = shadow_fixture(w, h);
    sunless.sun = None;

    renderer.set_quality(Quality::FULL, QualityParams::FULL);
    let shadowed = renderer.render(&scene, size).expect("a shadowed render");
    let control = renderer.render(&sunless, size).expect("a sunless render");

    for (axis, params) in [
        (
            "shadow_map_size",
            QualityParams {
                shadow_map_size: 512,
                ..QualityParams::FULL
            },
        ),
        (
            "pcf",
            QualityParams {
                pcf: 1,
                ..QualityParams::FULL
            },
        ),
    ] {
        renderer.set_quality(Quality::FULL, params);
        assert_ne!(
            renderer
                .render(&scene, size)
                .expect("a cheaper render")
                .rgba,
            shadowed.rgba,
            "a lowered `{axis}` must move a frame whose sun casts a shadow",
        );
        assert_eq!(
            renderer
                .render(&sunless, size)
                .expect("a sunless render")
                .rgba,
            control.rgba,
            "and must not reach a frame that draws no shadow map (`{axis}`)",
        );
    }
}

/// **A unit albedo is the analytic fog's own arithmetic.** `1.0` means purely scattering, so the
/// ambient term `α·(1 - T)·fog_color` collapses to the `mix` the composite has always run, and the
/// shipped fog's bytes live in that intrinsic rather than in an algebraically equal sum. Two
/// readings hold it there:
///
/// - the branch boundary is **exact**: `0.999` already renders differently from `1.0`, so the
///   neutral path is a genuine skip and not a value that happens to round back;
/// - `fog_anisotropy` never reaches the ambient term — an isotropic in-scatter has no phase — so
///   moving `g` across a shaft-free medium must not move a byte.
///
/// Losing less light to absorption is brighter, so the albedo is monotone up to that boundary.
#[test]
fn needs_gpu_a_unit_albedo_is_the_analytic_fogs_own_arithmetic() {
    let Some(renderer) = skip_or_renderer("a_unit_albedo_is_the_analytic_fogs_own_arithmetic")
    else {
        return;
    };
    let (w, h) = (96u32, 96u32);
    let size = FrameSize::native(w, h);
    let mut scene = shafts_scene(w, h);
    let haze = |albedo: f32, anisotropy: f32| PostData {
        fog_density: 0.05,
        fog_albedo: albedo,
        fog_anisotropy: anisotropy,
        ..PostData::default()
    };
    let frame = |scene: &mut RenderScene, post: PostData| {
        scene.post = Some(post);
        renderer.render(scene, size).expect("a hazed render")
    };

    let unit = frame(&mut scene, haze(1.0, 0.7));
    let mut previous = column_luma(&unit, 0, w);
    for albedo in [0.999_f32, 0.99, 0.95, 0.9] {
        let dimmer = frame(&mut scene, haze(albedo, 0.7));
        assert_ne!(
            dimmer.rgba, unit.rgba,
            "an albedo of {albedo} must not render as the neutral `1.0`",
        );
        let mean = column_luma(&dimmer, 0, w);
        assert!(
            mean < previous,
            "absorbing more must darken the haze: {previous:.3} -> {mean:.3} at {albedo}",
        );
        previous = mean;
    }

    // The phase belongs to the directional term alone, and this medium has none.
    let isotropic = frame(&mut scene, haze(1.0, 0.0));
    assert_eq!(
        unit.rgba, isotropic.rgba,
        "`fog_anisotropy` must not reach the ambient in-scatter",
    );
}

/// **Beam reach tracks the medium's extinction.** The march stops at `3 / σ_t`, so a thick enough
/// medium cannot reach the lit volume at all: the slot's sheet sits four-odd world units off the
/// camera, and at `σ_t = 0.9` the reach is 3.33, inside which no ray has crossed into it yet. The
/// beam then contributes **exactly nothing** while the very same `shafts` at `σ_t = 0.2` (reach 15)
/// lights the band by ~193 luma.
///
/// Each reading is the difference between that medium with and without its directional term, which
/// is what separates the beams from the extinction they now share a density with: raising σ_t
/// darkens the frame on its own, and a bare "the band is darker" reading would credit that to the
/// march.
#[test]
fn needs_gpu_beam_reach_tracks_the_mediums_extinction() {
    let Some(renderer) = skip_or_renderer("beam_reach_tracks_the_mediums_extinction") else {
        return;
    };
    let (w, h) = (96u32, 96u32);
    let size = FrameSize::native(w, h);
    let mut scene = shafts_scene(w, h);
    let beam_term = |scene: &mut RenderScene, density: f32| {
        scene.post = Some(medium_post(density, 0.0));
        let off = renderer.render(scene, size).expect("the medium, no beams");
        scene.post = Some(medium_post(density, 2.0));
        let on = renderer
            .render(scene, size)
            .expect("the medium and its beams");
        column_luma(&on, 0, w / 2) - column_luma(&off, 0, w / 2)
    };

    let reaching = beam_term(&mut scene, 0.2);
    let too_thick = beam_term(&mut scene, 0.9);
    assert!(
        reaching > 100.0,
        "the fixture must carry a beam worth measuring: {reaching:.2}",
    );
    assert_eq!(
        too_thick, 0.0,
        "a reach of 3.33 units cannot see the slot's sheet: {too_thick:.2}",
    );
}

/// **The fog dims its own beams** — the coupling the two-density split could not express. The same
/// `shafts` in a thicker medium reads darker down-ray, because the beams' light is extinguished on
/// its way back through the medium that scattered it: a ray's lit stretch starts past the slot's
/// sheet, so what survives is `α·(exp(-σ_t·t₀) - exp(-σ_t·end))` and thickening σ_t cuts into it.
///
/// Measured over `0.2 → 0.3 → 0.4`, the beam band falls ~193 → ~166 → ~115 luma and the centre
/// band, whose rays enter the sheet furthest out, falls ~218 → ~149 → ~45.
#[test]
fn needs_gpu_a_thicker_medium_dims_its_own_beams() {
    let Some(renderer) = skip_or_renderer("a_thicker_medium_dims_its_own_beams") else {
        return;
    };
    let (w, h) = (96u32, 96u32);
    let size = FrameSize::native(w, h);
    let mut scene = shafts_scene(w, h);
    let beam_terms = |scene: &mut RenderScene, density: f32| {
        scene.post = Some(medium_post(density, 0.0));
        let off = renderer.render(scene, size).expect("the medium, no beams");
        scene.post = Some(medium_post(density, 2.0));
        let on = renderer
            .render(scene, size)
            .expect("the medium and its beams");
        (
            column_luma(&on, 0, w / 2) - column_luma(&off, 0, w / 2),
            column_luma(&on, w / 2 - 4, w / 2 + 4) - column_luma(&off, w / 2 - 4, w / 2 + 4),
        )
    };

    let mut thinner = beam_terms(&mut scene, 0.2);
    for density in [0.3_f32, 0.4] {
        let thicker = beam_terms(&mut scene, density);
        assert!(
            thicker.0 < thinner.0 - 10.0,
            "a thicker medium must dim the beam band: {:.2} -> {:.2} at {density}",
            thinner.0,
            thicker.0,
        );
        assert!(
            thicker.1 < thinner.1 - 10.0,
            "and dim the centre band harder still: {:.2} -> {:.2} at {density}",
            thinner.1,
            thicker.1,
        );
        thinner = thicker;
    }
}

/// **A low-albedo medium absorbs instead of glowing.** α splits the one extinction into scattering
/// and absorption, so at the same thickness the medium goes from bright haze to dark smoke without
/// the surfaces behind it coming back — the knob the two-effect model had nowhere to put.
///
/// Both terms are read, because α scales both. Against a grey ambient at `σ_t = 0.35`, a purely
/// scattering medium lifts the frame to ~490 luma (the unhazed scene reads ~138) while `α = 0.02`
/// takes it to ~88 — *below* the medium-free frame, which is what "absorbing" means. Against a
/// black ambient the beams alone are left, and their term falls ~193 → ~123 → ~56 → exactly 0 as α
/// runs 1 → 0.5 → 0.2 → 0.
#[test]
fn needs_gpu_a_low_albedo_medium_absorbs_instead_of_glowing() {
    let Some(renderer) = skip_or_renderer("a_low_albedo_medium_absorbs_instead_of_glowing") else {
        return;
    };
    let (w, h) = (96u32, 96u32);
    let size = FrameSize::native(w, h);
    let mut scene = shafts_scene(w, h);

    let bare = column_luma(&renderer.render(&scene, size).expect("no medium"), 0, w);
    let hazed = |scene: &mut RenderScene, albedo: f32| {
        scene.post = Some(PostData {
            fog_density: 0.35,
            fog_albedo: albedo,
            ..PostData::default()
        });
        column_luma(&renderer.render(scene, size).expect("a hazed render"), 0, w)
    };
    let scattering = hazed(&mut scene, 1.0);
    let absorbing = hazed(&mut scene, 0.02);
    assert!(
        scattering > bare + 100.0,
        "a scattering medium must read as bright haze: {bare:.2} -> {scattering:.2}",
    );
    assert!(
        absorbing < bare - 20.0,
        "an absorbing one must read darker than no medium at all: {bare:.2} -> {absorbing:.2}",
    );

    // The directional term carries the same α, so the beams fade with the haze rather than
    // surviving it — and at zero the medium scatters nothing at all.
    let beam_term = |scene: &mut RenderScene, albedo: f32| {
        let mut post = medium_post(0.2, 0.0);
        post.fog_albedo = albedo;
        scene.post = Some(post);
        let off = renderer.render(scene, size).expect("the medium, no beams");
        let mut post = medium_post(0.2, 2.0);
        post.fog_albedo = albedo;
        scene.post = Some(post);
        let on = renderer
            .render(scene, size)
            .expect("the medium and its beams");
        column_luma(&on, 0, w / 2) - column_luma(&off, 0, w / 2)
    };
    let mut brighter = beam_term(&mut scene, 1.0);
    for albedo in [0.5_f32, 0.2] {
        let dimmer = beam_term(&mut scene, albedo);
        assert!(
            dimmer < brighter - 10.0,
            "a lower albedo must dim the beams: {brighter:.2} -> {dimmer:.2} at {albedo}",
        );
        brighter = dimmer;
    }
    assert_eq!(
        beam_term(&mut scene, 0.0),
        0.0,
        "a purely absorbing medium scatters no sun light at all",
    );
}

/// **`fog_anisotropy` moves the beam/haze balance.** `g` decides how the sun's in-scattered light
/// is spread over angle, so it decides whether the directional term reads as a beam or as fill.
///
/// The reading is the *shape* of the term, not its size: the centre band's rays point most nearly
/// into the sun, while the far-left band's simply cross more lit volume. At `g = 0` the medium
/// scatters equally in every direction, so the far-left band wins on path length alone and the
/// centre/left ratio sits below 1 (~0.88) — uniform fill. Raising `g` concentrates the light along
/// the sun's own travel and the ratio climbs past 1 (~1.09 at 0.3, ~1.73 at 0.7, ~2.87 at 0.9):
/// the term has become a beam pointed at the light.
#[test]
fn needs_gpu_fog_anisotropy_moves_the_beam_haze_balance() {
    let Some(renderer) = skip_or_renderer("fog_anisotropy_moves_the_beam_haze_balance") else {
        return;
    };
    let (w, h) = (96u32, 96u32);
    let size = FrameSize::native(w, h);
    let mut scene = shafts_scene(w, h);
    let forward_bias = |scene: &mut RenderScene, anisotropy: f32| {
        let mut post = medium_post(0.2, 0.0);
        post.fog_anisotropy = anisotropy;
        scene.post = Some(post);
        let off = renderer.render(scene, size).expect("the medium, no beams");
        let mut post = medium_post(0.2, 2.0);
        post.fog_anisotropy = anisotropy;
        scene.post = Some(post);
        let on = renderer
            .render(scene, size)
            .expect("the medium and its beams");
        let edge = column_luma(&on, 0, 8) - column_luma(&off, 0, 8);
        let centre =
            column_luma(&on, w / 2 - 4, w / 2 + 4) - column_luma(&off, w / 2 - 4, w / 2 + 4);
        centre / edge.max(1.0)
    };

    let isotropic = forward_bias(&mut scene, 0.0);
    assert!(
        isotropic < 1.0,
        "isotropic scattering must read as fill, the longer path winning: {isotropic:.2}",
    );
    let mut previous = isotropic;
    for anisotropy in [0.3_f32, 0.7, 0.9] {
        let biased = forward_bias(&mut scene, anisotropy);
        assert!(
            biased > previous + 0.1,
            "a stronger forward lobe must concentrate the term: {previous:.2} -> {biased:.2} \
             at {anisotropy}",
        );
        previous = biased;
    }
    assert!(
        previous > 2.0,
        "and at 0.9 the term must be a beam, not a wash: {previous:.2}",
    );
}

// --- Height-coherent beams (Plan/volumetric-medium.md) ---------------------------------

/// The mean luma of a full-width row band `y_from..y_to` — [`column_luma`]'s height-axis twin, for
/// the fixtures that read the medium's *vertical* gradient rather than its across-frame shape.
fn row_luma(img: &RenderedImage, y_from: u32, y_to: u32) -> f64 {
    let mut sum = 0u64;
    for y in y_from..y_to {
        for x in 0..img.width {
            sum += u64::from(luma(img, x, y));
        }
    }
    sum as f64 / f64::from((y_to - y_from) * img.width)
}

/// A medium of the shafts fixture's thickness, shaped by height about `fog_height` and carrying
/// `intensity` on its directional term.
fn shaped_medium(falloff: f32, height: f32, intensity: f32) -> PostData {
    PostData {
        fog_height: height,
        fog_height_falloff: falloff,
        ..medium_post(0.2, intensity)
    }
}

/// **A height-free medium ignores `fog_height`, in the marched term as much as in the haze.**
/// `fog_height` only means anything through `fog_height_falloff`, so at a zero falloff the shared
/// optical depth is exactly `σ_t·t` and the local density ratio exactly `1.0`. That collapse is
/// the formula's own limit rather than a branch, and multiplying by an exact `1.0` preserves every
/// bit, so the claim has to hold on bytes and not merely on appearance.
///
/// **What the fixture switches off to stay non-vacuous: nothing.** The scene renders a real beam
/// over a real medium throughout (`σ_t = 0.2`, `shafts = 2.0`, a sun and the slot's shadow). The
/// guard is the second half: the same two heights under a *nonzero* falloff must move the **beam
/// term** — the on-minus-off difference, which cancels the haze the falloff also moves — so the
/// byte equality above is the height half being absent rather than `fog_height` going unread by
/// the march.
#[test]
fn needs_gpu_a_height_free_medium_ignores_fog_height_in_both_terms() {
    let Some(renderer) = skip_or_renderer("a_height_free_medium_ignores_fog_height_in_both_terms")
    else {
        return;
    };
    let (w, h) = (96u32, 96u32);
    let size = FrameSize::native(w, h);
    let mut scene = shafts_scene(w, h);
    let frame = |scene: &mut RenderScene, post: PostData| {
        scene.post = Some(post);
        renderer.render(scene, size).expect("a medium render")
    };

    let at_ground = frame(&mut scene, shaped_medium(0.0, 0.0, 2.0));
    let overhead = frame(&mut scene, shaped_medium(0.0, 8.0, 2.0));
    assert_eq!(
        at_ground.rgba, overhead.rgba,
        "with no falloff the medium has no height, so `fog_height` must not move a byte",
    );

    let beam_term = |scene: &mut RenderScene, height: f32| {
        let off = frame(scene, shaped_medium(0.15, height, 0.0));
        let on = frame(scene, shaped_medium(0.15, height, 2.0));
        column_luma(&on, 0, w / 2) - column_luma(&off, 0, w / 2)
    };
    let low = beam_term(&mut scene, 0.0);
    let high = beam_term(&mut scene, 8.0);
    assert!(
        (low - high).abs() > 10.0,
        "under a falloff the march must read `fog_height` too: {low:.2} vs {high:.2}",
    );
}

/// A black emissive box: it contributes depth and a shadow-map occluder and **no radiance**, so a
/// fixture built out of these measures the medium and nothing else.
fn dark_box(center: Vec3, size: Vec3) -> MeshInstance {
    MeshInstance {
        medium: 1.0,
        shape: MeshShape::Cube,
        model: Mat4::from_translation(center) * Mat4::from_scale(size),
        normal_matrix: Mat3::IDENTITY,
        color: [0.0, 0.0, 0.0, 1.0],
        material: [1.0, 0.0, 1.0],
        use_vertex_material: false,
        fade: 1.0,
        fade_height_min: -f32::MAX,
        flash: 0.0,
        flash_color: [1.0; 3],
        wrap: 0.0,
        scatter_color: [1.0; 3],
        bone_offset: 0,
        cast_shadows: true,
        viewmodel: false,
        rim: [0.0; 4],
    }
}

/// The height-gradient fixture: a sun straight overhead through a **gap in a ceiling**, which
/// makes the lit volume a vertical sheet spanning every height, and a camera at `y = 6` looking
/// horizontally through it at a back wall.
///
/// That shape is the whole design, and it is the [`shafts_scene`] offset argument applied to the
/// other axis. A beam that descends or fans would confound "the medium is denser low down" with
/// "there is more lit volume low down"; a vertical sheet has the same lit thickness at every
/// height, so a row band's reading is the medium's own density profile. Every ray also ends on the
/// same back wall, so the march's `end` varies only by the secant of the ray's pitch rather than
/// by what each ray happens to see.
///
/// The wall and the ceiling are [`dark_box`]es and the ambient is black, so the frame carries no
/// radiance but the marched term. The medium is thin on purpose — `σ_t = 0.05` over the wall's
/// eighteen-odd units is an optical depth near 0.9, where in-scatter still tracks local density
/// instead of saturating against its own extinction.
fn height_beam_scene(w: u32, h: u32) -> RenderScene {
    let mut scene = RenderScene::empty((w, h));
    let position = Vec3::new(0.0, 6.0, 10.0);
    scene.camera = crate::scene::RenderCamera {
        view: Mat4::look_at_rh(position, Vec3::new(0.0, 6.0, -6.0), Vec3::Y),
        projection: Mat4::perspective_rh(
            60.0_f32.to_radians(),
            crate::scene::aspect_ratio((w, h)),
            0.1,
            1000.0,
        ),
        position,
    };
    scene.ambient = [0.0, 0.0, 0.0];
    // Straight down, so the gap's light sheet is vertical and the same at every height.
    scene.sun = Some(test_sun(
        Vec3::new(0.0, -1.0, 0.0),
        Vec3::new(1.0, 1.0, 1.0),
        0.002,
    ));
    scene.meshes = vec![
        // The back wall every ray ends on.
        dark_box(Vec3::new(0.0, 0.0, -6.0), Vec3::new(80.0, 36.0, 1.0)),
        // The ceiling, with a four-unit gap at z ∈ [-2, 2].
        dark_box(Vec3::new(0.0, 16.0, -11.0), Vec3::new(40.0, 1.0, 18.0)),
        dark_box(Vec3::new(0.0, 16.0, 11.0), Vec3::new(40.0, 1.0, 18.0)),
    ];
    scene
}

/// **A height medium thickens the beams toward the ground.** The haze already did this; the march
/// integrating the same optical depth and the same local density is what makes the beams do it —
/// the point.
///
/// The reading is the marched term's *vertical shape* on [`height_beam_scene`]: the bottom third
/// of the frame over the top third, each as an on-minus-off difference so the haze — which the
/// falloff moves harder than anything else — cancels. `fog_height` sits at the camera's own eye
/// level, so the bottom band looks through medium the falloff thickens and the top band through
/// medium it thins.
///
/// **Switched off to stay non-vacuous:** three things, and the first is the off-set proper.
/// (1) The falloff-0 reading is taken by the same probes on the same fixture, and the fixture is
/// built symmetric about eye level, so it lands at ~1 and a march blind to the medium's height
/// would report ~1 again at every falloff. (2) `fog_anisotropy` is 0, because the phase function
/// alone separates up-rays from down-rays by several times and would dominate the reading — the
/// phase is gated by its own fixture. (3) The ambient is black and the geometry emissive-black, so
/// no surface radiance rides along.
#[test]
fn needs_gpu_a_height_medium_thickens_the_beams_toward_the_ground() {
    let Some(renderer) = skip_or_renderer("a_height_medium_thickens_the_beams_toward_the_ground")
    else {
        return;
    };
    let (w, h) = (128u32, 128u32);
    let size = FrameSize::native(w, h);
    let mut scene = height_beam_scene(w, h);
    let eye_level = scene.camera.position.y;
    let thin = |falloff: f32, intensity: f32| PostData {
        fog_height: eye_level,
        fog_height_falloff: falloff,
        fog_anisotropy: 0.0,
        ..medium_post(0.05, intensity)
    };
    let low_over_high = |scene: &mut RenderScene, falloff: f32| {
        scene.post = Some(thin(falloff, 0.0));
        let off = renderer.render(scene, size).expect("the medium, no beams");
        scene.post = Some(thin(falloff, 40.0));
        let on = renderer
            .render(scene, size)
            .expect("the medium and its beams");
        let lower = row_luma(&on, h - h / 3, h) - row_luma(&off, h - h / 3, h);
        let upper = row_luma(&on, 0, h / 3) - row_luma(&off, 0, h / 3);
        lower / upper.max(1.0)
    };

    let flat = low_over_high(&mut scene, 0.0);
    assert!(
        (flat - 1.0).abs() < 0.15,
        "a height-free medium must read the same above and below eye level: {flat:.3}",
    );
    let mut previous = flat;
    for falloff in [0.1_f32, 0.2, 0.3, 0.4] {
        let graded = low_over_high(&mut scene, falloff);
        assert!(
            graded > previous + 0.1,
            "a steeper medium must push the beams further toward the ground: \
             {previous:.3} -> {graded:.3} at {falloff}",
        );
        previous = graded;
    }
    assert!(
        previous > 1.6,
        "and at 0.4 the ground half must clearly dominate: {previous:.3}",
    );
}

/// The medium-agreement fixture: a black emissive plate under a camera at `y = 8` tilted down at
/// it, and a sun travelling down and **away** from the lens so every marched point between the two
/// is lit.
///
/// Everything that is not the medium is switched off, which is what lets a rendered byte be read
/// back as a physical quantity: the plate is emissive black (no shading, no albedo), the ambient
/// is black, there is no occluder so `V ≡ 1`, and the caller pins `tonemap: None`, `exposure: 1.0`,
/// `bloom_strength: 0.0` and `dither: false` so the composite's output is the linear HDR value
/// itself, sRGB-encoded by the target and nothing more.
fn agreement_scene(w: u32, h: u32) -> RenderScene {
    let mut scene = RenderScene::empty((w, h));
    let position = Vec3::new(0.0, 8.0, 0.0);
    scene.camera = crate::scene::RenderCamera {
        view: Mat4::look_at_rh(position, Vec3::new(0.0, 0.0, -20.0), Vec3::Y),
        projection: Mat4::perspective_rh(
            60.0_f32.to_radians(),
            crate::scene::aspect_ratio((w, h)),
            0.1,
            1000.0,
        ),
        position,
    };
    scene.ambient = [0.0, 0.0, 0.0];
    // Down and away from the camera, so the plate never shadows the volume above it.
    scene.sun = Some(test_sun(
        Vec3::new(0.0, -0.5, -1.0),
        Vec3::new(1.0, 1.0, 1.0),
        0.002,
    ));
    // Top face at y = 0, wide and deep enough that the centre of the frame lands well inside it.
    scene.meshes = vec![slab(Vec3::new(0.0, -30.0, -25.0), 60.0, [0.0, 0.0, 0.0])];
    scene
}

/// The sRGB transfer function's inverse, so an output byte reads back as the linear value the
/// composite produced (the target is `Rgba8UnormSrgb`, so the hardware encoded it on store).
fn linear_from_srgb8(byte: u8) -> f64 {
    let c = f64::from(byte) / 255.0;
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

/// The mean decoded-linear red of a square patch centred on the frame — the frames below are
/// achromatic, and averaging a patch keeps one texel of the shaft target's half-resolution
/// footprint from deciding the reading.
fn centre_linear(img: &RenderedImage) -> f64 {
    let (cx, cy) = (img.width / 2, img.height / 2);
    let mut sum = 0.0;
    for y in (cy - 4)..(cy + 4) {
        for x in (cx - 4)..(cx + 4) {
            sum += linear_from_srgb8(rgb_at(img, x, y)[0]);
        }
    }
    sum / 64.0
}

/// **The two terms agree on one medium.** The ambient term's `1 - T(d)` and the directional
/// term's own integral are the same physical quantity measured two ways, and they
/// are computed from the same optical depth — so they must match to the march's discretisation
/// error, under a height medium as much as a flat one.
///
/// The reading is arranged so both come out of the composite as a bare linear number:
///
/// - **the haze frame** authors `fog_color` white over the black plate, so `mix(0, 1, 1 - T)`
///   leaves exactly the extinguished fraction in the output;
/// - **the beam frame** authors `fog_color` black and `shafts = 4π` over an isotropic phase
///   (`fog_anisotropy: 0`, whose `p ≡ 1/4π` the intensity cancels), so the output is
///   `σ_s·Σ T(t)·ratio(t)·dt` — a 32-step quadrature of `∫₀ᵈ σ_t(t)·T(t) dt`, which is `1 - T(d)`.
///
/// The tolerance is **0.02 in linear output**, on readings around 0.6, and it is set by the
/// measurement rather than the integral: the Bayer offsets plus the 4×4 blur make the quadrature
/// effectively midpoint, whose error over a total optical depth near 1 at 32 steps is under a
/// tenth of a percent, while the march resolves the plate's distance at *half* the composite's
/// extent and the output is quantized to 8-bit sRGB. It is well inside the effect being gated: a
/// march reading a plain `exp(-σ_t·t)` reports 0.659 where the haze says 0.587 in the height case
/// below, a gap of 0.07.
#[test]
fn needs_gpu_the_marched_transmittance_matches_the_analytic_haze() {
    let Some(renderer) = skip_or_renderer("the_marched_transmittance_matches_the_analytic_haze")
    else {
        return;
    };
    let (w, h) = (128u32, 128u32);
    let size = FrameSize::native(w, h);
    let mut scene = agreement_scene(w, h);
    let bare = PostData {
        bloom_strength: 0.0,
        exposure: 1.0,
        tonemap: TonemapOperator::None,
        dither: false,
        fog_density: 0.05,
        fog_anisotropy: 0.0,
        ..PostData::default()
    };

    for (label, falloff) in [("flat", 0.0_f32), ("height", 0.05)] {
        scene.post = Some(PostData {
            fog_color: [1.0, 1.0, 1.0],
            fog_height_falloff: falloff,
            ..bare
        });
        let haze = centre_linear(&renderer.render(&scene, size).expect("the haze frame"));
        scene.post = Some(PostData {
            fog_color: [0.0, 0.0, 0.0],
            fog_height_falloff: falloff,
            shafts: 4.0 * std::f32::consts::PI,
            ..bare
        });
        let march = centre_linear(&renderer.render(&scene, size).expect("the beam frame"));

        assert!(
            haze > 0.3 && haze < 0.9,
            "the {label} medium must extinguish a measurable share: {haze:.4}",
        );
        assert!(
            (march - haze).abs() < 0.02,
            "the {label} medium's two terms must integrate the same optical depth: \
             haze {haze:.4} vs march {march:.4}",
        );
    }
}

/// The sky-haze fixture's gradient, **opposed in red and blue**: red at the horizon, blue at the
/// zenith, black below. That opposition is what lets a reading be `red − blue` — the wall is flat,
/// so a corner ray carries a little more medium than the centre ray, and a thickness difference
/// moves both channels *together* while the sky's direction dependence moves them apart.
fn sky_haze_env() -> crate::scene::EnvironmentData {
    crate::scene::EnvironmentData {
        sky_zenith: [0.0, 0.0, 0.6],
        sky_horizon: [0.6, 0.0, 0.0],
        sky_ground: [0.0, 0.0, 0.0],
        sun_disc_size: 0.0,
        sun_disc_intensity: 0.0,
        ambient: [0.0, 0.0, 0.0],
        ibl_intensity: 0.0,
    }
}

/// The sky-haze fixture: a camera at the origin looking down −Z at one emissive-black wall that
/// **fills the frame**, under [`sky_haze_env`].
///
/// Everything that is not the medium is switched off, and each of those is load-bearing rather
/// than tidy. The wall covers every pixel, so the sky is never *drawn* and the only route a sky
/// colour has into the image is the haze. It is emissive black, so the pre-fog value is exactly
/// zero and a rendered byte reads back as the in-scattered radiance alone. `ibl_intensity: 0.0`
/// multiplies the prefiltered environment by an exact zero, so the sky lights nothing either — and
/// that is what lets a test change the gradient, or drop the `Environment` outright, and attribute
/// every moved byte to the sky term.
///
/// There is no sun, so the directional term does not exist here at all: this fixture gates the
/// ambient one.
fn sky_haze_scene(w: u32, h: u32) -> RenderScene {
    let mut scene = RenderScene::empty((w, h));
    scene.camera = origin_camera(w, h);
    scene.ambient = [0.0, 0.0, 0.0];
    scene.environment = Some(sky_haze_env());
    // Far enough for a real optical depth, wide enough that even a corner ray lands on it.
    scene.meshes = vec![dark_box(
        Vec3::new(0.0, 0.0, -40.0),
        Vec3::new(200.0, 200.0, 1.0),
    )];
    scene
}

/// The sky-haze fixture's post: a real medium under a composite that does nothing else, so the
/// output is the linear radiance sRGB-encoded and nothing more. `fog_color` is achromatic, which
/// puts the slab's own `red − blue` at exactly zero and makes any reading the sky's.
fn sky_haze_post(fog_sky: f32) -> PostData {
    PostData {
        bloom_strength: 0.0,
        exposure: 1.0,
        tonemap: TonemapOperator::None,
        dither: false,
        fog_density: 0.05,
        fog_color: [0.3, 0.3, 0.3],
        fog_sky,
        ..PostData::default()
    }
}

/// Mean decoded-linear `red − blue` over a band of rows — the sky-haze fixture's reading.
fn band_red_minus_blue(img: &RenderedImage, y_from: u32, y_to: u32) -> f64 {
    let mut sum = 0.0;
    for y in y_from..y_to {
        for x in 0..img.width {
            let rgb = rgb_at(img, x, y);
            sum += linear_from_srgb8(rgb[0]) - linear_from_srgb8(rgb[2]);
        }
    }
    sum / f64::from((y_to - y_from) * img.width)
}

/// **At `fog_sky: 0` the `Environment`'s colours reach no pixel.** The medium's ambient radiance is
/// the authored slab exactly as it was before the sky term existed, so the whole image is byte-
/// identical across a gradient swap — and the composite's three new uniform lanes cost nothing.
///
/// **Switched off to stay non-vacuous:** the two frames differ in the *only* input the sky term
/// reads, and the fixture removes every other route that input has into the image (no sky
/// pixel is visible, `ibl_intensity` is zero, the wall is emissive black). The guard is the same
/// pair at `fog_sky: 1`, which must differ — without it the equality would also hold for a shader
/// that read nothing at all.
///
/// The composite is deliberately *working* — exposure, AgX, a grade and the dither all on — so the
/// claim is byte-identity through the shipped chain rather than through a bare passthrough.
#[test]
fn needs_gpu_a_zero_sky_blend_never_reads_the_environments_colours() {
    let Some(renderer) = skip_or_renderer("a_zero_sky_blend_never_reads_the_environments_colours")
    else {
        return;
    };
    let (w, h) = (96u32, 96u32);
    let size = FrameSize::native(w, h);
    let mut scene = sky_haze_scene(w, h);
    let swapped = crate::scene::EnvironmentData {
        sky_zenith: sky_haze_env().sky_horizon,
        sky_horizon: sky_haze_env().sky_zenith,
        ..sky_haze_env()
    };
    let render = |scene: &mut RenderScene, env, fog_sky| {
        scene.environment = Some(env);
        scene.post = Some(PostData {
            exposure: 1.3,
            tonemap: TonemapOperator::Agx,
            contrast: 1.15,
            dither: true,
            ..sky_haze_post(fog_sky)
        });
        renderer.render(scene, size).expect("a sky-haze frame")
    };

    let flat = render(&mut scene, sky_haze_env(), 0.0);
    let flat_swapped = render(&mut scene, swapped, 0.0);
    assert_eq!(
        flat.rgba, flat_swapped.rgba,
        "a zero sky blend must leave the ambient in-scatter the authored slab, whatever the \
         Environment's gradient says",
    );

    let coupled = render(&mut scene, sky_haze_env(), 1.0);
    let coupled_swapped = render(&mut scene, swapped, 1.0);
    assert_ne!(
        coupled.rgba, coupled_swapped.rgba,
        "the gradient swap must be visible at all, or the equality above proves nothing",
    );
}

/// **No `Environment` is no sky term, at any `fog_sky`.** There is no sky to in-scatter, so the
/// recorder zeroes the blend into the uniform — one bind group and one pipeline, the shadow pass's
/// zeroed-uniform precedent — and a scene that authors the field with no sky renders exactly like
/// one that omits it.
///
/// **Switched off to stay non-vacuous:** the guard at the end. The fixture's wall hides the sky and
/// its `ibl_intensity` is zero, so adding an `Environment` changes the image through *this term
/// only* — and it must, or the three equalities above it would hold for a field nothing reads.
#[test]
fn needs_gpu_without_an_environment_the_sky_blend_does_not_exist() {
    let Some(renderer) = skip_or_renderer("without_an_environment_the_sky_blend_does_not_exist")
    else {
        return;
    };
    let (w, h) = (96u32, 96u32);
    let size = FrameSize::native(w, h);
    let mut scene = sky_haze_scene(w, h);
    scene.environment = None;

    scene.post = Some(sky_haze_post(0.0));
    let unauthored = renderer.render(&scene, size).expect("the skyless frame");
    for fog_sky in [0.5_f32, 1.0] {
        scene.post = Some(sky_haze_post(fog_sky));
        let authored = renderer
            .render(&scene, size)
            .expect("the skyless frame, sky coupling authored");
        assert_eq!(
            unauthored.rgba, authored.rgba,
            "fog_sky {fog_sky} must be inert with no Environment to in-scatter",
        );
    }

    scene.environment = Some(sky_haze_env());
    scene.post = Some(sky_haze_post(1.0));
    let with_sky = renderer
        .render(&scene, size)
        .expect("the same frame under a sky");
    assert_ne!(
        unauthored.rgba, with_sky.rgba,
        "an Environment must make the sky term real, or the inertness above is vacuous",
    );
}

/// **The haze takes the sky's direction dependence.** Toward the zenith it in-scatters the zenith
/// colour, in the horizon band the horizon's — which is the whole point of the term, and the
/// reason a distant surface fades into the sky instead of into a slab.
///
/// The reading is `red − blue` over two row bands of [`sky_haze_scene`], whose gradient opposes
/// those two channels. The medium's own thickness varies slightly across the frame and moves both
/// channels together, so the difference cancels it.
///
/// **Switched off to stay non-vacuous:** the `fog_sky: 0` half is the off-set, taken by the same
/// probes on the same fixture. `fog_color` is achromatic there, so both bands must read a flat
/// zero — a haze blind to direction reads zero at `1.0` too. The surface is emissive black and the
/// ambient is zero, so no surface radiance rides along, and there is no sun, so the directional
/// term does not exist.
#[test]
fn needs_gpu_the_haze_takes_the_skys_direction_dependence() {
    let Some(renderer) = skip_or_renderer("the_haze_takes_the_skys_direction_dependence") else {
        return;
    };
    let (w, h) = (128u32, 128u32);
    let size = FrameSize::native(w, h);
    let mut scene = sky_haze_scene(w, h);
    let bands = |scene: &mut RenderScene, fog_sky: f32| {
        scene.post = Some(sky_haze_post(fog_sky));
        let img = renderer.render(scene, size).expect("a sky-haze frame");
        (
            band_red_minus_blue(&img, 0, h / 8),
            band_red_minus_blue(&img, h / 2 - h / 16, h / 2 + h / 16),
        )
    };

    let (flat_up, flat_level) = bands(&mut scene, 0.0);
    assert!(
        flat_up.abs() < 0.01 && flat_level.abs() < 0.01,
        "an authored slab has no direction: up {flat_up:.4}, level {flat_level:.4}",
    );

    let (sky_up, sky_level) = bands(&mut scene, 1.0);
    assert!(
        sky_up < -0.08,
        "haze toward the zenith must take the sky's blue: {sky_up:.4}",
    );
    assert!(
        sky_level > 0.08,
        "haze in the horizon band must take its red: {sky_level:.4}",
    );
}

/// **The blend is monotone between the authored slab and the sky**, and affine in `fog_sky` — the
/// ambient radiance is one `mix`, so a half blend lands halfway and an author gets a dial rather
/// than a switch.
///
/// **Switched off to stay non-vacuous:** the `0.0` endpoint is the off-set, and it is the *same*
/// reading the direction fixture pins at zero, so the ladder starts from a measured slab rather
/// than an assumed one. Composite chain, surface and ambient are all as
/// [`the_haze_takes_the_skys_direction_dependence`] leaves them.
#[test]
fn needs_gpu_the_sky_blend_is_monotone_from_the_slab_to_the_sky() {
    let Some(renderer) = skip_or_renderer("the_sky_blend_is_monotone_from_the_slab_to_the_sky")
    else {
        return;
    };
    let (w, h) = (128u32, 128u32);
    let size = FrameSize::native(w, h);
    let mut scene = sky_haze_scene(w, h);
    let level = |scene: &mut RenderScene, fog_sky: f32| {
        scene.post = Some(sky_haze_post(fog_sky));
        let img = renderer.render(scene, size).expect("a sky-haze frame");
        band_red_minus_blue(&img, h / 2 - h / 16, h / 2 + h / 16)
    };

    let slab = level(&mut scene, 0.0);
    let half = level(&mut scene, 0.5);
    let sky = level(&mut scene, 1.0);
    assert!(
        slab < half && half < sky,
        "the blend must run monotonically from the slab to the sky: \
         {slab:.4} -> {half:.4} -> {sky:.4}",
    );
    assert!(
        (half - 0.5 * (slab + sky)).abs() < 0.01,
        "and affinely, because the radiance is one mix: {half:.4} vs {:.4}",
        0.5 * (slab + sky),
    );
}

/// **`fog_start` gates the march as well as the haze.** The medium does not exist inside it, so a
/// start distance past the geometry leaves the directional term at **exactly** zero — the density
/// ratio is 0 at every step, and the composite adds a blurred field of zeros.
///
/// This is a deliberate amendment: `fog_start` was a fog-only knob, and the two terms
/// cannot agree on one medium while one of them starts at the lens and the other does not.
///
/// The ladder skips the small start distances, and the reason is a real property rather than a
/// convenience: `fog_start` is **not** monotone on the beam. Clearing the medium near the lens
/// also clears the extinction that was dimming everything past it, so a start distance short of
/// the lit volume *brightens* the term (measured: 193 → 226 luma at `fog_start: 6`). Only once the
/// start eats into the lit volume itself does the term fall.
///
/// **Switched off to stay non-vacuous:** every reading is the on-minus-off difference at the *same*
/// `fog_start`, so the haze — which the start distance also removes — cancels and what is left is
/// the marched term alone. The `fog_start: 0` reading is the guard that the probe sees a beam at
/// all before the later ones take it away.
#[test]
fn needs_gpu_fog_start_removes_the_beams_along_with_the_haze() {
    let Some(renderer) = skip_or_renderer("fog_start_removes_the_beams_along_with_the_haze") else {
        return;
    };
    let (w, h) = (96u32, 96u32);
    let size = FrameSize::native(w, h);
    let mut scene = shafts_scene(w, h);
    let beam_term = |scene: &mut RenderScene, start: f32| {
        let started = |intensity: f32| PostData {
            fog_start: start,
            ..medium_post(0.2, intensity)
        };
        scene.post = Some(started(0.0));
        let off = renderer.render(scene, size).expect("the medium, no beams");
        scene.post = Some(started(2.0));
        let on = renderer
            .render(scene, size)
            .expect("the medium and its beams");
        column_luma(&on, 0, w / 2) - column_luma(&off, 0, w / 2)
    };

    let from_the_lens = beam_term(&mut scene, 0.0);
    assert!(
        from_the_lens > 100.0,
        "the fixture must carry a beam worth removing: {from_the_lens:.2}",
    );
    let partway = beam_term(&mut scene, 12.0);
    assert!(
        partway < from_the_lens - 10.0,
        "a start distance over most of the lit volume must eat into the beam: \
         {from_the_lens:.2} -> {partway:.2}",
    );
    // Past the march's own `3 / σ_t` reach of fifteen units, so no step is inside the medium.
    assert_eq!(
        beam_term(&mut scene, 40.0),
        0.0,
        "a start distance past the whole march leaves no in-scatter at all",
    );
}

// --- Depth of field (Plan/post-process-pipeline.md) ------------------------------------

/// One patch of the depth-of-field fixture: a 4x4 grid of emissive white cubes with one-cube gaps,
/// i.e. a square wave in both screen axes, at `center` with cube size `unit`.
///
/// Emissive and against an unlit background, so the measurement is the *filter* and not a lighting
/// term that a blur would move for its own reasons.
fn dof_patch(scene: &mut RenderScene, center: Vec3, unit: f32) {
    for cell in 0..16 {
        let (i, j) = ((cell % 4) as f32, (cell / 4) as f32);
        let mut c = cube(
            Vec3::new(
                center.x + (i - 1.5) * 2.0 * unit,
                center.y + (j - 1.5) * 2.0 * unit,
                center.z,
            ),
            Vec3::splat(unit),
            [1.0, 1.0, 1.0, 1.0],
        );
        c.material = [1.0, 0.0, 1.0];
        scene.meshes.push(c);
    }
}

/// The depth-of-field fixture: three identical square-wave patches at three distances — one
/// **nearer** than the focal plane, one **on** it, one four times **past** it — each scaled in
/// proportion to its distance so all three project to the same size and the same spatial frequency,
/// and each offset sideways so no two overlap on screen.
///
/// That is the whole design. One patch cannot separate "the effect blurs" from "the effect blurs
/// everything", and two cannot separate a far-field-only bug from a working near field. Three
/// probes of one pattern at one screen size make the comparison a pure question about the circle of
/// confusion, which is what the test has to prove.
///
/// [`DOF_FOCUS`] is the focal distance, matched to the middle patch.
fn dof_scene(w: u32, h: u32) -> RenderScene {
    let mut scene = RenderScene::empty((w, h));
    let position = Vec3::ZERO;
    scene.camera = crate::scene::RenderCamera {
        view: Mat4::look_at_rh(position, Vec3::new(0.0, 0.0, -1.0), Vec3::Y),
        projection: Mat4::perspective_rh(
            60.0_f32.to_radians(),
            crate::scene::aspect_ratio((w, h)),
            0.1,
            1000.0,
        ),
        position,
    };
    scene.ambient = [0.0, 0.0, 0.0];
    // Sizes and offsets are `distance / DOF_FOCUS` times the focal patch's, so every patch is the
    // same number of pixels wide and sits the same fraction of the frame off centre.
    dof_patch(&mut scene, DOF_NEAR, 0.25 * DOF_NEAR.z / -DOF_FOCUS);
    dof_patch(&mut scene, DOF_SHARP, 0.25);
    dof_patch(&mut scene, DOF_FAR, 0.25 * DOF_FAR.z / -DOF_FOCUS);
    scene
}

/// The focal plane the fixture is built around, and the three patch centres: 0.4x, 1x and 4x that
/// distance, at −0.6, 0 and +0.6 of the frame's half-width.
///
/// The sideways separation is load-bearing, not cosmetic: a defocused patch spreads by its own
/// circle of confusion, and at the aperture these tests use that is 0.12 of the half-width. Parked
/// any closer, each neighbour's bokeh would reach into the focal patch's probe window and the
/// "stays sharp" clause would fail for a reason that has nothing to do with the estimator.
const DOF_FOCUS: f32 = 10.0;
const DOF_NEAR: Vec3 = Vec3::new(-1.385, 0.0, -4.0);
const DOF_SHARP: Vec3 = Vec3::new(0.0, 0.0, -10.0);
const DOF_FAR: Vec3 = Vec3::new(13.85, 0.0, -40.0);

fn dof_post(dof: f32) -> PostData {
    PostData {
        dof,
        focus_distance: DOF_FOCUS,
        ..PostData::default()
    }
}

/// How much of a square-wave patch's contrast survives: the standard deviation of luma over a
/// window around `at`.
///
/// A blur is exactly what collapses that number — it carries the bright cells' energy out into the
/// gaps — and it is blind to a uniform brightness change, so it measures the filter and not the
/// exposure. Preferred over a mean edge gradient, which at these frame sizes reads the sub-pixel
/// phase of a near-Nyquist pattern as much as it reads sharpness.
fn patch_contrast(img: &RenderedImage, at: (u32, u32), radius: u32) -> f64 {
    let (x0, x1) = (
        at.0.saturating_sub(radius),
        (at.0 + radius).min(img.width - 1),
    );
    let (y0, y1) = (
        at.1.saturating_sub(radius),
        (at.1 + radius).min(img.height - 1),
    );
    let mut samples = Vec::new();
    for y in y0..=y1 {
        for x in x0..=x1 {
            samples.push(f64::from(luma(img, x, y)));
        }
    }
    let mean = samples.iter().sum::<f64>() / samples.len() as f64;
    (samples.iter().map(|v| (v - mean).powi(2)).sum::<f64>() / samples.len() as f64).sqrt()
}

/// **The central assertion.** Both defocused patches lose their square wave and the one on
/// the focal plane keeps it. A "blur everything" bug fails the third clause; a far-field-only bug
/// fails the second; an effect that does nothing fails the first two.
#[test]
fn needs_gpu_dof_blurs_both_defocused_patches_and_leaves_the_focal_plane_sharp() {
    let Some(renderer) =
        skip_or_renderer("dof_blurs_both_defocused_patches_and_leaves_the_focal_plane_sharp")
    else {
        return;
    };
    let (w, h) = (384u32, 384u32);
    let size = FrameSize::native(w, h);
    let mut scene = dof_scene(w, h);
    let window = w / 16;
    let at = |world: Vec3| project(&scene.camera, world, w, h);
    let (near, sharp, far) = (at(DOF_NEAR), at(DOF_SHARP), at(DOF_FAR));

    let off = renderer.render(&scene, size).expect("dof off");
    scene.post = Some(dof_post(0.06));
    let on = renderer.render(&scene, size).expect("dof on");

    let probe = |img: &RenderedImage, p| patch_contrast(img, p, window);
    for (name, p) in [("near", near), ("far", far)] {
        let (before, after) = (probe(&off, p), probe(&on, p));
        assert!(
            before > 40.0,
            "the {name} patch must carry a real square wave before it can lose one: {before:.2}",
        );
        assert!(
            after < before * 0.5,
            "the {name} patch is defocused and must lose at least half its edge energy: \
             {before:.2} -> {after:.2}",
        );
    }
    let (sharp_off, sharp_on) = (probe(&off, sharp), probe(&on, sharp));
    assert!(
        sharp_on > sharp_off * 0.9,
        "the patch on the focal plane must stay sharp: {sharp_off:.2} -> {sharp_on:.2}",
    );
}

/// **Byte identity, on bytes, for the depth-of-field-off case.** A scene whose `dof` is zero has every
/// distance in focus, so neither pass is built and the frame must be the frozen one — the
/// composite's second optional input is a 1×1 stand-in and its uniform flag is zero.
///
/// The guard against passing vacuously is the third render: the same scene with a real aperture
/// does move pixels, so the equality above is a skip and not an effect that never worked.
#[test]
fn needs_gpu_dof_at_zero_is_byte_identical_to_no_post_process() {
    let Some(renderer) = skip_or_renderer("dof_at_zero_is_byte_identical_to_no_post_process")
    else {
        return;
    };
    let (w, h) = (96u32, 96u32);
    let size = FrameSize::native(w, h);
    let mut scene = dof_scene(w, h);

    let absent = renderer.render(&scene, size).expect("no PostProcess");
    scene.post = Some(dof_post(0.0));
    let in_focus = renderer.render(&scene, size).expect("dof at zero");
    assert_eq!(
        absent.rgba, in_focus.rgba,
        "an aperture of zero must not move a single byte",
    );

    scene.post = Some(dof_post(0.06));
    assert_ne!(
        renderer.render(&scene, size).expect("dof on").rgba,
        in_focus.rgba,
        "a real aperture on the same scene must render differently — else nothing was proved",
    );
}

/// Scale invariance: both depth-of-field targets are a fraction of the *output* extent and the
/// circle of confusion is a fraction of the frame's height, so how much the far patch loses is the
/// same number at scale 1 and scale 2. The comparison is the on/off ratio at each scale, because
/// supersampling legitimately changes the absolute sharpness (a quality change) but must never
/// change the look.
#[test]
fn needs_gpu_dof_blur_is_the_same_at_any_render_scale() {
    let Some(renderer) = skip_or_renderer("dof_blur_is_the_same_at_any_render_scale") else {
        return;
    };
    let (w, h) = (192u32, 192u32);
    let mut scene = dof_scene(w, h);
    let far = project(&scene.camera, DOF_FAR, w, h);
    let window = w / 16;

    let ratio = |size: FrameSize, scene: &mut RenderScene| {
        scene.post = None;
        let off = renderer.render(scene, size).expect("dof off");
        scene.post = Some(dof_post(0.06));
        let on = renderer.render(scene, size).expect("dof on");
        patch_contrast(&on, far, window) / patch_contrast(&off, far, window).max(1e-6)
    };

    let native = ratio(FrameSize::native(w, h), &mut scene);
    let doubled = ratio(
        FrameSize::scaled(w, h, crate::RenderScale::new(2.0).unwrap()),
        &mut scene,
    );
    assert!(
        native < 0.5,
        "the fixture must blur enough for the ratio to mean something: {native}",
    );
    assert!(
        (native - doubled).abs() < 0.10,
        "a scale change is a quality change, never a look change: {native} vs {doubled}",
    );
}

// --- Film emulation (Plan/post-process-pipeline.md) -----------------------------------

/// A flat emissive box: shading-free, so its rendered colour is exactly `color` and any spread
/// between two of its pixels is what the composite did and nothing else.
fn film_box(center: Vec3, size: Vec3, color: [f32; 3]) -> MeshInstance {
    let mut m = cube(center, size, [color[0], color[1], color[2], 1.0]);
    m.material = [1.0, 0.0, 1.0];
    m
}

// --- Radiance ceiling and non-finite texels (Plan/audits/2026-10-03-lighting.md § O5) ---------

/// The sRGB encoding of a linear value, quantised as the `Rgba8UnormSrgb` target does.
fn srgb8_round(linear: f32) -> i32 {
    srgb8_from_linear(linear.clamp(0.0, 1.0)).round() as i32
}

/// **A mirror highlight is clamped below half-float overflow.** A metal ground at the roughness
/// floor, lit straight down the axis, shades to `1 / α²` per unit sun (≈ 244 000 here), past
/// `Rgba16Float`'s 65 504. The exposure is chosen so the radiance ceiling lands mid-range
/// (`HDR_CEILING × exposure ≈ 0.46`): a store that rounds the overflow to infinity reads 255, one
/// that saturates at 65 504 reads brighter than the ceiling, and only a shader-side clamp reads
/// the ceiling's own encoding.
#[test]
fn needs_gpu_a_mirror_highlight_is_clamped_below_half_float_overflow() {
    let Some(mut renderer) =
        skip_or_renderer("a_mirror_highlight_is_clamped_below_half_float_overflow")
    else {
        return;
    };
    let (w, h) = (33u32, 33u32);
    let size = FrameSize::native(w, h);
    let mut scene = RenderScene::empty((w, h));
    let position = Vec3::new(0.0, 4.0, 0.0);
    scene.camera = crate::scene::RenderCamera {
        view: Mat4::look_at_rh(position, Vec3::ZERO, Vec3::Z),
        projection: Mat4::perspective_rh(
            1.0_f32.to_radians(),
            crate::scene::aspect_ratio((w, h)),
            0.1,
            1000.0,
        ),
        position,
    };
    scene.ambient = [0.0; 3];
    let mut mirror = ground(50.0);
    mirror.color = [1.0; 4];
    mirror.material = [0.0, 1.0, 0.0];
    scene.meshes = vec![mirror];
    let sun_power = 4.0_f32;
    scene.sun = Some(test_sun(Vec3::NEG_Y, Vec3::splat(sun_power), 0.0));
    let exposure = 1.0 / 131_072.0;
    scene.post = Some(PostData {
        exposure,
        tonemap: TonemapOperator::None,
        bloom_strength: 0.0,
        dither: false,
        ..PostData::default()
    });
    renderer.set_quality(
        Quality {
            shadows: false,
            ..Quality::FULL
        },
        QualityParams::FULL,
    );

    // The oracle, independent of the shader: at n·h = n·v = n·l = 1 the sun term is
    // `π·D·V·F·n·l·sun = (1 / (4 α²)) · sun` with α = roughness², roughness clamped to 0.045.
    let alpha = 0.045_f32 * 0.045;
    let peak = sun_power / (4.0 * alpha * alpha);
    assert!(
        peak * exposure > 1.0,
        "the fixture must overflow the display range to mean anything: {}",
        peak * exposure
    );
    let ceiling = 60_000.0_f32;
    let want = srgb8_round(ceiling * exposure);

    let img = renderer.render(&scene, size).expect("a mirror frame");
    let px = rgb_at(&img, w / 2, h / 2);
    for (channel, &got) in px.iter().enumerate() {
        assert!(
            (i32::from(got) - want).abs() <= 2,
            "channel {channel}: the centre pixel {px:?} should encode the radiance ceiling \
             (~{want}); 255 means the half-float store overflowed to infinity"
        );
    }
}

/// **The reflections sum is clamped once.** The opaque pass stores up to the radiance ceiling
/// and `reflections` adds its own ceiling-bounded term with One+One, so the sum can pass
/// half-float's 65 504 and store infinity; every reader of `scene` sanitizes with `hdr_safe`,
/// which reads that infinity as the ceiling. Here the sun alone saturates the ground at the
/// ceiling and a 40 000 sky adds to it: an unsanitized read shows 255, or NaN-driven black.
#[test]
fn needs_gpu_a_reflection_over_a_bright_surface_reads_the_ceiling() {
    let Some(mut renderer) =
        skip_or_renderer("a_reflection_over_a_bright_surface_reads_the_ceiling")
    else {
        return;
    };
    let (w, h) = (33u32, 33u32);
    let size = FrameSize::native(w, h);
    let mut scene = RenderScene::empty((w, h));
    let position = Vec3::new(0.0, 4.0, 0.0);
    scene.camera = crate::scene::RenderCamera {
        view: Mat4::look_at_rh(position, Vec3::ZERO, Vec3::Z),
        projection: Mat4::perspective_rh(
            1.0_f32.to_radians(),
            crate::scene::aspect_ratio((w, h)),
            0.1,
            1000.0,
        ),
        position,
    };
    scene.ambient = [0.0; 3];
    let mut mirror = ground(50.0);
    mirror.color = [1.0; 4];
    mirror.material = [0.0, 1.0, 0.0];
    scene.meshes = vec![mirror];
    scene.sun = Some(test_sun(Vec3::NEG_Y, Vec3::splat(4.0), 0.0));
    scene.environment = Some(crate::scene::EnvironmentData {
        sky_zenith: [40_000.0; 3],
        sky_horizon: [40_000.0; 3],
        sky_ground: [40_000.0; 3],
        sun_disc_size: 0.0,
        sun_disc_intensity: 0.0,
        ambient: [0.0; 3],
        ibl_intensity: 1.0,
    });
    let exposure = 1.0 / 131_072.0;
    scene.post = Some(PostData {
        exposure,
        tonemap: TonemapOperator::None,
        bloom_strength: 0.0,
        dither: false,
        ..PostData::default()
    });
    renderer.set_quality(
        Quality {
            shadows: false,
            ..Quality::FULL
        },
        QualityParams::FULL,
    );

    let want = srgb8_round(60_000.0 * exposure);
    let img = renderer
        .render(&scene, size)
        .expect("a reflecting mirror frame");
    let px = rgb_at(&img, w / 2, h / 2);
    for (channel, &got) in px.iter().enumerate() {
        assert!(
            (i32::from(got) - want).abs() <= 2,
            "channel {channel}: the centre pixel {px:?} should encode the radiance ceiling \
             (~{want}); 255 or 0 means an infinity from the summed targets reached the composite"
        );
    }
}

/// Runs fullscreen effect `id` once over `inputs` (bindings `0..n`, then the nearest sampler,
/// then `uniform`) into a fresh `extent`-sized [`HDR_FORMAT`] target and reads it back. The
/// nearest sampler keeps every tap on one texel: a bilinear `0 × inf` is NaN on some hardware,
/// and the expectations stay exact.
fn run_effect(
    renderer: &Renderer,
    id: ShaderId,
    inputs: &[&wgpu::TextureView],
    uniform: &[u8],
    extent: (u32, u32),
) -> Vec<[f32; 4]> {
    let target = run_effect_into(renderer, id, inputs, uniform, extent, HDR_FORMAT);
    crate::gi_bake::read_probe_texture(&renderer.gpu, &target).expect("the target reads back")
}

/// [`run_effect`] into a `format` target, returned unread.
fn run_effect_into(
    renderer: &Renderer,
    id: ShaderId,
    inputs: &[&wgpu::TextureView],
    uniform: &[u8],
    (w, h): (u32, u32),
    format: wgpu::TextureFormat,
) -> wgpu::Texture {
    let device = renderer.gpu.device();
    let target = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("run_effect target"),
        size: wgpu::Extent3d {
            width: w,
            height: h,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let target_view = target.create_view(&wgpu::TextureViewDescriptor::default());
    let uniform_buf = init_buffer(device, wgpu::BufferUsages::UNIFORM, uniform);
    let mut entries: Vec<wgpu::BindGroupEntry> = inputs
        .iter()
        .enumerate()
        .map(|(binding, view)| wgpu::BindGroupEntry {
            binding: binding as u32,
            resource: wgpu::BindingResource::TextureView(view),
        })
        .collect();
    entries.push(wgpu::BindGroupEntry {
        binding: inputs.len() as u32,
        resource: wgpu::BindingResource::Sampler(&renderer.nearest_sampler),
    });
    entries.push(wgpu::BindGroupEntry {
        binding: inputs.len() as u32 + 1,
        resource: uniform_buf.as_entire_binding(),
    });
    let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("run_effect bind group"),
        layout: renderer.layouts.get(fullscreen_layout(id)),
        entries: &entries,
    });
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("run_effect"),
    });
    {
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("run_effect"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &target_view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        pass.set_pipeline(renderer.pipeline(id));
        pass.set_bind_group(0, &bind_group, &[]);
        pass.draw(0..3, 0..1);
    }
    renderer.gpu.queue().submit(Some(encoder.finish()));
    target
}

/// Above the roughness fade band the trace's confidence is 0, so a frame whose graph also builds the
/// SSR chain (and whose `reflections` reads the trace) renders the same pixels as the frame that
/// does not, every pass validates and records (no wgpu fault), at an odd extent and under
/// supersampling (the trace extent follows the output, not the render extent).
#[test]
fn needs_gpu_ssr_above_the_fade_band_leaves_the_frame_untouched() {
    let Some(renderer) = skip_or_renderer("ssr_above_the_fade_band_leaves_the_frame_untouched")
    else {
        return;
    };
    for size in [
        FrameSize::native(67, 45),
        FrameSize::scaled(64, 48, crate::RenderScale::new(2.0).unwrap()),
    ] {
        let (w, h) = size.output;
        let mut scene = RenderScene::empty(size.render);
        scene.post = Some(exact_post());
        scene.environment = Some(test_env());
        let eye = Vec3::new(0.0, 1.0, 8.0);
        scene.camera = crate::scene::RenderCamera {
            view: Mat4::look_at_rh(eye, Vec3::new(0.0, 1.0, 0.0), Vec3::Y),
            projection: Mat4::perspective_rh(
                60.0_f32.to_radians(),
                w as f32 / h as f32,
                0.1,
                100.0,
            ),
            position: eye,
        };
        let mut metal_box = cube(
            Vec3::new(0.0, -0.5, 0.0),
            Vec3::new(2.0, 1.0, 2.0),
            [0.8; 4],
        );
        metal_box.material = [0.8, 1.0, 0.0];
        scene.meshes = vec![metal_box];

        let plain = renderer.render(&scene, size).expect("plain render");
        scene.post = Some(PostData {
            ssr: 1.0,
            ..exact_post()
        });
        let chain = PostChain {
            ssr: Some(QualityParams::FULL.ssr_scale),
            ..PostChain::for_scene(&scene).masked(&renderer.quality, &renderer.quality_params)
        };
        let (graph, output) = build_frame_graph(size, chain);
        let scheduled = schedule(&graph).expect("the ssr graph schedules");
        let recorder = renderer.prepare_frame(&scene, size, None).unwrap();
        let with_ssr = renderer
            .gpu
            .execute(&scheduled, output, &recorder)
            .expect("the ssr frame renders");
        assert!(
            renderer.gpu.take_fault().is_none(),
            "{w}x{h}: an ssr pass faulted"
        );
        assert_eq!(
            plain.rgba, with_ssr.rgba,
            "{w}x{h}: the blend moved a pixel"
        );
    }
}

/// With SSR on, the mirror floor shows the cube: the floor just in front of its base is redder than
/// the sky-only frame, and floor whose ray leaves the screen is within 2/255 of it.
#[test]
fn needs_gpu_ssr_reflects_a_box_in_a_mirror_floor() {
    let Some(renderer) = skip_or_renderer("ssr_reflects_a_box_in_a_mirror_floor") else {
        return;
    };
    let size = FrameSize::native(96, 64);
    let mut scene = RenderScene::empty(size.render);
    scene.post = Some(exact_post());
    scene.environment = Some(test_env());
    let eye = Vec3::new(0.0, 4.5, 7.5);
    scene.camera = crate::scene::RenderCamera {
        view: Mat4::look_at_rh(eye, Vec3::new(0.0, 0.5, 0.0), Vec3::Y),
        projection: Mat4::perspective_rh(60.0_f32.to_radians(), 1.5, 0.1, 100.0),
        position: eye,
    };
    let mut floor = ground(50.0);
    floor.material = [0.02, 1.0, 0.0];
    let mut red_box = cube(Vec3::new(0.0, 0.5, 0.0), Vec3::ONE, [1.0, 0.0, 0.0, 1.0]);
    red_box.material = [0.5, 0.0, 1.0];
    scene.meshes = vec![floor, red_box];

    let off = renderer.render(&scene, size).expect("sky-only render");
    scene.post = Some(PostData {
        ssr: 1.0,
        ..exact_post()
    });
    let chain = PostChain {
        ssr: Some(QualityParams::FULL.ssr_scale),
        ..PostChain::for_scene(&scene).masked(&renderer.quality, &renderer.quality_params)
    };
    let (graph, output) = build_frame_graph(size, chain);
    let scheduled = schedule(&graph).expect("the ssr graph schedules");
    let recorder = renderer.prepare_frame(&scene, size, None).unwrap();
    let on = renderer
        .gpu
        .execute(&scheduled, output, &recorder)
        .expect("the ssr frame renders");
    assert!(renderer.gpu.take_fault().is_none(), "the ssr frame faulted");

    let (w, h) = size.output;
    let pixel = |image: &RenderedImage, (x, y): (u32, u32)| -> [i32; 3] {
        let at = ((y * w + x) * 4) as usize;
        std::array::from_fn(|c| i32::from(image.rgba[at + c]))
    };
    for z in [0.8, 1.0] {
        let at = project(&scene.camera, Vec3::new(0.0, 0.0, z), w, h);
        let (off, on) = (pixel(&off, at), pixel(&on, at));
        assert!(
            on[0] - on[2] > off[0] - off[2] + 8,
            "floor at z={z} {at:?} is not redder: off {off:?}, on {on:?}"
        );
    }
    for world in [
        Vec3::new(5.0, 0.0, 0.0),
        Vec3::new(-5.0, 0.0, 0.0),
        Vec3::new(4.0, 0.0, -2.0),
    ] {
        let at = project(&scene.camera, world, w, h);
        let (off, on) = (pixel(&off, at), pixel(&on, at));
        for c in 0..3 {
            assert!(
                (off[c] - on[c]).abs() <= 2,
                "floor {world} {at:?} moved: off {off:?}, on {on:?}"
            );
        }
    }
}

/// SSR's colour pyramid carries the probe and sky fill, so a metal box seen in a mirror floor keeps
/// its look: the floor's reflection of the box is at least half as bright as the box's own face.
#[test]
fn needs_gpu_ssr_reflects_a_metal_box_with_its_fill() {
    let Some(renderer) = skip_or_renderer("ssr_reflects_a_metal_box_with_its_fill") else {
        return;
    };
    let size = FrameSize::native(96, 64);
    let mut scene = RenderScene::empty(size.render);
    scene.post = Some(PostData {
        ssr: 1.0,
        ..exact_post()
    });
    scene.environment = Some(test_env());
    let eye = Vec3::new(0.0, 1.0, 7.5);
    scene.camera = crate::scene::RenderCamera {
        view: Mat4::look_at_rh(eye, Vec3::new(0.0, 0.5, 0.0), Vec3::Y),
        projection: Mat4::perspective_rh(60.0_f32.to_radians(), 1.5, 0.1, 100.0),
        position: eye,
    };
    let mut floor = ground(50.0);
    floor.material = [0.02, 1.0, 0.0];
    let mut metal_box = cube(Vec3::new(0.0, 0.5, 0.0), Vec3::ONE, [0.9, 0.9, 0.9, 1.0]);
    metal_box.material = [0.1, 1.0, 0.0];
    scene.meshes = vec![floor, metal_box];
    let chain = PostChain {
        ssr: Some(QualityParams::FULL.ssr_scale),
        ..PostChain::for_scene(&scene).masked(&renderer.quality, &renderer.quality_params)
    };
    let (graph, output) = build_frame_graph(size, chain);
    let scheduled = schedule(&graph).expect("the ssr graph schedules");
    let recorder = renderer.prepare_frame(&scene, size, None).unwrap();
    let on = renderer
        .gpu
        .execute(&scheduled, output, &recorder)
        .expect("the ssr frame renders");
    assert!(renderer.gpu.take_fault().is_none(), "the ssr frame faulted");

    let (w, h) = size.output;
    let mean_luma = |points: &[Vec3]| -> f32 {
        let sum: f32 = points
            .iter()
            .map(|&p| {
                let (x, y) = project(&scene.camera, p, w, h);
                let at = ((y * w + x) * 4) as usize;
                let c = |i: usize| f32::from(on.rgba[at + i]);
                0.2126 * c(0) + 0.7152 * c(1) + 0.0722 * c(2)
            })
            .sum();
        sum / points.len() as f32
    };
    let face = mean_luma(&[
        Vec3::new(-0.2, 0.3, 0.5),
        Vec3::new(0.0, 0.5, 0.5),
        Vec3::new(0.2, 0.7, 0.5),
    ]);
    let reflection = mean_luma(&[
        Vec3::new(-0.2, 0.0, 1.0),
        Vec3::new(0.0, 0.0, 2.0),
        Vec3::new(0.2, 0.0, 3.0),
    ]);
    assert!(
        reflection >= 0.5 * face,
        "the mirror shows the metal box at {reflection:.1} luma against {face:.1} on the box itself \
         (ratio {:.3})",
        reflection / face
    );
}

/// The mirror-floor scene of the SSR tests with SSR authored on and `view` requested, rendered
/// through the real path; the pixel reader maps a world point to its RGB bytes.
fn ssr_view_frame(
    renderer: &Renderer,
    ssr: f32,
    view: Option<crate::DebugView>,
) -> (RenderedImage, RenderScene) {
    ssr_view_frame_at(renderer, FrameSize::native(96, 64), ssr, view)
}

fn ssr_view_frame_at(
    renderer: &Renderer,
    size: FrameSize,
    ssr: f32,
    view: Option<crate::DebugView>,
) -> (RenderedImage, RenderScene) {
    let mut scene = RenderScene::empty(size.render);
    scene.post = Some(PostData {
        ssr,
        ..PostData::default()
    });
    scene.environment = Some(test_env());
    scene.debug_view = view;
    let eye = Vec3::new(0.0, 4.5, 7.5);
    scene.camera = crate::scene::RenderCamera {
        view: Mat4::look_at_rh(eye, Vec3::new(0.0, 0.5, 0.0), Vec3::Y),
        projection: Mat4::perspective_rh(60.0_f32.to_radians(), 1.5, 0.1, 100.0),
        position: eye,
    };
    let mut floor = ground(50.0);
    floor.material = [0.02, 1.0, 0.0];
    let mut red_box = cube(Vec3::new(0.0, 0.5, 0.0), Vec3::ONE, [1.0, 0.0, 0.0, 1.0]);
    red_box.material = [0.5, 0.0, 1.0];
    scene.meshes = vec![floor, red_box];
    let image = renderer.render(&scene, size).expect("render");
    (image, scene)
}

fn view_pixel(image: &RenderedImage, (x, y): (u32, u32)) -> [u8; 3] {
    let at = ((y * image.width + x) * 4) as usize;
    [image.rgba[at], image.rgba[at + 1], image.rgba[at + 2]]
}

/// The atlas view letterboxes the atlas: black outside the square, linearised depth gray in the
/// granted spot's tile, flat dark blue in an ungranted cell.
#[test]
fn needs_gpu_local_shadow_atlas_view_shows_the_granted_tile() {
    let Some(mut renderer) = skip_or_renderer("local_shadow_atlas_view_shows_the_granted_tile")
    else {
        return;
    };
    renderer.set_quality(
        Quality::FULL,
        QualityParams {
            local_shadow_atlas_size: 256,
            ..QualityParams::FULL
        },
    );
    let (w, h) = (160u32, 120u32);
    let mut scene = local_shadow_fixture(w, h, true);
    scene.debug_view = Some(crate::DebugView::LocalShadowAtlas);
    let image = renderer
        .render(&scene, FrameSize::native(w, h))
        .expect("render");
    // The square is 120 px wide, starting at x = 20; atlas texel t lands on 20 + t * 120 / 256.
    assert_eq!(view_pixel(&image, (5, 60)), [0, 0, 0], "left of the square");
    let [r, g, b] = view_pixel(&image, (38, 18));
    assert!(
        r == g && g == b && r > 20,
        "the spot's tile is gray: {r},{g},{b}"
    );
    let [r, g, b] = view_pixel(&image, (113, 94));
    assert!(
        r < 30 && b > r + 20,
        "an ungranted cell is dark blue: {r},{g},{b}"
    );
}

/// A view never conjures its subject: with no casting light the frame is the one without the view.
#[test]
fn needs_gpu_local_shadow_atlas_view_without_a_grant_draws_the_normal_frame() {
    let Some(renderer) = skip_or_renderer("local_shadow_atlas_view_without_a_grant") else {
        return;
    };
    let (w, h) = (160u32, 120u32);
    let plain = local_shadow_fixture(w, h, false);
    let mut viewed = local_shadow_fixture(w, h, false);
    viewed.debug_view = Some(crate::DebugView::LocalShadowAtlas);
    let size = FrameSize::native(w, h);
    let plain = renderer.render(&plain, size).expect("render");
    let viewed = renderer.render(&viewed, size).expect("render");
    assert_eq!(plain.rgba, viewed.rgba);
}

/// The upsample is premultiplied by confidence: a miss tap beside a hit must not drag the hit's
/// radiance toward black, so the reflection's last row keeps most of the box's red.
#[test]
fn needs_gpu_ssr_upsample_leaves_no_dark_halo() {
    let Some(renderer) = skip_or_renderer("ssr_upsample_leaves_no_dark_halo") else {
        return;
    };
    let (off, scene) = ssr_view_frame(&renderer, 0.0, None);
    let (on, _) = ssr_view_frame(&renderer, 1.0, None);
    let (_, centre) = project(&scene.camera, Vec3::new(0.0, 0.0, 0.0), on.width, on.height);
    let x = on.width / 2;
    let red = |image: &RenderedImage, y: u32| i32::from(view_pixel(image, (x, y))[0]);
    let edge = (centre..on.height)
        .rev()
        .find(|&y| red(&on, y) > red(&off, y) + 8)
        .expect("no floor texel picked up the reflection");
    assert!(
        red(&on, edge) >= 120,
        "reflection edge row {edge} is a dark halo: red {} (off {})",
        red(&on, edge),
        red(&off, edge)
    );
}

/// The confidence view draws the trace's alpha as gray: confident under the cube, zero in the
/// sky and where the ray leaves the top of the screen.
#[test]
fn needs_gpu_ssr_confidence_view_shows_the_trace() {
    let Some(renderer) = skip_or_renderer("ssr_confidence_view_shows_the_trace") else {
        return;
    };
    let (image, scene) = ssr_view_frame(&renderer, 1.0, Some(crate::DebugView::SsrConfidence));
    let (w, h) = (image.width, image.height);
    for z in [0.8, 1.0] {
        let at = project(&scene.camera, Vec3::new(0.0, 0.0, z), w, h);
        let [r, g, b] = view_pixel(&image, at);
        assert!(
            r >= 128 && r == g && g == b,
            "floor z={z} {at:?}: {r},{g},{b}"
        );
    }
    assert_eq!(view_pixel(&image, (w / 2, 1)), [0, 0, 0], "sky");
    let at = project(&scene.camera, Vec3::new(5.0, 0.0, 0.0), w, h);
    assert_eq!(view_pixel(&image, at), [0, 0, 0], "a ray leaving the top");
}

/// The footprint view draws a gray level over the pyramid on a hit and magenta on a miss.
#[test]
fn needs_gpu_ssr_footprint_view_marks_misses_magenta() {
    let Some(renderer) = skip_or_renderer("ssr_footprint_view_marks_misses_magenta") else {
        return;
    };
    let (image, scene) = ssr_view_frame(&renderer, 1.0, Some(crate::DebugView::SsrFootprint));
    let (w, h) = (image.width, image.height);
    assert_eq!(view_pixel(&image, (w / 2, 1)), [255, 0, 255], "sky");
    let at = project(&scene.camera, Vec3::new(0.0, 0.0, 0.9), w, h);
    let [r, g, b] = view_pixel(&image, at);
    assert!(r == g && g == b, "floor under the cube {at:?}: {r},{g},{b}");
}

/// A view never turns SSR on: with `ssr` at zero the frame is the normal one.
#[test]
fn needs_gpu_a_debug_view_without_ssr_draws_the_normal_frame() {
    let Some(renderer) = skip_or_renderer("a_debug_view_without_ssr_draws_the_normal_frame") else {
        return;
    };
    let (plain, _) = ssr_view_frame(&renderer, 0.0, None);
    for view in crate::DebugView::ALL {
        // The irradiance view needs no subject: it always replaces the frame.
        if view == crate::DebugView::Irradiance {
            continue;
        }
        let (viewed, _) = ssr_view_frame(&renderer, 0.0, Some(view));
        assert_eq!(plain.rgba, viewed.rgba, "{} moved the frame", view.name());
    }
}

/// Authoring `PostData.ssr` is the whole switch: through `Renderer::render` alone it reddens the
/// mirror floor, `ssr: 0` is the no-post frame, and `Quality::LOW` renders an `ssr: 1` scene as
/// the `ssr: 0` one.
#[test]
fn needs_gpu_authored_ssr_reflects_through_the_real_path() {
    let Some(mut renderer) = skip_or_renderer("authored_ssr_reflects_through_the_real_path") else {
        return;
    };
    let size = FrameSize::native(96, 64);
    let mut scene = RenderScene::empty(size.render);
    scene.environment = Some(test_env());
    let eye = Vec3::new(0.0, 4.5, 7.5);
    scene.camera = crate::scene::RenderCamera {
        view: Mat4::look_at_rh(eye, Vec3::new(0.0, 0.5, 0.0), Vec3::Y),
        projection: Mat4::perspective_rh(60.0_f32.to_radians(), 1.5, 0.1, 100.0),
        position: eye,
    };
    let mut floor = ground(50.0);
    floor.material = [0.02, 1.0, 0.0];
    let mut red_box = cube(Vec3::new(0.0, 0.5, 0.0), Vec3::ONE, [1.0, 0.0, 0.0, 1.0]);
    red_box.material = [0.5, 0.0, 1.0];
    scene.meshes = vec![floor, red_box];
    let with_ssr = |renderer: &Renderer, scene: &mut RenderScene, ssr: f32| {
        scene.post = Some(PostData {
            ssr,
            ..PostData::default()
        });
        renderer.render(scene, size).expect("render")
    };

    let off = with_ssr(&renderer, &mut scene, 0.0);
    let on = with_ssr(&renderer, &mut scene, 1.0);
    let (w, h) = size.output;
    let at = project(&scene.camera, Vec3::new(0.0, 0.0, 0.8), w, h);
    let i = ((at.1 * w + at.0) * 4) as usize;
    let redness = |image: &RenderedImage| i32::from(image.rgba[i]) - i32::from(image.rgba[i + 2]);
    assert!(
        redness(&on) > redness(&off) + 8,
        "authored ssr did not redden the floor at {at:?}"
    );

    scene.post = None;
    let none = renderer.render(&scene, size).expect("no-post render");
    assert_eq!(none.rgba, off.rgba, "ssr: 0 is not the no-post frame");

    renderer.set_quality(crate::Quality::LOW, QualityParams::LOW);
    let low_off = with_ssr(&renderer, &mut scene, 0.0);
    let low_on = with_ssr(&renderer, &mut scene, 1.0);
    assert_eq!(low_on.rgba, low_off.rgba, "LOW still traced");
}

/// The SSR trace of `scene`, read back from a harness graph that makes it an output: the trace
/// extent and its texels as `[radiance, confidence]`.
fn ssr_trace_texels(
    renderer: &Renderer,
    scene: &RenderScene,
    size: FrameSize,
) -> (u32, u32, Vec<[f32; 4]>) {
    let (graph, trace) = crate::graph::build_ssr_trace_graph(size);
    let recorder = renderer.prepare_frame(scene, size, None).unwrap();
    let (tw, th) = crate::ssr_pyramid::trace_extent(size, QualityParams::FULL.ssr_scale);
    let copy = renderer
        .gpu
        .device()
        .create_texture(&wgpu::TextureDescriptor {
            label: Some("ssr trace copy"),
            size: wgpu::Extent3d {
                width: tw,
                height: th,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba16Float,
            usage: wgpu::TextureUsages::COPY_DST | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
    renderer
        .gpu
        .execute_into(
            &schedule(&graph).expect("the trace graph schedules"),
            &recorder,
            &[(trace, copy.as_image_copy())],
        )
        .expect("the trace renders");
    let bytes = read_texels(renderer, &copy, 8, wgpu::TextureAspect::All);
    assert!(renderer.gpu.take_fault().is_none(), "the trace faulted");
    let texels = bytes
        .chunks_exact(8)
        .map(|texel| {
            std::array::from_fn(|c| {
                crate::gi_bake::f16_to_f32(u16::from_le_bytes([texel[c * 2], texel[c * 2 + 1]]))
            })
        })
        .collect();
    (tw, th, texels)
}

/// The transparent layer of `scene`, read back from a harness graph that makes it an output:
/// each texel as `[distance × coverage, coverage]`.
fn layer_texels(renderer: &Renderer, scene: &RenderScene, size: FrameSize) -> Vec<[f32; 2]> {
    let (graph, layer) = crate::graph::build_transparent_depth_graph(size);
    let recorder = renderer.prepare_frame(scene, size, None).unwrap();
    let copy = renderer
        .gpu
        .device()
        .create_texture(&wgpu::TextureDescriptor {
            label: Some("transparent layer copy"),
            size: wgpu::Extent3d {
                width: size.render.0,
                height: size.render.1,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rg16Float,
            usage: wgpu::TextureUsages::COPY_DST | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
    renderer
        .gpu
        .execute_into(
            &schedule(&graph).expect("the layer graph schedules"),
            &recorder,
            &[(layer, copy.as_image_copy())],
        )
        .expect("the layer renders");
    let bytes = read_texels(renderer, &copy, 4, wgpu::TextureAspect::All);
    assert!(renderer.gpu.take_fault().is_none(), "the layer faulted");
    bytes
        .chunks_exact(4)
        .map(|texel| {
            std::array::from_fn(|c| {
                crate::gi_bake::f16_to_f32(u16::from_le_bytes([texel[c * 2], texel[c * 2 + 1]]))
            })
        })
        .collect()
}

fn layer_sprite(center: Vec3, color: [f32; 4], additive: bool) -> crate::scene::SpriteInstance {
    crate::scene::SpriteInstance {
        center,
        right: Vec3::new(4.0, 0.0, 0.0),
        up: Vec3::new(0.0, 4.0, 0.0),
        color,
        texture: None,
        uv: crate::scene::SpriteInstance::FULL_UV,
        lit: false,
        layer: 0,
        additive,
        flash: 0.0,
        flash_color: [1.0; 3],
        uv_repeat: crate::scene::SpriteInstance::NO_TILING,
        uv_offset: [0.0; 2],
        medium: 1.0,
    }
}

/// A world text run covering `[-2, 2]²` at depth `z`, on the embedded face.
fn layer_text(z: f32) -> crate::scene::TextRun {
    let face = ludion_font::FontFace::embedded();
    crate::scene::TextRun {
        position: Vec3::new(-2.0, 2.0, z),
        size: 4.0,
        color: [1.0; 4],
        text: "A".to_string(),
        quads: ludion_font::layout(face, "A", 4.0, 0.0).quads(
            [-2.0, 2.0],
            4.0,
            ludion_font::YAxis::Up,
        ),
        extent: [4.0, 4.0],
        font: None,
    }
}

/// The layer texel at the frame's centre, over the sky, with the frame's other texels untouched.
fn centre_layer(texels: &[[f32; 2]], size: FrameSize) -> [f32; 2] {
    texels[((size.render.1 / 2) * size.render.0 + size.render.0 / 2) as usize]
}

/// The layer target holds weighted distance in R and coverage in G, at measured distances: an
/// opaque sprite reads its own distance at full coverage, a soft one half coverage at the same
/// distance, two stacked ones the blend-weighted mean, and a black additive quad claims nothing.
/// A particle and a text run each read coverage, a sprite behind an opaque wall reads none (the
/// depth test holds) and world text behind it does (`Always`, as the colour pass draws it).
/// Skips with no GPU.
#[test]
fn needs_gpu_the_layer_target_holds_distance_and_coverage() {
    let Some(renderer) = skip_or_renderer("the_layer_target_holds_distance_and_coverage") else {
        return;
    };
    let (w, h) = (64u32, 64u32);
    let size = FrameSize::native(w, h);
    let mut scene = RenderScene::empty((w, h));
    scene.camera = origin_camera(w, h);
    scene.post = Some(exact_post());
    let read = |scene: &RenderScene| centre_layer(&layer_texels(&renderer, scene, size), size);
    let close = |got: f32, want: f32, what: &str| {
        assert!(
            (got - want).abs() <= want.abs() * 0.004 + 1e-3,
            "{what}: got {got}, want {want}"
        );
    };

    let bare = read(&scene);
    assert_eq!(bare, [0.0, 0.0], "an empty frame clears the layer");

    scene.sprites = vec![layer_sprite(Vec3::new(0.0, 0.0, -7.0), [1.0; 4], false)];
    let [r, g] = read(&scene);
    close(g, 1.0, "opaque coverage");
    close(r / g, 7.0, "opaque distance");

    scene.sprites = vec![layer_sprite(
        Vec3::new(0.0, 0.0, -7.0),
        [1.0, 1.0, 1.0, 0.5],
        false,
    )];
    let [r, g] = read(&scene);
    close(g, 0.5, "soft coverage");
    close(r / g, 7.0, "soft distance");

    scene.sprites = vec![
        layer_sprite(Vec3::new(0.0, 0.0, -15.0), [1.0; 4], false),
        layer_sprite(Vec3::new(0.0, 0.0, -5.0), [1.0, 1.0, 1.0, 0.5], false),
    ];
    let [r, g] = read(&scene);
    close(g, 1.0, "stacked coverage");
    close(r, 0.5 * 5.0 + 0.5 * 15.0, "stacked weighted distance");

    scene.sprites = vec![layer_sprite(
        Vec3::new(0.0, 0.0, -7.0),
        [0.0, 0.0, 0.0, 1.0],
        true,
    )];
    assert_eq!(
        read(&scene),
        [0.0, 0.0],
        "a black additive quad claims nothing"
    );

    scene.sprites.clear();
    scene.particles = vec![particle(Vec3::new(0.0, 0.0, -6.0), 3.0, [1.0; 4], 0.0)];
    let [r, g] = read(&scene);
    assert!(g > 0.5, "a particle covers the centre: {g}");
    close(r / g, 6.0, "particle distance");

    scene.particles.clear();
    scene.texts = vec![layer_text(-6.0)];
    let covered = layer_texels(&renderer, &scene, size)
        .iter()
        .filter(|texel| texel[1] > 0.0)
        .count();
    assert!(covered > 0, "a text run covers some texels");

    // A wall in front of everything: depth-tested kinds vanish behind it, text does not.
    scene.meshes = vec![cube(
        Vec3::new(0.0, 0.0, -3.0),
        Vec3::new(8.0, 8.0, 0.5),
        [0.5, 0.5, 0.5, 1.0],
    )];
    let behind_text = layer_texels(&renderer, &scene, size)
        .iter()
        .filter(|texel| texel[1] > 0.0)
        .count();
    assert!(behind_text > 0, "text behind a wall still reads coverage");
    scene.texts.clear();
    scene.sprites = vec![layer_sprite(Vec3::new(0.0, 0.0, -7.0), [1.0; 4], false)];
    assert_eq!(
        read(&scene),
        [0.0, 0.0],
        "a sprite behind a wall reads none"
    );
}

/// Under an orthographic camera the layer measures along the view direction, not along the
/// per-pixel ray: two sprites at one view depth and far apart read the same mean distance.
/// Skips with no GPU.
#[test]
fn needs_gpu_an_orthographic_layer_measures_along_the_view() {
    let Some(renderer) = skip_or_renderer("an_orthographic_layer_measures_along_the_view") else {
        return;
    };
    let (w, h) = (64u32, 64u32);
    let size = FrameSize::native(w, h);
    let mut scene = RenderScene::empty((w, h));
    scene.camera = crate::scene::RenderCamera {
        view: Mat4::look_at_rh(Vec3::ZERO, Vec3::new(0.0, 0.0, -1.0), Vec3::Y),
        projection: Mat4::orthographic_rh(-5.0, 5.0, -5.0, 5.0, 0.1, 1000.0),
        position: Vec3::ZERO,
    };
    scene.post = Some(exact_post());
    let small = |x: f32| {
        let mut sprite = layer_sprite(Vec3::new(x, 0.0, -10.0), [1.0; 4], false);
        sprite.right = Vec3::new(1.0, 0.0, 0.0);
        sprite.up = Vec3::new(0.0, 1.0, 0.0);
        sprite
    };
    scene.sprites = vec![small(-3.0), small(3.0)];
    let texels = layer_texels(&renderer, &scene, size);
    let mut means = Vec::new();
    for x in [-3.0, 3.0] {
        let (px, py) = project(&scene.camera, Vec3::new(x, 0.0, -10.0), w, h);
        let [r, g] = texels[(py * w + px) as usize];
        assert!(g > 0.99, "the sprite at x = {x} covers its centre: {g}");
        means.push(r / g);
    }
    for mean in &means {
        assert!(
            (mean - 10.0).abs() < 0.04,
            "ortho distance along the view: {means:?}"
        );
    }
}

/// A frame after a transparent is byte-identical to one that never had it: the layer's transient
/// set coming and going leaves no trace in the picture. Skips with no GPU.
#[test]
fn needs_gpu_a_frame_after_a_transparent_is_byte_identical_to_one_that_never_had_it() {
    let Some(renderer) = skip_or_renderer("a_frame_after_a_transparent_is_byte_identical") else {
        return;
    };
    let Some(fresh) = skip_or_renderer("a_frame_after_a_transparent_is_byte_identical") else {
        return;
    };
    let (w, h) = (64u32, 64u32);
    let size = FrameSize::native(w, h);
    let mut bare = RenderScene::empty((w, h));
    bare.camera = origin_camera(w, h);
    bare.post = Some(PostData {
        fog_density: 0.05,
        ..exact_post()
    });
    bare.meshes = vec![cube(
        Vec3::new(0.0, 0.0, -9.0),
        Vec3::splat(6.0),
        [0.5, 0.5, 0.5, 1.0],
    )];
    let mut with_sprite = bare.clone();
    with_sprite.sprites = vec![layer_sprite(
        Vec3::new(0.0, 0.0, -5.0),
        [1.0, 0.5, 0.2, 0.7],
        false,
    )];
    assert!(PostChain::for_scene(&with_sprite).transparent_depth_scheduled());
    assert!(!PostChain::for_scene(&bare).transparent_depth_scheduled());

    let first = renderer.render(&bare, size).expect("first frame");
    let flipped = renderer.render(&with_sprite, size).expect("sprite frame");
    let third = renderer.render(&bare, size).expect("third frame");
    let never = fresh.render(&bare, size).expect("fresh frame");
    assert_ne!(first.rgba, flipped.rgba, "the sprite is visible");
    assert_eq!(third.rgba, first.rgba, "the third frame matches the first");
    assert_eq!(
        third.rgba, never.rgba,
        "the third frame matches a fresh renderer's"
    );
}

// --- Fog at each transparent's own distance ----------------------------------------------

/// The exact-output post with fog on: an output byte is the sRGB encoding of the linear value
/// the composite produced, and `dither: false` keeps two different pixels comparable.
fn own_fog_post(density: f32, color: [f32; 3]) -> PostData {
    PostData {
        fog_density: density,
        fog_color: color,
        ..exact_post()
    }
}

/// The fog fixture's sky and `FOG_COLOR`, with no slabs: a camera at the origin looking down −Z.
fn own_fog_scene(w: u32, h: u32, density: f32) -> RenderScene {
    let mut scene = fog_scene(w, h);
    scene.meshes.clear();
    scene.post = Some(own_fog_post(density, FOG_COLOR));
    scene
}

/// A square sprite of side `size`.
fn sized_sprite(
    center: Vec3,
    size: f32,
    color: [f32; 4],
    additive: bool,
) -> crate::scene::SpriteInstance {
    let mut sprite = layer_sprite(center, color, additive);
    sprite.right = Vec3::new(size, 0.0, 0.0);
    sprite.up = Vec3::new(0.0, size, 0.0);
    sprite
}

const OPAQUE_SLAB: [f32; 4] = [SLAB_COLOR[0], SLAB_COLOR[1], SLAB_COLOR[2], 1.0];

fn channel_gap(a: [u8; 3], b: [u8; 3]) -> u8 {
    (0..3).map(|c| a[c].abs_diff(b[c])).max().unwrap()
}

/// The linear value `surface` becomes at `distance` in a fog of `density` toward `fog`.
fn fogged(surface: [f32; 3], fog: [f32; 3], density: f32, distance: f32) -> [f32; 3] {
    let transmitted = (-density * distance).exp();
    std::array::from_fn(|c| surface[c] * transmitted + fog[c] * (1.0 - transmitted))
}

/// Every pixel of `texels` that the layer covers fully, as `(x, y)`.
fn covered_pixels(texels: &[[f32; 2]], width: u32) -> Vec<(u32, u32)> {
    texels
        .iter()
        .enumerate()
        .filter(|(_, texel)| texel[1] > 0.999)
        .map(|(i, _)| (i as u32 % width, i as u32 / width))
        .collect()
}

/// An opaque sprite is fogged at its own distance, so the same sprite reads the same over a wall
/// 40 units behind it and over the open sky. Skips with no GPU.
#[test]
fn needs_gpu_a_sprite_is_fogged_the_same_over_a_wall_and_over_the_sky() {
    let Some(renderer) =
        skip_or_renderer("a_sprite_is_fogged_the_same_over_a_wall_and_over_the_sky")
    else {
        return;
    };
    let (w, h) = (64u32, 64u32);
    let size = FrameSize::native(w, h);
    let mut sky = own_fog_scene(w, h, 0.03);
    sky.sprites = vec![sized_sprite(
        Vec3::new(0.0, 0.0, -5.0),
        3.0,
        OPAQUE_SLAB,
        false,
    )];
    let mut wall = sky.clone();
    wall.meshes = vec![slab(
        Vec3::new(0.0, 0.0, -60.0),
        40.0,
        [0.0625, 0.5, 0.0625],
    )];
    let over_sky = renderer.render(&sky, size).expect("sky frame");
    let over_wall = renderer.render(&wall, size).expect("wall frame");
    let mut worst = 0;
    for (x, y) in [(28, 28), (36, 28), (28, 36), (36, 36), (32, 32)] {
        worst = worst.max(channel_gap(
            rgb_at(&over_sky, x, y),
            rgb_at(&over_wall, x, y),
        ));
    }
    assert!(
        worst <= 1,
        "the sprite must not take its fog from what is behind it: worst gap {worst} LSB, \
         sky {:?} wall {:?}",
        rgb_at(&over_sky, 32, 32),
        rgb_at(&over_wall, 32, 32),
    );
}

/// A sprite across the top edge of a wall has no step where the wall ends: both sides are fogged
/// at the sprite's distance. Skips with no GPU.
#[test]
fn needs_gpu_a_sprite_straddling_the_horizon_has_no_fog_seam() {
    let Some(renderer) = skip_or_renderer("a_sprite_straddling_the_horizon_has_no_fog_seam") else {
        return;
    };
    let (w, h) = (64u32, 64u32);
    let mut scene = own_fog_scene(w, h, 0.03);
    scene.meshes = vec![slab(Vec3::new(0.0, -30.0, -60.0), 60.0, SLAB_COLOR)];
    scene.sprites = vec![sized_sprite(
        Vec3::new(0.0, 0.0, -5.0),
        3.0,
        OPAQUE_SLAB,
        false,
    )];
    let image = renderer
        .render(&scene, FrameSize::native(w, h))
        .expect("seam frame");
    let above = rgb_at(&image, 32, 28);
    let below = rgb_at(&image, 32, 36);
    assert!(
        channel_gap(above, below) <= 1,
        "a step across the wall's top edge: above {above:?} below {below:?}",
    );
}

/// A far sprite against the sky moves toward the fog colour by the analytic extinction.
/// Skips with no GPU.
#[test]
fn needs_gpu_a_far_sprite_against_the_sky_is_fogged() {
    let Some(renderer) = skip_or_renderer("a_far_sprite_against_the_sky_is_fogged") else {
        return;
    };
    let (w, h) = (64u32, 64u32);
    let (density, distance) = (0.03_f32, 50.0_f32);
    let mut scene = own_fog_scene(w, h, density);
    scene.sprites = vec![sized_sprite(
        Vec3::new(0.0, 0.0, -distance),
        20.0,
        OPAQUE_SLAB,
        false,
    )];
    let image = renderer
        .render(&scene, FrameSize::native(w, h))
        .expect("far sprite frame");
    let want = fogged(SLAB_COLOR, FOG_COLOR, density, distance);
    let got = rgb_at(&image, 32, 32);
    for c in 0..3 {
        let expected = srgb8_from_linear(want[c]);
        assert!(
            (f32::from(got[c]) - expected).abs() <= 2.0,
            "channel {c}: got {got:?}, want {expected} (linear {want:?})",
        );
    }
}

/// A half-transparent sprite across a wall's top edge stays continuous. With layer colour `S`
/// and weight `a` over a background `B`, composite fogs the image once, so the transmitted
/// colour is off the ideal `a(S·T_s + L·F_s) + (1-a)(B·T_b + L·F_b)` by exactly
/// `a(1-a)(S-B)(F_s-F_b)`. Each side must sit within that error of the ideal, plus a decode
/// tolerance, and the jump across the edge within the ideal jump plus both errors. Skips with no
/// GPU.
#[test]
fn needs_gpu_a_soft_sprite_is_continuous_over_a_fog_edge() {
    let Some(renderer) = skip_or_renderer("a_soft_sprite_is_continuous_over_a_fog_edge") else {
        return;
    };
    let (w, h) = (64u32, 64u32);
    let size = FrameSize::native(w, h);
    let (density, alpha) = (0.03_f32, 0.5_f32);
    let mut bare = own_fog_scene(w, h, density);
    bare.meshes = vec![slab(Vec3::new(0.0, -30.0, -60.0), 60.0, SLAB_COLOR)];
    let mut soft = bare.clone();
    soft.sprites = vec![sized_sprite(
        Vec3::new(0.0, 0.0, -5.0),
        3.0,
        [0.0625, 0.5, 0.0625, alpha],
        false,
    )];
    let background = renderer.render(&bare, size).expect("background frame");
    let image = renderer.render(&soft, size).expect("soft frame");
    let sprite = [0.0625_f32, 0.5, 0.0625];
    let f_sprite = 1.0 - (-density * 5.0).exp();
    let read = |img: &RenderedImage, y: u32| -> [f64; 3] {
        let p = rgb_at(img, 32, y);
        std::array::from_fn(|c| linear_from_srgb8(p[c]))
    };
    let mut worst_jump = 0.0_f64;
    let mut worst_gap = 0.0_f64;
    let mut ideal = [[0.0_f64; 3]; 2];
    let mut bound = [[0.0_f64; 3]; 2];
    for (side, y) in [28u32, 36].into_iter().enumerate() {
        let bg = read(&background, y);
        let got = read(&image, y);
        // The unfogged background is the sky above the edge and the slab below it.
        let (unfogged, f_bg): ([f64; 3], f64) = if side == 0 {
            (std::array::from_fn(|c| bg[c]), 0.0)
        } else {
            let tangent = (f32::from(36u8) + 0.5 - 32.0) / 32.0 * 30.0_f32.to_radians().tan();
            let distance = 30.0 * (1.0 + tangent * tangent).sqrt();
            (
                std::array::from_fn(|c| f64::from(SLAB_COLOR[c])),
                f64::from(1.0 - (-density * distance).exp()),
            )
        };
        for c in 0..3 {
            let fog_light = f64::from(FOG_COLOR[c]);
            let s = f64::from(sprite[c]);
            let a = f64::from(alpha);
            ideal[side][c] = a
                * (s * (1.0 - f64::from(f_sprite)) + fog_light * f64::from(f_sprite))
                + (1.0 - a) * bg[c];
            bound[side][c] =
                a * (1.0 - a) * (s - unfogged[c]).abs() * (f64::from(f_sprite) - f_bg).abs();
            worst_gap = worst_gap.max((got[c] - ideal[side][c]).abs() - bound[side][c]);
        }
        let jump = std::array::from_fn::<f64, 3, _>(|c| got[c]);
        if side == 1 {
            let above = read(&image, 28);
            for c in 0..3 {
                let ideal_jump = (ideal[1][c] - ideal[0][c]).abs();
                worst_jump = worst_jump
                    .max((jump[c] - above[c]).abs() - ideal_jump - bound[0][c] - bound[1][c]);
            }
        }
    }
    eprintln!(
        "soft sprite: worst excess over the error bound {worst_gap:.5}, jump excess {worst_jump:.5}"
    );
    assert!(
        worst_gap <= 0.01,
        "a side is off the ideal by more than a(1-a)|S-B||F_s-F_b|: excess {worst_gap}",
    );
    assert!(
        worst_jump <= 0.01,
        "the jump across the fog edge exceeds the ideal jump plus both errors: {worst_jump}",
    );
}

/// A particle and a text run are each fogged at their own distance: the same pixels over a wall
/// and over the sky read the same. Skips with no GPU.
#[test]
fn needs_gpu_particles_and_world_text_are_fogged_at_their_own_distance() {
    let Some(renderer) =
        skip_or_renderer("particles_and_world_text_are_fogged_at_their_own_distance")
    else {
        return;
    };
    let (w, h) = (64u32, 64u32);
    let size = FrameSize::native(w, h);
    let mut sky = own_fog_scene(w, h, 0.03);
    sky.particles = vec![particle(Vec3::new(0.0, 0.0, -5.0), 3.0, OPAQUE_SLAB, 0.0)];
    let mut sky_text = own_fog_scene(w, h, 0.03);
    sky_text.texts = vec![layer_text(-5.0)];
    for (what, scene) in [("particle", sky), ("text", sky_text)] {
        let mut wall = scene.clone();
        wall.meshes = vec![slab(
            Vec3::new(0.0, 0.0, -60.0),
            40.0,
            [0.0625, 0.5, 0.0625],
        )];
        let interior = covered_pixels(&layer_texels(&renderer, &scene, size), w);
        assert!(!interior.is_empty(), "{what} covers some pixels outright");
        let over_sky = renderer.render(&scene, size).expect("sky frame");
        let over_wall = renderer.render(&wall, size).expect("wall frame");
        let worst = interior
            .iter()
            .map(|&(x, y)| channel_gap(rgb_at(&over_sky, x, y), rgb_at(&over_wall, x, y)))
            .max()
            .unwrap();
        assert!(
            worst <= 1,
            "{what}: fogged by what is behind it, worst gap {worst} LSB over {} pixels",
            interior.len(),
        );
    }
}

/// A black additive texel adds nothing and claims no fog: the frame equals the one without the
/// sprite, byte for byte. Skips with no GPU.
#[test]
fn needs_gpu_a_black_additive_texel_claims_no_fog() {
    let Some(renderer) = skip_or_renderer("a_black_additive_texel_claims_no_fog") else {
        return;
    };
    let (w, h) = (64u32, 64u32);
    let size = FrameSize::native(w, h);
    let mut bare = own_fog_scene(w, h, 0.03);
    bare.meshes = vec![slab(Vec3::new(0.0, 0.0, -60.0), 40.0, SLAB_COLOR)];
    let mut black = bare.clone();
    black.sprites = vec![sized_sprite(
        Vec3::new(0.0, 0.0, -5.0),
        3.0,
        [0.0, 0.0, 0.0, 1.0],
        true,
    )];
    assert_eq!(
        renderer.render(&bare, size).expect("bare").rgba,
        renderer.render(&black, size).expect("black").rgba,
        "a black additive quad must leave the wall's fog alone",
    );
}

/// A bright additive texel over black is dimmed by the fog at its own distance: a black sky
/// and a zero fog colour leave extinction as the only term. Skips with no GPU.
#[test]
fn needs_gpu_a_bright_additive_texel_is_dimmed_at_its_own_distance() {
    let Some(renderer) = skip_or_renderer("a_bright_additive_texel_is_dimmed_at_its_own_distance")
    else {
        return;
    };
    let (w, h) = (64u32, 64u32);
    let (density, distance) = (0.03_f32, 5.0_f32);
    let mut scene = RenderScene::empty((w, h));
    scene.camera = origin_camera(w, h);
    scene.post = Some(own_fog_post(density, [0.0; 3]));
    // An all-black sky: without an `Environment` the clear is the engine's historical colour.
    scene.environment = Some(crate::scene::EnvironmentData {
        sky_zenith: [0.0; 3],
        sky_horizon: [0.0; 3],
        sky_ground: [0.0; 3],
        sun_disc_size: 0.0,
        sun_disc_intensity: 0.0,
        ambient: [0.0; 3],
        ibl_intensity: 0.0,
    });
    let emitted = [1.0_f32, 0.5, 0.25];
    scene.sprites = vec![sized_sprite(
        Vec3::new(0.0, 0.0, -distance),
        3.0,
        [emitted[0], emitted[1], emitted[2], 1.0],
        true,
    )];
    let image = renderer
        .render(&scene, FrameSize::native(w, h))
        .expect("additive frame");
    assert_eq!(rgb_at(&image, 2, 2), [0, 0, 0], "the background is black");
    let transmitted = (-density * distance).exp();
    let got = rgb_at(&image, 32, 32);
    for c in 0..3 {
        let expected = srgb8_from_linear(emitted[c] * transmitted);
        assert!(
            (f32::from(got[c]) - expected).abs() <= 2.0,
            "channel {c}: got {got:?}, want {expected}",
        );
    }
}

/// Pixels no transparent touches are exactly what the frame without it renders. Skips with no
/// GPU.
#[test]
fn needs_gpu_pixels_a_transparent_misses_are_untouched() {
    let Some(renderer) = skip_or_renderer("pixels_a_transparent_misses_are_untouched") else {
        return;
    };
    let (w, h) = (64u32, 64u32);
    let size = FrameSize::native(w, h);
    let mut bare = own_fog_scene(w, h, 0.03);
    bare.meshes = vec![slab(Vec3::new(0.0, 0.0, -60.0), 40.0, SLAB_COLOR)];
    let mut with = bare.clone();
    with.sprites = vec![sized_sprite(
        Vec3::new(0.0, 0.0, -5.0),
        3.0,
        OPAQUE_SLAB,
        false,
    )];
    let a = renderer.render(&bare, size).expect("bare");
    let b = renderer.render(&with, size).expect("sprite");
    let mut checked = 0;
    for y in 0..h {
        for x in 0..w {
            if (12..52).contains(&x) && (12..52).contains(&y) {
                continue;
            }
            checked += 1;
            assert_eq!(
                pixel(&a, x, y),
                pixel(&b, x, y),
                "pixel ({x}, {y}) outside the quad"
            );
        }
    }
    assert!(checked > 1000);
    assert_ne!(a.rgba, b.rgba, "the sprite is visible");
}

/// The live path: a ring-read stream of frames that toggles the layer pass leaves every bare
/// frame equal to a fresh renderer's, and every sprite frame equal to another. Skips with no
/// GPU.
#[test]
fn needs_gpu_a_streamed_flip_leaves_the_image_untouched() {
    let (w, h) = (64u32, 64u32);
    let mut bare = own_fog_scene(w, h, 0.03);
    bare.meshes = vec![slab(Vec3::new(0.0, 0.0, -60.0), 40.0, SLAB_COLOR)];
    let mut with = bare.clone();
    with.sprites = vec![sized_sprite(
        Vec3::new(0.0, 0.0, -5.0),
        3.0,
        OPAQUE_SLAB,
        false,
    )];
    streamed_flip_is_neutral(
        "a_streamed_flip_leaves_the_image_untouched",
        FrameSize::native(w, h),
        &bare,
        &with,
    );
}

/// Streams `bare` and `with` alternately through a ring and holds every returned `bare` frame
/// to a fresh renderer's render of it and every `with` frame to the first one. Skips with no GPU.
fn streamed_flip_is_neutral(name: &str, size: FrameSize, bare: &RenderScene, with: &RenderScene) {
    let Some(renderer) = skip_or_renderer(name) else {
        return;
    };
    let Some(fresh) = skip_or_renderer(name) else {
        return;
    };
    let mut ring = renderer.new_stream_readback(size).expect("ring");
    // (frame was a sprite frame, image), in return order.
    let mut returned: Vec<(bool, RenderedImage)> = Vec::new();
    let counts = |returned: &[(bool, RenderedImage)]| {
        let sprites = returned.iter().filter(|(sprite, _)| *sprite).count();
        (returned.len() - sprites, sprites)
    };
    // Frames alternate bare, sprite, bare, ...; the ring may drop a frame in flight, so keep
    // submitting until enough of each kind has come back.
    for i in 0..60u64 {
        let scene = if i % 2 == 0 { bare } else { with };
        if let Some((stamp, image)) = renderer
            .render_streamed(scene, size, &mut ring, FrameIndex(i), None)
            .expect("streamed frame")
        {
            returned.push((stamp.0 % 2 == 1, image));
        }
        while let Some((stamp, image)) = renderer.collect_streamed(&mut ring).expect("collect") {
            returned.push((stamp.0 % 2 == 1, image));
        }
        let (bares, sprites) = counts(&returned);
        if bares >= 3 && sprites >= 2 {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    let (bares, sprites) = counts(&returned);
    assert!(
        bares >= 3 && sprites >= 2,
        "the stream returned {bares} bare and {sprites} sprite frames"
    );
    let never = fresh.render(bare, size).expect("fresh frame");
    let first_sprite = &returned.iter().find(|(sprite, _)| *sprite).unwrap().1;
    for (sprite, frame) in &returned {
        if *sprite {
            assert_eq!(
                frame.rgba, first_sprite.rgba,
                "sprite frames differ from each other"
            );
        } else {
            assert_eq!(
                frame.rgba, never.rgba,
                "a bare streamed frame differs from a fresh render"
            );
        }
    }
    assert_ne!(
        first_sprite.rgba, never.rgba,
        "the sprite must show, or the test proves nothing"
    );
}

fn ortho_fog_camera() -> crate::scene::RenderCamera {
    crate::scene::RenderCamera {
        view: Mat4::look_at_rh(Vec3::ZERO, Vec3::new(0.0, 0.0, -1.0), Vec3::Y),
        projection: Mat4::orthographic_rh(-20.0, 20.0, -20.0, 20.0, 0.1, 1000.0),
        position: Vec3::ZERO,
    }
}

/// Under an orthographic camera a plane facing it is fogged by view depth alone: every pixel
/// matches, for the plain medium, for a sky-tinted medium, and (along a row) for a height
/// falloff. Skips with no GPU.
#[test]
fn needs_gpu_ortho_fog_does_not_fan() {
    let Some(renderer) = skip_or_renderer("ortho_fog_does_not_fan") else {
        return;
    };
    let (w, h) = (64u32, 64u32);
    let size = FrameSize::native(w, h);
    let mut scene = own_fog_scene(w, h, 0.03);
    scene.camera = ortho_fog_camera();
    scene.meshes = vec![slab(Vec3::new(0.0, 0.0, -60.0), 100.0, SLAB_COLOR)];
    let spread = |image: &RenderedImage, pixels: &[(u32, u32)]| {
        let first = rgb_at(image, pixels[0].0, pixels[0].1);
        pixels
            .iter()
            .map(|&(x, y)| channel_gap(first, rgb_at(image, x, y)))
            .max()
            .unwrap()
    };
    let everywhere = [
        (0, 0),
        (63, 0),
        (0, 63),
        (63, 63),
        (32, 32),
        (8, 40),
        (55, 12),
    ];
    let one_row = [(0, 32), (10, 32), (32, 32), (50, 32), (63, 32)];

    let plain = renderer.render(&scene, size).expect("plain");
    assert!(spread(&plain, &everywhere) <= 1, "plain fog fans");

    let mut sky = scene.clone();
    if let Some(post) = sky.post.as_mut() {
        post.fog_sky = 1.0;
    }
    let sky_image = renderer.render(&sky, size).expect("sky fog");
    assert!(spread(&sky_image, &everywhere) <= 1, "sky-tinted fog fans");

    let mut height = scene.clone();
    if let Some(post) = height.post.as_mut() {
        post.fog_height_falloff = 0.05;
    }
    let height_image = renderer.render(&height, size).expect("height fog");
    assert!(
        spread(&height_image, &one_row) <= 1,
        "height fog fans along a row"
    );
}

/// Under an orthographic camera a sprite and a wall at one view depth fog alike, though they
/// sit far apart in the frame. Skips with no GPU.
#[test]
fn needs_gpu_an_ortho_sprite_and_a_wall_at_one_distance_fog_alike() {
    let Some(renderer) = skip_or_renderer("an_ortho_sprite_and_a_wall_at_one_distance_fog_alike")
    else {
        return;
    };
    let (w, h) = (64u32, 64u32);
    let mut scene = own_fog_scene(w, h, 0.03);
    scene.camera = ortho_fog_camera();
    scene.meshes = vec![slab(Vec3::new(-17.5, 0.0, -32.5), 25.0, SLAB_COLOR)];
    scene.sprites = vec![sized_sprite(
        Vec3::new(10.0, 0.0, -20.0),
        8.0,
        OPAQUE_SLAB,
        false,
    )];
    let image = renderer
        .render(&scene, FrameSize::native(w, h))
        .expect("ortho frame");
    // The slab's shading is not exactly its emissive colour, so each surface is held to its own
    // unfogged value run through the analytic extinction at the one shared depth.
    let mut clear = scene.clone();
    clear.post = Some(own_fog_post(0.0, FOG_COLOR));
    let unfogged = renderer
        .render(&clear, FrameSize::native(w, h))
        .expect("unfogged frame");
    for (what, x) in [("wall", 12), ("sprite", 48)] {
        let base = rgb_at(&unfogged, x, 32);
        let surface = std::array::from_fn(|c| linear_from_srgb8(base[c]) as f32);
        let want = fogged(surface, FOG_COLOR, 0.03, 20.0);
        let got = rgb_at(&image, x, 32);
        for c in 0..3 {
            let expected = srgb8_from_linear(want[c]);
            assert!(
                (f32::from(got[c]) - expected).abs() <= 2.0,
                "{what} channel {c}: got {got:?}, want {expected} from base {base:?}",
            );
        }
    }
}

// --- Clustered forward: the cap falls (Plan/clustered-forward.md) ----------------------

/// A point light at `pos` with the fixture's shared shape.
fn point_light(pos: Vec3, intensity: f32, range: f32) -> crate::scene::PointLightData {
    crate::scene::PointLightData {
        position: pos,
        color: Vec3::ONE,
        intensity,
        range,
        dynamic: false,
        shadow: None,
        scatter: 0.0,
    }
}

/// The 18-light fixture, replacing `select_lights_keeps_nearest_and_breaks_ties_by_entity` and
/// asserting the opposite outcome (`Plan/clustered-forward.md`). A top-down camera over a
/// ground plane: sixteen lights crowd the origin — exactly the set the deleted nearest-16
/// selection would have kept — and lights **17 and 18** sit far out on either side, each with a
/// range that reaches its own patch of ground and nothing else.
///
/// Two claims, and they are the plan's identity argument made testable:
///
/// - The 17th and 18th lights **visibly contribute**. Under the old cap they were dropped
///   outright (farthest from the camera's ground target); now their patches are brighter than
///   the same frame rendered with only the first sixteen.
/// - Adding them leaves the crowded centre **byte-identical**. Their attenuation is a hard zero
///   at `dist >= range`, and the centre is far outside both ranges, so each contributes exactly
///   `+0.0` to that fragment's accumulator — bit-preserving in IEEE-754. That is precisely why a
///   conservative cluster build (the degenerate one included) cannot move a pixel.
#[test]
fn needs_gpu_lights_past_the_old_cap_light_their_own_patches_and_leave_the_rest_exact() {
    let Some(renderer) = skip_or_renderer(
        "lights_past_the_old_cap_light_their_own_patches_and_leave_the_rest_exact",
    ) else {
        return;
    };
    let (w, h) = (192u32, 108u32);
    let size = FrameSize::native(w, h);

    let mut scene = RenderScene::empty((w, h));
    scene.ambient = [0.01, 0.01, 0.01];
    let position = Vec3::new(0.0, 40.0, 0.01);
    scene.camera = crate::scene::RenderCamera {
        view: Mat4::look_at_rh(position, Vec3::ZERO, Vec3::Y),
        projection: Mat4::perspective_rh(
            60.0_f32.to_radians(),
            crate::scene::aspect_ratio((w, h)),
            0.1,
            1000.0,
        ),
        position,
    };
    scene.meshes = vec![ground(60.0)];

    // Sixteen lights crowded within a couple of units of the origin — the old cap's whole budget.
    let mut lights: Vec<crate::scene::PointLightData> = (0..16)
        .map(|i| point_light(Vec3::new(i as f32 * 0.1 - 0.8, 3.0, 0.0), 0.6, 12.0))
        .collect();
    // The two the cap could never express, each reaching only its own patch (range 8 << 28).
    const FAR: [Vec3; 2] = [Vec3::new(28.0, 2.0, 0.0), Vec3::new(-28.0, 2.0, 0.0)];
    scene.lights = lights.clone();
    let capped = renderer.render(&scene, size).expect("sixteen-light render");

    lights.extend(FAR.iter().map(|&p| point_light(p, 6.0, 8.0)));
    scene.lights = lights;
    assert_eq!(scene.lights.len(), 18);
    let uncapped = renderer
        .render(&scene, size)
        .expect("eighteen-light render");

    for far in FAR {
        let at = project(&scene.camera, far.with_y(0.0), w, h);
        let before = luma(&capped, at.0, at.1);
        let after = luma(&uncapped, at.0, at.1);
        assert!(
            after > before + 30,
            "the light at {far:?} must light its own patch at {at:?}: {before} -> {after}",
        );
    }

    // The crowded centre is outside both far lights' ranges, so they add exactly +0.0 there.
    let centre = project(&scene.camera, Vec3::ZERO, w, h);
    assert_eq!(
        rgb_at(&capped, centre.0, centre.1),
        rgb_at(&uncapped, centre.0, centre.1),
        "a light whose range does not reach a fragment must add exactly +0.0 to it",
    );
    // …and the centre is genuinely lit, so that equality is not two black pixels agreeing.
    assert!(
        luma(&capped, centre.0, centre.1) > 60,
        "the sixteen crowded lights must actually light the centre",
    );
}

/// Sprite parity: `sprite.wgsl`'s `shade_two_sided` walks the same cluster slice the mesh path
/// does, so a lit sprite sees lights past the old sixteen too. A billboard quad ringed by
/// eighteen point lights renders differently from the same quad under only the first sixteen —
/// and both differ from the unlit quad, so neither half of the comparison is a dark frame.
#[test]
fn needs_gpu_a_lit_sprite_sees_lights_past_the_old_cap() {
    let Some(renderer) = skip_or_renderer("a_lit_sprite_sees_lights_past_the_old_cap") else {
        return;
    };
    let (w, h) = (96u32, 96u32);
    let size = FrameSize::native(w, h);

    let mut scene = RenderScene::empty((w, h));
    scene.ambient = [0.02, 0.02, 0.02];
    scene.camera = origin_camera(w, h);
    // An untextured (white) billboard four units down −Z, filling a good part of the frame.
    scene.sprites = vec![crate::scene::SpriteInstance {
        center: Vec3::new(0.0, 0.0, -4.0),
        right: Vec3::new(3.0, 0.0, 0.0),
        up: Vec3::new(0.0, 3.0, 0.0),
        color: [1.0, 1.0, 1.0, 1.0],
        texture: None,
        uv: crate::scene::SpriteInstance::FULL_UV,
        lit: true,
        layer: 0,
        additive: false,
        flash: 0.0,
        flash_color: [1.0; 3],
        uv_repeat: crate::scene::SpriteInstance::NO_TILING,
        uv_offset: [0.0; 2],
        medium: 1.0,
    }];

    let unlit = renderer.render(&scene, size).expect("unlit sprite render");

    // Eighteen lights on a ring in front of the quad — every one of them reaches it. The ring
    // sits 1.5 units nearer the camera than the quad on purpose: a ring in the quad's own plane
    // would be exactly edge-on to its normal, and `ndl` would be zero for all eighteen.
    let ring: Vec<crate::scene::PointLightData> = (0..18)
        .map(|i| {
            let a = i as f32 * (std::f32::consts::TAU / 18.0);
            point_light(Vec3::new(2.5 * a.cos(), 2.5 * a.sin(), -2.5), 0.5, 6.0)
        })
        .collect();

    scene.lights = ring[..16].to_vec();
    let capped = renderer.render(&scene, size).expect("sixteen-light sprite");
    scene.lights = ring;
    let uncapped = renderer
        .render(&scene, size)
        .expect("eighteen-light sprite");

    assert_ne!(
        unlit.rgba, capped.rgba,
        "the fixture's sprite must be lit at all, or the comparison below proves nothing",
    );
    assert_ne!(
        capped.rgba, uncapped.rgba,
        "the seventeenth and eighteenth lights must reach a sprite, not just a mesh",
    );
}

/// Any light population renders, including well past the old cap and — the wgpu-shaped edge case
/// — none at all: an empty light array still has to bind, because a zero-sized storage binding is
/// a validation error and the generated `min_binding_size` is one element. The zero case pads to
/// one zeroed element with every cell's count at 0, so the shader never reads the padding; this
/// pins that it renders rather than panicking, and that it is byte-stable across renders.
#[test]
fn needs_gpu_every_light_population_renders_including_none_at_all() {
    let Some(renderer) = skip_or_renderer("every_light_population_renders_including_none_at_all")
    else {
        return;
    };
    let (w, h) = (64u32, 64u32);
    let size = FrameSize::native(w, h);
    let mut scene = RenderScene::empty((w, h));
    scene.ambient = [0.05, 0.05, 0.05];
    scene.meshes = vec![
        ground(20.0),
        cube(Vec3::new(0.0, 1.0, 0.0), Vec3::splat(1.5), [0.8; 4]),
    ];

    for count in [0usize, 1, 16, 17, 64, 200] {
        scene.lights = (0..count)
            .map(|i| {
                let a = i as f32 * (std::f32::consts::TAU / 32.0);
                point_light(Vec3::new(5.0 * a.cos(), 3.0, 5.0 * a.sin()), 0.2, 9.0)
            })
            .collect();
        let first = renderer
            .render(&scene, size)
            .unwrap_or_else(|d| panic!("{count} lights must render: {d:?}"));
        let second = renderer.render(&scene, size).expect("second render");
        assert_eq!(
            first.rgba, second.rgba,
            "{count} lights must render byte-identically twice",
        );
    }
}

// --- Clustered forward: the real build (Plan/clustered-forward.md) --------------------

/// **The conformance gate** (`Plan/clustered-forward.md` decision 7), and the permanent home of
/// the degenerate build: a randomized clustered-vs-degenerate byte-compare.
///
/// The degenerate build files every light into every cell — no sphere test can be more
/// conservative than admitting everything — so it is the oracle the culled build is scored
/// against. Rendering the same scene both ways *through the same pipeline* and demanding
/// **byte** equality checks both halves of the identity argument at once: a light the culled
/// build wrongly dropped shows as a hole, and every light it admits spuriously is proven
/// harmless by the same comparison passing (attenuation is a hard zero at `dist >= range`, and
/// `acc + 0.0` is bit-preserving).
///
/// The camera set is chosen for the build's fallback paths, not for prettiness: lights sitting
/// *behind* the camera and lights whose spheres *span the near plane* both break the projection's
/// convex-hull precondition and must take the full-tile fallback. Populations run past the old
/// sixteen-light cap in both directions, point-only and with spots mixed in (whose cone factor
/// is the other exact zero a spurious admission must hit).
#[test]
fn needs_gpu_the_culled_build_renders_byte_identically_to_the_degenerate_oracle() {
    let Some(renderer) =
        skip_or_renderer("the_culled_build_renders_byte_identically_to_the_degenerate_oracle")
    else {
        return;
    };
    let (w, h) = (160u32, 90u32);
    let size = FrameSize::native(w, h);

    let look = |eye: Vec3, target: Vec3| crate::scene::RenderCamera {
        view: Mat4::look_at_rh(eye, target, Vec3::Y),
        projection: Mat4::perspective_rh(
            60.0_f32.to_radians(),
            crate::scene::aspect_ratio((w, h)),
            0.1,
            1000.0,
        ),
        position: eye,
    };

    /// One camera plus the shape of the random light cloud rendered under it.
    struct OracleCase {
        name: &'static str,
        camera: crate::scene::RenderCamera,
        /// Where the cloud is centred.
        centre: Vec3,
        /// Half-extent of the cloud on X/Z (Y gets a fifth of it — lights stay near the ground).
        extent: f32,
        /// The span the random light ranges are drawn from.
        range: (f32, f32),
    }

    let cases = [
        // Ordinary framing: most lights in front, the cloud wider than the frustum.
        OracleCase {
            name: "overhead",
            camera: look(Vec3::new(0.0, 24.0, 26.0), Vec3::ZERO),
            centre: Vec3::new(0.0, 2.0, 0.0),
            extent: 24.0,
            range: (4.0, 14.0),
        },
        // Ground level inside the cloud: roughly half the population sits *behind* the camera,
        // which is the full-tile fallback's main path.
        OracleCase {
            name: "half the cloud behind the camera",
            camera: look(Vec3::new(0.0, 2.5, 0.0), Vec3::new(0.0, 2.0, -20.0)),
            centre: Vec3::new(0.0, 2.0, 0.0),
            extent: 22.0,
            range: (5.0, 15.0),
        },
        // Lights packed around the camera itself: every sphere straddles the near plane.
        OracleCase {
            name: "spheres spanning the near plane",
            camera: look(Vec3::new(0.0, 2.5, 4.0), Vec3::new(0.0, 1.5, -20.0)),
            centre: Vec3::new(0.0, 2.5, 4.0),
            extent: 3.0,
            range: (3.0, 9.0),
        },
        // An oblique camera, so no tile or shell boundary lines up with a world axis.
        OracleCase {
            name: "oblique",
            camera: look(Vec3::new(-15.0, 7.0, 13.0), Vec3::new(3.0, 0.0, -2.0)),
            centre: Vec3::ZERO,
            extent: 20.0,
            range: (3.0, 12.0),
        },
    ];

    let mut rng = ludion_core::rng::DeterministicRng::new(0x0C_1057_E5ED);
    for OracleCase {
        name,
        camera,
        centre,
        extent,
        range: (range_lo, range_hi),
    } in cases
    {
        let mut scene = RenderScene::empty((w, h));
        scene.ambient = [0.02, 0.02, 0.03];
        scene.camera = camera;
        // Enough geometry that both lit consumers are exercised: meshes for `mesh.wgsl`'s
        // `shade` and a billboard for `sprite.wgsl`'s `shade_two_sided`.
        scene.meshes = vec![ground(50.0)];
        for i in 0..8 {
            let a = i as f32 * (std::f32::consts::TAU / 8.0);
            scene.meshes.push(cube(
                Vec3::new(9.0 * a.cos(), 1.2, 9.0 * a.sin()),
                Vec3::splat(2.4),
                [0.7, 0.65, 0.6, 1.0],
            ));
        }
        scene.sprites = vec![crate::scene::SpriteInstance {
            center: Vec3::new(0.0, 2.5, -6.0),
            right: Vec3::new(3.0, 0.0, 0.0),
            up: Vec3::new(0.0, 3.0, 0.0),
            color: [1.0, 1.0, 1.0, 1.0],
            texture: None,
            uv: crate::scene::SpriteInstance::FULL_UV,
            lit: true,
            layer: 0,
            additive: false,
            flash: 0.0,
            flash_color: [1.0; 3],
            uv_repeat: crate::scene::SpriteInstance::NO_TILING,
            uv_offset: [0.0; 2],
            medium: 1.0,
        }];

        let unlit = renderer
            .render(&scene, size)
            .unwrap_or_else(|d| panic!("{name}: light-free render: {d:?}"));

        // Point-only populations, then the same sizes with spots mixed in.
        for (population, spots) in [
            (8usize, 0usize),
            (64, 0),
            (300, 0),
            (8, 6),
            (64, 40),
            (300, 120),
        ] {
            scene.lights = (0..population)
                .map(|_| {
                    let axis = |rng: &mut ludion_core::rng::DeterministicRng| {
                        (rng.next_f32() - 0.5) * 2.0 * extent
                    };
                    crate::scene::PointLightData {
                        position: centre
                            + Vec3::new(axis(&mut rng), axis(&mut rng) * 0.2, axis(&mut rng)),
                        color: Vec3::new(
                            0.4 + rng.next_f32(),
                            0.4 + rng.next_f32(),
                            0.4 + rng.next_f32(),
                        ),
                        intensity: 0.15 + rng.next_f32() * 0.35,
                        range: range_lo + rng.next_f32() * (range_hi - range_lo),
                        dynamic: false,
                        shadow: None,
                        scatter: 0.0,
                    }
                })
                .collect();
            // Narrow, mid, wide (> 45°, the range-sphere fallback) and hard-edged cones,
            // pointing anywhere from straight down to sideways.
            scene.spot_lights = (0..spots)
                .map(|i| {
                    let mut r = || rng.next_f32();
                    let position = centre
                        + Vec3::new(
                            (r() - 0.5) * 2.0 * extent,
                            (r() - 0.5) * 0.4 * extent + 2.0,
                            (r() - 0.5) * 2.0 * extent,
                        );
                    let direction =
                        Vec3::new(r() * 2.0 - 1.0, -0.2 - r() * 1.5, r() * 2.0 - 1.0).normalize();
                    let (inner, outer) = match i % 4 {
                        0 => (2.0 + r() * 6.0, 10.0 + r() * 10.0),
                        1 => (20.0, 30.0 + r() * 14.0),
                        2 => (40.0, 50.0 + r() * 35.0),
                        _ => {
                            let edge = 10.0 + r() * 40.0;
                            (edge, edge)
                        }
                    };
                    crate::scene::SpotLightData {
                        position,
                        direction,
                        color: Vec3::new(0.4 + r(), 0.4 + r(), 0.4 + r()),
                        intensity: 0.4 + r() * 0.8,
                        range: range_lo + r() * (range_hi - range_lo) * 1.5,
                        cos_inner: f32::to_radians(inner).cos(),
                        cos_outer: f32::to_radians(outer).cos(),
                        shadow: None,
                        scatter: 0.0,
                    }
                })
                .collect();
            let population = format!("{population}+{spots} spots");

            renderer.cluster_build.set(ClusterBuild::Culled);
            let culled = renderer
                .render(&scene, size)
                .unwrap_or_else(|d| panic!("{name}/{population}: culled render: {d:?}"));
            renderer.cluster_build.set(ClusterBuild::Degenerate);
            let oracle = renderer
                .render(&scene, size)
                .unwrap_or_else(|d| panic!("{name}/{population}: oracle render: {d:?}"));
            renderer.cluster_build.set(ClusterBuild::Culled);

            assert_ne!(
                oracle.rgba, unlit.rgba,
                "{name}/{population}: the lights must actually light something, or the \
                 comparison below is two identical dark frames agreeing",
            );
            assert_eq!(
                culled.rgba, oracle.rgba,
                "{name}/{population}: the culled build must be byte-identical to the \
                 all-lights-in-every-cell oracle — a difference is a conservatism hole",
            );
        }
    }
}

// --- Spot lights (Plan/spot-light.md) ---------------------------------------------------

/// A spot of the fixtures' shared white shape.
fn spot_light(
    position: Vec3,
    direction: Vec3,
    intensity: f32,
    range: f32,
    inner_deg: f32,
    outer_deg: f32,
) -> crate::scene::SpotLightData {
    crate::scene::SpotLightData {
        position,
        direction: direction.normalize(),
        color: Vec3::ONE,
        intensity,
        range,
        cos_inner: inner_deg.to_radians().cos(),
        cos_outer: outer_deg.to_radians().cos(),
        shadow: None,
        scatter: 0.0,
    }
}

/// A downward spot over a ground plane lights a pool under its axis, and a ground point within
/// its range but outside its outer cone renders **exactly** the spot-free value — the cone
/// factor is a hard 0.0 there. Run for a soft edge and a hard edge (`inner == outer`, the
/// no-divide path).
#[test]
fn needs_gpu_a_spot_lights_its_pool_and_nothing_outside_its_cone() {
    let Some(renderer) = skip_or_renderer("a_spot_lights_its_pool_and_nothing_outside_its_cone")
    else {
        return;
    };
    let (w, h) = (192u32, 108u32);
    let size = FrameSize::native(w, h);

    let mut scene = RenderScene::empty((w, h));
    scene.ambient = [0.01, 0.01, 0.01];
    let eye = Vec3::new(0.0, 40.0, 0.01);
    scene.camera = crate::scene::RenderCamera {
        view: Mat4::look_at_rh(eye, Vec3::ZERO, Vec3::Y),
        projection: Mat4::perspective_rh(
            60.0_f32.to_radians(),
            crate::scene::aspect_ratio((w, h)),
            0.1,
            1000.0,
        ),
        position: eye,
    };
    scene.meshes = vec![ground(60.0)];
    let unlit = renderer.render(&scene, size).expect("spot-free render");

    // Apex 10 up, outer 20°: the pool's rim is ~3.6 from the axis. (8, 0, 0) is 12.8 from the
    // apex (inside range 20) at ~39° off the axis (outside the cone).
    let pool = project(&scene.camera, Vec3::ZERO, w, h);
    let outside = project(&scene.camera, Vec3::new(8.0, 0.0, 0.0), w, h);
    for (name, inner) in [("soft", 15.0), ("hard", 20.0)] {
        scene.spot_lights = vec![spot_light(
            Vec3::new(0.0, 10.0, 0.0),
            Vec3::NEG_Y,
            3.0,
            20.0,
            inner,
            20.0,
        )];
        let lit = renderer.render(&scene, size).expect("spot render");
        assert!(
            luma(&lit, pool.0, pool.1) > luma(&unlit, pool.0, pool.1) + 30,
            "{name}: the spot must light the pool under its axis",
        );
        assert_eq!(
            rgb_at(&lit, outside.0, outside.1),
            rgb_at(&unlit, outside.0, outside.1),
            "{name}: a fragment outside the outer cone must receive exactly +0.0",
        );
    }
}

/// Sprite parity: `shade_two_sided` applies the same cone. A lit billboard with a spot aimed at
/// its centre is brighter there, and its corner — outside the outer cone — is untouched.
#[test]
fn needs_gpu_a_lit_sprite_inside_a_spot_cone_is_lit() {
    let Some(renderer) = skip_or_renderer("a_lit_sprite_inside_a_spot_cone_is_lit") else {
        return;
    };
    let (w, h) = (96u32, 96u32);
    let size = FrameSize::native(w, h);

    let mut scene = RenderScene::empty((w, h));
    scene.ambient = [0.02, 0.02, 0.02];
    scene.camera = origin_camera(w, h);
    scene.sprites = vec![crate::scene::SpriteInstance {
        center: Vec3::new(0.0, 0.0, -4.0),
        right: Vec3::new(3.0, 0.0, 0.0),
        up: Vec3::new(0.0, 3.0, 0.0),
        color: [1.0, 1.0, 1.0, 1.0],
        texture: None,
        uv: crate::scene::SpriteInstance::FULL_UV,
        lit: true,
        layer: 0,
        additive: false,
        flash: 0.0,
        flash_color: [1.0; 3],
        uv_repeat: crate::scene::SpriteInstance::NO_TILING,
        uv_offset: [0.0; 2],
        medium: 1.0,
    }];
    let unlit = renderer.render(&scene, size).expect("spot-free sprite");

    // Apex 2.5 in front of the quad, outer 25°: the corner sample is ~36° off the axis.
    scene.spot_lights = vec![spot_light(
        Vec3::new(0.0, 0.0, -1.5),
        Vec3::NEG_Z,
        2.0,
        6.0,
        15.0,
        25.0,
    )];
    let lit = renderer.render(&scene, size).expect("spot-lit sprite");

    let centre = project(&scene.camera, Vec3::new(0.0, 0.0, -4.0), w, h);
    let corner = project(&scene.camera, Vec3::new(1.3, 1.3, -4.0), w, h);
    assert!(
        luma(&lit, centre.0, centre.1) > luma(&unlit, centre.0, centre.1) + 30,
        "a sprite inside the cone must be lit",
    );
    assert_eq!(
        rgb_at(&lit, corner.0, corner.1),
        rgb_at(&unlit, corner.0, corner.1),
        "the sprite's corner, outside the outer cone, must receive exactly +0.0",
    );
}

/// Each pipeline's hand-written vertex attribute layout matches the WGSL-reflected `@location`
/// inputs generated into `shader_gen` (Plan/shader-bindgen.md decision 3, the retained half of the
/// retired vertex scaffolding). The reflected side is generated from the WGSL (regenerated
/// and drift-gated); this comparison is the hand-owned bridge to the pipeline attribute literals,
/// so changing either side without the other fails here instead of corrupting a rendered frame.
#[test]
fn vertex_inputs_match_generated_reflection() {
    fn locs_formats(attrs: &[wgpu::VertexAttribute]) -> Vec<(u32, wgpu::VertexFormat)> {
        attrs
            .iter()
            .map(|a| (a.shader_location, a.format))
            .collect()
    }

    // Mesh feeds two buffers (per-vertex 0-5 + 18-19, per-instance 6-17) into one `vs_main`; the
    // reflected side is in ascending location order regardless of which buffer declares what, so
    // the two-buffer reconstruction is sorted the same way before comparing.
    let mut mesh = locs_formats(&MESH_VERTEX_ATTRS);
    mesh.extend(locs_formats(&MESH_INSTANCE_ATTRS));
    mesh.sort_by_key(|(location, _)| *location);
    assert_eq!(mesh.as_slice(), crate::shader_gen::MESH_VS_MAIN_INPUTS);
    // Shadow reads the same buffers but only position (0), the model rows (6-9), the bone-palette
    // offset lane (16) and the skin weights/bones (18-19).
    let mut shadow = locs_formats(&SHADOW_VERTEX_ATTRS);
    shadow.extend(locs_formats(&SHADOW_INSTANCE_ATTRS));
    shadow.sort_by_key(|(location, _)| *location);
    assert_eq!(shadow.as_slice(), crate::shader_gen::SHADOW_VS_MAIN_INPUTS);
    assert_eq!(
        locs_formats(&GROUND_MARKER_INSTANCE_ATTRS).as_slice(),
        crate::shader_gen::GROUND_MARKER_VS_MAIN_INPUTS,
    );
    assert_eq!(
        locs_formats(&SPRITE_INSTANCE_ATTRS).as_slice(),
        crate::shader_gen::SPRITE_VS_MAIN_INPUTS,
    );
    assert_eq!(
        locs_formats(&PARTICLE_INSTANCE_ATTRS).as_slice(),
        crate::shader_gen::PARTICLE_VS_MAIN_INPUTS,
    );
    assert_eq!(
        locs_formats(&TEXT_VERTEX_ATTRS).as_slice(),
        crate::shader_gen::TEXT_VS_MAIN_INPUTS,
    );
    assert_eq!(
        locs_formats(&UI_RECT_INSTANCE_ATTRS).as_slice(),
        crate::shader_gen::UI_RECT_VS_MAIN_INPUTS,
    );
    assert_eq!(
        locs_formats(&UI_TEXT_VERTEX_ATTRS).as_slice(),
        crate::shader_gen::UI_TEXT_VS_MAIN_INPUTS,
    );
    // The prepass reuses the mesh arrays verbatim rather than declaring a third pair, so this
    // pins that reuse: `prepass.wgsl` must keep declaring all twenty locations, in the mesh's
    // formats, however few of them its body reads.
    assert_eq!(mesh.as_slice(), crate::shader_gen::PREPASS_VS_MAIN_INPUTS);
    assert_eq!(
        mesh.as_slice(),
        crate::shader_gen::SHADOW_CUTOUT_VS_MAIN_INPUTS
    );
    // The decal receiver pass reuses them the same way.
    assert_eq!(
        mesh.as_slice(),
        crate::shader_gen::DECAL_RECEIVER_VS_MAIN_INPUTS
    );
}

/// The eight `(mirrored, double_sided, cutout)` caster keys each pick their own pipeline slot, and
/// the default key is the depth-only pass's main pipeline, the one every caster pass binds first.
#[test]
fn caster_keys_map_to_distinct_pipeline_slots() {
    let mut slots = Vec::new();
    for bits in 0..8u8 {
        let key = CasterPipeline {
            variant: MeshVariant {
                mirrored: bits & 1 != 0,
                double_sided: bits & 2 != 0,
            },
            cutout: bits & 4 != 0,
        };
        slots.push((caster_shader(key) as usize, key.variant.index()));
    }
    slots.sort_unstable();
    slots.dedup();
    assert_eq!(slots.len(), 8, "every caster key owns a pipeline slot");

    let default = CasterPipeline::default();
    assert_eq!(
        (caster_shader(default), default.variant.index()),
        (ShaderId::Shadow, 0),
        "the all-false key is the Shadow main pipeline",
    );
}

/// **A marker-free scene packs no instances**, which is what `build_ground_marker_draw` turns
/// into no buffer, no pipeline bound and no draw — the golden-neutrality guarantee stated at the
/// only place it can be checked without an adapter.
#[test]
fn a_marker_free_scene_packs_no_ground_marker_instances() {
    let scene = RenderScene::empty((320, 200));
    assert!(scene.ground_markers.is_empty());
    assert!(ground_marker_raws(&scene.ground_markers).is_empty());
}

/// Both shapes collapse onto one rounded-box SDF, in the marker's declared order, with the model
/// matrix in the mesh pass's column packing and the HDR colour carried verbatim (so a bright
/// telegraph reaches the bloom chain unclamped).
#[test]
fn ground_markers_pack_both_shapes_onto_one_sdf() {
    use crate::scene::{GroundMarkerInstance, GroundMarkerShape, GroundMarkerStyle};

    let placement = Mat4::from_translation(Vec3::new(2.0, crate::GROUND_MARKER_LIFT, -5.0));
    let markers = [
        GroundMarkerInstance {
            shape: GroundMarkerShape::Disc { radius: 3.0 },
            model: placement,
            color: [6.0, 0.5, 0.25, 0.75],
            inner_color: [6.0, 0.5, 0.25, 0.75],
            style: GroundMarkerStyle::default(),
            flicker_seed: 0,
            clip: None,
        },
        GroundMarkerInstance {
            shape: GroundMarkerShape::Segment {
                length: 8.0,
                width: 2.0,
            },
            model: Mat4::IDENTITY,
            color: [0.1, 0.2, 0.3, 1.0],
            inner_color: [0.1, 0.2, 0.3, 1.0],
            style: GroundMarkerStyle::default(),
            flicker_seed: 0,
            clip: None,
        },
    ];
    let raws = ground_marker_raws(&markers);

    assert_eq!(raws.len(), 2, "one instance per marker, in scene order");
    // A disc is a zero-extent box whose corner radius is the whole shape.
    assert_eq!(raws[0].shape, [0.0, 0.0, 3.0, 0.0]);
    assert_eq!(raws[0].wedge, [0.0; 4]);
    assert_eq!(raws[0].model, placement.to_cols_array_2d());
    assert_eq!(raws[0].color, [6.0, 0.5, 0.25, 0.75]);
    // An 8 x 2 segment is a 6-long straight section with 1-radius caps: the drawn footprint is
    // exactly the authored length and width.
    assert_eq!(raws[1].shape, [3.0, 0.0, 1.0, 0.0]);
    assert_eq!(raws[1].model, Mat4::IDENTITY.to_cols_array_2d());
    // A default style packs today's constants, and a disc's extra lane is the "no inner zone" one.
    assert_eq!(raws[0].style_a, [1.5, 1.0, 0.45, 0.06]);
    assert_eq!(raws[0].style_b, [0.12, 0.30, 0.0, 0.0]);
    assert_eq!(raws[0].extra, [-1.0, 0.0, 0.0, 0.0]);
}

/// A ring packs kind 2 with `(thickness, fill, dashes)` in the extra lane and no wedge lane.
#[test]
fn a_ground_marker_ring_packs_its_lane() {
    use crate::scene::{GroundMarkerInstance, GroundMarkerShape, GroundMarkerStyle};

    let raws = ground_marker_raws(&[GroundMarkerInstance {
        shape: GroundMarkerShape::Ring {
            radius: 4.0,
            thickness: 0.5,
            fill: 2.0,
            dashes: 12,
        },
        model: Mat4::IDENTITY,
        color: [1.0; 4],
        inner_color: [1.0; 4],
        style: GroundMarkerStyle::default(),
        flicker_seed: 0,
        clip: None,
    }]);
    assert_eq!(raws[0].shape, [0.0, 0.0, 4.0, 2.0]);
    assert_eq!(raws[0].wedge, [0.0; 4]);
    assert_eq!(raws[0].extra, [0.5, 1.0, 12.0, 0.0]);
    assert_eq!(raws[0].clip, [-1.0, 0.0, 0.0, 0.0]);
}

/// A clipped marker packs its profile's float offset in lane 11, exactly.
#[test]
fn a_ground_marker_clip_packs_its_profile_offset() {
    use crate::scene::{GroundMarkerInstance, GroundMarkerShape, GroundMarkerStyle};

    let marker = |clip| GroundMarkerInstance {
        shape: GroundMarkerShape::Disc { radius: 2.0 },
        model: Mat4::IDENTITY,
        color: [1.0; 4],
        inner_color: [1.0; 4],
        style: GroundMarkerStyle::default(),
        flicker_seed: 0,
        clip,
    };
    let raws = ground_marker_raws(&[marker(Some(0)), marker(Some(200)), marker(None)]);
    assert_eq!(raws[0].clip, [0.0, 0.0, 0.0, 0.0]);
    assert_eq!(raws[1].clip, [200.0, 0.0, 0.0, 0.0]);
    assert_eq!(raws[2].clip, [-1.0, 0.0, 0.0, 0.0]);
}

/// The two-zone half-angle rides `extra.x`, `grid: none` packs a zero grid width, and the seed
/// is masked to 24 bits so it is an exact float integer.
#[test]
fn a_ground_marker_packs_zones_grid_and_flicker() {
    use crate::scene::{GroundMarkerInstance, GroundMarkerShape, GroundMarkerStyle};

    let raws = ground_marker_raws(&[GroundMarkerInstance {
        shape: GroundMarkerShape::Wedge {
            half_angle: 0.7,
            inner_radius: 3.0,
            outer_radius: 7.0,
            fill: 0.0,
            grid_spacing: 1.0,
            inner_half_angle: Some(0.3),
        },
        model: Mat4::IDENTITY,
        color: [1.0; 4],
        inner_color: [1.0, 0.0, 0.0, 1.0],
        style: GroundMarkerStyle {
            grid: false,
            flicker: 0.5,
            ..GroundMarkerStyle::default()
        },
        flicker_seed: 0xABCD_EF12,
        clip: None,
    }]);
    assert_eq!(raws[0].extra, [0.3, 0.0, 0.0, 0.0]);
    assert_eq!(raws[0].inner_color, [1.0, 0.0, 0.0, 1.0]);
    assert_eq!(raws[0].style_a[1], 0.0);
    assert_eq!(raws[0].style_b[2], 0.5);
    assert_eq!(raws[0].style_b[3], (0xCD_EF12_u32) as f32);
}

/// A wedge packs its bounding disc with the wedge flag in `shape.w`, and its angle, inner
/// radius, (clamped) fill and grid spacing in the wedge lane.
#[test]
fn a_ground_marker_wedge_packs_its_lane() {
    use crate::scene::{GroundMarkerInstance, GroundMarkerShape, GroundMarkerStyle};

    let raws = ground_marker_raws(&[GroundMarkerInstance {
        shape: GroundMarkerShape::Wedge {
            half_angle: 0.5,
            inner_radius: 3.0,
            outer_radius: 7.0,
            fill: 0.25,
            grid_spacing: 1.0,
            inner_half_angle: None,
        },
        model: Mat4::IDENTITY,
        color: [0.25, 1.6, 0.6, 0.9],
        inner_color: [0.25, 1.6, 0.6, 0.9],
        style: GroundMarkerStyle::default(),
        flicker_seed: 0,
        clip: None,
    }]);
    assert_eq!(raws[0].shape, [0.0, 0.0, 7.0, 1.0]);
    assert_eq!(raws[0].wedge, [0.5, 3.0, 0.25, 1.0]);
    assert_eq!(raws[0].extra, [-1.0, 0.0, 0.0, 0.0]);
}

/// A segment at least as wide as it is long degenerates to a disc of radius `length / 2` rather
/// than to a negative half-extent the SDF would read as a hole.
#[test]
fn an_over_wide_segment_degenerates_to_a_disc() {
    use crate::scene::GroundMarkerShape;

    assert_eq!(
        GroundMarkerShape::Segment {
            length: 2.0,
            width: 5.0,
        }
        .rounded_box(),
        ([0.0, 0.0], 1.0),
    );
}

/// The particle fixture: a lit ground plane under the default framing, so the background the
/// discs draw over is neither black nor uniform.
fn particle_scene(w: u32, h: u32) -> RenderScene {
    let mut scene = RenderScene::empty((w, h));
    scene.ambient = [0.35, 0.35, 0.4];
    scene.meshes = vec![ground(20.0)];
    scene
}

/// One camera-facing disc at `center`, billboarded along the world axes (the default framing
/// looks down −Z, so those *are* its camera basis).
fn particle(center: Vec3, size: f32, color: [f32; 4], softness: f32) -> ParticleInstance {
    ParticleInstance {
        center,
        right: Vec3::X,
        up: Vec3::Y,
        size,
        color,
        softness,
        additive: false,
        lit: false,
        viewmodel: false,
        medium: 1.0,
    }
}

/// A scene with no viewmodel particle packs exactly the world list and starts the viewmodel
/// ones at its end; a flagged one packs after every world one, each list sorted on its own.
#[test]
fn viewmodel_particles_pack_after_the_world_ones() {
    let view = Mat4::IDENTITY;
    let world = [
        particle(Vec3::new(0.0, 0.0, -1.0), 1.0, [1.0; 4], 0.0),
        particle(Vec3::new(0.0, 0.0, -9.0), 2.0, [1.0; 4], 0.0),
    ];
    let (raws, start) = particle_raws_split(&view, &world);
    let bytes = |raws: &[ParticleInstanceRaw]| bytemuck::cast_slice::<_, u8>(raws).to_vec();
    assert_eq!(bytes(&raws), bytes(&particle_raws(&view, &world)));
    assert_eq!(start, world.len());

    let mut mixed = vec![
        ParticleInstance {
            viewmodel: true,
            ..particle(Vec3::new(0.0, 0.0, -0.3), 3.0, [1.0; 4], 0.0)
        },
        world[0],
        ParticleInstance {
            viewmodel: true,
            ..particle(Vec3::new(0.0, 0.0, -0.6), 4.0, [1.0; 4], 0.0)
        },
        world[1],
    ];
    let (raws, start) = particle_raws_split(&view, &mixed);
    let sizes: Vec<f32> = raws.iter().map(|r| r.params[0]).collect();
    assert_eq!((start, sizes), (2, vec![2.0, 1.0, 4.0, 3.0]));
    mixed.retain(|p| !p.viewmodel);
    assert_eq!(bytes(&raws[..2]), bytes(&particle_raws(&view, &mixed)));
}

/// Particles **draw, and draw deterministically, and cost nothing when there are none**: (a) a
/// mixed alpha/additive, mixed-softness handful changes the pixels a particle-free frame has;
/// (b) that frame renders byte-identically twice, so the sort and the one instanced draw are as
/// deterministic as the rest of the pass; (c) clearing the list restores the bare frame's exact
/// bytes, which is `build_particle_draw`'s `None` leaving the recorded command stream the
/// pre-particle one — the golden-neutrality guarantee at the only place a GPU can check it.
/// Skips with no GPU.
#[test]
fn needs_gpu_particles_draw_and_are_deterministic() {
    let Some(renderer) = skip_or_renderer("particles_draw_and_are_deterministic") else {
        return;
    };
    let (w, h) = (64u32, 64u32);
    let size = FrameSize::native(w, h);

    let mut scene = particle_scene(w, h);
    let bare = renderer.render(&scene, size).expect("particle-free render");

    scene.particles = vec![
        particle(Vec3::new(-1.0, 0.6, 0.0), 1.2, [0.9, 0.3, 0.1, 0.8], 0.0),
        particle(Vec3::new(0.0, 0.9, -1.0), 1.6, [0.2, 0.5, 0.9, 0.6], 1.0),
        particle(Vec3::new(1.1, 0.4, 0.5), 0.9, [1.0, 0.9, 0.6, 1.0], 0.35),
    ];
    scene.particles[1].additive = true;
    scene.particles[2].additive = true;

    // (a) they draw.
    let once = renderer.render(&scene, size).expect("particle render");
    assert_ne!(
        once.rgba, bare.rgba,
        "a handful of particles must change the pixels they cover",
    );

    // (b) twice, byte for byte.
    let twice = renderer
        .render(&scene, size)
        .expect("particle render again");
    assert_eq!(once.rgba, twice.rgba, "particle rendering is deterministic");

    // (c) and an empty list is the pre-particle frame exactly.
    scene.particles.clear();
    let cleared = renderer.render(&scene, size).expect("cleared render");
    assert_eq!(
        cleared.rgba, bare.rgba,
        "an empty particle list must bind no pipeline and issue no draw",
    );
}

/// **A particle-free scene packs no instances**, which is what `build_particle_draw` turns into
/// no buffer, no pipeline bound and no draw — stated where it can be checked without an adapter.
#[test]
fn a_particle_free_scene_packs_no_particle_instances() {
    let scene = RenderScene::empty((320, 200));
    assert!(scene.particles.is_empty());
    assert!(particle_raws(&scene.camera.view, &scene.particles).is_empty());
}

/// The draw order is back-to-front by view depth under a **total** order: finite depths sort
/// farthest-first, a NaN depth sorts by its sign bit (`f32::total_cmp` places a positive NaN
/// above every finite value and a negative one below, so either end is possible and the
/// platform picks; the quad draws nothing, so only the order is affected), and equal depths —
/// including two NaNs of one sign — fall through to the index tie-break. Totality is the
/// load-bearing half: a comparator that called a NaN equal to two depths unequal to each other
/// would be intransitive, which `sort_by` may answer with a panic and which would leave the
/// order up to the sort's internals either way.
#[test]
fn particles_sort_back_to_front_with_an_index_tie_break() {
    // Looking down −Z from z = 5: a smaller world z is farther, so a more negative view z.
    let view = Mat4::look_at_rh(Vec3::new(0.0, 0.0, 5.0), Vec3::ZERO, Vec3::Y);
    let at = |z: f32| particle(Vec3::new(0.0, 0.0, z), 1.0, [1.0; 4], 0.0);

    // Depths −1 (nearest), −15 (farthest), −5: farthest first.
    let mixed = [at(4.0), at(-10.0), at(0.0)];
    assert_eq!(particle_order(&view, &mixed), vec![1, 2, 0]);

    // Every depth NaN: they compare equal to each other, so the order is the extract's own.
    let nan = Vec3::new(f32::NAN, 0.0, 0.0);
    let all_nan = [
        particle(nan, 1.0, [1.0; 4], 0.0),
        particle(nan, 1.0, [1.0; 4], 0.0),
        particle(nan, 1.0, [1.0; 4], 0.0),
    ];
    assert_eq!(particle_order(&view, &all_nan), vec![0, 1, 2]);

    // One NaN among finite depths: it sorts past both to the top end, and the two finite ones
    // still sort back-to-front against each other.
    let one_nan = [at(4.0), at(-10.0), particle(nan, 1.0, [1.0; 4], 0.0)];
    assert_eq!(particle_order(&view, &one_nan), vec![1, 0, 2]);
}

/// A sprite at `center` on `layer`, everything else the plain unlit default.
fn sprite_at(center: Vec3, layer: i16) -> SpriteInstance {
    SpriteInstance {
        center,
        right: Vec3::X,
        up: Vec3::Y,
        color: [1.0; 4],
        texture: None,
        uv: SpriteInstance::FULL_UV,
        lit: false,
        layer,
        additive: false,
        flash: 0.0,
        flash_color: [1.0; 3],
        uv_repeat: SpriteInstance::NO_TILING,
        uv_offset: [0.0; 2],
        medium: 1.0,
    }
}

/// Sprites sort by `layer` first, then farthest-first within a layer, then scene order — and a
/// NaN centre sorts past every finite depth rather than comparing equal to all of them.
#[test]
fn sprites_sort_by_layer_then_back_to_front_with_an_index_tie_break() {
    let view = Mat4::look_at_rh(Vec3::new(0.0, 0.0, 5.0), Vec3::ZERO, Vec3::Y);
    let at = |z: f32| sprite_at(Vec3::new(0.0, 0.0, z), 0);

    // A higher layer draws after a lower one whatever the depth.
    let layers = [
        sprite_at(Vec3::new(0.0, 0.0, -10.0), 1),
        sprite_at(Vec3::new(0.0, 0.0, 4.0), 0),
    ];
    assert_eq!(sprite_order(&view, &layers), vec![1, 0]);

    // Within a layer the farthest comes first.
    assert_eq!(
        sprite_order(&view, &[at(4.0), at(-10.0), at(0.0)]),
        vec![1, 2, 0]
    );

    // Equal depths keep scene order.
    assert_eq!(
        sprite_order(&view, &[at(0.0), at(0.0), at(0.0)]),
        vec![0, 1, 2]
    );

    // One NaN among finite depths sorts to the far end of the key, past both.
    let nan = sprite_at(Vec3::new(f32::NAN, 0.0, 0.0), 0);
    assert_eq!(
        sprite_order(&view, &[nan, at(4.0), at(-10.0)]),
        vec![2, 1, 0]
    );
}

/// A NaN depth among many finite ones neither panics the sort nor disorders the finite sprites.
#[test]
fn a_nan_sprite_depth_neither_panics_nor_disorders_the_sort() {
    let view = Mat4::look_at_rh(Vec3::new(0.0, 0.0, 5.0), Vec3::ZERO, Vec3::Y);
    let sprites: Vec<SpriteInstance> = (0..64usize)
        .map(|i| {
            let z = ((i * 37) % 64) as f32 - 32.0;
            let x = if i % 3 == 0 { f32::NAN } else { 0.0 };
            sprite_at(Vec3::new(x, 0.0, z), 0)
        })
        .collect();

    // Under this view a smaller world z is farther; the NaN indices follow the finite ones.
    let mut finite: Vec<usize> = (0..64).filter(|i| i % 3 != 0).collect();
    finite.sort_by(|&a, &b| sprites[a].center.z.total_cmp(&sprites[b].center.z));
    let mut expected = finite;
    expected.extend((0..64).filter(|i| i % 3 == 0));

    assert_eq!(sprite_order(&view, &sprites), expected);
}

/// The camera sits on the +X side, so view depth is distance along -X and world z is
/// irrelevant: depths are -1, -15 and -5, farthest first is `[1, 2, 0]`, while sorting by world
/// z would give `[0, 2, 1]`. The on-axis fixtures elsewhere cannot tell the two apart, because
/// there view z is world z shifted.
#[test]
fn a_side_camera_sorts_by_view_depth_not_world_z() {
    let view = Mat4::look_at_rh(Vec3::new(5.0, 0.0, 0.0), Vec3::ZERO, Vec3::Y);
    let centers = [
        Vec3::new(4.0, 0.0, -10.0),
        Vec3::new(-10.0, 0.0, 4.0),
        Vec3::ZERO,
    ];

    let particles: Vec<ParticleInstance> = centers
        .iter()
        .map(|&c| particle(c, 1.0, [1.0; 4], 0.0))
        .collect();
    assert_eq!(particle_order(&view, &particles), vec![1, 2, 0]);

    let sprites: Vec<SpriteInstance> = centers.iter().map(|&c| sprite_at(c, 0)).collect();
    assert_eq!(sprite_order(&view, &sprites), vec![1, 2, 0]);
}

/// The additive flag is a real second blend mode on the one pipeline: (a) the same disc with the
/// flag flipped renders different bytes; (b) an additive white disc is nowhere darker than the
/// background it draws over, in any channel — which is exactly what withholding the output alpha
/// under `(One, OneMinusSrcAlpha)` buys, and what an alpha disc of the same colour would violate
/// wherever the background is brighter than the disc. Skips with no GPU.
#[test]
fn needs_gpu_additive_particles_only_ever_brighten() {
    let Some(renderer) = skip_or_renderer("additive_particles_only_ever_brighten") else {
        return;
    };
    let (w, h) = (64u32, 64u32);
    let size = FrameSize::native(w, h);

    let mut scene = particle_scene(w, h);
    let bare = renderer.render(&scene, size).expect("particle-free render");

    // Half-transparent white, so an *alpha* disc would pull the bright ground down toward it.
    scene.particles = vec![particle(
        Vec3::new(0.0, 0.5, 0.0),
        2.0,
        [1.0, 1.0, 1.0, 0.5],
        0.0,
    )];
    let alpha = renderer.render(&scene, size).expect("alpha render");
    scene.particles[0].additive = true;
    let additive = renderer.render(&scene, size).expect("additive render");

    // (a) the flag is not decorative.
    assert_ne!(
        alpha.rgba, additive.rgba,
        "flipping `additive` must change the bytes the same disc produces",
    );
    assert_ne!(
        additive.rgba, bare.rgba,
        "the disc must draw at all, or (b) below is two identical frames agreeing",
    );

    // (b) never darker, anywhere, in any channel.
    for (i, (&lit, &dark)) in additive.rgba.iter().zip(bare.rgba.iter()).enumerate() {
        assert!(
            lit >= dark,
            "additive particle darkened byte {i}: {lit} < {dark}",
        );
    }
}

/// A camera at the origin looking down −Z over an empty, unlit-ambient scene: the fixture the
/// lit-particle tests share, so a world position lands at a known pixel.
fn dark_particle_scene(w: u32, h: u32) -> RenderScene {
    let mut scene = RenderScene::empty((w, h));
    scene.ambient = [0.0; 3];
    scene.camera = origin_camera(w, h);
    scene
}

/// **A lit particle follows the light, an unlit twin ignores it**, for both blends and for a
/// point light and a spot: a hard disc inside the light's reach is far brighter than one outside
/// it, while the unlit twins render the same pixel at both positions. The light sits 1.5 units
/// nearer the camera than the discs on purpose: the term is `|n·l|` with `n` the camera-facing
/// normal, so a light in the discs' own plane would be exactly edge-on and give zero.
#[test]
fn needs_gpu_lit_particles_follow_the_light_and_unlit_twins_do_not() {
    let Some(renderer) = skip_or_renderer("lit_particles_follow_the_light") else {
        return;
    };
    let (w, h) = (96u32, 96u32);
    let size = FrameSize::native(w, h);
    let inside = Vec3::new(-1.5, 0.0, -6.0);
    let outside = Vec3::new(1.0, 0.0, -6.0);
    let cam = origin_camera(w, h);
    let (pi, po) = (project(&cam, inside, w, h), project(&cam, outside, w, h));

    for additive in [false, true] {
        for spot in [false, true] {
            let mut scene = dark_particle_scene(w, h);
            if spot {
                scene.spot_lights = vec![spot_light(
                    Vec3::new(-1.5, 0.0, -4.5),
                    Vec3::NEG_Z,
                    6.0,
                    6.0,
                    10.0,
                    20.0,
                )];
            } else {
                // Range 2.5 reaches the inside disc (1.5 away) and not the outside one (~2.9).
                scene.lights = vec![point_light(Vec3::new(-1.5, 0.0, -4.5), 6.0, 2.5)];
            }
            let name = format!("additive={additive} spot={spot}");
            let mut render = |lit: bool| {
                scene.particles = [inside, outside]
                    .iter()
                    .map(|&c| ParticleInstance {
                        additive,
                        lit,
                        ..particle(c, 0.8, [1.0; 4], 0.0)
                    })
                    .collect();
                renderer.render(&scene, size).expect("particle render")
            };
            let lit = render(true);
            let unlit = render(false);
            assert!(
                luma(&lit, pi.0, pi.1) > luma(&lit, po.0, po.1) + 60,
                "{name}: the lit disc inside the light must outshine the one outside",
            );
            // Within the output dither's step: the light moves nothing but noise.
            assert!(
                luma(&unlit, pi.0, pi.1).abs_diff(luma(&unlit, po.0, po.1)) <= 6,
                "{name}: an unlit twin ignores the light",
            );
        }
    }
}

/// **One rule**: a lit particle's centre pixel is the lit sprite's, for the same point, colour,
/// blend and light. The light is small and sits well off the screen centre and the main diagonal,
/// so only the few tiles near the disc file it: a wrong or Y-flipped cell in the vertex stage
/// would read an empty cell and drop the light, and this test would go red.
#[test]
fn needs_gpu_a_lit_particle_matches_a_lit_sprite_at_its_centre() {
    let Some(renderer) = skip_or_renderer("lit_particle_matches_lit_sprite") else {
        return;
    };
    let (w, h) = (96u32, 96u32);
    let size = FrameSize::native(w, h);
    let centre = Vec3::new(2.0, -1.2, -6.0);
    let (cx, cy) = project(&origin_camera(w, h), centre, w, h);

    for additive in [false, true] {
        let mut scene = dark_particle_scene(w, h);
        scene.lights = vec![point_light(Vec3::new(2.0, -1.2, -5.4), 3.0, 1.4)];
        scene.sprites = vec![crate::scene::SpriteInstance {
            center: centre,
            right: Vec3::new(0.5, 0.0, 0.0),
            up: Vec3::new(0.0, 0.5, 0.0),
            color: [0.8, 0.6, 0.4, 1.0],
            texture: None,
            uv: crate::scene::SpriteInstance::FULL_UV,
            lit: true,
            layer: 0,
            additive,
            flash: 0.0,
            flash_color: [1.0; 3],
            uv_repeat: crate::scene::SpriteInstance::NO_TILING,
            uv_offset: [0.0; 2],
            medium: 1.0,
        }];
        let sprite = renderer.render(&scene, size).expect("sprite render");
        scene.sprites.clear();
        scene.particles = vec![ParticleInstance {
            additive,
            lit: true,
            ..particle(centre, 0.5, [0.8, 0.6, 0.4, 1.0], 0.0)
        }];
        let disc = renderer.render(&scene, size).expect("particle render");

        let (s, d) = (rgb_at(&sprite, cx, cy), rgb_at(&disc, cx, cy));
        assert!(
            s.iter().map(|&c| c as u32).sum::<u32>() > 60,
            "additive={additive}: the sprite centre must be genuinely lit, got {s:?}",
        );
        for c in 0..3 {
            assert!(
                (s[c] as i32 - d[c] as i32).abs() <= 3,
                "additive={additive}: sprite {s:?} vs particle {d:?} at the centre",
            );
        }
    }
}

/// The atlas the renderer uploads is the embedded face's PNG: the face's declared size, white
/// pixels with alpha as coverage.
#[test]
fn the_embedded_atlas_png_matches_the_face_and_carries_coverage() {
    let face = ludion_font::FontFace::embedded();
    let (w, h, rgba) = ludion_assets::decode_png(ludion_font::EMBEDDED_PNG, "default.png")
        .expect("the embedded font atlas PNG decodes");
    assert_eq!(
        (w, h),
        (face.document().atlas.width, face.document().atlas.height)
    );
    assert_eq!(rgba.len(), (w * h * 4) as usize);
    assert!(rgba.chunks(4).any(|p| p[3] != 0), "the atlas is not blank");
    assert!(
        rgba.chunks(4)
            .all(|p| p[3] == 0 || p[..3] == [255, 255, 255]),
        "covered texels are white; alpha carries the coverage",
    );
}
