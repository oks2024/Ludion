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

use crate::lobe;
use crate::scene::EnvironmentData;
use crate::shader_gen::{self, PrefilterParams, SkyColors};

/// One cube face's edge length for the diffuse irradiance map. A smooth gradient needs no more — the
/// map is a low-frequency hemisphere integral.
const IRRADIANCE_SIZE: u32 = 32;
/// The split-sum BRDF LUT's edge length (x = NdotV, y = roughness).
const BRDF_SIZE: u32 = 128;

/// The IBL working format for the prefiltered cubes and the LUT — HDR, storage-writable, filterable.
const IBL_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;

/// The resolved IBL inputs for one frame: the three sampled views (the mesh pass binds the
/// irradiance cube; the `reflections` pass binds the specular cube and the LUT), plus the
/// `EnvParams` the shader branches on. Views are cheap wgpu handle clones (an `Arc` bump each).
pub(crate) struct ResolvedEnv {
    pub irradiance: wgpu::TextureView,
    pub specular: wgpu::TextureView,
    pub brdf: wgpu::TextureView,
    /// `[flag, ibl_intensity, specular_max_lod, 0]` — lanes 0–1 feed `mesh.wgsl`'s `EnvParams.params`, lanes 0–2 the `reflections` uniform's `env`; the frame recorder fills lane 3 with the GI sky-visibility strength for both.
    pub params: [f32; 4],
}

/// A prefiltered environment, keyed by a content hash of its sky colors.
struct Cached {
    hash: u64,
    size: u32,
    irradiance_cube: wgpu::TextureView,
    specular_cube: wgpu::TextureView,
    /// The texture behind `specular_cube`, kept so tests can read the cube back.
    #[allow(dead_code, reason = "read by the tests")]
    specular_texture: wgpu::Texture,
}

/// The IBL prefilter: compute pipelines, the once-computed BRDF LUT, the 1×1 fallback cube, and the
/// single-entry content-hash cache.
pub(crate) struct Ibl {
    irradiance_pipeline: wgpu::ComputePipeline,
    prefilter_pipeline: wgpu::ComputePipeline,
    /// The probe path: the same lobe over a captured, mipped source cube.
    prefilter_cube_pipeline: wgpu::ComputePipeline,
    downsample_pipeline: wgpu::ComputePipeline,
    /// The environment-independent split-sum LUT (2D), computed once.
    brdf_view: wgpu::TextureView,
    /// The texture behind `brdf_view`, kept so tests can read the LUT back.
    #[allow(dead_code, reason = "read by the tests")]
    brdf_texture: wgpu::Texture,
    /// A 1×1×6 cube bound to both cube slots on an env-less frame (flag off ⇒ never sampled).
    fallback_cube: wgpu::TextureView,
    cache: RefCell<Option<Cached>>,
}

impl Ibl {
    /// Builds the compute pipelines and computes the BRDF LUT and the fallback cube.
    pub(crate) fn new(gpu: &Gpu) -> Ibl {
        let device = gpu.device();
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("ibl"),
            source: wgpu::ShaderSource::Wgsl(std::borrow::Cow::Borrowed(concat!(
                include_str!("shaders/snippets/dfg.wgsl"),
                "\n",
                include_str!("shaders/ibl.wgsl")
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
        let prefilter_cube_pipeline = pipeline("prefilter_cube");
        let downsample_pipeline = pipeline("downsample");
        let brdf_pipeline = pipeline("brdf");

        let (brdf_view, brdf_texture) = compute_brdf(gpu, &brdf_pipeline);
        let fallback_cube = fallback_cube(device);

        Ibl {
            irradiance_pipeline,
            prefilter_pipeline,
            prefilter_cube_pipeline,
            downsample_pipeline,
            brdf_view,
            brdf_texture,
            fallback_cube,
            cache: RefCell::new(None),
        }
    }

    /// Resolves the IBL inputs for `env`. `None` ⇒ the fallback cubes and the flag off (the mesh
    /// shader keeps its flat-ambient path). `Some` ⇒ prefilter (cache-hit on an unchanged sky) and
    /// the flag on, carrying `ibl_intensity` and the specular max LOD. The specular cube's face edge
    /// is `specular_size` (a power of two, at least 16); its chain stops at 8×8; a change of size re-prefilters like a change of sky.
    pub(crate) fn resolve(
        &self,
        gpu: &Gpu,
        env: Option<&EnvironmentData>,
        specular_size: u32,
    ) -> ResolvedEnv {
        debug_assert!(specular_size.is_power_of_two());
        let max_lod = lobe::chain_max_lod(specular_size) as f32;
        let Some(env) = env else {
            return ResolvedEnv {
                irradiance: self.fallback_cube.clone(),
                specular: self.fallback_cube.clone(),
                brdf: self.brdf_view.clone(),
                params: [0.0, 0.0, 0.0, 0.0],
            };
        };

        let hash = sky_hash(env);
        {
            let cache = self.cache.borrow();
            if let Some(c) = cache
                .as_ref()
                .filter(|c| c.hash == hash && c.size == specular_size)
            {
                return ResolvedEnv {
                    irradiance: c.irradiance_cube.clone(),
                    specular: c.specular_cube.clone(),
                    brdf: self.brdf_view.clone(),
                    params: [1.0, env.ibl_intensity, max_lod, 0.0],
                };
            }
        }

        let (irradiance_cube, specular_cube, specular_texture) =
            self.prefilter(gpu, env, specular_size);
        let out = ResolvedEnv {
            irradiance: irradiance_cube.clone(),
            specular: specular_cube.clone(),
            brdf: self.brdf_view.clone(),
            params: [1.0, env.ibl_intensity, max_lod, 0.0],
        };
        *self.cache.borrow_mut() = Some(Cached {
            hash,
            size: specular_size,
            irradiance_cube,
            specular_cube,
            specular_texture,
        });
        out
    }

    /// Runs the irradiance + per-mip specular compute passes for `env`, returning the two sampled
    /// **cube** views. One encoder, one submit.
    fn prefilter(
        &self,
        gpu: &Gpu,
        env: &EnvironmentData,
        specular_size: u32,
    ) -> (wgpu::TextureView, wgpu::TextureView, wgpu::Texture) {
        let max_lod = lobe::chain_max_lod(specular_size);
        let mips = max_lod + 1;
        let device = gpu.device();
        let colors_buf = uniform(device, "ibl colors", bytemuck::bytes_of(&sky_colors(env)));

        let irradiance = cube_texture(device, "irradiance", IRRADIANCE_SIZE, 1);
        let specular = cube_texture(device, "specular", specular_size, mips);

        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("ibl prefilter"),
        });

        // Diffuse irradiance: one storage view of all six layers at mip 0.
        {
            let out_view = storage_array_view(&irradiance, 0, 0);
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
        Self::dispatch_chain(
            device,
            &mut encoder,
            &self.prefilter_pipeline,
            &specular,
            0,
            [0.0, 0.0],
            |_, out_view, params_buf| {
                device.create_bind_group(&wgpu::BindGroupDescriptor {
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
                            resource: wgpu::BindingResource::TextureView(out_view),
                        },
                    ],
                })
            },
        );

        gpu.queue().submit(Some(encoder.finish()));

        (cube_view(&irradiance), cube_view(&specular), specular)
    }

