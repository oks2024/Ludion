//! The host-side quality chain: how a machine's render quality is chosen, and where that choice is
//! resolved.
//!
//! [`Quality`] itself is plain data in `ludion-renderer` — which effects to draw. This module is the
//! half that reads a machine's sources and turns them into one resolved [`QualitySetting`]: a host's
//! `--quality` flag, else [`QUALITY_ENV`], else everything the scene authors.
//!
//! **Quality is a per-machine knob, exactly as `LUDION_THREADS` is** (`ludion devdocs determinism`):
//! how much GPU a box is willing to spend is a property of the box, not of the project, so it lives
//! in the environment rather than in any committed file — and a host's flag beats the variable
//! outright, without validating it, so a flag still works on a machine whose `LUDION_QUALITY` is
//! malformed. Nothing about the simulation depends on it: the `WorldHash` is identical at every
//! quality.
//!
//! **A malformed setting refuses to start** ([`code::QUALITY_SETTING_INVALID`], `L0016`), rather than
//! falling back to a default. A silently-ignored setting means rendering at a quality nobody chose,
//! which is the one outcome the knob exists to prevent.
//!
//! A setting's value is either a preset name ([`Quality::PRESET_NAMES`]) or **the path to a TOML
//! override file** — there is no third form, so anything that is not a preset is read as a path. An
//! override file is a base preset plus per-effect and per-parameter overrides, every key optional:
//!
//! ```toml
//! preset = "medium"
//!
//! [effects]
//! ssao = false
//! shafts = true
//! dof = false
//! gi_dynamic = false
//! shadows = true
//! ssr = false
//!
//! [params]
//! ssao_samples = 8
//! shaft_steps = 16
//! dof_taps = 12
//! shadow_map_size = 1024
//! sun_cascades = 2
//! local_shadow_atlas_size = 2048
//! ibl_specular_size = 128
//! pcf = 1
//! gi_rays = 32
//! gi_probe_scale = 1.5
//! render_scale = 0.75
//! particle_density = 0.25
//! ssr_scale = 0.25
//! ```
//!
//! `[params]` names every axis a preset modulates. Each count is bounded by its own
//! [`QualityParams::FULL`] value: `full` is the ceiling, so a quality setting only ever goes down.
//! **`gi_probe_scale` is the one axis that reads the other way** — it multiplies an authored
//! *length* rather than naming a count, so cheaper is larger and `full`'s `1.0` is its floor.
//! `render_scale` is a multiplier too but reads with the counts: it is the fraction of the output
//! extent the frame is drawn at, so cheaper is smaller and `full`'s `1.0` is its ceiling. Rendering
//! *above* the output extent is the host's own `--scale`, not a machine's quality — and where the
//! two meet, [`resolve_render_scale`] settles it: an explicit scale beats a quality-supplied one,
//! which beats native. `particle_density` is the third multiplier and reads the same way round: it
//! is the fraction of every emitter's authored `rate` and `burst` that is actually emitted, so
//! `full`'s `1.0` is its ceiling too — a fuller plume is the emitter's own to author, never a
//! machine's to ask for.
//! `ssr_scale` is the fourth and reads the same way: the reflection trace's extent as a fraction of
//! the output extent, so cheaper is smaller and `full`'s `0.5` is its ceiling.
//!
//! Unknown keys and unknown sections are rejected, not ignored: a file written against a build that
//! knows more effects than this one must fail loudly here rather than render at a quality it does not
//! describe.
//!
//! [`resolve_startup_quality`] is the only function here that reads the process environment;
//! everything under it is pure over its inputs, so the precedence and the parsing are testable
//! without mutating the environment (which edition 2024 makes `unsafe`).

use std::fmt;
use std::path::Path;

use ludion_core::codes::code;
use ludion_core::diagnostics::Diagnostic;
use ludion_protocol::perf::QualityProfile;
use ludion_renderer::{Quality, QualityParams, RenderScale};
use serde::Deserialize;

/// The environment variable naming a machine's render quality: a preset name, or the path to a TOML
/// override file. Unset (or empty) means [`Quality::FULL`].
///
/// The per-machine knob — see the module docs. A host's `--quality` flag beats it.
pub const QUALITY_ENV: &str = "LUDION_QUALITY";

/// A resolved quality setting: the effect toggles a renderer applies, the parameters it applies them
/// at, and the name of the choice that produced both.
///
/// The label is not decoration. Several presets resolve to the same toggles, so a [`Quality`] alone
/// cannot say which one a machine asked for — and a host that echoes its settings, or a report that
/// records them, must name what was chosen rather than what it happened to equal. It is the preset
/// name as typed and lowercased, or [`CUSTOM_LABEL`](QualitySetting::CUSTOM_LABEL) for an override
/// file.
#[derive(Debug, Clone, PartialEq)]
pub struct QualitySetting {
    /// Which optional effects this machine renders.
    pub quality: Quality,
    /// How hard each of those effects works.
    pub params: QualityParams,
    /// The name of the choice: a preset name, or [`CUSTOM_LABEL`](QualitySetting::CUSTOM_LABEL).
    pub label: String,
}

impl QualitySetting {
    /// The label an override file resolves to — a file's toggles are its own, so no preset name
    /// describes them.
    pub const CUSTOM_LABEL: &'static str = "custom";

    /// The label of the setting that renders everything a scene authors — what a machine that was
    /// never told anything resolves to, and the only setting a calibrated budget describes.
    pub const FULL_LABEL: &'static str = "full";

    /// Everything the scene authors, at every axis's ceiling, labelled `"full"`.
    ///
    /// Named rather than left to [`Default`] because callers that must not vary with a machine — the
    /// golden harness above all — say so at the call site, and a `default()` there would read as an
    /// unconsidered choice.
    pub fn full() -> QualitySetting {
        QualitySetting {
            quality: Quality::FULL,
            params: QualityParams::FULL,
            label: QualitySetting::FULL_LABEL.to_string(),
        }
    }

    /// The local-light shadow atlas edge a frame at this setting plans with: the quality's axis, or 0
    /// when it masks shadows or is not a valid edge. The one rule the profile panel and the
    /// `local_shadows` marks share, and the one `PostChain::masked` applies.
    pub(crate) fn local_shadow_edge(&self) -> u32 {
        if self.quality.shadows
            && ludion_renderer::is_valid_atlas_edge(self.params.local_shadow_atlas_size)
        {
            self.params.local_shadow_atlas_size
        } else {
            0
        }
    }

    /// This setting as the descriptor a [`ProfileReport`](ludion_protocol::perf::ProfileReport)
    /// carries: the label, the toggles it resolved to, and the parameters it resolved to.
    ///
    /// Every field is read from the resolved setting rather than restated, so a report can never
    /// claim a sample count the GPU did not run.
    ///
    /// # Example
    /// ```
    /// use ludion_engine::{QualitySetting, resolve_startup_quality};
    ///
    /// let descriptor = QualitySetting::full().descriptor();
    /// assert_eq!(descriptor.preset, "full");
    /// assert_eq!(descriptor.ssao_samples, 16);
    ///
    /// let medium = resolve_startup_quality(Some("medium"), None).unwrap().descriptor();
    /// assert_eq!(medium.ssao_samples, 8);
    /// ```
    pub fn descriptor(&self) -> QualityProfile {
        let params = self.params;
        QualityProfile {
            preset: self.label.clone(),
            ssao: self.quality.ssao,
            shafts: self.quality.shafts,
            dof: self.quality.dof,
            gi_dynamic: self.quality.gi_dynamic,
            shadows: self.quality.shadows,
            ssr: self.quality.ssr,
            ssao_samples: params.ssao_samples,
            shaft_steps: params.shaft_steps,
            dof_taps: params.dof_taps,
            shadow_map_size: params.shadow_map_size,
            local_shadow_atlas_size: params.local_shadow_atlas_size,
            sun_cascades: params.sun_cascades,
            pcf: params.pcf,
            gi_rays: params.gi_rays,
            gi_probe_scale: params.gi_probe_scale,
            render_scale: params.render_scale,
            ssr_scale: params.ssr_scale,
            ibl_specular_size: params.ibl_specular_size,
        }
    }
}

