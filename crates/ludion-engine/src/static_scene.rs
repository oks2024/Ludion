//! The persistent static scene: the placed mesh instances of every `Static` entity, kept between
//! frames and invalidated exactly by the ECS write journal.
//!
//! Extract emits the instances as the **prefix** of the one mesh index space (`meshes`,
//! `mesh_owners`, and their assets first in `mesh_assets`), so every consumer of that space reads
//! the same indices whether the scene is kept or rebuilt. A [`StaticScene::default`] refreshed with
//! [`Feed::Full`] is the rebuild from scratch: it is what the host off switch runs every frame, and
//! what every one-shot extract runs once, so "kept" and "rebuilt" are the same code and the same
//! order (ascending `EntityId`; a `Mesh`, then its grid cells row-major).
//!
//! An entity is **held** when it carries `Static`, is eligible ([`static_ineligibility`], no skinned
//! asset), every `Parent` ancestor carries `Static`, and neither it nor an ancestor is *posed*
//! (`ParentJoint`, `SkinBone`, or a prefab root's `Animation`: poses the journal never sees).
//! Anything else takes the dynamic path; a *rejected* one is reported (`L1341`, `L1342`) each
//! frame, a *fallback* one (an asset that does not resolve, a non-finite placement) is silently
//! retried the next frame.

use std::sync::Arc;

use ludion_core::codes::code;
use ludion_core::collections::{StableMap, StableSet};
use ludion_core::diagnostics::Diagnostic;
use ludion_core::ids::EntityId;
use ludion_core::time::FrameIndex;
use ludion_ecs::{Parent, World};
use ludion_renderer::{MeshData, MeshInstance, MeshShape, StaticRange};

use crate::components::{
    Animation, GiDynamic, Mesh, MeshGrid, MeshSource, ParentJoint, ParticleEmitter, PointLight,
    SkinBone, SpotLight, Sprite, Static, Text,
};
use crate::extract::{
    MeshInherit, MeshKind, PlacedMesh, PlacedMeshSource, SceneAssets, mesh_instance,
    place_mesh_parts, placed_mesh, render_model,
};
use crate::hierarchy::{FrameVisibility, MAX_HIERARCHY_DEPTH, WorldAffines};
use crate::prefab::PrefabInstance;
use crate::static_check::{
    HotStatic, StaticIneligible, ineligible_diagnostic, parent_diagnostic, pose_source,
    static_ineligibility,
};
use crate::voxelizer::{StaticGi, resolve_static};

/// What invalidated the scene since the last refresh.
pub(crate) enum Feed {
    /// Everything: a fresh scene, a replaced world, or the switch turned back on.
    Full,
    /// The ids [`World::drain_watched`] returned, at sim frame `frame`.
    Written {
        ids: Vec<EntityId>,
        frame: FrameIndex,
    },
}

/// What a refresh reads, all borrowed from the frame.
pub(crate) struct Frame<'a> {
    pub(crate) world: &'a World,
    pub(crate) ids: &'a [EntityId],
    pub(crate) assets: &'a mut dyn SceneAssets,
    pub(crate) poses: &'a mut WorldAffines,
    pub(crate) visibility: &'a FrameVisibility,
}

/// One placed mesh of a held entity.
struct Part {
    /// The placement with its source stripped to a primitive; [`Part::placed`] restores it.
    placed: PlacedMesh<'static>,
    /// The mesh document the part draws, when it is not a primitive.
    asset: Option<(MeshKind, String, Arc<MeshData>)>,
    /// The instance, its shape a placeholder for an asset until the flat lists are built.
    instance: MeshInstance,
}

impl Part {
    /// The placement as extract's walk would have made it, for the GI voxelizer.
    fn placed(&self, index: usize) -> PlacedMesh<'_> {
        let source = match &self.asset {
            None => self.placed.source,
            Some((MeshKind::Voxel, path, _)) => PlacedMeshSource::Voxel(path),
            Some((MeshKind::Sculpt, path, _)) => PlacedMeshSource::Sculpt(path),
            Some((_, path, _)) => PlacedMeshSource::Asset(path),
        };
        PlacedMesh {
            index,
            source,
            ..self.placed.clone()
        }
    }
}