    /// One `pipeline` dispatch per mip of the specular chain, into the six layers of `target` from
    /// `base_layer`. Mip `m` runs at `lobe::mip_roughness(m, max_lod)` and the face size of that
    /// mip; `source` fills the params uniform's `zw` (the probe path's source cube, zeros for the
    /// sky). `bind` builds the bind group from the mip, its storage view and the params buffer.
    /// The sky and the probes share this loop, so both integrate the same lobe per mip.
    fn dispatch_chain(
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        pipeline: &wgpu::ComputePipeline,
        target: &wgpu::Texture,
        base_layer: u32,
        source: [f32; 2],
        bind: impl Fn(u32, &wgpu::TextureView, &wgpu::Buffer) -> wgpu::BindGroup,
    ) {
        let max_lod = target.mip_level_count() - 1;
        for mip in 0..=max_lod {
            let size = (target.width() >> mip).max(1);
            let roughness = lobe::mip_roughness(mip, max_lod);
            let params_buf = uniform(
                device,
                "ibl prefilter params",
                bytemuck::bytes_of(&prefilter_params(roughness, size as f32, source)),
            );
            let out_view = storage_array_view(target, mip, base_layer);
            let bind = bind(mip, &out_view, &params_buf);
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("ibl prefilter"),
                timestamp_writes: None,
            });
            pass.set_pipeline(pipeline);
            pass.set_bind_group(0, &bind, &[]);
            let [wx, wy, _] = shader_gen::IBL_PREFILTER_WORKGROUP_SIZE;
            pass.dispatch_workgroups(size.div_ceil(wx), size.div_ceil(wy), 6);
        }
    }

    /// Prefilters a captured `source` cube into slot `slot` of the cube array `target`, in one
    /// encoder and one submit: the source's mip chain is box-filtered down, then each target mip
    /// integrates the lobe over it with filtered importance sampling.
    ///
    /// `source` is built by [`source_cube`]: face², six layers, a full chain to 1×1, `Rgba16Float`,
    /// `TEXTURE_BINDING | STORAGE_BINDING | COPY_DST`, with mip 0 filled by the caller. `target` is
    /// built by [`probe_array`]; slot `s` occupies layers `6s..6s + 6` and no other slot is written.
    pub(crate) fn prefilter_probe(
        &self,
        gpu: &Gpu,
        source: &wgpu::Texture,
        target: &wgpu::Texture,
        slot: u32,
    ) {
        let device = gpu.device();
        let src_mips = source.mip_level_count();
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("ibl probe prefilter"),
        });

        for mip in 1..src_mips {
            let src_view = source.create_view(&wgpu::TextureViewDescriptor {
                label: Some("ibl downsample src"),
                dimension: Some(wgpu::TextureViewDimension::D2Array),
                base_mip_level: mip - 1,
                mip_level_count: Some(1),
                ..Default::default()
            });
            let dst_view = storage_array_view(source, mip, 0);
            let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("ibl downsample"),
                layout: &self.downsample_pipeline.get_bind_group_layout(0),
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: shader_gen::DOWNSAMPLE_SRC_BINDING,
                        resource: wgpu::BindingResource::TextureView(&src_view),
                    },
                    wgpu::BindGroupEntry {
                        binding: shader_gen::DOWNSAMPLE_DST_BINDING,
                        resource: wgpu::BindingResource::TextureView(&dst_view),
                    },
                ],
            });
            let size = (source.width() >> mip).max(1);
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("ibl downsample"),
                timestamp_writes: None,
            });
            pass.set_pipeline(&self.downsample_pipeline);
            pass.set_bind_group(0, &bind, &[]);
            let [wx, wy, _] = shader_gen::IBL_DOWNSAMPLE_WORKGROUP_SIZE;
            pass.dispatch_workgroups(size.div_ceil(wx), size.div_ceil(wy), 6);
        }

        let source_view = cube_view(source);
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("ibl probe source"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Linear,
            ..Default::default()
        });
        Self::dispatch_chain(
            device,
            &mut encoder,
            &self.prefilter_cube_pipeline,
            target,
            6 * slot,
            [source.width() as f32, (src_mips - 1) as f32],
            |_, out_view, params_buf| {
                device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("ibl prefilter cube"),
                    layout: &self.prefilter_cube_pipeline.get_bind_group_layout(0),
                    entries: &[
                        wgpu::BindGroupEntry {
                            binding: shader_gen::PREFILTER_PARAMS_BINDING,
                            resource: params_buf.as_entire_binding(),
                        },
                        wgpu::BindGroupEntry {
                            binding: shader_gen::SPECULAR_OUT_BINDING,
                            resource: wgpu::BindingResource::TextureView(out_view),
                        },
                        wgpu::BindGroupEntry {
                            binding: shader_gen::SOURCE_CUBE_BINDING,
                            resource: wgpu::BindingResource::TextureView(&source_view),
                        },
                        wgpu::BindGroupEntry {
                            binding: shader_gen::SOURCE_SAMPLER_BINDING,
                            resource: wgpu::BindingResource::Sampler(&sampler),
                        },
                    ],
                })
            },
        );

        gpu.queue().submit(Some(encoder.finish()));
    }
}

