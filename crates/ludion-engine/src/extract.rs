//! `extract()` — the world → [`RenderScene`] bridge.
//!
//! Walks the world in **ascending entity-slot order** (`World::entities()`), which for a freshly
//! instantiated scene is exactly document order — so a `RenderScene` is a deterministic function of
//! the world, and two extractions are byte-equal. It reads the built-in components via typed
//! `World::get`, resolves asset paths to decoded textures (deduplicated by content hash), and bakes
//! transforms and the camera into `glam` matrices. The result holds no ECS references and no paths.
//!
//! Camera rule: exactly one [`Camera`] is expected. Zero → a documented default framing
//! ([`RenderCamera::default_framing`]). More than one → an `L2101` error naming the extra camera.

use std::any::TypeId;
use std::borrow::Cow;
use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;

use ludion_assets::Image;
use ludion_core::codes::code;
use ludion_core::collections::StableMap;
use ludion_core::diagnostics::Diagnostic;
use ludion_core::ids::EntityId;
use ludion_core::math::glam::{Mat3, Mat4, Quat, Vec3};
use ludion_core::math::{Affine3, euler_xyz_degrees_to_quat, quat_to_euler_xyz_degrees};
use ludion_core::par;
use ludion_core::time::{FrameIndex, GameClock, SimTime, TickRate};
use ludion_ecs::World;
use ludion_protocol::command::ViewPose;
use ludion_renderer::{
    EnvironmentData, FontDraw, GROUND_MARKER_LIFT, GiData, GroundMarkerInstance, MeshData,
    MeshInstance, MeshShape, ParticleInstance, PointLightData, PostData, QualityParams,
    RenderCamera, RenderScene, SkinJoint, SpotLightData, SpriteInstance, SunData, TextRun,
    TextureImage, UiInstance, WorldVoxelGrid, aspect_ratio,
};
use ludion_scene::{AtlasDocument, AtlasFilter, ClipSampler, FontMode};

use crate::assembly_mesh::AssemblyData;
use crate::asset_cache::{AssetCache, AssetRefs, SourceFiles};
use crate::character::CharacterData;
use crate::components::{
    Animation, Camera, ClipAnchor, DirectionalLight, Environment, GiDynamic, GiVolume,
    GroundMarker, GroundMarkerShape, Mesh, MeshGrid, MeshSource, ParticleEmitter, PointLight,
    PostProcess, Projection, SkinBone, SpotLight, Sprite, SpriteBlend, Text, Tilemap,
    TonemapOperator, Transform, UiList, UiRect, UiText, pointer_token,
};
use crate::docs::DocStore;
use crate::fonts::FontLibrary;
use crate::hierarchy::WorldAffines;
use crate::load_observer::LoadPhase;
use crate::particles::{self, CameraBasis};
use crate::patterns::{self, BulletKind, PatternDoc, Volley};
use crate::ui;
use crate::voxel_mesh::VoxelStaticGrid;

/// Small constant ambient term added to every lit surface when a scene declares no `Environment`.
/// Re-points at [`ludion_renderer::DEFAULT_AMBIENT`] so extract and the renderer's own empty-scene
/// default are one value.
const AMBIENT: [f32; 3] = ludion_renderer::DEFAULT_AMBIENT;

/// How `extract` turns a scene asset path into decoded data — the seam `extract_inner` resolves
/// assets through; `CachedAssets` is the loader, and `RefLoader` collects references. A single
/// `&mut dyn SceneAssets` (rather than two closures) keeps the cache's one `&mut AssetCache` from
/// being borrowed twice at once.
pub(crate) trait SceneAssets {
    /// Resolves a sprite/tilemap texture path to a decoded, premultiplied [`TextureImage`],
    /// `Arc`-shared so a cached hit is a refcount bump, not a byte copy.
    fn texture(&mut self, rel: &str) -> Result<Arc<TextureImage>, Diagnostic>;
    /// Resolves a `Sprite.atlas` / `Tilemap` legend `atlas` path to its parsed `kind: atlas`
    /// document and packed colour image. `Arc`-shared like a mesh: a stage is one atlas, so every
    /// quad on screen resolves the same entry.
    fn atlas(&mut self, rel: &str) -> Result<Arc<AtlasAsset>, Vec<Diagnostic>>;
    /// Resolves a `Mesh.shape: asset` path to renderer-ready [`MeshData`], `Arc`-shared so a cached
    /// hit is a refcount bump, not a buffer copy (`RenderScene::mesh_assets` holds the same `Arc`).
    fn mesh(&mut self, rel: &str) -> Result<Arc<MeshData>, Vec<Diagnostic>>;
    /// Resolves a `Mesh.shape: voxel` path to renderer-ready [`MeshData`] (the voxel document,
    /// greedily meshed). Lands in the same `mesh_assets` list as [`mesh`](SceneAssets::mesh).
    fn voxel(&mut self, rel: &str) -> Result<Arc<MeshData>, Vec<Diagnostic>>;
    /// Resolves a `Mesh.shape: voxel` path to its authored occupancy grid (the D15 accessor —
    /// `Plan/gi-wave0-voxelizer.md` §6), kept reachable past meshing for the voxelizer.
    fn voxel_grid(&mut self, rel: &str) -> Result<Arc<VoxelStaticGrid>, Vec<Diagnostic>>;
    /// Resolves a `Mesh.shape: sculpt` path to renderer-ready [`MeshData`] (the sculpt document's
    /// parts meshed as one surface). Lands in the same `mesh_assets` list as
    /// [`mesh`](SceneAssets::mesh).
    fn sculpt(&mut self, rel: &str) -> Result<Arc<MeshData>, Vec<Diagnostic>>;
    /// Resolves a `Mesh.shape: assembly` path to its meshed parts (every part file read and meshed
    /// on its own, with the rest pose that places them). Each part's `MeshData` lands in the same
    /// `mesh_assets` list as [`mesh`](SceneAssets::mesh), one entry per part.
    fn assembly(&mut self, rel: &str) -> Result<Arc<AssemblyData>, Vec<Diagnostic>>;
    /// Resolves a `Mesh.shape: character` path to its loaded character: the mesh path (resolved
    /// through [`mesh`](SceneAssets::mesh) like an `asset`) and its bound clips.
    fn character(&mut self, rel: &str) -> Result<Arc<CharacterData>, Vec<Diagnostic>>;
    /// The world voxelization already built for the instance set `signature` stands for, or `None`
    /// when this loader holds none. The inner `Option` is the voxelizer's own answer (a scene with
    /// nothing rasterizable has no grid).
    ///
    /// The persistent-state half of the voxelizer's cache: rasterizing a district is a
    /// multi-million-cell walk, so it is load-time work, not per-frame work.
    fn voxelization(&self, signature: u64) -> Option<Option<Arc<WorldVoxelGrid>>>;
    /// The voxelization this loader retains under *any* signature, when a host is holding the GI
    /// baseline — the second chance the voxelizer takes after [`voxelization`](SceneAssets::voxelization)
    /// misses, so a live world whose movers are unmarked keeps the held grid instead of re-walking
    /// millions of cells. `None` when nothing is held or nothing is retained (a first build is never
    /// held), and always `None` for a loader with no retention. Recording that the served grid is
    /// *stale* is this call's own business.
    fn held_voxelization(&mut self) -> Option<Option<Arc<WorldVoxelGrid>>>;
    /// Records `grid` as the voxelization of `signature`, handing it straight back so the caller
    /// stores and returns in one step. A loader with nowhere to keep it returns `grid` unchanged.
    fn set_voxelization(
        &mut self,
        signature: u64,
        grid: Option<Arc<WorldVoxelGrid>>,
    ) -> Option<Arc<WorldVoxelGrid>>;
    /// A load phase starts. A loader with no observer ignores it.
    fn observe_begin(&mut self, _phase: LoadPhase) {}
    /// The load phase last begun ends.
    fn observe_end(&mut self, _phase: LoadPhase) {}
}

/// The cached loader: each asset resolves **through** the [`AssetCache`] (miss = load + decode once,
/// hit = a map lookup). Every asset hands out the cache's own `Arc` — a refcount bump, not a buffer
/// copy (`Plan/voxel-artist-scale.md`: a streamed voxel model's `MeshData` runs to
/// megabytes).
struct CachedAssets<'a> {
    root: &'a Path,
    cache: &'a mut AssetCache,
}

impl SceneAssets for CachedAssets<'_> {
    fn texture(&mut self, rel: &str) -> Result<Arc<TextureImage>, Diagnostic> {
        self.cache.load(self.root, rel).cloned()
    }
    fn atlas(&mut self, rel: &str) -> Result<Arc<AtlasAsset>, Vec<Diagnostic>> {
        self.cache.load_atlas(self.root, rel).cloned()
    }
    fn mesh(&mut self, rel: &str) -> Result<Arc<MeshData>, Vec<Diagnostic>> {
        self.cache.load_mesh(self.root, rel).cloned()
    }
    fn voxel(&mut self, rel: &str) -> Result<Arc<MeshData>, Vec<Diagnostic>> {
        self.cache.load_voxel(self.root, rel).cloned()
    }
    fn voxel_grid(&mut self, rel: &str) -> Result<Arc<VoxelStaticGrid>, Vec<Diagnostic>> {
        self.cache.load_voxel_grid(self.root, rel).cloned()
    }
    fn sculpt(&mut self, rel: &str) -> Result<Arc<MeshData>, Vec<Diagnostic>> {
        self.cache.load_sculpt(self.root, rel).cloned()
    }
    fn assembly(&mut self, rel: &str) -> Result<Arc<AssemblyData>, Vec<Diagnostic>> {
        self.cache.load_assembly(self.root, rel).cloned()
    }
    fn character(&mut self, rel: &str) -> Result<Arc<CharacterData>, Vec<Diagnostic>> {
        self.cache.load_character(self.root, rel).cloned()
    }
    fn voxelization(&self, signature: u64) -> Option<Option<Arc<WorldVoxelGrid>>> {
        self.cache.voxelization(signature)
    }
    fn held_voxelization(&mut self) -> Option<Option<Arc<WorldVoxelGrid>>> {
        self.cache.held_voxelization()
    }
    fn set_voxelization(
        &mut self,
        signature: u64,
        grid: Option<Arc<WorldVoxelGrid>>,
    ) -> Option<Arc<WorldVoxelGrid>> {
        self.cache.set_voxelization(signature, grid)
    }
    fn observe_begin(&mut self, phase: LoadPhase) {
        if let Some(observer) = self.cache.observer_mut() {
            observer.begin(phase);
        }
    }
    fn observe_end(&mut self, phase: LoadPhase) {
        if let Some(observer) = self.cache.observer_mut() {
            observer.end(phase);
        }
    }
}

// --- Atlases ------------------------------------------------------------------------------------

/// A resolved atlas: the parsed `kind: atlas` document and the packed colour image its
/// `maps: { color }` names.
///
/// The two are one asset because neither is usable alone — a UV rect means nothing without the
/// extent it divides by, and the packed image means nothing without the regions that address it.
pub(crate) struct AtlasAsset {
    /// The derived, committed atlas document.
    pub(crate) doc: AtlasDocument,
    /// The packed colour image, already premultiplied by the bake, in its renderer-ready form.
    pub(crate) texture: Arc<TextureImage>,
}

impl AtlasAsset {
    /// The UV rect and derived world size of the region `frame` names, or the `L1152` that says this
    /// atlas has no such region.
    ///
    /// The one place a frame name becomes geometry, so `extract` and `check-scene` cannot disagree
    /// about which references draw. `pointer` is the JSON Pointer the reference's fields hang off
    /// (`/entities/3/components/Sprite`); the error lands on its `/frame`.
    fn frame(
        &self,
        rel: &str,
        frame: &str,
        pointer: &str,
    ) -> Result<([f32; 4], [f32; 2]), Diagnostic> {
        let Some(region) = self.doc.region(frame) else {
            return Err(frame_unknown(&self.doc, rel, frame, pointer));
        };
        let [x, y, w, h] = region.rect;
        // The packed image's own extent, never the document's — a rect outside it is `L1153`,
        // reported by `check-scene`'s on-disk atlas pass rather than silently clamped here.
        let (iw, ih) = (
            self.texture.width.max(1) as f32,
            self.texture.height.max(1) as f32,
        );
        let uv = [
            x as f32 / iw,
            y as f32 / ih,
            (x + w) as f32 / iw,
            (y + h) as f32 / ih,
        ];
        let ppu = self.doc.pixels_per_unit as f32;
        let size = [w as f32 / ppu, h as f32 / ppu];
        Ok((uv, size))
    }
}

/// The atlases one placement-and-extract pass has resolved, keyed by document path.
///
/// [`place`] wants each region's pixel size (a quad naming an atlas with no authored `size` is sized
/// by its own art) and [`extract`] wants the same region's UV rect and packed image, both in the same
/// pass over the same world. Holding the answer here means an atlas is read, parsed and decoded once
/// per pass however many quads name it — a stage of a thousand tiles is one read — and that the two
/// consumers cannot resolve to different documents.
///
/// A *failure* is remembered too, so a missing atlas costs one failed read rather than one per quad.
/// The diagnostic is still built per occurrence, which is the [`resolve_texture`] behaviour exactly:
/// what is cached is the IO, never the reporting.
#[derive(Default)]
pub(crate) struct AtlasCache {
    resolved: StableMap<String, Result<Arc<AtlasAsset>, Vec<Diagnostic>>>,
}

impl AtlasCache {
    /// The atlas at `rel`, loaded through `assets` on this pass's first ask.
    ///
    /// Hands back an owned `Result` — an `Arc` clone is a refcount bump, and the error clone happens
    /// only on the failing path — so a caller can report or push without holding this map borrowed.
    fn resolve(
        &mut self,
        assets: &mut dyn SceneAssets,
        rel: &str,
    ) -> Result<Arc<AtlasAsset>, Vec<Diagnostic>> {
        if !self.resolved.as_inner().contains_key(rel) {
            let loaded = assets.atlas(rel);
            self.resolved.insert(rel.to_string(), loaded);
        }
        match self.resolved.as_inner().get(rel).expect("just inserted") {
            Ok(asset) => Ok(asset.clone()),
            Err(causes) => Err(causes.clone()),
        }
    }

    /// The world size of the region `frame` names, or `None` when the atlas or the region does not
    /// resolve.
    ///
    /// Geometry only, never a diagnostic: [`place`] sizes what it can and falls back to the
    /// untextured quad's `[1, 1]`, so `render.bounds` still answers for a scene with a missing atlas;
    /// [`extract`] is the one place a broken reference is reported.
    fn region_size(
        &mut self,
        assets: &mut dyn SceneAssets,
        rel: &str,
        frame: &str,
    ) -> Option<[f32; 2]> {
        let asset = self.resolve(assets, rel).ok()?;
        // Through the one region→geometry derivation, so a quad's size and its UV rect can never be
        // read off different arithmetic. The pointer is unused on this path: the `L1152` it would
        // carry is discarded here and raised by `extract`.
        let (_, size) = asset.frame(rel, frame, "").ok()?;
        Some(size)
    }

    /// The region's pivot as a fraction of its own extent — `[x / w, y / h]` from the top-left, so a
    /// centre pivot is `[0.5, 0.5]` — or `None` when the atlas or region does not resolve. Geometry
    /// only, like [`region_size`](AtlasCache::region_size): [`place`] anchors rotation about it and
    /// [`extract`] is the one place a broken reference is reported.
    fn region_pivot_frac(
        &mut self,
        assets: &mut dyn SceneAssets,
        rel: &str,
        frame: &str,
    ) -> Option<[f32; 2]> {
        let asset = self.resolve(assets, rel).ok()?;
        let region = asset.doc.region(frame)?;
        let [_, _, w, h] = region.rect;
        Some([
            region.pivot[0] as f32 / w.max(1) as f32,
            region.pivot[1] as f32 / h.max(1) as f32,
        ])
    }
}

/// Reads and decodes the loose PNG at `root`/`rel` for a `RenderScene`. Every loose texture is
/// premultiplied, so every `RenderScene::textures` entry carries premultiplied colour; atlas
/// colour maps are the exception (see [`load_atlas`]).
///
/// # Errors
/// `L1101` if the file cannot be read, `L1102` if the PNG cannot be decoded.
pub(crate) fn load_texture(root: &Path, rel: &str) -> Result<Arc<TextureImage>, Diagnostic> {
    let image = ludion_assets::load_png(root, rel).map(Image::premultiplied)?;
    Ok(Arc::new(TextureImage {
        width: image.width,
        height: image.height,
        rgba: image.rgba,
        hash: image.hash,
        // A loose PNG has no atlas document to declare a filter: nearest, the pixel-art
        // default the sprite pass has always bound.
        filter_linear: false,
        premultiplied_on_load: true,
    }))
}

/// Reads and validates the atlas document at `root`/`rel`, then decodes the colour map it names.
///
/// # Errors
/// `L1101` if either file cannot be read, `L1102` if the PNG cannot be decoded, `L1153` if the
/// document does not parse or validate as a `kind: atlas`.
pub(crate) fn load_atlas(root: &Path, rel: &str) -> Result<AtlasAsset, Vec<Diagnostic>> {
    load_atlas_with_sources(root, rel).map(|(asset, _)| asset)
}

/// [`load_atlas`], additionally reporting the two files it read — the document, then the colour PNG —
/// each with the content hash of the bytes it decoded.
pub(crate) fn load_atlas_with_sources(
    root: &Path,
    rel: &str,
) -> Result<(AtlasAsset, SourceFiles), Vec<Diagnostic>> {
    let bytes = ludion_assets::load_bytes(root, rel).map_err(|diagnostic| vec![diagnostic])?;
    let source = std::str::from_utf8(&bytes.bytes).map_err(|_| {
        vec![
            Diagnostic::new(
                code::SPRITE_DOCUMENT_STRUCTURE,
                format!("atlas `{rel}` is not valid UTF-8 text"),
            )
            .with_help("an atlas document is a strict-YAML text file (`*.atlas.yaml`)"),
        ]
    })?;
    let doc = ludion_scene::check_atlas(source)?;
    // NOT premultiplied: `ludion generate-atlas` bakes the packed image premultiplied, and
    // premultiplying already-premultiplied colour darkens every translucent texel a second time.
    let color = ludion_core::paths::sibling(rel, &doc.color_map);
    let image = ludion_assets::load_png(root, &color).map_err(|diagnostic| vec![diagnostic])?;
    let texture = Arc::new(TextureImage {
        width: image.width,
        height: image.height,
        rgba: image.rgba,
        hash: image.hash,
        // The atlas document consumes its own `filter:` here: `linear` for art authored at a
        // density the screen will not match, else nearest.
        filter_linear: doc.filter == AtlasFilter::Linear,
        premultiplied_on_load: false,
    });
    let sources = vec![(rel.to_string(), bytes.hash), (color, image.hash)];
    Ok((AtlasAsset { doc, texture }, sources))
}

/// Resolves the `atlas` + `frame` pair a `Sprite` or one `Tilemap` legend entry carries, reporting
/// exactly what a render would: `L1151` when the atlas cannot be read or parsed (or when `frame` is
/// missing), `L1152` with a did-you-mean when it has no such region. An empty result means the
/// reference draws.
///
/// `pointer` is the JSON Pointer the pair hangs off (`/entities/3/components/Sprite`), so the codes
/// land on its `/atlas` and `/frame`. This is `check-scene`'s entry point into the same resolution
/// `extract` performs — one home for the rules, so a scene that checks clean cannot fail to draw.
pub fn check_atlas_reference(
    project_root: &Path,
    pointer: &str,
    atlas_rel: &str,
    frame: Option<&str>,
) -> Vec<Diagnostic> {
    let Some(frame) = frame else {
        return vec![atlas_without_frame(pointer)];
    };
    match load_atlas(project_root, atlas_rel) {
        Err(causes) => vec![atlas_unresolved(project_root, atlas_rel, pointer, causes)],
        Ok(asset) => match asset.frame(atlas_rel, frame, pointer) {
            Ok(_) => Vec::new(),
            Err(diagnostic) => vec![diagnostic],
        },
    }
}

/// The `L1151` an unreadable or unparseable atlas reports, carrying the underlying messages as its
/// help — the [`resolve_texture`] precedent, nearest-filename suggestion included.
fn atlas_unresolved(
    project_root: &Path,
    rel: &str,
    pointer: &str,
    causes: Vec<Diagnostic>,
) -> Diagnostic {
    let mut help = causes
        .iter()
        .map(|diagnostic| diagnostic.message.as_str())
        .collect::<Vec<_>>()
        .join("; ");
    if let Some(name) = suggest_asset(project_root, rel) {
        help = format!("{help} (nearest match is `{name}`)");
    }
    Diagnostic::new(
        code::SPRITE_ATLAS_UNRESOLVED,
        format!("atlas `{rel}` could not be resolved"),
    )
    .with_path(format!("{pointer}/atlas"))
    .with_help(help)
}

/// The `L1151` an atlas reference with no `frame` reports. Schema validation rejects the same shape
/// before a scene ever instantiates ([`Sprite`]'s `validate_semantics`); this is the defensive twin
/// for a world assembled in code, which reports rather than panics.
fn atlas_without_frame(pointer: &str) -> Diagnostic {
    Diagnostic::new(
        code::SPRITE_ATLAS_UNRESOLVED,
        "`atlas` is set, but no `frame` names the region to draw".to_string(),
    )
    .with_path(format!("{pointer}/atlas"))
    .with_help("add `frame: <region name>` from the atlas's `regions:` map")
}

/// The mirror of [`atlas_without_frame`]: a reference that names the region but not the document it
/// lives in. A `Sprite` says this at check time (`atlas_reference_diagnostics`); a bullet kind's
/// pair lives in a data-doc, so it is said here, where the reference is resolved.
pub(crate) fn frame_without_atlas(pointer: &str) -> Diagnostic {
    Diagnostic::new(
        code::SPRITE_ATLAS_UNRESOLVED,
        "`frame` names an atlas region, but no `atlas` is set".to_string(),
    )
    .with_path(format!("{pointer}/frame"))
    .with_help("add `atlas: <path to a kind: atlas document>`, or drop `frame`")
}

/// The `L1152` a frame naming no region reports, with the nearest region name when one is plausibly
/// the intended one.
fn frame_unknown(doc: &AtlasDocument, rel: &str, frame: &str, pointer: &str) -> Diagnostic {
    let help = match doc.nearest_region_name(frame) {
        Some(nearest) => format!("did you mean `{nearest}`?"),
        None => "the atlas's `regions:` mapping lists every legal name, sorted".to_string(),
    };
    Diagnostic::new(
        code::SPRITE_FRAME_UNKNOWN,
        format!("atlas `{rel}` has no region named `{frame}`"),
    )
    .with_path(format!("{pointer}/frame"))
    .with_help(help)
}

/// Extracts a [`RenderScene`] from `world`, resolving textures relative to `project_root` and using
/// `viewport` (width, height) for the camera aspect ratio.
///
/// The **tick-0, full-density** seam: it is [`extract_at`] at sim time zero and
/// [`QualityParams::FULL`]'s `particle_density`, which is what a test, a doctest or a one-shot path
/// that has nothing else to say wants. A host that has ticked, or that carries a machine's quality,
/// calls [`extract_at`] and passes both — keeping this three-argument shape means the callers with
/// no opinion cannot accidentally express one.
///
/// Each call decodes through its own fresh, prefetched `AssetCache` ([`extract_at`]).
///
/// # Errors
/// - `L2101` if the world has more than one [`Camera`].
/// - `L1101`/`L1102` if a referenced texture cannot be read or decoded (all such errors are
///   collected; each carries a JSON Pointer to the offending `Sprite` and, where cheap, a
///   nearest-filename suggestion).
///
/// # Example
/// ```
/// use ludion_ecs::World;
/// use std::path::Path;
///
/// let registry = ludion_engine::builtin_registry();
/// let scene = ludion_scene::check("format: 1\nentities: []\n", &registry).unwrap();
/// let world = ludion_scene::instantiate(&scene, &registry).unwrap();
/// let render_scene = ludion_engine::extract(&world, (1280, 720), Path::new(".")).unwrap();
/// assert!(render_scene.meshes.is_empty());
/// ```
pub fn extract(
    world: &World,
    viewport: (u32, u32),
    project_root: &Path,
) -> Result<RenderScene, Vec<Diagnostic>> {
    extract_at(
        world,
        viewport,
        project_root,
        SimTime::from_ticks(0, TickRate::DEFAULT),
        QualityParams::FULL.particle_density,
    )
}

/// [`extract`], at an explicit point in simulated time.
///
/// `now` is the *sim* time the host has reached; every closed form below measures against
/// **gameplay** time, which is `now` read through the world's
/// [`GameClock`](ludion_core::time::GameClock) — so a game holding its clock freezes what the frame
/// draws while the sim keeps ticking. A world with no clock reads the two as one.
///
/// That time is read by exactly one family of concerns, the closed-form evaluators: the two clip
/// evaluators derive an [`Animation`]-carrying entity's clip time as
/// `(now − started) × seconds_per_tick` — the assembly evaluator poses a `Mesh`'s parts from it, the
/// flipbook evaluator picks which atlas region a `Sprite` shows — and the same subtraction places a
/// particle emitter's live window and a [`Volley`](crate::Volley)'s bullets. Everything else in a
/// `RenderScene` is a pure function of the world, so a world with none of them extracts
/// byte-identically at every `now` — which is why [`extract`] can keep its three-argument shape and
/// pass tick 0 (the one-shot render paths run their driver to frame 0 anyway).
///
/// A host that has actually ticked passes its driver's own [`SimTime`]; time is a parameter here and
/// never read from a clock, the determinism rule every sim-side surface follows.
///
/// `particle_density` is the host's [`QualityParams::particle_density`] — the machine's own setting,
/// arriving as a parameter for the same reason `now` does, since a render path that read a quality
/// out of the environment would silently re-bless every golden on a box whose `LUDION_QUALITY` is
/// set. It scales each emitter's `rate` and `burst` before its live window is computed, so a thinned
/// emitter releases fewer particles rather than hiding some of them, and it reaches nothing else in
/// the scene. Goldens, contact sheets and tests pass [`QualityParams::FULL`]'s value: a per-machine
/// setting leaking into an instrument would make two agents' images disagree for a reason neither
/// can see in the file.
///
/// A bullet kind whose atlas reference fails draws the untextured quad; a one-shot extract has no
/// log to report that to, so it does not (the content-root validators and a live host's
/// `extract_through` do).
///
/// Each call decodes through its own fresh `AssetCache`, prefetched in parallel — the loader a live
/// host uses. Nothing carries between calls, so a one-shot render always re-reads the files and
/// re-voxelizes.
///
/// # Errors
/// Exactly [`extract`]'s.
pub fn extract_at(
    world: &World,
    viewport: (u32, u32),
    project_root: &Path,
    now: SimTime,
    particle_density: f32,
) -> Result<RenderScene, Vec<Diagnostic>> {
    let mut cache = AssetCache::new();
    cache.prefetch(project_root, &asset_refs(world, project_root));
    extract_through(
        world,
        viewport,
        project_root,
        &mut cache,
        now,
        particle_density,
        // The public extract is every CLI, golden and offline path: a host's view override is not
        // theirs to see, which is what keeps every committed image a function of the scene.
        None,
        &mut Vec::new(),
    )
}

/// [`extract`], but resolving sprite textures **through** an [`AssetCache`] — a miss loads and decodes
/// the PNG once and inserts it, a hit is a map lookup. This is the live/streamed/screenshot path
/// (called from [`Engine::extract_render_scene`](crate::Engine)), where re-reading every texture from
/// disk each frame was the per-frame IO `Plan/phase-7` decision 5 kills.
///
/// `degraded` collects the diagnostics of references that drew a fallback instead of failing the
/// frame (a bullet kind whose atlas does not resolve), re-pointed at the document field that
/// authored them, each paired with the latch key the caller reports it under; it is filled on `Err`
/// too.
#[allow(clippy::too_many_arguments)]
pub(crate) fn extract_through(
    world: &World,
    viewport: (u32, u32),
    project_root: &Path,
    cache: &mut AssetCache,
    now: SimTime,
    particle_density: f32,
    view: Option<&ViewPose>,
    degraded: &mut Vec<(String, Diagnostic)>,
) -> Result<RenderScene, Vec<Diagnostic>> {
    // The cache owns the decoded bytes; `extract`'s `RenderScene` gets a handle, so a hit reads out
    // of the cache instead of touching disk. Every asset is `Arc`-shared (a hit is a refcount bump,
    // textures included), and the renderer's content-hash GPU caches skip re-uploading unchanged
    // bytes.
    //
    // Staleness is a property of *this* extract, so it starts cleared: a scene that no longer
    // authors a `GiVolume` never consults the voxelization cache and must not inherit the last
    // answer.
    cache.clear_gi_stale();
    let mut assets = CachedAssets {
        root: project_root,
        cache,
    };
    extract_inner(
        world,
        viewport,
        project_root,
        &mut assets,
        now,
        particle_density,
        view,
        degraded,
    )
}

