//! Smoke test: reflect every real repo WGSL file together (`ludion_shader_bindgen::load_units`, the
//! same loader the bin uses) and confirm they reflect without error, that the shared `Camera`/`Light`
//! snippet structs unify into a single entry each, and that the shader lists match the shader
//! directories, so a new shader cannot be silently left out.

use std::path::{Path, PathBuf};

use ludion_shader_bindgen::{
    BindingKind, BufferSpace, COMPUTE_SHADERS, RENDER_GRAPH_SHADERS, RENDERER_SHADERS, ShaderStage,
    StorageAccess, StorageFormat, TextureDim, load_units, reflect,
};

fn workspace() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn units() -> Vec<ludion_shader_bindgen::ShaderUnit> {
    load_units(&workspace()).expect("the repo's shader files load")
}

/// The sorted `*.wgsl` file names directly under `dir` (not its `snippets/` subfolder).
fn wgsl_files(dir: &str) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(workspace().join(dir))
        .unwrap_or_else(|e| panic!("{dir}: {e}"))
        .map(|entry| entry.expect("a readable directory entry").path())
        .filter(|path| path.is_file() && path.extension().is_some_and(|ext| ext == "wgsl"))
        .map(|path| path.file_name().unwrap().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

fn is_sorted_and_unique(list: &[&str]) -> bool {
    list.windows(2).all(|pair| pair[0] < pair[1])
}

#[test]
fn shader_lists_match_the_shader_dirs() {
    assert_eq!(
        wgsl_files("crates/ludion-renderer/src/shaders"),
        RENDERER_SHADERS,
        "RENDERER_SHADERS must list every `.wgsl` in the renderer's shaders directory"
    );
    assert_eq!(
        wgsl_files("crates/ludion-render-graph/src/shaders"),
        RENDER_GRAPH_SHADERS,
        "RENDER_GRAPH_SHADERS must list every `.wgsl` in the render graph's shaders directory"
    );
    assert!(is_sorted_and_unique(RENDERER_SHADERS));
    assert!(is_sorted_and_unique(RENDER_GRAPH_SHADERS));
    assert!(is_sorted_and_unique(COMPUTE_SHADERS));
    for name in COMPUTE_SHADERS {
        assert!(
            RENDERER_SHADERS.contains(name),
            "{name} is a compute shader but not a renderer shader"
        );
    }
}

#[test]
fn all_repo_shaders_reflect_and_unify() {
    let reflection = reflect(&units()).expect("every repo shader should reflect without error");

    // Camera is copy-pasted across mesh/sprite/text — it must unify into exactly one struct.
    assert_eq!(
        reflection
            .structs
            .iter()
            .filter(|s| s.name == "Camera")
            .count(),
        1,
        "Camera must unify to a single struct"
    );
    let camera = reflection.struct_named("Camera").unwrap();
    assert_eq!(camera.size, 80); // mat4x4 (64) + vec4 (16)

    // The clustered light set is composed into both mesh and sprite; each struct must unify to one
    // entry. `Light` is reachable only through the `light_list` storage array — a runtime-sized
    // `array<Light>` — and must still be emitted so the CPU side has a POD to fill the buffer with.
    for (name, size) in [("Light", 48), ("LightHeader", 64)] {
        assert_eq!(
            reflection.structs.iter().filter(|s| s.name == name).count(),
            1,
            "{name} must unify to a single struct"
        );
        assert_eq!(reflection.struct_named(name).unwrap().size, size);
    }

    // The shared camera uniform unifies to group 0 / binding 0. It is read by every vertex stage
    // and by mesh's fragment stage (which samples `camera.cam_pos` for specular), so the merged
    // visibility is both stages — exactly the derived BGL visibility Wave 4 wants.
    let camera_binding = reflection.binding_named("camera").unwrap();
    assert_eq!((camera_binding.group, camera_binding.binding), (0, 0));
    assert!(matches!(
        camera_binding.kind,
        BindingKind::Buffer {
            address_space: BufferSpace::Uniform,
            min_size: 80,
        }
    ));
    assert_eq!(
        camera_binding.visibility,
        vec![ShaderStage::Vertex, ShaderStage::Fragment]
    );

    // The whole light group is read by fragment stages only — the header uniform and all three
    // read-only storage buffers, each sized to ONE element (naga sizes a runtime array that way, so
    // `min_binding_size` is the element stride, not the buffer's length).
    for (name, min) in [
        ("lights", 64u32),
        ("light_list", 48),
        ("cluster_table", 8),
        ("cluster_lights", 4),
    ] {
        let binding = reflection.binding_named(name).unwrap();
        assert_eq!(binding.group, 1);
        assert_eq!(binding.visibility, vec![ShaderStage::Fragment]);
        let BindingKind::Buffer { min_size, .. } = binding.kind else {
            panic!("{name} must be a buffer binding");
        };
        assert_eq!(min_size, min, "{name}'s min_binding_size");
    }

    // The cluster grid's shape is hoisted from the shared snippet, unified once across both units.
    assert_eq!(reflection.const_value("CLUSTER_TILES_X"), Some(16));
    assert_eq!(reflection.const_value("CLUSTER_TILES_Y"), Some(9));
    assert_eq!(reflection.const_value("CLUSTER_SLICES"), Some(16));

    // Mesh's vertex entry point carries all 20 @location inputs (per-vertex 0-5, per-instance 6-15,
    // the occlusion-fade lane at 16, the hit-flash lane at 17, and the skin weights/bones at 18-19).
    let mesh_vs = reflection
        .entry_points
        .iter()
        .find(|e| e.unit == "mesh.wgsl" && e.stage == ShaderStage::Vertex)
        .unwrap();
    assert_eq!(mesh_vs.inputs.len(), 20);
    assert_eq!(mesh_vs.inputs[0].location, 0);
    assert_eq!(mesh_vs.inputs[19].location, 19);

    // Bind group unification (decision 13): the `{src_tex, src_sampler}` group unifies across
    // blit/vignette into ONE layout, while threshold's `{src_tex, src_sampler, bloom}` (the
    // Environment/IBL bloom-threshold uniform) is a *different* name-set and reflects as a separate
    // layout — the shape falls out of the key, no conflict. `sprite_tex`/`sprite_sampler` (group 2)
    // stays distinct from the atlas set despite being structurally identical, because the key is the
    // resource names, not the wgpu shape.
    let shared = reflection.bind_group_named("src_tex_src_sampler").unwrap();
    assert_eq!(shared.group, 0);
    assert_eq!(
        shared.units,
        vec!["blit.wgsl".to_string(), "vignette.wgsl".to_string()]
    );
    assert!(
        reflection
            .bind_group_named("sprite_tex_sprite_sampler")
            .is_some(),
        "the sprite texture group stays distinct from the atlas group despite the same shape"
    );
    // Thirty-two layouts across every renderer and render-graph shader — a count that moves whenever
    // a shader gains or loses a group, so it is the tripwire that a new one was reflected on purpose.
    // Mesh/prepass's group 0 (`camera` + `bone_palette`) and shadow's (`shadow_view` + `bone_palette`)
    // are each a distinct name-set from ground-marker/sprite/particle/text's plain `camera`, so the
    // bone palette (Plan/skeletal-animation.md W2) adds one layout over the pre-skinning count, and
    // the debug-line pass's scene-depth + output-extent group (`debug_depth`) adds another, and the
    // reflections pass's normal + specular + depth group a third, and the SSR pyramids' three
    // (Hi-Z seed, Hi-Z reduce, colour) three more. Of
    // them exactly three are compute-only (gi_bake, gi_inject, the IBL prefilter trio): the emitter
    // skips their BGLs because compute pipelines keep auto-derived layouts.
    assert_eq!(reflection.bind_groups.len(), 32);
    let mut compute_only: Vec<&str> = reflection
        .bind_groups
        .iter()
        .filter(|g| {
            g.members.iter().all(|m| {
                reflection.binding_named(m).unwrap().visibility == vec![ShaderStage::Compute]
            })
        })
        .flat_map(|g| g.units.iter().map(String::as_str))
        .collect();
    compute_only.sort();
    assert_eq!(compute_only, ["gi_bake.wgsl", "gi_inject.wgsl", "ibl.wgsl"]);

    // The mesh sun group is a mesh-only @group(2): a uniform + a depth texture + a comparison sampler.
    let sun_group = reflection
        .bind_group_named("sun_shadow_tex_shadow_samp")
        .expect("mesh's group-2 sun layout");
    assert_eq!(sun_group.group, 2);
    assert_eq!(sun_group.units, vec!["mesh.wgsl".to_string()]);
    let shadow_samp = reflection.binding_named("shadow_samp").unwrap();
    assert_eq!(shadow_samp.kind, BindingKind::Sampler { comparison: true });

    // The compute shaders reflect too: their `Light` unifies with the snippet's (a drift is now a
    // bindgen error), and the GI pair shares one `GiBinParams`.
    let light = reflection.struct_named("Light").unwrap();
    for unit in ["gi_bake.wgsl", "gi_inject.wgsl"] {
        assert!(light.units.contains(&unit.to_string()), "Light in {unit}");
    }
    assert_eq!(
        reflection
            .structs
            .iter()
            .filter(|s| s.name == "GiBinParams")
            .count(),
        1
    );
    assert_eq!(reflection.const_value("HIT_RAYS"), Some(16));

    // `var<storage, read_write>` reflects, and a storage texture carries dim, format and access.
    assert_eq!(
        reflection.binding_named("bake_hit_cache").unwrap().kind,
        BindingKind::Buffer {
            address_space: BufferSpace::Storage { read_only: false },
            min_size: 4,
        }
    );
    let probes_out = reflection.binding_named("probes_out").unwrap();
    assert_eq!(
        probes_out.kind,
        BindingKind::StorageTexture {
            dim: TextureDim::D3,
            format: StorageFormat::Rgba16Float,
            access: StorageAccess::WriteOnly,
        }
    );
    assert_eq!(probes_out.visibility, vec![ShaderStage::Compute]);
    assert!(matches!(
        reflection.binding_named("specular_out").unwrap().kind,
        BindingKind::StorageTexture {
            dim: TextureDim::D2Array,
            ..
        }
    ));
}
