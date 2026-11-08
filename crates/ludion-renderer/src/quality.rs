//! Per-machine render quality: which optional effects a client is willing to pay for.
//!
//! A **host** setting, never scene content (the same rule [`RenderScale`](crate::RenderScale) states
//! for resolution) — the same scene is the same scene at any quality, and content must not dictate
//! the client's GPU budget. It is render-only: never sim state, never hashed into a `WorldHash`,
//! never snapshotted, so two machines at different qualities run the same simulation and differ only
//! in pixels.
//!
//! This module holds plain data and nothing else. Resolving a [`Quality`] — from a config file, an
//! environment variable, a command line — belongs to the host that owns those sources, which keeps
//! this crate free of I/O exactly as [`RenderScene`](crate::RenderScene) keeps it free of asset
//! paths.

/// The most cascades a sun can fit: the 2×2 atlas cap. The schema bound on
/// `DirectionalLight.cascades` matches it.
pub const MAX_SUN_CASCADES: u32 = 4;

/// Which optional effects this machine renders.
///
/// [`Quality::FULL`] — also [`Default`] — is every toggle true: masking is then the identity, so a
/// renderer that was never told a quality draws exactly what it always drew. A false toggle is
/// indistinguishable from the scene not authoring that effect at all
/// ([`PostChain::masked`](crate::PostChain::masked)): its passes are not scheduled, not weakened.
///
/// [`shadows`](Quality::shadows) is the one toggle whose off state is not a frame a scene could
/// author — nothing spells "a sun that lights without casting" — so it keeps the narrower half of
/// the same invariant: the passes leave the graph, and the light they belonged to is untouched.
///
/// Bloom is deliberately absent — it is always on, part of the imaging chain rather than an optional
/// effect, and the composite pass that applies it runs on every frame regardless.
///
/// # Example
/// ```
/// use ludion_renderer::Quality;
///
/// assert_eq!(Quality::default(), Quality::FULL);
/// assert_eq!(Quality::preset("Low"), Some(Quality::LOW));
/// assert_eq!(Quality::preset("cinematic"), None);
/// assert!(Quality::FULL.ssao && !Quality::LOW.ssao);
/// assert!(Quality::FULL.ssr && !Quality::LOW.ssr);
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Quality {
    /// Render ambient occlusion on the scenes that author it.
    pub ssao: bool,
    /// Render volumetric light shafts on the scenes that author them.
    pub shafts: bool,
    /// Render depth of field on the scenes that author it.
    pub dof: bool,
    /// Inject dynamic-light bounce on the scenes that author it (a `GiVolume` with a non-zero
    /// `bounce` and at least one `GiDynamic`-marked light).
    ///
    /// False removes the `gi_inject` pass from the frame graph, binds the 1×1×1 fallback volume and
    /// zeroes the shader's flag — it never re-classifies a light. A marked light *baking* instead
    /// would hand the low-end machine a per-frame re-bake, which is precisely the cost the marker
    /// exists to remove; masked therefore means "the marked lights contribute direct light only",
    /// and the bake's inputs stay world state that no host setting can reach.
    pub gi_dynamic: bool,
    /// Render the sun's shadow map on the scenes that carry a sun.
    ///
    /// False removes the `shadow` pass and its map from the frame graph and zeroes the light-space
    /// matrix the scene pass uploads, so `mesh.wgsl`'s `sun_shadow` takes its fully-lit early-out
    /// and never samples a map that was not drawn. **The sun keeps lighting the scene**: its colour
    /// and direction are untouched, so a masked frame is a sunlit shadowless one and not a sunless
    /// one — turning the light off would be a look change, which is the one thing a machine's
    /// setting must never make.
    ///
    /// Volumetric light shafts march that same map, so masking shadows masks them too
    /// ([`PostChain::masked`](crate::PostChain::masked)): the march has nothing to read, and keeping
    /// the map alive for it would make "shadows off" mean "the shadow pass still runs".
    pub shadows: bool,
    /// Render screen-space reflections on the scenes that author them (`PostData.ssr > 0`).
    ///
    /// False removes the `ssr_trace` pass and the two pyramids it reads from the frame graph and
    /// zeroes the flag `reflections` reads, so the pass falls back to the sky fill it blends
    /// against — exactly what a scene with `ssr: 0` renders.
    pub ssr: bool,
}

impl Quality {
    /// Everything the scene authors, rendered — the default, and the exact behaviour of every render
    /// path.
    pub const FULL: Quality = Quality {
        ssao: true,
        shafts: true,
        dof: true,
        gi_dynamic: true,
        shadows: true,
        ssr: true,
    };

    /// Equal to [`FULL`](Quality::FULL): every effect a scene authors still runs at this preset, and
    /// what separates the two is how hard each one works
    /// ([`QualityParams::HIGH`](QualityParams::HIGH)) rather than which ones exist.
    pub const HIGH: Quality = Quality::FULL;

    /// Equal to [`FULL`](Quality::FULL), for the same reason as [`HIGH`](Quality::HIGH) — the
    /// separation is [`QualityParams::MEDIUM`](QualityParams::MEDIUM).
    pub const MEDIUM: Quality = Quality::FULL;

    /// None of the optional effects: a machine at this preset renders every scene as if it authored
    /// no ambient occlusion, no light shafts, nothing out of focus, and no light marked dynamic for
    /// global illumination, no screen-space reflections — and its suns light without casting.
    pub const LOW: Quality = Quality {
        ssao: false,
        shafts: false,
        dof: false,
        gi_dynamic: false,
        shadows: false,
        ssr: false,
    };

