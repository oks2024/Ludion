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
//! The drawn camera is the rig's pose with the presentation [`CameraOffset`] composed on
//! (`compose_camera`, read by both `build_camera` and the blend's camera), so a scene without one
//! extracts the rig's pose unchanged.

use std::any::TypeId;
use std::borrow::Cow;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use ludion_assets::AssetRoot;
use ludion_assets::Image;
use ludion_assets::cube::CubeLut;
use ludion_core::codes::code;
use ludion_core::collections::StableMap;
use ludion_core::diagnostics::{Diagnostic, pointer_token};
use ludion_core::ids::EntityId;
use ludion_core::math::glam::{Mat3, Mat4, Quat, Vec3};
use ludion_core::math::{Affine3, Trs, euler_xyz_degrees_to_quat, quat_to_euler_xyz_degrees};
use ludion_core::par;
use ludion_core::time::{FrameIndex, GameClock, GameTime, Q16_PER_TICK, SimTime, TickRate};
use ludion_ecs::{ReadColumn, World};
use ludion_protocol::command::ViewPose;
use ludion_renderer::{
    BlendData, BlendMode, BlendedMesh, CameraBlend, DebugSegment, DecalBlend, DecalInstance,
    DecalMaps, DecalReceivers, DropShadowInstance, EnvironmentData, FontDraw, GROUND_MARKER_LIFT,
    GiData, GroundMarkerInstance, InstanceKey, LocalShadowData, MAX_SUN_CASCADES, MapImage,
    MeshData, MeshInstance, MeshShape, MeshTexture, ParticleInstance, PointLightData, PostData,
    ProbeData, ProbeVolume, ProjectileItem, QualityParams, RenderCamera, RenderScene, SkinJoint,
    SpotLightData, SpriteInstance, SunCascadeConfig, SunData, TextRun, TextureImage, TextureWrap,
    UiInstance, WorldVoxelGrid, aspect_ratio,
};
use ludion_renderer::{DEFAULT_POROSITY, LodSettings};
use ludion_renderer::{UiClip, UiRotation};
use ludion_scene::{AtlasDocument, AtlasFilter, ClipSampler, FontMode};

use crate::assembly_mesh::AssemblyData;
use crate::asset_cache::{AssetCache, AssetRefs, SourceFiles};
use crate::character::CharacterData;
use crate::components::PoseSnapshot;
use crate::components::SubtitleView;
use crate::components::{
    Animation, BlendDepth, Camera, CameraOffset, ClipAnchor, Decal,
    DecalBlend as DecalBlendAuthored, DecalMap, DecalReceiverSet, DirectionalLight, DropShadow,
    Environment, GiDynamic, GiVolume, GroundMarker, GroundMarkerGrid, GroundMarkerShape, MapWrap,
    Mesh, MeshBlend, MeshGrid, MeshSource, ParentJoint, ParticleEmitter, PointLight, PostProcess,
    PostProfile, ProbeShape, Projection, ReflectionProbe, Rim, SkinBone, SpotLight, Sprite,
    SpriteBlend, Text, Tilemap, TonemapOperator, Transform, UiList, UiRadial, UiRect, UiShape,
    UiText,
};
use crate::components::{AnimationLayer, AnimationLayers, BlendClock, LayerMode, layer_weight_at};
use crate::components::{ExposureMode, GrainMotion, Metering};
use crate::dialogue::Subtitles;
use crate::docs::DocStore;
use crate::extract_observer::{ExtractPhase, SkinCounts};
use crate::fonts::FontLibrary;
use crate::hierarchy::{FrameVisibility, WorldAffines, level_trs};
use crate::load_observer::LoadPhase;
use crate::particles::{self, CameraBasis};
use crate::patterns::{self, BulletKind, PatternDoc, Volley};
use crate::prefab::PrefabInstance;
use crate::prefab_clips::{PartRests, PrefabClipData};
use crate::projectile_draw::ProjectileDraw;
use crate::sequences::bound::rebased_instant;
use crate::static_scene::{self, Feed, StaticScene};
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
    /// Resolves a decal's map at the content-root path `rel`. Always the PNG, never a cooked twin:
    /// sync cooks only the maps mesh documents name.
    fn map(&mut self, rel: &str) -> Result<MapImage, Diagnostic>;
    /// Resolves a `Sprite.atlas` / `Tilemap` legend `atlas` path to its parsed `kind: atlas`
    /// document and packed colour image. `Arc`-shared like a mesh: a stage is one atlas, so every
    /// quad on screen resolves the same entry.
    fn atlas(&mut self, rel: &str) -> Result<Arc<AtlasAsset>, Vec<Diagnostic>>;
    /// Resolves a `PostProcess.lut` path to its parsed `.cube` table, `Arc`-shared like an atlas.
    fn lut(&mut self, rel: &str) -> Result<Arc<CubeLut>, Vec<Diagnostic>>;
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
    /// Resolves a prefab's content-root path to the clips it binds and its parts' authored rests,
    /// for an animated instance root.
    fn prefab_clips(&mut self, rel: &str) -> Result<Arc<PrefabClipData>, Vec<Diagnostic>>;
    /// The part rests of prefab source `text` (the text an instance was expanded from), memoised by
    /// content; `None` when it does not parse or this loader holds no cache.
    fn prefab_rests(&mut self, text: &str) -> Option<Arc<PartRests>>;
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
    /// An extract phase starts. A loader with no extract observer ignores it.
    fn extract_begin(&mut self, _phase: ExtractPhase) {}
    /// The extract phase last begun ends.
    fn extract_end(&mut self, _phase: ExtractPhase) {}
    /// The extract's skin counters, inside the `Skin` bracket.
    fn extract_skin(&mut self, _counts: SkinCounts) {}
    /// The memo of ground-marker visibility profiles; `None` for a loader with nowhere to keep it,
    /// whose clipped markers cast fresh every extract.
    fn clip_cache(&mut self) -> Option<&mut crate::ground_marker_clip::ClipCache> {
        None
    }
}

/// The cached loader: each asset resolves **through** the [`AssetCache`] (miss = load + decode once,
/// hit = a map lookup). Every asset hands out the cache's own `Arc` — a refcount bump, not a buffer
/// copy (`Plan/voxel-artist-scale.md`: a streamed voxel model's `MeshData` runs to
/// megabytes).
struct CachedAssets<'a> {
    root: AssetRoot<'a>,
    cache: &'a mut AssetCache,
}

