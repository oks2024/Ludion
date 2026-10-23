//! The probe relight kernel (`shaders/probe_relight.wgsl`, `Plan/reflections.md` decision 17): the
//! normal-free diffuse irradiance E at a set of world points. A capture bakes E at its capture point
//! (stored with the cube, measured by [`ProbeRelight::measure`]), and every frame the
//! `probe_relight` graph pass writes E at the ranked probes' centres into the `probe_live`
//! transient; the `reflections` pass scales the captured geometry radiance by their ratio.

use ludion_core::math::glam::Vec3;
use ludion_render_graph::Gpu;
use wgpu::util::DeviceExt;

use crate::gi_bake::ResolvedGi;
use crate::ibl::ResolvedEnv;
use crate::probe::MAX_PROBES;
use crate::render::GiStrengths;
use crate::shader_gen::{self, RelightParams};

const WORKGROUP: u32 = shader_gen::PROBE_RELIGHT_RELIGHT_MAIN_WORKGROUP_SIZE[0];

/// The relight compute pipeline, its sampler and the table a relight-free frame binds.
pub(crate) struct ProbeRelight {
    pipeline: wgpu::ComputePipeline,
    sampler: wgpu::Sampler,
    fallback: wgpu::TextureView,
}

/// One frame's relight upload: the params uniform and the point count.
pub(crate) struct RelightFrame {
    params: wgpu::Buffer,
    count: u32,
}