/// A probe capture's source cube: `size`² faces, six layers, a full mip chain to 1×1 (the
/// downsample fills mips 1..), `Rgba16Float`; the caller fills mip 0.
pub(crate) fn source_cube(device: &wgpu::Device, size: u32) -> wgpu::Texture {
    device.create_texture(&wgpu::TextureDescriptor {
        label: Some("ibl probe source"),
        size: wgpu::Extent3d {
            width: size,
            height: size,
            depth_or_array_layers: 6,
        },
        mip_level_count: size.ilog2() + 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: IBL_FORMAT,
        usage: wgpu::TextureUsages::TEXTURE_BINDING
            | wgpu::TextureUsages::STORAGE_BINDING
            | wgpu::TextureUsages::COPY_SRC
            | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    })
}

/// The prefiltered probe cube array: `slots` cubes of `size`² faces (`6 · slots` layers) with the
/// sky's roughness chain, readable by a shader and by a copy and writable by a copy (a stored entry
/// is uploaded into a slot).
pub(crate) fn probe_array(device: &wgpu::Device, size: u32, slots: u32) -> wgpu::Texture {
    device.create_texture(&wgpu::TextureDescriptor {
        label: Some("ibl probe array"),
        size: wgpu::Extent3d {
            width: size,
            height: size,
            depth_or_array_layers: 6 * slots,
        },
        mip_level_count: lobe::chain_max_lod(size) + 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: IBL_FORMAT,
        usage: wgpu::TextureUsages::STORAGE_BINDING
            | wgpu::TextureUsages::TEXTURE_BINDING
            | wgpu::TextureUsages::COPY_SRC
            | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    })
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

/// The `PrefilterParams` uniform: `[roughness, size, source_size, source_max_mip]`; `source` is
/// zeros on the sky path.
fn prefilter_params(roughness: f32, size: f32, source: [f32; 2]) -> PrefilterParams {
    PrefilterParams {
        params: [roughness, size, source[0], source[1]],
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

/// A cube texture: 6 layers, `mips` levels, storage-writable, sampleable and copyable (tests read
/// the prefiltered cubes back).
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
        usage: wgpu::TextureUsages::STORAGE_BINDING
            | wgpu::TextureUsages::TEXTURE_BINDING
            | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    })
}