/// A held entity.
struct Entry {
    /// Its `Parent` ancestors, nearest first: the entities whose losing `Static` evicts it.
    chain: Vec<EntityId>,
    /// The distinct prefab roots of it and its chain: an `Animation` appearing on one poses it.
    prefab_roots: Vec<EntityId>,
    /// A hidden entity keeps its placements for the GI voxelizer and draws nothing.
    hidden: bool,
    parts: Vec<Part>,
}

/// The flattened outputs extract copies in as the prefix of the scene's mesh lists.
#[derive(Default)]
pub(crate) struct Flat {
    pub(crate) instances: Vec<MeshInstance>,
    pub(crate) owners: Vec<EntityId>,
    /// First-encounter order over `instances`, deduplicated by path.
    pub(crate) assets: Vec<Arc<MeshData>>,
    pub(crate) asset_paths: Vec<String>,
    /// The prefix descriptor for `RenderScene::statics`.
    pub(crate) range: StaticRange,
}

/// A mesh document in use, with the entities that draw it.
struct AssetUse {
    kind: MeshKind,
    path: String,
    data: Arc<MeshData>,
    users: Vec<EntityId>,
}

/// A prefab root and the held entities whose pose it may drive.
struct PrefabUse {
    root: EntityId,
    users: Vec<EntityId>,
}

#[derive(Default)]
pub(crate) struct StaticScene {
    epoch: Option<u64>,
    entries: StableMap<EntityId, Entry>,
    /// `Static` entities that are not held, retried every frame; `Some` is a reported rejection.
    loose: StableMap<EntityId, Option<(String, Diagnostic)>>,
    hot: HotStatic,
    flat: Flat,
    asset_uses: Vec<AssetUse>,
    prefab_uses: Vec<PrefabUse>,
    /// The GI voxelizer's resolved view of the held placements; built on demand, dropped on change.
    gi: Option<StaticGi>,
}

enum Class {
    Held(Entry),
    Loose(Option<(String, Diagnostic)>),
    NotStatic,
}

impl StaticScene {
    /// The feed for this frame: a replaced world (a different `epoch`) is watched afresh and
    /// rebuilt from scratch; otherwise the journal's drain.
    pub(crate) fn feed(&mut self, world: &mut World, epoch: u64, frame: FrameIndex) -> Feed {
        if self.epoch == Some(epoch) {
            return Feed::Written {
                ids: world.drain_watched::<Static>(),
                frame,
            };
        }
        self.epoch = Some(epoch);
        world.watch_with::<Static>();
        let _ = world.drain_watched::<Static>();
        Feed::Full
    }

    /// Forgets the world the scene was built for; the next [`feed`](StaticScene::feed) is full.
    pub(crate) fn invalidate(&mut self) {
        self.epoch = None;
    }

    /// Whether `id` is held: placed here, and left out of the dynamic walk.
    pub(crate) fn held(&self, id: EntityId) -> bool {
        self.entries.as_inner().contains_key(&id)
    }

    pub(crate) fn flat(&self) -> &Flat {
        &self.flat
    }

    /// Resolves the held placements for the GI voxelizer once per change to the static set,
    /// hidden ones included, in the order the flat lists use. `ids` is the walk's entity list, for
    /// the error pointers.
    ///
    /// # Errors
    /// A mesh document the voxelizer cannot load; nothing is cached, so the next frame retries.
    pub(crate) fn ensure_gi(
        &mut self,
        ids: &[EntityId],
        assets: &mut dyn SceneAssets,
    ) -> Result<(), Vec<Diagnostic>> {
        if self.gi.is_some() {
            return Ok(());
        }
        let mut placements = Vec::new();
        for (&id, entry) in &self.entries {
            let index = ids.binary_search(&id).unwrap_or(0);
            placements.extend(entry.parts.iter().map(|part| part.placed(index)));
        }
        self.gi = Some(resolve_static(&placements, assets)?);
        Ok(())
    }

    /// What [`ensure_gi`](StaticScene::ensure_gi) built; empty before it, or with no statics.
    pub(crate) fn gi(&self) -> Option<&StaticGi> {
        self.gi.as_ref()
    }

