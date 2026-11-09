//! Runtime prefab instantiation: [`spawn_prefab`] spawns a `kind: prefab` document from a system.
//!
//! The [`PrefabLibrary`] resource reads each prefab from the content root on first use and validates
//! it once with the spawn-time passes — the document's own checks, the singleton and hierarchy-body
//! rules, collider layers against the running scene's `physics:` block, atlas references and skin
//! bones. A prefab that fails is cached as its diagnostics, so a bad file is not re-read every spawn.
//! Like the [`PathLibrary`](crate::path_follow), the library is engine configuration installed at
//! every world rebuild, read afresh from committed content and never hashed; `asset.reload` refreshes
//! a prefab it holds. A spawn is the one [`ludion_scene::instantiate_into`] path, so it is
//! all-or-nothing and its ids come back in part order.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::sync::Arc;

use ludion_core::codes::code;
use ludion_core::diagnostics::Diagnostic;
use ludion_core::ids::EntityId;
use ludion_core::time::FrameIndex;
use ludion_ecs::{Phase, Registry, World, is_presentation};
use ludion_scene::{
    ExpandedScene, InstanceHooks, InstanceTransform, InstantiateOptions, ScenePhysics,
};

pub use ludion_components::prefab::PrefabInstance;

use crate::components::Transform;
use crate::hierarchy::level_trs;
use crate::instantiate::EngineHooks;
use crate::scene_check::{
    prefab_singleton_diagnostics, scene_atlas_diagnostics, scene_hierarchy_body_diagnostics,
    scene_layer_diagnostics, scene_motor_diagnostics, scene_sensor_diagnostics,
};
use crate::skin_bone::{BonePose, apply_bone_poses, spawn_poses};

/// A prefab that passed every spawn-time pass: its parts and the resolved skin-bone poses.
pub(crate) struct Ready {
    pub(crate) expanded: ExpandedScene,
    pub(crate) poses: Vec<BonePose>,
}

/// The prefabs [`spawn_prefab`] reads, loaded lazily from `root`. `physics` is the running scene's
/// block, so a prefab's collider layers are checked against the layers the scene declares.
#[derive(Clone)]
pub(crate) struct PrefabLibrary {
    root: PathBuf,
    registry: Arc<Registry>,
    physics: Option<ScenePhysics>,
    /// Keyed by the path a spawn names; a failed load is cached as its diagnostics.
    prefabs: BTreeMap<String, Result<Arc<Ready>, Vec<Diagnostic>>>,
    /// The frame whose tick runs next, kept current by the engine: the stamp of a text read now.
    pub(crate) frame: FrameIndex,
    /// The last source read per key, kept to re-log when a recording is armed after the read.
    texts: BTreeMap<String, String>,
    /// Every source read from disk (or accepted by a reload) since the engine last drained it, as
    /// `(stamp, key, source, reload)`, for the recording.
    log: Vec<(FrameIndex, String, String, bool)>,
    /// `Some` under replay: the recorded read texts, which stand in for the disk.
    source: Option<Arc<PrefabTextSource>>,
    /// The rebuild epoch of this library: the key into `source` under replay.
    epoch: u32,
    /// Per loaded key, every prefab its expansion resolved, transitively (failed loads included): the
    /// entries a reload of one of them must re-validate.
    deps: BTreeMap<String, BTreeSet<String>>,
    /// The held texts as they stood between accepted reloads: every read goes into the last one, a
    /// successful [`replace`](Self::replace) freezes it and starts the next. A parallel record to
    /// `texts`, so save can diff an instance against the text it was expanded from.
    snapshots: Vec<BTreeMap<String, String>>,
    /// Instance root → the snapshot its text came from, for instances spawned or applied here.
    bound: BTreeMap<EntityId, usize>,
    /// Instance root → the placement it was spawned or loaded at, so an apply builds at that
    /// placement instead of recovering it from the live poses. Not hashed or snapshotted: spawns and
    /// loads replay, so it rebuilds the same. Unknown after a restore.
    placements: BTreeMap<EntityId, InstanceTransform>,
    /// The size of `bound` plus `placements` after the last prune: the next one is due when they
    /// double it, so pruning stays amortized O(1) per spawn.
    pruned_len: usize,
    /// Roots despawned from a sim system, whose non-parented presentation parts the next frame
    /// boundary sweeps (a sim query cannot see them).
    sweep: Vec<EntityId>,
}

/// The prefab texts a scene load or district switch reads, before the world it builds exists: the
/// one resolver `ludion_scene::expand` is given there. A key resolves to one text per load; the
/// `(key, text)` pairs it read end up in [`reads`](LoadReads::reads) for the new library to seed.
pub(crate) struct LoadReads {
    root: PathBuf,
    source: Option<Arc<PrefabTextSource>>,
    pub(crate) epoch: u32,
    /// Every `(key, text)` read from the disk or the recording so far, in first-read order.
    pub(crate) reads: Vec<(String, String)>,
    /// The outcomes a restart carries over from the library it replaces: resolved before any read,
    /// and never part of `reads`, so the rebuilt world files nothing new in its epoch.
    pub(crate) held: HeldPrefabs,
}

/// What a library has already decided in its epoch, for a restart to inherit: every held text
/// (reloads included) and every refusal, with the dependencies of the refused keys.
#[derive(Default)]
pub(crate) struct HeldPrefabs {
    texts: BTreeMap<String, String>,
    failed: BTreeMap<String, Vec<Diagnostic>>,
    deps: BTreeMap<String, BTreeSet<String>>,
}

impl LoadReads {
    /// A resolver over `root`, or over `source` in `epoch` under replay.
    pub(crate) fn new(root: PathBuf, source: Option<Arc<PrefabTextSource>>, epoch: u32) -> Self {
        LoadReads {
            root,
            source,
            epoch,
            reads: Vec::new(),
            held: HeldPrefabs::default(),
        }
    }

    /// Resolves held outcomes before reading, for a restart.
    pub(crate) fn with_held(mut self, held: HeldPrefabs) -> Self {
        self.held = held;
        self
    }

