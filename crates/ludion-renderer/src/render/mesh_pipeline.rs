//! The mesh shader's pipelines: the scene pass, its medium-mask twin, their `VIEWMODEL` twins and the
//! probe capture, one builder over their fragment stages. Kept beside `render.rs` rather than in it.

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

/// The scene pass's pipelines compiled with `VIEWMODEL` true, the gun's, by [`MeshVariant::index`].
/// The gun casts no shadow and is left out of the prepass and probe captures, so only the two
/// scene-pass fragments have twins.
pub(super) struct ViewmodelTwins {
    pub(super) scene: [wgpu::RenderPipeline; 4],
    pub(super) medium: [wgpu::RenderPipeline; 4],
}

impl ViewmodelTwins {
    pub(super) fn build(
        device: &wgpu::Device,
        label: &str,
        shader: &wgpu::ShaderModule,
        layout: &wgpu::PipelineLayout,
    ) -> Self {
        let twins = |fragment: MeshFragment<'static>| {
            let fragment = MeshFragment {
                constants: &[("VIEWMODEL", 1.0)],
                ..fragment
            };
            MeshVariant::ALL.map(|variant| {
                build_mesh_pipeline_for(device, label, shader, layout, variant, fragment)
            })
        };
        ViewmodelTwins {
            scene: twins(SCENE),
            medium: twins(MEDIUM),
        }
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
