//! wgpu execution of a scheduled frame graph.
//!
//! [`Gpu`] owns the backend objects — adapter, device, queue, and the two pipelines the
//! milestone's passes need. The renderer always draws into an offscreen texture
//! (`engine-architecture.md` §3); a window, when there is one, is an additional sink that blits that
//! texture ([`crate::present`]), never a render target of its own. [`Gpu::request`] performs adapter
//! selection with a software-rasterizer fallback so the code runs on WARP / llvmpipe in CI and asks
//! for no surface at all; [`Gpu::request_for_surface`] makes the identical choice constrained to
//! adapters that can present to a given surface. [`Gpu::render_to_image`] resolves the schedule's
//! virtual resources to physical textures (honoring the aliasing assignment), records every pass in
//! scheduled order, and reads the final target back to RGBA bytes via a deliberate blocking wait —
//! the answer must correspond to an exact frame.
//!
//! This module is the one place wgpu types appear behind the public API; the declarative graph
//! ([`crate::graph`], [`schedule`](mod@crate::schedule)) stays backend-free.

use std::borrow::Cow;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use ludion_assets::cooked::{CompressedFormat, CookTarget};
use ludion_core::codes::code;
use ludion_core::diagnostics::Diagnostic;

use crate::graph::{PassKind, ResourceKind};
use crate::readback;
use crate::resource::{ResourceId, TextureDesc, TextureFormat};
use crate::schedule::{Aliasing, Schedule};

/// A GPU context: adapter, device, queue, and the graph's pipelines.
///
/// Construct one with [`Gpu::request`] (headless) or [`Gpu::request_for_surface`] (constrained to an
/// adapter that can present to a window); it is reusable across renders. Creating it is the only
/// fallible, adapter-dependent step — everything downstream is deterministic given the same
/// adapter.
pub struct Gpu {
    pub(crate) device: wgpu::Device,
    pub(crate) queue: wgpu::Queue,
    /// Kept because a [`PresentTarget`](crate::PresentTarget) must query its surface's capabilities
    /// against the very adapter the device came from.
    pub(crate) adapter: wgpu::Adapter,
    gradient_pipeline: wgpu::RenderPipeline,
    vignette_pipeline: wgpu::RenderPipeline,
    vignette_bind_group_layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    adapter_name: String,
    adapter_backend: String,
    pub(crate) timestamps_supported: bool,
    /// The BC/ASTC compression features the adapter offered and the device requested.
    compression: wgpu::Features,
    faults: Arc<Mutex<GpuFaults>>,
}

/// The device's uncaptured errors and its loss, filled by the callbacks [`Gpu`] installs and drained
/// by [`Gpu::take_fault`].
#[derive(Default)]
struct GpuFaults {
    /// Validation / out-of-memory / internal error messages since the last take, oldest first.
    validation: Vec<String>,
    /// Messages past [`MAX_PENDING_FAULTS`], counted so a runaway loop cannot grow the sink.
    dropped: usize,
    /// Why the device was lost; once set it is never cleared.
    lost: Option<String>,
}

/// The most validation messages kept between two [`Gpu::take_fault`] calls.
const MAX_PENDING_FAULTS: usize = 8;

/// The largest 2D texture extent the engine's device is pinned to — the WebGPU floor, so a device
/// that has only the floor (iOS) renders what a workstation GPU does.
pub const MAX_TEXTURE_DIMENSION_2D: u32 = 8192;

/// The limit set the engine requests instead of the adapter's own: the WebGPU floor, raised only
/// where shipped content needs it. A workstation GPU that reports far more no longer hides a
/// requirement the floor-only devices (iOS) would hit.
pub fn engine_limits() -> wgpu::Limits {
    wgpu::Limits {
        // The renderer binds `@group(4)` (`shaders/mesh.wgsl`); the floor is 4 groups.
        max_bind_groups: 5,
        // The mesh pass's vertex input takes 20 attributes: six per-vertex (position, normal, color,
        // material, uv, tangent at locations 0-5), two more per-vertex (skin `weights` 18, `bones`
        // 19), and twelve per-instance (model rows 6-9, color 10, material 11, normal-matrix rows
        // 12-15, occlusion `fade` 16, hit-`flash` 17 — `shaders/mesh.wgsl`). The floor is 16.
        max_vertex_attributes: 20,
        ..wgpu::Limits::default()
    }
}

/// A rendered frame in tightly packed, top-to-bottom RGBA8 (no row padding).
///
/// # Example (requires a GPU adapter)
/// ```no_run
/// let (graph, output) = ludion_render_graph::build_graph(320, 180);
/// let schedule = ludion_render_graph::schedule(&graph).unwrap();
/// let gpu = ludion_render_graph::Gpu::request().unwrap();
/// let image = gpu.render_to_image(&schedule, output).unwrap();
/// assert_eq!(image.rgba.len(), 320 * 180 * 4);
/// image.write_png("frame.png").unwrap();
/// ```
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RenderedImage {
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// Packed RGBA8 bytes, `width * height * 4` long, row-major, top row first.
    pub rgba: Vec<u8>,
}

/// Locks the fault sink; a callback that panicked mid-push leaves plain strings, still usable.
fn lock_faults(faults: &Mutex<GpuFaults>) -> MutexGuard<'_, GpuFaults> {
    faults.lock().unwrap_or_else(PoisonError::into_inner)
}

/// The color format all graph textures use (must match the WGSL targets and the readback format).
pub(crate) const TARGET_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8UnormSrgb;

/// The texture-compression features the device requests when the adapter offers them.
const COMPRESSION_FEATURES: wgpu::Features =
    wgpu::Features::TEXTURE_COMPRESSION_BC.union(wgpu::Features::TEXTURE_COMPRESSION_ASTC);

/// Whether a device with `features` can sample `format`: BC7 needs `TEXTURE_COMPRESSION_BC`, ASTC
/// 4×4 needs `TEXTURE_COMPRESSION_ASTC`.
///
/// # Example
/// ```
/// use ludion_assets::cooked::CompressedFormat;
/// use ludion_render_graph::compressed_format_supported;
/// assert!(compressed_format_supported(wgpu::Features::TEXTURE_COMPRESSION_BC, CompressedFormat::Bc7));
/// assert!(!compressed_format_supported(wgpu::Features::empty(), CompressedFormat::Astc4x4));
/// ```
pub fn compressed_format_supported(features: wgpu::Features, format: CompressedFormat) -> bool {
    match format {
        CompressedFormat::Bc7 => features.contains(wgpu::Features::TEXTURE_COMPRESSION_BC),
        CompressedFormat::Astc4x4 => features.contains(wgpu::Features::TEXTURE_COMPRESSION_ASTC),
    }
}

/// Every cook target a device with `features` samples, `Desktop` (BC7) before `Ios` (ASTC): BC
/// leads because an Apple-silicon Mac offers both and cooks desktop.
fn cook_targets_for(features: wgpu::Features) -> Vec<CookTarget> {
    [CookTarget::Desktop, CookTarget::Ios]
        .into_iter()
        .filter(|target| compressed_format_supported(features, target.format()))
        .collect()
}

