//! GPU integration tests: real headless rendering on this machine's adapter.
//!
//! Each test skips gracefully (printing why) when no adapter is available, so CI without a GPU stays
//! green — but on a machine with an adapter (hardware or software) they run for real. The headline
//! test is the Phase 2 determinism criterion: rendering the fixed graph twice in-process yields
//! byte-identical pixels.

use ludion_render_graph::{
    Extent2d, FrameTiming, Gpu, PassKind, PassRecorder, PassTiming, PassTimingRing, RecordedPass,
    RenderGraph, ResourceId, Schedule, TextureDesc, build_graph, schedule,
};

/// Records every pass of the proving graph as a bare clear of its color target.
///
/// The pass timing ring only exists on the *external-recorder* path (`Gpu::render_streamed`), so a
/// recorder is needed even for the built-in proving graph; `begin_clear` attaches each pass's
/// timestamp pair for us, which is exactly the contract a real recorder honors.
struct ClearEachPass;

impl PassRecorder for ClearEachPass {
    fn record(&self, pass: &mut RecordedPass<'_>) {
        let _ = pass.begin_clear(wgpu::Color::BLUE);
    }
}

/// Drives `count` streamed frames through both rings, blocking between frames so every sample is
/// collectable, and returns the timing sets in collection order.
fn timed_frames(
    gpu: &Gpu,
    scheduled: &Schedule,
    output: ResourceId,
    timing: &mut PassTimingRing<usize>,
    count: usize,
) -> Vec<(usize, Vec<PassTiming>)> {
    let mut images = gpu.new_stream_readback(scheduled, output);
    let mut collected = Vec::new();
    for frame in 0..count {
        gpu.render_streamed(
            scheduled,
            output,
            &ClearEachPass,
            &mut images,
            frame,
            Some(timing),
        )
        .unwrap();
        drain_timings(gpu, timing, &mut collected);
    }
    drain_timings(gpu, timing, &mut collected);
    collected
        .into_iter()
        .map(|(stamp, sample)| (stamp, sample.passes))
        .collect()
}

/// Drains the timing ring in two rounds of wait-then-collect: a sample needs one completion for
/// its frame, which releases its resolve, and one for the resolve itself.
fn drain_timings(
    gpu: &Gpu,
    timing: &mut PassTimingRing<usize>,
    into: &mut Vec<(usize, FrameTiming)>,
) {
    for _ in 0..2 {
        gpu.device()
            .poll(wgpu::PollType::wait_indefinitely())
            .unwrap();
        while let Some(sample) = timing.try_collect_timings(gpu) {
            into.push(sample);
        }
    }
}

/// Requests a GPU, or returns `None` after printing a skip reason.
fn gpu_or_skip(test: &str) -> Option<Gpu> {
    match Gpu::request() {
        Ok(gpu) => {
            eprintln!(
                "[{test}] using adapter: {} ({})",
                gpu.adapter_name(),
                gpu.adapter_backend()
            );
            Some(gpu)
        }
        Err(diagnostic) => {
            ludion_core::require_gpu::skip_no_adapter(true);
            eprintln!("[{test}] SKIPPED: no GPU adapter available:\n{diagnostic}");
            None
        }
    }
}

/// The headless request lands on a native backend: an `all()` regression would silently re-add GL's
/// start-up cost, and a backend switch would move goldens.
#[test]
fn needs_gpu_request_uses_a_primary_backend() {
    if std::env::var_os("WGPU_BACKEND").is_some() {
        return;
    }
    let Some(gpu) = gpu_or_skip("request_uses_a_primary_backend") else {
        return;
    };
    assert!(
        matches!(gpu.adapter_backend(), "Vulkan" | "Dx12" | "Metal"),
        "headless request picked {}",
        gpu.adapter_backend()
    );
}

/// **Phase 2 determinism criterion.** Rendering the same graph twice in one process produces
/// byte-identical pixels.
#[test]
fn needs_gpu_two_renders_are_byte_identical() {
    let Some(gpu) = gpu_or_skip("two_renders_are_byte_identical") else {
        return;
    };
    let (graph, output) = build_graph(320, 180);
    let scheduled = schedule(&graph).unwrap();

    let a = gpu.render_to_image(&scheduled, output).unwrap();
    let b = gpu.render_to_image(&scheduled, output).unwrap();

    assert_eq!(a.width, 320);
    assert_eq!(a.height, 180);
    assert_eq!(a.rgba.len(), 320 * 180 * 4);
    assert_eq!(a, b, "two renders of the same graph must be byte-identical");

    // The gradient pass writes a non-trivial image; the frame is not uniformly one color.
    let first = &a.rgba[0..4];
    assert!(
        a.rgba.chunks_exact(4).any(|px| px != first),
        "the rendered frame should not be a single flat color",
    );
}