    /// The text of the prefab `path`, as written in the document.
    ///
    /// # Errors
    /// `L1230` for an absolute or unreadable path, or under replay one the recording does not hold.
    pub(crate) fn resolve(&mut self, path: &str) -> Result<String, Diagnostic> {
        let key = prefab_key(path).map_err(|mut found| found.swap_remove(0))?;
        if let Some((_, held)) = self.reads.iter().find(|(read, _)| *read == key) {
            return Ok(held.clone());
        }
        if let Some(held) = self.held.texts.get(&key) {
            return Ok(held.clone());
        }
        if let Some(failed) = self.held.failed.get(&key) {
            return Err(failed[0].clone());
        }
        let text = fetch(&self.root, self.source.as_deref(), self.epoch, &key)?;
        self.reads.push((key, text.clone()));
        Ok(text)
    }
}

/// The text of the prefab `key`: from the disk, or under replay the one `epoch` read.
fn fetch(
    root: &std::path::Path,
    source: Option<&PrefabTextSource>,
    epoch: u32,
    key: &str,
) -> Result<String, Diagnostic> {
    match source {
        Some(source) => source
            .get(&(epoch, key.to_owned()))
            .cloned()
            .ok_or_else(|| {
                Diagnostic::new(
                    code::PREFAB_UNRESOLVED,
                    format!("prefab `{key}` is not recorded in this replay"),
                )
                .with_help("the recording holds the prefab texts its session read")
            }),
        None => read_prefab_text(root, key),
    }
}

/// The prefab texts a recording's reads carry, as `(rebuild epoch, path)` → source. A live library
/// reads a path at most once per epoch, so the first record wins.
pub(crate) type PrefabTextSource = BTreeMap<(u32, String), String>;

impl PrefabLibrary {
    /// An empty library resolving paths against `root`, or against `source` under replay.
    pub(crate) fn new(
        root: PathBuf,
        registry: Arc<Registry>,
        physics: Option<ScenePhysics>,
        frame: FrameIndex,
        source: Option<Arc<PrefabTextSource>>,
        epoch: u32,
    ) -> Self {
        PrefabLibrary {
            root,
            registry,
            physics,
            prefabs: BTreeMap::new(),
            frame,
            texts: BTreeMap::new(),
            log: Vec::new(),
            source,
            epoch,
            deps: BTreeMap::new(),
            snapshots: vec![BTreeMap::new()],
            bound: BTreeMap::new(),
            placements: BTreeMap::new(),
            pruned_len: 0,
            sweep: Vec::new(),
        }
    }

    /// Holds `text` for `key` in the texts and in the current snapshot.
    fn hold(&mut self, key: &str, text: &str) {
        self.texts.insert(key.to_owned(), text.to_owned());
        self.snapshots
            .last_mut()
            .expect("a library always has a snapshot")
            .insert(key.to_owned(), text.to_owned());
    }

    /// The snapshot the instance rooted at `root` was expanded from: the one it is bound to, else
    /// the earliest holding `key` (a scene-loaded instance, whose texts the load seeded). `None` when
    /// no snapshot holds `key`, as after a restore; the instance then resolves from disk.
    pub(crate) fn expanded_from(&self, root: EntityId, key: &str) -> Option<usize> {
        self.bound.get(&root).copied().or_else(|| {
            self.snapshots
                .iter()
                .position(|held| held.contains_key(key))
        })
    }

    /// The held texts of snapshot `index`.
    pub(crate) fn snapshot(&self, index: usize) -> &BTreeMap<String, String> {
        &self.snapshots[index]
    }

    /// Binds the instance rooted at `root` to the current snapshot.
    pub(crate) fn bind(&mut self, root: EntityId) {
        self.bound.insert(root, self.snapshots.len() - 1);
    }

    /// Records the placement the instance rooted at `root` was spawned or loaded at.
    pub(crate) fn place(&mut self, root: EntityId, placement: InstanceTransform) {
        self.placements.insert(root, placement);
    }

    /// Whether the instance records have doubled since the last prune (never below 64).
    fn prune_due(&self) -> bool {
        self.bound.len() + self.placements.len() >= self.pruned_len.max(32) * 2
    }

    /// The instance roots holding a record.
    fn recorded_roots(&self) -> BTreeSet<EntityId> {
        self.bound
            .keys()
            .chain(self.placements.keys())
            .copied()
            .collect()
    }

    /// Drops the records of every root `alive` rejects.
    pub(crate) fn prune(&mut self, alive: impl Fn(EntityId) -> bool) {
        self.bound.retain(|&root, _| alive(root));
        self.placements.retain(|&root, _| alive(root));
        self.pruned_len = self.bound.len() + self.placements.len();
    }

    /// Drops the records of the despawned instance rooted at `root`.
    pub(crate) fn forget(&mut self, root: EntityId) {
        self.bound.remove(&root);
        self.placements.remove(&root);
    }

    /// The placement of the instance rooted at `root`, `None` when unknown (after a restore).
    pub(crate) fn placement(&self, root: EntityId) -> Option<&InstanceTransform> {
        self.placements.get(&root)
    }

    /// The outcomes a restart inherits: the held texts, the refusals and the refused keys' dependencies.
    pub(crate) fn held_for_restart(&self) -> HeldPrefabs {
        let failed: BTreeMap<_, _> = self
            .prefabs
            .iter()
            .filter_map(|(key, loaded)| Some((key.clone(), loaded.as_ref().err()?.clone())))
            .collect();
        let deps = failed
            .keys()
            .filter_map(|key| Some((key.clone(), self.deps.get(key)?.clone())))
            .collect();
        HeldPrefabs {
            texts: self.texts.clone(),
            failed,
            deps,
        }
    }

    /// Takes over a replaced library's outcomes without logging them: the recording already holds
    /// them in this epoch. The texts go into the current snapshot, so a later instance diffs
    /// against the text it was expanded from.
    pub(crate) fn adopt(&mut self, held: HeldPrefabs) {
        for (key, text) in &held.texts {
            self.hold(key, text);
        }
        for (key, diagnostics) in held.failed {
            self.prefabs.insert(key, Err(diagnostics));
        }
        self.deps.extend(held.deps);
    }

