//! The GPU bake of a world-space SH-L1 probe volume (`shaders/gi_bake.wgsl`) — sky visibility, and
//! the one-bounce radiance the same march brings back.
//!
//! One compute dispatch traces a fixed ray set from every probe through the occupancy grid
//! `voxel_bake.rs` uploads, and writes each probe's visibility as four floats — the SH DC term in
//! `.x`, the L1 vector in `.yzw`. A ray that leaves the grid contributes 1, a ray that meets an
//! occupied cell contributes 0. No light source enters *that* term; its product is a *multiplier*
//! for the ambient term, which is what keeps a zero-strength scene bit-preserving. How many rays is
//! the machine's own [`QualityParams::gi_rays`](crate::QualityParams::gi_rays), carried in
//! [`BakeInput::rays`] and uploaded to the shader — a different count re-derives the whole set
//! rather than truncating it, so it joins the cache key and rides out to the injection on
//! [`ResolvedGi::rays`].
//!
//! **Bounce.** When the authored `GiVolume.bounce` is non-zero the same walk also gathers the hit
//! cell's outgoing radiance, `albedo × (emissive + direct)`, into three more SH-L1 volumes — one per
//! colour channel, structurally identical to the visibility one. Three `Rgba16Float` textures rather
//! than one packed texture, because hardware trilinear between probes is not optional and a packed
//! axis would blend across coefficient sets. Per-probe cost is 8 B → 32 B. At `bounce == 0.0` the
//! volumes are never allocated, the fallbacks are bound, the shader's radiance work never runs, and
//! both the bake's cost and its visibility output are the visibility-only bake's exactly.
//!
//! [`GiBake`] is `ibl.rs`'s shape verbatim: a `Renderer` field built once in
//! [`crate::render::Renderer::new`], a [`GiBake::resolve`] call per `prepare_frame`, a single-entry
//! cache, and 1×1×1 fallback textures bound on a frame with no `GiVolume` so the flag — not a
//! missing binding — is what turns the feature off.
//!
//! **The light bin.** [`BakeInput::lights`] is the **static subset** — the lights on entities the
//! scene did not mark `GiDynamic` — and `gi_bin.rs` bins it over the grid's world box once per bake,
//! so the shader's per-hit direct term loops the hit cell's own slice instead of every light.
//! Conservative filing makes that byte-identical to the flat loop rather than an approximation of
//! it; the argument lives at `direct_at` in the WGSL, and
//! `a_binned_bake_and_a_degenerate_bin_write_the_same_bytes` is its gate.
//!
//! **The hit cache.** When the bounce half runs, the same march writes down which cell each of
//! [`HIT_RAYS`] strided rays hit and through which face, one packed `u32` per `(probe, k)` (layout at
//! [`HIT_FACE_SHIFT`]). Static geometry makes that answer stable between bakes, so a per-frame
//! dynamic-light pass can replay it instead of marching — and inherits this march's leak control
//! exactly, since it *is* this march's answers. It is cached and invalidated with the volumes it
//! rides beside; it is not history, but a pure function of `(grid, lattice)`.
//!
//! **Cache key.** `(grid.hash, probe_spacing, lattice count, lights_signature, bounce_enabled,
//! rays)`: the grid's own stamped content hash covers every geometric input, the spacing and count
//! are the only other things the lattice depends on, and [`lights_signature`] covers every light
//! the bounce integrates — which is the **static subset only**, so a marked light a system moves
//! never reaches the key. Two deliberate exclusions, both because the quantity is applied at
//! *sample* time and changing it re-reads the same baked volume: `normal_bias`, and `bounce`'s
//! non-zero *value* (only its zero/non-zero discriminant participates, because that is what changes
//! what is written). One more because the bake never reads it: the sun's `light_space`/`bias`,
//! which are camera-fitted and would otherwise re-bake the world on every camera move.
//!
//! The documented consequence, now for unmarked lights alone: **an unmarked light a system moves
//! invalidates the bake every frame**, exactly as an unmarked mesh a system moves invalidates the
//! voxelization. Marking the entity is what makes its motion free.
//!
//! **The hold.** A host can freeze that consequence for a live session
//! ([`Renderer::set_gi_hold`](crate::Renderer::set_gi_hold)): under a hold a key miss with a cache
//! present serves the cached volumes and sets [`GiBake::stale`], rather than dispatching — but only
//! across a *content* miss (grid, lights); a *structural* miss (probe spacing, lattice count,
//! bounce on or off, ray count) always rebakes, since those change what a bake is. It is a host
//! setting, not scene content and not a quality axis, and every one-shot path leaves it off — so a
//! golden, a replay and a `ludion render` bake exactly as they always did.
//!
//! **Format.** `Rgba16Float`, because the consumer needs hardware trilinear filtering between probes
//! and that format is filterable with no device feature requested. Precision is ample for
//! visibility (in `[0, 1]`, L1 magnitude bounded by 0.75) and HDR-capable for bounce radiance.
//!
//! **Determinism.** The ray set is a pure function of the ray index and the count, the light sum runs in the
//! pinned extract order, each probe is one invocation, and there are no atomics and no
//! cross-invocation reduction — but the accumulation is GPU float arithmetic. So this is
//! deterministic *per machine*, exactly where the IBL prefilter sits: off the bit-exact CPU tier,
//! gated by per-OS goldens. It is not a cross-platform bit-identity claim.

use std::cell::RefCell;

use bytemuck::Zeroable;
use ludion_core::codes::code;
use ludion_core::diagnostics::Diagnostic;
use ludion_render_graph::Gpu;

use crate::gi_bin::{BinBuild, build_bin};
use crate::scene::{GiData, MAX_GI_TEXTURE_AXIS, PointLightData, SunData, WorldVoxelGrid};
use crate::shader_gen::{self, GiBakeParams, Light};

/// The probe volume's format — HDR, storage-writable, and filterable without the
/// `float32-filterable` feature (see the module doc).
const PROBE_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;

/// The compute shader's workgroup size: one workgroup bakes a block of probes.
const WORKGROUP: [u32; 3] = shader_gen::GI_BAKE_BAKE_WORKGROUP_SIZE;

/// How many of a bake's rays the hit cache records per probe — the direction budget the per-frame
/// dynamic-light injection replays.
///
/// **A constant, not a quality axis**, and that is the whole distinction between it and
/// [`QualityParams::gi_rays`](crate::QualityParams::gi_rays): this number sizes the cache buffer
/// ([`hit_cache_bytes`]), so a machine's setting must not move it. What the setting moves is the
/// *stride* — slot `k` holds ray `k * (gi_rays / HIT_RAYS)` — which is why a selectable `gi_rays`
/// has to be a whole multiple of this.
///
/// **The shader's `const HIT_RAYS` is the source** (hoisted into `shader_gen`), because the dispatch
/// sizes its own writes from it. Taken **strided** rather than as a prefix: only the ray set's azimuth
/// is golden-angle distributed, while its elevation is a monotone sweep, so a contiguous prefix is a
/// polar cap and a strided subset is not.
pub(crate) const HIT_RAYS: u32 = shader_gen::HIT_RAYS;

/// A packed hit word's face-code bit position — the flat cell index occupies bits 0..24, the face
/// code bits 25..27, bits 28..31 are zero:
///
/// ```text
///  31    28 27 25 24                                    0
/// +--------+-----+--------------------------------------+
/// |  0000  | face|            flat cell index           |
/// +--------+-----+--------------------------------------+
/// ```
///
/// 25 bits hold any cell the `L2103` 32 M-cell budget admits (`2^25` = 33 554 432). Face codes are
/// `0 = -X, 1 = +X, 2 = -Y, 3 = +Y, 4 = -Z, 5 = +Z`, [`HIT_FACE_BOUNDARY`], [`HIT_FACE_ESCAPED`].
///
/// The layout is stated *here*, beside the buffer that carries it, rather than only in WGSL:
/// `gi_inject.wgsl` is the reader, and `the_shader_and_the_cpu_agree_on_the_hit_cache_layout` reads
/// both sources so the three declarations cannot drift apart.
pub(crate) const HIT_FACE_SHIFT: u32 = 25;

/// Face code 6: the ray entered its hit cell by no DDA step, so the surface normal is not an axis
/// and the reader recomputes it as `-ray_direction(k)` — from `k` alone, which is why the word needs
/// no room for it.
pub(crate) const HIT_FACE_BOUNDARY: u32 = 6;

/// Face code 7: the ray escaped the grid. Escape **folds into the face code** rather than taking a
/// sentinel word — the reader is already switching on the code, and a sentinel would be a second
/// thing to test. The whole word is then `HIT_FACE_ESCAPED << HIT_FACE_SHIFT`.
pub(crate) const HIT_FACE_ESCAPED: u32 = 7;

/// Debug labels for the three per-colour-channel bounce volumes, indexed R, G, B.
const BOUNCE_LABELS: [&str; 3] = ["gi bounce r", "gi bounce g", "gi bounce b"];

/// The resolved probe volumes for one frame: the views the mesh pass binds, plus the two vec4s
/// that place them.
pub(crate) struct ResolvedGi {
    /// The baked SH-L1 visibility volume, or the 1×1×1 fallback on a frame with no `GiVolume`.
    pub probes: wgpu::TextureView,
    /// The baked SH-L1 bounce volumes in R, G, B order, or the 1×1×1 fallbacks on a frame that
    /// baked no bounce (no `GiVolume`, or one authored at `bounce: 0.0`).
    pub bounce: [wgpu::TextureView; 3],
    /// `[origin.x, origin.y, origin.z, flag]` — the volume's world-space origin and whether it
    /// holds a real bake (`1.0`) or the fallback (`0.0`).
    pub origin: [f32; 4],
    /// `[1/span.x, 1/span.y, 1/span.z, normal_bias]` — what turns a world position into a `[0, 1]`
    /// sample coordinate, and the sample-time normal offset.
    pub scale: [f32; 4],
    /// The bake's hit cache — one packed `u32` per `(probe, k)` for the first [`HIT_RAYS`] rays
    /// ([`HIT_FACE_SHIFT`] has the layout) — or `None` on a frame that baked no bounce.
    ///
    /// Handed out here because this is where a resolved frame's GI resources live; `gi_inject.rs`'s
    /// per-frame pass is its one reader, and binds it exactly as it was written.
    pub hits: Option<wgpu::Buffer>,
    /// How many rays the bake behind [`hits`](ResolvedGi::hits) actually traced, or `0` on a frame
    /// that baked nothing.
    ///
    /// It travels *with* the cache rather than being read a second time from the machine's
    /// [`QualityParams`](crate::QualityParams), because the injection replays these words and
    /// re-derives each slot's direction by dividing by the count — and the ray set is re-derived at
    /// every count, so dividing by a different one reconstructs directions the march never took. One
    /// value, handed from the writer to the reader.
    pub rays: u32,
}

/// Everything one bake needs, gathered so the absent case is a single `Option` rather than several
/// that could disagree.
pub(crate) struct BakeInput<'a> {
    /// The voxelization the rays march through — its origin, cell size and stamped hash.
    pub grid: &'a WorldVoxelGrid,
    /// The authored knobs and the probe lattice `ludion-engine` sized from `grid`.
    pub gi: &'a GiData,
    /// `voxel_bake.rs`'s per-cell albedo texture for `grid` — the `albedo` of `L_out`.
    pub albedo: &'a wgpu::Texture,
    /// `voxel_bake.rs`'s emissive/occupancy texture for `grid` (`r` = emissive, `g` = occupancy).
    pub occupancy: &'a wgpu::Texture,
    /// The **static** lights — every light whose entity carries no `GiDynamic` marker — in
    /// `ludion-engine`'s ascending-`EntityId` extract order, which is also the bake's float
    /// summation order. [`static_lights`] is the one filter that produces this, and callers apply it
    /// rather than passing `scene.lights`: a marked light is injected per frame, so letting one
    /// reach here would both double-count it and put its per-frame motion in the cache key.
    pub lights: &'a [PointLightData],
    /// The scene's single directional sun, or `None`. Only its direction and premultiplied colour
    /// are read; the shadow map's `light_space`/`bias` are the render pass's business.
    pub sun: Option<&'a SunData>,
    /// Rays traced per probe — the machine's [`QualityParams::gi_rays`](crate::QualityParams), and
    /// the one input here that is a *host* setting rather than scene content.
    ///
    /// It is part of the cache key, because a different count re-derives every direction rather than
    /// truncating the set: two counts' volumes are not comparable, and a bake held over from another
    /// count would be a stale volume, not a coarser one.
    pub rays: u32,
}

/// One finished bake: the visibility volume, plus the bounce trio and the hit cache when the scene
/// asked for bounce.
#[derive(Clone)]
struct Baked {
    probes: wgpu::Texture,
    bounce: Option<[wgpu::Texture; 3]>,
    /// `Some` exactly when `bounce` is — the cache rides the same discriminant, because its only
    /// consumer is the dynamic half of the bounce term.
    hits: Option<wgpu::Buffer>,
    /// The ray count this bake's march traced — what `hits` is only meaningful at.
    rays: u32,
    /// The probe lattice this bake was sized for; `hits` and every volume are exactly this big.
    count: [u32; 3],
}