impl ProbeRelight {
    pub(crate) fn new(gpu: &Gpu) -> ProbeRelight {
        let device = gpu.device();
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("probe relight"),
            source: wgpu::ShaderSource::Wgsl(std::borrow::Cow::Borrowed(include_str!(
                "shaders/probe_relight.wgsl"
            ))),
        });
        // Auto-derived layout, as `GiBake::new`: the volumes are sampled filterable.
        let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("probe relight"),
            layout: None,
            module: &module,
            entry_point: Some("relight_main"),
            compilation_options: wgpu::PipelineCompilationOptions::default(),
            cache: None,
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("probe relight sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let fallback = device
            .create_texture(&wgpu::TextureDescriptor {
                label: Some("probe relight fallback"),
                size: wgpu::Extent3d {
                    width: 1,
                    height: 1,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba32Float,
                usage: wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            })
            .create_view(&wgpu::TextureViewDescriptor::default());
        ProbeRelight {
            pipeline,
            sampler,
            fallback,
        }
    }

    /// Uploads the params for a dispatch over `points` (at most [`MAX_PROBES`]). The inputs are the
    /// frame's resolved environment and GI volumes, so the composition follows `mesh.wgsl`'s ambient
    /// term.
    pub(crate) fn prepare(
        &self,
        device: &wgpu::Device,
        env: &ResolvedEnv,
        gi: &ResolvedGi,
        strengths: GiStrengths,
        ambient: [f32; 3],
        points: &[Vec3],
    ) -> RelightFrame {
        debug_assert!(points.len() <= MAX_PROBES);
        let mut params: RelightParams = bytemuck::Zeroable::zeroed();
        for (slot, p) in params.points.iter_mut().zip(points) {
            *slot = [p.x, p.y, p.z, 0.0];
        }
        params.counts = [
            points.len() as f32,
            env.params[0],
            env.params[1],
            gi.origin[3],
        ];
        params.gi_origin = gi.origin;
        params.gi_scale = gi.scale;
        params.strengths = [strengths.sky_visibility, strengths.bounce, 0.0, 0.0];
        params.ambient = [ambient[0], ambient[1], ambient[2], 0.0];
        RelightFrame {
            params: device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("probe relight params"),
                contents: bytemuck::bytes_of(&params),
                usage: wgpu::BufferUsages::UNIFORM,
            }),
            count: points.len() as u32,
        }
    }

    /// The bind group for one dispatch, over `frame`'s params and the table `out`. Split from
    /// [`dispatch`](ProbeRelight::dispatch) because a `RecordedPass` lends its encoder mutably: the
    /// group is built before the compute pass is opened.
    pub(crate) fn bind_group(
        &self,
        device: &wgpu::Device,
        frame: &RelightFrame,
        env: &ResolvedEnv,
        gi: &ResolvedGi,
        out: &wgpu::TextureView,
    ) -> wgpu::BindGroup {
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("probe relight"),
            layout: &self
                .pipeline
                .get_bind_group_layout(shader_gen::RELIGHT_GROUP),
            entries: &[
                wgpu::BindGroupEntry {
                    binding: shader_gen::RELIGHT_BINDING,
                    resource: frame.params.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: shader_gen::RELIGHT_IRRADIANCE_BINDING,
                    resource: wgpu::BindingResource::TextureView(&env.irradiance),
                },
                wgpu::BindGroupEntry {
                    binding: shader_gen::RELIGHT_SAMP_BINDING,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
                wgpu::BindGroupEntry {
                    binding: shader_gen::RELIGHT_VIS_BINDING,
                    resource: wgpu::BindingResource::TextureView(&gi.probes),
                },
                wgpu::BindGroupEntry {
                    binding: shader_gen::RELIGHT_BOUNCE_R_BINDING,
                    resource: wgpu::BindingResource::TextureView(&gi.bounce[0]),
                },
                wgpu::BindGroupEntry {
                    binding: shader_gen::RELIGHT_BOUNCE_G_BINDING,
                    resource: wgpu::BindingResource::TextureView(&gi.bounce[1]),
                },
                wgpu::BindGroupEntry {
                    binding: shader_gen::RELIGHT_BOUNCE_B_BINDING,
                    resource: wgpu::BindingResource::TextureView(&gi.bounce[2]),
                },
                wgpu::BindGroupEntry {
                    binding: shader_gen::RELIGHT_OUT_BINDING,
                    resource: wgpu::BindingResource::TextureView(out),
                },
            ],
        })
    }

    /// Issues the dispatch into an already-opened compute pass; nothing for an empty point list.
    pub(crate) fn dispatch(
        &self,
        pass: &mut wgpu::ComputePass<'_>,
        frame: &RelightFrame,
        bind_group: &wgpu::BindGroup,
    ) {
        if frame.count == 0 {
            return;
        }
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(shader_gen::RELIGHT_GROUP, bind_group, &[]);
        pass.dispatch_workgroups(frame.count.div_ceil(WORKGROUP), 1, 1);
    }

    /// The 1×1 table a frame with no relight binds to `reflections`; its probe list is empty, so
    /// nothing reads it.
    pub(crate) fn fallback(&self) -> &wgpu::TextureView {
        &self.fallback
    }

    /// One-shot relight over `points`, read back synchronously: E per point. The capture-time
    /// measurement and the tests' seam; it runs the same kernel into a scratch table in its own
    /// submit.
    pub(crate) fn measure(
        &self,
        gpu: &Gpu,
        env: &ResolvedEnv,
        gi: &ResolvedGi,
        strengths: GiStrengths,
        ambient: [f32; 3],
        points: &[Vec3],
    ) -> Vec<[f32; 3]> {
        let device = gpu.device();
        let scratch = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("probe relight scratch"),
            size: wgpu::Extent3d {
                width: MAX_PROBES as u32,
                height: 1,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba32Float,
            usage: wgpu::TextureUsages::STORAGE_BINDING | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = scratch.create_view(&wgpu::TextureViewDescriptor::default());
        let frame = self.prepare(device, env, gi, strengths, ambient, points);
        let bind = self.bind_group(device, &frame, env, gi, &view);
        let row = (MAX_PROBES * 16) as u64;
        let staging = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("probe relight readback"),
            size: row,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut encoder =
            device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("probe relight"),
                timestamp_writes: None,
            });
            self.dispatch(&mut pass, &frame, &bind);
        }
        encoder.copy_texture_to_buffer(
            scratch.as_image_copy(),
            wgpu::TexelCopyBufferInfo {
                buffer: &staging,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(row as u32),
                    rows_per_image: Some(1),
                },
            },
            wgpu::Extent3d {
                width: MAX_PROBES as u32,
                height: 1,
                depth_or_array_layers: 1,
            },
        );
        gpu.queue().submit(Some(encoder.finish()));
        let (tx, rx) = std::sync::mpsc::channel();
        staging.slice(..).map_async(wgpu::MapMode::Read, move |r| {
            let _ = tx.send(r);
        });
        device
            .poll(wgpu::PollType::wait_indefinitely())
            .expect("the device polls");
        rx.recv().unwrap().expect("the buffer maps");
        let mapped = staging.slice(..).get_mapped_range().expect("mapped range");
        let floats: &[f32] = bytemuck::cast_slice(&mapped);
        let out = floats
            .chunks_exact(4)
            .take(points.len())
            .map(|c| [c[0], c[1], c[2]])
            .collect();
        drop(mapped);
        staging.unmap();
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gi_bake::GiBake;
    use crate::ibl::Ibl;
    use crate::ibl::tests::{read_rgba16_mips, test_env};
    use crate::render::Renderer;

    fn setup(what: &str) -> Option<(Renderer, Ibl, GiBake)> {
        let r = match Renderer::new() {
            Ok(r) => r,
            Err(d) if ludion_core::require_gpu::is_no_adapter(std::slice::from_ref(&d)) => {
                eprintln!("SKIPPED {what}: no GPU adapter");
                return None;
            }
            Err(d) => panic!("renderer init failed: {d:?}"),
        };
        let (ibl, gi) = (Ibl::new(r.gpu()), GiBake::new(r.gpu()));
        Some((r, ibl, gi))
    }

    const NO_GI: GiStrengths = GiStrengths {
        sky_visibility: 0.0,
        bounce: 0.0,
    };

    #[test]
    fn needs_gpu_without_an_environment_or_gi_e_is_the_ambient() {
        let Some((r, ibl, gi)) = setup("relight ambient") else {
            return;
        };
        let gpu = r.gpu();
        let env = ibl.resolve(gpu, None, r.ibl_specular_size());
        let gi = gi.resolve(gpu, None);
        let relight = ProbeRelight::new(gpu);
        let ambient = [0.05, 0.1, 0.2];
        let out = relight.measure(gpu, &env, &gi, NO_GI, ambient, &[Vec3::new(1.0, 2.0, 3.0)]);
        assert_eq!(out, vec![ambient]);
    }

    #[test]
    fn needs_gpu_with_an_environment_e_is_k_times_the_axis_mean_plus_ambient() {
        let Some((r, ibl, gi)) = setup("relight environment") else {
            return;
        };
        let gpu = r.gpu();
        let mut data = test_env();
        data.ibl_intensity = 0.9;
        let env = ibl.resolve(gpu, Some(&data), r.ibl_specular_size());
        let gi = gi.resolve(gpu, None);
        let relight = ProbeRelight::new(gpu);
        let ambient = [0.05, 0.1, 0.2];
        let got = relight.measure(gpu, &env, &gi, NO_GI, ambient, &[Vec3::ZERO])[0];

        // Each axis direction is the centre of a cube face: the mean of its four centre texels.
        let mip0 = &read_rgba16_mips(gpu, env.irradiance.texture())[0];
        let edge = env.irradiance.texture().width() as usize;
        let mut sum = [0.0_f32; 3];
        for face in 0..6 {
            for (x, y) in [(-1, -1), (0, -1), (-1, 0), (0, 0)] {
                let at = ((face * edge + (edge / 2).wrapping_add_signed(y)) * edge
                    + (edge / 2).wrapping_add_signed(x))
                    * 4;
                for c in 0..3 {
                    sum[c] += mip0[at + c] / 4.0;
                }
            }
        }
        for c in 0..3 {
            let want = sum[c] / 6.0 * 0.9 + ambient[c];
            assert!(
                (got[c] - want).abs() < 1e-4,
                "channel {c}: {} vs {want}",
                got[c]
            );
        }
    }

    #[test]
    fn needs_gpu_a_point_gives_the_same_bits_alone_or_in_a_batch() {
        let Some((r, ibl, gi)) = setup("relight batch") else {
            return;
        };
        let gpu = r.gpu();
        let env = ibl.resolve(gpu, Some(&test_env()), r.ibl_specular_size());
        let gi = gi.resolve(gpu, None);
        let relight = ProbeRelight::new(gpu);
        let ambient = [0.01, 0.02, 0.03];
        let point = Vec3::new(4.0, 5.0, 6.0);
        let alone = relight.measure(gpu, &env, &gi, NO_GI, ambient, &[point]);
        let batch: Vec<Vec3> = (0..MAX_PROBES)
            .map(|i| {
                if i == 17 {
                    point
                } else {
                    Vec3::splat(i as f32)
                }
            })
            .collect();
        let many = relight.measure(gpu, &env, &gi, NO_GI, ambient, &batch);
        assert_eq!(alone[0], many[17]);
    }
}