/// The shared body of [`extract`] and [`extract_through`], parameterized by how an asset path is
/// turned into decoded data (through an [`AssetCache`], the host's or a fresh one). `degraded` takes
/// the diagnostics of fallbacks that did not fail the extract, each with its latch key.
#[allow(clippy::too_many_arguments)]
fn extract_inner(
    world: &World,
    viewport: (u32, u32),
    project_root: &Path,
    assets: &mut dyn SceneAssets,
    now: SimTime,
    particle_density: f32,
    view: Option<&ViewPose>,
    degraded: &mut Vec<(String, Diagnostic)>,
) -> Result<RenderScene, Vec<Diagnostic>> {
    // Gameplay time, read once: every closed form below — a volley's bullets, a particle emitter's
    // live window, a clip's phase — measures `now − started`, so reading the world's `GameClock`
    // here is what freezes the picture while a game is held. A world with no clock (a one-shot
    // render, a game that never freezes) reads sim time unchanged.
    let now = world
        .resource::<GameClock>()
        .map_or(now, |clock| clock.game_time(now));
    let ids: Vec<EntityId> = world
        .entities()
        .chain(world.presentation_entities())
        .collect();
    let mut camera = resolve_camera(world, &ids, viewport, view).map_err(|d| vec![d])?;
    let fonts = font_library(world, project_root);

    // One walk, one sizing computation: `place` produces every renderable's world geometry, and both
    // this function and `render.bounds` consume it — so a sprite's size, a mesh's transform, and a
    // text run's layout can never disagree between what is *drawn* and what `render.bounds` *reports*
    // (the sizing-rule seam; see [`place`]). `atlases` outlives the call because placement and the
    // texture resolution below ask the same atlases for different halves of the same region.
    let mut atlases = AtlasCache::default();
    let mut poses = WorldAffines::new();
    let mut placed = place(
        world,
        &ids,
        &camera,
        &fonts,
        assets,
        &mut atlases,
        &mut poses,
        now,
    );
    drop_non_finite(&mut placed, degraded);

    // Resolve sprite textures in first-encounter (entity) order, collecting every load error before
    // giving up. Geometry is already fixed above — only the path→index mapping and its IO live here.
    // `textures` is the output list and the *only* thing iterated; `path_index` is a lookup index
    // over it, so dedup cost does not grow with the number of distinct paths.
    let mut textures: Vec<Arc<TextureImage>> = Vec::new();
    let mut path_index: StableMap<String, usize> = StableMap::new();
    let mut errors: Vec<Diagnostic> = Vec::new();

    let mut sprites = Vec::with_capacity(placed.sprites.len());
    for sprite in &placed.sprites {
        let mut uv = sprite.uv;
        // Geometry is already final (`place` applied the region size); all that is resolved here is
        // where the pixels come from. An atlas dedups through the same `path_index` a loose texture
        // does — an `*.atlas.yaml` path and a `*.png` path cannot collide — so every quad of one
        // atlas lands on one `textures` entry and therefore one batch.
        let texture = match (sprite.atlas, sprite.texture) {
            (Some(atlas_rel), _) => resolve_atlas_frame(
                project_root,
                atlas_rel,
                sprite.frame.as_deref(),
                sprite.site,
                assets,
                &mut atlases,
                &mut textures,
                &mut path_index,
                &mut errors,
            )
            .map(|(index, region_uv)| {
                uv = region_uv;
                index
            }),
            (None, Some(rel)) => resolve_texture(
                project_root,
                rel,
                sprite.site,
                assets,
                &mut textures,
                &mut path_index,
                &mut errors,
            ),
            (None, None) => None,
        };
        // Scroll is time evaluated here, once, from the frame's own `SimTime` — never accumulated,
        // so it cannot drift and never enters the world hash. Folding it back into `[0, 1)` keeps
        // the number small (and its float steps even) however long the session runs; the shader's
        // `fract` makes the whole offset periodic with period 1 anyway.
        let seconds = now.seconds();
        let scrolled = |rate: f32| (f64::from(rate) * seconds).rem_euclid(1.0) as f32;
        let mut uv_repeat = sprite.uv_repeat;
        let mut uv_offset = [scrolled(sprite.uv_scroll[0]), scrolled(sprite.uv_scroll[1])];
        let wraps = uv_repeat != SpriteInstance::NO_TILING || uv_offset != [0.0, 0.0];
        // Flip mirrors the sampled sub-rectangle in place — no second copy of the region, no shader
        // branch. `flip_x` swaps the u endpoints, `flip_y` the v endpoints; both is a 180° flip.
        // A wrapping quad mirrors the *tiled* coordinate instead, by walking the tiling backwards
        // (`fract(-r*t + r + o) == fract((1 - t) * r + o)`): mirroring the rect would only mirror
        // each tile, leaving the strip scrolling the way it did before the flip. At the defaults the
        // two are the same arithmetic, so no clamping quad changes.
        if sprite.flip_x {
            if wraps {
                uv_offset[0] = (uv_repeat[0] + uv_offset[0]).rem_euclid(1.0);
                uv_repeat[0] = -uv_repeat[0];
            } else {
                uv = [uv[2], uv[1], uv[0], uv[3]];
            }
        }
        if sprite.flip_y {
            if wraps {
                uv_offset[1] = (uv_repeat[1] + uv_offset[1]).rem_euclid(1.0);
                uv_repeat[1] = -uv_repeat[1];
            } else {
                uv = [uv[0], uv[3], uv[2], uv[1]];
            }
        }
        sprites.push(SpriteInstance {
            center: sprite.center,
            right: sprite.right,
            up: sprite.up,
            color: sprite.color,
            texture,
            uv,
            lit: sprite.lit,
            layer: sprite.layer,
            additive: sprite.additive,
            flash: sprite.flash,
            flash_color: sprite.flash_color,
            uv_repeat,
            uv_offset,
        });
    }

    // Projectile-pattern bullets are sprites like any other, appended after the entity sprites and
    // before every sort and snap below — the renderer breaks a `(layer, depth)` tie by scene order,
    // so where they enter this list is the tie-break they get.
    extract_volleys(
        world,
        &ids,
        project_root,
        assets,
        &mut atlases,
        &mut poses,
        &mut textures,
        &mut path_index,
        degraded,
        now,
        CameraBasis::from_view(camera.view),
        &mut sprites,
    );

    // `pixel_snap` (extract-only, hash-neutral): when the single orthographic camera opts in and a
    // sprite names an atlas whose `pixels_per_unit` defines a texel grid, snap the view height so one
    // source texel spans a whole number of screen pixels, and snap each sprite centre onto that grid —
    // the crisp-vs-shimmer difference the gallery golden shows. Read here, never in `place`, so
    // `render.bounds` and the world hash never see it.
    // Skipped while a view override is in force: the override is a perspective camera, and the snap
    // would overwrite its projection with the scene camera's orthographic one.
    if view.is_none()
        && let Some((height, near, far)) = orthographic_pixel_snap(world, &ids)
        && let Some(ppu) = pixel_snap_ppu(&placed.sprites, assets, &mut atlases)
    {
        let aspect = aspect_ratio(viewport);
        let snapped_height = snap_view_height(height, viewport.1, ppu);
        let half_h = snapped_height * 0.5;
        let half_w = half_h * aspect;
        camera.projection = Mat4::orthographic_rh(-half_w, half_w, -half_h, half_h, near, far);
        let texel = 1.0 / ppu;
        for sprite in &mut sprites {
            sprite.center.x = (sprite.center.x / texel).round() * texel;
            sprite.center.y = (sprite.center.y / texel).round() * texel;
        }
    }

    // Resolve UI elements (Phase 14): screen-space, no world placement, so — unlike sprites/meshes/
    // text — they are resolved directly here rather than through `place()`/`Placed`. `render.bounds`
    // reports only world-space renderables (a rect projected from a real camera), and a screen-space
    // element has no such projection to report; keeping UI out of `Placed` keeps that boundary exact
    // by construction. Textures resolve through the same `textures`/`path_index` dedup as sprites
    // above, so a UI rect sharing a sprite's texture path is one `RenderScene.textures` entry.
    let mut ui_entries: Vec<(i32, EntityId, UiInstance)> = Vec::new();
    for (index, &id) in ids.iter().enumerate() {
        if let Some(rect) = get::<UiRect>(world, id)
            && rect.visible
        {
            let rect_px = ui::resolve_ui_rect(rect.anchor, rect.offset, rect.size, viewport);
            // The UV sub-rect the rect samples. A loose `texture:` or an untextured fill samples the
            // whole image (byte-identical to the pre-atlas path); an `atlas` + `frame` reference
            // narrows it to that region, resolved through the same `resolve_atlas_frame` and the
            // same `textures`/`path_index` dedup a `Sprite` uses — so a run of UI rects sharing one
            // atlas is one texture entry and one batch. `atlas` and `texture` are mutually exclusive
            // (`L1151`, validated); the match order mirrors the sprite path.
            let mut uv = SpriteInstance::FULL_UV;
            let texture = match (rect.atlas.as_deref(), rect.texture.as_deref()) {
                (Some(atlas_rel), _) => resolve_atlas_frame(
                    project_root,
                    atlas_rel,
                    rect.frame.as_deref(),
                    TextureSite::UiRect(index),
                    assets,
                    &mut atlases,
                    &mut textures,
                    &mut path_index,
                    &mut errors,
                )
                .map(|(idx, region_uv)| {
                    uv = region_uv;
                    idx
                }),
                (None, Some(rel)) => resolve_texture(
                    project_root,
                    rel,
                    TextureSite::UiRect(index),
                    assets,
                    &mut textures,
                    &mut path_index,
                    &mut errors,
                ),
                (None, None) => None,
            };
            ui_entries.push((
                rect.order,
                id,
                UiInstance::Rect {
                    rect_px,
                    texture,
                    color: rect.color,
                    uv,
                },
            ));
        }
        if let Some(text) = get::<UiText>(world, id)
            && text.visible
        {
            let face = fonts.face(text.font.as_deref());
            let quads = ui::ui_text_draw(face, text, viewport);
            let font = resolve_font(
                project_root,
                placed_font(&fonts, text.font.as_deref()).as_ref(),
                TextureSite::Font(index, "UiText"),
                assets,
                &mut textures,
                &mut path_index,
                &mut errors,
            );
            ui_entries.push((
                text.order,
                id,
                UiInstance::Text {
                    color: text.color,
                    text: text.text.clone(),
                    quads,
                    font,
                },
            ));
        }
        if let Some(list) = get::<UiList>(world, id)
            && list.visible
        {
            // An empty list draws nothing at all — panel included — independent of `visible`: the
            // emptiness check gates the background too, not just the lines.
            if !list.items.is_empty() {
                let face = fonts.face(list.font.as_deref());
                let (layout, lines) = ui::ui_list_draw(face, list, viewport);
                let font = resolve_font(
                    project_root,
                    placed_font(&fonts, list.font.as_deref()).as_ref(),
                    TextureSite::Font(index, "UiList"),
                    assets,
                    &mut textures,
                    &mut path_index,
                    &mut errors,
                );
                // The background and every line share one `(order, EntityId)` key; the sort below is
                // stable, so emitting the panel first is what puts it behind its own text.
                if list.background[3] > 0.0 {
                    ui_entries.push((
                        list.order,
                        id,
                        UiInstance::Rect {
                            rect_px: layout.background_px,
                            texture: None,
                            color: list.background,
                            uv: SpriteInstance::FULL_UV,
                        },
                    ));
                }
                // Clamped, never a diagnostic: a game that shrinks a list mid-frame must not be able
                // to author an error.
                let selected = (list.selected as usize).min(list.items.len() - 1);
                // The line's own item index comes from `ui_list_lines`, so neither the title offset
                // nor the visible window's start is re-derived here.
                for (item, quads) in lines {
                    let color = match item {
                        None => list.color,
                        // Disabled outranks selected: an authored cursor can sit on a disabled item,
                        // and it must still read as disabled.
                        Some(item) if !list.items[item].enabled => list.disabled_color,
                        Some(item) if item == selected => list.selected_color,
                        Some(_) => list.color,
                    };
                    ui_entries.push((
                        list.order,
                        id,
                        UiInstance::Text {
                            color,
                            text: match item {
                                None => list.title.clone(),
                                Some(index) => list.items[index].text.clone(),
                            },
                            quads,
                            font,
                        },
                    ));
                }
            }
        }
    }
    // Pinned, documented order: ascending `order`, ties broken by ascending `EntityId` — screen
    // space has no view depth to sort by instead. This is the determinism contract for
    // `RenderScene.ui` (mirrors the extract-determinism guarantee every other field already has).
    ui_entries.sort_by_key(|(order, id, _)| (*order, *id));
    let ui: Vec<UiInstance> = ui_entries
        .into_iter()
        .map(|(_, _, instance)| instance)
        .collect();

    // World text resolves its face's atlas here, after the UI, so a face shared by a `UiText` and a
    // `Text` is one `textures` entry.
    let texts: Vec<TextRun> = std::mem::take(&mut placed.texts)
        .into_iter()
        .map(|(_, mut run, font, index)| {
            run.font = resolve_font(
                project_root,
                font.as_ref(),
                TextureSite::Font(index, "Text"),
                assets,
                &mut textures,
                &mut path_index,
                &mut errors,
            );
            run
        })
        .collect();

    // Resolve asset meshes, voxel models, sculpts, and assemblies in the same first-encounter (entity)
    // order, deduplicated by path — so two `Mesh` components pointing at the same document produce one
    // `mesh_assets` entry (one run of entries for an assembly, one per part), and two extractions of
    // one world produce identical `mesh_assets`. All four share this one list (every one becomes a
    // `MeshShape::Asset(index)`); their paths never collide (`meshes/*.mesh.yaml` vs
    // `voxels/*.voxel.yaml` vs `sculpts/*.sculpt.yaml` vs `*.assembly.yaml`). A character resolves
    // to its mesh's path, so it shares that entry with a plain `asset` of the same mesh. Primitives
    // resolve inline in `place`.
    let mut mesh_assets: Vec<Arc<MeshData>> = Vec::new();
    let mut mesh_path_index: StableMap<String, usize> = StableMap::new();
    // An assembly's parts land in `mesh_assets` as one contiguous run, so one first-encounter entry
    // per assembly path — the index of its first part — is the whole dedup index.
    let mut assembly_part_base: StableMap<String, usize> = StableMap::new();

    // Every `SkinBone` part, grouped by the skin entity it names, in ascending entity order — so the
    // first part naming a `(skin, bone)` pair wins if two ever do (the `L1196` case slipping past a
    // hand-built world). A stale skin id, an unknown bone name, or a skin with no joints is never
    // checked here: an entry the mesh loop's asset has no matching joint for is simply never matched
    // by `posed_palette`, the documented runtime degrade to rest.
    let mut skin_followers: StableMap<EntityId, Vec<(&str, Mat4)>> = StableMap::new();
    for &id in &ids {
        let Some(skin_bone) = get::<SkinBone>(world, id) else {
            continue;
        };
        let part_world = render_pose(world, &mut poses, id).model();
        let entries = skin_followers.entry(skin_bone.skin).or_default();
        if !entries.iter().any(|(name, _)| *name == skin_bone.bone) {
            entries.push((skin_bone.bone.as_str(), part_world));
        }
    }

    let mut meshes = Vec::with_capacity(placed.meshes.len());
    // Every skinned instance's bone palette, in extract order; the renderer prepends its own
    // identity entry 0, so an instance's `bone_offset` into the combined buffer starts at 1. A joint
    // a `SkinBone` part names takes that part's world pose every frame; every other joint keeps its
    // rest pose relative to its parent (`posed_palette`); an instance with no followers plays its
    // character's `Animation` clip (`sampled_palette`), or pushes its asset's joints' rest entries
    // verbatim when there is none.
    let mut bone_palette: Vec<Mat4> = Vec::new();
    let mut palette_scratch: Vec<Mat4> = Vec::new();
    let mut last_fade = None;
    for mesh in &placed.meshes {
        // Per-instance fade rides the entity's optional `Fade` component; absent ⇒ the
        // render-identical default `(1.0, -f32::MAX)`, which `mesh.wgsl`'s screen-door discard can
        // never reject. A mesh with no `Fade` of its own takes its nearest ancestor's, so a faded
        // collider root fades its presentation children. Read once per entity: consecutive placed
        // meshes of one entity (a grid's cells, an assembly's parts) fade as one model.
        let fade = match last_fade {
            Some((id, fade)) if id == mesh.id => fade,
            _ => {
                let fade = crate::hierarchy::inherited_fade(world, mesh.id);
                last_fade = Some((mesh.id, fade));
                fade
            }
        };
        let mut character: Option<Arc<CharacterData>> = None;
        let shape = match mesh.source {
            PlacedMeshSource::Primitive(shape) => Some(shape),
            PlacedMeshSource::Asset(rel) => resolve_mesh(
                rel,
                mesh.index,
                mesh.site,
                MeshKind::Asset,
                assets,
                &mut mesh_assets,
                &mut mesh_path_index,
                &mut errors,
            )
            .map(MeshShape::Asset),
            PlacedMeshSource::Voxel(rel) => resolve_mesh(
                rel,
                mesh.index,
                mesh.site,
                MeshKind::Voxel,
                assets,
                &mut mesh_assets,
                &mut mesh_path_index,
                &mut errors,
            )
            .map(MeshShape::Asset),
            PlacedMeshSource::Sculpt(rel) => resolve_mesh(
                rel,
                mesh.index,
                mesh.site,
                MeshKind::Sculpt,
                assets,
                &mut mesh_assets,
                &mut mesh_path_index,
                &mut errors,
            )
            .map(MeshShape::Asset),
            // A character draws its skinned mesh, deduplicated by the mesh path so a plain `asset`
            // of the same mesh shares the one upload.
            PlacedMeshSource::Character(rel) => {
                let pointer = mesh
                    .site
                    .shape_pointer(mesh.index, MeshKind::Character.shape_field());
                match assets.character(rel) {
                    Ok(data) => {
                        let shape = resolve_mesh(
                            &data.mesh,
                            mesh.index,
                            mesh.site,
                            MeshKind::Character,
                            assets,
                            &mut mesh_assets,
                            &mut mesh_path_index,
                            &mut errors,
                        )
                        .map(MeshShape::Asset);
                        character = Some(data);
                        shape
                    }
                    Err(diagnostics) => {
                        for diagnostic in diagnostics {
                            errors.push(mesh_error(rel, "character", &pointer, diagnostic));
                        }
                        None
                    }
                }
            }
            // One placement, N instances: an assembly's parts are pushed here rather than resolved to
            // a single shape. They go in the assembly's own authored order, inside the ascending-slot
            // walk, so the instance order is (ascending entity, then authored part).
            PlacedMeshSource::Assembly(rel) => {
                let Some((data, base)) = resolve_assembly(
                    rel,
                    mesh.index,
                    mesh.site,
                    "assembly",
                    assets,
                    &mut mesh_assets,
                    &mut assembly_part_base,
                    &mut errors,
                ) else {
                    continue;
                };
                let (pose, crossfade) = animation_pose(world, mesh.id, &data, now);
                for (part, (local, local_normal)) in
                    part_placements_faded(&data, pose, crossfade.as_ref())
                        .into_iter()
                        .enumerate()
                {
                    let index = base + part;
                    meshes.push(MeshInstance {
                        shape: MeshShape::Asset(index),
                        model: mesh.model * local,
                        normal_matrix: mesh.normal_matrix * local_normal,
                        color: mesh.color,
                        material: mesh.material,
                        use_vertex_material: mesh_assets[index].authored_material,
                        fade: fade.fade,
                        fade_height_min: fade.height_min,
                        flash: mesh.flash,
                        flash_color: mesh.flash_color,
                        bone_offset: 0,
                    });
                }
                continue;
            }
            PlacedMeshSource::AssemblyPart(rel, part) => resolve_assembly_part(
                rel,
                part,
                mesh.index,
                mesh.site,
                assets,
                &mut mesh_assets,
                &mut assembly_part_base,
                &mut errors,
            )
            .map(MeshShape::Asset),
        };
        if let Some(shape) = shape {
            // Selection, not multiplication: a mesh whose geometry authors its own material uses it
            // entirely; the component's material fields apply only to meshes that author none
            // (primitives, plain asset meshes). Resolved per-mesh into the shader's branch-free flag.
            let use_vertex_material = match shape {
                MeshShape::Asset(idx) => mesh_assets[idx].authored_material,
                _ => false,
            };
            let bone_offset = match shape {
                MeshShape::Asset(idx) if !mesh_assets[idx].skin_joints.is_empty() => {
                    // Entry 0 is the renderer's identity, so an asset's own entries start at 1.
                    let offset = 1 + bone_palette.len() as u32;
                    let joints = &mesh_assets[idx].skin_joints;
                    let (pose, crossfade) = character.as_deref().map_or((None, None), |data| {
                        character_pose(world, mesh.id, data, now)
                    });
                    match skin_followers.get(&mesh.id) {
                        Some(followed) if !followed.is_empty() => {
                            posed_palette(
                                joints,
                                mesh.model,
                                followed,
                                &mut palette_scratch,
                                &mut bone_palette,
                            );
                        }
                        _ if pose.is_none() && crossfade.is_none() => {
                            bone_palette.extend(joints.iter().map(|joint| joint.rest));
                        }
                        _ => sampled_palette(
                            joints,
                            pose,
                            crossfade.as_ref(),
                            &mut palette_scratch,
                            &mut bone_palette,
                        ),
                    }
                    offset
                }
                _ => 0,
            };
            meshes.push(MeshInstance {
                shape,
                model: mesh.model,
                normal_matrix: mesh.normal_matrix,
                color: mesh.color,
                material: mesh.material,
                use_vertex_material,
                fade: fade.fade,
                fade_height_min: fade.height_min,
                flash: mesh.flash,
                flash_color: mesh.flash_color,
                bone_offset,
            });
        }
    }

    // A `MeshGrid` places one mesh per cell, so a legend entry that fails to load fails once per
    // cell, identically; report each exact (code, pointer, message) once.
    let mut seen = std::collections::BTreeSet::new();
    errors.retain(|e| {
        seen.insert((
            e.code.as_str().to_string(),
            e.path.clone(),
            e.message.clone(),
        ))
    });

    if !errors.is_empty() {
        return Err(errors);
    }

    // The scene's `Environment` (if any) is the one lighting surface: its authored `ambient`
    // supersedes the historical constant. Absent ⇒ `AMBIENT` byte-for-byte. The default
    // `Environment.ambient` is zero, not `AMBIENT`: under an `Environment` the sky's IBL is the ambient.
    let environment = extract_environment(world, &ids);
    let ambient = environment.map_or(AMBIENT, |env| env.ambient);

    // The GI voxelizer is opt-in and its absence is the historical path: with no `GiVolume` the
    // grid is never built and nothing downstream has one to read. It runs off `placed.meshes` —
    // the same ascending-entity-slot walk everything else here consumes — because that order is
    // what makes its last-writer combine rule deterministic. An unchanged instance set hits the
    // loader's voxelization cache and rasterizes nothing.
    let (voxel, gi) = match extract_gi_volume(world, &ids) {
        None => (None, None),
        Some((index, volume)) => {
            assets.observe_begin(LoadPhase::Voxelize);
            let voxel = crate::voxelizer::voxelize(&placed.meshes, volume.cell_size, assets);
            assets.observe_end(LoadPhase::Voxelize);
            let voxel = voxel.map_err(|diagnostics| gi_volume_errors(diagnostics, index))?;
            let gi = match voxel.as_deref() {
                None => None,
                Some(grid) => Some(gi_data(grid, &volume).map_err(|diagnostic| {
                    vec![diagnostic.with_path(format!(
                        "/entities/{index}/components/GiVolume/probe_spacing"
                    ))]
                })?),
            };
            (voxel, gi)
        }
    };

    Ok(RenderScene {
        camera,
        ambient,
        textures,
        mesh_assets,
        meshes,
        sprites,
        ground_markers: extract_ground_markers(world, &ids, &mut poses),
        debug_lines: Vec::new(),
        particles: extract_particles(
            world,
            &ids,
            &mut poses,
            now,
            particle_density,
            CameraBasis::from_view(camera.view),
        ),
        lights: extract_lights(placed.lights),
        spot_lights: extract_spot_lights(placed.spot_lights),
        sun: extract_sun(world, &ids),
        environment,
        post: extract_post(world, &ids),
        texts,
        ui,
        voxel,
        gi,
        bone_palette,
    })
}

/// The renderer's [`GiData`] for a finished grid: the authored knobs verbatim, plus the probe
/// lattice the voxelizer sizes over the grid's own box.
///
/// # Errors
/// `L2105` if `probe_spacing` lays more probes than the bake will allocate. The diagnostic carries
/// no path — the caller points it at the authored field.
fn gi_data(grid: &WorldVoxelGrid, volume: &GiVolume) -> Result<GiData, Diagnostic> {
    let lattice = crate::voxelizer::probe_lattice(grid, volume.probe_spacing)?;
    Ok(GiData {
        probe_spacing: volume.probe_spacing,
        normal_bias: volume.normal_bias,
        sky_visibility: volume.sky_visibility,
        bounce: volume.bounce,
        count: lattice.count,
        origin: grid.origin,
        span: lattice.span,
    })
}

/// The scene's single [`GiVolume`] (the GI voxelizer's opt-in switch) with its entity's position in
/// the extract walk, or `None` — the [`extract_post`] posture: the first in the pinned
/// ascending-slot walk wins, the rest are ignored, and absence is meaningful (nothing is
/// voxelized). The index is carried so a sizing error can point at the authored `cell_size`.
fn extract_gi_volume(world: &World, ids: &[EntityId]) -> Option<(usize, GiVolume)> {
    ids.iter()
        .enumerate()
        .find_map(|(index, &id)| get::<GiVolume>(world, id).map(|gi| (index, *gi)))
}

/// Points a voxelizer diagnostic that carries no path of its own at the authored
/// `GiVolume.cell_size` — the sizing error's cause. A diagnostic that already names a site (a voxel
/// document re-pointed at its `Mesh` component) keeps it.
fn gi_volume_errors(diagnostics: Vec<Diagnostic>, index: usize) -> Vec<Diagnostic> {
    diagnostics
        .into_iter()
        .map(|diagnostic| {
            if diagnostic.path.is_some() {
                diagnostic
            } else {
                diagnostic.with_path(format!("/entities/{index}/components/GiVolume/cell_size"))
            }
        })
        .collect()
}

/// The scene's single directional "sun", or `None`. Takes the first `DirectionalLight` in the
/// pinned ascending-slot walk (the lowest live `EntityId` — the same deterministic order everything
/// else uses) and ignores any others: the 16-point-light-cap posture, no error (plan decision 5).
/// Produces plain [`SunData`] for `RenderScene.sun`: `mesh.wgsl` reads it as the directional light
/// term and as the light-space matrix the shadow pass renders and samples through.
fn extract_sun(world: &World, ids: &[EntityId]) -> Option<SunData> {
    let (id, sun) = ids
        .iter()
        .find_map(|&id| get::<DirectionalLight>(world, id).map(|sun| (id, sun)))?;
    let pose = render_pose(world, &mut WorldAffines::new(), id);
    Some(build_sun(sun, Vec3::from(pose.translation())))
}

/// The scene's single sky/lighting [`Environment`], or `None`. Takes the first `Environment` in the
/// pinned ascending-slot walk (the lowest live `EntityId` — the `DirectionalLight` sun precedent) and
/// ignores any others; absence is meaningful (the engine keeps its historical ambient/clear
/// pipeline). Produces plain [`EnvironmentData`] for `RenderScene.environment`, which drives the sky
/// pass and the prefiltered IBL `mesh.wgsl` samples.
fn extract_environment(world: &World, ids: &[EntityId]) -> Option<EnvironmentData> {
    let env = ids.iter().find_map(|&id| get::<Environment>(world, id))?;
    Some(EnvironmentData {
        sky_zenith: env.sky_zenith,
        sky_horizon: env.sky_horizon,
        sky_ground: env.sky_ground,
        sun_disc_size: env.sun_disc_size,
        sun_disc_intensity: env.sun_disc_intensity,
        ambient: env.ambient,
        ibl_intensity: env.ibl_intensity,
    })
}

/// The scene's single [`PostProcess`] (imaging) singleton, or `None` — the `extract_environment`
/// posture: the first `PostProcess` in the pinned ascending-slot walk (the lowest live `EntityId`)
/// wins, the rest are ignored, and absence is meaningful (the renderer falls back to its own shipped
/// post defaults, which are field for field the ones `PostProcess::default()` carries — a test in
/// `ludion-renderer` pins the two together).
fn extract_post(world: &World, ids: &[EntityId]) -> Option<PostData> {
    let post = ids.iter().find_map(|&id| get::<PostProcess>(world, id))?;
    Some(PostData {
        bloom_threshold: post.bloom_threshold,
        bloom_strength: post.bloom_strength,
        exposure: post.exposure,
        tonemap: match post.tonemap {
            TonemapOperator::Agx => ludion_renderer::TonemapOperator::Agx,
            TonemapOperator::Aces => ludion_renderer::TonemapOperator::Aces,
            TonemapOperator::Neutral => ludion_renderer::TonemapOperator::Neutral,
            TonemapOperator::None => ludion_renderer::TonemapOperator::None,
        },
        dither: post.dither,
        contrast: post.contrast,
        saturation: post.saturation,
        temperature: post.temperature,
        tint: post.tint,
        lift: post.lift,
        gamma: post.gamma,
        gain: post.gain,
        fog_density: post.fog_density,
        fog_color: post.fog_color,
        fog_start: post.fog_start,
        fog_height: post.fog_height,
        fog_height_falloff: post.fog_height_falloff,
        fog_albedo: post.fog_albedo,
        fog_sky: post.fog_sky,
        fog_anisotropy: post.fog_anisotropy,
        vignette: post.vignette,
        vignette_radius: post.vignette_radius,
        ssao: post.ssao,
        ssao_radius: post.ssao_radius,
        ssao_bias: post.ssao_bias,
        ssao_direct: post.ssao_direct,
        shafts: post.shafts,
        dof: post.dof,
        focus_distance: post.focus_distance,
        chromatic_aberration: post.chromatic_aberration,
        halation: post.halation,
        grain: post.grain,
        grain_size: post.grain_size,
        ssr: post.ssr,
        ssr_fade: post.ssr_fade,
        ssr_max_distance: post.ssr_max_distance,
        ssr_thickness: post.ssr_thickness,
    })
}

