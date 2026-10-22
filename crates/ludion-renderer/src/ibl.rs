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
        let prefilter_cube_pipeline = pipeline("prefilter_cube");
        let downsample_pipeline = pipeline("downsample");
        let brdf_pipeline = pipeline("brdf");

        let brdf_view = compute_brdf(gpu, &brdf_pipeline);
        let fallback_cube = fallback_cube(device);

        Ibl {
            irradiance_pipeline,
            prefilter_pipeline,
            prefilter_cube_pipeline,
            downsample_pipeline,
            brdf_view,
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