/// The encoded PNG is deterministic too (this is what `ludion render` writes).
#[test]
fn needs_gpu_png_encoding_is_deterministic() {
    let Some(gpu) = gpu_or_skip("png_encoding_is_deterministic") else {
        return;
    };
    let (graph, output) = build_graph(128, 96);
    let scheduled = schedule(&graph).unwrap();
    let image = gpu.render_to_image(&scheduled, output).unwrap();

    let png_a = image.encode_png().unwrap();
    let png_b = image.encode_png().unwrap();
    assert_eq!(png_a, png_b, "PNG encoding must be deterministic");
    assert_eq!(&png_a[1..4], b"PNG", "output is a PNG");
}

/// The 3-deep async staging ring (risk-register row 2) renders several frames; every frame matches
/// the blocking path byte-for-byte, proving the pipelined readback is correct.
#[test]
fn needs_gpu_staging_ring_matches_blocking_readback() {
    let Some(gpu) = gpu_or_skip("staging_ring_matches_blocking_readback") else {
        return;
    };
    let (graph, output) = build_graph(96, 64);
    let scheduled = schedule(&graph).unwrap();

    let reference = gpu.render_to_image(&scheduled, output).unwrap();

    // Seven frames forces the ring (depth 3) to recycle buffers several times.
    let frames = gpu.render_frames_via_ring(&scheduled, output, 7).unwrap();
    assert_eq!(frames.len(), 7);
    for (i, frame) in frames.iter().enumerate() {
        assert_eq!(
            *frame, reference,
            "ring frame {i} differs from the blocking readback",
        );
    }
}

/// `dump-timings` returns well-shaped JSON on any adapter: the supported flag plus one entry per
/// pass. Values are not asserted (they are wall-clock GPU measurements).
#[test]
fn needs_gpu_dump_timings_json_has_expected_shape() {
    let Some(gpu) = gpu_or_skip("dump_timings_json_has_expected_shape") else {
        return;
    };
    let (graph, output) = build_graph(64, 64);
    let scheduled = schedule(&graph).unwrap();

    // `None`: the proving graph uses the built-in pass kinds, not an external recorder.
    let json = gpu.dump_timings_json(&scheduled, output, None).unwrap();
    let value: serde_json::Value = serde_json::from_str(&json).expect("timings output is JSON");

    assert!(
        value
            .get("timestamps_supported")
            .and_then(|v| v.as_bool())
            .is_some(),
        "must report timestamps_supported as a bool: {json}",
    );
    let passes = value
        .get("passes")
        .and_then(|v| v.as_array())
        .expect("passes is an array");
    assert_eq!(passes.len(), 4, "one entry per scheduled pass");
    let names: Vec<&str> = passes
        .iter()
        .map(|p| {
            p.get("name")
                .and_then(|n| n.as_str())
                .expect("each pass has a name")
        })
        .collect();
    assert_eq!(names, ["gradient", "vignette", "bleed", "present"]);

    if value["timestamps_supported"] == serde_json::Value::Bool(true) {
        assert!(value.get("timestamp_period_ns").is_some());
        assert!(
            passes.iter().all(|p| p.get("gpu_time_ms").is_some()),
            "timing-capable adapters report gpu_time_ms per pass",
        );
    }
}