    /// Brings the scene up to date with `feed`, reporting rejections and hot statics to `degraded`.
    pub(crate) fn refresh(
        &mut self,
        frame: &mut Frame<'_>,
        feed: Feed,
        degraded: &mut Vec<(String, Diagnostic)>,
    ) {
        let world = frame.world;
        let mut todo: StableSet<EntityId> = StableSet::default();
        let mut written = Vec::new();
        let mut frame_at = FrameIndex::ZERO;
        let full = matches!(feed, Feed::Full);
        match feed {
            Feed::Full => {
                self.clear();
                if world.component_count::<Static>() == 0 {
                    return;
                }
                for &id in frame.ids {
                    if world.has::<Static>(id) {
                        todo.insert(id);
                    }
                }
            }
            Feed::Written { ids, frame: at } => {
                frame_at = at;
                for &id in &ids {
                    todo.insert(id);
                    if !world.is_alive(id) || !world.has::<Static>(id) {
                        // An ancestor that lost `Static` no longer expands to its descendants.
                        for (&child, entry) in &self.entries {
                            if entry.chain.contains(&id) {
                                todo.insert(child);
                            }
                        }
                    }
                }
                written = ids;
                for &id in self.loose.as_inner().keys() {
                    todo.insert(id);
                }
                self.stale_assets(frame.assets, &mut todo);
                self.stale_prefab_roots(world, &mut todo);
            }
        }
        let mut changed = full;
        let mut inherit = MeshInherit::default();
        for &id in todo.iter() {
            let had = self.entries.remove(&id).is_some();
            self.loose.remove(&id);
            match classify(frame, &mut inherit, id) {
                Class::Held(entry) => {
                    self.entries.insert(id, entry);
                    changed = true;
                }
                Class::Loose(report) => {
                    self.loose.insert(id, report);
                    changed |= had;
                }
                Class::NotStatic => changed |= had,
            }
            if !self.held(id) {
                self.hot.forget(id);
            }
        }
        let written: Vec<EntityId> = written.into_iter().filter(|&id| self.held(id)).collect();
        for (id, diagnostic) in self.hot.observe(frame_at, &written) {
            let key = format!("{}/static-hot", crate::diagnostics::offender_key(world, id));
            degraded.push((key, diagnostic));
        }
        for report in self.loose.as_inner().values().flatten() {
            degraded.push(report.clone());
        }
        if changed {
            self.flatten();
        }
    }

    fn clear(&mut self) {
        self.entries = StableMap::default();
        self.loose = StableMap::default();
        self.hot = HotStatic::default();
        self.flat = Flat::default();
        self.asset_uses = Vec::new();
        self.prefab_uses = Vec::new();
        self.gi = None;
    }

    /// Marks the users of every document whose decoded data is no longer the one placed with
    /// (`asset.reload`, a cook moving): the re-resolve is a cache lookup per distinct path.
    fn stale_assets(&self, assets: &mut dyn SceneAssets, todo: &mut StableSet<EntityId>) {
        for used in &self.asset_uses {
            let same = used
                .kind
                .load(assets, &used.path)
                .is_ok_and(|now| Arc::ptr_eq(&now, &used.data));
            if !same {
                for &id in &used.users {
                    todo.insert(id);
                }
            }
        }
    }

    /// Marks the users of every prefab root that now carries `Animation`: it poses their parts
    /// every frame, which no write to them journals. Loose entries are retried anyway.
    fn stale_prefab_roots(&self, world: &World, todo: &mut StableSet<EntityId>) {
        for used in &self.prefab_uses {
            if world.is_alive(used.root) && world.has::<Animation>(used.root) {
                for &id in &used.users {
                    todo.insert(id);
                }
            }
        }
    }