impl SceneAssets for CachedAssets<'_> {
    fn texture(&mut self, rel: &str) -> Result<Arc<TextureImage>, Diagnostic> {
        self.cache.load(self.root, rel).cloned()
    }
    fn map(&mut self, rel: &str) -> Result<MapImage, Diagnostic> {
        self.cache
            .load_source_map(self.root, rel)
            .map(|(image, _)| image)
    }
    fn atlas(&mut self, rel: &str) -> Result<Arc<AtlasAsset>, Vec<Diagnostic>> {
        self.cache.load_atlas(self.root, rel).cloned()
    }
    fn lut(&mut self, rel: &str) -> Result<Arc<CubeLut>, Vec<Diagnostic>> {
        self.cache.load_lut(self.root, rel).cloned()
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
    fn prefab_clips(&mut self, rel: &str) -> Result<Arc<PrefabClipData>, Vec<Diagnostic>> {
        self.cache.load_prefab_clips(self.root, rel).cloned()
    }
    fn prefab_rests(&mut self, text: &str) -> Option<Arc<PartRests>> {
        self.cache.prefab_rests(text)
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
    fn clip_cache(&mut self) -> Option<&mut crate::ground_marker_clip::ClipCache> {
        Some(self.cache.clip_cache_mut())
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
    fn extract_begin(&mut self, phase: ExtractPhase) {
        if let Some(observer) = self.cache.extract_observer_mut() {
            observer.begin(phase);
        }
    }
    fn extract_end(&mut self, phase: ExtractPhase) {
        if let Some(observer) = self.cache.extract_observer_mut() {
            observer.end(phase);
        }
    }
    fn extract_skin(&mut self, counts: SkinCounts) {
        if let Some(observer) = self.cache.extract_observer_mut() {
            observer.skin(counts);
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
pub(crate) fn load_texture(
    root: AssetRoot<'_>,
    rel: &str,
) -> Result<Arc<TextureImage>, Diagnostic> {
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
        mean: [1.0; 4],
    }))
}

/// Reads and validates the atlas document at `root`/`rel`, then decodes the colour map it names.
///
/// # Errors
/// `L1101` if either file cannot be read, `L1102` if the PNG cannot be decoded, `L1153` if the
/// document does not parse or validate as a `kind: atlas`.
pub(crate) fn load_atlas(root: AssetRoot<'_>, rel: &str) -> Result<AtlasAsset, Vec<Diagnostic>> {
    load_atlas_with_sources(root, rel).map(|(asset, _)| asset)
}

/// [`load_atlas`], additionally reporting the two files it read — the document, then the colour PNG —
/// each with the content hash of the bytes it decoded.
pub(crate) fn load_atlas_with_sources(
    root: AssetRoot<'_>,
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
        mean: [1.0; 4],
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
    project_root: AssetRoot<'_>,
    pointer: &str,
    atlas_rel: &str,
    frame: Option<&str>,
) -> Vec<Diagnostic> {
    let Some(frame) = frame else {
        return vec![atlas_without_frame(pointer)];
    };
    check_loaded_atlas(
        project_root,
        pointer,
        atlas_rel,
        frame,
        &load_atlas(project_root, atlas_rel),
    )
}

/// [`check_atlas_reference`] past the load: the `L1151`/`L1152` set for `frame` against an atlas
/// already loaded (or already failed), so a caller checking many references decodes each atlas once.
pub(crate) fn check_loaded_atlas(
    project_root: AssetRoot<'_>,
    pointer: &str,
    atlas_rel: &str,
    frame: &str,
    loaded: &Result<AtlasAsset, Vec<Diagnostic>>,
) -> Vec<Diagnostic> {
    match loaded {
        Err(causes) => vec![atlas_unresolved(
            project_root,
            atlas_rel,
            pointer,
            causes.clone(),
        )],
        Ok(asset) => match asset.frame(atlas_rel, frame, pointer) {
            Ok(_) => Vec::new(),
            Err(diagnostic) => vec![diagnostic],
        },
    }
}

/// The `L1151` an unreadable or unparseable atlas reports. The underlying causes travel as
/// `related`, their codes and remedies intact; the help names the fix and, for a missing file, the
/// nearest filename.
fn atlas_unresolved(
    project_root: AssetRoot<'_>,
    rel: &str,
    pointer: &str,
    causes: Vec<Diagnostic>,
) -> Diagnostic {
    let mut diagnostic = Diagnostic::new(
        code::SPRITE_ATLAS_UNRESOLVED,
        format!("atlas `{rel}` could not be resolved"),
    )
    .with_path(format!("{pointer}/atlas"))
    .with_help("fix the cause reported below, or correct the `atlas` path; a committed atlas is regenerated with `ludion generate-atlas`");
    diagnostic.related = causes;
    if let Some(name) = suggest_asset(project_root, rel) {
        diagnostic = diagnostic.append_help(format!("nearest match is `{name}`"));
    }
    diagnostic
}

/// The `L1151` an atlas reference with no `frame` reports. Schema validation rejects the same shape
/// before a scene ever instantiates ([`Sprite`]'s `validate_semantics`); this is the defensive twin
/// for a world assembled in code, which reports rather than panics.
pub(crate) fn atlas_without_frame(pointer: &str) -> Diagnostic {
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
/// use ludion_assets::AssetRoot;
/// use std::path::Path;
///
/// let registry = ludion_engine::builtin_registry();
/// let scene = ludion_scene::check("format: 1\nentities: []\n", &registry).unwrap();
/// let world = ludion_scene::instantiate(&scene, &registry).unwrap();
/// let render_scene = ludion_engine::extract(&world, (1280, 720), AssetRoot::content(Path::new("."))).unwrap();
/// assert!(render_scene.meshes.is_empty());
/// ```
pub fn extract(
    world: &World,
    viewport: (u32, u32),
    project_root: AssetRoot<'_>,
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
/// prefab evaluator poses the parts of an instance whose root carries the `Animation`, the
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
    project_root: AssetRoot<'_>,
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
    project_root: AssetRoot<'_>,
    cache: &mut AssetCache,
    now: SimTime,
    particle_density: f32,
    view: Option<&ViewPose>,
    degraded: &mut Vec<(String, Diagnostic)>,
) -> Result<RenderScene, Vec<Diagnostic>> {
    extract_through_blend(
        world,
        viewport,
        project_root,
        cache,
        now,
        particle_density,
        view,
        degraded,
        None,
        None,
        None,
    )
}

/// [`extract_through`], also recording the poses and per-item keys a display-rate blend needs into
/// `blend` when given. The returned scene is identical either way.
#[allow(clippy::too_many_arguments)]
pub(crate) fn extract_through_blend(
    world: &World,
    viewport: (u32, u32),
    project_root: AssetRoot<'_>,
    cache: &mut AssetCache,
    now: SimTime,
    particle_density: f32,
    view: Option<&ViewPose>,
    degraded: &mut Vec<(String, Diagnostic)>,
    blend: Option<&mut BlendData>,
    projectiles: Option<&mut ProjectileDraw>,
    statics: Option<(&mut StaticScene, Feed)>,
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
    cache.age_failures();
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
        blend,
        projectiles,
        statics,
    )
}

/// The shared body of [`extract`] and [`extract_through`], parameterized by how an asset path is
/// turned into decoded data (through an [`AssetCache`], the host's or a fresh one). `degraded` takes
/// the diagnostics of fallbacks that did not fail the extract, each with its latch key.
#[allow(clippy::too_many_arguments)]
fn extract_inner(
    world: &World,
    viewport: (u32, u32),
    project_root: AssetRoot<'_>,
    assets: &mut dyn SceneAssets,
    now: SimTime,
    particle_density: f32,
    view: Option<&ViewPose>,
    degraded: &mut Vec<(String, Diagnostic)>,
    blend: Option<&mut BlendData>,
    projectiles: Option<&mut ProjectileDraw>,
    statics: Option<(&mut StaticScene, Feed)>,
) -> Result<RenderScene, Vec<Diagnostic>> {
    assets.extract_begin(ExtractPhase::Place);
    // Gameplay time, read once: every closed form below — a volley's bullets, a particle emitter's
    // live window, a clip's phase — measures `now − started`, so reading the world's `GameClock`
    // here is what freezes the picture while a game is held. A world with no clock (a one-shot
    // render, a game that never freezes) reads sim time unchanged.
    let sim_now = now;
    let now = world
        .resource::<GameClock>()
        .map_or_else(|| GameTime::from(now), |clock| clock.game_instant(now));
    let ids: Vec<EntityId> = world
        .entities()
        .chain(world.presentation_entities())
        .collect();
    let mut camera = resolve_camera(world, &ids, viewport, view).map_err(|d| vec![d])?;
    crate::camera_offset::report(world, &ids, degraded);
    let viewmodel_frame = crate::viewmodel::FrameViewmodel::of(world, &ids);
    crate::viewmodel::report(world, &ids, &viewmodel_frame, view.is_some(), degraded);
    let fonts = font_library(world, project_root);
    let locale = fonts
        .has_locale_overrides()
        .then(|| crate::localization::active_locale(world));
    let locale = locale.as_deref();

    // One walk, one sizing computation: `place` produces every renderable's world geometry, and both
    // this function and `render.bounds` consume it — so a sprite's size, a mesh's transform, and a
    // text run's layout can never disagree between what is *drawn* and what `render.bounds` *reports*
    // (the sizing-rule seam; see [`place`]). `atlases` outlives the call because placement and the
    // texture resolution below ask the same atlases for different halves of the same region.
    let mut atlases = AtlasCache::default();
    let mut poses = WorldAffines::new();
    assets.extract_end(ExtractPhase::Place);
    assets.extract_begin(ExtractPhase::Poses);
    let (skin_followers, socket_palettes) = place_poses(world, &ids, assets, &mut poses, now);
    assets.extract_end(ExtractPhase::Poses);
    assets.extract_begin(ExtractPhase::Place);
    // The visibility of this frame, before the placement: a kept static bakes its own.
    let visibility = FrameVisibility::of(world, &ids);
    // The statics are the prefix of the mesh lists; off, or one-shot, this is a fresh scene refreshed
    // in full, which is exactly what the kept one must equal.
    let mut fresh = StaticScene::default();
    let (statics, feed) = statics.unwrap_or((&mut fresh, Feed::Full));
    statics.refresh(
        &mut static_scene::Frame {
            world,
            ids: &ids,
            assets,
            poses: &mut poses,
            visibility: &visibility,
        },
        feed,
        degraded,
    );
    // The GI voxelizer's view of the statics is resolved only for a scene that has a `GiVolume`,
    // and only when the static set changed; its error surfaces at the GI stage below.
    let gi_volume = extract_gi_volume(world, &ids);
    let static_gi = match gi_volume {
        Some(_) => statics.ensure_gi(&ids, assets),
        None => Ok(()),
    };
    let statics = &*statics;
    let mut placed = place_with(
        world,
        &ids,
        &camera,
        &fonts,
        assets,
        &mut atlases,
        &mut poses,
        now,
        skin_followers,
        &|id| statics.held(id),
    );
    drop_non_finite(world, &mut placed, degraded);
    // Hidden entities leave every list that feeds the scene, early, so the blend keys and the pixel
    // snap stay consistent. Meshes stay: the GI voxelizer reads them and ignores `Visibility`.
    placed
        .sprites
        .retain(|sprite| !visibility.hidden(sprite.id));
    placed.texts.retain(|text| !visibility.hidden(text.0));
    placed.lights.retain(|light| !visibility.hidden(light.0));
    placed
        .spot_lights
        .retain(|light| !visibility.hidden(light.0));
    assets.extract_end(ExtractPhase::Place);

    assets.extract_begin(ExtractPhase::Sprites);
    // Resolve sprite textures in first-encounter (entity) order, collecting every load error before
    // giving up. Geometry is already fixed above — only the path→index mapping and its IO live here.
    // `textures` is the output list and the *only* thing iterated; `path_index` is a lookup index
    // over it, so dedup cost does not grow with the number of distinct paths.
    let mut textures: Vec<Arc<TextureImage>> = Vec::new();
    let mut path_index: StableMap<String, usize> = StableMap::new();
    let mut errors: Vec<Diagnostic> = Vec::new();

    // The layer table, `None` for a scene that authors no layers: that frame takes no layer path.
    let mut frame_layers = match crate::render_layers::FrameLayers::of(world, &ids, view.is_some())
    {
        Ok(layers) => layers,
        Err(diagnostic) => {
            errors.push(diagnostic);
            None
        }
    };
    let collect = blend.is_some() || frame_layers.is_some();
    let mut sprites = Vec::with_capacity(placed.sprites.len());
    // The entity behind each sprite, parallel to `sprites` (blend and layer data only).
    let mut sprite_keys: Vec<Option<EntityId>> = Vec::new();
    let mut bullet_keys: Vec<(InstanceKey, u32)> = Vec::new();
    for sprite in &placed.sprites {
        if collect {
            sprite_keys.push(Some(sprite.id));
        }
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
            medium: sprite.medium,
        });
    }

    // Projectile-pattern bullets are sprites like any other, appended after the entity sprites and
    // before every sort and snap below — the renderer breaks a `(layer, depth)` tie by scene order,
    // so where they enter this list is the tie-break they get.
    extract_volleys(
        world,
        &ids,
        &visibility,
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
        collect.then_some(&mut bullet_keys),
    );
    // Volley bullets carry no entity key: they blend by instance key (`bullet_keys`).
    sprite_keys.resize(if collect { sprites.len() } else { 0 }, None);
    // The pixel snap below drops the blend keys; the layer masks still need each sprite's owner.
    let sprite_layer_owners = if frame_layers.is_some() {
        sprite_keys.clone()
    } else {
        Vec::new()
    };

    // `pixel_snap` (extract-only, hash-neutral): when the single orthographic camera opts in and a
    // sprite names an atlas whose `pixels_per_unit` defines a texel grid, snap the view height so one
    // source texel spans a whole number of screen pixels, and snap each sprite centre onto that grid —
    // the crisp-vs-shimmer difference the gallery golden shows. Read here, never in `place`, so
    // `render.bounds` and the world hash never see it.
    // Skipped while a view override is in force: the override is a perspective camera, and the snap
    // would overwrite its projection with the scene camera's orthographic one.
    let mut pixel_snapped = false;
    if view.is_none()
        && let Some((height, near, far)) = orthographic_pixel_snap(world, &ids)
        && let Some(ppu) = pixel_snap_ppu(&placed.sprites, assets, &mut atlases)
    {
        pixel_snapped = true;
        // Blending would undo the texel snap: the sprites and the camera take the newer tick.
        sprite_keys.clear();
        bullet_keys.clear();
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
    assets.extract_end(ExtractPhase::Sprites);

    assets.extract_begin(ExtractPhase::Ui);
    // Resolve UI elements (Phase 14): screen-space, no world placement, so — unlike sprites/meshes/
    // text — they are resolved directly here rather than through `place()`/`Placed`. `render.bounds`
    // reports only world-space renderables (a rect projected from a real camera), and a screen-space
    // element has no such projection to report; keeping UI out of `Placed` keeps that boundary exact
    // by construction. Textures resolve through the same `textures`/`path_index` dedup as sprites
    // above, so a UI rect sharing a sprite's texture path is one `RenderScene.textures` entry.
    let mut ui_entries: Vec<(i32, EntityId, UiInstance)> = Vec::new();
    let mut hud = ui::hud::HudFrame {
        world,
        view: camera.view,
        projection: camera.projection,
        viewport,
        real_frame: sim_now.ticks(),
        game_frame: now.ticks_floor(),
        rate: sim_now.rate(),
        anchor_views: Default::default(),
    };
    let mut groups = ui::group::UiGroups::default();
    let mut view_table = crate::secondary_view::extract_views(
        world,
        &ids,
        frame_layers.as_ref(),
        &mut errors,
        degraded,
    );
    hud.anchor_views = ui::hud::anchor_views(&hud, &view_table, &ids, &mut poses);
    for (index, &id) in ids.iter().enumerate() {
        if let Some(rect) = get::<UiRect>(world, id)
            && rect.visible
            && let shown = crate::secondary_view::resolve_rect_view(
                &view_table,
                rect.view.as_deref(),
                index,
                &mut errors,
            )
            && shown != crate::secondary_view::RectView::Hidden
        {
            let hud_start = ui_entries.len();
            let rect_motion = ui::hud::pre_rect(&hud, id, rect, degraded);
            let rect = &*rect_motion;
            let view = shown.slot();
            let rect_px = ui::hud::rect_box(&hud, &mut poses, &mut groups, rect, degraded);
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
            let region_texels = texture.map(|index| {
                let image = &textures[index];
                [
                    ((uv[2] - uv[0]) * image.width as f32).round(),
                    ((uv[3] - uv[1]) * image.height as f32).round(),
                ]
            });
            ui::hud::emit_rect(
                &hud,
                id,
                rect,
                rect_px,
                texture,
                uv,
                region_texels,
                view,
                &mut ui_entries,
                degraded,
            );
            let element = ui::hud::HudElement::Rect { fill: rect.fill };
            ui::hud::post(
                &hud,
                &mut poses,
                id,
                element,
                rect.anchor,
                &mut ui_entries,
                hud_start,
                ui::hud::Grouping {
                    group: rect.group,
                    fill_group: rect.fill_group,
                    rotation_deg: rect.rotation,
                    groups: &mut groups,
                    degraded: &mut *degraded,
                },
            );
        }
        if let Some(text) = get::<UiText>(world, id)
            && text.visible
        {
            let hud_start = ui_entries.len();
            let text_motion = ui::hud::pre_text(&hud, id, text, degraded);
            let text = &*text_motion;
            let face = fonts.face(text.font.as_deref(), locale);
            let quads = ui::ui_text_draw(face, text, viewport);
            let key = crate::diagnostics::offender_key(world, id);
            let draws = resolve_faces(
                FaceSite {
                    fonts: &fonts,
                    name: text.font.as_deref(),
                    locale,
                    texture_site: TextureSite::Font(index, "UiText"),
                    outline: text.outline.as_ref(),
                    size: text.size,
                    viewport,
                    key: &key,
                },
                &ui::faces_in(&quads),
                project_root,
                assets,
                &mut textures,
                &mut path_index,
                &mut errors,
                degraded,
            );
            let shadow = ui::resolve_shadow(text.shadow.as_ref(), viewport);
            for (quads, font, outline) in ui::text_runs(quads, &draws) {
                let shadow = ui::run_shadow(
                    shadow,
                    font.as_ref(),
                    text.font.as_deref().unwrap_or("embedded"),
                    &key,
                    degraded,
                );
                ui_entries.push((
                    text.order,
                    id,
                    UiInstance::Text {
                        color: text.color,
                        text: text.shown().to_owned(),
                        quads,
                        font,
                        clip: UiClip::NONE,
                        outline,
                        shadow,
                        rotation: UiRotation::NONE,
                    },
                ));
            }
            let element = ui::hud::HudElement::Text;
            ui::hud::post(
                &hud,
                &mut poses,
                id,
                element,
                text.anchor,
                &mut ui_entries,
                hud_start,
                ui::hud::Grouping {
                    group: text.group,
                    fill_group: false,
                    rotation_deg: text.rotation,
                    groups: &mut groups,
                    degraded: &mut *degraded,
                },
            );
        }
        if let Some(list) = get::<UiList>(world, id)
            && list.visible
        {
            let hud_start = ui_entries.len();
            let list_motion = ui::hud::pre_list(&hud, id, list, degraded);
            let list = &*list_motion;
            // An empty list draws nothing at all — panel included — independent of `visible`: the
            // emptiness check gates the background too, not just the lines.
            if !list.items.is_empty() {
                let face = fonts.face(list.font.as_deref(), locale);
                let (layout, lines) = ui::ui_list_draw(face, list, viewport);
                let key = crate::diagnostics::offender_key(world, id);
                let draws = resolve_faces(
                    FaceSite {
                        fonts: &fonts,
                        name: list.font.as_deref(),
                        locale,
                        texture_site: TextureSite::Font(index, "UiList"),
                        outline: list.outline.as_ref(),
                        size: list.size,
                        viewport,
                        key: &key,
                    },
                    &ui::faces_in(lines.iter().flat_map(|(_, quads)| quads)),
                    project_root,
                    assets,
                    &mut textures,
                    &mut path_index,
                    &mut errors,
                    degraded,
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
                            clip: UiClip::NONE,
                            view: None,
                            rotation: UiRotation::NONE,
                        },
                    ));
                }
                // Clamped, never a diagnostic: a game that shrinks a list mid-frame must not be able
                // to author an error.
                let shadow = ui::resolve_shadow(list.shadow.as_ref(), viewport);
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
                    for (quads, font, outline) in ui::text_runs(quads, &draws) {
                        let shadow = ui::run_shadow(
                            shadow,
                            font.as_ref(),
                            list.font.as_deref().unwrap_or("embedded"),
                            &key,
                            degraded,
                        );
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
                                clip: UiClip::NONE,
                                outline,
                                shadow,
                                rotation: UiRotation::NONE,
                            },
                        ));
                    }
                }
            }
            let element = ui::hud::HudElement::List;
            ui::hud::post(
                &hud,
                &mut poses,
                id,
                element,
                list.anchor,
                &mut ui_entries,
                hud_start,
                ui::hud::Grouping {
                    group: list.group,
                    fill_group: false,
                    rotation_deg: 0.0,
                    groups: &mut groups,
                    degraded: &mut *degraded,
                },
            );
        }
        if let Some(radial) = get::<UiRadial>(world, id)
            && radial.visible
        {
            let hud_start = ui_entries.len();
            ui::hud::pre_radial(&hud, id, degraded);
            // An empty wheel draws nothing, as an empty list does.
            if !radial.items.is_empty() {
                let face = fonts.face(radial.font.as_deref(), locale);
                let used = ui::faces_for_text(
                    face,
                    radial
                        .items
                        .iter()
                        .flat_map(|i| std::iter::once(i.text.as_str()).chain(i.detail.as_deref())),
                );
                let draws = resolve_faces(
                    FaceSite {
                        fonts: &fonts,
                        name: radial.font.as_deref(),
                        locale,
                        texture_site: TextureSite::Font(index, "UiRadial"),
                        outline: radial.outline.as_ref(),
                        size: radial.size,
                        viewport,
                        key: &crate::diagnostics::offender_key(world, id),
                    },
                    &used,
                    project_root,
                    assets,
                    &mut textures,
                    &mut path_index,
                    &mut errors,
                    degraded,
                );
                let drawn =
                    ui::radial::radial_draw(face, radial, viewport, &draws, &mut |item, entry| {
                        let atlas = entry.atlas.as_deref()?;
                        resolve_atlas_frame(
                            project_root,
                            atlas,
                            entry.frame.as_deref(),
                            TextureSite::UiRadialItem(index, item),
                            assets,
                            &mut atlases,
                            &mut textures,
                            &mut path_index,
                            &mut errors,
                        )
                    });
                ui_entries.extend(drawn.into_iter().map(|row| (radial.order, id, row)));
            }
            ui::hud::post(
                &hud,
                &mut poses,
                id,
                ui::hud::HudElement::Radial,
                radial.anchor,
                &mut ui_entries,
                hud_start,
                ui::hud::Grouping {
                    group: radial.group,
                    fill_group: false,
                    rotation_deg: 0.0,
                    groups: &mut groups,
                    degraded: &mut *degraded,
                },
            );
        }
        if let Some(view) = get::<SubtitleView>(world, id)
            && view.visible
            && let Some(subtitles) = world.resource::<Subtitles>()
        {
            let face = fonts.face(view.font.as_deref(), locale);
            let drawn = ui::subtitle::subtitle_draw(face, view, subtitles, viewport);
            let draws = resolve_faces(
                FaceSite {
                    fonts: &fonts,
                    name: view.font.as_deref(),
                    locale,
                    texture_site: TextureSite::Font(index, "SubtitleView"),
                    outline: None,
                    size: view.size,
                    viewport,
                    key: &crate::diagnostics::offender_key(world, id),
                },
                &ui::faces_in(drawn.runs.iter().flat_map(|(_, quads)| quads)),
                project_root,
                assets,
                &mut textures,
                &mut path_index,
                &mut errors,
                degraded,
            );
            // One `(order, id)` key and a stable sort: the panel, pushed first, stays behind the text.
            if let Some(rect_px) = drawn.background_px {
                ui_entries.push((
                    view.order,
                    id,
                    UiInstance::Rect {
                        rect_px,
                        texture: None,
                        color: view.background,
                        uv: SpriteInstance::FULL_UV,
                        clip: UiClip::NONE,
                        view: None,
                        rotation: UiRotation::NONE,
                    },
                ));
            }
            for (color, quads) in drawn.runs {
                let text: String = quads.iter().map(|quad| quad.ch).collect();
                for (quads, font, outline) in ui::text_runs(quads, &draws) {
                    ui_entries.push((
                        view.order,
                        id,
                        UiInstance::Text {
                            color,
                            text: text.clone(),
                            quads,
                            font,
                            clip: UiClip::NONE,
                            outline,
                            shadow: None,
                            rotation: UiRotation::NONE,
                        },
                    ));
                }
            }
        }
        if let Some(shape) = get::<UiShape>(world, id)
            && shape.visible
        {
            let hud_start = ui_entries.len();
            let shape = ui::hud::pre_shape(&hud, id, shape, degraded);
            match ui::shape::shape_instances(&shape, viewport) {
                Ok(shape_rows) => {
                    ui_entries.extend(shape_rows.rows.into_iter().map(|row| (shape.order, id, row)));
                    if let Some(count) = shape_rows.dash_overflow {
                        degraded.push((
                            format!("{}/ui-dash", crate::diagnostics::offender_key(world, id)),
                            Diagnostic::new(
                                code::UI_DASH_TOO_DENSE,
                                format!(
                                    "{id} has a `dash` that would draw {count} dashes (the limit is 1024); its stroke draws solid"
                                ),
                            )
                            .with_help(
                                "lengthen `on`/`off`; `ludion explain L2306`",
                            ),
                        ));
                    }
                }
                Err(fault) => degraded.push((
                    format!("{}/ui-polygon", crate::diagnostics::offender_key(world, id)),
                    Diagnostic::new(
                        code::UI_POLYGON_DEGENERATE,
                        format!(
                            "{id} has a `{}` that cannot be drawn: {fault}; it draws nothing",
                            ui::shape::path_kind(&shape.shape)
                        ),
                    )
                    .with_help(
                        "write a simple polygon of 3+ distinct points or a polyline of 2+; `ludion explain L2305`",
                    ),
                )),
            }
            ui::hud::post(
                &hud,
                &mut poses,
                id,
                ui::hud::HudElement::Shape,
                shape.anchor,
                &mut ui_entries,
                hud_start,
                ui::hud::Grouping {
                    group: shape.group,
                    fill_group: false,
                    rotation_deg: shape.rotation,
                    groups: &mut groups,
                    degraded: &mut *degraded,
                },
            );
        }
    }
    // Pinned, documented order: ascending `order`, ties broken by ascending `EntityId` — screen
    // space has no view depth to sort by instead. This is the determinism contract for
    // `RenderScene.ui` (mirrors the extract-determinism guarantee every other field already has).
    ui_entries.sort_by_key(|(order, id, _)| (*order, *id));
    let ui_orders: Vec<i32> = ui_entries.iter().map(|e| e.0).collect();
    let ui: Vec<UiInstance> = ui_entries
        .into_iter()
        .map(|(_, _, instance)| instance)
        .collect();

    // World text resolves its face's atlas here, after the UI, so a face shared by a `UiText` and a
    // `Text` is one `textures` entry.
    let text_keys: Vec<Option<EntityId>> = if collect {
        placed.texts.iter().map(|text| Some(text.0)).collect()
    } else {
        Vec::new()
    };
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
    assets.extract_end(ExtractPhase::Ui);

    assets.extract_begin(ExtractPhase::Meshes);
    // Resolve asset meshes, voxel models, sculpts, and assemblies in the same first-encounter (entity)
    // order, deduplicated by path — so two `Mesh` components pointing at the same document produce one
    // `mesh_assets` entry (one run of entries for an assembly, one per part), and two extractions of
    // one world produce identical `mesh_assets`. All four share this one list (every one becomes a
    // `MeshShape::Asset(index)`); their paths never collide (`meshes/*.mesh.yaml` vs
    // `voxels/*.voxel.yaml` vs `sculpts/*.sculpt.yaml` vs `*.assembly.yaml`). A character resolves
    // to its mesh's path, so it shares that entry with a plain `asset` of the same mesh. Primitives
    // resolve inline in `place`.
    let static_flat = statics.flat();
    let static_count = static_flat.instances.len();
    let mut mesh_assets: Vec<Arc<MeshData>> = static_flat.assets.clone();
    let mut mesh_path_index: StableMap<String, usize> =
        static_flat.asset_paths.iter().cloned().zip(0..).collect();
    // An assembly's parts land in `mesh_assets` as one contiguous run, so one first-encounter entry
    // per assembly path — the index of its first part — is the whole dedup index.
    let mut assembly_part_base: StableMap<String, usize> = StableMap::new();

    let mut meshes = Vec::with_capacity(static_count + placed.meshes.len());
    meshes.extend_from_slice(&static_flat.instances);
    // The owning entity of each `meshes` entry, parallel to it: a drop shadow's caster exclusion.
    let mut mesh_owners: Vec<EntityId> = Vec::with_capacity(static_count + placed.meshes.len());
    mesh_owners.extend_from_slice(&static_flat.owners);
    // Every skinned instance's bone palette, in extract order (filled in the `Skin` block; the
    // offsets are assigned here); the renderer prepends its own identity entry 0, so an instance's
    // `bone_offset` into the combined buffer starts at 1. A joint a `SkinBone` part names takes that
    // part's world pose every frame; every other joint keeps its rest pose relative to its parent
    // (`posed_palette`); an instance with no followers plays its character's `Animation` clip
    // (`sampled_palette`), or pushes its asset's joints' rest entries verbatim when there is none.
    let mut skin_jobs: Vec<SkinJob> = Vec::new();
    let mut skin_joints = 0usize;
    let mut inherit = MeshInherit::default();
    // Blended instances, outside `meshes`; `blend_route` numbers their owning entities consecutively.
    let mut blended: Vec<BlendedMesh> = Vec::new();
    // The owning entity of each `blended` entry, parallel to it (layer masks).
    let mut blended_owners: Vec<EntityId> = Vec::new();
    let mut blend_route = BlendRoute::default();
    for mesh in &placed.meshes {
        // Hidden meshes stay in `placed` for the voxelizer (the static GI bake ignores `Visibility`)
        // but are never resolved, skinned or drawn.
        if visibility.hidden(mesh.id) {
            continue;
        }
        let (fade, rim, medium) = inherit.resolve(world, mesh);
        let cast_shadows = visibility.casts(mesh.id);
        let rain_occluder = crate::rain::rain_occluder(world, mesh.id, cast_shadows);
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
                let placed = assembly_part_models(world, mesh.id, &data, mesh.model, now);
                for (part, (local, local_normal)) in placed.into_iter().enumerate() {
                    let index = base + part;
                    let instance = MeshInstance {
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
                        wrap: mesh.wrap,
                        scatter_color: mesh.scatter_color,
                        bone_offset: 0,
                        cast_shadows,
                        rain_occluder,
                        viewmodel: false,
                        rim: resolve_rim_lane(&mesh_assets, Some(index), rim),
                        medium,
                        porosity: mesh.porosity.unwrap_or(DEFAULT_POROSITY),
                    };
                    let mode = blend_route.mode(
                        world,
                        mesh.id,
                        mesh.blend,
                        mesh_assets[index].blend,
                        mesh.lit,
                        mesh.blend_depth,
                        viewmodel_frame.contains(mesh.id),
                        degraded,
                    );
                    match mode {
                        Some(mode) => {
                            crate::rain::report_blended_occluder(world, mesh.id, degraded);
                            blended.push(blended_mesh(
                                instance,
                                mode,
                                mesh.lit,
                                mesh.blend_depth == BlendDepth::Nearest,
                                blend_route.group(mesh.id),
                            ));
                            blended_owners.push(mesh.id);
                        }
                        None => {
                            mesh_owners.push(mesh.id);
                            meshes.push(instance);
                        }
                    }
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
            let bone_offset = match shape {
                MeshShape::Asset(idx) if !mesh_assets[idx].skin_joints.is_empty() => {
                    // Entry 0 is the renderer's identity, so an asset's own entries start at 1.
                    // The palette itself is posed in the `Skin` block; every path writes exactly
                    // `skin_joints.len()` entries, so the offset is known here.
                    let offset = 1 + skin_joints as u32;
                    skin_joints += mesh_assets[idx].skin_joints.len();
                    skin_jobs.push(SkinJob {
                        skin: mesh.id,
                        asset: idx,
                        character: character.clone(),
                        model: mesh.model,
                        followed: placed.skin_followers.get(&mesh.id).map(Vec::as_slice),
                    });
                    offset
                }
                _ => 0,
            };
            let instance = mesh_instance(
                mesh,
                shape,
                &mesh_assets,
                fade,
                rim,
                medium,
                cast_shadows,
                rain_occluder,
                bone_offset,
            );
            let document_blend = matches!(shape, MeshShape::Asset(idx) if mesh_assets[idx].blend);
            match blend_route.mode(
                world,
                mesh.id,
                mesh.blend,
                document_blend,
                mesh.lit,
                mesh.blend_depth,
                viewmodel_frame.contains(mesh.id),
                degraded,
            ) {
                Some(mode) => {
                    crate::rain::report_blended_occluder(world, mesh.id, degraded);
                    blended.push(blended_mesh(
                        instance,
                        mode,
                        mesh.lit,
                        mesh.blend_depth == BlendDepth::Nearest,
                        blend_route.group(mesh.id),
                    ));
                    blended_owners.push(mesh.id);
                }
                None => {
                    mesh_owners.push(mesh.id);
                    meshes.push(instance);
                }
            }
        }
    }

    // Flagged meshes lose their shadow before decals and drop shadows read `cast_shadows`.
    let viewmodel_camera = if viewmodel_frame.is_empty() {
        None
    } else {
        match crate::viewmodel::site(world, &ids, view.is_some()) {
            crate::viewmodel::Site::Camera(cam) => Some(cam),
            _ => None,
        }
    };
    let viewmodel_emitters = viewmodel_camera.is_some()
        && ids.iter().any(|&id| {
            viewmodel_frame.contains(id)
                && world.has::<ParticleEmitter>(id)
                && !visibility.hidden(id)
        });
    let viewmodel = viewmodel_camera.and_then(|cam| {
        crate::viewmodel::apply(
            world,
            cam,
            &viewmodel_frame,
            &mut meshes,
            &mesh_owners,
            viewport,
            viewmodel_emitters,
        )
    });
    let silhouettes =
        crate::silhouette::extract_silhouettes(world, &meshes, &mesh_owners, degraded);
    assets.extract_end(ExtractPhase::Meshes);

    assets.extract_begin(ExtractPhase::Skin);
    let (bone_palette, skin_counts) =
        pose_skins(world, &skin_jobs, &mesh_assets, now, &socket_palettes);
    debug_assert_eq!(bone_palette.len(), skin_joints);
    assets.extract_skin(skin_counts);
    assets.extract_end(ExtractPhase::Skin);

    assets.extract_begin(ExtractPhase::Decals);
    let (decals, decal_ids) = extract_decals(
        world,
        &ids,
        &visibility,
        &mut poses,
        &meshes,
        &mesh_owners,
        project_root,
        assets,
        &mut errors,
    );
    assets.extract_end(ExtractPhase::Decals);

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

    assets.extract_begin(ExtractPhase::Gi);
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
    // The composed viewmodel is screen-space: its moving pose must not rasterize into the grid or
    // churn the instance signature. Nothing below reads `placed.meshes`.
    if viewmodel_camera.is_some() {
        placed.meshes.retain(|m| !viewmodel_frame.contains(m.id));
    }
    let (voxel, gi) = match gi_volume {
        None => (None, None),
        Some((index, volume)) => {
            static_gi.map_err(|diagnostics| gi_volume_errors(diagnostics, index))?;
            let resolved = statics.gi().expect("ensure_gi succeeded");
            assets.observe_begin(LoadPhase::Voxelize);
            let voxel =
                crate::voxelizer::voxelize(resolved, &placed.meshes, volume.cell_size, assets);
            assets.observe_end(LoadPhase::Voxelize);
            let voxel = voxel
                .map_err(|diagnostics| gi_volume_errors(diagnostics, index))?
                .grid;
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
    assets.extract_end(ExtractPhase::Gi);

    assets.extract_begin(ExtractPhase::Lights);
    let (ground_markers, ground_marker_ids, ground_marker_profiles) = extract_ground_markers(
        world,
        &ids,
        &visibility,
        &mut poses,
        sim_now,
        assets,
        degraded,
    );
    let (drop_shadows, drop_shadow_ids) =
        extract_drop_shadows(world, &ids, &visibility, &mut poses, &mesh_owners);
    let (light_ids, spot_light_ids): (Vec<EntityId>, Vec<EntityId>) = if collect {
        (
            placed.lights.iter().map(|(id, _)| *id).collect(),
            placed.spot_lights.iter().map(|(id, _)| *id).collect(),
        )
    } else {
        (Vec::new(), Vec::new())
    };
    assets.extract_end(ExtractPhase::Lights);

    let mut particle_keys: Vec<InstanceKey> = Vec::new();
    assets.extract_begin(ExtractPhase::Particles);
    let particles = extract_particles(
        world,
        &ids,
        &visibility,
        &mut poses,
        now,
        particle_density,
        CameraBasis::from_view(camera.view),
        viewmodel_camera.map(|cam| (&viewmodel_frame, crate::viewmodel::camera_frame(world, cam))),
        collect.then_some(&mut particle_keys),
    );
    assets.extract_end(ExtractPhase::Particles);
    assets.extract_begin(ExtractPhase::Lights);
    let lights = extract_lights(placed.lights);
    let spot_lights = extract_spot_lights(placed.spot_lights);
    let probes = extract_probes(world, &ids, &mut poses);
    let sun = extract_sun(world, &ids);
    let post = extract_post(world, &ids, now);
    let lut = extract_lut(world, &ids, project_root, assets)?;
    crate::secondary_view::resolve_view_posts(
        &mut view_table,
        world,
        &ids,
        now,
        project_root,
        assets,
    )?;
    assets.extract_end(ExtractPhase::Lights);
    let screen_effects = crate::screen_effect::extract_screen_effects(
        world,
        &ids,
        now,
        viewport,
        &ui_orders,
        &view_table,
        degraded,
    );
    let view_entities: Vec<(EntityId, (u32, u32))> = view_table
        .entities
        .iter()
        .zip(&view_table.views)
        .map(|(&entity, view)| (entity, view.size))
        .collect();
    let mut views = view_table.views;
    for (view, effects) in views.iter_mut().zip(screen_effects.per_view) {
        view.screen_effects = effects;
    }
    let mut scene = RenderScene {
        prewarm: Vec::new(),
        camera,
        cull_camera: None,
        lod: scene_lod(world, &ids),
        ambient,
        background: None,
        unlit: false,
        textures,
        mesh_assets,
        statics: static_flat.range,
        meshes,
        blended,
        sprites,
        ground_markers,
        ground_marker_profiles,
        silhouettes,
        drop_shadows,
        decals,
        debug_lines: Vec::new(),
        debug_view: None,
        particles,
        lights,
        spot_lights,
        probes,
        probe_world: None,
        sun,
        environment,
        post,
        lut,
        texts,
        ui,
        voxel,
        gi,
        bone_palette,
        viewmodel,
        views,
        rain: crate::rain::extract_rain(world, &ids, now, degraded),
        layered: None,
        screen_effects: screen_effects.frame,
    };
    if let Some(data) = scene.viewmodel.as_mut() {
        data.particles = scene
            .particles
            .iter()
            .enumerate()
            .filter(|(_, p)| p.viewmodel)
            .map(|(index, p)| ludion_renderer::ViewmodelParticle {
                particle: index as u32,
                center: p.center,
                right: p.right,
                up: p.up,
            })
            .collect();
    }
    // Appended last among the world's lists so decals and drop shadows never land on a shot and GI
    // never sees one; `None` draws with the default table.
    assets.extract_begin(ExtractPhase::Projectiles);
    let projectile_keys = match projectiles {
        Some(draw) => {
            let muzzle = muzzle_point(
                world,
                &ids,
                &mut poses,
                &viewmodel_frame,
                viewmodel_camera,
                &scene,
                draw,
                degraded,
            );
            crate::projectile_draw::extract_projectiles(
                &mut scene,
                world,
                draw,
                muzzle,
                assets,
                &mut mesh_path_index,
                degraded,
            )
        }
        None => crate::projectile_draw::extract_projectiles(
            &mut scene,
            world,
            &mut ProjectileDraw::default(),
            None,
            assets,
            &mut mesh_path_index,
            degraded,
        ),
    };
    assets.extract_end(ExtractPhase::Projectiles);
    if let (Some(cam), Some(_)) = (viewmodel_camera, &scene.viewmodel) {
        ludion_renderer::compose_viewmodel(&mut scene, drawn_camera_model(world, &ids, cam));
    }
    let mut keys = blend.is_some().then(|| BlendKeys {
        meshes: mesh_owners
            .iter()
            .zip(&scene.meshes)
            .enumerate()
            // The kept statics are fixed: keyed to nothing, so the pose table never holds them.
            .map(|(i, (&owner, mesh))| (i >= static_count && !mesh.viewmodel).then_some(owner))
            // Appended projectile meshes have no owner entity: unkeyed, but present, so the
            // list stays parallel to `scene.meshes`.
            .chain(
                projectile_keys
                    .iter()
                    .filter(|(_, item)| {
                        matches!(item, ProjectileItem::Mesh(_) | ProjectileItem::Posed(_))
                    })
                    .map(|_| None),
            )
            .collect(),
        projectiles: projectile_keys,
        sprites: sprite_keys,
        lights: light_ids.clone(),
        spot_lights: spot_light_ids.clone(),
        texts: text_keys.clone(),
        ground_markers: ground_marker_ids.clone(),
        drop_shadows: drop_shadow_ids.clone(),
        decals: decal_ids.clone(),
        particles: particle_keys.clone(),
        bullets: bullet_keys,
    });
    // A layered frame keeps in the main scene only what the main camera draws; the full scene rides on
    // it for the views. The blend keys follow the filtered lists, so they stay parallel to them.
    let mut full_keys = None;
    if let Some(layers) = frame_layers.as_mut() {
        let masks = layers.masks(
            world,
            &scene,
            &crate::render_layers::Owners {
                meshes: &mesh_owners,
                blended: &blended_owners,
                sprites: &sprite_layer_owners,
                ground_markers: &ground_marker_ids,
                drop_shadows: &drop_shadow_ids,
                decals: &decal_ids,
                particles: &particle_keys,
                texts: &text_keys,
                lights: &light_ids,
                spot_lights: &spot_light_ids,
            },
        );
        let full = std::mem::replace(&mut scene, RenderScene::empty(viewport));
        let (main, remap) = ludion_renderer::split_layers(full, masks);
        scene = main;
        full_keys = keys.clone();
        if let Some(keys) = keys.as_mut() {
            keys.retain(&remap);
        }
    }
    if let (Some(blend), Some(keys)) = (blend, keys) {
        let camera_key = if view.is_none() && !pixel_snapped {
            single_camera(world, &ids).ok().flatten()
        } else {
            None
        };
        fill_blend(
            blend,
            world,
            &mut poses,
            &ids,
            viewport,
            camera_key,
            (keys, full_keys),
            &view_entities,
        );
    }
    Ok(scene)
}

/// The entities behind each keyed `RenderScene` list, as extract collected them.
#[derive(Clone)]
struct BlendKeys {
    meshes: Vec<Option<EntityId>>,
    sprites: Vec<Option<EntityId>>,
    lights: Vec<EntityId>,
    spot_lights: Vec<EntityId>,
    texts: Vec<Option<EntityId>>,
    ground_markers: Vec<EntityId>,
    drop_shadows: Vec<EntityId>,
    decals: Vec<EntityId>,
    particles: Vec<InstanceKey>,
    bullets: Vec<(InstanceKey, u32)>,
    projectiles: Vec<(u64, ProjectileItem)>,
}

impl BlendKeys {
    /// Follows a layer filter: drops the keys of removed items and renumbers the indices the bullet and
    /// projectile keys hold. A key list shorter than its scene list (appended items have none) is a
    /// prefix of it, so it filters by its own indices.
    fn retain(&mut self, remap: &ludion_renderer::Remap) {
        fn keep<T>(keys: &mut Vec<T>, remap: &[Option<u32>]) {
            let mut index = 0;
            keys.retain(|_| {
                let kept = remap.get(index).is_none_or(Option::is_some);
                index += 1;
                kept
            });
        }
        let at = |map: &[Option<u32>], index: u32| map.get(index as usize).copied().flatten();
        keep(&mut self.meshes, &remap.meshes);
        keep(&mut self.lights, &remap.lights);
        keep(&mut self.spot_lights, &remap.spot_lights);
        keep(&mut self.sprites, &remap.sprites);
        keep(&mut self.texts, &remap.texts);
        keep(&mut self.ground_markers, &remap.ground_markers);
        keep(&mut self.drop_shadows, &remap.drop_shadows);
        keep(&mut self.decals, &remap.decals);
        keep(&mut self.particles, &remap.particles);
        self.bullets = std::mem::take(&mut self.bullets)
            .into_iter()
            .filter_map(|(key, sprite)| Some((key, at(&remap.sprites, sprite)?)))
            .collect();
        self.projectiles = std::mem::take(&mut self.projectiles)
            .into_iter()
            .filter_map(|(id, item)| {
                let item = match item {
                    ProjectileItem::Mesh(i) => ProjectileItem::Mesh(at(&remap.meshes, i)?),
                    ProjectileItem::Posed(i) => ProjectileItem::Posed(at(&remap.meshes, i)?),
                    ProjectileItem::Particle(i) => {
                        ProjectileItem::Particle(at(&remap.particles, i)?)
                    }
                };
                Some((id, item))
            })
            .collect();
    }
}

/// The entity whose blend delta moves `id`: the outermost parent across a resolved `ParentJoint`
/// on the way up, else `id` itself. Reads through the unrecorded hierarchy accessors (`parent_of`,
/// `joint_rest`), so it records no access.
fn socket_key(world: &World, id: EntityId) -> EntityId {
    let mut key = id;
    let mut node = id;
    for _ in 0..=crate::hierarchy::MAX_HIERARCHY_DEPTH {
        let Some(ludion_ecs::Parent(parent)) = crate::hierarchy::parent_of(world, node) else {
            break;
        };
        if !world.is_alive(parent) {
            break;
        }
        if crate::parent_joint::joint_rest(world, node).is_some() {
            key = parent;
        }
        node = parent;
    }
    key
}

/// Writes `keys` into the per-list key vectors of `blend` and returns the owners of its particle and
/// bullet instances. A socket child and its descendants take their skin's blend delta, never their
/// own: the presenter then moves `skin_new × J × L` by the skin's delta, exactly as it moves the skin.
fn fill_keys(blend: &mut BlendData, world: &World, keys: BlendKeys) -> BTreeSet<EntityId> {
    let some = |ids: Vec<EntityId>| -> Vec<Option<EntityId>> {
        ids.into_iter()
            .map(|id| Some(socket_key(world, id)))
            .collect()
    };
    let optional = |ids: Vec<Option<EntityId>>| -> Vec<Option<EntityId>> {
        ids.into_iter()
            .map(|id| id.map(|id| socket_key(world, id)))
            .collect()
    };
    blend.meshes = optional(keys.meshes);
    blend.sprites = optional(keys.sprites);
    blend.lights = some(keys.lights);
    blend.spot_lights = some(keys.spot_lights);
    blend.texts = optional(keys.texts);
    blend.ground_markers = some(keys.ground_markers);
    blend.drop_shadows = some(keys.drop_shadows);
    blend.decals = some(keys.decals);
    let instance_owners: BTreeSet<EntityId> = keys
        .particles
        .iter()
        .chain(keys.bullets.iter().map(|(key, _)| key))
        .map(|key| key.owner)
        .collect();
    blend.particles = keys.particles;
    blend.bullets = keys.bullets;
    blend.projectiles = keys.projectiles;
    instance_owners
}

/// The entities that drive the keyed lists of `blend`.
fn keyed_entities(blend: &BlendData) -> impl Iterator<Item = EntityId> + '_ {
    [
        &blend.meshes,
        &blend.sprites,
        &blend.lights,
        &blend.spot_lights,
        &blend.texts,
        &blend.ground_markers,
        &blend.drop_shadows,
        &blend.decals,
    ]
    .into_iter()
    .flat_map(|list| list.iter().flatten().copied())
}