    /// Holds the texts a scene load read and logs them at the library's frame, so the recording files
    /// them in this world's epoch and a later spawn composes the text the load saw.
    pub(crate) fn seed(&mut self, reads: Vec<(String, String)>) {
        for (key, text) in reads {
            self.hold(&key, &text);
            self.log.push((self.frame, key, text, false));
        }
    }

    /// Takes the texts read since the last drain.
    pub(crate) fn take_log(&mut self) -> Vec<(FrameIndex, String, String, bool)> {
        std::mem::take(&mut self.log)
    }

    /// Logs the held texts at frame zero, for a recording armed after the reads: a replay starts
    /// from the header scene at frame 0, so it must resolve them from there. Each key logs the text
    /// of the earliest snapshot holding it as a read, then every accepted reload that changed a
    /// text, in order, so the replay's library ends on the live one and the header scene's load sees
    /// the text it was built from. Reads not yet drained are superseded by it.
    pub(crate) fn relog_all(&mut self) {
        self.log.clear();
        let mut read = BTreeSet::new();
        for held in &self.snapshots {
            for (key, source) in held {
                if read.insert(key) {
                    self.log
                        .push((FrameIndex::ZERO, key.clone(), source.clone(), false));
                }
            }
        }
        for pair in self.snapshots.windows(2) {
            for (key, source) in &pair[1] {
                if pair[0].get(key).is_some_and(|before| before != source) {
                    self.log
                        .push((FrameIndex::ZERO, key.clone(), source.clone(), true));
                }
            }
        }
    }

    /// The prefab `rel`, loading and validating it on first use. A text already held (seeded by a
    /// scene load, or read as a nested prefab) is loaded as is: one text per key per epoch.
    pub(crate) fn ready(&mut self, rel: &str) -> Result<Arc<Ready>, Vec<Diagnostic>> {
        if !self.prefabs.contains_key(rel) {
            let source = match self.texts.get(rel) {
                Some(held) => Ok(held.clone()),
                None => self.read(rel).inspect(|source| {
                    self.hold(rel, source);
                    self.log
                        .push((self.frame, rel.to_owned(), source.clone(), false));
                }),
            };
            let loaded = match source {
                Ok(source) => {
                    let mut nested = Vec::new();
                    let mut deps = BTreeSet::new();
                    let loaded = self.load(rel, &source, &mut nested, &mut deps);
                    self.record_nested(nested);
                    self.deps.insert(rel.to_owned(), deps);
                    loaded
                }
                Err(diagnostics) => Err(diagnostics),
            };
            self.prefabs.insert(rel.to_owned(), loaded);
        }
        self.prefabs[rel].clone()
    }

    /// The content root prefab paths resolve against.
    pub(crate) fn root(&self) -> &std::path::Path {
        &self.root
    }

    /// The component registry prefabs are validated against.
    pub(crate) fn registry(&self) -> &Arc<Registry> {
        &self.registry
    }

    /// Runs every pass over `source` as the prefab `key` without caching or logging anything.
    ///
    /// # Errors
    /// The diagnostics a load of `key` would give.
    pub(crate) fn validate(&self, key: &str, source: &str) -> Result<(), Vec<Diagnostic>> {
        self.load(key, source, &mut Vec::new(), &mut BTreeSet::new())
            .map(drop)
    }

    /// The text of `rel`: from the disk, or under replay the one this epoch read.
    fn read(&self, rel: &str) -> Result<String, Vec<Diagnostic>> {
        fetch(&self.root, self.source.as_deref(), self.epoch, rel).map_err(|found| vec![found])
    }

    /// Logs the nested prefab texts a load read, like a top-level read.
    fn record_nested(&mut self, nested: Vec<(String, String)>) {
        for (key, text) in nested {
            self.hold(&key, &text);
            self.log.push((self.frame, key, text, false));
        }
    }

    /// Whether `key` has been loaded (successfully or not) or its text is held — read as a nested
    /// prefab, or by a scene load.
    pub(crate) fn holds(&self, key: &str) -> bool {
        self.prefabs.contains_key(key) || self.texts.contains_key(key)
    }

    /// Re-validates `source` as the prefab `key`, then every cached prefab that expanded `key`, from
    /// its held text; nothing changes when `key` or a dependent that loaded before now fails. Live,
    /// `source` is the reloaded file; under replay, the recorded reload text, so both run this same
    /// code.
    ///
    /// # Errors
    /// `key`'s own diagnostics, or a dependent's, prefixed ``reloading `key` breaks `dependent`: ``.
    pub(crate) fn replace(&mut self, key: &str, source: &str) -> Result<(), Vec<Diagnostic>> {
        let mut nested = Vec::new();
        let mut deps = BTreeSet::new();
        let loaded = self.load(key, source, &mut nested, &mut deps);
        self.record_nested(nested);
        let ready = loaded?;

        let previous = self.texts.insert(key.to_owned(), source.to_owned());
        let dependents: Vec<String> = self
            .deps
            .iter()
            .filter(|(dependent, resolved)| dependent.as_str() != key && resolved.contains(key))
            .map(|(dependent, _)| dependent.clone())
            .collect();
        let mut reloaded = Vec::new();
        for dependent in dependents {
            let Some(text) = self.texts.get(&dependent).cloned() else {
                continue;
            };
            let mut nested = Vec::new();
            let mut resolved = BTreeSet::new();
            let loaded = self.load(&dependent, &text, &mut nested, &mut resolved);
            self.record_nested(nested);
            if let Err(broken) = &loaded
                && matches!(self.prefabs.get(&dependent), Some(Ok(_)))
            {
                let mut broken = broken.clone();
                match previous {
                    Some(old) => self.texts.insert(key.to_owned(), old),
                    None => self.texts.remove(key),
                };
                for diagnostic in &mut broken {
                    diagnostic.message = format!(
                        "reloading `{key}` breaks `{dependent}`: {}",
                        diagnostic.message
                    );
                }
                return Err(broken);
            }
            reloaded.push((dependent, loaded, resolved));
        }

        for (dependent, loaded, resolved) in reloaded {
            self.prefabs.insert(dependent.clone(), loaded);
            self.deps.insert(dependent, resolved);
        }
        self.prefabs.insert(key.to_owned(), Ok(ready));
        self.deps.insert(key.to_owned(), deps);
        let frozen = self.snapshots.last().expect("a snapshot").clone();
        self.snapshots.push(frozen);
        self.snapshots
            .last_mut()
            .expect("a snapshot")
            .insert(key.to_owned(), source.to_owned());
        self.log
            .push((self.frame, key.to_owned(), source.to_owned(), true));
        Ok(())
    }

