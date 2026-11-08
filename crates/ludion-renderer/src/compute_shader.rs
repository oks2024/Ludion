//! The compute shaders built outside [`ShaderId`](crate::render::ShaderId): which snippets each
//! composes ahead of its body. `render.rs`'s `compose_builtin` is the one join, shared with the
//! render shaders. `capture_code_key` hashes the same composed strings the pipelines compile.
//! Compute pipelines are built once per renderer; the dev hot reload replaces only the `Ibl`
//! (`shader_reload.rs`) and refuses the others. The bindgen registry
//! (`ludion_shader_bindgen::COMPOSITIONS`) holds the second mapping — the renderer cannot depend on
//! bindgen outside tests — and `compute_composition_matches_bindgen_registry` compares the two.

use crate::render::{Snippet, compose_builtin};

/// A compute shader and its composition.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum ComputeShader {
    Cull,
    GiBake,
    GiInject,
    Ibl,
    ProbeRelight,
}

impl ComputeShader {
    /// Every compute shader, in the order of `ludion_shader_bindgen::COMPUTE_SHADERS` (sorted).
    pub(crate) const ALL: [ComputeShader; 5] = [
        ComputeShader::Cull,
        ComputeShader::GiBake,
        ComputeShader::GiInject,
        ComputeShader::Ibl,
        ComputeShader::ProbeRelight,
    ];

    /// The body's file name under `shaders/`.
    pub(crate) fn file_name(self) -> &'static str {
        match self {
            ComputeShader::Cull => "cull.wgsl",
            ComputeShader::GiBake => "gi_bake.wgsl",
            ComputeShader::GiInject => "gi_inject.wgsl",
            ComputeShader::Ibl => "ibl.wgsl",
            ComputeShader::ProbeRelight => "probe_relight.wgsl",
        }
    }

    /// The snippets composed ahead of the body, in order.
    pub(crate) fn snippets(self) -> &'static [Snippet] {
        match self {
            ComputeShader::Ibl => &[Snippet::Math, Snippet::Dfg, Snippet::SkyGradient],
            ComputeShader::GiBake | ComputeShader::GiInject => &[Snippet::Math, Snippet::GiRays],
            ComputeShader::Cull | ComputeShader::ProbeRelight => &[],
        }
    }

    /// The compiled-in body, minus the snippets.
    pub(crate) fn body_source(self) -> &'static str {
        match self {
            ComputeShader::Cull => include_str!("shaders/cull.wgsl"),
            ComputeShader::GiBake => include_str!("shaders/gi_bake.wgsl"),
            ComputeShader::GiInject => include_str!("shaders/gi_inject.wgsl"),
            ComputeShader::Ibl => include_str!("shaders/ibl.wgsl"),
            ComputeShader::ProbeRelight => include_str!("shaders/probe_relight.wgsl"),
        }
    }

    /// The source the pipeline compiles: the snippets, then the body.
    pub(crate) fn composed(self) -> String {
        compose_builtin(self.snippets(), self.body_source())
    }
}

/// `file_name` composed the way the bindgen registry says, read from disk.
#[cfg(test)]
pub(crate) fn registry_composed(file_name: &str) -> String {
    let shaders_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("src")
        .join("shaders");
    let mut parts: Vec<String> = ludion_shader_bindgen::snippets_for(file_name)
        .iter()
        .map(|name| {
            std::fs::read_to_string(shaders_dir.join("snippets").join(name))
                .unwrap_or_else(|e| panic!("read snippet {name}: {e}"))
        })
        .collect();
    parts.push(
        std::fs::read_to_string(shaders_dir.join(file_name))
            .unwrap_or_else(|e| panic!("read body {file_name}: {e}")),
    );
    parts.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The renderer's compute compositions are the bindgen registry's, so `shader_gen.rs` is
    /// generated from what the GPU compiles.
    #[test]
    fn compute_composition_matches_bindgen_registry() {
        for shader in ComputeShader::ALL {
            assert_eq!(
                shader.composed(),
                registry_composed(shader.file_name()),
                "composition drift for {}: `ComputeShader::snippets` and the bindgen \
                 `COMPOSITIONS` registry disagree — reconcile them",
                shader.file_name()
            );
        }
    }

    #[test]
    fn compute_shader_names_match_the_bindgen_list() {
        let names: Vec<&str> = ComputeShader::ALL.iter().map(|s| s.file_name()).collect();
        assert_eq!(names, ludion_shader_bindgen::COMPUTE_SHADERS);
    }

    /// `shafts` and `composite` keep their own `world_from_depth`: it moves pixels through
    /// compilation alone (`Plan/transparent-depth.md`, A fragility). Their bodies stay the
    /// snippet's, with only the uniform's name spelled in.
    #[test]
    fn kept_world_from_depth_copies_match_the_snippet() {
        let snippet = include_str!("shaders/snippets/world_from_depth.wgsl");
        let body_start = snippet.find("{\n").expect("snippet function body") + 2;
        let body = &snippet[body_start..snippet.rfind("}\n").expect("snippet function end")];
        for (uniform, source) in [
            ("shafts", include_str!("shaders/shafts.wgsl")),
            ("composite", include_str!("shaders/composite.wgsl")),
        ] {
            let expected = body.replace("inv_view_proj", &format!("{uniform}.inv_view_proj"));
            assert!(
                source.contains(&expected),
                "{uniform}.wgsl's world_from_depth no longer matches snippets/world_from_depth.wgsl \
                 statement for statement; the copy is kept out of the snippet on purpose \
                 (Plan/transparent-depth.md, A fragility), so edit both together"
            );
        }
    }
}