/// The pipelined ring yields exactly one timing set per streamed frame, in that frame's stamp order,
/// naming every scheduled pass. On a timestamp-capable adapter each pass carries a measurement; on
/// one without, the same set arrives with no measurements (the `timestamps_supported: false` shape).
#[test]
fn needs_gpu_pass_timing_ring_yields_one_set_per_frame() {
    let Some(gpu) = gpu_or_skip("pass_timing_ring_yields_one_set_per_frame") else {
        return;
    };
    let (graph, output) = build_graph(64, 64);
    let scheduled = schedule(&graph).unwrap();
    let mut timing: PassTimingRing<usize> = gpu.new_pass_timing_ring();

    let collected = timed_frames(&gpu, &scheduled, output, &mut timing, 5);

    let stamps: Vec<usize> = collected.iter().map(|(stamp, _)| *stamp).collect();
    assert_eq!(stamps, [0, 1, 2, 3, 4], "one set per frame, in stamp order");
    for (stamp, passes) in &collected {
        let names: Vec<&str> = passes.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(
            names,
            ["gradient", "vignette", "bleed", "present"],
            "frame {stamp} must name every scheduled pass, in scheduled order",
        );
        if timing.timestamps_supported() {
            assert!(
                passes.iter().all(|p| p.gpu_time_ms.is_some()),
                "a timestamp-capable adapter measures every pass",
            );
        } else {
            assert!(
                passes.iter().all(|p| p.gpu_time_ms.is_none()),
                "without TIMESTAMP_QUERY the shape survives but the measurements do not",
            );
        }
    }
    assert_eq!(
        timing.timestamps_supported(),
        timing.timestamp_period_ns().is_some(),
        "the timestamp period is present exactly when timing is",
    );
}

/// A linear graph of `passes` passes: a gradient, then vignettes each reading its predecessor.
fn linear_graph(passes: usize) -> (RenderGraph, ResourceId) {
    let mut graph = RenderGraph::new();
    let mut previous = graph.transient("t0", TextureDesc::color(32, 32));
    graph.add_pass("p0", PassKind::Gradient, previous);
    for i in 1..passes - 1 {
        let next = graph.transient(format!("t{i}"), TextureDesc::color(32, 32));
        graph.add_pass(
            format!("p{i}"),
            PassKind::Vignette { source: previous },
            next,
        );
        previous = next;
    }
    let output = graph.output("final", TextureDesc::present_target(32, 32));
    graph.add_pass(
        format!("p{}", passes - 1),
        PassKind::Vignette { source: previous },
        output,
    );
    (graph, output)
}

/// A graph above the old 32-pass cap is timed pass by pass on a timestamp-capable adapter; one above
/// `MAX_PASSES` is named with no measurements and its image still renders.
#[test]
fn needs_gpu_pass_timing_ring_times_a_graph_above_the_old_cap() {
    let Some(gpu) = gpu_or_skip("pass_timing_ring_times_a_graph_above_the_old_cap") else {
        return;
    };
    let max = PassTimingRing::<usize>::MAX_PASSES;

    let (graph, output) = linear_graph(40);
    let scheduled = schedule(&graph).unwrap();
    let mut timing: PassTimingRing<usize> = gpu.new_pass_timing_ring();
    let collected = timed_frames(&gpu, &scheduled, output, &mut timing, 3);
    assert_eq!(collected.len(), 3);
    for (stamp, passes) in &collected {
        assert_eq!(passes.len(), 40, "frame {stamp} names every pass");
        if timing.timestamps_supported() {
            assert!(
                passes.iter().all(|p| p.gpu_time_ms.is_some()),
                "frame {stamp}: a graph above 32 passes is timed on a capable adapter",
            );
        }
    }

    let (graph, output) = linear_graph(max + 1);
    let scheduled = schedule(&graph).unwrap();
    let mut timing: PassTimingRing<usize> = gpu.new_pass_timing_ring();
    let mut images = gpu.new_stream_readback(&scheduled, output);
    let mut rendered = Vec::new();
    for frame in 0..3 {
        rendered.extend(
            gpu.render_streamed(
                &scheduled,
                output,
                &ClearEachPass,
                &mut images,
                frame,
                Some(&mut timing),
            )
            .unwrap(),
        );
    }
    gpu.device()
        .poll(wgpu::PollType::wait_indefinitely())
        .unwrap();
    while let Some(image) = images.try_collect(&gpu).unwrap() {
        rendered.push(image);
    }
    let mut samples = Vec::new();
    drain_timings(&gpu, &mut timing, &mut samples);
    assert_eq!(samples.len(), 3, "an oversize graph still files a sample");
    for (_, sample) in &samples {
        assert_eq!(sample.passes.len(), max + 1);
        assert!(sample.passes.iter().all(|p| p.gpu_time_ms.is_none()));
        assert!(sample.frame_gpu_time_ms.is_none());
    }
    assert!(!rendered.is_empty(), "the oversize frames still render");
    let blue = [0, 0, 255, 255];
    assert!(rendered[0].1.rgba.chunks_exact(4).all(|px| px == blue));
}