    /// Runs every pass over `source`, collecting every diagnostic of the later passes; each names
    /// `path` in its message and keeps its `/entities/{i}/…` pointer into the prefab.
    ///
    /// Nested instances expand first, so the passes run on the flat body; the `(key, text)` of each
    /// nested prefab read is pushed to `nested` for the caller to log, and every key the expansion
    /// resolved (held or read) to `deps`.
    fn load(
        &self,
        path: &str,
        source: &str,
        nested: &mut Vec<(String, String)>,
        deps: &mut BTreeSet<String>,
    ) -> Result<Arc<Ready>, Vec<Diagnostic>> {
        let name_the_file = |diagnostics: Vec<Diagnostic>| -> Vec<Diagnostic> {
            diagnostics
                .into_iter()
                .map(|mut diagnostic| {
                    diagnostic.message = format!("in `{path}`: {}", diagnostic.message);
                    diagnostic
                })
                .collect()
        };
        let expanded = self.expand_text(path, source, nested, deps)?;
        let mut body = expanded.scene.clone();
        let mut diagnostics = prefab_singleton_diagnostics(&body);
        diagnostics.extend(ludion_scene::mixed_root_diagnostic(&body.entities));
        diagnostics.extend(scene_hierarchy_body_diagnostics(&body));
        body.physics = self.physics.clone();
        diagnostics.extend(scene_layer_diagnostics(&body));
        diagnostics.extend(scene_sensor_diagnostics(&body));
        diagnostics.extend(scene_motor_diagnostics(&body));
        diagnostics.extend(scene_atlas_diagnostics(&self.root, &body));
        let poses = match spawn_poses(ludion_assets::AssetRoot::content(&self.root), &body) {
            Ok(poses) => poses,
            Err(errors) => {
                diagnostics.extend(errors);
                Vec::new()
            }
        };
        diagnostics.extend(crate::parent_joint::scene_parent_joint_diagnostics(
            &self.root, &body,
        ));
        // A lint in the body is reported by check-scene on the expanded scene; it must not refuse a spawn.
        diagnostics.retain(|d| d.severity.is_error());
        if !diagnostics.is_empty() {
            let located = diagnostics.into_iter().map(|d| expanded.locate(d));
            return Err(name_the_file(located.collect()));
        }
        Ok(Arc::new(Ready { expanded, poses }))
    }

    /// Parses `source` as the prefab `path` and expands its nested instances: the first half of
    /// [`Self::load`], with the same `nested` and `deps` bookkeeping and file-named diagnostics.
    fn expand_text(
        &self,
        path: &str,
        source: &str,
        nested: &mut Vec<(String, String)>,
        deps: &mut BTreeSet<String>,
    ) -> Result<ExpandedScene, Vec<Diagnostic>> {
        let name_the_file = |diagnostics: Vec<Diagnostic>| -> Vec<Diagnostic> {
            diagnostics
                .into_iter()
                .map(|mut diagnostic| {
                    diagnostic.message = format!("in `{path}`: {}", diagnostic.message);
                    diagnostic
                })
                .collect()
        };
        let mut doc = ludion_scene::check_prefab(source, &self.registry).map_err(name_the_file)?;
        doc.rebase(path, &self.registry);
        doc.expand(&self.registry, &mut |nested_path| {
            let key = prefab_key(nested_path).map_err(|mut found| found.swap_remove(0))?;
            deps.insert(key.clone());
            // One text per key per epoch: a held text wins over a fresh read, so a replay
            // (which resolves the first record) sees what the live run composed.
            if let Some(held) = self.texts.get(&key) {
                return Ok(held.clone());
            }
            let text = self.read(&key).map_err(|mut found| found.swap_remove(0))?;
            nested.push((key, text.clone()));
            Ok(text)
        })
        .map_err(name_the_file)
    }

    /// The root part paths of the prefab `key`, in part order: what [`Self::ready`] would yield,
    /// without caching or logging anything. Empty when the prefab cannot be read or expanded.
    pub(crate) fn root_parts_of(&self, key: &str) -> Vec<String> {
        match self.prefabs.get(key) {
            Some(Ok(ready)) => root_parts(&ready.expanded),
            Some(Err(_)) => Vec::new(),
            None => {
                let Ok(text) = self
                    .texts
                    .get(key)
                    .cloned()
                    .map_or_else(|| self.read(key), Ok)
                else {
                    return Vec::new();
                };
                self.expand_text(key, &text, &mut Vec::new(), &mut BTreeSet::new())
                    .map_or_else(|_| Vec::new(), |expanded| root_parts(&expanded))
            }
        }
    }
}

/// The part paths whose `Transform` and `Parent` are the instance's placement: the named parts of
/// the flattened prefab with no `Parent` component. A whole `prefab.revert` leaves them alone and a
/// pointer naming them is `L1238`; `prefab.overrides` reports them, so both read the rule here.
pub(crate) fn root_parts(expanded: &ExpandedScene) -> Vec<String> {
    expanded
        .scene
        .entities
        .iter()
        .filter(|entity| entity.components.iter().all(|(name, _)| name != "Parent"))
        .filter_map(|entity| entity.name.clone())
        .collect()
}

/// Names parts like a scene load does and writes each named part's [`PrefabInstance`].
pub(crate) struct PrefabHooks<'a> {
    pub(crate) source: &'a str,
}

impl InstanceHooks for PrefabHooks<'_> {
    fn name(&self, world: &mut World, entity: EntityId, name: &str) {
        EngineHooks.name(world, entity, name);
    }

    fn place(&self, world: &mut World, root: EntityId, placement: &ludion_core::math::Trs) {
        EngineHooks.place(world, root, placement);
    }

    fn provenance(&self, world: &mut World, entity: EntityId, part: &str, root: EntityId) {
        world
            .insert(
                entity,
                PrefabInstance {
                    source: self.source.to_owned(),
                    part: part.to_owned(),
                    root,
                },
            )
            .expect("the entity was just instantiated and is alive");
    }
}