    /// Every name [`preset`](Quality::preset) accepts, cheapest first — the list a host prints when
    /// it rejects one.
    pub const PRESET_NAMES: [&str; 4] = ["low", "medium", "high", "full"];

    /// The preset `name` selects, or `None` if it is not one of [`PRESET_NAMES`](Quality::PRESET_NAMES).
    ///
    /// Case-insensitive, because the name reaches a host as human-typed text (a config value, a flag)
    /// and `Low` meaning something different from `low` would be a trap, not a feature.
    pub fn preset(name: &str) -> Option<Quality> {
        match name.to_ascii_lowercase().as_str() {
            "low" => Some(Quality::LOW),
            "medium" => Some(Quality::MEDIUM),
            "high" => Some(Quality::HIGH),
            "full" => Some(Quality::FULL),
            _ => None,
        }
    }
}

impl Default for Quality {
    fn default() -> Self {
        Quality::FULL
    }
}

/// How hard each effect works — the parameter axes a quality setting modulates.
///
/// A [`Quality`] says which effects run; this says how hard each one works. The two are separate
/// because they move independently: an effect that is off has no sample count, and an effect that is
/// on can still be made cheaper. That is what separates [`MEDIUM`](QualityParams::MEDIUM) and
/// [`HIGH`](QualityParams::HIGH) from [`FULL`](QualityParams::FULL), whose [`Quality`] toggles are
/// identical.
///
/// Every axis is modulated. Four are uniform loop bounds the shaders read (`ssao_samples`,
/// `shaft_steps`, `dof_taps`, `pcf`); `gi_rays` is a uniform both GI shaders read, one as a loop
/// bound and one as a divisor; `sun_cascades` caps how many cascades a sun fits; six are structure
/// rather than arithmetic (`shadow_map_size` as the shadow map's extent, `local_shadow_atlas_size`
/// as the local-light atlas's, `ibl_specular_size` as the sky specular cube's, `gi_probe_scale` as
/// the probe lattice's, `render_scale` as the extent the scene is drawn at before it is resampled
/// to the output, `ssr_scale` as the reflection trace's); and `particle_density` is spent before
/// this crate ever sees the frame, thinning how many particles reach the scene at all. A preset
/// carrying the full value for an axis is therefore a statement about that axis, not a placeholder.
///
/// It is reported beside a profile measurement for the reason a worker count is: two runs taken
/// under different machine settings are not comparable, and only the report can say so.
///
/// # Example
/// ```
/// use ludion_renderer::QualityParams;
///
/// assert_eq!(QualityParams::FULL.ssao_samples, 16);
/// assert_eq!(QualityParams::MEDIUM.ssao_samples, 8);
/// assert_eq!(QualityParams::preset("Medium"), Some(QualityParams::MEDIUM));
/// // A cheaper preset draws the sun into a smaller map and averages fewer comparison taps.
/// assert_eq!(QualityParams::MEDIUM.shadow_map_size, 1024);
/// assert_eq!(QualityParams::FULL.shadow_map_size, 2048);
/// // It also caps how many cascades a sun that authors them may fit.
/// assert_eq!(QualityParams::FULL.sun_cascade_count(Some(4)), Some(4));
/// assert_eq!(QualityParams::MEDIUM.sun_cascade_count(Some(4)), Some(2));
/// assert_eq!(QualityParams::MEDIUM.sun_cascade_count(None), None);
/// // The sky's specular cube halves its face edge at the cheapest preset.
/// assert_eq!(QualityParams::FULL.ibl_specular_size, 256);
/// assert_eq!(QualityParams::LOW.ibl_specular_size, 128);
/// // Only the cheapest preset draws below the output extent; `full`'s 1.0 is that axis's ceiling.
/// assert_eq!(QualityParams::LOW.render_scale, 0.75);
/// assert_eq!(QualityParams::MEDIUM.render_scale, 1.0);
/// // It is also the only preset that thins the particle population, on the same reading.
/// assert_eq!(QualityParams::LOW.particle_density, 0.25);
/// assert_eq!(QualityParams::MEDIUM.particle_density, 1.0);
/// ```
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct QualityParams {
    /// Hemisphere samples per ambient-occlusion pixel (`Ssao.samples` in `ssao.wgsl`).
    pub ssao_samples: u32,
    /// Ray-march steps per light-shaft pixel (`Shafts.steps` in `shafts.wgsl`).
    pub shaft_steps: u32,
    /// Taps in the depth-of-field spiral (`Dof.taps` in `dof.wgsl`).
    pub dof_taps: u32,
    /// Edge length of the square sun shadow map, in texels — the extent
    /// [`build_frame_graph`](crate::build_frame_graph) declares the `shadow_map` transient at. The
    /// only axis the frame graph itself carries ([`PostChain::shadow_map`](crate::PostChain)),
    /// because a resource's extent is graph structure; the mesh shader needs no telling, since it
    /// derives its texel size from `textureDimensions`.
    pub shadow_map_size: u32,
    /// Caps the scene's `DirectionalLight.cascades` at this quality, `1..=`[`MAX_SUN_CASCADES`]. A
    /// scene without `cascades` is untouched. A count, so cheaper is smaller; masked with the sun's
    /// shadows at [`Quality::LOW`].
    pub sun_cascades: u32,
    /// Edge length of the square local-light shadow atlas, in texels — the extent
    /// [`build_frame_graph`](crate::build_frame_graph) declares the `local_shadow_atlas` transient at.
    /// Structure like [`shadow_map_size`](QualityParams::shadow_map_size), carried by
    /// [`PostChain::local_shadow_atlas`](crate::PostChain). **A multiple of 16**: the atlas is an 8×8
    /// grid of cells whose spot tiles are 2×2. The caster budget is a view count fixed by
    /// `local_shadow::ATLAS_CELLS`, so only resolution scales.
    pub local_shadow_atlas_size: u32,
    /// Edge length of the sky IBL's specular cube faces, in texels — a power of two, at least 16 (two
    /// mips), with a mip chain below it that stops at 8×8. Structure like [`shadow_map_size`](QualityParams::shadow_map_size): the mesh
    /// shaders learn the lod range from the `reflections` uniform, not from the size.
    pub ibl_specular_size: u32,
    /// Edge length of the sun shadow map's PCF kernel, in taps (`Sun.pcf` in `mesh.wgsl`): `3` is
    /// the 3×3 the mesh pass averages, `1` a single centre tap. Odd, because the kernel is centred
    /// on the shaded texel.
    pub pcf: u32,
    /// Rays cast per probe by the GI bake (`params.flags.y` in `gi_bake.wgsl`, `params.rays.x` in
    /// `gi_inject.wgsl`).
    ///
    /// **A whole multiple of the hit cache's 16 slots**, because the cache records every
    /// `gi_rays / 16`'th ray and a fractional stride would stop those slots spanning the sphere.
    ///
    /// The count re-derives the whole ray set rather than truncating it — the elevation term divides
    /// by it — so a cheaper preset bakes a *different*, equally well-formed sphere rather than a
    /// prefix of this one. Both GI shaders therefore read the same value: the injection replays the
    /// bake's cached hits and must divide by the count that march actually traced.
    ///
    /// Unlike [`shadow_map_size`](QualityParams::shadow_map_size), this axis stays observable at
    /// [`Quality::LOW`]: `gi_dynamic: false` masks the *injection*, but the bake still runs and its
    /// sky-visibility and bounce volumes still light the frame.
    pub gi_rays: u32,
    /// Multiplier on the scene's authored `GiVolume.probe_spacing` before the probe lattice is laid.
    /// `1.0` bakes at exactly the spacing the scene asked for; a larger factor bakes coarser.
    ///
    /// **The one axis that reads upward**, because it is a spacing and not a count: cheaper is
    /// *larger* here, so its ceiling is `1.0` rather than its floor, and the lattice only ever loses
    /// probes to it (`count = ceil(span / spacing) + 1` falls as the spacing grows). The authored
    /// spacing stays the look; a machine scales it.
    pub gi_probe_scale: f32,
    /// Fraction of the output extent the scene is drawn at before being resampled up to it. `1.0`
    /// draws every output pixel natively; `0.75` draws 56% of them and magnifies.
    ///
    /// A *multiplier* like [`gi_probe_scale`](QualityParams::gi_probe_scale) rather than a count,
    /// but it reads the ordinary way round: it scales the frame the machine pays for rather than an
    /// authored length, so cheaper is *smaller* and `full`'s `1.0` is this axis's **ceiling**.
    /// Bounding it there is what keeps "a quality setting only ever spends less than the scene's own
    /// authoring implies" true of resolution as well as of the counts — supersampling above native
    /// stays the host's own [`RenderScale`](crate::RenderScale), which no machine's quality reaches.
    ///
    /// It is the bluntest axis in the set: cost falls with the *square* of it and every pass the
    /// render extent sizes takes the discount at once, which is why `low` is the only preset that
    /// spends it. [`HIGH`](QualityParams::HIGH) and [`MEDIUM`](QualityParams::MEDIUM) hold `1.0`
    /// deliberately — those two are meant to be a coarser *estimate* of the same picture, and a
    /// resampled frame is not a coarser estimate but a softer image, which is a look change at their
    /// price point rather than a saving.
    pub render_scale: f32,
    /// Fraction of each emitter's authored `rate` and `burst` that is actually emitted. `1.0` emits
    /// the population the scene asked for; `0.25` emits a quarter of it.
    ///
    /// A *multiplier* that reads the ordinary way round, like
    /// [`render_scale`](QualityParams::render_scale): cheaper is smaller and `full`'s `1.0` is this
    /// axis's **ceiling**, because a machine's setting may only ever spend less than the authoring
    /// implies — emitting *more* particles than an emitter names would be a look change, and the
    /// look is content's to decide.
    ///
    /// **The one axis no shader and no [`masked`](crate::PostChain::masked) site reads, because this
    /// crate never sees it.** It is spent upstream, in the host's extract step, where an emitter's
    /// numbers become the particles a [`RenderScene`](crate::RenderScene) carries: the rate is
    /// scaled *before* the live window is computed, so a thinned emitter releases fewer particles
    /// rather than hiding some of the ones it released. What arrives here is an ordinary, shorter
    /// particle list, indistinguishable from a scene that authored a quieter plume — the same
    /// "cheaper equals unauthored" shape the effect toggles keep.
    ///
    /// It thins the *population* only: sizes, colours, lifetimes and blending stay the emitter's own
    /// at every preset, so a plume goes sparser rather than different. `low` is the only preset that
    /// spends it, because sparser is visible where the counts above are not. It is also the one axis
    /// the profile report's quality descriptor does not carry — the descriptor is wire-schema-hashed
    /// and the particle feature ships no schema revision — so a report names it through `preset`.
    pub particle_density: f32,
    /// Fraction of the **output** extent the SSR trace and its pyramids are sized at (not of the
    /// render extent: `render_scale` does not move it). Structure like
    /// [`shadow_map_size`](QualityParams::shadow_map_size) — a resource's extent is graph
    /// structure ([`PostChain::ssr`](crate::PostChain)). Cheaper is *smaller* and `full`'s value is the
    /// **ceiling**: a quality setting only ever spends less than the scene's authoring implies.
    pub ssr_scale: f32,
}

