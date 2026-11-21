//! Save collapse: the live prefab instances of a world written back as instance entries.
//!
//! [`collapse`] turns the flat scene of [`flatten_world`] into the document a reload reproduces the
//! world from. An instance is the live entities sharing one [`PrefabInstance`] `root`; when every
//! part is named `<instance>/<part>` it becomes one entry, otherwise it stays plain entities.
//!
//! Every "expected" value comes from a scratch build — a candidate document expanded and
//! instantiated by the load pipeline itself — so what the entry records is exactly what a reload would
//! produce. The live part's differences from the expected one become `overrides`, the prefab parts
//! with no live part become `removed`, and a moved instance becomes a new `transform`. Three position
//! spaces meet here: the live order (`flatten_world` on the live world), the flat order of an expanded
//! candidate (`E`; the one two components are compared in) and the document order the entries are
//! written in.

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};

use ludion_core::codes::code;
use ludion_core::diagnostics::{Diagnostic, pointer_token};
use ludion_core::ids::EntityId;
use ludion_core::math::{Trs, quat_to_euler_xyz_degrees};
use ludion_ecs::{Registry, World};
use ludion_scene::{
    ExpandedScene, InstanceEntry, InstanceTransform, Scene, SceneEntity, identical,
    normalize_prefab_path, partial_diff,
};
use serde_json::Value;

use crate::components::{Name, Transform};
use crate::engine::{
    flatten_ids, flatten_world, hierarchy_fault, instantiate_expanded_world, ref_target,
    remap_entity_refs,
};
use crate::hierarchy::level_trs;
use crate::prefab::{PrefabInstance, PrefabLibrary, prefab_key, read_prefab_text};
use crate::scene_check::expand_scene_with;

/// The registered name of [`PrefabInstance`]; `flatten_world` leaves it out and this module reads it
/// from the world.
pub(crate) const PROVENANCE: &str = "PrefabInstance";

const TRANSFORM: &str = "Transform";
const PARENT: &str = "Parent";
const SKIN_BONE: &str = "SkinBone";
const HELP: &str = "author it inside the prefab, or unpack the instance";

type Components = Vec<(String, Value)>;

/// The world being saved.
#[derive(Clone)]
pub(crate) struct Live<'a> {
    pub world: &'a World,
    pub registry: &'a Registry,
    pub project_root: &'a std::path::Path,
    /// The parsed source document, whose entries of the same name are the priors.
    pub authored: Option<&'a Scene>,
    /// The prefab texts instances were expanded from; without it every instance resolves from disk.
    pub library: Option<&'a PrefabLibrary>,
    /// Whether a group whose top-level roots do not reproduce falls back to identity placement with
    /// each root's live pose as an override; an apply writes root poses through Euler angles instead
    /// and leaves its top-level roots out of the final check.
    pub exact_fallback: bool,
    /// The placement the instance was spawned or loaded at, when known: a group with no authored
    /// prior is built there and never treated as moved, so its parts that do not reproduce are
    /// per-part pose edits. `None` recovers the placement from the anchor's live pose.
    pub placement: Option<InstanceTransform>,
    /// Entities present only as reference targets: never grouped, no provenance block.
    pub stubs: Option<&'a BTreeSet<EntityId>>,
    /// Each entity's position in the whole world — what a save names an unnamed entity by — when the
    /// collapse is scoped; `None` when the flat order is the world's.
    pub positions: Option<&'a [usize]>,
}

/// What the save would write for one instance.
pub(crate) struct InstanceDiff {
    pub root: EntityId,
    pub name: String,
    pub entry: InstanceEntry,
    /// The saved `transform` differs, bit for bit, from the placement the instance was built at.
    pub placement_differs: bool,
    /// Per part path, the components the part holds that its prefab part lacks.
    pub gained: Gained,
}

/// Per part path, the names of the components a live part holds that its prefab part does not.
pub(crate) type Gained = BTreeMap<String, Vec<String>>;

/// What [`collapse_diffs`] yields: the document entities, what each instance's entry says, and the
/// refusals found among the entities outside every instance, by the root they are about.
type Collapsed = (
    Vec<SceneEntity>,
    Vec<InstanceDiff>,
    BTreeMap<EntityId, Diagnostic>,
);

/// What [`Collapser::run`] yields: the document entities, per group whether its placement differs, and
/// its gained components.
type Run = (Vec<SceneEntity>, Vec<bool>, Vec<Gained>);

/// What [`Collapser::others`] yields: per group its component differences by part path, and its gains.
type Others = (Vec<BTreeMap<String, Components>>, Vec<Gained>);

/// One instance of a `prefab.overrides` read: its root, prefab and what the save would write for it.
pub(crate) struct InstanceOverrides {
    pub root: EntityId,
    pub prefab: String,
    pub result: Result<Option<InstanceDiff>, Diagnostic>,
    /// The prefab's root part paths; empty when `result` is refused or the prefab cannot be read.
    pub root_parts: Vec<String>,
}

/// A refused collapse and, when one instance caused it, that instance's root.
pub(crate) struct Refusal {
    pub root: Option<EntityId>,
    pub diagnostic: Diagnostic,
}

impl From<Diagnostic> for Refusal {
    fn from(diagnostic: Diagnostic) -> Self {
        Refusal {
            root: None,
            diagnostic,
        }
    }
}

fn refuse(message: impl Into<String>) -> Diagnostic {
    Diagnostic::new(code::PREFAB_INSTANCE_SAVE, message).with_help(HELP)
}

fn refuse_at(root: EntityId, message: impl Into<String>) -> Refusal {
    Refusal {
        root: Some(root),
        diagnostic: refuse(message),
    }
}

/// A named instance: its parts (live position, path) and the prefab they come from.
struct Group {
    name: String,
    source: String,
    root: EntityId,
    parts: Vec<(usize, String)>,
}

/// One entry of the document being written.
#[derive(Clone, Copy)]
enum Slot {
    Plain(usize),
    Group(usize),
}

/// What an instance entry says about its placement, apart from `overrides` and `removed`.
#[derive(Clone)]
struct Setting {
    transform: InstanceTransform,
    /// `Transform` overrides by part path, as authored or derived.
    poses: BTreeMap<String, Value>,
}

/// An authored entry of the same name and prefab as a live instance.
struct Prior {
    prefab: String,
    setting: Setting,
}

/// A scratch build: the expanded candidate and each flat entity's components, entity references in
/// flat-candidate space.
struct Built {
    expanded: ExpandedScene,
    comps: Vec<Components>,
}

/// How the three position spaces line up, from one expansion.
struct Layout {
    /// Flat-candidate position of each live position.
    e_of_live: Vec<usize>,
    /// The live position of each flat-candidate position.
    live_of_e: Vec<usize>,
    /// Document position of a flat-candidate entity, `None` for a part that is not its instance's first.
    doc_of_e: Vec<Option<usize>>,
}

/// Names the instance of a part: `name` is `<instance>/<part>` with a non-empty, slash-free instance.
fn instance_name<'n>(name: Option<&'n str>, part: &str) -> Option<&'n str> {
    let instance = name?.strip_suffix(part)?.strip_suffix('/')?;
    (!instance.is_empty() && !instance.contains('/')).then_some(instance)
}

/// Rewrites each entity-reference `index` of `value` through `map`; `Err` carries a target `map`
/// has no answer for.
fn remap_value(
    registry: &Registry,
    name: &str,
    value: &mut Value,
    map: &dyn Fn(usize) -> Option<usize>,
) -> Result<(), usize> {
    let Some(info) = registry.get(name) else {
        return Ok(());
    };
    let sites = info.entity_ref_sites(value);
    for field in &sites {
        let reference = if field.suffix.is_empty() {
            Some(&mut *value)
        } else {
            value.pointer_mut(&field.suffix)
        };
        let Some(object) = reference.and_then(Value::as_object_mut) else {
            continue;
        };
        let Some(index) = object.get("index").and_then(Value::as_u64) else {
            continue;
        };
        let mapped = map(index as usize).ok_or(index as usize)?;
        object.insert("index".to_owned(), Value::from(mapped as u64));
    }
    Ok(())
}

fn remap_components(
    registry: &Registry,
    components: &mut Components,
    map: &dyn Fn(usize) -> Option<usize>,
) -> Result<(), usize> {
    for (name, value) in components {
        remap_value(registry, name, value, map)?;
    }
    Ok(())
}