/// Fills `blend` from the keys extract collected: the pose table over every unique key, the snap
/// set, the camera and each view's camera. `keys` is the main lists' and, for a layered frame, the
/// unfiltered source's; `views` is each rendered view's entity and size. Reads the same
/// `render_pose` the extract drew with.
#[allow(clippy::too_many_arguments)]
fn fill_blend(
    blend: &mut BlendData,
    world: &World,
    poses: &mut WorldAffines,
    ids: &[EntityId],
    viewport: (u32, u32),
    camera_key: Option<EntityId>,
    (keys, full): (BlendKeys, Option<BlendKeys>),
    views: &[(EntityId, (u32, u32))],
) {
    let mut instance_owners = fill_keys(blend, world, keys);
    if let Some(full) = full {
        let mut source = BlendData::default();
        instance_owners.extend(fill_keys(&mut source, world, full));
        blend.layered = Some(Box::new(source));
    }
    let unique: BTreeSet<EntityId> = keyed_entities(blend)
        .chain(blend.layered.iter().flat_map(|l| keyed_entities(l)))
        .collect();
    // Clip poses keep the newer tick (display-paced-present decision 15), so the pose table holds
    // stored transforms, not the clip-posed ones in `poses`.
    let mut stored = WorldAffines::new();
    blend.poses = unique
        .iter()
        .map(|&id| (id, render_pose(world, &mut stored, id).model()))
        .collect();
    blend.snaps = crate::interpolation::snap_set(
        world,
        unique
            .iter()
            .copied()
            .chain(instance_owners)
            .chain(camera_key)
            .chain(views.iter().map(|&(entity, _)| entity)),
    );
    let offset = camera_key.and_then(|_| crate::camera_offset::active_entity(world, ids));
    // A `NoInterpolate` on the offset entity cuts the camera for the tick, as one on the camera does.
    if let (Some(key), Some((entity, _))) = (camera_key, offset)
        && !crate::interpolation::snap_set(world, std::iter::once(entity)).is_empty()
        && let Err(at) = blend.snaps.binary_search(&key)
    {
        blend.snaps.insert(at, key);
    }
    blend.camera = camera_key.map(|key| {
        let cam = get::<Camera>(world, key).cloned().unwrap_or_default();
        let (near, far) = cam.depth_range();
        let pose = render_pose(world, poses, key);
        let (rotation, translation, projection) = compose_camera(
            pose.quat(),
            Vec3::from(pose.translation()),
            &cam,
            offset.as_ref().map(|(_, offset)| offset),
        );
        CameraBlend {
            key,
            model: Mat4::from_rotation_translation(rotation, translation),
            fov_y_deg: match projection {
                Projection::Perspective { fov_y_deg } => Some(fov_y_deg),
                Projection::Orthographic { .. } => None,
            },
            near,
            far,
            aspect: aspect_ratio(viewport),
        }
    });
    // A view's camera is drawn without the presentation offset, and keyed even when the main camera
    // is not (the editor override and pixel snapping concern the main camera only).
    blend.views = views
        .iter()
        .map(|&(key, size)| {
            let cam = get::<Camera>(world, key).cloned().unwrap_or_default();
            let (near, far) = cam.depth_range();
            let pose = render_pose(world, poses, key);
            let (rotation, translation, projection) =
                compose_camera(pose.quat(), Vec3::from(pose.translation()), &cam, None);
            Some(CameraBlend {
                key,
                model: Mat4::from_rotation_translation(rotation, translation),
                fov_y_deg: match projection {
                    Projection::Perspective { fov_y_deg } => Some(fov_y_deg),
                    Projection::Orthographic { .. } => None,
                },
                near,
                far,
                aspect: aspect_ratio(size),
            })
        })
        .collect();
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

/// Why the scene's `GiVolume` may resolve to no grid: the volume's entity index and what the
/// voxelizer would skip, or `None` when the scene authors no `GiVolume`. It re-walks placement over a
/// fresh asset cache, so it is for error paths only (the voxel-sheet dumps), never a frame.
///
/// # Errors
/// As [`crate::voxelizer::voxelize`]: a mesh document that will not load.
pub(crate) fn gi_volume_skips(
    world: &World,
    camera: &RenderCamera,
    project_root: AssetRoot<'_>,
    now: SimTime,
) -> Result<Option<(usize, crate::voxelizer::SkipReport)>, Vec<Diagnostic>> {
    let ids: Vec<EntityId> = world
        .entities()
        .chain(world.presentation_entities())
        .collect();
    let Some((index, _)) = extract_gi_volume(world, &ids) else {
        return Ok(None);
    };
    let placed = place_uncached(world, &ids, camera, project_root, now);
    let mut cache = AssetCache::new();
    let mut assets = CachedAssets {
        root: project_root,
        cache: &mut cache,
    };
    let skipped = crate::voxelizer::skip_report(&placed.meshes, &mut assets)?;
    Ok(Some((index, skipped)))
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
fn extract_post(world: &World, ids: &[EntityId], now: GameTime) -> Option<PostData> {
    main_post(world, ids).map(|(_, post)| post_data(post, now))
}

/// The `PostProcess` that images the main frame and its entity index: the lowest-`EntityId` one
/// without a `PostProfile` (a profile is a view's named look, never the scene's).
fn main_post<'w>(world: &'w World, ids: &[EntityId]) -> Option<(usize, &'w PostProcess)> {
    ids.iter().enumerate().find_map(|(index, &id)| {
        get::<PostProcess>(world, id)
            .filter(|_| !world.has::<PostProfile>(id))
            .map(|post| (index, post))
    })
}

/// One `PostProcess` as the renderer's baked imaging.
pub(crate) fn post_data(post: &PostProcess, now: GameTime) -> PostData {
    PostData {
        bloom_threshold: post.bloom_threshold,
        bloom_strength: post.bloom_strength,
        exposure: post.exposure,
        tonemap: match post.tonemap {
            TonemapOperator::Agx => ludion_renderer::TonemapOperator::Agx,
            TonemapOperator::Aces => ludion_renderer::TonemapOperator::Aces,
            TonemapOperator::Neutral => ludion_renderer::TonemapOperator::Neutral,
            TonemapOperator::None => ludion_renderer::TonemapOperator::None,
        },
        exposure_compensation: post.exposure_compensation,
        meter: (post.exposure_mode == ExposureMode::Metered).then_some(
            ludion_renderer::MeterData {
                weighting: match post.metering {
                    Metering::Average => ludion_renderer::MeterWeighting::Average,
                    Metering::Center => ludion_renderer::MeterWeighting::Center,
                },
                clip: post.metering_clip,
                range: post.exposure_range,
            },
        ),
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
        halation_color: post.halation_color,
        anamorphic: post.anamorphic,
        anamorphic_length: post.anamorphic_length,
        anamorphic_tint: post.anamorphic_tint,
        anamorphic_threshold: post.anamorphic_threshold,
        grain: post.grain,
        grain_size: post.grain_size,
        grain_tick: (post.grain_motion == GrainMotion::Animated).then(|| now.ticks_floor() as u32),
        ssr: post.ssr,
        ssr_fade: post.ssr_fade,
        ssr_max_distance: post.ssr_max_distance,
        ssr_thickness: post.ssr_thickness,
    }
}

/// The `.cube` LUT of the lowest-`EntityId` `PostProcess`, resolved through `assets`. A load failure
/// is reported at `/entities/{index}/components/PostProcess/lut/path`.
fn extract_lut(
    world: &World,
    ids: &[EntityId],
    project_root: AssetRoot<'_>,
    assets: &mut dyn SceneAssets,
) -> Result<Option<ludion_renderer::LutData>, Vec<Diagnostic>> {
    let Some((index, post)) = main_post(world, ids) else {
        return Ok(None);
    };
    lut_for(index, post, project_root, assets)
}

/// The `.cube` LUT of `post`, the `PostProcess` of entity `index`; a load failure is reported at its
/// `lut/path`.
pub(crate) fn lut_for(
    index: usize,
    post: &PostProcess,
    project_root: AssetRoot<'_>,
    assets: &mut dyn SceneAssets,
) -> Result<Option<ludion_renderer::LutData>, Vec<Diagnostic>> {
    let Some(lut) = &post.lut else {
        return Ok(None);
    };
    let rel = ludion_core::paths::normalize(&lut.path);
    match assets.lut(&rel) {
        Ok(table) => Ok(Some(ludion_renderer::LutData {
            lut: table,
            strength: lut.strength,
        })),
        Err(diagnostics) => Err(diagnostics
            .into_iter()
            .map(|base| {
                let diagnostic =
                    base.with_path(format!("/entities/{index}/components/PostProcess/lut/path"));
                match suggest_asset(project_root, &rel) {
                    Some(name) => diagnostic.append_help(format!("nearest match is `{name}`")),
                    None => diagnostic,
                }
            })
            .collect()),
    }
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
    visibility: &FrameVisibility,
    poses: &mut WorldAffines,
    now: SimTime,
    assets: &mut dyn SceneAssets,
    degraded: &mut Vec<(String, Diagnostic)>,
) -> (Vec<GroundMarkerInstance>, Vec<EntityId>, Vec<f32>) {
    let mut pass = crate::ground_marker_clip::ClipPass::new(world, assets.clip_cache());
    let (instances, markers): (Vec<_>, Vec<_>) = ids
        .iter()
        .filter(|&&id| !visibility.hidden(id))
        .filter_map(|&id| {
            let marker = get::<GroundMarker>(world, id)?;
            let pose = render_pose(world, poses, id);
            let shape = scaled_shape(marker.shape, pose.scale()[0]);
            let clip = get::<crate::components::GroundMarkerClip>(world, id).and_then(|clip| {
                pass.clip(
                    id,
                    marker.shape,
                    clip,
                    (pose.translation(), pose.quat(), pose.scale()[0]),
                    degraded,
                )
            });
            let translation = Vec3::from(pose.translation()) + Vec3::Y * GROUND_MARKER_LIFT;
            let style = marker.style.map_or_else(Default::default, |s| {
                ludion_renderer::GroundMarkerStyle {
                    outline_px: s.outline_px,
                    grid: s.grid == GroundMarkerGrid::Polar,
                    grid_px: s.grid_px,
                    grid_alpha: s.grid_alpha,
                    interior_alpha: s.interior_alpha,
                    inner_alpha: s.inner_alpha,
                    fill_alpha: s.fill_alpha,
                    flicker: s.flicker,
                }
            });
            Some((
                GroundMarkerInstance {
                    shape,
                    model: Mat4::from_rotation_translation(pose.quat(), translation),
                    color: marker.color,
                    inner_color: marker.inner_color.unwrap_or(marker.color),
                    style,
                    flicker_seed: flicker_seed(id, now),
                    clip,
                },
                id,
            ))
        })
        .unzip();
    (instances, markers, pass.finish())
}

/// The flicker dropout's per-(entity, sim tick) seed: a fixed integer mix of the entity slot,
/// generation and tick, masked to 24 bits so it rides a float lane exactly. Wrapping ops only, so
/// it is the same on every platform; render-only, never hashed.
fn flicker_seed(id: EntityId, now: SimTime) -> u32 {
    let ticks = now.ticks();
    let mut h = (ticks as u32) ^ ((ticks >> 32) as u32).rotate_left(16);
    h = h.wrapping_mul(0x9E37_79B1)
        ^ id.index().wrapping_mul(0x85EB_CA6B)
        ^ id.generation().wrapping_mul(0xC2B2_AE35);
    h ^= h >> 15;
    h = h.wrapping_mul(0x2C1B_3C6D);
    h ^= h >> 12;
    h & 0xFF_FFFF
}

/// A component shape in final world units: every length times `scale` (a wedge's angle and fill are
/// not lengths and stay unscaled; its `fill` is clamped to `[0, 1]`). A negative scale has no
/// meaningful footprint, so it clamps to `0.0` (an invisible shape) rather than mirroring geometry.
fn scaled_shape(shape: GroundMarkerShape, scale: f32) -> ludion_renderer::GroundMarkerShape {
    let scale = scale.max(0.0);
    match shape {
        GroundMarkerShape::Disc { radius } => ludion_renderer::GroundMarkerShape::Disc {
            radius: radius * scale,
        },
        GroundMarkerShape::Segment { length, width } => {
            ludion_renderer::GroundMarkerShape::Segment {
                length: length * scale,
                width: width * scale,
            }
        }
        GroundMarkerShape::Wedge {
            half_angle_deg,
            outer_radius,
            inner_radius,
            fill,
            grid_spacing,
            inner_half_angle_deg,
        } => {
            let half_angle = half_angle_deg.to_radians();
            ludion_renderer::GroundMarkerShape::Wedge {
                half_angle,
                inner_radius: inner_radius * scale,
                outer_radius: outer_radius * scale,
                fill: fill.clamp(0.0, 1.0),
                grid_spacing: grid_spacing * scale,
                inner_half_angle: inner_half_angle_deg.map(|a| a.to_radians().min(half_angle)),
            }
        }
        GroundMarkerShape::Ring {
            radius,
            thickness,
            fill,
            dashes,
        } => ludion_renderer::GroundMarkerShape::Ring {
            radius: radius * scale,
            thickness: thickness * scale,
            fill: fill.clamp(0.0, 1.0),
            dashes,
        },
    }
}

/// Every [`DropShadow`] in the world, resolved into the renderer's [`DropShadowInstance`] in the
/// ascending-`EntityId` walk order `ids` carries.
///
/// Baked here so the renderer interprets nothing: dimensions are final world units (times the
/// uniform `Transform.scale.x`), the anchor is the entity's world position, and `yaw` is the
/// rotation about world +Y alone (a pitch or roll never tilts the footprint). `exclude` lists the
/// `meshes` indices owned by the caster set — the entity's hierarchy root and all that root's
/// descendants — so the shadow never lands on the thing casting it. `mesh_owners` is parallel to
/// the extracted `meshes`, which makes the list ascending by construction.
///
/// Like ground markers, drop shadows do not travel through [`place`] or `render.bounds`. The
/// hierarchy's child map is built only when a shadow exists.
fn extract_drop_shadows(
    world: &World,
    ids: &[EntityId],
    visibility: &FrameVisibility,
    poses: &mut WorldAffines,
    mesh_owners: &[EntityId],
) -> (Vec<DropShadowInstance>, Vec<EntityId>) {
    // Built on the first shadow: the child map, and each owner's mesh indices (ascending).
    let mut tree = None;
    // Shadows under one root share its caster set, so its exclusion list is built once.
    let mut by_root: BTreeMap<EntityId, Vec<u32>> = BTreeMap::new();
    ids.iter()
        .filter(|&&id| !visibility.hidden(id))
        .filter_map(|&id| {
            let shadow = get::<DropShadow>(world, id)?;
            let pose = render_pose(world, poses, id);
            let root = crate::hierarchy::hierarchy_root(world, id);
            let exclude = by_root
                .entry(root)
                .or_insert_with(|| {
                    let (children, by_owner) = tree.get_or_insert_with(|| {
                        let mut by_owner: BTreeMap<EntityId, Vec<u32>> = BTreeMap::new();
                        for (index, &owner) in mesh_owners.iter().enumerate() {
                            by_owner.entry(owner).or_default().push(index as u32);
                        }
                        (ludion_ecs::ChildMap::build(world), by_owner)
                    });
                    let mut casters: BTreeSet<EntityId> =
                        children.descendants(root).into_iter().collect();
                    casters.insert(root);
                    let mut exclude: Vec<u32> = casters
                        .iter()
                        .filter_map(|caster| by_owner.get(caster))
                        .flatten()
                        .copied()
                        .collect();
                    exclude.sort_unstable();
                    exclude
                })
                .clone();
            let along_x = pose.quat() * Vec3::X;
            Some((
                DropShadowInstance {
                    shape: scaled_shape(shadow.shape, pose.scale()[0]),
                    anchor: Vec3::from(pose.translation()),
                    yaw: ludion_core::math::atan2(-along_x.z, along_x.x),
                    opacity: shadow.opacity,
                    softness: shadow.softness,
                    max_distance: shadow.max_distance,
                    fade_distance: shadow.fade_distance,
                    exclude,
                },
                id,
            ))
        })
        .unzip()
}

/// Every [`Decal`] in the world, resolved into the renderer's [`DecalInstance`] in draw order:
/// ascending `layer`, then descending `EntityId`, so the lowest id draws last and wins.
///
/// The box matrices come from the entity's composed world pose (the `Parent` chain and scale) times
/// the authored `size`; a decal whose box is not invertible (a zero scale while animating out) covers
/// nothing and is skipped. Receivers are indices into `meshes`, which `mesh_owners` parallels: `all`
/// is `Unskinned` (the receiver pass marks skinned pixels), `hierarchy` admits only the unskinned
/// meshes owned by the decal's hierarchy root and its descendants. The hierarchy's child map is
/// built only when a `hierarchy` decal exists.
///
/// Maps resolve through `assets`, so they share the asset cache's entries (and its cooked role) with
/// a mesh's maps. A map that fails to load pushes its diagnostic onto `errors`, pointed at the
/// decal's map path, and leaves its slot empty.
#[allow(clippy::too_many_arguments)]
#[allow(clippy::disallowed_methods)] // render-side: decal box matrices, never reaches a hash
fn extract_decals(
    world: &World,
    ids: &[EntityId],
    visibility: &FrameVisibility,
    poses: &mut WorldAffines,
    meshes: &[MeshInstance],
    mesh_owners: &[EntityId],
    project_root: AssetRoot<'_>,
    assets: &mut dyn SceneAssets,
    errors: &mut Vec<Diagnostic>,
) -> (Vec<DecalInstance>, Vec<EntityId>) {
    let skinned = |index: usize| meshes.get(index).is_some_and(|mesh| mesh.bone_offset != 0);
    let mut children = None;
    let mut decals: Vec<(i16, EntityId, DecalInstance)> = Vec::new();
    for (index, &id) in ids.iter().enumerate() {
        if visibility.hidden(id) {
            continue;
        }
        let Some(decal) = get::<Decal>(world, id) else {
            continue;
        };
        let box_to_world =
            render_pose(world, poses, id).model() * Mat4::from_scale(Vec3::from(decal.size));
        let determinant = box_to_world.determinant();
        if !determinant.is_finite() || determinant.abs() < f32::MIN_POSITIVE {
            continue;
        }
        let receivers = match decal.receivers {
            DecalReceiverSet::All => DecalReceivers::Unskinned,
            DecalReceiverSet::Hierarchy => {
                let root = crate::hierarchy::hierarchy_root(world, id);
                let children = children.get_or_insert_with(|| ludion_ecs::ChildMap::build(world));
                let mut owners: BTreeSet<EntityId> =
                    children.descendants(root).into_iter().collect();
                owners.insert(root);
                DecalReceivers::Only(
                    mesh_owners
                        .iter()
                        .enumerate()
                        .filter(|&(index, owner)| owners.contains(owner) && !skinned(index))
                        .map(|(index, _)| index as u32)
                        .collect(),
                )
            }
        };
        let instance = DecalInstance {
            box_to_world,
            world_to_box: box_to_world.inverse(),
            blend: match decal.blend {
                DecalBlendAuthored::Alpha => DecalBlend::Alpha,
                DecalBlendAuthored::Multiply => DecalBlend::Multiply,
                DecalBlendAuthored::Add => DecalBlend::Add,
                DecalBlendAuthored::Coat => DecalBlend::Coat,
            },
            color: decal.color,
            opacity: decal.opacity,
            roughness: decal.roughness,
            metallic: decal.metallic,
            ripples: decal.ripples,
            uv_rect: decal.uv_rect,
            angle_fade: decal.angle_fade,
            layer: decal.layer,
            maps: DecalMaps {
                base_color: decal_map(
                    project_root,
                    assets,
                    errors,
                    index,
                    "base_color_texture",
                    decal.base_color_texture.as_ref(),
                ),
                normal: decal_map(
                    project_root,
                    assets,
                    errors,
                    index,
                    "normal_texture",
                    decal.normal_texture.as_ref(),
                ),
                metallic_roughness: decal_map(
                    project_root,
                    assets,
                    errors,
                    index,
                    "metallic_roughness_texture",
                    decal.metallic_roughness_texture.as_ref(),
                ),
                normal_scale: decal.normal_texture.as_ref().map_or(1.0, |map| map.scale),
            },
            receivers,
        };
        decals.push((decal.layer, id, instance));
    }
    decals.sort_by_key(|(layer, id, _)| (*layer, std::cmp::Reverse(*id)));
    decals
        .into_iter()
        .map(|(_, id, instance)| (instance, id))
        .unzip()
}

/// Resolves one of a decal's maps, or `None` when the decal authors none or the load failed (the
/// diagnostic is pushed onto `errors` at `/entities/{index}/components/Decal/{field}/path`).
fn decal_map(
    project_root: AssetRoot<'_>,
    assets: &mut dyn SceneAssets,
    errors: &mut Vec<Diagnostic>,
    index: usize,
    field: &str,
    map: Option<&DecalMap>,
) -> Option<MeshTexture> {
    let map = map?;
    let rel = ludion_core::paths::normalize(&map.path);
    match assets.map(&rel) {
        Ok(image) => Some(MeshTexture {
            image,
            wrap: match map.wrap {
                MapWrap::Repeat => TextureWrap::Repeat,
                MapWrap::Clamp => TextureWrap::Clamp,
                MapWrap::Mirror => TextureWrap::Mirror,
            },
        }),
        Err(base) => {
            let mut diagnostic =
                base.with_path(format!("/entities/{index}/components/Decal/{field}/path"));
            if let Some(name) = suggest_asset(project_root, &rel) {
                diagnostic = diagnostic.append_help(format!("nearest match is `{name}`"));
            }
            errors.push(diagnostic);
            None
        }
    }
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
#[allow(clippy::too_many_arguments)]
fn extract_particles(
    world: &World,
    ids: &[EntityId],
    visibility: &FrameVisibility,
    poses: &mut WorldAffines,
    now: GameTime,
    particle_density: f32,
    basis: CameraBasis,
    viewmodel: Option<(&crate::viewmodel::FrameViewmodel, Mat4)>,
    keys: Option<&mut Vec<InstanceKey>>,
) -> Vec<ParticleInstance> {
    let camera_inverse = viewmodel.map(|(_, model)| crate::viewmodel::camera_inverse(model));
    let local_basis = CameraBasis::from_view(Mat4::IDENTITY);
    let mut emitters = Vec::new();
    let mut live = 0;
    for &id in ids {
        if visibility.hidden(id) {
            continue;
        }
        let Some(emitter) = get::<ParticleEmitter>(world, id) else {
            continue;
        };
        let pose = render_pose(world, poses, id);
        live += particles::live_bound(emitter, now, particle_density);
        // A viewmodel emitter simulates in the first-person camera's frame, so its particles are
        // camera-local and `compose_viewmodel` places them on the drawn camera.
        let gun = match (viewmodel, &camera_inverse) {
            (Some((frame, _)), Some(inverse)) if frame.contains(id) => Some(inverse),
            _ => None,
        };
        match gun {
            Some(inverse) => emitters.push((
                id,
                emitter,
                crate::viewmodel::camera_local(inverse, pose.model()),
                local_basis,
                true,
            )),
            None => emitters.push((id, emitter, pose.transform(), basis, false)),
        }
    }

    // The keyed path pairs each particle with its instance key; the plain path is unchanged.
    if let Some(keys) = keys {
        let evaluate = || {
            par::par_map_chunks(&emitters, PARTICLE_PAR_CHUNK, |_, chunk| {
                chunk
                    .iter()
                    .flat_map(|(id, emitter, transform, basis, gun)| {
                        particles::evaluate_keyed(emitter, transform, now, particle_density, *basis)
                            .map(|(stream, mut p)| {
                                p.viewmodel = *gun;
                                (particles::instance_key(*id, emitter.started, stream), p)
                            })
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
        for (key, particle) in lists.into_iter().flatten() {
            keys.push(key);
            out.push(particle);
        }
        return out;
    }

    let evaluate = || {
        par::par_map_chunks(&emitters, PARTICLE_PAR_CHUNK, |_, chunk| {
            chunk
                .iter()
                .flat_map(|(_, emitter, transform, basis, gun)| {
                    particles::evaluate(emitter, transform, now, particle_density, *basis).map(
                        |mut p| {
                            p.viewmodel = *gun;
                            p
                        },
                    )
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
    visibility: &FrameVisibility,
    project_root: AssetRoot<'_>,
    assets: &mut dyn SceneAssets,
    atlases: &mut AtlasCache,
    poses: &mut WorldAffines,
    textures: &mut Vec<Arc<TextureImage>>,
    path_index: &mut StableMap<String, usize>,
    degraded: &mut Vec<(String, Diagnostic)>,
    now: GameTime,
    basis: CameraBasis,
    out: &mut Vec<SpriteInstance>,
    mut bullet_keys: Option<&mut Vec<(InstanceKey, u32)>>,
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
        if visibility.hidden(id) {
            continue;
        }
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
            if let Some(keys) = bullet_keys.as_deref_mut() {
                keys.push((
                    InstanceKey {
                        owner: id,
                        started: volley.started.0,
                        burst: false,
                        index: i64::from(bullet),
                    },
                    out.len() as u32,
                ));
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
                medium: 1.0,
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
    project_root: AssetRoot<'_>,
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
/// validation never subtracts light. A casting light carries its shadow tunables ([`local_shadow`]).
fn point_light_data(light: &PointLight, position: Vec3, dynamic: bool) -> Option<PointLightData> {
    let clamped =
        ludion_renderer::light_model::local_light(light.color, light.intensity, light.range)?;
    Some(PointLightData {
        position,
        color: clamped.color,
        intensity: clamped.intensity,
        range: clamped.range,
        dynamic,
        shadow: local_shadow(
            light.cast_shadows,
            light.shadow_bias,
            light.shadow_near,
            light.range,
            light.shadow_distance,
            light.shadow_fade,
        ),
        scatter: light.scatter.max(0.0),
    })
}

/// A casting light's [`LocalShadowData`], `None` when it doesn't cast. The bias clamps to
/// non-negative (`f32::max` drops a NaN) and a near plane that is not finite and in `(0, range)`
/// falls back to the default capped at half the range, so a Rust-built value that skipped
/// validation (`L1312`) still gets a usable view. A `distance` that is not finite and positive
/// becomes `None`; the fade clamps into `[0, distance]`, and is `0` without a distance (`L1322`).
pub(crate) fn local_shadow(
    cast: bool,
    bias: f32,
    near: f32,
    range: f32,
    distance: Option<f32>,
    fade: f32,
) -> Option<LocalShadowData> {
    if !cast {
        return None;
    }
    let near = if near.is_finite() && near > 0.0 && near < range {
        near
    } else {
        0.05_f32.min(range * 0.5)
    };
    let distance = distance.filter(|d| d.is_finite() && *d > 0.0);
    let fade = distance.map_or(0.0, |d| fade.max(0.0).min(d));
    Some(LocalShadowData {
        bias: bias.max(0.0),
        near,
        distance,
        fade,
    })
}

/// Builds [`SpotLightData`] from a `SpotLight` placed at `position`.
///
/// Like [`point_light_data`], an unusable range yields `None` and colour and intensity clamp to
/// non-negative; a zero-length or non-finite direction has no axis, so it also yields `None`. The
/// angles clamp into `[0, 89.9]` degrees with the inner never past the outer, so the cosines stay
/// positive and ordered for a Rust-built value that skipped validation (`L1272`). A casting light
/// carries its shadow tunables ([`local_shadow`]).
fn spot_light_data(light: &SpotLight, position: Vec3) -> Option<SpotLightData> {
    let clamped =
        ludion_renderer::light_model::local_light(light.color, light.intensity, light.range)?;
    let direction = Vec3::from(light.direction).try_normalize()?;
    let (cos_inner, cos_outer) =
        ludion_renderer::light_model::spot_cosines(light.inner_angle, light.outer_angle);
    Some(SpotLightData {
        position,
        direction,
        color: clamped.color,
        intensity: clamped.intensity,
        range: clamped.range,
        cos_inner,
        cos_outer,
        shadow: local_shadow(
            light.cast_shadows,
            light.shadow_bias,
            light.shadow_near,
            light.range,
            light.shadow_distance,
            light.shadow_fade,
        ),
        scatter: light.scatter.max(0.0),
    })
}

/// Builds [`ProbeData`] from a `ReflectionProbe` at `pose`, or `None` for a value `L1274` refuses
/// (a Rust-built world skips validation). The pose gives the position and orientation; scale is
/// ignored, the size lives in the shape.
fn probe_data(probe: &ReflectionProbe, pose: &RenderPose, entity: EntityId) -> Option<ProbeData> {
    let positive = |v: f32| v.is_finite() && v > 0.0;
    let volume = match probe.shape {
        ProbeShape::Box { extent } if extent.into_iter().all(positive) => ProbeVolume::Box {
            extent: Vec3::from(extent),
        },
        ProbeShape::Sphere { radius } if positive(radius) => ProbeVolume::Sphere { radius },
        _ => return None,
    };
    if !(probe.blend.is_finite() && probe.blend >= 0.0) {
        return None;
    }
    Some(ProbeData {
        entity,
        position: Vec3::from(pose.translation()),
        rotation: pose.quat(),
        volume,
        blend: probe.blend,
        priority: probe.priority,
    })
}

/// Every usable [`ReflectionProbe`] in ascending-`EntityId` order (the order `ids` carries),
/// unranked: `ludion_renderer::probe_rank` orders them.
pub(crate) fn extract_probes(
    world: &World,
    ids: &[EntityId],
    poses: &mut WorldAffines,
) -> Vec<ProbeData> {
    ids.iter()
        .filter_map(|&id| {
            let probe = get::<ReflectionProbe>(world, id)?;
            probe_data(probe, &render_pose(world, poses, id), id)
        })
        .collect()
}

/// Whether any entity has a [`ReflectionProbe`], for the `probes` debug-draw category.
pub(crate) fn any_probe(world: &World) -> bool {
    world
        .entities()
        .chain(world.presentation_entities())
        .any(|id| world.has::<ReflectionProbe>(id))
}

/// Whether any entity has a `Mesh`, for the `cull` debug-draw category.
pub(crate) fn any_mesh(world: &World) -> bool {
    world
        .entities()
        .chain(world.presentation_entities())
        .any(|id| get::<Mesh>(world, id).is_some())
}

/// Whether any point or spot light casts a shadow, for the `local_shadows` debug-draw category.
pub(crate) fn any_local_shadow_caster(world: &World) -> bool {
    world
        .entities()
        .chain(world.presentation_entities())
        .any(|id| {
            get::<PointLight>(world, id).is_some_and(|light| light.cast_shadows)
                || get::<SpotLight>(world, id).is_some_and(|light| light.cast_shadows)
        })
}

/// Segments in one great circle of a sphere probe's volume.
const PROBE_CIRCLE_SEGMENTS: usize = 32;
/// Half-length of the cross that marks a probe's capture point.
const PROBE_CAPTURE_HALF: f32 = 0.25;
/// The colour of a probe past `MAX_PROBES`, which is not shaded.
const PROBE_UNSHADED_COLOR: [f32; 4] = [0.25, 0.25, 0.25, 1.0];

/// The `probes` debug-draw category: each probe's volume wireframe and its capture point, coloured
/// by rank like the `probe_index` view. Linear RGBA, as `DebugSegment.color` is.
pub(crate) fn probe_debug_lines(world: &World) -> Vec<DebugSegment> {
    let ids: Vec<EntityId> = world
        .entities()
        .chain(world.presentation_entities())
        .collect();
    let mut probes = extract_probes(world, &ids, &mut WorldAffines::new());
    // The renderer keeps the first `MAX_PROBES` in entity order and ranks those.
    let held = probes.len().min(ludion_renderer::MAX_PROBES);
    probes[..held].sort_by(ludion_renderer::probe_rank);
    let mut lines = Vec::new();
    for (rank, probe) in probes.iter().enumerate() {
        let color = if rank < held {
            let [r, g, b] =
                ludion_renderer::PROBE_VIEW_PALETTE[rank % 8].map(ludion_sprite::srgb_decode);
            [r, g, b, 1.0]
        } else {
            PROBE_UNSHADED_COLOR
        };
        let mut line = |a: Vec3, b: Vec3| {
            lines.push(DebugSegment {
                a: a.to_array(),
                b: b.to_array(),
                color,
            });
        };
        match probe.volume {
            ProbeVolume::Box { extent } => {
                let corner = |i: usize| {
                    let sign = |bit: usize| if i >> bit & 1 == 1 { 1.0 } else { -1.0 };
                    probe.position
                        + probe.rotation * (extent * Vec3::new(sign(0), sign(1), sign(2)))
                };
                for i in 0..8 {
                    for bit in 0..3 {
                        if i >> bit & 1 == 0 {
                            line(corner(i), corner(i | 1 << bit));
                        }
                    }
                }
            }
            ProbeVolume::Sphere { radius } => {
                for (u, v) in [(Vec3::X, Vec3::Y), (Vec3::Y, Vec3::Z), (Vec3::Z, Vec3::X)] {
                    let at = |step: usize| {
                        let (sin, cos) = ludion_core::math::sin_cos(
                            step as f32 / PROBE_CIRCLE_SEGMENTS as f32 * std::f32::consts::TAU,
                        );
                        probe.position + (u * cos + v * sin) * radius
                    };
                    for step in 0..PROBE_CIRCLE_SEGMENTS {
                        line(at(step), at(step + 1));
                    }
                }
            }
        }
        for axis in [Vec3::X, Vec3::Y, Vec3::Z] {
            line(
                probe.position - axis * PROBE_CAPTURE_HALF,
                probe.position + axis * PROBE_CAPTURE_HALF,
            );
        }
    }
    lines
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
    // Out of `[0, 45]` degrees is rejected upstream (`L1282`); extract falls back to a point light.
    let angular_radius = if (0.0..=45.0).contains(&sun.angular_radius) {
        sun.angular_radius.to_radians()
    } else {
        0.0
    };

    SunData {
        direction,
        color: (Vec3::from(sun.color) * sun.intensity).max(Vec3::ZERO),
        light_space: projection * view,
        bias: sun.shadow_bias / (far - near),
        angular_radius,
        cascades: sun_cascade_config(sun, near, far),
    }
}

/// The sun's authored cascade request for the renderer to fit, or `None` when `cascades` is unset.
/// A Rust-built bad value bypasses validation, so it is clamped rather than rejected: a count into
/// `1..=MAX_SUN_CASCADES`, a non-positive or non-finite distance to the component default, and splits that are not
/// `count - 1` strictly ascending fractions in (0, 1) dropped (the practical split scheme).
fn sun_cascade_config(sun: &DirectionalLight, near: f32, far: f32) -> Option<SunCascadeConfig> {
    let count = sun.cascades?.clamp(1, MAX_SUN_CASCADES);
    let shadow_distance = if sun.shadow_distance.is_finite() && sun.shadow_distance > 0.0 {
        sun.shadow_distance
    } else {
        DirectionalLight::default().shadow_distance
    };
    let splits = sun
        .cascade_splits
        .as_deref()
        .filter(|splits| {
            splits.len() + 1 == count as usize
                && splits.iter().all(|s| s.is_finite() && *s > 0.0 && *s < 1.0)
                && splits.windows(2).all(|pair| pair[0] < pair[1])
        })
        .filter(|splits| !splits.is_empty())
        .map(|splits| {
            let mut padded = [0.0; 3];
            padded[..splits.len()].copy_from_slice(splits);
            padded
        });
    Some(SunCascadeConfig {
        count,
        shadow_distance,
        splits,
        shadow_near: near,
        shadow_far: far,
    })
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
    /// Every `SkinBone` part, grouped by the skin entity it names, in ascending entity order — so the
    /// first part naming a `(skin, bone)` pair wins if two ever do (the `L1196` case slipping past a
    /// hand-built world). A stale skin id, an unknown bone name, or a skin with no joints is never
    /// checked here: an entry the mesh loop's asset has no matching joint for is simply never matched
    /// by `posed_palette`, the documented runtime degrade to rest.
    pub(crate) skin_followers: StableMap<EntityId, Vec<(&'w str, Mat4)>>,
}

/// One placed mesh: its entity, world transform, flat color, and unresolved geometry source. Like a
/// [`PlacedSprite`]'s texture, a mesh's *geometry* is left unresolved here — a mesh's world transform
/// (and therefore `render.bounds`) does not depend on whether an asset document loads, so asset
/// resolution and its errors stay in [`extract`].
#[derive(Clone)]
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
    /// The `Mesh` component's wrap diffuse amount and scatter tint. `wrap == 0.0` is render-identical.
    pub(crate) wrap: f32,
    pub(crate) scatter_color: [f32; 3],
    /// The `Mesh` component's own rim; inheritance down `Parent` is resolved at instance build.
    pub(crate) rim: Option<Rim>,
    /// The `Mesh` component's own medium scale; inheritance down `Parent` is resolved at instance build.
    pub(crate) medium: Option<f32>,
    /// The `Mesh` component's wet-surface porosity; inert on geometry that authors its own material.
    pub(crate) porosity: Option<f32>,
    /// The `Mesh` component's own blend; never inherited.
    pub(crate) blend: Option<MeshBlend>,
    /// The `Mesh` component's `lit`; read only when the mesh draws blended.
    pub(crate) lit: bool,
    /// The `Mesh` component's `blend_depth`; read only when the mesh draws blended.
    pub(crate) blend_depth: BlendDepth,
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
pub(crate) fn placed_source(shape: &MeshSource) -> PlacedMeshSource<'_> {
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
/// key names the entity (`{offender}/sprite`, `{offender}/mesh`, see
/// [`offender_key`](crate::diagnostics::offender_key)), not its walk position: a walk index is reused.
fn drop_non_finite(
    world: &World,
    placed: &mut Placed<'_>,
    degraded: &mut Vec<(String, Diagnostic)>,
) {
    let mut last_sprite = None;
    placed.sprites.retain(|s| {
        let finite = s.center.is_finite() && s.right.is_finite() && s.up.is_finite();
        if !finite && last_sprite != Some(s.id) {
            last_sprite = Some(s.id);
            degraded.push((
                format!("{}/sprite", crate::diagnostics::offender_key(world, s.id)),
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
                format!("{}/mesh", crate::diagnostics::offender_key(world, m.id)),
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
    /// One item of a `UiRadial`: its `atlas` and `frame` pair.
    UiRadialItem(usize, usize),
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
            TextureSite::UiRadialItem(index, item) => {
                format!("/entities/{index}/components/UiRadial/items/{item}")
            }
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
    /// `Sprite.medium`, resolved to `1.0` when absent.
    pub(crate) medium: f32,
}

/// [`place`], over a fresh [`AssetCache`] of its own — what a caller with no host cache wants
/// (`render.bounds`). Not prefetched: it only resolves atlas region sizes, so a prefetch would
/// decode meshes it never reads.
pub(crate) fn place_uncached<'w>(
    world: &'w World,
    ids: &[EntityId],
    camera: &RenderCamera,
    project_root: AssetRoot<'_>,
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
        GameTime::from(now),
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
    fn map(&mut self, _: &str) -> Result<MapImage, Diagnostic> {
        Err(Self::unresolved().remove(0))
    }
    fn texture(&mut self, _: &str) -> Result<Arc<TextureImage>, Diagnostic> {
        Err(Self::unresolved().remove(0))
    }
    fn atlas(&mut self, _: &str) -> Result<Arc<AtlasAsset>, Vec<Diagnostic>> {
        Err(Self::unresolved())
    }
    fn lut(&mut self, _: &str) -> Result<Arc<CubeLut>, Vec<Diagnostic>> {
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
    fn prefab_clips(&mut self, _: &str) -> Result<Arc<PrefabClipData>, Vec<Diagnostic>> {
        Err(Self::unresolved())
    }
    fn prefab_rests(&mut self, _: &str) -> Option<Arc<PartRests>> {
        None
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
pub(crate) fn asset_refs(world: &World, project_root: AssetRoot<'_>) -> AssetRefs {
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
        GameTime::from_ticks(0, TickRate::DEFAULT),
    );
    let mut refs = AssetRefs::default();
    // The main frame's LUT, and every profile's: a view takes the profile's.
    let profile_luts = ids
        .iter()
        .filter(|&&id| world.has::<PostProfile>(id))
        .filter_map(|&id| get::<PostProcess>(world, id));
    for post in main_post(world, &ids)
        .map(|(_, post)| post)
        .into_iter()
        .chain(profile_luts)
    {
        if let Some(lut) = &post.lut {
            refs.luts.insert(ludion_core::paths::normalize(&lut.path));
        }
    }
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
    for &id in &ids {
        if get::<Animation>(world, id).is_some()
            && let Some(instance) = get::<PrefabInstance>(world, id)
            && instance.root == id
            && !drives_a_model_mesh(world, id)
        {
            refs.prefab_clips
                .insert(ludion_core::paths::normalize(&instance.source));
        }
        let Some(decal) = get::<Decal>(world, id) else {
            continue;
        };
        for map in [
            &decal.base_color_texture,
            &decal.normal_texture,
            &decal.metallic_roughness_texture,
        ]
        .into_iter()
        .flatten()
        {
            refs.maps.insert(ludion_core::paths::normalize(&map.path));
        }
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
    now: GameTime,
) -> Placed<'w> {
    let (skin_followers, _) = place_poses(world, ids, assets, poses, now);
    place_with(
        world,
        ids,
        camera,
        fonts,
        assets,
        atlases,
        poses,
        now,
        skin_followers,
        &|_| false,
    )
}

/// The animation half of [`place`]: prefab part poses, then `ParentJoint` socket joints. Returns the
/// skin followers [`place_with`] attaches to their skins, and the palettes it posed for socketed skins
/// ([`pose_skins`] reuses them). [`extract_inner`] calls the halves apart so the profile can time
/// them separately.
pub(crate) fn place_poses<'w>(
    world: &'w World,
    ids: &[EntityId],
    assets: &mut dyn SceneAssets,
    poses: &mut WorldAffines,
    now: GameTime,
) -> (SkinFollowers<'w>, SocketPalettes) {
    prefab_part_poses(world, ids, assets, now, poses);
    pose_sockets(world, ids, assets, poses, now)
}

/// The component columns [`place_with`] reads per entity, each resolved once per call.
struct PlaceColumns<'w> {
    transform: ReadColumn<'w, Transform>,
    gi_dynamic: ReadColumn<'w, GiDynamic>,
    mesh: ReadColumn<'w, Mesh>,
    mesh_grid: ReadColumn<'w, MeshGrid>,
    sprite: ReadColumn<'w, Sprite>,
    tilemap: ReadColumn<'w, Tilemap>,
    point_light: ReadColumn<'w, PointLight>,
    spot_light: ReadColumn<'w, SpotLight>,
    text: ReadColumn<'w, Text>,
}

impl<'w> PlaceColumns<'w> {
    fn of(world: &'w World) -> Self {
        Self {
            transform: world.read_column(),
            gi_dynamic: world.read_column(),
            mesh: world.read_column(),
            mesh_grid: world.read_column(),
            sprite: world.read_column(),
            tilemap: world.read_column(),
            point_light: world.read_column(),
            spot_light: world.read_column(),
            text: world.read_column(),
        }
    }
}

/// Every mesh placement an entity's `Mesh` and `MeshGrid` make, from its world `model` and
/// `normal_matrix` alone: the `Mesh` first, then each non-empty grid cell row-major, in the
/// entity's local XZ plane under its whole pose (see `MeshGrid`'s placement convention).
///
/// Pure — no `World`, no `EntityId` — so the placement of a part is a function of its component
/// values and its pose: the one placement every mesh shares, dynamic or kept in the
/// [`StaticScene`](crate::static_scene::StaticScene), and the one a prefab's unspawned statics can
/// be placed with. A space is the empty cell; a glyph with no legend entry draws nothing
/// (validation rejects it as `L1020`; extract stays panic-free on an unvalidated world).
pub(crate) fn place_mesh_parts<'a>(
    mesh: Option<&'a Mesh>,
    grid: Option<&'a MeshGrid>,
    model: Mat4,
    normal_matrix: Mat3,
    mut emit: impl FnMut(MeshSite, Mat4, Mat3, &'a Mesh),
) {
    if let Some(mesh) = mesh {
        emit(MeshSite::Component, model, normal_matrix, mesh);
    }
    if let Some(grid) = grid {
        let mut glyph_buf = [0u8; 4];
        for (row_index, row) in grid.rows.iter().enumerate() {
            for (col_index, glyph) in row.chars().enumerate() {
                let Some(mesh) = grid.legend.get(glyph.encode_utf8(&mut glyph_buf)) else {
                    continue;
                };
                let (cell, basis) = mesh_grid_cell(grid, col_index, row_index);
                emit(
                    MeshSite::GridCell(glyph),
                    model * cell,
                    normal_matrix * basis,
                    mesh,
                );
            }
        }
    }
}

/// One [`PlacedMesh`] from a [`place_mesh_parts`] emission. Every cell of a grid carries the one
/// entity `id`, so `render.bounds` unions the grid into one renderable and a `Fade` applies to all
/// of it.
pub(crate) fn placed_mesh<'a>(
    id: EntityId,
    index: usize,
    site: MeshSite,
    model: Mat4,
    normal_matrix: Mat3,
    mesh: &'a Mesh,
    dynamic: bool,
) -> PlacedMesh<'a> {
    PlacedMesh {
        id,
        index,
        site,
        model,
        normal_matrix,
        color: mesh.color,
        material: [mesh.roughness, mesh.metallic, mesh.emissive],
        flash: mesh.flash,
        flash_color: mesh.flash_color,
        wrap: mesh.wrap,
        scatter_color: mesh.scatter_color,
        rim: mesh.rim,
        medium: mesh.medium,
        porosity: mesh.porosity,
        blend: mesh.blend,
        lit: mesh.lit,
        blend_depth: mesh.blend_depth,
        source: placed_source(&mesh.shape),
        dynamic,
    }
}

/// The placement half of [`place`], after [`place_poses`].
#[allow(clippy::too_many_arguments)]
pub(crate) fn place_with<'w>(
    world: &'w World,
    ids: &[EntityId],
    camera: &RenderCamera,
    fonts: &FontLibrary,
    assets: &mut dyn SceneAssets,
    atlases: &mut AtlasCache,
    poses: &mut WorldAffines,
    now: GameTime,
    skin_followers: StableMap<EntityId, Vec<(&'w str, Mat4)>>,
    skip: &dyn Fn(EntityId) -> bool,
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
        skin_followers,
    };

    let locale = fonts
        .has_locale_overrides()
        .then(|| crate::localization::active_locale(world));
    let locale = locale.as_deref();
    let cols = PlaceColumns::of(world);
    for (index, &id) in ids.iter().enumerate() {
        let pose = render_pose_with(&cols.transform, world, poses, id);
        let translation = Vec3::from(pose.translation());
        // Read once per entity (`Plan/gi-wave3-dynamic-injection.md` §3.2), whichever of Mesh/
        // PointLight this entity carries reads the same answer — an entity carrying neither is
        // simply never consulted, the marker's documented inert case.
        let dynamic = cols.gi_dynamic.get(id).is_some();

        if !skip(id) {
            let (mesh, grid) = (cols.mesh.get(id), cols.mesh_grid.get(id));
            if mesh.is_some() || grid.is_some() {
                place_mesh_parts(
                    mesh,
                    grid,
                    pose.model(),
                    pose.normal_matrix(),
                    |site, model, normal_matrix, mesh| {
                        placed.meshes.push(placed_mesh(
                            id,
                            index,
                            site,
                            model,
                            normal_matrix,
                            mesh,
                            dynamic,
                        ));
                    },
                );
            }
        }

        if let Some(sprite) = cols.sprite.get(id) {
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
                medium: sprite.medium.unwrap_or(1.0),
            });
        }

        if let Some(tilemap) = cols.tilemap.get(id) {
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
                        medium: 1.0,
                    });
                }
            }
        }

        if let Some(light) = cols.point_light.get(id)
            && let Some(data) = point_light_data(light, translation, dynamic)
        {
            placed.lights.push((id, data));
        }

        if let Some(light) = cols.spot_light.get(id)
            && let Some(data) = spot_light_data(light, translation)
        {
            placed.spot_lights.push((id, data));
        }

        if let Some(text) = cols.text.get(id) {
            let laid = ludion_font::layout(
                fonts.face(text.font.as_deref(), locale),
                text.shown(),
                text.size,
                0.0,
            );
            let quads = laid.quads(
                [translation.x, translation.y],
                text.size,
                ludion_font::YAxis::Up,
            );
            let mut chain = placed_fonts(fonts, text.font.as_deref(), locale);
            // One run per chain face: the renderer draws one atlas per run.
            for (quads, _, _) in ui::text_runs(quads, &[]) {
                let face = quads.first().map_or(0, |q| usize::from(q.face));
                placed.texts.push((
                    id,
                    TextRun {
                        position: translation,
                        size: text.size,
                        color: text.color,
                        text: text.shown().to_owned(),
                        quads,
                        extent: laid.extent(),
                        font: None,
                    },
                    chain.get_mut(face).and_then(Option::take),
                    index,
                ));
            }
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

/// The world model and normal matrix [`place_with`] gives `id`'s meshes.
pub(crate) fn render_model(world: &World, poses: &mut WorldAffines, id: EntityId) -> (Mat4, Mat3) {
    let pose = render_pose(world, poses, id);
    (pose.model(), pose.normal_matrix())
}

fn render_pose(world: &World, poses: &mut WorldAffines, id: EntityId) -> RenderPose {
    render_pose_with(&world.read_column(), world, poses, id)
}

/// [`render_pose`] reading the stored [`Transform`] through a column the caller built once.
fn render_pose_with(
    transforms: &ReadColumn<'_, Transform>,
    world: &World,
    poses: &mut WorldAffines,
    id: EntityId,
) -> RenderPose {
    let affine = poses.parented(world, id);
    RenderPose {
        local: if affine.is_some() {
            Transform::default()
        } else {
            transforms.get(id).copied().unwrap_or_default()
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

/// Appends skin `skin`'s bone-palette entries for this frame to `out`: the `SkinBone` followers'
/// pose when `followed` names any ([`posed_palette`]), else its character's `Animation` clip and
/// crossfade at the held `now` ([`sampled_palette`]), else each joint's precomposed rest entry.
/// Returns the path taken; `scratch` holds the joints' posed model matrices unless it was
/// [`PalettePath::Rest`]. The `Skin` block and the socket pre-pass both call it, and a skin poses
/// once per extract: a socketed skin in the pre-pass ([`pose_each`]), whose palette `pose_skins`
/// then copies, any other in `pose_skins`. A socket child and the skin it rides therefore can never
/// read two evaluations of one pose.
#[allow(clippy::too_many_arguments)]
pub(crate) fn skin_palette(
    world: &World,
    skin: EntityId,
    joints: &[SkinJoint],
    character: Option<&CharacterData>,
    skin_model: Mat4,
    followed: Option<&[(&str, Mat4)]>,
    now: GameTime,
    scratch: &mut Vec<Mat4>,
    out: &mut Vec<Mat4>,
) -> PalettePath {
    let held_now = held_for(world, skin, now);
    let (pose, crossfade) = character.map_or((PoseSide::EMPTY, None), |data| {
        character_pose(world, skin, data, held_now)
    });
    let layers = character.map_or_else(Vec::new, |data| {
        layer_poses(world, skin, data, joints.len(), held_now)
    });
    match followed {
        Some(followed) if !followed.is_empty() => {
            posed_palette(joints, skin_model, followed, scratch, out);
            PalettePath::Followed
        }
        _ if pose.is_empty() && crossfade.is_none() && layers.is_empty() => {
            out.extend(joints.iter().map(|joint| joint.rest));
            PalettePath::Rest
        }
        _ => {
            sampled_palette_sides(joints, pose, crossfade.as_ref(), &layers, scratch, out);
            PalettePath::Sampled {
                fading: crossfade.is_some(),
                layers: layers.len() as u8,
                blend_extra: (pose.count().saturating_sub(1)
                    + crossfade
                        .as_ref()
                        .map_or(0, |fade| fade.from.count().saturating_sub(1)))
                    as u8,
            }
        }
    }
}

/// Which way [`skin_palette`] wrote a palette.
#[derive(Debug, Clone, Copy)]
pub(crate) enum PalettePath {
    /// Each joint's precomposed rest entry; nothing posed.
    Rest,
    /// `SkinBone` followers drove the bones.
    Followed,
    /// The character's clip was sampled; `fading` when a crossfade blended a second clip, and
    /// `layers` animation layers composed over it; `blend_extra` counts the blend-space entries
    /// beyond one per side.
    Sampled {
        fading: bool,
        layers: u8,
        blend_extra: u8,
    },
}

impl PalettePath {
    /// Whether `scratch` holds posed model matrices.
    fn posed(self) -> bool {
        !matches!(self, PalettePath::Rest)
    }
}

/// One skinned instance awaiting its palette in the `Skin` block.
struct SkinJob<'a> {
    skin: EntityId,
    asset: usize,
    character: Option<Arc<CharacterData>>,
    model: Mat4,
    followed: Option<&'a [(&'a str, Mat4)]>,
}

/// Skin jobs per parallel claim: instances are similar in size, so 16 keeps a 500-crowd at ~32 chunks.
const SKIN_PAR_CHUNK: usize = 16;

/// Below this many joints a frame poses on one thread: posing is ~70 ns a joint, so 2048 joints is
/// ~0.15 ms of work against ~0.1 ms of scoped spawns.
const SKIN_PAR_MIN_JOINTS: usize = 2048;

/// Every job's bone palette, concatenated in job order, and the frame's [`SkinCounts`].
///
/// A job whose skin the socket pre-pass already posed from the same inputs copies that palette
/// ([`SocketPalettes`]) and counts as sampled; every other job poses here.
///
/// Jobs pose in parallel through [`par::par_map_chunks`] in fixed-size chunks whose results are
/// concatenated in chunk order, so the palette is byte-identical at any thread count.
fn pose_skins(
    world: &World,
    jobs: &[SkinJob],
    mesh_assets: &[Arc<MeshData>],
    now: GameTime,
    socketed: &SocketPalettes,
) -> (Vec<Mat4>, SkinCounts) {
    let total: usize = jobs
        .iter()
        .map(|job| mesh_assets[job.asset].skin_joints.len())
        .sum();
    let planned: Vec<(&SkinJob, Reuse<'_>)> = jobs
        .iter()
        .map(|job| (job, socketed.reusable(job, &mesh_assets[job.asset])))
        .collect();
    let evaluate = || {
        par::par_map_chunks(&planned, SKIN_PAR_CHUNK, |_, chunk| {
            let mut scratch = Vec::new();
            let mut out = Vec::new();
            let mut counts = SkinCounts::default();
            for &(job, reuse) in chunk {
                let joints = &mesh_assets[job.asset].skin_joints;
                let path = match reuse {
                    Some((stored, path)) => {
                        out.extend_from_slice(stored);
                        path
                    }
                    None => skin_palette(
                        world,
                        job.skin,
                        joints,
                        job.character.as_deref(),
                        job.model,
                        job.followed,
                        now,
                        &mut scratch,
                        &mut out,
                    ),
                };
                counts.instances += 1;
                counts.palettes_sampled += 1;
                counts.joints_sampled += match path {
                    PalettePath::Rest => 0,
                    PalettePath::Followed => joints.len() as u64,
                    PalettePath::Sampled {
                        fading,
                        layers,
                        blend_extra,
                    } => {
                        joints.len() as u64
                            * (1 + u64::from(fading) + u64::from(layers) + u64::from(blend_extra))
                    }
                };
            }
            (out, counts)
        })
    };
    // Copies cost nothing next to a scoped spawn: only the jobs that pose count toward going wide.
    let posing: usize = planned
        .iter()
        .filter(|(_, reuse)| reuse.is_none())
        .map(|(job, _)| mesh_assets[job.asset].skin_joints.len())
        .sum();
    let chunks = if posing < SKIN_PAR_MIN_JOINTS {
        par::with_threads(1, evaluate)
    } else {
        evaluate()
    };
    let mut palette = Vec::with_capacity(total);
    let mut counts = SkinCounts::default();
    for (out, chunk) in chunks {
        palette.extend(out);
        counts.add(chunk);
    }
    (palette, counts)
}

/// `joint`'s posed model matrix in skin `skin`'s model space (the space of the joint's rest model
/// transform), as [`skin_palette`] poses it this frame; `None` on the rest path or for an unknown
/// joint, where the caller keeps the rest level.
#[allow(clippy::too_many_arguments)]
pub(crate) fn posed_joint_model(
    world: &World,
    skin: EntityId,
    joints: &[SkinJoint],
    character: Option<&CharacterData>,
    skin_model: Mat4,
    followed: Option<&[(&str, Mat4)]>,
    now: GameTime,
    joint: &str,
) -> Option<Mat4> {
    let index = joints.iter().position(|j| j.name == joint)?;
    let (mut scratch, mut out) = (Vec::new(), Vec::new());
    skin_palette(
        world,
        skin,
        joints,
        character,
        skin_model,
        followed,
        now,
        &mut scratch,
        &mut out,
    )
    .posed()
    .then(|| scratch[index])
}

/// A matrix as the affine the hierarchy folds compose; [`RenderPose::world`]'s inverse.
pub(crate) fn mat4_affine(m: Mat4) -> Affine3 {
    Affine3 {
        cols: [m.x_axis, m.y_axis, m.z_axis, m.w_axis].map(|c| [c.x, c.y, c.z]),
    }
}

/// The part world poses that drive each skin's bones: `SkinBone` parts grouped by skin, the first
/// part per bone winning.
fn skin_followers<'w>(
    world: &'w World,
    ids: &[EntityId],
    poses: &mut WorldAffines,
) -> StableMap<EntityId, Vec<(&'w str, Mat4)>> {
    let mut followers: StableMap<EntityId, Vec<(&'w str, Mat4)>> = StableMap::new();
    for &id in ids {
        let Some(skin_bone) = get::<SkinBone>(world, id) else {
            continue;
        };
        let part_world = render_pose(world, poses, id).model();
        let entries = followers.entry(skin_bone.skin).or_default();
        if !entries.iter().any(|(name, _)| *name == skin_bone.bone) {
            entries.push((skin_bone.bone.as_str(), part_world));
        }
    }
    followers
}

/// Builds the skin followers and poses every `ParentJoint` child's joint level in `poses` with the
/// skin's posed joint model (or, under an assembly, the part's final drawn pivot frame), so the
/// child and everything below it draw at `skin world × posed joint × local`; returns the followers.
/// Outer skins first (by ancestor depth), so a skin that is itself a socket child reads its
/// parent's override. Silent on every miss: the mesh loop reports load errors, and a miss keeps the
/// joint's rest level.
///
/// A `SkinBone` part under a socket child memoizes that child before its posed joint exists, and a
/// follower on a skin feeds that skin's palette, so the two depend on each other. A pass that sets a
/// differing posed joint on an already memoized child is stale: the memo is cleared and the pass
/// reruns, at most `sockets + 1` times (an acyclic nesting gains a level per pass). A cycle (a part
/// under a skin's own socket that drives that same skin) settles at the cap with the last pass,
/// deterministically. A scene with no part or skin under a socket runs one pass. Each pass poses
/// every socketed skin once, in parallel per depth group, and returns the final pass's palettes
/// ([`SocketPalettes`]) beside the followers.
fn pose_sockets<'w>(
    world: &'w World,
    ids: &[EntityId],
    assets: &mut dyn SceneAssets,
    poses: &mut WorldAffines,
    now: GameTime,
) -> (SkinFollowers<'w>, SocketPalettes) {
    let mut sockets: Vec<(usize, EntityId, EntityId)> = Vec::new();
    for &id in ids {
        if !world.has::<ParentJoint>(id) || !world.has::<ludion_ecs::Parent>(id) {
            continue;
        }
        let Some(&ludion_ecs::Parent(skin)) = get::<ludion_ecs::Parent>(world, id) else {
            continue;
        };
        let mut depth = 0;
        let mut node = skin;
        while depth <= crate::hierarchy::MAX_HIERARCHY_DEPTH
            && world.has::<ludion_ecs::Parent>(node)
        {
            let Some(&ludion_ecs::Parent(up)) = get::<ludion_ecs::Parent>(world, node) else {
                break;
            };
            depth += 1;
            node = up;
        }
        sockets.push((depth, id, skin));
    }
    sockets.sort_by_key(|&(depth, id, _)| (depth, id));
    let mut store = SocketPalettes::default();
    for pass in 0..=sockets.len() {
        let followers = skin_followers(world, ids, poses);
        let stale = pose_each(world, &sockets, assets, poses, &followers, now, &mut store);
        if !stale || pass == sockets.len() {
            return (followers, store);
        }
        poses.clear_memo();
    }
    unreachable!("the loop returns on its last pass")
}

/// Each skin's `SkinBone` followers: bone name and the part's world pose.
type SkinFollowers<'w> = StableMap<EntityId, Vec<(&'w str, Mat4)>>;

/// The palettes [`pose_each`] computed for socketed skins, kept so [`pose_skins`] copies a skin's
/// palette instead of posing it again: a skin is posed once per extract.
#[derive(Default)]
pub(crate) struct SocketPalettes {
    /// Each evaluate chunk's palette run as `par_map_chunks` returned it, never concatenated.
    chunks: Vec<Vec<Mat4>>,
    skins: StableMap<EntityId, StoredPalette>,
}

/// One skin's stored palette and the inputs it was posed with.
struct StoredPalette {
    mesh: Arc<MeshData>,
    character: Option<Arc<CharacterData>>,
    model: Mat4,
    chunk: usize,
    range: std::ops::Range<usize>,
    path: PalettePath,
}

/// A stored palette slice and its path, when [`SocketPalettes::reusable`] accepts a job.
type Reuse<'a> = Option<(&'a [Mat4], PalettePath)>;

impl SocketPalettes {
    /// The stored palette for `job` when it was posed from the same inputs [`pose_skins`] would use.
    fn reusable(&self, job: &SkinJob, mesh: &Arc<MeshData>) -> Reuse<'_> {
        let stored = self.skins.get(&job.skin)?;
        let same_character = match (&stored.character, &job.character) {
            (Some(a), Some(b)) => Arc::ptr_eq(a, b),
            (None, None) => true,
            _ => false,
        };
        let same_model = stored
            .model
            .to_cols_array()
            .iter()
            .zip(job.model.to_cols_array())
            .all(|(a, b)| a.to_bits() == b.to_bits());
        (Arc::ptr_eq(&stored.mesh, mesh) && same_character && same_model).then(|| {
            (
                &self.chunks[stored.chunk][stored.range.clone()],
                stored.path,
            )
        })
    }
}

/// [`pose_each`]'s per-call asset cache: `(is_character, rel)` to the mesh and its character.
type ResolvedSocketAssets<'w> =
    StableMap<(bool, &'w str), (Arc<MeshData>, Option<Arc<CharacterData>>)>;

/// One socketed skin to pose in a [`pose_each`] depth group.
struct SocketJob {
    skin: EntityId,
    mesh: Arc<MeshData>,
    character: Option<Arc<CharacterData>>,
    model: Mat4,
    /// The joint indices this skin's sockets ride.
    joints: Vec<usize>,
}

/// What posing a [`SocketJob`] produced: its palette length, path and the requested joints' models.
struct SocketPosed {
    len: usize,
    path: PalettePath,
    joints: Vec<Option<Mat4>>,
}

/// One pass of [`pose_sockets`]: poses each socket child's joint; true when it set a differing
/// value on a child a lookup had already memoized (compared bit for bit, so a NaN cannot loop).
///
/// Sockets are taken a depth group at a time (a skin under a socket child sits strictly deeper than
/// that child, so no group member reads another's joint): gathered in socket order, each distinct skin
/// posed once in parallel through [`par::par_map_chunks`] and its palette kept in `store`, then applied
/// in socket order. Same result at any thread count.
fn pose_each(
    world: &World,
    sockets: &[(usize, EntityId, EntityId)],
    assets: &mut dyn SceneAssets,
    poses: &mut WorldAffines,
    followers: &StableMap<EntityId, Vec<(&str, Mat4)>>,
    now: GameTime,
    store: &mut SocketPalettes,
) -> bool {
    *store = SocketPalettes::default();
    let mut stale = false;
    let mut resolved: ResolvedSocketAssets = StableMap::new();
    let mut assembly_assets: StableMap<&str, Arc<AssemblyData>> = StableMap::new();
    for group in sockets.chunk_by(|a, b| a.0 == b.0) {
        let mut jobs: Vec<SocketJob> = Vec::new();
        let mut job_of: StableMap<EntityId, usize> = StableMap::new();
        // (socket child, job, slot in the job's joints), in socket order.
        let mut applies: Vec<(EntityId, usize, usize)> = Vec::new();
        // Assembly sockets: (socket child, part level), and each assembly's placements this group.
        let mut assembly_levels: Vec<(EntityId, Mat4)> = Vec::new();
        let mut placements: StableMap<EntityId, Vec<(Mat4, Mat3)>> = StableMap::new();
        for &(_, id, skin) in group {
            if !world.is_alive(skin) {
                continue;
            }
            let Some(joint) = get::<ParentJoint>(world, id) else {
                continue;
            };
            let Some(mesh) = get::<Mesh>(world, skin) else {
                continue;
            };
            if let MeshSource::Assembly(rel) = &mesh.shape {
                let Some(data) = assembly_assets.get(&rel.as_str()).cloned().or_else(|| {
                    let data = assets.assembly(rel).ok()?;
                    assembly_assets.insert(rel.as_str(), data.clone());
                    Some(data)
                }) else {
                    continue;
                };
                let Some(part) = data.parts.iter().position(|p| p.name == joint.joint) else {
                    continue;
                };
                let placed = placements.entry(skin).or_insert_with(|| {
                    let model = render_pose(world, poses, skin).model();
                    assembly_part_models(world, skin, &data, model, now)
                });
                let pivot = Vec3::from(data.parts[part].pivot);
                assembly_levels.push((id, placed[part].0 * Mat4::from_translation(pivot)));
                continue;
            }
            // A distinct asset resolves once per call: sockets share a handful of paths. Only
            // successes are cached, so a failing path is retried (and reported) per socket.
            let (is_character, rel) = match &mesh.shape {
                MeshSource::Asset(rel) => (false, rel.as_str()),
                MeshSource::Character(rel) => (true, rel.as_str()),
                _ => continue,
            };
            let (data, character) = match resolved.get(&(is_character, rel)) {
                Some((data, character)) => (data.clone(), character.clone()),
                None => {
                    let (mesh_rel, character) = if is_character {
                        match assets.character(rel) {
                            Ok(data) => (data.mesh.clone(), Some(data)),
                            Err(_) => continue,
                        }
                    } else {
                        (rel.to_owned(), None)
                    };
                    let Ok(data) = assets.mesh(&mesh_rel) else {
                        continue;
                    };
                    resolved.insert((is_character, rel), (data.clone(), character.clone()));
                    (data, character)
                }
            };
            let Some(index) = data.skin_joints.iter().position(|j| j.name == joint.joint) else {
                continue;
            };
            let slot = *job_of.entry(skin).or_insert_with(|| {
                jobs.push(SocketJob {
                    skin,
                    mesh: data,
                    character,
                    model: render_pose(world, poses, skin).model(),
                    joints: Vec::new(),
                });
                jobs.len() - 1
            });
            jobs[slot].joints.push(index);
            applies.push((id, slot, jobs[slot].joints.len() - 1));
        }
        let total: usize = jobs.iter().map(|job| job.mesh.skin_joints.len()).sum();
        let evaluate = || {
            par::par_map_chunks(&jobs, SKIN_PAR_CHUNK, |_, chunk| {
                let (mut scratch, mut out) = (Vec::new(), Vec::new());
                let mut posed = Vec::with_capacity(chunk.len());
                for job in chunk {
                    let start = out.len();
                    let path = skin_palette(
                        world,
                        job.skin,
                        &job.mesh.skin_joints,
                        job.character.as_deref(),
                        job.model,
                        followers.get(&job.skin).map(Vec::as_slice),
                        now,
                        &mut scratch,
                        &mut out,
                    );
                    posed.push(SocketPosed {
                        len: out.len() - start,
                        path,
                        joints: job
                            .joints
                            .iter()
                            .map(|&index| path.posed().then(|| scratch[index]))
                            .collect(),
                    });
                }
                (out, posed)
            })
        };
        let chunks = if total < SKIN_PAR_MIN_JOINTS {
            par::with_threads(1, evaluate)
        } else {
            evaluate()
        };
        let mut results: Vec<SocketPosed> = Vec::with_capacity(jobs.len());
        let mut chunk_jobs = jobs.iter();
        for (out, posed) in chunks {
            let chunk = store.chunks.len();
            store.chunks.push(out);
            let mut start = 0;
            for result in posed {
                let job = chunk_jobs.next().expect("one result per job");
                store.skins.insert(
                    job.skin,
                    StoredPalette {
                        mesh: job.mesh.clone(),
                        character: job.character.clone(),
                        model: job.model,
                        chunk,
                        range: start..start + result.len,
                        path: result.path,
                    },
                );
                start += result.len;
                results.push(result);
            }
        }
        for (id, job, slot) in applies {
            let Some(model) = results[job].joints[slot] else {
                continue;
            };
            stale |= apply_posed_joint(poses, id, model);
        }
        for (id, model) in assembly_levels {
            stale |= apply_posed_joint(poses, id, model);
        }
    }
    stale
}

/// Sets `id`'s posed joint level to `model`; true when it differs (bit for bit, so a NaN cannot
/// loop) from the value on a child a lookup had already memoized.
fn apply_posed_joint(poses: &mut WorldAffines, id: EntityId, model: Mat4) -> bool {
    let posed = mat4_affine(model);
    let mut stale = false;
    if poses.is_memoized(id) {
        let same = poses.posed_joint(id).is_some_and(|old| {
            old.cols
                .iter()
                .flatten()
                .zip(posed.cols.iter().flatten())
                .all(|(a, b)| a.to_bits() == b.to_bits())
        });
        stale = !same;
    }
    poses.set_posed_joint(id, posed);
    stale
}

/// Appends one skinned instance's bone-palette entries to `out`, one per `joints` entry
/// (parent-first, so one forward pass suffices): a joint a `followed` pair names takes that part's
/// world pose (`skin_model⁻¹ × part_world`, then `× inverse_bind`); every other joint composes its
/// rest local transform onto its parent's already-posed model (a root composes onto the skin's own
/// frame, i.e. its `rest_local` verbatim). Only called when `followed` is non-empty — the caller's
/// fast path for a part-less instance pushes each joint's precomposed [`SkinJoint::rest`] instead, to
/// stay byte-identical to the render-only rest pose. `posed_model` is the caller's scratch, cleared
/// on entry, so a frame of instances allocates it once.
#[allow(clippy::disallowed_methods)] // render-side: GPU matrix, never reaches a hash
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
    sampled_palette_layered(joints, pose, fade, &[], posed_model, out);
}

/// [`sampled_palette`] with `layers` composed over each joint's base pose, in order. With no layers
/// the path is exactly [`sampled_palette`]'s; a joint no layer touches keeps its base `local`
/// bit-for-bit.
pub(crate) fn sampled_palette_layered(
    joints: &[SkinJoint],
    pose: ClipPose<'_, ClipSampler>,
    fade: Option<&CrossfadePose<'_, ClipSampler>>,
    layers: &[LayerPose<'_>],
    posed_model: &mut Vec<Mat4>,
    out: &mut Vec<Mat4>,
) {
    let fade = fade.map(|fade| PoseFade {
        from: PoseSide::single(fade.from),
        weight: fade.weight,
    });
    sampled_palette_sides(
        joints,
        PoseSide::single(pose),
        fade.as_ref(),
        layers,
        posed_model,
        out,
    );
}

/// [`sampled_palette_layered`] over [`PoseSide`]s: each side may be a blend of several weighted
/// clips. A side of one weight-1 entry samples exactly as a plain clip.
pub(crate) fn sampled_palette_sides(
    joints: &[SkinJoint],
    pose: PoseSide<'_>,
    fade: Option<&PoseFade<'_>>,
    layers: &[LayerPose<'_>],
    posed_model: &mut Vec<Mat4>,
    out: &mut Vec<Mat4>,
) {
    posed_model.clear();
    let pose = pose.bound(joints.len());
    let fade = fade.map(|fade| PoseFade {
        from: fade.from.bound(joints.len()),
        weight: fade.weight,
    });
    let fade = fade.as_ref();
    for (index, joint) in joints.iter().enumerate() {
        let sampled = match fade {
            None => pose.sample(index, joint),
            Some(fade) => blended_joint_pose(
                joint,
                fade.from.sample(index, joint),
                pose.sample(index, joint),
                fade.weight,
            ),
        };
        let sampled = if layers.is_empty() {
            sampled
        } else {
            layered_joint_pose(joint, index, sampled, layers).or(sampled)
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

/// One side of a character pose: up to three weighted clip samples (a plain clip is one entry of
/// weight 1; a blend-space point set is up to three), each `(sampler, clip time, weight)`.
#[derive(Clone, Copy)]
pub(crate) struct PoseSide<'a> {
    entries: [Option<(&'a ClipSampler, f64, f64)>; 3],
}

/// The outgoing side of a crossfade and the incoming side's share `weight` in `[0, 1)`.
pub(crate) struct PoseFade<'a> {
    pub from: PoseSide<'a>,
    pub weight: f64,
}

impl<'a> PoseSide<'a> {
    /// No clip: the rest pose.
    pub(crate) const EMPTY: PoseSide<'static> = PoseSide { entries: [None; 3] };

    /// A plain clip side, weight 1.
    pub(crate) fn single(side: ClipPose<'a, ClipSampler>) -> Self {
        PoseSide {
            entries: [side.map(|(sampler, t)| (sampler, t, 1.0)), None, None],
        }
    }

    /// A weighted side from up to three entries.
    pub(crate) fn weighted(entries: [Option<(&'a ClipSampler, f64, f64)>; 3]) -> Self {
        PoseSide { entries }
    }

    /// The same entries all sampled at clip time `0`: a layer's own first frame.
    fn at_zero(self) -> Self {
        PoseSide {
            entries: self.entries.map(|entry| entry.map(|(s, _, w)| (s, 0.0, w))),
        }
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.entries.iter().all(Option::is_none)
    }

    pub(crate) fn count(&self) -> usize {
        self.entries.iter().flatten().count()
    }

    /// The side without any sampler bound to a different joint count than `targets` (stale after a
    /// reload that changed the skin).
    fn bound(self, targets: usize) -> Self {
        PoseSide {
            entries: self
                .entries
                .map(|entry| entry.filter(|(sampler, _, _)| sampler.targets() == targets)),
        }
    }

    /// Joint `index`'s sampled channels. One weight-1 entry is the sampler's answer verbatim; more
    /// blend in entry order, rotation by running slerp (`w / accumulated`), translation and scale
    /// by weighted sum, a channel an entry lacks reading as the joint's rest. `None` when no entry
    /// has a track for the joint.
    fn sample(&self, index: usize, joint: &SkinJoint) -> Option<ludion_scene::PartPose> {
        let first = self.entries.iter().flatten().next()?;
        if self.count() == 1 && first.2 == 1.0 {
            return first.0.pose(index, first.1);
        }
        let widen = |v: Vec3| [v.x, v.y, v.z].map(f64::from);
        let rest_q = [
            joint.rest_rotation.x,
            joint.rest_rotation.y,
            joint.rest_rotation.z,
            joint.rest_rotation.w,
        ]
        .map(f64::from);
        let (rest_t, rest_s) = (widen(joint.rest_translation), widen(joint.rest_scale));
        let mut tracked = false;
        let mut accumulated = 0.0;
        let mut rotation = rest_q;
        let mut translation = [0.0; 3];
        let mut scale = [0.0; 3];
        for &(sampler, t, weight) in self.entries.iter().flatten() {
            let sample = sampler.pose(index, t);
            tracked |= sample.is_some();
            let sample = sample.unwrap_or_default();
            let q = sample.rotation.unwrap_or(rest_q);
            let before = accumulated;
            accumulated += weight;
            rotation = if before == 0.0 {
                q
            } else {
                ludion_scene::slerp(rotation, q, weight / accumulated)
            };
            let (v, k) = (
                sample.translation.unwrap_or(rest_t),
                sample.scale.unwrap_or(rest_s),
            );
            for axis in 0..3 {
                translation[axis] += weight * v[axis];
                scale[axis] += weight * k[axis];
            }
        }
        tracked.then_some(ludion_scene::PartPose {
            rotation: Some(rotation),
            translation: Some(translation),
            scale: Some(scale),
        })
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

/// One `AnimationLayers` layer ready to sample: its clip (or blend) at the layer's own clock, the
/// reference an additive layer measures from, the mask's per-joint weights (`None` is the whole skin),
/// how it combines, and its weight now, in `(0, 1]`.
pub(crate) struct LayerPose<'a> {
    pub pose: PoseSide<'a>,
    pub reference: PoseSide<'a>,
    pub mask: Option<&'a [f32]>,
    pub mode: LayerMode,
    pub weight: f64,
}

/// The layers of `id`'s `AnimationLayers` that act at `now` (the held time the base pose reads), in
/// list order. A layer with no weight, an unbound clip or an unknown mask is skipped silently, as a
/// base `Animation` naming an unbound clip is. Empty — without allocating — when the entity has no
/// `AnimationLayers`.
pub(crate) fn layer_poses<'a>(
    world: &World,
    id: EntityId,
    data: &'a CharacterData,
    joint_count: usize,
    now: GameTime,
) -> Vec<LayerPose<'a>> {
    let Some(stack) = get::<AnimationLayers>(world, id) else {
        return Vec::new();
    };
    // A layer blend's phase reads its `BlendClock.layers` entry, advanced by the fraction of a tick
    // `now` is past the clock's tick, else the stateless `elapsed * rate`.
    let clock = get::<BlendClock>(world, id);
    let fraction =
        now.ticks_since(FrameIndex::new(now.ticks_floor())) * now.rate().seconds_per_tick();
    stack
        .layers
        .iter()
        .take(ludion_components::animation_layers::MAX_ANIMATION_LAYERS)
        .filter_map(|layer| {
            let weight = f64::from(layer_weight_at(layer, now));
            if weight <= 0.0 || weight.is_nan() {
                return None;
            }
            let pose = if let Some(binding) = data.clip(&layer.clip) {
                let elapsed = clock_elapsed(now, layer.started, layer.speed, None) * binding.speed;
                PoseSide::single(Some((
                    &binding.sampler,
                    ludion_scene::clip_phase(&binding.clip, elapsed),
                )))
            } else {
                let blend = data.blend(&layer.clip)?;
                let rate = crate::blend_clock::blend_rate(blend, layer.params);
                let live = clock.and_then(|c| {
                    c.layers.iter().find(|entry| {
                        entry.name == layer.name
                            && entry.blend == layer.clip
                            && entry.started == layer.started
                    })
                });
                let phase = live.map_or_else(
                    || crate::blend_clock::layer_fallback_phase(blend, layer, now),
                    |entry| entry.phase + rate * f64::from(layer.speed) * fraction,
                );
                let mut entries = [None; 3];
                for (slot, (index, weight)) in
                    entries.iter_mut().zip(blend.weights(layer.params).iter())
                {
                    let point = &blend.points[usize::from(index)];
                    *slot = Some((
                        &data.clips[point.clip].sampler,
                        blend.point_time(index, phase),
                        weight,
                    ));
                }
                PoseSide::weighted(entries)
            }
            .bound(joint_count);
            let mask = match layer.mask.as_str() {
                "" => None,
                name => Some(data.mask(name)?),
            };
            let reference = match layer.mode {
                LayerMode::Additive if !layer.reference.is_empty() => {
                    PoseSide::single(Some((&data.clip(&layer.reference)?.sampler, 0.0)))
                        .bound(joint_count)
                }
                _ => pose.at_zero(),
            };
            Some(LayerPose {
                pose,
                reference,
                mask,
                mode: layer.mode,
                weight,
            })
        })
        .collect()
}

/// One joint's channels after `layers` compose over its `base` sample, or `None` when no layer
/// reaches the joint (it has no track in the layer clip, or `weight × mask` is zero) — the caller
/// then keeps `base` untouched. Every channel is carried as `f64`, a missing one reading as the
/// joint's rest value.
///
/// Override blends toward the layer's channels by `weight × mask` (exactly 1 takes them verbatim);
/// additive adds the layer's change from its own first frame: rotation `base ⊗ slerp(I, r⁻¹ ⊗ c, k)`,
/// translation `base + k (c − r)`, scale `base × (1 + k (c / r − 1))`.
fn layered_joint_pose(
    joint: &SkinJoint,
    index: usize,
    base: Option<ludion_scene::PartPose>,
    layers: &[LayerPose<'_>],
) -> Option<ludion_scene::PartPose> {
    let widen = |v: Vec3| [v.x, v.y, v.z].map(f64::from);
    let rest_q = [
        joint.rest_rotation.x,
        joint.rest_rotation.y,
        joint.rest_rotation.z,
        joint.rest_rotation.w,
    ]
    .map(f64::from);
    let channels = |pose: Option<ludion_scene::PartPose>| {
        let pose = pose.unwrap_or_default();
        (
            pose.rotation.unwrap_or(rest_q),
            pose.translation
                .unwrap_or_else(|| widen(joint.rest_translation)),
            pose.scale.unwrap_or_else(|| widen(joint.rest_scale)),
        )
    };
    let mut current: Option<([f64; 4], [f64; 3], [f64; 3])> = None;
    for layer in layers {
        let Some(sample) = layer.pose.sample(index, joint) else {
            continue;
        };
        let mask = layer
            .mask
            .map_or(1.0, |m| m.get(index).copied().unwrap_or(0.0));
        let k = layer.weight * f64::from(mask);
        if k <= 0.0 || k.is_nan() {
            continue;
        }
        let (rotation, translation, scale) = current.get_or_insert_with(|| channels(base));
        let (c_rot, c_trans, c_scale) = channels(Some(sample));
        match layer.mode {
            LayerMode::Override if k >= 1.0 => {
                (*rotation, *translation, *scale) = (c_rot, c_trans, c_scale);
            }
            LayerMode::Override => {
                *rotation = ludion_scene::slerp(*rotation, c_rot, k);
                for i in 0..3 {
                    translation[i] += (c_trans[i] - translation[i]) * k;
                    scale[i] += (c_scale[i] - scale[i]) * k;
                }
            }
            LayerMode::Additive => {
                let (r_rot, r_trans, r_scale) = channels(layer.reference.sample(index, joint));
                let delta = quat_mul(quat_conjugate(r_rot), c_rot);
                *rotation = quat_mul(
                    *rotation,
                    ludion_scene::slerp([0.0, 0.0, 0.0, 1.0], delta, k),
                );
                for i in 0..3 {
                    translation[i] += k * (c_trans[i] - r_trans[i]);
                    let ratio = if r_scale[i].abs() < 1e-12 {
                        1.0
                    } else {
                        c_scale[i] / r_scale[i]
                    };
                    scale[i] *= 1.0 + k * (ratio - 1.0);
                }
            }
        }
    }
    current.map(|(rotation, translation, scale)| ludion_scene::PartPose {
        rotation: Some(rotation),
        translation: Some(translation),
        scale: Some(scale),
    })
}

/// The Hamilton product `a ⊗ b` of two `[x, y, z, w]` quaternions (plain arithmetic, no
/// transcendentals).
fn quat_mul(a: [f64; 4], b: [f64; 4]) -> [f64; 4] {
    [
        a[3] * b[0] + a[0] * b[3] + a[1] * b[2] - a[2] * b[1],
        a[3] * b[1] - a[0] * b[2] + a[1] * b[3] + a[2] * b[0],
        a[3] * b[2] + a[0] * b[1] - a[1] * b[0] + a[2] * b[3],
        a[3] * b[3] - a[0] * b[0] - a[1] * b[1] - a[2] * b[2],
    ]
}

/// The conjugate of a unit `[x, y, z, w]` quaternion, which is its inverse.
fn quat_conjugate(q: [f64; 4]) -> [f64; 4] {
    [-q[0], -q[1], -q[2], q[3]]
}

/// The instance rim lane `(color·strength, power)` for a resolved rim; all-zero is no rim.
fn rim_lane(rim: Option<Rim>) -> [f32; 4] {
    match rim {
        Some(r) => [
            r.color[0] * r.strength,
            r.color[1] * r.strength,
            r.color[2] * r.strength,
            r.power,
        ],
        None => [0.0; 4],
    }
}

/// A document's `rim:` packed as an instance rim lane — the arithmetic of `Mesh.rim`, so a document
/// rim and an equal `Mesh.rim` produce identical lanes.
pub(crate) fn doc_rim_lane(doc: &ludion_scene::DocRim) -> [f32; 4] {
    rim_lane(Some(Rim {
        color: doc.color.map(|c| c as f32),
        strength: doc.strength as f32,
        power: doc.power as f32,
    }))
}

/// The lane a mesh instance draws with: its document's rim when it authors one, else the entity's
/// resolved (own or inherited) lane. Primitives have no document, so `asset` is `None` for them.
pub(crate) fn resolve_rim_lane(
    mesh_assets: &[Arc<MeshData>],
    asset: Option<usize>,
    entity: [f32; 4],
) -> [f32; 4] {
    asset
        .and_then(|index| mesh_assets[index].rim)
        .unwrap_or(entity)
}

/// The per-entity values a mesh inherits down `Parent`, resolved once per entity: the dynamic mesh
/// loop and the [`StaticScene`](crate::static_scene::StaticScene) both read them here.
#[derive(Default)]
pub(crate) struct MeshInherit {
    last_fade: Option<(EntityId, crate::components::Fade)>,
    last_rim: Option<(EntityId, [f32; 4], f32)>,
    fade_memo: crate::hierarchy::InheritMemo<Option<crate::components::Fade>>,
    rim_memo: crate::hierarchy::InheritMemo<Option<Rim>>,
    medium_memo: crate::hierarchy::InheritMemo<Option<f32>>,
}

impl MeshInherit {
    /// `(fade, rim lane, medium)` of `mesh`.
    ///
    /// Per-instance fade rides the entity's optional `Fade` component; absent ⇒ the
    /// render-identical default `(1.0, -f32::MAX)`, which `mesh.wgsl`'s screen-door discard can
    /// never reject. A mesh with no `Fade` of its own takes its nearest ancestor's, so a faded
    /// collider root fades its presentation children. Rim and medium resolve like `Fade`: the mesh's
    /// own, else its nearest ancestor's `Mesh` value. Read once per entity: consecutive placed meshes
    /// of one entity (a grid's cells, an assembly's parts) fade as one model.
    pub(crate) fn resolve(
        &mut self,
        world: &World,
        mesh: &PlacedMesh<'_>,
    ) -> (crate::components::Fade, [f32; 4], f32) {
        let fade = match self.last_fade {
            Some((id, fade)) if id == mesh.id => fade,
            _ => {
                let fade = self
                    .fade_memo
                    .resolve(
                        world,
                        mesh.id,
                        |id| {
                            world
                                .get::<crate::components::Fade>(id)
                                .ok()
                                .flatten()
                                .copied()
                        },
                        |parent, own| own.or(parent),
                        None,
                    )
                    .unwrap_or_default();
                self.last_fade = Some((mesh.id, fade));
                fade
            }
        };
        let (rim, medium) = match self.last_rim {
            Some((id, rim, medium)) if id == mesh.id => (rim, medium),
            _ => {
                let own_rim =
                    |id: EntityId| world.get::<Mesh>(id).ok().flatten().and_then(|m| m.rim);
                let own_medium =
                    |id: EntityId| world.get::<Mesh>(id).ok().flatten().and_then(|m| m.medium);
                let rim = rim_lane(mesh.rim.or_else(|| {
                    self.rim_memo
                        .resolve_ancestors(world, mesh.id, own_rim, |p, o| o.or(p), None)
                }));
                let medium = mesh
                    .medium
                    .or_else(|| {
                        self.medium_memo.resolve_ancestors(
                            world,
                            mesh.id,
                            own_medium,
                            |p, o| o.or(p),
                            None,
                        )
                    })
                    .unwrap_or(1.0);
                self.last_rim = Some((mesh.id, rim, medium));
                (rim, medium)
            }
        };
        (fade, rim, medium)
    }
}

/// The instance of one placed mesh with a resolved single `shape`: the build the dynamic mesh loop
/// and the [`StaticScene`](crate::static_scene::StaticScene) share, so the bytes of a static and of
/// the same mesh drawn dynamically cannot diverge.
///
/// Selection, not multiplication: a mesh whose geometry authors its own material uses it entirely;
/// the component's material fields apply only to meshes that author none (primitives, plain asset
/// meshes). Resolved per mesh into the shader's branch-free flag. `rim` is the entity's resolved
/// lane; an asset's own rim wins.
#[allow(clippy::too_many_arguments)]
pub(crate) fn mesh_instance(
    mesh: &PlacedMesh<'_>,
    shape: MeshShape,
    mesh_assets: &[Arc<MeshData>],
    fade: crate::components::Fade,
    rim: [f32; 4],
    medium: f32,
    cast_shadows: bool,
    rain_occluder: bool,
    bone_offset: u32,
) -> MeshInstance {
    let asset = match shape {
        MeshShape::Asset(idx) => Some(idx),
        _ => None,
    };
    MeshInstance {
        shape,
        model: mesh.model,
        normal_matrix: mesh.normal_matrix,
        color: mesh.color,
        material: mesh.material,
        use_vertex_material: asset.is_some_and(|idx| mesh_assets[idx].authored_material),
        fade: fade.fade,
        fade_height_min: fade.height_min,
        flash: mesh.flash,
        flash_color: mesh.flash_color,
        wrap: mesh.wrap,
        scatter_color: mesh.scatter_color,
        bone_offset,
        cast_shadows,
        rain_occluder,
        viewmodel: false,
        rim: resolve_rim_lane(mesh_assets, asset, rim),
        medium,
        porosity: mesh.porosity.unwrap_or(DEFAULT_POROSITY),
    }
}

/// A blended instance: it never casts a shadow, whatever `Visibility` says.
fn blended_mesh(
    instance: MeshInstance,
    mode: MeshBlend,
    lit: bool,
    nearest: bool,
    group: u32,
) -> BlendedMesh {
    BlendedMesh {
        lit,
        nearest,
        instance: MeshInstance {
            cast_shadows: false,
            rain_occluder: false,
            ..instance
        },
        mode: match mode {
            MeshBlend::Alpha => BlendMode::Alpha,
            MeshBlend::Additive => BlendMode::Additive,
        },
        group,
    }
}

/// Resolves which instances draw blended and numbers their owning entities' sort groups.
#[derive(Default)]
struct BlendRoute {
    /// The last entity that pushed a blended instance, and its group.
    group: Option<(EntityId, u32)>,
    viewmodel_reported: Vec<EntityId>,
    lit_reported: Vec<EntityId>,
    depth_reported: Vec<EntityId>,
}

impl BlendRoute {
    /// The mode an instance draws with: the entity's `Mesh.blend`, else `Alpha` when its document
    /// says `alpha_mode: blend`. A blended `Viewmodel` mesh draws opaque, with `L2360` once per entity;
    /// a `lit: false` mesh that draws opaque reports `L2361` once per entity, and a `blend_depth: nearest`
    /// one `L2362`.
    #[allow(clippy::too_many_arguments)]
    fn mode(
        &mut self,
        world: &World,
        id: EntityId,
        own: Option<MeshBlend>,
        document_blend: bool,
        lit: bool,
        blend_depth: BlendDepth,
        viewmodel: bool,
        degraded: &mut Vec<(String, Diagnostic)>,
    ) -> Option<MeshBlend> {
        let mode = own.or(document_blend.then_some(MeshBlend::Alpha));
        if (mode.is_none() || viewmodel)
            && blend_depth == BlendDepth::Nearest
            && !self.depth_reported.contains(&id)
        {
            self.depth_reported.push(id);
            let at = crate::diagnostics::offender_key(world, id);
            degraded.push((
                format!("{at}/blend-depth-on-opaque"),
                Diagnostic::new(
                    code::MESH_BLEND_DEPTH_ON_OPAQUE,
                    format!(
                        "{id} sets `Mesh.blend_depth: nearest`, which only applies to a blended mesh; \
                         it draws opaque"
                    ),
                )
                .with_help("add `blend: alpha`, or remove `blend_depth`"),
            ));
        }
        if (mode.is_none() || viewmodel) && !lit && !self.lit_reported.contains(&id) {
            self.lit_reported.push(id);
            let at = crate::diagnostics::offender_key(world, id);
            degraded.push((
                format!("{at}/lit-on-opaque"),
                Diagnostic::new(
                    code::MESH_LIT_ON_OPAQUE,
                    format!(
                        "{id} sets `Mesh.lit: false`, which only applies to a blended mesh; it draws lit"
                    ),
                )
                .with_help("add `blend: alpha`, or remove `lit`"),
            ));
        }
        let mode = mode?;
        if !viewmodel {
            return Some(mode);
        }
        if !self.viewmodel_reported.contains(&id) {
            self.viewmodel_reported.push(id);
            let at = crate::diagnostics::offender_key(world, id);
            degraded.push((
                format!("{at}/blend-on-viewmodel"),
                Diagnostic::new(
                    code::MESH_BLEND_ON_VIEWMODEL,
                    format!(
                        "{id} is blended (`Mesh.blend` or its document's `alpha_mode: blend`) on a \
                         `Viewmodel` mesh; the mesh draws opaque"
                    ),
                )
                .with_help("remove `blend`, or give the viewmodel an opaque mesh document"),
            ));
        }
        None
    }

    /// The sort group of `id`'s blended instances: consecutive per entity in extract order.
    fn group(&mut self, id: EntityId) -> u32 {
        match self.group {
            Some((last, group)) if last == id => group,
            last => {
                let group = last.map_or(0, |(_, g)| g + 1);
                self.group = Some((id, group));
                group
            }
        }
    }
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
///
/// A camera beside a [`RenderView`](crate::components::RenderView) is a secondary view, not a main
/// camera: it is neither counted nor returned.
pub(crate) fn single_camera(
    world: &World,
    ids: &[EntityId],
) -> Result<Option<EntityId>, Diagnostic> {
    let cameras: Vec<(usize, EntityId)> = ids
        .iter()
        .copied()
        .enumerate()
        .filter(|&(_, id)| {
            world.has::<Camera>(id) && !world.has::<crate::components::RenderView>(id)
        })
        .collect();

    match cameras.as_slice() {
        [] => Ok(None),
        [(_, id)] => Ok(Some(*id)),
        [_, (second_index, _), ..] => Err(Diagnostic::new(
            code::RENDER_MULTIPLE_CAMERAS,
            format!(
                "the scene has {} main cameras (cameras without a `RenderView`), but rendering needs exactly one",
                cameras.len()
            ),
        )
        .with_path(format!("/entities/{second_index}/components/Camera"))
        .with_help("keep one `Camera` without a `RenderView`; a camera for a secondary view takes a `RenderView` (`ludion docs render-views`)")),
    }
}

/// The mesh LOD knobs of the scene's camera ([`Camera::lod_bias`], [`Camera::shadow_bias_levels`]);
/// the defaults when the scene has no single camera. A free `render.view` camera takes them too, so
/// a flown view selects levels as the player's camera would.
fn scene_lod(world: &World, ids: &[EntityId]) -> LodSettings {
    let camera = single_camera(world, ids)
        .ok()
        .flatten()
        .and_then(|id| get::<Camera>(world, id).cloned())
        .unwrap_or_default();
    LodSettings {
        bias: camera.lod_bias.filter(|b| b.is_finite()).unwrap_or(0.0),
        shadow_levels: camera.shadow_bias_levels.unwrap_or(0).min(3),
    }
}

/// Finds the single camera and builds a [`RenderCamera`]; zero cameras → default framing.
///
/// `view` is the host's [`render.view`](crate::render_view) override when one is in force. It
/// replaces the viewpoint and nothing else: the scene's camera is still resolved first, so an
/// ambiguous world is still `L2101`, and its clip planes still shape the override's projection.
pub(crate) fn resolve_camera(
    world: &World,
    ids: &[EntityId],
    viewport: (u32, u32),
    view: Option<&ViewPose>,
) -> Result<RenderCamera, Diagnostic> {
    let scene = single_camera(world, ids)?;
    match (view, scene) {
        (Some(pose), scene) => Ok(crate::render_view::camera(world, scene, pose, viewport)),
        (None, None) => Ok(RenderCamera::default_framing(viewport)),
        (None, Some(id)) => Ok(build_camera(world, ids, id, viewport)),
    }
}

/// The camera the stream draws through for `view` (the host's `render.view` override, or the scene's
/// own camera): what the extract resolves before any per-frame adjustment. `cull_from` holds it.
///
/// # Errors
/// `L2101` when the world has more than one `Camera`.
pub(crate) fn stream_camera(
    world: &World,
    viewport: (u32, u32),
    view: Option<&ViewPose>,
) -> Result<RenderCamera, Diagnostic> {
    let ids: Vec<EntityId> = world
        .entities()
        .chain(world.presentation_entities())
        .collect();
    resolve_camera(world, &ids, viewport, view)
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

/// The rendered camera pose and projection: the rig's `rotation` and `translation` with the active
/// presentation offset composed on, and `cam`'s projection with its `fov_delta`. The one place the
/// offset meets the camera — `build_camera` and the blend's camera both read it, so the drawn frame
/// and the interpolated one cannot disagree. `None` returns its inputs untouched.
pub(crate) fn compose_camera(
    rotation: Quat,
    translation: Vec3,
    cam: &Camera,
    offset: Option<&CameraOffset>,
) -> (Quat, Vec3, Projection) {
    let Some(offset) = offset else {
        return (rotation, translation, cam.projection);
    };
    let (translation, rotation) = offset.pose(translation.to_array(), rotation.to_array());
    (
        Quat::from_array(rotation),
        Vec3::from(translation),
        offset.projection(cam.projection),
    )
}

/// The camera entity's own world model, without any presentation offset: the frame a viewmodel
/// item's local pose is measured in.
pub(crate) fn camera_entity_model(world: &World, id: EntityId) -> Mat4 {
    render_pose(world, &mut WorldAffines::new(), id).model()
}

/// The camera's rig pose with the active presentation offset composed on, and its projection.
fn drawn_pose(
    world: &World,
    ids: &[EntityId],
    id: EntityId,
    cam: &Camera,
) -> (Quat, Vec3, Projection) {
    let pose = render_pose(world, &mut WorldAffines::new(), id);
    let offset = crate::camera_offset::active(world, ids);
    compose_camera(
        pose.quat(),
        Vec3::from(pose.translation()),
        cam,
        offset.as_ref(),
    )
}

/// The drawn camera's world model (camera space to world): the one transform a viewmodel item is
/// composed onto. [`build_camera`] inverts exactly this matrix.
pub(crate) fn drawn_camera_model(world: &World, ids: &[EntityId], id: EntityId) -> Mat4 {
    let cam = get::<Camera>(world, id).cloned().unwrap_or_default();
    let (rotation, translation, _) = drawn_pose(world, ids, id, &cam);
    Mat4::from_rotation_translation(rotation, translation)
}

/// Where a shot is drawn from: the named muzzle's world point on the drawn camera, or `None` (no
/// muzzle named, no `from_muzzle` look, no live shot, or the name unresolved, which latches `L4029`).
///
/// The gun draws through the lens projection and a shot through the world one, so the muzzle's
/// camera-local x and y are rescaled by the focal-length ratio to land on the same screen pixel.
#[allow(clippy::too_many_arguments)]
fn muzzle_point(
    world: &World,
    ids: &[EntityId],
    poses: &mut WorldAffines,
    frame: &crate::viewmodel::FrameViewmodel,
    camera: Option<EntityId>,
    scene: &RenderScene,
    draw: &ProjectileDraw,
    degraded: &mut Vec<(String, Diagnostic)>,
) -> Option<Vec3> {
    let name = draw.visuals.muzzle.as_deref()?;
    let visuals = &draw.visuals;
    if !visuals.fallback.from_muzzle && !visuals.tags.values().any(|v| v.from_muzzle) {
        return None;
    }
    if world
        .resource::<crate::Projectiles>()
        .is_none_or(|res| res.pool().is_empty())
    {
        return None;
    }
    let muzzle = ids
        .iter()
        .copied()
        .find(|&id| get::<crate::components::Name>(world, id).is_some_and(|n| n.0 == name))
        .filter(|&id| frame.contains(id));
    let (Some(cam), Some(muzzle)) = (camera, muzzle) else {
        degraded.push((
            format!("projectile-muzzle/{name}"),
            Diagnostic::new(
                code::PROJECTILE_MUZZLE_UNRESOLVED,
                format!(
                    "projectile muzzle `{name}` is not a `Viewmodel` entity in a frame with a \
                     first-person camera; shots draw at their simulated position"
                ),
            )
            .with_help("`ludion explain L4029`"),
        ));
        return None;
    };
    let cam_inverse = crate::viewmodel::camera_inverse(crate::viewmodel::camera_frame(world, cam));
    let local =
        crate::viewmodel::camera_local(&cam_inverse, render_pose(world, poses, muzzle).model())
            .translation;
    let lens = crate::viewmodel::lens(world, cam).0;
    let lens_focal = Mat4::perspective_rh(lens.fov_y_deg.to_radians(), 1.0, 0.1, 100.0)
        .y_axis
        .y;
    let k = lens_focal / scene.camera.projection.y_axis.y;
    Some(
        drawn_camera_model(world, ids, cam).transform_point3(Vec3::new(
            local[0] * k,
            local[1] * k,
            local[2],
        )),
    )
}

/// Builds the view + projection matrices for the camera entity `id`, with the active presentation
/// offset among `ids` composed onto the rig's pose.
pub(crate) fn build_camera(
    world: &World,
    ids: &[EntityId],
    id: EntityId,
    viewport: (u32, u32),
) -> RenderCamera {
    let cam = get::<Camera>(world, id).cloned().unwrap_or_default();
    let (near, far) = cam.depth_range();
    let (rotation, translation, cam_projection) = drawn_pose(world, ids, id, &cam);
    let model = Mat4::from_rotation_translation(rotation, translation);
    #[allow(clippy::disallowed_methods)] // render-side: GPU matrix, never reaches a hash
    let view = model.inverse();
    let aspect = aspect_ratio(viewport);
    let projection = match cam_projection {
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
pub(crate) fn font_library(world: &World, project_root: AssetRoot<'_>) -> FontLibrary {
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
    spread_px: u32,
}

/// One [`PlacedFont`] per chain face, in chain order; `[None]` for the embedded face.
fn placed_fonts(
    fonts: &FontLibrary,
    name: Option<&str>,
    locale: Option<&str>,
) -> Vec<Option<PlacedFont>> {
    let Some(face) = fonts.table_face(name, locale) else {
        return vec![None];
    };
    fonts
        .chain_paths(name, locale)
        .into_iter()
        .zip(face.chain())
        .map(|((doc, image), face)| {
            let atlas = &face.document().atlas;
            Some(PlacedFont {
                image: image.to_string(),
                doc: doc.to_string(),
                size: [atlas.width, atlas.height],
                sdf: face.document().mode == FontMode::Sdf,
                spread_px: atlas.spread_px.unwrap_or(0),
            })
        })
        .collect()
}

/// What [`resolve_faces`] needs to know about one text component.
struct FaceSite<'a> {
    fonts: &'a FontLibrary,
    name: Option<&'a str>,
    locale: Option<&'a str>,
    texture_site: TextureSite,
    outline: Option<&'a crate::components::TextOutline>,
    size: f32,
    viewport: (u32, u32),
    key: &'a str,
}

/// Resolves the atlas and outline of each chain face in `used`; the rest of the returned vector,
/// indexed by chain face, stays empty.
#[allow(clippy::too_many_arguments)]
fn resolve_faces(
    site: FaceSite<'_>,
    used: &[u8],
    project_root: AssetRoot<'_>,
    assets: &mut dyn SceneAssets,
    textures: &mut Vec<Arc<TextureImage>>,
    path_index: &mut StableMap<String, usize>,
    errors: &mut Vec<Diagnostic>,
    degraded: &mut Vec<(String, Diagnostic)>,
) -> Vec<ui::FaceDraw> {
    let face = site.fonts.face(site.name, site.locale);
    let placed = placed_fonts(site.fonts, site.name, site.locale);
    let mut draws = vec![(None, None); placed.len()];
    for &k in used {
        let k = usize::from(k);
        let Some(chain_face) = face.chain().nth(k) else {
            continue;
        };
        let font = resolve_font(
            project_root,
            placed.get(k).and_then(Option::as_ref),
            site.texture_site,
            assets,
            textures,
            path_index,
            errors,
        );
        let outline = ui::resolve_outline(
            chain_face,
            site.name.unwrap_or("embedded"),
            font.map_or(0, |f| f.spread_px),
            site.outline,
            site.size,
            site.viewport,
            site.key,
            degraded,
        );
        draws[k] = (font, outline);
    }
    draws
}

/// Resolves a face's atlas image to a [`FontDraw`] through the shared texture dedup. A missing or
/// undecodable atlas is an ordinary `L1101`/`L1102` — never a silent fallback to the embedded atlas,
/// whose glyphs the proportional layout was not measured with. An atlas whose size disagrees with
/// the face document is `L1255` and is not drawn: its UVs would sample the wrong texels.
fn resolve_font(
    project_root: AssetRoot<'_>,
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
        spread_px: font.spread_px,
    })
}

/// Loads (or reuses) the texture at `rel` and returns its index, or records a diagnostic and
/// returns `None`. `assets` is how a path becomes an `Arc`-shared [`TextureImage`] (through
/// the [`AssetCache`], which hands every extract the same allocation); `path_index` still
/// dedups within one extract so a texture shared by several sprites is one entry in the
/// `RenderScene`.
fn resolve_texture(
    project_root: AssetRoot<'_>,
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
                diagnostic = diagnostic.append_help(format!("nearest match is `{name}`"));
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
    project_root: AssetRoot<'_>,
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
pub(crate) enum MeshKind {
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
    pub(crate) fn load(
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
    world: &'a World,
    id: EntityId,
    data: &'a AssemblyData,
    now: GameTime,
) -> (AssemblySide<'a>, Option<(AssemblySide<'a>, f64)>) {
    animation_pose_at(world, id, data, now, now, 0.0)
}

/// [`animation_pose`] sampled `back` gameplay seconds before `now`: clip times and the crossfade
/// weight read `then`, and a live blend phase is wound back by `rate · speed · back` on the same
/// params. `back == 0.0` (with `then == now`) is exactly [`animation_pose`].
pub(crate) fn animation_pose_at<'a>(
    world: &'a World,
    id: EntityId,
    data: &'a AssemblyData,
    now: GameTime,
    then: GameTime,
    back: f64,
) -> (AssemblySide<'a>, Option<(AssemblySide<'a>, f64)>) {
    let Some(animation) = get::<Animation>(world, id) else {
        return (AssemblySide::EMPTY, None);
    };
    // A blend's phase reads the entity's `BlendClock` (advanced by the fraction of a tick `now` is
    // past the clock's tick), else the stateless `elapsed * rate`. Unrecorded: the ragdoll switch
    // calls this from a sim command.
    let clock = world.get_unrecorded::<BlendClock>(id).ok().flatten();
    let fraction =
        now.ticks_since(FrameIndex::new(now.ticks_floor())) * now.rate().seconds_per_tick();
    let side = |name: &str, elapsed: f64, phase: Option<(f64, f32)>| {
        if let Some(clip) = data.clip(name) {
            return AssemblySide::single(Some((clip, ludion_scene::clip_phase(clip, elapsed))));
        }
        let Some(blend) = data.blend(name) else {
            return AssemblySide::EMPTY;
        };
        let rate = crate::blend_clock::blend_rate(blend, animation.params);
        let phase = phase.map_or(elapsed * rate, |(phase, speed)| {
            let mut advanced = phase + rate * f64::from(speed) * fraction;
            if back > 0.0 {
                advanced -= rate * f64::from(speed) * back;
            }
            advanced
        });
        AssemblySide::blend(data, blend, animation.params, phase)
    };
    let live = clock.filter(|c| c.blend == animation.clip && c.started == animation.started);
    let pose = side(
        &animation.clip,
        clip_elapsed(animation, then),
        live.map(|c| (c.phase, animation.speed)),
    );
    let outgoing = clock.filter(|c| !c.from_blend.is_empty() && c.from_blend == animation.from);
    // A `PoseSnapshot` is the outgoing side only for the fade it was captured for: an empty `from`
    // and a matching `fade_started` (the fade being active is `fade_weight` returning `Some`).
    let snapshot = get::<PoseSnapshot>(world, id)
        .filter(|s| animation.from.is_empty() && animation.fade_started == s.started);
    let fade = fade_weight(animation, then).map(|weight| {
        let outgoing_side = match snapshot {
            Some(snapshot) => AssemblySide::snapshot(snapshot),
            None => side(
                &animation.from,
                from_clip_elapsed(animation, then),
                outgoing.map(|c| (c.from_phase, animation.from_speed)),
            ),
        };
        (outgoing_side, weight)
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
pub(crate) fn fade_weight(animation: &Animation, now: GameTime) -> Option<f64> {
    let fade = f64::from(animation.fade);
    if !(fade > 0.0 && fade.is_finite()) {
        return None;
    }
    let ticks = now.ticks_since(animation.fade_started).max(0.0);
    let weight = ticks * now.rate().seconds_per_tick() / fade;
    (weight < 1.0).then_some(weight)
}

/// The clip an entity's [`Animation`] selects on a character, with the clip time to sample it at —
/// or `None`, meaning the rest pose. [`animation_pose`]'s twin, silent on every miss for the same
/// reason; the character's per-clip overlay speed multiplies `Animation.speed`.
pub(crate) fn character_pose<'a>(
    world: &World,
    id: EntityId,
    data: &'a CharacterData,
    now: GameTime,
) -> (PoseSide<'a>, Option<PoseFade<'a>>) {
    let Some(animation) = get::<Animation>(world, id) else {
        return (PoseSide::EMPTY, None);
    };
    // A blend's phase reads the entity's `BlendClock` (advanced by the fraction of a tick `now` is
    // past the clock's tick), else the stateless `elapsed * rate`.
    let clock = get::<BlendClock>(world, id);
    let fraction =
        now.ticks_since(FrameIndex::new(now.ticks_floor())) * now.rate().seconds_per_tick();
    let side = |name: &str, elapsed: f64, phase: Option<(f64, f32)>| {
        if let Some(binding) = data.clip(name) {
            return PoseSide::single(Some((
                &binding.sampler,
                ludion_scene::clip_phase(&binding.clip, elapsed * binding.speed),
            )));
        }
        let Some(blend) = data.blend(name) else {
            return PoseSide::EMPTY;
        };
        let rate = crate::blend_clock::blend_rate(blend, animation.params);
        let phase = phase.map_or(elapsed * rate, |(phase, speed)| {
            phase + rate * f64::from(speed) * fraction
        });
        let weights = blend.weights(animation.params);
        let mut entries = [None; 3];
        for (slot, (index, weight)) in entries.iter_mut().zip(weights.iter()) {
            let point = &blend.points[usize::from(index)];
            *slot = Some((
                &data.clips[point.clip].sampler,
                blend.point_time(index, phase),
                weight,
            ));
        }
        PoseSide::weighted(entries)
    };
    let live = clock.filter(|c| c.blend == animation.clip && c.started == animation.started);
    let pose = side(
        &animation.clip,
        clip_elapsed(animation, now),
        live.map(|c| (c.phase, animation.speed)),
    );
    let outgoing = clock.filter(|c| !c.from_blend.is_empty() && c.from_blend == animation.from);
    let fade = fade_weight(animation, now).map(|weight| PoseFade {
        from: side(
            &animation.from,
            from_clip_elapsed(animation, now),
            outgoing.map(|c| (c.from_phase, animation.from_speed)),
        ),
        weight,
    });
    (pose, fade)
}

/// Whether `id`'s `Mesh` is an assembly or a character: its `Animation` then plays that model's
/// clips, and a prefab root carrying one does not also play the prefab's (as `animation_events`).
fn drives_a_model_mesh(world: &World, id: EntityId) -> bool {
    get::<Mesh>(world, id).is_some_and(|mesh| {
        matches!(
            mesh.shape,
            MeshSource::Assembly(_) | MeshSource::Character(_)
        )
    })
}

/// One playing prefab instance: its loaded clips and the sides of its `Animation`, as indices into
/// `data.clips` with the clip time to sample each at.
struct PrefabPlay {
    data: Arc<PrefabClipData>,
    /// The rests of the text the instance was expanded from, when the library still holds it.
    rests: Option<Arc<PartRests>>,
    pose: Option<(usize, f64)>,
    fade: Option<(Option<(usize, f64)>, f64)>,
}

/// Poses the parts of every prefab instance whose root carries an [`Animation`]: each part the
/// selected clip has a track for takes the sampled pose in `poses`, so the part and everything
/// placed below it draw posed. Visual-only like an assembly's clips: no stored `Transform` changes.
///
/// A clip is chosen by game logic at runtime, so every miss (no clip bound under the name, an
/// instance whose prefab will not load) is silent and leaves the rest pose, as for
/// [`animation_pose`].
///
/// Track values are the part's absolute local TRS, a channel the track lacks reading the part's
/// authored rest. The rest is read from the text the instance was expanded from, so editing the
/// prefab file does not shift an instance already live; an instance with no held text (after a
/// restore) reads the file's rest. The sample is applied **relative to that rest**: a part whose
/// stored `Transform` is the authored one takes the sample as is, and one the instance placed or
/// overrode takes `stored * rest^-1 * sample`, so the placement rides on top (exact for uniform
/// scale).
fn prefab_part_poses(
    world: &World,
    ids: &[EntityId],
    assets: &mut dyn SceneAssets,
    now: GameTime,
    poses: &mut WorldAffines,
) {
    let mut plays: BTreeMap<EntityId, PrefabPlay> = BTreeMap::new();
    // The rests of each distinct (snapshot, prefab) text, so N instances cost one hash per frame.
    let mut bound_rests: BTreeMap<(usize, String), Option<Arc<PartRests>>> = BTreeMap::new();
    for &id in ids {
        let Some(animation) = get::<Animation>(world, id) else {
            continue;
        };
        let now = held(animation, rebased_instant(world, id, now));
        let Some(instance) = get::<PrefabInstance>(world, id).filter(|i| i.root == id) else {
            continue;
        };
        if drives_a_model_mesh(world, id) {
            continue;
        }
        let Ok(data) = assets.prefab_clips(&instance.source) else {
            continue;
        };
        let library = world.resource::<crate::prefab::PrefabLibrary>();
        let rests = library
            .and_then(|library| library.expanded_from(id, &instance.source))
            .and_then(|snapshot| {
                bound_rests
                    .entry((snapshot, instance.source.clone()))
                    .or_insert_with(|| {
                        let text = library?.snapshot(snapshot).get(&instance.source)?;
                        assets.prefab_rests(text)
                    })
                    .clone()
            });
        let side = |name: &str, elapsed: f64| {
            let index = data.clips.iter().position(|binding| binding.name == name)?;
            Some((
                index,
                ludion_scene::clip_phase(&data.clips[index].clip, elapsed),
            ))
        };
        let pose = side(&animation.clip, clip_elapsed(animation, now));
        let fade = fade_weight(animation, now).map(|weight| {
            (
                side(&animation.from, from_clip_elapsed(animation, now)),
                weight,
            )
        });
        if pose.is_some() || fade.as_ref().is_some_and(|(from, _)| from.is_some()) {
            plays.insert(
                id,
                PrefabPlay {
                    data: data.clone(),
                    rests,
                    pose,
                    fade,
                },
            );
        }
    }
    if plays.is_empty() {
        return;
    }
    for &id in ids {
        let Some(instance) = get::<PrefabInstance>(world, id) else {
            continue;
        };
        let Some(play) = plays.get(&instance.root) else {
            continue;
        };
        if !world.has::<Transform>(id) {
            continue;
        }
        let stored = transform_of(world, id);
        let rest = play
            .rests
            .as_deref()
            .unwrap_or(&play.data.rests)
            .get(&instance.part)
            .copied()
            .unwrap_or(stored);
        let rest_trs = level_trs(&rest);
        let sample = |side: Option<(usize, f64)>| {
            let (index, t) = side?;
            ludion_scene::joint_pose(&play.data.clips[index].clip, &instance.part, t)
        };
        let sampled = match &play.fade {
            None => sample(play.pose).map(|pose| apply_channels(&rest_trs, &pose)),
            Some((from, weight)) => {
                blended_prefab_part(&rest_trs, sample(*from), sample(play.pose), *weight)
            }
        };
        let Some(sampled) = sampled else {
            continue;
        };
        let posed = if stored == rest {
            sampled
        } else {
            level_trs(&stored)
                .mul_trs(&rest_trs.inverse())
                .mul_trs(&sampled)
        };
        poses.set_posed(id, posed);
    }
}

/// `rest` with each channel `pose` animates replaced.
fn apply_channels(rest: &Trs, pose: &ludion_scene::PartPose) -> Trs {
    let narrow = |v: [f64; 3]| v.map(|c| c as f32);
    Trs {
        translation: pose.translation.map_or(rest.translation, narrow),
        rotation: pose.rotation.map_or(rest.rotation, |q| q.map(|c| c as f32)),
        scale: pose.scale.map_or(rest.scale, narrow),
    }
}

/// [`blended_joint_pose`]'s twin for a prefab part: `from` and `to` blended `weight` of the way to
/// `to`, each missing channel (or side) read as `rest`. `None` when neither side has a track.
fn blended_prefab_part(
    rest: &Trs,
    from: Option<ludion_scene::PartPose>,
    to: Option<ludion_scene::PartPose>,
    weight: f64,
) -> Option<Trs> {
    if from.is_none() && to.is_none() {
        return None;
    }
    let (from, to) = (from.unwrap_or_default(), to.unwrap_or_default());
    let widen = |v: [f32; 3]| v.map(f64::from);
    let lerp3 = |a: Option<[f64; 3]>, b: Option<[f64; 3]>, rest: [f32; 3]| {
        let rest = widen(rest);
        let (a, b) = (a.unwrap_or(rest), b.unwrap_or(rest));
        [0, 1, 2].map(|i| (a[i] + (b[i] - a[i]) * weight) as f32)
    };
    let rest_q = rest.rotation.map(f64::from);
    let rotation = ludion_scene::slerp(
        from.rotation.unwrap_or(rest_q),
        to.rotation.unwrap_or(rest_q),
        weight,
    );
    Some(Trs {
        translation: lerp3(from.translation, to.translation, rest.translation),
        rotation: rotation.map(|c| c as f32),
        scale: lerp3(from.scale, to.scale, rest.scale),
    })
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
    now: GameTime,
) -> Option<String> {
    let atlas = sprite.atlas.as_deref()?;
    sprite.frame.as_deref()?;
    let animation = get::<Animation>(world, id)?;
    if animation.clip.is_empty() {
        return None;
    }
    let now = held(animation, rebased_instant(world, id, now));
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
    now: impl Into<GameTime>,
    started: FrameIndex,
    speed: f32,
    anchor: Option<&ClipAnchor>,
) -> f64 {
    let now = now.into();
    let (base, origin) = match anchor {
        Some(anchor) if anchor.started == started => (anchor.ticks, anchor.frame),
        _ => (0.0, started),
    };
    let played = now.ticks_since(origin).max(0.0);
    base + played * f64::from(speed)
}

/// The clip clock in seconds: [`clock_ticks`] at the sim's seconds-per-tick.
pub(crate) fn clock_elapsed(
    now: impl Into<GameTime>,
    started: FrameIndex,
    speed: f32,
    anchor: Option<&ClipAnchor>,
) -> f64 {
    let now = now.into();
    clock_ticks(now, started, speed, anchor) * now.rate().seconds_per_tick()
}

/// `now` snapped down to the animation's pose grid: the last tick that is a multiple of `hold`
/// (a 0 reads as 1). Applied only where extract poses; events and the ragdoll keep the exact tick.
/// A `hold` of 1 or less is no grid, so the fractional instant passes through; a stop-motion `hold`
/// snaps to a whole tick.
pub(crate) fn held(animation: &Animation, now: impl Into<GameTime>) -> GameTime {
    let now = now.into();
    if animation.hold <= 1 {
        return now;
    }
    GameTime::from_ticks(held_ticks(animation.hold, now.ticks_floor()), now.rate())
}

/// `ticks` snapped down to the last multiple of `hold` (a 0 reads as 1) — the pose grid itself.
pub(crate) fn held_ticks(hold: u32, ticks: u64) -> u64 {
    let hold = u64::from(hold.max(1));
    ticks - ticks % hold
}

/// [`held`] for an entity's own `Animation`; `now` unchanged when it has none.
fn held_for(world: &World, id: EntityId, now: GameTime) -> GameTime {
    let now = rebased_instant(world, id, now);
    get::<Animation>(world, id).map_or(now, |animation| held(animation, now))
}

/// [`clock_ticks`] as a pose reads it at a held `now`: before the frame of an anchor written between two
/// grid ticks, the clock is that anchor's `grid_ticks`, the pose the grid tick showed. Exact-tick reads
/// (`now >= anchor.frame`) never take that branch.
fn pose_clock_ticks(
    now: GameTime,
    started: FrameIndex,
    speed: f32,
    anchor: Option<&ClipAnchor>,
) -> f64 {
    if let Some(&ClipAnchor {
        started: s,
        frame,
        grid_ticks: Some(grid),
        ..
    }) = anchor
        && s == started
        && now.as_q16() < frame.as_u64().saturating_mul(Q16_PER_TICK)
    {
        return grid;
    }
    clock_ticks(now, started, speed, anchor)
}

/// The incoming clip's clock in ticks (before any character binding speed).
pub(crate) fn clip_ticks(animation: &Animation, now: impl Into<GameTime>) -> f64 {
    pose_clock_ticks(
        now.into(),
        animation.started,
        animation.speed,
        animation.anchor.as_ref(),
    )
}

/// Changes `animation`'s playback rate mid-clip without a jump: the clip time reached at `now` is
/// kept (pose and events both read it) and `speed` covers the ticks after it. A negative or
/// non-finite `speed` freezes (`0.0`); the current speed is a no-op, so a per-tick caller does not
/// churn the world hash. Before the clip has played a tick (`now` at or before `started`) the rate
/// simply applies from `started`.
///
/// It takes `now` as [`SimTime`] (`ctx.game_time`), not a frame: a speed change applies on the floor
/// gameplay frame, so under a time scale the pose jumps by the fraction times the speed difference.
/// It moves the incoming side only:
/// during a crossfade the outgoing clip keeps the rate and anchor it had when the fade began.
/// Writing `speed` directly is only right together with a new `started`. Under a `hold`, a call between
/// two steps changes the pose only from the next step.
///
/// ```
/// use ludion_core::time::{SimTime, TickRate};
/// use ludion_engine::{Animation, set_clip_speed};
///
/// let mut animation = Animation { clip: "walk".into(), ..Animation::default() };
/// set_clip_speed(&mut animation, 0.5, SimTime::from_ticks(60, TickRate::DEFAULT));
/// assert_eq!(animation.speed, 0.5);
/// assert_eq!(animation.anchor.unwrap().ticks, 60.0);
/// ```
pub fn set_clip_speed(animation: &mut Animation, speed: f32, now: SimTime) {
    let speed = if speed.is_finite() && speed >= 0.0 {
        speed
    } else {
        0.0
    };
    #[allow(clippy::float_cmp)]
    if speed == animation.speed {
        return;
    }
    let now_ticks = now.ticks();
    let now = GameTime::from(now);
    if now_ticks <= animation.started.as_u64() {
        animation.anchor = None;
    } else {
        let grid = held(animation, now);
        let grid_ticks = (grid != now).then(|| clip_ticks(animation, grid));
        animation.anchor = Some(crate::components::ClipAnchor {
            started: animation.started,
            frame: ludion_core::time::FrameIndex::new(now_ticks),
            ticks: clip_ticks(animation, now),
            grid_ticks,
        });
    }
    animation.speed = speed;
}

/// The incoming clip's elapsed seconds (before any character binding speed).
pub(crate) fn clip_elapsed(animation: &Animation, now: impl Into<GameTime>) -> f64 {
    let now = now.into();
    pose_clock_ticks(
        now,
        animation.started,
        animation.speed,
        animation.anchor.as_ref(),
    ) * now.rate().seconds_per_tick()
}

/// The outgoing clip's elapsed seconds.
pub(crate) fn from_clip_elapsed(animation: &Animation, now: impl Into<GameTime>) -> f64 {
    let now = now.into();
    pose_clock_ticks(
        now,
        animation.from_started,
        animation.from_speed,
        animation.from_anchor.as_ref(),
    ) * now.rate().seconds_per_tick()
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
    part_placements_with(assembly, pose, None, true, &[])
}

/// `id`'s final part placements (assembly frame) as extract draws them at `now`: base clip or blend,
/// crossfade, `hold`, layers, then IK. One sequence for the mesh loop, the part sockets and
/// [`posed_joint_world`](crate::posed_joint_world), so a socket cannot disagree with the part it
/// rides. `model` is the entity's world matrix (the IK goals need it).
pub(crate) fn assembly_part_models(
    world: &World,
    id: EntityId,
    data: &AssemblyData,
    model: Mat4,
    now: GameTime,
) -> Vec<(Mat4, Mat3)> {
    let held_now = held_for(world, id, now);
    let (pose, crossfade) = animation_pose(world, id, data, held_now);
    let layers = assembly_layer_poses(world, id, data, held_now);
    let mut placed = part_placements_layered(data, pose, crossfade, &layers);
    let ik_goals = crate::assembly_ik::assembly_ik_goals(world, id, data, model, held_now);
    if !ik_goals.is_empty() {
        crate::assembly_ik::apply_assembly_ik(data, &ik_goals, &mut placed);
    }
    if data.ground.is_some() {
        crate::assembly_ik::apply_assembly_ground(world, id, data, model, &ik_goals, &mut placed);
    }
    placed
}

/// [`part_placements_faded`] with an entity's `AnimationLayers` composed over the base pose
/// (`Plan/animation-layers.md` decisions 27-29). No layers is exactly [`part_placements_faded`].
pub(crate) fn part_placements_layered(
    assembly: &AssemblyData,
    pose: AssemblySide<'_>,
    fade: Option<(AssemblySide<'_>, f64)>,
    layers: &[AssemblyLayerPose<'_>],
) -> Vec<(Mat4, Mat3)> {
    part_placements_sides(assembly, pose, fade, true, layers)
}

/// [`part_placements`], with `scale` saying whether a clip's scale channel applies: the render path
/// plays it, the ragdoll switch (`crate::ragdoll`, rigid bodies can't scale) does not. Rotation is
/// the shared quaternion sampler's (`ludion_scene::sample_rotation`, libm-only), so render, the
/// model sheet and the ragdoll switch pose a part identically.
///
/// While `fade` is `Some`, each part's two sampled poses blend in quaternion space (rotation through
/// `ludion_scene::slerp`, translation and scale lerp, a channel one side lacks reading as that
/// part's rest value).
///
/// `layers` compose over the base pose per part; a part no layer reaches takes the exact path it
/// would without them, and the rest return applies only when there are no layers either.
pub(crate) fn part_placements_with(
    assembly: &AssemblyData,
    pose: ClipPose<'_>,
    fade: Option<&CrossfadePose<'_>>,
    scale: bool,
    layers: &[AssemblyLayerPose<'_>],
) -> Vec<(Mat4, Mat3)> {
    let fade = fade.map(|fade| (AssemblySide::single(fade.from), fade.weight));
    part_placements_sides(assembly, AssemblySide::single(pose), fade, scale, layers)
}

/// [`part_placements_with`] over [`AssemblySide`]s, each of which may be a blend: the one
/// implementation every wrapper reaches. A side of one weight-1 clip poses exactly as a plain clip.
pub(crate) fn part_placements_sides(
    assembly: &AssemblyData,
    pose: AssemblySide<'_>,
    fade: Option<(AssemblySide<'_>, f64)>,
    scale: bool,
    layers: &[AssemblyLayerPose<'_>],
) -> Vec<(Mat4, Mat3)> {
    if pose.is_empty() && fade.is_none() && layers.is_empty() {
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
        .enumerate()
        .map(|(index, part)| {
            let parent_offset = part
                .parent
                .map_or(Vec3::ZERO, |index| Vec3::from(assembly.parts[index].offset));
            let layered = layered_part_pose(index, &part.name, pose, fade, layers);
            let (rotation, sampled_scale, sampled_translation) = match (layered, fade) {
                (Some(layered), _) => (
                    layered.rotation.map_or(Quat::IDENTITY, quat_from_f64),
                    layered.scale,
                    layered.translation,
                ),
                (None, None) => {
                    if pose.is_empty() {
                        // Layers exist but none reaches this part, and nothing is playing.
                        return Mat4::from_translation(Vec3::from(part.offset) - parent_offset);
                    }
                    let sampled = pose.sample(&part.name);
                    let rotation = sampled.rotation.map_or(Quat::IDENTITY, quat_from_f64);
                    (rotation, sampled.scale, sampled.translation)
                }
                (None, Some(fade)) => blended_part_pose(pose, fade, &part.name),
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

fn quat_from_f64(q: [f64; 4]) -> Quat {
    Quat::from_xyzw(q[0] as f32, q[1] as f32, q[2] as f32, q[3] as f32)
}

/// One part's rotation, scale and translation while a crossfade is in progress: the current and
/// outgoing samples blended `fade.weight` of the way to the current one. A channel one side lacks
/// contributes the part's rest value (identity rotation, zero translation, unit scale); a channel
/// neither side animates stays `None`, so an untouched part keeps its exact rest matrices.
fn blended_part_pose(
    pose: AssemblySide<'_>,
    fade: (AssemblySide<'_>, f64),
    part: &str,
) -> (Quat, Option<[f64; 3]>, Option<[f64; 3]>) {
    let blended = blended_part_pose_f64(fade.0.sample(part), pose.sample(part), fade.1);
    (
        blended.rotation.map_or(Quat::IDENTITY, quat_from_f64),
        blended.scale,
        blended.translation,
    )
}

/// [`blended_part_pose`] before the rotation is cast to `f32`, so a layer composes over the exact
/// `f64` blend.
fn blended_part_pose_f64(
    from: ludion_scene::PartPose,
    to: ludion_scene::PartPose,
    w: f64,
) -> ludion_scene::PartPose {
    let lerp3 = |a: Option<[f64; 3]>, b: Option<[f64; 3]>, rest: f64| match (a, b) {
        (None, None) => None,
        (a, b) => {
            let (a, b) = (a.unwrap_or([rest; 3]), b.unwrap_or([rest; 3]));
            Some([0, 1, 2].map(|i| a[i] + (b[i] - a[i]) * w))
        }
    };
    let rotation = match (from.rotation, to.rotation) {
        (None, None) => None,
        (a, b) => {
            let identity = [0.0, 0.0, 0.0, 1.0];
            Some(ludion_scene::slerp(
                a.unwrap_or(identity),
                b.unwrap_or(identity),
                w,
            ))
        }
    };
    ludion_scene::PartPose {
        rotation,
        scale: lerp3(from.scale, to.scale, 1.0),
        translation: lerp3(from.translation, to.translation, 0.0),
    }
}

/// One `AnimationLayers` layer ready to pose an assembly: its clip or blend at the layer's own clock, the
/// reference an additive layer measures from (clip and time), the mask's per-part weights (`None` is
/// every part), how it combines, and its weight now, in `(0, 1]`.
pub(crate) struct AssemblyLayerPose<'a> {
    pub side: AssemblySide<'a>,
    pub reference: Option<(&'a ludion_scene::AnimationDocument, f64)>,
    pub mask: Option<&'a [f32]>,
    pub mode: LayerMode,
    pub weight: f64,
}

/// The layers of `id`'s `AnimationLayers` that act on an assembly at `now` (the held time the base
/// pose reads), in list order. A layer with no weight, an unbound clip or blend, an unknown mask or an
/// unknown `reference` is skipped silently, as a base `Animation` naming an unbound clip is. A blend
/// layer's phase reads its `BlendClock.layers` entry, else the stateless fallback. Empty — without
/// allocating — when the entity has no `AnimationLayers`.
pub(crate) fn assembly_layer_poses<'a>(
    world: &World,
    id: EntityId,
    data: &'a AssemblyData,
    now: GameTime,
) -> Vec<AssemblyLayerPose<'a>> {
    assembly_layer_poses_at(world, id, data, now, now, 0.0)
}

/// [`assembly_layer_poses`] sampled `back` gameplay seconds before `now`, as [`animation_pose_at`]
/// does for the base pose: clip layers, weights and the stateless fallback read `then`, a live blend
/// layer's phase is wound back by `rate · speed · back`.
pub(crate) fn assembly_layer_poses_at<'a>(
    world: &World,
    id: EntityId,
    data: &'a AssemblyData,
    now: GameTime,
    then: GameTime,
    back: f64,
) -> Vec<AssemblyLayerPose<'a>> {
    let Some(stack) = get::<AnimationLayers>(world, id) else {
        return Vec::new();
    };
    let clock = world.get_unrecorded::<BlendClock>(id).ok().flatten();
    let fraction =
        now.ticks_since(FrameIndex::new(now.ticks_floor())) * now.rate().seconds_per_tick();
    stack
        .layers
        .iter()
        .take(ludion_components::animation_layers::MAX_ANIMATION_LAYERS)
        .filter_map(|layer| {
            let live = clock.and_then(|c| {
                c.layers.iter().find(|entry| {
                    entry.name == layer.name
                        && entry.blend == layer.clip
                        && entry.started == layer.started
                })
            });
            let live_phase = live.map(|entry| {
                let rate = data.blend(&layer.clip).map_or(0.0, |blend| {
                    crate::blend_clock::blend_rate(blend, layer.params)
                });
                let mut advanced = entry.phase + rate * f64::from(layer.speed) * fraction;
                if back > 0.0 {
                    advanced -= rate * f64::from(layer.speed) * back;
                }
                advanced
            });
            assembly_layer_pose(data, layer, then, live_phase)
        })
        .collect()
}

/// One layer ready to pose `data` at `now`, or `None` when it has no weight, names no bound clip or
/// blend, or an unknown mask or reference. A blend layer plays `live_phase` (its `BlendClock` entry,
/// already advanced), else the stateless `elapsed * rate`.
pub(crate) fn assembly_layer_pose<'a>(
    data: &'a AssemblyData,
    layer: &AnimationLayer,
    now: GameTime,
    live_phase: Option<f64>,
) -> Option<AssemblyLayerPose<'a>> {
    let weight = f64::from(layer_weight_at(layer, now));
    if weight <= 0.0 || weight.is_nan() {
        return None;
    }
    let side = if let Some(clip) = data.clip(&layer.clip) {
        let t =
            ludion_scene::clip_phase(clip, clock_elapsed(now, layer.started, layer.speed, None));
        AssemblySide::single(Some((clip, t)))
    } else {
        let blend = data.blend(&layer.clip)?;
        let phase = live_phase
            .unwrap_or_else(|| crate::blend_clock::layer_fallback_phase(blend, layer, now));
        AssemblySide::blend(data, blend, layer.params, phase)
    };
    let mask = match layer.mask.as_str() {
        "" => None,
        name => Some(data.mask(name)?),
    };
    let reference = match layer.mode {
        LayerMode::Additive if !layer.reference.is_empty() => {
            Some((data.clip(&layer.reference)?, 0.0))
        }
        _ => None,
    };
    Some(AssemblyLayerPose {
        side,
        reference,
        mask,
        mode: layer.mode,
        weight,
    })
}

/// Part `index`'s base channels with `layers` composed over them, or `None` when no layer reaches the
/// part (the layer clip has no track for it, or `weight × mask` is zero) — the caller then takes the
/// exact base path. Channels are `f64`, cast where the base path casts.
///
/// Override blends each channel the layer clip keys toward the layer by `k = weight × mask`
/// (verbatim at `k ≥ 1`); a channel it does not key keeps the base. Additive measures from the part's
/// rest — a rigid part's sample already is its delta from rest — or from the reference clip's first
/// frame: `R = R_base · slerp(I, r⁻¹ c, k)`, `T = T_base + k (c − r)`, `S = S_base · (1 + k (c / r − 1))`.
fn layered_part_pose(
    index: usize,
    name: &str,
    pose: AssemblySide<'_>,
    fade: Option<(AssemblySide<'_>, f64)>,
    layers: &[AssemblyLayerPose<'_>],
) -> Option<ludion_scene::PartPose> {
    let mut current: Option<ludion_scene::PartPose> = None;
    for layer in layers {
        let sample = layer.side.sample(name);
        if sample.rotation.is_none() && sample.translation.is_none() && sample.scale.is_none() {
            continue;
        }
        let mask = layer
            .mask
            .map_or(1.0, |m| m.get(index).copied().unwrap_or(0.0));
        let k = layer.weight * f64::from(mask);
        if k <= 0.0 || k.is_nan() {
            continue;
        }
        let base = current.get_or_insert_with(|| match fade {
            Some((from, weight)) => {
                blended_part_pose_f64(from.sample(name), pose.sample(name), weight)
            }
            None => pose.sample(name),
        });
        let identity = [0.0, 0.0, 0.0, 1.0];
        match layer.mode {
            LayerMode::Override => {
                if let Some(c) = sample.rotation {
                    base.rotation = Some(if k >= 1.0 {
                        c
                    } else {
                        ludion_scene::slerp(base.rotation.unwrap_or(identity), c, k)
                    });
                }
                if let Some(c) = sample.translation {
                    let b = base.translation.unwrap_or([0.0; 3]);
                    base.translation = Some(if k >= 1.0 {
                        c
                    } else {
                        [0, 1, 2].map(|i| b[i] + (c[i] - b[i]) * k)
                    });
                }
                if let Some(c) = sample.scale {
                    let b = base.scale.unwrap_or([1.0; 3]);
                    base.scale = Some(if k >= 1.0 {
                        c
                    } else {
                        [0, 1, 2].map(|i| b[i] + (c[i] - b[i]) * k)
                    });
                }
            }
            LayerMode::Additive => {
                let reference = layer
                    .reference
                    .map(|(clip, time)| ludion_scene::part_pose(clip, name, time))
                    .unwrap_or_default();
                if let Some(c) = sample.rotation {
                    let r = reference.rotation.unwrap_or(identity);
                    let delta = quat_mul(quat_conjugate(r), c);
                    base.rotation = Some(quat_mul(
                        base.rotation.unwrap_or(identity),
                        ludion_scene::slerp(identity, delta, k),
                    ));
                }
                if let Some(c) = sample.translation {
                    let r = reference.translation.unwrap_or([0.0; 3]);
                    let b = base.translation.unwrap_or([0.0; 3]);
                    base.translation = Some([0, 1, 2].map(|i| b[i] + k * (c[i] - r[i])));
                }
                if let Some(c) = sample.scale {
                    let r = reference.scale.unwrap_or([1.0; 3]);
                    let b = base.scale.unwrap_or([1.0; 3]);
                    base.scale = Some([0, 1, 2].map(|i| {
                        let ratio = if r[i].abs() < 1e-12 { 1.0 } else { c[i] / r[i] };
                        b[i] * (1.0 + k * (ratio - 1.0))
                    }));
                }
            }
        }
    }
    current
}

/// One side of an assembly pose: up to three weighted clip samples (a plain clip is one entry of
/// weight 1; a blend space's active points are up to three), each `(clip, clip time, weight)`.
#[derive(Clone, Copy)]
pub(crate) struct AssemblySide<'a> {
    entries: [Option<(&'a ludion_scene::AnimationDocument, f64, f64)>; 3],
    /// A captured pose standing in for the clips (a ragdoll folded back into its assembly).
    snapshot: Option<&'a PoseSnapshot>,
}

impl<'a> AssemblySide<'a> {
    /// No clip: the rest pose.
    pub(crate) const EMPTY: AssemblySide<'static> = AssemblySide {
        entries: [None; 3],
        snapshot: None,
    };

    /// A plain clip side, weight 1.
    pub(crate) fn single(side: ClipPose<'a>) -> Self {
        AssemblySide {
            entries: [side.map(|(clip, t)| (clip, t, 1.0)), None, None],
            snapshot: None,
        }
    }

    /// A weighted side from up to three entries.
    pub(crate) fn weighted(
        entries: [Option<(&'a ludion_scene::AnimationDocument, f64, f64)>; 3],
    ) -> Self {
        AssemblySide {
            entries,
            snapshot: None,
        }
    }

    /// The side a [`PoseSnapshot`] holds: each part it names at its captured channels.
    pub(crate) fn snapshot(snapshot: &'a PoseSnapshot) -> Self {
        AssemblySide {
            entries: [None; 3],
            snapshot: Some(snapshot),
        }
    }

    /// The side of `blend` at `params` on shared `phase`: one entry per active point, each clip at
    /// the time the point plays on that phase.
    pub(crate) fn blend(
        data: &'a AssemblyData,
        blend: &crate::blend_space::BlendSpaceData,
        params: [f32; 2],
        phase: f64,
    ) -> Self {
        let mut entries = [None; 3];
        for (slot, (index, weight)) in entries.iter_mut().zip(blend.weights(params).iter()) {
            let point = &blend.points[usize::from(index)];
            *slot = Some((
                &data.clips[point.clip].clip,
                blend.point_time(index, phase),
                weight,
            ));
        }
        AssemblySide::weighted(entries)
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.snapshot.is_none() && self.entries.iter().all(Option::is_none)
    }

    /// The part's sampled channels. One weight-1 entry is that clip's `part_pose` verbatim; more
    /// blend in entry order, rotation by running slerp (`w / accumulated`, a missing rotation read
    /// as identity), translation and scale by weighted sum (missing read as 0 and 1). A channel no
    /// entry keys stays `None`, so an untouched part keeps its exact rest. A snapshot side returns
    /// its captured rotation and translation for a part it names, and the rest for one it doesn't.
    pub(crate) fn sample(&self, part: &str) -> ludion_scene::PartPose {
        if let Some(snapshot) = self.snapshot {
            return snapshot
                .parts
                .iter()
                .find(|captured| captured.part == part)
                .map_or_else(ludion_scene::PartPose::default, |captured| {
                    ludion_scene::PartPose {
                        rotation: Some(captured.rotation.map(f64::from)),
                        translation: Some(captured.translation.map(f64::from)),
                        scale: None,
                    }
                });
        }
        let Some(first) = self.entries.iter().flatten().next() else {
            return ludion_scene::PartPose::default();
        };
        if self.entries.iter().flatten().count() == 1 && first.2 == 1.0 {
            return ludion_scene::part_pose(first.0, part, first.1);
        }
        let identity = [0.0, 0.0, 0.0, 1.0];
        let mut keyed = [false; 3];
        let mut accumulated = 0.0;
        let mut rotation = identity;
        let mut translation = [0.0; 3];
        let mut scale = [0.0; 3];
        for &(clip, t, weight) in self.entries.iter().flatten() {
            let sample = ludion_scene::part_pose(clip, part, t);
            keyed[0] |= sample.rotation.is_some();
            keyed[1] |= sample.translation.is_some();
            keyed[2] |= sample.scale.is_some();
            let q = sample.rotation.unwrap_or(identity);
            let before = accumulated;
            accumulated += weight;
            rotation = if before == 0.0 {
                q
            } else {
                ludion_scene::slerp(rotation, q, weight / accumulated)
            };
            let (v, k) = (
                sample.translation.unwrap_or([0.0; 3]),
                sample.scale.unwrap_or([1.0; 3]),
            );
            for axis in 0..3 {
                translation[axis] += weight * v[axis];
                scale[axis] += weight * k[axis];
            }
        }
        ludion_scene::PartPose {
            rotation: keyed[0].then_some(rotation),
            translation: keyed[1].then_some(translation),
            scale: keyed[2].then_some(scale),
        }
    }
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
/// `lantern`) suggests here exactly as it does for a misspelled component or field name. A file that
/// exists but failed to load is not a typo, so it gets no suggestion.
pub(crate) fn suggest_asset(project_root: AssetRoot<'_>, rel: &str) -> Option<String> {
    let joined = project_root.dir().join(rel);
    if joined.exists() {
        return None;
    }
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
    use std::path::Path;

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
                ludion_assets::AssetRoot::content(&root),
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
            ludion_assets::AssetRoot::content(&root),
            &mut CachedAssets {
                root: ludion_assets::AssetRoot::content(&root),
                cache: &mut AssetCache::new(),
            },
            tick_zero(),
            ludion_renderer::QualityParams::FULL.particle_density,
            None,
            &mut degraded,
            None,
            None,
            None,
        )
        .expect("the world extracts");

        assert_eq!(scene.sprites.len(), 1);
        assert_eq!(scene.meshes.len(), 0);
        let reported: Vec<_> = degraded
            .iter()
            .inspect(|(_, d)| assert_eq!(d.code, code::RENDER_PLACEMENT_NON_FINITE))
            .map(|(key, d)| (key.as_str(), d.path.as_deref()))
            .collect();
        let sprite_key = format!(
            "{}/sprite",
            crate::diagnostics::offender_key(&world, nan_sprite)
        );
        let mesh_key = format!(
            "{}/mesh",
            crate::diagnostics::offender_key(&world, inf_mesh)
        );
        assert_eq!(
            reported,
            [
                (sprite_key.as_str(), Some("/entities/0/components/Sprite")),
                (mesh_key.as_str(), Some("/entities/2/components/Mesh"))
            ]
        );
    }

    /// `L2109` latches per entity, not per scene-walk position: once A is reported, B going
    /// non-finite at the walk index A vacated (A and a spare are despawned in LIFO order, so B takes
    /// the spare's slot) is a new offender, and A is not reported twice.
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
        let spare = engine.world_mut().spawn();
        let logged = reported_after_extract(&mut engine);
        assert_eq!(logged.len(), 1, "{logged:?}");
        assert!(logged[0].contains(&a.to_string()), "{logged:?}");
        assert!(!logged[0].contains("  "), "{logged:?}");

        engine.world_mut().despawn(a).unwrap();
        engine.world_mut().despawn(spare).unwrap();
        let b = spawn_nan_sprite(&mut engine);
        assert_ne!(a.index(), b.index());
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

    /// A clip with a rotation track on both joints of [`two_joint_rig`].
    fn both_joints_clip(degrees: f64) -> AnimationDocument {
        let track = |target: &str| AnimationTrack {
            target: target.to_string(),
            rotation: vec![linear(0.0, [0.0; 3]), linear(1.0, [0.0, 0.0, degrees])],
            translation: Vec::new(),
            scale: Vec::new(),
        };
        AnimationDocument {
            duration: 1.0,
            tracks: vec![track("root"), track("child")],
            ..Default::default()
        }
    }

    fn bits(m: Mat4) -> [u32; 16] {
        m.to_cols_array().map(f32::to_bits)
    }

    fn layer<'a>(
        sampler: &'a ClipSampler,
        t: f64,
        mode: LayerMode,
        weight: f64,
        mask: Option<&'a [f32]>,
    ) -> LayerPose<'a> {
        LayerPose {
            pose: PoseSide::single(Some((sampler, t))),
            reference: PoseSide::single(Some((sampler, 0.0))),
            mask,
            mode,
            weight,
        }
    }

    /// No layers is today's path, and a layer whose clip has no track for a joint leaves that joint's
    /// base entry bit-identical.
    #[test]
    fn layers_that_do_not_reach_a_joint_leave_it_bit_identical() {
        let joints = two_joint_rig();
        let base = both_joints_clip(60.0);
        let base_sampler = sampler_for(&joints, &base);
        let mut want = Vec::new();
        sampled_palette(
            &joints,
            Some((&base_sampler, 0.4)),
            None,
            &mut Vec::new(),
            &mut want,
        );
        let mut empty = Vec::new();
        sampled_palette_layered(
            &joints,
            Some((&base_sampler, 0.4)),
            None,
            &[],
            &mut Vec::new(),
            &mut empty,
        );
        assert_eq!(
            empty.iter().map(|m| bits(*m)).collect::<Vec<_>>(),
            want.iter().map(|m| bits(*m)).collect::<Vec<_>>()
        );

        // The layer clip tracks only `child`: `root` is untouched.
        let layer_clip = sampled_clip(
            "child",
            vec![linear(0.0, [0.0; 3]), linear(1.0, [0.0, 90.0, 0.0])],
            Vec::new(),
        );
        let layer_sampler = sampler_for(&joints, &layer_clip);
        let mut layered = Vec::new();
        sampled_palette_layered(
            &joints,
            Some((&base_sampler, 0.4)),
            None,
            &[layer(&layer_sampler, 0.5, LayerMode::Override, 1.0, None)],
            &mut Vec::new(),
            &mut layered,
        );
        assert_eq!(bits(layered[0]), bits(want[0]));
        assert_ne!(bits(layered[1]), bits(want[1]));
    }

    /// A weight-1 override over the whole skin draws exactly the layer clip played alone.
    #[test]
    fn a_full_override_layer_equals_the_clip_played_alone() {
        let joints = two_joint_rig();
        let base = both_joints_clip(60.0);
        let over = both_joints_clip(-40.0);
        let (base_sampler, over_sampler) =
            (sampler_for(&joints, &base), sampler_for(&joints, &over));
        let mut alone = Vec::new();
        sampled_palette(
            &joints,
            Some((&over_sampler, 0.7)),
            None,
            &mut Vec::new(),
            &mut alone,
        );
        let mut layered = Vec::new();
        sampled_palette_layered(
            &joints,
            Some((&base_sampler, 0.2)),
            None,
            &[layer(
                &over_sampler,
                0.7,
                LayerMode::Override,
                1.0,
                Some(&[1.0, 1.0]),
            )],
            &mut Vec::new(),
            &mut layered,
        );
        assert_eq!(
            layered.iter().map(|m| bits(*m)).collect::<Vec<_>>(),
            alone.iter().map(|m| bits(*m)).collect::<Vec<_>>()
        );
    }

    /// A mask weight of zero keeps a joint on the base pose; half blends halfway.
    #[test]
    fn an_override_layer_follows_the_mask() {
        let joints = two_joint_rig();
        let base = both_joints_clip(0.0);
        let over = both_joints_clip(90.0);
        let (base_sampler, over_sampler) =
            (sampler_for(&joints, &base), sampler_for(&joints, &over));
        let mask = [0.0_f32, 0.5];
        let mut out = Vec::new();
        sampled_palette_layered(
            &joints,
            Some((&base_sampler, 1.0)),
            None,
            &[layer(
                &over_sampler,
                1.0,
                LayerMode::Override,
                1.0,
                Some(&mask),
            )],
            &mut Vec::new(),
            &mut out,
        );
        assert!(close(out[0], joints[0].rest));
        // Child: halfway to 90 degrees about Z, on top of its rest translation.
        let want = Mat4::from_rotation_translation(
            Quat::from_rotation_z(45f32.to_radians()),
            Vec3::new(0.0, 1.0, 0.0),
        );
        assert!(close(out[1], want), "{:?}", out[1]);
    }

    /// An additive layer sampled at its own first frame adds nothing; later it adds its change.
    #[test]
    fn an_additive_layer_at_its_first_frame_is_the_base() {
        let joints = two_joint_rig();
        let base = both_joints_clip(30.0);
        let add = both_joints_clip(40.0);
        let (base_sampler, add_sampler) = (sampler_for(&joints, &base), sampler_for(&joints, &add));
        let mut plain = Vec::new();
        sampled_palette(
            &joints,
            Some((&base_sampler, 1.0)),
            None,
            &mut Vec::new(),
            &mut plain,
        );
        let mut at_zero = Vec::new();
        sampled_palette_layered(
            &joints,
            Some((&base_sampler, 1.0)),
            None,
            &[layer(&add_sampler, 0.0, LayerMode::Additive, 1.0, None)],
            &mut Vec::new(),
            &mut at_zero,
        );
        for (a, b) in at_zero.iter().zip(&plain) {
            assert!(a.abs_diff_eq(*b, 1e-5), "{a:?} vs {b:?}");
        }
        // At the end of the layer clip its 40 degrees stack on the base's 30 (root yaw about Z).
        let mut stacked = Vec::new();
        sampled_palette_layered(
            &joints,
            Some((&base_sampler, 1.0)),
            None,
            &[layer(&add_sampler, 1.0, LayerMode::Additive, 1.0, None)],
            &mut Vec::new(),
            &mut stacked,
        );
        let want = Mat4::from_quat(Quat::from_rotation_z(70f32.to_radians()));
        assert!(stacked[0].abs_diff_eq(want, 1e-4), "{:?}", stacked[0]);
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

    #[allow(clippy::disallowed_methods)] // render-side: test rig, never reaches a hash
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
                            ..Camera::default()
                        },
                    )
                    .unwrap();
                let camera = build_camera(&world, &[id], id, (320, 180));
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
        assert_eq!(data.cascades, None, "an unset sun carries no cascades");
    }

    /// An authored cascade request travels to the renderer; Rust-built bad values are sanitized.
    #[test]
    fn sun_carries_the_sanitized_cascade_config() {
        let sun = DirectionalLight {
            cascades: Some(4),
            shadow_distance: 60.0,
            cascade_splits: Some(vec![0.06, 0.15, 0.4]),
            ..DirectionalLight::default()
        };
        assert_eq!(
            build_sun(&sun, Vec3::ZERO).cascades,
            Some(SunCascadeConfig {
                count: 4,
                shadow_distance: 60.0,
                splits: Some([0.06, 0.15, 0.4]),
                shadow_near: 1.0,
                shadow_far: 200.0,
            })
        );

        let bad = DirectionalLight {
            cascades: Some(0),
            shadow_distance: -5.0,
            cascade_splits: Some(vec![0.5, 0.2]),
            ..DirectionalLight::default()
        };
        assert_eq!(
            build_sun(&bad, Vec3::ZERO).cascades,
            Some(SunCascadeConfig {
                count: 1,
                shadow_distance: 100.0,
                splits: None,
                shadow_near: 1.0,
                shadow_far: 200.0,
            })
        );
    }

    /// `angular_radius` reaches the renderer in radians; a value outside `[0, 45]` degrees falls back
    /// to a point light.
    #[test]
    fn sun_angular_radius_is_radians_and_falls_back_to_zero() {
        let radius = |degrees: f32| {
            let sun = DirectionalLight {
                angular_radius: degrees,
                ..DirectionalLight::default()
            };
            build_sun(&sun, Vec3::ZERO).angular_radius
        };
        assert_eq!(radius(0.5), 0.5f32.to_radians());
        assert_eq!(radius(0.0), 0.0);
        for bad in [-1.0, 46.0, f32::NAN, f32::INFINITY] {
            assert_eq!(radius(bad), 0.0);
        }
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
                ..PointLight::default()
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

    /// `scatter` is carried onto both local lights: default `0`, an authored value passes through,
    /// and a negative Rust-built one clamps to `0`.
    #[test]
    fn scatter_is_carried_and_clamped_to_non_negative() {
        let point = |scatter| PointLight {
            scatter,
            ..PointLight::default()
        };
        let spot = |scatter| SpotLight {
            scatter,
            ..SpotLight::default()
        };
        let p = |l: &PointLight| point_light_data(l, Vec3::ZERO, false).unwrap().scatter;
        let s = |l: &SpotLight| spot_light_data(l, Vec3::ZERO).unwrap().scatter;
        assert_eq!(p(&PointLight::default()), 0.0);
        assert_eq!(s(&SpotLight::default()), 0.0);
        assert_eq!(p(&point(0.75)), 0.75);
        assert_eq!(s(&spot(0.75)), 0.75);
        assert_eq!(p(&point(-1.0)), 0.0);
        assert_eq!(s(&spot(-1.0)), 0.0);
    }

    /// Only a casting light carries shadow data: the authored bias and near pass through, and a
    /// Rust-built bad near or NaN bias falls back instead of reaching the renderer.
    #[test]
    fn local_lights_carry_shadow_data_only_when_casting() {
        let point_shadow =
            |light: &PointLight| point_light_data(light, Vec3::ZERO, false).unwrap().shadow;
        assert_eq!(point_shadow(&PointLight::default()), None);
        let casting = PointLight {
            cast_shadows: true,
            shadow_bias: 0.1,
            shadow_near: 0.2,
            ..PointLight::default()
        };
        let want = Some(LocalShadowData {
            bias: 0.1,
            near: 0.2,
            distance: None,
            fade: 0.0,
        });
        assert_eq!(point_shadow(&casting), want);
        let spot = SpotLight {
            cast_shadows: true,
            shadow_bias: 0.1,
            shadow_near: 0.2,
            ..SpotLight::default()
        };
        assert_eq!(spot_light_data(&spot, Vec3::ZERO).unwrap().shadow, want);
        let plain = spot_light_data(&SpotLight::default(), Vec3::ZERO).unwrap();
        assert_eq!(plain.shadow, None);

        for near in [-1.0, 0.0, 10.0, 12.0, f32::NAN] {
            let bad = PointLight {
                cast_shadows: true,
                shadow_near: near,
                ..PointLight::default()
            };
            assert_eq!(point_shadow(&bad).unwrap().near, 0.05, "near {near}");
        }
        let tiny = PointLight {
            cast_shadows: true,
            range: 0.04,
            shadow_near: 1.0,
            ..PointLight::default()
        };
        assert_eq!(point_shadow(&tiny).unwrap().near, 0.02);
        let nan_bias = PointLight {
            cast_shadows: true,
            shadow_bias: f32::NAN,
            ..PointLight::default()
        };
        assert_eq!(point_shadow(&nan_bias).unwrap().bias, 0.0);

        let faded = |distance: Option<f32>, fade: f32| {
            point_shadow(&PointLight {
                cast_shadows: true,
                shadow_distance: distance,
                shadow_fade: fade,
                ..PointLight::default()
            })
            .unwrap()
        };
        let ok = faded(Some(30.0), 8.0);
        assert_eq!((ok.distance, ok.fade), (Some(30.0), 8.0));
        let spot = SpotLight {
            cast_shadows: true,
            shadow_distance: Some(30.0),
            shadow_fade: 8.0,
            ..SpotLight::default()
        };
        let spot = spot_light_data(&spot, Vec3::ZERO).unwrap().shadow.unwrap();
        assert_eq!((spot.distance, spot.fade), (Some(30.0), 8.0));
        for bad in [0.0, -2.0, f32::NAN, f32::INFINITY] {
            let data = faded(Some(bad), 5.0);
            assert_eq!((data.distance, data.fade), (None, 0.0), "distance {bad}");
        }
        assert_eq!(faded(None, 5.0).fade, 0.0);
        assert_eq!(faded(Some(10.0), 40.0).fade, 10.0);
        assert_eq!(faded(Some(10.0), -3.0).fade, 0.0);
        assert_eq!(faded(Some(10.0), f32::NAN).fade, 0.0);
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

    fn game_tick(ticks: u64) -> GameTime {
        GameTime::from_ticks(ticks, TickRate::DEFAULT)
    }

    /// Animated grain's tick is gameplay time: a running clock yields consecutive ticks and a held
    /// one yields the same tick at two sim times. Static grain carries none.
    #[test]
    fn grain_tick_follows_the_game_clock() {
        use crate::components::PostProcess;
        use ludion_ecs::World;

        let mut world = World::new();
        let entity = world.spawn();
        world
            .insert(
                entity,
                PostProcess {
                    grain: 0.05,
                    grain_motion: GrainMotion::Animated,
                    ..PostProcess::default()
                },
            )
            .unwrap();
        let ids: Vec<EntityId> = world.entities().collect();
        let tick = |now: GameTime| extract_post(&world, &ids, now).unwrap().grain_tick;
        assert_eq!(tick(game_tick(4)), Some(4));
        assert_eq!(tick(game_tick(5)), Some(5));

        // One held frame: sim tick 6 reads as the game tick 5 the unheld clock gave at sim tick 5.
        let running = GameClock::default();
        let mut held = GameClock::default();
        held.hold();
        held.advance();
        let sim = |ticks: u64| SimTime::from_ticks(ticks, TickRate::DEFAULT);
        assert_eq!(tick(running.game_instant(sim(5))), Some(5));
        assert_eq!(tick(held.game_instant(sim(6))), Some(5));
        assert_eq!(tick(held.game_instant(sim(7))), Some(6));

        world
            .insert(
                entity,
                PostProcess {
                    grain: 0.05,
                    ..PostProcess::default()
                },
            )
            .unwrap();
        assert_eq!(
            extract_post(&world, &ids, game_tick(4)).unwrap().grain_tick,
            None
        );
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
        let post = extract_post(&world, &ids, game_tick(0)).expect("a post-process is present");
        assert_eq!(
            post.bloom_strength, 0.25,
            "the lowest-EntityId PostProcess wins"
        );
        assert_eq!(post.bloom_threshold, 1.0);

        let empty = World::new();
        let empty_ids: Vec<EntityId> = empty.entities().collect();
        assert!(extract_post(&empty, &empty_ids, game_tick(0)).is_none());
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
            extract_post(&world, &ids, game_tick(0)),
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
                    exposure_mode: ExposureMode::Metered,
                    exposure_compensation: -0.5,
                    metering: Metering::Average,
                    metering_clip: [0.05, 0.1],
                    exposure_range: [-3.0, 1.0],
                    dither: true,
                    contrast: 1.2,
                    saturation: 0.8,
                    temperature: 0.35,
                    tint: -0.15,
                    lift: [0.01, 0.02, 0.03],
                    gamma: [1.1, 1.0, 0.9],
                    gain: [1.05, 1.0, 0.95],
                    lut: None,
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
                    halation_color: Some([1.0, 0.25, 0.08]),
                    anamorphic: 0.4,
                    anamorphic_length: 0.6,
                    anamorphic_tint: [1.0, 0.5, 0.25],
                    anamorphic_threshold: Some(2.0),
                    grain: 0.02,
                    grain_size: 0.005,
                    grain_motion: GrainMotion::Animated,
                    ssr: 0.7,
                    ssr_fade: [0.3, 0.5],
                    ssr_max_distance: 12.0,
                    ssr_thickness: 0.25,
                },
            )
            .unwrap();
        let ids: Vec<EntityId> = world.entities().collect();
        let post = extract_post(&world, &ids, game_tick(0)).expect("a post-process is present");
        assert_eq!(
            post,
            PostData {
                bloom_threshold: 0.75,
                bloom_strength: 0.2,
                exposure: 1.75,
                tonemap: ludion_renderer::TonemapOperator::Agx,
                exposure_compensation: -0.5,
                meter: Some(ludion_renderer::MeterData {
                    weighting: ludion_renderer::MeterWeighting::Average,
                    clip: [0.05, 0.1],
                    range: [-3.0, 1.0],
                }),
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
                halation_color: Some([1.0, 0.25, 0.08]),
                anamorphic: 0.4,
                anamorphic_length: 0.6,
                anamorphic_tint: [1.0, 0.5, 0.25],
                anamorphic_threshold: Some(2.0),
                grain: 0.02,
                grain_size: 0.005,
                grain_tick: Some(0),
                ssr: 0.7,
                ssr_fade: [0.3, 0.5],
                ssr_max_distance: 12.0,
                ssr_thickness: 0.25,
            }
        );

        // The authored defaults are the renderer's own, field for field.
        world.insert(entity, PostProcess::default()).unwrap();
        assert_eq!(
            extract_post(&world, &ids, game_tick(0)),
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
                extract_post(&world, &ids, game_tick(0)).unwrap().tonemap,
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

        let with_spots = extract(
            &world,
            (320, 180),
            ludion_assets::AssetRoot::content(Path::new(".")),
        )
        .unwrap();
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
        let without = extract(
            &world,
            (320, 180),
            ludion_assets::AssetRoot::content(Path::new(".")),
        )
        .unwrap();
        assert!(without.spot_lights.is_empty());
        assert_eq!(
            with_spots.lights, without.lights,
            "point lights unperturbed"
        );
        assert_eq!(with_spots.lights.len(), 1);
    }

    /// Probes reach `RenderScene.probes` in ascending-`EntityId` order, bad ones dropped; a parented
    /// probe takes the composed world pose.
    #[test]
    fn probes_extract_in_entity_order_with_the_composed_pose() {
        use ludion_core::math::glam::Vec3;
        use ludion_ecs::Parent;
        use ludion_ecs::World;
        use std::path::Path;

        let mut world = World::new();
        let spawn = |world: &mut World, translation: [f32; 3], rotation: [f32; 3]| {
            let id = world.spawn();
            let xform = Transform {
                translation,
                rotation,
                ..Transform::default()
            };
            world.insert(id, xform).unwrap();
            id
        };
        let root = spawn(&mut world, [5.0, 0.0, 0.0], [0.0; 3]);
        world.insert(root, ReflectionProbe::default()).unwrap();
        let broken = spawn(&mut world, [0.0; 3], [0.0; 3]);
        let bad = ReflectionProbe {
            shape: ProbeShape::Sphere { radius: 0.0 },
            ..ReflectionProbe::default()
        };
        world.insert(broken, bad).unwrap();
        let pivot = spawn(&mut world, [0.0, 0.0, 10.0], [0.0, 90.0, 0.0]);
        let child = spawn(&mut world, [1.0, 0.0, 0.0], [0.0; 3]);
        world.insert(child, Parent(pivot)).unwrap();
        world
            .insert(
                child,
                ReflectionProbe {
                    priority: 3,
                    ..ReflectionProbe::default()
                },
            )
            .unwrap();

        let scene = extract(
            &world,
            (320, 180),
            ludion_assets::AssetRoot::content(Path::new(".")),
        )
        .unwrap();
        assert_eq!(scene.probes.len(), 2);
        assert_eq!(scene.probes[0].entity, root);
        assert_eq!(scene.probes[0].position, Vec3::new(5.0, 0.0, 0.0));
        assert_eq!(scene.probes[1].entity, child);
        assert_eq!(scene.probes[1].priority, 3);
        // A +90° turn about Y carries the child's local +X to world -Z.
        assert!(
            (scene.probes[1].position - Vec3::new(0.0, 0.0, 9.0)).length() < 1e-4,
            "{:?}",
            scene.probes[1].position
        );
        let turned = scene.probes[1].rotation * Vec3::X;
        assert!((turned - Vec3::NEG_Z).length() < 1e-4, "{turned:?}");

        world.remove::<ReflectionProbe>(root).unwrap();
        world.remove::<ReflectionProbe>(child).unwrap();
        let none = extract(
            &world,
            (320, 180),
            ludion_assets::AssetRoot::content(Path::new(".")),
        )
        .unwrap();
        assert!(none.probes.is_empty());
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
                    shadow: None,
                    scatter: 0.0,
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

    /// [`SocketPalettes::reusable`] hands back a stored palette only when the mesh, the character and
    /// the model all match the inputs it was posed with.
    mod socket_palette_reuse {
        use super::*;
        use crate::character::CharacterData;
        use ludion_renderer::MeshData;

        fn mesh() -> Arc<MeshData> {
            Arc::new(MeshData {
                positions: Vec::new(),
                normals: Vec::new(),
                colors: Vec::new(),
                materials: Vec::new(),
                porosity: Vec::new(),
                uvs: Vec::new(),
                tangents: Vec::new(),
                weights: Vec::new(),
                bone_indices: Vec::new(),
                skin_joints: Vec::new(),
                textures: None,
                authored_material: false,
                base_color_mean: [1.0; 4],
                rim: None,
                blend: false,
                double_sided: false,
                indices: Vec::new(),
                hash: 0,
                lods: Vec::new(),
            })
        }

        fn character() -> Arc<CharacterData> {
            Arc::new(CharacterData {
                mesh: String::new(),
                clips: Vec::new(),
                masks: Vec::new(),
                blends: Vec::new(),
            })
        }

        fn job(
            skin: EntityId,
            character: Option<Arc<CharacterData>>,
            model: Mat4,
        ) -> SkinJob<'static> {
            SkinJob {
                skin,
                asset: 0,
                character,
                model,
                followed: None,
            }
        }

        #[test]
        fn reusable_declines_on_each_guard_input() {
            let mut world = World::new();
            let skin = world.spawn();
            let (stored_mesh, stored_character) = (mesh(), character());
            let model = Mat4::from_translation(Vec3::new(1.0, 2.0, 3.0));
            let palette = vec![Mat4::IDENTITY, model];
            let store = SocketPalettes {
                chunks: vec![vec![Mat4::ZERO, palette[0], palette[1]]],
                skins: [(
                    skin,
                    StoredPalette {
                        mesh: stored_mesh.clone(),
                        character: Some(stored_character.clone()),
                        model,
                        chunk: 0,
                        range: 1..3,
                        path: PalettePath::Followed,
                    },
                )]
                .into_iter()
                .collect(),
            };
            let same = job(skin, Some(stored_character.clone()), model);

            let (slice, path) = store
                .reusable(&same, &stored_mesh)
                .expect("same inputs reuse");
            assert_eq!(slice, palette.as_slice());
            assert!(matches!(path, PalettePath::Followed));

            assert!(
                store.reusable(&same, &mesh()).is_none(),
                "an equal-content mesh behind another Arc declines"
            );
            let other_character = job(skin, Some(character()), model);
            assert!(store.reusable(&other_character, &stored_mesh).is_none());
            let no_character = job(skin, None, model);
            assert!(store.reusable(&no_character, &stored_mesh).is_none());
            let other_skin = job(world.spawn(), Some(stored_character), model);
            assert!(store.reusable(&other_skin, &stored_mesh).is_none());

            let mut cols = model.to_cols_array();
            cols[1] = -0.0;
            let signed = Mat4::from_cols_array(&cols);
            assert_eq!(signed, model, "-0.0 equals 0.0 numerically");
            let one_bit = job(skin, same.character.clone(), signed);
            assert!(
                store.reusable(&one_bit, &stored_mesh).is_none(),
                "a model that differs by one bit declines"
            );
        }
    }

    /// The assembly clip evaluator: what [`part_placements`] does to a two-part fixture, at rest and
    /// posed. The fixture is the module-doc frames worked by hand, so an expected matrix here is an
    /// arithmetic statement, not a re-derivation of the code under test.
    mod clip_evaluator {
        use super::*;
        use crate::assembly_mesh::{AssemblyClipData, AssemblyData, AssemblyPartData};
        use ludion_renderer::MeshData;
        use ludion_scene::{AnimationDocument, AnimationTrack, Ease, Keyframe};

        #[test]
        fn held_ticks_snaps_down_to_the_grid() {
            assert_eq!(held_ticks(5, 14), 10);
            assert_eq!(held_ticks(5, 15), 15);
            assert_eq!(held_ticks(0, 7), 7);
        }

        #[test]
        fn held_passes_a_fractional_instant_through_unless_stop_motion() {
            let q = ludion_core::time::Q16_PER_TICK;
            let now = GameTime::from_q16(7 * q + q / 4, TickRate::DEFAULT);
            let smooth = Animation::default();
            assert_eq!(held(&smooth, now), now);
            let one = Animation {
                hold: 1,
                ..Animation::default()
            };
            assert_eq!(held(&one, now), now);
            let stop_motion = Animation {
                hold: 3,
                ..Animation::default()
            };
            assert_eq!(
                held(&stop_motion, now),
                GameTime::from_ticks(6, TickRate::DEFAULT)
            );
        }

        #[test]
        fn fractional_clock_ticks_match_integers_on_whole_ticks() {
            let q = ludion_core::time::Q16_PER_TICK;
            for now in [0u64, 3, 60, 1000] {
                for started in [0u64, 3, 600] {
                    let whole = GameTime::from_ticks(now, TickRate::DEFAULT);
                    let old = now.saturating_sub(started) as f64 * f64::from(0.75f32);
                    let got = clock_ticks(whole, FrameIndex::new(started), 0.75, None);
                    assert_eq!(got.to_bits(), old.to_bits(), "{now}/{started}");
                }
            }
            let half = GameTime::from_q16(10 * q + q / 2, TickRate::DEFAULT);
            assert_eq!(clock_ticks(half, FrameIndex::new(4), 1.0, None), 6.5);
            assert_eq!(clock_ticks(half, FrameIndex::new(40), 1.0, None), 0.0);
        }

        /// An empty mesh — the evaluator never reads geometry, only `offset`/`pivot`/`parent`.
        fn mesh() -> Arc<MeshData> {
            Arc::new(MeshData {
                positions: Vec::new(),
                normals: Vec::new(),
                colors: Vec::new(),
                materials: Vec::new(),
                porosity: Vec::new(),
                uvs: Vec::new(),
                tangents: Vec::new(),
                weights: Vec::new(),
                bone_indices: Vec::new(),
                skin_joints: Vec::new(),
                textures: None,
                authored_material: false,
                base_color_mean: [1.0; 4],
                rim: None,
                blend: false,
                double_sided: false,
                indices: Vec::new(),
                hash: 0,
                lods: Vec::new(),
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
                masks: Vec::new(),
                blends: Vec::new(),
                ik: Vec::new(),
                ground: None,
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

            let rigid = part_placements_with(&data, pose, None, false, &[]);
            assert_eq!(rigid, part_placements(&data, None));
        }

        /// The ragdoll's and model sheet's entry points pass no layers, and an empty layer list on the
        /// render entry is exactly the plain pose, rest return included.
        #[test]
        fn no_layers_is_the_plain_pose_bit_for_bit() {
            let data = assembly(vec![clip(
                "walk",
                "arm",
                [0.0; 3],
                [90.0, 0.0, 0.0],
                [0.0; 3],
            )]);
            let pose = Some((data.clip("walk").unwrap(), 0.4));
            assert_eq!(
                part_placements_layered(&data, AssemblySide::single(pose), None, &[]),
                part_placements(&data, pose)
            );
            assert_eq!(
                part_placements_layered(&data, AssemblySide::EMPTY, None, &[]),
                part_placements(&data, None)
            );
        }

        /// A side of one weight-1 entry is that clip's `part_pose` verbatim, so a plain clip poses
        /// through the blend-capable path with the exact same bits.
        #[test]
        fn a_single_entry_side_is_the_plain_clip_bit_for_bit() {
            let data = assembly(vec![clip(
                "walk",
                "arm",
                [0.0; 3],
                [90.0, 0.0, 0.0],
                [1.0, 2.0, 3.0],
            )]);
            let walk = data.clip("walk").unwrap();
            for t in [0.0, 0.1, 0.4, 0.77, 1.0] {
                let side = AssemblySide::weighted([Some((walk, t, 1.0)), None, None]);
                for part in ["arm", "body", "missing"] {
                    assert_eq!(
                        side.sample(part),
                        ludion_scene::part_pose(walk, part, t),
                        "{part} at {t}"
                    );
                }
                assert_eq!(
                    part_placements_sides(&data, side, None, true, &[]),
                    part_placements(&data, Some((walk, t)))
                );
            }
        }

        /// Two weighted entries blend in entry order; a channel no entry keys stays unset, so an
        /// untouched part keeps its exact rest.
        #[test]
        fn a_two_entry_side_blends_keyed_channels_only() {
            let data = assembly(vec![
                clip("a", "arm", [90.0, 0.0, 0.0], [0.0; 3], [0.0; 3]),
                clip("b", "arm", [0.0; 3], [2.0, 0.0, 0.0], [0.0; 3]),
            ]);
            let (a, b) = (data.clip("a").unwrap(), data.clip("b").unwrap());
            let side = AssemblySide::weighted([Some((a, 1.0, 0.5)), Some((b, 1.0, 0.5)), None]);
            let arm = side.sample("arm");
            assert!(arm.rotation.is_some() && arm.translation.is_some());
            assert!(arm.scale.is_none());
            assert_eq!(arm.translation, Some([1.0, 0.0, 0.0]));
            let body = side.sample("body");
            assert!(body.rotation.is_none() && body.scale.is_none());
            assert_eq!(side.sample("missing"), ludion_scene::PartPose::default());
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
                masks: Vec::new(),
                blends: Vec::new(),
                ik: Vec::new(),
                ground: None,
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
                grid_ticks: None,
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
                ludion_assets::AssetRoot::content(Path::new(".")),
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
                    ludion_assets::AssetRoot::content(Path::new(".")),
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
            let one_shot = extract_at(
                &world,
                (320, 180),
                ludion_assets::AssetRoot::content(&dir),
                tick_zero(),
                density,
            )
            .unwrap();
            let live = extract_through(
                &world,
                (320, 180),
                ludion_assets::AssetRoot::content(&dir),
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

        /// Hiding a static mesh leaves the voxelization alone: the static GI bake ignores
        /// `Visibility`, so a toggle is a signature hit with a bit-identical grid, while the scene
        /// itself drops the mesh.
        #[test]
        fn hiding_a_static_mesh_does_not_invalidate_the_signature() {
            let mut cache = AssetCache::new();
            let before = rasterization_count();
            let mut world = world_with_a_cube(0.0, 0.5);
            let shown = voxelize_through(&world, &mut cache);

            let cube = world.entities().next().expect("the cube");
            world
                .insert(
                    cube,
                    crate::components::Visibility {
                        visible: false,
                        ..Default::default()
                    },
                )
                .unwrap();
            let hidden = extract_through(
                &world,
                (320, 180),
                ludion_assets::AssetRoot::content(Path::new(".")),
                &mut cache,
                tick_zero(),
                QualityParams::FULL.particle_density,
                None,
                &mut Vec::new(),
            )
            .expect("the fixture world extracts");
            assert!(hidden.meshes.is_empty(), "the hidden cube is not drawn");
            assert_eq!(
                rasterization_count() - before,
                1,
                "a visibility toggle must be a cache hit, not a rebuild"
            );
            assert_eq!(
                shown.hash,
                hidden.voxel.expect("the grid survives").hash,
                "the static bake ignores Visibility"
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

        /// A scratch project holding `meshes/prop.mesh.yaml`, one triangle at height `y`, and the
        /// gallery's skinned mannequin as `meshes/skin.mesh.yaml`.
        fn project_with_a_prop(tag: &str, y: f32) -> std::path::PathBuf {
            let dir = std::env::temp_dir()
                .join(format!("ludion-voxel-cache-{tag}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(dir.join("meshes")).unwrap();
            write_prop(&dir, y);
            let gallery = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../examples/gallery/meshes/skin-mannequin.mesh.yaml");
            std::fs::copy(gallery, dir.join("meshes/skin.mesh.yaml")).unwrap();
            dir
        }

        fn write_prop(dir: &std::path::Path, y: f32) {
            std::fs::write(
                dir.join("meshes/prop.mesh.yaml"),
                format!(
                    "format: 1\nkind: mesh\npositions:\n  - 0.0\n  - {y:?}\n  - 0.0\n  - 3.0\n  - \
                     {y:?}\n  - 0.0\n  - 0.0\n  - {y:?}\n  - 3.0\nindices:\n  - 0\n  - 1\n  - 2\n"
                ),
            )
            .unwrap();
        }

        fn place_mesh(world: &mut World, shape: MeshSource, dynamic: bool) {
            let entity = world.spawn();
            world.insert(entity, Transform::default()).unwrap();
            world
                .insert(
                    entity,
                    Mesh {
                        shape,
                        ..Mesh::default()
                    },
                )
                .unwrap();
            if dynamic {
                world.insert(entity, GiDynamic::default()).unwrap();
            }
        }

        fn world_with_a_prop() -> World {
            let mut world = World::new();
            let volume = world.spawn();
            world
                .insert(
                    volume,
                    GiVolume {
                        cell_size: 0.5,
                        ..GiVolume::default()
                    },
                )
                .unwrap();
            place_mesh(
                &mut world,
                MeshSource::Asset("meshes/prop.mesh.yaml".into()),
                false,
            );
            world
        }

        fn voxelize_in(
            world: &World,
            dir: &std::path::Path,
            cache: &mut AssetCache,
        ) -> Arc<WorldVoxelGrid> {
            extract_through(
                world,
                (320, 180),
                ludion_assets::AssetRoot::content(dir),
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

        /// An unchanged asset-mesh world rasterizes nothing on the next extract; rewriting the
        /// mesh document and reloading it rebuilds once.
        #[test]
        fn an_asset_mesh_enters_the_signature_by_its_hash() {
            let dir = project_with_a_prop("hash", 0.0);
            let world = world_with_a_prop();
            let mut cache = AssetCache::new();
            let before = rasterization_count();

            let first = voxelize_in(&world, &dir, &mut cache);
            let again = voxelize_in(&world, &dir, &mut cache);
            assert_eq!(rasterization_count() - before, 1, "an unchanged world hits");
            assert!(Arc::ptr_eq(&first, &again));

            write_prop(&dir, 2.0);
            cache
                .reload(
                    ludion_assets::AssetRoot::content(&dir),
                    "meshes/prop.mesh.yaml",
                )
                .unwrap();
            let edited = voxelize_in(&world, &dir, &mut cache);
            assert_eq!(
                rasterization_count() - before,
                2,
                "an edited mesh rebuilds once"
            );
            assert_ne!(edited.hash, first.hash);
            voxelize_in(&world, &dir, &mut cache);
            assert_eq!(rasterization_count() - before, 2);
            let _ = std::fs::remove_dir_all(&dir);
        }

        /// A moving `GiDynamic` mesh beside an asset mesh costs the asset-mesh grid no rebuild.
        #[test]
        fn moving_a_dynamic_mesh_beside_an_asset_mesh_rebuilds_nothing() {
            let dir = project_with_a_prop("dynamic", 0.0);
            let mut world = world_with_a_prop();
            place_mesh(&mut world, MeshSource::Cube, true);
            let mut cache = AssetCache::new();
            let before = rasterization_count();
            voxelize_in(&world, &dir, &mut cache);
            let mover = world.entities().last().unwrap();
            world
                .insert(
                    mover,
                    Transform {
                        translation: [5.0, 0.0, 0.0],
                        ..Transform::default()
                    },
                )
                .unwrap();
            voxelize_in(&world, &dir, &mut cache);
            assert_eq!(rasterization_count() - before, 1);
            let _ = std::fs::remove_dir_all(&dir);
        }

        /// The skip report counts each reason: dynamic, skinned, cutout (`mask` or `blend`) and unsupported sources, with
        /// the entity index of the first; asset meshes it rasterizes are not counted.
        /// The camera's LOD knobs reach `RenderScene.lod`, and `Camera.lod_bias: 2` turns a chained
        /// instance's mid-distance pick coarser; a world without the knobs gets the defaults.
        #[test]
        fn the_camera_lod_knobs_reach_the_scene_and_coarsen_the_pick() {
            let lod_of = |camera: crate::components::Camera| {
                let mut world = World::new();
                let id = world.spawn();
                world.insert(id, camera).unwrap();
                let ids: Vec<_> = world.entities().collect();
                scene_lod(&world, &ids)
            };
            assert_eq!(
                lod_of(crate::components::Camera::default()),
                LodSettings::default()
            );
            let biased = lod_of(crate::components::Camera {
                lod_bias: Some(2.0),
                shadow_bias_levels: Some(1),
                ..crate::components::Camera::default()
            });
            assert_eq!(
                biased,
                LodSettings {
                    bias: 2.0,
                    shadow_levels: 1
                }
            );
            let level = |bias: f32| {
                let size = ludion_renderer::lod_size(
                    1.0,
                    6.0,
                    2.0 * ludion_core::math::exp2(-bias),
                    false,
                );
                ludion_renderer::lod_level(size, &[0.5, 0.25, 0.125], 0)
            };
            assert!(level(biased.bias) > level(0.0));
        }

        #[test]
        fn the_skip_report_counts_what_was_left_out() {
            let dir = project_with_a_prop("skips", 0.0);
            let mut world = World::new();
            let camera = world.spawn();
            world
                .insert(camera, crate::components::Camera::default())
                .unwrap();
            place_mesh(
                &mut world,
                MeshSource::Asset("meshes/prop.mesh.yaml".into()),
                false,
            );
            place_mesh(
                &mut world,
                MeshSource::Asset("meshes/skin.mesh.yaml".into()),
                false,
            );
            place_mesh(&mut world, MeshSource::Cube, true);
            place_mesh(&mut world, MeshSource::Cube, true);
            let prop = std::fs::read_to_string(dir.join("meshes/prop.mesh.yaml")).unwrap();
            std::fs::write(
                dir.join("meshes/mask.mesh.yaml"),
                format!("{prop}alpha_mode: mask\n"),
            )
            .unwrap();
            place_mesh(
                &mut world,
                MeshSource::Asset("meshes/mask.mesh.yaml".into()),
                false,
            );
            std::fs::write(
                dir.join("meshes/blend.mesh.yaml"),
                format!("{prop}alpha_mode: blend\n"),
            )
            .unwrap();
            place_mesh(
                &mut world,
                MeshSource::Asset("meshes/blend.mesh.yaml".into()),
                false,
            );

            let ids: Vec<_> = world.entities().collect();
            let camera = single_camera(&world, &ids).unwrap().unwrap();
            let camera = build_camera(&world, &ids, camera, (320, 180));
            let placed = place_uncached(
                &world,
                &ids,
                &camera,
                ludion_assets::AssetRoot::content(&dir),
                tick_zero(),
            );
            let mut cache = AssetCache::new();
            let mut assets = CachedAssets {
                root: ludion_assets::AssetRoot::content(&dir),
                cache: &mut cache,
            };
            let voxelized =
                crate::voxelizer::voxelize(&Default::default(), &placed.meshes, 0.5, &mut assets)
                    .unwrap();
            assert!(voxelized.grid.is_some());
            let report = crate::voxelizer::skip_report(&placed.meshes, &mut assets).unwrap();
            assert_eq!((report.dynamic.count, report.dynamic.first), (2, Some(3)));
            assert_eq!((report.skinned.count, report.skinned.first), (1, Some(2)));
            assert_eq!(report.non_finite.count, 0);
            assert_eq!(report.unsupported.count, 0);
            assert_eq!(
                (report.cutout.count, report.cutout.first),
                (2, Some(5)),
                "a mask and a blend document are both skipped"
            );
            let _ = std::fs::remove_dir_all(&dir);
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
                ludion_assets::AssetRoot::content(Path::new(".")),
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
