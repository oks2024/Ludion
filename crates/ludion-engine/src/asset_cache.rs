//! `AssetCache` — the engine's CPU-side decoded-image cache.
//!
//! `extract()` reads sprite textures **through** this cache instead of re-reading and re-decoding
//! every PNG from disk on every frame. A miss loads and decodes the PNG once and inserts it; a hit is
//! a plain map lookup — so the live frame stream no longer does per-frame disk IO (the per-frame cost
//! `Plan/phase-7-designer-flow.md` decision 5 sets out to kill). `asset.reload` is the one thing that
//! finally has a cache to invalidate: it drops an entry and re-reads it from disk.
//!
//! # The cache key
//!
//! The key is the sprite's texture path **as it appears in the scene** (the `rel` string `extract`
//! resolves against the project root), in one lexical spelling (`ludion_core::paths::normalize`:
//! separators, `.` and `..` collapsed) so `assets/x.png`, a Windows `assets\x.png` and a mesh map's
//! `meshes/../assets/x.png` collide — one file is one edge. This keeps the *hot* path —
//! `extract`, run every frame — a pure string lookup with no filesystem access.
//!
//! An `asset.reload` arrives with either that same relative path (a client naming a scene asset) or an
//! **absolute** path (the host forwarding a filesystem-watcher event). Both are matched to a key on
//! the *cold* reload path only: a direct relative-key hit first, else by canonical filesystem identity
//! (`std::fs::canonicalize`) — the OS's own identity for a file, robust to separator, `..`, and
//! symlink differences across platforms (notably FSEvents' `/var`→`/private/var` rewriting on macOS).
//! Canonicalization is deliberately confined to reload; it never touches the per-frame `extract`.
//!
//! Most asset kinds are built from more than one file, and each is reachable from any of them. An
//! **assembly** is the `*.assembly.yaml` document plus every `*.part.yaml` and `*.anim.yaml` it
//! names, so a reload of a part or clip file re-reads every cached assembly that named it. A
//! **character** is its document plus its mesh document and every clip it binds, so a reload of the
//! mesh or a clip re-reads the character. A **sculpt** is its document plus the voxel and mesh documents its parts name, and a **mesh** is its
//! document plus the texture PNGs it binds. An **atlas** is the `*.atlas.yaml` document plus the
//! packed colour PNG its `maps: { color }` names, so re-running `ludion generate-atlas` — which
//! rewrites both — invalidates the entry from either side. One `dependents` map records every such
//! edge, whatever the kind.
//!
//! A mesh's texture maps are entries of their own, shared by every mesh that names the file, so one PNG
//! is one decode however many meshes bind it. Under `textures: cooked` a map an `[[import]]` table
//! produced or a `[cook] roots` directory holds is held per colour role instead (`MapRole`): its
//! cooked file's bytes differ between sRGB and linear, so one path can hold two entries, each
//! either the cooked image or — with its reason recorded for the load's one `L1271` — the PNG's
//! decode. Switching the setting drops every map and the meshes built from them. A decal's maps
//! always read their PNG: sync cooks only what mesh documents name.
//!
//! After a world rebuild the first cached extract (or `render.bounds`) keeps only what the new world
//! names ([`AssetCache::retain_referenced`]), re-hashes the files the kept entries were built from and
//! drops what changed; the extract then calls [`AssetCache::prefetch`], which decodes the misses on
//! the worker pool and inserts the results serially in sorted key order, so the cache is the same at
//! any thread count and the extract that follows only hits. A scene load therefore never clears the
//! cache: what two worlds share is decoded once. A successful `scene.save` evicts what the world no
//! longer names ([`AssetCache::evict_unreferenced`]), without the re-hash.
//!
//! # Remembered failures
//!
//! A remembered failure is a short-lived memo for the live frame stream, never a verdict. A failed
//! load cannot name every file it would have read (a missing part of an assembly leaves no
//! dependency edge), so the memo is never matched to a changed file; the whole memo clears on:
//! any `asset.reload`, whatever the path; a world rebuild or eviction; a real cook setting change;
//! any one-shot extract (screenshot, profile, dump); and every `FAILURE_RETRY_PASSES` extract
//! passes, so a host with no watcher still heals. A cook fallback is not a failure: it decoded, is
//! held as an entry with its reason in `cook_misses`, and is re-tested by
//! [`AssetCache::retry_cook_misses`].
//!
//! Asset bytes live here and nowhere in the world, so a reload is invisible to the `WorldHash` and a
//! recorded session — sim-invisible and replay-safe.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use ludion_assets::AssetRoot;
use ludion_assets::cooked::ColorSpace;
use ludion_core::diagnostics::Diagnostic;
use ludion_renderer::{MapImage, MeshData, TextureImage, WorldVoxelGrid};
use ludion_scene::MeshDocument;

use crate::assembly_body::AssemblyBody;
use crate::assembly_mesh::AssemblyData;
use crate::character::CharacterData;
use crate::cooked_maps::{CookLookup, CookMiss, CookRole};
use crate::extract::{AtlasAsset, load_texture};
use crate::extract_observer::ExtractObserver;
use crate::load_observer::{LoadKind, LoadObserver, LoadPhase};
use crate::prefab_clips::{PartRests, PrefabClipData};
use crate::voxel_mesh::VoxelStaticGrid;
use ludion_assets::cube::CubeLut;

/// How many extract passes a remembered load failure survives with no other clearer, so a host with
/// no file watcher still retries within about a second at 60 fps.
const FAILURE_RETRY_PASSES: u32 = 60;

/// The most distinct prefab texts whose parsed rests are held. The one map bounded by time (keyed by
/// text version) rather than by project size; a parse is cheap, so clearing when full costs only a
/// world with more live prefab texts than this.
const PREFAB_RESTS_CAP: usize = 256;

/// Which pixels a mesh map's cache entry holds: the PNG's decode, or — under `textures: cooked`, for
/// a map an `[[import]]` table produced or a `[cook] roots` directory holds — the cooked file read
/// for one colour role (a cooked map's bytes differ by role, so one path can hold two entries).
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
enum MapRole {
    Source,
    Cooked(CookRole),
}

/// Which of the cache's maps an entry lives in — the tag a dependency edge carries so a reload
/// re-runs the right loader.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
enum AssetKind {
    Texture,
    /// A texture map a mesh document binds: held once however many meshes name it (per role under
    /// cooked).
    MeshMap(MapRole),
    Mesh,
    Voxel,
    VoxelGrid,
    Sculpt,
    Assembly,
    Character,
    /// Keys the remembered failure of an assembly's body read; it never holds an entry.
    AssemblyBody,
    PrefabClips,
    Atlas,
    /// A `.cube` LUT a `PostProcess` names.
    Lut,
}

impl AssetKind {
    fn load_kind(self) -> LoadKind {
        match self {
            AssetKind::Texture | AssetKind::MeshMap(_) => LoadKind::Texture,
            AssetKind::Mesh => LoadKind::Mesh,
            AssetKind::Voxel => LoadKind::Voxel,
            AssetKind::VoxelGrid => LoadKind::VoxelGrid,
            AssetKind::Sculpt => LoadKind::Sculpt,
            AssetKind::Assembly | AssetKind::AssemblyBody => LoadKind::Assembly,
            AssetKind::Character => LoadKind::Character,
            AssetKind::PrefabClips => LoadKind::PrefabClips,
            AssetKind::Atlas => LoadKind::Atlas,
            AssetKind::Lut => LoadKind::Texture,
        }
    }

    fn load_phase(self) -> LoadPhase {
        match self {
            AssetKind::Texture | AssetKind::MeshMap(_) => LoadPhase::Texture,
            AssetKind::Mesh => LoadPhase::Mesh,
            _ => LoadPhase::Other,
        }
    }
}

/// The assets a world references, as normalized cache keys, one set per kind — what
/// [`AssetCache::prefetch`] decodes ahead of the first extract. Sets, so a key named by a thousand
/// entities is one decode and the walk order is the sort order.
#[derive(Default)]
pub(crate) struct AssetRefs {
    pub(crate) textures: BTreeSet<String>,
    pub(crate) atlases: BTreeSet<String>,
    pub(crate) luts: BTreeSet<String>,
    pub(crate) meshes: BTreeSet<String>,
    pub(crate) voxels: BTreeSet<String>,
    pub(crate) sculpts: BTreeSet<String>,
    /// Skinned and voxel assemblies alike; [`AssetCache::assembly_body`] tells them apart.
    pub(crate) assemblies: BTreeSet<String>,
    /// The prefabs whose clips an animated instance root plays.
    pub(crate) prefab_clips: BTreeSet<String>,
    /// Maps a decal names, always read as PNG; a mesh's maps come from its document.
    pub(crate) maps: BTreeSet<String>,
}

/// One decoded entry, ready to insert: what a worker thread hands back to the cache's owner. Every
/// kind but a mesh, which needs its maps from the cache to be assembled.
enum Decoded {
    Texture(Arc<TextureImage>),
    /// A mesh map under its role, with why a cooked role fell back to the PNG.
    MeshMap(MapRole, MapImage, Option<CookMiss>),
    Voxel(MeshData),
    VoxelGrid(VoxelStaticGrid),
    Sculpt(MeshData),
    Assembly(AssemblyData),
    Character(CharacterData),
    PrefabClips(PrefabClipData),
    Atlas(AtlasAsset),
    Lut(CubeLut),
}

/// Runs `kind`'s loader for `rel` with no access to the cache, returning the value and the files it
/// read with their content hashes. Pure, so a prefetch can run it on any thread. Never called for
/// [`AssetKind::Mesh`]. `cook` is the lookup a [`MapRole::Cooked`] map reads through.
///
/// The recorded hash is that of the bytes the loader decoded, so an edit racing the decode is caught
/// by the next revalidation.
fn decode(
    project_root: AssetRoot<'_>,
    kind: AssetKind,
    rel: &str,
    key: &str,
    cook: Option<&CookLookup>,
) -> Result<(Decoded, SourceFiles), Vec<Diagnostic>> {
    Ok(match kind {
        AssetKind::Texture => {
            // Premultiplied on the miss, never on the hit: the convention is a property of the
            // decoded bytes, and every frame after this one reads them straight back out.
            let image = load_texture(project_root, rel).map_err(|d| vec![d])?;
            let hash = image.hash;
            (Decoded::Texture(image), vec![(key.to_string(), hash)])
        }
        AssetKind::MeshMap(role) => {
            // The PNG's bytes are read either way: a cooked map's key is their hash.
            let bytes = ludion_assets::load_bytes(project_root, rel).map_err(|d| vec![d])?;
            let hash = bytes.hash;
            let cooked = match (role, cook) {
                (MapRole::Cooked(role), Some(cook)) => {
                    let cap = cook.cap_for(key).unwrap_or(None);
                    Some(cook.read(bytes.hash, role, cap))
                }
                (MapRole::Cooked(_), None) => Some(Err(CookMiss::Unsupported)),
                (MapRole::Source, _) => None,
            };
            let source = || {
                crate::mesh_asset::map_from_bytes(&bytes, rel)
                    .map(MapImage::Source)
                    .map_err(|d| vec![d])
            };
            let (image, miss) = match cooked {
                Some(Ok(cooked)) => (MapImage::Cooked(cooked), None),
                Some(Err(miss)) => (source()?, Some(miss)),
                None => (source()?, None),
            };
            (
                Decoded::MeshMap(role, image, miss),
                vec![(key.to_string(), hash)],
            )
        }
        AssetKind::Mesh => unreachable!("a mesh is assembled against the cache's maps"),
        AssetKind::AssemblyBody => unreachable!("a body read decodes no entry"),
        AssetKind::Voxel => {
            let mesh = crate::voxel_mesh::load(project_root, rel)?;
            let hash = mesh.hash;
            (Decoded::Voxel(mesh), vec![(key.to_string(), hash)])
        }
        AssetKind::VoxelGrid => {
            let grid = crate::voxel_mesh::load_grid(project_root, rel)?;
            let hash = grid.hash;
            (Decoded::VoxelGrid(grid), vec![(key.to_string(), hash)])
        }
        AssetKind::Sculpt => {
            let (mesh, sources) = crate::sculpt_mesh::load_with_sources(project_root, rel)?;
            (Decoded::Sculpt(mesh), sources)
        }
        AssetKind::Assembly => {
            let (data, sources) = crate::assembly_mesh::load_with_sources(project_root, rel)?;
            (Decoded::Assembly(data), sources)
        }
        AssetKind::Character => {
            let (data, sources) = crate::character::load_with_sources(project_root, rel)?;
            (Decoded::Character(data), sources)
        }
        AssetKind::PrefabClips => {
            let (data, sources) = crate::prefab_clips::load_with_sources(project_root, rel)?;
            (Decoded::PrefabClips(data), sources)
        }
        AssetKind::Atlas => {
            let (asset, sources) = crate::extract::load_atlas_with_sources(project_root, rel)?;
            (Decoded::Atlas(asset), sources)
        }
        AssetKind::Lut => {
            let lut = ludion_assets::cube::load_cube(project_root, rel).map_err(|d| vec![d])?;
            let hash = lut.hash;
            (Decoded::Lut(lut), vec![(key.to_string(), hash)])
        }
    })
}

/// A CPU cache of decoded assets — texture [`TextureImage`]s, asset [`MeshData`], voxel-model
/// [`MeshData`], the voxel-model [`VoxelStaticGrid`] (D15), sculpt-model [`MeshData`], meshed
/// [`AssemblyData`], and [`AtlasAsset`]s — keyed by slash-normalized project-root-relative path.
///
/// One map per asset kind (a path names exactly one kind, though a voxel path names both a mesh and a
/// grid), all under the one cache so `asset.reload` and a rebuild's revalidation invalidate textures,
/// meshes, voxel models, assemblies, and atlases uniformly. Every kind records which files it was read from
/// in the one [`dependents`](AssetCache::dependents) map, so a reload of any input re-reads every
/// entry built from it. Asset meshes and voxel models are held
/// separately because their reload path re-runs a different loader (`.mesh.yaml` faceting vs
/// `.voxel.yaml` greedy meshing), even though both yield [`MeshData`]. The voxel grid is held
/// separately again — a distinct shape ([`VoxelStaticGrid`], not [`MeshData`]) derived from the same
/// document by a different accessor ([`crate::voxel_mesh::static_grid`]). `BTreeMap` (not a hashed
/// map) both satisfies the sim-side determinism denylist and keeps the reload key-scan order stable;
/// the cache is never part of any hash, so its only ordering requirement is not flaking.
///
/// Textures, atlases, meshes, voxels, and voxel grids are stored as `Arc<_>`: a hit hands the
/// caller a clone of the `Arc` (a refcount bump), not the underlying buffers, which run to
/// megabytes for a large voxel model streamed every extract (`Plan/voxel-artist-scale.md`).
pub(crate) struct AssetCache {
    images: BTreeMap<String, Arc<TextureImage>>,
    /// The maps meshes bind, one entry per file (per role under cooked) whichever meshes name it.
    mesh_maps: BTreeMap<(String, MapRole), MapImage>,
    /// Why each cooked entry of [`mesh_maps`](AssetCache::mesh_maps) holds its PNG instead, in step
    /// with it.
    cook_misses: BTreeMap<(String, CookRole), CookMiss>,
    /// Where cooked maps are read from, or `None` to decode every map from its PNG. Host state, like
    /// [`observer`](AssetCache::observer): changing it drops every mesh map and the meshes built
    /// from them ([`set_cook`](AssetCache::set_cook)).
    cook: Option<Arc<CookLookup>>,
    meshes: BTreeMap<String, Arc<MeshData>>,
    voxels: BTreeMap<String, Arc<MeshData>>,
    voxel_grids: BTreeMap<String, Arc<VoxelStaticGrid>>,
    sculpts: BTreeMap<String, Arc<MeshData>>,
    assemblies: BTreeMap<String, Arc<AssemblyData>>,
    characters: BTreeMap<String, Arc<CharacterData>>,
    prefab_clips: BTreeMap<String, Arc<PrefabClipData>>,
    /// Part rests parsed from a prefab's held text, by the text's content hash: an instance poses
    /// against the text it was expanded from, which may differ from the file.
    prefab_rests: BTreeMap<u64, Arc<PartRests>>,
    /// The clipped ground markers' visibility profiles; derived, never an asset on disk.
    clip_cache: crate::ground_marker_clip::ClipCache,
    /// Loads that failed, by kind and the raw `rel` they were asked for, so the live frame stream does
    /// not re-read the disk every frame. A short-lived memo, never a verdict: see the module docs for
    /// what clears it.
    failures: BTreeMap<(AssetKind, String), Vec<Diagnostic>>,
    /// Extract passes since the memo was last cleared.
    failure_passes: u32,
    atlases: BTreeMap<String, Arc<AtlasAsset>>,
    luts: BTreeMap<String, Arc<CubeLut>>,
    /// Which cached entries each **source file** feeds, as `(kind, entry key)`: an entry's own
    /// document key maps to itself, and every other file it read (an assembly's part or clip, a
    /// sculpt's voxel or mesh part, a mesh's texture, an atlas's colour map) maps to every entry that
    /// names it — one limb file, or one PNG, legitimately serves several. This is what makes an edit
    /// to such a file reloadable: the watcher reports the file that changed, and only the cache knows
    /// which entries were read out of it.
    dependents: BTreeMap<String, Vec<(AssetKind, String)>>,
    /// The content hash of each source file in [`dependents`](AssetCache::dependents) as of the
    /// decode that last read it: what [`retain_referenced`](AssetCache::retain_referenced) compares
    /// the file on disk against to find the entries that went stale between two worlds.
    source_hashes: BTreeMap<String, u64>,
    voxelization: Option<(u64, Option<Arc<WorldVoxelGrid>>)>,
    /// Whether a host is holding the voxelized baseline: a signature miss then serves the retained
    /// entry **without rewriting its key**, so the held grid stays the baseline however far the world
    /// moves from it. Host/session state a host sets per frame
    /// ([`Engine::set_gi_rebake`](crate::Engine::set_gi_rebake)) — never world content, so it is not
    /// cleared by [`reset_voxelization`](AssetCache::reset_voxelization). Every world rebuild forces one unheld frame, so the
    /// new world's first build replaces the retained entry and becomes the held baseline.
    gi_hold: bool,
    /// Whether the last hold-aware read served the retained grid for a signature it was not built
    /// under. Reset at the start of every extract and recomputed there, never latched — the
    /// signature comparison runs every frame anyway, so a world that returns to the held baseline is
    /// honestly not stale.
    gi_stale: bool,
    /// Load instrumentation a host attached. Host state, not content, so
    /// [`reset_voxelization`](AssetCache::reset_voxelization) leaves it alone — the scene load is the
    /// very event it measures.
    observer: Option<Box<dyn LoadObserver>>,
    /// Extract instrumentation a host attached. Host state like `observer`.
    extract_observer: Option<Box<dyn ExtractObserver>>,
}

impl AssetCache {
    /// The normalized path a cached mesh or voxel model was loaded from, by identity of its `Arc`.
    /// Linear: for the profile report only.
    pub(crate) fn mesh_path(&self, mesh: &Arc<MeshData>) -> Option<&str> {
        self.meshes
            .iter()
            .chain(&self.voxels)
            .find(|(_, cached)| Arc::ptr_eq(cached, mesh))
            .map(|(path, _)| path.as_str())
    }

    /// An empty cache.
    pub(crate) fn new() -> Self {
        AssetCache {
            images: BTreeMap::new(),
            mesh_maps: BTreeMap::new(),
            cook_misses: BTreeMap::new(),
            cook: None,
            meshes: BTreeMap::new(),
            voxels: BTreeMap::new(),
            voxel_grids: BTreeMap::new(),
            sculpts: BTreeMap::new(),
            assemblies: BTreeMap::new(),
            characters: BTreeMap::new(),
            prefab_clips: BTreeMap::new(),
            prefab_rests: BTreeMap::new(),
            clip_cache: Default::default(),
            failures: BTreeMap::new(),
            failure_passes: 0,
            atlases: BTreeMap::new(),
            luts: BTreeMap::new(),
            dependents: BTreeMap::new(),
            source_hashes: BTreeMap::new(),
            voxelization: None,
            gi_hold: false,
            gi_stale: false,
            observer: None,
            extract_observer: None,
        }
    }

    /// Attaches (or detaches) the load observer.
    pub(crate) fn set_observer(&mut self, observer: Option<Box<dyn LoadObserver>>) {
        self.observer = observer;
    }