fn find<'c>(components: &'c Components, name: &str) -> Option<&'c Value> {
    components
        .iter()
        .find(|(held, _)| held == name)
        .map(|(_, value)| value)
}

/// Whether two optional component values are the same state: signed zeros differ, as in the hash.
fn same(a: Option<&Value>, b: Option<&Value>) -> bool {
    match (a, b) {
        (Some(a), Some(b)) => identical(a, b),
        (None, None) => true,
        _ => false,
    }
}

/// A pose's bits, so a signed zero is a difference.
fn pose_bits(pose: &Transform) -> [u32; 9] {
    let mut bits = [0; 9];
    for (slot, value) in bits.iter_mut().zip(
        pose.translation
            .iter()
            .chain(&pose.rotation)
            .chain(&pose.scale),
    ) {
        *slot = value.to_bits();
    }
    bits
}

fn pose_of(value: &Value) -> Option<Transform> {
    serde_json::from_value(value.clone()).ok()
}

fn pose_value(trs: &Trs) -> Value {
    let transform = Transform {
        translation: trs.translation,
        rotation: quat_to_euler_xyz_degrees(trs.rotation),
        scale: trs.scale,
    };
    serde_json::to_value(transform).expect("a Transform serializes")
}

/// The collapsed document entities for `flat`, the live scene of `ids` (see [`flatten_world`]).
///
/// # Errors
/// `L1234` for a world an instance entry cannot express; `L1230` for an unreadable prefab; see the
/// diagnostic's message for the instance and part.
pub(crate) fn collapse(
    live: Live<'_>,
    flat: Vec<SceneEntity>,
    ids: &[EntityId],
) -> Result<Vec<SceneEntity>, Diagnostic> {
    collapse_diffs(live, flat, ids)
        .map(|(entities, ..)| entities)
        .map_err(|refusal| refusal.diagnostic)
}

/// [`collapse`] with what each instance's entry says, in the order the instances first appear.
fn collapse_diffs(
    live: Live<'_>,
    flat: Vec<SceneEntity>,
    ids: &[EntityId],
) -> Result<Collapsed, Refusal> {
    let mut collapser = Collapser::new(live, flat, ids)?;
    collapser.probe()?;
    let outside = if collapser.live.stubs.is_some() {
        collapser.outside_refusals()
    } else {
        BTreeMap::new()
    };
    let (entities, differs, gained) = collapser.run()?;
    let entities = collapser.reconcile(entities)?;
    // The reconciled entries: reconcile drops a `removed` path the prefab no longer has.
    let diffs = collapser
        .groups
        .iter()
        .zip(&collapser.group_slot)
        .zip(differs)
        .zip(gained)
        .map(
            |(((group, &slot), placement_differs), gained)| InstanceDiff {
                root: group.root,
                name: group.name.clone(),
                entry: *entities[slot]
                    .instance
                    .clone()
                    .expect("a group slot holds its entry"),
                placement_differs,
                gained,
            },
        )
        .collect();
    Ok((entities, diffs, outside))
}

/// What [`scope`] yields: the entities, their ids, the reference-only stubs among them and each
/// entity's position in the whole world.
type Scoped = (
    Vec<SceneEntity>,
    Vec<EntityId>,
    BTreeSet<EntityId>,
    Vec<usize>,
);

/// The inputs of a collapse scoped to the instances whose root passes `keep`: their entities, in live
/// order, and the entities they refer to as reference-only stubs (flattened without their own
/// references) — the rest of the world is not instance data.
///
/// # Errors
/// `L1018` for a reference or `Parent` cycle among the instances' entities.
pub(crate) fn scope(
    live: &Live<'_>,
    keep: &dyn Fn(EntityId) -> bool,
) -> Result<Scoped, Diagnostic> {
    let world = live.world;
    let registry = live.registry;
    let everything: Vec<EntityId> = world
        .entities()
        .chain(world.presentation_entities())
        .collect();
    let members: BTreeSet<EntityId> = everything
        .iter()
        .copied()
        .filter(|&id| {
            world
                .get::<PrefabInstance>(id)
                .ok()
                .flatten()
                .is_some_and(|held| keep(held.root))
        })
        .collect();
    let mut stubs = BTreeSet::new();
    for &id in &members {
        for info in registry
            .iter()
            .filter(|info| !info.entity_refs().is_empty())
        {
            let Ok(Some(value)) = info.serialize(world, id) else {
                continue;
            };
            for field in &info.entity_ref_sites(&value) {
                if let Some(target) = ref_target(&value, field)
                    && !members.contains(&target)
                    && world.is_alive(target)
                {
                    stubs.insert(target);
                }
            }
        }
    }
    let (ids, positions): (Vec<EntityId>, Vec<usize>) = everything
        .into_iter()
        .enumerate()
        .filter(|(_, id)| members.contains(id) || stubs.contains(id))
        .map(|(position, id)| (id, position))
        .unzip();
    let entities = flatten_ids(world, registry, &ids, &|id| !stubs.contains(&id))?;
    let mut scene = Scene {
        name: None,
        generated_from: None,
        seed: None,
        physics: None,
        preload: Vec::new(),
        render_layers: Vec::new(),
        entities,
    };
    if let Some(cycle) = ludion_scene::hierarchy_cycles(&scene, registry).first() {
        return Err(hierarchy_fault(world, &ids, cycle));
    }
    Ok((std::mem::take(&mut scene.entities), ids, stubs, positions))
}

/// What a save would write for each of `roots` (an instance's `PrefabInstance.root`): `None` for one
/// that is not a named group, `Err` for one the save refuses. Only instances are collapsed; a refusal
/// is attributed to its instance, which is then left out and the rest collapsed again.
///
/// # Errors
/// `L1018` for a reference or `Parent` cycle among the instances' entities.
pub(crate) fn instance_overrides(
    live: Live<'_>,
    roots: &[EntityId],
) -> Result<Vec<Result<Option<InstanceDiff>, Diagnostic>>, Diagnostic> {
    let mut refused: BTreeMap<EntityId, Diagnostic> = BTreeMap::new();
    let mut diffs = Vec::new();
    let mut unattributed = false;
    loop {
        let (flat, ids, stubs, positions) = scope(&live, &|root| !refused.contains_key(&root))?;
        let scoped = Live {
            stubs: Some(&stubs),
            positions: Some(&positions),
            ..live.clone()
        };
        match collapse_diffs(scoped, flat, &ids) {
            Ok((_, found, outside)) => {
                for (root, diagnostic) in outside {
                    refused.entry(root).or_insert(diagnostic);
                }
                diffs = found;
                break;
            }
            Err(Refusal {
                root: Some(root),
                diagnostic,
            }) if !refused.contains_key(&root) => {
                refused.insert(root, diagnostic);
            }
            Err(_) => {
                unattributed = true;
                break;
            }
        }
    }
    if unattributed {
        for &root in roots {
            if refused.contains_key(&root) {
                continue;
            }
            let (flat, ids, stubs, positions) = scope(&live, &|held| held == root)?;
            let alone = Live {
                stubs: Some(&stubs),
                positions: Some(&positions),
                ..live.clone()
            };
            match collapse_diffs(alone, flat, &ids) {
                Ok((_, found, outside)) => {
                    diffs.extend(found);
                    for (root, diagnostic) in outside {
                        refused.entry(root).or_insert(diagnostic);
                    }
                }
                Err(refusal) => {
                    refused.insert(root, refusal.diagnostic);
                }
            }
        }
    }
    Ok(roots
        .iter()
        .map(|root| match refused.get(root) {
            Some(diagnostic) => Err(diagnostic.clone()),
            None => Ok(diffs
                .iter()
                .position(|diff| diff.root == *root)
                .map(|at| diffs.swap_remove(at))),
        })
        .collect())
}

/// The `prefab.revert` pointers of one part's overrides: the leaves of each component the prefab part
/// has, and `/Component` alone for one in `gained`, which revert removes whole.
pub(crate) fn override_pointers(components: &Components, gained: &[String]) -> Vec<String> {
    components
        .iter()
        .flat_map(|(name, value)| {
            if gained.contains(name) {
                leaf_pointers(name, &Value::Null)
            } else {
                leaf_pointers(name, value)
            }
        })
        .collect()
}

