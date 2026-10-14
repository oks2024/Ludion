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
//! is one decode however many meshes bind it.
//!
//! After a world rebuild the engine's first cached extract calls [`AssetCache::prefetch`] with the
//! assets the world names: it decodes them on the worker pool and inserts the results serially in
//! sorted key order, so the cache is the same at any thread count and the extract that follows only
//! hits. A failed decode is left for that extract to report.
//!
//! Asset bytes live here and nowhere in the world, so a reload is invisible to the `WorldHash` and a
//! recorded session — sim-invisible and replay-safe.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::sync::Arc;

use ludion_core::diagnostics::Diagnostic;
use ludion_renderer::{MeshData, TextureImage, WorldVoxelGrid};
use ludion_scene::MeshDocument;

use crate::assembly_mesh::AssemblyData;
use crate::character::CharacterData;
use crate::extract::{AtlasAsset, load_texture};
use crate::load_observer::{LoadKind, LoadObserver, LoadPhase};
use crate::voxel_mesh::VoxelStaticGrid;

/// Which of the cache's maps an entry lives in — the tag a dependency edge carries so a reload
/// re-runs the right loader.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum AssetKind {
    Texture,
    /// A texture map a mesh document binds: held once however many meshes name it.
    MeshMap,
    Mesh,
    Voxel,
    VoxelGrid,
    Sculpt,
    Assembly,
    Character,
    Atlas,
}

impl AssetKind {
    fn load_kind(self) -> LoadKind {
        match self {
            AssetKind::Texture | AssetKind::MeshMap => LoadKind::Texture,
            AssetKind::Mesh => LoadKind::Mesh,
            AssetKind::Voxel => LoadKind::Voxel,
            AssetKind::VoxelGrid => LoadKind::VoxelGrid,
            AssetKind::Sculpt => LoadKind::Sculpt,
            AssetKind::Assembly => LoadKind::Assembly,
            AssetKind::Character => LoadKind::Character,
            AssetKind::Atlas => LoadKind::Atlas,
        }
    }