/// Drops the placement tags from the collected point lights, handing the renderer **every** light
/// the scene placed. There is no cap and no selection: `Plan/clustered-forward.md` decision 2
/// deleted both, so a scene may author any number of lights and all of them reach the GPU.
///
/// **The order is the contract, and it is load-bearing** (the plan's identity argument). The list
/// keeps the exact ascending-`EntityId` extract order it was collected in — *unsorted*. The renderer
/// uploads it verbatim as the global light array, and a fragment sums its cluster cell's lights in
/// ascending index order, so this order **is** the per-fragment floating-point summation order of
/// every lit pixel. Reordering here (or "simplifying" into a sort of any kind) changes that sum and
/// churns every lit golden; it would also break the clustered loop's proof that it is arithmetically
/// identical to a flat loop over the whole array.
fn extract_lights(lights: Vec<(EntityId, PointLightData)>) -> Vec<PointLightData> {
    lights.into_iter().map(|(_, light)| light).collect()
}

/// The spot lights in the same unsorted ascending-`EntityId` extract order as [`extract_lights`];
/// the renderer packs them after every point light, so this order is their summation order.
fn extract_spot_lights(lights: Vec<(EntityId, SpotLightData)>) -> Vec<SpotLightData> {
    lights.into_iter().map(|(_, light)| light).collect()
}

/// Every [`GroundMarker`] in the world, resolved against its [`Transform`] into the renderer's
/// [`GroundMarkerInstance`] — in the ascending-`EntityId` walk order `ids` already carries, which
/// is the markers' blend order and therefore deterministic by construction.
///
/// Two things are baked here so the renderer interprets nothing:
/// - **Dimensions are final world units.** The authored shape is the marker's maximum extent, and
///   the entity's **uniform** `Transform.scale.x` multiplies it — a warmup filling in is a scale
///   write, so the size must reach the shader as a length, not as a matrix the signed-distance
///   evaluation would have to undo. Only `scale.x` is read (`scale.y`/`scale.z` are ignored, so a
///   non-uniform scale never distorts the footprint); a negative `scale.x` clamps to `0.0`, the same
///   invisible-marker outcome a zero scale already produces, rather than mirroring the shape.
/// - **The model matrix is rigid** for the same reason: rotation and translation only, with
///   [`GROUND_MARKER_LIFT`] added to Y so the quad never z-fights the floor it lies on.
///
/// Unlike sprites, meshes and text, markers do not travel through [`place`]: `render.bounds` reports
/// the scene's *content*, and a telegraph is an overlay drawn on top of it rather than a thing that
/// occupies the world.
fn extract_ground_markers(
    world: &World,
    ids: &[EntityId],
    poses: &mut WorldAffines,
) -> Vec<GroundMarkerInstance> {
    ids.iter()
        .filter_map(|&id| {
            let marker = get::<GroundMarker>(world, id)?;
            let pose = render_pose(world, poses, id);
            // A negative scale has no meaningful footprint to draw; clamp to the existing
            // zero-coverage behaviour (an invisible marker) rather than mirroring garbage geometry.
            let scale = pose.scale()[0].max(0.0);
            let shape = match marker.shape {
                GroundMarkerShape::Disc { radius } => ludion_renderer::GroundMarkerShape::Disc {
                    radius: radius * scale,
                },
                GroundMarkerShape::Segment { length, width } => {
                    ludion_renderer::GroundMarkerShape::Segment {
                        length: length * scale,
                        width: width * scale,
                    }
                }
            };
            let translation = Vec3::from(pose.translation()) + Vec3::Y * GROUND_MARKER_LIFT;
            Some(GroundMarkerInstance {
                shape,
                model: Mat4::from_rotation_translation(pose.quat(), translation),
                color: marker.color,
            })
        })
        .collect()
}

/// Every particle of every [`ParticleEmitter`] in the world at `now`, in
/// `(ascending EntityId, particle index)` order — the order [`ParticleInstance`] documents, and the
/// only order in the frame, since the renderer sorts a copy of its own.
///
/// This is a **pure read**, and pure in the strong sense: [`particles::evaluate`] is a closed-form
/// function of the emitter, its transform and `now`, so nothing is spawned, stepped or written back.
/// A world holding emitters therefore hashes and records exactly as one without them, and pausing an
/// engine freezes the plume rather than reshuffling it.
///
/// Emitters evaluate in parallel through [`par::par_map_chunks`], whose results come back in chunk
/// order and are concatenated in it, so the list is the same at any thread count and the order
/// above holds. A frame with fewer than [`PARTICLE_PAR_MIN_LIVE`] live particles runs the same code
/// on one thread, paying no spawn.
///
/// `particle_density` is the machine's [`QualityParams::particle_density`], spent here and nowhere
/// else: it scales every emitter's `rate` and `burst` before the live window is computed, so this is
/// the whole of that axis. The purity above survives it — the density is an argument like `now`, not
/// a read of any ambient state — so a thinned plume is still the same pure function of the world,
/// evaluated with a different constant.
///
/// Like ground markers, emitters do not travel through [`place`]: `render.bounds` reports the
/// scene's *content*, and a plume is dressing over it.
fn extract_particles(
    world: &World,
    ids: &[EntityId],
    poses: &mut WorldAffines,
    now: SimTime,
    particle_density: f32,
    basis: CameraBasis,
) -> Vec<ParticleInstance> {
    let mut emitters = Vec::new();
    let mut live = 0;
    for &id in ids {
        let Some(emitter) = get::<ParticleEmitter>(world, id) else {
            continue;
        };
        let pose = render_pose(world, poses, id);
        live += particles::live_bound(emitter, now, particle_density);
        emitters.push((emitter, pose.transform()));
    }

    let evaluate = || {
        par::par_map_chunks(&emitters, PARTICLE_PAR_CHUNK, |_, chunk| {
            chunk
                .iter()
                .flat_map(|(emitter, transform)| {
                    particles::evaluate(emitter, transform, now, particle_density, basis)
                })
                .collect::<Vec<_>>()
        })
    };
    let lists = if live < PARTICLE_PAR_MIN_LIVE {
        par::with_threads(1, evaluate)
    } else {
        evaluate()
    };

    let mut out = Vec::with_capacity(lists.iter().map(Vec::len).sum());
    for list in lists {
        out.extend(list);
    }
    out
}

/// One emitter per chunk: emitters differ wildly in size, so workers claim them one at a time.
const PARTICLE_PAR_CHUNK: usize = 1;

/// Below this many live particles a frame stays on one thread: a parallel call spawns fresh scoped
/// threads, and 4096 (one emitter at its cap) is ~0.4 ms of work at ~0.1 µs a particle against
/// ~0.1 ms of spawns.
const PARTICLE_PAR_MIN_LIVE: usize = 4096;

/// Appends one [`SpriteInstance`] per live bullet of every [`Volley`], in
/// `(ascending EntityId, bullet index)` order.
///
/// The volley's `Transform` is the origin captured at fire time and its bullet kind comes from the
/// `kind: pattern` document its `pattern` names — a store or document that is not loaded draws
/// nothing (`advance_volleys` is the seam that reports it, once, and reaps the volley). Everything
/// else is the ordinary sprite path: the kind's `size` laid along the game plane's two axes, or the
/// camera's when `billboard`, the same [`resolve_atlas_frame`] dedup. A kind whose atlas reference
/// fails draws the untextured quad and lands its `L1151`/`L1152` in `degraded`, once per kind per
/// extract — a pattern author's mid-tune atlas edit must not blank the frame.
///
/// Like ground markers and particles, bullets do not travel through [`place`]: `render.bounds`
/// reports the scene's content, and a volley is transient.
#[allow(clippy::too_many_arguments)]
fn extract_volleys(
    world: &World,
    ids: &[EntityId],
    project_root: &Path,
    assets: &mut dyn SceneAssets,
    atlases: &mut AtlasCache,
    poses: &mut WorldAffines,
    textures: &mut Vec<Arc<TextureImage>>,
    path_index: &mut StableMap<String, usize>,
    degraded: &mut Vec<(String, Diagnostic)>,
    now: SimTime,
    basis: CameraBasis,
    out: &mut Vec<SpriteInstance>,
) {
    let Some(store) = world.resource::<DocStore<PatternDoc>>() else {
        return;
    };
    let plane = ludion_spatial::Plane::of(world);
    let (plane_right, plane_up) = (Vec3::from(plane.right()), Vec3::from(plane.forward()));

    // One resolution per bullet kind: `(pattern, bullet)` to `(uv, texture)`.
    type Resolved = ([f32; 4], Option<usize>);
    let mut resolved: BTreeMap<(&str, &str), Resolved> = BTreeMap::new();

    // Volleys are found through the ECS's ascending presence cursor — the world hash's own walk —
    // rather than by probing every entity for a component almost none of them carry. `extract` holds
    // `&World`, where a typed query (which needs `&mut`) cannot reach. `ids` is the live-entity list
    // in ascending slot order, so a volley's position in it — the scene index a texture diagnostic
    // points at — is one binary search away, and the enumeration order is unchanged.
    for slot in world.entity_indices_with(TypeId::of::<Volley>()) {
        let Ok(index) = ids.binary_search_by_key(&slot, |id| id.index()) else {
            continue;
        };
        let id = ids[index];
        let Some(volley) = get::<Volley>(world, id) else {
            continue;
        };
        let kind = store
            .get(&volley.pattern)
            .and_then(|doc| doc.bullets.get(&volley.bullet));
        let Some(kind) = kind else {
            continue;
        };
        let origin = Vec3::from(render_pose(world, poses, id).translation());
        let (right, up) = if kind.billboard {
            (basis.right, basis.up)
        } else {
            (plane_right, plane_up)
        };
        let (uv, texture) = *resolved
            .entry((volley.pattern.as_str(), volley.bullet.as_str()))
            .or_insert_with(|| {
                bullet_texture(
                    project_root,
                    kind,
                    (&volley.pattern, &volley.bullet),
                    TextureSite::Volley(index),
                    assets,
                    atlases,
                    textures,
                    path_index,
                    degraded,
                )
            });
        let age = patterns::bullet_age(volley, now);
        for bullet in 0..volley.count {
            if !patterns::bullet_is_alive(volley, kind, bullet, now) {
                continue;
            }
            out.push(SpriteInstance {
                center: patterns::bullet_motion(volley, origin, kind, bullet, plane, now.rate())
                    .position_at(age),
                right: right * kind.size[0],
                up: up * kind.size[1],
                color: kind.color,
                texture,
                uv,
                lit: false,
                layer: kind.layer,
                additive: kind.blend == SpriteBlend::Additive,
                flash: 0.0,
                flash_color: [1.0; 3],
                uv_repeat: SpriteInstance::NO_TILING,
                uv_offset: [0.0; 2],
            });
        }
    }
}

/// The `(uv, texture index)` one bullet kind's quads sample: its atlas region, or the whole white
/// quad when it names none or its reference fails. Resolved once per kind per extract; a failure is
/// re-pointed from the volley's scene position to the document field
/// `pattern/<doc>/bullets/<name>` (the validator's own key) and pushed to `degraded` instead of
/// failing the frame.
#[allow(clippy::too_many_arguments)]
fn bullet_texture(
    project_root: &Path,
    kind: &BulletKind,
    (pattern, name): (&str, &str),
    site: TextureSite,
    assets: &mut dyn SceneAssets,
    atlases: &mut AtlasCache,
    textures: &mut Vec<Arc<TextureImage>>,
    path_index: &mut StableMap<String, usize>,
    degraded: &mut Vec<(String, Diagnostic)>,
) -> ([f32; 4], Option<usize>) {
    let mut failed = Vec::new();
    let resolved = match (kind.atlas.as_deref(), kind.frame.as_deref()) {
        (Some(rel), frame) => resolve_atlas_frame(
            project_root,
            rel,
            frame,
            site,
            assets,
            atlases,
            textures,
            path_index,
            &mut failed,
        ),
        (None, Some(_)) => {
            failed.push(frame_without_atlas(&site.base_pointer()));
            None
        }
        (None, None) => None,
    };
    let (site_base, doc_base) = (
        site.base_pointer(),
        format!("pattern/{pattern}/bullets/{name}"),
    );
    for mut d in failed {
        if let Some(tail) = d.path.as_deref().and_then(|p| p.strip_prefix(&site_base)) {
            d.path = Some(format!("{doc_base}{tail}"));
        }
        d.message
            .push_str("; this bullet kind draws untextured until the reference resolves");
        degraded.push((d.path.clone().unwrap_or_else(|| d.message.clone()), d));
    }
    match resolved {
        Some((index, uv)) => (uv, Some(index)),
        None => (SpriteInstance::FULL_UV, None),
    }
}

/// Builds [`PointLightData`] from a `PointLight` placed at `position`.
///
/// A range that is not finite and positive has no light to cast, so it yields `None`; colour and
/// intensity clamp to non-negative (`f32::max` drops a NaN), so a Rust-built value that skipped
/// validation never subtracts light.
fn point_light_data(light: &PointLight, position: Vec3, dynamic: bool) -> Option<PointLightData> {
    if !(light.range.is_finite() && light.range > 0.0) {
        return None;
    }
    Some(PointLightData {
        position,
        color: Vec3::from(light.color).max(Vec3::ZERO),
        intensity: light.intensity.max(0.0),
        range: light.range,
        dynamic,
    })
}

/// Builds [`SpotLightData`] from a `SpotLight` placed at `position`.
///
/// Like [`point_light_data`], an unusable range yields `None` and colour and intensity clamp to
/// non-negative; a zero-length or non-finite direction has no axis, so it also yields `None`. The
/// angles clamp into `[0, 89.9]` degrees with the inner never past the outer, so the cosines stay
/// positive and ordered for a Rust-built value that skipped validation (`L1272`).
fn spot_light_data(light: &SpotLight, position: Vec3) -> Option<SpotLightData> {
    if !(light.range.is_finite() && light.range > 0.0) {
        return None;
    }
    let direction = Vec3::from(light.direction).try_normalize()?;
    let outer = light.outer_angle.clamp(0.0, 89.9);
    let inner = light.inner_angle.clamp(0.0, 89.9).min(outer);
    Some(SpotLightData {
        position,
        direction,
        color: Vec3::from(light.color).max(Vec3::ZERO),
        intensity: light.intensity.max(0.0),
        range: light.range,
        cos_inner: ludion_core::math::cos(inner.to_radians()),
        cos_outer: ludion_core::math::cos(outer.to_radians()),
    })
}

/// Builds [`SunData`] from a `DirectionalLight` and its Transform-anchored frustum center. Pure and
/// deterministic (bit-exact CPU, unit-tested with pinned values). Mirrors [`build_camera`]'s glam
/// calls — `look_to_rh` + `orthographic_rh`, the wgpu 0..1 depth convention — so the light-space
/// matrix uses the one NDC convention the renderer already assumes.
fn build_sun(sun: &DirectionalLight, center: Vec3) -> SunData {
    // A zero-length or non-finite direction, a non-positive extent or an empty depth range is
    // rejected upstream by the component's semantic validator (`L1123`); a game that constructs the
    // component in Rust bypasses that, so extract stays panic-free by falling back to the default
    // direction and frustum rather than emitting a NaN matrix (the normal-matrix zero-determinant
    // fallback posture).
    let direction = Vec3::from(sun.direction)
        .try_normalize()
        .unwrap_or_else(|| Vec3::from(DirectionalLight::default().direction).normalize());

    let defaults = DirectionalLight::default();
    let (extent, near, far) = if sun.shadow_extent.is_finite()
        && sun.shadow_extent > 0.0
        && sun.shadow_near.is_finite()
        && sun.shadow_far.is_finite()
        && sun.shadow_near < sun.shadow_far
    {
        (sun.shadow_extent, sun.shadow_near, sun.shadow_far)
    } else {
        (
            defaults.shadow_extent,
            defaults.shadow_near,
            defaults.shadow_far,
        )
    };
    // Eye placed back along −direction so the ortho [near, far] volume brackets the center (which
    // sits at the midpoint of the depth range). The view is built from the direction, not from a
    // `center − eye` target: that difference is zero for a symmetric range (`near + far == 0`) and
    // flips the view for a negative midpoint. The up degenerates when the light points along ±Y
    // (up ∥ view direction); fall back to +Z there — deterministic, no NaN.
    let back = 0.5 * (near + far);
    let eye = center - direction * back;
    let up = if direction.y.abs() > 0.999 {
        Vec3::Z
    } else {
        Vec3::Y
    };
    let view = Mat4::look_to_rh(eye, direction, up);
    let projection = Mat4::orthographic_rh(-extent, extent, -extent, extent, near, far);

    SunData {
        direction,
        color: (Vec3::from(sun.color) * sun.intensity).max(Vec3::ZERO),
        light_space: projection * view,
        bias: sun.shadow_bias / (far - near),
    }
}

/// The world-space geometry of every renderable entity — the shared output that both [`extract`]
/// (→ [`RenderScene`]) and [`render.bounds`](crate::bounds) derive from, so the two can never
/// disagree on where or how big anything is. If they did, the reported bounds would lie, which is
/// worse than no bounds; funnelling both through one placement pass makes that divergence
/// unrepresentable.
///
/// Textures are left as unresolved paths: a sprite's *geometry* does not depend on whether its
/// texture loads, and `render.bounds` must answer even for a scene with a missing asset, so path→index
/// resolution and its IO errors stay in [`extract`].
///
/// The one thing placement does read off disk is an **atlas region's pixel size**, because a `Sprite`
/// naming an atlas with no authored `size` is sized by its own art and no amount of world reading can
/// know that. It goes through the same [`AtlasCache`] the extract then resolves UV rects from — one
/// read, one derivation — and a reference that does not resolve falls back to the untextured quad's
/// `[1, 1]` here and is reported as `L1151`/`L1152` there. So a bound still answers for a broken
/// scene, and for a working one it is exactly what is drawn.
///
/// Placement borrows the world for the duration of one extract: asset paths stay `&str` into the
/// components rather than per-frame `String` copies (a tilemap places one quad per cell).
pub(crate) struct Placed<'w> {
    /// Sprite/billboard quads, each tagged with its entity (and where its texture reference lives).
    pub(crate) sprites: Vec<PlacedSprite<'w>>,
    /// Mesh placements, each tagged with its entity and carrying an unresolved shape source.
    pub(crate) meshes: Vec<PlacedMesh<'w>>,
    /// Text runs, each tagged with its entity, its unresolved face atlas (`TextRun::font` is filled
    /// when extract resolves it) and its entity index (the error-pointer site).
    pub(crate) texts: Vec<(EntityId, TextRun, Option<PlacedFont>, usize)>,
    /// Point lights, each tagged with its entity — not a bounds target (no visible geometry). The
    /// `EntityId` is what fixes the collection order [`extract_lights`] then hands on verbatim.
    pub(crate) lights: Vec<(EntityId, PointLightData)>,
    /// Spot lights, collected like `lights` and handed on in the same ascending-`EntityId` order.
    pub(crate) spot_lights: Vec<(EntityId, SpotLightData)>,
}

/// One placed mesh: its entity, world transform, flat color, and unresolved geometry source. Like a
/// [`PlacedSprite`]'s texture, a mesh's *geometry* is left unresolved here — a mesh's world transform
/// (and therefore `render.bounds`) does not depend on whether an asset document loads, so asset
/// resolution and its errors stay in [`extract`].
pub(crate) struct PlacedMesh<'w> {
    /// The entity this mesh belongs to.
    pub(crate) id: EntityId,
    /// The entity's position in the extract walk — the `/entities/<index>` segment of an
    /// asset/voxel-load error's JSON Pointer, formatted only if a load actually fails.
    pub(crate) index: usize,
    /// Which component (or legend entry) the shape came from, for the load-error pointer.
    pub(crate) site: MeshSite,
    /// Local→world transform.
    pub(crate) model: Mat4,
    /// Flat RGBA color (linear).
    pub(crate) color: [f32; 4],
    /// The `Mesh` component's material `(roughness, metallic, emissive)`. Applied only when the
    /// resolved geometry authors no material of its own (selected at [`extract`]).
    pub(crate) material: [f32; 3],
    /// The `Mesh` component's hit-flash `(amount, color)`. `amount == 0.0` is render-identical.
    pub(crate) flash: f32,
    /// The colour the hit-flash lerps toward.
    pub(crate) flash_color: [f32; 3],
    /// The normal matrix (object→world for normals): the model's upper-3×3 for a uniform scale, its
    /// inverse-transpose otherwise. See [`normal_matrix`].
    pub(crate) normal_matrix: Mat3,
    /// Where the geometry comes from — a primitive shape (resolved) or an unresolved asset path.
    pub(crate) source: PlacedMeshSource<'w>,
    /// Whether this entity carries the `GiDynamic` marker. Read once per entity in [`place`] and
    /// carried here purely for the voxelizer to filter on (`Plan/gi-wave3-dynamic-injection.md`
    /// §3.2): a marked instance still renders normally (this flag plays no part in building
    /// `RenderScene.meshes`) but is excluded from the voxelizer's instance collection and — because
    /// exclusion happens before the signature is hashed — from its instance signature too, so a
    /// marked mesh's motion cannot perturb the signature at all.
    pub(crate) dynamic: bool,
}