    /// Rebuilds the flat lists and the asset-use table from the entries.
    fn flatten(&mut self) {
        let mut flat = Flat::default();
        let mut index: StableMap<String, usize> = StableMap::default();
        let mut uses: StableMap<String, AssetUse> = StableMap::default();
        let mut roots: StableMap<EntityId, Vec<EntityId>> = StableMap::default();
        for (&id, entry) in &self.entries {
            for &root in &entry.prefab_roots {
                roots.entry(root).or_default().push(id);
            }
            for part in &entry.parts {
                let Some((kind, path, data)) = &part.asset else {
                    if !entry.hidden {
                        flat.instances.push(part.instance);
                        flat.owners.push(id);
                    }
                    continue;
                };
                let used = uses.entry(path.clone()).or_insert_with(|| AssetUse {
                    kind: *kind,
                    path: path.clone(),
                    data: data.clone(),
                    users: Vec::new(),
                });
                if used.users.last() != Some(&id) {
                    used.users.push(id);
                }
                if entry.hidden {
                    continue;
                }
                let slot = match index.as_inner().get(path) {
                    Some(&slot) => slot,
                    None => {
                        flat.assets.push(data.clone());
                        flat.asset_paths.push(path.clone());
                        index.insert(path.clone(), flat.assets.len() - 1);
                        flat.assets.len() - 1
                    }
                };
                let mut instance = part.instance;
                instance.shape = MeshShape::Asset(slot);
                flat.instances.push(instance);
                flat.owners.push(id);
            }
        }
        flat.range = StaticRange::of(&flat.instances, &flat.assets);
        self.flat = flat;
        self.gi = None;
        self.asset_uses = uses.into_iter().map(|(_, used)| used).collect();
        self.prefab_uses = roots
            .into_iter()
            .map(|(root, users)| PrefabUse { root, users })
            .collect();
    }
}

/// Whether `node` belongs to a prefab instance whose live root carries `Animation`.
fn prefab_root_animated(world: &World, node: EntityId) -> bool {
    world
        .get::<PrefabInstance>(node)
        .ok()
        .flatten()
        .is_some_and(|i| world.is_alive(i.root) && world.has::<Animation>(i.root))
}

fn prefab_root_of(world: &World, node: EntityId) -> Option<EntityId> {
    world
        .get::<PrefabInstance>(node)
        .ok()
        .flatten()
        .map(|i| i.root)
}