impl QualityParams {
    /// Every axis at the value the scene's own authoring implies — the ceiling, and what a machine
    /// that was never told a quality renders.
    ///
    /// The shadow map's 2048² and its 3×3 kernel are `Plan/shadow-mapping.md` decision 1 — the
    /// resolution the sun was authored against, and the one every committed golden is blessed at.
    /// [`gi_probe_scale`](QualityParams::gi_probe_scale) is `1.0` here for the same reason the
    /// counts are at their maxima: it is a *multiplier* on an authored length, so its ceiling is the
    /// value that leaves the authoring alone. So is
    /// [`render_scale`](QualityParams::render_scale) — `1.0` is native, every output pixel drawn
    /// once, and nothing above it is a *quality* setting. And so is
    /// [`particle_density`](QualityParams::particle_density): every emitter releases exactly the
    /// population it authored.
    pub const FULL: QualityParams = QualityParams {
        ssao_samples: 16,
        shaft_steps: 32,
        dof_taps: 32,
        shadow_map_size: 2048,
        sun_cascades: MAX_SUN_CASCADES,
        local_shadow_atlas_size: 4096,
        ibl_specular_size: 256,
        pcf: 3,
        gi_rays: 64,
        gi_probe_scale: 1.0,
        render_scale: 1.0,
        particle_density: 1.0,
        ssr_scale: 0.5,
    };

