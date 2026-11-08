//! The mesh shader's pipelines: the scene pass, its medium-mask twin, their `VIEWMODEL` and
//! `SUN_CASCADE_VIEW` twins and the probe capture, one builder over their fragment stages. Kept
//! beside `render.rs` rather than in it.

use super::{
    HDR_FORMAT, MESH_INSTANCE_ATTRS, MESH_VERTEX_ATTRS, MeshInstanceRaw, MeshVariant, MeshVertex,
    OUTPUT_FORMAT, depth_state, mesh_primitive,
};

/// The opaque scene pass's three targets, in the graph's declaration order: `scene` (HDR),
/// `scene_normal` (HDR) and `scene_specular` (`OUTPUT_FORMAT`, sRGB 8-bit).
const SCENE: MeshFragment<'static> = MeshFragment {
    entry: "fs_main",
    formats: &[HDR_FORMAT, HDR_FORMAT, OUTPUT_FORMAT],
    constants: &[],
};

/// The scene pass of a frame with a mesh below `medium` 1: [`SCENE`]'s three targets plus
/// `scene_medium` (`R8Unorm`), the per-pixel medium scale the composite reads.
const MEDIUM: MeshFragment<'static> = MeshFragment {
    entry: "fs_medium",
    formats: &[
        HDR_FORMAT,
        HDR_FORMAT,
        OUTPUT_FORMAT,
        wgpu::TextureFormat::R8Unorm,
    ],
    constants: &[],
};

/// `variant` picks the winding and the culling ([`mesh_primitive`]).
pub(super) fn build_mesh_pipeline(
    device: &wgpu::Device,
    label: &str,
    shader: &wgpu::ShaderModule,
    layout: &wgpu::PipelineLayout,
    variant: MeshVariant,
) -> wgpu::RenderPipeline {
    build_mesh_pipeline_for(device, label, shader, layout, variant, SCENE)
}

pub(super) fn build_mesh_medium_pipeline(
    device: &wgpu::Device,
    label: &str,
    shader: &wgpu::ShaderModule,
    layout: &wgpu::PipelineLayout,
    variant: MeshVariant,
) -> wgpu::RenderPipeline {
    build_mesh_pipeline_for(device, label, shader, layout, variant, MEDIUM)
}

/// The probe capture's scene pass: [`SCENE`]'s three targets plus `scene_emissive` (HDR), the
/// part of the shaded colour the live-ambient relight must not scale.
pub(super) fn build_mesh_capture_pipeline(
    device: &wgpu::Device,
    label: &str,
    shader: &wgpu::ShaderModule,
    layout: &wgpu::PipelineLayout,
    variant: MeshVariant,
) -> wgpu::RenderPipeline {
    let capture = MeshFragment {
        entry: "fs_capture",
        formats: &[HDR_FORMAT, HDR_FORMAT, OUTPUT_FORMAT, HDR_FORMAT],
        constants: &[],
    };
    build_mesh_pipeline_for(device, label, shader, layout, variant, capture)
}

/// The scene pass's two fragments, plain and medium-mask, compiled with one set of pipeline
/// constants, by [`MeshVariant::index`].
pub(super) struct SceneTwins {
    scene: [wgpu::RenderPipeline; 4],
    medium: [wgpu::RenderPipeline; 4],
}

impl SceneTwins {
    /// The gun's twins, `VIEWMODEL` true. The gun casts no shadow and is left out of the prepass and
    /// probe captures, so only the two scene-pass fragments have twins.
    pub(super) fn viewmodel(
        device: &wgpu::Device,
        label: &str,
        shader: &wgpu::ShaderModule,
        layout: &wgpu::PipelineLayout,
    ) -> Self {
        Self::build(device, label, shader, layout, &[("VIEWMODEL", 1.0)])
    }