/// The JSON pointers of the leaves of component `component`'s `value`: objects recurse, anything
/// else (an array, a scalar, an empty object) is a leaf. Keys are escaped as RFC 6901 asks.
pub(crate) fn leaf_pointers(component: &str, value: &Value) -> Vec<String> {
    fn walk(prefix: String, value: &Value, out: &mut Vec<String>) {
        match value.as_object().filter(|object| !object.is_empty()) {
            Some(object) => {
                for (key, inner) in object {
                    let key = pointer_token(key);
                    walk(format!("{prefix}/{key}"), inner, out);
                }
            }
            None => out.push(prefix),
        }
    }
    let mut out = Vec::new();
    walk(format!("/{}", pointer_token(component)), value, &mut out);
    out
}

/// The version a candidate document names a prefab by: `{key}@{snapshot}`. The version is a suffix
/// so the key's directory stays the real one: a prefab resolves its nested paths against it.
fn versioned(version: usize, key: &str) -> String {
    format!("{}@{version}", normalize_prefab_path(key))
}

/// Splits `{rest}@{snapshot}`.
fn split_version(path: &str) -> Option<(usize, &str)> {
    let (rest, version) = path.rsplit_once('@')?;
    Some((version.parse().ok()?, rest))
}

/// `message` without the `@{snapshot}` a versioned key carries in a candidate document.
fn plain_key(message: &str) -> String {
    let mut plain = String::with_capacity(message.len());
    let mut rest = message;
    while let Some(at) = rest.find('@') {
        plain.push_str(&rest[..at]);
        let after = &rest[at + 1..];
        let digits = after.bytes().take_while(u8::is_ascii_digit).count();
        let ends_key = !after[digits..].starts_with(|c: char| c.is_alphanumeric());
        if digits > 0 && ends_key {
            rest = &after[digits..];
        } else {
            plain.push('@');
            rest = after;
        }
    }
    plain.push_str(rest);
    plain
}

/// The edits of the instance made of `parts` (live entity, part path) as one collapsed entry: the
/// `overrides` and `removed` an apply writes into the prefab. The instance is collapsed alone as
/// `apply/<part>`, so it becomes an entry whether or not its spawn was named.
///
/// What is the instance's context rather than its content never appears: the anchor's `Transform` is
/// placement, and a `Parent` onto an entity outside the instance is where it hangs.
///
/// # Errors
/// `L1237` for a component that refers outside the instance (other than that `Parent`); `L1234` and
/// `L1230` from the collapse itself.
pub(crate) fn instance_entry(
    world: &World,
    registry: &Registry,
    project_root: &std::path::Path,
    parts: &[(EntityId, String)],
    placement: Option<InstanceTransform>,
) -> Result<InstanceEntry, Diagnostic> {
    let (flat, ids) = flatten_world(world, registry)?;
    let mut position = BTreeMap::new();
    let mut sub_flat = Vec::new();
    let mut sub_ids = Vec::new();
    let mut paths = Vec::new();
    for (live, id) in ids.iter().enumerate() {
        if let Some((_, path)) = parts.iter().find(|(part, _)| part == id) {
            position.insert(live, sub_ids.len());
            sub_flat.push(flat[live].clone());
            sub_ids.push(*id);
            paths.push(path.as_str());
        }
    }
    for (entity, path) in sub_flat.iter_mut().zip(&paths) {
        let label = entity.name.clone();
        entity.name = Some(format!("apply/{path}"));
        let mut kept = Components::new();
        for (name, mut value) in std::mem::take(&mut entity.components) {
            let refers_out = remap_value(registry, &name, &mut value, &|index| {
                position.get(&index).copied()
            });
            match refers_out {
                Ok(()) => kept.push((name, value)),
                Err(_) if name == PARENT => {}
                Err(_) => {
                    return Err(Diagnostic::new(
                        code::PREFAB_APPLY,
                        format!(
                            "part `{path}`{} refers to an entity outside the instance through `{name}`",
                            label.map_or_else(String::new, |label| format!(" (`{label}`)"))
                        ),
                    )
                    .with_help("a prefab holds only references between its own parts; remove the reference first"));
                }
            }
        }
        entity.components = kept;
    }
    let anchor = sub_flat
        .iter()
        .position(|entity| find(&entity.components, PARENT).is_none())
        .map(|at| paths[at].to_owned());

    let live = Live {
        world,
        registry,
        project_root,
        authored: None,
        library: world.resource::<PrefabLibrary>(),
        exact_fallback: false,
        placement,
        stubs: None,
        positions: None,
    };
    let mut entry = collapse(live, sub_flat, &sub_ids)?
        .pop()
        .and_then(|entity| entity.instance)
        .map(|entry| *entry)
        .expect("a named instance collapses to one entry");
    if let Some(anchor) = anchor
        && let Some(components) = entry.overrides.get_mut(&anchor)
    {
        components.retain(|(name, _)| name != TRANSFORM);
        if components.is_empty() {
            entry.overrides.remove(&anchor);
        }
    }
    Ok(entry)
}

struct Collapser<'a> {
    live: Live<'a>,
    ids: &'a [EntityId],
    flat: Vec<SceneEntity>,
    groups: Vec<Group>,
    slots: Vec<Slot>,
    /// Each group's slot.
    group_slot: Vec<usize>,
    /// Each live position's named group.
    group_of: Vec<Option<usize>>,
    /// A readable name per live entity, for messages.
    labels: Vec<String>,
    /// Per group, the prefab part paths with no live part.
    removed: Vec<Vec<String>>,
    /// Per group, the path of its first surviving part.
    first: Vec<String>,
    /// Per group, whether the entry needs `presentation: true` to reproduce its parts' allocator.
    presentation: Vec<bool>,
    /// The plain entities by slot, references in document space.
    plain: Vec<Option<SceneEntity>>,
    /// Per group, the library snapshot its instance was expanded from; `None` resolves from disk.
    versions: Vec<Option<usize>>,
    /// Prefab texts read from the disk, by key: one read per collapse.
    disk: RefCell<BTreeMap<String, String>>,
    /// Namespaced prefab texts by `(snapshot, key)`.
    namespaced: RefCell<BTreeMap<(usize, String), String>>,
    /// An ungrouped instance's dead `root` mapped to its first live part.
    alias: BTreeMap<EntityId, EntityId>,
    /// Each live position's provenance; `None` for a plain entity and for a stub.
    provenance: Vec<Option<PrefabInstance>>,
}