    /// A modest discount on the three modulated axes. Each estimator pairs a count-free *direction*
    /// sequence (Roberts' R2, the golden angle) with a *magnitude* term that divides by the count,
    /// so a shorter count keeps the same directions and re-spreads them over the same radius range
    /// or march length — a coarser estimate of the same integral, never a lopsided one. It is
    /// re-spreading rather than truncation, and `gi_bake.rs`'s
    /// `the_fibonacci_ray_set_is_a_whole_sphere_at_every_quality_ray_count` is where that
    /// distinction is measured.
    ///
    /// The sun's two axes stay at the ceiling: the first thing a halved shadow map costs is contact
    /// hardness, which is a *look* at this preset's price point rather than a saving. So does the
    /// probe lattice, for the sharper version of the same reason — coarser probes move where the
    /// bounce lands, and the discount here is meant to be a coarser *estimate* of the same picture.
    /// So does the render extent, which is the same judgement once more: a resampled frame is softer
    /// rather than coarser, and softness is a look. So does the particle population, for the
    /// sharpest version of it — a plume with a quarter of the particles is not a coarser estimate of
    /// the same effect but a thinner effect, and how full a plume reads is exactly what the emitter
    /// was authored to say.
    pub const HIGH: QualityParams = QualityParams {
        ssao_samples: 12,
        shaft_steps: 24,
        dof_taps: 20,
        shadow_map_size: QualityParams::FULL.shadow_map_size,
        sun_cascades: 3,
        local_shadow_atlas_size: QualityParams::FULL.local_shadow_atlas_size,
        ibl_specular_size: QualityParams::FULL.ibl_specular_size,
        pcf: 3,
        gi_rays: 48,
        gi_probe_scale: 1.0,
        render_scale: QualityParams::FULL.render_scale,
        particle_density: QualityParams::FULL.particle_density,
        ssr_scale: QualityParams::FULL.ssr_scale,
    };

    /// Roughly half of [`FULL`](QualityParams::FULL) on every modulated axis — the cheapest the
    /// effects run while still running. The shadow map is a quarter of the texels for the same
    /// reason, and its kernel stays 3×3: a single tap on a half-resolution map is a hard aliased
    /// edge, which is the one artefact the filter exists to remove.
    ///
    /// The frame itself stays native, and every plume stays as full as it was authored. Every other
    /// axis here spends a smaller budget on the same image;
    /// [`render_scale`](QualityParams::render_scale) spends it on a smaller image and
    /// [`particle_density`](QualityParams::particle_density) on a thinner one, and `low` is the
    /// preset willing to make either trade.
    pub const MEDIUM: QualityParams = QualityParams {
        ssao_samples: 8,
        shaft_steps: 16,
        dof_taps: 12,
        shadow_map_size: 1024,
        sun_cascades: 2,
        local_shadow_atlas_size: 2048,
        ibl_specular_size: QualityParams::FULL.ibl_specular_size,
        pcf: 3,
        gi_rays: 32,
        gi_probe_scale: 1.5,
        render_scale: QualityParams::FULL.render_scale,
        particle_density: QualityParams::FULL.particle_density,
        ssr_scale: QualityParams::FULL.ssr_scale,
    };