/// The canonical key of a prefab path: forward slashes, no empty or `.` segments, so every spelling
/// of one file shares a cache entry, a recorded text and an `asset.reload` key. A path that is
/// absolute, prefixed or climbs out of the content root once its `..` segments fold is refused
/// ([`ludion_core::paths::content_refusal`]), whoever supplied it: a recorded `PrefabInstance.source`
/// is document data like any payload.
pub(crate) fn prefab_key(path: &str) -> Result<String, Vec<Diagnostic>> {
    if let Some(refusal) = ludion_core::paths::content_refusal(path) {
        return Err(vec![
            Diagnostic::new(
                code::PREFAB_UNRESOLVED,
                format!(
                    "cannot use prefab `{path}`: the path {}",
                    refusal.describe()
                ),
            )
            .with_help(
                "name the prefab relative to the content root, e.g. `prefabs/crate.prefab.yaml`",
            ),
        ]);
    }
    Ok(ludion_scene::normalize_prefab_path(path))
}

/// The text of the prefab `key` (a [`prefab_key`]) on disk under `root`. A key that did not come
/// through [`prefab_key`] (a recorded `PrefabInstance.source`) is confined here, before any read.
pub(crate) fn read_prefab_text(root: &std::path::Path, key: &str) -> Result<String, Diagnostic> {
    prefab_key(key).map_err(|mut found| found.swap_remove(0))?;
    std::fs::read_to_string(root.join(key)).map_err(|error| {
        Diagnostic::new(
            code::PREFAB_UNRESOLVED,
            format!("cannot read prefab `{key}`: {error}"),
        )
        .with_help("the path is relative to the content root")
    })
}

/// Spawns the prefab at `path` (relative to the content root) into `world` and returns its part ids
/// in the prefab's part order.
///
/// `placement` is baked onto every root of the group — a part with no `Parent` — as
/// `placement ∘ local`; `None` leaves the poses as authored. `name` prefixes every part's
/// [`Name`](crate::Name) as `name/part`; without it the parts carry their bare names. Each part
/// gets a [`PrefabInstance`], and entity references between parts point at this copy's ids.
///
/// `presentation` spawns every part from the presentation allocator, like an instance entry's
/// `presentation: true`; a part carrying a sim-only component is refused with `L1320`. Without it each
/// part keeps its prefab's kind. A sim system may spawn only sim parts and a presentation system only
/// presentation parts; the other way round is refused with `L0028`.
///
/// The prefab is read and validated once per path per world: later spawns reuse the result, and
/// `asset.reload` refreshes it. Live instances never change when the file does.
///
/// # Errors
/// `L1230` if `path` is absolute, the file cannot be read or `world` has no prefab library (it was
/// not built by an engine); the prefab's own diagnostics (`L1226`–`L1229`, `L1221`/`L1222`, `L1186`/`L1187`,
/// `L1151`/`L1152`/`L1209`, `L1194`–`L1197`, component errors) when it is invalid; `L1320` for a
/// `presentation` spawn of a sim-only component; `L0028` when the spawn crosses the phase boundary. Nothing is spawned on any error.
///
/// # Example
/// ```
/// use ludion_engine::{Engine, spawn_prefab};
///
/// let root = std::env::temp_dir().join(format!("ludion-spawn-prefab-doc-{}", std::process::id()));
/// std::fs::create_dir_all(&root).unwrap();
/// std::fs::write(
///     root.join("crate.prefab.yaml"),
///     "format: 1\nkind: prefab\nname: crate\nentities:\n  - name: lid\n    components: {}\n",
/// )
/// .unwrap();
///
/// let mut engine = Engine::builder().project_root(&root).build().unwrap();
/// let ids = spawn_prefab(engine.world_mut(), "crate.prefab.yaml", None, Some("crate-1"), false).unwrap();
/// assert_eq!(ids.len(), 1);
/// # std::fs::remove_dir_all(&root).ok();
/// ```
pub fn spawn_prefab(
    world: &mut World,
    path: &str,
    placement: Option<Transform>,
    name: Option<&str>,
    presentation: bool,
) -> Result<Vec<EntityId>, Vec<Diagnostic>> {
    let key = prefab_key(path)?;
    let Some(library) = world.resource_mut::<PrefabLibrary>() else {
        return Err(vec![
            Diagnostic::new(
                code::PREFAB_UNRESOLVED,
                format!("cannot spawn prefab `{path}`: this world has no prefab library"),
            )
            .with_help("spawn prefabs from a system of an engine-built game"),
        ]);
    };
    let ready = library.ready(&key)?;
    let registry = Arc::clone(&library.registry);
    let presentation_copy = if presentation {
        Some(ludion_scene::presentation_copy(
            &ready.expanded,
            &registry,
            &key,
        )?)
    } else {
        None
    };
    let ids = spawn_expanded(
        world,
        &key,
        presentation_copy.as_ref().unwrap_or(&ready.expanded),
        placement,
        name,
    )?;
    apply_bone_poses(world, &ids, &ready.poses);
    Ok(ids)
}