fn instance_with(backends: wgpu::Backends) -> wgpu::Instance {
    let mut descriptor = wgpu::InstanceDescriptor::new_without_display_handle();
    descriptor.backends = backends;
    wgpu::Instance::new(descriptor.with_env())
}

/// `(backends, allow_software_fallback)`, tried in order; a `WGPU_BACKEND` pin is the only tier.
fn headless_tiers(pinned: Option<wgpu::Backends>) -> Vec<(wgpu::Backends, bool)> {
    match pinned {
        Some(backends) => vec![(backends, true)],
        None => vec![
            (wgpu::Backends::VULKAN | wgpu::Backends::METAL, false),
            (wgpu::Backends::PRIMARY, true),
            (wgpu::Backends::all(), true),
        ],
    }
}

/// Refuses a copy source that is not an output-kind resource: only outputs are exempt from
/// transient aliasing, so any other resource's texture may hold another resource's pixels by the
/// time the copy runs.
fn check_copy_sources(
    schedule: &Schedule,
    sources: impl IntoIterator<Item = ResourceId>,
) -> Result<(), Diagnostic> {
    for id in sources {
        let decl = schedule.resource(id);
        if decl.kind != ResourceKind::Output {
            return Err(Diagnostic::new(
                code::RENDER_GPU_ERROR,
                format!(
                    "`{}` is a copy source but is not an output resource",
                    decl.name
                ),
            )
            .with_help("declare the resource with `RenderGraph::output` so it is never aliased"));
        }
    }
    Ok(())
}

impl Gpu {
    /// The one way any host creates a wgpu instance: `Backends::PRIMARY` (Vulkan, Metal, DX12)
    /// with the `WGPU_*` environment overrides applied. `Instance::default()` enables every
    /// backend, so GL's initialisation would be paid by every process; a `WGPU_BACKEND` pin
    /// replaces the set.
    ///
    /// # Example
    /// ```no_run
    /// let instance = ludion_render_graph::Gpu::instance();
    /// // Create a surface on `instance`, then `Gpu::request_for_surface(&instance, &surface)`.
    /// # drop(instance);
    /// ```
    pub fn instance() -> wgpu::Instance {
        instance_with(wgpu::Backends::PRIMARY)
    }

    /// Requests a headless GPU context.
    ///
    /// Tries the native backend first (Vulkan or Metal, hardware adapters only: the cheapest
    /// start-up), then `Backends::PRIMARY` with a fallback (software) adapter allowed so WARP on
    /// Windows and llvmpipe on CI still work, then every backend (GL-only machines). A
    /// `WGPU_BACKEND` pin is the only tier. No surface is requested — use
    /// [`Gpu::request_for_surface`] when the process also presents to a window.
    ///
    /// # Errors
    /// - `L2003` if no adapter can be found at all (headless environment with no GPU and no
    ///   software rasterizer).
    /// - `L2004` if an adapter was found but the device/queue could not be created.
    pub fn request() -> Result<Gpu, Diagnostic> {
        let mut last = None;
        for (backends, allow_fallback) in headless_tiers(wgpu::Backends::from_env()) {
            match Gpu::request_from(&instance_with(backends), None, allow_fallback) {
                Err(d) if d.code == code::RENDER_NO_ADAPTER => last = Some(d),
                other => return other,
            }
        }
        Err(last.expect("headless_tiers is never empty"))
    }

    /// Requests a GPU context on an adapter that can present to `surface`.
    ///
    /// Identical to [`Gpu::request`] in every other respect — same power preference, same
    /// software-rasterizer retry, same device limits — so a windowed process renders on the adapter
    /// a headless one would have picked whenever that adapter can also present. The caller owns the
    /// instance (build it with [`Gpu::instance`]) and the surface: the surface must be created
    /// *before* the device, because the adapter choice depends on it.
    ///
    /// # Errors
    /// - `L2003` if no adapter compatible with `surface` can be found.
    /// - `L2004` if an adapter was found but the device/queue could not be created.
    pub fn request_for_surface(
        instance: &wgpu::Instance,
        surface: &wgpu::Surface<'_>,
    ) -> Result<Gpu, Diagnostic> {
        Gpu::request_from(instance, Some(surface), true)
    }