    /// [`Quality::LOW`] turns ambient occlusion, light shafts, depth of field and the sun's shadows
    /// off, so the six counts those effects read are never uploaded and never read here: no frame
    /// this preset renders can distinguish them from any other value — there is no shadow map to
    /// size and no kernel to walk. They repeat [`MEDIUM`](QualityParams::MEDIUM)'s rather than a
    /// sentinel so the descriptor a profile report carries stays a plain count at every preset —
    /// an axis that reads as `0` or `None` at one preset would have to be special-cased by every
    /// reader for a distinction the pixels do not make.
    ///
    /// **The two GI axes are the exception, and they carry real values.** `gi_dynamic: false` masks
    /// the per-frame *injection*, not the bake: a `GiVolume` scene still bakes its sky-visibility
    /// and bounce volumes at this preset, so both the ray count and the probe spacing land in
    /// pixels here exactly as they do at every other preset.
    /// **`render_scale` is the other exception, and the only preset to spend it.** The frame is
    /// drawn at three quarters of the output extent — 56% of the pixels — and resampled up, which
    /// discounts every pass the render extent sizes at once. It is the bluntest saving in the set,
    /// and blunt is what this preset is for.
    /// **`particle_density` is the third**, and the one saving here that is not a GPU-side one at
    /// all: a quarter of every emitter's `rate` and `burst` is a quarter of the particles the host
    /// evaluates, uploads and blends. A plume reads visibly sparser for it, which is why no preset
    /// above spends it and why this one may.
    pub const LOW: QualityParams = QualityParams {
        ibl_specular_size: 128,
        gi_rays: 16,
        gi_probe_scale: 2.0,
        render_scale: 0.75,
        particle_density: 0.25,
        ..QualityParams::MEDIUM
    };

    /// The parameters `name` selects, or `None` if it is not one of
    /// [`PRESET_NAMES`](Quality::PRESET_NAMES). Case-insensitive, for the reason
    /// [`Quality::preset`] is.
    pub fn preset(name: &str) -> Option<QualityParams> {
        match name.to_ascii_lowercase().as_str() {
            "low" => Some(QualityParams::LOW),
            "medium" => Some(QualityParams::MEDIUM),
            "high" => Some(QualityParams::HIGH),
            "full" => Some(QualityParams::FULL),
            _ => None,
        }
    }

    /// The cascade count a frame fits for a sun authoring `cascades`: `None` (the anchored box)
    /// when unauthored, else `min(cascades, sun_cascades)`, both clamped into
    /// `1..=`[`MAX_SUN_CASCADES`] (a Rust-built value is not validated).
    pub fn sun_cascade_count(&self, cascades: Option<u32>) -> Option<u32> {
        cascades.map(|authored| {
            let clamp = |count: u32| count.clamp(1, MAX_SUN_CASCADES);
            clamp(authored).min(clamp(self.sun_cascades))
        })
    }
}