impl<'a> Collapser<'a> {
    fn new(live: Live<'a>, flat: Vec<SceneEntity>, ids: &'a [EntityId]) -> Result<Self, Refusal> {
        let provenance: Vec<Option<PrefabInstance>> = ids
            .iter()
            .map(|&id| {
                if live.stubs.is_some_and(|stubs| stubs.contains(&id)) {
                    return None;
                }
                live.world.get::<PrefabInstance>(id).ok().flatten().cloned()
            })
            .collect();
        let mut roots: Vec<(EntityId, Vec<usize>)> = Vec::new();
        for (position, held) in provenance.iter().enumerate() {
            let Some(held) = held else { continue };
            match roots.iter_mut().find(|(root, _)| *root == held.root) {
                Some((_, members)) => members.push(position),
                None => roots.push((held.root, vec![position])),
            }
        }

        let alias = roots
            .iter()
            .filter(|(root, _)| !ids.contains(root))
            .map(|(root, members)| (*root, ids[members[0]]))
            .collect();
        let mut groups: Vec<Group> = Vec::new();
        let mut group_of = vec![None; ids.len()];
        for (root, members) in &roots {
            let held = |position: usize| provenance[position].as_ref().expect("a member");
            let source = held(members[0]).source.clone();
            if let Some(&other) = members.iter().find(|&&m| held(m).source != source) {
                return Err(refuse_at(
                    *root,
                    format!(
                        "the parts of one prefab instance come from different prefabs: `{source}` and `{}`",
                        held(other).source
                    ),
                ));
            }
            // A runtime room has no prefab text: its parts save as plain entities.
            if crate::prefab::is_room_source(live.library, live.project_root, &source) {
                continue;
            }
            let names: Vec<Option<&str>> = members
                .iter()
                .map(|&m| instance_name(flat[m].name.as_deref(), &held(m).part))
                .collect();
            let Some(name) = names[0] else {
                if names.iter().any(Option::is_some) {
                    return Err(refuse_at(
                        *root,
                        format!(
                            "the parts of the `{source}` instance are only partly named `<instance>/<part>`"
                        ),
                    ));
                }
                continue;
            };
            if names.iter().any(|other| *other != Some(name)) {
                return Err(refuse_at(
                    *root,
                    format!(
                        "the parts of the `{source}` instance `{name}` are named for different instances"
                    ),
                ));
            }
            let group = groups.len();
            let mut parts = Vec::with_capacity(members.len());
            for &member in members {
                let path = held(member).part.clone();
                if parts.iter().any(|(_, seen)| *seen == path) {
                    return Err(refuse_at(
                        *root,
                        format!("instance `{name}` has two parts at path `{path}`"),
                    ));
                }
                group_of[member] = Some(group);
                parts.push((member, path));
            }
            if let Some(other) = groups.iter().find(|g| g.name == name) {
                return Err(refuse_at(
                    *root,
                    format!(
                        "two instances are both named `{name}` (prefabs `{}` and `{source}`)",
                        other.source
                    ),
                ));
            }
            groups.push(Group {
                name: name.to_owned(),
                source,
                root: *root,
                parts,
            });
        }

        let mut slots = Vec::new();
        let mut group_slot = vec![usize::MAX; groups.len()];
        for (position, group) in group_of.iter().enumerate() {
            match *group {
                Some(group) if group_slot[group] == usize::MAX => {
                    group_slot[group] = slots.len();
                    slots.push(Slot::Group(group));
                }
                Some(_) => {}
                None => slots.push(Slot::Plain(position)),
            }
        }
        let mut labels: Vec<String> = flat
            .iter()
            .enumerate()
            .map(|(position, entity)| {
                let world = live.positions.map_or(position, |world| world[position]);
                entity_label(entity.name.as_deref(), world)
            })
            .collect();
        for group in &groups {
            for (position, path) in &group.parts {
                labels[*position] = format!("part `{path}` of instance `{}`", group.name);
            }
        }
        let versions = groups
            .iter()
            .map(|group| {
                live.library
                    .and_then(|library| library.expanded_from(group.root, &group.source))
            })
            .collect();
        Ok(Collapser {
            live,
            ids,
            flat,
            groups,
            slots,
            group_slot,
            group_of,
            labels,
            removed: Vec::new(),
            first: Vec::new(),
            presentation: Vec::new(),
            plain: Vec::new(),
            versions,
            disk: RefCell::new(BTreeMap::new()),
            namespaced: RefCell::new(BTreeMap::new()),
            alias,
            provenance,
        })
    }

    /// How a candidate document names `prefab` for `group`: by the snapshot its instance was
    /// expanded from, when it has one.
    fn candidate_path(&self, group: usize, prefab: &str) -> String {
        match self.versions[group] {
            Some(version) => versioned(version, prefab),
            None => prefab.to_owned(),
        }
    }

    /// The resolver of every candidate expansion: a versioned path reads the snapshot (the disk for
    /// a key the snapshot never held) and keeps nested instances in that snapshot; any other path
    /// reads the disk.
    fn resolve(&self, path: &str) -> Result<String, Diagnostic> {
        let Some((version, rest)) = split_version(path) else {
            return self.resolve_disk(path);
        };
        let key = normalize_prefab_path(rest);
        if let Some(text) = self.namespaced.borrow().get(&(version, key.clone())) {
            return Ok(text.clone());
        }
        let held = self
            .live
            .library
            .and_then(|library| library.snapshot(version).get(&key).cloned());
        let text = match held {
            Some(text) => text,
            None => read_prefab_text(self.live.project_root, &key)?,
        };
        let text = self.namespace(version, text);
        self.namespaced
            .borrow_mut()
            .insert((version, key), text.clone());
        Ok(text)
    }

    /// The text of the prefab file at `path` as written in a scene.
    fn resolve_disk(&self, path: &str) -> Result<String, Diagnostic> {
        let key = prefab_key(path).map_err(|mut found| found.swap_remove(0))?;
        if let Some(text) = self.disk.borrow().get(&key) {
            return Ok(text.clone());
        }
        let text = read_prefab_text(self.live.project_root, &key)?;
        self.disk.borrow_mut().insert(key, text.clone());
        Ok(text)
    }

    /// `text` with every nested instance's prefab naming snapshot `version`. A text that does not
    /// parse is returned as is, for the expansion to report.
    fn namespace(&self, version: usize, text: String) -> String {
        let Ok(mut document) = ludion_scene::parse_prefab(&text) else {
            return text;
        };
        let mut nested = false;
        for entity in &mut document.entities {
            if let Some(entry) = entity.instance.as_mut() {
                entry.prefab = versioned(version, &entry.prefab);
                nested = true;
            }
        }
        if nested {
            ludion_scene::emit_prefab(&document, self.live.registry)
        } else {
            text
        }
    }

    /// Expands each group alone at identity to learn which parts the prefab has now, then fixes
    /// which parts are removed and which is first, and prepares the plain entities.
    fn probe(&mut self) -> Result<(), Refusal> {
        let entities = self
            .groups
            .iter()
            .enumerate()
            .map(|(g, group)| {
                entry_entity(
                    group,
                    InstanceEntry {
                        prefab: self.candidate_path(g, &group.source),
                        ..Default::default()
                    },
                )
            })
            .collect();
        let probe = Scene {
            name: None,
            generated_from: None,
            seed: None,
            physics: None,
            preload: Vec::new(),
            render_layers: Vec::new(),
            entities,
        };
        let expanded =
            expand_scene_with(self.live.registry, &probe, &mut |path| self.resolve(path))
                .map_err(|found| unloadable(&found))?;
        let (mut first, mut removed, mut presentation) = (Vec::new(), Vec::new(), Vec::new());
        for (group, entry) in self.groups.iter().zip(0..) {
            let instance = expanded
                .instances
                .iter()
                .find(|instance| instance.depth == 0 && instance.entry == entry);
            let full = instance
                .map(|instance| instance.paths.clone())
                .unwrap_or_default();
            for (position, path) in &group.parts {
                if !full.contains(path) {
                    let when = if self.versions[entry].is_some() {
                        "when it was expanded"
                    } else {
                        "on disk now"
                    };
                    return Err(refuse_at(
                        group.root,
                        format!(
                            "{} is not a part of `{}` as it is {when}",
                            self.labels[*position], group.source
                        ),
                    ));
                }
            }
            // (position, live is presentation, the prefab's part is presentation) per live part.
            let kinds: Vec<(usize, bool, bool)> = group
                .parts
                .iter()
                .filter_map(|(position, path)| {
                    let at = full.iter().position(|held| held == path)?;
                    let own = expanded.scene.entities[instance?.parts.start + at].presentation;
                    Some((
                        *position,
                        ludion_ecs::is_presentation(self.ids[*position]),
                        own,
                    ))
                })
                .collect();
            // The entry's flag moves every part at once; a partial move would save without the key
            // and reload as the prefab's kinds.
            let flag = kinds.iter().any(|(_, live, own)| live != own);
            if flag
                && kinds.iter().any(|(_, live, _)| !live)
                && let Some((position, live, own)) = kinds.iter().find(|(_, live, own)| live != own)
            {
                let kind = |presentation: bool| if presentation { "presentation" } else { "sim" };
                return Err(refuse_at(
                    group.root,
                    format!(
                        "{} is {}, but `{}` makes it {}: an instance entry cannot express the mix",
                        self.labels[*position],
                        kind(*live),
                        group.source,
                        kind(*own)
                    ),
                ));
            }
            presentation.push(flag);
            let live_paths = |path: &String| group.parts.iter().any(|(_, held)| held == path);
            first.push(
                full.iter()
                    .find(|path| live_paths(path))
                    .cloned()
                    .unwrap_or_default(),
            );
            removed.push(
                full.iter()
                    .filter(|path| !live_paths(path))
                    .cloned()
                    .collect(),
            );
        }
        self.first = first;
        self.removed = removed;
        self.presentation = presentation;
        self.prepare_plain()
    }

    /// Document position of each live entity a reference may name: a plain entity, or the first part
    /// of a group (what an entry resolves to).
    fn doc_of_live(&self) -> Vec<Option<usize>> {
        let mut doc = vec![None; self.ids.len()];
        for (slot, kind) in self.slots.iter().enumerate() {
            match *kind {
                Slot::Plain(position) => doc[position] = Some(slot),
                Slot::Group(group) => {
                    let group = &self.groups[group];
                    for (position, path) in &group.parts {
                        if *path == self.first[self.group_of[*position].expect("a part")] {
                            doc[*position] = Some(slot);
                        }
                    }
                }
            }
        }
        doc
    }