/// One baked probe volume set, keyed by everything the bake read. The textures rather than views, so
/// a test can copy them back; the sampling views are per-frame handles, which cost nothing beside
/// the buffers and bind groups `prepare_frame` already builds.
struct Cached {
    grid_hash: u64,
    spacing_bits: u32,
    lights_signature: u64,
    bounce_enabled: bool,
    baked: Baked,
}

/// What one blocking readback of the probe volumes yields — the `ludion render --dump-gi` payload's
/// material.
pub struct GiReadback {
    /// Every probe's visibility SH-L1, flat-indexed `x + count.x·(y + count.y·z)`.
    pub visibility: Vec<[f32; 4]>,
    /// Every probe's bounce **DC term** as linear RGB, in the same flat order, or `None` when the
    /// scene baked no bounce. The L1 bands are deliberately not read back: the sheet draws the DC
    /// term, and reading three more volumes to display nothing is cost with no reader.
    pub bounce: Option<Vec<[f32; 3]>>,
    /// Every probe's **injected** DC irradiance as linear RGB, same flat order, or `None` when the
    /// scene schedules no `gi_inject` pass (`gi_inject::scheduled_lattice`'s three content gates —
    /// no `GiVolume`, `bounce: 0.0`, or no `GiDynamic`-marked light; the CLI dumps at `FULL`
    /// quality, so the machine's own switch is never the reason here). Filled by
    /// `Renderer::read_gi_probes`, not by `GiBake::resolve_and_read` — `dynamic` volumes carry no
    /// cache of their own to report (`gi_inject::GiInject` recomputes whole every
    /// dispatch), unlike [`bake_cached`](GiReadback::bake_cached).
    pub dynamic: Option<Vec<[f32; 3]>>,
    /// Whether this readback's **static** bake was a cache hit (`true`, the renderer's single-entry
    /// cache already held it) or a fresh bake (`false`) — the `--dump-gi` header's "is something
    /// thrashing the bake" signal (`Plan/gi-wave3-dynamic-injection.md` §3.1: an *unmarked* light a
    /// system moves invalidates the bake every frame it moves in; a `GiDynamic`-marked one never
    /// reaches the cache key at all). A one-shot CLI dump always starts from an empty renderer, so
    /// this reads `false` on every invocation — still the honest answer, not a vacuous one: it is
    /// what a live game's first frame reports too, and what a hand-run second call over the same
    /// `Renderer` would show flip to `true`.
    pub bake_cached: bool,
}

/// The GI bake: the compute pipeline, the 1×1×1 fallback volumes, and the single-entry cache.
pub(crate) struct GiBake {
    pipeline: wgpu::ComputePipeline,
    /// Bound on a frame with no `GiVolume` — valid to bind, never sampled (the flag is off).
    fallback: wgpu::TextureView,
    /// Bound on a frame that baked no bounce, in both roles at once: the compute pass's storage
    /// targets (nothing writes them — the shader's stores are behind the same flag) and the mesh
    /// pass's sampled bindings. One 1×1×1 texture per channel rather than one shared three ways, so
    /// no bind group ever aliases a writable storage binding.
    bounce_fallback: [wgpu::TextureView; 3],
    /// Bound in the hit cache's slot on a bake that writes none (wgpu rejects a zero-sized storage
    /// binding). One word, never written — the shader's cache stores sit behind the bounce flag.
    hits_fallback: wgpu::Buffer,
    cache: RefCell<Option<Cached>>,
    /// Whether a host is holding the baked baseline: a content miss (grid, lights) with a cache
    /// present then serves the cached volumes and reports staleness instead of dispatching; a
    /// structural miss (spacing, lattice count, bounce, ray count) always rebakes. Session state a
    /// *host* sets ([`Renderer::set_gi_hold`](crate::Renderer::set_gi_hold)), never scene content and
    /// never a [`Quality`](crate::Quality) axis — it changes nothing about what a bake *is*, only
    /// whether one runs, and every one-shot path leaves it `false`.
    hold: bool,
    /// Whether the last [`bake_or_cached`](GiBake::bake_or_cached) served a cached bake for a key it
    /// was not baked under. Recomputed on every call, never latched: a world that returns to the held
    /// baseline matches the key again and is honestly not stale.
    stale: std::cell::Cell<bool>,
    /// **Test-only.** Counts completed dispatches — the proof a cache hit skips the bake entirely
    /// rather than merely reproducing the same volume (`voxel_bake.rs`'s `uploads` precedent).
    #[cfg(test)]
    bakes: std::cell::Cell<u32>,
    /// **Test-only.** Which light bin the next bake builds — the seam the conformance oracle needs
    /// to bake one scene through this exact shader both binned and degenerate. It does not exist
    /// outside `cfg(test)`: the degenerate bin is an oracle, never a shipping mode.
    #[cfg(test)]
    bin_build: std::cell::Cell<BinBuild>,
}