impl Default for QualityParams {
    fn default() -> Self {
        QualityParams::FULL
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every name in the advertised list resolves, in any casing, and nothing else does — the list
    /// and the lookup cannot drift apart.
    #[test]
    fn every_advertised_preset_name_resolves_in_any_casing() {
        for name in Quality::PRESET_NAMES {
            let resolved = Quality::preset(name).expect("an advertised name must resolve");
            assert_eq!(Quality::preset(&name.to_uppercase()), Some(resolved));
            assert_eq!(
                Quality::preset(&format!("{}{}", &name[..1].to_uppercase(), &name[1..])),
                Some(resolved),
            );
        }
        for unknown in ["", "ultra", "lo", "low ", "cinematic"] {
            assert_eq!(
                Quality::preset(unknown),
                None,
                "`{unknown}` is not a preset name",
            );
        }
    }

    /// The `u32` value of `const <name>: u32 = <n>u;` in `source`, or `None` when the shader
    /// declares no such constant.
    fn shader_const(source: &str, name: &str) -> Option<u32> {
        let tail = source.split_once(&format!("const {name}: u32 = "))?.1;
        tail.split_once('u')?.0.trim().parse().ok()
    }

    /// `source` with every whole-line comment removed, so a scrape counts what the GPU runs and not
    /// what the prose around it says.
    fn shader_code(source: &str) -> String {
        source
            .lines()
            .filter(|line| !line.trim_start().starts_with("//"))
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// A modulated axis reaches its shader as a uniform field read at **every** site that uses the
    /// count — the loop bound and each normalizing divisor. A site left holding a literal would make
    /// a lowered preset take fewer samples than it divides by, which is not a cheaper estimate but a
    /// wrong one, and no golden covers it because `full` would still be exact.
    #[test]
    fn each_modulated_axis_is_a_uniform_its_shader_reads_at_every_site() {
        for (shader, source, field, sites) in [
            (
                "ssao.wgsl",
                include_str!("shaders/ssao.wgsl"),
                "ssao.samples",
                3,
            ),
            // Four: the plain march and the layered march each read the count as the loop bound and
            // as the step's divisor.
            (
                "shafts.wgsl",
                include_str!("shaders/shafts.wgsl"),
                "shafts.steps",
                4,
            ),
            // Two: the segment march reads the count as the loop bound and as the step's divisor.
            (
                "medium_scatter.wgsl",
                include_str!("shaders/medium_scatter.wgsl"),
                "scatter.steps",
                2,
            ),
            (
                "dof.wgsl",
                include_str!("shaders/dof.wgsl"),
                "dof_params.taps",
                3,
            ),
            // Nine: the anchored sun kernel, the cascade kernel and the local-light kernel each read
            // the count once as the loop's reach and twice more as the square that normalizes it.
            (
                "mesh_shading.wgsl",
                include_str!("shaders/snippets/mesh_shading.wgsl"),
                "sun.pcf",
                9,
            ),
            // Four: the march's loop bound, the ray set's elevation denominator, the hit cache's
            // stride, and the estimator's own `1/N`.
            (
                "gi_bake.wgsl",
                include_str!("shaders/gi_bake.wgsl"),
                "params.flags.y",
                4,
            ),
            // Two: the injection never loops the ray set — it replays a strided subset of it — so
            // the count reaches it only as the elevation denominator and the stride that recovers a
            // slot's ray index.
            (
                "gi_inject.wgsl",
                include_str!("shaders/gi_inject.wgsl"),
                "params.rays.x",
                2,
            ),
        ] {
            let code = shader_code(source);
            let found = code.matches(field).count();
            assert_eq!(
                found, sites,
                "`{shader}` reads `{field}` at {found} sites, not the {sites} the estimator has \
                 (its loop bound and every term of its normalizing divisor)",
            );
        }
        for (shader, source, name) in [
            (
                "ssao.wgsl",
                include_str!("shaders/ssao.wgsl"),
                "SSAO_SAMPLES",
            ),
            (
                "shafts.wgsl",
                include_str!("shaders/shafts.wgsl"),
                "SHAFT_STEPS",
            ),
            ("dof.wgsl", include_str!("shaders/dof.wgsl"), "DOF_TAPS"),
            // Both GI shaders lose *two* constants to the axis: the ray count itself, and the hit
            // cache's stride, which is now `gi_rays / HIT_RAYS` and so moves with it.
            ("gi_bake.wgsl", include_str!("shaders/gi_bake.wgsl"), "RAYS"),
            (
                "gi_bake.wgsl",
                include_str!("shaders/gi_bake.wgsl"),
                "CACHE_STRIDE",
            ),
            (
                "gi_inject.wgsl",
                include_str!("shaders/gi_inject.wgsl"),
                "RAYS",
            ),
            (
                "gi_inject.wgsl",
                include_str!("shaders/gi_inject.wgsl"),
                "CACHE_STRIDE",
            ),
        ] {
            assert_eq!(
                shader_const(source, name),
                None,
                "`{shader}` must not declare `const {name}`: the count is a quality setting the \
                 host supplies, and a literal beside the uniform is a second answer",
            );
        }
    }

    /// **The constraint on `gi_rays`'s permitted values.** The bake's hit cache holds
    /// [`HIT_RAYS`](crate::gi_bake::HIT_RAYS) slots at a stride of `gi_rays / HIT_RAYS`, so a count
    /// that is not a whole multiple of that budget strides fractionally and the cached slots stop
    /// spanning the sphere evenly. Every preset must satisfy it, and `full` is the ceiling.
    ///
    /// `gi_probe_scale` reads the other way, and this states that too: it multiplies a *spacing*, so
    /// `full`'s `1.0` is its minimum and a cheaper preset asks for more.
    #[test]
    fn every_presets_gi_axes_satisfy_their_own_constraints() {
        for (label, params) in [
            ("low", QualityParams::LOW),
            ("medium", QualityParams::MEDIUM),
            ("high", QualityParams::HIGH),
            ("full", QualityParams::FULL),
        ] {
            let rays = params.gi_rays;
            assert_eq!(
                rays % crate::gi_bake::HIT_RAYS,
                0,
                "`{label}` casts {rays} rays, which strides the {}-slot hit cache fractionally",
                crate::gi_bake::HIT_RAYS,
            );
            assert!(
                (crate::gi_bake::HIT_RAYS..=QualityParams::FULL.gi_rays).contains(&rays),
                "`{label}` casts {rays} rays, outside {}..={}",
                crate::gi_bake::HIT_RAYS,
                QualityParams::FULL.gi_rays,
            );
            assert!(
                params.gi_probe_scale >= QualityParams::FULL.gi_probe_scale,
                "`{label}` scales the authored probe spacing by {}, below `full`'s {} — this axis \
                 is a multiplier on a length, so `full` is its floor",
                params.gi_probe_scale,
                QualityParams::FULL.gi_probe_scale,
            );
        }
        assert_eq!(
            QualityParams::FULL.gi_probe_scale,
            1.0,
            "`full` bakes at exactly the spacing the scene authored",
        );
    }

    /// An unauthored sun stays the anchored box at every quality; an authored count is capped by the
    /// preset, and a Rust-built value outside `1..=MAX_SUN_CASCADES` is clamped rather than trusted.
    #[test]
    fn the_sun_cascade_count_is_the_authored_count_capped_by_the_quality() {
        for params in [QualityParams::FULL, QualityParams::MEDIUM] {
            assert_eq!(params.sun_cascade_count(None), None);
        }
        assert_eq!(QualityParams::FULL.sun_cascade_count(Some(4)), Some(4));
        assert_eq!(QualityParams::HIGH.sun_cascade_count(Some(4)), Some(3));
        assert_eq!(QualityParams::MEDIUM.sun_cascade_count(Some(4)), Some(2));
        assert_eq!(QualityParams::MEDIUM.sun_cascade_count(Some(1)), Some(1));
        assert_eq!(QualityParams::FULL.sun_cascade_count(Some(9)), Some(4));
        let zero_cap = QualityParams {
            sun_cascades: 0,
            ..QualityParams::FULL
        };
        assert_eq!(zero_cap.sun_cascade_count(Some(3)), Some(1));
    }

    /// The presets are a ladder: no axis ever costs more at a cheaper preset. Two of the four share
    /// their [`Quality`] toggles with `FULL`, so this ordering is the only thing that makes them
    /// distinct — the strict half below is what keeps the assertion from passing vacuously.
    #[test]
    fn the_presets_are_ordered_cheapest_first() {
        let ladder = [
            ("low", QualityParams::LOW),
            ("medium", QualityParams::MEDIUM),
            ("high", QualityParams::HIGH),
            ("full", QualityParams::FULL),
        ];
        for pair in ladder.windows(2) {
            let [(cheaper, low), (dearer, high)] = pair else {
                unreachable!("windows(2) yields pairs")
            };
            for (axis, a, b) in [
                ("ssao_samples", low.ssao_samples, high.ssao_samples),
                ("shaft_steps", low.shaft_steps, high.shaft_steps),
                ("dof_taps", low.dof_taps, high.dof_taps),
                ("shadow_map_size", low.shadow_map_size, high.shadow_map_size),
                ("sun_cascades", low.sun_cascades, high.sun_cascades),
                (
                    "local_shadow_atlas_size",
                    low.local_shadow_atlas_size,
                    high.local_shadow_atlas_size,
                ),
                (
                    "ibl_specular_size",
                    low.ibl_specular_size,
                    high.ibl_specular_size,
                ),
                ("pcf", low.pcf, high.pcf),
                ("gi_rays", low.gi_rays, high.gi_rays),
            ] {
                assert!(
                    a <= b,
                    "`{axis}` is {a} at `{cheaper}` and {b} at `{dearer}`: a cheaper preset must \
                     never cost more",
                );
            }
            assert!(
                low.gi_probe_scale >= high.gi_probe_scale,
                "a coarser probe spacing is the cheaper one ({cheaper} vs {dearer})",
            );
            assert!(
                low.render_scale <= high.render_scale,
                "a cheaper preset must never draw more pixels ({cheaper} vs {dearer})",
            );
            assert!(
                low.ssr_scale <= high.ssr_scale,
                "a cheaper preset must never trace more pixels ({cheaper} vs {dearer})",
            );
            assert!(
                low.particle_density <= high.particle_density,
                "a cheaper preset must never emit more particles ({cheaper} vs {dearer})",
            );
        }
        // The strict half: every modulated axis genuinely moves across the three presets that render
        // the effects, so nothing here is an alias wearing a second name.
        for (medium, high, full) in [
            (
                QualityParams::MEDIUM.ssao_samples,
                QualityParams::HIGH.ssao_samples,
                QualityParams::FULL.ssao_samples,
            ),
            (
                QualityParams::MEDIUM.shaft_steps,
                QualityParams::HIGH.shaft_steps,
                QualityParams::FULL.shaft_steps,
            ),
            (
                QualityParams::MEDIUM.dof_taps,
                QualityParams::HIGH.dof_taps,
                QualityParams::FULL.dof_taps,
            ),
            (
                QualityParams::MEDIUM.gi_rays,
                QualityParams::HIGH.gi_rays,
                QualityParams::FULL.gi_rays,
            ),
            (
                QualityParams::MEDIUM.sun_cascades,
                QualityParams::HIGH.sun_cascades,
                QualityParams::FULL.sun_cascades,
            ),
        ] {
            assert!(medium < high && high < full, "{medium} < {high} < {full}");
        }
        // The sun's own axis moves too — between `medium` and `high` rather than across all three,
        // which is the whole reason that pair is not an alias.
        let maps: Vec<u32> = ladder
            .iter()
            .map(|(_, params)| params.shadow_map_size)
            .collect();
        assert!(
            maps[1] < maps[2],
            "a cheaper preset must draw the sun into a smaller map: {maps:?}",
        );
        let atlases: Vec<u32> = ladder
            .iter()
            .map(|(_, params)| params.local_shadow_atlas_size)
            .collect();
        assert!(
            atlases[1] < atlases[2],
            "a cheaper preset must draw local lights into a smaller atlas: {atlases:?}",
        );
        // The GI axes are the only two that also move at `low`, because `Quality::LOW` masks the
        // injection and not the bake — a `GiVolume` scene still pays for both there, so a preset
        // repeating `medium`'s numbers would be describing work it does not do.
        let rays: Vec<u32> = ladder.iter().map(|(_, params)| params.gi_rays).collect();
        assert!(
            rays[0] < rays[1],
            "a cheaper preset must trace fewer rays even at `low`: {rays:?}",
        );
        let scales: Vec<f32> = ladder
            .iter()
            .map(|(_, params)| params.gi_probe_scale)
            .collect();
        assert!(
            scales[0] > scales[1] && scales[1] > scales[2],
            "a coarser probe spacing is the cheaper one, and it moves across the bottom three \
             presets: {scales:?}",
        );
    }

    /// **The resolution axis, and the one preset that spends it.** `render_scale` reads the ordinary
    /// way round — cheaper is smaller — so `full`'s `1.0` is its *ceiling*, unlike `gi_probe_scale`
    /// beside it, and nothing here may ask to draw more pixels than the output has. `low` is
    /// deliberately alone below native: the saving is the bluntest in the set (it goes with the
    /// square of the factor and discounts every pass the render extent sizes at once), and the two
    /// presets in between are meant to be coarser estimates of the same image rather than softer
    /// ones.
    #[test]
    fn only_the_cheapest_preset_draws_below_the_output_extent() {
        let ladder = [
            ("low", QualityParams::LOW),
            ("medium", QualityParams::MEDIUM),
            ("high", QualityParams::HIGH),
            ("full", QualityParams::FULL),
        ];
        for (label, params) in ladder {
            // The floor is asked of `RenderScale` itself rather than restated here: a preset naming
            // a factor the resolution type would refuse is a preset that resolves and then cannot
            // be rendered, and there is no second opinion about where that floor is.
            let scale = params.render_scale;
            assert!(
                crate::RenderScale::new(scale).is_ok() && scale <= QualityParams::FULL.render_scale,
                "`{label}` renders at {scale}× the output extent, outside {}..={}",
                crate::RenderScale::MIN,
                QualityParams::FULL.render_scale,
            );
        }
        assert_eq!(
            QualityParams::FULL.render_scale,
            1.0,
            "`full` draws every output pixel natively: this axis's ceiling, not its floor — \
             supersampling above it is the host's own `RenderScale` and not a machine's quality",
        );
        let below: Vec<&str> = ladder
            .iter()
            .filter(|(_, params)| params.render_scale < QualityParams::FULL.render_scale)
            .map(|(label, _)| *label)
            .collect();
        assert_eq!(
            below,
            ["low"],
            "exactly one preset draws below the output extent",
        );
        assert_eq!(QualityParams::LOW.render_scale, 0.75);
    }

    /// **The particle axis, and the one preset that spends it.** `particle_density` scales an
    /// emitter's authored `rate` and `burst`, so it reads the way `render_scale` does — cheaper is
    /// smaller and `full`'s `1.0` is its *ceiling*, since a machine asking for more particles than
    /// the emitter names would be changing the look rather than paying less for it. `low` is alone
    /// below the ceiling: a thinner plume is visible where a shorter sample loop is not, which makes
    /// it a saving only the preset that has already accepted a resampled frame can afford.
    #[test]
    fn only_the_cheapest_preset_thins_the_particle_population() {
        let ladder = [
            ("low", QualityParams::LOW),
            ("medium", QualityParams::MEDIUM),
            ("high", QualityParams::HIGH),
            ("full", QualityParams::FULL),
        ];
        for (label, params) in ladder {
            let density = params.particle_density;
            assert!(
                (0.0..=QualityParams::FULL.particle_density).contains(&density),
                "`{label}` emits {density}× the authored population, outside 0.0..={}",
                QualityParams::FULL.particle_density,
            );
        }
        assert_eq!(
            QualityParams::FULL.particle_density,
            1.0,
            "`full` emits exactly the population every emitter authored: this axis's ceiling, not \
             its floor — a denser plume is the emitter's own to ask for and never a machine's",
        );
        let thinned: Vec<&str> = ladder
            .iter()
            .filter(|(_, params)| params.particle_density < QualityParams::FULL.particle_density)
            .map(|(label, _)| *label)
            .collect();
        assert_eq!(thinned, ["low"], "exactly one preset thins the plumes");
        assert_eq!(QualityParams::LOW.particle_density, 0.25);
    }

    /// A PCF kernel is a *centred* square, so its edge length is odd at every preset — an even edge
    /// would average an off-centre neighbourhood and slide every shadow by half a texel.
    #[test]
    fn every_presets_pcf_kernel_is_odd() {
        for (label, params) in [
            ("low", QualityParams::LOW),
            ("medium", QualityParams::MEDIUM),
            ("high", QualityParams::HIGH),
            ("full", QualityParams::FULL),
        ] {
            let pcf = params.pcf;
            assert!(pcf % 2 == 1, "`{label}` centres a {pcf}x{pcf} kernel");
        }
    }

    /// Both preset lookups accept exactly the advertised names, so a host that resolves a setting
    /// can take the toggles and the parameters from the same string without a second check.
    #[test]
    fn the_two_preset_lookups_accept_the_same_names() {
        let params = [
            QualityParams::LOW,
            QualityParams::MEDIUM,
            QualityParams::HIGH,
            QualityParams::FULL,
        ];
        let toggles = [Quality::LOW, Quality::MEDIUM, Quality::HIGH, Quality::FULL];
        for ((name, expected), toggle) in Quality::PRESET_NAMES.into_iter().zip(params).zip(toggles)
        {
            assert_eq!(QualityParams::preset(name), Some(expected), "`{name}`");
            assert_eq!(
                QualityParams::preset(&name.to_uppercase()),
                Some(expected),
                "`{name}` upper-cased"
            );
            assert_eq!(Quality::preset(name), Some(toggle), "`{name}` toggles");
        }
        for name in Quality::PRESET_NAMES {
            assert!(
                QualityParams::preset(name).is_some(),
                "`{name}` resolves to parameters as well as toggles",
            );
            assert_eq!(
                QualityParams::preset(&name.to_uppercase()),
                QualityParams::preset(name),
            );
        }
        for unknown in ["", "ultra", "lo", "low ", "cinematic"] {
            assert_eq!(QualityParams::preset(unknown), None);
        }
        assert_eq!(QualityParams::default(), QualityParams::FULL);
    }

    /// The two ends of the scale: the default is `FULL` and turns nothing off, `LOW` turns every
    /// optional effect off.
    #[test]
    fn the_default_is_full_and_low_is_every_effect_off() {
        assert_eq!(Quality::default(), Quality::FULL);
        assert_eq!(
            Quality::FULL,
            Quality {
                ssao: true,
                shafts: true,
                dof: true,
                gi_dynamic: true,
                shadows: true,
                ssr: true,
            },
        );
        assert_eq!(
            Quality::LOW,
            Quality {
                ssao: false,
                shafts: false,
                dof: false,
                gi_dynamic: false,
                shadows: false,
                ssr: false,
            },
        );
    }
}
