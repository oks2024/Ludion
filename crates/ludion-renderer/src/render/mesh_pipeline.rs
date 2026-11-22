//! The mesh shader's pipelines: the scene pass, its medium-mask twin, their `VIEWMODEL` and
//! `SUN_CASCADE_VIEW` twins and the probe capture, one builder over their fragment stages. Kept
//! beside `render.rs` rather than in it.

use super::{
    HDR_FORMAT, MESH_INSTANCE_ATTRS, MESH_VERTEX_ATTRS, MeshInstanceRaw, MeshVariant, MeshVertex,
    OUTPUT_FORMAT, depth_state, mesh_primitive, premultiplied_blend,
};
use crate::scene::BlendMode;

/// The opaque scene pass's three targets, in the graph's declaration order: `scene` (HDR),
/// `scene_normal` (HDR) and `scene_specular` (`OUTPUT_FORMAT`, sRGB 8-bit).
const SCENE: MeshFragment<'static> = MeshFragment {
    entry: "fs_main",
    formats: &[HDR_FORMAT, HDR_FORMAT, OUTPUT_FORMAT],
    constants: &[],
    blend: None,
    write_mask: wgpu::ColorWrites::ALL,
    depth_write: true,
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
    blend: None,
    write_mask: wgpu::ColorWrites::ALL,
    depth_write: true,
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
        blend: None,
        write_mask: wgpu::ColorWrites::ALL,
        depth_write: true,
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

/// The mesh shader's lazily built twins: the `sun_cascades` debug view's scene pipelines,
/// `SUN_CASCADE_VIEW` true, and the `irradiance` view's, `IRRADIANCE_VIEW` true, each for world draws
/// and for the gun's, and the blended meshes' ([`BlendTwins`]).
/// Built on first use, so a game that never opens a view or blends neither compiles them nor pays
/// for them in any fragment. A shader reload builds a fresh, empty set.
pub(super) struct CascadeViewTwins {
    pub(super) blend: BlendTwins,
    label: &'static str,
    shader: wgpu::ShaderModule,
    layout: wgpu::PipelineLayout,
    /// `[world, gun]`.
    built: std::sync::OnceLock<[SceneTwins; 2]>,
    /// The `irradiance` view's `[world, gun]`.
    irradiance: std::sync::OnceLock<[SceneTwins; 2]>,
}

