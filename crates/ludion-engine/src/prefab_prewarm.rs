//! The prefab upload prewarm: a sim system names a prefab it is about to spawn, and the engine
//! decodes its assets and keeps its meshes and material maps GPU-resident ahead of the spawn.

use std::sync::Arc;

use ludion_assets::AssetRoot;
use ludion_core::diagnostics::Diagnostic;
use ludion_core::time::SimTime;
use ludion_ecs::{Registry, World};
use ludion_renderer::MeshData;
use ludion_scene::InstantiateOptions;

use crate::asset_cache::AssetCache;
use crate::components::Transform;
use crate::hierarchy::level_trs;
use crate::prefab::{PrefabHooks, Ready, library_ready};

/// What the engine last warmed, so an unchanged request is not rebuilt.
#[derive(Default)]
pub(crate) struct PrewarmState {
    pub(crate) generation: u64,
    pub(crate) meshes: Vec<Arc<MeshData>>,
}

/// One prewarm request: the read prefab and the registry that instantiates it.
pub(crate) struct PrewarmRequest {
    pub(crate) key: String,
    pub(crate) placement: Option<Transform>,
    pub(crate) ready: Arc<Ready>,
    pub(crate) registry: Arc<Registry>,
}

/// A sim system's request to prewarm a prefab it will spawn soon, so the later [`spawn_prefab`]
/// uploads nothing to the GPU.
///
/// Set it with [`prewarm_prefab`] on the room-clear tick, never the spawn tick. On the next rendered
/// frame the engine instantiates the prefab into a scratch world (no sim entity is created and no id
/// shifts), decodes its assets into the asset cache and uploads its meshes and material maps, keeping
/// them resident, never drawn, until the request is superseded, cleared or the world rebuilds. One
/// request at a time: a call naming a different prefab or placement replaces it, the same call again
/// is a no-op, and [`PrefabPrewarm::clear`] drops it.
///
/// A plain resource installed into every world, excluded from the `WorldHash` and from snapshots,
/// registering no component; a replay reproduces it because the system re-issues the request. A
/// headless sim never reads it. A prefab cannot predict a reflection probe's store key, so a room swap
/// with probes pairs it with [`ProbeRefreeze`](crate::ProbeRefreeze) on the swap tick.
///
/// [`spawn_prefab`]: crate::spawn_prefab
///
/// # Example
/// ```
/// use ludion_engine::{Engine, PrefabPrewarm, prewarm_prefab};
///
/// let root = std::env::temp_dir().join(format!("ludion-prewarm-doc-{}", std::process::id()));
/// std::fs::create_dir_all(&root).unwrap();
/// std::fs::write(
///     root.join("crate.prefab.yaml"),
///     "format: 1\nkind: prefab\nname: crate\nentities:\n  - name: lid\n    components: {}\n",
/// )
/// .unwrap();
///
/// let mut engine = Engine::builder().project_root(&root).build().unwrap();
/// prewarm_prefab(engine.world_mut(), "crate.prefab.yaml", None).unwrap();
/// let prewarm = engine.world_mut().resource_mut::<PrefabPrewarm>().unwrap();
/// assert!(prewarm.is_requested());
/// assert_eq!(prewarm.path(), Some("crate.prefab.yaml"));
/// prewarm.clear();
/// assert!(!prewarm.is_requested());
/// # std::fs::remove_dir_all(&root).ok();
/// ```
#[derive(Default)]
pub struct PrefabPrewarm {
    pub(crate) request: Option<PrewarmRequest>,
    pub(crate) generation: u64,
}

impl PrefabPrewarm {
    /// Drops the request; the next rendered frame frees what it kept resident.
    pub fn clear(&mut self) {
        if self.request.take().is_some() {
            self.generation += 1;
        }
    }

    /// Whether a request is held.
    pub fn is_requested(&self) -> bool {
        self.request.is_some()
    }

    /// The canonical key of the prefab the request names.
    pub fn path(&self) -> Option<&str> {
        self.request.as_ref().map(|request| request.key.as_str())
    }
}

/// Requests the prewarm of the prefab at `path` placed at `placement`, replacing any earlier request;
/// see [`PrefabPrewarm`]. The prefab is read and validated exactly as [`prepare_prefab`](crate::prepare_prefab) does.
///
/// # Errors
/// The diagnostics [`prepare_prefab`](crate::prepare_prefab) returns for `path`; a failed call leaves
/// the earlier request in place.
///
/// # Example
/// ```
/// use ludion_engine::{Engine, prewarm_prefab};
///
/// let mut engine = Engine::new();
/// // No project root, so no prefab library: the call reports why instead of requesting.
/// assert!(prewarm_prefab(engine.world_mut(), "room.prefab.yaml", None).is_err());
/// ```
pub fn prewarm_prefab(
    world: &mut World,
    path: &str,
    placement: Option<Transform>,
) -> Result<(), Vec<Diagnostic>> {
    let (key, ready) = library_ready(world, path)?;
    let registry = world
        .resource::<crate::prefab::PrefabLibrary>()
        .map(|library| Arc::clone(library.registry()))
        .expect("library_ready found the prefab library");
    if world.resource::<PrefabPrewarm>().is_none() {
        world.insert_resource(PrefabPrewarm::default());
    }
    let prewarm = world
        .resource_mut::<PrefabPrewarm>()
        .expect("the resource was just ensured");
    if prewarm
        .request
        .as_ref()
        .is_some_and(|held| held.key == key && held.placement == placement)
    {
        return Ok(());
    }
    prewarm.request = Some(PrewarmRequest {
        key,
        placement,
        ready,
        registry,
    });
    prewarm.generation += 1;
    Ok(())
}

/// The asset meshes of `request`'s prefab, decoded through `cache`: the prefab instantiated into a
/// scratch world and extracted. An extract error yields none, because the spawn will report it.
pub(crate) fn warm_meshes(
    request: &PrewarmRequest,
    root: AssetRoot<'_>,
    cache: &mut AssetCache,
    now: SimTime,
    particle_density: f32,
) -> Vec<Arc<MeshData>> {
    let mut scratch = World::new();
    let instantiated = ludion_scene::instantiate_expanded(
        &mut scratch,
        &request.ready.expanded,
        &request.registry,
        &InstantiateOptions {
            placement: request.placement.as_ref().map(level_trs),
            name_prefix: None,
            hooks: &PrefabHooks {
                source: &request.key,
            },
        },
    );
    if instantiated.is_err() {
        return Vec::new();
    }
    // The bench's per-phase totals stay the live frame's.
    let observer = cache.take_extract_observer();
    let refs = crate::extract::asset_refs(&scratch, root);
    cache.prefetch(root, &refs);
    let meshes = crate::extract::extract_through(
        &scratch,
        (64, 64),
        root,
        cache,
        now,
        particle_density,
        None,
        &mut Vec::new(),
    )
    .map(|scene| scene.mesh_assets)
    .unwrap_or_default();
    cache.set_extract_observer(observer);
    meshes
}