    /// The one adapter-selection and device-creation path, shared by [`Gpu::request`] and
    /// [`Gpu::request_for_surface`]; `compatible_surface` and `allow_fallback` are the only things
    /// that differ between them, so the two shells cannot drift. Without `allow_fallback` a
    /// software adapter counts as none (`L2003`), so the caller can widen the backend set.
    fn request_from(
        instance: &wgpu::Instance,
        compatible_surface: Option<&wgpu::Surface<'_>>,
        allow_fallback: bool,
    ) -> Result<Gpu, Diagnostic> {
        ludion_core::require_gpu::assert_gpu_allowed();
        let (adapter, via_fallback) = pollster::block_on(async {
            let high_perf = instance
                .request_adapter(&wgpu::RequestAdapterOptions {
                    power_preference: wgpu::PowerPreference::HighPerformance,
                    force_fallback_adapter: false,
                    compatible_surface,
                    ..Default::default()
                })
                .await;
            match high_perf {
                Ok(adapter)
                    if !allow_fallback
                        && adapter.get_info().device_type == wgpu::DeviceType::Cpu =>
                {
                    Err("only a software adapter is available on these backends".to_string())
                }
                Ok(adapter) => Ok((adapter, false)),
                Err(error) if !allow_fallback => Err(error.to_string()),
                Err(_) => instance
                    .request_adapter(&wgpu::RequestAdapterOptions {
                        power_preference: wgpu::PowerPreference::HighPerformance,
                        force_fallback_adapter: true,
                        compatible_surface,
                        ..Default::default()
                    })
                    .await
                    .map(|adapter| (adapter, true))
                    .map_err(|error| error.to_string()),
            }
        })
        .map_err(|error| {
            Diagnostic::new(
                code::RENDER_NO_ADAPTER,
                format!("no compatible GPU adapter was found: {error}"),
            )
            .with_help(
                "install GPU drivers, or make a software rasterizer available (WARP on Windows, \
                 Mesa llvmpipe on Linux) so headless rendering can proceed",
            )
        })?;

        let info = adapter.get_info();
        ludion_core::require_gpu::assert_hardware_adapter(
            &info.name,
            via_fallback || info.device_type == wgpu::DeviceType::Cpu,
        );
        let timestamps_supported = adapter.features().contains(wgpu::Features::TIMESTAMP_QUERY);
        let compression = adapter.features() & COMPRESSION_FEATURES;
        let required_features = if timestamps_supported {
            wgpu::Features::TIMESTAMP_QUERY | compression
        } else {
            compression
        };

        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("ludion-render-graph device"),
            required_features,
            required_limits: engine_limits(),
            ..Default::default()
        }))
        .map_err(|error| {
            Diagnostic::new(
                code::RENDER_DEVICE_FAILED,
                format!("the GPU adapter was found but the device could not be created: {error}"),
            )
            .with_help(
                "update GPU drivers; the adapter may not support the engine's pinned limit set \
                 (the WebGPU floor plus the raises in `engine_limits`)",
            )
        })?;

        let faults = Arc::new(Mutex::new(GpuFaults::default()));
        let sink = Arc::clone(&faults);
        device.on_uncaptured_error(Arc::new(move |error: wgpu::Error| {
            let mut faults = lock_faults(&sink);
            if faults.validation.len() < MAX_PENDING_FAULTS {
                faults.validation.push(error.to_string());
            } else {
                faults.dropped += 1;
            }
        }));
        let sink = Arc::clone(&faults);
        device.set_device_lost_callback(move |reason, message| {
            lock_faults(&sink).lost = Some(format!("{reason:?}: {message}"));
        });

        Ok(Gpu::assemble(
            device,
            queue,
            adapter,
            info.name,
            format!("{:?}", info.backend),
            timestamps_supported,
            compression,
            faults,
        ))
    }

    /// Builds the graph's own pipelines and sampler on `device` and wraps everything into a [`Gpu`].
    #[allow(clippy::too_many_arguments)]
    fn assemble(
        device: wgpu::Device,
        queue: wgpu::Queue,
        adapter: wgpu::Adapter,
        adapter_name: String,
        adapter_backend: String,
        timestamps_supported: bool,
        compression: wgpu::Features,
        faults: Arc<Mutex<GpuFaults>>,
    ) -> Gpu {
        let (gradient_pipeline, vignette_pipeline, vignette_bind_group_layout) =
            build_pipelines(&device);

        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("ludion-render-graph sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Nearest,
            ..Default::default()
        });

        Gpu {
            device,
            queue,
            adapter,
            gradient_pipeline,
            vignette_pipeline,
            vignette_bind_group_layout,
            sampler,
            adapter_name,
            adapter_backend,
            timestamps_supported,
            compression,
            faults,
        }
    }

    /// A second context over the same adapter, device and queue — for a thread that renders beside
    /// the first (the presenter). The fault sink is shared, not re-installed: wgpu keeps one
    /// uncaptured-error handler and one device-lost callback per device, so a fault on either
    /// context reaches [`Gpu::take_fault`] on both.
    ///
    /// # Errors
    /// Reserved for a future backend that can refuse; today it always succeeds.
    pub fn share(&self) -> Result<Gpu, Diagnostic> {
        Ok(Gpu::assemble(
            self.device.clone(),
            self.queue.clone(),
            self.adapter.clone(),
            self.adapter_name.clone(),
            self.adapter_backend.clone(),
            self.timestamps_supported,
            self.compression,
            Arc::clone(&self.faults),
        ))
    }

    /// Takes the fault the device has reported, if any: a lost device (`L2011`, sticky — every later
    /// call returns it again) or the validation / out-of-memory errors gathered since the last take,
    /// joined (`L2010`, cleared once taken).
    ///
    /// Every submitting `Gpu` method calls this on entry and on exit, and a pending fault replaces
    /// the method's own result: an `L2005` readback failure is the symptom, this is the cause. Work
    /// submitted straight through [`Gpu::device`] / [`Gpu::queue`] (the GI bake) surfaces at the next
    /// such call.
    pub fn take_fault(&self) -> Option<Diagnostic> {
        let mut faults = lock_faults(&self.faults);
        if let Some(reason) = &faults.lost {
            return Some(
                Diagnostic::new(
                    code::RENDER_DEVICE_LOST,
                    format!("the GPU device was lost ({reason})"),
                )
                .with_help(
                    "the GPU device was lost (driver reset/TDR, sleep, removed eGPU); restart the \
                     host — recovery is not supported",
                ),
            );
        }
        if faults.validation.is_empty() {
            return None;
        }
        let mut message = faults.validation.drain(..).collect::<Vec<_>>().join("; ");
        if faults.dropped > 0 {
            message = format!("{message}; and {} more", faults.dropped);
            faults.dropped = 0;
        }
        Some(
            Diagnostic::new(
                code::RENDER_GPU_ERROR,
                format!("the GPU rejected the renderer's work: {message}"),
            )
            .with_help(
                "the renderer issued work the GPU rejected; this is an engine bug — attach the \
                 message",
            ),
        )
    }

    /// Runs `body` between two [`take_fault`](Gpu::take_fault) checks: a fault already pending
    /// refuses the call, and one raised during it replaces the result.
    pub(crate) fn guarded<T>(
        &self,
        body: impl FnOnce() -> Result<T, Diagnostic>,
    ) -> Result<T, Diagnostic> {
        if let Some(fault) = self.take_fault() {
            return Err(fault);
        }
        let result = body();
        match self.take_fault() {
            Some(fault) => Err(fault),
            None => result,
        }
    }

    /// The human-readable adapter name (e.g. `"Microsoft Basic Render Driver"`, `"NVIDIA ..."`).
    pub fn adapter_name(&self) -> &str {
        &self.adapter_name
    }

    /// The backend the adapter uses (e.g. `"Vulkan"`, `"Dx12"`, `"Metal"`).
    pub fn adapter_backend(&self) -> &str {
        &self.adapter_backend
    }

    /// Whether this device can sample `format` — decided by the adapter's features, never the OS.
    ///
    /// # Example (requires a GPU adapter)
    /// ```no_run
    /// use ludion_assets::cooked::CompressedFormat;
    /// let gpu = ludion_render_graph::Gpu::request().unwrap();
    /// let _bc7 = gpu.supports_compressed(CompressedFormat::Bc7);
    /// ```
    pub fn supports_compressed(&self, format: CompressedFormat) -> bool {
        compressed_format_supported(self.compression, format)
    }

    /// The preferred cook target this device samples: the head of [`Gpu::cook_targets`], so
    /// `Desktop` if BC7 is supported, else `Ios` if ASTC is, else `None` (a map must then be
    /// sampled from its source).
    ///
    /// # Example (requires a GPU adapter)
    /// ```no_run
    /// let gpu = ludion_render_graph::Gpu::request().unwrap();
    /// let _target = gpu.cook_target();
    /// ```
    pub fn cook_target(&self) -> Option<CookTarget> {
        self.cook_targets().first().copied()
    }

    /// Every cook target this device samples, `Desktop` before `Ios`; empty when it supports
    /// neither format.
    ///
    /// # Example (requires a GPU adapter)
    /// ```no_run
    /// let gpu = ludion_render_graph::Gpu::request().unwrap();
    /// let _targets = gpu.cook_targets();
    /// ```
    pub fn cook_targets(&self) -> Vec<CookTarget> {
        cook_targets_for(self.compression)
    }

    /// Renders `schedule` and returns the `output` resource read back as packed RGBA8.
    ///
    /// Uses a blocking wait on the GPU, so the returned image corresponds to exactly this
    /// invocation's frame. Deterministic: the same schedule on the same adapter produces identical
    /// bytes.
    ///
    /// # Errors
    /// `L2005` if the final target cannot be mapped for readback; `L2010` / `L2011` if the GPU
    /// rejected the work or the device was lost (see [`Gpu::take_fault`]).
    pub fn render_to_image(
        &self,
        schedule: &Schedule,
        output: ResourceId,
    ) -> Result<RenderedImage, Diagnostic> {
        self.guarded(|| {
            let frame = self.allocate_physicals(schedule);
            let mut encoder = self
                .device
                .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                    label: Some("ludion-render-graph frame"),
                });
            self.record_passes(schedule, &frame, &mut encoder, None);
            self.queue.submit([encoder.finish()]);

            let output_view = &frame[schedule.aliasing.assignment[output.index()]];
            readback::read_texture_to_image(
                self,
                &output_view.texture,
                schedule.resource(output).desc(),
            )
        })
    }

    /// Returns physical textures for `schedule`, reusing `cache`'s when it was built for the same
    /// aliasing and reallocating it when it was not. See [`PhysicalCache`].
    pub(crate) fn ensure_physicals<'a>(
        &self,
        schedule: &Schedule,
        cache: &'a mut Option<PhysicalCache>,
    ) -> &'a [PhysicalGpuTexture] {
        let reusable = cache
            .as_ref()
            .is_some_and(|cached| cached.aliasing == schedule.aliasing);
        if !reusable {
            *cache = Some(PhysicalCache {
                aliasing: schedule.aliasing.clone(),
                textures: self.allocate_physicals(schedule),
            });
        }
        &cache.as_ref().expect("just ensured").textures
    }

    /// Allocates one wgpu texture (and view) per physical texture the aliaser assigned. Indexed by
    /// physical id.
    pub(crate) fn allocate_physicals(&self, schedule: &Schedule) -> Vec<PhysicalGpuTexture> {
        schedule
            .aliasing
            .physicals
            .iter()
            .map(|phys| {
                // A physical texture is 3D exactly when the resources bound to it are: the aliaser
                // never mixes the two (slice count is part of alias compatibility).
                let (depth_or_array_layers, dimension) = match phys.depth_slices {
                    Some(depth) => (depth, wgpu::TextureDimension::D3),
                    None => (1, wgpu::TextureDimension::D2),
                };
                let texture = self.device.create_texture(&wgpu::TextureDescriptor {
                    label: Some("ludion-render-graph physical"),
                    size: wgpu::Extent3d {
                        width: phys.extent.width,
                        height: phys.extent.height,
                        depth_or_array_layers,
                    },
                    mip_level_count: phys.mip_levels,
                    sample_count: 1,
                    dimension,
                    format: phys.format.to_wgpu(),
                    usage: physical_usages(phys),
                    view_formats: &[],
                });
                let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
                // An attachment or storage view covers exactly one mip, so a mipped texture also
                // gets one single-level view per mip.
                let mip_views = if phys.mip_levels > 1 {
                    (0..phys.mip_levels)
                        .map(|level| {
                            texture.create_view(&wgpu::TextureViewDescriptor {
                                base_mip_level: level,
                                mip_level_count: Some(1),
                                ..Default::default()
                            })
                        })
                        .collect()
                } else {
                    Vec::new()
                };
                PhysicalGpuTexture {
                    texture,
                    view,
                    mip_views,
                }
            })
            .collect()
    }

    /// Records every pass into `encoder` in scheduled order, resolving virtual resources to physical
    /// textures. `timestamp_writes` supplies per-pass timestamp queries when timings are collected.
    pub(crate) fn record_passes(
        &self,
        schedule: &Schedule,
        frame: &[PhysicalGpuTexture],
        encoder: &mut wgpu::CommandEncoder,
        query_set: Option<&wgpu::QuerySet>,
    ) {
        for (position, &pass_idx) in schedule.order.iter().enumerate() {
            let pass = &schedule.passes[pass_idx];
            // Externally-recorded passes are rejected before any attachment is resolved; every
            // built-in proving pass is color-only.
            let color = match pass.kind {
                PassKind::External { label } | PassKind::Compute { label } => panic!(
                    "pass `{label}` cannot run on the built-in proving executor; record it with \
                     Gpu::execute and a PassRecorder"
                ),
                PassKind::Gradient | PassKind::Vignette { .. } => *pass
                    .color_targets()
                    .first()
                    .expect("built-in proving passes always have a color target"),
            };
            let target = view_of(schedule, frame, color);

            let timestamp_writes = query_set.map(|set| wgpu::RenderPassTimestampWrites {
                query_set: set,
                beginning_of_pass_write_index: Some((position * 2) as u32),
                end_of_pass_write_index: Some((position * 2 + 1) as u32),
            });

            let mut render_pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some(&pass.name),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: target,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes,
                occlusion_query_set: None,
                multiview_mask: None,
            });

            match pass.kind {
                PassKind::Gradient => {
                    render_pass.set_pipeline(&self.gradient_pipeline);
                    render_pass.draw(0..3, 0..1);
                }
                PassKind::Vignette { source } => {
                    let input = &frame[schedule.aliasing.assignment[source.index()]].view;
                    let bind_group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
                        label: Some("ludion-render-graph vignette input"),
                        layout: &self.vignette_bind_group_layout,
                        entries: &[
                            wgpu::BindGroupEntry {
                                binding: 0,
                                resource: wgpu::BindingResource::TextureView(input),
                            },
                            wgpu::BindGroupEntry {
                                binding: 1,
                                resource: wgpu::BindingResource::Sampler(&self.sampler),
                            },
                        ],
                    });
                    render_pass.set_pipeline(&self.vignette_pipeline);
                    render_pass.set_bind_group(0, &bind_group, &[]);
                    render_pass.draw(0..3, 0..1);
                }
                // Rejected above, before the render pass was opened.
                PassKind::External { .. } | PassKind::Compute { .. } => unreachable!(),
            }
        }
    }

    /// The underlying wgpu device — for clients (`ludion-renderer`) that build their own pipelines
    /// and resources on top of this context. The device is reusable across renders.
    pub fn device(&self) -> &wgpu::Device {
        &self.device
    }

    /// The underlying wgpu queue, for the same clients.
    pub fn queue(&self) -> &wgpu::Queue {
        &self.queue
    }

    /// Executes `schedule` with an external [`PassRecorder`] and reads `output` back as RGBA8.
    ///
    /// This is the entry point `ludion-renderer` uses: the graph supplies deterministic scheduling
    /// and transient aliasing, and for each pass in order the executor resolves the target, depth,
    /// storage, and input texture views (honoring aliasing) and hands them to `recorder`, which
    /// records the actual GPU work. The recorder owns pipeline selection and the pass it opens (so a
    /// pass may attach the graph-owned depth buffer, or dispatch compute over the graph-owned storage
    /// texture instead of drawing). A single blocking readback wait
    /// follows, exactly as in
    /// [`Gpu::render_to_image`], so the returned image corresponds to this frame.
    ///
    /// # Errors
    /// `L2005` if the final target cannot be mapped for readback; `L2010` / `L2011` if the GPU
    /// rejected the work or the device was lost (see [`Gpu::take_fault`]).
    ///
    /// # Example (requires a GPU adapter)
    /// ```no_run
    /// use ludion_render_graph::{Gpu, RecordedPass, PassRecorder, RenderGraph, TextureDesc};
    ///
    /// struct Clear;
    /// impl PassRecorder for Clear {
    ///     fn record(&self, pass: &mut RecordedPass<'_>) {
    ///         // Begin a render pass that clears the target to a color, then draw nothing.
    ///         let _ = pass.begin_clear(wgpu::Color::BLUE);
    ///     }
    /// }
    ///
    /// let mut graph = RenderGraph::new();
    /// let out = graph.output("final", TextureDesc::present_target(16, 16));
    /// graph.add_external_pass("clear", "clear", vec![], vec![out]);
    /// let schedule = ludion_render_graph::schedule(&graph).unwrap();
    /// let gpu = Gpu::request().unwrap();
    /// let image = gpu.execute(&schedule, out, &Clear).unwrap();
    /// assert_eq!(image.rgba.len(), 16 * 16 * 4);
    /// ```
    pub fn execute(
        &self,
        schedule: &Schedule,
        output: ResourceId,
        recorder: &dyn PassRecorder,
    ) -> Result<RenderedImage, Diagnostic> {
        self.guarded(|| {
            let frame = self.allocate_physicals(schedule);
            let mut encoder = self
                .device
                .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                    label: Some("ludion-render-graph external frame"),
                });
            self.record_external_passes(schedule, &frame, &mut encoder, recorder, None);
            self.queue.submit([encoder.finish()]);

            let output_view = &frame[schedule.aliasing.assignment[output.index()]];
            readback::read_texture_to_image(
                self,
                &output_view.texture,
                schedule.resource(output).desc(),
            )
        })
    }

    /// Executes `schedule` like [`execute`](Gpu::execute), but copies each `(output, dest)` pair —
    /// `dest` a subresource of the caller's texture (a cube-array layer, say) — instead of reading back.
    /// One submit, no wait: the copy is ordered after the passes on the queue, so a later submit
    /// that samples `dest` sees the finished frame.
    ///
    /// Each `output` must declare [`TextureUsage::CopySrc`](crate::TextureUsage::CopySrc) and match
    /// its `dest`'s format; `dest`'s texture needs `COPY_DST` and room for `output`'s extent at
    /// `dest.origin`.
    ///
    /// # Errors
    /// `L2010` if a copy source is not an output-kind resource (a transient may be aliased away
    /// before the copy), or if the GPU rejected the work; `L2011` if the device was lost (see
    /// [`Gpu::take_fault`]).
    ///
    /// # Example (requires a GPU adapter)
    /// ```no_run
    /// use ludion_render_graph::{Gpu, RecordedPass, PassRecorder, RenderGraph, TextureDesc};
    ///
    /// struct Clear;
    /// impl PassRecorder for Clear {
    ///     fn record(&self, pass: &mut RecordedPass<'_>) {
    ///         let _ = pass.begin_clear(wgpu::Color::BLUE);
    ///     }
    /// }
    ///
    /// let gpu = Gpu::request().unwrap();
    /// let mut graph = RenderGraph::new();
    /// let out = graph.output("final", TextureDesc::present_target(16, 16));
    /// graph.add_external_pass("clear", "clear", vec![], vec![out]);
    /// let schedule = ludion_render_graph::schedule(&graph).unwrap();
    /// let target = gpu.device().create_texture(&wgpu::TextureDescriptor {
    ///     label: Some("layers"),
    ///     size: wgpu::Extent3d { width: 16, height: 16, depth_or_array_layers: 3 },
    ///     mip_level_count: 1,
    ///     sample_count: 1,
    ///     dimension: wgpu::TextureDimension::D2,
    ///     format: wgpu::TextureFormat::Rgba8UnormSrgb,
    ///     usage: wgpu::TextureUsages::COPY_DST | wgpu::TextureUsages::TEXTURE_BINDING,
    ///     view_formats: &[],
    /// });
    /// let dest = wgpu::TexelCopyTextureInfo {
    ///     texture: &target,
    ///     mip_level: 0,
    ///     origin: wgpu::Origin3d { x: 0, y: 0, z: 2 },
    ///     aspect: wgpu::TextureAspect::All,
    /// };
    /// gpu.execute_into(&schedule, &Clear, &[(out, dest)]).unwrap();
    /// ```
    pub fn execute_into(
        &self,
        schedule: &Schedule,
        recorder: &dyn PassRecorder,
        copies: &[(ResourceId, wgpu::TexelCopyTextureInfo<'_>)],
    ) -> Result<(), Diagnostic> {
        check_copy_sources(schedule, copies.iter().map(|(output, _)| *output))?;
        self.guarded(|| {
            let frame = self.allocate_physicals(schedule);
            let mut encoder = self
                .device
                .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                    label: Some("ludion-render-graph external frame into"),
                });
            self.record_external_passes(schedule, &frame, &mut encoder, recorder, None);
            for (output, dest) in copies {
                let output_view = &frame[schedule.aliasing.assignment[output.index()]];
                let extent = schedule.resource(*output).desc().extent;
                encoder.copy_texture_to_texture(
                    output_view.texture.as_image_copy(),
                    *dest,
                    wgpu::Extent3d {
                        width: extent.width,
                        height: extent.height,
                        depth_or_array_layers: 1,
                    },
                );
            }
            self.queue.submit([encoder.finish()]);
            Ok(())
        })
    }

    /// Records every externally-declared pass into `encoder` in scheduled order, resolving virtual
    /// resources to physical texture views and dispatching to `recorder`. When `query_set` is
    /// given, each pass receives a `(begin, end)` timestamp pair at `(position*2, position*2+1)` —
    /// the same index scheme as [`record_passes`](Gpu::record_passes) — which the recorder attaches
    /// via [`RecordedPass::timestamp_writes`].
    pub(crate) fn record_external_passes(
        &self,
        schedule: &Schedule,
        frame: &[PhysicalGpuTexture],
        encoder: &mut wgpu::CommandEncoder,
        recorder: &dyn PassRecorder,
        query_set: Option<&wgpu::QuerySet>,
    ) {
        for (position, &pass_idx) in schedule.order.iter().enumerate() {
            let pass = &schedule.passes[pass_idx];
            // A depth-only pass (the shadow map) carries no color targets — the recorder drives its
            // own depth-only render pass from `depth`.
            let colors: Vec<ColorTarget> = pass
                .color_targets()
                .iter()
                .map(|&c| {
                    let desc = &schedule.resources[c.index()].desc;
                    ColorTarget {
                        view: view_of(schedule, frame, c),
                        format: desc.format,
                        extent: desc.mip_extent(c.mip_level().unwrap_or(0)),
                    }
                })
                .collect();
            let depth_view = pass.depth_target().map(|d| view_of(schedule, frame, d));
            let storage_view = pass.storage_target().map(|s| view_of(schedule, frame, s));
            let inputs: Vec<&wgpu::TextureView> = pass
                .reads
                .iter()
                .map(|&r| view_of(schedule, frame, r))
                .collect();

            let mut recorded = RecordedPass {
                label: &pass.name,
                kind: pass.kind.as_str(),
                compute: pass.kind.is_compute(),
                colors,
                depth: depth_view,
                storage: storage_view,
                inputs: &inputs,
                device: &self.device,
                encoder,
                timestamps: query_set.map(|set| PassTimestamps {
                    set,
                    begin: (position * 2) as u32,
                    end: (position * 2 + 1) as u32,
                }),
            };
            recorder.record(&mut recorded);
        }
    }
}