impl CascadeViewTwins {
    pub(super) fn new(
        label: &'static str,
        shader: &wgpu::ShaderModule,
        layout: &wgpu::PipelineLayout,
    ) -> Self {
        CascadeViewTwins {
            blend: BlendTwins::new(label, shader, layout),
            label,
            shader: shader.clone(),
            layout: layout.clone(),
            built: std::sync::OnceLock::new(),
            irradiance: std::sync::OnceLock::new(),
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

    pub(super) fn get_irradiance(
        &self,
        device: &wgpu::Device,
        variant: MeshVariant,
        viewmodel: bool,
        medium: bool,
    ) -> &wgpu::RenderPipeline {
        let built = self.irradiance.get_or_init(|| {
            let build = |constants| {
                SceneTwins::build(device, self.label, &self.shader, &self.layout, constants)
            };
            [
                build(&[("IRRADIANCE_VIEW", 1.0)]),
                build(&[("IRRADIANCE_VIEW", 1.0), ("VIEWMODEL", 1.0)]),
            ]
        });
        built[usize::from(viewmodel)].get(variant, medium)
    }
}

/// The blended meshes' pipelines (`Mesh.blend`): the colour pass into the one HDR target, and the
/// transparent layer's twins into `Rg16Float` or `Rgba16Float`, each for both modes and every
/// [`MeshVariant`]. Built on the first blended frame, so a game that never blends neither compiles
/// them nor pays for them; the unlit set (`Mesh.lit: false`, `BLEND_UNLIT`) is a second set built on
/// the first unlit blend, so a frame with only lit blends compiles nothing more. A shader reload builds
/// a fresh, empty set.
pub(super) struct BlendTwins {
    label: &'static str,
    shader: wgpu::ShaderModule,
    layout: wgpu::PipelineLayout,
    /// `[lit, unlit]`, boxed: every shader's pipelines sit on `Renderer::new`'s stack, and these arrays are
    /// large.
    built: [std::sync::OnceLock<Box<BlendPipelines>>; 2],
    /// The `blend_depth: nearest` twins: the same `[lit, unlit]` sets testing `LessEqual` against the
    /// group's scratch depth, built on the first `nearest` draw.
    nearest: [std::sync::OnceLock<Box<BlendPipelines>>; 2],
    /// The depth-only pre-pass that writes a `nearest` group's front surface into the scratch depth.
    prepass: std::sync::OnceLock<[wgpu::RenderPipeline; 4]>,
}

struct BlendPipelines {
    /// `[mode][variant]`.
    color: [[wgpu::RenderPipeline; 4]; 2],
    /// `[carries medium][mode][variant]`.
    layer: [[[wgpu::RenderPipeline; 4]; 2]; 2],
}

impl BlendTwins {
    pub(super) fn new(
        label: &'static str,
        shader: &wgpu::ShaderModule,
        layout: &wgpu::PipelineLayout,
    ) -> Self {
        BlendTwins {
            label,
            shader: shader.clone(),
            layout: layout.clone(),
            built: [std::sync::OnceLock::new(), std::sync::OnceLock::new()],
            nearest: [std::sync::OnceLock::new(), std::sync::OnceLock::new()],
            prepass: std::sync::OnceLock::new(),
        }
    }

    fn pipelines(&self, device: &wgpu::Device, lit: bool, nearest: bool) -> &BlendPipelines {
        let (cell, compare) = if nearest {
            (&self.nearest, wgpu::CompareFunction::LessEqual)
        } else {
            (&self.built, wgpu::CompareFunction::Less)
        };
        cell[usize::from(!lit)].get_or_init(|| {
            let build = |fragment: MeshFragment<'_>, variant| {
                build_mesh_pipeline_with(
                    device,
                    self.label,
                    &self.shader,
                    &self.layout,
                    variant,
                    Some(fragment),
                    compare,
                )
            };
            let modes = [BlendMode::Alpha, BlendMode::Additive];
            let color = modes.map(|mode| {
                let fragment = MeshFragment {
                    entry: "fs_blend",
                    formats: &[HDR_FORMAT],
                    constants: if lit {
                        &[("BLENDED", 1.0)]
                    } else {
                        &[("BLENDED", 1.0), ("BLEND_UNLIT", 1.0)]
                    },
                    blend: Some(match mode {
                        BlendMode::Alpha => premultiplied_blend(),
                        BlendMode::Additive => additive_blend(),
                    }),
                    write_mask: wgpu::ColorWrites::ALL,
                    depth_write: false,
                };
                MeshVariant::ALL.map(|variant| build(fragment, variant))
            });
            let layer = [false, true].map(|medium| {
                modes.map(|mode| {
                    let fragment = MeshFragment {
                        entry: "fs_blend_layer",
                        formats: if medium {
                            &[wgpu::TextureFormat::Rgba16Float]
                        } else {
                            &[wgpu::TextureFormat::Rg16Float]
                        },
                        constants: match (mode, lit) {
                            (BlendMode::Alpha, true) => &[("BLENDED", 1.0)],
                            (BlendMode::Alpha, false) => &[("BLENDED", 1.0), ("BLEND_UNLIT", 1.0)],
                            (BlendMode::Additive, true) => {
                                &[("BLENDED", 1.0), ("BLEND_ADDITIVE", 1.0)]
                            }
                            (BlendMode::Additive, false) => &[
                                ("BLENDED", 1.0),
                                ("BLEND_UNLIT", 1.0),
                                ("BLEND_ADDITIVE", 1.0),
                            ],
                        },
                        blend: Some(premultiplied_blend()),
                        write_mask: if medium {
                            wgpu::ColorWrites::RED
                                | wgpu::ColorWrites::GREEN
                                | wgpu::ColorWrites::BLUE
                        } else {
                            wgpu::ColorWrites::RED | wgpu::ColorWrites::GREEN
                        },
                        depth_write: false,
                    };
                    MeshVariant::ALL.map(|variant| build(fragment, variant))
                })
            });
            Box::new(BlendPipelines { color, layer })
        })
    }

    /// The colour pass's pipeline for `mode`, `lit` and `variant`; `nearest` selects the `LessEqual` twin
    /// that tests against the group's scratch depth.
    pub(super) fn color(
        &self,
        device: &wgpu::Device,
        mode: BlendMode,
        lit: bool,
        nearest: bool,
        variant: MeshVariant,
    ) -> &wgpu::RenderPipeline {
        &self.pipelines(device, lit, nearest).color[mode as usize][variant.index()]
    }

    /// The transparent layer's pipeline; `medium` selects the `Rgba16Float` layer that carries blue.
    pub(super) fn layer(
        &self,
        device: &wgpu::Device,
        mode: BlendMode,
        lit: bool,
        nearest: bool,
        variant: MeshVariant,
        medium: bool,
    ) -> &wgpu::RenderPipeline {
        &self.pipelines(device, lit, nearest).layer[usize::from(medium)][mode as usize]
            [variant.index()]
    }

    /// The depth-only pipeline of a `nearest` group's pre-pass: the colour draws' vertex stage and layout,
    /// no fragment stage, `Less` with depth writes on.
    pub(super) fn prepass(
        &self,
        device: &wgpu::Device,
        variant: MeshVariant,
    ) -> &wgpu::RenderPipeline {
        let set = self.prepass.get_or_init(|| {
            MeshVariant::ALL.map(|variant| {
                build_mesh_pipeline_with(
                    device,
                    self.label,
                    &self.shader,
                    &self.layout,
                    variant,
                    None,
                    wgpu::CompareFunction::Less,
                )
            })
        });
        &set[variant.index()]
    }
}

/// `src + dst` on colour, destination alpha kept: the premultiplied colour is simply added.
fn additive_blend() -> wgpu::BlendState {
    wgpu::BlendState {
        color: wgpu::BlendComponent {
            src_factor: wgpu::BlendFactor::One,
            dst_factor: wgpu::BlendFactor::One,
            operation: wgpu::BlendOperation::Add,
        },
        alpha: wgpu::BlendComponent {
            src_factor: wgpu::BlendFactor::Zero,
            dst_factor: wgpu::BlendFactor::One,
            operation: wgpu::BlendOperation::Add,
        },
    }
}

/// A mesh pipeline's fragment stage: its entry point, colour targets, blend and pipeline constants.
#[derive(Clone, Copy)]
struct MeshFragment<'a> {
    entry: &'a str,
    formats: &'a [wgpu::TextureFormat],
    constants: &'a [(&'a str, f64)],
    /// Every target's blend; `None` for the opaque fragments.
    blend: Option<wgpu::BlendState>,
    write_mask: wgpu::ColorWrites,
    depth_write: bool,
}