    fn prepare_plain(&mut self) -> Result<(), Refusal> {
        // An instance with unnamed parts stays plain entities and keeps its provenance block.
        let position: BTreeMap<EntityId, usize> = self
            .ids
            .iter()
            .enumerate()
            .map(|(at, &id)| (id, at))
            .collect();
        if let Some(info) = self.live.registry.get(PROVENANCE) {
            for (at, &id) in self.ids.iter().enumerate() {
                if self.group_of[at].is_some() {
                    continue;
                }
                if let Some(held) = &self.provenance[at] {
                    let mut held = held.clone();
                    if let Some(&alive) = self.alias.get(&held.root) {
                        held.root = alive;
                    }
                    let value = serde_json::to_value(held).expect("a PrefabInstance serializes");
                    let value = remap_entity_refs(self.live.world, id, info, value, &position)?;
                    self.flat[at]
                        .components
                        .push((PROVENANCE.to_owned(), value));
                }
            }
        }
        let doc = self.doc_of_live();
        let mut plain = Vec::with_capacity(self.slots.len());
        for kind in &self.slots {
            let Slot::Plain(at) = *kind else {
                plain.push(None);
                continue;
            };
            let mut entity = self.flat[at].clone();
            let result = remap_components(self.live.registry, &mut entity.components, &|index| {
                doc.get(index).copied().flatten()
            });
            if let Err(target) = result {
                let root = self.group_of[target].map(|g| self.groups[g].root);
                let message = format!(
                    "{} refers to {}, which is not the first surviving part of its instance",
                    self.labels[at], self.labels[target]
                );
                return Err(match root {
                    Some(root) => refuse_at(root, message),
                    None => refuse(message).into(),
                });
            }
            plain.push(Some(entity));
        }
        self.plain = plain;
        Ok(())
    }

    /// The authored entry of the same name and prefab as `group`, when the source document has one.
    fn prior(&self, group: &Group) -> Option<Prior> {
        let authored = self.live.authored?;
        authored.entities.iter().find_map(|entity| {
            let entry = entity.instance.as_ref()?;
            (entity.name.as_deref() == Some(group.name.as_str())
                && normalize_prefab_path(&entry.prefab) == group.source)
                .then(|| Prior {
                    prefab: entry.prefab.clone(),
                    setting: Setting {
                        transform: entry.transform.clone(),
                        poses: entry
                            .overrides
                            .iter()
                            .filter_map(|(path, components)| {
                                let pose = find(components, TRANSFORM)?;
                                group
                                    .parts
                                    .iter()
                                    .any(|(_, held)| held == path)
                                    .then(|| (path.clone(), pose.clone()))
                            })
                            .collect(),
                    },
                })
        })
    }

    fn entry(
        &self,
        group: usize,
        prefab: &str,
        setting: &Setting,
        others: &BTreeMap<String, Components>,
    ) -> InstanceEntry {
        let mut overrides = others.clone();
        for (path, pose) in &setting.poses {
            overrides
                .entry(path.clone())
                .or_default()
                .push((TRANSFORM.to_owned(), pose.clone()));
        }
        InstanceEntry {
            prefab: prefab.to_owned(),
            presentation: self.presentation[group],
            transform: setting.transform.clone(),
            overrides,
            removed: self.removed[group].clone(),
        }
    }

    /// The document entities; a `candidate` names each instance's prefab by the snapshot it was
    /// expanded from, the output keeps the path as authored.
    fn document(
        &self,
        prefabs: &[String],
        settings: &[Setting],
        others: &[BTreeMap<String, Components>],
        candidate: bool,
    ) -> Vec<SceneEntity> {
        self.slots
            .iter()
            .zip(&self.plain)
            .map(|(kind, plain)| match *kind {
                Slot::Plain(_) => plain.clone().expect("a plain slot holds its entity"),
                Slot::Group(group) => {
                    let prefab = if candidate {
                        self.candidate_path(group, &prefabs[group])
                    } else {
                        prefabs[group].clone()
                    };
                    entry_entity(
                        &self.groups[group],
                        self.entry(group, &prefab, &settings[group], &others[group]),
                    )
                }
            })
            .collect()
    }

    /// Expands and instantiates the candidate `entities`; the result's components are in
    /// flat-candidate space.
    fn build(&self, entities: Vec<SceneEntity>) -> Result<Built, Diagnostic> {
        let registry = self.live.registry;
        let candidate = Scene {
            name: None,
            generated_from: None,
            seed: None,
            physics: None,
            preload: Vec::new(),
            render_layers: Vec::new(),
            entities,
        };
        let expanded = expand_scene_with(registry, &candidate, &mut |path| self.resolve(path))
            .map_err(|found| unloadable(&found))?;
        let (world, ids) = instantiate_expanded_world(&expanded, registry, self.live.project_root)
            .map_err(|found| unloadable(&found))?;
        let (flat, flat_ids) = flatten_world(&world, registry).map_err(|found| {
            refuse(format!(
                "the saved instances would not load: {}",
                found.message
            ))
        })?;
        let s_of: BTreeMap<EntityId, usize> = flat_ids
            .iter()
            .enumerate()
            .map(|(s, &id)| (id, s))
            .collect();
        let mut e_of_s = vec![0; flat.len()];
        for (e, id) in ids.iter().enumerate() {
            e_of_s[s_of[id]] = e;
        }
        let comps = ids
            .iter()
            .map(|id| {
                let mut components = flat[s_of[id]].components.clone();
                let _ = remap_components(registry, &mut components, &|s| e_of_s.get(s).copied());
                components
            })
            .collect();
        Ok(Built { expanded, comps })
    }

    fn layout(&self, expanded: &ExpandedScene) -> Layout {
        let mut start = vec![usize::MAX; self.slots.len()];
        for (e, &d) in expanded.entry_of.iter().enumerate() {
            if start[d] == usize::MAX {
                start[d] = e;
            }
        }
        let mut e_of_live = vec![0; self.ids.len()];
        for (slot, kind) in self.slots.iter().enumerate() {
            match *kind {
                Slot::Plain(position) => e_of_live[position] = start[slot],
                Slot::Group(group) => {
                    let instance = expanded
                        .instances
                        .iter()
                        .find(|i| i.depth == 0 && i.entry == slot)
                        .expect("a group's instance expands");
                    for (position, path) in &self.groups[group].parts {
                        let at = instance
                            .paths
                            .iter()
                            .position(|held| held == path)
                            .expect("the probe found every live path");
                        e_of_live[*position] = instance.parts.start + at;
                    }
                }
            }
        }
        let mut live_of_e = vec![0; self.ids.len()];
        for (position, &e) in e_of_live.iter().enumerate() {
            live_of_e[e] = position;
        }
        let doc_of_e = expanded
            .entry_of
            .iter()
            .enumerate()
            .map(|(e, &d)| (start[d] == e).then_some(d))
            .collect();
        Layout {
            e_of_live,
            live_of_e,
            doc_of_e,
        }
    }