    /// The cooked store mesh-map loading currently reads, `None` under source textures.
    pub(crate) fn cook(&self) -> Option<&CookLookup> {
        self.cook.as_deref()
    }

    /// Points mesh-map loading at a cooked store (`Some`) or back at the PNGs (`None`). A change
    /// drops every mesh map through its dependents, which drops the meshes built from them, so the
    /// next prefetch or extract rebuilds them under the new setting; an equal setting is a no-op.
    /// Returns whether it changed anything.
    pub(crate) fn set_cook(&mut self, cook: Option<Arc<CookLookup>>) -> bool {
        if self.cook == cook {
            return false;
        }
        self.forget_failures();
        self.cook = cook;
        let files: BTreeSet<String> = self
            .mesh_maps
            .keys()
            .map(|(path, _)| path.clone())
            .collect();
        for file in files {
            self.drop_dependents(&file);
        }
        self.prune_source_hashes();
        true
    }

    /// Re-tests every held cook fallback against the store: a map whose cooked file now reads, or
    /// fails for a different reason, is dropped with the meshes built from it, so the next prefetch
    /// or extract rebuilds it. A map still missing for the same reason keeps its held PNG with no
    /// re-decode. Returns whether anything was dropped.
    pub(crate) fn retry_cook_misses(&mut self) -> bool {
        let Some(cook) = self.cook.clone() else {
            return false;
        };
        let mut moved: BTreeSet<String> = BTreeSet::new();
        for ((key, role), held) in &self.cook_misses {
            let Some(hash) = self.source_hashes.get(key) else {
                continue;
            };
            let cap = cook.cap_for(key).unwrap_or(None);
            match cook.read(*hash, *role, cap) {
                Err(miss) if miss == *held => {}
                _ => {
                    moved.insert(key.clone());
                }
            }
        }
        let any = !moved.is_empty();
        for key in moved {
            self.drop_dependents(&key);
        }
        any
    }

    /// Forgets every remembered load failure.
    pub(crate) fn forget_failures(&mut self) {
        self.failures.clear();
        self.failure_passes = 0;
    }

    /// Counts one extract pass against the remembered failures and forgets them all once
    /// [`FAILURE_RETRY_PASSES`] have gone by.
    pub(crate) fn age_failures(&mut self) {
        if self.failures.is_empty() {
            return;
        }
        self.failure_passes += 1;
        if self.failure_passes >= FAILURE_RETRY_PASSES {
            self.forget_failures();
        }
    }

    /// The maps that loaded from their PNG although cooking was asked for, with why, in path order.
    pub(crate) fn cook_misses(&self) -> Vec<(String, CookMiss)> {
        self.cook_misses
            .iter()
            .map(|((path, _), miss)| (path.clone(), miss.clone()))
            .collect()
    }

    /// Attaches (or detaches) the extract observer.
    pub(crate) fn set_extract_observer(&mut self, observer: Option<Box<dyn ExtractObserver>>) {
        self.extract_observer = observer;
    }

    /// Detaches and returns the extract observer, so a caller can restore it after borrowing the slot.
    pub(crate) fn take_extract_observer(&mut self) -> Option<Box<dyn ExtractObserver>> {
        self.extract_observer.take()
    }

    /// The attached extract observer, if any.
    pub(crate) fn extract_observer_mut(&mut self) -> Option<&mut (dyn ExtractObserver + '_)> {
        match self.extract_observer.as_mut() {
            Some(observer) => Some(observer.as_mut()),
            None => None,
        }
    }

    /// Detaches and returns the load observer.
    pub(crate) fn take_observer(&mut self) -> Option<Box<dyn LoadObserver>> {
        self.observer.take()
    }

    /// Whether a load observer is attached.
    pub(crate) fn observed(&self) -> bool {
        self.observer.is_some()
    }

    /// The attached observer, if any — the engine brackets its scene phase through it.
    pub(crate) fn observer_mut(&mut self) -> Option<&mut dyn LoadObserver> {
        self.observer.as_deref_mut()
    }

    /// Forgets the voxelization — a loaded scene never reuses the previous world's.
    ///
    /// [`gi_hold`](AssetCache::gi_hold) deliberately survives: it is the *host's* lever, not the
    /// scene's. The unheld frame every rebuild forces is what makes the new world build once and then
    /// hold from there.
    pub(crate) fn reset_voxelization(&mut self) {
        self.voxelization = None;
        self.gi_stale = false;
    }

    /// Holds (or releases) the voxelized baseline — see [`gi_hold`](AssetCache::gi_hold).
    pub(crate) fn set_gi_hold(&mut self, hold: bool) {
        self.gi_hold = hold;
    }

    /// Clears the per-extract staleness flag. Called at the top of every extract, so a scene that
    /// stops authoring a `GiVolume` — and therefore never consults the voxelization cache again —
    /// cannot leave the last answer standing.
    pub(crate) fn clear_gi_stale(&mut self) {
        self.gi_stale = false;
    }

    /// Whether this extract served a *held* voxelization: a grid built under a signature the world
    /// no longer has. See [`gi_stale`](AssetCache::gi_stale).
    pub(crate) fn gi_stale(&self) -> bool {
        self.gi_stale
    }

    /// The world voxelization built under `signature`, or `None` on a miss. The inner `Option` is the
    /// voxelizer's own answer — a scene that places nothing rasterizable has no grid, and caching
    /// that verdict is what keeps it from being re-derived every frame too.
    ///
    /// The single entry is the whole design: one world is voxelized at a time, so a map would only
    /// grow without ever being read twice.
    pub(crate) fn voxelization(&self, signature: u64) -> Option<Option<Arc<WorldVoxelGrid>>> {
        self.voxelization
            .as_ref()
            .filter(|(key, _)| *key == signature)
            .map(|(_, grid)| grid.clone())
    }

    /// The retained voxelization, whatever signature built it, when this cache is holding the GI
    /// baseline — the voxelizer's second chance after [`voxelization`](AssetCache::voxelization)
    /// misses, and the read that records staleness.
    ///
    /// `None` when nothing is held or nothing is retained: the first-ever build is never held,
    /// because a grid that does not exist cannot be served. The key is deliberately left alone — the
    /// held grid remains the baseline until a rebake replaces it.
    pub(crate) fn held_voxelization(&mut self) -> Option<Option<Arc<WorldVoxelGrid>>> {
        if !self.gi_hold {
            return None;
        }
        let (_, grid) = self.voxelization.as_ref()?;
        self.gi_stale = true;
        Some(grid.clone())
    }

    /// Records `grid` as the voxelization of the instance set `signature` stands for, replacing any
    /// previous entry.
    pub(crate) fn set_voxelization(
        &mut self,
        signature: u64,
        grid: Option<Arc<WorldVoxelGrid>>,
    ) -> Option<Arc<WorldVoxelGrid>> {
        self.voxelization = Some((signature, grid.clone()));
        grid
    }

    /// Reads the texture at `rel` (relative to `project_root`) **through** the cache: a hit returns the
    /// decoded image, a miss loads, decodes and premultiplies the PNG once, inserts it, and returns
    /// it. This is the per-frame hot path — a hit is a pure map lookup, no filesystem access.
    ///
    /// # Errors
    /// `L1101`/`L1102` if a *missing* entry cannot be read or decoded (propagated from
    /// [`ludion_assets::load_png`]); a cached entry never errors.
    pub(crate) fn load(
        &mut self,
        project_root: AssetRoot<'_>,
        rel: &str,
    ) -> Result<&Arc<TextureImage>, Diagnostic> {
        let key = normalize_key(rel);
        if !self.images.contains_key(&key) {
            self.load_entry(project_root, AssetKind::Texture, rel)
                .map_err(first_diagnostic)?;
        } else if let Some(observer) = self.observer.as_deref_mut() {
            observer.reused(AssetKind::Texture.load_kind(), &key);
        }
        Ok(self.images.get(&key).expect("just inserted on a miss"))
    }

    /// Reads the asset mesh at `rel` **through** the cache — the mesh analog of [`load`](AssetCache::load):
    /// a hit returns the decoded [`MeshData`], a miss loads + validates + faceted-normalizes the
    /// document once (via [`crate::mesh_asset`]), inserts it, and returns it.
    ///
    /// # Errors
    /// `L1101`/`L1110`/`L1111` if a *missing* entry cannot be read, parsed, or validated; a cached
    /// entry never errors.
    pub(crate) fn load_mesh(
        &mut self,
        project_root: AssetRoot<'_>,
        rel: &str,
    ) -> Result<&Arc<MeshData>, Vec<Diagnostic>> {
        let key = normalize_key(rel);
        if !self.meshes.contains_key(&key) {
            self.load_entry(project_root, AssetKind::Mesh, rel)?;
        } else if let Some(observer) = self.observer.as_deref_mut() {
            observer.reused(AssetKind::Mesh.load_kind(), &key);
        }
        Ok(self.meshes.get(&key).expect("just inserted on a miss"))
    }

    /// Reads the voxel model at `rel` **through** the cache — the voxel analog of
    /// [`load_mesh`](AssetCache::load_mesh): a hit returns the meshed [`MeshData`], a miss reads +
    /// validates + greedily meshes the document once (via [`crate::voxel_mesh`]), inserts it, and
    /// returns it. So `asset.reload` of a `.voxel.yaml` re-checks + re-meshes, and the hash-keyed GPU
    /// mesh cache re-uploads on the new content hash.
    ///
    /// # Errors
    /// `L1101`/`L1113`/`L1114` if a *missing* entry cannot be read, parsed, or validated; a cached
    /// entry never errors.
    pub(crate) fn load_voxel(
        &mut self,
        project_root: AssetRoot<'_>,
        rel: &str,
    ) -> Result<&Arc<MeshData>, Vec<Diagnostic>> {
        let key = normalize_key(rel);
        if !self.voxels.contains_key(&key) {
            self.load_entry(project_root, AssetKind::Voxel, rel)?;
        } else if let Some(observer) = self.observer.as_deref_mut() {
            observer.reused(AssetKind::Voxel.load_kind(), &key);
        }
        Ok(self.voxels.get(&key).expect("just inserted on a miss"))
    }

    /// Reads the voxel model's occupancy grid at `rel` **through** the cache — the D15 accessor's
    /// cached entry point, mirroring [`load_voxel`](AssetCache::load_voxel) exactly: a hit returns the
    /// [`VoxelStaticGrid`], a miss reads + validates the document once (via
    /// [`crate::voxel_mesh::load_grid`]), inserts it, and returns it. Held in its own map, so a hit
    /// here does not require the mesh cache to have been populated (and vice versa).
    ///
    /// # Errors
    /// `L1101`/`L1113`/`L1114` if a *missing* entry cannot be read, parsed, or validated; a cached
    /// entry never errors.
    pub(crate) fn load_voxel_grid(
        &mut self,
        project_root: AssetRoot<'_>,
        rel: &str,
    ) -> Result<&Arc<VoxelStaticGrid>, Vec<Diagnostic>> {
        let key = normalize_key(rel);
        if !self.voxel_grids.contains_key(&key) {
            self.load_entry(project_root, AssetKind::VoxelGrid, rel)?;
        } else if let Some(observer) = self.observer.as_deref_mut() {
            observer.reused(AssetKind::VoxelGrid.load_kind(), &key);
        }
        Ok(self.voxel_grids.get(&key).expect("just inserted on a miss"))
    }

    /// Reads the sculpt model at `rel` **through** the cache — the sculpt analog of
    /// [`load_voxel`](AssetCache::load_voxel): a hit returns the meshed [`MeshData`], a miss reads +
    /// validates + meshes the document once (via [`crate::sculpt_mesh`]), inserts it, and returns it.
    ///
    /// # Errors
    /// `L1101`/`L1156`/`L1157`/`L1160` if a *missing* entry cannot be read, parsed, validated, or
    /// meshed; a cached entry never errors.
    pub(crate) fn load_sculpt(
        &mut self,
        project_root: AssetRoot<'_>,
        rel: &str,
    ) -> Result<&Arc<MeshData>, Vec<Diagnostic>> {
        let key = normalize_key(rel);
        if !self.sculpts.contains_key(&key) {
            self.load_entry(project_root, AssetKind::Sculpt, rel)?;
        } else if let Some(observer) = self.observer.as_deref_mut() {
            observer.reused(AssetKind::Sculpt.load_kind(), &key);
        }
        Ok(self.sculpts.get(&key).expect("just inserted on a miss"))
    }

    /// Reads the multi-part assembly at `rel` **through** the cache — the assembly analog of
    /// [`load_voxel`](AssetCache::load_voxel): a miss reads the assembly document *and every part and
    /// clip file it names*, validates and meshes them once (via [`crate::assembly_mesh`]), inserts the
    /// result, and returns it. The part meshes are `Arc`-shared, so a hit costs one refcount bump per
    /// part.
    ///
    /// A miss also records the reload dependency set — this assembly's key under each of its source
    /// files — which is what lets an edit to a *part* or a *clip* invalidate the assemblies read out
    /// of it.
    ///
    /// # Errors
    /// `L1101`/`L1137`–`L1142` if a *missing* entry (or one of its parts or clips) cannot be read,
    /// parsed, or validated; a cached entry never errors.
    pub(crate) fn load_assembly(
        &mut self,
        project_root: AssetRoot<'_>,
        rel: &str,
    ) -> Result<&Arc<AssemblyData>, Vec<Diagnostic>> {
        let key = normalize_key(rel);
        if !self.assemblies.contains_key(&key) {
            self.load_entry(project_root, AssetKind::Assembly, rel)?;
        } else if let Some(observer) = self.observer.as_deref_mut() {
            observer.reused(AssetKind::Assembly.load_kind(), &key);
        }
        Ok(self.assemblies.get(&key).expect("just inserted on a miss"))
    }

    /// The body of the assembly at `rel`: what the cache holds under that key, else what a
    /// remembered failure says, else a read of the document alone.
    ///
    /// # Errors
    /// `L1101`/`L1137`/`L1138` if the document of an assembly the cache does not hold cannot be read
    /// or validated.
    pub(crate) fn assembly_body(
        &mut self,
        project_root: AssetRoot<'_>,
        rel: &str,
    ) -> Result<AssemblyBody, Vec<Diagnostic>> {
        let key = normalize_key(rel);
        if self.characters.contains_key(&key) {
            return Ok(AssemblyBody::Skinned);
        }
        if self.assemblies.contains_key(&key) {
            return Ok(AssemblyBody::Voxel);
        }
        // Only a skinned document's load fails as a character. A failed voxel load says nothing: a
        // skinned document fails it too.
        if self
            .failures
            .contains_key(&(AssetKind::Character, rel.to_string()))
        {
            return Ok(AssemblyBody::Skinned);
        }
        let memo = (AssetKind::AssemblyBody, rel.to_string());
        if let Some(failure) = self.failures.get(&memo) {
            return Err(failure.clone());
        }
        crate::assembly_body::body_of(project_root, rel).inspect_err(|failure| {
            self.failures.insert(memo, failure.clone());
        })
    }

    /// Splits assembly keys into the voxel ones and the skinned ones. A key whose document cannot be
    /// read counts as voxel, so its error surfaces from the voxel load.
    fn partition_assemblies(
        &mut self,
        project_root: AssetRoot<'_>,
        keys: &BTreeSet<String>,
    ) -> (BTreeSet<String>, BTreeSet<String>) {
        let (skinned, voxel) = keys
            .iter()
            .cloned()
            .partition(|key| self.assembly_body(project_root, key) == Ok(AssemblyBody::Skinned));
        (voxel, skinned)
    }

    /// Reads the character at `rel` **through** the cache: a miss reads the character document, its
    /// mesh document and every clip file it names, and records all of them as reload dependencies —
    /// so editing the mesh or any clip re-reads the character.
    ///
    /// # Errors
    /// `L1101`/`L1137`/`L1138` (and the mesh's and clips' own codes) if a *missing* entry cannot be
    /// read, parsed, or validated; a cached entry never errors.
    pub(crate) fn load_character(
        &mut self,
        project_root: AssetRoot<'_>,
        rel: &str,
    ) -> Result<&Arc<CharacterData>, Vec<Diagnostic>> {
        let key = normalize_key(rel);
        if !self.characters.contains_key(&key) {
            self.load_entry(project_root, AssetKind::Character, rel)?;
        } else if let Some(observer) = self.observer.as_deref_mut() {
            observer.reused(AssetKind::Character.load_kind(), &key);
        }
        Ok(self.characters.get(&key).expect("just inserted on a miss"))
    }

    /// The memo of ground-marker visibility profiles.
    pub(crate) fn clip_cache_mut(&mut self) -> &mut crate::ground_marker_clip::ClipCache {
        &mut self.clip_cache
    }

    #[cfg(test)]
    /// Rays the ground-marker clip has cast through this cache.
    pub(crate) fn clip_rays_cast(&self) -> u64 {
        self.clip_cache.rays_cast()
    }

    /// The part rests of prefab source `text`, parsed once per distinct text; `None` when it does
    /// not parse.
    pub(crate) fn prefab_rests(&mut self, text: &str) -> Option<Arc<PartRests>> {
        let hash = ludion_assets::content_hash(text.as_bytes());
        if let Some(rests) = self.prefab_rests.get(&hash) {
            return Some(Arc::clone(rests));
        }
        let rests = Arc::new(crate::prefab_clips::rests_of_text(text)?);
        if self.prefab_rests.len() >= PREFAB_RESTS_CAP {
            self.prefab_rests.clear();
        }
        self.prefab_rests.insert(hash, Arc::clone(&rests));
        Some(rests)
    }

    /// Reads the prefab at `rel` **through** the cache for the clips it binds: a miss reads the
    /// prefab document and every clip file it names and records all of them as reload dependencies,
    /// so editing the prefab or any clip re-reads the entry.
    ///
    /// # Errors
    /// `L1101`/`L1226` (and the clips' own codes) if a *missing* entry cannot be read or parsed; a
    /// cached entry never errors.
    pub(crate) fn load_prefab_clips(
        &mut self,
        project_root: AssetRoot<'_>,
        rel: &str,
    ) -> Result<&Arc<PrefabClipData>, Vec<Diagnostic>> {
        let key = normalize_key(rel);
        if !self.prefab_clips.contains_key(&key) {
            self.load_entry(project_root, AssetKind::PrefabClips, rel)?;
        } else if let Some(observer) = self.observer.as_deref_mut() {
            observer.reused(AssetKind::PrefabClips.load_kind(), &key);
        }
        Ok(self
            .prefab_clips
            .get(&key)
            .expect("just inserted on a miss"))
    }

    /// Reads the atlas at `rel` **through** the cache — the atlas analog of
    /// [`load_assembly`](AssetCache::load_assembly): a miss reads and validates the `kind: atlas`
    /// document *and* decodes the packed colour PNG it names, inserts the pair, and returns it. A hit
    /// costs one refcount bump, which is what keeps a tilemap of a thousand cells off the disk.
    ///
    /// A miss also records both source files as reload dependencies, so an edit to either half of the
    /// pair — and `ludion generate-atlas` rewrites both — invalidates the entry.
    ///
    /// # Errors
    /// `L1101`/`L1102`/`L1153` if a *missing* entry (or its colour map) cannot be read, decoded,
    /// parsed, or validated; a cached entry never errors.
    pub(crate) fn load_atlas(
        &mut self,
        project_root: AssetRoot<'_>,
        rel: &str,
    ) -> Result<&Arc<AtlasAsset>, Vec<Diagnostic>> {
        let key = normalize_key(rel);
        if !self.atlases.contains_key(&key) {
            self.load_entry(project_root, AssetKind::Atlas, rel)?;
        } else if let Some(observer) = self.observer.as_deref_mut() {
            observer.reused(AssetKind::Atlas.load_kind(), &key);
        }
        Ok(self.atlases.get(&key).expect("just inserted on a miss"))
    }

    /// The `.cube` LUT at `rel`, decoded once and shared; the dependency edge is the file itself.
    ///
    /// # Errors
    /// `L1101`/`L2390`/`L2391` if a *missing* entry cannot be read or parsed; a cached entry never
    /// errors.
    pub(crate) fn load_lut(
        &mut self,
        project_root: AssetRoot<'_>,
        rel: &str,
    ) -> Result<&Arc<CubeLut>, Vec<Diagnostic>> {
        let key = normalize_key(rel);
        if !self.luts.contains_key(&key) {
            self.load_entry(project_root, AssetKind::Lut, rel)?;
        } else if let Some(observer) = self.observer.as_deref_mut() {
            observer.reused(AssetKind::Lut.load_kind(), &key);
        }
        Ok(self.luts.get(&key).expect("just inserted on a miss"))
    }