/// Instantiates `expanded` (the flattened prefab `key`) into `world`, then prunes dead instance
/// records and binds and places the new instance — the tail [`spawn_prefab`] and
/// `switch_skin_to_ragdoll` share. Bone poses are the caller's to apply.
pub(crate) fn spawn_expanded(
    world: &mut World,
    key: &str,
    expanded: &ExpandedScene,
    placement: Option<Transform>,
    name: Option<&str>,
) -> Result<Vec<EntityId>, Vec<Diagnostic>> {
    let Some(registry) = world
        .resource::<PrefabLibrary>()
        .map(|library| Arc::clone(&library.registry))
    else {
        return Err(vec![
            Diagnostic::new(
                code::PREFAB_UNRESOLVED,
                format!("cannot spawn prefab `{key}`: this world has no prefab library"),
            )
            .with_help("spawn prefabs from a system of an engine-built game"),
        ]);
    };
    let ids = ludion_scene::instantiate_expanded(
        world,
        expanded,
        &registry,
        &InstantiateOptions {
            placement: placement.as_ref().map(level_trs),
            name_prefix: name,
            hooks: &PrefabHooks { source: key },
        },
    )?;
    if world
        .resource::<PrefabLibrary>()
        .is_some_and(PrefabLibrary::prune_due)
    {
        let live: BTreeSet<EntityId> = world
            .resource::<PrefabLibrary>()
            .map(PrefabLibrary::recorded_roots)
            .unwrap_or_default()
            .into_iter()
            .filter(|&root| world.is_alive(root))
            .collect();
        if let Some(library) = world.resource_mut::<PrefabLibrary>() {
            library.prune(|root| live.contains(&root));
        }
    }
    if let (Some(&first), Some(library)) = (ids.first(), world.resource_mut::<PrefabLibrary>()) {
        library.bind(first);
        library.place(
            first,
            placement
                .as_ref()
                .map_or_else(InstanceTransform::default, |at| InstanceTransform {
                    translation: at.translation.map(f64::from),
                    rotation: at.rotation.map(f64::from),
                    scale: at.scale.map(f64::from),
                }),
        );
    }
    Ok(ids)
}

/// Despawns the prefab instance rooted at `root` (`ids[0]` of [`spawn_prefab`]) and returns its part
/// ids in ascending order.
///
/// Every entity whose [`PrefabInstance`] names `root` goes, in ascending id order, presentation
/// parts included; `Parent` descendants cascade as in [`World::despawn`]. From a sim system only sim
/// parts are visible: presentation parts parented under a sim part go with it, and any other
/// presentation part of the instance is swept at the next frame boundary. A sim system must declare
/// `.query::<(&PrefabInstance,)>()`.
///
/// # Errors
/// `L0002` if `root` is dead; `L1235` if it is not part of a prefab instance, or is a part other than
/// the instance's root; `L0028` for a sim root despawned from a presentation system. Nothing is
/// despawned on any error.
///
/// # Example
/// ```
/// use ludion_engine::{Engine, despawn_instance, spawn_prefab};
///
/// let root = std::env::temp_dir().join(format!("ludion-despawn-instance-doc-{}", std::process::id()));
/// std::fs::create_dir_all(&root).unwrap();
/// std::fs::write(
///     root.join("crate.prefab.yaml"),
///     "format: 1\nkind: prefab\nname: crate\nentities:\n  - name: lid\n    components: {}\n",
/// )
/// .unwrap();
///
/// let mut engine = Engine::builder().project_root(&root).build().unwrap();
/// let ids = spawn_prefab(engine.world_mut(), "crate.prefab.yaml", None, Some("crate-1"), false).unwrap();
/// let gone = despawn_instance(engine.world_mut(), ids[0]).unwrap();
/// assert_eq!(gone, ids);
/// assert!(!engine.world_mut().is_alive(ids[0]));
/// # std::fs::remove_dir_all(&root).ok();
/// ```
pub fn despawn_instance(world: &mut World, root: EntityId) -> Result<Vec<EntityId>, Diagnostic> {
    match world.get::<PrefabInstance>(root)? {
        None => {
            return Err(Diagnostic::new(
                code::PREFAB_NOT_INSTANCE,
                format!("entity {root} is not part of a prefab instance"),
            )
            .with_help("pass the instance's root — `ids[0]` of `spawn_prefab`"));
        }
        Some(instance) if instance.root != root => {
            return Err(Diagnostic::new(
                code::PREFAB_NOT_INSTANCE,
                format!(
                    "entity {root} is a part of the instance rooted at {}, not its root",
                    instance.root
                ),
            )
            .with_help("pass the instance's root — `ids[0]` of `spawn_prefab`"));
        }
        Some(_) => {}
    }
    if world.phase() == Phase::Presentation && !is_presentation(root) {
        return Err(world.despawn(root).unwrap_err());
    }
    let mut parts: Vec<EntityId> = world
        .query_ids::<(&PrefabInstance,)>()
        .filter(|(_, (instance,))| instance.root == root)
        .map(|(id, _)| id)
        .collect();
    parts.sort_unstable();
    for error in world.despawn_many(&parts) {
        // A cascade from an earlier part already freed this one.
        debug_assert!(error.code == code::DEAD_ENTITY, "{error:?}");
    }
    let in_sim = world.phase() == Phase::Sim;
    if let Some(library) = world.resource_mut::<PrefabLibrary>() {
        library.forget(root);
        if in_sim {
            library.sweep.push(root);
        }
    }
    Ok(parts)
}