/// The timestamp-query pair assigned to one recorded pass (query set + begin/end indices).
struct PassTimestamps<'a> {
    set: &'a wgpu::QuerySet,
    begin: u32,
    end: u32,
}

/// Records the GPU work for one externally-declared pass ([`PassKind::External`]).
///
/// The renderer implements this and dispatches on [`RecordedPass::kind`] to pick a pipeline. It is
/// handed a fully-resolved [`RecordedPass`] per pass, in scheduled order.
pub trait PassRecorder {
    /// Record the work for `pass` into its encoder.
    fn record(&self, pass: &mut RecordedPass<'_>);
}

/// The view a handle names: the single-level view for a mip handle, the whole-texture view otherwise.
fn view_of<'f>(
    schedule: &Schedule,
    frame: &'f [PhysicalGpuTexture],
    id: ResourceId,
) -> &'f wgpu::TextureView {
    let phys = &frame[schedule.aliasing.assignment[id.index()]];
    match id.mip_level() {
        Some(level) if !phys.mip_views.is_empty() => &phys.mip_views[level as usize],
        _ => &phys.view,
    }
}

/// One resolved color attachment: the view plus its format and pixel size (the named mip's, for a
/// mip handle).
pub struct ColorTarget<'a> {
    /// The attachment view.
    pub view: &'a wgpu::TextureView,
    /// The attachment's pixel format.
    pub format: TextureFormat,
    /// The attachment's pixel dimensions.
    pub extent: crate::resource::Extent2d,
}