/// A [`PlacedMesh`]'s unresolved geometry: a built-in primitive (already a [`MeshShape`]), a mesh
/// asset path, a voxel model path, or an assembly path — all but the primitive resolved to
/// `mesh_assets` indices by [`extract`].
#[derive(Clone, Copy)]
pub(crate) enum PlacedMeshSource<'w> {
    /// A built-in procedural primitive.
    Primitive(MeshShape),
    /// An unresolved mesh-asset path (`meshes/*.mesh.yaml`), relative to the content root.
    Asset(&'w str),
    /// An unresolved voxel-model path (`voxels/*.voxel.yaml`), relative to the content root.
    Voxel(&'w str),
    /// An unresolved sculpt-model path (`sculpts/*.sculpt.yaml`), relative to the content root.
    Sculpt(&'w str),
    /// An unresolved character path (`*.character.yaml`), relative to the content root; it resolves
    /// to its skinned mesh's `mesh_assets` index.
    Character(&'w str),
    /// An unresolved assembly path (`*.assembly.yaml`), relative to the content root. The one source
    /// that resolves to *several* meshes — one per part — all sharing this placement's transform.
    Assembly(&'w str),
    /// One named part (the second field) of an unresolved assembly path (the first), relative to
    /// the content root: resolves to exactly one mesh, sharing the assembly's part run.
    AssemblyPart(&'w str, &'w str),
}

/// The unresolved geometry source a `Mesh.shape` places.
fn placed_source(shape: &MeshSource) -> PlacedMeshSource<'_> {
    match shape {
        MeshSource::Cube => PlacedMeshSource::Primitive(MeshShape::Cube),
        MeshSource::Plane => PlacedMeshSource::Primitive(MeshShape::Plane),
        MeshSource::Sphere => PlacedMeshSource::Primitive(MeshShape::Sphere),
        MeshSource::Asset(path) => PlacedMeshSource::Asset(path),
        MeshSource::Voxel(path) => PlacedMeshSource::Voxel(path),
        MeshSource::Assembly(path) => PlacedMeshSource::Assembly(path),
        MeshSource::Sculpt(path) => PlacedMeshSource::Sculpt(path),
        MeshSource::Character(path) => PlacedMeshSource::Character(path),
        MeshSource::AssemblyPart { assembly, part } => {
            PlacedMeshSource::AssemblyPart(assembly, part)
        }
    }
}

/// The 3×3 of each `MeshGrid` orientation glyph `0..=7`: `R_y(90°·k) · diag(-1, 1, 1)` for `4..=7`
/// (a mirror of local X, then the turn), `R_y(90°·k)` for `0..=3`. Exact 0/±1 entries, so a cell's
/// matrix is its own inverse-transpose; a test pins them to `Transform.rotation`/`scale`.
const CELL_ORIENTATIONS: [Mat3; 8] = [
    Mat3::from_cols(Vec3::new(1.0, 0.0, 0.0), Vec3::Y, Vec3::new(0.0, 0.0, 1.0)),
    Mat3::from_cols(Vec3::new(0.0, 0.0, -1.0), Vec3::Y, Vec3::new(1.0, 0.0, 0.0)),
    Mat3::from_cols(
        Vec3::new(-1.0, 0.0, 0.0),
        Vec3::Y,
        Vec3::new(0.0, 0.0, -1.0),
    ),
    Mat3::from_cols(Vec3::new(0.0, 0.0, 1.0), Vec3::Y, Vec3::new(-1.0, 0.0, 0.0)),
    Mat3::from_cols(Vec3::new(-1.0, 0.0, 0.0), Vec3::Y, Vec3::new(0.0, 0.0, 1.0)),
    Mat3::from_cols(Vec3::new(0.0, 0.0, 1.0), Vec3::Y, Vec3::new(1.0, 0.0, 0.0)),
    Mat3::from_cols(Vec3::new(1.0, 0.0, 0.0), Vec3::Y, Vec3::new(0.0, 0.0, -1.0)),
    Mat3::from_cols(
        Vec3::new(0.0, 0.0, -1.0),
        Vec3::Y,
        Vec3::new(-1.0, 0.0, 0.0),
    ),
];

/// Cell `(col, row)`'s matrix in the grid's local space — `T(centre) · R_y(k) · M` — and its 3×3
/// orientation. The one place the grid's cell pose is computed: extract and the placement lint
/// both read it.
pub fn mesh_grid_cell(grid: &MeshGrid, col: usize, row: usize) -> (Mat4, Mat3) {
    let basis = CELL_ORIENTATIONS[usize::from(grid.orient_at(col, row))];
    let centre = Vec3::new(
        col as f32 * grid.cell_size,
        0.0,
        row as f32 * grid.cell_size,
    );
    let cell = Mat4::from_cols(
        basis.x_axis.extend(0.0),
        basis.y_axis.extend(0.0),
        basis.z_axis.extend(0.0),
        centre.extend(1.0),
    );
    (cell, basis)
}

/// Skips every sprite and mesh whose placement is not finite, reporting each as `L2109`. NaN
/// geometry draws nothing and says nothing; dropping it here keeps the sort and the GI voxelizer
/// downstream NaN-free. A tilemap or mesh grid reports once per entity, not per cell, and the latch
/// key names the entity (`{id}/sprite`, `{id}/mesh`), not its walk position: an entity id is stable
/// across despawns, a walk index is reused.
fn drop_non_finite(placed: &mut Placed<'_>, degraded: &mut Vec<(String, Diagnostic)>) {
    let mut last_sprite = None;
    placed.sprites.retain(|s| {
        let finite = s.center.is_finite() && s.right.is_finite() && s.up.is_finite();
        if !finite && last_sprite != Some(s.id) {
            last_sprite = Some(s.id);
            degraded.push((
                format!("{}/sprite", s.id),
                Diagnostic::new(
                    code::RENDER_PLACEMENT_NON_FINITE,
                    format!(
                        "{}'s sprite placement is not finite (centre {:?}, right {:?}, up {:?}); \
                         it is not drawn",
                        s.id, s.center, s.right, s.up
                    ),
                )
                .with_path(s.site.base_pointer())
                .with_help(
                    "guard the maths that wrote this entity's Transform, or one up its Parent chain",
                ),
            ));
        }
        finite
    });
    let mut last_reported = None;
    placed.meshes.retain(|m| {
        let finite = m.model.is_finite();
        if !finite && last_reported != Some(m.id) {
            last_reported = Some(m.id);
            degraded.push((
                format!("{}/mesh", m.id),
                Diagnostic::new(
                    code::RENDER_PLACEMENT_NON_FINITE,
                    format!(
                        "{}'s mesh model matrix is not finite ({:?}); it is not drawn",
                        m.id, m.model
                    ),
                )
                .with_path(m.site.component_pointer(m.index))
                .with_help(
                    "guard the maths that wrote this entity's Transform, or one up its Parent chain",
                ),
            ));
        }
        finite
    });
}

/// Where a [`PlacedMesh`]'s shape lives in the scene document, for an error path: a `Mesh`
/// component, or one `MeshGrid` legend entry (a grid places one mesh per cell).
#[derive(Clone, Copy)]
pub(crate) enum MeshSite {
    /// The entity's own `Mesh` component.
    Component,
    /// The `MeshGrid` legend entry for this glyph.
    GridCell(char),
}

impl MeshSite {
    /// `/entities/<index>/components/Mesh` or `/entities/<index>/components/MeshGrid`.
    pub(crate) fn component_pointer(self, index: usize) -> String {
        match self {
            MeshSite::Component => format!("/entities/{index}/components/Mesh"),
            MeshSite::GridCell(_) => format!("/entities/{index}/components/MeshGrid"),
        }
    }

    /// `/entities/<index>/components/Mesh/shape/<tail>` or
    /// `/entities/<index>/components/MeshGrid/legend/<glyph>/shape/<tail>`.
    pub(crate) fn shape_pointer(self, index: usize, tail: &str) -> String {
        let component = self.component_pointer(index);
        match self {
            MeshSite::Component => format!("{component}/shape/{tail}"),
            MeshSite::GridCell(glyph) => {
                let mut buf = [0u8; 4];
                format!(
                    "{component}/legend/{}/shape/{tail}",
                    pointer_token(glyph.encode_utf8(&mut buf))
                )
            }
        }
    }
}

/// Where a texture reference lives in the scene document, for an `L1101`/`L1102` error path. Carried
/// as (entity index, component field) and formatted into a JSON Pointer only when a load fails —
/// the success path never reads it, and a tilemap references one texture per cell.
#[derive(Clone, Copy)]
pub(crate) enum TextureSite {
    /// A `Sprite`'s own `texture` field.
    Sprite(usize),
    /// A tilemap cell, addressed through the legend entry for its glyph.
    TilemapCell(usize, char),
    /// A screen-space `UiRect`'s `texture` field.
    UiRect(usize),
    /// The bullet kind a `Volley` names, whose `atlas`/`frame` pair lives in a `kind: pattern`
    /// document rather than on the component — so the pointer names the volley that reached it.
    Volley(usize),
    /// A text component's `font`: the atlas image of the face the name resolves to.
    Font(usize, &'static str),
}

impl TextureSite {
    /// The JSON Pointer of the component (or legend entry) this site lives in — what `/texture`,
    /// `/atlas` and `/frame` all hang off, so a texture error and an atlas error at one site can
    /// never point at different places.
    fn base_pointer(self) -> String {
        match self {
            TextureSite::Sprite(index) => format!("/entities/{index}/components/Sprite"),
            TextureSite::TilemapCell(index, glyph) => {
                let mut buf = [0u8; 4];
                format!(
                    "/entities/{index}/components/Tilemap/legend/{}",
                    pointer_token(glyph.encode_utf8(&mut buf))
                )
            }
            TextureSite::UiRect(index) => format!("/entities/{index}/components/UiRect"),
            TextureSite::Volley(index) => format!("/entities/{index}/components/Volley"),
            TextureSite::Font(index, component) => {
                format!("/entities/{index}/components/{component}")
            }
        }
    }

    /// The JSON Pointer this site's `texture` field serializes to.
    fn pointer(self) -> String {
        let field = match self {
            TextureSite::Font(..) => "font",
            _ => "texture",
        };
        format!("{}/{field}", self.base_pointer())
    }
}

/// One placed sprite/billboard: its entity, its full-size quad vectors, and its unresolved texture.
pub(crate) struct PlacedSprite<'w> {
    /// The entity this sprite belongs to.
    pub(crate) id: EntityId,
    /// Where this quad's texture reference lives, for an `L1101`/`L1102` texture-error path.
    pub(crate) site: TextureSite,
    /// World-space center of the quad.
    pub(crate) center: Vec3,
    /// FULL width vector (see the sizing comment below): `|right|` is the drawn width in world units.
    pub(crate) right: Vec3,
    /// FULL height vector: `|up|` is the drawn height in world units.
    pub(crate) up: Vec3,
    /// RGBA tint (linear).
    pub(crate) color: [f32; 4],
    /// Unresolved texture path (relative to the content root — the directory of the scene the engine was started with), or `None` for an untextured quad.
    pub(crate) texture: Option<&'w str>,
    /// Unresolved `kind: atlas` document path (rooted like [`texture`](PlacedSprite::texture)), or
    /// `None`. Mutually exclusive with `texture` (`L1151`). The *geometry* it implies is already
    /// applied to `right`/`up`; what stays unresolved here is its packed image and UV rect.
    pub(crate) atlas: Option<&'w str>,
    /// The region name inside [`atlas`](PlacedSprite::atlas). Carried separately rather than paired
    /// with it so a half-written reference on a world assembled in code reports `L1151` instead of
    /// being silently dropped.
    ///
    /// Borrowed from the component in the ordinary case; **owned** when a flipbook chose it, because
    /// that name lives in the atlas document [`place`] only borrows for the length of its own call.
    pub(crate) frame: Option<Cow<'w, str>>,
    /// The sub-rectangle of the texture to sample, `[u0, v0, u1, v1]` with the PNG's top-left
    /// origin — [`SpriteInstance::FULL_UV`] for a loose texture, the whole image.
    pub(crate) uv: [f32; 4],
    /// Whether the quad takes light (the lit branch of `sprite.wgsl`).
    pub(crate) lit: bool,
    /// Mirror the UV rect left↔right (applied at [`extract`], where the region UV is known).
    pub(crate) flip_x: bool,
    /// Mirror the UV rect top↔bottom.
    pub(crate) flip_y: bool,
    /// The sort key — the primary sprite draw-order key.
    pub(crate) layer: i16,
    /// Additive compositing (the `FLAG_ADDITIVE` branch of `sprite.wgsl`).
    pub(crate) additive: bool,
    /// Hit-flash `(amount, color)`. `amount == 0.0` is render-identical.
    pub(crate) flash: f32,
    /// The colour the hit-flash lerps toward.
    pub(crate) flash_color: [f32; 3],
    /// How many times the sampled sub-rectangle tiles across the quad (`[1, 1]` for one copy).
    pub(crate) uv_repeat: [f32; 2],
    /// The scroll **rate** in frame widths per second, still unevaluated: [`extract`] turns it into
    /// an offset against the frame's `SimTime`, so placement stays time-of-day free and
    /// `render.bounds` never has to care.
    pub(crate) uv_scroll: [f32; 2],
}

/// [`place`], over a fresh [`AssetCache`] of its own — what a caller with no host cache wants
/// (`render.bounds`). Not prefetched: it only resolves atlas region sizes, so a prefetch would
/// decode meshes it never reads.
pub(crate) fn place_uncached<'w>(
    world: &'w World,
    ids: &[EntityId],
    camera: &RenderCamera,
    project_root: &Path,
    now: SimTime,
) -> Placed<'w> {
    let fonts = font_library(world, project_root);
    let mut cache = AssetCache::new();
    let mut assets = CachedAssets {
        root: project_root,
        cache: &mut cache,
    };
    let mut atlases = AtlasCache::default();
    let mut poses = WorldAffines::new();
    place(
        world,
        ids,
        camera,
        &fonts,
        &mut assets,
        &mut atlases,
        &mut poses,
        now,
    )
}

/// A loader that resolves nothing: [`asset_refs`] reads which assets a world names, never their
/// bytes, and [`place`] falls back to the untextured sizing on every `Err`.
struct RefLoader;

impl RefLoader {
    fn unresolved() -> Vec<Diagnostic> {
        vec![Diagnostic::new(
            code::ASSET_FILE_IO,
            "asset resolution is not available while collecting references",
        )]
    }
}

impl SceneAssets for RefLoader {
    fn texture(&mut self, _: &str) -> Result<Arc<TextureImage>, Diagnostic> {
        Err(Self::unresolved().remove(0))
    }
    fn atlas(&mut self, _: &str) -> Result<Arc<AtlasAsset>, Vec<Diagnostic>> {
        Err(Self::unresolved())
    }
    fn mesh(&mut self, _: &str) -> Result<Arc<MeshData>, Vec<Diagnostic>> {
        Err(Self::unresolved())
    }
    fn voxel(&mut self, _: &str) -> Result<Arc<MeshData>, Vec<Diagnostic>> {
        Err(Self::unresolved())
    }
    fn voxel_grid(&mut self, _: &str) -> Result<Arc<VoxelStaticGrid>, Vec<Diagnostic>> {
        Err(Self::unresolved())
    }
    fn sculpt(&mut self, _: &str) -> Result<Arc<MeshData>, Vec<Diagnostic>> {
        Err(Self::unresolved())
    }
    fn assembly(&mut self, _: &str) -> Result<Arc<AssemblyData>, Vec<Diagnostic>> {
        Err(Self::unresolved())
    }
    fn character(&mut self, _: &str) -> Result<Arc<CharacterData>, Vec<Diagnostic>> {
        Err(Self::unresolved())
    }
    fn voxelization(&self, _: u64) -> Option<Option<Arc<WorldVoxelGrid>>> {
        None
    }
    fn held_voxelization(&mut self) -> Option<Option<Arc<WorldVoxelGrid>>> {
        None
    }
    fn set_voxelization(
        &mut self,
        _: u64,
        grid: Option<Arc<WorldVoxelGrid>>,
    ) -> Option<Arc<WorldVoxelGrid>> {
        grid
    }
}

/// Every asset the world names, as normalized cache keys — the walk [`AssetCache::prefetch`] decodes
/// ahead of the first extract. It places the whole world exactly as [`extract`] would (every entity,
/// presentation ones included, a grid's or a tilemap's cells expanded), so a reference can never be
/// extracted without having been collected. The camera is a placeholder: no source depends on it, and
/// a world with several cameras must not fail the walk.
pub(crate) fn asset_refs(world: &World, project_root: &Path) -> AssetRefs {
    let ids: Vec<EntityId> = world
        .entities()
        .chain(world.presentation_entities())
        .collect();
    let camera = RenderCamera::default_framing((1280, 720));
    let fonts = font_library(world, project_root);
    let mut atlases = AtlasCache::default();
    let mut poses = WorldAffines::new();
    let placed = place(
        world,
        &ids,
        &camera,
        &fonts,
        &mut RefLoader,
        &mut atlases,
        &mut poses,
        SimTime::from_ticks(0, TickRate::DEFAULT),
    );
    let mut refs = AssetRefs::default();
    for sprite in &placed.sprites {
        match (sprite.atlas, sprite.texture) {
            (Some(atlas), _) => refs.atlases.insert(ludion_core::paths::normalize(atlas)),
            (None, Some(texture)) => refs.textures.insert(ludion_core::paths::normalize(texture)),
            (None, None) => false,
        };
    }
    for mesh in &placed.meshes {
        let (set, path) = match mesh.source {
            PlacedMeshSource::Primitive(_) => continue,
            PlacedMeshSource::Asset(path) => (&mut refs.meshes, path),
            PlacedMeshSource::Voxel(path) => (&mut refs.voxels, path),
            PlacedMeshSource::Sculpt(path) => (&mut refs.sculpts, path),
            PlacedMeshSource::Character(path) => (&mut refs.characters, path),
            PlacedMeshSource::Assembly(path) | PlacedMeshSource::AssemblyPart(path, _) => {
                (&mut refs.assemblies, path)
            }
        };
        set.insert(ludion_core::paths::normalize(path));
    }
    refs
}

/// Walks `ids` (ascending entity-slot order — the order [`extract`] and the world hash use) once and
/// computes each renderable entity's world-space placement. This is the single home of the sizing
/// math; see [`Placed`] for why.
///
/// `assets` and `atlases` serve two inputs placement cannot read out of the world: the pixel size of
/// an atlas region a `Sprite` names without authoring a `size`, and — for a `Sprite` whose
/// [`Animation`] names one of that atlas's flipbooks — which region it is showing at `now`. `atlases`
/// is handed in rather than made here so [`extract`] resolves each atlas once for both the size and
/// the UV rect.
#[allow(clippy::too_many_arguments)]
pub(crate) fn place<'w>(
    world: &'w World,
    ids: &[EntityId],
    camera: &RenderCamera,
    fonts: &FontLibrary,
    assets: &mut dyn SceneAssets,
    atlases: &mut AtlasCache,
    poses: &mut WorldAffines,
    now: SimTime,
) -> Placed<'w> {
    // The camera's world basis, for billboards. Through [`CameraBasis::from_view`], which the
    // particle path also calls: one derivation, so a sprite and a particle in the same frame can
    // never disagree about which way "up" is.
    let CameraBasis {
        right: cam_right,
        up: cam_up,
    } = CameraBasis::from_view(camera.view);

    let mut placed = Placed {
        sprites: Vec::new(),
        meshes: Vec::new(),
        texts: Vec::new(),
        lights: Vec::new(),
        spot_lights: Vec::new(),
    };

    for (index, &id) in ids.iter().enumerate() {
        let pose = render_pose(world, poses, id);
        let translation = Vec3::from(pose.translation());
        // Read once per entity (`Plan/gi-wave3-dynamic-injection.md` §3.2), whichever of Mesh/
        // PointLight this entity carries reads the same answer — an entity carrying neither is
        // simply never consulted, the marker's documented inert case.
        let dynamic = get::<GiDynamic>(world, id).is_some();

        if let Some(mesh) = get::<Mesh>(world, id) {
            let model = pose.model();
            let normal_matrix = pose.normal_matrix();
            placed.meshes.push(PlacedMesh {
                id,
                index,
                site: MeshSite::Component,
                model,
                normal_matrix,
                color: mesh.color,
                material: [mesh.roughness, mesh.metallic, mesh.emissive],
                flash: mesh.flash,
                flash_color: mesh.flash_color,
                source: placed_source(&mesh.shape),
                dynamic,
            });
        }

        if let Some(grid) = get::<MeshGrid>(world, id) {
            // One `PlacedMesh` per non-empty cell, row-major, in the entity's local XZ plane under
            // its whole pose (see `MeshGrid`'s placement convention). Every cell carries this one
            // entity `id`, so `render.bounds` unions the grid into one renderable and a `Fade`
            // applies to all of it.
            let (model, normal_matrix) = (pose.model(), pose.normal_matrix());
            let mut glyph_buf = [0u8; 4];
            for (row_index, row) in grid.rows.iter().enumerate() {
                for (col_index, glyph) in row.chars().enumerate() {
                    // A space is the empty cell; a glyph with no legend entry draws nothing
                    // (validation rejects it as `L1020`; extract stays panic-free on an unvalidated
                    // world).
                    let Some(mesh) = grid.legend.get(glyph.encode_utf8(&mut glyph_buf)) else {
                        continue;
                    };
                    let (cell, basis) = mesh_grid_cell(grid, col_index, row_index);
                    placed.meshes.push(PlacedMesh {
                        id,
                        index,
                        site: MeshSite::GridCell(glyph),
                        model: model * cell,
                        normal_matrix: normal_matrix * basis,
                        color: mesh.color,
                        material: [mesh.roughness, mesh.metallic, mesh.emissive],
                        flash: mesh.flash,
                        flash_color: mesh.flash_color,
                        source: placed_source(&mesh.shape),
                        dynamic,
                    });
                }
            }
        }

        if let Some(sprite) = get::<Sprite>(world, id) {
            // `right`/`up` are FULL width/height vectors (the shader's unit-quad corners span
            // [-0.5, 0.5], so the drawn quad is exactly `|right|` × `|up|` world units — a
            // `size: [2, 0.4]` sprite draws 2.0 × 0.4). A sprite is an XY quad in local space, so
            // `Transform.scale` x/y multiply in (z has no quad axis to act on) — for billboards too,
            // where only the *orientation* comes from the camera.
            //
            // A flipbook this entity's `Animation` selects on the atlas replaces the authored frame
            // name, for the derived size here and the UV rect at `extract` alike — one name, so a
            // quad's pixels and its extent can never come from different regions.
            let frame = match flipbook_frame_name(world, id, sprite, assets, atlases, now) {
                Some(name) => Some(Cow::Owned(name)),
                None => sprite.frame.as_deref().map(Cow::Borrowed),
            };
            // No authored `size` derives one from the atlas region's own pixels over the atlas's
            // `pixels_per_unit`; an untextured or loose-texture quad, and a reference that does not
            // resolve, are `[1, 1]`.
            let size = sprite
                .size
                .or_else(|| {
                    let atlas = sprite.atlas.as_deref()?;
                    atlases.region_size(assets, atlas, frame.as_deref()?)
                })
                .unwrap_or([1.0, 1.0]);
            let scale = pose.scale();
            let w = size[0] * scale[0];
            let h = size[1] * scale[1];
            let (right, up) = if sprite.billboard {
                (cam_right * w, cam_up * h)
            } else if let Some(model) = pose.world() {
                (
                    model.x_axis.truncate() * size[0],
                    model.y_axis.truncate() * size[1],
                )
            } else {
                (pose.quat() * Vec3::X * w, pose.quat() * Vec3::Y * h)
            };
            // Rotate about the region's pivot, not the quad centre: the pivot fraction (0..1 from the
            // region's top-left) offsets the centre by the rotated basis so the pivot point itself
            // stays at the entity translation. `fx` runs along `right`, `fy` down from the top (the
            // shader's V flip). A flip mirrors the pivot with the art, so it stays at the
            // translation. The default pivot (region centre, `[0.5, 0.5]`) is a zero offset, so this
            // is identical for every sprite that does not author one — and a no-op for an untextured
            // or loose-texture quad, which has no region to read a pivot from.
            let mut center = translation;
            if let (Some(atlas_rel), Some(frame_name)) = (sprite.atlas.as_deref(), frame.as_deref())
                && let Some([fx, fy]) = atlases.region_pivot_frac(assets, atlas_rel, frame_name)
            {
                let fx = if sprite.flip_x { 1.0 - fx } else { fx };
                let fy = if sprite.flip_y { 1.0 - fy } else { fy };
                center -= (fx - 0.5) * right + (0.5 - fy) * up;
            }
            // `atlas`/`frame` name one region of an atlas document; the packed image and the UV rect
            // it addresses stay unresolved here, exactly as a `texture:` path does — only the
            // region's *size* was needed above, and the two fields are mutually exclusive (`L1151`).
            placed.sprites.push(PlacedSprite {
                id,
                site: TextureSite::Sprite(index),
                center,
                right,
                up,
                color: sprite.color,
                texture: sprite.texture.as_deref(),
                atlas: sprite.atlas.as_deref(),
                frame,
                uv: SpriteInstance::FULL_UV,
                lit: sprite.lit,
                flip_x: sprite.flip_x,
                flip_y: sprite.flip_y,
                layer: sprite.layer,
                additive: matches!(sprite.blend, SpriteBlend::Additive),
                flash: sprite.flash,
                flash_color: sprite.flash_color,
                uv_repeat: sprite.uv_repeat,
                uv_scroll: sprite.uv_scroll,
            });
        }

        if let Some(tilemap) = get::<Tilemap>(world, id) {
            // Expand cells row-major (rows outer, columns inner) into the existing sprite batcher —
            // no new GPU work. Each cell is a `cell_size × cell_size` axis-aligned quad; only the
            // entity's Transform.translation is used, as the center of cell (0, 0). +X is column,
            // −Y is row (see `Tilemap`'s placement convention). Deterministic order → deterministic
            // render; every cell is tagged with this one entity `id`, so `render.bounds` unions them
            // into the whole grid's AABB as a single renderable.
            let cell = tilemap.cell_size;
            let right = Vec3::X * cell;
            let up = Vec3::Y * cell;
            let mut glyph_buf = [0u8; 4];
            for (row_index, row) in tilemap.rows.iter().enumerate() {
                for (col_index, glyph) in row.chars().enumerate() {
                    // A glyph with no legend entry renders nothing (scene/`component.set` validation
                    // rejects it as `L1020`; extract stays panic-free on an unvalidated world).
                    let Some(tile) = tilemap.legend.get(glyph.encode_utf8(&mut glyph_buf)) else {
                        continue;
                    };
                    let center = Vec3::new(
                        translation.x + col_index as f32 * cell,
                        translation.y - row_index as f32 * cell,
                        translation.z,
                    );
                    placed.sprites.push(PlacedSprite {
                        id,
                        site: TextureSite::TilemapCell(index, glyph),
                        center,
                        right,
                        up,
                        color: tile.color,
                        texture: tile.texture.as_deref(),
                        atlas: tile.atlas.as_deref(),
                        frame: tile.frame.as_deref().map(Cow::Borrowed),
                        uv: SpriteInstance::FULL_UV,
                        lit: tile.lit,
                        flip_x: tile.flip_x,
                        flip_y: tile.flip_y,
                        // A tilemap shares one layer for the whole grid; its cells are axis-aligned and
                        // never rotate, so no pivot and no additive/flash per cell.
                        layer: tilemap.layer,
                        additive: false,
                        flash: 0.0,
                        flash_color: [1.0; 3],
                        uv_repeat: SpriteInstance::NO_TILING,
                        uv_scroll: [0.0; 2],
                    });
                }
            }
        }

        if let Some(light) = get::<PointLight>(world, id)
            && let Some(data) = point_light_data(light, translation, dynamic)
        {
            placed.lights.push((id, data));
        }

        if let Some(light) = get::<SpotLight>(world, id)
            && let Some(data) = spot_light_data(light, translation)
        {
            placed.spot_lights.push((id, data));
        }

        if let Some(text) = get::<Text>(world, id) {
            let laid =
                ludion_font::layout(fonts.face(text.font.as_deref()), &text.text, text.size, 0.0);
            let quads = laid.quads(
                [translation.x, translation.y],
                text.size,
                ludion_font::YAxis::Up,
            );
            placed.texts.push((
                id,
                TextRun {
                    position: translation,
                    size: text.size,
                    color: text.color,
                    text: text.text.clone(),
                    quads,
                    extent: laid.extent(),
                    font: None,
                },
                placed_font(fonts, text.font.as_deref()),
                index,
            ));
        }
    }

    placed
}

/// Borrows component `T` off a live entity, or `None`. Extraction reads every renderable's
/// components once per frame, so this stays a borrow — callers copy or clone only the fields they
/// actually hand to the renderer.
fn get<T: ludion_ecs::Component>(world: &World, id: EntityId) -> Option<&T> {
    world.get::<T>(id).ok().flatten()
}

/// The entity's [`Transform`], or the identity transform if it has none.
fn transform_of(world: &World, id: EntityId) -> Transform {
    get::<Transform>(world, id).copied().unwrap_or_default()
}

/// An entity's world pose as render draws it. A root (or an entity degraded to one) is today's
/// arithmetic verbatim — its stored [`Transform`]; a child draws its exact world affine, shear kept.
/// Each accessor decomposes only what its site needs.
struct RenderPose {
    local: Transform,
    affine: Option<Affine3>,
}

impl RenderPose {
    fn translation(&self) -> [f32; 3] {
        self.affine.map_or(self.local.translation, |a| a.cols[3])
    }

    fn scale(&self) -> [f32; 3] {
        self.affine.map_or(self.local.scale, |a| a.lossy_scale())
    }

    fn quat(&self) -> Quat {
        match self.affine {
            Some(a) => Quat::from_array(a.to_trs_lossy().rotation),
            None => rotation_quat(self.local.rotation),
        }
    }

    /// The world affine as a matrix; `None` for a root.
    fn world(&self) -> Option<Mat4> {
        let [c0, c1, c2, t] = self.affine?.cols.map(Vec3::from);
        Some(Mat4::from_cols(
            c0.extend(0.0),
            c1.extend(0.0),
            c2.extend(0.0),
            t.extend(1.0),
        ))
    }

    fn model(&self) -> Mat4 {
        self.world()
            .unwrap_or_else(|| transform_matrix(&self.local))
    }

    /// [`normal_matrix`] of [`Self::model`]. A parented pose can carry shear, which column lengths
    /// do not show, so unless its columns are orthogonal it takes the inverse-transpose.
    fn normal_matrix(&self) -> Mat3 {
        let model = self.model();
        if self.affine.is_some() && !columns_orthogonal(Mat3::from_mat4(model)) {
            return inverse_transpose(Mat3::from_mat4(model));
        }
        normal_matrix(model, self.scale())
    }

    /// The pose as a [`Transform`], rotation in Euler degrees.
    fn transform(&self) -> Transform {
        match self.affine {
            Some(a) => {
                let trs = a.to_trs_lossy();
                Transform {
                    translation: trs.translation,
                    rotation: quat_to_euler_xyz_degrees(trs.rotation),
                    scale: trs.scale,
                }
            }
            None => self.local,
        }
    }
}

fn render_pose(world: &World, poses: &mut WorldAffines, id: EntityId) -> RenderPose {
    let affine = poses.parented(world, id);
    RenderPose {
        local: if affine.is_some() {
            Transform::default()
        } else {
            transform_of(world, id)
        },
        affine,
    }
}

/// `xform`'s world-space model matrix (`T · R · S`) — the exact arithmetic [`place`] builds a mesh's
/// `model` with, reused to build a `SkinBone` part's world transform so a moved part's pose and its
/// own rendered position can never disagree.
fn transform_matrix(xform: &Transform) -> Mat4 {
    Mat4::from_scale_rotation_translation(
        Vec3::from(xform.scale),
        rotation_quat(xform.rotation),
        Vec3::from(xform.translation),
    )
}

/// Appends one skinned instance's bone-palette entries to `out`, one per `joints` entry
/// (parent-first, so one forward pass suffices): a joint a `followed` pair names takes that part's
/// world pose (`skin_model⁻¹ × part_world`, then `× inverse_bind`); every other joint composes its
/// rest local transform onto its parent's already-posed model (a root composes onto the skin's own
/// frame, i.e. its `rest_local` verbatim). Only called when `followed` is non-empty — the caller's
/// fast path for a part-less instance pushes each joint's precomposed [`SkinJoint::rest`] instead, to
/// stay byte-identical to the render-only rest pose. `posed_model` is the caller's scratch, cleared
/// on entry, so a frame of instances allocates it once.
fn posed_palette(
    joints: &[SkinJoint],
    skin_model: Mat4,
    followed: &[(&str, Mat4)],
    posed_model: &mut Vec<Mat4>,
    out: &mut Vec<Mat4>,
) {
    let skin_model_inv = skin_model.inverse();
    posed_model.clear();
    for joint in joints {
        let model = match followed.iter().find(|(name, _)| *name == joint.name) {
            Some((_, part_world)) => skin_model_inv * *part_world,
            None => match joint.parent {
                Some(parent) => posed_model[parent as usize] * joint.rest_local,
                None => joint.rest_local,
            },
        };
        out.push(model * joint.inverse_bind);
        posed_model.push(model);
    }
}

/// Appends one skinned instance's bone-palette entries for `clip` sampled at clip time `t`, one per
/// `joints` entry (parent-first). A joint the clip has a track for takes `T · R · S` with each
/// channel the clip does not animate left at the joint's bind value — the formula the importer's
/// error bound measures; a joint with no track composes its exact `rest_local`. Scale composes down
/// the chain through the matrix product.
///
/// While `fade` is `Some`, each joint blends its outgoing and current samples in quaternion space
/// (`ludion_scene::slerp` for rotation, lerp for translation and scale), a channel one side lacks
/// reading as the joint's rest value; a joint neither side has a track for stays at `rest_local`.
///
/// A sampler bound to a different joint count than `joints` (stale after a reload that changed the
/// skin) reads as no clip, so the skin poses at rest rather than reading tracks by the wrong index.
///
/// `posed_model` is the caller's scratch, cleared on entry.
pub(crate) fn sampled_palette(
    joints: &[SkinJoint],
    pose: ClipPose<'_, ClipSampler>,
    fade: Option<&CrossfadePose<'_, ClipSampler>>,
    posed_model: &mut Vec<Mat4>,
    out: &mut Vec<Mat4>,
) {
    posed_model.clear();
    fn bound<'a>(side: ClipPose<'a, ClipSampler>, targets: usize) -> ClipPose<'a, ClipSampler> {
        side.filter(|(sampler, _)| sampler.targets() == targets)
    }
    let pose = bound(pose, joints.len());
    let fade = fade.map(|fade| CrossfadePose {
        from: bound(fade.from, joints.len()),
        weight: fade.weight,
    });
    let fade = fade.as_ref();
    for (index, joint) in joints.iter().enumerate() {
        let sample =
            |side: ClipPose<'_, ClipSampler>| side.and_then(|(sampler, t)| sampler.pose(index, t));
        let sampled = match fade {
            None => sample(pose),
            Some(fade) => blended_joint_pose(joint, sample(fade.from), sample(pose), fade.weight),
        };
        let local = match sampled {
            None => joint.rest_local,
            Some(pose) => Mat4::from_scale_rotation_translation(
                pose.scale
                    .map_or(joint.rest_scale, |s| Vec3::from(s.map(|v| v as f32))),
                pose.rotation.map_or(joint.rest_rotation, |q| {
                    Quat::from_xyzw(q[0] as f32, q[1] as f32, q[2] as f32, q[3] as f32)
                }),
                pose.translation
                    .map_or(joint.rest_translation, |v| Vec3::from(v.map(|c| c as f32))),
            ),
        };
        let model = match joint.parent {
            Some(parent) => posed_model[parent as usize] * local,
            None => local,
        };
        out.push(model * joint.inverse_bind);
        posed_model.push(model);
    }
}