/// A full ring skips the frame's timing rather than blocking for a slot — and recovers as soon as a
/// slot drains. Frames keep rendering throughout.
#[test]
fn needs_gpu_full_pass_timing_ring_skips_frames_then_recovers() {
    let Some(gpu) = gpu_or_skip("full_pass_timing_ring_skips_frames_then_recovers") else {
        return;
    };
    let (graph, output) = build_graph(64, 64);
    let scheduled = schedule(&graph).unwrap();
    let mut timing: PassTimingRing<usize> = gpu.new_pass_timing_ring();
    let mut images = gpu.new_stream_readback(&scheduled, output);

    // Six frames, never collecting: only the first DEPTH can be timed.
    for frame in 0..6 {
        gpu.render_streamed(
            &scheduled,
            output,
            &ClearEachPass,
            &mut images,
            frame,
            Some(&mut timing),
        )
        .unwrap();
    }

    let mut collected = Vec::new();
    drain_timings(&gpu, &mut timing, &mut collected);
    let stamps: Vec<usize> = collected.iter().map(|(stamp, _)| *stamp).collect();
    assert_eq!(
        stamps,
        (0..PassTimingRing::<usize>::DEPTH).collect::<Vec<_>>(),
        "a full ring skips a frame's timing instead of stalling for a slot",
    );

    // With the ring drained, timing resumes on the next frame.
    let resumed = timed_frames(&gpu, &scheduled, output, &mut timing, 1);
    assert_eq!(resumed.len(), 1, "the ring recovers once a slot frees");
    assert_eq!(resumed[0].1.len(), 4);
}

/// The compute half of the volume graph: fills every texel of a 3D storage texture, lighting only
/// the last z-slice. Written inline rather than under `src/shaders/`, because every shader living
/// there is reflected and drift-gated workspace-wide; a test fixture is not part of that surface.
const INJECT_WGSL: &str = r#"
@group(0) @binding(0) var volume: texture_storage_3d<rgba16float, write>;

@compute @workgroup_size(1, 1, 1)
fn main(@builtin(global_invocation_id) gid: vec3<u32>) {
    let lit = select(0.0, 1.0, gid.z == 3u);
    textureStore(volume, vec3<i32>(gid), vec4<f32>(lit, lit, lit, 1.0));
}
"#;

/// The consuming render pass: samples the volume's last z-slice full-screen. Reading slice 3 is what
/// makes the test discriminating — an un-run compute pass, one scheduled after this pass, or a
/// volume allocated flat all leave this load returning zero.
const SHADE_WGSL: &str = r#"
@group(0) @binding(0) var volume: texture_3d<f32>;

@vertex
fn vs_main(@builtin(vertex_index) vi: u32) -> @builtin(position) vec4<f32> {
    var corners = array<vec2<f32>, 3>(
        vec2<f32>(-1.0, -1.0),
        vec2<f32>( 3.0, -1.0),
        vec2<f32>(-1.0,  3.0),
    );
    return vec4<f32>(corners[vi], 0.0, 1.0);
}

@fragment
fn fs_main() -> @location(0) vec4<f32> {
    return vec4<f32>(textureLoad(volume, vec3<i32>(0, 0, 3), 0).rgb, 1.0);
}
"#;

/// Records the volume graph: `inject` dispatches compute over the graph-owned 3D storage texture,
/// `shade` samples it.
struct VolumeRecorder {
    inject: wgpu::ComputePipeline,
    inject_layout: wgpu::BindGroupLayout,
    shade: wgpu::RenderPipeline,
    shade_layout: wgpu::BindGroupLayout,
}