/// One pass, resolved to concrete GPU views and ready to record into.
///
/// Input and target textures have already been resolved through the transient-aliasing assignment,
/// so the recorder never touches [`ResourceId`]s. It records into [`encoder`](RecordedPass::encoder)
/// — usually by calling [`begin`](RecordedPass::begin) or [`begin_clear`](RecordedPass::begin_clear)
/// to open a render pass over every [`colors`](RecordedPass::colors) target, or by opening its own
/// render pass directly (e.g. to attach the graph-owned [`depth`](RecordedPass::depth) buffer
/// alongside the color targets).
///
/// A **depth-only** pass (the shadow map) has no color attachments ([`colors`](RecordedPass::colors)
/// is empty; `begin`/`begin_clear` panic), and the recorder opens its own depth-only render pass
/// from [`depth`](RecordedPass::depth).
///
/// A **compute** pass ([`is_compute`](RecordedPass::is_compute)) has no attachments at all: it opens
/// [`begin_compute`](RecordedPass::begin_compute), binds [`storage`](RecordedPass::storage) as its
/// writable output, and issues its own dispatch — the graph never sees a workgroup count.
///
/// # Timing contract
///
/// When the executor is collecting timings — the blocking [`Gpu::dump_timings_json`] dump or a
/// [`PassTimingRing`](crate::PassTimingRing)-armed [`Gpu::render_streamed`] — this pass has a
/// timestamp pair assigned. The [`begin`](RecordedPass::begin)/[`begin_clear`](RecordedPass::begin_clear)
/// helpers attach it automatically; a recorder that builds its **own** `RenderPassDescriptor` must
/// pass [`timestamp_writes`](RecordedPass::timestamp_writes) in it, or its pass reports zero time.
/// A recorder that opens several render passes for one graph pass should attach the pair to exactly
/// one — the timestamp indices are per *graph* pass, and wgpu forbids writing a query index twice.
pub struct RecordedPass<'a> {
    /// The pass's human-readable name.
    pub label: &'a str,
    /// The pass kind's dispatch token (the `label` given to
    /// [`add_external_pass`](crate::RenderGraph::add_external_pass) or
    /// [`add_external_compute_pass`](crate::RenderGraph::add_external_compute_pass)).
    pub kind: &'a str,
    /// Whether this pass dispatches compute work. Read through
    /// [`is_compute`](RecordedPass::is_compute).
    compute: bool,
    /// The resolved color attachments, empty for a depth-only pass. Read through
    /// [`colors`](RecordedPass::colors).
    colors: Vec<ColorTarget<'a>>,
    /// The resolved depth attachment view, when the pass declared a depth resource (the scene pass's
    /// depth buffer, or a shadow pass's shadow map). The graph owns and aliases this texture; the
    /// recorder attaches it and picks its clear/store ops, just as it does the color target's.
    pub depth: Option<&'a wgpu::TextureView>,
    /// The resolved storage-texture view a compute pass writes, when it declared one. The graph owns
    /// and aliases this texture; the recorder binds it into its own bind group (there is no
    /// attachment to attach it to).
    pub storage: Option<&'a wgpu::TextureView>,
    /// The resolved input views this pass samples, in declared-reads order.
    pub inputs: &'a [&'a wgpu::TextureView],
    /// The device, for a recorder that needs to allocate its own per-pass resources (e.g. a bind
    /// group over its inputs).
    pub device: &'a wgpu::Device,
    /// The frame's command encoder to record into.
    pub encoder: &'a mut wgpu::CommandEncoder,
    /// The timestamp pair assigned to this pass when timings are collected (see the struct docs).
    /// Private: recorders consume it through [`timestamp_writes`](RecordedPass::timestamp_writes).
    timestamps: Option<PassTimestamps<'a>>,
}