    fn build(
        device: &wgpu::Device,
        label: &str,
        shader: &wgpu::ShaderModule,
        layout: &wgpu::PipelineLayout,
        constants: &'static [(&'static str, f64)],
    ) -> Self {
        let twins = |fragment: MeshFragment<'static>| {
            let fragment = MeshFragment {
                constants,
                ..fragment
            };
            MeshVariant::ALL.map(|variant| {
                build_mesh_pipeline_for(device, label, shader, layout, variant, fragment)
            })
        };
        SceneTwins {
            scene: twins(SCENE),
            medium: twins(MEDIUM),
        }
    }

    /// The pipeline for `variant`: the medium-mask one when the scene pass writes `scene_medium`.
    pub(super) fn get(&self, variant: MeshVariant, medium: bool) -> &wgpu::RenderPipeline {
        let variants = if medium { &self.medium } else { &self.scene };
        &variants[variant.index()]
    }
}

/// The `sun_cascades` debug view's scene pipelines, `SUN_CASCADE_VIEW` true, for world draws and for
/// the gun's. Built on the view's first frame, so a game that never opens it neither compiles them
/// nor pays for the tint in any fragment. A shader reload builds a fresh, empty set.
pub(super) struct CascadeViewTwins {
    label: &'static str,
    shader: wgpu::ShaderModule,
    layout: wgpu::PipelineLayout,
    /// `[world, gun]`.
    built: std::sync::OnceLock<[SceneTwins; 2]>,
}

impl CascadeViewTwins {
    pub(super) fn new(
        label: &'static str,
        shader: &wgpu::ShaderModule,
        layout: &wgpu::PipelineLayout,
    ) -> Self {
        CascadeViewTwins {
            label,
            shader: shader.clone(),
            layout: layout.clone(),
            built: std::sync::OnceLock::new(),
        }
    }

    pub(super) fn get(
        &self,
        device: &wgpu::Device,
        variant: MeshVariant,
        viewmodel: bool,
        medium: bool,
    ) -> &wgpu::RenderPipeline {
        let built = self.built.get_or_init(|| {
            let build = |constants| {
                SceneTwins::build(device, self.label, &self.shader, &self.layout, constants)
            };
            [
                build(&[("SUN_CASCADE_VIEW", 1.0)]),
                build(&[("SUN_CASCADE_VIEW", 1.0), ("VIEWMODEL", 1.0)]),
            ]
        });
        built[usize::from(viewmodel)].get(variant, medium)
    }
}

/// A mesh pipeline's fragment stage: its entry point, colour targets and pipeline constants.
#[derive(Clone, Copy)]
struct MeshFragment<'a> {
    entry: &'a str,
    formats: &'a [wgpu::TextureFormat],
    constants: &'a [(&'a str, f64)],
}

fn build_mesh_pipeline_for(
    device: &wgpu::Device,
    label: &str,
    shader: &wgpu::ShaderModule,
    layout: &wgpu::PipelineLayout,
    variant: MeshVariant,
    fragment: MeshFragment<'_>,
) -> wgpu::RenderPipeline {
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some(label),
        layout: Some(layout),
        vertex: wgpu::VertexState {
            module: shader,
            entry_point: Some("vs_main"),
            buffers: &[
                Some(wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<MeshVertex>() as u64,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &MESH_VERTEX_ATTRS,
                }),
                Some(wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<MeshInstanceRaw>() as u64,
                    step_mode: wgpu::VertexStepMode::Instance,
                    attributes: &MESH_INSTANCE_ATTRS,
                }),
            ],
            compilation_options: wgpu::PipelineCompilationOptions::default(),
        },
        fragment: Some(wgpu::FragmentState {
            module: shader,
            entry_point: Some(fragment.entry),
            targets: &fragment
                .formats
                .iter()
                .map(|&format| {
                    Some(wgpu::ColorTargetState {
                        format,
                        blend: None,
                        write_mask: wgpu::ColorWrites::ALL,
                    })
                })
                .collect::<Vec<_>>(),
            compilation_options: wgpu::PipelineCompilationOptions {
                constants: fragment.constants,
                ..Default::default()
            },
        }),
        primitive: mesh_primitive(variant),
        depth_stencil: Some(depth_state(true, wgpu::CompareFunction::Less)),
        multisample: wgpu::MultisampleState::default(),
        multiview_mask: None,
        cache: None,
    })
}