impl GiBake {
    /// Builds the compute pipeline and the fallback volume.
    pub(crate) fn new(gpu: &Gpu) -> GiBake {
        let device = gpu.device();
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("gi bake"),
            source: wgpu::ShaderSource::Wgsl(std::borrow::Cow::Borrowed(include_str!(
                "shaders/gi_bake.wgsl"
            ))),
        });
        // The auto-derived layout is what makes the occupancy binding legal: wgpu infers a sampled
        // float texture's `filterable` from whether the entry point pairs it with a sampler, and the
        // trace only ever `textureLoad`s — so the non-filterable `Rg32Float` the voxel upload uses
        // satisfies the layout with no device feature and no hand-written BGL. Pairing that texture
        // with a sampler in the shader would break this, not merely change it.
        let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("gi bake"),
            layout: None,
            module: &module,
            entry_point: Some("bake"),
            compilation_options: wgpu::PipelineCompilationOptions::default(),
            cache: None,
        });

        GiBake {
            pipeline,
            fallback: fallback_volume(device, "gi fallback volume"),
            bounce_fallback: std::array::from_fn(|c| fallback_volume(device, BOUNCE_LABELS[c])),
            hits_fallback: device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("gi hit cache fallback"),
                size: std::mem::size_of::<u32>() as u64,
                usage: wgpu::BufferUsages::STORAGE,
                mapped_at_creation: false,
            }),
            cache: RefCell::new(None),
            hold: false,
            stale: std::cell::Cell::new(false),
            #[cfg(test)]
            bakes: std::cell::Cell::new(0),
            #[cfg(test)]
            bin_build: std::cell::Cell::new(BinBuild::Culled),
        }
    }

    /// Resolves the probe volumes for this frame. `None` (no `GiVolume`, or a scene with no geometry
    /// to place probes around) drops the cached volumes and returns the fallbacks with a zero flag.
    /// `Some` bakes on a cache miss and returns the volumes with the flag on.
    pub(crate) fn resolve(&self, gpu: &Gpu, input: Option<BakeInput<'_>>) -> ResolvedGi {
        let Some(input) = input else {
            *self.cache.borrow_mut() = None;
            // Nothing baked is nothing held back: a frame with no `GiVolume` reports no staleness
            // whatever the lever says, so the flag never survives GI being switched off.
            self.stale.set(false);
            return ResolvedGi {
                probes: self.fallback.clone(),
                bounce: self.bounce_fallback.clone(),
                origin: [0.0; 4],
                scale: [0.0; 4],
                hits: None,
                rays: 0,
            };
        };

        let (baked, _) = self.bake_or_cached(gpu, &input);
        ResolvedGi {
            probes: probe_view(&baked.probes, "gi probe view"),
            bounce: match &baked.bounce {
                Some(volumes) => std::array::from_fn(|c| probe_view(&volumes[c], BOUNCE_LABELS[c])),
                None => self.bounce_fallback.clone(),
            },
            origin: [input.gi.origin.x, input.gi.origin.y, input.gi.origin.z, 1.0],
            scale: [
                inverse_span(input.gi.span.x),
                inverse_span(input.gi.span.y),
                inverse_span(input.gi.span.z),
                input.gi.normal_bias,
            ],
            hits: baked.hits.clone(),
            // What the served bake traced, not what was asked for: a held bake is only ever
            // served across a content miss, so the two agree, but the bake is the authority.
            rays: baked.rays,
        }
    }

    /// [`resolve`](GiBake::resolve)'s cache-hit-or-bake core, extracted so a debug readback
    /// (`ludion render --dump-gi`) can get the baked **textures** themselves rather than just
    /// sampling views — [`resolve`] never hands a texture out, only views built from them.
    ///
    /// Returns whether this call was a cache **hit** (`true`) or paid for a fresh bake (`false`) —
    /// [`resolve`](GiBake::resolve)'s real-frame path discards it (a frame does not narrate its own
    /// cache), but [`resolve_and_read`](GiBake::resolve_and_read) carries it into
    /// [`GiReadback::bake_cached`], the `--dump-gi` header's "is something thrashing the bake"
    /// signal (`Plan/gi-wave3-dynamic-injection.md` §3.1, §10).
    fn bake_or_cached(&self, gpu: &Gpu, input: &BakeInput<'_>) -> (Baked, bool) {
        let (grid_hash, spacing_bits) = (input.grid.hash, input.gi.probe_spacing.to_bits());
        let signature = lights_signature(input.lights, input.sun);
        let bounce_enabled = input.gi.bounce > 0.0;
        // Structural inputs change what a bake *is* (its lattice, its ray set, whether it carries
        // bounce and a hit cache), so no hold may serve across them: `gi_inject` schedules from the
        // live bounce and count and needs a hit cache sized for that lattice. Content inputs (the
        // grid, the lights) only change what the volumes hold, so a held bake can stand in.
        let structural_match = |c: &Cached| {
            c.spacing_bits == spacing_bits
                && c.bounce_enabled == bounce_enabled
                && c.baked.rays == input.rays
                && c.baked.count == input.gi.count
        };
        let hit = self
            .cache
            .borrow()
            .as_ref()
            .filter(|c| {
                structural_match(c) && c.grid_hash == grid_hash && c.lights_signature == signature
            })
            .map(|c| c.baked.clone());
        match hit {
            Some(baked) => {
                self.stale.set(false);
                (baked, true)
            }
            None => {
                // Under a hold, a *content* miss with a cache present serves the volumes that cache
                // holds and reports staleness rather than paying the dispatch — the whole point of
                // the lever, since a live session's every unmarked mover writes a new key. A
                // structural miss always rebakes, and so does the first-ever bake: there is nothing
                // to serve, and an unlit frame is a worse answer than a stale one.
                if self.hold
                    && let Some(cached) = self.cache.borrow().as_ref()
                    && structural_match(cached)
                {
                    self.stale.set(true);
                    return (cached.baked.clone(), true);
                }
                self.stale.set(false);
                let baked = self.bake(gpu, input, bounce_enabled);
                *self.cache.borrow_mut() = Some(Cached {
                    grid_hash,
                    spacing_bits,
                    lights_signature: signature,
                    bounce_enabled,
                    baked: baked.clone(),
                });
                (baked, false)
            }
        }
    }

    /// **Debug readback** (`ludion render --dump-gi`): bakes (or reuses the cache for) `input`'s
    /// probe volumes exactly as a real frame's [`resolve`](GiBake::resolve) would, then reads them
    /// back to the CPU — flat-indexed `x + count.x·(y + count.y·z)`, [`WorldVoxelGrid`]'s own flat
    /// order. Blocking: a debug dump has no non-stalling path to be on.
    ///
    /// [`GiReadback::dynamic`] is left `None` here — the dynamic term is a different pass with a
    /// different (non-)caching story (`gi_inject.rs`'s own readback), and this module has no
    /// `GiInject` to run it with. `Renderer::read_gi_probes` is what fills it in, over exactly this
    /// `GiReadback`.
    ///
    /// # Errors
    /// `L2005` if a staging buffer cannot be mapped.
    pub(crate) fn resolve_and_read(
        &self,
        gpu: &Gpu,
        input: &BakeInput<'_>,
    ) -> Result<GiReadback, Diagnostic> {
        let (baked, bake_cached) = self.bake_or_cached(gpu, input);
        let visibility = read_probe_texture(gpu, &baked.probes)?;
        let bounce = match &baked.bounce {
            None => None,
            Some(volumes) => {
                // The DC term of each channel is that volume's `.x`; the L1 bands stay on the GPU.
                let mut channels = Vec::with_capacity(3);
                for volume in volumes {
                    channels.push(read_probe_texture(gpu, volume)?);
                }
                Some(
                    (0..visibility.len())
                        .map(|i| std::array::from_fn(|c| channels[c][i][0]))
                        .collect(),
                )
            }
        };
        Ok(GiReadback {
            visibility,
            bounce,
            dynamic: None,
            bake_cached,
        })
    }

    /// Runs the bake for `input`, returning the finished probe volumes. One encoder, one dispatch,
    /// one submit — the bounce trio rides the same dispatch, because it comes off the same march.
    ///
    /// The lattice's size is an invariant, not a runtime branch: `ludion-engine`'s `probe_lattice`
    /// caps every axis at [`MAX_GI_TEXTURE_AXIS`] and reports `L2105` before the renderer is handed
    /// a count, so an oversized volume cannot reach here. Skipping the bake instead would leave a
    /// scene that authored and validated a `GiVolume` silently unlit.
    fn bake(&self, gpu: &Gpu, input: &BakeInput<'_>, bounce_enabled: bool) -> Baked {
        #[cfg(test)]
        self.bakes.set(self.bakes.get() + 1);

        let device = gpu.device();
        let [cx, cy, cz] = input.gi.count;
        // The stride the shader derives is `rays / HIT_RAYS`; a fractional one would file the cached
        // slots into a polar cap. `ludion-engine`'s `L0016` refuses such a count before a host can
        // set it, and every shipped preset satisfies it (`every_presets_gi_axes_satisfy_their_own_
        // constraints`), so this is the invariant restated where the dispatch depends on it.
        debug_assert!(
            input.rays >= HIT_RAYS && input.rays.is_multiple_of(HIT_RAYS),
            "gi_rays {} must be a positive whole multiple of the {HIT_RAYS}-slot hit cache",
            input.rays,
        );
        debug_assert!(
            cx.max(cy).max(cz) <= MAX_GI_TEXTURE_AXIS,
            "probe lattice {cx}x{cy}x{cz} exceeds the {MAX_GI_TEXTURE_AXIS}-texel GI axis cap that \
             ludion-engine's `probe_lattice` enforces as L2105",
        );
        let size = wgpu::Extent3d {
            width: cx,
            height: cy,
            depth_or_array_layers: cz,
        };
        let texture = probe_volume(device, "gi probe volume", size);
        let bounce = bounce_enabled
            .then(|| std::array::from_fn(|c| probe_volume(device, BOUNCE_LABELS[c], size)));

        let storage_view = probe_view(&texture, "gi probe storage");
        let bounce_views: [wgpu::TextureView; 3] = match &bounce {
            Some(volumes) => std::array::from_fn(|c| probe_view(&volumes[c], BOUNCE_LABELS[c])),
            None => self.bounce_fallback.clone(),
        };

        // The light array the shader sums, premultiplied into `mesh.wgsl`'s own `Light` — the same
        // convention, so the bounce is consistent with the light the player sees.
        let mut lights: Vec<Light> = input.lights.iter().map(premultiplied).collect();
        let light_count = lights.len() as u32;

        // The world-space bin over exactly that array, built before the padding below so a bin index
        // can only ever address a real light.
        #[cfg(not(test))]
        let bin_build = BinBuild::Culled;
        // Under `cfg(test)` only, the conformance oracle can point this at the degenerate bin and
        // bake the same scene both ways through this same shader. A shipping build has no such
        // switch: the line above is then the only one that exists.
        #[cfg(test)]
        let bin_build = self.bin_build.get();
        let bin = build_bin(input.grid, &lights, bin_build);

        // wgpu rejects a zero-sized buffer binding, and naga sizes a runtime array as exactly one
        // element; an empty scene uploads a single zeroed light the shader never reaches, because
        // every bin cell's count is 0.
        if lights.is_empty() {
            lights.push(Light::zeroed());
        }
        let mut bin_indices = bin.indices;
        if bin_indices.is_empty() {
            bin_indices.push(0);
        }

        let (sun_dir, sun_color) = match input.sun {
            Some(sun) => (
                [sun.direction.x, sun.direction.y, sun.direction.z, 1.0],
                [sun.color.x, sun.color.y, sun.color.z, 0.0],
            ),
            None => ([0.0; 4], [0.0; 4]),
        };
        let params = GiBakeParams {
            grid: [
                input.grid.origin.x,
                input.grid.origin.y,
                input.grid.origin.z,
                input.grid.cell_size,
            ],
            probe: [
                input.gi.origin.x,
                input.gi.origin.y,
                input.gi.origin.z,
                input.gi.probe_spacing,
            ],
            counts: [cx, cy, cz, light_count],
            flags: [u32::from(bounce_enabled), input.rays, 0, 0],
            sun_dir,
            sun_color,
            bin: bin.params,
        };
        let (params_buf, lights_buf, bin_table_buf, bin_indices_buf) = {
            use wgpu::util::DeviceExt;
            let storage = |label: &str, contents: &[u8]| {
                device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some(label),
                    contents,
                    usage: wgpu::BufferUsages::STORAGE,
                })
            };
            (
                device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("gi bake params"),
                    contents: bytemuck::bytes_of(&params),
                    usage: wgpu::BufferUsages::UNIFORM,
                }),
                storage("gi bake lights", bytemuck::cast_slice(&lights)),
                storage("gi bake bin table", bytemuck::cast_slice(&bin.table)),
                storage("gi bake bin indices", bytemuck::cast_slice(&bin_indices)),
            )
        };

        // The hit cache: one word per (probe, ray-prefix entry), allocated only when the bounce half
        // runs and written whole by the dispatch below. `COPY_SRC` so a test can read it back.
        let hits = bounce_enabled.then(|| {
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("gi hit cache"),
                size: hit_cache_bytes(input.gi.count),
                usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
                mapped_at_creation: false,
            })
        });
        let voxel_view = |texture: &wgpu::Texture, label: &str| {
            texture.create_view(&wgpu::TextureViewDescriptor {
                label: Some(label),
                dimension: Some(wgpu::TextureViewDimension::D3),
                ..Default::default()
            })
        };
        let occupancy_view = voxel_view(input.occupancy, "gi bake occupancy");
        let albedo_view = voxel_view(input.albedo, "gi bake albedo");

        let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("gi bake"),
            layout: &self.pipeline.get_bind_group_layout(0),
            entries: &[
                wgpu::BindGroupEntry {
                    binding: shader_gen::BAKE_PARAMS_BINDING,
                    resource: params_buf.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: shader_gen::OCCUPANCY_BINDING,
                    resource: wgpu::BindingResource::TextureView(&occupancy_view),
                },
                wgpu::BindGroupEntry {
                    binding: shader_gen::PROBES_OUT_BINDING,
                    resource: wgpu::BindingResource::TextureView(&storage_view),
                },
                wgpu::BindGroupEntry {
                    binding: shader_gen::BAKE_ALBEDO_BINDING,
                    resource: wgpu::BindingResource::TextureView(&albedo_view),
                },
                wgpu::BindGroupEntry {
                    binding: shader_gen::BAKE_LIGHT_LIST_BINDING,
                    resource: lights_buf.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: shader_gen::BOUNCE_R_OUT_BINDING,
                    resource: wgpu::BindingResource::TextureView(&bounce_views[0]),
                },
                wgpu::BindGroupEntry {
                    binding: shader_gen::BOUNCE_G_OUT_BINDING,
                    resource: wgpu::BindingResource::TextureView(&bounce_views[1]),
                },
                wgpu::BindGroupEntry {
                    binding: shader_gen::BOUNCE_B_OUT_BINDING,
                    resource: wgpu::BindingResource::TextureView(&bounce_views[2]),
                },
                wgpu::BindGroupEntry {
                    binding: shader_gen::BAKE_BIN_TABLE_BINDING,
                    resource: bin_table_buf.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: shader_gen::BAKE_BIN_INDICES_BINDING,
                    resource: bin_indices_buf.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: shader_gen::BAKE_HIT_CACHE_BINDING,
                    resource: hits
                        .as_ref()
                        .unwrap_or(&self.hits_fallback)
                        .as_entire_binding(),
                },
            ],
        });

        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("gi bake"),
        });
        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("gi bake"),
                timestamp_writes: None,
            });
            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(0, &bind, &[]);
            pass.dispatch_workgroups(
                cx.div_ceil(WORKGROUP[0]),
                cy.div_ceil(WORKGROUP[1]),
                cz.div_ceil(WORKGROUP[2]),
            );
        }
        gpu.queue().submit(Some(encoder.finish()));

        Baked {
            probes: texture,
            bounce,
            hits,
            rays: input.rays,
            count: input.gi.count,
        }
    }

    /// Holds (or releases) the baked baseline — see [`hold`](GiBake::hold).
    pub(crate) fn set_hold(&mut self, hold: bool) {
        self.hold = hold;
    }

    /// Whether the last resolve served a held bake for a key it was not baked under.
    pub(crate) fn stale(&self) -> bool {
        self.stale.get()
    }

    /// **Test-only.** How many dispatches have actually run.
    #[cfg(test)]
    pub(crate) fn bake_count(&self) -> u32 {
        self.bakes.get()
    }

    /// **Test-only.** The cached visibility volume, or `None` with nothing baked — the seam the
    /// readback assertions copy from.
    #[cfg(test)]
    pub(crate) fn cached_probes(&self) -> Option<wgpu::Texture> {
        self.cache.borrow().as_ref().map(|c| c.baked.probes.clone())
    }

    /// **Test-only.** The cached bounce volumes in R, G, B order, or `None` when the last bake
    /// skipped them.
    #[cfg(test)]
    pub(crate) fn cached_bounce(&self) -> Option<[wgpu::Texture; 3]> {
        self.cache
            .borrow()
            .as_ref()
            .and_then(|c| c.baked.bounce.clone())
    }

    /// **Test-only.** The cached hit cache, or `None` when the last bake skipped the bounce half.
    #[cfg(test)]
    pub(crate) fn cached_hits(&self) -> Option<wgpu::Buffer> {
        self.cache
            .borrow()
            .as_ref()
            .and_then(|c| c.baked.hits.clone())
    }

    /// **Test-only.** Points the next bake at a different light bin — the oracle's seam.
    #[cfg(test)]
    pub(crate) fn set_bin_build(&self, build: BinBuild) {
        self.bin_build.set(build);
    }
}

/// One [`PointLightData`] in `mesh.wgsl`'s premultiplied `Light` convention: `color * intensity` is
/// the diffuse response at `ndl = 1`, and the range rides in `position.w`.
///
/// Shared with `gi_inject.rs`, which premultiplies the *dynamic* subset the same way: the two halves
/// of one bounce term must agree on what a light is down to the bit, and a second copy of this
/// three-line conversion is exactly how they would stop agreeing.
pub(crate) fn premultiplied(light: &PointLightData) -> Light {
    let c = light.color * light.intensity;
    Light {
        position: [
            light.position.x,
            light.position.y,
            light.position.z,
            light.range,
        ],
        color: [c.x, c.y, c.z, 0.0],
    }
}

/// The bake's `O(lights)` cache-key term: an XXH3 over the sun's presence, direction and colour,
/// then every light's premultiplied `Light` bytes in the pinned extract order.
///
/// **The premultiplied form, not the authored one**, is what is hashed, because it is exactly what
/// the bake reads: two lights whose `color`/`intensity` differ but whose product is equal produce
/// the same volume, and the key says so rather than re-baking. Symmetrically, the sun's
/// `light_space` and `bias` are *excluded* — they are camera-fitted and change every frame the
/// camera moves, and the bake never reads either.
fn lights_signature(lights: &[PointLightData], sun: Option<&SunData>) -> u64 {
    let mut bytes = Vec::with_capacity(1 + 24 + lights.len() * std::mem::size_of::<Light>());
    match sun {
        None => bytes.push(0u8),
        Some(sun) => {
            bytes.push(1u8);
            for v in [
                sun.direction.x,
                sun.direction.y,
                sun.direction.z,
                sun.color.x,
                sun.color.y,
                sun.color.z,
            ] {
                bytes.extend_from_slice(&v.to_le_bytes());
            }
        }
    }
    for light in lights {
        bytes.extend_from_slice(bytemuck::bytes_of(&premultiplied(light)));
    }
    ludion_assets::content_hash(&bytes)
}

/// A probe-lattice-sized 3D volume in [`PROBE_FORMAT`] — storage-writable by the bake, sampled by
/// the mesh pass, copyable for the readback path.
fn probe_volume(device: &wgpu::Device, label: &str, size: wgpu::Extent3d) -> wgpu::Texture {
    device.create_texture(&wgpu::TextureDescriptor {
        label: Some(label),
        size,
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D3,
        format: PROBE_FORMAT,
        usage: wgpu::TextureUsages::STORAGE_BINDING
            | wgpu::TextureUsages::TEXTURE_BINDING
            | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    })
}

/// A whole-texture `D3` view of a probe volume, used for both the storage binding the bake writes
/// through and the sampled binding the mesh pass reads.
fn probe_view(texture: &wgpu::Texture, label: &str) -> wgpu::TextureView {
    texture.create_view(&wgpu::TextureViewDescriptor {
        label: Some(label),
        dimension: Some(wgpu::TextureViewDimension::D3),
        ..Default::default()
    })
}

/// `1 / span`, with a zero span (an axis whose lattice is a single probe wide — impossible while the
/// count floor holds, but the reciprocal must stay finite regardless) mapping to `0`, so a sample
/// coordinate on that axis collapses to the one probe instead of becoming an infinity.
fn inverse_span(span: f32) -> f32 {
    if span > 0.0 { 1.0 / span } else { 0.0 }
}

/// How many bytes the hit cache takes for a `count`-shaped probe lattice: one `u32` per
/// `(probe, k)`, `k < HIT_RAYS`.
///
/// The numbers this bounds, stated where the allocation is: a district lattice of ~460 k probes
/// costs ~30 MB, and the `MAX_GI_PROBES` cap of 4 M probes would cost 256 MB — a number to state,
/// not to reach (the same guardrail's doc comment carries this line).
fn hit_cache_bytes(count: [u32; 3]) -> u64 {
    u64::from(count[0])
        * u64::from(count[1])
        * u64::from(count[2])
        * u64::from(HIT_RAYS)
        * std::mem::size_of::<u32>() as u64
}