    /// Loads the `kind` entry at `rel` from disk, inserts it, and records the files it was read out
    /// of. The one place each kind's loader is named: a miss and a reload both run through it.
    fn load_entry(
        &mut self,
        project_root: AssetRoot<'_>,
        kind: AssetKind,
        rel: &str,
    ) -> Result<(), Vec<Diagnostic>> {
        let memo = (kind, rel.to_string());
        if let Some(failure) = self.failures.get(&memo) {
            return Err(failure.clone());
        }
        let key = normalize_key(rel);
        let phase = kind.load_phase();
        if let Some(observer) = self.observer.as_deref_mut() {
            observer.begin(phase);
        }
        let decoded = self.decode_entry(project_root, kind, rel, &key);
        if let Some(observer) = self.observer.as_deref_mut() {
            observer.end(phase);
        }
        let sources = match decoded {
            Ok(sources) => sources,
            Err(failure) => {
                self.failures.insert(memo, failure.clone());
                return Err(failure);
            }
        };
        self.report_decoded(project_root, kind, &key, &sources);
        self.record_sources(kind, &key, &sources);
        Ok(())
    }

    /// Tells the observer what a successful decode read: per source, the on-disk size. A mesh's
    /// document is a mesh decode and each map after it a texture decode; every other kind is one
    /// decode over all its files.
    fn report_decoded(
        &mut self,
        project_root: AssetRoot<'_>,
        kind: AssetKind,
        key: &str,
        sources: &SourceFiles,
    ) {
        // A cooked map read its store file instead of decoding its PNG.
        if let AssetKind::MeshMap(role) = kind
            && let Some(MapImage::Cooked(cooked)) = self.mesh_maps.get(&(key.to_string(), role))
        {
            let bytes = cooked
                .image
                .levels
                .iter()
                .map(|level| level.len() as u64)
                .sum();
            if let Some(observer) = self.observer.as_deref_mut() {
                observer.cooked(LoadKind::Texture, key, bytes);
            }
            return;
        }
        let Some(observer) = self.observer.as_deref_mut() else {
            return;
        };
        let len = |src: &(String, u64)| {
            std::fs::metadata(project_root.dir().join(&src.0)).map_or(0, |m| m.len())
        };
        if kind == AssetKind::Mesh {
            // The maps report themselves: each is its own entry, shared by every mesh naming it.
            if let Some(document) = sources.first() {
                observer.decoded(LoadKind::Mesh, key, len(document));
            }
        } else {
            observer.decoded(kind.load_kind(), key, sources.iter().map(len).sum());
        }
    }

    /// The role the map at the normalized `key` is held under for the colour role `space`: cooked
    /// when the lookup covers the path, else the PNG's.
    fn role(&self, key: &str, space: ColorSpace, alpha_cutoff: Option<f32>) -> MapRole {
        match &self.cook {
            Some(cook) if cook.cap_for(key).is_some() => MapRole::Cooked(CookRole {
                space,
                alpha_cutoff: alpha_cutoff.map(f32::to_bits),
            }),
            _ => MapRole::Source,
        }
    }

    /// The map held under `key` and `role`, with the content hash of its PNG.
    fn held_map(&self, key: &str, role: MapRole) -> Option<(MapImage, u64)> {
        let image = self.mesh_maps.get(&(key.to_string(), role))?;
        Some((
            image.clone(),
            self.source_hashes.get(key).copied().unwrap_or(0),
        ))
    }

    /// Reads a mesh's map at the resolved path `resolved` in the colour role `space` **through**
    /// the cache: a hit is reported as reused, a miss decodes it once as an entry of its own. Every
    /// mesh naming one file shares the `Arc`, so the decode counter is unique files, not references.
    /// A decal's maps never come here: they read their PNG ([`load_source_map`](Self::load_source_map)).
    pub(crate) fn load_map(
        &mut self,
        project_root: AssetRoot<'_>,
        resolved: &str,
        space: ColorSpace,
        alpha_cutoff: Option<f32>,
    ) -> Result<(MapImage, u64), Diagnostic> {
        let role = self.role(&normalize_key(resolved), space, alpha_cutoff);
        self.load_map_as(project_root, resolved, role)
    }

    /// Reads a decal's map: always the PNG, because sync cooks only the maps mesh documents name,
    /// so a cook lookup covering the path would be a miss nothing can fill.
    pub(crate) fn load_source_map(
        &mut self,
        project_root: AssetRoot<'_>,
        resolved: &str,
    ) -> Result<(MapImage, u64), Diagnostic> {
        self.load_map_as(project_root, resolved, MapRole::Source)
    }

    fn load_map_as(
        &mut self,
        project_root: AssetRoot<'_>,
        resolved: &str,
        role: MapRole,
    ) -> Result<(MapImage, u64), Diagnostic> {
        let key = normalize_key(resolved);
        if !self.mesh_maps.contains_key(&(key.clone(), role)) {
            self.load_entry(project_root, AssetKind::MeshMap(role), resolved)
                .map_err(first_diagnostic)?;
        } else if let Some(observer) = self.observer.as_deref_mut() {
            observer.reused(LoadKind::Texture, &key);
        }
        Ok(self.held_map(&key, role).expect("just inserted on a miss"))
    }

    /// Runs `kind`'s loader, inserts the result under `key`, and returns the files it read.
    fn decode_entry(
        &mut self,
        project_root: AssetRoot<'_>,
        kind: AssetKind,
        rel: &str,
        key: &str,
    ) -> Result<SourceFiles, Vec<Diagnostic>> {
        if kind == AssetKind::Mesh {
            let (doc, hash) = crate::mesh_asset::read_document(project_root, rel)?;
            let levels = crate::mesh_asset::read_levels(project_root, rel, &doc)?;
            let (mesh, sources) = crate::mesh_asset::assemble(
                rel,
                &doc,
                hash,
                &levels,
                &mut |path, space, alpha_cutoff| {
                    self.load_map(project_root, path, space, alpha_cutoff)
                },
            )?;
            self.meshes.insert(key.to_string(), Arc::new(mesh));
            return Ok(sources);
        }
        let cook = self.cook.clone();
        let (decoded, sources) = decode(project_root, kind, rel, key, cook.as_deref())?;
        self.insert(key, decoded);
        Ok(sources)
    }

    /// Inserts a decoded entry under `key`.
    fn insert(&mut self, key: &str, decoded: Decoded) {
        let key = key.to_string();
        match decoded {
            Decoded::Texture(image) => self.images.insert(key, image).is_some(),
            Decoded::MeshMap(role, image, miss) => {
                if let MapRole::Cooked(cook_role) = role {
                    match miss {
                        Some(miss) => self.cook_misses.insert((key.clone(), cook_role), miss),
                        None => self.cook_misses.remove(&(key.clone(), cook_role)),
                    };
                }
                self.mesh_maps.insert((key, role), image).is_some()
            }
            Decoded::Voxel(mesh) => self.voxels.insert(key, Arc::new(mesh)).is_some(),
            Decoded::VoxelGrid(grid) => self.voxel_grids.insert(key, Arc::new(grid)).is_some(),
            Decoded::Sculpt(mesh) => self.sculpts.insert(key, Arc::new(mesh)).is_some(),
            Decoded::Assembly(data) => self.assemblies.insert(key, Arc::new(data)).is_some(),
            Decoded::Character(data) => self.characters.insert(key, Arc::new(data)).is_some(),
            Decoded::PrefabClips(data) => self.prefab_clips.insert(key, Arc::new(data)).is_some(),
            Decoded::Atlas(asset) => self.atlases.insert(key, Arc::new(asset)).is_some(),
            Decoded::Lut(lut) => self.luts.insert(key, Arc::new(lut)).is_some(),
        };
    }

    /// Whether the `kind` entry at `key` is cached.
    fn holds(&self, kind: AssetKind, key: &str) -> bool {
        match kind {
            AssetKind::Texture => self.images.contains_key(key),
            AssetKind::MeshMap(role) => self.mesh_maps.contains_key(&(key.to_string(), role)),
            AssetKind::Mesh => self.meshes.contains_key(key),
            AssetKind::Voxel => self.voxels.contains_key(key),
            AssetKind::VoxelGrid => self.voxel_grids.contains_key(key),
            AssetKind::Sculpt => self.sculpts.contains_key(key),
            AssetKind::Assembly => self.assemblies.contains_key(key),
            AssetKind::Character => self.characters.contains_key(key),
            AssetKind::AssemblyBody => false,
            AssetKind::PrefabClips => self.prefab_clips.contains_key(key),
            AssetKind::Atlas => self.atlases.contains_key(key),
            AssetKind::Lut => self.luts.contains_key(key),
        }
    }

    /// Removes the `kind` entry at `key` from its map.
    fn remove_entry(&mut self, kind: AssetKind, key: &str) {
        match kind {
            AssetKind::Texture => self.images.remove(key).is_some(),
            AssetKind::MeshMap(role) => {
                if let MapRole::Cooked(cook_role) = role {
                    self.cook_misses.remove(&(key.to_string(), cook_role));
                }
                self.mesh_maps.remove(&(key.to_string(), role)).is_some()
            }
            AssetKind::Mesh => self.meshes.remove(key).is_some(),
            AssetKind::Voxel => self.voxels.remove(key).is_some(),
            AssetKind::VoxelGrid => self.voxel_grids.remove(key).is_some(),
            AssetKind::Sculpt => self.sculpts.remove(key).is_some(),
            AssetKind::Assembly => self.assemblies.remove(key).is_some(),
            AssetKind::Character => self.characters.remove(key).is_some(),
            AssetKind::AssemblyBody => false,
            AssetKind::PrefabClips => self.prefab_clips.remove(key).is_some(),
            AssetKind::Atlas => self.atlases.remove(key).is_some(),
            AssetKind::Lut => self.luts.remove(key).is_some(),
        };
    }

    /// Records the `kind` entry `key` as depending on each of `sources` (its own document included).
    fn record_sources(&mut self, kind: AssetKind, key: &str, sources: &SourceFiles) {
        for (source, hash) in sources {
            let source = normalize_key(source);
            self.source_hashes.insert(source.clone(), *hash);
            let dependents = self.dependents.entry(source).or_default();
            if !dependents.iter().any(|(k, held)| *k == kind && held == key) {
                dependents.push((kind, key.to_string()));
            }
        }
    }

    /// Forgets every dependency edge pointing at the `kind` entry `key` — the first half of
    /// re-reading it, since a re-read entry may name a different set of files than the one it was
    /// built from (a re-baked atlas its colour map, a re-authored assembly its parts).
    fn drop_sources(&mut self, kind: AssetKind, key: &str) {
        for dependents in self.dependents.values_mut() {
            dependents.retain(|(k, held)| !(*k == kind && held == key));
        }
        self.dependents
            .retain(|_, dependents| !dependents.is_empty());
    }

    /// Forgets the recorded hash of every file no entry depends on any more.
    fn prune_source_hashes(&mut self) {
        let dependents = &self.dependents;
        self.source_hashes
            .retain(|file, _| dependents.contains_key(file));
    }

    /// Removes every entry built from the file `file`, with its edges, and returns them — the drop
    /// half of [`reload`](AssetCache::reload).
    fn drop_dependents(&mut self, file: &str) -> Vec<(AssetKind, String)> {
        let dependents = self.dependents.get(file).cloned().unwrap_or_default();
        for (kind, entry) in &dependents {
            self.remove_entry(*kind, entry);
            self.drop_sources(*kind, entry);
        }
        self.prune_source_hashes();
        dependents
    }

    /// Drops every dependency edge whose entry the cache no longer holds, and the hashes of files
    /// nothing depends on any more.
    fn retain_edges(&mut self) {
        let mut dependents = std::mem::take(&mut self.dependents);
        for entries in dependents.values_mut() {
            entries.retain(|(kind, entry)| self.holds(*kind, entry));
        }
        dependents.retain(|_, entries| !entries.is_empty());
        self.dependents = dependents;
        self.prune_source_hashes();
    }

    /// Evicts every entry the world `refs` describes no longer names, with the dependency edges,
    /// hashes and cook fallbacks that went with them, and forgets the remembered failures. Nothing is
    /// re-read or re-hashed: a cold authoring moment (a rebuild, a scene save) is the only caller.
    pub(crate) fn evict_unreferenced(&mut self, refs: &AssetRefs) {
        self.forget_failures();
        let mut meshes = refs.meshes.clone();
        for key in &refs.assemblies {
            if let Some(character) = self.characters.get(key) {
                meshes.insert(normalize_key(&character.mesh));
            }
        }
        self.images.retain(|key, _| refs.textures.contains(key));
        self.atlases.retain(|key, _| refs.atlases.contains(key));
        self.luts.retain(|key, _| refs.luts.contains(key));
        self.voxels.retain(|key, _| refs.voxels.contains(key));
        self.voxel_grids.retain(|key, _| refs.voxels.contains(key));
        self.sculpts.retain(|key, _| refs.sculpts.contains(key));
        self.assemblies
            .retain(|key, _| refs.assemblies.contains(key));
        self.characters
            .retain(|key, _| refs.assemblies.contains(key));
        self.prefab_clips
            .retain(|key, _| refs.prefab_clips.contains(key));
        self.prefab_rests.clear();
        self.meshes.retain(|key, _| meshes.contains(key));
        self.retain_edges();

        // A map no kept mesh binds and no decal names is an orphan.
        let dependents = &self.dependents;
        let decal_maps: BTreeSet<&str> = refs.maps.iter().map(String::as_str).collect();
        self.mesh_maps.retain(|(key, _), _| {
            decal_maps.contains(key.as_str())
                || dependents
                    .get(key)
                    .is_some_and(|entries| entries.iter().any(|(kind, _)| *kind == AssetKind::Mesh))
        });
        let maps = &self.mesh_maps;
        self.cook_misses
            .retain(|(key, role), _| maps.contains_key(&(key.clone(), MapRole::Cooked(*role))));
        self.retain_edges();
    }

    /// Makes the cache the working set of the world `refs` describes: evicts every entry the world
    /// does not name (so a chain of scene switches never accumulates), then re-hashes the files the
    /// survivors were built from and drops every entry built from one that changed or vanished — the
    /// drop half of [`reload`](AssetCache::reload). The next [`prefetch`](AssetCache::prefetch) then
    /// decodes only what is missing, so a scene that shares assets with the last one pays for none of
    /// them. Nothing is re-read here: a dropped entry is decoded again by the prefetch or the extract
    /// that follows.
    pub(crate) fn retain_referenced(&mut self, project_root: AssetRoot<'_>, refs: &AssetRefs) {
        self.evict_unreferenced(refs);

        self.begin(LoadPhase::Other);
        let changed: Vec<String> = {
            let files: Vec<(&String, u64)> = self
                .source_hashes
                .iter()
                .map(|(file, hash)| (file, *hash))
                .collect();
            let moved = ludion_core::par::par_map_chunks(&files, 16, |_, chunk| {
                chunk
                    .iter()
                    .map(|(file, hash)| {
                        !matches!(
                            ludion_assets::load_bytes(project_root, file),
                            Ok(bytes) if bytes.hash == *hash
                        )
                    })
                    .collect::<Vec<bool>>()
            });
            files
                .iter()
                .zip(moved.into_iter().flatten())
                .filter(|(_, moved)| *moved)
                .map(|((file, _), _)| (*file).clone())
                .collect()
        };
        self.end(LoadPhase::Other);
        for file in changed {
            if self.dependents.contains_key(&file) {
                self.drop_dependents(&file);
            }
        }
        self.prune_source_hashes();
    }

    /// Tells the observer the walked `kind` entry `key` was already held.
    fn report_reused(&mut self, kind: AssetKind, key: &str) {
        if let Some(observer) = self.observer.as_deref_mut() {
            observer.reused(kind.load_kind(), key);
        }
    }

    /// Decodes every asset in `refs` that the cache does not hold yet, in parallel, ahead of the
    /// extract that would otherwise decode them one by one on its miss path.
    ///
    /// Four stages, each decoding on worker threads and then inserting **serially in sorted key
    /// order**, so the cache's contents, its dependency edges and the observer's event order never
    /// depend on the thread count: the non-mesh kinds, every mesh document, every map those
    /// documents bind or a decal names plus the loose textures, and last each mesh assembled against its maps. A
    /// failure is dropped here — the extract that follows hits the same miss and reports it with its
    /// usual diagnostic. Every walked entry the cache already holds (and the maps of a held mesh) is
    /// reported as reused. Voxel grids and the volley, particle, UI and font atlases stay lazy.
    ///
    /// Never call the observer from a worker: each stage is bracketed from this thread.
    pub(crate) fn prefetch(&mut self, project_root: AssetRoot<'_>, refs: &AssetRefs) {
        // Stage 1: everything that is one file-set to one value.
        let (voxel, skinned) = self.partition_assemblies(project_root, &refs.assemblies);
        let mut jobs: Vec<(AssetKind, &str)> = Vec::new();
        let others: [(AssetKind, &BTreeSet<String>); 7] = [
            (AssetKind::Atlas, &refs.atlases),
            (AssetKind::Lut, &refs.luts),
            (AssetKind::Voxel, &refs.voxels),
            (AssetKind::Sculpt, &refs.sculpts),
            (AssetKind::Assembly, &voxel),
            (AssetKind::Character, &skinned),
            (AssetKind::PrefabClips, &refs.prefab_clips),
        ];
        for (kind, keys) in others {
            for key in keys {
                if self.holds(kind, key) {
                    self.report_reused(kind, key);
                } else {
                    jobs.push((kind, key.as_str()));
                }
            }
        }
        self.decode_batch(project_root, LoadPhase::Other, &jobs);

        // Stage 2: the mesh documents — the named ones and each character's skinned mesh.
        let mut mesh_keys: BTreeSet<String> = refs.meshes.clone();
        for key in &skinned {
            if let Some(character) = self.characters.get(key) {
                mesh_keys.insert(normalize_key(&character.mesh));
            }
        }
        let (held, todo): (Vec<&String>, Vec<&String>) = mesh_keys
            .iter()
            .partition(|key| self.meshes.contains_key(*key));
        for key in &held {
            self.report_reused(AssetKind::Mesh, key);
        }
        let held: BTreeSet<&String> = held.into_iter().collect();
        let held_maps: Vec<String> = self
            .dependents
            .iter()
            .filter(|(_, entries)| {
                entries
                    .iter()
                    .any(|(kind, _)| matches!(kind, AssetKind::MeshMap(_)))
                    && entries
                        .iter()
                        .any(|(kind, entry)| *kind == AssetKind::Mesh && held.contains(entry))
            })
            .map(|(file, _)| file.clone())
            .collect();
        for map in &held_maps {
            self.report_reused(AssetKind::MeshMap(MapRole::Source), map);
        }
        self.begin(LoadPhase::Mesh);
        let read = ludion_core::par::par_map_chunks(&todo, 1, |_, chunk| {
            crate::mesh_asset::read_document(project_root, chunk[0]).and_then(|(doc, hash)| {
                let levels = crate::mesh_asset::read_levels(project_root, chunk[0], &doc)?;
                Ok((doc, hash, levels))
            })
        });
        self.end(LoadPhase::Mesh);
        let docs: Vec<(
            &String,
            MeshDocument,
            u64,
            Vec<crate::mesh_asset::LevelSource>,
        )> = todo
            .into_iter()
            .zip(read)
            .filter_map(|(key, read)| {
                read.ok()
                    .map(|(doc, hash, levels)| (key, doc, hash, levels))
            })
            .collect();

        // Stage 3: the maps those documents bind and the decals name, then the loose textures.
        let mut maps: BTreeSet<(String, MapRole)> = BTreeSet::new();
        for (key, doc, _, levels) in &docs {
            let paths = crate::mesh_asset::map_paths(key, doc).into_iter().chain(
                levels
                    .iter()
                    .flat_map(|level| crate::mesh_asset::map_paths(&level.rel, &level.doc)),
            );
            for (path, space, alpha_cutoff) in paths {
                let path = normalize_key(&path);
                let role = self.role(&path, space, alpha_cutoff);
                if self.mesh_maps.contains_key(&(path.clone(), role)) {
                    self.report_reused(AssetKind::MeshMap(role), &path);
                } else {
                    maps.insert((path, role));
                }
            }
        }
        for path in &refs.maps {
            let role = MapRole::Source;
            if self.mesh_maps.contains_key(&(path.clone(), role)) {
                self.report_reused(AssetKind::MeshMap(role), path);
            } else {
                maps.insert((path.clone(), role));
            }
        }
        let mut jobs: Vec<(AssetKind, &str)> = maps
            .iter()
            .map(|(key, role)| (AssetKind::MeshMap(*role), key.as_str()))
            .collect();
        for key in &refs.textures {
            if self.images.contains_key(key) {
                self.report_reused(AssetKind::Texture, key);
            } else {
                jobs.push((AssetKind::Texture, key.as_str()));
            }
        }
        self.decode_batch(project_root, LoadPhase::Texture, &jobs);

        // Stage 4: assemble serially, so a map that failed above fails its meshes the same way.
        self.begin(LoadPhase::Mesh);
        for (key, doc, hash, levels) in &docs {
            let assembled = crate::mesh_asset::assemble(
                key,
                doc,
                *hash,
                levels,
                &mut |path, space, alpha_cutoff| {
                    let path_key = normalize_key(path);
                    self.held_map(&path_key, self.role(&path_key, space, alpha_cutoff))
                        .ok_or_else(|| {
                            Diagnostic::new(
                                ludion_core::codes::code::ASSET_FILE_IO,
                                format!("mesh map `{path}` did not decode"),
                            )
                        })
                },
            );
            if let Ok((mesh, sources)) = assembled {
                self.meshes.insert((*key).clone(), Arc::new(mesh));
                self.report_decoded(project_root, AssetKind::Mesh, key, &sources);
                self.record_sources(AssetKind::Mesh, key, &sources);
            }
        }
        self.end(LoadPhase::Mesh);
    }