impl Default for QualitySetting {
    /// [`QualitySetting::full`] — what a machine that was never told anything renders.
    fn default() -> Self {
        QualitySetting::full()
    }
}

/// The factor a frame is drawn at on this host: `explicit` when the host named one, else the one
/// `setting` carries, else — since `full`'s is `1.0` — native.
///
/// Precedence is the shape [`resolve_startup_quality`] already uses for the setting itself: what a
/// host said outright beats what a machine implied. It is the same judgement, too — a `--scale` is
/// asked for one run at a time and usually to *supersample*, which no quality may do, so a machine
/// that renders below native still renders exactly what it was told to when it is told.
///
/// **The two sources are resolved here rather than merged when either is set**, which is what makes
/// the result independent of the order a host sets them in: neither value is ever written over the
/// other, so `set_quality` then `set_render_scale` and the reverse leave the same frame extent.
///
/// # Panics
/// If `setting` carries a `render_scale` outside [`RenderScale`]'s own range. Every producer of one
/// checks it against that same range — the presets by `QualityParams`'s tests, an override file by
/// this module's `L0016` — and `every_presets_render_scale_is_one_the_resolution_type_accepts` holds
/// the two ranges equal.
///
/// # Example
/// ```
/// use ludion_engine::{RenderScale, resolve_render_scale, resolve_startup_quality};
///
/// let low = resolve_startup_quality(Some("low"), None).unwrap();
/// assert_eq!(resolve_render_scale(None, &low).factor(), 0.75);
///
/// // An explicit scale wins, in either direction.
/// let supersampled = RenderScale::new(2.0).unwrap();
/// assert_eq!(resolve_render_scale(Some(supersampled), &low), supersampled);
/// ```
pub fn resolve_render_scale(
    explicit: Option<RenderScale>,
    setting: &QualitySetting,
) -> RenderScale {
    explicit.unwrap_or_else(|| {
        RenderScale::new(setting.params.render_scale).expect(
            "a setting's `render_scale` is checked against this very range where it is made",
        )
    })
}

/// Resolves a host process's render quality once at startup.
///
/// Precedence is `explicit` (a `--quality` flag) → [`QUALITY_ENV`] → `packaged` (the default a
/// packaged build carries, such as an iOS export's `[ios] quality`) → [`Quality::FULL`]. A source
/// wins outright and the ones below it are not even read for validity, so a flag still works on a
/// box whose `LUDION_QUALITY` is malformed. A value that is absent, empty, or all whitespace counts
/// as not chosen at all, from any source.
///
/// # Errors
/// One `L0016` [`Diagnostic`] when the winning value is neither a preset name nor a usable override
/// file. Hosts print it and refuse to start; the alternative is a process rendering at a quality
/// nobody chose.
///
/// # Example
/// ```
/// use ludion_engine::{Quality, resolve_startup_quality};
///
/// // A host's flag wins outright, in any casing, and keeps its name.
/// let chosen = resolve_startup_quality(Some("Low"), None).unwrap();
/// assert_eq!(chosen.quality, Quality::LOW);
/// assert_eq!(chosen.label, "low");
///
/// // Anything that is not a preset is a file path — and refuses to start when it is not one either.
/// let rejected = resolve_startup_quality(Some("medum"), None).unwrap_err();
/// assert_eq!(rejected.code.as_str(), "L0016");
/// ```
pub fn resolve_startup_quality(
    explicit: Option<&str>,
    packaged: Option<&str>,
) -> Result<QualitySetting, Diagnostic> {
    resolve_setting(
        explicit,
        std::env::var(QUALITY_ENV).ok().as_deref(),
        packaged,
    )
}

/// The pure half of [`resolve_startup_quality`]: the same precedence over an explicitly supplied
/// environment value, so the chain is testable without mutating the process environment.
fn resolve_setting(
    explicit: Option<&str>,
    env: Option<&str>,
    packaged: Option<&str>,
) -> Result<QualitySetting, Diagnostic> {
    if let Some(value) = chosen(explicit) {
        return resolve_value(value, Origin::Flag);
    }
    if let Some(value) = chosen(env) {
        return resolve_value(value, Origin::Env);
    }
    match chosen(packaged) {
        Some(value) => resolve_value(value, Origin::Packaged),
        None => Ok(QualitySetting::default()),
    }
}

/// The trimmed value a source actually chose: `None` for absent, empty, or all-whitespace.
fn chosen(raw: Option<&str>) -> Option<&str> {
    raw.map(str::trim).filter(|value| !value.is_empty())
}

/// Resolves one value: a preset name, else the override file it names.
fn resolve_value(value: &str, origin: Origin) -> Result<QualitySetting, Diagnostic> {
    if let Some(quality) = Quality::preset(value) {
        return Ok(QualitySetting {
            quality,
            params: QualityParams::preset(value)
                .expect("the two lookups accept the same names, held by a test"),
            label: value.to_ascii_lowercase(),
        });
    }
    let path = Path::new(value);
    let text = std::fs::read_to_string(path).map_err(|error| {
        invalid(format!(
            "{origin} is `{value}`, which is neither a quality preset ({}) nor a readable TOML \
             file: {error}",
            preset_list(),
        ))
    })?;
    let (quality, params) = parse_override_file(&text, path)?;
    Ok(QualitySetting {
        quality,
        params,
        label: QualitySetting::CUSTOM_LABEL.to_string(),
    })
}

/// Which source a value came from, so the diagnostic says where to go and fix it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Origin {
    Flag,
    Env,
    Packaged,
}

impl fmt::Display for Origin {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Origin::Flag => write!(f, "the `--quality` setting"),
            Origin::Env => write!(f, "`{QUALITY_ENV}`"),
            Origin::Packaged => write!(f, "the build's packaged quality"),
        }
    }
}

/// An override file's document: a base preset plus per-effect and per-parameter overrides. Every key
/// is optional and unknown ones are refused (see the module docs).
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct OverrideFile {
    #[serde(default)]
    preset: Option<String>,
    #[serde(default)]
    effects: Option<Effects>,
    #[serde(default)]
    params: Option<Params>,
}

/// The `[effects]` table: each toggle overrides the base preset's, and an absent one keeps it.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Effects {
    #[serde(default)]
    ssao: Option<bool>,
    #[serde(default)]
    shafts: Option<bool>,
    #[serde(default)]
    dof: Option<bool>,
    #[serde(default)]
    gi_dynamic: Option<bool>,
    #[serde(default)]
    shadows: Option<bool>,
    #[serde(default)]
    ssr: Option<bool>,
}

/// The `[params]` table: every axis a preset modulates. Each overrides the base preset's, and an
/// absent one keeps it.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Params {
    #[serde(default)]
    ssao_samples: Option<u32>,
    #[serde(default)]
    shaft_steps: Option<u32>,
    #[serde(default)]
    dof_taps: Option<u32>,
    #[serde(default)]
    shadow_map_size: Option<u32>,
    #[serde(default)]
    sun_cascades: Option<u32>,
    #[serde(default)]
    local_shadow_atlas_size: Option<u32>,
    #[serde(default)]
    ibl_specular_size: Option<u32>,
    #[serde(default)]
    pcf: Option<u32>,
    #[serde(default)]
    gi_rays: Option<u32>,
    #[serde(default)]
    gi_probe_scale: Option<f32>,
    #[serde(default)]
    render_scale: Option<f32>,
    #[serde(default)]
    particle_density: Option<f32>,
    #[serde(default)]
    ssr_scale: Option<f32>,
}

/// How many rays the GI bake's hit cache holds per probe, and therefore the granularity `gi_rays`
/// must respect: the cache takes every `gi_rays / GI_HIT_RAYS`'th ray, so a count that is not a whole
/// multiple of it strides fractionally and the cached directions stop spanning the sphere.
///
/// Restated here rather than exported from `ludion-renderer`, which keeps it crate-private beside
/// the buffer it sizes; `the_hit_cache_granularity_matches_the_renderers` holds the two together.
const GI_HIT_RAYS: u32 = 16;