impl<'a> RecordedPass<'a> {
    /// Whether this pass dispatches compute work rather than drawing. A compute recorder opens
    /// [`begin_compute`](RecordedPass::begin_compute) and never touches the attachment accessors.
    pub fn is_compute(&self) -> bool {
        self.compute
    }

    /// The resolved color attachments in declared order; empty for a depth-only or compute pass.
    pub fn colors(&self) -> &[ColorTarget<'a>] {
        &self.colors
    }

    /// Opens a render pass over every color target that *loads* existing contents (no clear). The
    /// caller sets pipelines and draws, then drops the returned pass.
    ///
    /// # Panics
    /// On a pass with no color targets.
    pub fn begin(&mut self) -> wgpu::RenderPass<'_> {
        self.color_pass(wgpu::LoadOp::Load)
    }

    /// Opens a render pass over every color target that clears each to `color` first.
    ///
    /// # Panics
    /// On a pass with no color targets.
    pub fn begin_clear(&mut self, color: wgpu::Color) -> wgpu::RenderPass<'_> {
        self.color_pass(wgpu::LoadOp::Clear(color))
    }

    /// Opens a compute pass carrying this pass's timestamp pair, for a
    /// [`PassKind::Compute`] recorder. The caller sets the pipeline and
    /// bind groups (binding [`storage`](RecordedPass::storage) as the writable output), issues its
    /// dispatch, and drops the returned pass.
    ///
    /// This is the compute counterpart of [`begin`](RecordedPass::begin): the timestamps are
    /// attached here, so a compute pass reports its GPU time under its own name with no
    /// special-casing anywhere downstream.
    pub fn begin_compute(&mut self) -> wgpu::ComputePass<'_> {
        let timestamp_writes = self.compute_timestamp_writes();
        self.encoder
            .begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some(self.label),
                timestamp_writes,
            })
    }

    /// This pass's timestamp writes, for a recorder that builds its own `RenderPassDescriptor`
    /// (e.g. to attach a depth buffer). `None` outside a timing run. Descriptor-level timestamp
    /// writes are the wgpu-30 way to bracket a pass — no extra feature beyond `TIMESTAMP_QUERY`
    /// (encoder-level `write_timestamp` would need `TIMESTAMP_QUERY_INSIDE_ENCODERS`, which Apple
    /// GPUs do not expose).
    pub fn timestamp_writes(&self) -> Option<wgpu::RenderPassTimestampWrites<'a>> {
        self.timestamps
            .as_ref()
            .map(|t| wgpu::RenderPassTimestampWrites {
                query_set: t.set,
                beginning_of_pass_write_index: Some(t.begin),
                end_of_pass_write_index: Some(t.end),
            })
    }

    /// This pass's timestamp writes in compute form, for a recorder that builds its own
    /// `ComputePassDescriptor` instead of calling [`begin_compute`](RecordedPass::begin_compute).
    /// The query indices are the same pair — a graph pass is timed once, whichever kind it is.
    pub fn compute_timestamp_writes(&self) -> Option<wgpu::ComputePassTimestampWrites<'a>> {
        self.timestamps
            .as_ref()
            .map(|t| wgpu::ComputePassTimestampWrites {
                query_set: t.set,
                beginning_of_pass_write_index: Some(t.begin),
                end_of_pass_write_index: Some(t.end),
            })
    }

    fn color_pass(&mut self, load: wgpu::LoadOp<wgpu::Color>) -> wgpu::RenderPass<'_> {
        let timestamp_writes = self.timestamp_writes();
        assert!(
            !self.colors.is_empty(),
            "this pass is depth-only and has no color target"
        );
        let attachments: Vec<_> = self
            .colors
            .iter()
            .map(|c| {
                Some(wgpu::RenderPassColorAttachment {
                    view: c.view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load,
                        store: wgpu::StoreOp::Store,
                    },
                })
            })
            .collect();
        self.encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some(self.label),
            color_attachments: &attachments,
            depth_stencil_attachment: None,
            timestamp_writes,
            occlusion_query_set: None,
            multiview_mask: None,
        })
    }
}