    /// Decodes `jobs` on the worker pool inside one `phase` bracket, then inserts the successes in
    /// job order and tells the observer about each.
    fn decode_batch(
        &mut self,
        project_root: AssetRoot<'_>,
        phase: LoadPhase,
        jobs: &[(AssetKind, &str)],
    ) {
        if jobs.is_empty() {
            return;
        }
        self.begin(phase);
        let cook = self.cook.clone();
        let decoded = ludion_core::par::par_map_chunks(jobs, 1, |_, chunk| {
            let (kind, key) = chunk[0];
            decode(project_root, kind, key, key, cook.as_deref())
        });
        self.end(phase);
        for ((kind, key), result) in jobs.iter().zip(decoded) {
            if let Ok((value, sources)) = result {
                self.insert(key, value);
                self.report_decoded(project_root, *kind, key, &sources);
                self.record_sources(*kind, key, &sources);
            }
        }
    }

    fn begin(&mut self, phase: LoadPhase) {
        if let Some(observer) = self.observer.as_deref_mut() {
            observer.begin(phase);
        }
    }

    fn end(&mut self, phase: LoadPhase) {
        if let Some(observer) = self.observer.as_deref_mut() {
            observer.end(phase);
        }
    }

    /// Resolves an `asset.reload` path (relative *or* absolute) to the cache key it names, or `None` if
    /// this cache holds no such asset (an uncached path — an editor temp file, unrelated churn — is a
    /// no-op the handler ignores). Searches all asset kinds. See the module docs for the matching rule.
    ///
    /// An entry resolves under **any** of the files it was read from: the key returned for a part file
    /// (or a texture PNG, or a packed atlas image) is that file's own path, which
    /// [`reload`](AssetCache::reload) expands into every entry built out of it.
    pub(crate) fn resolve_key(
        &self,
        project_root: AssetRoot<'_>,
        incoming: &str,
    ) -> Option<String> {
        let direct = normalize_key(incoming);
        if self.dependents.contains_key(&direct) {
            return Some(direct);
        }
        // Cold path: match by canonical filesystem identity, which handles a host-forwarded absolute
        // path whose textual form differs from the scene's relative one.
        let target = std::fs::canonicalize(project_root.dir().join(incoming)).ok()?;
        self.dependents
            .keys()
            .find(|rel| {
                std::fs::canonicalize(project_root.dir().join(rel.as_str()))
                    .ok()
                    .as_ref()
                    == Some(&target)
            })
            .cloned()
    }

    /// Re-reads every entry built from the file `key` and returns that file's own content hash.
    /// `key` must be one [`resolve_key`](AssetCache::resolve_key) returned: a cached entry's document,
    /// or any file one was read from (an assembly's part or clip, a sculpt's voxel or mesh part, a
    /// mesh's texture, an atlas's packed image).
    ///
    /// The whole entry is re-read whichever of its files moved — a bake rewrites an atlas's document
    /// and image together, and a document holding regions the new image no longer has is exactly the
    /// drift `L1152` reports. A stale dependency edge is dropped first, since a re-read entry may name
    /// a different set of files. The returned hash is the changed file's, not a derived one: one file
    /// can feed several entries, so no single derived hash describes the reload, but the file that
    /// moved always does.
    ///
    /// # Errors
    /// `L1101`/`L1102` (texture), `L1101`/`L1110`/`L1111` (mesh, or a sculpt's mesh part),
    /// `L1101`/`L1113`/`L1114` (voxel, or a sculpt's voxel part), `L1101`/`L1156`/`L1157`/`L1160`
    /// (sculpt), `L1101`/`L1137`–`L1142` (assembly), or `L1101`/`L1102`/`L1153` (atlas) if a dependent
    /// entry can no longer be read, decoded, or validated. Every dependent is re-read; the first
    /// failure is returned and each failed entry is left dropped, so a later reload, a one-shot
    /// extract, or a live pass once the failure is forgotten reinstates it.
    pub(crate) fn reload(
        &mut self,
        project_root: AssetRoot<'_>,
        key: &str,
    ) -> Result<u64, Diagnostic> {
        // Drop every entry before any re-read: one failing early must not leave a later dependent
        // (a character built on a mesh that just changed) holding the stale copy.
        self.forget_failures();
        let dependents = self.drop_dependents(key);
        let mut first_error = None;
        for (kind, entry) in &dependents {
            // A mesh re-read above may already have brought a shared map back.
            if self.holds(*kind, entry) {
                continue;
            }
            if let Err(diagnostics) = self.load_entry(project_root, *kind, entry) {
                first_error.get_or_insert_with(|| first_diagnostic(diagnostics));
            }
        }
        if let Some(error) = first_error {
            return Err(error);
        }
        Ok(ludion_assets::load_bytes(project_root, key)?.hash)
    }

    /// How many parsed prefab rests are held. Test-only.
    #[cfg(test)]
    pub(crate) fn prefab_rests_len(&self) -> usize {
        self.prefab_rests.len()
    }

    /// Whether the cache holds a texture at `key` (in its normalized form). Test-only.
    #[cfg(test)]
    pub(crate) fn contains(&self, key: &str) -> bool {
        self.images.contains_key(&normalize_key(key))
    }

    /// Whether the cache holds a mesh map at `key` (in its normalized form). Test-only.
    #[cfg(test)]
    pub(crate) fn contains_mesh_map(&self, key: &str) -> bool {
        let key = normalize_key(key);
        self.mesh_maps.keys().any(|(path, _)| *path == key)
    }

    /// The map held at `key` (in its normalized form) for the colour role `space`, as the cache
    /// would hand it to a mesh. Test-only.
    #[cfg(test)]
    pub(crate) fn mesh_map_image(&self, key: &str, space: ColorSpace) -> Option<MapImage> {
        let key = normalize_key(key);
        self.held_map(&key, self.role(&key, space, None))
            .map(|(image, _)| image)
    }

    /// Whether the cache holds a mesh at `key` (in its normalized form). Test-only.
    #[cfg(test)]
    pub(crate) fn contains_mesh(&self, key: &str) -> bool {
        self.meshes.contains_key(&normalize_key(key))
    }

    /// Whether the cache holds a voxel model at `key` (in its normalized form). Test-only.
    #[cfg(test)]
    pub(crate) fn contains_voxel(&self, key: &str) -> bool {
        self.voxels.contains_key(&normalize_key(key))
    }

    /// Whether the cache holds a voxel model's occupancy grid at `key` (in its normalized form).
    /// Test-only.
    #[cfg(test)]
    pub(crate) fn contains_voxel_grid(&self, key: &str) -> bool {
        self.voxel_grids.contains_key(&normalize_key(key))
    }

    /// Whether the cache holds a sculpt model at `key` (in its normalized form). Test-only.
    #[cfg(test)]
    pub(crate) fn contains_sculpt(&self, key: &str) -> bool {
        self.sculpts.contains_key(&normalize_key(key))
    }

    /// Whether the cache holds a meshed assembly at `key` (in its normalized form). Test-only.
    #[cfg(test)]
    pub(crate) fn contains_assembly(&self, key: &str) -> bool {
        self.assemblies.contains_key(&normalize_key(key))
    }

    /// Whether the cache holds a character at `key` (in its normalized form). Test-only.
    #[cfg(test)]
    pub(crate) fn contains_character(&self, key: &str) -> bool {
        self.characters.contains_key(&normalize_key(key))
    }

    /// Whether the cache holds a prefab's clips at `key` (in its normalized form). Test-only.
    #[cfg(test)]
    pub(crate) fn contains_prefab_clips(&self, key: &str) -> bool {
        self.prefab_clips.contains_key(&normalize_key(key))
    }

    /// Whether the cache holds an atlas at `key` (in its normalized form). Test-only.
    #[cfg(test)]
    pub(crate) fn contains_atlas(&self, key: &str) -> bool {
        self.atlases.contains_key(&normalize_key(key))
    }
}

/// Collapses a mesh-load diagnostic list (always non-empty on error) to its first entry — for the
/// single-`Diagnostic` `reload` reply the `asset.reload` handler expects.
fn first_diagnostic(diagnostics: Vec<Diagnostic>) -> Diagnostic {
    diagnostics
        .into_iter()
        .next()
        .expect("a mesh-load failure always carries at least one diagnostic")
}

/// The one lexical spelling of a path key (separators, `.` and `..` collapsed), so a scene's
/// `assets/x.png`, a Windows `assets\x.png` and a mesh map's `meshes/../assets/x.png` map to the same
/// entry. No filesystem access, so it is safe on the per-frame hot path.
fn normalize_key(rel: &str) -> String {
    ludion_core::paths::normalize(rel)
}

/// The files an asset was read from besides its own document, as `(resolved path, content hash)`.
pub(crate) type SourceFiles = Vec<(String, u64)>;