/// The largest [`QualityParams::gi_probe_scale`] a setting may ask for.
///
/// Probe count falls as the cube of this, so `4.0` already bakes a sixty-fourth of `full`'s probes —
/// past that a room-sized volume collapses to the two-probe-per-axis floor and the lattice stops
/// describing the room at all. It is also where the authoring rule "`probe_spacing` ≤ the thinnest
/// occluder" (`ludion docs global-illumination`) stops being satisfiable for ordinary wall
/// thicknesses, so a larger factor buys frame time by turning leak control off rather than by
/// estimating more coarsely. The shipped `low` sits at `2.0`, comfortably inside it.
const GI_PROBE_SCALE_MAX: f32 = 4.0;

/// Parses an override file's text into the quality it describes. Pure over the text; `path` names
/// the file in diagnostics.
fn parse_override_file(text: &str, path: &Path) -> Result<(Quality, QualityParams), Diagnostic> {
    let document: OverrideFile = toml::from_str(text).map_err(|error| {
        invalid(format!(
            "`{}` is not a valid quality override file: {error}",
            path.display(),
        ))
    })?;
    let (mut quality, mut params) = match document.preset.as_deref() {
        Some(name) => Quality::preset(name)
            .zip(QualityParams::preset(name))
            .ok_or_else(|| {
                invalid(format!(
                    "`{}` names the preset `{name}`, which is not one of {}",
                    path.display(),
                    preset_list(),
                ))
            })?,
        None => (Quality::FULL, QualityParams::FULL),
    };
    if let Some(effects) = document.effects {
        quality.ssao = effects.ssao.unwrap_or(quality.ssao);
        quality.shafts = effects.shafts.unwrap_or(quality.shafts);
        quality.dof = effects.dof.unwrap_or(quality.dof);
        quality.gi_dynamic = effects.gi_dynamic.unwrap_or(quality.gi_dynamic);
        quality.shadows = effects.shadows.unwrap_or(quality.shadows);
        quality.ssr = effects.ssr.unwrap_or(quality.ssr);
    }
    if let Some(overrides) = document.params {
        for (key, value, axis, ceiling) in [
            (
                "ssao_samples",
                overrides.ssao_samples,
                &mut params.ssao_samples,
                QualityParams::FULL.ssao_samples,
            ),
            (
                "shaft_steps",
                overrides.shaft_steps,
                &mut params.shaft_steps,
                QualityParams::FULL.shaft_steps,
            ),
            (
                "dof_taps",
                overrides.dof_taps,
                &mut params.dof_taps,
                QualityParams::FULL.dof_taps,
            ),
            (
                "shadow_map_size",
                overrides.shadow_map_size,
                &mut params.shadow_map_size,
                QualityParams::FULL.shadow_map_size,
            ),
            (
                "sun_cascades",
                overrides.sun_cascades,
                &mut params.sun_cascades,
                QualityParams::FULL.sun_cascades,
            ),
            (
                "local_shadow_atlas_size",
                overrides.local_shadow_atlas_size,
                &mut params.local_shadow_atlas_size,
                QualityParams::FULL.local_shadow_atlas_size,
            ),
            (
                "ibl_specular_size",
                overrides.ibl_specular_size,
                &mut params.ibl_specular_size,
                QualityParams::FULL.ibl_specular_size,
            ),
            (
                "pcf",
                overrides.pcf,
                &mut params.pcf,
                QualityParams::FULL.pcf,
            ),
            (
                "gi_rays",
                overrides.gi_rays,
                &mut params.gi_rays,
                QualityParams::FULL.gi_rays,
            ),
        ] {
            let Some(value) = value else { continue };
            // Each axis is bounded below by the one sample an estimator needs to mean anything, and
            // above by what the scene's own authoring implies: `full` is the ceiling, and a setting
            // that could exceed it would stop being a *quality* knob and start being a look one.
            if !(1..=ceiling).contains(&value) {
                return Err(invalid(format!(
                    "`{}` sets `{key}` to {value}, which is outside 1..={ceiling}",
                    path.display(),
                )));
            }
            *axis = value;
        }
        // Two axes carry a constraint of their own beyond the range, and each gets its own sentence
        // because they are different mistakes. Both are checked after the range above, so `0` is
        // reported as out of range rather than as the shape error it also is.
        //
        // A PCF kernel is a square *centred* on the shaded texel, so an even edge has no centre and
        // would slide every shadow half a texel.
        if let Some(pcf) = overrides.pcf.filter(|pcf| pcf % 2 == 0) {
            return Err(invalid(format!(
                "`{}` sets `pcf` to {pcf}, which is even: the kernel is a square centred on the \
                 shaded texel, so its edge length in taps must be odd",
                path.display(),
            )));
        }
        // The sky cube's mip chain halves down to 8x8, so a face edge that is not a power of two
        // would leave its last mips a texel short of the lod range the shader is told.
        if let Some(size) = overrides.ibl_specular_size.filter(|s| !s.is_power_of_two()) {
            return Err(invalid(format!(
                "`{}` sets `ibl_specular_size` to {size}, which is not a power of two: the specular \
                 cube's mips halve down to 8x8, and the shader's lod range counts them",
                path.display(),
            )));
        }
        if let Some(size) = overrides.ibl_specular_size.filter(|s| *s < 16) {
            return Err(invalid(format!(
                "`{}` sets `ibl_specular_size` to {size}, below 16: the specular chain stops at an \
                 8x8 face and needs at least two mips",
                path.display(),
            )));
        }
        // The local-light atlas is an 8x8 grid of cells and a spot's tile is 2x2 of them, so an
        // edge that does not divide by 16 would cut a tile across a texel.
        if let Some(size) = overrides
            .local_shadow_atlas_size
            .filter(|size| size % 16 != 0)
        {
            return Err(invalid(format!(
                "`{}` sets `local_shadow_atlas_size` to {size}, which is not a multiple of 16: the \
                 atlas is an 8x8 grid of cells whose spot tiles are 2x2, so its edge must divide by 16",
                path.display(),
            )));
        }
        // The GI bake's hit cache holds a fixed sixteen slots at a stride of `gi_rays / 16`, so a
        // count that is not a whole multiple of that budget files them into a polar cap instead of
        // over the sphere.
        if let Some(rays) = overrides.gi_rays.filter(|rays| rays % GI_HIT_RAYS != 0) {
            return Err(invalid(format!(
                "`{}` sets `gi_rays` to {rays}, which is not a multiple of {GI_HIT_RAYS}: the \
                 bake's hit cache records every `gi_rays / {GI_HIT_RAYS}`'th ray, and a fractional \
                 stride crowds those directions into one pole instead of spanning the sphere",
                path.display(),
            )));
        }
        // The one axis that is a multiplier on an authored length rather than a count: cheaper is
        // *larger*, so `full`'s 1.0 is its floor and the ceiling is a separate judgement about where
        // a coarser lattice stops describing the scene at all.
        if let Some(scale) = overrides.gi_probe_scale {
            if !scale.is_finite() || !(1.0..=GI_PROBE_SCALE_MAX).contains(&scale) {
                return Err(invalid(format!(
                    "`{}` sets `gi_probe_scale` to {scale}, which is outside \
                     1.0..={GI_PROBE_SCALE_MAX}: it multiplies the scene's authored `probe_spacing`, \
                     so 1.0 is the finest lattice a machine may ask for and anything below it would \
                     spend more than the scene authored",
                    path.display(),
                )));
            }
            params.gi_probe_scale = scale;
        }
        // A multiplier like `gi_probe_scale`, but on the frame the machine pays for rather than on
        // an authored length — so it reads with the counts: cheaper is smaller and `full`'s 1.0 is
        // the ceiling. Above it would be supersampling, which is the host's own `--scale` and not a
        // per-machine quality; below the floor there is too little left to resample from.
        if let Some(scale) = overrides.render_scale {
            // The floor is the renderer's own, read from it rather than restated: the factor a
            // setting resolves to is applied *through* `RenderScale`, so a value accepted here and
            // refused there would be a setting that resolves and then cannot be rendered.
            let (floor, ceiling) = (RenderScale::MIN, QualityParams::FULL.render_scale);
            if !scale.is_finite() || !(floor..=ceiling).contains(&scale) {
                return Err(invalid(format!(
                    "`{}` sets `render_scale` to {scale}, which is outside {floor}..=1.0: it is the \
                     fraction of the output extent the frame is drawn at, so 1.0 is native and the \
                     most a machine may ask for, and rendering above the output extent is the \
                     host's `--scale` rather than a quality setting",
                    path.display(),
                )));
            }
            params.render_scale = scale;
        }
        // The third multiplier, and the one the renderer never sees: it scales every emitter's
        // authored `rate` and `burst` in the host's extract step, so a thinned emitter releases
        // fewer particles rather than hiding some of them. It reads with the counts — cheaper is
        // smaller, `full`'s 1.0 is the ceiling — and its floor is a real value: `0.0` is every plume
        // silenced, which is a legitimate thing for a machine with no budget for them to ask for.
        if let Some(density) = overrides.particle_density {
            if !density.is_finite()
                || !(0.0..=QualityParams::FULL.particle_density).contains(&density)
            {
                return Err(invalid(format!(
                    "`{}` sets `particle_density` to {density}, which is outside 0.0..=1.0: it is \
                     the fraction of each emitter's authored `rate` and `burst` that is emitted, so \
                     1.0 is the population the scene asked for and the most a machine may ask for, \
                     and a fuller plume is the emitter's own to author rather than a quality setting",
                    path.display(),
                )));
            }
            params.particle_density = density;
        }
        // The fourth multiplier, read like `render_scale` and bounded the same way: the trace is
        // drawn at a fraction of the output extent, so `full`'s value is the ceiling and the
        // floor is the smallest fraction `RenderScale` itself accepts.
        if let Some(scale) = overrides.ssr_scale {
            let (floor, ceiling) = (RenderScale::MIN, QualityParams::FULL.ssr_scale);
            if !scale.is_finite() || !(floor..=ceiling).contains(&scale) {
                return Err(invalid(format!(
                    "`{}` sets `ssr_scale` to {scale}, which is outside {floor}..={ceiling}: it is \
                     the fraction of the output extent the reflection trace runs at, so {ceiling} is \
                     the finest trace a machine may ask for, and below {floor} there is too little \
                     left to resample from",
                    path.display(),
                )));
            }
            params.ssr_scale = scale;
        }
    }
    Ok((quality, params))
}