/// Decides what `id` is this frame and, when held, places it.
fn classify(frame: &mut Frame<'_>, inherit: &mut MeshInherit, id: EntityId) -> Class {
    let world = frame.world;
    if !world.is_alive(id) || !world.has::<Static>(id) {
        return Class::NotStatic;
    }
    let key = crate::diagnostics::offender_key(world, id);
    let mesh = world.get::<Mesh>(id).ok().flatten();
    let grid = world.get::<MeshGrid>(id).ok().flatten();
    let group = mesh.is_none() && grid.is_none();
    let mut shapes: Vec<&MeshSource> = mesh.iter().map(|m| &m.shape).collect();
    let mut blended = mesh.is_some_and(|m| m.blend.is_some());
    if let Some(grid) = grid {
        shapes.extend(grid.legend.values().map(|m| &m.shape));
        blended |= grid.legend.values().any(|m| m.blend.is_some());
    }
    let has = |name: &str| match name {
        "Mesh" => mesh.is_some(),
        "MeshGrid" => grid.is_some(),
        "SkinBone" => world.has::<SkinBone>(id),
        "ParentJoint" => world.has::<ParentJoint>(id),
        "Viewmodel" => world.has::<crate::components::Viewmodel>(id),
        "Sprite" => world.has::<Sprite>(id),
        "ParticleEmitter" => world.has::<ParticleEmitter>(id),
        "Text" => world.has::<Text>(id),
        "Decal" => world.has::<crate::components::Decal>(id),
        "PointLight" => world.has::<PointLight>(id),
        "SpotLight" => world.has::<SpotLight>(id),
        "DirectionalLight" => world.has::<crate::components::DirectionalLight>(id),
        "GroundMarker" => world.has::<crate::components::GroundMarker>(id),
        "Volley" => world.has::<crate::patterns::Volley>(id),
        "PatternPlayer" => world.has::<crate::patterns::PatternPlayer>(id),
        _ => false,
    };
    let subject = id.to_string();
    if let Some(reason) = static_ineligibility(&has, &shapes, blended) {
        return rejected(key, ineligible_diagnostic(&subject, &reason), "static");
    }
    if group {
        // A transform group draws nothing and is held by no one; it is still a `Static` ancestor.
        return Class::NotStatic;
    }
    if let Some(by) = pose_source(&has, prefab_root_animated(world, id)) {
        let reason = StaticIneligible::Posed { ancestor: None, by };
        return rejected(key, ineligible_diagnostic(&subject, &reason), "static");
    }

    let mut chain = Vec::new();
    let mut prefab_roots: Vec<EntityId> = prefab_root_of(world, id).into_iter().collect();
    let mut node = id;
    for _ in 0..=MAX_HIERARCHY_DEPTH {
        let Some(Parent(parent)) = crate::hierarchy::parent_of(world, node) else {
            break;
        };
        if !world.is_alive(parent) {
            break;
        }
        if !world.has::<Static>(parent) {
            let ancestor = parent.to_string();
            return rejected(key, parent_diagnostic(&subject, &ancestor), "static-parent");
        }
        let ancestor_has = |name: &str| match name {
            "ParentJoint" => world.has::<ParentJoint>(parent),
            "SkinBone" => world.has::<SkinBone>(parent),
            _ => false,
        };
        if let Some(by) = pose_source(&ancestor_has, prefab_root_animated(world, parent)) {
            let reason = StaticIneligible::Posed {
                ancestor: Some(parent.to_string()),
                by,
            };
            return rejected(key, ineligible_diagnostic(&subject, &reason), "static");
        }
        if let Some(root) = prefab_root_of(world, parent)
            && !prefab_roots.contains(&root)
        {
            prefab_roots.push(root);
        }
        chain.push(parent);
        node = parent;
    }

    let (model, normal_matrix) = render_model(world, frame.poses, id);
    let dynamic = world.has::<GiDynamic>(id);
    let (hidden, casts) = (frame.visibility.hidden(id), frame.visibility.casts(id));
    let mut sites = Vec::new();
    place_mesh_parts(
        mesh,
        grid,
        model,
        normal_matrix,
        |site, model, normal, m| {
            sites.push((site, model, normal, m));
        },
    );
    let mut parts = Vec::with_capacity(sites.len());
    let mut loaded: StableMap<String, Arc<MeshData>> = StableMap::default();
    for (site, model, normal, m) in sites {
        let placed = placed_mesh(id, 0, site, model, normal, m, dynamic);
        if !placed.model.is_finite() {
            return Class::Loose(None);
        }
        let kind = match placed.source {
            PlacedMeshSource::Primitive(_) => None,
            PlacedMeshSource::Asset(rel) => Some((MeshKind::Asset, rel)),
            PlacedMeshSource::Voxel(rel) => Some((MeshKind::Voxel, rel)),
            PlacedMeshSource::Sculpt(rel) => Some((MeshKind::Sculpt, rel)),
            _ => return Class::Loose(None),
        };
        let (asset, shape) = match kind {
            None => (
                None,
                match placed.source {
                    PlacedMeshSource::Primitive(shape) => shape,
                    _ => return Class::Loose(None),
                },
            ),
            Some((kind, rel)) => {
                let data = match loaded.as_inner().get(rel) {
                    Some(data) => data.clone(),
                    None => match kind.load(frame.assets, rel) {
                        Ok(data) => {
                            loaded.insert(rel.to_string(), data.clone());
                            data
                        }
                        Err(_) => return Class::Loose(None),
                    },
                };
                if !data.skin_joints.is_empty() {
                    return rejected(
                        key,
                        ineligible_diagnostic(&subject, &StaticIneligible::SkinnedAsset),
                        "static",
                    );
                }
                if data.blend {
                    return rejected(
                        key,
                        ineligible_diagnostic(&subject, &StaticIneligible::Blended),
                        "static",
                    );
                }
                (Some((kind, rel.to_string(), data)), MeshShape::Asset(0))
            }
        };
        let (fade, rim, medium) = inherit.resolve(world, &placed);
        let assets: &[Arc<MeshData>] = match &asset {
            Some((_, _, data)) => std::slice::from_ref(data),
            None => &[],
        };
        let rain_occluder = crate::rain::rain_occluder(world, id, casts);
        let instance = mesh_instance(
            &placed,
            shape,
            assets,
            fade,
            rim,
            medium,
            casts,
            rain_occluder,
            0,
        );
        parts.push(Part {
            placed: PlacedMesh {
                source: PlacedMeshSource::Primitive(MeshShape::Cube),
                ..placed
            },
            asset,
            instance,
        });
    }
    Class::Held(Entry {
        chain,
        prefab_roots,
        hidden,
        parts,
    })
}

fn rejected(key: String, diagnostic: Diagnostic, suffix: &str) -> Class {
    debug_assert!(
        diagnostic.code == code::STATIC_INELIGIBLE
            || diagnostic.code == code::STATIC_PARENT_NOT_STATIC
    );
    Class::Loose(Some((format!("{key}/{suffix}"), diagnostic)))
}