/// Despawns the presentation parts of instances a sim system despawned, which its queries could not
/// see. Runs at the frame boundary, in Host phase.
pub(crate) fn sweep_presentation_parts(world: &mut World) {
    let Some(library) = world.resource_mut::<PrefabLibrary>() else {
        return;
    };
    if library.sweep.is_empty() {
        return;
    }
    let roots: BTreeSet<EntityId> = std::mem::take(&mut library.sweep).into_iter().collect();
    let mut parts: Vec<EntityId> = world
        .query_ids::<(&PrefabInstance,)>()
        .filter(|(id, (instance,))| is_presentation(*id) && roots.contains(&instance.root))
        .map(|(id, _)| id)
        .collect();
    parts.sort_unstable();
    let _ = world.despawn_many(&parts);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::components::builtin_registry;

    fn root(tag: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("ludion-prefab-unit-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("a temp content root");
        dir
    }

    fn world(root: &std::path::Path) -> World {
        let mut world = World::new();
        world.insert_resource(PrefabLibrary::new(
            root.to_path_buf(),
            Arc::new(builtin_registry()),
            None,
            FrameIndex::ZERO,
            None,
            0,
        ));
        world
    }

    #[test]
    fn prefab_keys_refuse_anything_outside_the_content_root() {
        for path in [
            "../x.prefab.yaml",
            "C:x.prefab.yaml",
            "a/../../x.prefab.yaml",
            "/abs/x.prefab.yaml",
            "\\\\h\\s\\x.prefab.yaml",
        ] {
            let errors = prefab_key(path).unwrap_err();
            assert_eq!(errors[0].code, code::PREFAB_UNRESOLVED, "{path}");
            assert!(errors[0].message.contains(path), "{}", errors[0].message);
        }
        assert_eq!(prefab_key("a/../b.prefab.yaml").unwrap(), "b.prefab.yaml");
        assert_eq!(prefab_key("./a//b.prefab.yaml").unwrap(), "a/b.prefab.yaml");
    }

    fn prefab(parts: &str) -> String {
        format!("format: 1\nkind: prefab\nname: unit\nentities:\n{parts}")
    }

    /// Spawns `parts` and asserts the refusal carries `expected` with nothing spawned.
    fn refused(tag: &str, parts: &str, expected: ludion_core::diagnostics::DiagnosticCode) {
        refused_in(&root(tag), parts, expected);
    }

    /// [`refused`] over an existing content `root`, removed afterwards.
    fn refused_in(
        root: &std::path::Path,
        parts: &str,
        expected: ludion_core::diagnostics::DiagnosticCode,
    ) {
        std::fs::write(root.join("p.prefab.yaml"), prefab(parts)).unwrap();
        let mut world = world(root);
        world.spawn();
        let before = (
            world.entity_count(),
            world.allocator_state(),
            world.presentation_allocator_state(),
        );
        let errors = spawn_prefab(&mut world, "p.prefab.yaml", None, None, false).unwrap_err();
        assert!(
            errors.iter().any(|d| d.code == expected),
            "{expected}: {errors:?}"
        );
        assert!(errors[0].message.starts_with("in `p.prefab.yaml`: "));
        assert_eq!(
            (
                world.entity_count(),
                world.allocator_state(),
                world.presentation_allocator_state()
            ),
            before
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn instance_records_stay_bounded_and_a_live_instance_keeps_its_placement() {
        let root = root("prune");
        std::fs::write(
            root.join("p.prefab.yaml"),
            prefab("  - name: a\n    components: {}\n"),
        )
        .unwrap();
        let mut world = world(&root);
        let keeper = spawn_prefab(&mut world, "p.prefab.yaml", None, None, false).unwrap()[0];
        for _ in 0..1000 {
            let ids = spawn_prefab(&mut world, "p.prefab.yaml", None, None, false).unwrap();
            world.despawn(ids[0]).unwrap();
        }
        let library = world.resource::<PrefabLibrary>().unwrap();
        assert!(library.bound.len() < 200, "{}", library.bound.len());
        assert!(library.placements.len() < 200);
        assert!(library.placement(keeper).is_some());
        assert!(library.bound.contains_key(&keeper));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_singleton_part_is_refused_and_spawns_nothing() {
        refused(
            "singleton",
            "  - name: eye\n    components:\n      Camera: {}\n",
            code::PREFAB_SINGLETON,
        );
    }

    #[test]
    fn a_rigid_child_is_refused_and_spawns_nothing() {
        refused(
            "rigid",
            "  - name: base\n    components: {}\n  - name: ball\n    components:\n      Parent:\n        index: 0\n        generation: 0\n      Collider:\n        shape:\n          box:\n            half_extents:
              - 0.5
              - 0.5
              - 0.5\n        body:\n          rigid: {}\n",
            code::HIERARCHY_RIGID_BODY,
        );
    }

    #[test]
    fn a_socket_on_a_part_with_no_skinned_mesh_is_refused_and_spawns_nothing() {
        refused(
            "socket-bare",
            "  - name: base
    components: {}
  - name: prop
    components:
      Parent:
        index: 0
        generation: 0
      ParentJoint:
        joint: child
",
            code::PARENT_JOINT_NOT_SKINNED,
        );
    }

    #[test]
    fn a_socket_on_an_unknown_joint_is_refused_and_spawns_nothing() {
        refused_in(
            &crate::parent_joint::tests::rig_dir("prefab-socket"),
            "  - name: base
    components:
      Mesh:
        shape:
          asset: meshes/rig.mesh.yaml
  - name: prop
    components:
      Parent:
        index: 0
        generation: 0
      ParentJoint:
        joint: chlid
",
            code::PARENT_JOINT_UNKNOWN_JOINT,
        );
    }

    #[test]
    fn an_undeclared_layer_is_refused_and_spawns_nothing() {
        refused(
            "layer",
            "  - name: wall\n    components:\n      Collider:\n        shape:\n          box:\n            half_extents:
              - 0.5
              - 0.5
              - 0.5\n        body: static\n        layer: ghost\n",
            code::COLLIDER_LAYER_UNKNOWN,
        );
    }

    #[test]
    fn a_failing_prefab_is_cached() {
        let root = root("cached");
        let file = root.join("p.prefab.yaml");
        std::fs::write(
            &file,
            prefab("  - name: eye\n    components:\n      Camera: {}\n"),
        )
        .unwrap();
        let mut world = world(&root);
        let first = spawn_prefab(&mut world, "p.prefab.yaml", None, None, false).unwrap_err();
        std::fs::remove_file(&file).unwrap();
        let second = spawn_prefab(&mut world, "p.prefab.yaml", None, None, false).unwrap_err();
        assert_eq!(first, second);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_dotted_or_backslashed_path_names_the_canonical_source() {
        let root = root("canonical");
        std::fs::write(
            root.join("p.prefab.yaml"),
            prefab("  - name: a\n    components: {}\n"),
        )
        .unwrap();
        let mut world = world(&root);
        for path in ["./p.prefab.yaml", ".\\p.prefab.yaml", "p.prefab.yaml"] {
            let ids = spawn_prefab(&mut world, path, None, None, false).unwrap();
            let instance = world.get::<PrefabInstance>(ids[0]).unwrap().unwrap();
            assert_eq!(instance.source, "p.prefab.yaml", "{path}");
        }
        let library = world.resource::<PrefabLibrary>().unwrap();
        assert_eq!(
            library.prefabs.len(),
            1,
            "one cache entry for every spelling"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn an_absolute_path_is_refused_and_spawns_nothing() {
        let root = root("absolute");
        std::fs::write(
            root.join("p.prefab.yaml"),
            prefab("  - name: a\n    components: {}\n"),
        )
        .unwrap();
        let mut world = world(&root);
        let absolute = root.join("p.prefab.yaml");
        let errors =
            spawn_prefab(&mut world, absolute.to_str().unwrap(), None, None, false).unwrap_err();
        assert_eq!(errors[0].code, code::PREFAB_UNRESOLVED);
        assert_eq!(world.entity_count(), 0);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn snapshots_bind_spawns_and_find_seeded_keys() {
        let root = root("snapshots");
        let file = root.join("p.prefab.yaml");
        std::fs::write(&file, prefab("  - name: a\n    components: {}\n")).unwrap();
        let mut world = world(&root);
        let first = spawn_prefab(&mut world, "p.prefab.yaml", None, None, false).unwrap()[0];
        let library = world.resource_mut::<PrefabLibrary>().unwrap();
        let edited = prefab("  - name: a\n    components: {}\n  - name: b\n    components: {}\n");
        library.replace("p.prefab.yaml", &edited).unwrap();
        let second = spawn_prefab(&mut world, "p.prefab.yaml", None, None, false).unwrap()[0];
        let library = world.resource_mut::<PrefabLibrary>().unwrap();
        assert_eq!(library.expanded_from(first, "p.prefab.yaml"), Some(0));
        assert_eq!(library.expanded_from(second, "p.prefab.yaml"), Some(1));
        assert!(
            library.snapshot(0)["p.prefab.yaml"].len() < library.snapshot(1)["p.prefab.yaml"].len()
        );

        library.seed(vec![("seeded.prefab.yaml".to_owned(), "x".to_owned())]);
        let stranger = EntityId::new(99, 0);
        assert_eq!(
            library.expanded_from(stranger, "seeded.prefab.yaml"),
            Some(1)
        );
        assert_eq!(library.expanded_from(stranger, "nope.prefab.yaml"), None);

        let count = library.snapshots.len();
        let broken = prefab("  - name: eye\n    components:\n      Camera: {}\n");
        assert!(library.replace("p.prefab.yaml", &broken).is_err());
        assert_eq!(library.snapshots.len(), count);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_missing_file_and_a_missing_library_are_l1230() {
        let root = root("missing");
        let mut with_library = world(&root);
        let errors =
            spawn_prefab(&mut with_library, "nope.prefab.yaml", None, None, false).unwrap_err();
        assert_eq!(errors[0].code, code::PREFAB_UNRESOLVED);
        assert!(errors[0].message.contains("nope.prefab.yaml"));

        let errors =
            spawn_prefab(&mut World::new(), "nope.prefab.yaml", None, None, false).unwrap_err();
        assert_eq!(errors[0].code, code::PREFAB_UNRESOLVED);
        let _ = std::fs::remove_dir_all(&root);
    }

    const THREE: &str = "  - name: a\n    components: {}\n  - name: b\n    components: {}\n  - name: c\n    components: {}\n";

    fn spawn_three(world: &mut World) -> Vec<EntityId> {
        spawn_prefab(world, "p.prefab.yaml", None, None, false).unwrap()
    }

    #[test]
    fn despawning_an_instance_removes_exactly_its_parts_in_ascending_order() {
        let root = root("despawn");
        std::fs::write(root.join("p.prefab.yaml"), prefab(THREE)).unwrap();
        let mut world = world(&root);
        let first = spawn_three(&mut world);
        let unrelated = world.spawn();
        let second = spawn_three(&mut world);
        let mut expected = first.clone();
        expected.sort_unstable();
        assert_eq!(despawn_instance(&mut world, first[0]).unwrap(), expected);
        assert!(first.iter().all(|&id| !world.is_alive(id)));
        assert!(second.iter().all(|&id| world.is_alive(id)));
        assert!(world.is_alive(unrelated));
        let library = world.resource::<PrefabLibrary>().unwrap();
        assert!(!library.bound.contains_key(&first[0]));
        assert!(library.bound.contains_key(&second[0]));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn spawn_despawn_spawn_is_deterministic_and_reuses_the_slots() {
        let root = root("despawn-det");
        std::fs::write(root.join("p.prefab.yaml"), prefab(THREE)).unwrap();
        let run = || {
            let mut world = world(&root);
            let first = spawn_three(&mut world);
            despawn_instance(&mut world, first[0]).unwrap();
            let again = spawn_three(&mut world);
            (first, again)
        };
        let (first, again) = run();
        assert_eq!(run(), (first.clone(), again.clone()));
        let slots = |ids: &[EntityId]| ids.iter().map(|id| id.index).collect::<BTreeSet<_>>();
        assert_eq!(slots(&first), slots(&again));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn despawning_a_non_root_refuses_and_leaves_everything() {
        let root = root("despawn-errors");
        std::fs::write(root.join("p.prefab.yaml"), prefab(THREE)).unwrap();
        let mut world = world(&root);
        let ids = spawn_three(&mut world);
        let plain = world.spawn();
        let dead = world.spawn();
        world.despawn(dead).unwrap();
        let count = world.entity_count();

        assert_eq!(
            despawn_instance(&mut world, dead).unwrap_err().code,
            code::DEAD_ENTITY
        );
        assert_eq!(
            despawn_instance(&mut world, plain).unwrap_err().code,
            code::PREFAB_NOT_INSTANCE
        );
        let part = despawn_instance(&mut world, ids[1]).unwrap_err();
        assert_eq!(part.code, code::PREFAB_NOT_INSTANCE);
        assert!(
            part.message.contains(&ids[0].to_string()),
            "{}",
            part.message
        );
        assert_eq!(world.entity_count(), count);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_part_parented_under_another_part_goes_without_an_error() {
        let root = root("despawn-parent");
        let parts = "  - name: a\n    components: {}\n  - name: b\n    components:\n      Parent:\n        index: 0\n        generation: 0\n";
        std::fs::write(root.join("p.prefab.yaml"), prefab(parts)).unwrap();
        let mut world = world(&root);
        let ids = spawn_prefab(&mut world, "p.prefab.yaml", None, None, false).unwrap();
        let gone = despawn_instance(&mut world, ids[0]).unwrap();
        assert_eq!(gone.len(), 2);
        assert!(ids.iter().all(|&id| !world.is_alive(id)));
        let _ = std::fs::remove_dir_all(&root);
    }
}