impl VolumeRecorder {
    fn new(gpu: &Gpu) -> VolumeRecorder {
        let device = gpu.device();
        let inject_module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("inject"),
            source: wgpu::ShaderSource::Wgsl(INJECT_WGSL.into()),
        });
        let shade_module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("shade"),
            source: wgpu::ShaderSource::Wgsl(SHADE_WGSL.into()),
        });

        let inject_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("inject bind group layout"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::COMPUTE,
                ty: wgpu::BindingType::StorageTexture {
                    access: wgpu::StorageTextureAccess::WriteOnly,
                    format: wgpu::TextureFormat::Rgba16Float,
                    view_dimension: wgpu::TextureViewDimension::D3,
                },
                count: None,
            }],
        });
        let inject_pipeline_layout =
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("inject layout"),
                bind_group_layouts: &[Some(&inject_layout)],
                immediate_size: 0,
            });
        let inject = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("inject pipeline"),
            layout: Some(&inject_pipeline_layout),
            module: &inject_module,
            entry_point: Some("main"),
            compilation_options: wgpu::PipelineCompilationOptions::default(),
            cache: None,
        });

        let shade_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("shade bind group layout"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Float { filterable: true },
                    view_dimension: wgpu::TextureViewDimension::D3,
                    multisampled: false,
                },
                count: None,
            }],
        });
        let shade_pipeline_layout =
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("shade layout"),
                bind_group_layouts: &[Some(&shade_layout)],
                immediate_size: 0,
            });
        let shade = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("shade pipeline"),
            layout: Some(&shade_pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shade_module,
                entry_point: Some("vs_main"),
                buffers: &[],
                compilation_options: wgpu::PipelineCompilationOptions::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &shade_module,
                entry_point: Some("fs_main"),
                targets: &[Some(wgpu::ColorTargetState {
                    format: wgpu::TextureFormat::Rgba8UnormSrgb,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: wgpu::PipelineCompilationOptions::default(),
            }),
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });

        VolumeRecorder {
            inject,
            inject_layout,
            shade,
            shade_layout,
        }
    }
}

impl PassRecorder for VolumeRecorder {
    fn record(&self, pass: &mut RecordedPass<'_>) {
        let device = pass.device;
        match pass.kind {
            "inject" => {
                assert!(pass.is_compute(), "the inject pass is declared as compute");
                assert!(
                    pass.colors().is_empty(),
                    "a compute pass has no color attachment"
                );
                let view = pass.storage.expect("the graph resolved the storage target");
                let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("inject bind group"),
                    layout: &self.inject_layout,
                    entries: &[wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(view),
                    }],
                });
                let mut compute = pass.begin_compute();
                compute.set_pipeline(&self.inject);
                compute.set_bind_group(0, &bind_group, &[]);
                compute.dispatch_workgroups(2, 2, 4);
            }
            "shade" => {
                assert!(!pass.is_compute(), "the shade pass draws");
                let input = pass.inputs[0];
                let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("shade bind group"),
                    layout: &self.shade_layout,
                    entries: &[wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(input),
                    }],
                });
                let mut render = pass.begin_clear(wgpu::Color::RED);
                render.set_pipeline(&self.shade);
                render.set_bind_group(0, &bind_group, &[]);
                render.draw(0..3, 0..1);
            }
            other => panic!("unexpected pass `{other}`"),
        }
    }
}

/// The volume graph: a compute pass filling a 3D storage transient, and a render pass sampling it.
/// The compute pass is declared **second** on purpose — only the read edge can put it first.
fn volume_graph() -> (RenderGraph, ResourceId) {
    let mut graph = RenderGraph::new();
    let volume = graph.transient("volume", TextureDesc::hdr_storage_volume(2, 2, 4));
    let out = graph.output("final", TextureDesc::present_target(8, 8));
    graph.add_external_pass("shade", "shade", vec![volume], vec![out]);
    graph.add_external_compute_pass("inject", "inject", vec![], volume);
    (graph, out)
}

/// A compute pass allocates and writes a 3D storage transient a later render pass samples: the read
/// edge runs the compute first (it is declared second), the volume really carries four z-slices, and
/// the sampled last slice reaches the frame.
#[test]
fn needs_gpu_compute_pass_fills_a_volume_the_render_pass_samples() {
    let Some(gpu) = gpu_or_skip("compute_pass_fills_a_volume_the_render_pass_samples") else {
        return;
    };
    let (graph, out) = volume_graph();
    let scheduled = schedule(&graph).unwrap();
    assert_eq!(
        scheduled.order_names(),
        ["inject", "shade"],
        "the storage write orders the compute pass ahead of its reader",
    );

    let image = gpu
        .execute(&scheduled, out, &VolumeRecorder::new(&gpu))
        .unwrap();
    assert_eq!(image.rgba.len(), 8 * 8 * 4);
    assert!(
        image
            .rgba
            .chunks_exact(4)
            .all(|px| px == [255, 255, 255, 255]),
        "every pixel must carry the volume's lit last slice; a zero (or red) frame means the \
         compute pass did not run first, or the volume was not allocated 3D",
    );
}