/// A write storage view of the six layers from `base_layer` at mip `mip` (a `D2Array` the compute
/// shader stores into).
fn storage_array_view(texture: &wgpu::Texture, mip: u32, base_layer: u32) -> wgpu::TextureView {
    texture.create_view(&wgpu::TextureViewDescriptor {
        label: Some("ibl storage view"),
        dimension: Some(wgpu::TextureViewDimension::D2Array),
        base_mip_level: mip,
        mip_level_count: Some(1),
        base_array_layer: base_layer,
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
fn compute_brdf(gpu: &Gpu, pipeline: &wgpu::ComputePipeline) -> (wgpu::TextureView, wgpu::Texture) {
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
        usage: wgpu::TextureUsages::STORAGE_BINDING
            | wgpu::TextureUsages::TEXTURE_BINDING
            | wgpu::TextureUsages::COPY_SRC,
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
    let view = texture.create_view(&wgpu::TextureViewDescriptor {
        label: Some("ibl brdf view"),
        dimension: Some(wgpu::TextureViewDimension::D2),
        ..Default::default()
    });
    (view, texture)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// The size is part of the cache key and sets the lod range the shader is told.
    #[test]
    fn needs_gpu_the_sky_cube_follows_the_quality_size() {
        let gpu = match Gpu::request() {
            Ok(gpu) => gpu,
            Err(d) if ludion_core::require_gpu::is_no_adapter(std::slice::from_ref(&d)) => {
                eprintln!("SKIPPED the sky cube size test: no GPU adapter");
                return;
            }
            Err(d) => panic!("gpu request failed: {d:?}"),
        };
        let env = EnvironmentData {
            sky_zenith: [0.1, 0.2, 0.6],
            sky_horizon: [0.7, 0.7, 0.8],
            sky_ground: [0.1, 0.1, 0.1],
            sun_disc_size: 0.0,
            sun_disc_intensity: 0.0,
            ambient: [0.0; 3],
            ibl_intensity: 1.0,
        };
        let ibl = Ibl::new(&gpu);
        let lod = |size| ibl.resolve(&gpu, Some(&env), size).params[2];
        assert_eq!([lod(256), lod(128), lod(256)], [5.0, 4.0, 5.0]);
    }

    fn gpu_or_skip(what: &str) -> Option<Gpu> {
        match Gpu::request() {
            Ok(gpu) => Some(gpu),
            Err(d) if ludion_core::require_gpu::is_no_adapter(std::slice::from_ref(&d)) => {
                eprintln!("SKIPPED {what}: no GPU adapter");
                None
            }
            Err(d) => panic!("gpu request failed: {d:?}"),
        }
    }

    pub(crate) fn test_env() -> EnvironmentData {
        EnvironmentData {
            sky_zenith: [0.1, 0.2, 0.6],
            sky_horizon: [0.7, 0.7, 0.8],
            sky_ground: [0.1, 0.1, 0.1],
            sun_disc_size: 0.0,
            sun_disc_intensity: 0.0,
            ambient: [0.0; 3],
            ibl_intensity: 1.0,
        }
    }

    /// Reads every mip of a 6-layer-multiple `Rgba16Float` texture back: per mip, per layer, rgba
    /// `f32` texels row-major. Unpads the 256-byte row alignment.
    pub(crate) fn read_rgba16_mips(gpu: &Gpu, texture: &wgpu::Texture) -> Vec<Vec<f32>> {
        let device = gpu.device();
        let layers = texture.depth_or_array_layers();
        (0..texture.mip_level_count())
            .map(|mip| {
                let edge = (texture.width() >> mip).max(1);
                let row = (edge * 8).div_ceil(256) * 256;
                let buffer = device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("rgba16 readback"),
                    size: u64::from(row * edge * layers),
                    usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                    mapped_at_creation: false,
                });
                let mut encoder =
                    device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
                encoder.copy_texture_to_buffer(
                    wgpu::TexelCopyTextureInfo {
                        texture,
                        mip_level: mip,
                        origin: wgpu::Origin3d::ZERO,
                        aspect: wgpu::TextureAspect::All,
                    },
                    wgpu::TexelCopyBufferInfo {
                        buffer: &buffer,
                        layout: wgpu::TexelCopyBufferLayout {
                            offset: 0,
                            bytes_per_row: Some(row),
                            rows_per_image: Some(edge),
                        },
                    },
                    wgpu::Extent3d {
                        width: edge,
                        height: edge,
                        depth_or_array_layers: layers,
                    },
                );
                gpu.queue().submit(Some(encoder.finish()));
                let (tx, rx) = std::sync::mpsc::channel();
                buffer.slice(..).map_async(wgpu::MapMode::Read, move |r| {
                    let _ = tx.send(r);
                });
                device
                    .poll(wgpu::PollType::wait_indefinitely())
                    .expect("the device polls");
                rx.recv().unwrap().expect("the buffer maps");
                let mapped = buffer.slice(..).get_mapped_range().expect("mapped range");
                let mut out = Vec::new();
                for layer in 0..layers {
                    for y in 0..edge {
                        for x in 0..edge * 4 {
                            let at = ((layer * edge + y) * row + x * 2) as usize;
                            let bits = u16::from_le_bytes([mapped[at], mapped[at + 1]]);
                            out.push(crate::gi_bake::f16_to_f32(bits));
                        }
                    }
                }
                out
            })
            .collect()
    }

    /// The LUT's two channels (R = scale, G = bias) by midpoint quadrature over the hemisphere, in f64:
    /// ∫ (1 or Fc) · D · V · ndl dω, with `correlated` choosing the height-correlated visibility the
    /// LUT integrates or the old uncorrelated Schlick-Smith (k = α/2) it replaced.
    fn integrate_dfg(
        ndv: f64,
        roughness: f64,
        correlated: bool,
        mu_steps: u32,
        phi_steps: u32,
    ) -> [f64; 2] {
        let alpha = roughness * roughness;
        let a2 = alpha * alpha;
        let v = [(1.0 - ndv * ndv).sqrt(), 0.0, ndv];
        let (mut a, mut b) = (0.0, 0.0);
        for i in 0..mu_steps {
            let ndl = (f64::from(i) + 0.5) / f64::from(mu_steps);
            let sin_l = (1.0 - ndl * ndl).sqrt();
            for j in 0..phi_steps {
                let phi = std::f64::consts::PI * (f64::from(j) + 0.5) / f64::from(phi_steps);
                let l = [sin_l * phi.cos(), sin_l * phi.sin(), ndl];
                let h = [l[0] + v[0], l[1] + v[1], l[2] + v[2]];
                let len = (h[0] * h[0] + h[1] * h[1] + h[2] * h[2]).sqrt();
                let h = [h[0] / len, h[1] / len, h[2] / len];
                let ndh = h[2];
                let vdh = v[0] * h[0] + v[1] * h[1] + v[2] * h[2];
                let d_ndh = ndh * ndh * (a2 - 1.0) + 1.0;
                let d = a2 / (std::f64::consts::PI * d_ndh * d_ndh);
                let vis = if correlated {
                    let gv = ndl * (ndv * ndv * (1.0 - a2) + a2).sqrt();
                    let gl = ndv * (ndl * ndl * (1.0 - a2) + a2).sqrt();
                    0.5 / (gv + gl)
                } else {
                    let k = alpha / 2.0;
                    let g = ndv / (ndv * (1.0 - k) + k) * (ndl / (ndl * (1.0 - k) + k));
                    g / (4.0 * ndl * ndv)
                };
                let fc = (1.0 - vdh).powi(5);
                let w = d * vis * ndl;
                a += (1.0 - fc) * w;
                b += fc * w;
            }
        }
        // dω = dμ dφ; φ covers half the circle, doubled by symmetry.
        let cell = 2.0 * std::f64::consts::PI / (f64::from(mu_steps) * 2.0 * f64::from(phi_steps));
        [a * cell * 2.0, b * cell * 2.0]
    }

    /// The LUT is the numeric integral of the direct lobe's own BRDF, so R + G is its single-scatter
    /// albedo at F0 = 1.
    #[test]
    fn needs_gpu_the_dfg_lut_matches_a_numeric_integration() {
        let Some(gpu) = gpu_or_skip("the DFG LUT oracle test") else {
            return;
        };
        let ibl = Ibl::new(&gpu);
        let lut = read_rgba16_mips(&gpu, &ibl.brdf_texture).remove(0);
        let size = BRDF_SIZE as usize;
        let texel = |x: usize, y: usize| {
            [
                f64::from(lut[(y * size + x) * 4]),
                f64::from(lut[(y * size + x) * 4 + 1]),
            ]
        };
        let center = |i: usize| (i as f64 + 0.5) / f64::from(BRDF_SIZE);
        let mut max_err = 0.0_f64;
        // (roughness, ndv), the review's points first.
        let points = [(0.25, 0.2), (0.5, 0.2), (0.5, 0.5), (0.75, 0.5), (1.0, 0.5)];
        for (r, ndv) in points {
            let x = ((ndv * f64::from(BRDF_SIZE)) as usize).min(size - 1);
            let y = ((r * f64::from(BRDF_SIZE)) as usize).min(size - 1);
            let got = texel(x, y);
            let want = integrate_dfg(center(x), center(y), true, 2048, 1024);
            for c in 0..2 {
                let err = (got[c] - want[c]).abs();
                max_err = max_err.max(err);
                assert!(
                    err < 0.015,
                    "LUT[{c}] at r={r} ndv={ndv}: gpu {} vs integral {}",
                    got[c],
                    want[c]
                );
            }
            eprintln!("dfg r={r} ndv={ndv}: gpu {got:?} integral {want:?}");
        }
        eprintln!("dfg LUT max error {max_err}");
        // The review's single-scatter albedos, with the corrected and the old visibility.
        let review = [
            (0.25, 0.2, 0.95, 0.79),
            (0.5, 0.2, 0.85, 0.66),
            (0.5, 0.5, 0.86, 0.75),
        ];
        for (r, ndv, new, old) in review {
            let x = ((ndv * f64::from(BRDF_SIZE)) as usize).min(size - 1);
            let y = ((r * f64::from(BRDF_SIZE)) as usize).min(size - 1);
            let [a, b] = integrate_dfg(center(x), center(y), true, 2048, 1024);
            let [oa, ob] = integrate_dfg(center(x), center(y), false, 2048, 1024);
            eprintln!("dfg r={r} ndv={ndv}: a+b new {} old {}", a + b, oa + ob);
            assert!(
                (a + b - new).abs() < 0.02,
                "correlated a+b at r={r} ndv={ndv}: {}",
                a + b
            );
            assert!(
                (oa + ob - old).abs() < 0.02,
                "uncorrelated a+b at r={r} ndv={ndv}: {}",
                oa + ob
            );
        }
    }

    /// The sky specular cube's bytes at 128², every mip. Recorded on macOS (the unpinned M5); the
    /// Environment goldens are the cross-machine proof, this pins the shared chain on this one.
    const SKY_CUBE_HASH_MACOS: u64 = 8321340599969571538;

    #[test]
    fn needs_gpu_the_sky_cube_bytes_are_unchanged_by_the_shared_chain() {
        let Some(gpu) = gpu_or_skip("the sky cube bytes test") else {
            return;
        };
        let ibl = Ibl::new(&gpu);
        ibl.resolve(&gpu, Some(&test_env()), 128);
        let texture = ibl
            .cache
            .borrow()
            .as_ref()
            .unwrap()
            .specular_texture
            .clone();
        let mut bytes = Vec::new();
        for mip in read_rgba16_mips(&gpu, &texture) {
            for v in mip {
                bytes.extend_from_slice(&v.to_le_bytes());
            }
        }
        let hash = ludion_assets::content_hash(&bytes);
        if std::env::consts::OS == "macos" {
            assert_eq!(hash, SKY_CUBE_HASH_MACOS, "the sky specular cube moved");
        } else {
            eprintln!("SKIPPED the sky cube comparison: hash {hash} is recorded on macOS only");
        }
    }

    /// Rust twin of `ibl.wgsl`'s `face_dir`.
    pub(crate) fn face_dir(face: u32, u: f32, v: f32) -> [f32; 3] {
        let d = match face {
            0 => [1.0, -v, -u],
            1 => [-1.0, -v, u],
            2 => [u, 1.0, v],
            3 => [u, -1.0, -v],
            4 => [u, -v, 1.0],
            _ => [-u, -v, -1.0],
        };
        let len = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
        [d[0] / len, d[1] / len, d[2] / len]
    }

    /// Rust twin of `ibl.wgsl`'s `sky_gradient`.
    fn sky_gradient(env: &EnvironmentData, dir: [f32; 3]) -> [f32; 3] {
        let t = dir[1];
        let (far, k) = if t >= 0.0 {
            (env.sky_zenith, t.clamp(0.0, 1.0).sqrt())
        } else {
            (env.sky_ground, (-t).clamp(0.0, 1.0).sqrt())
        };
        std::array::from_fn(|c| env.sky_horizon[c] + (far[c] - env.sky_horizon[c]) * k)
    }

    const LOBE_SAMPLES: u32 = 1 << 16;
    /// The sample count of the direction search, which prefilters many candidate directions per point.
    const SEARCH_SAMPLES: u32 = 1 << 14;

    fn dot3(a: [f64; 3], b: [f64; 3]) -> f64 {
        a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
    }

    fn norm3(a: [f64; 3]) -> [f64; 3] {
        let len = dot3(a, a).sqrt();
        a.map(|x| x / len)
    }

    fn cross3(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
        [
            a[1] * b[2] - a[2] * b[1],
            a[2] * b[0] - a[0] * b[2],
            a[0] * b[1] - a[1] * b[0],
        ]
    }

    /// `reflect(-v, h)`: the light direction a facet with normal `h` sends to the viewer `v`.
    fn mirror(v: [f64; 3], h: [f64; 3]) -> [f64; 3] {
        let k = 2.0 * dot3(v, h);
        [k * h[0] - v[0], k * h[1] - v[1], k * h[2] - v[2]]
    }

    /// Hammersley point `i` of `n`.
    fn hammersley(i: u32, count: u32) -> (f64, f64) {
        (
            (f64::from(i) + 0.5) / f64::from(count),
            f64::from(i.reverse_bits()) / 4_294_967_296.0,
        )
    }

    /// A GGX half vector (`a2 = α²`) around `n`, by importance sampling `D·ndh`.
    fn ggx_half(n: [f64; 3], u: f64, v: f64, a2: f64) -> [f64; 3] {
        let cos2 = (1.0 - u) / (1.0 + (a2 - 1.0) * u);
        let (cos, sin) = (cos2.sqrt(), (1.0 - cos2).max(0.0).sqrt());
        let phi = 2.0 * std::f64::consts::PI * v;
        let up = if n[2].abs() < 0.999 {
            [0.0, 0.0, 1.0]
        } else {
            [1.0, 0.0, 0.0]
        };
        let t = norm3(cross3(up, n));
        let b = cross3(n, t);
        let (x, y) = (sin * phi.cos(), sin * phi.sin());
        std::array::from_fn(|c| t[c] * x + b[c] * y + n[c] * cos)
    }

    /// Height-correlated Smith visibility with the `1/(4·ndl·ndv)` factor folded in (`dfg.wgsl`).
    fn visibility_smith(ndl: f64, ndv: f64, alpha: f64) -> f64 {
        let a2 = alpha * alpha;
        let gv = ndl * (ndv * ndv * (1.0 - a2) + a2).sqrt();
        let gl = ndv * (ndl * ndl * (1.0 - a2) + a2).sqrt();
        0.5 / (gv + gl)
    }

    /// The BRDF-weighted mean of `f` over the floor's (`n = +Y`) visible light directions for viewer
    /// `v`: samples `h ~ D`, keeps `ndl > 0`, weights `4·V·ndl·vdh/ndh`, the split sum's own quantity.
    fn floor_lobe_mean(v: [f64; 3], alpha: f64, f: impl Fn([f64; 3]) -> f64) -> f64 {
        let a2 = alpha * alpha;
        let (mut sum, mut norm) = (0.0, 0.0);
        for i in 0..LOBE_SAMPLES {
            let (u, w) = hammersley(i, LOBE_SAMPLES);
            let h = ggx_half([0.0, 1.0, 0.0], u, w, a2);
            let l = mirror(v, h);
            let (ndl, ndv, vdh, ndh) = (l[1], v[1], dot3(v, h), h[1]);
            if ndl <= 0.0 || vdh <= 0.0 {
                continue;
            }
            let weight = 4.0 * visibility_smith(ndl, ndv, alpha) * ndl * vdh / ndh;
            sum += f(l) * weight;
            norm += weight;
        }
        sum / norm
    }

    /// The ideal prefilter along `d` (Karis: `N = V = d`, weight `(d·l)+`), over the full sphere.
    fn prefiltered(d: [f64; 3], alpha: f64, count: u32, f: impl Fn([f64; 3]) -> f64) -> f64 {
        let a2 = alpha * alpha;
        let (mut sum, mut norm) = (0.0, 0.0);
        for i in 0..count {
            let (u, w) = hammersley(i, count);
            let l = mirror(d, ggx_half(d, u, w, a2));
            let weight = dot3(d, l).max(0.0);
            sum += f(l) * weight;
            norm += weight;
        }
        sum / norm
    }

    /// Frostbite 2014 §4.9.3: the direction the specular lobe's mean radiance actually comes from.
    fn dominant_direction(n: [f64; 3], r: [f64; 3], alpha: f64) -> [f64; 3] {
        let s = 1.0 - alpha;
        let f = s * (s.sqrt() + alpha);
        norm3(std::array::from_fn(|c| n[c] + (r[c] - n[c]) * f))
    }

    fn sky_luma(env: &EnvironmentData, d: [f64; 3]) -> f64 {
        let c = sky_gradient(env, d.map(|x| x as f32));
        0.2126 * f64::from(c[0]) + 0.7152 * f64::from(c[1]) + 0.0722 * f64::from(c[2])
    }

    fn overcast_env() -> EnvironmentData {
        EnvironmentData {
            sky_zenith: [0.015, 0.028, 0.075],
            sky_horizon: [0.07, 0.1, 0.17],
            sky_ground: [0.008, 0.01, 0.018],
            ..test_env()
        }
    }

    fn viewer(ndv: f64) -> [f64; 3] {
        [(1.0 - ndv * ndv).sqrt(), ndv, 0.0]
    }

    /// Prefiltered radiance along the mirror direction R, against the BRDF-weighted mean radiance a
    /// floor (`n = +Y`, nothing below it) actually reflects, with and without the dominant-direction
    /// bend. Prints the table `rel_err_R` / `rel_err_dom` per sky, roughness and `ndv`, beside two
    /// alternatives at fewer samples: Filament's `normalize(mix(r, n, α²))` and an oracle that picks the
    /// best of 16 directions from R to n, which bounds what any direction fix can reach.
    ///
    /// CONFIRMED if, for roughness >= 0.5 and ndv <= 0.3, the worst `rel_err_R` is >= 0.10 on either
    /// sky and the dominant direction at least halves it (mean over those points and at the worst
    /// point) without raising any error above 0.05; REFUTED if `rel_err_R` is < 0.05 everywhere or the
    /// dominant direction is no better there; otherwise INCONCLUSIVE.
    #[test]
    fn dominant_direction_against_a_full_brdf_integral() {
        let flat = EnvironmentData {
            sky_zenith: [0.3; 3],
            sky_horizon: [0.3; 3],
            sky_ground: [0.3; 3],
            ..test_env()
        };
        let probe_v = viewer(0.5);
        let (ref_flat, pre_flat) = (
            floor_lobe_mean(probe_v, 0.25, |l| sky_luma(&flat, l)),
            prefiltered(probe_v, 0.25, LOBE_SAMPLES, |l| sky_luma(&flat, l)),
        );
        let flat_luma = sky_luma(&flat, [0.0, 1.0, 0.0]);
        assert!((ref_flat - flat_luma).abs() < 1e-9 && (pre_flat - flat_luma).abs() < 1e-9);
        let r = [-viewer(0.8)[0], 0.8, 0.0];
        let sharp = prefiltered(r, 0.02 * 0.02, LOBE_SAMPLES, |l| {
            sky_luma(&overcast_env(), l)
        });
        let want = sky_luma(&overcast_env(), r);
        assert!(
            (sharp - want).abs() <= 0.01 * want,
            "mirror prefilter {sharp} vs the sky {want}"
        );
        let (mut region_r, mut region_dom, mut region_n) = (0.0, 0.0, 0);
        let (mut worst_r, mut worst_r_dom, mut worst_all_r) = (0.0_f64, 0.0, 0.0_f64);
        let (mut worst_rise, mut worst_dom_err) = (f64::MIN, 0.0_f64);
        // Per alternative direction: [region error sum, region worst, worst increase over R anywhere].
        let (mut filament, mut oracle) = ([0.0_f64, 0.0, f64::MIN], [0.0_f64, 0.0, f64::MIN]);
        for (name, env) in [("test_env", test_env()), ("blend-metal", overcast_env())] {
            println!(
                "{name}: roughness ndv  L_ref  L_pre(R)  L_pre(dom)  rel_err_R  rel_err_dom  \
                 rel_err_filament  rel_err_oracle (best angle t of 15, R to n)"
            );
            for roughness in [0.25, 0.5, 0.75, 1.0] {
                let alpha = roughness * roughness;
                for ndv in [0.05, 0.1, 0.2, 0.3, 0.5, 0.8, 1.0] {
                    let v = viewer(ndv);
                    let n = [0.0, 1.0, 0.0];
                    let r = [-v[0], v[1], 0.0];
                    let reference = floor_lobe_mean(v, alpha, |l| sky_luma(&env, l));
                    let at_r = prefiltered(r, alpha, LOBE_SAMPLES, |l| sky_luma(&env, l));
                    let dom = dominant_direction(n, r, alpha);
                    let at_dom = prefiltered(dom, alpha, LOBE_SAMPLES, |l| sky_luma(&env, l));
                    let (err_r, err_dom) = (
                        (at_r - reference).abs() / reference,
                        (at_dom - reference).abs() / reference,
                    );
                    // Filament: normalize(mix(r, n, α²)); the oracle: the best of 16 directions on the
                    // great circle from R to n, by the same error.
                    let search = |d: [f64; 3]| {
                        let at = prefiltered(d, alpha, SEARCH_SAMPLES, |l| sky_luma(&env, l));
                        (at - reference).abs() / reference
                    };
                    let a2 = alpha * alpha;
                    let err_fil = search(norm3(std::array::from_fn(|c| r[c] + (n[c] - r[c]) * a2)));
                    let angle = dot3(n, r).clamp(-1.0, 1.0).acos();
                    let (err_ora, best_t) = (0..16)
                        .map(|t| {
                            let k = f64::from(t) / 15.0;
                            let d = if angle < 1e-6 {
                                r
                            } else {
                                let (wr, wn) = (((1.0 - k) * angle).sin(), (k * angle).sin());
                                norm3(std::array::from_fn(|c| wr * r[c] + wn * n[c]))
                            };
                            (search(d), t)
                        })
                        .min_by(|a, b| a.0.total_cmp(&b.0))
                        .expect("16 candidates");
                    println!(
                        "  {roughness:.2} {ndv:.2}  {reference:.5}  {at_r:.5}  {at_dom:.5}  {err_r:.4}  \
                         {err_dom:.4}  {err_fil:.4}  {err_ora:.4} (t={best_t})"
                    );
                    for (acc, err) in [(&mut filament, err_fil), (&mut oracle, err_ora)] {
                        acc[2] = acc[2].max(err - err_r);
                        if roughness >= 0.5 && ndv <= 0.3 {
                            acc[0] += err;
                            acc[1] = acc[1].max(err);
                        }
                    }
                    worst_all_r = worst_all_r.max(err_r);
                    worst_rise = worst_rise.max(err_dom - err_r);
                    worst_dom_err = worst_dom_err.max(err_dom);
                    if roughness >= 0.5 && ndv <= 0.3 {
                        region_r += err_r;
                        region_dom += err_dom;
                        region_n += 1;
                        if err_r > worst_r {
                            (worst_r, worst_r_dom) = (err_r, err_dom);
                        }
                    }
                }
            }
        }
        let (mean_r, mean_dom) = (
            region_r / f64::from(region_n),
            region_dom / f64::from(region_n),
        );
        println!(
            "rough>=0.5, ndv<=0.3: mean rel_err_R {mean_r:.4} -> dom {mean_dom:.4}; worst rel_err_R \
             {worst_r:.4} -> dom {worst_r_dom:.4}; worst rel_err_R anywhere {worst_all_r:.4}; worst \
             dom increase over R {worst_rise:.4}; worst rel_err_dom anywhere {worst_dom_err:.4}"
        );
        for (name, acc) in [("Filament mix(r, n, α²)", filament), ("oracle", oracle)] {
            println!(
                "{name}: region mean {:.4}, region worst {:.4}, worst increase over R {:.4}",
                acc[0] / f64::from(region_n),
                acc[1],
                acc[2]
            );
        }
        let halves = mean_dom <= 0.5 * mean_r && worst_r_dom <= 0.5 * worst_r;
        let verdict = if worst_r >= 0.10 && halves && worst_dom_err <= 0.05 {
            "CONFIRMED"
        } else if worst_all_r < 0.05 || mean_dom >= mean_r {
            "REFUTED"
        } else {
            "INCONCLUSIVE"
        };
        println!("dominant direction verdict: {verdict}");
    }

    /// Raw ambient occlusion against Lagarde's specular occlusion, for a cone (Jimenez's aperture around
    /// `n`) and a V-groove crease, against the visible fraction of the BRDF-weighted floor lobe. Prints
    /// the mean and max error per model for raw, Lagarde with `α` and Lagarde with perceptual roughness.
    ///
    /// CONFIRMED if in both models raw's mean abs error is >= 0.05 and Lagarde(α)'s is <= 0.5x raw's;
    /// REFUTED if raw's mean is < 0.03 in both or Lagarde(α) is no better than raw in both; otherwise
    /// INCONCLUSIVE.
    #[test]
    fn specular_occlusion_form_against_visibility_models() {
        let deg = std::f64::consts::PI / 180.0;
        // ∫ vis · ndl dω / π over the upper hemisphere, by cosine-weighted sampling.
        let ao_of = |vis: &dyn Fn([f64; 3]) -> bool| -> f64 {
            let mut seen = 0.0;
            for i in 0..LOBE_SAMPLES {
                let (u, w) = hammersley(i, LOBE_SAMPLES);
                let (cos, phi) = ((1.0 - u).sqrt(), 2.0 * std::f64::consts::PI * w);
                let sin = u.sqrt();
                seen += f64::from(u8::from(vis([sin * phi.cos(), cos, sin * phi.sin()])));
            }
            seen / f64::from(LOBE_SAMPLES)
        };
        let lagarde = |ndv: f64, ao: f64, exponent_source: f64| -> f64 {
            ((ndv + ao).powf((-16.0 * exponent_source - 1.0).exp2()) - 1.0 + ao).clamp(0.0, 1.0)
        };

        // A cone opened to 90 degrees and a crease at 0 degrees see the whole hemisphere.
        let wall_flat = [0.0, 1.0, 0.0];
        for ndv in [0.1, 0.9] {
            for roughness in [0.1, 0.9_f64] {
                let cone = floor_lobe_mean(viewer(ndv), roughness * roughness, |l| {
                    f64::from(u8::from(l[1] >= 0.0))
                });
                let crease = floor_lobe_mean(viewer(ndv), roughness * roughness, |l| {
                    f64::from(u8::from(l[1] > 0.0 && dot3(l, wall_flat) >= 0.0))
                });
                assert!((cone - 1.0).abs() < 1e-9 && (crease - 1.0).abs() < 1e-9);
            }
        }

        // (model, ao, so_ref, ndv, roughness) per configuration.
        let mut cone = Vec::new();
        let mut crease = Vec::new();
        for ao in [0.3_f64, 0.5, 0.7, 0.9] {
            let cos_v = (1.0 - ao).sqrt();
            let vis = move |l: [f64; 3]| l[1] >= cos_v;
            for ndv in [0.1, 0.3, 0.6, 0.9] {
                for roughness in [0.1, 0.3, 0.6, 0.9_f64] {
                    let so = floor_lobe_mean(viewer(ndv), roughness * roughness, |l| {
                        f64::from(u8::from(vis(l)))
                    });
                    cone.push((ao, so, ndv, roughness));
                }
            }
        }
        for gamma in [30.0_f64, 60.0, 90.0, 120.0] {
            let wall = [-(gamma * deg).sin(), (gamma * deg).cos(), 0.0];
            let vis = move |l: [f64; 3]| l[1] > 0.0 && dot3(l, wall) >= 0.0;
            let ao = ao_of(&vis);
            assert!(ao > 0.0 && ao < 1.0, "crease {gamma}: ao {ao}");
            for dir in [[1.0, 0.0], [-1.0, 0.0], [0.0, 1.0]] {
                for ndv in [0.1_f64, 0.3, 0.6, 0.9] {
                    let s = (1.0 - ndv * ndv).sqrt();
                    let v = [s * dir[0], ndv, s * dir[1]];
                    for roughness in [0.1, 0.3, 0.6, 0.9_f64] {
                        let so = floor_lobe_mean(v, roughness * roughness, |l| {
                            f64::from(u8::from(vis(l)))
                        });
                        crease.push((ao, so, ndv, roughness));
                    }
                }
            }
        }

        let mut means = Vec::new();
        for (name, rows) in [("cone", &cone), ("crease", &crease)] {
            let (mut raw, mut by_alpha, mut by_rough) = ([0.0_f64; 2], [0.0_f64; 2], [0.0_f64; 2]);
            for &(ao, so, ndv, roughness) in rows {
                for (acc, form) in [
                    (&mut raw, ao),
                    (&mut by_alpha, lagarde(ndv, ao, roughness * roughness)),
                    (&mut by_rough, lagarde(ndv, ao, roughness)),
                ] {
                    let err = (form - so).abs();
                    acc[0] += err;
                    acc[1] = acc[1].max(err);
                }
            }
            let n = rows.len() as f64;
            println!(
                "{name} ({} configs): mean|max abs error  raw {:.4}|{:.4}  Lagarde(alpha) {:.4}|{:.4}  \
                 Lagarde(perceptual) {:.4}|{:.4}",
                rows.len(),
                raw[0] / n,
                raw[1],
                by_alpha[0] / n,
                by_alpha[1],
                by_rough[0] / n,
                by_rough[1]
            );
            means.push((raw[0] / n, by_alpha[0] / n));
        }
        let confirmed = means.iter().all(|&(raw, l)| raw >= 0.05 && l <= 0.5 * raw);
        let refuted =
            means.iter().all(|&(raw, _)| raw < 0.03) || means.iter().all(|&(raw, l)| l >= raw);
        let verdict = if confirmed {
            "CONFIRMED"
        } else if refuted {
            "REFUTED"
        } else {
            "INCONCLUSIVE"
        };
        println!("specular occlusion verdict: {verdict}");
    }

    /// Round-to-nearest binary16 for finite, normal-range values (the sky is neither zero nor huge).
    fn f32_to_f16(v: f32) -> u16 {
        let bits = v.to_bits();
        let sign = ((bits >> 16) & 0x8000) as u16;
        let exp = ((bits >> 23) & 0xff) as i32 - 127 + 15;
        assert!(
            (1..31).contains(&exp),
            "{v} is outside the normal f16 range"
        );
        let mant = bits & 0x007f_ffff;
        let mut half = ((exp as u32) << 10) | (mant >> 13);
        if mant & 0x1000 != 0 && (mant & 0x0fff != 0 || half & 1 != 0) {
            half += 1;
        }
        sign | half as u16
    }

    /// A probe prefiltered from a CPU-written copy of the analytic sky matches the sky cube: within
    /// 1% per channel at mip 0 (the f16 source, 0.1% observed), alpha within 1e-3 of 1, and the
    /// neighbouring slot untouched. Above mip 0 the probe reads a box-mipped source where the sky
    /// samples the gradient analytically, so the filtered importance sampling blurs the gradient's
    /// horizon kink and the gap grows with roughness (median 5% / worst 27% at the roughest mip on
    /// the M5); the bounds there are median 6% and worst 35% per channel, relative to the sky.
    #[test]
    fn needs_gpu_a_sky_filled_source_prefilters_to_the_sky_cube() {
        let Some(gpu) = gpu_or_skip("the sky-filled source test") else {
            return;
        };
        let env = test_env();
        const SIZE: u32 = 64;
        let source = source_cube(gpu.device(), SIZE);
        for face in 0..6 {
            let mut texels = Vec::new();
            for y in 0..SIZE {
                for x in 0..SIZE {
                    let u = (x as f32 + 0.5) / SIZE as f32 * 2.0 - 1.0;
                    let v = (y as f32 + 0.5) / SIZE as f32 * 2.0 - 1.0;
                    let rgb = sky_gradient(&env, face_dir(face, u, v));
                    for c in [rgb[0], rgb[1], rgb[2], 1.0] {
                        texels.extend_from_slice(&f32_to_f16(c).to_le_bytes());
                    }
                }
            }
            gpu.queue().write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: &source,
                    mip_level: 0,
                    origin: wgpu::Origin3d {
                        x: 0,
                        y: 0,
                        z: face,
                    },
                    aspect: wgpu::TextureAspect::All,
                },
                &texels,
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(SIZE * 8),
                    rows_per_image: Some(SIZE),
                },
                wgpu::Extent3d {
                    width: SIZE,
                    height: SIZE,
                    depth_or_array_layers: 1,
                },
            );
        }

        let ibl = Ibl::new(&gpu);
        let target = probe_array(gpu.device(), SIZE, 2);
        ibl.prefilter_probe(&gpu, &source, &target, 1);

        ibl.resolve(&gpu, Some(&env), SIZE);
        let sky = ibl
            .cache
            .borrow()
            .as_ref()
            .unwrap()
            .specular_texture
            .clone();
        let sky_mips = read_rgba16_mips(&gpu, &sky);
        let probe_mips = read_rgba16_mips(&gpu, &target);
        assert_eq!(sky_mips.len(), probe_mips.len());
        for (mip, (sky, probe)) in sky_mips.iter().zip(&probe_mips).enumerate() {
            let layer_len = sky.len() / 6;
            assert!(
                probe[..layer_len * 6].iter().all(|&v| v == 0.0),
                "mip {mip}: slot 0 was written"
            );
            let mut errors = Vec::new();
            for (i, (&want, &got)) in sky.iter().zip(&probe[layer_len * 6..]).enumerate() {
                if i % 4 == 3 {
                    assert!((got - 1.0).abs() < 1e-3, "mip {mip} texel {i}: alpha {got}");
                } else {
                    errors.push((got - want).abs() / want.abs().max(1e-3));
                }
            }
            errors.sort_by(f32::total_cmp);
            let (median, worst) = (errors[errors.len() / 2], errors[errors.len() - 1]);
            let (median_bound, worst_bound) = if mip == 0 {
                (0.001, 0.01)
            } else {
                (0.06, 0.35)
            };
            assert!(
                median <= median_bound && worst <= worst_bound,
                "mip {mip}: median {median}, worst {worst} vs the sky cube"
            );
        }
    }
}