/// One joint's crossfaded channels: `from` and `to` blended `weight` of the way to `to`, each
/// missing channel (or missing side) read as the joint's rest value. `None` when neither side has a
/// track for the joint, so it keeps its exact `rest_local`.
fn blended_joint_pose(
    joint: &SkinJoint,
    from: Option<ludion_scene::PartPose>,
    to: Option<ludion_scene::PartPose>,
    weight: f64,
) -> Option<ludion_scene::PartPose> {
    if from.is_none() && to.is_none() {
        return None;
    }
    let (from, to) = (from.unwrap_or_default(), to.unwrap_or_default());
    let widen = |v: Vec3| [v.x, v.y, v.z].map(f64::from);
    let lerp3 = |a: Option<[f64; 3]>, b: Option<[f64; 3]>, rest: Vec3| {
        let rest = widen(rest);
        let (a, b) = (a.unwrap_or(rest), b.unwrap_or(rest));
        Some([0, 1, 2].map(|i| a[i] + (b[i] - a[i]) * weight))
    };
    let rest_q = joint.rest_rotation;
    let rest_q = [rest_q.x, rest_q.y, rest_q.z, rest_q.w].map(f64::from);
    let rotation = ludion_scene::slerp(
        from.rotation.unwrap_or(rest_q),
        to.rotation.unwrap_or(rest_q),
        weight,
    );
    Some(ludion_scene::PartPose {
        rotation: Some(rotation),
        translation: lerp3(from.translation, to.translation, joint.rest_translation),
        scale: lerp3(from.scale, to.scale, joint.rest_scale),
    })
}

/// The normal matrix for a mesh: the transform that carries object-space normals to world space.
///
/// - **Uniform scale** (`sx == sy == sz`): the model's own upper-3×3. A similarity transform already
///   preserves normal direction, and reusing it verbatim keeps every uniform-scale golden
///   byte-identical — the renderer applies it with the same arithmetic the old upper-3×3 path used.
/// - **Non-uniform scale**: the inverse-transpose of the upper-3×3 — what actually keeps a normal
///   perpendicular to its surface when the axes scale unequally. Built from the cofactor columns
///   (cross products of the other two model-basis columns) divided by the determinant, with **true
///   per-component division** (not `det.recip()`): a normal that lies along an unscaled axis then maps
///   back at magnitude 1, exactly as the old path fed it, so an axis-aligned mesh under non-uniform
///   scale (every such committed scene — ground planes, a wall cube) stays byte-identical while a
///   slanted normal (a sphere, a rotated mesh) is corrected. A zero determinant (an axis scaled to
///   zero) falls back to the undivided cofactor: same direction, finite, no NaN fed to the BRDF.
pub(crate) fn normal_matrix(model: Mat4, scale: [f32; 3]) -> Mat3 {
    let m3 = Mat3::from_mat4(model);
    if scale[0] == scale[1] && scale[1] == scale[2] {
        return m3;
    }
    inverse_transpose(m3)
}

/// Whether `m3`'s columns are mutually perpendicular (no shear), within float rounding.
fn columns_orthogonal(m3: Mat3) -> bool {
    let (a, b, c) = (m3.x_axis, m3.y_axis, m3.z_axis);
    let perpendicular = |u: Vec3, v: Vec3| u.dot(v).abs() <= 1e-5 * u.length() * v.length();
    perpendicular(a, b) && perpendicular(b, c) && perpendicular(c, a)
}

/// The inverse-transpose of `m3`, from its cofactor columns with per-component division (see
/// [`normal_matrix`]); a zero determinant falls back to the undivided cofactor.
fn inverse_transpose(m3: Mat3) -> Mat3 {
    let (a, b, c) = (m3.x_axis, m3.y_axis, m3.z_axis);
    let (bc, ca, ab) = (b.cross(c), c.cross(a), a.cross(b));
    let det = a.dot(bc);
    if det != 0.0 {
        Mat3::from_cols(bc / det, ca / det, ab / det)
    } else {
        Mat3::from_cols(bc, ca, ab)
    }
}

/// A glam quaternion of a `Transform.rotation` (Euler XYZ degrees), via `ludion_core::math`.
pub(crate) fn rotation_quat(rotation_deg: [f32; 3]) -> Quat {
    Quat::from_array(euler_xyz_degrees_to_quat(rotation_deg))
}

/// Finds the scene's single camera entity, distinguishing the three cases both [`extract`] and
/// [`render.bounds`](crate::bounds) must handle:
/// - `Ok(None)` — zero cameras. `extract` falls back to [`RenderCamera::default_framing`];
///   `render.bounds` reports `L2102` (a projected rect needs a real camera).
/// - `Ok(Some(id))` — exactly one camera; use it.
/// - `Err(L2101)` — more than one, the JSON Pointer naming the second camera's entity.
pub(crate) fn single_camera(
    world: &World,
    ids: &[EntityId],
) -> Result<Option<EntityId>, Diagnostic> {
    let cameras: Vec<(usize, EntityId)> = ids
        .iter()
        .copied()
        .enumerate()
        .filter(|&(_, id)| world.has::<Camera>(id))
        .collect();

    match cameras.as_slice() {
        [] => Ok(None),
        [(_, id)] => Ok(Some(*id)),
        [_, (second_index, _), ..] => Err(Diagnostic::new(
            code::RENDER_MULTIPLE_CAMERAS,
            format!(
                "the scene has {} cameras, but rendering needs exactly one",
                cameras.len()
            ),
        )
        .with_path(format!("/entities/{second_index}/components/Camera"))
        .with_help("keep a single `Camera` component in the scene")),
    }
}

/// Finds the single camera and builds a [`RenderCamera`]; zero cameras → default framing.
///
/// `view` is the host's [`render.view`](crate::render_view) override when one is in force. It
/// replaces the viewpoint and nothing else: the scene's camera is still resolved first, so an
/// ambiguous world is still `L2101`, and its clip planes still shape the override's projection.
fn resolve_camera(
    world: &World,
    ids: &[EntityId],
    viewport: (u32, u32),
    view: Option<&ViewPose>,
) -> Result<RenderCamera, Diagnostic> {
    let scene = single_camera(world, ids)?;
    match (view, scene) {
        (Some(pose), scene) => Ok(crate::render_view::camera(world, scene, pose, viewport)),
        (None, None) => Ok(RenderCamera::default_framing(viewport)),
        (None, Some(id)) => Ok(build_camera(world, id, viewport)),
    }
}

/// The single camera's `(height, near, far)` when it is an orthographic one with `pixel_snap` on, or
/// `None`. The extract-only pixel-snap seam reads this and nothing else about the camera.
fn orthographic_pixel_snap(world: &World, ids: &[EntityId]) -> Option<(f32, f32, f32)> {
    let id = single_camera(world, ids).ok()??;
    let cam = get::<Camera>(world, id)?;
    match cam.projection {
        Projection::Orthographic {
            height,
            pixel_snap: true,
        } => {
            let (near, far) = cam.depth_range();
            Some((height, near, far))
        }
        _ => None,
    }
}

/// The `pixels_per_unit` that pixel-snapping's texel grid uses: the atlas named by the first
/// (lowest-slot) sprite that names one, resolved through the pass's own [`AtlasCache`], or `None`
/// when no sprite draws from an atlas (nothing to snap to). Deterministic: placement order is the
/// ascending entity-slot walk.
fn pixel_snap_ppu(
    sprites: &[PlacedSprite<'_>],
    assets: &mut dyn SceneAssets,
    atlases: &mut AtlasCache,
) -> Option<f32> {
    for sprite in sprites {
        if let Some(rel) = sprite.atlas
            && let Ok(asset) = atlases.resolve(assets, rel)
        {
            return Some(asset.doc.pixels_per_unit as f32);
        }
    }
    None
}

/// Snaps an orthographic view height so one source texel spans a whole number of screen pixels: the
/// authored height's texel-to-pixel ratio rounded to the nearest integer `≥ 1`, then the height that
/// ratio implies. A degenerate (non-positive) height is returned unchanged.
fn snap_view_height(height: f32, viewport_h: u32, ppu: f32) -> f32 {
    if height <= 0.0 || ppu <= 0.0 {
        return height;
    }
    let vh = viewport_h as f32;
    let px_per_texel = (vh / (height * ppu)).round().max(1.0);
    vh / (px_per_texel * ppu)
}

/// Builds the view + projection matrices for the camera entity `id`.
pub(crate) fn build_camera(world: &World, id: EntityId, viewport: (u32, u32)) -> RenderCamera {
    let cam = get::<Camera>(world, id).copied().unwrap_or_default();
    let (near, far) = cam.depth_range();
    let pose = render_pose(world, &mut WorldAffines::new(), id);
    let translation = Vec3::from(pose.translation());
    let model = Mat4::from_rotation_translation(pose.quat(), translation);
    let view = model.inverse();
    let aspect = aspect_ratio(viewport);
    let projection = match cam.projection {
        Projection::Perspective { fov_y_deg } => {
            Mat4::perspective_rh(fov_y_deg.to_radians(), aspect, near, far)
        }
        // `pixel_snap` is applied at the end of `extract_inner` (it needs an atlas's
        // `pixels_per_unit`), never here — this builds the unsnapped projection.
        Projection::Orthographic { height, .. } => {
            let half_h = height * 0.5;
            let half_w = half_h * aspect;
            Mat4::orthographic_rh(-half_w, half_w, -half_h, half_h, near, far)
        }
    };
    RenderCamera {
        view,
        projection,
        position: translation,
    }
}

/// The font table extract resolves faces through: the world's own [`FontLibrary`] when the engine
/// built one, else the table loaded from `project_root` — one loader, so a bare `extract()` over an
/// instantiated world sees what a hosted engine does.
pub(crate) fn font_library(world: &World, project_root: &Path) -> FontLibrary {
    world
        .resource::<FontLibrary>()
        .cloned()
        .unwrap_or_else(|| FontLibrary::load(project_root))
}

/// A table face's atlas, as placement sees it: the root-relative image path, the size its document
/// declares, and whether it is an SDF. `None` for the embedded face, which draws through the
/// renderer's dedicated atlas.
pub(crate) struct PlacedFont {
    image: String,
    doc: String,
    size: [u32; 2],
    sdf: bool,
}

fn placed_font(fonts: &FontLibrary, name: Option<&str>) -> Option<PlacedFont> {
    let face = fonts.table_face(name)?;
    let (doc, image) = fonts.paths(name)?;
    let atlas = &face.document().atlas;
    Some(PlacedFont {
        image: image.to_string(),
        doc: doc.to_string(),
        size: [atlas.width, atlas.height],
        sdf: face.document().mode == FontMode::Sdf,
    })
}

/// Resolves a face's atlas image to a [`FontDraw`] through the shared texture dedup. A missing or
/// undecodable atlas is an ordinary `L1101`/`L1102` — never a silent fallback to the embedded atlas,
/// whose glyphs the proportional layout was not measured with. An atlas whose size disagrees with
/// the face document is `L1255` and is not drawn: its UVs would sample the wrong texels.
fn resolve_font(
    project_root: &Path,
    font: Option<&PlacedFont>,
    site: TextureSite,
    assets: &mut dyn SceneAssets,
    textures: &mut Vec<Arc<TextureImage>>,
    path_index: &mut StableMap<String, usize>,
    errors: &mut Vec<Diagnostic>,
) -> Option<FontDraw> {
    let font = font?;
    let texture = resolve_texture(
        project_root,
        &font.image,
        site,
        assets,
        textures,
        path_index,
        errors,
    )?;
    let image = &textures[texture];
    if [image.width, image.height] != font.size {
        errors.push(
            Diagnostic::new(
                code::FONT_LOAD,
                format!(
                    "the atlas `{}` is {}x{} but `{}` declares {}x{}",
                    font.image, image.width, image.height, font.doc, font.size[0], font.size[1]
                ),
            )
            .with_path(site.pointer())
            .with_help("re-run `ludion generate-atlas` on its source"),
        );
        return None;
    }
    Some(FontDraw {
        texture,
        sdf: font.sdf,
    })
}

/// Loads (or reuses) the texture at `rel` and returns its index, or records a diagnostic and
/// returns `None`. `assets` is how a path becomes an `Arc`-shared [`TextureImage`] (through
/// the [`AssetCache`], which hands every extract the same allocation); `path_index` still
/// dedups within one extract so a texture shared by several sprites is one entry in the
/// `RenderScene`.
fn resolve_texture(
    project_root: &Path,
    rel: &str,
    site: TextureSite,
    assets: &mut dyn SceneAssets,
    textures: &mut Vec<Arc<TextureImage>>,
    path_index: &mut StableMap<String, usize>,
    errors: &mut Vec<Diagnostic>,
) -> Option<usize> {
    if let Some(&idx) = path_index.as_inner().get(rel) {
        return Some(idx);
    }
    match assets.texture(rel) {
        Ok(texture) => {
            let idx = textures.len();
            textures.push(texture);
            path_index.insert(rel.to_string(), idx);
            Some(idx)
        }
        Err(base) => {
            let mut diagnostic = base.with_path(site.pointer());
            if let Some(name) = suggest_asset(project_root, rel) {
                diagnostic = diagnostic.with_help(format!("nearest match is `{name}`"));
            }
            errors.push(diagnostic);
            None
        }
    }
}

/// Looks `frame` up in the atlas at `rel` and returns its `textures` index and UV rect — or records
/// a diagnostic and returns `None`.
///
/// Mirrors [`resolve_texture`] one step deeper: `path_index` dedups **by atlas path**, so every quad
/// naming one atlas shares a single `RenderScene::textures` entry (one batch, one upload), while
/// `atlases` — already populated by [`place`], which asked the same atlases for their region sizes —
/// keeps the document read once per pass. A failure is *reported* per occurrence, exactly as a failed
/// texture load is; only the read behind it is shared.
#[allow(clippy::too_many_arguments)]
fn resolve_atlas_frame(
    project_root: &Path,
    rel: &str,
    frame: Option<&str>,
    site: TextureSite,
    assets: &mut dyn SceneAssets,
    atlases: &mut AtlasCache,
    textures: &mut Vec<Arc<TextureImage>>,
    path_index: &mut StableMap<String, usize>,
    errors: &mut Vec<Diagnostic>,
) -> Option<(usize, [f32; 4])> {
    let pointer = site.base_pointer();
    let Some(frame) = frame else {
        errors.push(atlas_without_frame(&pointer));
        return None;
    };
    let asset = match atlases.resolve(assets, rel) {
        Ok(asset) => asset,
        Err(causes) => {
            errors.push(atlas_unresolved(project_root, rel, &pointer, causes));
            return None;
        }
    };
    let (uv, _) = match asset.frame(rel, frame, &pointer) {
        Ok(resolved) => resolved,
        Err(diagnostic) => {
            errors.push(diagnostic);
            return None;
        }
    };
    let index = match path_index.as_inner().get(rel) {
        Some(&index) => index,
        None => {
            let index = textures.len();
            textures.push(Arc::clone(&asset.texture));
            path_index.insert(rel.to_string(), index);
            index
        }
    };
    Some((index, uv))
}

/// Which mesh-like asset a `Mesh.shape` names — selects the loader and the diagnostic label, so
/// asset meshes and voxel models share one first-encounter dedup path into `mesh_assets`.
#[derive(Clone, Copy)]
enum MeshKind {
    /// A `*.mesh.yaml` asset mesh.
    Asset,
    /// A `*.voxel.yaml` voxel model.
    Voxel,
    /// A `*.sculpt.yaml` sculpt model.
    Sculpt,
    /// The skinned mesh a `*.character.yaml` document names.
    Character,
}

impl MeshKind {
    /// Loads the document at `rel` through `assets` with this kind's loader.
    fn load(
        self,
        assets: &mut dyn SceneAssets,
        rel: &str,
    ) -> Result<Arc<MeshData>, Vec<Diagnostic>> {
        match self {
            MeshKind::Asset | MeshKind::Character => assets.mesh(rel),
            MeshKind::Voxel => assets.voxel(rel),
            MeshKind::Sculpt => assets.sculpt(rel),
        }
    }

    /// The noun prefixing a load diagnostic's message (`mesh asset` / `voxel model` /
    /// `sculpt model`).
    fn label(self) -> &'static str {
        match self {
            MeshKind::Asset => "mesh asset",
            MeshKind::Voxel => "voxel model",
            MeshKind::Sculpt => "sculpt model",
            MeshKind::Character => "character mesh",
        }
    }

    /// The serialized `Mesh.shape` variant key — the last segment of a load error's JSON Pointer.
    fn shape_field(self) -> &'static str {
        match self {
            MeshKind::Asset => "asset",
            MeshKind::Voxel => "voxel",
            MeshKind::Sculpt => "sculpt",
            MeshKind::Character => "character",
        }
    }
}

/// Loads (or reuses) the mesh-like document at `rel` and returns its `mesh_assets` index, or records
/// the document diagnostic(s) — re-pointed at the referencing `Mesh` component — and returns `None`.
/// Mirrors [`resolve_texture`]: `path_index` dedups so a document shared by several `Mesh` components
/// is one entry (one GPU upload), whether it is an asset mesh or a voxel model.
#[allow(clippy::too_many_arguments)]
fn resolve_mesh(
    rel: &str,
    index: usize,
    site: MeshSite,
    kind: MeshKind,
    assets: &mut dyn SceneAssets,
    mesh_assets: &mut Vec<Arc<MeshData>>,
    path_index: &mut StableMap<String, usize>,
    errors: &mut Vec<Diagnostic>,
) -> Option<usize> {
    if let Some(&idx) = path_index.as_inner().get(rel) {
        return Some(idx);
    }
    match kind.load(assets, rel) {
        Ok(mesh) => {
            let idx = mesh_assets.len();
            mesh_assets.push(mesh);
            path_index.insert(rel.to_string(), idx);
            Some(idx)
        }
        Err(diagnostics) => {
            let pointer = site.shape_pointer(index, kind.shape_field());
            for diagnostic in diagnostics {
                errors.push(mesh_error(rel, kind.label(), &pointer, diagnostic));
            }
            None
        }
    }
}

/// Loads (or reuses) the assembly at `rel` and returns it with the `mesh_assets` index of its **first**
/// part, or records the load diagnostic(s) — re-pointed at the referencing `Mesh` component — and
/// returns `None`.
///
/// The parts of one assembly are pushed as a contiguous run, so part `i` is always `base + i`: one
/// index per assembly path is the whole dedup state, and a second entity referencing the same
/// assembly re-uses the same run (one GPU upload per part, however many entities draw the model).
///
/// `pointer_field` is the trailing `Mesh/shape/...` segment a load failure is pointed at — `assembly`
/// for a whole-assembly `Mesh`, `assembly_part/assembly` for one naming a single part.
#[allow(clippy::too_many_arguments)]
fn resolve_assembly(
    rel: &str,
    index: usize,
    site: MeshSite,
    pointer_field: &str,
    assets: &mut dyn SceneAssets,
    mesh_assets: &mut Vec<Arc<MeshData>>,
    part_base: &mut StableMap<String, usize>,
    errors: &mut Vec<Diagnostic>,
) -> Option<(Arc<AssemblyData>, usize)> {
    let data = match assets.assembly(rel) {
        Ok(data) => data,
        Err(diagnostics) => {
            let pointer = site.shape_pointer(index, pointer_field);
            for diagnostic in diagnostics {
                errors.push(mesh_error(rel, "assembly", &pointer, diagnostic));
            }
            return None;
        }
    };
    let base = match part_base.as_inner().get(rel) {
        Some(&base) => base,
        None => {
            let base = mesh_assets.len();
            for part in &data.parts {
                mesh_assets.push(part.mesh.clone());
            }
            part_base.insert(rel.to_string(), base);
            base
        }
    };
    Some((data, base))
}

/// Resolves one named part of an assembly to its `mesh_assets` index, through [`resolve_assembly`] —
/// so a whole-assembly `Mesh` and an `assembly_part` one naming the same path share the one GPU
/// upload run.
///
/// # Errors
/// Whatever [`resolve_assembly`] reports for a load failure (pointed at `.../assembly_part/assembly`),
/// or [`code::MESH_PART_UNKNOWN`] at `.../assembly_part/part` when `part` names no part of the
/// resolved assembly.
#[allow(clippy::too_many_arguments)]
fn resolve_assembly_part(
    rel: &str,
    part: &str,
    index: usize,
    site: MeshSite,
    assets: &mut dyn SceneAssets,
    mesh_assets: &mut Vec<Arc<MeshData>>,
    part_base: &mut StableMap<String, usize>,
    errors: &mut Vec<Diagnostic>,
) -> Option<usize> {
    let (data, base) = resolve_assembly(
        rel,
        index,
        site,
        "assembly_part/assembly",
        assets,
        mesh_assets,
        part_base,
        errors,
    )?;
    match data.parts.iter().position(|p| p.name == part) {
        Some(part_index) => Some(base + part_index),
        None => {
            errors.push(
                Diagnostic::new(
                    code::MESH_PART_UNKNOWN,
                    format!("assembly `{rel}` has no part named `{part}`"),
                )
                .with_path(site.shape_pointer(index, "assembly_part/part")),
            );
            None
        }
    }
}

/// The clip an entity's [`Animation`] selects on `data`, with the clip time to sample it at — or
/// `None`, meaning the rest pose.
///
/// Every way of not animating collapses into that one `None`: no `Animation` component, a clip name
/// this assembly does not bind, or an empty name. An unknown name is deliberately silent (no
/// diagnostic, no log): the name is chosen at runtime by game logic no document-time check can see,
/// and a per-frame complaint about it would fire sixty times a second.
///
/// Crate-visible so [`crate::ragdoll::switch_to_ragdoll`] resolves the same pose extraction draws,
/// at the caller's gameplay time rather than a frame's render time.
pub(crate) fn animation_pose<'a>(
    world: &World,
    id: EntityId,
    data: &'a AssemblyData,
    now: SimTime,
) -> (ClipPose<'a>, Option<CrossfadePose<'a>>) {
    let Some(animation) = get::<Animation>(world, id) else {
        return (None, None);
    };
    let side = |name: &str, elapsed: f64| {
        let clip = data.clip(name)?;
        Some((clip, ludion_scene::clip_phase(clip, elapsed)))
    };
    let pose = side(&animation.clip, clip_elapsed(animation, now));
    let fade = fade_weight(animation, now).map(|weight| CrossfadePose {
        from: side(&animation.from, from_clip_elapsed(animation, now)),
        weight,
    });
    (pose, fade)
}

/// A clip and the clip time to sample it at; `None` is the rest pose.
/// `C` is the clip form: the document itself for an assembly, a [`ClipSampler`] for a character.
pub(crate) type ClipPose<'a, C = ludion_scene::AnimationDocument> = Option<(&'a C, f64)>;

/// The outgoing side of a crossfade in progress, and the incoming side's share `weight` in `[0, 1)`.
pub(crate) struct CrossfadePose<'a, C = ludion_scene::AnimationDocument> {
    pub from: ClipPose<'a, C>,
    pub weight: f64,
}

/// The incoming side's share of an active crossfade at `now`, or `None` when the animation is not
/// fading: no `fade`, or `fade_started + fade` has passed, so the plain incoming pose is drawn.
/// Derived from tick counts like [`clock_ticks`]; a `fade_started` in the future reads as `0`.
fn fade_weight(animation: &Animation, now: SimTime) -> Option<f64> {
    let fade = f64::from(animation.fade);
    if !(fade > 0.0 && fade.is_finite()) {
        return None;
    }
    let ticks = now.ticks().saturating_sub(animation.fade_started.as_u64());
    let weight = ticks as f64 * now.rate().seconds_per_tick() / fade;
    (weight < 1.0).then_some(weight)
}

/// The clip an entity's [`Animation`] selects on a character, with the clip time to sample it at —
/// or `None`, meaning the rest pose. [`animation_pose`]'s twin, silent on every miss for the same
/// reason; the character's per-clip overlay speed multiplies `Animation.speed`.
pub(crate) fn character_pose<'a>(
    world: &World,
    id: EntityId,
    data: &'a CharacterData,
    now: SimTime,
) -> (
    ClipPose<'a, ClipSampler>,
    Option<CrossfadePose<'a, ClipSampler>>,
) {
    let Some(animation) = get::<Animation>(world, id) else {
        return (None, None);
    };
    let side = |name: &str, elapsed: f64| {
        let binding = data.clip(name)?;
        Some((
            &binding.sampler,
            ludion_scene::clip_phase(&binding.clip, elapsed * binding.speed),
        ))
    };
    let pose = side(&animation.clip, clip_elapsed(animation, now));
    let fade = fade_weight(animation, now).map(|weight| CrossfadePose {
        from: side(&animation.from, from_clip_elapsed(animation, now)),
        weight,
    });
    (pose, fade)
}

/// The atlas region the entity's [`Animation`] is showing at `now`, or `None` — meaning the sprite
/// draws the frame it authored.
///
/// The flipbook twin of [`animation_pose`], and silent for the same reason. A flipbook hard-cuts to
/// `clip`: `from` and `fade` are ignored (`L1209` flags an authored sprite crossfade). Every way of not
/// animating collapses into that one `None` — no `Animation` component, an empty clip name, a name
/// this atlas binds no flipbook to, a `texture:` sprite, an atlas that will not load, or a frame the
/// atlas has no region for. A clip name is chosen at runtime by game logic no document-time check can
/// see, so an unknown one is a degrade to the authored frame rather than a diagnostic fired sixty
/// times a second. The frames of a baked flipbook are validated against the atlas's own regions, so
/// the last of those cases is unreachable through the pipeline and never becomes an `L1152` for a
/// name no author wrote.
fn flipbook_frame_name(
    world: &World,
    id: EntityId,
    sprite: &Sprite,
    assets: &mut dyn SceneAssets,
    atlases: &mut AtlasCache,
    now: SimTime,
) -> Option<String> {
    let atlas = sprite.atlas.as_deref()?;
    sprite.frame.as_deref()?;
    let animation = get::<Animation>(world, id)?;
    if animation.clip.is_empty() {
        return None;
    }
    let asset = atlases.resolve(assets, atlas).ok()?;
    let flipbook = asset.doc.flipbook(&animation.clip)?;
    let frame = ludion_scene::flipbook_frame(
        flipbook,
        clip_ticks(animation, now),
        f64::from(now.rate().hz()),
    );
    let name = flipbook.frames.get(frame)?;
    asset.doc.region(name).is_some().then(|| name.clone())
}

/// Clip ticks consumed by frame `now` for a clip started at `started` and played at `speed`: each
/// played tick counts its speed. When `anchor` belongs to this playback
/// (`anchor.started == started`) the count continues from the anchor's ticks at the anchor's frame;
/// a stale anchor, left by a restamp, is ignored.
///
/// Derived from the exact tick counts on every call and never accumulated — the
/// `SimTime::seconds` discipline, which is what keeps a pose a pure function of
/// `(clip, sim time)` and therefore identical across a replay. A `started` frame in the future
/// saturates to zero rather than wrapping into an enormous positive time. A flipbook keeps the
/// ticks so its frame can divide by the rate last; seconds are [`clock_elapsed`].
pub(crate) fn clock_ticks(
    now: SimTime,
    started: FrameIndex,
    speed: f32,
    anchor: Option<&ClipAnchor>,
) -> f64 {
    let (base, origin) = match anchor {
        Some(anchor) if anchor.started == started => (anchor.ticks, anchor.frame),
        _ => (0.0, started),
    };
    let played = now.ticks().saturating_sub(origin.as_u64());
    base + played as f64 * f64::from(speed)
}

/// The clip clock in seconds: [`clock_ticks`] at the sim's seconds-per-tick.
pub(crate) fn clock_elapsed(
    now: SimTime,
    started: FrameIndex,
    speed: f32,
    anchor: Option<&ClipAnchor>,
) -> f64 {
    clock_ticks(now, started, speed, anchor) * now.rate().seconds_per_tick()
}

/// The incoming clip's clock in ticks (before any character binding speed).
pub(crate) fn clip_ticks(animation: &Animation, now: SimTime) -> f64 {
    clock_ticks(
        now,
        animation.started,
        animation.speed,
        animation.anchor.as_ref(),
    )
}

/// The incoming clip's elapsed seconds (before any character binding speed).
pub(crate) fn clip_elapsed(animation: &Animation, now: SimTime) -> f64 {
    clock_elapsed(
        now,
        animation.started,
        animation.speed,
        animation.anchor.as_ref(),
    )
}

/// The outgoing clip's elapsed seconds.
pub(crate) fn from_clip_elapsed(animation: &Animation, now: SimTime) -> f64 {
    clock_elapsed(
        now,
        animation.from_started,
        animation.from_speed,
        animation.from_anchor.as_ref(),
    )
}