/// A compute pass is timed like any other: it appears in `dump-timings` under its own name, in
/// scheduled order, with a measurement wherever the adapter has `TIMESTAMP_QUERY`.
#[test]
fn needs_gpu_compute_pass_is_named_and_timed_in_dump_timings() {
    let Some(gpu) = gpu_or_skip("compute_pass_is_named_and_timed_in_dump_timings") else {
        return;
    };
    let (graph, out) = volume_graph();
    let scheduled = schedule(&graph).unwrap();
    let recorder = VolumeRecorder::new(&gpu);

    let json = gpu
        .dump_timings_json(&scheduled, out, Some(&recorder))
        .unwrap();
    let value: serde_json::Value = serde_json::from_str(&json).expect("timings output is JSON");
    let passes = value["passes"].as_array().expect("passes is an array");
    let names: Vec<&str> = passes
        .iter()
        .map(|p| p["name"].as_str().expect("each pass has a name"))
        .collect();
    assert_eq!(names, ["inject", "shade"]);

    if value["timestamps_supported"] == serde_json::Value::Bool(true) {
        assert!(
            passes.iter().all(|p| p.get("gpu_time_ms").is_some()),
            "a compute pass reports GPU time like a render pass: {json}",
        );
    }
}

/// The live profiler's ring times a compute pass the same way, so a graph containing one needs no
/// special-casing downstream of the executor.
#[test]
fn needs_gpu_compute_pass_reaches_the_pass_timing_ring() {
    let Some(gpu) = gpu_or_skip("compute_pass_reaches_the_pass_timing_ring") else {
        return;
    };
    let (graph, out) = volume_graph();
    let scheduled = schedule(&graph).unwrap();
    let recorder = VolumeRecorder::new(&gpu);
    let mut timing: PassTimingRing<usize> = gpu.new_pass_timing_ring();
    let mut images = gpu.new_stream_readback(&scheduled, out);

    let mut collected = Vec::new();
    for frame in 0..2 {
        gpu.render_streamed(
            &scheduled,
            out,
            &recorder,
            &mut images,
            frame,
            Some(&mut timing),
        )
        .unwrap();
        drain_timings(&gpu, &mut timing, &mut collected);
    }
    drain_timings(&gpu, &mut timing, &mut collected);

    assert_eq!(collected.len(), 2, "one timing set per streamed frame");
    for (stamp, sample) in &collected {
        let names: Vec<&str> = sample.passes.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(
            names,
            ["inject", "shade"],
            "frame {stamp} must name the compute pass in scheduled order",
        );
        if timing.timestamps_supported() {
            assert!(sample.passes.iter().all(|p| p.gpu_time_ms.is_some()));
            if ["Vulkan", "Dx12", "Metal"].contains(&gpu.adapter_backend()) {
                assert!(
                    sample.frame_gpu_time_ms.is_some_and(|ms| ms > 0.0),
                    "frame {stamp} must report a frame span",
                );
            }
        }
    }
}

