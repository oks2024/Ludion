//! Image-based lighting: GPU prefiltering of the analytic sky gradient into the mesh pass's IBL
//! inputs (`shaders/ibl.wgsl`).
//!
//! At environment load/change the [`Ibl`] runs three compute passes — a diffuse **irradiance** cube,
//! a roughness-mipped **specular** cube, and the split-sum **BRDF LUT** — from deterministic text
//! inputs (the sky colors). The BRDF LUT is environment-independent, so it is computed once in
//! [`Ibl::new`]; the two cubes are content-hash-cached (the mesh/texture-cache precedent), so a live
//! edit re-prefilters only when the sky colors actually change. Determinism gates on same-input ⇒
//! identical bytes on a pinned GPU (fixed sample counts, pure Hammersley, no RNG).
//!
//! A scene with no `Environment` binds 1×1 fallback cubes and the flag off, so the mesh shader takes
//! the exact historical flat-ambient path — byte-identical to before.

use std::cell::RefCell;

use ludion_render_graph::Gpu;

use crate::scene::EnvironmentData;
use crate::shader_gen::{self, PrefilterParams, SkyColors};

/// One cube face's edge length for the diffuse irradiance map. A smooth gradient needs no more — the
/// map is a low-frequency hemisphere integral.
const IRRADIANCE_SIZE: u32 = 32;
/// The specular prefilter cube's base (mip 0) face edge length.
const SPECULAR_SIZE: u32 = 64;
/// The specular prefilter's mip count (roughness levels 0..=`SPECULAR_MIPS-1`). Mip `m` is prefiltered
/// at roughness `m / (SPECULAR_MIPS-1)`; the mesh shader samples LOD `roughness * (SPECULAR_MIPS-1)`.
const SPECULAR_MIPS: u32 = 5;
/// The split-sum BRDF LUT's edge length (x = NdotV, y = roughness).
const BRDF_SIZE: u32 = 128;

/// The IBL working format for the prefiltered cubes and the LUT — HDR, storage-writable, filterable.
const IBL_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;

/// The resolved IBL inputs for one frame: the three sampled views the mesh pass binds, plus the
/// `EnvParams` the shader branches on. Views are cheap wgpu handle clones (an `Arc` bump each).
pub(crate) struct ResolvedEnv {
    pub irradiance: wgpu::TextureView,
    pub specular: wgpu::TextureView,
    pub brdf: wgpu::TextureView,
    /// `[flag, ibl_intensity, specular_max_lod, 0]` — the mesh `EnvParams.params` lane.
    pub params: [f32; 4],
}

/// A prefiltered environment, keyed by a content hash of its sky colors.
struct Cached {
    hash: u64,
    irradiance_cube: wgpu::TextureView,
    specular_cube: wgpu::TextureView,
}

/// The IBL prefilter: compute pipelines, the once-computed BRDF LUT, the 1×1 fallback cube, and the
/// single-entry content-hash cache.
pub(crate) struct Ibl {
    irradiance_pipeline: wgpu::ComputePipeline,
    prefilter_pipeline: wgpu::ComputePipeline,
    /// The environment-independent split-sum LUT (2D), computed once.
    brdf_view: wgpu::TextureView,
    /// A 1×1×6 cube bound to both cube slots on an env-less frame (flag off ⇒ never sampled).
    fallback_cube: wgpu::TextureView,
    /// The maximum specular LOD (`SPECULAR_MIPS - 1`), handed to the shader.
    spec_max_lod: f32,
    cache: RefCell<Option<Cached>>,
}