/// Folds the content hashes of the files an asset was read from into its own: `own` unchanged when
/// there are none, otherwise the hash of their little-endian bytes in order. The result moves when
/// *any* source file changes — which is what invalidates the renderer's GPU cache on a reload — and
/// an asset with no extra sources keeps its file's hash. Hashing a little buffer of the component
/// hashes is the same mixing style the voxelizer's instance signature uses.
pub(crate) fn fold_source_hashes(own: u64, sources: &[u64]) -> u64 {
    if sources.is_empty() {
        return own;
    }
    let mut bytes = Vec::with_capacity((sources.len() + 1) * 8);
    bytes.extend_from_slice(&own.to_le_bytes());
    for hash in sources {
        bytes.extend_from_slice(&hash.to_le_bytes());
    }
    ludion_assets::content_hash(&bytes)
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use ludion_assets::cooked::CookTarget;

    use super::*;

    /// Encodes a 1×1 RGBA PNG to disk for cache tests.
    fn write_png(path: &Path, rgba: [u8; 4]) {
        let mut bytes = Vec::new();
        {
            let mut encoder = png::Encoder::new(&mut bytes, 1, 1);
            encoder.set_color(png::ColorType::Rgba);
            encoder.set_depth(png::BitDepth::Eight);
            let mut writer = encoder.write_header().unwrap();
            writer.write_image_data(&rgba).unwrap();
        }
        std::fs::write(path, &bytes).unwrap();
    }

    #[test]
    fn miss_loads_hit_reuses_reload_re_reads() {
        let dir = std::env::temp_dir().join(format!("ludion-asset-cache-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        write_png(&dir.join("t.png"), [10, 20, 30, 255]);

        let mut cache = AssetCache::new();
        // Miss loads.
        let first = cache
            .load(ludion_assets::AssetRoot::content(&dir), "t.png")
            .unwrap()
            .hash;
        assert!(cache.contains("t.png"));
        // Hit reuses the same decoded bytes even after the file changes on disk (no re-read).
        write_png(&dir.join("t.png"), [200, 100, 50, 255]);
        let cached = cache
            .load(ludion_assets::AssetRoot::content(&dir), "t.png")
            .unwrap()
            .hash;
        assert_eq!(cached, first, "a hit must not re-read the changed file");

        // resolve_key finds the entry from the relative name and from an absolute path.
        let by_rel = cache.resolve_key(ludion_assets::AssetRoot::content(&dir), "t.png");
        assert_eq!(by_rel.as_deref(), Some("t.png"));
        let abs = dir.join("t.png");
        let by_abs = cache.resolve_key(
            ludion_assets::AssetRoot::content(&dir),
            &abs.to_string_lossy(),
        );
        assert_eq!(
            by_abs.as_deref(),
            Some("t.png"),
            "an absolute path resolves to the rel key"
        );

        // Reload drops the entry and re-reads the new bytes → a different hash.
        let reloaded = cache
            .reload(ludion_assets::AssetRoot::content(&dir), "t.png")
            .unwrap();
        assert_ne!(reloaded, first, "reload must pick up the changed file");
        assert_eq!(
            cache
                .load(ludion_assets::AssetRoot::content(&dir), "t.png")
                .unwrap()
                .hash,
            reloaded
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A texture path that climbs out of the content root or is absolute is refused before any read,
    /// even when a real file sits at the escaped location.
    #[test]
    fn a_texture_outside_the_content_root_is_refused() {
        let base =
            std::env::temp_dir().join(format!("ludion-asset-cache-escape-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let root = base.join("content");
        std::fs::create_dir_all(&root).unwrap();
        write_png(&base.join("x.png"), [1, 2, 3, 255]);

        let mut cache = AssetCache::new();
        let absolute = base.join("x.png").to_string_lossy().into_owned();
        for path in ["../x.png", "a/../../x.png", absolute.as_str()] {
            let error = cache
                .load(ludion_assets::AssetRoot::content(&root), path)
                .unwrap_err();
            assert_eq!(
                error.code,
                ludion_core::codes::code::CONTENT_PATH_OUTSIDE_ROOT,
                "{path}: {error:?}"
            );
            assert!(!cache.contains(path), "{path}");
        }
        let _ = std::fs::remove_dir_all(&base);
    }

    /// A hot-reloaded texture is premultiplied like the miss path, so a translucent texture does
    /// not brighten when its file is edited.
    #[test]
    fn a_reloaded_texture_is_premultiplied() {
        let dir = std::env::temp_dir().join(format!(
            "ludion-asset-cache-premultiply-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        write_png(&dir.join("t.png"), [10, 20, 30, 128]);

        let mut cache = AssetCache::new();
        cache
            .load(ludion_assets::AssetRoot::content(&dir), "t.png")
            .unwrap();
        write_png(&dir.join("t.png"), [200, 100, 50, 128]);
        cache
            .reload(ludion_assets::AssetRoot::content(&dir), "t.png")
            .unwrap();

        let raw =
            ludion_assets::load_png(ludion_assets::AssetRoot::content(&dir), "t.png").unwrap();
        let expected = raw.clone().premultiplied().rgba;
        let reloaded = cache
            .load(ludion_assets::AssetRoot::content(&dir), "t.png")
            .unwrap()
            .rgba
            .clone();
        assert_eq!(
            reloaded, expected,
            "reload must premultiply like the miss path"
        );
        assert_ne!(
            reloaded, raw.rgba,
            "the fixture must actually change under premultiply"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// An atlas is cached like an assembly: reachable from **either** of its two source files, so a
    /// bake — which rewrites the document and the packed image together — invalidates the entry
    /// whichever one the watcher reports.
    #[test]
    fn an_atlas_reloads_from_either_of_its_two_files() {
        fn atlas_doc(ppu: &str) -> String {
            format!(
                "format: 1\nkind: atlas\ngenerated_from: a.atlas-source.yaml\npixels_per_unit: \
                 {ppu}\nmaps:\n  color: a.png\nregions:\n  a/one:\n    rect:\n      - 0\n      - \
                 0\n      - 1\n      - 1\n    pivot:\n      - 0.5\n      - 0.5\n"
            )
        }
        let dir =
            std::env::temp_dir().join(format!("ludion-asset-cache-atlas-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        write_png(&dir.join("a.png"), [10, 20, 30, 255]);
        std::fs::write(dir.join("a.atlas.yaml"), atlas_doc("8.0")).unwrap();

        let mut cache = AssetCache::new();
        let first = cache
            .load_atlas(ludion_assets::AssetRoot::content(&dir), "a.atlas.yaml")
            .unwrap()
            .texture
            .hash;
        assert!(cache.contains_atlas("a.atlas.yaml"));

        // Both source files resolve to the atlas's own key — the colour PNG included, which is the
        // half a texture-only cache would miss.
        assert_eq!(
            cache
                .resolve_key(ludion_assets::AssetRoot::content(&dir), "a.atlas.yaml")
                .as_deref(),
            Some("a.atlas.yaml")
        );
        assert_eq!(
            cache
                .resolve_key(ludion_assets::AssetRoot::content(&dir), "a.png")
                .as_deref(),
            Some("a.png"),
            "the packed image is a key of its own, expanded by `reload`"
        );

        // A rewritten image, reported under the *image's* key, re-decodes the atlas.
        write_png(&dir.join("a.png"), [200, 100, 50, 255]);
        cache
            .reload(ludion_assets::AssetRoot::content(&dir), "a.png")
            .unwrap();
        let after_image = cache
            .load_atlas(ludion_assets::AssetRoot::content(&dir), "a.atlas.yaml")
            .unwrap()
            .texture
            .hash;
        assert_ne!(after_image, first, "the packed image moved");

        // A rewritten document, reported under its own key, re-parses it.
        std::fs::write(dir.join("a.atlas.yaml"), atlas_doc("16.0")).unwrap();
        cache
            .reload(ludion_assets::AssetRoot::content(&dir), "a.atlas.yaml")
            .unwrap();
        assert_eq!(
            cache
                .load_atlas(ludion_assets::AssetRoot::content(&dir), "a.atlas.yaml")
                .unwrap()
                .doc
                .pixels_per_unit,
            16.0
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn resolve_key_of_uncached_path_is_none() {
        let dir = std::env::temp_dir();
        let cache = AssetCache::new();
        assert_eq!(
            cache.resolve_key(
                ludion_assets::AssetRoot::content(&dir),
                "ludion-not-cached-xyz.png"
            ),
            None
        );
    }

    /// A mesh document is cached like an image: a miss loads + decodes once, a hit reuses, and
    /// `resolve_key` + `reload` treat it as a first-class asset kind. Two vertices differ between the
    /// two writes so the content hash changes on reload.
    #[test]
    fn mesh_miss_loads_hit_reuses_reload_re_reads() {
        fn mesh_doc(z: f64) -> String {
            format!(
                "format: 1\nkind: mesh\npositions:\n  - 0.0\n  - 0.0\n  - {z}\n  - 1.0\n  - 0.0\n  \
                 - 0.0\n  - 0.0\n  - 1.0\n  - 0.0\nindices:\n  - 0\n  - 1\n  - 2\n"
            )
        }
        let dir = std::env::temp_dir().join(format!("ludion-mesh-cache-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("meshes")).unwrap();
        let rel = "meshes/m.mesh.yaml";
        std::fs::write(dir.join(rel), mesh_doc(0.0)).unwrap();

        let mut cache = AssetCache::new();
        let first = cache
            .load_mesh(ludion_assets::AssetRoot::content(&dir), rel)
            .unwrap()
            .hash;
        assert!(cache.contains_mesh(rel));
        // Normals were absent → computed faceted (+Z face) for all three vertices.
        assert_eq!(
            cache
                .load_mesh(ludion_assets::AssetRoot::content(&dir), rel)
                .unwrap()
                .normals,
            vec![0.0, 0.0, 1.0, 0.0, 0.0, 1.0, 0.0, 0.0, 1.0]
        );

        // A hit does not re-read a changed file.
        std::fs::write(dir.join(rel), mesh_doc(5.0)).unwrap();
        assert_eq!(
            cache
                .load_mesh(ludion_assets::AssetRoot::content(&dir), rel)
                .unwrap()
                .hash,
            first
        );

        // resolve_key finds the mesh from its relative name and from an absolute path.
        assert_eq!(
            cache
                .resolve_key(ludion_assets::AssetRoot::content(&dir), rel)
                .as_deref(),
            Some(rel)
        );
        let abs = dir.join(rel);
        assert_eq!(
            cache
                .resolve_key(
                    ludion_assets::AssetRoot::content(&dir),
                    &abs.to_string_lossy()
                )
                .as_deref(),
            Some(rel)
        );

        // Reload drops the entry and re-reads the changed bytes → a different hash.
        let reloaded = cache
            .reload(ludion_assets::AssetRoot::content(&dir), rel)
            .unwrap();
        assert_ne!(reloaded, first);
        assert_eq!(
            cache
                .load_mesh(ludion_assets::AssetRoot::content(&dir), rel)
                .unwrap()
                .hash,
            reloaded
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A voxel model is cached in its own map: a miss reads + greedily meshes once, a hit reuses, and
    /// `resolve_key` + `reload` treat it as a first-class asset kind (re-checking + re-meshing on the
    /// changed bytes, so the content hash moves). Mirrors the mesh-asset cache test.
    #[test]
    fn voxel_miss_loads_hit_reuses_reload_re_reads() {
        fn voxel_doc(g: &str) -> String {
            format!(
                "format: 1
kind: voxel
size:
  - 1
  - 1
  - 1
palette:
  {g}:
    color:
      - 1.0
      - 0.5
      - 0.2
      - 1.0
slices:
  -
    - \"{g}\"
"
            )
        }
        let dir = std::env::temp_dir().join(format!("ludion-voxel-cache-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("voxels")).unwrap();
        let rel = "voxels/v.voxel.yaml";
        // A single filled cell (glyph `A`) → a full box, 24 verts.
        std::fs::write(dir.join(rel), voxel_doc("A")).unwrap();

        let mut cache = AssetCache::new();
        let first = cache
            .load_voxel(ludion_assets::AssetRoot::content(&dir), rel)
            .unwrap()
            .hash;
        assert!(cache.contains_voxel(rel));
        assert_eq!(
            cache
                .load_voxel(ludion_assets::AssetRoot::content(&dir), rel)
                .unwrap()
                .positions
                .len(),
            24 * 3,
            "one filled cell meshes to a 24-vertex box"
        );

        // A hit does not re-read a changed file.
        std::fs::write(dir.join(rel), voxel_doc("B")).unwrap();
        assert_eq!(
            cache
                .load_voxel(ludion_assets::AssetRoot::content(&dir), rel)
                .unwrap()
                .hash,
            first
        );

        // resolve_key finds the voxel from its relative name and from an absolute path.
        assert_eq!(
            cache
                .resolve_key(ludion_assets::AssetRoot::content(&dir), rel)
                .as_deref(),
            Some(rel)
        );
        let abs = dir.join(rel);
        assert_eq!(
            cache
                .resolve_key(
                    ludion_assets::AssetRoot::content(&dir),
                    &abs.to_string_lossy()
                )
                .as_deref(),
            Some(rel)
        );

        // Reload drops the entry and re-reads the changed bytes → a different hash.
        let reloaded = cache
            .reload(ludion_assets::AssetRoot::content(&dir), rel)
            .unwrap();
        assert_ne!(reloaded, first);
        assert_eq!(
            cache
                .load_voxel(ludion_assets::AssetRoot::content(&dir), rel)
                .unwrap()
                .hash,
            reloaded
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A sculpt model is cached in its own map exactly like a voxel model: a miss reads + meshes once,
    /// a hit reuses, and `resolve_key` + `reload` re-check and re-mesh the changed bytes.
    #[test]
    fn sculpt_miss_loads_hit_reuses_reload_re_reads() {
        fn sculpt_doc(radius: &str) -> String {
            format!(
                "format: 1
kind: sculpt
resolution: 16
triangles: 200
palette:
  S:
    color:
      - 0.5
      - 0.5
      - 0.5
      - 1.0
bones:
  - name: root
parts:
  - name: ball
    shape: sphere
    radius: {radius}
    material: S
"
            )
        }
        let dir = std::env::temp_dir().join(format!("ludion-sculpt-cache-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("sculpts")).unwrap();
        let rel = "sculpts/ball.sculpt.yaml";
        std::fs::write(dir.join(rel), sculpt_doc("0.5")).unwrap();

        let mut cache = AssetCache::new();
        let first = cache
            .load_sculpt(ludion_assets::AssetRoot::content(&dir), rel)
            .unwrap()
            .hash;
        assert!(cache.contains_sculpt(rel));
        assert!(
            !cache
                .load_sculpt(ludion_assets::AssetRoot::content(&dir), rel)
                .unwrap()
                .positions
                .is_empty()
        );

        // A hit does not re-read a changed file.
        std::fs::write(dir.join(rel), sculpt_doc("0.4")).unwrap();
        assert_eq!(
            cache
                .load_sculpt(ludion_assets::AssetRoot::content(&dir), rel)
                .unwrap()
                .hash,
            first
        );

        assert_eq!(
            cache
                .resolve_key(ludion_assets::AssetRoot::content(&dir), rel)
                .as_deref(),
            Some(rel)
        );
        let abs = dir.join(rel);
        assert_eq!(
            cache
                .resolve_key(
                    ludion_assets::AssetRoot::content(&dir),
                    &abs.to_string_lossy()
                )
                .as_deref(),
            Some(rel)
        );

        // Reload drops the entry and re-meshes the changed bytes → a different hash.
        let reloaded = cache
            .reload(ludion_assets::AssetRoot::content(&dir), rel)
            .unwrap();
        assert_ne!(reloaded, first);
        assert_eq!(
            cache
                .load_sculpt(ludion_assets::AssetRoot::content(&dir), rel)
                .unwrap()
                .hash,
            reloaded
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// An assembly is cached like a voxel model, with the one difference that makes it its own kind:
    /// it is read from **several** files, so `resolve_key` answers for a *part* file too and
    /// `reload`ing that part re-meshes the whole assembly (its content hash folds every part's, so
    /// every per-part mesh key moves and the renderer re-uploads).
    #[test]
    fn assembly_miss_loads_hit_reuses_and_a_part_edit_reloads() {
        let dir =
            std::env::temp_dir().join(format!("ludion-assembly-cache-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("models/parts")).unwrap();
        let rel = "models/fixture.assembly.yaml";
        let body = "models/parts/body.part.yaml";
        std::fs::write(
            dir.join(rel),
            "format: 1
kind: assembly
name: fixture
size:
  - 4
  - 2
  - 1
palette:
  A:
    color:
      - 1.0
      - 0.0
      - 0.0
      - 1.0
parts:
  - name: body
    file: parts/body.part.yaml
    place:
      - 0
      - 0
      - 0
  - name: arm
    file: parts/arm.part.yaml
    parent: body
    place:
      - 2
      - 1
      - 0
",
        )
        .unwrap();
        let part_doc = |size: &str, row: &str| {
            format!("format: 1\nkind: part\nsize:\n{size}slices:\n  -\n    - \"{row}\"\n")
        };
        std::fs::write(dir.join(body), part_doc("  - 2\n  - 1\n  - 1\n", "AA")).unwrap();
        std::fs::write(
            dir.join("models/parts/arm.part.yaml"),
            part_doc("  - 1\n  - 1\n  - 1\n", "A"),
        )
        .unwrap();

        let mut cache = AssetCache::new();
        let first: Vec<u64> = cache
            .load_assembly(ludion_assets::AssetRoot::content(&dir), rel)
            .unwrap()
            .parts
            .iter()
            .map(|part| part.mesh.hash)
            .collect();
        assert!(cache.contains_assembly(rel));
        assert_eq!(first.len(), 2);
        assert_ne!(first[0], first[1], "each part gets its own GPU cache key");

        // A hit does not re-read a changed part file, and hands back the same `Arc` allocation.
        std::fs::write(dir.join(body), part_doc("  - 2\n  - 1\n  - 1\n", "A.")).unwrap();
        let held = cache
            .load_assembly(ludion_assets::AssetRoot::content(&dir), rel)
            .unwrap()
            .clone();
        assert_eq!(
            held.parts
                .iter()
                .map(|part| part.mesh.hash)
                .collect::<Vec<_>>(),
            first
        );

        // Both source files resolve — the assembly document and the part — by relative and by
        // absolute path. A part answers with its own key, which `reload` expands to its assemblies.
        assert_eq!(
            cache
                .resolve_key(ludion_assets::AssetRoot::content(&dir), rel)
                .as_deref(),
            Some(rel)
        );
        assert_eq!(
            cache
                .resolve_key(ludion_assets::AssetRoot::content(&dir), body)
                .as_deref(),
            Some(body)
        );
        let abs = dir.join(body);
        assert_eq!(
            cache
                .resolve_key(
                    ludion_assets::AssetRoot::content(&dir),
                    &abs.to_string_lossy()
                )
                .as_deref(),
            Some(body)
        );

        // Reloading the *part* re-reads and re-meshes the assembly that named it: every part's mesh
        // key moves, because the assembly's content hash folds every part file's.
        let reloaded = cache
            .reload(ludion_assets::AssetRoot::content(&dir), body)
            .unwrap();
        let after: Vec<u64> = cache
            .load_assembly(ludion_assets::AssetRoot::content(&dir), rel)
            .unwrap()
            .parts
            .iter()
            .map(|part| part.mesh.hash)
            .collect();
        assert!(
            after.iter().zip(&first).all(|(new, old)| new != old),
            "a part edit must invalidate every part's GPU cache key: {first:?} → {after:?}"
        );
        assert_eq!(
            reloaded,
            ludion_assets::load_bytes(ludion_assets::AssetRoot::content(&dir), body)
                .unwrap()
                .hash,
            "the reported hash is the changed file's own"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A **clip** file is a source file of the assembly that binds it, so `resolve_key` answers for
    /// it and `reload`ing it re-reads the assembly — the part-edit path exactly. The one difference
    /// is deliberate: a clip edit changes the pose, never the geometry, so every part's mesh key is
    /// left where it is and the renderer keeps its GPU buffers.
    #[test]
    fn a_clip_edit_reloads_the_assembly_without_moving_a_mesh_key() {
        let dir = std::env::temp_dir().join(format!("ludion-assembly-clip-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("models/parts")).unwrap();
        std::fs::create_dir_all(dir.join("models/anims")).unwrap();
        let rel = "models/fixture.assembly.yaml";
        let clip = "models/anims/walk.anim.yaml";
        std::fs::write(
            dir.join(rel),
            "format: 1
kind: assembly
name: fixture
size:
  - 2
  - 1
  - 1
palette:
  A:
    color:
      - 1.0
      - 0.0
      - 0.0
      - 1.0
parts:
  - name: body
    file: parts/body.part.yaml
    place:
      - 0
      - 0
      - 0
clips:
  walk: anims/walk.anim.yaml
",
        )
        .unwrap();
        std::fs::write(
            dir.join("models/parts/body.part.yaml"),
            "format: 1\nkind: part\nsize:\n  - 2\n  - 1\n  - 1\nslices:\n  -\n    - \"AA\"\n",
        )
        .unwrap();
        let clip_source = |degrees: f64| {
            format!(
                "format: 1
kind: animation
name: walk
duration: 1.0
loop: true
tracks:
  body:
    rotation:
      - t: 0.0
        v:
          - 0.0
          - 0.0
          - 0.0
      - t: 1.0
        v:
          - {degrees}
          - 0.0
          - 0.0
"
            )
        };
        std::fs::write(dir.join(clip), clip_source(90.0)).unwrap();

        let mut cache = AssetCache::new();
        let before = cache
            .load_assembly(ludion_assets::AssetRoot::content(&dir), rel)
            .unwrap()
            .clone();
        assert_eq!(
            before.clip("walk").unwrap().tracks[0].rotation[1].v[0],
            90.0
        );

        // The clip file is a source of the assembly, so it resolves as a reload key.
        assert_eq!(
            cache
                .resolve_key(ludion_assets::AssetRoot::content(&dir), clip)
                .as_deref(),
            Some(clip)
        );

        std::fs::write(dir.join(clip), clip_source(45.0)).unwrap();
        let reported = cache
            .reload(ludion_assets::AssetRoot::content(&dir), clip)
            .unwrap();
        let after = cache
            .load_assembly(ludion_assets::AssetRoot::content(&dir), rel)
            .unwrap()
            .clone();
        assert_eq!(
            after.clip("walk").unwrap().tracks[0].rotation[1].v[0],
            45.0,
            "the re-read assembly carries the edited clip"
        );
        assert_eq!(
            after.parts[0].mesh.hash, before.parts[0].mesh.hash,
            "a clip edit must not move a geometry cache key"
        );
        assert_eq!(
            reported,
            ludion_assets::load_bytes(ludion_assets::AssetRoot::content(&dir), clip)
                .unwrap()
                .hash,
            "the reported hash is the changed file's own"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A character is cached like an assembly, and its mesh document and each clip file are reload
    /// sources: a miss loads, a hit reuses the same `Arc`, and reloading either file re-reads the
    /// character.
    #[test]
    fn character_miss_loads_hit_reuses_and_a_mesh_or_clip_edit_reloads() {
        let dir =
            std::env::temp_dir().join(format!("ludion-character-cache-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("characters")).unwrap();
        std::fs::create_dir_all(dir.join("meshes")).unwrap();
        std::fs::create_dir_all(dir.join("clips")).unwrap();
        let gallery = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/gallery");
        let mesh = "meshes/skin-mannequin.mesh.yaml";
        let clip = "clips/idle.anim.yaml";
        std::fs::copy(gallery.join(mesh), dir.join(mesh)).unwrap();
        std::fs::copy(gallery.join(clip), dir.join(clip)).unwrap();
        let rel = "characters/hero.assembly.yaml";
        std::fs::write(
            dir.join(rel),
            "format: 1\nkind: assembly\nmesh: ../meshes/skin-mannequin.mesh.yaml\nclips:\n  idle:\n    clip: ../clips/idle.anim.yaml\n",
        )
        .unwrap();

        let mut cache = AssetCache::new();
        assert!(!cache.contains_character(rel));
        let first = cache
            .load_character(ludion_assets::AssetRoot::content(&dir), rel)
            .unwrap()
            .clone();
        assert!(cache.contains_character(rel));
        assert_eq!(first.mesh, mesh);
        assert_eq!(first.clip("idle").unwrap().clip.duration, 2.0);
        let hit = cache
            .load_character(ludion_assets::AssetRoot::content(&dir), rel)
            .unwrap()
            .clone();
        assert!(Arc::ptr_eq(&first, &hit), "a hit reuses the cached entry");

        // The mesh document is a source of the character.
        assert_eq!(
            cache
                .resolve_key(ludion_assets::AssetRoot::content(&dir), mesh)
                .as_deref(),
            Some(mesh)
        );
        cache
            .reload(ludion_assets::AssetRoot::content(&dir), mesh)
            .unwrap();
        let after_mesh = cache
            .load_character(ludion_assets::AssetRoot::content(&dir), rel)
            .unwrap()
            .clone();
        assert!(
            !Arc::ptr_eq(&first, &after_mesh),
            "a mesh reload re-reads the character"
        );

        // So is each clip file.
        let edited = std::fs::read_to_string(dir.join(clip))
            .unwrap()
            .replace("duration: 2.0", "duration: 3.0");
        std::fs::write(dir.join(clip), edited).unwrap();
        assert_eq!(
            cache
                .resolve_key(ludion_assets::AssetRoot::content(&dir), clip)
                .as_deref(),
            Some(clip)
        );
        cache
            .reload(ludion_assets::AssetRoot::content(&dir), clip)
            .unwrap();
        let after_clip = cache
            .load_character(ludion_assets::AssetRoot::content(&dir), rel)
            .unwrap()
            .clone();
        assert_eq!(after_clip.clip("idle").unwrap().clip.duration, 3.0);

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A prefab's clips are cached like a character's: a miss loads, a hit reuses the same `Arc`, and
    /// reloading a clip file re-reads the entry.
    #[test]
    fn prefab_clips_miss_loads_hit_reuses_and_a_clip_edit_reloads() {
        let dir =
            std::env::temp_dir().join(format!("ludion-prefab-clips-cache-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("clips")).unwrap();
        let fixture =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/import-node-clips/expected");
        let rel = "model.prefab.yaml";
        let clip = "clips/open.anim.yaml";
        for file in [rel, clip, "clips/snap.anim.yaml"] {
            std::fs::copy(fixture.join(file), dir.join(file)).unwrap();
        }

        let mut cache = AssetCache::new();
        assert!(!cache.contains_prefab_clips(rel));
        let first = cache
            .load_prefab_clips(ludion_assets::AssetRoot::content(&dir), rel)
            .unwrap()
            .clone();
        assert!(cache.contains_prefab_clips(rel));
        let duration = first.clips[0].clip.duration;
        let hit = cache
            .load_prefab_clips(ludion_assets::AssetRoot::content(&dir), rel)
            .unwrap()
            .clone();
        assert!(Arc::ptr_eq(&first, &hit), "a hit reuses the cached entry");

        let edited = std::fs::read_to_string(dir.join(clip))
            .unwrap()
            .replace(&format!("duration: {duration:?}"), "duration: 9.0");
        std::fs::write(dir.join(clip), edited).unwrap();
        assert_eq!(
            cache
                .resolve_key(ludion_assets::AssetRoot::content(&dir), clip)
                .as_deref(),
            Some(clip)
        );
        cache
            .reload(ludion_assets::AssetRoot::content(&dir), clip)
            .unwrap();
        let after = cache
            .load_prefab_clips(ludion_assets::AssetRoot::content(&dir), rel)
            .unwrap()
            .clone();
        assert!(
            !Arc::ptr_eq(&first, &after),
            "a clip edit re-reads the entry"
        );
        assert_eq!(after.clips[0].clip.duration, 9.0);

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// One dependent failing to re-read must not leave a later dependent of the same file stale: the
    /// mesh is valid but renames a joint the character's clip tracks, so the character fails `L1138`
    /// while the mesh entry itself must still be re-read.
    #[test]
    fn a_failed_reload_still_re_reads_every_other_dependent() {
        let dir = std::env::temp_dir().join(format!("ludion-reload-all-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("characters")).unwrap();
        std::fs::create_dir_all(dir.join("meshes")).unwrap();
        std::fs::create_dir_all(dir.join("clips")).unwrap();
        let gallery = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/gallery");
        let mesh = "meshes/skin-mannequin.mesh.yaml";
        let clip = "clips/idle.anim.yaml";
        std::fs::copy(gallery.join(mesh), dir.join(mesh)).unwrap();
        std::fs::copy(gallery.join(clip), dir.join(clip)).unwrap();
        let rel = "characters/hero.assembly.yaml";
        std::fs::write(
            dir.join(rel),
            "format: 1\nkind: assembly\nmesh: ../meshes/skin-mannequin.mesh.yaml\nclips:\n  idle:\n    clip: ../clips/idle.anim.yaml\n",
        )
        .unwrap();

        // The character first, so the mesh's dependents are [Character, Mesh].
        let mut cache = AssetCache::new();
        cache
            .load_character(ludion_assets::AssetRoot::content(&dir), rel)
            .unwrap();
        cache
            .load_mesh(ludion_assets::AssetRoot::content(&dir), mesh)
            .unwrap();

        let renamed = std::fs::read_to_string(dir.join(mesh))
            .unwrap()
            .replace("  - name: head", "  - name: skull");
        std::fs::write(dir.join(mesh), renamed).unwrap();
        let error = cache
            .reload(ludion_assets::AssetRoot::content(&dir), mesh)
            .unwrap_err();
        assert_eq!(error.code.as_str(), "L1138", "{error:?}");
        assert!(
            !cache.contains_character(rel),
            "the failed entry is dropped"
        );
        let joints = &cache
            .load_mesh(ludion_assets::AssetRoot::content(&dir), mesh)
            .unwrap()
            .skin_joints;
        assert!(
            joints.iter().any(|joint| joint.name == "skull"),
            "the mesh entry was re-read despite the character failing first"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A voxel model's occupancy grid is cached like its mesh: a miss reads + builds the
    /// [`VoxelStaticGrid`] once, a hit reuses the same `Arc` (a refcount bump, not a rebuild) —
    /// mirrors `voxel_miss_loads_hit_reuses_reload_re_reads` for the D15 accessor's cached entry
    /// point.
    #[test]
    fn voxel_grid_miss_loads_hit_reuses() {
        fn voxel_doc(g: &str) -> String {
            format!(
                "format: 1
kind: voxel
size:
  - 1
  - 1
  - 1
palette:
  {g}:
    color:
      - 1.0
      - 0.5
      - 0.2
      - 1.0
    emissive: 2.0
slices:
  -
    - \"{g}\"
"
            )
        }
        let dir =
            std::env::temp_dir().join(format!("ludion-voxel-grid-cache-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("voxels")).unwrap();
        let rel = "voxels/v.voxel.yaml";
        // A single filled cell (glyph `A`), emissive so the D15 payload's material lane is exercised.
        std::fs::write(dir.join(rel), voxel_doc("A")).unwrap();

        let mut cache = AssetCache::new();
        let first = cache
            .load_voxel_grid(ludion_assets::AssetRoot::content(&dir), rel)
            .unwrap()
            .clone();
        assert!(cache.contains_voxel_grid(rel));
        assert_eq!(first.extent, [1, 1, 1]);
        let cell = first.cells[0].expect("the one cell is filled");
        assert_eq!(cell.albedo, [1.0, 0.5, 0.2, 1.0]);
        assert_eq!(cell.emissive, 2.0);

        // A hit does not re-read a changed file, and hands back the same `Arc` allocation.
        std::fs::write(dir.join(rel), voxel_doc("B")).unwrap();
        let second = cache
            .load_voxel_grid(ludion_assets::AssetRoot::content(&dir), rel)
            .unwrap();
        assert!(Arc::ptr_eq(&first, second), "a hit reuses the cached Arc");

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A 1×1 RGBA PNG's bytes, `red` distinguishing one file's content hash from another's.
    fn png_bytes(red: u8) -> Vec<u8> {
        ludion_assets::encode_png(1, 1, &[red, 20, 30, 255]).expect("a 1x1 RGBA PNG encodes")
    }

    /// A one-triangle mesh document binding `path` as its base-colour map.
    fn textured_mesh_doc(path: &str) -> String {
        format!(
            "format: 1
kind: mesh
positions:
  - 0.0
  - 0.0
  - 0.0
  - 1.0
  - 0.0
  - 0.0
  - 0.0
  - 1.0
  - 0.0
uvs:
  - 0.0
  - 0.0
  - 1.0
  - 0.0
  - 0.0
  - 1.0
base_color_texture:
  path: {path}
indices:
  - 0
  - 1
  - 2
"
        )
    }

    /// A sculpt naming one `shape: voxel` part, a one-cell voxel document (`glyph` picks its content
    /// hash) beside it.
    fn write_voxel_sculpt(dir: &Path, glyph: &str) {
        std::fs::write(
            dir.join("sculpts/hull.voxel.yaml"),
            format!(
                "format: 1
kind: voxel
size:
  - 1
  - 1
  - 1
palette:
  {glyph}:
    color:
      - 1.0
      - 0.5
      - 0.2
      - 1.0
slices:
  -
    - \"{glyph}\"
"
            ),
        )
        .unwrap();
    }

    /// A sculpt is read from its voxel part's file too: `resolve_key` answers for the part, `reload`
    /// of it re-meshes the sculpt, and the sculpt's hash — which folds the part's — moves.
    #[test]
    fn a_sculpt_part_edit_reloads_the_sculpt() {
        let dir =
            std::env::temp_dir().join(format!("ludion-sculpt-part-cache-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("sculpts")).unwrap();
        let rel = "sculpts/hull.sculpt.yaml";
        let part = "sculpts/hull.voxel.yaml";
        std::fs::write(
            dir.join(rel),
            "format: 1
kind: sculpt
resolution: 16
triangles: 200
palette:
  S:
    color:
      - 0.5
      - 0.5
      - 0.5
      - 1.0
bones:
  - name: root
parts:
  - name: hull
    shape: voxel
    path: hull.voxel.yaml
    size: 0.5
    material: S
",
        )
        .unwrap();
        write_voxel_sculpt(&dir, "A");

        let mut cache = AssetCache::new();
        let first = cache
            .load_sculpt(ludion_assets::AssetRoot::content(&dir), rel)
            .unwrap()
            .hash;
        assert_ne!(
            first,
            ludion_assets::load_bytes(ludion_assets::AssetRoot::content(&dir), rel)
                .unwrap()
                .hash
        );

        // The part file resolves — by relative name and by absolute path — to the sculpt's edge.
        assert_eq!(
            cache
                .resolve_key(ludion_assets::AssetRoot::content(&dir), part)
                .as_deref(),
            Some(part)
        );
        let abs = dir.join(part);
        assert_eq!(
            cache
                .resolve_key(
                    ludion_assets::AssetRoot::content(&dir),
                    &abs.to_string_lossy()
                )
                .as_deref(),
            Some(part)
        );

        write_voxel_sculpt(&dir, "B");
        let reloaded = cache
            .reload(ludion_assets::AssetRoot::content(&dir), part)
            .unwrap();
        assert_eq!(
            reloaded,
            ludion_assets::load_bytes(ludion_assets::AssetRoot::content(&dir), part)
                .unwrap()
                .hash
        );
        assert!(cache.contains_sculpt(rel));
        assert_ne!(
            cache
                .load_sculpt(ludion_assets::AssetRoot::content(&dir), rel)
                .unwrap()
                .hash,
            first
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A mesh is read from its texture PNG too: editing the PNG and reloading it re-reads the mesh,
    /// whose hash folds the PNG's — so the renderer's mesh (and material bind group) re-keys.
    #[test]
    fn a_mesh_texture_edit_reloads_the_mesh() {
        let dir =
            std::env::temp_dir().join(format!("ludion-mesh-tex-cache-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("meshes")).unwrap();
        std::fs::create_dir_all(dir.join("textures")).unwrap();
        let rel = "meshes/m.mesh.yaml";
        std::fs::write(dir.join(rel), textured_mesh_doc("../textures/t.png")).unwrap();
        std::fs::write(dir.join("textures/t.png"), png_bytes(10)).unwrap();

        let mut cache = AssetCache::new();
        let first = cache
            .load_mesh(ludion_assets::AssetRoot::content(&dir), rel)
            .unwrap()
            .hash;
        assert_ne!(
            first,
            ludion_assets::load_bytes(ludion_assets::AssetRoot::content(&dir), rel)
                .unwrap()
                .hash
        );

        // A watcher reports the canonical absolute path of the PNG.
        let abs = std::fs::canonicalize(dir.join("textures/t.png")).unwrap();
        let key = cache
            .resolve_key(
                ludion_assets::AssetRoot::content(&dir),
                &abs.to_string_lossy(),
            )
            .expect("the mesh's texture is a dependency");

        std::fs::write(dir.join("textures/t.png"), png_bytes(200)).unwrap();
        let reloaded = cache
            .reload(ludion_assets::AssetRoot::content(&dir), &key)
            .unwrap();
        assert_eq!(
            reloaded,
            ludion_assets::load_bytes(ludion_assets::AssetRoot::content(&dir), "textures/t.png")
                .unwrap()
                .hash
        );
        assert_ne!(
            cache
                .load_mesh(ludion_assets::AssetRoot::content(&dir), rel)
                .unwrap()
                .hash,
            first
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// One PNG read as a loose texture and as a mesh map is two entries with one edge: a single
    /// reload of the file refreshes both.
    #[test]
    fn one_png_reload_refreshes_a_loose_texture_and_a_mesh_map() {
        let dir =
            std::env::temp_dir().join(format!("ludion-shared-png-cache-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("meshes")).unwrap();
        let rel = "meshes/m.mesh.yaml";
        let png_rel = "meshes/t.png";
        std::fs::write(dir.join(rel), textured_mesh_doc("t.png")).unwrap();
        std::fs::write(dir.join(png_rel), png_bytes(10)).unwrap();

        let mut cache = AssetCache::new();
        let mesh_first = cache
            .load_mesh(ludion_assets::AssetRoot::content(&dir), rel)
            .unwrap()
            .hash;
        let image_first = cache
            .load(ludion_assets::AssetRoot::content(&dir), png_rel)
            .unwrap()
            .hash;

        std::fs::write(dir.join(png_rel), png_bytes(200)).unwrap();
        let reloaded = cache
            .reload(ludion_assets::AssetRoot::content(&dir), png_rel)
            .unwrap();
        assert_ne!(reloaded, image_first);
        assert!(cache.contains(png_rel) && cache.contains_mesh(rel));
        assert_eq!(
            cache
                .load(ludion_assets::AssetRoot::content(&dir), png_rel)
                .unwrap()
                .hash,
            reloaded
        );
        assert_ne!(
            cache
                .load_mesh(ludion_assets::AssetRoot::content(&dir), rel)
                .unwrap()
                .hash,
            mesh_first
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A mesh map spelled `../textures/t.png` and a sprite loading `textures/t.png` are one edge:
    /// every spelling resolves to the one key, and one reload refreshes both entries.
    #[test]
    fn a_parent_spelled_mesh_map_and_a_loose_texture_share_one_key() {
        let dir =
            std::env::temp_dir().join(format!("ludion-parent-png-cache-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("meshes")).unwrap();
        std::fs::create_dir_all(dir.join("textures")).unwrap();
        let rel = "meshes/m.mesh.yaml";
        let png_rel = "textures/t.png";
        std::fs::write(dir.join(rel), textured_mesh_doc("../textures/t.png")).unwrap();
        std::fs::write(dir.join(png_rel), png_bytes(10)).unwrap();

        let mut cache = AssetCache::new();
        let mesh_first = cache
            .load_mesh(ludion_assets::AssetRoot::content(&dir), rel)
            .unwrap()
            .hash;
        let image_first = cache
            .load(ludion_assets::AssetRoot::content(&dir), png_rel)
            .unwrap()
            .hash;

        for spelling in ["textures/t.png", "meshes/../textures/t.png"] {
            assert_eq!(
                cache
                    .resolve_key(ludion_assets::AssetRoot::content(&dir), spelling)
                    .as_deref(),
                Some(png_rel),
                "{spelling}"
            );
        }

        std::fs::write(dir.join(png_rel), png_bytes(200)).unwrap();
        let key = cache
            .resolve_key(
                ludion_assets::AssetRoot::content(&dir),
                "meshes/../textures/t.png",
            )
            .unwrap();
        let reloaded = cache
            .reload(ludion_assets::AssetRoot::content(&dir), &key)
            .unwrap();
        assert_ne!(reloaded, image_first);
        assert_eq!(
            cache
                .load(ludion_assets::AssetRoot::content(&dir), png_rel)
                .unwrap()
                .hash,
            reloaded
        );
        assert_ne!(
            cache
                .load_mesh(ludion_assets::AssetRoot::content(&dir), rel)
                .unwrap()
                .hash,
            mesh_first
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Records every decode and every cooked read the cache reports, shared with the test that
    /// attached it.
    struct Decodes(
        std::sync::Arc<std::sync::Mutex<Vec<(LoadKind, String)>>>,
        std::sync::Arc<std::sync::Mutex<Vec<(LoadKind, String)>>>,
    );

    impl LoadObserver for Decodes {
        fn begin(&mut self, _: LoadPhase) {}
        fn end(&mut self, _: LoadPhase) {}
        fn decoded(&mut self, kind: LoadKind, key: &str, _: u64) {
            self.0.lock().unwrap().push((kind, key.to_string()));
        }
        fn reused(&mut self, _: LoadKind, _: &str) {}
        fn cooked(&mut self, kind: LoadKind, key: &str, _: u64) {
            self.1.lock().unwrap().push((kind, key.to_string()));
        }
    }

    /// The decoded image of a source map.
    fn source_image(image: &MapImage) -> &Arc<TextureImage> {
        match image {
            MapImage::Source(image) => image,
            MapImage::Cooked(_) => panic!("expected a source map"),
        }
    }

    /// Two meshes binding one PNG decode it once: the map is an entry of its own, not a per-mesh copy,
    /// and a reload of the PNG re-keys both meshes while leaving the map cached.
    #[test]
    fn a_map_shared_by_two_meshes_decodes_once() {
        let dir = std::env::temp_dir().join(format!("ludion-shared-map-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("meshes")).unwrap();
        std::fs::create_dir_all(dir.join("textures")).unwrap();
        for name in ["a", "b"] {
            std::fs::write(
                dir.join(format!("meshes/{name}.mesh.yaml")),
                textured_mesh_doc("../textures/t.png"),
            )
            .unwrap();
        }
        std::fs::write(dir.join("textures/t.png"), png_bytes(10)).unwrap();

        let log = Default::default();
        let mut cache = AssetCache::new();
        cache.set_observer(Some(Box::new(Decodes(
            Arc::clone(&log),
            Default::default(),
        ))));
        let a = cache
            .load_mesh(
                ludion_assets::AssetRoot::content(&dir),
                "meshes/a.mesh.yaml",
            )
            .unwrap()
            .clone();
        let b = cache
            .load_mesh(
                ludion_assets::AssetRoot::content(&dir),
                "meshes/b.mesh.yaml",
            )
            .unwrap()
            .clone();
        let textures = log
            .lock()
            .unwrap()
            .iter()
            .filter(|(kind, _)| *kind == LoadKind::Texture)
            .count();
        assert_eq!(textures, 1, "one file is one decode");
        assert!(cache.contains_mesh_map("textures/t.png"));
        let (a_maps, b_maps) = (a.textures.as_ref().unwrap(), b.textures.as_ref().unwrap());
        assert!(Arc::ptr_eq(
            source_image(&a_maps.base_color.as_ref().unwrap().image),
            source_image(&b_maps.base_color.as_ref().unwrap().image)
        ));

        std::fs::write(dir.join("textures/t.png"), png_bytes(200)).unwrap();
        cache
            .reload(ludion_assets::AssetRoot::content(&dir), "textures/t.png")
            .unwrap();
        assert!(cache.contains_mesh_map("textures/t.png"));
        for (rel, before) in [("meshes/a.mesh.yaml", &a), ("meshes/b.mesh.yaml", &b)] {
            assert_ne!(
                cache
                    .load_mesh(ludion_assets::AssetRoot::content(&dir), rel)
                    .unwrap()
                    .hash,
                before.hash,
                "{rel}"
            );
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    // --- cooked mesh maps ---

    const KIT_MESH: &str = "assets/kit/k/meshes/m.mesh.yaml";
    const KIT_BASE: &str = "assets/kit/k/textures/b.png";
    const KIT_NORMAL: &str = "assets/kit/k/textures/n.png";

    /// An 8×8 RGBA PNG whose pixels `seed` distinguishes, so its content hash is its own.
    fn png8(seed: u8) -> Vec<u8> {
        let rgba: Vec<u8> = (0..64u8)
            .flat_map(|i| [i.wrapping_mul(4).wrapping_add(seed), seed, 255 - i, 255])
            .collect();
        ludion_assets::encode_png(8, 8, &rgba).expect("an 8x8 RGBA PNG encodes")
    }

    /// A scratch project with one `[[import]]` table (`to = assets/kit`, an optional cap) and a mesh
    /// binding a base-colour and a normal map under it.
    fn cook_project(test: &str, cap: Option<u32>) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("ludion-cook-cache-{}-{test}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("assets/kit/k/meshes")).unwrap();
        std::fs::create_dir_all(dir.join("assets/kit/k/textures")).unwrap();
        let cap = cap.map_or(String::new(), |cap| format!("max_texture_size = {cap}\n"));
        std::fs::write(
            dir.join("ludion.toml"),
            format!(
                "name = \"fixture\"\ndefault_scene = \"scenes/main.yaml\"\n\n[[import]]\nfrom = \"art/*.gltf\"\nto = \"assets/kit\"\n{cap}"
            ),
        )
        .unwrap();
        let doc = textured_mesh_doc("../textures/b.png").replace(
            "indices:",
            "normal_texture:\n  path: ../textures/n.png\nindices:",
        );
        let tangents: String = "  - 1.0\n  - 0.0\n  - 0.0\n  - 1.0\n".repeat(3);
        let doc = doc.replace(
            "base_color_texture:",
            &format!("tangents:\n{tangents}base_color_texture:"),
        );
        std::fs::write(dir.join(KIT_MESH), doc).unwrap();
        std::fs::write(dir.join(KIT_BASE), png8(10)).unwrap();
        std::fs::write(dir.join(KIT_NORMAL), png8(90)).unwrap();
        dir
    }

    /// Cooks the PNG at `png` for `target`, `space` and `cap` into the project's store under its key.
    fn store_cooked(
        dir: &Path,
        png: &str,
        target: CookTarget,
        space: ColorSpace,
        cap: Option<u32>,
    ) -> PathBuf {
        let source = ludion_assets::load_png(ludion_assets::AssetRoot::content(dir), png).unwrap();
        let settings = ludion_assets::cooked::CookSettings {
            target,
            color_space: space,
            max_texture_size: cap,
            alpha_cutoff: None,
        };
        let cooked = ludion_cook::cook(&source, &settings).unwrap();
        let store = dir.join(".ludion/cooked").join(target.name());
        std::fs::create_dir_all(&store).unwrap();
        let file = store.join(ludion_assets::cooked::cooked_file_name(
            ludion_assets::cooked::cook_key(source.hash, &settings),
        ));
        std::fs::write(&file, ludion_cook::write_ktx2(&cooked)).unwrap();
        file
    }

    fn store_both(dir: &Path, cap: Option<u32>) -> (PathBuf, PathBuf) {
        (
            store_cooked(dir, KIT_BASE, CookTarget::Desktop, ColorSpace::Srgb, cap),
            store_cooked(
                dir,
                KIT_NORMAL,
                CookTarget::Desktop,
                ColorSpace::Linear,
                cap,
            ),
        )
    }

    fn lookup(dir: &Path, target: Option<CookTarget>) -> Option<Arc<CookLookup>> {
        Some(Arc::new(
            crate::cooked_maps::lookup_for(dir, target.as_slice())
                .expect("the fixture has a manifest"),
        ))
    }

    fn cooked_image(
        cache: &AssetCache,
        png: &str,
        space: ColorSpace,
    ) -> Arc<ludion_renderer::CookedMap> {
        match cache.mesh_map_image(png, space).expect("the map is held") {
            MapImage::Cooked(cooked) => cooked,
            MapImage::Source(_) => panic!("{png} fell back to its PNG"),
        }
    }

    /// A cooked map arrives as the cooked image in its own colour role — base colour sRGB, normal
    /// linear — and the observer is told it was read, not decoded.
    #[test]
    fn a_cooked_map_is_read_in_its_role_and_reported_cooked() {
        let dir = cook_project("hit", None);
        store_both(&dir, None);
        let (decoded, cooked) = (Arc::default(), Arc::default());
        let mut cache = AssetCache::new();
        cache.set_observer(Some(Box::new(Decodes(
            Arc::clone(&decoded),
            Arc::clone(&cooked),
        ))));
        cache.set_cook(lookup(&dir, Some(CookTarget::Desktop)));
        cache
            .load_mesh(ludion_assets::AssetRoot::content(&dir), KIT_MESH)
            .unwrap();

        assert!(cooked_image(&cache, KIT_BASE, ColorSpace::Srgb).image.srgb);
        let normal = cooked_image(&cache, KIT_NORMAL, ColorSpace::Linear);
        assert!(!normal.image.srgb, "a data map is cooked linear");
        assert!(cache.cook_misses().is_empty());
        assert_eq!(cooked.lock().unwrap().len(), 2);
        assert!(
            decoded
                .lock()
                .unwrap()
                .iter()
                .all(|(kind, _)| *kind != LoadKind::Texture),
            "a cooked map is not a PNG decode"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The base-colour mean is the source pixels' mean on both texture axes — a capped cook included —
    /// so a cooked and an uncooked load voxelize the same grid.
    #[test]
    fn the_base_color_mean_is_bit_identical_cooked_and_from_the_png() {
        for (test, cap) in [("mean-full", None), ("mean-capped", Some(4))] {
            let dir = cook_project(test, cap);
            store_both(&dir, cap);

            let mut cooked = AssetCache::new();
            cooked.set_cook(lookup(&dir, Some(CookTarget::Desktop)));
            let from_cook = cooked
                .load_mesh(ludion_assets::AssetRoot::content(&dir), KIT_MESH)
                .unwrap()
                .base_color_mean;
            cooked_image(&cooked, KIT_BASE, ColorSpace::Srgb);

            let mut plain = AssetCache::new();
            let from_png = plain
                .load_mesh(ludion_assets::AssetRoot::content(&dir), KIT_MESH)
                .unwrap()
                .base_color_mean;
            assert!(matches!(
                plain.mesh_map_image(KIT_BASE, ColorSpace::Srgb),
                Some(MapImage::Source(_))
            ));

            assert_ne!(from_png, [1.0; 4], "{test}");
            assert_eq!(
                from_cook.map(f32::to_bits),
                from_png.map(f32::to_bits),
                "{test}"
            );
            let _ = std::fs::remove_dir_all(&dir);
        }
    }

    /// A map with no store file loads from its PNG and is reported missing; the other still hits.
    #[test]
    fn a_missing_cooked_file_falls_back_to_the_png() {
        let dir = cook_project("missing", None);
        store_cooked(
            &dir,
            KIT_NORMAL,
            CookTarget::Desktop,
            ColorSpace::Linear,
            None,
        );
        let mut cache = AssetCache::new();
        cache.set_cook(lookup(&dir, Some(CookTarget::Desktop)));
        cache
            .load_mesh(ludion_assets::AssetRoot::content(&dir), KIT_MESH)
            .unwrap();

        assert!(matches!(
            cache.mesh_map_image(KIT_BASE, ColorSpace::Srgb),
            Some(MapImage::Source(_))
        ));
        assert_eq!(
            cache.cook_misses(),
            vec![(KIT_BASE.to_string(), CookMiss::Missing(None))]
        );
        cooked_image(&cache, KIT_NORMAL, ColorSpace::Linear);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Bytes that are not a KTX2 file under a key fall back, with no panic.
    #[test]
    fn a_corrupt_cooked_file_falls_back_to_the_png() {
        let dir = cook_project("corrupt", None);
        let (base, _) = store_both(&dir, None);
        std::fs::write(&base, b"not a ktx2 file at all").unwrap();
        let mut cache = AssetCache::new();
        cache.set_cook(lookup(&dir, Some(CookTarget::Desktop)));
        cache
            .load_mesh(ludion_assets::AssetRoot::content(&dir), KIT_MESH)
            .unwrap();

        assert!(matches!(
            cache.mesh_map_image(KIT_BASE, ColorSpace::Srgb),
            Some(MapImage::Source(_))
        ));
        assert!(matches!(
            cache.cook_misses().as_slice(),
            [(path, CookMiss::Corrupt(_))] if path == KIT_BASE
        ));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The table's `max_texture_size` is part of the key: a store cooked at the table's cap hits and
    /// one cooked uncapped does not.
    #[test]
    fn the_import_tables_cap_is_part_of_the_cooked_key() {
        let dir = cook_project("cap", Some(4));
        store_both(&dir, None);
        let mut uncapped = AssetCache::new();
        uncapped.set_cook(lookup(&dir, Some(CookTarget::Desktop)));
        uncapped
            .load_mesh(ludion_assets::AssetRoot::content(&dir), KIT_MESH)
            .unwrap();
        assert_eq!(
            uncapped.cook_misses().len(),
            2,
            "uncapped files do not answer a capped table"
        );

        store_both(&dir, Some(4));
        let mut capped = AssetCache::new();
        capped.set_cook(lookup(&dir, Some(CookTarget::Desktop)));
        capped
            .load_mesh(ludion_assets::AssetRoot::content(&dir), KIT_MESH)
            .unwrap();
        assert!(capped.cook_misses().is_empty());
        assert_eq!(
            cooked_image(&capped, KIT_BASE, ColorSpace::Srgb)
                .image
                .width,
            4
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A GPU with neither format has no target: every covered map loads from its PNG, as unsupported.
    #[test]
    fn no_target_loads_every_covered_map_from_its_png() {
        let dir = cook_project("no-target", None);
        store_both(&dir, None);
        let mut cache = AssetCache::new();
        cache.set_cook(lookup(&dir, None));
        cache
            .load_mesh(ludion_assets::AssetRoot::content(&dir), KIT_MESH)
            .unwrap();

        assert!(matches!(
            cache.mesh_map_image(KIT_BASE, ColorSpace::Srgb),
            Some(MapImage::Source(_))
        ));
        let misses = cache.cook_misses();
        assert_eq!(misses.len(), 2);
        assert!(
            misses
                .iter()
                .all(|(_, miss)| *miss == CookMiss::Unsupported)
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A map outside every import tree is never looked up: it loads from its PNG with no miss.
    #[test]
    fn a_map_outside_the_import_trees_loads_silently_from_its_png() {
        let dir = cook_project("outside", None);
        std::fs::create_dir_all(dir.join("meshes")).unwrap();
        std::fs::create_dir_all(dir.join("textures")).unwrap();
        std::fs::write(
            dir.join("meshes/o.mesh.yaml"),
            textured_mesh_doc("../textures/o.png"),
        )
        .unwrap();
        std::fs::write(dir.join("textures/o.png"), png8(50)).unwrap();
        let mut cache = AssetCache::new();
        cache.set_cook(lookup(&dir, Some(CookTarget::Desktop)));
        cache
            .load_mesh(
                ludion_assets::AssetRoot::content(&dir),
                "meshes/o.mesh.yaml",
            )
            .unwrap();

        assert!(matches!(
            cache.mesh_map_image("textures/o.png", ColorSpace::Srgb),
            Some(MapImage::Source(_))
        ));
        assert!(cache.cook_misses().is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A cooked mesh re-keys its GPU material group (its hash differs from the source mesh's), and
    /// the source mesh's hash is exactly the fold of the document and PNG hashes.
    #[test]
    fn a_cooked_mesh_hash_differs_from_the_source_fold() {
        let dir = cook_project("hash", None);
        store_both(&dir, None);
        let doc = std::fs::read(dir.join(KIT_MESH)).unwrap();
        let pngs = [KIT_BASE, KIT_NORMAL]
            .map(|png| ludion_assets::content_hash(&std::fs::read(dir.join(png)).unwrap()));
        let expected = fold_source_hashes(ludion_assets::content_hash(&doc), &pngs);

        let mut source = AssetCache::new();
        let source_hash = source
            .load_mesh(ludion_assets::AssetRoot::content(&dir), KIT_MESH)
            .unwrap()
            .hash;
        assert_eq!(source_hash, expected);

        let mut cooked = AssetCache::new();
        cooked.set_cook(lookup(&dir, Some(CookTarget::Desktop)));
        assert_ne!(
            cooked
                .load_mesh(ludion_assets::AssetRoot::content(&dir), KIT_MESH)
                .unwrap()
                .hash,
            source_hash
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A decal's map always reads its PNG: a covering cook lookup and a filled store change
    /// nothing for it, while a mesh naming the same file still gets the cooked image.
    #[test]
    fn a_decal_map_reads_its_png_whatever_the_cook_holds() {
        let dir = cook_project("decal-source", None);
        store_both(&dir, None);
        let root = ludion_assets::AssetRoot::content(&dir);
        let mut cache = AssetCache::new();
        cache.set_cook(lookup(&dir, Some(CookTarget::Desktop)));
        let (image, _) = cache.load_source_map(root, KIT_BASE).unwrap();
        assert!(matches!(image, MapImage::Source(_)));
        assert!(cache.cook_misses().is_empty());
        cache.load_mesh(root, KIT_MESH).unwrap();
        cooked_image(&cache, KIT_BASE, ColorSpace::Srgb);

        let empty = cook_project("decal-prefetch", None);
        let mut fresh = AssetCache::new();
        fresh.set_cook(lookup(&empty, Some(CookTarget::Desktop)));
        let refs = AssetRefs {
            maps: [KIT_BASE.to_string()].into(),
            ..AssetRefs::default()
        };
        fresh.prefetch(ludion_assets::AssetRoot::content(&empty), &refs);
        assert!(fresh.contains_mesh_map(KIT_BASE));
        assert!(fresh.cook_misses().is_empty());
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(&empty);
    }

    /// A cooked prefetch fills the same cache at any thread count, hits and misses alike.
    #[test]
    fn a_cooked_prefetch_is_thread_count_invariant() {
        let dir = cook_project("prefetch", None);
        store_cooked(
            &dir,
            KIT_NORMAL,
            CookTarget::Desktop,
            ColorSpace::Linear,
            None,
        );
        let refs = AssetRefs {
            meshes: [KIT_MESH.to_string()].into(),
            ..AssetRefs::default()
        };
        let prefetched = |threads: usize| {
            let mut cache = AssetCache::new();
            cache.set_cook(lookup(&dir, Some(CookTarget::Desktop)));
            ludion_core::par::with_threads(threads, || {
                cache.prefetch(ludion_assets::AssetRoot::content(&dir), &refs)
            });
            cache
        };
        let one = prefetched(1);
        assert!(one.contains_mesh(KIT_MESH));
        assert_eq!(one.cook_misses().len(), 1);
        assert_eq!(fingerprint(&one), fingerprint(&prefetched(2)));
        assert_eq!(fingerprint(&one), fingerprint(&prefetched(8)));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Changing the cook setting drops the maps and the meshes built from them; an equal setting
    /// keeps them.
    #[test]
    fn toggling_the_cook_setting_drops_and_rebuilds_the_maps() {
        let dir = cook_project("toggle", None);
        store_both(&dir, None);
        let mut cache = AssetCache::new();
        cache
            .load_mesh(ludion_assets::AssetRoot::content(&dir), KIT_MESH)
            .unwrap();
        assert!(matches!(
            cache.mesh_map_image(KIT_BASE, ColorSpace::Srgb),
            Some(MapImage::Source(_))
        ));

        cache.set_cook(lookup(&dir, Some(CookTarget::Desktop)));
        assert!(!cache.contains_mesh(KIT_MESH) && !cache.contains_mesh_map(KIT_BASE));
        cache
            .load_mesh(ludion_assets::AssetRoot::content(&dir), KIT_MESH)
            .unwrap();
        cooked_image(&cache, KIT_BASE, ColorSpace::Srgb);

        cache.set_cook(lookup(&dir, Some(CookTarget::Desktop)));
        assert!(
            cache.contains_mesh(KIT_MESH),
            "an equal setting keeps the cache"
        );

        cache.set_cook(None);
        assert!(!cache.contains_mesh(KIT_MESH));
        cache
            .load_mesh(ludion_assets::AssetRoot::content(&dir), KIT_MESH)
            .unwrap();
        assert!(matches!(
            cache.mesh_map_image(KIT_BASE, ColorSpace::Srgb),
            Some(MapImage::Source(_))
        ));
        assert!(cache.cook_misses().is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    const ASSEMBLY: &str = "format: 1
kind: assembly
name: fixture
size:
  - 2
  - 1
  - 1
palette:
  A:
    color:
      - 1.0
      - 0.0
      - 0.0
      - 1.0
parts:
  - name: body
    file: parts/body.part.yaml
    place:
      - 0
      - 0
      - 0
";

    const PART: &str =
        "format: 1\nkind: part\nsize:\n  - 2\n  - 1\n  - 1\nslices:\n  -\n    - \"AA\"\n";

    const ATLAS: &str = "format: 1\nkind: atlas\ngenerated_from: a.atlas-source.yaml\n\
        pixels_per_unit: 8.0\nmaps:\n  color: a.png\nregions:\n  a/one:\n    rect:\n      - 0\n      \
        - 0\n      - 1\n      - 1\n    pivot:\n      - 0.5\n      - 0.5\n";

    const VOXEL: &str = "format: 1\nkind: voxel\nsize:\n  - 1\n  - 1\n  - 1\npalette:\n  A:\n    \
        color:\n      - 1.0\n      - 0.5\n      - 0.2\n      - 1.0\nslices:\n  -\n    - \"A\"\n";

    /// A project with three textured meshes (two sharing one map), a loose sprite PNG, an atlas, a
    /// voxel and an assembly, plus the refs naming all of them.
    fn prefetch_fixture(test: &str) -> (std::path::PathBuf, AssetRefs) {
        let dir =
            std::env::temp_dir().join(format!("ludion-prefetch-{test}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        for sub in ["meshes", "textures", "voxels", "models/parts"] {
            std::fs::create_dir_all(dir.join(sub)).unwrap();
        }
        let write = |rel: &str, bytes: &[u8]| std::fs::write(dir.join(rel), bytes).unwrap();
        let mesh = |path: &str| textured_mesh_doc(path).into_bytes();
        write("meshes/a.mesh.yaml", &mesh("../textures/t.png"));
        write("meshes/b.mesh.yaml", &mesh("../textures/t.png"));
        write("meshes/c.mesh.yaml", &mesh("../textures/u.png"));
        write("textures/t.png", &png_bytes(10));
        write("textures/u.png", &png_bytes(20));
        write("textures/s.png", &png_bytes(30));
        write("a.png", &png_bytes(40));
        write("a.atlas.yaml", ATLAS.as_bytes());
        write("voxels/v.voxel.yaml", VOXEL.as_bytes());
        write("models/fixture.assembly.yaml", ASSEMBLY.as_bytes());
        write("models/parts/body.part.yaml", PART.as_bytes());
        let set = |keys: &[&str]| keys.iter().map(|key| key.to_string()).collect();
        let refs = AssetRefs {
            textures: set(&["textures/s.png"]),
            atlases: set(&["a.atlas.yaml"]),
            meshes: set(&[
                "meshes/a.mesh.yaml",
                "meshes/b.mesh.yaml",
                "meshes/c.mesh.yaml",
            ]),
            voxels: set(&["voxels/v.voxel.yaml"]),
            assemblies: set(&["models/fixture.assembly.yaml"]),
            ..AssetRefs::default()
        };
        (dir, refs)
    }

    /// Everything a prefetch decides about a cache: every entry's key and content hash, and the
    /// dependency edges in their recorded order.
    fn fingerprint(cache: &AssetCache) -> String {
        let images = |map: &BTreeMap<String, Arc<TextureImage>>| {
            map.iter()
                .map(|(key, image)| (key.clone(), image.hash))
                .collect::<Vec<_>>()
        };
        let meshes = |map: &BTreeMap<String, Arc<MeshData>>| {
            map.iter()
                .map(|(key, mesh)| (key.clone(), mesh.hash))
                .collect::<Vec<_>>()
        };
        let mesh_maps: Vec<_> = cache
            .mesh_maps
            .iter()
            .map(|((key, role), image)| (key.clone(), *role, image.identity()))
            .collect();
        let atlases: Vec<_> = cache
            .atlases
            .iter()
            .map(|(key, atlas)| (key.clone(), atlas.texture.hash))
            .collect();
        let assemblies: Vec<_> = cache
            .assemblies
            .iter()
            .map(|(key, data)| {
                let parts: Vec<u64> = data.parts.iter().map(|part| part.mesh.hash).collect();
                (key.clone(), parts)
            })
            .collect();
        format!(
            "{:?}\n{mesh_maps:?}\n{:?}\n{:?}\n{atlases:?}\n{assemblies:?}\n{:?}\n{:?}\n{:?}",
            images(&cache.images),
            meshes(&cache.meshes),
            meshes(&cache.voxels),
            cache.dependents,
            cache.source_hashes,
            cache.cook_misses(),
        )
    }

    /// The prefetch inserts in sorted key order whatever the worker count, so the cache — entries,
    /// hashes and dependency edges — is the same at 1, 2 and 8 threads, and every entry equals the
    /// lazy loader's.
    #[test]
    fn prefetch_is_thread_count_invariant() {
        let (dir, refs) = prefetch_fixture("invariant");
        let prefetched = |threads: usize| {
            let mut cache = AssetCache::new();
            ludion_core::par::with_threads(threads, || {
                cache.prefetch(ludion_assets::AssetRoot::content(&dir), &refs)
            });
            cache
        };
        let one = prefetched(1);
        assert_eq!(fingerprint(&one), fingerprint(&prefetched(2)));
        assert_eq!(fingerprint(&one), fingerprint(&prefetched(8)));

        let mut lazy = AssetCache::new();
        for rel in &refs.meshes {
            assert_eq!(
                one.meshes[rel].hash,
                lazy.load_mesh(ludion_assets::AssetRoot::content(&dir), rel)
                    .unwrap()
                    .hash,
                "{rel}"
            );
        }
        for rel in &refs.voxels {
            assert_eq!(
                one.voxels[rel].hash,
                lazy.load_voxel(ludion_assets::AssetRoot::content(&dir), rel)
                    .unwrap()
                    .hash
            );
        }
        for rel in &refs.atlases {
            assert_eq!(
                one.atlases[rel].texture.hash,
                lazy.load_atlas(ludion_assets::AssetRoot::content(&dir), rel)
                    .unwrap()
                    .texture
                    .hash
            );
        }
        for rel in &refs.assemblies {
            assert_eq!(
                one.assemblies[rel].parts[0].mesh.hash,
                lazy.load_assembly(ludion_assets::AssetRoot::content(&dir), rel)
                    .unwrap()
                    .parts[0]
                    .mesh
                    .hash
            );
        }
        assert!(one.contains("textures/s.png"));
        assert!(one.contains_mesh_map("textures/u.png"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Evicting down to one mesh drops the other meshes, the maps only they bound, and every edge and
    /// recorded hash that pointed at them.
    #[test]
    fn retain_referenced_drops_orphan_maps_and_prunes_edges() {
        let (dir, refs) = prefetch_fixture("retain");
        let mut cache = AssetCache::new();
        cache.prefetch(ludion_assets::AssetRoot::content(&dir), &refs);
        assert!(cache.contains_mesh_map("textures/u.png"));

        let keep = AssetRefs {
            meshes: ["meshes/a.mesh.yaml".to_string()].into_iter().collect(),
            ..AssetRefs::default()
        };
        cache.retain_referenced(ludion_assets::AssetRoot::content(&dir), &keep);

        assert!(cache.contains_mesh("meshes/a.mesh.yaml"));
        assert!(cache.contains_mesh_map("textures/t.png"));
        assert_eq!(
            cache.resolve_key(ludion_assets::AssetRoot::content(&dir), "textures/u.png"),
            None
        );
        assert_eq!(
            cache.resolve_key(
                ludion_assets::AssetRoot::content(&dir),
                "meshes/c.mesh.yaml"
            ),
            None
        );
        assert!(!cache.contains("textures/s.png"));
        assert!(!cache.contains_voxel("voxels/v.voxel.yaml"));
        assert!(!cache.contains_atlas("a.atlas.yaml"));
        let edges: Vec<_> = cache.dependents.keys().collect();
        let hashes: Vec<_> = cache.source_hashes.keys().collect();
        assert_eq!(edges, hashes);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A decal's map is an entry of its own: held with no mesh, evicted unless a decal still names it,
    /// re-read by a reload of its PNG, and decoded by the prefetch.
    #[test]
    fn a_decal_only_map_is_held_retained_prefetched_and_reloaded() {
        let dir = std::env::temp_dir().join(format!("ludion-decal-map-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("decals")).unwrap();
        let png_rel = "decals/d.png";
        std::fs::write(dir.join(png_rel), png_bytes(10)).unwrap();
        let named = AssetRefs {
            maps: [png_rel.to_string()].into_iter().collect(),
            ..AssetRefs::default()
        };

        // (a) Loaded with no mesh involved.
        let mut cache = AssetCache::new();
        let (_, first) = cache
            .load_map(
                ludion_assets::AssetRoot::content(&dir),
                png_rel,
                ColorSpace::Srgb,
                None,
            )
            .unwrap();
        assert!(cache.contains_mesh_map(png_rel));
        assert!(cache.mesh_map_image(png_rel, ColorSpace::Srgb).is_some());

        // (b) Kept while a decal names it, evicted once none does.
        cache.retain_referenced(ludion_assets::AssetRoot::content(&dir), &named);
        assert!(cache.contains_mesh_map(png_rel));
        cache.retain_referenced(
            ludion_assets::AssetRoot::content(&dir),
            &AssetRefs::default(),
        );
        assert!(!cache.contains_mesh_map(png_rel));

        // (c) A reload of the PNG re-reads it under the same key.
        cache
            .load_map(
                ludion_assets::AssetRoot::content(&dir),
                png_rel,
                ColorSpace::Srgb,
                None,
            )
            .unwrap();
        std::fs::write(dir.join(png_rel), png_bytes(200)).unwrap();
        cache
            .reload(ludion_assets::AssetRoot::content(&dir), png_rel)
            .unwrap();
        let (_, reloaded) = cache
            .load_map(
                ludion_assets::AssetRoot::content(&dir),
                png_rel,
                ColorSpace::Srgb,
                None,
            )
            .unwrap();
        assert_ne!(reloaded, first);

        // (d) The prefetch decodes it, and a second one leaves the held entry alone.
        let mut fresh = AssetCache::new();
        fresh.prefetch(ludion_assets::AssetRoot::content(&dir), &named);
        assert!(fresh.contains_mesh_map(png_rel));
        let held = fresh.mesh_map_image(png_rel, ColorSpace::Srgb).unwrap();
        fresh.prefetch(ludion_assets::AssetRoot::content(&dir), &named);
        assert_eq!(
            fresh
                .mesh_map_image(png_rel, ColorSpace::Srgb)
                .unwrap()
                .identity(),
            held.identity()
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A fresh scratch project root for the revalidation tests.
    fn revalidation_dir(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("ludion-revalidate-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn write_file(dir: &Path, rel: &str, text: &str) {
        let path = dir.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    const REVALIDATE_ATLAS: &str = "format: 1\nkind: atlas\ngenerated_from: a.atlas-source.yaml\npixels_per_unit: 8.0\nmaps:\n  color: a.png\nregions:\n  a/one:\n    rect:\n      - 0\n      - 0\n      - 1\n      - 1\n    pivot:\n      - 0.5\n      - 0.5\n";

    fn write_atlas(dir: &Path, rgba: [u8; 4]) {
        write_file(dir, "a.atlas.yaml", REVALIDATE_ATLAS);
        write_png(&dir.join("a.png"), rgba);
    }

    /// The one-cell voxel document at `voxels/v.voxel.yaml`; `glyph` picks its content hash.
    fn write_voxel(dir: &Path, glyph: &str) {
        write_file(
            dir,
            "voxels/v.voxel.yaml",
            &format!(
                "format: 1\nkind: voxel\nsize:\n  - 1\n  - 1\n  - 1\npalette:\n  {glyph}:\n    color:\n      - 1.0\n      - 0.5\n      - 0.2\n      - 1.0\nslices:\n  -\n    - \"{glyph}\"\n"
            ),
        );
    }

    /// The sculpt at `sculpts/hull.sculpt.yaml` and its voxel part beside it.
    fn write_sculpt(dir: &Path, glyph: &str) {
        write_file(
            dir,
            "sculpts/hull.sculpt.yaml",
            "format: 1\nkind: sculpt\nresolution: 16\ntriangles: 200\npalette:\n  S:\n    color:\n      - 0.5\n      - 0.5\n      - 0.5\n      - 1.0\nbones:\n  - name: root\nparts:\n  - name: hull\n    shape: voxel\n    path: hull.voxel.yaml\n    size: 0.5\n    material: S\n",
        );
        write_voxel_sculpt(dir, glyph);
    }

    /// The assembly `models/<name>.assembly.yaml` with its one part `models/parts/<name>.part.yaml`
    /// (`row` picks the part's content), binding the clip `models/anims/walk.anim.yaml` when `clip`.
    fn write_assembly(dir: &Path, name: &str, row: &str, clip: bool) {
        let clips = if clip {
            "clips:\n  walk: anims/walk.anim.yaml\n"
        } else {
            ""
        };
        write_file(
            dir,
            &format!("models/{name}.assembly.yaml"),
            &format!(
                "format: 1\nkind: assembly\nname: {name}\nsize:\n  - 2\n  - 1\n  - 1\npalette:\n  A:\n    color:\n      - 1.0\n      - 0.0\n      - 0.0\n      - 1.0\nparts:\n  - name: body\n    file: parts/{name}.part.yaml\n    place:\n      - 0\n      - 0\n      - 0\n{clips}"
            ),
        );
        write_file(
            dir,
            &format!("models/parts/{name}.part.yaml"),
            &format!(
                "format: 1\nkind: part\nsize:\n  - 2\n  - 1\n  - 1\nslices:\n  -\n    - \"{row}\"\n"
            ),
        );
    }

    fn write_walk_clip(dir: &Path, degrees: f64) {
        write_file(
            dir,
            "models/anims/walk.anim.yaml",
            &format!(
                "format: 1\nkind: animation\nname: walk\nduration: 1.0\nloop: true\ntracks:\n  body:\n    rotation:\n      - t: 0.0\n        v:\n          - 0.0\n          - 0.0\n          - 0.0\n      - t: 1.0\n        v:\n          - {degrees}\n          - 0.0\n          - 0.0\n"
            ),
        );
    }

    const CHARACTER: &str = "characters/hero.assembly.yaml";
    const CHARACTER_CLIP: &str = "clips/idle.anim.yaml";

    fn write_character(dir: &Path) {
        let gallery = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/gallery");
        for rel in ["meshes/skin-mannequin.mesh.yaml", CHARACTER_CLIP] {
            write_file(
                dir,
                rel,
                &std::fs::read_to_string(gallery.join(rel)).unwrap(),
            );
        }
        write_file(
            dir,
            CHARACTER,
            "format: 1\nkind: assembly\nmesh: ../meshes/skin-mannequin.mesh.yaml\nclips:\n  idle:\n    clip: ../clips/idle.anim.yaml\n",
        );
    }

    fn named(key: &str) -> BTreeSet<String> {
        BTreeSet::from([key.to_string()])
    }

    #[test]
    fn revalidation_drops_an_assembly_whose_part_changed() {
        let dir = revalidation_dir("assembly-part");
        write_assembly(&dir, "fixture", "AA", false);
        write_assembly(&dir, "other", "AA", false);
        let (rel, other) = ("models/fixture.assembly.yaml", "models/other.assembly.yaml");
        let refs = AssetRefs {
            assemblies: BTreeSet::from([rel.to_string(), other.to_string()]),
            ..AssetRefs::default()
        };

        let mut cache = AssetCache::new();
        let first = cache
            .load_assembly(ludion_assets::AssetRoot::content(&dir), rel)
            .unwrap()
            .parts[0]
            .mesh
            .hash;
        cache
            .load_assembly(ludion_assets::AssetRoot::content(&dir), other)
            .unwrap();
        write_assembly(&dir, "fixture", "A.", false);
        cache.retain_referenced(ludion_assets::AssetRoot::content(&dir), &refs);

        assert!(!cache.contains_assembly(rel), "its part changed on disk");
        assert!(
            cache.contains_assembly(other),
            "an untouched assembly stays"
        );
        assert_ne!(
            cache
                .load_assembly(ludion_assets::AssetRoot::content(&dir), rel)
                .unwrap()
                .parts[0]
                .mesh
                .hash,
            first
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn revalidation_drops_an_assembly_whose_clip_changed() {
        let dir = revalidation_dir("assembly-clip");
        write_assembly(&dir, "fixture", "AA", true);
        write_walk_clip(&dir, 90.0);
        let rel = "models/fixture.assembly.yaml";
        let refs = AssetRefs {
            assemblies: named(rel),
            ..AssetRefs::default()
        };

        let mut cache = AssetCache::new();
        let first = cache
            .load_assembly(ludion_assets::AssetRoot::content(&dir), rel)
            .unwrap()
            .clone();
        assert_eq!(first.clip("walk").unwrap().tracks[0].rotation[1].v[0], 90.0);
        write_walk_clip(&dir, 45.0);
        cache.retain_referenced(ludion_assets::AssetRoot::content(&dir), &refs);

        assert!(!cache.contains_assembly(rel), "its clip changed on disk");
        let after = cache
            .load_assembly(ludion_assets::AssetRoot::content(&dir), rel)
            .unwrap();
        assert_eq!(after.clip("walk").unwrap().tracks[0].rotation[1].v[0], 45.0);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn revalidation_drops_an_atlas_whose_png_changed() {
        let dir = revalidation_dir("atlas");
        write_atlas(&dir, [10, 20, 30, 255]);
        let rel = "a.atlas.yaml";
        let refs = AssetRefs {
            atlases: named(rel),
            ..AssetRefs::default()
        };

        let mut cache = AssetCache::new();
        let first = cache
            .load_atlas(ludion_assets::AssetRoot::content(&dir), rel)
            .unwrap()
            .texture
            .hash;
        assert!(cache.contains_atlas(rel));
        write_png(&dir.join("a.png"), [200, 100, 50, 255]);
        cache.retain_referenced(ludion_assets::AssetRoot::content(&dir), &refs);

        assert!(!cache.contains_atlas(rel), "its colour PNG changed on disk");
        assert_ne!(
            cache
                .load_atlas(ludion_assets::AssetRoot::content(&dir), rel)
                .unwrap()
                .texture
                .hash,
            first
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn revalidation_drops_a_character_whose_clip_changed() {
        let dir = revalidation_dir("character");
        write_character(&dir);
        let refs = AssetRefs {
            assemblies: named(CHARACTER),
            ..AssetRefs::default()
        };

        let mut cache = AssetCache::new();
        let first = cache
            .load_character(ludion_assets::AssetRoot::content(&dir), CHARACTER)
            .unwrap();
        assert_eq!(first.clip("idle").unwrap().clip.duration, 2.0);
        let edited = std::fs::read_to_string(dir.join(CHARACTER_CLIP))
            .unwrap()
            .replace("duration: 2.0", "duration: 3.0");
        write_file(&dir, CHARACTER_CLIP, &edited);
        cache.retain_referenced(ludion_assets::AssetRoot::content(&dir), &refs);

        assert!(
            !cache.contains_character(CHARACTER),
            "its clip changed on disk"
        );
        let after = cache
            .load_character(ludion_assets::AssetRoot::content(&dir), CHARACTER)
            .unwrap();
        assert_eq!(after.clip("idle").unwrap().clip.duration, 3.0);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A voxel document with a `lods` chain loads through the cache with its levels attached, and
    /// an edit to the chain (only a threshold) reloads it.
    #[test]
    fn a_voxel_chain_loads_with_its_levels_and_reloads_on_edit() {
        let dir = revalidation_dir("voxel-chain");
        let rel = "voxels/v.voxel.yaml";
        let chain = |tail: &str| {
            format!(
                "format: 1\nkind: voxel\nsize:\n  - 4\n  - 4\n  - 4\nlods:\n  - screen_size: 0.25\n  - screen_size: {tail}\npalette:\n  A:\n    color:\n      - 1.0\n      - 0.5\n      - 0.2\n      - 1.0\nslices:\n{}",
                "  -\n    - \"AAAA\"\n    - \"AAAA\"\n    - \"AAAA\"\n    - \"AAAA\"\n".repeat(4)
            )
        };
        write_file(&dir, rel, &chain("0.08"));
        let mut cache = AssetCache::new();
        let mesh = cache
            .load_voxel(ludion_assets::AssetRoot::content(&dir), rel)
            .unwrap()
            .clone();
        assert_eq!(mesh.lods.len(), 2);
        assert_eq!(mesh.lods[0].screen_size, 0.25);
        assert_eq!(mesh.lods[1].screen_size, 0.08);

        write_file(&dir, rel, &chain("0.05"));
        let refs = AssetRefs {
            voxels: named(rel),
            ..AssetRefs::default()
        };
        cache.retain_referenced(ludion_assets::AssetRoot::content(&dir), &refs);
        let edited = cache
            .load_voxel(ludion_assets::AssetRoot::content(&dir), rel)
            .unwrap();
        assert_eq!(edited.lods[1].screen_size, 0.05);
        assert_ne!(edited.hash, mesh.hash);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn revalidation_drops_an_edited_voxel_and_its_grid() {
        let dir = revalidation_dir("voxel");
        write_voxel(&dir, "A");
        let rel = "voxels/v.voxel.yaml";
        let refs = AssetRefs {
            voxels: named(rel),
            ..AssetRefs::default()
        };

        let mut cache = AssetCache::new();
        let mesh = cache
            .load_voxel(ludion_assets::AssetRoot::content(&dir), rel)
            .unwrap()
            .hash;
        let grid = cache
            .load_voxel_grid(ludion_assets::AssetRoot::content(&dir), rel)
            .unwrap()
            .hash;
        assert!(cache.contains_voxel(rel) && cache.contains_voxel_grid(rel));
        write_voxel(&dir, "B");
        cache.retain_referenced(ludion_assets::AssetRoot::content(&dir), &refs);

        assert!(!cache.contains_voxel(rel));
        assert!(!cache.contains_voxel_grid(rel));
        assert_ne!(
            cache
                .load_voxel(ludion_assets::AssetRoot::content(&dir), rel)
                .unwrap()
                .hash,
            mesh
        );
        assert_ne!(
            cache
                .load_voxel_grid(ludion_assets::AssetRoot::content(&dir), rel)
                .unwrap()
                .hash,
            grid
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn revalidation_drops_a_sculpt_whose_part_changed() {
        let dir = revalidation_dir("sculpt");
        write_sculpt(&dir, "A");
        let rel = "sculpts/hull.sculpt.yaml";
        let refs = AssetRefs {
            sculpts: named(rel),
            ..AssetRefs::default()
        };

        let mut cache = AssetCache::new();
        let first = cache
            .load_sculpt(ludion_assets::AssetRoot::content(&dir), rel)
            .unwrap()
            .hash;
        write_voxel_sculpt(&dir, "B");
        cache.retain_referenced(ludion_assets::AssetRoot::content(&dir), &refs);

        assert!(
            !cache.contains_sculpt(rel),
            "its voxel part changed on disk"
        );
        assert_ne!(
            cache
                .load_sculpt(ludion_assets::AssetRoot::content(&dir), rel)
                .unwrap()
                .hash,
            first
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// With nothing edited, revalidation keeps every kind, and the recorded file hashes are exactly the
    /// files the dependency edges name.
    #[test]
    fn an_untouched_entry_survives_revalidation() {
        let dir = revalidation_dir("untouched");
        write_atlas(&dir, [10, 20, 30, 255]);
        write_voxel(&dir, "A");
        write_sculpt(&dir, "A");
        write_assembly(&dir, "fixture", "AA", true);
        write_walk_clip(&dir, 90.0);
        write_character(&dir);
        let refs = AssetRefs {
            atlases: named("a.atlas.yaml"),
            voxels: named("voxels/v.voxel.yaml"),
            sculpts: named("sculpts/hull.sculpt.yaml"),
            assemblies: BTreeSet::from([
                "models/fixture.assembly.yaml".to_string(),
                CHARACTER.to_string(),
            ]),
            ..AssetRefs::default()
        };

        let mut cache = AssetCache::new();
        cache
            .load_atlas(ludion_assets::AssetRoot::content(&dir), "a.atlas.yaml")
            .unwrap();
        cache
            .load_voxel(
                ludion_assets::AssetRoot::content(&dir),
                "voxels/v.voxel.yaml",
            )
            .unwrap();
        cache
            .load_voxel_grid(
                ludion_assets::AssetRoot::content(&dir),
                "voxels/v.voxel.yaml",
            )
            .unwrap();
        cache
            .load_sculpt(
                ludion_assets::AssetRoot::content(&dir),
                "sculpts/hull.sculpt.yaml",
            )
            .unwrap();
        cache
            .load_assembly(
                ludion_assets::AssetRoot::content(&dir),
                "models/fixture.assembly.yaml",
            )
            .unwrap();
        cache
            .load_character(ludion_assets::AssetRoot::content(&dir), CHARACTER)
            .unwrap();
        cache.retain_referenced(ludion_assets::AssetRoot::content(&dir), &refs);

        assert!(cache.contains_atlas("a.atlas.yaml"));
        assert!(cache.contains_voxel("voxels/v.voxel.yaml"));
        assert!(cache.contains_voxel_grid("voxels/v.voxel.yaml"));
        assert!(cache.contains_sculpt("sculpts/hull.sculpt.yaml"));
        assert!(cache.contains_assembly("models/fixture.assembly.yaml"));
        assert!(cache.contains_character(CHARACTER));
        let edges: Vec<_> = cache.dependents.keys().collect();
        let hashes: Vec<_> = cache.source_hashes.keys().collect();
        assert_eq!(edges, hashes);
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn texture_err(cache: &mut AssetCache, dir: &Path) -> String {
        let error = cache
            .load(ludion_assets::AssetRoot::content(dir), "t.png")
            .map(|_| ())
            .unwrap_err();
        format!("{error:?}")
    }

    #[test]
    fn a_cook_fallback_retries_once_the_store_file_appears() {
        let dir = cook_project("retry-appears", None);
        let mut cache = AssetCache::new();
        cache.set_cook(lookup(&dir, Some(CookTarget::Desktop)));
        let root = ludion_assets::AssetRoot::content(&dir);
        let fallback_hash = cache.load_mesh(root, KIT_MESH).unwrap().hash;
        assert_eq!(cache.cook_misses().len(), 2);
        assert!(!cache.retry_cook_misses(), "no new file, no move");

        store_both(&dir, None);
        assert!(cache.retry_cook_misses());
        assert!(!cache.contains_mesh_map(KIT_BASE));
        assert!(!cache.contains_mesh(KIT_MESH));
        let cooked_hash = cache.load_mesh(root, KIT_MESH).unwrap().hash;
        assert_ne!(
            cooked_hash, fallback_hash,
            "a mid-session cook re-keys the mesh"
        );
        cooked_image(&cache, KIT_BASE, ColorSpace::Srgb);
        assert!(cache.cook_misses().is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_cook_fallback_with_no_new_file_keeps_its_entry() {
        let dir = cook_project("retry-none", None);
        let mut cache = AssetCache::new();
        cache.set_cook(lookup(&dir, Some(CookTarget::Desktop)));
        cache
            .load_mesh(ludion_assets::AssetRoot::content(&dir), KIT_MESH)
            .unwrap();
        assert!(!cache.retry_cook_misses());
        assert!(cache.contains_mesh(KIT_MESH));
        assert_eq!(cache.cook_misses().len(), 2);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_failed_load_is_remembered_until_forgotten() {
        let dir = revalidation_dir("memo-forget");
        let mut cache = AssetCache::new();
        let first = texture_err(&mut cache, &dir);
        write_png(&dir.join("t.png"), [1, 2, 3, 255]);
        assert_eq!(texture_err(&mut cache, &dir), first);
        cache.forget_failures();
        assert!(
            cache
                .load(ludion_assets::AssetRoot::content(&dir), "t.png")
                .is_ok()
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_remembered_failure_expires_after_its_passes() {
        let dir = revalidation_dir("memo-expire");
        let mut cache = AssetCache::new();
        texture_err(&mut cache, &dir);
        write_png(&dir.join("t.png"), [1, 2, 3, 255]);
        for _ in 0..FAILURE_RETRY_PASSES - 1 {
            cache.age_failures();
        }
        cache
            .load(ludion_assets::AssetRoot::content(&dir), "t.png")
            .unwrap_err();
        cache.age_failures();
        assert!(
            cache
                .load(ludion_assets::AssetRoot::content(&dir), "t.png")
                .is_ok()
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_rebuild_a_reload_and_a_cook_change_each_clear_remembered_failures() {
        let dir = revalidation_dir("memo-clearers");
        let root = ludion_assets::AssetRoot::content(&dir);
        let remembered = |cache: &mut AssetCache| {
            texture_err(cache, &dir);
            assert!(!cache.failures.is_empty());
        };

        let mut cache = AssetCache::new();
        remembered(&mut cache);
        cache.retain_referenced(root, &AssetRefs::default());
        assert!(cache.failures.is_empty(), "a rebuild");

        remembered(&mut cache);
        write_png(&dir.join("other.png"), [1, 2, 3, 255]);
        cache.load(root, "other.png").unwrap();
        cache.reload(root, "other.png").unwrap();
        assert!(cache.failures.is_empty(), "a reload");

        remembered(&mut cache);
        assert!(!cache.set_cook(None), "an equal setting changes nothing");
        assert!(!cache.failures.is_empty());
        let cook_dir = cook_project("memo-clearers-cook", None);
        assert!(cache.set_cook(lookup(&cook_dir, Some(CookTarget::Desktop))));
        assert!(cache.failures.is_empty(), "a cook change");
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(&cook_dir);
    }

    #[test]
    fn a_failure_keeps_its_diagnostic() {
        let dir = revalidation_dir("memo-diagnostic");
        let mut cache = AssetCache::new();
        let cold = texture_err(&mut cache, &dir);
        assert_eq!(texture_err(&mut cache, &dir), cold);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn prefab_rests_never_exceed_their_cap() {
        let text = include_str!("../../../examples/import-node-clips/expected/model.prefab.yaml");
        let mut cache = AssetCache::new();
        for n in 0..PREFAB_RESTS_CAP + 8 {
            let named = text.replacen("name: hinge", &format!("name: hinge{n}"), 1);
            assert!(cache.prefab_rests(&named).is_some(), "text {n} parses");
            assert!(cache.prefab_rests_len() <= PREFAB_RESTS_CAP);
        }
        let later = text.replacen("name: hinge", "name: later", 1);
        assert!(cache.prefab_rests(&later).is_some());
    }
}