    fn load_phase(self) -> LoadPhase {
        match self {
            AssetKind::Texture | AssetKind::MeshMap => LoadPhase::Texture,
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
    pub(crate) meshes: BTreeSet<String>,
    pub(crate) voxels: BTreeSet<String>,
    pub(crate) sculpts: BTreeSet<String>,
    pub(crate) assemblies: BTreeSet<String>,
    pub(crate) characters: BTreeSet<String>,
}

/// One decoded entry, ready to insert: what a worker thread hands back to the cache's owner. Every
/// kind but a mesh, which needs its maps from the cache to be assembled.
enum Decoded {
    Texture(Arc<TextureImage>),
    MeshMap(Arc<TextureImage>),
    Voxel(MeshData),
    VoxelGrid(VoxelStaticGrid),
    Sculpt(MeshData),
    Assembly(AssemblyData),
    Character(CharacterData),
    Atlas(AtlasAsset),
}

/// Runs `kind`'s loader for `rel` with no access to the cache, returning the value and the files it
/// read. Pure, so a prefetch can run it on any thread. Never called for [`AssetKind::Mesh`].
fn decode(
    project_root: &Path,
    kind: AssetKind,
    rel: &str,
    key: &str,
) -> Result<(Decoded, Vec<String>), Vec<Diagnostic>> {
    let own = || vec![key.to_string()];
    Ok(match kind {
        AssetKind::Texture => {
            // Premultiplied on the miss, never on the hit: the convention is a property of the
            // decoded bytes, and every frame after this one reads them straight back out.
            let image = load_texture(project_root, rel).map_err(|d| vec![d])?;
            (Decoded::Texture(image), own())
        }
        AssetKind::MeshMap => {
            let image = crate::mesh_asset::decode_map(project_root, rel).map_err(|d| vec![d])?;
            (Decoded::MeshMap(image), own())
        }
        AssetKind::Mesh => unreachable!("a mesh is assembled against the cache's maps"),
        AssetKind::Voxel => {
            let mesh = crate::voxel_mesh::load(project_root, rel)?;
            (Decoded::Voxel(mesh), own())
        }
        AssetKind::VoxelGrid => {
            let grid = crate::voxel_mesh::load_grid(project_root, rel)?;
            (Decoded::VoxelGrid(grid), own())
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
        AssetKind::Atlas => {
            let asset = crate::extract::load_atlas(project_root, rel)?;
            let color = ludion_core::paths::sibling(rel, &asset.doc.color_map);
            (Decoded::Atlas(asset), vec![key.to_string(), color])
        }
    })
}

/// A CPU cache of decoded assets — texture [`TextureImage`]s, asset [`MeshData`], voxel-model
/// [`MeshData`], the voxel-model [`VoxelStaticGrid`] (D15), sculpt-model [`MeshData`], meshed
/// [`AssemblyData`], and [`AtlasAsset`]s — keyed by slash-normalized project-root-relative path.
///
/// One map per asset kind (a path names exactly one kind, though a voxel path names both a mesh and a
/// grid), all under the one cache so `asset.reload` and `scene.load` invalidate textures, meshes,
/// voxel models, assemblies, and atlases uniformly. Every kind records which files it was read from
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
    /// The maps meshes bind, one entry per file whichever meshes name it.
    mesh_maps: BTreeMap<String, Arc<TextureImage>>,
    meshes: BTreeMap<String, Arc<MeshData>>,
    voxels: BTreeMap<String, Arc<MeshData>>,
    voxel_grids: BTreeMap<String, Arc<VoxelStaticGrid>>,
    sculpts: BTreeMap<String, Arc<MeshData>>,
    assemblies: BTreeMap<String, Arc<AssemblyData>>,
    characters: BTreeMap<String, Arc<CharacterData>>,
    atlases: BTreeMap<String, Arc<AtlasAsset>>,
    /// Which cached entries each **source file** feeds, as `(kind, entry key)`: an entry's own
    /// document key maps to itself, and every other file it read (an assembly's part or clip, a
    /// sculpt's voxel or mesh part, a mesh's texture, an atlas's colour map) maps to every entry that
    /// names it — one limb file, or one PNG, legitimately serves several. This is what makes an edit
    /// to such a file reloadable: the watcher reports the file that changed, and only the cache knows
    /// which entries were read out of it.
    dependents: BTreeMap<String, Vec<(AssetKind, String)>>,
    voxelization: Option<(u64, Option<Arc<WorldVoxelGrid>>)>,
    /// Whether a host is holding the voxelized baseline: a signature miss then serves the retained
    /// entry **without rewriting its key**, so the held grid stays the baseline however far the world
    /// moves from it. Host/session state a host sets per frame
    /// ([`Engine::set_gi_rebake`](crate::Engine::set_gi_rebake)) — never world content, so it is not
    /// cleared by [`clear`](AssetCache::clear). Every world rebuild forces one unheld frame, so the
    /// new world's first build replaces the retained entry and becomes the held baseline.
    gi_hold: bool,
    /// Whether the last hold-aware read served the retained grid for a signature it was not built
    /// under. Reset at the start of every extract and recomputed there, never latched — the
    /// signature comparison runs every frame anyway, so a world that returns to the held baseline is
    /// honestly not stale.
    gi_stale: bool,
    /// Load instrumentation a host attached. Host state, not content, so [`clear`](AssetCache::clear)
    /// leaves it alone — the scene load that clears the cache is the very event it measures.
    observer: Option<Box<dyn LoadObserver>>,
}

impl AssetCache {
    /// An empty cache.
    pub(crate) fn new() -> Self {
        AssetCache {
            images: BTreeMap::new(),
            mesh_maps: BTreeMap::new(),
            meshes: BTreeMap::new(),
            voxels: BTreeMap::new(),
            voxel_grids: BTreeMap::new(),
            sculpts: BTreeMap::new(),
            assemblies: BTreeMap::new(),
            characters: BTreeMap::new(),
            atlases: BTreeMap::new(),
            dependents: BTreeMap::new(),
            voxelization: None,
            gi_hold: false,
            gi_stale: false,
            observer: None,
        }
    }

    /// Attaches (or detaches) the load observer.
    pub(crate) fn set_observer(&mut self, observer: Option<Box<dyn LoadObserver>>) {
        self.observer = observer;
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

    /// Drops every entry — called on a `scene.load`, so a new scene's assets are never shadowed by a
    /// previous scene's stale bytes.
    ///
    /// [`gi_hold`](AssetCache::gi_hold) deliberately survives: it is the *host's* lever, not the
    /// scene's. The unheld frame every rebuild forces is what makes the new world build once and then
    /// hold from there.
    pub(crate) fn clear(&mut self) {
        self.images.clear();
        self.mesh_maps.clear();
        self.meshes.clear();
        self.voxels.clear();
        self.voxel_grids.clear();
        self.sculpts.clear();
        self.assemblies.clear();
        self.characters.clear();
        self.atlases.clear();
        self.dependents.clear();
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
        project_root: &Path,
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
        project_root: &Path,
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
        project_root: &Path,
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
        project_root: &Path,
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
        project_root: &Path,
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
        project_root: &Path,
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

    /// Reads the character at `rel` **through** the cache: a miss reads the character document, its
    /// mesh document and every clip file it names, and records all of them as reload dependencies —
    /// so editing the mesh or any clip re-reads the character.
    ///
    /// # Errors
    /// `L1101`/`L1199`/`L1200` (and the mesh's and clips' own codes) if a *missing* entry cannot be
    /// read, parsed, or validated; a cached entry never errors.
    pub(crate) fn load_character(
        &mut self,
        project_root: &Path,
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
        project_root: &Path,
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

    /// Loads the `kind` entry at `rel` from disk, inserts it, and records the files it was read out
    /// of. The one place each kind's loader is named: a miss and a reload both run through it.
    fn load_entry(
        &mut self,
        project_root: &Path,
        kind: AssetKind,
        rel: &str,
    ) -> Result<(), Vec<Diagnostic>> {
        let key = normalize_key(rel);
        let phase = kind.load_phase();
        if let Some(observer) = self.observer.as_deref_mut() {
            observer.begin(phase);
        }
        let decoded = self.decode_entry(project_root, kind, rel, &key);
        if let Some(observer) = self.observer.as_deref_mut() {
            observer.end(phase);
        }
        let sources = decoded?;
        self.report_decoded(project_root, kind, &key, &sources);
        self.record_sources(kind, &key, &sources);
        Ok(())
    }

    /// Tells the observer what a successful decode read: per source, the on-disk size. A mesh's
    /// document is a mesh decode and each map after it a texture decode; every other kind is one
    /// decode over all its files.
    fn report_decoded(
        &mut self,
        project_root: &Path,
        kind: AssetKind,
        key: &str,
        sources: &[String],
    ) {
        let Some(observer) = self.observer.as_deref_mut() else {
            return;
        };
        let len = |src: &String| std::fs::metadata(project_root.join(src)).map_or(0, |m| m.len());
        if kind == AssetKind::Mesh {
            // The maps report themselves: each is its own entry, shared by every mesh naming it.
            if let Some(document) = sources.first() {
                observer.decoded(LoadKind::Mesh, key, len(document));
            }
        } else {
            observer.decoded(kind.load_kind(), key, sources.iter().map(len).sum());
        }
    }

    /// Reads the map at the resolved path `resolved` **through** the cache: a hit is reported as
    /// reused, a miss decodes it once as an entry of its own. Every mesh naming one file shares the
    /// `Arc`, so the decode counter is unique files, not references.
    fn mesh_map(
        &mut self,
        project_root: &Path,
        resolved: &str,
    ) -> Result<Arc<TextureImage>, Diagnostic> {
        let key = normalize_key(resolved);
        if !self.mesh_maps.contains_key(&key) {
            self.load_entry(project_root, AssetKind::MeshMap, resolved)
                .map_err(first_diagnostic)?;
        } else if let Some(observer) = self.observer.as_deref_mut() {
            observer.reused(LoadKind::Texture, &key);
        }
        Ok(Arc::clone(
            self.mesh_maps.get(&key).expect("just inserted on a miss"),
        ))
    }

    /// Runs `kind`'s loader, inserts the result under `key`, and returns the files it read.
    fn decode_entry(
        &mut self,
        project_root: &Path,
        kind: AssetKind,
        rel: &str,
        key: &str,
    ) -> Result<Vec<String>, Vec<Diagnostic>> {
        if kind == AssetKind::Mesh {
            let (doc, hash) = crate::mesh_asset::read_document(project_root, rel)?;
            let (mesh, sources) = crate::mesh_asset::assemble(rel, &doc, hash, &mut |path| {
                self.mesh_map(project_root, path)
            })?;
            self.meshes.insert(key.to_string(), Arc::new(mesh));
            return Ok(sources);
        }
        let (decoded, sources) = decode(project_root, kind, rel, key)?;
        self.insert(key, decoded);
        Ok(sources)
    }

    /// Inserts a decoded entry under `key`.
    fn insert(&mut self, key: &str, decoded: Decoded) {
        let key = key.to_string();
        match decoded {
            Decoded::Texture(image) => self.images.insert(key, image).is_some(),
            Decoded::MeshMap(image) => self.mesh_maps.insert(key, image).is_some(),
            Decoded::Voxel(mesh) => self.voxels.insert(key, Arc::new(mesh)).is_some(),
            Decoded::VoxelGrid(grid) => self.voxel_grids.insert(key, Arc::new(grid)).is_some(),
            Decoded::Sculpt(mesh) => self.sculpts.insert(key, Arc::new(mesh)).is_some(),
            Decoded::Assembly(data) => self.assemblies.insert(key, Arc::new(data)).is_some(),
            Decoded::Character(data) => self.characters.insert(key, Arc::new(data)).is_some(),
            Decoded::Atlas(asset) => self.atlases.insert(key, Arc::new(asset)).is_some(),
        };
    }

    /// Whether the `kind` entry at `key` is cached.
    fn holds(&self, kind: AssetKind, key: &str) -> bool {
        match kind {
            AssetKind::Texture => self.images.contains_key(key),
            AssetKind::MeshMap => self.mesh_maps.contains_key(key),
            AssetKind::Mesh => self.meshes.contains_key(key),
            AssetKind::Voxel => self.voxels.contains_key(key),
            AssetKind::VoxelGrid => self.voxel_grids.contains_key(key),
            AssetKind::Sculpt => self.sculpts.contains_key(key),
            AssetKind::Assembly => self.assemblies.contains_key(key),
            AssetKind::Character => self.characters.contains_key(key),
            AssetKind::Atlas => self.atlases.contains_key(key),
        }
    }

    /// Removes the `kind` entry at `key` from its map.
    fn remove_entry(&mut self, kind: AssetKind, key: &str) {
        match kind {
            AssetKind::Texture => self.images.remove(key).is_some(),
            AssetKind::MeshMap => self.mesh_maps.remove(key).is_some(),
            AssetKind::Mesh => self.meshes.remove(key).is_some(),
            AssetKind::Voxel => self.voxels.remove(key).is_some(),
            AssetKind::VoxelGrid => self.voxel_grids.remove(key).is_some(),
            AssetKind::Sculpt => self.sculpts.remove(key).is_some(),
            AssetKind::Assembly => self.assemblies.remove(key).is_some(),
            AssetKind::Character => self.characters.remove(key).is_some(),
            AssetKind::Atlas => self.atlases.remove(key).is_some(),
        };
    }

    /// Records the `kind` entry `key` as depending on each of `sources` (its own document included).
    fn record_sources(&mut self, kind: AssetKind, key: &str, sources: &[String]) {
        for source in sources {
            let dependents = self.dependents.entry(normalize_key(source)).or_default();
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

    /// Decodes every asset in `refs` that the cache does not hold yet, in parallel, ahead of the
    /// extract that would otherwise decode them one by one on its miss path.
    ///
    /// Four stages, each decoding on worker threads and then inserting **serially in sorted key
    /// order**, so the cache's contents, its dependency edges and the observer's event order never
    /// depend on the thread count: the non-mesh kinds, every mesh document, every map those
    /// documents bind plus the loose textures, and last each mesh assembled against its maps. A
    /// failure is dropped here — the extract that follows hits the same miss and reports it with its
    /// usual diagnostic. Voxel grids and the volley, particle, UI and font atlases stay lazy.
    ///
    /// Never call the observer from a worker: each stage is bracketed from this thread.
    pub(crate) fn prefetch(&mut self, project_root: &Path, refs: &AssetRefs) {
        // Stage 1: everything that is one file-set to one value.
        let mut jobs: Vec<(AssetKind, &str)> = Vec::new();
        let others: [(AssetKind, &BTreeSet<String>); 5] = [
            (AssetKind::Atlas, &refs.atlases),
            (AssetKind::Voxel, &refs.voxels),
            (AssetKind::Sculpt, &refs.sculpts),
            (AssetKind::Assembly, &refs.assemblies),
            (AssetKind::Character, &refs.characters),
        ];
        for (kind, keys) in others {
            jobs.extend(
                keys.iter()
                    .filter(|key| !self.holds(kind, key))
                    .map(|key| (kind, key.as_str())),
            );
        }
        self.decode_batch(project_root, LoadPhase::Other, &jobs);

        // Stage 2: the mesh documents — the named ones and each character's skinned mesh.
        let mut mesh_keys: BTreeSet<String> = refs.meshes.clone();
        for key in &refs.characters {
            if let Some(character) = self.characters.get(key) {
                mesh_keys.insert(normalize_key(&character.mesh));
            }
        }
        let todo: Vec<&String> = mesh_keys
            .iter()
            .filter(|key| !self.meshes.contains_key(*key))
            .collect();
        self.begin(LoadPhase::Mesh);
        let read = ludion_core::par::par_map_chunks(&todo, 1, |_, chunk| {
            crate::mesh_asset::read_document(project_root, chunk[0])
        });
        self.end(LoadPhase::Mesh);
        let docs: Vec<(&String, MeshDocument, u64)> = todo
            .into_iter()
            .zip(read)
            .filter_map(|(key, read)| read.ok().map(|(doc, hash)| (key, doc, hash)))
            .collect();

        // Stage 3: the maps those documents bind, then the loose textures.
        let mut maps: BTreeSet<String> = BTreeSet::new();
        for (key, doc, _) in &docs {
            for path in crate::mesh_asset::map_paths(key, doc) {
                let path = normalize_key(&path);
                if !self.mesh_maps.contains_key(&path) {
                    maps.insert(path);
                }
            }
        }
        let mut jobs: Vec<(AssetKind, &str)> = maps
            .iter()
            .map(|key| (AssetKind::MeshMap, key.as_str()))
            .collect();
        jobs.extend(
            refs.textures
                .iter()
                .filter(|key| !self.images.contains_key(*key))
                .map(|key| (AssetKind::Texture, key.as_str())),
        );
        self.decode_batch(project_root, LoadPhase::Texture, &jobs);

        // Stage 4: assemble serially, so a map that failed above fails its meshes the same way.
        self.begin(LoadPhase::Mesh);
        for (key, doc, hash) in &docs {
            let assembled = crate::mesh_asset::assemble(key, doc, *hash, &mut |path| {
                self.mesh_maps
                    .get(&normalize_key(path))
                    .cloned()
                    .ok_or_else(|| {
                        Diagnostic::new(
                            ludion_core::codes::code::ASSET_FILE_IO,
                            format!("mesh map `{path}` did not decode"),
                        )
                    })
            });
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
    fn decode_batch(&mut self, project_root: &Path, phase: LoadPhase, jobs: &[(AssetKind, &str)]) {
        if jobs.is_empty() {
            return;
        }
        self.begin(phase);
        let decoded = ludion_core::par::par_map_chunks(jobs, 1, |_, chunk| {
            let (kind, key) = chunk[0];
            decode(project_root, kind, key, key)
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
    pub(crate) fn resolve_key(&self, project_root: &Path, incoming: &str) -> Option<String> {
        let direct = normalize_key(incoming);
        if self.dependents.contains_key(&direct) {
            return Some(direct);
        }
        // Cold path: match by canonical filesystem identity, which handles a host-forwarded absolute
        // path whose textual form differs from the scene's relative one.
        let target = std::fs::canonicalize(project_root.join(incoming)).ok()?;
        self.dependents
            .keys()
            .find(|rel| {
                std::fs::canonicalize(project_root.join(rel.as_str()))
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
    /// failure is returned and each failed entry is left dropped, so a later successful reload (or
    /// the next `extract`) reinstates it.
    pub(crate) fn reload(&mut self, project_root: &Path, key: &str) -> Result<u64, Diagnostic> {
        let dependents = self.dependents.get(key).cloned().unwrap_or_default();
        // Drop every entry before any re-read: one failing early must not leave a later dependent
        // (a character built on a mesh that just changed) holding the stale copy.
        for (kind, entry) in &dependents {
            self.remove_entry(*kind, entry);
            self.drop_sources(*kind, entry);
        }
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

    /// Whether the cache holds a texture at `key` (in its normalized form). Test-only.
    #[cfg(test)]
    pub(crate) fn contains(&self, key: &str) -> bool {
        self.images.contains_key(&normalize_key(key))
    }

    /// Whether the cache holds a mesh map at `key` (in its normalized form). Test-only.
    #[cfg(test)]
    pub(crate) fn contains_mesh_map(&self, key: &str) -> bool {
        self.mesh_maps.contains_key(&normalize_key(key))
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
        let first = cache.load(&dir, "t.png").unwrap().hash;
        assert!(cache.contains("t.png"));
        // Hit reuses the same decoded bytes even after the file changes on disk (no re-read).
        write_png(&dir.join("t.png"), [200, 100, 50, 255]);
        let cached = cache.load(&dir, "t.png").unwrap().hash;
        assert_eq!(cached, first, "a hit must not re-read the changed file");

        // resolve_key finds the entry from the relative name and from an absolute path.
        let by_rel = cache.resolve_key(&dir, "t.png");
        assert_eq!(by_rel.as_deref(), Some("t.png"));
        let abs = dir.join("t.png");
        let by_abs = cache.resolve_key(&dir, &abs.to_string_lossy());
        assert_eq!(
            by_abs.as_deref(),
            Some("t.png"),
            "an absolute path resolves to the rel key"
        );

        // Reload drops the entry and re-reads the new bytes → a different hash.
        let reloaded = cache.reload(&dir, "t.png").unwrap();
        assert_ne!(reloaded, first, "reload must pick up the changed file");
        assert_eq!(cache.load(&dir, "t.png").unwrap().hash, reloaded);

        let _ = std::fs::remove_dir_all(&dir);
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
        cache.load(&dir, "t.png").unwrap();
        write_png(&dir.join("t.png"), [200, 100, 50, 128]);
        cache.reload(&dir, "t.png").unwrap();

        let raw = ludion_assets::load_png(&dir, "t.png").unwrap();
        let expected = raw.clone().premultiplied().rgba;
        let reloaded = cache.load(&dir, "t.png").unwrap().rgba.clone();
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
        let first = cache.load_atlas(&dir, "a.atlas.yaml").unwrap().texture.hash;
        assert!(cache.contains_atlas("a.atlas.yaml"));

        // Both source files resolve to the atlas's own key — the colour PNG included, which is the
        // half a texture-only cache would miss.
        assert_eq!(
            cache.resolve_key(&dir, "a.atlas.yaml").as_deref(),
            Some("a.atlas.yaml")
        );
        assert_eq!(
            cache.resolve_key(&dir, "a.png").as_deref(),
            Some("a.png"),
            "the packed image is a key of its own, expanded by `reload`"
        );

        // A rewritten image, reported under the *image's* key, re-decodes the atlas.
        write_png(&dir.join("a.png"), [200, 100, 50, 255]);
        cache.reload(&dir, "a.png").unwrap();
        let after_image = cache.load_atlas(&dir, "a.atlas.yaml").unwrap().texture.hash;
        assert_ne!(after_image, first, "the packed image moved");

        // A rewritten document, reported under its own key, re-parses it.
        std::fs::write(dir.join("a.atlas.yaml"), atlas_doc("16.0")).unwrap();
        cache.reload(&dir, "a.atlas.yaml").unwrap();
        assert_eq!(
            cache
                .load_atlas(&dir, "a.atlas.yaml")
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
        assert_eq!(cache.resolve_key(&dir, "ludion-not-cached-xyz.png"), None);
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
        let first = cache.load_mesh(&dir, rel).unwrap().hash;
        assert!(cache.contains_mesh(rel));
        // Normals were absent → computed faceted (+Z face) for all three vertices.
        assert_eq!(
            cache.load_mesh(&dir, rel).unwrap().normals,
            vec![0.0, 0.0, 1.0, 0.0, 0.0, 1.0, 0.0, 0.0, 1.0]
        );

        // A hit does not re-read a changed file.
        std::fs::write(dir.join(rel), mesh_doc(5.0)).unwrap();
        assert_eq!(cache.load_mesh(&dir, rel).unwrap().hash, first);

        // resolve_key finds the mesh from its relative name and from an absolute path.
        assert_eq!(cache.resolve_key(&dir, rel).as_deref(), Some(rel));
        let abs = dir.join(rel);
        assert_eq!(
            cache.resolve_key(&dir, &abs.to_string_lossy()).as_deref(),
            Some(rel)
        );

        // Reload drops the entry and re-reads the changed bytes → a different hash.
        let reloaded = cache.reload(&dir, rel).unwrap();
        assert_ne!(reloaded, first);
        assert_eq!(cache.load_mesh(&dir, rel).unwrap().hash, reloaded);

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
        let first = cache.load_voxel(&dir, rel).unwrap().hash;
        assert!(cache.contains_voxel(rel));
        assert_eq!(
            cache.load_voxel(&dir, rel).unwrap().positions.len(),
            24 * 3,
            "one filled cell meshes to a 24-vertex box"
        );

        // A hit does not re-read a changed file.
        std::fs::write(dir.join(rel), voxel_doc("B")).unwrap();
        assert_eq!(cache.load_voxel(&dir, rel).unwrap().hash, first);

        // resolve_key finds the voxel from its relative name and from an absolute path.
        assert_eq!(cache.resolve_key(&dir, rel).as_deref(), Some(rel));
        let abs = dir.join(rel);
        assert_eq!(
            cache.resolve_key(&dir, &abs.to_string_lossy()).as_deref(),
            Some(rel)
        );

        // Reload drops the entry and re-reads the changed bytes → a different hash.
        let reloaded = cache.reload(&dir, rel).unwrap();
        assert_ne!(reloaded, first);
        assert_eq!(cache.load_voxel(&dir, rel).unwrap().hash, reloaded);

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
        let first = cache.load_sculpt(&dir, rel).unwrap().hash;
        assert!(cache.contains_sculpt(rel));
        assert!(!cache.load_sculpt(&dir, rel).unwrap().positions.is_empty());

        // A hit does not re-read a changed file.
        std::fs::write(dir.join(rel), sculpt_doc("0.4")).unwrap();
        assert_eq!(cache.load_sculpt(&dir, rel).unwrap().hash, first);

        assert_eq!(cache.resolve_key(&dir, rel).as_deref(), Some(rel));
        let abs = dir.join(rel);
        assert_eq!(
            cache.resolve_key(&dir, &abs.to_string_lossy()).as_deref(),
            Some(rel)
        );

        // Reload drops the entry and re-meshes the changed bytes → a different hash.
        let reloaded = cache.reload(&dir, rel).unwrap();
        assert_ne!(reloaded, first);
        assert_eq!(cache.load_sculpt(&dir, rel).unwrap().hash, reloaded);

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
            .load_assembly(&dir, rel)
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
        let held = cache.load_assembly(&dir, rel).unwrap().clone();
        assert_eq!(
            held.parts
                .iter()
                .map(|part| part.mesh.hash)
                .collect::<Vec<_>>(),
            first
        );

        // Both source files resolve — the assembly document and the part — by relative and by
        // absolute path. A part answers with its own key, which `reload` expands to its assemblies.
        assert_eq!(cache.resolve_key(&dir, rel).as_deref(), Some(rel));
        assert_eq!(cache.resolve_key(&dir, body).as_deref(), Some(body));
        let abs = dir.join(body);
        assert_eq!(
            cache.resolve_key(&dir, &abs.to_string_lossy()).as_deref(),
            Some(body)
        );

        // Reloading the *part* re-reads and re-meshes the assembly that named it: every part's mesh
        // key moves, because the assembly's content hash folds every part file's.
        let reloaded = cache.reload(&dir, body).unwrap();
        let after: Vec<u64> = cache
            .load_assembly(&dir, rel)
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
            ludion_assets::load_bytes(&dir, body).unwrap().hash,
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
        let before = cache.load_assembly(&dir, rel).unwrap().clone();
        assert_eq!(
            before.clip("walk").unwrap().tracks[0].rotation[1].v[0],
            90.0
        );

        // The clip file is a source of the assembly, so it resolves as a reload key.
        assert_eq!(cache.resolve_key(&dir, clip).as_deref(), Some(clip));

        std::fs::write(dir.join(clip), clip_source(45.0)).unwrap();
        let reported = cache.reload(&dir, clip).unwrap();
        let after = cache.load_assembly(&dir, rel).unwrap().clone();
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
            ludion_assets::load_bytes(&dir, clip).unwrap().hash,
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
        let rel = "characters/hero.character.yaml";
        std::fs::write(
            dir.join(rel),
            "format: 1\nkind: character\nmesh: ../meshes/skin-mannequin.mesh.yaml\nclips:\n  idle:\n    clip: ../clips/idle.anim.yaml\n",
        )
        .unwrap();

        let mut cache = AssetCache::new();
        assert!(!cache.contains_character(rel));
        let first = cache.load_character(&dir, rel).unwrap().clone();
        assert!(cache.contains_character(rel));
        assert_eq!(first.mesh, mesh);
        assert_eq!(first.clip("idle").unwrap().clip.duration, 2.0);
        let hit = cache.load_character(&dir, rel).unwrap().clone();
        assert!(Arc::ptr_eq(&first, &hit), "a hit reuses the cached entry");

        // The mesh document is a source of the character.
        assert_eq!(cache.resolve_key(&dir, mesh).as_deref(), Some(mesh));
        cache.reload(&dir, mesh).unwrap();
        let after_mesh = cache.load_character(&dir, rel).unwrap().clone();
        assert!(
            !Arc::ptr_eq(&first, &after_mesh),
            "a mesh reload re-reads the character"
        );

        // So is each clip file.
        let edited = std::fs::read_to_string(dir.join(clip))
            .unwrap()
            .replace("duration: 2.0", "duration: 3.0");
        std::fs::write(dir.join(clip), edited).unwrap();
        assert_eq!(cache.resolve_key(&dir, clip).as_deref(), Some(clip));
        cache.reload(&dir, clip).unwrap();
        let after_clip = cache.load_character(&dir, rel).unwrap().clone();
        assert_eq!(after_clip.clip("idle").unwrap().clip.duration, 3.0);

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// One dependent failing to re-read must not leave a later dependent of the same file stale: the
    /// mesh is valid but renames a joint the character's clip tracks, so the character fails `L1200`
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
        let rel = "characters/hero.character.yaml";
        std::fs::write(
            dir.join(rel),
            "format: 1\nkind: character\nmesh: ../meshes/skin-mannequin.mesh.yaml\nclips:\n  idle:\n    clip: ../clips/idle.anim.yaml\n",
        )
        .unwrap();

        // The character first, so the mesh's dependents are [Character, Mesh].
        let mut cache = AssetCache::new();
        cache.load_character(&dir, rel).unwrap();
        cache.load_mesh(&dir, mesh).unwrap();

        let renamed = std::fs::read_to_string(dir.join(mesh))
            .unwrap()
            .replace("  - name: head", "  - name: skull");
        std::fs::write(dir.join(mesh), renamed).unwrap();
        let error = cache.reload(&dir, mesh).unwrap_err();
        assert_eq!(error.code.as_str(), "L1200", "{error:?}");
        assert!(
            !cache.contains_character(rel),
            "the failed entry is dropped"
        );
        let joints = &cache.load_mesh(&dir, mesh).unwrap().skin_joints;
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
        let first = cache.load_voxel_grid(&dir, rel).unwrap().clone();
        assert!(cache.contains_voxel_grid(rel));
        assert_eq!(first.extent, [1, 1, 1]);
        let cell = first.cells[0].expect("the one cell is filled");
        assert_eq!(cell.albedo, [1.0, 0.5, 0.2, 1.0]);
        assert_eq!(cell.emissive, 2.0);

        // A hit does not re-read a changed file, and hands back the same `Arc` allocation.
        std::fs::write(dir.join(rel), voxel_doc("B")).unwrap();
        let second = cache.load_voxel_grid(&dir, rel).unwrap();
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
        let first = cache.load_sculpt(&dir, rel).unwrap().hash;
        assert_ne!(first, ludion_assets::load_bytes(&dir, rel).unwrap().hash);

        // The part file resolves — by relative name and by absolute path — to the sculpt's edge.
        assert_eq!(cache.resolve_key(&dir, part).as_deref(), Some(part));
        let abs = dir.join(part);
        assert_eq!(
            cache.resolve_key(&dir, &abs.to_string_lossy()).as_deref(),
            Some(part)
        );

        write_voxel_sculpt(&dir, "B");
        let reloaded = cache.reload(&dir, part).unwrap();
        assert_eq!(
            reloaded,
            ludion_assets::load_bytes(&dir, part).unwrap().hash
        );
        assert!(cache.contains_sculpt(rel));
        assert_ne!(cache.load_sculpt(&dir, rel).unwrap().hash, first);

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
        let first = cache.load_mesh(&dir, rel).unwrap().hash;
        assert_ne!(first, ludion_assets::load_bytes(&dir, rel).unwrap().hash);

        // A watcher reports the canonical absolute path of the PNG.
        let abs = std::fs::canonicalize(dir.join("textures/t.png")).unwrap();
        let key = cache
            .resolve_key(&dir, &abs.to_string_lossy())
            .expect("the mesh's texture is a dependency");

        std::fs::write(dir.join("textures/t.png"), png_bytes(200)).unwrap();
        let reloaded = cache.reload(&dir, &key).unwrap();
        assert_eq!(
            reloaded,
            ludion_assets::load_bytes(&dir, "textures/t.png")
                .unwrap()
                .hash
        );
        assert_ne!(cache.load_mesh(&dir, rel).unwrap().hash, first);

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
        let mesh_first = cache.load_mesh(&dir, rel).unwrap().hash;
        let image_first = cache.load(&dir, png_rel).unwrap().hash;

        std::fs::write(dir.join(png_rel), png_bytes(200)).unwrap();
        let reloaded = cache.reload(&dir, png_rel).unwrap();
        assert_ne!(reloaded, image_first);
        assert!(cache.contains(png_rel) && cache.contains_mesh(rel));
        assert_eq!(cache.load(&dir, png_rel).unwrap().hash, reloaded);
        assert_ne!(cache.load_mesh(&dir, rel).unwrap().hash, mesh_first);

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
        let mesh_first = cache.load_mesh(&dir, rel).unwrap().hash;
        let image_first = cache.load(&dir, png_rel).unwrap().hash;

        for spelling in ["textures/t.png", "meshes/../textures/t.png"] {
            assert_eq!(
                cache.resolve_key(&dir, spelling).as_deref(),
                Some(png_rel),
                "{spelling}"
            );
        }

        std::fs::write(dir.join(png_rel), png_bytes(200)).unwrap();
        let key = cache.resolve_key(&dir, "meshes/../textures/t.png").unwrap();
        let reloaded = cache.reload(&dir, &key).unwrap();
        assert_ne!(reloaded, image_first);
        assert_eq!(cache.load(&dir, png_rel).unwrap().hash, reloaded);
        assert_ne!(cache.load_mesh(&dir, rel).unwrap().hash, mesh_first);

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Records every decode the cache reports, shared with the test that attached it.
    struct Decodes(std::sync::Arc<std::sync::Mutex<Vec<(LoadKind, String)>>>);

    impl LoadObserver for Decodes {
        fn begin(&mut self, _: LoadPhase) {}
        fn end(&mut self, _: LoadPhase) {}
        fn decoded(&mut self, kind: LoadKind, key: &str, _: u64) {
            self.0.lock().unwrap().push((kind, key.to_string()));
        }
        fn reused(&mut self, _: LoadKind, _: &str) {}
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
        cache.set_observer(Some(Box::new(Decodes(Arc::clone(&log)))));
        let a = cache.load_mesh(&dir, "meshes/a.mesh.yaml").unwrap().clone();
        let b = cache.load_mesh(&dir, "meshes/b.mesh.yaml").unwrap().clone();
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
            &a_maps.base_color.as_ref().unwrap().image,
            &b_maps.base_color.as_ref().unwrap().image
        ));

        std::fs::write(dir.join("textures/t.png"), png_bytes(200)).unwrap();
        cache.reload(&dir, "textures/t.png").unwrap();
        assert!(cache.contains_mesh_map("textures/t.png"));
        for (rel, before) in [("meshes/a.mesh.yaml", &a), ("meshes/b.mesh.yaml", &b)] {
            assert_ne!(
                cache.load_mesh(&dir, rel).unwrap().hash,
                before.hash,
                "{rel}"
            );
        }
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
            "{:?}\n{:?}\n{:?}\n{:?}\n{atlases:?}\n{assemblies:?}\n{:?}",
            images(&cache.images),
            images(&cache.mesh_maps),
            meshes(&cache.meshes),
            meshes(&cache.voxels),
            cache.dependents,
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
            ludion_core::par::with_threads(threads, || cache.prefetch(&dir, &refs));
            cache
        };
        let one = prefetched(1);
        assert_eq!(fingerprint(&one), fingerprint(&prefetched(2)));
        assert_eq!(fingerprint(&one), fingerprint(&prefetched(8)));

        let mut lazy = AssetCache::new();
        for rel in &refs.meshes {
            assert_eq!(
                one.meshes[rel].hash,
                lazy.load_mesh(&dir, rel).unwrap().hash,
                "{rel}"
            );
        }
        for rel in &refs.voxels {
            assert_eq!(
                one.voxels[rel].hash,
                lazy.load_voxel(&dir, rel).unwrap().hash
            );
        }
        for rel in &refs.atlases {
            assert_eq!(
                one.atlases[rel].texture.hash,
                lazy.load_atlas(&dir, rel).unwrap().texture.hash
            );
        }
        for rel in &refs.assemblies {
            assert_eq!(
                one.assemblies[rel].parts[0].mesh.hash,
                lazy.load_assembly(&dir, rel).unwrap().parts[0].mesh.hash
            );
        }
        assert!(one.contains("textures/s.png"));
        assert!(one.contains_mesh_map("textures/u.png"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