impl Ibl {
    /// Builds the compute pipelines and computes the BRDF LUT and the fallback cube.
    pub(crate) fn new(gpu: &Gpu) -> Ibl {
        let device = gpu.device();
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("ibl"),
            source: wgpu::ShaderSource::Wgsl(std::borrow::Cow::Borrowed(include_str!(
                "shaders/ibl.wgsl"
            ))),
        });
        let pipeline = |entry: &str| {
            device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some(entry),
                layout: None,
                module: &module,
                entry_point: Some(entry),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                cache: None,
            })
        };
        let irradiance_pipeline = pipeline("irradiance");
        let prefilter_pipeline = pipeline("prefilter");
        let brdf_pipeline = pipeline("brdf");

        let brdf_view = compute_brdf(gpu, &brdf_pipeline);
        let fallback_cube = fallback_cube(device);

        Ibl {
            irradiance_pipeline,
            prefilter_pipeline,
            brdf_view,
            fallback_cube,
            spec_max_lod: (SPECULAR_MIPS - 1) as f32,
            cache: RefCell::new(None),
        }
    }

    /// Resolves the IBL inputs for `env`. `None` ⇒ the fallback cubes and the flag off (the mesh
    /// shader keeps its flat-ambient path). `Some` ⇒ prefilter (cache-hit on an unchanged sky) and
    /// the flag on, carrying `ibl_intensity` and the specular max LOD.
    pub(crate) fn resolve(&self, gpu: &Gpu, env: Option<&EnvironmentData>) -> ResolvedEnv {
        let Some(env) = env else {
            return ResolvedEnv {
                irradiance: self.fallback_cube.clone(),
                specular: self.fallback_cube.clone(),
                brdf: self.brdf_view.clone(),
                params: [0.0, 0.0, self.spec_max_lod, 0.0],
            };
        };

        let hash = sky_hash(env);
        {
            let cache = self.cache.borrow();
            if let Some(c) = cache.as_ref().filter(|c| c.hash == hash) {
                return ResolvedEnv {
                    irradiance: c.irradiance_cube.clone(),
                    specular: c.specular_cube.clone(),
                    brdf: self.brdf_view.clone(),
                    params: [1.0, env.ibl_intensity, self.spec_max_lod, 0.0],
                };
            }
        }

        let (irradiance_cube, specular_cube) = self.prefilter(gpu, env);
        let out = ResolvedEnv {
            irradiance: irradiance_cube.clone(),
            specular: specular_cube.clone(),
            brdf: self.brdf_view.clone(),
            params: [1.0, env.ibl_intensity, self.spec_max_lod, 0.0],
        };
        *self.cache.borrow_mut() = Some(Cached {
            hash,
            irradiance_cube,
            specular_cube,
        });
        out
    }

    /// Runs the irradiance + per-mip specular compute passes for `env`, returning the two sampled
    /// **cube** views. One encoder, one submit.
    fn prefilter(
        &self,
        gpu: &Gpu,
        env: &EnvironmentData,
    ) -> (wgpu::TextureView, wgpu::TextureView) {
        let device = gpu.device();
        let colors_buf = uniform(device, "ibl colors", bytemuck::bytes_of(&sky_colors(env)));

        let irradiance = cube_texture(device, "irradiance", IRRADIANCE_SIZE, 1);
        let specular = cube_texture(device, "specular", SPECULAR_SIZE, SPECULAR_MIPS);

        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("ibl prefilter"),
        });

        // Diffuse irradiance: one storage view of all six layers at mip 0.
        {
            let out_view = storage_array_view(&irradiance, 0);
            // The irradiance entry point reads `colors` (0) and writes `irradiance_out` (2); its auto
            // layout expects both those slots.
            let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("ibl irradiance"),
                layout: &self.irradiance_pipeline.get_bind_group_layout(0),
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: shader_gen::COLORS_BINDING,
                        resource: colors_buf.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: shader_gen::IRRADIANCE_OUT_BINDING,
                        resource: wgpu::BindingResource::TextureView(&out_view),
                    },
                ],
            });
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("ibl irradiance"),
                timestamp_writes: None,
            });
            pass.set_pipeline(&self.irradiance_pipeline);
            pass.set_bind_group(0, &bind, &[]);
            let [wx, wy, _] = shader_gen::IBL_IRRADIANCE_WORKGROUP_SIZE;
            pass.dispatch_workgroups(
                IRRADIANCE_SIZE.div_ceil(wx),
                IRRADIANCE_SIZE.div_ceil(wy),
                6,
            );
        }

        // Specular: one dispatch per mip, its roughness in a per-mip uniform.
        for mip in 0..SPECULAR_MIPS {
            let size = (SPECULAR_SIZE >> mip).max(1);
            let roughness = if SPECULAR_MIPS <= 1 {
                0.0
            } else {
                mip as f32 / (SPECULAR_MIPS - 1) as f32
            };
            let params_buf = uniform(
                device,
                "ibl prefilter params",
                bytemuck::bytes_of(&prefilter_params(roughness, size as f32)),
            );
            let out_view = storage_array_view(&specular, mip);
            let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("ibl prefilter"),
                layout: &self.prefilter_pipeline.get_bind_group_layout(0),
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: shader_gen::COLORS_BINDING,
                        resource: colors_buf.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: shader_gen::PREFILTER_PARAMS_BINDING,
                        resource: params_buf.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: shader_gen::SPECULAR_OUT_BINDING,
                        resource: wgpu::BindingResource::TextureView(&out_view),
                    },
                ],
            });
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("ibl prefilter"),
                timestamp_writes: None,
            });
            pass.set_pipeline(&self.prefilter_pipeline);
            pass.set_bind_group(0, &bind, &[]);
            let [wx, wy, _] = shader_gen::IBL_PREFILTER_WORKGROUP_SIZE;
            pass.dispatch_workgroups(size.div_ceil(wx), size.div_ceil(wy), 6);
        }

        gpu.queue().submit(Some(encoder.finish()));

        (cube_view(&irradiance), cube_view(&specular))
    }
}

/// Content hash of the sky colors — the only prefilter inputs (disc/intensity/bloom don't affect the
/// prefiltered textures), so a change to those alone never re-prefilters.
fn sky_hash(env: &EnvironmentData) -> u64 {
    let mut bytes = Vec::with_capacity(36);
    for v in [env.sky_zenith, env.sky_horizon, env.sky_ground] {
        for c in v {
            bytes.extend_from_slice(&c.to_le_bytes());
        }
    }
    ludion_assets::content_hash(&bytes)
}