/// Where every part of `assembly` sits inside the assembly's own frame: per part, in authored order,
/// the transform placing its mesh and the rotation that same transform applies to its normals.
///
/// This is the one place a part's placement is decided, and it is decided for the whole assembly at
/// once because the part hierarchy ([`AssemblyPartData::parent`](crate::AssemblyPartData::parent)) is
/// only walkable with every part in hand.
///
/// **At rest** (`pose` is `None`) the hierarchy costs nothing:
/// [`AssemblyPartData::offset`](crate::AssemblyPartData::offset) already carries each part mesh's own
/// origin into the assembly frame, so every placement is a plain translation — which leaves normals
/// untouched, hence the identity rotation. That branch is byte-for-byte what this function did
/// before clips existed, and it is the branch every un-animated assembly in every scene takes.
///
/// **Posed**, each part's transform is built parent-relative and composed down the chain:
///
/// ```text
/// local_i = T(offset_i − offset_parent + translation_i) · T(pivot_i) · R_i · S_i · T(−pivot_i)
/// world_i = world_parent · local_i
/// ```
///
/// `S_i` is the clip's scale channel, and is the identity (the expression is then the plain
/// rotate-about-pivot one, bit for bit) for a part whose track has none.
///
/// A part **translates in its rest frame, then rotates about its pivot** — the ordinary `T · R`
/// order, so a translation key is never bent by that same part's rotation (a torso authored to bob
/// `+Y` bobs along world +Y however far the torso has turned). The translation therefore folds into
/// the same `T` as the offset, which is exactly what the code does.
///
/// `offset` is absolute in the assembly frame, so subtracting the parent's is what stops the chain
/// from translating twice; a part with no track has `R = I` and `translation = 0`, and both factors
/// are then *exactly* the identity, so the rest pose survives the composition bit-for-bit. Rotation
/// is the shared quaternion sampler's shortest-path slerp between keys (Euler-degree keys, as on
/// disk, converted through `ludion_core::math::f64::euler_xyz_degrees_to_quat`), applied about
/// [`AssemblyPartData::pivot`](crate::AssemblyPartData::pivot) — which
/// [`crate::assembly_mesh`] already expressed in the **mirrored** part-mesh frame, so nothing here
/// re-mirrors anything. Translation is in part-local voxel units.
///
/// The normal matrix is the composed rotation while these transforms are rigid, so the upper-left 3×3
/// *is* the correct normal basis; once any part scales, it is the inverse-transpose instead.
///
/// Crate-visible rather than private because the model-view harness's posed gap analysis
/// (`crate::model_sheet`, `Plan/voxel-animation.md` decision 11) rasterizes the *same* matrices the
/// renderer draws with — a second pose implementation is exactly the thing that would let a strip
/// disagree with what a game shows.
pub(crate) fn part_placements(
    assembly: &AssemblyData,
    pose: Option<(&ludion_scene::AnimationDocument, f64)>,
) -> Vec<(Mat4, Mat3)> {
    part_placements_with(assembly, pose, None, true)
}

/// [`part_placements`] for the render site: the same pose, blended with `fade`'s outgoing clip when a
/// crossfade is in progress.
pub(crate) fn part_placements_faded(
    assembly: &AssemblyData,
    pose: ClipPose<'_>,
    fade: Option<&CrossfadePose<'_>>,
) -> Vec<(Mat4, Mat3)> {
    part_placements_with(assembly, pose, fade, true)
}

/// [`part_placements`], with `scale` saying whether a clip's scale channel applies: the render path
/// plays it, the ragdoll switch (`crate::ragdoll`, rigid bodies can't scale) does not. Rotation is
/// the shared quaternion sampler's (`ludion_scene::sample_rotation`, libm-only), so render, the
/// model sheet and the ragdoll switch pose a part identically.
///
/// While `fade` is `Some`, each part's two sampled poses blend in quaternion space (rotation through
/// `ludion_scene::slerp`, translation and scale lerp, a channel one side lacks reading as that
/// part's rest value).
pub(crate) fn part_placements_with(
    assembly: &AssemblyData,
    pose: ClipPose<'_>,
    fade: Option<&CrossfadePose<'_>>,
    scale: bool,
) -> Vec<(Mat4, Mat3)> {
    if pose.is_none() && fade.is_none() {
        return assembly
            .parts
            .iter()
            .map(|part| {
                (
                    Mat4::from_translation(Vec3::from(part.offset)),
                    Mat3::IDENTITY,
                )
            })
            .collect();
    }

    let mut scaled = false;
    let locals: Vec<Mat4> = assembly
        .parts
        .iter()
        .map(|part| {
            let parent_offset = part
                .parent
                .map_or(Vec3::ZERO, |index| Vec3::from(assembly.parts[index].offset));
            let (rotation, sampled_scale, sampled_translation) = match fade {
                None => {
                    let (clip, time) = pose.expect("a rest pose without a fade returned early");
                    let sampled = ludion_scene::part_pose(clip, &part.name, time);
                    let rotation = sampled.rotation.map_or(Quat::IDENTITY, |q| {
                        Quat::from_xyzw(q[0] as f32, q[1] as f32, q[2] as f32, q[3] as f32)
                    });
                    (rotation, sampled.scale, sampled.translation)
                }
                Some(fade) => blended_part_pose(pose, fade, &part.name),
            };
            let pivot = Vec3::from(part.pivot);
            // Rotation about the pivot as one rigid transform, so an identity rotation yields the
            // exact identity matrix (`pivot − pivot` is exactly zero) instead of a
            // translate-rotate-untranslate round trip that only *nearly* cancels.
            let about_pivot = match sampled_scale.filter(|_| scale) {
                // `T(pivot) · R · S · T(−pivot)`: the part scales about its pivot, then turns.
                Some(s) => {
                    scaled = true;
                    let s = Vec3::from(s.map(|axis| axis as f32));
                    Mat4::from_translation(pivot)
                        * Mat4::from_quat(rotation)
                        * Mat4::from_scale(s)
                        * Mat4::from_translation(-pivot)
                }
                None => Mat4::from_rotation_translation(rotation, pivot - rotation * pivot),
            };
            let translation =
                sampled_translation.map_or(Vec3::ZERO, |v| Vec3::from(v.map(|axis| axis as f32)));
            // Rest-frame translation: folded into the offset's own `T`, ahead of the rotation, so
            // the key moves the part along the assembly's axes rather than the part's turned ones.
            Mat4::from_translation(Vec3::from(part.offset) - parent_offset + translation)
                * about_pivot
        })
        .collect();

    compose_chain(assembly, &locals, scaled)
}

/// One part's rotation, scale and translation while a crossfade is in progress: the current and
/// outgoing samples blended `fade.weight` of the way to the current one. A channel one side lacks
/// contributes the part's rest value (identity rotation, zero translation, unit scale); a channel
/// neither side animates stays `None`, so an untouched part keeps its exact rest matrices.
fn blended_part_pose(
    pose: ClipPose<'_>,
    fade: &CrossfadePose<'_>,
    part: &str,
) -> (Quat, Option<[f64; 3]>, Option<[f64; 3]>) {
    let sample = |side: ClipPose<'_>| {
        side.map_or_else(ludion_scene::PartPose::default, |(clip, time)| {
            ludion_scene::part_pose(clip, part, time)
        })
    };
    let (from, to) = (sample(fade.from), sample(pose));
    let w = fade.weight;
    let lerp3 = |a: Option<[f64; 3]>, b: Option<[f64; 3]>, rest: f64| match (a, b) {
        (None, None) => None,
        (a, b) => {
            let (a, b) = (a.unwrap_or([rest; 3]), b.unwrap_or([rest; 3]));
            Some([0, 1, 2].map(|i| a[i] + (b[i] - a[i]) * w))
        }
    };
    let rotation = match (from.rotation, to.rotation) {
        (None, None) => Quat::IDENTITY,
        (a, b) => {
            let identity = [0.0, 0.0, 0.0, 1.0];
            let q = ludion_scene::slerp(a.unwrap_or(identity), b.unwrap_or(identity), w);
            Quat::from_xyzw(q[0] as f32, q[1] as f32, q[2] as f32, q[3] as f32)
        }
    };
    (
        rotation,
        lerp3(from.scale, to.scale, 1.0),
        lerp3(from.translation, to.translation, 0.0),
    )
}

/// Composes each part's parent-relative transform down its chain to the root, memoized.
///
/// A validated assembly is a tree, but its parts are in **authored** order — a child may be written
/// before its parent — so this resolves each part's ancestors on demand rather than assuming a
/// topological order. The depth counter is a cycle guard: a malformed assembly that slipped past
/// validation degrades to an unresolved (identity-parented) part instead of looping forever.
///
/// The normal matrix is the model's upper 3×3 while every transform is rigid; once any part sampled a
/// scale (`scaled`), every part's is the inverse-transpose of it, as a scaled ancestor bends its
/// descendants' normals too.
fn compose_chain(assembly: &AssemblyData, locals: &[Mat4], scaled: bool) -> Vec<(Mat4, Mat3)> {
    let mut world: Vec<Option<Mat4>> = vec![None; locals.len()];
    for index in 0..locals.len() {
        // Walk up to the nearest already-resolved ancestor, then resolve back down.
        let mut chain = Vec::new();
        let mut cursor = Some(index);
        while let Some(current) = cursor {
            if world[current].is_some() || chain.len() > locals.len() {
                break;
            }
            chain.push(current);
            cursor = assembly.parts[current].parent;
        }
        for &current in chain.iter().rev() {
            let base = assembly.parts[current]
                .parent
                .and_then(|parent| world[parent])
                .unwrap_or(Mat4::IDENTITY);
            world[current] = Some(base * locals[current]);
        }
    }
    world
        .into_iter()
        .map(|model| {
            let model = model.unwrap_or(Mat4::IDENTITY);
            let m3 = Mat3::from_mat4(model);
            (model, if scaled { inverse_transpose(m3) } else { m3 })
        })
        .collect()
}

/// Re-points a mesh/voxel-document diagnostic at the referencing `Mesh` component's shape field,
/// keeping its code, message (prefixed with `label` + the path), and help — so an author sees both
/// which document is wrong and which entity referenced it (mirrors the texture-error style). Shared
/// with the voxelizer, which loads the same voxel documents through a second accessor.
pub(crate) fn mesh_error(
    rel: &str,
    label: &str,
    mesh_pointer: &str,
    mut diagnostic: Diagnostic,
) -> Diagnostic {
    diagnostic.message = format!("{label} `{rel}`: {}", diagnostic.message);
    diagnostic.with_path(mesh_pointer.to_string())
}