/// The wgpu usage bits for one physical texture: the union its resources declared, plus
/// `TEXTURE_BINDING` on every 2D color target.
///
/// A present target samples the graph's output texture ([`crate::present`]), and `present_target`
/// declares only `RenderAttachment | CopySrc` — but a present target is a *sink*, not a pass, so the
/// declarative graph never learns about it and the usage cannot be declared there without moving
/// every `dump-graph` byte. Granting the bit here instead keeps the dump (and therefore the
/// snapshot gate) exactly as it was; a usage flag widens what the texture may be bound to and
/// changes nothing about what is drawn into it.
fn physical_usages(phys: &crate::schedule::PhysicalTexture) -> wgpu::TextureUsages {
    let declared = TextureDesc::wgpu_usages(&phys.usages);
    let flat_color = phys.depth_slices.is_none() && phys.format != TextureFormat::Depth32Float;
    if flat_color {
        declared | wgpu::TextureUsages::TEXTURE_BINDING
    } else {
        declared
    }
}

/// One physical texture plus its default view.
pub(crate) struct PhysicalGpuTexture {
    pub(crate) texture: wgpu::Texture,
    pub(crate) view: wgpu::TextureView,
    /// One single-level view per mip; empty when the texture is not mipped.
    pub(crate) mip_views: Vec<wgpu::TextureView>,
}

/// A frame's physical textures, held across frames so the graph is not re-created on the GPU every
/// frame ([`Gpu::ensure_physicals`]).
///
/// Reuse is sound because a graph texture never carries information between frames: every pass
/// either clears its attachment or covers it whole (the overlay's `LoadOp::Load` sits under an
/// opaque, unblended full-target blit), and the scheduler already rejects a read of a resource no
/// pass writes. Two consecutive frames therefore differ only in what they draw, never in what they
/// inherit.
///
/// The key is the whole [`Aliasing`] — both which physical backs each virtual resource *and* every
/// physical's extent, slice count, format and usages. That is exactly what decides which texture a
/// pass is handed, so a resize, a post-chain change, or any re-alias rebuilds rather than handing a
/// pass a texture built for something else.
pub(crate) struct PhysicalCache {
    aliasing: Aliasing,
    textures: Vec<PhysicalGpuTexture>,
}

impl Schedule {
    /// The declaration of a resource by handle (used by the executor for readback sizing).
    pub(crate) fn resource(&self, id: ResourceId) -> &crate::graph::ResourceDecl {
        &self.resources[id.index()]
    }
}

impl crate::graph::ResourceDecl {
    pub(crate) fn desc(&self) -> &TextureDesc {
        &self.desc
    }
}