    /// The document entities, and per group whether its saved placement differs from the one it was
    /// built at and which components its parts gained over the prefab.
    fn run(&mut self) -> Result<Run, Refusal> {
        if self.groups.is_empty() {
            let entities = self
                .plain
                .iter()
                .map(|entity| entity.clone().expect("every slot is plain"))
                .collect();
            return Ok((entities, Vec::new(), Vec::new()));
        }
        let count = self.groups.len();
        let priors: Vec<Option<Prior>> = self.groups.iter().map(|g| self.prior(g)).collect();
        let prefabs: Vec<String> = self
            .groups
            .iter()
            .zip(&priors)
            .map(|(group, prior)| {
                prior
                    .as_ref()
                    .map_or(group.source.clone(), |p| p.prefab.clone())
            })
            .collect();
        // Without an authored prior a group is built where it was placed: the caller's placement,
        // else the one the library recorded for its root.
        let base: Vec<Setting> = priors
            .iter()
            .zip(&self.groups)
            .map(|(prior, group)| {
                prior.as_ref().map_or_else(
                    || Setting {
                        transform: self
                            .live
                            .placement
                            .clone()
                            .or_else(|| {
                                self.live
                                    .library
                                    .and_then(|library| library.placement(group.root))
                                    .cloned()
                            })
                            .unwrap_or_default(),
                        poses: BTreeMap::new(),
                    },
                    |p| p.setting.clone(),
                )
            })
            .collect();
        let placement_bits = |t: &InstanceTransform| -> [u64; 9] {
            let mut bits = [0; 9];
            for (slot, value) in bits
                .iter_mut()
                .zip(t.translation.iter().chain(&t.rotation).chain(&t.scale))
            {
                *slot = value.to_bits();
            }
            bits
        };
        let differs = |settings: &[Setting]| -> Vec<bool> {
            settings
                .iter()
                .zip(&base)
                .map(|(s, b)| placement_bits(&s.transform) != placement_bits(&b.transform))
                .collect()
        };
        let no_poses: Vec<Setting> = base
            .iter()
            .map(|s| Setting {
                transform: s.transform.clone(),
                poses: BTreeMap::new(),
            })
            .collect();
        let none: Vec<BTreeMap<String, Components>> = vec![BTreeMap::new(); count];

        // C0: the prefab alone, placed as authored — the expected value of every component.
        let c0 = self.build(self.document(&prefabs, &no_poses, &none, true))?;
        let layout = self.layout(&c0.expanded);
        let live_comps: Vec<Components> = (0..self.ids.len())
            .map(|position| {
                let mut components = self.flat[position].components.clone();
                let _ = remap_components(self.live.registry, &mut components, &|index| {
                    layout.e_of_live.get(index).copied()
                });
                components
            })
            .collect();
        let (others, gained) = self.others(&c0, &layout, &live_comps)?;

        // C1: with the live component differences and the authored poses.
        let c1 = self.build(self.document(&prefabs, &base, &others, true))?;
        let mut settings = base.clone();
        let mut moved: Vec<Option<usize>> = vec![None; count];
        let mut dirty = false;
        for g in 0..count {
            let slot = self.group_slot[g];
            let instance = c1
                .expanded
                .instances
                .iter()
                .find(|i| i.depth == 0 && i.entry == slot)
                .expect("a group's instance expands");
            let anchor = c1
                .expanded
                .roots(instance, self.live.registry)
                .first()
                .copied();
            let reproduces = |e: usize| {
                same(
                    find(&c1.comps[e], TRANSFORM),
                    find(&live_comps[layout.live_of_e[e]], TRANSFORM),
                )
            };
            if instance.parts.clone().all(reproduces) {
                continue;
            }
            dirty = true;
            if let Some(anchor) = anchor
                .filter(|&e| !reproduces(e))
                .filter(|_| self.live.placement.is_none())
            {
                moved[g] = Some(anchor);
                settings[g].poses.clear();
            } else {
                for (position, path) in &self.groups[g].parts {
                    if !reproduces(layout.e_of_live[*position]) {
                        settings[g].poses.remove(path);
                    }
                }
            }
        }
        if !dirty {
            let exempt = self.exempt(&c1, &layout, &live_comps);
            self.verify(&c1, &layout, &live_comps, &exempt)?;
            let entities = self.document(&prefabs, &settings, &others, false);
            return Ok((entities, differs(&settings), gained));
        }

        // C2: a moved instance's anchor at identity placement gives its new placement.
        if moved.iter().any(Option::is_some) {
            let mut at_origin = settings.clone();
            for (g, anchor) in moved.iter().enumerate() {
                if anchor.is_some() {
                    at_origin[g].transform = InstanceTransform::default();
                }
            }
            let c2 = self.build(self.document(&prefabs, &at_origin, &others, true))?;
            for (g, anchor) in moved.iter().enumerate() {
                let Some(anchor) = anchor else { continue };
                let anchor_live = layout.live_of_e[*anchor];
                // A root the prefab gives no `Transform` sits at the default pose.
                let here = find(&c2.comps[*anchor], TRANSFORM)
                    .and_then(pose_of)
                    .unwrap_or_default();
                let Some(live_pose) = find(&live_comps[anchor_live], TRANSFORM).and_then(pose_of)
                else {
                    return Err(refuse_at(
                        self.groups[g].root,
                        format!(
                            "{} has no `Transform`, which its prefab gives it",
                            self.labels[anchor_live]
                        ),
                    ));
                };
                let placement = placement_for(&level_trs(&live_pose), &level_trs(&here));
                // A recovered zero is unsigned: the verify rebuild catches a placement that needed one.
                let unsigned = |v: f32| f64::from(v) + 0.0;
                settings[g].transform = InstanceTransform {
                    translation: placement.translation.map(unsigned),
                    rotation: quat_to_euler_xyz_degrees(placement.rotation).map(unsigned),
                    scale: placement.scale.map(unsigned),
                };
            }
        }

        // C3: the poses the placement does not explain become overrides.
        let c3 = self.build(self.document(&prefabs, &settings, &others, true))?;
        self.poses(&c3, &layout, &live_comps, &mut settings, None);

        // C4: the candidate as it would be saved. A group whose top-level roots do not land on the
        // live poses is placed at identity, where each root's pose is exact.
        let mut c4 = self.build(self.document(&prefabs, &settings, &others, true))?;
        let mut exempt = self.exempt(&c4, &layout, &live_comps);
        if self.live.exact_fallback {
            let failing: Vec<usize> = (0..count)
                .filter(|&g| {
                    self.top_level_roots(&c4, g).iter().any(|&e| {
                        !exempt[e]
                            && !same(
                                find(&c4.comps[e], TRANSFORM),
                                find(&live_comps[layout.live_of_e[e]], TRANSFORM),
                            )
                    })
                })
                .collect();
            if !failing.is_empty() {
                for &g in &failing {
                    settings[g].transform = InstanceTransform::default();
                    settings[g].poses.clear();
                }
                let c5 = self.build(self.document(&prefabs, &settings, &others, true))?;
                self.poses(&c5, &layout, &live_comps, &mut settings, Some(&failing));
                c4 = self.build(self.document(&prefabs, &settings, &others, true))?;
                exempt = self.exempt(&c4, &layout, &live_comps);
            }
        }
        self.verify(&c4, &layout, &live_comps, &exempt)?;
        let entities = self.document(&prefabs, &settings, &others, false);
        Ok((entities, differs(&settings), gained))
    }

    /// The `Transform` overrides the placement does not explain, for the groups in `only` (all when
    /// `None`), read off `built`.
    fn poses(
        &self,
        built: &Built,
        layout: &Layout,
        live_comps: &[Components],
        settings: &mut [Setting],
        only: Option<&[usize]>,
    ) {
        for (g, (group, setting)) in self.groups.iter().zip(settings.iter_mut()).enumerate() {
            if only.is_some_and(|only| !only.contains(&g)) {
                continue;
            }
            for (position, path) in &group.parts {
                if find(&live_comps[*position], SKIN_BONE).is_some() {
                    continue;
                }
                let e = layout.e_of_live[*position];
                let scratch = find(&built.comps[e], TRANSFORM);
                let Some(live_value) = find(&live_comps[*position], TRANSFORM) else {
                    continue;
                };
                if same(scratch, Some(live_value)) {
                    continue;
                }
                let diff = if built.expanded.is_placed(e, self.live.registry) {
                    placed_pose(built, e, scratch, live_value)
                } else {
                    match scratch {
                        Some(held) => partial_diff(held, live_value, &[]),
                        None => Some(live_value.clone()),
                    }
                };
                if let Some(diff) = diff {
                    setting.poses.insert(path.clone(), diff);
                }
            }
        }
    }

    /// The roots of group `g`'s own instance that its own placement alone places: not a root of a
    /// nested instance placed with a non-identity `transform`.
    fn top_level_roots(&self, built: &Built, g: usize) -> Vec<usize> {
        let registry = self.live.registry;
        let slot = self.group_slot[g];
        let expanded = &built.expanded;
        let nested: Vec<usize> = expanded
            .instances
            .iter()
            .filter(|i| i.depth >= 1 && i.placement != Trs::IDENTITY)
            .flat_map(|i| expanded.roots(i, registry))
            .collect();
        expanded
            .instances
            .iter()
            .filter(|i| i.depth == 0 && i.entry == slot)
            .flat_map(|i| expanded.roots(i, registry))
            .filter(|e| !nested.contains(e))
            .collect()
    }