/// A GPU validation error comes back as `L2010` from `take_fault` — never a panic — and is cleared
/// once taken; the next render is clean.
#[test]
fn needs_gpu_validation_error_becomes_a_fault_not_a_panic() {
    let Some(gpu) = gpu_or_skip("validation_error_becomes_a_fault_not_a_panic") else {
        return;
    };
    assert!(gpu.take_fault().is_none(), "a fresh device has no fault");

    let _invalid = gpu.device().create_texture(&wgpu::TextureDescriptor {
        label: Some("zero-width texture"),
        size: wgpu::Extent3d {
            width: 0,
            height: 4,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8UnormSrgb,
        usage: wgpu::TextureUsages::TEXTURE_BINDING,
        view_formats: &[],
    });

    let fault = gpu.take_fault().expect("the validation error is reported");
    assert_eq!(fault.code.as_str(), "L2010");
    assert!(gpu.take_fault().is_none(), "a taken fault is cleared");

    let (graph, output) = build_graph(16, 16);
    let scheduled = schedule(&graph).unwrap();
    gpu.render_to_image(&scheduled, output)
        .expect("the next render is clean");
}

/// A fault raised between renders replaces the next render's result rather than hiding behind it.
#[test]
fn needs_gpu_pending_fault_refuses_the_next_render_once() {
    let Some(gpu) = gpu_or_skip("pending_fault_refuses_the_next_render_once") else {
        return;
    };
    let _invalid = gpu.device().create_buffer(&wgpu::BufferDescriptor {
        label: Some("zero-usage buffer"),
        size: 4,
        usage: wgpu::BufferUsages::empty(),
        mapped_at_creation: false,
    });
    let (graph, output) = build_graph(16, 16);
    let scheduled = schedule(&graph).unwrap();

    let refused = gpu.render_to_image(&scheduled, output).unwrap_err();
    assert_eq!(refused.code.as_str(), "L2010");
    gpu.render_to_image(&scheduled, output)
        .expect("the fault was taken; the next render runs");
}

/// Records the multi-target graph: `opaque` clears its two colours to distinct values through its
/// own descriptor; `mip1`/`mip2` clear their single mip attachment; `final` clears the output.
struct MrtRecorder;

impl PassRecorder for MrtRecorder {
    fn record(&self, pass: &mut RecordedPass<'_>) {
        match pass.kind {
            "opaque" => {
                let colors = pass.colors();
                assert_eq!(colors.len(), 2);
                assert_eq!(
                    colors[0].format,
                    ludion_render_graph::resource::TextureFormat::Rgba16Float
                );
                assert_eq!(
                    colors[1].format,
                    ludion_render_graph::resource::TextureFormat::Rgba8UnormSrgb
                );
                let clears = [wgpu::Color::RED, wgpu::Color::GREEN];
                let attachments: Vec<_> = colors
                    .iter()
                    .zip(clears)
                    .map(|(c, clear)| {
                        Some(wgpu::RenderPassColorAttachment {
                            view: c.view,
                            depth_slice: None,
                            resolve_target: None,
                            ops: wgpu::Operations {
                                load: wgpu::LoadOp::Clear(clear),
                                store: wgpu::StoreOp::Store,
                            },
                        })
                    })
                    .collect();
                let timestamp_writes = pass.timestamp_writes();
                let _ = pass.encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("opaque"),
                    color_attachments: &attachments,
                    depth_stencil_attachment: None,
                    timestamp_writes,
                    occlusion_query_set: None,
                    multiview_mask: None,
                });
            }
            "mip1" | "mip2" => {
                let level = if pass.kind == "mip1" { 1 } else { 2 };
                assert_eq!(pass.colors().len(), 1);
                assert_eq!(
                    pass.colors()[0].extent,
                    Extent2d {
                        width: 8 >> level,
                        height: 8 >> level
                    }
                );
                let _ = pass.begin_clear(wgpu::Color::BLUE);
            }
            "final" => {
                assert_eq!(pass.inputs.len(), 1);
                let _ = pass.begin_clear(wgpu::Color::WHITE);
            }
            other => panic!("unexpected pass `{other}`"),
        }
    }
}

/// A multi-target pass and single-mip attachments of a mipped transient record without a wgpu
/// validation error (a view spanning several mips would be rejected as an attachment).
#[test]
fn needs_gpu_multi_target_pass_and_mip_attachments_validate() {
    let Some(gpu) = gpu_or_skip("multi_target_pass_and_mip_attachments_validate") else {
        return;
    };
    let mut graph = RenderGraph::new();
    let scene = graph.transient("scene", TextureDesc::hdr_color(8, 8));
    let normal = graph.transient("normal", TextureDesc::color(8, 8));
    let pyramid = graph.transient("pyramid", TextureDesc::color(8, 8).with_mip_levels(3));
    let out = graph.output("final", TextureDesc::present_target(8, 8));
    graph.add_external_pass("opaque", "opaque", vec![], vec![scene, normal]);
    graph.add_external_pass("mip1", "mip1", vec![], vec![pyramid.mip(1)]);
    graph.add_external_pass("mip2", "mip2", vec![pyramid.mip(1)], vec![pyramid.mip(2)]);
    graph.add_external_pass("final", "final", vec![normal], vec![out]);
    let scheduled = schedule(&graph).unwrap();

    let image = gpu.execute(&scheduled, out, &MrtRecorder).unwrap();
    assert!(
        image
            .rgba
            .chunks_exact(4)
            .all(|px| px == [255, 255, 255, 255]),
        "the final pass clears the output white"
    );
    assert!(
        gpu.take_fault().is_none(),
        "wgpu rejected the multi-target or mip attachments"
    );
}