/// The probe count a lattice of `count` probes falls to when its spacing is multiplied by `scale` —
/// the arithmetic behind [`QualityParams::gi_probe_scale`](crate::QualityParams::gi_probe_scale).
///
/// The lattice covers a world span of `(count - 1) · spacing` per axis, so re-laying it at
/// `spacing · scale` needs `ceil((count - 1) · spacing / (spacing · scale)) + 1` probes — **the
/// spacing cancels**, which is what lets [`PostChain::masked`](crate::PostChain::masked) compute the
/// scaled extent from the count alone, with no scene in reach. `scale` at `1.0` is the exact
/// identity (`ceil(n / 1) == n` for every count a lattice can hold), which is what keeps `full` a
/// byte-for-byte unchanged bake.
///
/// Scaling only ever *removes* probes, so the `L2103`/`L2105` budgets `ludion-engine` enforced on
/// the authored spacing cannot be breached from here — and the floor of two probes per axis, which
/// trilinear interpolation needs, survives because the `+ 1` sits outside the `ceil` of a positive.
pub(crate) fn scaled_probe_count(count: [u32; 3], scale: f32) -> [u32; 3] {
    std::array::from_fn(|axis| ((count[axis] - 1) as f32 / scale).ceil() as u32 + 1)
}

/// `gi` re-laid at `scale` times its authored probe spacing: the same volume, coarser.
///
/// Every field the lattice implies moves together — the spacing the bake places probes at, the count
/// it dispatches over, and the span the mesh pass samples through — so a caller cannot scale one and
/// forget another. The authored strengths (`sky_visibility`, `bounce`, `normal_bias`) are the look
/// and are untouched: a machine's setting spends less, it does not re-author.
pub(crate) fn scaled_lattice(gi: &GiData, scale: f32) -> GiData {
    let count = scaled_probe_count(gi.count, scale);
    let probe_spacing = gi.probe_spacing * scale;
    GiData {
        probe_spacing,
        count,
        // `ludion-engine`'s `probe_lattice` arithmetic, verbatim, so a scale of 1.0 reproduces the
        // span it computed rather than merely approximating it.
        span: ludion_core::math::glam::Vec3::new(
            (count[0] - 1) as f32 * probe_spacing,
            (count[1] - 1) as f32 * probe_spacing,
            (count[2] - 1) as f32 * probe_spacing,
        ),
        ..*gi
    }
}

/// The static light subset a bake is entitled to see: every light whose entity carries no
/// `GiDynamic` marker, in the extract order it arrived in.
///
/// **This filter is the bake-thrash fix**, and it is why it is a named function rather than an
/// inline `filter` at each call site: a marked light is injected per frame, so it must reach neither
/// the volumes (it would be counted twice) nor [`lights_signature`] (its per-frame motion would
/// invalidate the bake on every frame — precisely the cost the marker exists to remove). Callers
/// building a [`BakeInput`] apply it; nothing downstream re-checks the tag.
pub(crate) fn static_lights(lights: &[PointLightData]) -> Vec<PointLightData> {
    lights
        .iter()
        .filter(|light| !light.dynamic)
        .copied()
        .collect()
}

/// A 1×1×1 volume bound on frames that baked nothing — valid to bind, never sampled (the flag is
/// off) and never written (the shader's bounce stores sit behind the same flag). `STORAGE_BINDING`
/// rides along because the bounce fallbacks fill the compute pass's write slots on a
/// visibility-only bake, which is the one place a bind group needs a texture in both roles.
fn fallback_volume(device: &wgpu::Device, label: &str) -> wgpu::TextureView {
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some(label),
        size: wgpu::Extent3d {
            width: 1,
            height: 1,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D3,
        format: PROBE_FORMAT,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::STORAGE_BINDING,
        view_formats: &[],
    });
    texture.create_view(&wgpu::TextureViewDescriptor {
        label: Some(label),
        dimension: Some(wgpu::TextureViewDimension::D3),
        ..Default::default()
    })
}

/// The blocking readback path for a baked probe volume: copies `texture` to a staging buffer,
/// `poll(Wait)`, and decodes its `Rgba16Float` texels to `f32`, flat-indexed `x + w·(y + h·z)`
/// (`WorldVoxelGrid`'s own flat order). Mirrors `ludion-render-graph`'s blocking pixel readback, but a
/// probe volume is a 3D f16 texture rather than a 2D RGBA8 render target, so it cannot reuse that
/// path's row layout.
///
/// `pub(crate)`: the dynamic volume `gi_inject.rs` writes is the same format and layout (§4.1's
/// "unify the storage" taken literally, one texel wider than it needs — the reserved `.a` — rather
/// than a second decode path), so its own debug readback reuses this rather than re-deriving it.
///
/// # Errors
/// `L2005` if the staging buffer cannot be mapped.
pub(crate) fn read_probe_texture(
    gpu: &Gpu,
    texture: &wgpu::Texture,
) -> Result<Vec<[f32; 4]>, Diagnostic> {
    let size = texture.size();
    let (w, h, d) = (size.width, size.height, size.depth_or_array_layers);
    let bytes_per_row = (w * 8).div_ceil(256) * 256;

    let device = gpu.device();
    let buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("gi probe readback"),
        size: (bytes_per_row * h * d) as u64,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("gi probe readback"),
    });
    encoder.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::TexelCopyBufferInfo {
            buffer: &buffer,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(bytes_per_row),
                rows_per_image: Some(h),
            },
        },
        size,
    );
    gpu.queue().submit(Some(encoder.finish()));

    let (tx, rx) = std::sync::mpsc::channel();
    buffer.slice(..).map_async(wgpu::MapMode::Read, move |r| {
        let _ = tx.send(r);
    });
    device
        .poll(wgpu::PollType::wait_indefinitely())
        .map_err(|error| map_failure(format!("device poll failed: {error}")))?;
    match rx.recv() {
        Ok(Ok(())) => {}
        other => {
            return Err(map_failure(format!(
                "buffer map did not complete: {other:?}"
            )));
        }
    }

    let out = {
        let mapped = buffer
            .slice(..)
            .get_mapped_range()
            .map_err(|error| map_failure(format!("could not access mapped bytes: {error}")))?;
        let mut out = Vec::with_capacity((w * h * d) as usize);
        for z in 0..d {
            for y in 0..h {
                for x in 0..w {
                    let base = ((z * h + y) * bytes_per_row + x * 8) as usize;
                    out.push(std::array::from_fn(|c| {
                        let at = base + c * 2;
                        f16_to_f32(u16::from_le_bytes([mapped[at], mapped[at + 1]]))
                    }));
                }
            }
        }
        out
    };
    buffer.unmap();
    Ok(out)
}

fn map_failure(detail: String) -> Diagnostic {
    Diagnostic::new(
        code::RENDER_READBACK_FAILED,
        format!("could not read the baked GI probe volume back from the GPU: {detail}"),
    )
    .with_help(
        "this usually indicates a lost device or an out-of-memory condition; retry, and check GPU \
         driver stability",
    )
}