/// Suggests the nearest existing filename in `rel`'s directory (relative to `project_root`), if one
/// is a plausible typo. Goes through [`ludion_core::suggest::nearest`] — the one nearest-match
/// helper every other diagnostic in the engine uses — so a transposed letter (`lantren` for
/// `lantern`) suggests here exactly as it does for a misspelled component or field name.
fn suggest_asset(project_root: &Path, rel: &str) -> Option<String> {
    let joined = project_root.join(rel);
    let dir = joined.parent()?;
    let target = joined.file_name()?.to_string_lossy().to_string();
    let entries = std::fs::read_dir(dir).ok()?;
    let names: Vec<String> = entries
        .filter_map(|e| e.ok())
        .filter_map(|e| e.file_name().into_string().ok())
        .collect();
    ludion_core::suggest::nearest(&target, names.iter().map(String::as_str)).map(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ludion_scene::{AnimationDocument, AnimationTrack, Ease, Keyframe};

    /// Frame 0 at the default tick rate — what every test that does not care about clip time passes
    /// for `now`, and exactly what the three-argument [`extract`] passes.
    fn tick_zero() -> SimTime {
        SimTime::from_ticks(0, TickRate::DEFAULT)
    }

    /// Two extracts through one cache hand back the cache's own texture bytes — a loose PNG and an
    /// atlas alike — rather than a fresh copy each frame.
    #[test]
    fn a_cached_extract_shares_texture_bytes_with_the_cache() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/gallery");
        let mut world = World::new();
        let loose = world.spawn();
        world
            .insert(
                loose,
                Sprite {
                    texture: Some("assets/checker.png".into()),
                    ..Sprite::default()
                },
            )
            .unwrap();
        let atlased = world.spawn();
        world
            .insert(
                atlased,
                Sprite {
                    atlas: Some("sprites/gallery.atlas.yaml".into()),
                    frame: Some("ship/idle".into()),
                    ..Sprite::default()
                },
            )
            .unwrap();

        let mut cache = crate::asset_cache::AssetCache::new();
        let mut extract_once = || {
            extract_through(
                &world,
                (320, 180),
                &root,
                &mut cache,
                tick_zero(),
                ludion_renderer::QualityParams::FULL.particle_density,
                None,
                &mut Vec::new(),
            )
            .expect("the fixture world extracts")
        };
        let first = extract_once();
        let second = extract_once();

        assert_eq!(first.textures.len(), 2);
        assert_eq!(second.textures.len(), 2);
        for (a, b) in first.textures.iter().zip(&second.textures) {
            assert_eq!(
                a.rgba.as_ptr(),
                b.rgba.as_ptr(),
                "texture bytes were copied"
            );
            assert!(Arc::ptr_eq(a, b));
        }
    }

    /// A NaN sprite and an infinite mesh scale are skipped and reported once each; the finite
    /// sprite still draws.
    #[test]
    fn a_non_finite_placement_is_skipped_with_l2109() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/gallery");
        let mut world = World::new();
        let nan_sprite = world.spawn();
        world.insert(nan_sprite, Sprite::default()).unwrap();
        world
            .insert(
                nan_sprite,
                Transform {
                    translation: [f32::NAN, 0.0, 0.0],
                    ..Transform::default()
                },
            )
            .unwrap();
        let good_sprite = world.spawn();
        world.insert(good_sprite, Sprite::default()).unwrap();
        let inf_mesh = world.spawn();
        world
            .insert(
                inf_mesh,
                Transform {
                    scale: [f32::INFINITY, 1.0, 1.0],
                    ..Transform::default()
                },
            )
            .unwrap();
        world
            .insert(
                inf_mesh,
                Mesh {
                    shape: MeshSource::Cube,
                    ..Mesh::default()
                },
            )
            .unwrap();

        let mut degraded = Vec::new();
        let scene = extract_inner(
            &world,
            (320, 180),
            &root,
            &mut CachedAssets {
                root: &root,
                cache: &mut AssetCache::new(),
            },
            tick_zero(),
            ludion_renderer::QualityParams::FULL.particle_density,
            None,
            &mut degraded,
        )
        .expect("the world extracts");

        assert_eq!(scene.sprites.len(), 1);
        assert_eq!(scene.meshes.len(), 0);
        let reported: Vec<_> = degraded
            .iter()
            .inspect(|(_, d)| assert_eq!(d.code, code::RENDER_PLACEMENT_NON_FINITE))
            .map(|(key, d)| (key.as_str(), d.path.as_deref()))
            .collect();
        let sprite_key = format!("{nan_sprite}/sprite");
        let mesh_key = format!("{inf_mesh}/mesh");
        assert_eq!(
            reported,
            [
                (sprite_key.as_str(), Some("/entities/0/components/Sprite")),
                (mesh_key.as_str(), Some("/entities/2/components/Mesh"))
            ]
        );
    }

    /// `L2109` latches per entity, not per scene-walk position: once A is reported, B going
    /// non-finite at the walk index A vacated is a new offender, and A is not reported twice.
    #[test]
    fn l2109_is_latched_per_entity_not_per_walk_index() {
        use crate::Engine;
        use ludion_protocol::Command;
        use ludion_protocol::command::SimStepParams;
        use ludion_protocol::event::Event;

        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/gallery");
        let mut engine = Engine::builder()
            .project_root(&root)
            .build()
            .expect("the engine builds");
        engine
            .load_inline_scene("format: 1\nentities: []\n")
            .expect("the scene loads");

        let spawn_nan_sprite = |engine: &mut Engine| {
            let world = engine.world_mut();
            let id = world.spawn();
            world.insert(id, Sprite::default()).unwrap();
            world
                .insert(
                    id,
                    Transform {
                        translation: [f32::NAN, 0.0, 0.0],
                        ..Transform::default()
                    },
                )
                .unwrap();
            id
        };
        let reported_after_extract = |engine: &mut Engine| -> Vec<String> {
            engine
                .extract_render_scene_through(320, 180, false)
                .expect("a non-finite placement never fails the frame");
            engine.take_events();
            engine
                .execute(Command::SimStep(SimStepParams {
                    ticks: 1,
                    hash: false,
                }))
                .unwrap();
            engine
                .take_events()
                .into_iter()
                .filter_map(|event| match event {
                    Event::Log(log) => log.diagnostic,
                    _ => None,
                })
                .filter(|d| d.code.as_str() == "L2109")
                .map(|d| d.message)
                .collect()
        };

        let a = spawn_nan_sprite(&mut engine);
        let logged = reported_after_extract(&mut engine);
        assert_eq!(logged.len(), 1, "{logged:?}");
        assert!(logged[0].contains(&a.to_string()), "{logged:?}");
        assert!(!logged[0].contains("  "), "{logged:?}");

        engine.world_mut().despawn(a).unwrap();
        let b = spawn_nan_sprite(&mut engine);
        assert_ne!(a, b);
        let logged = reported_after_extract(&mut engine);
        assert_eq!(logged.len(), 1, "B reuses A's walk index: {logged:?}");
        assert!(logged[0].contains(&b.to_string()), "{logged:?}");
        assert!(!logged[0].contains("  "), "{logged:?}");

        let logged = reported_after_extract(&mut engine);
        assert!(logged.is_empty(), "each entity latches once: {logged:?}");
    }

    /// Under a non-uniform scale the normal matrix must send a slanted normal to the direction the
    /// inverse-transpose gives — perpendicular to the *scaled* surface — not the wrong direction the
    /// old model-upper-3×3 path produced. For `scale = [3, 1, 1]` the inverse-transpose is
    /// `diag(1/3, 1, 1)`, so a 45° normal `(1, 1, 0)` must tilt toward the shrunk X axis.
    #[test]
    fn normal_matrix_corrects_non_uniform_scale() {
        let scale = [3.0, 1.0, 1.0];
        let model = Mat4::from_scale(Vec3::from(scale));
        let n = Vec3::new(1.0, 1.0, 0.0);

        let got = (normal_matrix(model, scale) * n).normalize();
        // The inverse-transpose direction, computed the obvious (division) way for the reference.
        let want = (Vec3::new(1.0 / 3.0, 1.0, 0.0)).normalize();
        assert!((got - want).length() < 1e-6, "got {got:?}, want {want:?}");

        // And it must genuinely differ from the buggy upper-3×3 path (which would over-tilt to +X).
        let buggy = (Mat3::from_mat4(model) * n).normalize();
        assert!(
            (got - buggy).length() > 0.1,
            "normal matrix must differ from the naive upper-3×3"
        );
    }

    /// A zero-scaled axis must not produce NaNs/infinities — the cofactor construction divides by
    /// nothing, so a degenerate transform yields a finite (lower-rank) matrix, not garbage.
    #[test]
    fn normal_matrix_is_finite_under_a_zero_scaled_axis() {
        let scale = [3.0, 1.0, 0.0];
        let m = normal_matrix(Mat4::from_scale(Vec3::from(scale)), scale);
        assert!(
            m.to_cols_array().iter().all(|v| v.is_finite()),
            "degenerate scale must stay finite: {m:?}",
        );
    }

    /// Each orientation glyph is exactly the turn (and mirror) `Transform` gives — the sign
    /// convention `MeshGrid` documents.
    #[test]
    fn grid_orientations_match_transform_rotation_and_mirror() {
        for (orient, basis) in CELL_ORIENTATIONS.iter().enumerate() {
            let turns = (orient % 4) as f32;
            let mirror = if orient >= 4 { -1.0 } else { 1.0 };
            let expected = Mat3::from_quat(rotation_quat([0.0, 90.0 * turns, 0.0]))
                * Mat3::from_diagonal(Vec3::new(mirror, 1.0, 1.0));
            assert!(
                basis.abs_diff_eq(expected, 1e-6),
                "orientation {orient}: {basis:?} vs {expected:?}"
            );
        }
    }

    /// A uniform scale returns the model's upper-3×3 *exactly* (bit-for-bit) — the invariant that
    /// keeps every existing uniform-scale golden byte-identical.
    #[test]
    fn normal_matrix_is_the_model_upper_3x3_under_uniform_scale() {
        let model = Mat4::from_scale_rotation_translation(
            Vec3::splat(2.5),
            rotation_quat([20.0, -35.0, 12.0]),
            Vec3::new(4.0, -1.0, 7.0),
        );
        assert_eq!(
            normal_matrix(model, [2.5, 2.5, 2.5]),
            Mat3::from_mat4(model)
        );
    }

    /// A parent scaled `[1, √7, 2]` with a child turned 45° about z has three equal-length model
    /// columns (all 2) that are not perpendicular — a shear the column lengths hide. The normal
    /// matrix must still be the inverse-transpose: a normal stays perpendicular to the surface.
    #[test]
    fn parented_normal_matrix_survives_a_shear_with_equal_column_lengths() {
        use ludion_core::math::Trs;
        let parent = Trs {
            scale: [1.0, 7f32.sqrt(), 2.0],
            ..Trs::IDENTITY
        };
        let child = Trs {
            rotation: rotation_quat([0.0, 0.0, 45.0]).to_array(),
            ..Trs::IDENTITY
        };
        let pose = RenderPose {
            local: Transform::default(),
            affine: Some(parent.to_affine().mul_affine(&child.to_affine())),
        };
        let model = Mat3::from_mat4(pose.model());
        assert!(
            (pose.scale()[0] - pose.scale()[1]).abs() < 1e-4,
            "{:?}",
            pose.scale()
        );
        let normal = pose.normal_matrix();
        for (n, t) in [
            (Vec3::X, Vec3::Y),
            (Vec3::X, Vec3::Z),
            (Vec3::Y, Vec3::X),
            (Vec3::new(1.0, 1.0, 0.0), Vec3::new(1.0, -1.0, 0.0)),
        ] {
            let across = (normal * n).dot(model * t);
            assert!(across.abs() < 1e-5, "n {n:?} t {t:?}: {across}");
        }
    }

    /// A two-joint rig — `root` then `child` at rest-local `T(0, 1, 0)` — with identity inverse binds,
    /// so a joint's rest-palette entry is its rest model matrix verbatim: the fixture [`posed_palette`]'s
    /// own unit tests share.
    fn two_joint_rig() -> Vec<SkinJoint> {
        let child_rest_local = Mat4::from_translation(Vec3::new(0.0, 1.0, 0.0));
        vec![
            SkinJoint {
                name: "root".to_string(),
                parent: None,
                rest_local: Mat4::IDENTITY,
                rest_translation: Vec3::ZERO,
                rest_rotation: Quat::IDENTITY,
                rest_scale: Vec3::ONE,
                inverse_bind: Mat4::IDENTITY,
                rest: Mat4::IDENTITY,
            },
            SkinJoint {
                name: "child".to_string(),
                parent: Some(0),
                rest_local: child_rest_local,
                rest_translation: Vec3::new(0.0, 1.0, 0.0),
                rest_rotation: Quat::IDENTITY,
                rest_scale: Vec3::ONE,
                inverse_bind: Mat4::IDENTITY,
                rest: child_rest_local,
            },
        ]
    }

    fn sampled_clip(
        target: &str,
        rotation: Vec<Keyframe>,
        scale: Vec<Keyframe>,
    ) -> AnimationDocument {
        AnimationDocument {
            duration: 1.0,
            tracks: vec![AnimationTrack {
                target: target.to_string(),
                rotation,
                translation: Vec::new(),
                scale,
            }],
            ..Default::default()
        }
    }

    fn linear(t: f64, v: [f64; 3]) -> Keyframe {
        Keyframe {
            t,
            v,
            ease: Ease::Linear,
        }
    }

    fn sampler_for(joints: &[SkinJoint], clip: &AnimationDocument) -> ClipSampler {
        ClipSampler::new(clip, joints.iter().map(|joint| joint.name.as_str()))
    }

    fn close(a: Mat4, b: Mat4) -> bool {
        a.to_cols_array()
            .iter()
            .zip(b.to_cols_array())
            .all(|(x, y)| (x - y).abs() < 1e-6)
    }

    /// A clip that names neither joint leaves the whole skin on its rest chain.
    #[test]
    fn sampled_palette_leaves_untracked_joints_at_rest() {
        let joints = two_joint_rig();
        let clip = sampled_clip("elsewhere", vec![linear(0.0, [0.0; 3])], Vec::new());
        let mut out = Vec::new();
        sampled_palette(
            &joints,
            Some((&sampler_for(&joints, &clip), 0.5)),
            None,
            &mut Vec::new(),
            &mut out,
        );
        assert!(close(out[0], joints[0].rest));
        assert!(close(out[1], joints[1].rest));
    }

    /// A sampler bound to another joint list (a stale one after a reload) poses rest instead of
    /// reading a track by the wrong index.
    #[test]
    fn sampled_palette_poses_rest_for_a_sampler_bound_to_other_joints() {
        let joints = two_joint_rig();
        let clip = sampled_clip(
            &joints[0].name,
            vec![linear(0.0, [0.0; 3]), linear(1.0, [0.0, 0.0, 90.0])],
            Vec::new(),
        );
        let other = ClipSampler::new(
            &clip,
            [joints[1].name.as_str(), joints[0].name.as_str(), "extra"],
        );
        let mut out = Vec::new();
        sampled_palette(
            &joints,
            Some((&other, 0.5)),
            None,
            &mut Vec::new(),
            &mut out,
        );
        assert!(close(out[0], joints[0].rest));
        assert!(close(out[1], joints[1].rest));
    }

    /// A parent scale key composes down the chain: the child's origin sits twice as far out.
    #[test]
    fn sampled_palette_composes_scale_down_the_chain() {
        let joints = two_joint_rig();
        let clip = sampled_clip(
            "root",
            Vec::new(),
            vec![linear(0.0, [2.0; 3]), linear(1.0, [2.0; 3])],
        );
        let mut out = Vec::new();
        sampled_palette(
            &joints,
            Some((&sampler_for(&joints, &clip), 0.5)),
            None,
            &mut Vec::new(),
            &mut out,
        );
        assert!((out[1].transform_point3(Vec3::ZERO) - Vec3::new(0.0, 2.0, 0.0)).length() < 1e-6);
    }

    /// Rotation interpolates as a quaternion slerp, not per-axis Euler lerp.
    #[test]
    fn sampled_palette_rotation_is_the_quaternion_sample() {
        let joints = two_joint_rig();
        let keys = vec![linear(0.0, [0.0; 3]), linear(1.0, [0.0, 170.0, 0.0])];
        let clip = sampled_clip("root", keys.clone(), Vec::new());
        let mut out = Vec::new();
        sampled_palette(
            &joints,
            Some((&sampler_for(&joints, &clip), 0.5)),
            None,
            &mut Vec::new(),
            &mut out,
        );
        let q = ludion_scene::sample_rotation(&keys, 0.5).unwrap();
        let want = Quat::from_xyzw(q[0] as f32, q[1] as f32, q[2] as f32, q[3] as f32);
        assert!(close(out[0], Mat4::from_quat(want)));
    }

    /// The palette as it was before the sampler: every joint looked up by name in the clip each call.
    fn reference_palette(
        joints: &[SkinJoint],
        pose: ClipPose<'_>,
        fade: Option<&CrossfadePose<'_>>,
    ) -> Vec<Mat4> {
        let mut out = Vec::new();
        let mut posed_model: Vec<Mat4> = Vec::new();
        for joint in joints {
            let sample = |side: ClipPose<'_>| {
                side.and_then(|(clip, t)| ludion_scene::joint_pose(clip, &joint.name, t))
            };
            let sampled = match fade {
                None => sample(pose),
                Some(fade) => {
                    blended_joint_pose(joint, sample(fade.from), sample(pose), fade.weight)
                }
            };
            let local = match sampled {
                None => joint.rest_local,
                Some(pose) => Mat4::from_scale_rotation_translation(
                    pose.scale
                        .map_or(joint.rest_scale, |s| Vec3::from(s.map(|v| v as f32))),
                    pose.rotation.map_or(joint.rest_rotation, |q| {
                        Quat::from_xyzw(q[0] as f32, q[1] as f32, q[2] as f32, q[3] as f32)
                    }),
                    pose.translation
                        .map_or(joint.rest_translation, |v| Vec3::from(v.map(|c| c as f32))),
                ),
            };
            let model = match joint.parent {
                Some(parent) => posed_model[parent as usize] * local,
                None => local,
            };
            out.push(model * joint.inverse_bind);
            posed_model.push(model);
        }
        out
    }

    fn three_joint_rig() -> Vec<SkinJoint> {
        let mut joints = two_joint_rig();
        let tip_local =
            Mat4::from_rotation_translation(Quat::from_rotation_z(0.3), Vec3::new(0.5, 0.75, 0.0));
        joints[1].inverse_bind = Mat4::from_translation(Vec3::new(0.0, -1.0, 0.0));
        joints.push(SkinJoint {
            name: "tip".to_string(),
            parent: Some(1),
            rest_local: tip_local,
            rest_translation: Vec3::new(0.5, 0.75, 0.0),
            rest_rotation: Quat::from_rotation_z(0.3),
            rest_scale: Vec3::ONE,
            inverse_bind: tip_local.inverse(),
            rest: tip_local,
        });
        joints
    }

    /// The sampler path draws the same bits as sampling every joint by name, with and without a
    /// crossfade, for tracks that skip a joint, animate only some channels, or exist on one side only.
    #[test]
    fn sampled_palette_is_bit_identical_to_per_joint_sampling() {
        let joints = three_joint_rig();
        let incoming = AnimationDocument {
            duration: 1.0,
            tracks: vec![
                AnimationTrack {
                    target: "tip".to_string(),
                    rotation: vec![
                        linear(0.0, [0.0, 0.0, 0.0]),
                        linear(1.0, [30.0, 80.0, -20.0]),
                    ],
                    translation: vec![linear(0.0, [0.0, 0.0, 0.0]), linear(1.0, [1.0, 2.0, 3.0])],
                    scale: Vec::new(),
                },
                AnimationTrack {
                    target: "root".to_string(),
                    rotation: Vec::new(),
                    translation: Vec::new(),
                    scale: vec![linear(0.0, [1.0, 1.5, 1.0]), linear(1.0, [2.0, 1.0, 0.5])],
                },
            ],
            ..Default::default()
        };
        let outgoing = AnimationDocument {
            duration: 1.0,
            tracks: vec![AnimationTrack {
                target: "child".to_string(),
                rotation: vec![linear(0.0, [10.0, 0.0, 0.0]), linear(1.0, [0.0, 0.0, 90.0])],
                translation: Vec::new(),
                scale: Vec::new(),
            }],
            ..Default::default()
        };
        let incoming_sampler = sampler_for(&joints, &incoming);
        let outgoing_sampler = sampler_for(&joints, &outgoing);
        let nothing = AnimationDocument {
            duration: 1.0,
            tracks: vec![AnimationTrack {
                target: "elsewhere".to_string(),
                rotation: vec![linear(0.0, [0.0; 3])],
                translation: Vec::new(),
                scale: Vec::new(),
            }],
            ..Default::default()
        };
        let nothing_sampler = sampler_for(&joints, &nothing);

        for t in [0.0, 0.3, 0.77, 1.0] {
            // No fade.
            let want = reference_palette(&joints, Some((&incoming, t)), None);
            let mut got = Vec::new();
            sampled_palette(
                &joints,
                Some((&incoming_sampler, t)),
                None,
                &mut Vec::new(),
                &mut got,
            );
            assert_palette_bits(&got, &want, "no fade");

            // A fade with both sides, one where the outgoing side is gone, one where no side tracks
            // anything, and one where only the outgoing side tracks a joint.
            for weight in [0.0, 0.25, 0.9] {
                let cases: [(ClipPose<'_>, ClipPose<'_, ClipSampler>, &str); 3] = [
                    (
                        Some((&outgoing, t)),
                        Some((&outgoing_sampler, t)),
                        "both sides",
                    ),
                    (None, None, "outgoing side gone"),
                    (
                        Some((&nothing, t)),
                        Some((&nothing_sampler, t)),
                        "untracked outgoing",
                    ),
                ];
                for (from, from_sampler, label) in cases {
                    let want = reference_palette(
                        &joints,
                        Some((&incoming, t)),
                        Some(&CrossfadePose { from, weight }),
                    );
                    let mut got = Vec::new();
                    sampled_palette(
                        &joints,
                        Some((&incoming_sampler, t)),
                        Some(&CrossfadePose {
                            from: from_sampler,
                            weight,
                        }),
                        &mut Vec::new(),
                        &mut got,
                    );
                    assert_palette_bits(&got, &want, label);
                }
            }
        }
    }

    fn assert_palette_bits(got: &[Mat4], want: &[Mat4], label: &str) {
        assert_eq!(got.len(), want.len(), "{label}");
        for (i, (g, w)) in got.iter().zip(want).enumerate() {
            let bits = |m: &Mat4| m.to_cols_array().map(f32::to_bits);
            assert_eq!(bits(g), bits(w), "{label}: joint {i}");
        }
    }

    /// A followed root moves its own entry to the part's world pose (through the identity skin
    /// model), and the unfollowed child composes its rest-local transform onto that moved root — it
    /// follows its parent's new pose, not its own rest entry.
    #[test]
    fn posed_palette_composes_an_unfollowed_child_onto_its_moved_parent() {
        let joints = two_joint_rig();
        let root_world = Mat4::from_translation(Vec3::new(0.0, 3.0, 0.0));
        let mut out = Vec::new();
        posed_palette(
            &joints,
            Mat4::IDENTITY,
            &[("root", root_world)],
            &mut Vec::new(),
            &mut out,
        );

        assert_eq!(
            out[0], root_world,
            "the followed root's entry is its own world pose"
        );
        assert_eq!(
            out[1],
            root_world * joints[1].rest_local,
            "the unfollowed child composes its rest-local transform onto the moved root"
        );
    }

    /// A part placed at exactly `skin_model × rest_model_j` (the spawn-pose invariant) reproduces the
    /// joint's precomposed rest entry, within float error — the un-moved-part case must agree with the
    /// caller's byte-exact fast path.
    #[test]
    fn posed_palette_reproduces_rest_when_the_part_sits_at_the_spawn_pose() {
        let joints = two_joint_rig();
        let skin_model = Mat4::from_scale_rotation_translation(
            Vec3::splat(2.0),
            Quat::IDENTITY,
            Vec3::new(5.0, 0.0, 0.0),
        );
        let part_world = skin_model * joints[1].rest_local; // root's rest model is identity
        let mut out = Vec::new();
        posed_palette(
            &joints,
            skin_model,
            &[("child", part_world)],
            &mut Vec::new(),
            &mut out,
        );

        for (got, want) in out[1]
            .to_cols_array()
            .iter()
            .zip(joints[1].rest.to_cols_array())
        {
            assert!(
                (got - want).abs() < 1e-5,
                "{:?} vs {:?}",
                out[1],
                joints[1].rest
            );
        }
    }

    /// A straight-down sun (which triggers the `up` fallback, since the light points along −Y)
    /// projects its frustum center to NDC depth `0.5` (the center sits at the midpoint of the depth
    /// range) and maps a world point at `+shadow_extent` on X to the `−1` clip edge. Pinned values,
    /// bit-exact CPU — the wgpu 0..1 depth convention `orthographic_rh` produces.
    #[test]
    fn sun_light_space_projects_center_and_extent_edge_to_pinned_ndc() {
        let sun = DirectionalLight {
            direction: [0.0, -1.0, 0.0],
            shadow_extent: 40.0,
            shadow_near: 1.0,
            shadow_far: 200.0,
            ..DirectionalLight::default()
        };
        let light_space = build_sun(&sun, Vec3::ZERO).light_space;

        let center = light_space.project_point3(Vec3::ZERO);
        assert!(
            (center - Vec3::new(0.0, 0.0, 0.5)).length() < 1e-5,
            "center should project to NDC (0, 0, 0.5): {center:?}"
        );
        let edge = light_space.project_point3(Vec3::new(40.0, 0.0, 0.0));
        assert!(
            (edge - Vec3::new(-1.0, 0.0, 0.5)).length() < 1e-5,
            "+extent on X should map to the −1 clip edge at mid-depth: {edge:?}"
        );
    }

    /// The Transform translation anchors the frustum **center** (settled decision 4, amended): the
    /// anchor point itself always projects to NDC `(0, 0, 0.5)`, wherever it sits in the world.
    #[test]
    fn sun_frustum_is_centered_on_the_transform_anchor() {
        let sun = DirectionalLight {
            direction: [0.0, -1.0, 0.0],
            ..DirectionalLight::default()
        };
        let anchor = Vec3::new(5.0, 2.0, -3.0);
        let ndc = build_sun(&sun, anchor).light_space.project_point3(anchor);
        assert!(
            (ndc - Vec3::new(0.0, 0.0, 0.5)).length() < 1e-5,
            "the anchor should project to the frustum center: {ndc:?}"
        );
    }

    /// A camera whose depth range is not a frustum — reachable only by a game building the
    /// component in Rust — extracts with the default range, so the projection stays finite.
    #[test]
    fn a_degenerate_camera_depth_range_extracts_finite() {
        let finite = |camera: &RenderCamera| {
            camera
                .projection
                .to_cols_array()
                .iter()
                .all(|v| v.is_finite())
        };
        for (label, near, far) in [("near == far", 5.0, 5.0), ("near 0", 0.0, 1000.0)] {
            for projection in [
                Projection::Perspective { fov_y_deg: 60.0 },
                Projection::Orthographic {
                    height: 10.0,
                    pixel_snap: false,
                },
            ] {
                let mut world = World::new();
                let id = world.spawn();
                world
                    .insert(
                        id,
                        Camera {
                            projection,
                            near,
                            far,
                        },
                    )
                    .unwrap();
                let camera = build_camera(&world, id, (320, 180));
                assert!(
                    finite(&camera),
                    "{label} {projection:?}: {:?}",
                    camera.projection
                );
            }
        }
    }

    /// For the (non-fallback) default angled direction and an off-origin anchor, `light_space` is
    /// exactly the mirrored `orthographic_rh * look_to_rh` composition — pins the eye placement
    /// (`center − dir · midpoint`), the `+Y` up choice, the extent, and the depth convention.
    #[test]
    fn sun_light_space_matches_the_mirrored_glam_build() {
        let sun = DirectionalLight::default();
        assert!(
            Vec3::from(sun.direction).normalize().y.abs() <= 0.999,
            "the default direction must not hit the up fallback"
        );
        let center = Vec3::new(3.0, 1.0, -2.0);
        let got = build_sun(&sun, center).light_space;

        let direction = Vec3::from(sun.direction).normalize();
        let back = 0.5 * (sun.shadow_near + sun.shadow_far);
        let view = Mat4::look_to_rh(center - direction * back, direction, Vec3::Y);
        let e = sun.shadow_extent;
        let projection = Mat4::orthographic_rh(-e, e, -e, e, sun.shadow_near, sun.shadow_far);
        assert_eq!(got, projection * view);
    }

    /// The direction is normalized and the color is premultiplied by intensity (mirroring the
    /// point-light convention). Powers of two keep the premultiplication bit-exact.
    #[test]
    fn sun_normalizes_direction_and_premultiplies_color() {
        let sun = DirectionalLight {
            direction: [0.0, -2.0, 0.0],
            color: [0.5, 0.25, 0.125],
            intensity: 2.0,
            ..DirectionalLight::default()
        };
        let data = build_sun(&sun, Vec3::ZERO);
        assert_eq!(data.direction, Vec3::new(0.0, -1.0, 0.0));
        assert_eq!(data.color, Vec3::new(1.0, 0.5, 0.25));
        assert_eq!(
            data.bias,
            sun.shadow_bias / (sun.shadow_far - sun.shadow_near)
        );
    }

    /// `shadow_bias` is authored in world units; the renderer takes light-NDC depth, so the same
    /// authored bias is a smaller NDC value over a deeper range.
    #[test]
    fn the_shadow_bias_is_authored_in_world_units() {
        let sun = DirectionalLight {
            shadow_near: 1.0,
            shadow_far: 201.0,
            shadow_bias: 0.5,
            ..DirectionalLight::default()
        };
        assert_eq!(build_sun(&sun, Vec3::ZERO).bias, 0.5 / 200.0);

        let deeper = DirectionalLight {
            shadow_far: 401.0,
            ..sun
        };
        assert_eq!(build_sun(&deeper, Vec3::ZERO).bias, 0.5 / 400.0);
    }

    /// A degenerate (zero-length) direction — reachable only by a game constructing the component in
    /// Rust and skipping validation — falls back to the default direction, keeping the matrix finite
    /// (no NaN), so extract stays panic-free on an unvalidated world.
    #[test]
    fn sun_degenerate_direction_falls_back_to_the_default() {
        let sun = DirectionalLight {
            direction: [0.0, 0.0, 0.0],
            ..DirectionalLight::default()
        };
        let data = build_sun(&sun, Vec3::ZERO);
        assert_eq!(
            data.direction,
            Vec3::from(DirectionalLight::default().direction).normalize()
        );
        assert!(
            data.light_space
                .to_cols_array()
                .iter()
                .all(|v| v.is_finite()),
            "a degenerate direction must not produce a non-finite matrix: {:?}",
            data.light_space
        );
    }

    /// A zero extent or an empty/inverted depth range — reachable only by a game constructing the
    /// component in Rust — extracts with the default frustum, so the matrix and the bias stay finite.
    #[test]
    fn a_degenerate_shadow_frustum_falls_back_to_the_default() {
        let want = build_sun(&DirectionalLight::default(), Vec3::ZERO);
        for (label, sun) in [
            (
                "zero extent",
                DirectionalLight {
                    shadow_extent: 0.0,
                    ..DirectionalLight::default()
                },
            ),
            (
                "near == far",
                DirectionalLight {
                    shadow_near: 5.0,
                    shadow_far: 5.0,
                    ..DirectionalLight::default()
                },
            ),
            (
                "near > far",
                DirectionalLight {
                    shadow_near: 300.0,
                    shadow_far: 200.0,
                    ..DirectionalLight::default()
                },
            ),
            (
                "nan extent",
                DirectionalLight {
                    shadow_extent: f32::NAN,
                    ..DirectionalLight::default()
                },
            ),
        ] {
            let got = build_sun(&sun, Vec3::ZERO);
            assert!(
                got.light_space
                    .to_cols_array()
                    .iter()
                    .all(|v| v.is_finite()),
                "{label}: non-finite light_space {:?}",
                got.light_space
            );
            assert_eq!(got.light_space, want.light_space, "{label}");
            assert!(
                got.bias.is_finite() && got.bias >= 0.0,
                "{label}: {}",
                got.bias
            );
            assert_eq!(got.bias, want.bias, "{label}");
        }

        // A symmetric range is a valid frustum (the eye sits at the anchor, `near + far == 0`), not
        // a degenerate one: it must keep its own matrix, finite, with the anchor at mid-depth.
        let symmetric = DirectionalLight {
            shadow_near: -100.0,
            shadow_far: 100.0,
            ..DirectionalLight::default()
        };
        let got = build_sun(&symmetric, Vec3::ZERO);
        assert!(
            got.light_space
                .to_cols_array()
                .iter()
                .all(|v| v.is_finite()),
            "symmetric range: non-finite light_space {:?}",
            got.light_space
        );
        assert_ne!(got.light_space, want.light_space, "symmetric range");
        let center = Vec3::new(3.0, 1.0, -2.0);
        let ndc = build_sun(&symmetric, center)
            .light_space
            .project_point3(center);
        assert!(
            (ndc - Vec3::new(0.0, 0.0, 0.5)).length() < 1e-5,
            "the anchor should project to NDC z 0.5: {ndc:?}"
        );
    }

    /// A negative or NaN colour or intensity — Rust-built values that skipped validation — extracts
    /// as black, never as negative or NaN light. A point light with no usable range is dropped.
    #[test]
    fn negative_or_nan_light_colour_extracts_as_black() {
        for (color, intensity) in [
            ([-1.0, 0.5, 0.5], 1.0),
            ([f32::NAN, 0.5, 0.5], 1.0),
            ([1.0, 1.0, 1.0], -2.0),
            ([1.0, 1.0, 1.0], f32::NAN),
        ] {
            let sun = build_sun(
                &DirectionalLight {
                    color,
                    intensity,
                    ..DirectionalLight::default()
                },
                Vec3::ZERO,
            );
            assert!(
                sun.color.to_array().iter().all(|c| *c >= 0.0),
                "sun {color:?} x {intensity}: {:?}",
                sun.color
            );

            let light = PointLight {
                color,
                intensity,
                range: 10.0,
            };
            let data = point_light_data(&light, Vec3::ZERO, false).expect("a usable range");
            let radiance = data.color * data.intensity;
            assert!(
                radiance.to_array().iter().all(|c| *c >= 0.0),
                "point {color:?} x {intensity}: {radiance:?}"
            );
        }
        for range in [0.0, -1.0, f32::NAN, f32::INFINITY] {
            let light = PointLight {
                range,
                ..PointLight::default()
            };
            assert!(
                point_light_data(&light, Vec3::ZERO, false).is_none(),
                "range {range}"
            );
        }
    }

    /// With multiple suns, extract takes the lowest live `EntityId` (first in the pinned
    /// ascending-slot walk) and ignores the rest — the 16-light-cap posture, no error. A sunless
    /// world yields `None`.
    #[test]
    fn extract_sun_takes_the_lowest_entity_and_none_when_absent() {
        use ludion_ecs::World;

        let mut world = World::new();
        let first = world.spawn();
        world
            .insert(
                first,
                DirectionalLight {
                    color: [0.1, 0.2, 0.3],
                    intensity: 1.0,
                    ..DirectionalLight::default()
                },
            )
            .unwrap();
        let second = world.spawn();
        world
            .insert(
                second,
                DirectionalLight {
                    color: [0.9, 0.8, 0.7],
                    intensity: 1.0,
                    ..DirectionalLight::default()
                },
            )
            .unwrap();

        let ids: Vec<EntityId> = world.entities().collect();
        let sun = extract_sun(&world, &ids).expect("a sun is present");
        assert_eq!(
            sun.color,
            Vec3::new(0.1, 0.2, 0.3),
            "the lowest-EntityId sun wins"
        );

        let empty = World::new();
        let empty_ids: Vec<EntityId> = empty.entities().collect();
        assert!(extract_sun(&empty, &empty_ids).is_none());
    }

    /// With multiple environments, extract takes the lowest live `EntityId` (first in the pinned
    /// ascending-slot walk) and ignores the rest — the `extract_sun` posture. A world with no
    /// `Environment` yields `None` (the historical-pipeline signal).
    #[test]
    fn extract_environment_takes_the_lowest_entity_and_none_when_absent() {
        use crate::components::Environment;
        use ludion_ecs::World;

        let mut world = World::new();
        let first = world.spawn();
        world
            .insert(
                first,
                Environment {
                    ibl_intensity: 0.5,
                    ..Environment::default()
                },
            )
            .unwrap();
        let second = world.spawn();
        world
            .insert(
                second,
                Environment {
                    ibl_intensity: 9.0,
                    ..Environment::default()
                },
            )
            .unwrap();

        let ids: Vec<EntityId> = world.entities().collect();
        let env = extract_environment(&world, &ids).expect("an environment is present");
        assert_eq!(
            env.ibl_intensity, 0.5,
            "the lowest-EntityId environment wins"
        );

        let empty = World::new();
        let empty_ids: Vec<EntityId> = empty.entities().collect();
        assert!(extract_environment(&empty, &empty_ids).is_none());
    }

    /// With multiple `PostProcess` components, extract takes the lowest live `EntityId` (first in
    /// the pinned ascending-slot walk) and ignores the rest — the `extract_environment` posture. A
    /// world with no `PostProcess` yields `None`, which is what makes the renderer fall back to its
    /// shipped post defaults (the absent-is-byte-identical inversion).
    #[test]
    fn extract_post_takes_the_lowest_entity_and_none_when_absent() {
        use crate::components::PostProcess;
        use ludion_ecs::World;

        let mut world = World::new();
        let first = world.spawn();
        world
            .insert(
                first,
                PostProcess {
                    bloom_strength: 0.25,
                    ..PostProcess::default()
                },
            )
            .unwrap();
        let second = world.spawn();
        world
            .insert(
                second,
                PostProcess {
                    bloom_strength: 9.0,
                    ..PostProcess::default()
                },
            )
            .unwrap();

        let ids: Vec<EntityId> = world.entities().collect();
        let post = extract_post(&world, &ids).expect("a post-process is present");
        assert_eq!(
            post.bloom_strength, 0.25,
            "the lowest-EntityId PostProcess wins"
        );
        assert_eq!(post.bloom_threshold, 1.0);

        let empty = World::new();
        let empty_ids: Vec<EntityId> = empty.entities().collect();
        assert!(extract_post(&empty, &empty_ids).is_none());
    }

    /// **The other half of the byte-identity claim.** A default `PostProcess` extracts to exactly
    /// [`PostData::default`] — the numbers the renderer falls back to when a scene declares no
    /// component at all.
    ///
    /// Without this the two defaults are two independent lists that happen to agree today: a knob
    /// re-authored on one side only would leave "an empty `PostProcess: {}` renders identically to no
    /// component" quietly false, and the only symptom would be a golden moving for no stated reason.
    #[test]
    fn a_default_post_process_extracts_to_the_renderers_own_defaults() {
        use crate::components::PostProcess;
        use ludion_ecs::World;

        let mut world = World::new();
        let entity = world.spawn();
        world.insert(entity, PostProcess::default()).unwrap();
        let ids: Vec<EntityId> = world.entities().collect();

        assert_eq!(
            extract_post(&world, &ids),
            Some(PostData::default()),
            "the component's defaults and the renderer's fallback must be one set of numbers",
        );
    }

    /// Every `PostProcess` field reaches `PostData`, including the two closed enums' variant
    /// mappings — a hand-written match per enum is exactly where a variant can go missing.
    #[test]
    fn extract_post_carries_every_field() {
        use crate::components::PostProcess;
        use ludion_ecs::World;

        let mut world = World::new();
        let entity = world.spawn();
        world
            .insert(
                entity,
                PostProcess {
                    bloom_threshold: 0.75,
                    bloom_strength: 0.2,
                    exposure: 1.75,
                    tonemap: TonemapOperator::Agx,
                    dither: true,
                    contrast: 1.2,
                    saturation: 0.8,
                    temperature: 0.35,
                    tint: -0.15,
                    lift: [0.01, 0.02, 0.03],
                    gamma: [1.1, 1.0, 0.9],
                    gain: [1.05, 1.0, 0.95],
                    fog_density: 0.02,
                    fog_color: [0.1, 0.12, 0.18],
                    fog_start: 6.0,
                    fog_height: 3.5,
                    fog_height_falloff: 0.25,
                    fog_albedo: 0.55,
                    fog_sky: 0.65,
                    fog_anisotropy: 0.4,
                    vignette: 0.4,
                    vignette_radius: 0.3,
                    ssao: 0.65,
                    ssao_radius: 1.25,
                    ssao_bias: 0.05,
                    ssao_direct: 0.35,
                    shafts: 0.45,
                    dof: 0.03,
                    focus_distance: 14.0,
                    chromatic_aberration: 0.004,
                    halation: 0.3,
                    grain: 0.02,
                    grain_size: 0.005,
                    ssr: 0.7,
                    ssr_fade: [0.3, 0.5],
                    ssr_max_distance: 12.0,
                    ssr_thickness: 0.25,
                },
            )
            .unwrap();
        let ids: Vec<EntityId> = world.entities().collect();
        let post = extract_post(&world, &ids).expect("a post-process is present");
        assert_eq!(
            post,
            PostData {
                bloom_threshold: 0.75,
                bloom_strength: 0.2,
                exposure: 1.75,
                tonemap: ludion_renderer::TonemapOperator::Agx,
                dither: true,
                contrast: 1.2,
                saturation: 0.8,
                temperature: 0.35,
                tint: -0.15,
                lift: [0.01, 0.02, 0.03],
                gamma: [1.1, 1.0, 0.9],
                gain: [1.05, 1.0, 0.95],
                fog_density: 0.02,
                fog_color: [0.1, 0.12, 0.18],
                fog_start: 6.0,
                fog_height: 3.5,
                fog_height_falloff: 0.25,
                fog_albedo: 0.55,
                fog_sky: 0.65,
                fog_anisotropy: 0.4,
                vignette: 0.4,
                vignette_radius: 0.3,
                ssao: 0.65,
                ssao_radius: 1.25,
                ssao_bias: 0.05,
                ssao_direct: 0.35,
                shafts: 0.45,
                dof: 0.03,
                focus_distance: 14.0,
                chromatic_aberration: 0.004,
                halation: 0.3,
                grain: 0.02,
                grain_size: 0.005,
                ssr: 0.7,
                ssr_fade: [0.3, 0.5],
                ssr_max_distance: 12.0,
                ssr_thickness: 0.25,
            }
        );

        // The authored defaults are the renderer's own, field for field.
        world.insert(entity, PostProcess::default()).unwrap();
        assert_eq!(
            extract_post(&world, &ids),
            Some(PostData::default()),
            "a default `PostProcess` is the renderer's default `PostData`"
        );

        for (authored, expected) in [
            (TonemapOperator::Agx, ludion_renderer::TonemapOperator::Agx),
            (
                TonemapOperator::Aces,
                ludion_renderer::TonemapOperator::Aces,
            ),
            (
                TonemapOperator::Neutral,
                ludion_renderer::TonemapOperator::Neutral,
            ),
            (
                TonemapOperator::None,
                ludion_renderer::TonemapOperator::None,
            ),
        ] {
            world
                .insert(
                    entity,
                    PostProcess {
                        tonemap: authored,
                        ..PostProcess::default()
                    },
                )
                .unwrap();
            assert_eq!(
                extract_post(&world, &ids).unwrap().tonemap,
                expected,
                "{authored:?} must map across the boundary"
            );
        }
    }

    /// A spot light extracts normalized, with cosines of its half-angles; a Rust-built value that
    /// skipped validation is dropped (no range, no axis) or clamped (angles, colour, intensity).
    #[test]
    fn spot_light_data_normalizes_and_clamps() {
        let light = SpotLight {
            direction: [0.0, -3.0, 0.0],
            inner_angle: 60.0,
            outer_angle: 90.0,
            ..SpotLight::default()
        };
        let data = spot_light_data(&light, Vec3::new(1.0, 2.0, 3.0)).expect("a usable spot");
        assert_eq!(data.position, Vec3::new(1.0, 2.0, 3.0));
        assert_eq!(data.direction, Vec3::NEG_Y);
        assert!((data.cos_inner - 0.5).abs() < 1e-6, "{}", data.cos_inner);
        assert!(
            data.cos_outer > 0.0 && data.cos_outer < 0.002,
            "a 90 degree outer clamps just short of a hemisphere: {}",
            data.cos_outer
        );

        let swapped = SpotLight {
            inner_angle: 40.0,
            outer_angle: 10.0,
            ..SpotLight::default()
        };
        let data = spot_light_data(&swapped, Vec3::ZERO).unwrap();
        assert_eq!(data.cos_inner, data.cos_outer, "inner never passes outer");

        let negative = SpotLight {
            color: [-1.0, f32::NAN, 0.5],
            intensity: -2.0,
            ..SpotLight::default()
        };
        let data = spot_light_data(&negative, Vec3::ZERO).unwrap();
        assert!(
            (data.color * data.intensity)
                .to_array()
                .iter()
                .all(|c| *c >= 0.0)
        );

        for direction in [[0.0; 3], [f32::NAN, 1.0, 0.0], [f32::INFINITY, 0.0, 0.0]] {
            let light = SpotLight {
                direction,
                ..SpotLight::default()
            };
            assert!(
                spot_light_data(&light, Vec3::ZERO).is_none(),
                "{direction:?}"
            );
        }
        for range in [0.0, -1.0, f32::NAN, f32::INFINITY] {
            let light = SpotLight {
                range,
                ..SpotLight::default()
            };
            assert!(
                spot_light_data(&light, Vec3::ZERO).is_none(),
                "range {range}"
            );
        }
    }

    /// Spots reach `RenderScene.spot_lights` in ascending-`EntityId` order, bad ones dropped, and
    /// their presence leaves the point-light list exactly as it was without them.
    #[test]
    fn spots_extract_in_entity_order_and_leave_point_lights_alone() {
        use ludion_core::math::glam::Vec3;
        use ludion_ecs::World;
        use std::path::Path;

        fn at(world: &mut World, x: f32) -> EntityId {
            let id = world.spawn();
            world
                .insert(
                    id,
                    Transform {
                        translation: [x, 0.0, 0.0],
                        ..Transform::default()
                    },
                )
                .unwrap();
            id
        }

        let mut world = World::new();
        let first = at(&mut world, 5.0);
        world
            .insert(
                first,
                SpotLight {
                    direction: [0.0, 0.0, -2.0],
                    ..SpotLight::default()
                },
            )
            .unwrap();
        let point = at(&mut world, -1.0);
        world.insert(point, PointLight::default()).unwrap();
        let broken = at(&mut world, 0.0);
        world
            .insert(
                broken,
                SpotLight {
                    direction: [0.0; 3],
                    ..SpotLight::default()
                },
            )
            .unwrap();
        let last = at(&mut world, 7.0);
        world.insert(last, SpotLight::default()).unwrap();

        let with_spots = extract(&world, (320, 180), Path::new(".")).unwrap();
        assert_eq!(
            with_spots
                .spot_lights
                .iter()
                .map(|s| s.position.x)
                .collect::<Vec<_>>(),
            [5.0, 7.0],
            "ascending EntityId, the zero-direction spot dropped"
        );
        assert_eq!(with_spots.spot_lights[0].direction, Vec3::NEG_Z);

        world.remove::<SpotLight>(first).unwrap();
        world.remove::<SpotLight>(last).unwrap();
        let without = extract(&world, (320, 180), Path::new(".")).unwrap();
        assert!(without.spot_lights.is_empty());
        assert_eq!(
            with_spots.lights, without.lights,
            "point lights unperturbed"
        );
        assert_eq!(with_spots.lights.len(), 1);
    }

    /// The order contract, replacing the two `select_lights` tests the cap removal deleted
    /// (`Plan/clustered-forward.md` decision 2). Every placed light survives — 18 of them, well past
    /// the old 16-light cap — in the exact ascending-`EntityId` collection order, *unsorted*, even
    /// though that order is deliberately not nearest-camera-first. This order is the per-fragment
    /// float summation order of every lit pixel; a sort of any kind here would churn every lit
    /// golden and break the clustered loop's identity argument.
    #[test]
    fn extract_lights_passes_every_light_through_in_extract_order() {
        use ludion_ecs::World;

        let mut world = World::new();
        let mut placed: Vec<(EntityId, PointLightData)> = Vec::new();
        // 18 lights authored far → near, so any distance sort WOULD reorder them.
        for i in 0..18u32 {
            let id = world.spawn();
            placed.push((
                id,
                PointLightData {
                    position: Vec3::new((18 - i) as f32 * 3.0, 0.0, 0.0),
                    color: Vec3::ONE,
                    intensity: 1.0,
                    range: 10.0,
                    dynamic: false,
                },
            ));
        }
        let authored: Vec<f32> = placed.iter().map(|(_, l)| l.position.x).collect();

        let extracted = extract_lights(placed);
        assert_eq!(
            extracted.len(),
            18,
            "the cap is gone: nothing is dropped above 16 lights"
        );
        assert_eq!(
            extracted.iter().map(|l| l.position.x).collect::<Vec<_>>(),
            authored,
            "extract order is preserved verbatim (no sort, no selection)"
        );
    }

    /// The assembly clip evaluator: what [`part_placements`] does to a two-part fixture, at rest and
    /// posed. The fixture is the module-doc frames worked by hand, so an expected matrix here is an
    /// arithmetic statement, not a re-derivation of the code under test.
    mod clip_evaluator {
        use super::*;
        use crate::assembly_mesh::{AssemblyClipData, AssemblyData, AssemblyPartData};
        use ludion_renderer::MeshData;
        use ludion_scene::{AnimationDocument, AnimationTrack, Ease, Keyframe};

        /// An empty mesh — the evaluator never reads geometry, only `offset`/`pivot`/`parent`.
        fn mesh() -> Arc<MeshData> {
            Arc::new(MeshData {
                positions: Vec::new(),
                normals: Vec::new(),
                colors: Vec::new(),
                materials: Vec::new(),
                uvs: Vec::new(),
                tangents: Vec::new(),
                weights: Vec::new(),
                bone_indices: Vec::new(),
                skin_joints: Vec::new(),
                textures: None,
                authored_material: false,
                indices: Vec::new(),
                hash: 0,
            })
        }

        fn part(
            name: &str,
            offset: [f32; 3],
            pivot: [f32; 3],
            parent: Option<usize>,
        ) -> AssemblyPartData {
            AssemblyPartData {
                name: name.to_string(),
                mesh: mesh(),
                size: [1, 1, 1],
                offset,
                pivot,
                parent,
            }
        }

        /// `body` at the origin with `arm` parented to it, its mesh origin at `(2, 1, 0)` and its
        /// pivot one unit **left** of that origin (so the joint sits at world `(1, 1, 0)`).
        fn assembly(clips: Vec<AssemblyClipData>) -> AssemblyData {
            AssemblyData {
                size: [3, 2, 1],
                parts: vec![
                    part("body", [0.0, 0.0, 0.0], [0.0, 0.0, 0.0], None),
                    part("arm", [2.0, 1.0, 0.0], [-1.0, 0.0, 0.0], Some(0)),
                ],
                clips,
                joints: Vec::new(),
            }
        }

        /// A clip binding `name`: over one linear second `part` rotates from `[0, 0, 0]` to `to`
        /// **and** translates by `part_shift`, while `body` translates by `body_shift`. `part`
        /// carrying both channels at once is what lets a test pin the `T · R` order.
        fn clip(
            name: &str,
            part: &str,
            to: [f64; 3],
            part_shift: [f64; 3],
            body_shift: [f64; 3],
        ) -> AssemblyClipData {
            let channel = |from: [f64; 3], to: [f64; 3]| {
                vec![
                    Keyframe {
                        t: 0.0,
                        v: from,
                        ease: Ease::Linear,
                    },
                    Keyframe {
                        t: 1.0,
                        v: to,
                        ease: Ease::Linear,
                    },
                ]
            };
            AssemblyClipData {
                name: name.to_string(),
                file: format!("anims/{name}.anim.yaml"),
                clip: AnimationDocument {
                    name: Some(name.to_string()),
                    duration: 1.0,
                    looped: true,
                    tracks: vec![
                        AnimationTrack {
                            target: "body".to_string(),
                            rotation: Vec::new(),
                            translation: channel([0.0; 3], body_shift),
                            scale: Vec::new(),
                        },
                        AnimationTrack {
                            target: part.to_string(),
                            rotation: channel([0.0; 3], to),
                            translation: channel([0.0; 3], part_shift),
                            scale: Vec::new(),
                        },
                    ],
                    ..Default::default()
                },
            }
        }

        /// No clip is the historical path exactly: one plain translation per part and an identity
        /// normal basis, whatever the hierarchy says.
        #[test]
        fn no_pose_is_the_rest_translation() {
            let data = assembly(Vec::new());
            let placements = part_placements(&data, None);
            assert_eq!(placements[0].0, Mat4::from_translation(Vec3::ZERO));
            assert_eq!(
                placements[1].0,
                Mat4::from_translation(Vec3::new(2.0, 1.0, 0.0))
            );
            assert_eq!(placements[0].1, Mat3::IDENTITY);
            assert_eq!(placements[1].1, Mat3::IDENTITY);
        }

        /// A clip sampled at its first instant reproduces the rest pose **bit-for-bit**, through the
        /// posed branch — the property that lets an animated entity sit still without drifting.
        #[test]
        fn the_first_instant_of_a_zeroed_clip_is_the_rest_pose() {
            let data = assembly(vec![clip("walk", "arm", [0.0; 3], [0.0; 3], [0.0; 3])]);
            let posed = part_placements(&data, Some((data.clip("walk").unwrap(), 0.0)));
            assert_eq!(posed, part_placements(&data, None));
        }

        /// The parent-chain composition **and** the `T · R` order, pinned to an exact matrix.
        ///
        /// At the clip's end `arm` has turned 90° about +X *and* translated `(0, 1, 0)`, while its
        /// parent `body` has slid `(0, 0, 3)`. Working it by hand:
        ///
        /// - `body` animates translation only: `local = T(0, 0, 3)`.
        /// - `arm`'s pivot sits at `(-1, 0, 0)` from its own mesh origin, and a +90° X rotation maps
        ///   `(-1, 0, 0)` to itself, so `rotate-about(pivot)` is the bare rotation.
        /// - the translation key applies in `arm`'s **rest** frame, so it adds to the offset before
        ///   the rotation: `local = T(2, 1, 0) · T(0, 1, 0) · R = T(2, 2, 0) · R`.
        /// - composed: `world = T(0, 0, 3) · T(2, 2, 0) · R = T(2, 2, 3) · R`.
        ///
        /// The rejected `T · R · T` order (translating in the *rotated* frame) would send the key
        /// `(0, 1, 0)` to `(0, 0, 1)` and land the arm at `(2, 1, 4)` instead — asserted against
        /// below, so this test fails if the order is ever flipped back.
        #[test]
        fn a_child_follows_its_parent_and_turns_about_its_own_pivot() {
            let data = assembly(vec![clip(
                "walk",
                "arm",
                [90.0, 0.0, 0.0],
                [0.0, 1.0, 0.0],
                [0.0, 0.0, 3.0],
            )]);
            let posed = part_placements(&data, Some((data.clip("walk").unwrap(), 1.0)));

            // The parent: a pure translation, its own rest offset plus the clip's slide.
            assert_eq!(
                posed[0].0,
                Mat4::from_translation(Vec3::new(0.0, 0.0, 3.0)),
                "the root takes its track's translation and nothing else"
            );

            let turn = Quat::from_rotation_x(std::f32::consts::FRAC_PI_2);
            let close = |got: Mat4, want: Mat4| {
                got.to_cols_array()
                    .iter()
                    .zip(want.to_cols_array())
                    .all(|(a, b)| (a - b).abs() < 1e-6)
            };

            // The child: turned 90° about +X, translated in its rest frame, carried by its parent.
            let expected = Mat4::from_rotation_translation(turn, Vec3::new(2.0, 2.0, 3.0));
            let got = posed[1].0;
            assert!(close(got, expected), "got {got:?}, want {expected:?}");

            // A translation key must never be bent by its own part's rotation.
            let rotated_frame = Mat4::from_rotation_translation(turn, Vec3::new(2.0, 1.0, 4.0));
            assert!(
                !close(got, rotated_frame),
                "the translation key must apply in the rest frame, not the rotated one"
            );

            // The normal basis is the composed rotation — the same 90° turn, no scale anywhere.
            let up = posed[1].1 * Vec3::Y;
            assert!(
                (up - Vec3::Z).length() < 1e-6,
                "+Y must rotate onto +Z: {up:?}"
            );
        }

        /// Halfway through a linear clip is halfway through the pose — the proof the evaluator is
        /// actually sampling time and not snapping between keys.
        #[test]
        fn a_mid_clip_time_differs_from_both_ends() {
            let data = assembly(vec![clip(
                "walk",
                "arm",
                [90.0, 0.0, 0.0],
                [0.0; 3],
                [0.0, 0.0, 4.0],
            )]);
            let walk = data.clip("walk").unwrap();
            let (start, middle, end) = (
                part_placements(&data, Some((walk, 0.0))),
                part_placements(&data, Some((walk, 0.5))),
                part_placements(&data, Some((walk, 1.0))),
            );
            assert_ne!(start[1].0, middle[1].0);
            assert_ne!(middle[1].0, end[1].0);
            // The root's translation channel is linear, so the midpoint is exactly half the slide.
            assert_eq!(
                middle[0].0,
                Mat4::from_translation(Vec3::new(0.0, 0.0, 2.0))
            );
        }

        /// A part the clip never mentions stays exactly at rest even while its siblings move — but a
        /// *child* of a moving part still follows it, which is the whole point of the chain.
        #[test]
        fn an_unanimated_child_still_follows_a_moving_parent() {
            let data = assembly(vec![clip(
                "walk",
                "nobody",
                [90.0, 0.0, 0.0],
                [0.0; 3],
                [0.0, 5.0, 0.0],
            )]);
            let posed = part_placements(&data, Some((data.clip("walk").unwrap(), 1.0)));
            assert_eq!(posed[0].0, Mat4::from_translation(Vec3::new(0.0, 5.0, 0.0)));
            assert_eq!(
                posed[1].0,
                Mat4::from_translation(Vec3::new(2.0, 6.0, 0.0)),
                "the arm has no track, so it rides the parent unchanged"
            );
            assert_eq!(
                posed[1].1,
                Mat3::IDENTITY,
                "no rotation anywhere in the chain"
            );
        }

        /// A scale key on `body` stretches its own x-axis and carries the child out along it; the
        /// normals go through the inverse-transpose, and a ragdoll (`scale: false`) sees none of it.
        #[test]
        fn a_scale_key_stretches_the_part_and_moves_its_child() {
            let key = |t: f64, v: [f64; 3]| Keyframe {
                t,
                v,
                ease: Ease::Linear,
            };
            let data = assembly(vec![AssemblyClipData {
                name: "squash".to_string(),
                file: "anims/squash.anim.yaml".to_string(),
                clip: AnimationDocument {
                    name: Some("squash".to_string()),
                    duration: 1.0,
                    looped: true,
                    tracks: vec![AnimationTrack {
                        target: "body".to_string(),
                        rotation: Vec::new(),
                        translation: Vec::new(),
                        scale: vec![key(0.0, [1.0; 3]), key(1.0, [2.0, 1.0, 1.0])],
                    }],
                    ..Default::default()
                },
            }]);
            let pose = Some((data.clip("squash").unwrap(), 1.0));
            let posed = part_placements(&data, pose);
            assert_eq!(posed[0].0.x_axis.truncate().length(), 2.0);
            assert_eq!(
                posed[1].0.transform_point3(Vec3::ZERO),
                Vec3::new(4.0, 1.0, 0.0)
            );
            assert_eq!(posed[0].1 * Vec3::X, Vec3::new(0.5, 0.0, 0.0));

            let rigid = part_placements_with(&data, pose, None, false);
            assert_eq!(rigid, part_placements(&data, None));
        }

        /// A child authored **before** its parent composes just as well: the chain is resolved by
        /// links, never by document order.
        #[test]
        fn a_child_authored_before_its_parent_still_composes() {
            let data = AssemblyData {
                size: [3, 2, 1],
                parts: vec![
                    part("arm", [2.0, 1.0, 0.0], [-1.0, 0.0, 0.0], Some(1)),
                    part("body", [0.0, 0.0, 0.0], [0.0, 0.0, 0.0], None),
                ],
                clips: vec![clip("walk", "arm", [0.0; 3], [0.0; 3], [0.0, 0.0, 3.0])],
                joints: Vec::new(),
            };
            let posed = part_placements(&data, Some((data.clip("walk").unwrap(), 1.0)));
            assert_eq!(posed[1].0, Mat4::from_translation(Vec3::new(0.0, 0.0, 3.0)));
            assert_eq!(posed[0].0, Mat4::from_translation(Vec3::new(2.0, 1.0, 3.0)));
        }

        /// Clip ticks and seconds are derived from the exact tick counts, scaled by `speed`; a
        /// start frame in the future saturates to zero rather than wrapping, and an anchor of this
        /// playback continues its ticks from the anchor's frame.
        #[test]
        fn clock_ticks_and_elapsed_derive_from_tick_counts() {
            let at = |ticks| SimTime::from_ticks(ticks, TickRate::DEFAULT);
            let ticks =
                |now, started, speed| clock_ticks(at(now), FrameIndex::new(started), speed, None);
            let secs =
                |now, started, speed| clock_elapsed(at(now), FrameIndex::new(started), speed, None);
            assert_eq!(ticks(60, 0, 1.0), 60.0);
            assert_eq!(secs(60, 0, 1.0), 1.0);
            assert_eq!(secs(90, 30, 1.0), 1.0);
            assert_eq!(secs(60, 0, 2.0), 2.0);
            assert_eq!(ticks(60, 0, 0.0), 0.0);
            assert_eq!(ticks(10, 600, 1.0), 0.0);
            assert_eq!(secs(10, 600, 1.0), 0.0);

            let anchor = ClipAnchor {
                started: FrameIndex::new(10),
                frame: FrameIndex::new(40),
                ticks: 30.0,
            };
            let anchored =
                |started| clock_ticks(at(100), FrameIndex::new(started), 0.5, Some(&anchor));
            assert_eq!(anchored(10), 30.0 + 60.0 * 0.5);
            assert_eq!(anchored(11), 44.5, "a stale anchor is ignored");
            assert_eq!(
                clock_elapsed(at(100), FrameIndex::new(10), 0.5, Some(&anchor)),
                1.0
            );
        }
    }

    /// The voxelizer's instance-signature cache, through the cached extract path
    /// (`Plan/gi-wave1-sky-visibility.md` §11 test 7). Every assertion is on
    /// [`crate::voxelizer::rasterization_count`] — the per-cell walk itself — so a hit is proven to
    /// skip the work rather than merely to reproduce the same grid.
    mod voxelization_cache {
        use ludion_ecs::World;

        use super::*;
        use crate::asset_cache::AssetCache;
        use crate::components::{GiDynamic, GiVolume, Mesh, MeshSource, Transform};
        use crate::voxelizer::rasterization_count;

        /// A world with one cube at `x` and the opt-in `GiVolume` at `cell_size`.
        fn world_with_a_cube(x: f32, cell_size: f32) -> World {
            let mut world = World::new();
            let cube = world.spawn();
            world
                .insert(
                    cube,
                    Transform {
                        translation: [x, 0.0, 0.0],
                        ..Transform::default()
                    },
                )
                .unwrap();
            world
                .insert(
                    cube,
                    Mesh {
                        shape: MeshSource::Cube,
                        ..Mesh::default()
                    },
                )
                .unwrap();
            let volume = world.spawn();
            world
                .insert(
                    volume,
                    GiVolume {
                        cell_size,
                        ..GiVolume::default()
                    },
                )
                .unwrap();
            world
        }

        /// [`world_with_a_cube`] plus a second, **marked** cube at `x` — an anchor cube keeps the
        /// grid bounded (so a marked cube's exclusion, not an empty scene, is what a test is
        /// measuring) while the marked cube itself is the T13 fixture: excluded from the voxelizer's
        /// instance collection and signature both.
        fn world_with_a_marked_cube_added(x: f32, cell_size: f32) -> World {
            let mut world = world_with_a_cube(0.0, cell_size);
            let marked = world.spawn();
            world
                .insert(
                    marked,
                    Transform {
                        translation: [x, 0.0, 0.0],
                        ..Transform::default()
                    },
                )
                .unwrap();
            world
                .insert(
                    marked,
                    Mesh {
                        shape: MeshSource::Cube,
                        ..Mesh::default()
                    },
                )
                .unwrap();
            world.insert(marked, GiDynamic::default()).unwrap();
            world
        }

        /// Extracts `world` through `cache` and returns the grid it built.
        fn voxelize_through(world: &World, cache: &mut AssetCache) -> Arc<WorldVoxelGrid> {
            extract_through(
                world,
                (320, 180),
                Path::new("."),
                cache,
                tick_zero(),
                QualityParams::FULL.particle_density,
                None,
                &mut Vec::new(),
            )
            .expect("the fixture world extracts")
            .voxel
            .expect("a GiVolume builds a grid")
        }

        /// Every one-shot extract builds its own fresh cache, so nothing is shared between calls:
        /// each one rasterizes, as the loader that retained nothing always did.
        #[test]
        fn a_one_shot_extract_always_rebakes() {
            let world = world_with_a_cube(0.0, 0.5);
            let before = rasterization_count();
            for _ in 0..2 {
                extract_at(
                    &world,
                    (320, 180),
                    Path::new("."),
                    tick_zero(),
                    QualityParams::FULL.particle_density,
                )
                .expect("the fixture world extracts");
            }
            assert_eq!(rasterization_count() - before, 2);
        }

        /// A one-shot extract (fresh, prefetched cache) resolves the same assets as the live
        /// cache's lazy path.
        #[test]
        fn a_one_shot_extract_matches_the_live_cache() {
            let dir = std::env::temp_dir().join(format!(
                "ludion-one-shot-{}-{}",
                std::process::id(),
                "matches"
            ));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(dir.join("meshes")).unwrap();
            std::fs::create_dir_all(dir.join("textures")).unwrap();
            let png = |red: u8| ludion_assets::encode_png(1, 1, &[red, 20, 30, 255]).unwrap();
            std::fs::write(dir.join("textures/t.png"), png(10)).unwrap();
            std::fs::write(dir.join("textures/s.png"), png(30)).unwrap();
            let list = |values: &[f32]| {
                values
                    .iter()
                    .map(|v| format!("  - {v:?}\n"))
                    .collect::<String>()
            };
            std::fs::write(
                dir.join("meshes/a.mesh.yaml"),
                format!(
                    "format: 1\nkind: mesh\npositions:\n{}uvs:\n{}base_color_texture:\n  path: \
                     ../textures/t.png\nindices:\n  - 0\n  - 1\n  - 2\n",
                    list(&[0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0]),
                    list(&[0.0, 0.0, 1.0, 0.0, 0.0, 1.0]),
                ),
            )
            .unwrap();

            let mut world = World::new();
            let mesh = world.spawn();
            world
                .insert(
                    mesh,
                    Mesh {
                        shape: MeshSource::Asset("meshes/a.mesh.yaml".into()),
                        ..Mesh::default()
                    },
                )
                .unwrap();
            let sprite = world.spawn();
            world
                .insert(
                    sprite,
                    Sprite {
                        texture: Some("textures/s.png".into()),
                        ..Sprite::default()
                    },
                )
                .unwrap();

            let density = QualityParams::FULL.particle_density;
            let one_shot = extract_at(&world, (320, 180), &dir, tick_zero(), density).unwrap();
            let live = extract_through(
                &world,
                (320, 180),
                &dir,
                &mut AssetCache::new(),
                tick_zero(),
                density,
                None,
                &mut Vec::new(),
            )
            .unwrap();
            let _ = std::fs::remove_dir_all(&dir);

            let hashes = |scene: &RenderScene| {
                (
                    scene.mesh_assets.iter().map(|m| m.hash).collect::<Vec<_>>(),
                    scene.textures.iter().map(|t| t.hash).collect::<Vec<_>>(),
                    scene.meshes.len(),
                    scene.sprites.len(),
                )
            };
            assert_eq!(hashes(&one_shot), hashes(&live));
            assert_eq!(one_shot.mesh_assets.len(), 1);
            assert_eq!(
                one_shot.textures.len(),
                1,
                "the sprite; a mesh map rides its MeshData"
            );
        }

        /// A second extract of an unchanged world rasterizes **nothing** and hands back the very
        /// same grid allocation — the per-frame multi-million-cell walk becomes load-time work.
        #[test]
        fn a_second_extract_of_an_unchanged_world_rasterizes_nothing() {
            let world = world_with_a_cube(0.0, 0.5);
            let mut cache = AssetCache::new();

            let before = rasterization_count();
            let first = voxelize_through(&world, &mut cache);
            assert_eq!(
                rasterization_count() - before,
                1,
                "the first extract rasterizes once"
            );

            let second = voxelize_through(&world, &mut cache);
            assert_eq!(
                rasterization_count() - before,
                1,
                "an unchanged world is a signature hit, not a second rasterization"
            );
            assert!(
                Arc::ptr_eq(&first, &second),
                "a hit hands back the cached grid itself"
            );
        }

        /// Moving one instance moves its model matrix, so the signature moves and the grid is
        /// rebuilt — the cache changes *how often* the grid is produced, never *what* it is.
        #[test]
        fn moving_an_instance_invalidates_the_signature() {
            let mut cache = AssetCache::new();
            let before = rasterization_count();

            let here = voxelize_through(&world_with_a_cube(0.0, 0.5), &mut cache);
            let moved = voxelize_through(&world_with_a_cube(4.0, 0.5), &mut cache);
            assert_eq!(
                rasterization_count() - before,
                2,
                "a moved instance must rebuild"
            );
            assert_ne!(here.origin, moved.origin, "and the rebuilt grid differs");

            // Moving back is a miss too: the cache holds one entry, and it now describes the move.
            let back = voxelize_through(&world_with_a_cube(0.0, 0.5), &mut cache);
            assert_eq!(rasterization_count() - before, 3);
            assert_eq!(back.hash, here.hash, "and reproduces the original grid");
        }

        /// **T13's cache-hit half** — the mesh-side twin of T5c's move assertion
        /// (`Plan/gi-wave3-dynamic-injection.md` §11): moving a **marked** instance is a signature
        /// hit, not a rebuild, because the marked instance never entered the signature to begin
        /// with. Contrast [`moving_an_instance_invalidates_the_signature`], which pins the unchanged
        /// consequence for an unmarked instance — that a *marked* light's motion is free and an
        /// *unmarked* one's is not is the decision this pair keeps honest.
        #[test]
        fn moving_a_marked_instance_does_not_invalidate_the_signature() {
            let mut cache = AssetCache::new();
            let before = rasterization_count();

            let here = voxelize_through(&world_with_a_marked_cube_added(4.0, 0.5), &mut cache);
            assert_eq!(
                rasterization_count() - before,
                1,
                "the first extract rasterizes once"
            );

            let moved = voxelize_through(&world_with_a_marked_cube_added(9.0, 0.5), &mut cache);
            assert_eq!(
                rasterization_count() - before,
                1,
                "moving the marked instance must be a cache hit, not a rebuild"
            );
            assert_eq!(
                here.hash, moved.hash,
                "the grid the marked motion cannot touch is bit-identical"
            );
        }

        /// The authored `cell_size` is in the signature on its own account — the same instances at a
        /// different resolution are a different grid.
        #[test]
        fn changing_only_the_cell_size_invalidates_the_signature() {
            let mut cache = AssetCache::new();
            let before = rasterization_count();

            let coarse = voxelize_through(&world_with_a_cube(0.0, 0.5), &mut cache);
            let fine = voxelize_through(&world_with_a_cube(0.0, 0.25), &mut cache);

            assert_eq!(
                rasterization_count() - before,
                2,
                "a changed cell_size must rebuild"
            );
            assert_eq!(coarse.extent, [2, 2, 2]);
            assert_eq!(fine.extent, [4, 4, 4]);
        }

        /// **The hold, and every transition through it** (the CPU half of the rebake lever). Moving
        /// an *unmarked* cube under a hold rasterizes nothing, hands back the very same grid, and
        /// reports staleness; moving it back to the held baseline is a plain signature hit and is
        /// honestly not stale — the flag is recomputed from that comparison every extract, never
        /// latched. Contrast [`moving_an_instance_invalidates_the_signature`], the same motion at
        /// `Auto`.
        #[test]
        fn a_held_extract_serves_the_retained_grid_and_reports_stale() {
            let mut cache = AssetCache::new();
            let before = rasterization_count();

            let baseline = voxelize_through(&world_with_a_cube(0.0, 0.5), &mut cache);
            assert_eq!(rasterization_count() - before, 1);
            assert!(!cache.gi_stale(), "the first build is the current world");

            cache.set_gi_hold(true);
            let held = voxelize_through(&world_with_a_cube(4.0, 0.5), &mut cache);
            assert_eq!(
                rasterization_count() - before,
                1,
                "a held signature miss must serve the retained grid, not re-walk the cells",
            );
            assert!(cache.gi_stale(), "and must report that it is held");
            assert!(
                Arc::ptr_eq(&baseline, &held),
                "the served grid is the retained allocation itself",
            );

            // The held key is left alone, so the baseline stays the baseline however far the world
            // wanders — and a world that wanders back is a real hit.
            let back = voxelize_through(&world_with_a_cube(0.0, 0.5), &mut cache);
            assert_eq!(rasterization_count() - before, 1);
            assert!(
                !cache.gi_stale(),
                "a world returned to the held baseline is honestly not stale",
            );
            assert!(Arc::ptr_eq(&baseline, &back));

            // The rebake: one extract with the hold suppressed rebuilds, and what it builds becomes
            // the new baseline — holding again over that same world is a hit, not a held serve.
            cache.set_gi_hold(false);
            let rebaked = voxelize_through(&world_with_a_cube(4.0, 0.5), &mut cache);
            assert_eq!(
                rasterization_count() - before,
                2,
                "a suppressed hold rebuilds"
            );
            assert!(!cache.gi_stale());
            assert!(!Arc::ptr_eq(&baseline, &rebaked));

            cache.set_gi_hold(true);
            let after = voxelize_through(&world_with_a_cube(4.0, 0.5), &mut cache);
            assert_eq!(rasterization_count() - before, 2);
            assert!(!cache.gi_stale(), "the rebake became the new baseline");
            assert!(Arc::ptr_eq(&rebaked, &after));
        }

        /// A *first* build is never held — an empty cache has nothing to serve, and an unvoxelized
        /// scene would be a worse answer than a stale one. A scene load resets the voxelization
        /// ([`AssetCache::reset_voxelization`]) without touching the host's lever, which is exactly what
        /// makes a new scene under a hold cost one build and then hold from there.
        #[test]
        fn a_first_build_under_a_hold_still_builds() {
            let mut cache = AssetCache::new();
            cache.set_gi_hold(true);
            let before = rasterization_count();

            voxelize_through(&world_with_a_cube(0.0, 0.5), &mut cache);
            assert_eq!(
                rasterization_count() - before,
                1,
                "an empty cache cannot be held"
            );
            assert!(!cache.gi_stale());

            // A new scene: the retained grid goes, the lever stays, so the next world builds once.
            cache.reset_voxelization();
            voxelize_through(&world_with_a_cube(9.0, 0.5), &mut cache);
            assert_eq!(
                rasterization_count() - before,
                2,
                "a cleared cache rebuilds"
            );
            assert!(!cache.gi_stale());
            voxelize_through(&world_with_a_cube(12.0, 0.5), &mut cache);
            assert_eq!(
                rasterization_count() - before,
                2,
                "and the hold survived the clear",
            );
            assert!(cache.gi_stale());
        }

        /// Staleness belongs to the extract that computed it: a scene that stops authoring a
        /// `GiVolume` consults the voxelization cache not at all, and must not inherit the last
        /// answer.
        #[test]
        fn dropping_the_gi_volume_clears_the_stale_flag() {
            let mut cache = AssetCache::new();
            voxelize_through(&world_with_a_cube(0.0, 0.5), &mut cache);
            cache.set_gi_hold(true);
            voxelize_through(&world_with_a_cube(4.0, 0.5), &mut cache);
            assert!(cache.gi_stale());

            let mut world = World::new();
            let cube = world.spawn();
            world.insert(cube, Transform::default()).unwrap();
            world
                .insert(
                    cube,
                    Mesh {
                        shape: MeshSource::Cube,
                        ..Mesh::default()
                    },
                )
                .unwrap();
            let scene = extract_through(
                &world,
                (320, 180),
                Path::new("."),
                &mut cache,
                tick_zero(),
                QualityParams::FULL.particle_density,
                None,
                &mut Vec::new(),
            )
            .expect("the fixture world extracts");
            assert!(scene.voxel.is_none(), "no GiVolume, no grid");
            assert!(!cache.gi_stale());
        }

        /// The grid the voxelizer hands out is sealed: its stamped `hash` re-derives from its own
        /// bytes, so `VoxelBake`'s `u64` compare stands for the buffers it skips re-reading.
        #[test]
        fn the_produced_grid_carries_its_own_content_hash() {
            let mut cache = AssetCache::new();
            let grid = voxelize_through(&world_with_a_cube(0.0, 0.5), &mut cache);
            assert_eq!(
                grid.hash,
                grid.content_hash(),
                "the stamped hash must equal a freshly computed one"
            );
        }
    }
}