fn build_mesh_pipeline_for(
    device: &wgpu::Device,
    label: &str,
    shader: &wgpu::ShaderModule,
    layout: &wgpu::PipelineLayout,
    variant: MeshVariant,
    fragment: MeshFragment<'_>,
) -> wgpu::RenderPipeline {
    build_mesh_pipeline_with(
        device,
        label,
        shader,
        layout,
        variant,
        Some(fragment),
        wgpu::CompareFunction::Less,
    )
}

/// [`build_mesh_pipeline_for`] with the depth compare chosen; `fragment: None` builds a depth-only
/// pipeline that writes depth (the `nearest` pre-pass).
fn build_mesh_pipeline_with(
    device: &wgpu::Device,
    label: &str,
    shader: &wgpu::ShaderModule,
    layout: &wgpu::PipelineLayout,
    variant: MeshVariant,
    fragment: Option<MeshFragment<'_>>,
    compare: wgpu::CompareFunction,
) -> wgpu::RenderPipeline {
    let targets: Vec<Option<wgpu::ColorTargetState>> = fragment
        .iter()
        .flat_map(|fragment| {
            fragment.formats.iter().map(|&format| {
                Some(wgpu::ColorTargetState {
                    format,
                    blend: fragment.blend,
                    write_mask: fragment.write_mask,
                })
            })
        })
        .collect();
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
        fragment: fragment.as_ref().map(|fragment| wgpu::FragmentState {
            module: shader,
            entry_point: Some(fragment.entry),
            targets: &targets,
            compilation_options: wgpu::PipelineCompilationOptions {
                constants: fragment.constants,
                ..Default::default()
            },
        }),
        primitive: mesh_primitive(variant),
        depth_stencil: Some(depth_state(
            fragment.as_ref().is_none_or(|f| f.depth_write),
            compare,
        )),
        multisample: wgpu::MultisampleState::default(),
        multiview_mask: None,
        cache: None,
    })
}