/// The sky's three colors as the `SkyColors` uniform (rgb, `w` pad).
fn sky_colors(env: &EnvironmentData) -> SkyColors {
    let vec4 = |[r, g, b]: [f32; 3]| [r, g, b, 0.0];
    SkyColors {
        zenith: vec4(env.sky_zenith),
        horizon: vec4(env.sky_horizon),
        ground: vec4(env.sky_ground),
    }
}

/// The `PrefilterParams` uniform: `[roughness, size, 0, 0]`.
fn prefilter_params(roughness: f32, size: f32) -> PrefilterParams {
    PrefilterParams {
        params: [roughness, size, 0.0, 0.0],
    }
}

fn uniform(device: &wgpu::Device, label: &str, data: &[u8]) -> wgpu::Buffer {
    use wgpu::util::DeviceExt;
    device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some(label),
        contents: data,
        usage: wgpu::BufferUsages::UNIFORM,
    })
}

/// A cube texture: 6 layers, `mips` levels, storage-writable and sampleable.
fn cube_texture(device: &wgpu::Device, label: &str, size: u32, mips: u32) -> wgpu::Texture {
    device.create_texture(&wgpu::TextureDescriptor {
        label: Some(label),
        size: wgpu::Extent3d {
            width: size,
            height: size,
            depth_or_array_layers: 6,
        },
        mip_level_count: mips,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: IBL_FORMAT,
        usage: wgpu::TextureUsages::STORAGE_BINDING | wgpu::TextureUsages::TEXTURE_BINDING,
        view_formats: &[],
    })
}

/// A write storage view of all six layers at mip `mip` (a `D2Array` the compute shader stores into).
fn storage_array_view(texture: &wgpu::Texture, mip: u32) -> wgpu::TextureView {
    texture.create_view(&wgpu::TextureViewDescriptor {
        label: Some("ibl storage view"),
        dimension: Some(wgpu::TextureViewDimension::D2Array),
        base_mip_level: mip,
        mip_level_count: Some(1),
        base_array_layer: 0,
        array_layer_count: Some(6),
        ..Default::default()
    })
}

/// A `Cube` sampling view over all mips of a 6-layer texture.
fn cube_view(texture: &wgpu::Texture) -> wgpu::TextureView {
    texture.create_view(&wgpu::TextureViewDescriptor {
        label: Some("ibl cube view"),
        dimension: Some(wgpu::TextureViewDimension::Cube),
        ..Default::default()
    })
}

/// A 1×1×6 cube bound on env-less frames — valid to bind, never sampled (the flag is off).
fn fallback_cube(device: &wgpu::Device) -> wgpu::TextureView {
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("ibl fallback cube"),
        size: wgpu::Extent3d {
            width: 1,
            height: 1,
            depth_or_array_layers: 6,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: IBL_FORMAT,
        usage: wgpu::TextureUsages::TEXTURE_BINDING,
        view_formats: &[],
    });
    cube_view(&texture)
}

/// Computes the split-sum BRDF LUT (once), returning its 2D sampling view.
fn compute_brdf(gpu: &Gpu, pipeline: &wgpu::ComputePipeline) -> wgpu::TextureView {
    let device = gpu.device();
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("ibl brdf lut"),
        size: wgpu::Extent3d {
            width: BRDF_SIZE,
            height: BRDF_SIZE,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: IBL_FORMAT,
        usage: wgpu::TextureUsages::STORAGE_BINDING | wgpu::TextureUsages::TEXTURE_BINDING,
        view_formats: &[],
    });
    let storage_view = texture.create_view(&wgpu::TextureViewDescriptor {
        label: Some("ibl brdf storage"),
        dimension: Some(wgpu::TextureViewDimension::D2),
        ..Default::default()
    });
    let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("ibl brdf"),
        layout: &pipeline.get_bind_group_layout(0),
        entries: &[wgpu::BindGroupEntry {
            binding: shader_gen::BRDF_OUT_BINDING,
            resource: wgpu::BindingResource::TextureView(&storage_view),
        }],
    });
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("ibl brdf"),
    });
    {
        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some("ibl brdf"),
            timestamp_writes: None,
        });
        pass.set_pipeline(pipeline);
        pass.set_bind_group(0, &bind, &[]);
        let [wx, wy, _] = shader_gen::IBL_BRDF_WORKGROUP_SIZE;
        pass.dispatch_workgroups(BRDF_SIZE.div_ceil(wx), BRDF_SIZE.div_ceil(wy), 1);
    }
    gpu.queue().submit(Some(encoder.finish()));
    texture.create_view(&wgpu::TextureViewDescriptor {
        label: Some("ibl brdf view"),
        dimension: Some(wgpu::TextureViewDimension::D2),
        ..Default::default()
    })
}