/// Builds the gradient (no inputs) and vignette (one sampled input) pipelines plus the vignette bind
/// group layout. Both target [`TARGET_FORMAT`] and use the shared fullscreen-triangle vertex stage.
fn build_pipelines(
    device: &wgpu::Device,
) -> (
    wgpu::RenderPipeline,
    wgpu::RenderPipeline,
    wgpu::BindGroupLayout,
) {
    let gradient_module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("gradient"),
        source: wgpu::ShaderSource::Wgsl(Cow::Borrowed(include_str!("shaders/gradient.wgsl"))),
    });
    let vignette_module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("vignette"),
        source: wgpu::ShaderSource::Wgsl(Cow::Borrowed(include_str!("shaders/vignette.wgsl"))),
    });

    let color_target = wgpu::ColorTargetState {
        format: TARGET_FORMAT,
        blend: None,
        write_mask: wgpu::ColorWrites::ALL,
    };

    // Gradient: no bind groups.
    let gradient_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("gradient layout"),
        bind_group_layouts: &[],
        immediate_size: 0,
    });
    let gradient_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("gradient pipeline"),
        layout: Some(&gradient_layout),
        vertex: wgpu::VertexState {
            module: &gradient_module,
            entry_point: Some("vs_main"),
            buffers: &[],
            compilation_options: wgpu::PipelineCompilationOptions::default(),
        },
        fragment: Some(wgpu::FragmentState {
            module: &gradient_module,
            entry_point: Some("fs_main"),
            targets: &[Some(color_target.clone())],
            compilation_options: wgpu::PipelineCompilationOptions::default(),
        }),
        primitive: wgpu::PrimitiveState::default(),
        depth_stencil: None,
        multisample: wgpu::MultisampleState::default(),
        multiview_mask: None,
        cache: None,
    });

    // Vignette: one texture + sampler bind group.
    let vignette_bind_group_layout =
        device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("vignette bind group layout"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
    let vignette_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("vignette layout"),
        bind_group_layouts: &[Some(&vignette_bind_group_layout)],
        immediate_size: 0,
    });
    let vignette_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("vignette pipeline"),
        layout: Some(&vignette_layout),
        vertex: wgpu::VertexState {
            module: &vignette_module,
            entry_point: Some("vs_main"),
            buffers: &[],
            compilation_options: wgpu::PipelineCompilationOptions::default(),
        },
        fragment: Some(wgpu::FragmentState {
            module: &vignette_module,
            entry_point: Some("fs_main"),
            targets: &[Some(color_target)],
            compilation_options: wgpu::PipelineCompilationOptions::default(),
        }),
        primitive: wgpu::PrimitiveState::default(),
        depth_stencil: None,
        multisample: wgpu::MultisampleState::default(),
        multiview_mask: None,
        cache: None,
    });

    (
        gradient_pipeline,
        vignette_pipeline,
        vignette_bind_group_layout,
    )
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_copy_source_must_be_an_output_resource() {
        let mut graph = crate::RenderGraph::new();
        let mid = graph.transient("mid", crate::TextureDesc::color(8, 8));
        let out = graph.output("final", crate::TextureDesc::present_target(8, 8));
        graph.add_external_pass("a", "a", vec![], vec![mid]);
        graph.add_external_pass("b", "b", vec![mid], vec![out]);
        let schedule = crate::schedule(&graph).unwrap();
        assert!(check_copy_sources(&schedule, [out]).is_ok());
        let err = check_copy_sources(&schedule, [out, mid]).unwrap_err();
        assert_eq!(err.code.as_str(), "L2010");
        assert!(err.message.contains("`mid`"), "{}", err.message);
    }

    use super::*;

    #[test]
    fn headless_tiers_try_native_hardware_first_and_a_pin_alone() {
        assert_eq!(
            headless_tiers(None),
            vec![
                (wgpu::Backends::VULKAN | wgpu::Backends::METAL, false),
                (wgpu::Backends::PRIMARY, true),
                (wgpu::Backends::all(), true),
            ]
        );
        assert_eq!(
            headless_tiers(Some(wgpu::Backends::DX12)),
            vec![(wgpu::Backends::DX12, true)]
        );
    }

    /// A shared context reads the original's fault sink: one handler per device, two views of it.
    #[test]
    fn needs_gpu_share_sees_the_original_gpus_faults() {
        let gpu = match Gpu::request() {
            Ok(gpu) => gpu,
            Err(d) if ludion_core::require_gpu::is_no_adapter(std::slice::from_ref(&d)) => {
                eprintln!("SKIPPED share: no GPU adapter");
                return;
            }
            Err(d) => panic!("gpu request failed: {d:?}"),
        };
        let shared = gpu.share().expect("share");
        assert!(Arc::ptr_eq(&gpu.faults, &shared.faults));
        lock_faults(&gpu.faults).validation.push("boom".to_owned());
        let fault = shared.take_fault().expect("the shared gpu sees the fault");
        assert!(fault.message.contains("boom"));
        assert!(
            gpu.take_fault().is_none(),
            "the sink is drained once for both"
        );
    }

    /// Two cleared 16² outputs land in layers 2 and 1 of a 3-layer texture, with no readback of the graph.
    #[test]
    fn needs_gpu_execute_into_copies_the_output_to_a_layer() {
        struct Red;
        impl PassRecorder for Red {
            fn record(&self, pass: &mut RecordedPass<'_>) {
                let _ = pass.begin_clear(wgpu::Color::RED);
            }
        }
        let gpu = match Gpu::request() {
            Ok(gpu) => gpu,
            Err(d) if ludion_core::require_gpu::is_no_adapter(std::slice::from_ref(&d)) => {
                eprintln!("SKIPPED execute_into: no GPU adapter");
                return;
            }
            Err(d) => panic!("gpu request failed: {d:?}"),
        };
        let mut graph = crate::RenderGraph::new();
        let out = graph.output("final", crate::TextureDesc::present_target(16, 16));
        let second = graph.output("second", crate::TextureDesc::present_target(16, 16));
        graph.add_external_pass("clear", "clear", vec![], vec![out, second]);
        let schedule = crate::schedule(&graph).unwrap();
        let device = gpu.device();
        let target = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("layers"),
            size: wgpu::Extent3d {
                width: 16,
                height: 16,
                depth_or_array_layers: 3,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8UnormSrgb,
            usage: wgpu::TextureUsages::COPY_DST | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let layer = |z| wgpu::TexelCopyTextureInfo {
            texture: &target,
            mip_level: 0,
            origin: wgpu::Origin3d { x: 0, y: 0, z },
            aspect: wgpu::TextureAspect::All,
        };
        gpu.execute_into(&schedule, &Red, &[(out, layer(2)), (second, layer(1))])
            .unwrap();

        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("layers readback"),
            size: 256 * 16 * 3,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut encoder =
            device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
        encoder.copy_texture_to_buffer(
            layer(0),
            wgpu::TexelCopyBufferInfo {
                buffer: &buffer,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(256),
                    rows_per_image: Some(16),
                },
            },
            target.size(),
        );
        gpu.queue().submit([encoder.finish()]);
        let (tx, rx) = std::sync::mpsc::channel();
        buffer.slice(..).map_async(wgpu::MapMode::Read, move |r| {
            let _ = tx.send(r);
        });
        device
            .poll(wgpu::PollType::wait_indefinitely())
            .expect("the device polls");
        rx.recv().unwrap().expect("the buffer maps");
        let mapped = buffer.slice(..).get_mapped_range().expect("mapped range");
        let texel = |z: usize, x: usize, y: usize| {
            let at = z * 256 * 16 + y * 256 + x * 4;
            [mapped[at], mapped[at + 1], mapped[at + 2], mapped[at + 3]]
        };
        assert_eq!(texel(2, 0, 0), [255, 0, 0, 255]);
        assert_eq!(texel(2, 15, 15), [255, 0, 0, 255]);
        assert_eq!(texel(1, 15, 15), [255, 0, 0, 255]);
        assert_eq!(texel(0, 8, 8), [0, 0, 0, 0], "layer 0 was written");
    }

    #[test]
    fn compressed_support_follows_the_matching_feature() {
        let bc = wgpu::Features::TEXTURE_COMPRESSION_BC;
        let astc = wgpu::Features::TEXTURE_COMPRESSION_ASTC;
        assert!(compressed_format_supported(bc, CompressedFormat::Bc7));
        assert!(!compressed_format_supported(bc, CompressedFormat::Astc4x4));
        assert!(compressed_format_supported(astc, CompressedFormat::Astc4x4));
        assert!(!compressed_format_supported(astc, CompressedFormat::Bc7));
        assert!(!compressed_format_supported(
            wgpu::Features::empty(),
            CompressedFormat::Bc7
        ));
    }

    #[test]
    fn cook_target_prefers_bc_then_astc_then_none() {
        let bc = wgpu::Features::TEXTURE_COMPRESSION_BC;
        let astc = wgpu::Features::TEXTURE_COMPRESSION_ASTC;
        assert_eq!(
            cook_targets_for(bc | astc),
            [CookTarget::Desktop, CookTarget::Ios]
        );
        assert_eq!(cook_targets_for(bc), [CookTarget::Desktop]);
        assert_eq!(cook_targets_for(astc), [CookTarget::Ios]);
        assert!(cook_targets_for(wgpu::Features::empty()).is_empty());
    }

    /// The pinned limit set is the WebGPU floor plus its commented raises: the texture bound the
    /// renderer validates against is the floor's, and the raises are the ones the mesh pass needs.
    #[test]
    fn engine_limits_are_the_floor_plus_the_documented_raises() {
        let limits = engine_limits();
        let floor = wgpu::Limits::default();
        assert_eq!(limits.max_texture_dimension_2d, MAX_TEXTURE_DIMENSION_2D);
        assert_eq!(limits.max_bind_groups, 5);
        assert_eq!(limits.max_vertex_attributes, 20);
        assert_eq!(
            wgpu::Limits {
                max_bind_groups: floor.max_bind_groups,
                max_vertex_attributes: floor.max_vertex_attributes,
                ..limits
            },
            floor,
            "no other limit is raised over the floor",
        );
    }
}