    /// Per flat-candidate position, whether its `Transform` is left out of [`Self::verify`]: a
    /// `SkinBone` part's pose comes from the skin pipeline, a nested instance's root under a
    /// non-identity placement is baked through Euler angles, and so are an apply's top-level roots.
    fn exempt(&self, built: &Built, layout: &Layout, live_comps: &[Components]) -> Vec<bool> {
        let registry = self.live.registry;
        let mut exempt: Vec<bool> = (0..built.comps.len())
            .map(|e| find(&live_comps[layout.live_of_e[e]], SKIN_BONE).is_some())
            .collect();
        for instance in &built.expanded.instances {
            let baked = instance.depth >= 1 && instance.placement != Trs::IDENTITY;
            if baked || (!self.live.exact_fallback && instance.depth == 0) {
                for e in built.expanded.roots(instance, registry) {
                    exempt[e] = true;
                }
            }
        }
        exempt
    }

    /// Refuses a save whose candidate does not reload to every live component of every instance part:
    /// the entry cannot express it, and writing it would lose the edit silently.
    fn verify(
        &self,
        built: &Built,
        layout: &Layout,
        live_comps: &[Components],
        exempt: &[bool],
    ) -> Result<(), Refusal> {
        for group in &self.groups {
            for (position, _) in &group.parts {
                let e = layout.e_of_live[*position];
                let (reloaded, live) = (&built.comps[e], &live_comps[*position]);
                let skipped = |name: &str| name == TRANSFORM && exempt[e];
                let differs = reloaded
                    .iter()
                    .find(|(name, value)| !skipped(name) && !same(find(live, name), Some(value)))
                    .map(|(name, _)| name)
                    .or_else(|| {
                        live.iter()
                            .find(|(name, _)| !skipped(name) && find(reloaded, name).is_none())
                            .map(|(name, _)| name)
                    });
                if let Some(name) = differs {
                    return Err(refuse_at(
                        group.root,
                        format!(
                            "{}'s `{name}` would not reload as it is live: the instance entry cannot express it",
                            self.labels[*position]
                        ),
                    ));
                }
            }
        }
        Ok(())
    }

    /// Makes the output load against the prefab files as they are now: an override naming a part
    /// the file lacks is refused, a `removed` entry naming one is dropped (the part is absent either
    /// way), and the whole document must expand.
    fn reconcile(&self, mut output: Vec<SceneEntity>) -> Result<Vec<SceneEntity>, Refusal> {
        if self.groups.is_empty() {
            return Ok(output);
        }
        let groups: Vec<(usize, usize)> = self
            .slots
            .iter()
            .enumerate()
            .filter_map(|(slot, kind)| match *kind {
                Slot::Group(group) => Some((slot, group)),
                Slot::Plain(_) => None,
            })
            .collect();
        let probe = Scene {
            name: None,
            generated_from: None,
            seed: None,
            physics: None,
            preload: Vec::new(),
            render_layers: Vec::new(),
            entities: groups
                .iter()
                .map(|&(slot, group)| {
                    let entry = output[slot].instance.as_ref().expect("a group slot");
                    entry_entity(
                        &self.groups[group],
                        InstanceEntry {
                            prefab: entry.prefab.clone(),
                            ..Default::default()
                        },
                    )
                })
                .collect(),
        };
        let current = expand_scene_with(self.live.registry, &probe, &mut |path| {
            self.resolve_disk(path)
        })
        .map_err(|found| would_not_load(&found))?;
        for (entry, &(slot, group)) in groups.iter().enumerate() {
            let paths = current
                .instances
                .iter()
                .find(|instance| instance.depth == 0 && instance.entry == entry)
                .map(|instance| instance.paths.clone())
                .unwrap_or_default();
            let group = &self.groups[group];
            let instance = output[slot].instance.as_mut().expect("a group slot");
            if let Some(lost) = instance.overrides.keys().find(|path| !paths.contains(path)) {
                return Err(Refusal {
                    root: Some(group.root),
                    diagnostic: Diagnostic::new(
                        code::PREFAB_INSTANCE_SAVE,
                        format!(
                            "instance `{}` overrides part `{lost}`, which `{}` no longer has",
                            group.name, group.source
                        ),
                    )
                    .with_help("revert or re-apply that part, or restore it in the prefab"),
                });
            }
            instance.removed.retain(|path| paths.contains(path));
        }
        let whole = Scene {
            name: None,
            generated_from: None,
            seed: None,
            physics: None,
            preload: Vec::new(),
            render_layers: Vec::new(),
            entities: output,
        };
        expand_scene_with(self.live.registry, &whole, &mut |path| {
            self.resolve_disk(path)
        })
        .map_err(|found| would_not_load(&found))?;
        let invalid = ludion_scene::validate(&whole, self.live.registry);
        if !invalid.is_empty() {
            return Err(would_not_load(&invalid).into());
        }
        Ok(whole.entities)
    }

    /// What a save would refuse about the plain entities, by the root of the instance it is about: one
    /// that refers to a part of an instance other than its first surviving part. Only the entities
    /// a scoped collapse leaves out are read here, straight from the world.
    fn outside_refusals(&self) -> BTreeMap<EntityId, Diagnostic> {
        let world = self.live.world;
        let subset: BTreeMap<EntityId, usize> = self
            .ids
            .iter()
            .enumerate()
            .map(|(at, &id)| (id, at))
            .collect();
        let mut found = BTreeMap::new();
        let live = world.entities().chain(world.presentation_entities());
        for (position, id) in live.enumerate() {
            if world.has::<PrefabInstance>(id) {
                continue;
            }
            for info in self.live.registry.iter() {
                if info.name() == PROVENANCE || info.entity_refs().is_empty() {
                    continue;
                }
                let Ok(Some(value)) = info.serialize(world, id) else {
                    continue;
                };
                for field in &info.entity_ref_sites(&value) {
                    let Some(&at) = ref_target(&value, field).and_then(|t| subset.get(&t)) else {
                        continue;
                    };
                    let Some(g) = self.group_of[at] else { continue };
                    let group = &self.groups[g];
                    let is_first = group
                        .parts
                        .iter()
                        .any(|(part, path)| *part == at && *path == self.first[g]);
                    if is_first || found.contains_key(&group.root) {
                        continue;
                    }
                    let name = world
                        .get::<Name>(id)
                        .ok()
                        .flatten()
                        .map(|name| name.0.clone());
                    found.insert(
                        group.root,
                        refuse(format!(
                            "{} refers to {}, which is not the first surviving part of its instance",
                            entity_label(name.as_deref(), position),
                            self.labels[at]
                        )),
                    );
                }
            }
        }
        found
    }

    /// Each group's non-`Transform` component differences, by part path, references in document space.
    fn others(
        &self,
        expected: &Built,
        layout: &Layout,
        live_comps: &[Components],
    ) -> Result<Others, Refusal> {
        let registry = self.live.registry;
        let mut all = Vec::with_capacity(self.groups.len());
        let mut gains = Vec::with_capacity(self.groups.len());
        for group in &self.groups {
            let mut by_path = BTreeMap::new();
            let mut gained = Gained::new();
            for (position, path) in &group.parts {
                let e = layout.e_of_live[*position];
                let (want, live) = (&expected.comps[e], &live_comps[*position]);
                if let Some((lost, _)) = want.iter().find(|(name, _)| find(live, name).is_none()) {
                    return Err(refuse_at(
                        group.root,
                        format!(
                            "{} lost its `{lost}` component, which `{}` gives it",
                            self.labels[*position], group.source
                        ),
                    ));
                }
                if find(live, TRANSFORM).is_some() && find(want, TRANSFORM).is_none() {
                    gained
                        .entry(path.clone())
                        .or_default()
                        .push(TRANSFORM.to_owned());
                }
                let mut diffs: Components = Vec::new();
                for (name, value) in live.iter().filter(|(name, _)| name != TRANSFORM) {
                    let atomic: Vec<String> = registry
                        .get(name)
                        .map(|info| {
                            info.entity_ref_sites(value)
                                .into_iter()
                                .map(|f| f.suffix)
                                .collect()
                        })
                        .unwrap_or_default();
                    let atomic: Vec<&str> = atomic.iter().map(String::as_str).collect();
                    let diff = match find(want, name) {
                        Some(held) => partial_diff(held, value, &atomic),
                        None => {
                            gained.entry(path.clone()).or_default().push(name.clone());
                            Some(value.clone())
                        }
                    };
                    let Some(mut diff) = diff else { continue };
                    let result = remap_value(registry, name, &mut diff, &|index| {
                        layout.doc_of_e.get(index).copied().flatten()
                    });
                    if let Err(target) = result {
                        return Err(refuse_at(
                            group.root,
                            format!(
                                "{} refers to {} through `{name}`, which is not the first surviving part of its instance",
                                self.labels[*position], self.labels[layout.live_of_e[target]]
                            ),
                        ));
                    }
                    diffs.push((name.clone(), diff));
                }
                if !diffs.is_empty() {
                    by_path.insert(path.clone(), diffs);
                }
            }
            all.push(by_path);
            gains.push(gained);
        }
        Ok((all, gains))
    }
}