/// One IEEE-754 binary16 texel channel as an `f32` — the probe volume's storage format, decoded here
/// so a reader gets world quantities rather than bit patterns. `f32::powi` (not
/// `ludion_core::math::powf`): an integer power is exact repeated squaring, not a platform
/// transcendental, so the determinism rule it exists for does not apply.
fn f16_to_f32(bits: u16) -> f32 {
    let sign = if bits & 0x8000 != 0 { -1.0 } else { 1.0 };
    let exponent = ((bits >> 10) & 0x1f) as i32;
    let mantissa = (bits & 0x3ff) as f32;
    match exponent {
        0 => sign * mantissa * 2.0f32.powi(-24),
        31 => {
            sign * if mantissa == 0.0 {
                f32::INFINITY
            } else {
                f32::NAN
            }
        }
        _ => sign * (1.0 + mantissa / 1024.0) * 2.0f32.powi(exponent - 15),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gi_bin::{BIN_CELLS, bin_cell_of, build_bin};
    use crate::quality::QualityParams;
    use crate::voxel_bake::VoxelBake;
    use ludion_core::math::glam::{Mat4, Vec3};

    /// The ray count every fixture below bakes at, named rather than defaulted.
    ///
    /// **It is what makes the exact-value assertions in this module legitimate.** The ray set is
    /// re-derived at every count rather than truncated, so a probe's `33/64`, its `1.0` and its
    /// `[0.0; 4]` are statements about *this* count and no other — every one of them is a full-quality
    /// test, and saying so at the fixture is what keeps that from being an accident of whichever
    /// value happened to be the default.
    const FULL_RAYS: u32 = QualityParams::FULL.gi_rays;

    fn skip_or_gpu(name: &str) -> Option<Gpu> {
        match Gpu::request() {
            Ok(gpu) => Some(gpu),
            Err(d) if ludion_core::require_gpu::is_no_adapter(std::slice::from_ref(&d)) => {
                eprintln!("SKIPPED {name}: no GPU adapter");
                None
            }
            Err(d) => panic!("gpu request failed: {d:?}"),
        }
    }

    /// The fixture world: a `16 × 12 × 16` unit-cell grid whose bottom `floor` cell rows are solid
    /// across the whole footprint and whose remainder is open to the sky. Six rows is deliberately
    /// deeper than the push-out's reach, so a probe in the middle of the slab has no free cell to be
    /// pushed to and must report darkness.
    fn slab_grid(floor: u32) -> WorldVoxelGrid {
        let extent = [16u32, 12, 16];
        let total = (extent[0] * extent[1] * extent[2]) as usize;
        let mut occupancy = vec![false; total];
        for z in 0..extent[2] {
            for y in 0..floor {
                for x in 0..extent[0] {
                    let flat = x + extent[0] * (y + extent[1] * z);
                    occupancy[flat as usize] = true;
                }
            }
        }
        WorldVoxelGrid {
            origin: Vec3::ZERO,
            cell_size: 1.0,
            extent,
            occupancy,
            albedo: vec![[1.0, 1.0, 1.0, 1.0]; total],
            emissive: vec![0.0; total],
            hash: 0,
        }
        .sealed()
    }

    /// A closed emissive box: an `8³` unit-cell grid whose outer one-cell skin is solid with albedo
    /// `1.0` and emissive `radiance`, and whose interior is free. Every ray from an interior probe
    /// meets that skin and none escapes, which is what lets a test state the expected reconstruction
    /// as *the emitter's own radiance* rather than as a recorded number.
    fn shell_grid(radiance: f32) -> WorldVoxelGrid {
        let extent = [8u32, 8, 8];
        let total = (extent[0] * extent[1] * extent[2]) as usize;
        let mut occupancy = vec![false; total];
        for z in 0..extent[2] {
            for y in 0..extent[1] {
                for x in 0..extent[0] {
                    let skin = x == 0
                        || y == 0
                        || z == 0
                        || x == extent[0] - 1
                        || y == extent[1] - 1
                        || z == extent[2] - 1;
                    let flat = x + extent[0] * (y + extent[1] * z);
                    occupancy[flat as usize] = skin;
                }
            }
        }
        WorldVoxelGrid {
            origin: Vec3::ZERO,
            cell_size: 1.0,
            extent,
            occupancy,
            albedo: vec![[1.0, 1.0, 1.0, 1.0]; total],
            emissive: vec![radiance; total],
            hash: 0,
        }
        .sealed()
    }

    /// The lattice `ludion-engine` would size over `grid` at `probe_spacing`, computed here so the
    /// renderer's tests need no engine dependency (the arithmetic itself is pinned by the
    /// voxelizer's own tests).
    fn gi_data(grid: &WorldVoxelGrid, probe_spacing: f32) -> GiData {
        let count = std::array::from_fn(|axis| {
            let span = grid.extent[axis] as f32 * grid.cell_size;
            (span / probe_spacing).ceil().max(1.0) as u32 + 1
        });
        GiData {
            probe_spacing,
            normal_bias: 0.25,
            sky_visibility: 1.0,
            bounce: 0.0,
            count,
            origin: grid.origin,
            span: Vec3::new(
                (count[0] - 1) as f32 * probe_spacing,
                (count[1] - 1) as f32 * probe_spacing,
                (count[2] - 1) as f32 * probe_spacing,
            ),
        }
    }

    /// **Test-only.** Copies a baked probe volume back to the CPU as one `[f32; 4]` per probe, flat
    /// indexed `x + count.x·(y + count.y·z)` — the same flat order [`WorldVoxelGrid`] uses.
    ///
    /// Blocking, and unpads the 256-byte row alignment `copy_texture_to_buffer` requires (unlike the
    /// `write_texture` upload path, which needs none). It lives under `cfg(test)`: reading a probe
    /// volume back is how the bake is *asserted*, never how it is used.
    fn read_probes(gpu: &Gpu, texture: &wgpu::Texture) -> Vec<[f32; 4]> {
        let size = texture.size();
        let (w, h, d) = (size.width, size.height, size.depth_or_array_layers);
        let bytes_per_row = (w * 8).div_ceil(256) * 256;

        let device = gpu.device();
        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("gi probe readback"),
            size: (bytes_per_row * h * d) as u64,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("gi probe readback"),
        });
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &buffer,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(bytes_per_row),
                    rows_per_image: Some(h),
                },
            },
            size,
        );
        gpu.queue().submit(Some(encoder.finish()));

        let (tx, rx) = std::sync::mpsc::channel();
        buffer.slice(..).map_async(wgpu::MapMode::Read, move |r| {
            let _ = tx.send(r);
        });
        device
            .poll(wgpu::PollType::wait_indefinitely())
            .expect("device poll succeeds");
        rx.recv().expect("map completes").expect("map succeeds");

        let mapped = buffer.slice(..).get_mapped_range().expect("mapped range");
        let mut out = Vec::with_capacity((w * h * d) as usize);
        for z in 0..d {
            for y in 0..h {
                for x in 0..w {
                    let base = ((z * h + y) * bytes_per_row + x * 8) as usize;
                    out.push(std::array::from_fn(|c| {
                        let at = base + c * 2;
                        f16_to_f32(u16::from_le_bytes([mapped[at], mapped[at + 1]]))
                    }));
                }
            }
        }
        drop(mapped);
        buffer.unmap();
        out
    }

    /// A point light in the shape `extract` hands the renderer. `dynamic: false` throughout — this
    /// module bakes, and the bake's whole subject is the static (unmarked) light set.
    fn point_light(position: Vec3, color: Vec3, intensity: f32, range: f32) -> PointLightData {
        PointLightData {
            position,
            color,
            intensity,
            range,
            dynamic: false,
        }
    }

    /// Bakes `grid` at `probe_spacing` and [`FULL_RAYS`], reading the visibility volume back — the
    /// whole path a caller of [`GiBake::resolve`] drives, minus the frame around it. Lightless and
    /// sunless, so the visibility assertions measure only the march.
    fn bake_and_read(
        gpu: &Gpu,
        grid: &WorldVoxelGrid,
        probe_spacing: f32,
    ) -> (GiData, Vec<[f32; 4]>) {
        let voxel_bake = VoxelBake::new();
        let voxel = voxel_bake
            .resolve(gpu, Some(grid))
            .expect("a Some grid uploads its textures");
        let gi = gi_data(grid, probe_spacing);
        let bake = GiBake::new(gpu);
        let resolved = bake.resolve(
            gpu,
            Some(BakeInput {
                grid,
                gi: &gi,
                albedo: &voxel.albedo,
                occupancy: &voxel.emissive_occupancy,
                lights: &[],
                sun: None,
                rays: FULL_RAYS,
            }),
        );
        assert_eq!(resolved.origin[3], 1.0, "a baked frame carries the flag on");
        let probes = bake.cached_probes().expect("the bake caches its volume");
        (gi, read_probes(gpu, &probes))
    }

    /// **The physics gate.** On a world whose bottom six cell rows are solid and whose upper half is
    /// open sky, a probe above the slab sees most of the sphere and leans *upward*, while a probe
    /// buried in the middle of the slab — deeper than the push-out can reach — reads exactly zero.
    /// Without this the bake is unverified: every other test here would pass on a shader that wrote
    /// a constant.
    #[test]
    fn needs_gpu_a_probe_above_a_slab_sees_sky_and_one_buried_inside_it_reads_zero() {
        let Some(gpu) =
            skip_or_gpu("a_probe_above_a_slab_sees_sky_and_one_buried_inside_it_reads_zero")
        else {
            return;
        };
        let grid = slab_grid(6);
        let (gi, probes) = bake_and_read(&gpu, &grid, 2.0);
        let at = |i: usize, j: usize, k: usize| {
            probes[i + gi.count[0] as usize * (j + gi.count[1] as usize * k)]
        };

        // World (8, 10, 8): four cells clear of the slab's top, in the middle of the footprint.
        let above = at(4, 5, 4);
        assert!(
            above[0] > 0.6 && above[0] < 0.95,
            "a probe above the slab must see most of the sphere but not all of it — the slab \
             blocks the steeper half of its downward rays; DC was {}",
            above[0]
        );
        // `.yzw` is the L1 vector, so index 2 is its world +Y component.
        assert!(
            above[2] > 4.0 * above[1].abs().max(above[3].abs()),
            "the L1 vector must lean toward the open sky (+Y) and dominate its horizontal wobble; \
             it was {above:?}"
        );

        // World (8, 2, 8): the middle of a six-row slab, with every cell within the push-out's 5³
        // reach also solid.
        let buried = at(4, 1, 4);
        assert_eq!(
            buried, [0.0; 4],
            "a probe with no free cell in reach must contribute darkness, not sky"
        );
    }

    /// **The rim gate.** The lattice routinely puts probes exactly on the grid's far faces —
    /// `count = ceil(span / spacing) + 1` lands the last one on the face itself whenever the spacing
    /// divides the span, which `slab_grid`'s 16-unit X at spacing 2.0 does (probe `i = 8` sits at
    /// `x = 16`, the grid's own max face). Every ray from there with a positive X component leaves
    /// the box at the instant it starts, so it sees sky no matter what is behind it.
    ///
    /// The expected value is the ray set's own arithmetic, not a tuned tolerance: visibility is
    /// binary per ray, the rays pointing out of the face escape and the rays pointing into the slab
    /// are blocked by its first cell, so the DC term is exactly the fraction of the [`FULL_RAYS`]
    /// spherical Fibonacci directions with `x > 0`. That fraction is 33/64 = 0.515625, and both the
    /// accumulated sum and the quotient are exact in binary — no rounding to absorb.
    ///
    /// **A full-quality statement, not a universal one**: a cheaper `gi_rays` re-derives the whole
    /// set, so its outward fraction is a different (equally exact) number. The fixture bakes at
    /// [`FULL_RAYS`] and the expected value is computed from it.
    ///
    /// This probe reads 0.0 under a DDA that treats "leaves the box at t = 0" as "did not leave":
    /// the ray gets clamped back into the boundary cell and reads *its* occupancy, which is the slab.
    /// The symptom is a dark rim around every GI volume.
    #[test]
    fn needs_gpu_a_probe_on_the_grids_max_face_sees_the_sky_behind_it() {
        let Some(gpu) = skip_or_gpu("a_probe_on_the_grids_max_face_sees_the_sky_behind_it") else {
            return;
        };
        let grid = slab_grid(6);
        let (gi, probes) = bake_and_read(&gpu, &grid, 2.0);
        assert_eq!(
            gi.count,
            [9, 7, 9],
            "the lattice reaches the far face exactly"
        );

        // Probe (8, 1, 4) = world (16, 2, 8): on the max-X face, at a height the slab fills, so the
        // one cell the boundary clamp would read is solid and the two answers are 0.0 and ~0.5.
        let rim = probes[8 + 9 * (1 + 7 * 4)];
        let outward = ray_set().iter().filter(|d| d[0] > 0.0).count();
        assert_eq!(
            outward, 33,
            "the ray set's outward half, counted not assumed"
        );
        assert_eq!(
            rim[0],
            outward as f32 / FULL_RAYS as f32,
            "a probe on the max face must see every ray that points off it; it read {}",
            rim[0],
        );
        assert!(
            rim[1] > 0.5 && rim[1] > 4.0 * rim[2].abs().max(rim[3].abs()),
            "and lean out through that face (+X), dominating the other two bands: {rim:?}"
        );
    }

    /// `gi_bake.wgsl`'s `ray_direction` restated on the CPU: 64 spherical Fibonacci directions,
    /// evenly spaced in `cos(elevation)` and advanced by the golden angle in azimuth. It exists so a
    /// test can state an expected visibility as *the fraction of the sphere a probe can reach*
    /// rather than as a recorded number. Every ray's `|cos(phi)|` is above 0.004, hundreds of times
    /// the `f32` error in the azimuth, so which side of the face a ray falls on is not a precision
    /// question.
    fn ray_set() -> Vec<[f32; 3]> {
        ray_set_of(FULL_RAYS as usize)
    }

    /// The same construction at an arbitrary ray count — what a quality setting moving `RAYS` would
    /// make the shader trace.
    fn ray_set_of(rays: usize) -> Vec<[f32; 3]> {
        const GOLDEN_RATIO_INVERSE: f32 = 0.618_034;
        (0..rays)
            .map(|i| {
                let y = 1.0 - 2.0 * (i as f32 + 0.5) / rays as f32;
                let r = (1.0f32 - y * y).max(0.0).sqrt();
                let phi = std::f32::consts::TAU * (i as f32 * GOLDEN_RATIO_INVERSE).fract();
                [r * phi.cos(), y, r * phi.sin()]
            })
            .collect()
    }

    /// **The `gi_rays` quality axis's precondition** (`Plan/scalability.md`): moving `RAYS`
    /// must leave the estimator well formed at the new count.
    ///
    /// It does, but *not* by truncation, and the difference is the whole point. Only the azimuth
    /// `TAU * fract(i * phi_inv)` is count-free; the elevation `1 - 2(i + 0.5)/RAYS` divides by the
    /// count, so a smaller `RAYS` **re-derives every direction** rather than keeping a prefix of the
    /// 64-ray set — ray `i` of a 32-ray set is nowhere near ray `i` of the 64-ray one. That is the
    /// desirable behaviour: each count is a complete sphere on its own terms, which is what these
    /// assertions state. Two consequences ride on it and are load-bearing elsewhere — the bake's
    /// result changes wholesale with the count (so no two counts' probes are comparable, and the
    /// count belongs in the bake's cache key), and `gi_inject.wgsl` must divide by the *same* count
    /// the bake traced or replay a direction the march never took.
    ///
    /// The contrast case at the end is why [`HIT_RAYS`] is strided rather than prefixed: a
    /// contiguous prefix of a **fixed** set really is a polar cap, and that failure is a different
    /// thing from the re-derivation above.
    #[test]
    fn the_fibonacci_ray_set_is_a_whole_sphere_at_every_quality_ray_count() {
        for rays in [16usize, 32, 48, 64] {
            let set = ray_set_of(rays);

            // Balanced: a one-sided set's mean direction has length O(1), not O(1/1000).
            let sum = set.iter().fold([0.0f64; 3], |a, d| {
                [
                    a[0] + f64::from(d[0]),
                    a[1] + f64::from(d[1]),
                    a[2] + f64::from(d[2]),
                ]
            });
            let mean = (sum[0] * sum[0] + sum[1] * sum[1] + sum[2] * sum[2]).sqrt() / rays as f64;
            assert!(
                mean < 0.01,
                "{rays} rays lean toward one direction; the mean direction's length was {mean}",
            );

            // Even on every axis: each open hemisphere holds about half the set.
            for (axis, name) in [(0, "x"), (1, "y"), (2, "z")] {
                let positive = set.iter().filter(|d| d[axis] > 0.0).count() as i64;
                let skew = (positive - rays as i64 / 2).abs();
                assert!(
                    skew <= 2,
                    "{rays} rays put {positive} in the +{name} hemisphere, not about {}",
                    rays / 2,
                );
            }

            // Distribution *quality* is what must be count-invariant: the closest pair sits at a
            // near-constant multiple of the packing bound `acos(1 - 2/N)` at every count, so a
            // cheaper preset is a coarser sphere and never a clumped one.
            let mut min_sep = std::f64::consts::PI;
            for (i, a) in set.iter().enumerate() {
                for b in &set[i + 1..] {
                    let dot = f64::from(a[0]) * f64::from(b[0])
                        + f64::from(a[1]) * f64::from(b[1])
                        + f64::from(a[2]) * f64::from(b[2]);
                    min_sep = min_sep.min(dot.clamp(-1.0, 1.0).acos());
                }
            }
            let bound = (1.0 - 2.0 / rays as f64).acos();
            let ratio = min_sep / bound;
            assert!(
                (1.4..1.7).contains(&ratio),
                "{rays} rays pack at {ratio}x the bound; every count measures ~1.55x",
            );
        }

        // The strided-vs-prefix contrast, stated as the numbers rather than left to the comment.
        let prefix_low = ray_set()[..HIT_RAYS as usize]
            .iter()
            .fold(f32::MAX, |m, d| m.min(d[1]));
        assert!(
            prefix_low > 0.5,
            "a contiguous prefix of the 64-ray set is a cap about +Y; its lowest ray was {prefix_low}",
        );
        let native_low = ray_set_of(HIT_RAYS as usize)
            .iter()
            .fold(f32::MAX, |m, d| m.min(d[1]));
        assert!(
            native_low < -0.9,
            "a natively-16-ray set reaches both poles; its lowest ray was {native_low}",
        );
    }

    /// The hit cache holds every `RAYS / HIT_RAYS`'th ray, so a `gi_rays` a quality setting can
    /// select must be a whole multiple of [`HIT_RAYS`] — otherwise the stride is fractional and the
    /// cached slots stop spanning the sphere evenly. This is the constraint on the axis's *values*,
    /// separate from the direction maths above.
    #[test]
    fn every_selectable_ray_count_is_a_whole_number_of_cache_strides() {
        for rays in [16u32, 32, 48, 64] {
            assert_eq!(
                rays % HIT_RAYS,
                0,
                "{rays} rays would stride the {HIT_RAYS}-slot hit cache fractionally",
            );
        }
    }

    /// A grid with no occupancy at all: every ray escapes, so every probe's DC term is exactly 1 and
    /// its L1 vector is the ray set's own residual — near zero, because the spherical Fibonacci
    /// directions sum to nothing. This is the escape path's own gate, and the balance assertion is
    /// what would catch a ray set that had quietly become one-sided.
    #[test]
    fn needs_gpu_an_empty_grid_bakes_full_visibility_and_a_balanced_ray_set() {
        let Some(gpu) = skip_or_gpu("an_empty_grid_bakes_full_visibility_and_a_balanced_ray_set")
        else {
            return;
        };
        let grid = slab_grid(0);
        let (_, probes) = bake_and_read(&gpu, &grid, 4.0);

        for probe in &probes {
            assert_eq!(
                probe[0], 1.0,
                "every ray escapes an empty grid, so the DC term is exactly 1"
            );
            let lean = (probe[1] * probe[1] + probe[2] * probe[2] + probe[3] * probe[3]).sqrt();
            assert!(
                lean < 0.05,
                "an unoccluded probe has no preferred direction; the L1 vector's length was {lean}"
            );
        }
    }

    /// Resolving the same grid and spacing twice bakes once: the second call is a cache hit, not a
    /// second dispatch. A changed `probe_spacing` is a different lattice and must re-bake.
    #[test]
    fn needs_gpu_an_unchanged_grid_and_spacing_bakes_once() {
        let Some(gpu) = skip_or_gpu("an_unchanged_grid_and_spacing_bakes_once") else {
            return;
        };
        let grid = slab_grid(6);
        let voxel_bake = VoxelBake::new();
        let voxel = voxel_bake.resolve(&gpu, Some(&grid)).expect("an upload");
        let bake = GiBake::new(&gpu);
        let input = |gi: &GiData| {
            bake.resolve(
                &gpu,
                Some(BakeInput {
                    grid: &grid,
                    gi,
                    albedo: &voxel.albedo,
                    occupancy: &voxel.emissive_occupancy,
                    lights: &[],
                    sun: None,
                    rays: FULL_RAYS,
                }),
            )
        };

        let coarse = gi_data(&grid, 4.0);
        input(&coarse);
        assert_eq!(bake.bake_count(), 1, "the first resolve bakes");
        input(&coarse);
        assert_eq!(
            bake.bake_count(),
            1,
            "an unchanged grid and spacing is a cache hit, not a second dispatch"
        );

        let fine = gi_data(&grid, 2.0);
        input(&fine);
        assert_eq!(
            bake.bake_count(),
            2,
            "a changed probe_spacing is a different lattice and must re-bake"
        );
    }

    /// **The ray count is a cache key, because a different count is a different volume.** The
    /// elevation term divides by it, so lowering `gi_rays` re-derives every direction rather than
    /// keeping a prefix — a bake held over from another count would be a *stale* volume, not a
    /// coarser one, and nothing downstream could tell.
    ///
    /// The resolved frame carries the count out with it too ([`ResolvedGi::rays`]), which is what
    /// the injection divides by rather than re-reading the machine's setting.
    #[test]
    fn needs_gpu_a_changed_ray_count_re_bakes_and_travels_with_the_resolved_frame() {
        let Some(gpu) =
            skip_or_gpu("a_changed_ray_count_re_bakes_and_travels_with_the_resolved_frame")
        else {
            return;
        };
        let grid = slab_grid(6);
        let voxel_bake = VoxelBake::new();
        let voxel = voxel_bake.resolve(&gpu, Some(&grid)).expect("an upload");
        let bake = GiBake::new(&gpu);
        let mut gi = gi_data(&grid, 4.0);
        gi.bounce = 1.0;
        let run = |rays: u32| {
            bake.resolve(
                &gpu,
                Some(BakeInput {
                    grid: &grid,
                    gi: &gi,
                    albedo: &voxel.albedo,
                    occupancy: &voxel.emissive_occupancy,
                    lights: &[],
                    sun: None,
                    rays,
                }),
            )
        };

        let full = run(FULL_RAYS);
        assert_eq!(bake.bake_count(), 1, "the first resolve bakes");
        assert_eq!(
            full.rays, FULL_RAYS,
            "the resolved frame names its own count"
        );
        run(FULL_RAYS);
        assert_eq!(
            bake.bake_count(),
            1,
            "an unchanged count is a cache hit, not a second dispatch",
        );

        let lowered = run(QualityParams::LOW.gi_rays);
        assert_eq!(
            bake.bake_count(),
            2,
            "a changed `gi_rays` re-derives every direction and must re-bake — serving the cached \
             volume would hand a lowered machine the full-quality bake",
        );
        assert_eq!(lowered.rays, QualityParams::LOW.gi_rays);
        run(FULL_RAYS);
        assert_eq!(
            bake.bake_count(),
            3,
            "and back again: the single-entry cache holds one count at a time",
        );
    }

    /// A frame with no `GiVolume` binds the fallback volume and carries a zero flag, and drops any
    /// volume baked for a previous scene rather than leaving it resident for a stale cache hit.
    #[test]
    fn needs_gpu_resolving_none_returns_the_fallback_and_clears_the_cache() {
        let Some(gpu) = skip_or_gpu("resolving_none_returns_the_fallback_and_clears_the_cache")
        else {
            return;
        };
        let grid = slab_grid(6);
        let voxel_bake = VoxelBake::new();
        let voxel = voxel_bake.resolve(&gpu, Some(&grid)).expect("an upload");
        let bake = GiBake::new(&gpu);
        let gi = gi_data(&grid, 4.0);

        bake.resolve(
            &gpu,
            Some(BakeInput {
                grid: &grid,
                gi: &gi,
                albedo: &voxel.albedo,
                occupancy: &voxel.emissive_occupancy,
                lights: &[],
                sun: None,
                rays: FULL_RAYS,
            }),
        );
        assert!(bake.cached_probes().is_some(), "a GI frame caches a volume");

        let resolved = bake.resolve(&gpu, None);
        assert_eq!(
            resolved.origin[3], 0.0,
            "an absent GiVolume carries the flag off"
        );
        assert!(
            bake.cached_probes().is_none(),
            "None drops the cached volume"
        );
    }

    /// **The unit gate.** A probe fully enclosed by an emissive shell of radiance `L` with albedo
    /// `1.0` must reconstruct `gi_bounce ≈ L` in every direction.
    ///
    /// This is what pins the storage layout's units claim to arithmetic instead of prose. The bake
    /// writes `sh0 = (1/N)ΣL` and `sh1 = 3·(1/N)ΣL·ω`, and the consumer reconstructs
    /// `M(n) = sh0 + (2/3)·dot(sh1, n)` — the cosine-weighted mean incoming radiance, the *same*
    /// quantity `ibl.wgsl` stores in the irradiance cube (no 1/PI). With every ray hitting a surface
    /// of the same radiance, `M(n)` collapses to `L` for every `n`, so a bake that dropped the `3.0`
    /// normalization, folded in a stray `1/PI`, or averaged over the wrong ray count reads wrong
    /// here rather than merely looking dim at the sitting.
    #[test]
    fn needs_gpu_an_enclosing_emissive_shell_reconstructs_its_own_radiance() {
        let Some(gpu) = skip_or_gpu("an_enclosing_emissive_shell_reconstructs_its_own_radiance")
        else {
            return;
        };
        const RADIANCE: f32 = 2.0;
        let grid = shell_grid(RADIANCE);
        let voxel_bake = VoxelBake::new();
        let voxel = voxel_bake.resolve(&gpu, Some(&grid)).expect("an upload");
        let mut gi = gi_data(&grid, 4.0);
        gi.bounce = 1.0;
        assert_eq!(gi.count, [3, 3, 3], "probes at 0, 4 and 8 on every axis");

        let bake = GiBake::new(&gpu);
        let input = BakeInput {
            grid: &grid,
            gi: &gi,
            albedo: &voxel.albedo,
            occupancy: &voxel.emissive_occupancy,
            lights: &[],
            sun: None,
            rays: FULL_RAYS,
        };
        bake.resolve(&gpu, Some(input));

        let volumes = bake
            .cached_bounce()
            .expect("a non-zero bounce bakes the three volumes");
        let channels: Vec<Vec<[f32; 4]>> = volumes.iter().map(|t| read_probes(&gpu, t)).collect();
        // Probe (1, 1, 1) = world (4, 4, 4), the shell's interior centre.
        let center = 1 + 3 * (1 + 3);
        let normals = [
            Vec3::X,
            Vec3::NEG_X,
            Vec3::Y,
            Vec3::NEG_Y,
            Vec3::Z,
            Vec3::NEG_Z,
            Vec3::ONE.normalize(),
        ];
        let mut worst = 0.0f32;
        for (c, channel) in channels.iter().enumerate() {
            let sh = channel[center];
            for n in normals {
                // `mesh.wgsl`'s reconstruction, restated: sh0 + (2/3)·dot(sh1, n).
                let m = sh[0] + (2.0 / 3.0) * (sh[1] * n.x + sh[2] * n.y + sh[3] * n.z);
                let error = (m - RADIANCE).abs() / RADIANCE;
                worst = worst.max(error);
                assert!(
                    error < 0.02,
                    "channel {c} reconstructed {m} along {n:?} where the enclosing shell radiates \
                     {RADIANCE}; the SH-L1 coefficients were {sh:?}"
                );
            }
        }
        eprintln!("bounce reconstruction: worst relative error {worst:.5}");

        // The readback shape carries the same DC term, so the instrument and the volume agree.
        let read = bake
            .resolve_and_read(
                &gpu,
                &BakeInput {
                    grid: &grid,
                    gi: &gi,
                    albedo: &voxel.albedo,
                    occupancy: &voxel.emissive_occupancy,
                    lights: &[],
                    sun: None,
                    rays: FULL_RAYS,
                },
            )
            .expect("the readback maps");
        let dc = read
            .bounce
            .expect("a bounce bake reads back a bounce volume")[center];
        for (c, v) in dc.iter().enumerate() {
            assert_eq!(
                *v, channels[c][center][0],
                "the readback's DC term is the volume's own"
            );
        }
    }

    /// **The cache gate.** The bake's key covers everything the bounce integrates and nothing it
    /// applies at sample time: an unchanged light set is a hit, a moved or recoloured or
    /// re-intensified light re-bakes, and flipping `bounce` between two *non-zero* values does not
    /// (strength multiplies the sampled result, it does not change what is written). Only the
    /// zero/non-zero discriminant participates, because that is what decides whether the volumes
    /// exist at all.
    #[test]
    fn needs_gpu_the_bake_key_tracks_the_lights_and_the_bounce_switch() {
        let Some(gpu) = skip_or_gpu("the_bake_key_tracks_the_lights_and_the_bounce_switch") else {
            return;
        };
        let grid = slab_grid(6);
        let voxel_bake = VoxelBake::new();
        let voxel = voxel_bake.resolve(&gpu, Some(&grid)).expect("an upload");
        let bake = GiBake::new(&gpu);
        let mut gi = gi_data(&grid, 4.0);
        gi.bounce = 1.0;

        let run = |gi: &GiData, lights: &[PointLightData], sun: Option<&SunData>| {
            bake.resolve(
                &gpu,
                Some(BakeInput {
                    grid: &grid,
                    gi,
                    albedo: &voxel.albedo,
                    occupancy: &voxel.emissive_occupancy,
                    lights,
                    sun,
                    rays: FULL_RAYS,
                }),
            );
        };

        let lights = [point_light(Vec3::new(8.0, 8.0, 8.0), Vec3::ONE, 2.0, 10.0)];
        run(&gi, &lights, None);
        assert_eq!(bake.bake_count(), 1, "the first resolve bakes");
        run(&gi, &lights, None);
        assert_eq!(
            bake.bake_count(),
            1,
            "an unchanged light set is a cache hit, not a second dispatch"
        );

        let mut moved = lights;
        moved[0].position.x += 1.0;
        run(&gi, &moved, None);
        assert_eq!(bake.bake_count(), 2, "a moved light re-bakes");

        let mut recolored = moved;
        recolored[0].color = Vec3::new(1.0, 0.5, 0.25);
        run(&gi, &recolored, None);
        assert_eq!(bake.bake_count(), 3, "a changed light colour re-bakes");

        let mut brighter = recolored;
        brighter[0].intensity = 4.0;
        run(&gi, &brighter, None);
        assert_eq!(bake.bake_count(), 4, "a changed light intensity re-bakes");

        let mut stronger = gi;
        stronger.bounce = 1.75;
        run(&stronger, &brighter, None);
        assert_eq!(
            bake.bake_count(),
            4,
            "bounce strength is applied at sample time, so two non-zero values share one bake"
        );

        let mut off = gi;
        off.bounce = 0.0;
        run(&off, &brighter, None);
        assert_eq!(
            bake.bake_count(),
            5,
            "turning bounce off changes what is written and must re-bake"
        );
        assert!(
            bake.cached_bounce().is_none(),
            "a zero-bounce bake allocates no bounce volumes"
        );
    }

    /// The sun enters the key by the two fields the bake reads and by nothing else. Its
    /// `light_space` and `bias` are camera-fitted and move every frame the camera does; hashing them
    /// would re-voxelize the world on every step the player takes.
    #[test]
    fn needs_gpu_the_bake_key_reads_the_suns_direction_and_colour_but_not_its_shadow_fit() {
        let Some(gpu) =
            skip_or_gpu("the_bake_key_reads_the_suns_direction_and_colour_but_not_its_shadow_fit")
        else {
            return;
        };
        let grid = slab_grid(6);
        let voxel_bake = VoxelBake::new();
        let voxel = voxel_bake.resolve(&gpu, Some(&grid)).expect("an upload");
        let bake = GiBake::new(&gpu);
        let mut gi = gi_data(&grid, 4.0);
        gi.bounce = 1.0;

        let run = |sun: Option<&SunData>| {
            bake.resolve(
                &gpu,
                Some(BakeInput {
                    grid: &grid,
                    gi: &gi,
                    albedo: &voxel.albedo,
                    occupancy: &voxel.emissive_occupancy,
                    lights: &[],
                    sun,
                    rays: FULL_RAYS,
                }),
            );
        };
        let sun = |direction: Vec3, color: Vec3, fit: f32| SunData {
            direction,
            color,
            light_space: Mat4::from_scale(Vec3::splat(fit)),
            bias: fit,
        };
        let down = Vec3::new(0.0, -1.0, 0.0);

        run(Some(&sun(down, Vec3::ONE, 1.0)));
        assert_eq!(bake.bake_count(), 1, "the first sun bakes");
        run(Some(&sun(down, Vec3::ONE, 7.0)));
        assert_eq!(
            bake.bake_count(),
            1,
            "a re-fitted shadow map is not an input to the bake and must not invalidate it"
        );
        run(Some(&sun(
            Vec3::new(0.3, -1.0, 0.1).normalize(),
            Vec3::ONE,
            1.0,
        )));
        assert_eq!(bake.bake_count(), 2, "a moved sun re-bakes");
        run(Some(&sun(down, Vec3::new(1.0, 0.8, 0.6), 1.0)));
        assert_eq!(bake.bake_count(), 3, "a recoloured sun re-bakes");
        run(None);
        assert_eq!(bake.bake_count(), 4, "losing the sun re-bakes");
    }

    /// The hit cache's shape is written down twice — as Rust constants here and as WGSL constants in
    /// the shader that sizes its own writes from them, reflected into `shader_gen` — so this refuses
    /// a drift between the two.
    #[test]
    fn the_shader_and_the_cpu_agree_on_the_hit_cache_layout() {
        assert_eq!(shader_gen::HIT_RAYS, HIT_RAYS);
        assert_eq!(shader_gen::HIT_FACE_SHIFT, HIT_FACE_SHIFT);
        assert_eq!(shader_gen::HIT_FACE_BOUNDARY, HIT_FACE_BOUNDARY);
        assert_eq!(shader_gen::HIT_FACE_ESCAPED, HIT_FACE_ESCAPED);
        // The ray count is no longer a constant on either side — it is `params.flags.y`, a quality
        // axis — so what has to hold instead is the relation the cache's stride is derived from: the
        // budget divides the count wholly, at every value a preset can select. The full-quality end
        // of that is stated here, beside the buffer HIT_RAYS sizes.
        assert_eq!(
            QualityParams::FULL.gi_rays % HIT_RAYS,
            0,
            "the cache records every `gi_rays / HIT_RAYS`'th ray, so the budget must divide the \
             count wholly — `every_presets_gi_axes_satisfy_their_own_constraints` says so for the \
             other three presets",
        );
        // The packing must leave the cell index room for every cell `L2103` admits, and the face
        // code room for its seven values inside the same 32-bit word.
        const _: () = {
            assert!(1u64 << HIT_FACE_SHIFT >= 32_000_000);
            assert!(HIT_FACE_ESCAPED < 8 && HIT_FACE_SHIFT + 3 <= 32);
        };
    }

    /// **`gi_probe_scale` at `full` is the exact identity, and below it only ever removes probes.**
    ///
    /// The first half is the zero-golden-churn claim for this axis: a machine that was never told a
    /// quality must dispatch over the lattice `ludion-engine` sized, field for field, not a lattice
    /// that happens to round to the same numbers. The second is why the axis cannot breach the
    /// `L2103`/`L2105` budgets the engine already enforced on the authored spacing — it is stated as
    /// a property over the whole grid of counts rather than argued in prose.
    #[test]
    fn scaling_the_probe_spacing_is_the_identity_at_full_and_only_ever_coarsens() {
        let grid = slab_grid(6);
        for spacing in [0.5f32, 1.0, 2.0, 3.0, 4.0] {
            let authored = gi_data(&grid, spacing);
            assert_eq!(
                scaled_lattice(&authored, QualityParams::FULL.gi_probe_scale),
                authored,
                "`full` re-lays the authored lattice exactly, spacing and span included",
            );

            let mut previous = authored.count;
            for scale in [1.25f32, 1.5, 2.0, 3.0, 4.0] {
                let scaled = scaled_lattice(&authored, scale);
                assert_eq!(scaled.probe_spacing, spacing * scale);
                for axis in 0..3 {
                    assert!(
                        scaled.count[axis] <= previous[axis],
                        "a coarser spacing must never lay more probes: {:?} then {:?} at {scale}",
                        previous,
                        scaled.count,
                    );
                    assert!(
                        scaled.count[axis] >= 2,
                        "trilinear interpolation needs two probes per axis at every scale",
                    );
                    // The coarser lattice still covers the world box the authored one did, which is
                    // what makes it a cheaper estimate of the same volume rather than a smaller one.
                    let (span, authored_span) = (scaled.span.to_array(), authored.span.to_array());
                    assert!(
                        span[axis] >= authored_span[axis],
                        "the scaled lattice must still span the authored one: {span:?} vs \
                         {authored_span:?}",
                    );
                }
                previous = scaled.count;
            }
            // The strengths are the look and stay the scene's at every scale.
            let coarse = scaled_lattice(&authored, 2.0);
            assert_eq!(coarse.sky_visibility, authored.sky_visibility);
            assert_eq!(coarse.bounce, authored.bounce);
            assert_eq!(coarse.normal_bias, authored.normal_bias);
            assert_eq!(coarse.origin, authored.origin);
        }
        // And the scene-free half the graph relies on: the spacing cancels, so `PostChain::masked`
        // can answer the same question from the count alone.
        let authored = gi_data(&grid, 2.0);
        for scale in [1.0f32, 1.5, 2.0, 4.0] {
            assert_eq!(
                scaled_probe_count(authored.count, scale),
                scaled_lattice(&authored, scale).count,
            );
        }
    }

    /// The hit cache's memory line, as arithmetic rather than prose — the numbers the `MAX_GI_PROBES`
    /// guardrail comment states, so neither can rot alone.
    #[test]
    fn the_hit_cache_costs_what_the_guardrail_comment_says() {
        // A district lattice, ~460 k probes: ~30 MB.
        let district = hit_cache_bytes([110, 38, 110]);
        assert!(
            (28..32).contains(&(district / 1_000_000)),
            "a district's hit cache is ~30 MB, not {} MB",
            district / 1_000_000,
        );
        // The `MAX_GI_PROBES` cap of 4 M probes: 256 MB, a number to state and not to reach.
        assert_eq!(hit_cache_bytes([4_000_000, 1, 1]), 256_000_000);
    }

    /// **The bin oracle (`Plan/gi-wave3-dynamic-injection.md` §11 T8, static half).** The same scene
    /// baked through the same shader with the culled bin and with the degenerate all-lights-in-every-
    /// cell one must produce byte-identical volumes. That is the whole byte-identity claim of the
    /// bin's adoption: a light the bin drops was out of range of the cell being shaded, so its term
    /// was exactly `+0.0`, and the survivors keep their order.
    #[test]
    fn needs_gpu_a_binned_bake_and_a_degenerate_bin_write_the_same_bytes() {
        let Some(gpu) = skip_or_gpu("a_binned_bake_and_a_degenerate_bin_write_the_same_bytes")
        else {
            return;
        };
        let grid = slab_grid(6);
        let voxel_bake = VoxelBake::new();
        let voxel = voxel_bake.resolve(&gpu, Some(&grid)).expect("an upload");
        let mut gi = gi_data(&grid, 2.0);
        gi.bounce = 1.0;

        // Six torches spread across the slab's footprint at ranges well under the grid's 16 units —
        // the regime the bin exists for, where most cells see only some of them.
        let lights: Vec<PointLightData> = [
            (Vec3::new(2.0, 7.0, 2.0), Vec3::new(1.0, 0.6, 0.3)),
            (Vec3::new(14.0, 7.0, 2.0), Vec3::new(0.3, 1.0, 0.5)),
            (Vec3::new(2.0, 7.0, 14.0), Vec3::new(0.4, 0.5, 1.0)),
            (Vec3::new(14.0, 7.0, 14.0), Vec3::new(1.0, 1.0, 0.4)),
            (Vec3::new(8.0, 9.0, 8.0), Vec3::new(0.8, 0.8, 0.8)),
            (Vec3::new(8.0, 7.0, 2.0), Vec3::new(1.0, 0.2, 0.2)),
        ]
        .into_iter()
        .map(|(position, color)| point_light(position, color, 2.0, 4.0))
        .collect();

        // Non-vacuity, stated before the comparison: at the cells the bake actually shades — the
        // grid's own cells — the culled bin must both light some of them and drop lights at some of
        // them, or the two bakes below are the same loop wearing different names.
        let premultiplied_lights: Vec<Light> = lights.iter().map(premultiplied).collect();
        let bin = build_bin(&grid, &premultiplied_lights, BinBuild::Culled);
        assert_eq!(bin.table.len(), BIN_CELLS);
        let mut lit = 0usize;
        let mut culled_somewhere = 0usize;
        for z in 0..grid.extent[2] {
            for y in 0..grid.extent[1] {
                for x in 0..grid.extent[0] {
                    let centre = grid.origin
                        + Vec3::new(x as f32 + 0.5, y as f32 + 0.5, z as f32 + 0.5)
                            * grid.cell_size;
                    let held = bin.table[bin_cell_of(&bin.params, centre)][1] as usize;
                    lit += usize::from(held > 0);
                    culled_somewhere += usize::from(held < lights.len());
                }
            }
        }
        assert!(
            lit > 0 && culled_somewhere > 0,
            "of {} shaded cells, {lit} see a light and {culled_somewhere} see fewer than all {} — \
             the oracle needs both to be non-zero",
            grid.cell_count(),
            lights.len(),
        );

        let bake_with = |build: BinBuild| {
            let bake = GiBake::new(&gpu);
            bake.set_bin_build(build);
            bake.resolve(
                &gpu,
                Some(BakeInput {
                    grid: &grid,
                    gi: &gi,
                    albedo: &voxel.albedo,
                    occupancy: &voxel.emissive_occupancy,
                    lights: &lights,
                    sun: None,
                    rays: FULL_RAYS,
                }),
            );
            let volumes = bake.cached_bounce().expect("a bounce bake");
            let mut read: Vec<Vec<[f32; 4]>> =
                volumes.iter().map(|t| read_probes(&gpu, t)).collect();
            read.push(read_probes(
                &gpu,
                &bake.cached_probes().expect("a visibility volume"),
            ));
            read
        };

        let culled = bake_with(BinBuild::Culled);
        let degenerate = bake_with(BinBuild::Degenerate);
        assert!(
            culled[0].iter().any(|probe| probe[0] > 0.0),
            "the lights must actually bounce, or the comparison below is two black volumes agreeing",
        );
        assert_eq!(
            culled, degenerate,
            "the binned bake must be byte-identical to the all-lights-in-every-cell oracle — a \
             difference is a conservatism hole in the bin",
        );
    }

    /// **The exclusion is exact (§11 T5a).** A marked light is filtered out by
    /// [`static_lights`], and the volumes that result are byte-identical to a bake of the same scene
    /// with that light deleted outright — not merely similar. The third bake, with the same light
    /// left unmarked, is the non-vacuity guard: it must differ, or the fixture is proving nothing.
    #[test]
    fn needs_gpu_a_marked_light_leaves_the_bake_exactly() {
        let Some(gpu) = skip_or_gpu("a_marked_light_leaves_the_bake_exactly") else {
            return;
        };
        let grid = slab_grid(6);
        let voxel_bake = VoxelBake::new();
        let voxel = voxel_bake.resolve(&gpu, Some(&grid)).expect("an upload");
        let mut gi = gi_data(&grid, 2.0);
        gi.bounce = 1.0;

        let torch = point_light(Vec3::new(4.0, 7.0, 4.0), Vec3::new(1.0, 0.6, 0.3), 2.0, 6.0);
        let mut lamp = point_light(
            Vec3::new(11.0, 7.0, 11.0),
            Vec3::new(0.3, 0.5, 1.0),
            2.0,
            6.0,
        );
        lamp.dynamic = true;

        let bake_of = |lights: &[PointLightData]| {
            let bake = GiBake::new(&gpu);
            bake.resolve(
                &gpu,
                Some(BakeInput {
                    grid: &grid,
                    gi: &gi,
                    albedo: &voxel.albedo,
                    occupancy: &voxel.emissive_occupancy,
                    lights,
                    sun: None,
                    rays: FULL_RAYS,
                }),
            );
            let volumes = bake.cached_bounce().expect("a bounce bake");
            volumes
                .iter()
                .map(|t| read_probes(&gpu, t))
                .collect::<Vec<_>>()
        };

        let mixed = [torch, lamp];
        let filtered = static_lights(&mixed);
        assert_eq!(filtered, vec![torch], "the marker is what the filter reads");

        let with_marked = bake_of(&filtered);
        let deleted = bake_of(&[torch]);
        assert_eq!(
            with_marked, deleted,
            "a marked light must leave the bake exactly — the volumes are the deleted-light bake's",
        );

        let mut unmarked_lamp = lamp;
        unmarked_lamp.dynamic = false;
        let both = bake_of(&[torch, unmarked_lamp]);
        assert_ne!(
            both, deleted,
            "the lamp must actually change the bake when it is unmarked, or the exclusion above is \
             a comparison of a light against itself",
        );
    }

    /// **The thrash fix, both halves (§11 T5c).** Moving a *marked* light is a bake cache hit — its
    /// motion never reaches the lights signature — while moving an unmarked one still re-bakes, the
    /// documented consequence that stays a decision rather than drifting into a bug.
    #[test]
    fn needs_gpu_moving_a_marked_light_is_a_cache_hit_and_moving_an_unmarked_one_is_not() {
        let Some(gpu) =
            skip_or_gpu("moving_a_marked_light_is_a_cache_hit_and_moving_an_unmarked_one_is_not")
        else {
            return;
        };
        let grid = slab_grid(6);
        let voxel_bake = VoxelBake::new();
        let voxel = voxel_bake.resolve(&gpu, Some(&grid)).expect("an upload");
        let bake = GiBake::new(&gpu);
        let mut gi = gi_data(&grid, 4.0);
        gi.bounce = 1.0;

        let run = |lights: &[PointLightData]| {
            let statics = static_lights(lights);
            bake.resolve(
                &gpu,
                Some(BakeInput {
                    grid: &grid,
                    gi: &gi,
                    albedo: &voxel.albedo,
                    occupancy: &voxel.emissive_occupancy,
                    lights: &statics,
                    sun: None,
                    rays: FULL_RAYS,
                }),
            );
        };

        let torch = point_light(Vec3::new(4.0, 7.0, 4.0), Vec3::ONE, 2.0, 6.0);
        let mut lamp = point_light(Vec3::new(11.0, 7.0, 11.0), Vec3::ONE, 2.0, 6.0);
        lamp.dynamic = true;

        let mut lights = [torch, lamp];
        run(&lights);
        assert_eq!(bake.bake_count(), 1, "the first resolve bakes");

        for step in 1..=4 {
            lights[1].position.x += 0.75;
            lights[1].position.z -= 0.5;
            run(&lights);
            assert_eq!(
                bake.bake_count(),
                1,
                "step {step}: a marked light a system moves must invalidate nothing",
            );
        }

        lights[0].position.x += 0.75;
        run(&lights);
        assert_eq!(
            bake.bake_count(),
            2,
            "an unmarked light still re-bakes when it moves — the documented consequence, asserted \
             so it stays a decision",
        );
    }

    /// **The hold, and every transition through it.** A held bake serves the volumes it already has
    /// for a key it was not baked under and says so; a world that returns to the held baseline is a
    /// real key hit and is honestly not stale; releasing the hold for one frame (what
    /// `Engine::gi_rebake_now` does) re-bakes and makes the current world the new baseline.
    ///
    /// Asserted on [`GiBake::bake_count`] — the dispatch itself — so "held" means *no GPU work*,
    /// not merely the same volume reproduced.
    #[test]
    fn needs_gpu_a_held_bake_serves_the_cached_volumes_and_reports_stale() {
        let Some(gpu) = skip_or_gpu("a_held_bake_serves_the_cached_volumes_and_reports_stale")
        else {
            return;
        };
        let grid = slab_grid(6);
        let voxel_bake = VoxelBake::new();
        let voxel = voxel_bake.resolve(&gpu, Some(&grid)).expect("an upload");
        let gi = gi_data(&grid, 4.0);
        let run = |bake: &GiBake, lights: &[PointLightData]| {
            bake.resolve(
                &gpu,
                Some(BakeInput {
                    grid: &grid,
                    gi: &gi,
                    albedo: &voxel.albedo,
                    occupancy: &voxel.emissive_occupancy,
                    lights,
                    sun: None,
                    rays: FULL_RAYS,
                }),
            );
        };

        let baseline = [point_light(Vec3::new(4.0, 7.0, 4.0), Vec3::ONE, 2.0, 6.0)];
        let mut moved = baseline;
        moved[0].position.x += 3.0;

        let mut bake = GiBake::new(&gpu);
        run(&bake, &baseline);
        assert_eq!(bake.bake_count(), 1, "the first resolve bakes");
        assert!(
            !bake.stale(),
            "a fresh bake of the current world is not stale"
        );

        bake.set_hold(true);
        run(&bake, &moved);
        assert_eq!(
            bake.bake_count(),
            1,
            "a held key miss must serve the cached volumes, not dispatch",
        );
        assert!(bake.stale(), "and must report that they are held");

        // Not latched: the comparison runs every frame, so a world back at the baseline is clean.
        run(&bake, &baseline);
        assert_eq!(bake.bake_count(), 1, "the baseline is a real cache hit");
        assert!(
            !bake.stale(),
            "a world returned to the held baseline is honestly not stale",
        );

        // The rebake: one frame with the hold suppressed refreshes the cache, and what it bakes is
        // the new baseline — holding again over the same world is a hit, not a held serve.
        bake.set_hold(false);
        run(&bake, &moved);
        assert_eq!(bake.bake_count(), 2, "a suppressed hold re-bakes");
        assert!(!bake.stale());
        bake.set_hold(true);
        run(&bake, &moved);
        assert_eq!(bake.bake_count(), 2, "the rebake became the new baseline");
        assert!(!bake.stale());
    }

    /// A *first* bake is never held: there is nothing cached to serve, and an unlit frame would be a
    /// worse answer than a stale one. So a renderer that starts held still bakes once — which is
    /// what makes `scene.load` under a hold cost exactly one bake and no more.
    #[test]
    fn needs_gpu_a_first_bake_under_a_hold_still_bakes() {
        let Some(gpu) = skip_or_gpu("a_first_bake_under_a_hold_still_bakes") else {
            return;
        };
        let grid = slab_grid(6);
        let voxel_bake = VoxelBake::new();
        let voxel = voxel_bake.resolve(&gpu, Some(&grid)).expect("an upload");
        let gi = gi_data(&grid, 4.0);

        let mut bake = GiBake::new(&gpu);
        bake.set_hold(true);
        bake.resolve(
            &gpu,
            Some(BakeInput {
                grid: &grid,
                gi: &gi,
                albedo: &voxel.albedo,
                occupancy: &voxel.emissive_occupancy,
                lights: &[],
                sun: None,
                rays: FULL_RAYS,
            }),
        );
        assert_eq!(bake.bake_count(), 1, "an empty cache cannot be held");
        assert!(!bake.stale(), "and what it baked is the current world");

        // Losing the `GiVolume` clears the flag rather than leaving the last answer standing.
        bake.resolve(&gpu, None);
        assert!(!bake.stale());
    }

    /// **A hold serves a content miss, never a structural one.** Bounce switching on, the ray count
    /// and the probe spacing each change what a bake *is* — a held bounce-off bake has no hit cache
    /// for `gi_inject` to read, and a held bake at another count or spacing is the wrong lattice —
    /// so each re-bakes under a hold, while a moved light (content) is still served stale.
    #[test]
    fn needs_gpu_a_hold_never_serves_a_structural_miss() {
        let Some(gpu) = skip_or_gpu("a_hold_never_serves_a_structural_miss") else {
            return;
        };
        let grid = slab_grid(6);
        let voxel_bake = VoxelBake::new();
        let voxel = voxel_bake.resolve(&gpu, Some(&grid)).expect("an upload");
        let mut bake = GiBake::new(&gpu);
        let baseline = [point_light(Vec3::new(4.0, 7.0, 4.0), Vec3::ONE, 2.0, 6.0)];
        let run = |bake: &GiBake, gi: &GiData, lights: &[PointLightData], rays: u32| {
            bake.resolve(
                &gpu,
                Some(BakeInput {
                    grid: &grid,
                    gi,
                    albedo: &voxel.albedo,
                    occupancy: &voxel.emissive_occupancy,
                    lights,
                    sun: None,
                    rays,
                }),
            )
        };

        let mut gi = gi_data(&grid, 4.0);
        gi.bounce = 0.0;
        run(&bake, &gi, &baseline, FULL_RAYS);
        assert_eq!(bake.bake_count(), 1);
        bake.set_hold(true);

        gi.bounce = 1.0;
        let bounced = run(&bake, &gi, &baseline, FULL_RAYS);
        assert_eq!(
            bake.bake_count(),
            2,
            "bounce switching on is structural: a held bounce-off bake has no hit cache to serve",
        );
        assert!(!bake.stale());
        assert!(
            bounced.hits.is_some(),
            "a bounce bake carries its hit cache"
        );

        let mut moved = baseline;
        moved[0].position.x += 3.0;
        run(&bake, &gi, &moved, FULL_RAYS);
        assert_eq!(bake.bake_count(), 2, "a moved light is content: still held");
        assert!(bake.stale());

        let lowered = run(&bake, &gi, &baseline, QualityParams::LOW.gi_rays);
        assert_eq!(
            bake.bake_count(),
            3,
            "a changed ray count re-bakes under a hold"
        );
        assert!(!bake.stale());
        assert_eq!(lowered.rays, QualityParams::LOW.gi_rays);

        let fine = {
            let mut fine = gi_data(&grid, 2.0);
            fine.bounce = 1.0;
            fine
        };
        run(&bake, &fine, &baseline, QualityParams::LOW.gi_rays);
        assert_eq!(
            bake.bake_count(),
            4,
            "a changed probe spacing re-bakes under a hold"
        );
        assert!(!bake.stale());
    }

    /// The hit cache records this march's own answers: inside a sealed emissive shell every prefix
    /// ray hits a cell of that shell, and on an empty grid every one escapes. This is what a later
    /// per-frame gather replays, so a cache full of escapes (or of garbage from an unwritten buried
    /// probe) would silently produce no dynamic light at all.
    #[test]
    fn needs_gpu_the_hit_cache_records_the_marchs_own_answers() {
        let Some(gpu) = skip_or_gpu("the_hit_cache_records_the_marchs_own_answers") else {
            return;
        };
        let voxel_bake = VoxelBake::new();
        let cache_of = |grid: &WorldVoxelGrid, gi: &GiData| {
            let voxel = voxel_bake.resolve(&gpu, Some(grid)).expect("an upload");
            let bake = GiBake::new(&gpu);
            let resolved = bake.resolve(
                &gpu,
                Some(BakeInput {
                    grid,
                    gi,
                    albedo: &voxel.albedo,
                    occupancy: &voxel.emissive_occupancy,
                    lights: &[],
                    sun: None,
                    rays: FULL_RAYS,
                }),
            );
            assert!(
                resolved.hits.is_some(),
                "a bounce bake hands its hit cache out on the resolved frame",
            );
            let probes = (gi.count[0] * gi.count[1] * gi.count[2]) as usize;
            read_buffer_u32(
                &gpu,
                &bake.cached_hits().expect("a bounce bake caches its hits"),
                probes * HIT_RAYS as usize,
            )
        };

        let shell = shell_grid(2.0);
        let mut gi = gi_data(&shell, 4.0);
        gi.bounce = 1.0;
        assert_eq!(gi.count, [3, 3, 3]);
        let cache = cache_of(&shell, &gi);
        assert_eq!(cache.len(), 27 * HIT_RAYS as usize);
        // Probe (1, 1, 1) — world (4, 4, 4), the shell's interior centre: every ray meets the skin.
        let center = (1 + 3 * (1 + 3)) * HIT_RAYS as usize;
        for (k, &word) in cache[center..center + HIT_RAYS as usize].iter().enumerate() {
            let face = word >> HIT_FACE_SHIFT;
            assert!(
                face < HIT_FACE_ESCAPED,
                "ray {k} from a probe sealed inside a shell cannot escape; word {word:#x}",
            );
            assert!(
                (word & ((1 << HIT_FACE_SHIFT) - 1)) < shell.cell_count() as u32,
                "ray {k}'s cell index must address a real cell",
            );
        }

        let empty = slab_grid(0);
        let mut gi = gi_data(&empty, 4.0);
        gi.bounce = 1.0;
        let cache = cache_of(&empty, &gi);
        assert!(
            cache
                .iter()
                .all(|&word| word == HIT_FACE_ESCAPED << HIT_FACE_SHIFT),
            "every ray escapes an empty grid, so every cache word is the escape code",
        );
    }

    /// **Test-only.** Copies a storage buffer of `len` `u32`s back to the CPU. Blocking, like
    /// [`read_probes`]; the hit cache is asserted this way and used no other way on the CPU.
    fn read_buffer_u32(gpu: &Gpu, buffer: &wgpu::Buffer, len: usize) -> Vec<u32> {
        let bytes = (len * std::mem::size_of::<u32>()) as u64;
        let device = gpu.device();
        let staging = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("gi hit cache readback"),
            size: bytes,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("gi hit cache readback"),
        });
        encoder.copy_buffer_to_buffer(buffer, 0, &staging, 0, bytes);
        gpu.queue().submit(Some(encoder.finish()));

        let (tx, rx) = std::sync::mpsc::channel();
        staging.slice(..).map_async(wgpu::MapMode::Read, move |r| {
            let _ = tx.send(r);
        });
        device
            .poll(wgpu::PollType::wait_indefinitely())
            .expect("device poll succeeds");
        rx.recv().expect("map completes").expect("map succeeds");

        let mapped = staging.slice(..).get_mapped_range().expect("mapped range");
        let out = bytemuck::cast_slice::<u8, u32>(&mapped).to_vec();
        drop(mapped);
        staging.unmap();
        out
    }
}