/// An `L0016`, carrying the help every member of the family needs: the presets, and the file form.
fn invalid(message: String) -> Diagnostic {
    Diagnostic::new(code::QUALITY_SETTING_INVALID, message).with_help(format!(
        "use one of the presets ({}), or the path to a TOML override file (an optional `preset = \
         \"...\"` base, an optional `[effects]` table of \
         `ssao`/`shafts`/`dof`/`gi_dynamic`/`shadows`/`ssr` booleans, and an optional `[params]` table of \
         `ssao_samples`/`shaft_steps`/`dof_taps`/`shadow_map_size`/`sun_cascades`/`local_shadow_atlas_size`/`ibl_specular_size`/`pcf`/`gi_rays` \
         counts plus the `gi_probe_scale`, `render_scale`, `particle_density` and `ssr_scale` multipliers); a host's `--quality` \
         flag beats {QUALITY_ENV} either way",
        preset_list(),
    ))
}

/// The advertised preset names, from the renderer's own list so the two cannot drift.
fn preset_list() -> String {
    Quality::PRESET_NAMES.join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn resolved(explicit: Option<&str>, env: Option<&str>) -> QualitySetting {
        resolve_setting(explicit, env, None).expect("the setting resolves")
    }

    fn rejected(explicit: Option<&str>, env: Option<&str>) -> Diagnostic {
        resolve_setting(explicit, env, None).expect_err("the setting must be refused")
    }

    /// A scratch file holding `text`, removed when the test drops it.
    struct TempFile(std::path::PathBuf);

    impl TempFile {
        fn new(name: &str, text: &str) -> TempFile {
            let dir = std::env::temp_dir().join(format!("ludion-quality-{}", std::process::id()));
            std::fs::create_dir_all(&dir).expect("the scratch directory is creatable");
            let path = dir.join(name);
            std::fs::write(&path, text).expect("the scratch file is writable");
            TempFile(path)
        }

        fn value(&self) -> String {
            self.0.display().to_string()
        }
    }

    impl Drop for TempFile {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.0);
        }
    }

    /// Every advertised preset resolves from either source, in any casing, and keeps its own name
    /// lowercased — the label a host echoes.
    #[test]
    fn every_preset_name_resolves_in_any_casing_and_keeps_its_label() {
        for name in Quality::PRESET_NAMES {
            let expected = Quality::preset(name).expect("an advertised name resolves");
            for spelling in [name.to_string(), name.to_uppercase(), format!("  {name}  ")] {
                let from_flag = resolved(Some(&spelling), None);
                assert_eq!(from_flag.quality, expected);
                assert_eq!(from_flag.label, name);
                assert_eq!(resolved(None, Some(&spelling)), from_flag);
            }
        }
    }

    /// Nothing set anywhere is `FULL`, labelled `"full"` — the machine that was never told anything.
    #[test]
    fn an_unset_setting_is_full() {
        for env in [None, Some(""), Some("   ")] {
            let setting = resolved(None, env);
            assert_eq!(setting, QualitySetting::default());
            assert_eq!(setting.quality, Quality::FULL);
            assert_eq!(setting.label, "full");
        }
        assert_eq!(resolved(Some("  "), None), QualitySetting::default());
    }

    /// A flag must work on a box whose environment is malformed, so the explicit path never consults
    /// (or validates) the environment at all.
    #[test]
    fn an_explicit_value_beats_the_environment() {
        let setting = resolved(Some("low"), Some("not-a-quality"));
        assert_eq!(setting.quality, Quality::LOW);
        assert_eq!(setting.label, "low");
        assert_eq!(
            resolved(Some("full"), Some("low")).quality,
            Quality::FULL,
            "the flag decides even when the environment is perfectly valid",
        );
    }

    /// A packaged default applies only when neither the flag nor the environment chose, so the
    /// launcher of a packaged build can still override it; a malformed one refuses by name.
    #[test]
    fn a_packaged_default_ranks_below_the_flag_and_the_environment() {
        let packaged = |explicit, env| {
            resolve_setting(explicit, env, Some("medium")).expect("the setting resolves")
        };
        assert_eq!(packaged(None, None).params, QualityParams::MEDIUM);
        assert_eq!(packaged(None, Some("  ")).label, "medium");
        assert_eq!(packaged(None, Some("low")).quality, Quality::LOW);
        assert_eq!(packaged(Some("full"), Some("low")).label, "full");
        let refused = resolve_setting(None, None, Some("medum")).expect_err("a typo is refused");
        assert!(refused.message.contains("packaged"), "{}", refused.message);
    }

    /// A value that is neither a preset nor a readable file is refused, and the message names both
    /// interpretations so the reader knows which one they meant to give.
    #[test]
    fn an_unresolvable_value_is_l0016_naming_both_interpretations() {
        for source in [(Some("medum"), None), (None, Some("medum"))] {
            let diagnostic = rejected(source.0, source.1);
            assert_eq!(diagnostic.code, code::QUALITY_SETTING_INVALID);
            let message = diagnostic.message.clone();
            assert!(message.contains("medum"), "{message}");
            assert!(message.contains("TOML file"), "{message}");
            for name in Quality::PRESET_NAMES {
                assert!(
                    message.contains(name),
                    "the presets must be listed: {message}"
                );
            }
            let help = diagnostic.help.clone().expect("the family carries help");
            assert!(help.contains("[effects]"), "{help}");
        }
        assert!(
            rejected(None, Some("medum")).message.contains(QUALITY_ENV),
            "an environment value's diagnostic must name the variable",
        );
    }

    /// The descriptor carries the setting's own label, toggles and parameters — the report says what
    /// this machine chose, never what the build happens to compile.
    #[test]
    fn the_descriptor_carries_the_label_the_toggles_and_the_resolved_axes() {
        let low = resolved(Some("low"), None).descriptor();
        assert_eq!(low.preset, "low");
        assert!(!low.ssao && !low.shafts && !low.dof);

        let medium = resolved(Some("medium"), None).descriptor();
        assert_eq!(medium.preset, "medium");
        assert_ne!(
            medium.preset, low.preset,
            "the label distinguishes settings whose toggles coincide",
        );

        let full = QualitySetting::full().descriptor();
        let params = QualityParams::FULL;
        assert_eq!(full.ssao_samples, params.ssao_samples);
        assert_eq!(full.shaft_steps, params.shaft_steps);
        assert_eq!(full.dof_taps, params.dof_taps);
        assert_eq!(full.shadow_map_size, params.shadow_map_size);
        assert_eq!(full.pcf, params.pcf);
        assert_eq!(full.gi_rays, params.gi_rays);
        assert_eq!(full.gi_probe_scale, params.gi_probe_scale);
        assert_eq!(full.render_scale, params.render_scale);
        assert_eq!(full.ssr_scale, params.ssr_scale);
        assert_eq!(full.ibl_specular_size, params.ibl_specular_size);
        assert!(
            full.ssr && !low.ssr,
            "the toggle is read from the resolved Quality"
        );
        assert_eq!(low.ibl_specular_size, QualityParams::LOW.ibl_specular_size);
        assert!(
            full.gi_dynamic,
            "FULL renders dynamic injection, so the descriptor must say so",
        );
        assert!(
            !low.gi_dynamic,
            "LOW masks dynamic injection, so the descriptor must say so — the field is read from \
             the resolved Quality, never hardcoded",
        );
        assert!(full.shadows && !low.shadows);
        assert!(
            low.shadow_map_size > 0,
            "the axis still reports a plain count at a preset that draws no map — `shadows` is \
             what says the map was not drawn, which is why the toggle has to be there",
        );

        // The two presets whose toggles coincide with FULL's are told apart by their parameters, so
        // a report taken at `medium` is not comparable with one taken at `full` and says why.
        assert_eq!(medium.ssao_samples, QualityParams::MEDIUM.ssao_samples);
        assert_ne!(medium.ssao_samples, full.ssao_samples);
        let high = resolved(Some("high"), None).descriptor();
        assert_eq!(high.shaft_steps, QualityParams::HIGH.shaft_steps);
        assert_ne!(high.dof_taps, full.dof_taps);

        // Every GPU number in a report is a per-pixel cost, so the descriptor has to say how many
        // pixels there were: `low` measures a frame drawn at 0.75× the output extent, and nothing
        // else in the block hints at it.
        assert_eq!(low.render_scale, QualityParams::LOW.render_scale);
        assert!(
            low.render_scale < full.render_scale,
            "the two are not comparable, and this field is what says so: {} vs {}",
            low.render_scale,
            full.render_scale,
        );
        assert_eq!(
            medium.render_scale, full.render_scale,
            "the presets that differ only in how hard each effect works measure the same pixels",
        );
    }

    /// **An override file's descriptor is the case the toggle exists for.** A file labels itself
    /// `custom`, so `preset` cannot say what it turned off, and the axes alone would have a report
    /// claiming a 2048² shadow map for a frame that drew none. Every toggle is therefore read from
    /// the resolved `Quality`, and this pins the one that is otherwise unrecoverable.
    #[test]
    fn a_custom_files_descriptor_reports_the_shadows_it_turned_off() {
        let file = TempFile::new(
            "shadowless.toml",
            "preset = \"full\"\n[effects]\nshadows = false\n",
        );
        let descriptor = resolved(Some(&file.value()), None).descriptor();
        assert_eq!(descriptor.preset, QualitySetting::CUSTOM_LABEL);
        assert!(
            !descriptor.shadows,
            "the file turned the sun's shadows off, and nothing else in the descriptor can say so",
        );
        assert!(
            descriptor.ssao && descriptor.shafts && descriptor.dof && descriptor.gi_dynamic,
            "and it turned off nothing else — a lone false toggle is not inferable from its \
             neighbours either",
        );
        assert_eq!(
            descriptor.shadow_map_size,
            QualityParams::FULL.shadow_map_size,
            "the axis reports the base preset's number for a map that was never drawn: exactly the \
             reading the toggle beside it corrects",
        );
    }

    /// A path that does not exist is the same diagnostic: the two readings are indistinguishable, so
    /// they get one code and one message that names both.
    #[test]
    fn a_missing_file_path_is_l0016() {
        let diagnostic = rejected(Some("does/not/exist.toml"), None);
        assert_eq!(diagnostic.code, code::QUALITY_SETTING_INVALID);
        assert!(
            diagnostic.message.contains("does/not/exist.toml"),
            "{}",
            diagnostic.message,
        );
    }

    /// The effect toggles an override file's text describes.
    fn effects_of(text: &str) -> Quality {
        parse_override_file(text, Path::new("quality.toml"))
            .expect("the file parses")
            .0
    }

    /// The parameters an override file's text describes.
    fn params_of(text: &str) -> QualityParams {
        parse_override_file(text, Path::new("quality.toml"))
            .expect("the file parses")
            .1
    }

    /// A file composes: the preset is the base, `[effects]` overrides individual toggles, and an
    /// absent toggle keeps the base's.
    #[test]
    fn a_file_composes_a_preset_base_with_effect_overrides() {
        assert_eq!(
            effects_of(""),
            Quality::FULL,
            "an empty file is the default base with nothing overridden",
        );
        assert_eq!(effects_of("preset = \"low\"\n"), Quality::LOW);
        assert_eq!(
            effects_of("preset = \"LOW\"\n[effects]\nssao = true\n"),
            Quality {
                ssao: true,
                ..Quality::LOW
            },
        );
        assert_eq!(
            effects_of("[effects]\ndof = false\n"),
            Quality {
                dof: false,
                ..Quality::FULL
            },
            "no preset means the full base",
        );
        // Every toggle `[effects]` can spell, turned off against the full base: `Quality::LOW`
        // except that the full base's `ssr` is switched off too.
        assert_eq!(
            effects_of(
                "preset = \"full\"\n[effects]\nssao = false\nshafts = false\ndof = false\n\
                 gi_dynamic = false\nshadows = false\nssr = false\n",
            ),
            Quality::LOW,
        );
    }

    /// `gi_dynamic` and `shadows` are `[effects]` keys like their siblings: optional, and each
    /// overrides the base preset's toggle in either direction.
    #[test]
    fn gi_dynamic_and_shadows_are_effects_keys_overriding_both_directions() {
        assert_eq!(
            effects_of("preset = \"full\"\n[effects]\ngi_dynamic = false\n"),
            Quality {
                gi_dynamic: false,
                ..Quality::FULL
            },
            "the full base with dynamic injection turned off",
        );
        assert_eq!(
            effects_of("preset = \"low\"\n[effects]\ngi_dynamic = true\n"),
            Quality {
                gi_dynamic: true,
                ..Quality::LOW
            },
            "the low base with dynamic injection turned back on",
        );
        assert_eq!(
            effects_of("preset = \"full\"\n[effects]\nshadows = false\n"),
            Quality {
                shadows: false,
                ..Quality::FULL
            },
            "the full base with the sun's shadows turned off — the sun still lights the scene",
        );
        assert_eq!(
            effects_of("preset = \"low\"\n[effects]\nshadows = true\n"),
            Quality {
                shadows: true,
                ..Quality::LOW
            },
            "the low base with shadows bought back",
        );
    }

    /// `[params]` composes the same way `[effects]` does: the preset supplies the base, a named axis
    /// overrides it, and an absent one keeps it.
    #[test]
    fn a_file_composes_a_preset_base_with_param_overrides() {
        assert_eq!(
            params_of(""),
            QualityParams::FULL,
            "an empty file is the full base on every axis",
        );
        assert_eq!(params_of("preset = \"medium\"\n"), QualityParams::MEDIUM);
        assert_eq!(
            params_of("preset = \"medium\"\n[params]\nssao_samples = 4\n"),
            QualityParams {
                ssao_samples: 4,
                ..QualityParams::MEDIUM
            },
            "one named axis moves; the other two keep the base preset's",
        );
        assert_eq!(
            params_of(
                "[params]\nssao_samples = 2\nshaft_steps = 3\ndof_taps = 5\nshadow_map_size = 512\n\
                 sun_cascades = 3\nlocal_shadow_atlas_size = 256\nibl_specular_size = 64\npcf = 1\ngi_rays = 16\ngi_probe_scale = 2.5\n\
                 render_scale = 0.5\nparticle_density = 0.5\n",
            ),
            QualityParams {
                ssao_samples: 2,
                shaft_steps: 3,
                dof_taps: 5,
                shadow_map_size: 512,
                sun_cascades: 3,
                local_shadow_atlas_size: 256,
                ibl_specular_size: 64,
                pcf: 1,
                gi_rays: 16,
                gi_probe_scale: 2.5,
                render_scale: 0.5,
                particle_density: 0.5,
                ssr_scale: QualityParams::FULL.ssr_scale,
            },
            "every axis at once — the table names them all",
        );
        assert_eq!(
            effects_of("preset = \"low\"\n[params]\nssao_samples = 4\n"),
            Quality::LOW,
            "a `[params]` table never reaches the toggles",
        );
    }

    /// **`gi_rays` carries two distinct refusals, because they are two distinct mistakes.** A count
    /// above the ceiling is a machine asking for more than the scene authored; a count that is not a
    /// multiple of the hit cache's sixteen slots is a stride that would crowd the cached directions
    /// into one pole. Each says its own thing, and the four values the presets ship are legal.
    #[test]
    fn gi_rays_is_refused_separately_for_its_ceiling_and_for_its_stride() {
        let path = Path::new("quality.toml");
        let above = parse_override_file("[params]\ngi_rays = 80\n", path)
            .expect_err("a count above the ceiling must be refused");
        assert_eq!(above.code, code::QUALITY_SETTING_INVALID);
        assert!(
            above
                .message
                .contains(&format!("1..={}", QualityParams::FULL.gi_rays)),
            "the ceiling refusal names the range: {}",
            above.message,
        );

        let strided = parse_override_file("[params]\ngi_rays = 24\n", path)
            .expect_err("a count that strides the cache fractionally must be refused");
        assert_eq!(strided.code, code::QUALITY_SETTING_INVALID);
        assert!(
            strided.message.contains("multiple of 16"),
            "the stride refusal is its own sentence, not the range's: {}",
            strided.message,
        );
        assert!(
            !strided.message.contains("outside"),
            "24 is inside the range, so the reader must be told the actual problem: {}",
            strided.message,
        );

        for rays in [16u32, 32, 48, 64] {
            assert_eq!(
                params_of(&format!("[params]\ngi_rays = {rays}\n")).gi_rays,
                rays
            );
        }
    }

    /// **`gi_probe_scale` is the inverted axis, and its refusal says so.** Every other `[params]`
    /// key names a count whose ceiling is `full`'s; this one multiplies an authored *length*, so
    /// `full`'s `1.0` is its floor and anything below it would spend more than the scene asked for.
    #[test]
    fn gi_probe_scale_is_bounded_from_one_upward() {
        let path = Path::new("quality.toml");
        for bad in ["0.5", "0.0", "-1.0", "5.0", "nan"] {
            let diagnostic =
                parse_override_file(&format!("[params]\ngi_probe_scale = {bad}\n"), path)
                    .expect_err("an out-of-range scale must be refused");
            assert_eq!(diagnostic.code, code::QUALITY_SETTING_INVALID);
            assert!(
                diagnostic
                    .message
                    .contains(&format!("1.0..={GI_PROBE_SCALE_MAX}")),
                "the message names the range this axis actually has: {}",
                diagnostic.message,
            );
        }
        assert_eq!(
            params_of("[params]\ngi_probe_scale = 1.0\n").gi_probe_scale,
            1.0
        );
        assert_eq!(
            params_of(&format!(
                "[params]\ngi_probe_scale = {GI_PROBE_SCALE_MAX}\n"
            ))
            .gi_probe_scale,
            GI_PROBE_SCALE_MAX,
            "both ends of the range are legal",
        );
    }

    /// `ssr_scale` reads like `render_scale`: cheaper is smaller, `full`'s `0.5` is the ceiling and
    /// `RenderScale::MIN` the floor, and the `ssr` effect toggles in either direction.
    #[test]
    fn ssr_scale_is_bounded_and_ssr_is_an_effects_key() {
        let path = Path::new("quality.toml");
        for bad in ["0.1", "0.75", "0.0", "-0.5", "nan"] {
            let diagnostic = parse_override_file(&format!("[params]\nssr_scale = {bad}\n"), path)
                .expect_err("an out-of-range ssr scale must be refused");
            assert_eq!(diagnostic.code, code::QUALITY_SETTING_INVALID);
            assert!(
                diagnostic.message.contains("0.25..=0.5"),
                "the message names the range: {}",
                diagnostic.message,
            );
        }
        assert_eq!(params_of("[params]\nssr_scale = 0.25\n").ssr_scale, 0.25);
        assert_eq!(
            params_of("[params]\nssr_scale = 0.5\n").ssr_scale,
            QualityParams::FULL.ssr_scale,
            "both ends of the range are legal",
        );
        assert!(!effects_of("[effects]\nssr = false\n").ssr);
        assert!(effects_of("preset = \"low\"\n[effects]\nssr = true\n").ssr);
    }

    /// **`render_scale` is the other multiplier, and it reads the ordinary way round.** It names the
    /// fraction of the output extent a frame is drawn at, so cheaper is *smaller*, `full`'s `1.0` is
    /// its ceiling, and both ends of the range refuse: below the floor there is too little left to
    /// resample from, and above native the machine would be asking for supersampling, which is the
    /// host's own `--scale` and not a quality setting at all.
    #[test]
    fn render_scale_is_bounded_from_its_floor_up_to_native() {
        let path = Path::new("quality.toml");
        for bad in ["0.2", "0.0", "-0.5", "1.5", "4.0", "nan"] {
            let diagnostic =
                parse_override_file(&format!("[params]\nrender_scale = {bad}\n"), path)
                    .expect_err("an out-of-range render scale must be refused");
            assert_eq!(diagnostic.code, code::QUALITY_SETTING_INVALID);
            assert!(
                diagnostic
                    .message
                    .contains(&format!("{}..=1.0", RenderScale::MIN)),
                "the message names the range this axis actually has: {}",
                diagnostic.message,
            );
        }
        assert_eq!(
            params_of(&format!("[params]\nrender_scale = {}\n", RenderScale::MIN)).render_scale,
            RenderScale::MIN,
        );
        assert_eq!(
            params_of("[params]\nrender_scale = 1.0\n").render_scale,
            QualityParams::FULL.render_scale,
            "both ends of the range are legal",
        );
        assert_eq!(
            params_of("preset = \"low\"\n").render_scale,
            QualityParams::LOW.render_scale,
            "and a bare preset carries its own factor, which is the one `low` ships",
        );
        assert_eq!(
            params_of("preset = \"low\"\n[params]\nrender_scale = 1.0\n").render_scale,
            1.0,
            "a file can buy native back on a machine whose bottleneck is elsewhere",
        );
    }

    /// **`particle_density` is the third multiplier, and the one whose floor is a real value.** It
    /// scales every emitter's authored `rate` and `burst`, so it reads with the counts — cheaper is
    /// smaller and `full`'s `1.0` is the ceiling, a fuller plume being the emitter's own to author.
    /// `0.0` is inside the range on purpose: a machine with no budget for particles at all may
    /// silence every plume, which is a population of zero and not a malformed setting.
    #[test]
    fn particle_density_is_bounded_from_zero_up_to_the_authored_population() {
        let path = Path::new("quality.toml");
        for bad in ["-0.1", "1.5", "4.0", "nan"] {
            let diagnostic =
                parse_override_file(&format!("[params]\nparticle_density = {bad}\n"), path)
                    .expect_err("an out-of-range particle density must be refused");
            assert_eq!(diagnostic.code, code::QUALITY_SETTING_INVALID);
            assert!(
                diagnostic.message.contains("particle_density"),
                "the message names the key: {}",
                diagnostic.message,
            );
            assert!(
                diagnostic.message.contains("0.0..=1.0"),
                "the message names the range this axis actually has: {}",
                diagnostic.message,
            );
        }
        for edge in ["0.0", "1.0"] {
            assert_eq!(
                params_of(&format!("[params]\nparticle_density = {edge}\n")).particle_density,
                edge.parse::<f32>().expect("a literal float"),
                "both ends of the range are legal",
            );
        }
        assert_eq!(
            params_of("preset = \"low\"\n").particle_density,
            QualityParams::LOW.particle_density,
            "a bare preset carries its own density, which is the quarter `low` ships",
        );
        assert_eq!(
            params_of("preset = \"low\"\n[params]\nparticle_density = 1.0\n").particle_density,
            QualityParams::FULL.particle_density,
            "a file can buy the full plumes back on a machine whose bottleneck is elsewhere",
        );
    }

    /// **The engine's own cached extract spends the setting, counted on the stress scene.**
    /// `Engine::extract_render_scene` is the one path a served or windowed host renders through, so
    /// the read it makes of the setting is the read that has to be right; and
    /// `examples/gallery/particles-stress.yaml` is the population `particle_density` exists for —
    /// `low` is a laptop's lever against exactly that extract cost. Twenty endless emitters round
    /// independently, so the quarter is approximate; `low > 0` keeps sparser from meaning silent.
    #[test]
    fn the_engines_cached_extract_thins_the_stress_scene_at_low() {
        fn particles(setting: Option<QualitySetting>) -> usize {
            let path = Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../examples/gallery/particles-stress.yaml");
            let mut engine =
                crate::Engine::with_project_root(path.parent().expect("a scene has a parent"));
            let source = std::fs::read_to_string(&path).expect("the committed gallery scene reads");
            engine
                .load_inline_scene(&source)
                .expect("the committed gallery scene validates");
            if let Some(setting) = setting {
                engine.set_quality(setting);
            }
            engine
                .extract_render_scene(960, 540)
                .expect("the committed gallery scene extracts")
                .particles
                .len()
        }

        let full = particles(None);
        let low = particles(Some(QualitySetting {
            quality: Quality::LOW,
            params: QualityParams::LOW,
            label: "low".to_string(),
        }));
        assert!(
            full >= 20_000,
            "the stress scene stands its whole population at frame 0, not {full}",
        );
        let share = low as f64 / full as f64;
        assert!(
            low > 0 && (0.2..=0.3).contains(&share),
            "`low` emits {share:.3} of the stress population ({low} of {full}), not the quarter `QualityParams::LOW.particle_density` asks for",
        );
    }

    /// The precedence chain a frame's extent comes out of: an explicit scale, else the setting's,
    /// else native. Both directions of "explicit wins" are checked, because a machine at `low` that
    /// is *told* to supersample must supersample — the flag is the run's instruction, not a second
    /// opinion about the box.
    #[test]
    fn an_explicit_render_scale_beats_a_quality_supplied_one_which_beats_native() {
        let low = resolved(Some("low"), None);
        assert_eq!(
            resolve_render_scale(None, &low).factor(),
            QualityParams::LOW.render_scale,
            "with no explicit scale the setting's own is the frame's",
        );
        for name in ["full", "high", "medium"] {
            assert!(
                resolve_render_scale(None, &resolved(Some(name), None)).is_native(),
                "`{name}` draws every output pixel, so it resolves to native",
            );
        }
        assert!(
            resolve_render_scale(None, &QualitySetting::default()).is_native(),
            "and so does a machine that was never told anything",
        );

        for factor in [RenderScale::MIN, 0.5, 1.0, 2.0, RenderScale::MAX] {
            let explicit = RenderScale::new(factor).expect("an in-range factor");
            for setting in [&low, &QualitySetting::default()] {
                assert_eq!(
                    resolve_render_scale(Some(explicit), setting),
                    explicit,
                    "an explicit {factor}× is the frame's extent whatever the machine asked for",
                );
            }
        }
    }

    /// **The floor is one bound, and this is where its two ends are held together.** A
    /// `render_scale` is *accepted* here — from a preset or from a file's `[params]` — and *applied*
    /// through [`RenderScale`], so a factor this module admits and that type refuses would be a
    /// setting that resolves and then cannot be rendered, which is precisely the panic
    /// [`resolve_render_scale`] documents away. The test lives on this side because this side is the
    /// one with an independent opinion: the renderer's range is the definition.
    #[test]
    fn every_presets_render_scale_is_one_the_resolution_type_accepts() {
        for name in Quality::PRESET_NAMES {
            let scale = QualityParams::preset(name)
                .expect("an advertised name resolves")
                .render_scale;
            assert!(
                RenderScale::new(scale).is_ok(),
                "`{name}` renders at {scale}× the output extent, which the resolution type refuses",
            );
            assert!(
                scale <= QualityParams::FULL.render_scale,
                "`{name}` asks for more pixels than the output has, which is supersampling and not \
                 a quality setting",
            );
        }
        // And the ends of the range a file may name, which is the wider of the two openings: the
        // presets are a handful of fixed values, `[params]` is an arbitrary one.
        for edge in [RenderScale::MIN, QualityParams::FULL.render_scale] {
            let accepted = params_of(&format!("[params]\nrender_scale = {edge}\n")).render_scale;
            assert!(
                RenderScale::new(accepted).is_ok(),
                "a file may set `render_scale = {edge}`, so the resolution type must accept it",
            );
        }
        let below: Vec<&str> = Quality::PRESET_NAMES
            .into_iter()
            .filter(|name| {
                QualityParams::preset(name)
                    .expect("an advertised name resolves")
                    .render_scale
                    < QualityParams::FULL.render_scale
            })
            .collect();
        assert_eq!(
            below,
            ["low"],
            "the axis has to move somewhere, or the range above is guarding a constant — and `low` \
             is the one preset that spends it",
        );
    }

    /// The hit cache's slot budget is written down in two crates — here, to refuse a `gi_rays` that
    /// would stride it fractionally, and in `ludion-renderer` beside the buffer it sizes. Every
    /// shipped preset satisfies it, which is what this reads the renderer's own presets to say.
    #[test]
    fn the_hit_cache_granularity_matches_the_renderers() {
        for name in Quality::PRESET_NAMES {
            let params = QualityParams::preset(name).expect("an advertised name resolves");
            assert_eq!(
                params.gi_rays % GI_HIT_RAYS,
                0,
                "`{name}` casts {} rays, which this module would refuse from a file",
                params.gi_rays,
            );
            assert!(params.gi_probe_scale >= 1.0 && params.gi_probe_scale <= GI_PROBE_SCALE_MAX);
        }
    }

    /// Every count axis is bounded: at least one sample, and never more than `full`'s own count — a
    /// quality setting only goes down. (`gi_probe_scale` is not one of these: it is a multiplier on
    /// a length, and `gi_probe_scale_is_bounded_from_one_upward` states its inverted range.)
    #[test]
    fn a_param_outside_its_range_is_l0016_naming_the_bound() {
        for (key, bad, floor, ceiling) in [
            ("ssao_samples", 17, 1, QualityParams::FULL.ssao_samples),
            ("shaft_steps", 33, 1, QualityParams::FULL.shaft_steps),
            ("dof_taps", 33, 1, QualityParams::FULL.dof_taps),
            (
                "shadow_map_size",
                4096,
                1,
                QualityParams::FULL.shadow_map_size,
            ),
            ("sun_cascades", 5, 1, QualityParams::FULL.sun_cascades),
            // The smallest legal atlas edge is one 16-texel tile row, not `1`.
            (
                "local_shadow_atlas_size",
                8192,
                16,
                QualityParams::FULL.local_shadow_atlas_size,
            ),
            (
                "ibl_specular_size",
                512,
                16,
                QualityParams::FULL.ibl_specular_size,
            ),
            ("pcf", 5, 1, QualityParams::FULL.pcf),
            // The smallest legal `gi_rays` is one whole hit-cache stride, not `1`: the range admits
            // `1`, and the stride rule beside it does not.
            ("gi_rays", 80, GI_HIT_RAYS, QualityParams::FULL.gi_rays),
        ] {
            for value in [0, bad] {
                let diagnostic = parse_override_file(
                    &format!("[params]\n{key} = {value}\n"),
                    Path::new("quality.toml"),
                )
                .expect_err("an out-of-range count must be refused");
                assert_eq!(diagnostic.code, code::QUALITY_SETTING_INVALID);
                let message = diagnostic.message.clone();
                assert!(message.contains(key), "{message}");
                assert!(message.contains(&format!("1..={ceiling}")), "{message}");
            }
            assert_eq!(
                params_of(&format!("[params]\n{key} = {floor}\n")).ssao_samples,
                if key == "ssao_samples" {
                    floor
                } else {
                    QualityParams::FULL.ssao_samples
                },
                "the ends of the range are legal",
            );
        }
    }

    /// The `sun_cascades` override replaces the preset's cap in either direction, so a file can
    /// lower it below `medium`'s or restore the ceiling.
    #[test]
    fn a_sun_cascades_override_replaces_the_presets_cap() {
        let lowered = params_of("preset = \"medium\"\n[params]\nsun_cascades = 1\n");
        assert_eq!(lowered.sun_cascades, 1);
        assert_eq!(lowered.sun_cascade_count(Some(4)), Some(1));
        let restored = params_of("preset = \"medium\"\n[params]\nsun_cascades = 4\n");
        assert_eq!(restored.sun_cascade_count(Some(4)), Some(4));
    }

    #[test]
    fn an_indivisible_local_shadow_atlas_is_l0016_naming_the_grid() {
        let diagnostic = parse_override_file(
            "[params]\nlocal_shadow_atlas_size = 1000\n",
            Path::new("quality.toml"),
        )
        .expect_err("1000 does not divide by 16");
        assert_eq!(diagnostic.code, code::QUALITY_SETTING_INVALID);
        assert!(
            diagnostic.message.contains("multiple of 16"),
            "{}",
            diagnostic.message
        );
    }

    #[test]
    fn a_non_power_of_two_sky_cube_is_l0016_naming_the_mips() {
        let diagnostic = parse_override_file(
            "[params]\nibl_specular_size = 96\n",
            Path::new("quality.toml"),
        )
        .expect_err("96 is not a power of two");
        assert_eq!(diagnostic.code, code::QUALITY_SETTING_INVALID);
        assert!(
            diagnostic.message.contains("power of two"),
            "{}",
            diagnostic.message
        );
    }

    #[test]
    fn a_sky_cube_below_two_mips_is_refused() {
        let diagnostic = parse_override_file(
            "[params]\nibl_specular_size = 8\n",
            Path::new("quality.toml"),
        )
        .expect_err("8 leaves a single mip");
        assert_eq!(diagnostic.code, code::QUALITY_SETTING_INVALID);
        assert!(
            diagnostic.message.contains("below 16"),
            "{}",
            diagnostic.message
        );
    }

    /// `pcf` carries the one constraint no other axis has: a kernel centred on the shaded texel has
    /// an odd edge. In range but even is its own message, and `0` still reports as out of range —
    /// the reader is told the thing that is actually wrong.
    #[test]
    fn an_even_pcf_kernel_is_l0016_naming_the_centre() {
        let path = Path::new("quality.toml");
        let diagnostic = parse_override_file("[params]\npcf = 2\n", path)
            .expect_err("an even kernel must be refused");
        assert_eq!(diagnostic.code, code::QUALITY_SETTING_INVALID);
        let message = diagnostic.message.clone();
        assert!(message.contains("odd"), "{message}");
        assert!(message.contains("centred"), "{message}");
        assert!(
            parse_override_file("[params]\npcf = 0\n", path)
                .expect_err("zero taps is no kernel at all")
                .message
                .contains("1..=3"),
            "`0` is out of range, which is what the reader needs to hear first",
        );
        assert_eq!(params_of("[params]\npcf = 1\n").pcf, 1);
        assert_eq!(params_of("[params]\npcf = 3\n").pcf, 3);
    }

    /// A file resolves through the whole chain, from either source, and labels itself `"custom"` —
    /// no preset name describes toggles a file chose.
    #[test]
    fn a_file_resolves_and_is_labelled_custom() {
        let file = TempFile::new("custom.toml", "preset = \"full\"\n[effects]\ndof = false\n");
        let expected = Quality {
            dof: false,
            ..Quality::FULL
        };
        let from_flag = resolved(Some(&file.value()), None);
        assert_eq!(from_flag.quality, expected);
        assert_eq!(from_flag.label, QualitySetting::CUSTOM_LABEL);
        assert_eq!(resolved(None, Some(&file.value())), from_flag);
    }

    /// A future schema's key or section must fail on this binary rather than be ignored — the file
    /// would otherwise describe a quality this build does not render.
    #[test]
    fn unknown_keys_and_unknown_sections_are_refused() {
        let path = Path::new("quality.toml");
        for text in [
            "preset = \"low\"\nquality = \"low\"\n",
            "[effects]\nssao = false\nbloom = false\n",
            "[params]\nssao_samples = 8\nbloom_levels = 4\n",
            "[render]\nscale = 2\n",
        ] {
            let diagnostic =
                parse_override_file(text, path).expect_err("an unknown key must be refused");
            assert_eq!(diagnostic.code, code::QUALITY_SETTING_INVALID);
            assert!(
                diagnostic.message.contains("quality.toml"),
                "the file must be named: {}",
                diagnostic.message,
            );
        }
    }

    /// Malformed TOML, a wrong value type, and an unknown preset inside a file are all the same
    /// family — one code for every way a setting can fail to mean a quality.
    #[test]
    fn malformed_toml_and_bad_values_are_l0016() {
        let path = Path::new("quality.toml");
        for text in [
            "preset = low\n",
            "[effects\nssao = false\n",
            "[effects]\nssao = \"off\"\n",
            "preset = \"ultra\"\n",
            "preset = 3\n",
        ] {
            let diagnostic =
                parse_override_file(text, path).expect_err("a bad file must be refused");
            assert_eq!(diagnostic.code, code::QUALITY_SETTING_INVALID);
        }
        assert!(
            parse_override_file("preset = \"ultra\"\n", path)
                .unwrap_err()
                .message
                .contains("ultra"),
            "an unknown preset inside a file must name the offending value",
        );
    }

    /// A file that exists but does not parse is refused through the whole chain, not just in the
    /// parser — the resolve path must not swallow it back into "not a file either".
    #[test]
    fn an_unparsable_file_is_refused_by_the_resolver() {
        let file = TempFile::new("broken.toml", "[effects]\nssao = 3\n");
        let diagnostic = rejected(Some(&file.value()), None);
        assert_eq!(diagnostic.code, code::QUALITY_SETTING_INVALID);
        assert!(
            diagnostic.message.contains("broken.toml"),
            "{}",
            diagnostic.message,
        );
    }
}