/// The `Transform` override for a placed part (a root): the pose, in the part's own space, whose
/// placement lands on `live`. `None` when the build already lands on it exactly.
fn placed_pose(built: &Built, e: usize, scratch: Option<&Value>, live: &Value) -> Option<Value> {
    let unplaced = built.expanded.scene.entities[e]
        .components
        .iter()
        .find(|(name, _)| name == TRANSFORM)
        .map(|(_, value)| value);
    let own = unplaced.and_then(pose_of).unwrap_or_default();
    if scratch.and_then(pose_of).as_ref().map(pose_bits) == unplaced.and(Some(pose_bits(&own))) {
        // The build left the part unbaked, so the difference is exact.
        return match scratch {
            Some(held) => partial_diff(held, live, &[]),
            None => Some(live.clone()),
        };
    }
    let (here, wanted) = (scratch.and_then(pose_of)?, pose_of(live)?);
    if pose_bits(&here) == pose_bits(&wanted) {
        return None;
    }
    let local = local_under_placement(&level_trs(&own), &level_trs(&here), &level_trs(&wanted));
    let value = pose_value(&local);
    match unplaced {
        Some(_) => partial_diff(
            &serde_json::to_value(own).expect("a Transform serializes"),
            &value,
            &[],
        ),
        None => Some(value),
    }
}

/// The pose, in a part's own space, that lands on `wanted` under the placement that takes `own`
/// to `here`.
fn local_under_placement(own: &Trs, here: &Trs, wanted: &Trs) -> Trs {
    here.parent_for(own).localize(wanted)
}

/// The placement that takes an anchor's pose at identity placement, `here`, to `live`.
fn placement_for(live: &Trs, here: &Trs) -> Trs {
    live.parent_for(here)
}

/// How a message names a plain entity.
fn entity_label(name: Option<&str>, position: usize) -> String {
    match name {
        Some(name) => format!("entity `{name}`"),
        None => format!("the unnamed entity at position {position}"),
    }
}

fn entry_entity(group: &Group, entry: InstanceEntry) -> SceneEntity {
    SceneEntity {
        name: Some(group.name.clone()),
        instance: Some(Box::new(entry)),
        ..Default::default()
    }
}

/// An expansion or instantiation failure of a document: an unreadable prefab keeps its own code,
/// anything else is the text failing to load, described by `context` (what was being loaded). An
/// empty `found` still refuses, without a cause to name.
fn not_loadable(found: &[Diagnostic], context: &str) -> Diagnostic {
    let Some(first) = found.first() else {
        return refuse(format!("the saved instances {context}"));
    };
    if first.code == code::PREFAB_UNRESOLVED {
        return first.clone();
    }
    refuse(format!(
        "the saved instances {context}: {}",
        plain_key(&first.message)
    ))
}

fn unloadable(found: &[Diagnostic]) -> Diagnostic {
    not_loadable(found, "would not load")
}

fn would_not_load(found: &[Diagnostic]) -> Diagnostic {
    not_loadable(found, "would not load against the current prefab files")
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use ludion_core::math::{Trs, euler_xyz_degrees_to_quat};

    use super::{
        leaf_pointers, local_under_placement, override_pointers, placement_for, plain_key,
        split_version, versioned,
    };

    #[test]
    fn a_versioned_key_keeps_its_directory_and_reads_back() {
        let key = versioned(3, "prefabs/town/a.prefab.yaml");
        assert_eq!(key, "prefabs/town/a.prefab.yaml@3");
        assert_eq!(split_version(&key), Some((3, "prefabs/town/a.prefab.yaml")));
        assert_eq!(split_version("prefabs/a.prefab.yaml"), None);
        // The suffix leaves the directory real, so a prefab-relative path resolves as on disk.
        assert_eq!(
            ludion_core::paths::sibling(&key, "../../../assets/x.mesh.yaml"),
            "../assets/x.mesh.yaml"
        );
    }

    #[test]
    fn a_nested_entry_written_in_a_versioned_prefab_composes_to_the_real_key() {
        // `namespace` appends the version to the raw relative path; the rebase then resolves it
        // against the outer prefab's directory.
        let nested = versioned(3, "b.prefab.yaml");
        let rebased = ludion_core::paths::sibling("prefabs/a.prefab.yaml@3", &nested);
        assert_eq!(rebased, "prefabs/b.prefab.yaml@3");
        assert_eq!(split_version(&rebased), Some((3, "prefabs/b.prefab.yaml")));
    }

    #[test]
    fn plain_key_strips_the_version_suffix_only() {
        assert_eq!(
            plain_key("in `prefabs/a.prefab.yaml@3`: bad; mail me@3b"),
            "in `prefabs/a.prefab.yaml`: bad; mail me@3b"
        );
        assert_eq!(
            plain_key("prefabs/a.prefab.yaml@12"),
            "prefabs/a.prefab.yaml"
        );
        assert_eq!(plain_key("a@b"), "a@b");
    }

    fn random_poses(count: usize, seed: u32) -> Vec<[Trs; 3]> {
        let mut state = seed;
        let mut next = move |lo: f32, hi: f32| {
            state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            lo + (hi - lo) * ((state >> 8) as f32 / (1u32 << 24) as f32)
        };
        let mut pose = move || Trs {
            translation: [next(-9.0, 9.0), next(-9.0, 9.0), next(-9.0, 9.0)],
            rotation: euler_xyz_degrees_to_quat([
                next(-180.0, 180.0),
                next(-180.0, 180.0),
                next(-180.0, 180.0),
            ]),
            scale: [next(0.25, 4.0), next(0.25, 4.0), next(0.25, 4.0)],
        };
        (0..count).map(|_| [pose(), pose(), pose()]).collect()
    }

    fn near(a: [f32; 3], b: [f32; 3]) -> bool {
        (0..3).all(|i| (a[i] - b[i]).abs() < 1e-3)
    }

    #[test]
    fn a_recovered_placement_lands_the_anchor_on_its_live_translation() {
        for [live, here, _] in random_poses(500, 0xC0FF_EE11) {
            let t = placement_for(&live, &here).mul_trs(&here).translation;
            assert!(near(t, live.translation), "{live:?} {here:?} -> {t:?}");
        }
    }

    #[test]
    fn a_part_override_lands_the_part_on_its_wanted_translation() {
        for [own, here, wanted] in random_poses(500, 0xBADC_0DE5) {
            // The placement the build applied: it takes `own` to `here`.
            let placement = here.parent_for(&own);
            let local = local_under_placement(&own, &here, &wanted);
            let t = placement.mul_trs(&local).translation;
            assert!(
                near(t, wanted.translation),
                "{own:?} {here:?} {wanted:?} -> {t:?}"
            );
        }
    }

    #[test]
    fn a_gained_component_is_one_pointer() {
        let components = vec![
            ("Gained".to_owned(), json!({"a": 1, "b": 2})),
            ("Held".to_owned(), json!({"a": 1, "b": 2})),
        ];
        assert_eq!(
            override_pointers(&components, &["Gained".to_owned()]),
            ["/Gained", "/Held/a", "/Held/b"]
        );
    }

    #[test]
    fn leaves_are_scalars_arrays_and_empty_objects() {
        let value = json!({"a": {"b": 1, "c": [1, 2]}, "d": {}, "e": null});
        assert_eq!(
            leaf_pointers("Comp", &value),
            ["/Comp/a/b", "/Comp/a/c", "/Comp/d", "/Comp/e"]
        );
        assert_eq!(leaf_pointers("Comp", &json!({})), ["/Comp"]);
        assert_eq!(leaf_pointers("Comp", &json!(3)), ["/Comp"]);
    }

    #[test]
    fn keys_are_escaped_as_rfc_6901() {
        let value = json!({"a/b": {"c~d": 1}});
        assert_eq!(leaf_pointers("Comp", &value), ["/Comp/a~1b/c~0d"]);
    }
}
