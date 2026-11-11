//! Runtime room assembly: [`spawn_map`] stamps an authored `kind: map` into the live world as one
//! prefab-style instance, with a kit, seed and mirror the caller picks.
//!
//! The map, its kit and its fragments are read through the [`PrefabLibrary`]'s own text store, so the
//! recording and the restart carry them as they carry prefab texts: a replay needs no new record. A
//! compile is the pure `ludion_scene::generate` followed by the committed-prefab route (emit, then
//! the prefab loader), so a runtime room is entity for entity the `generate-map --prefab` file.

use std::collections::BTreeSet;
use std::path::Path;
use std::sync::Arc;

use ludion_core::codes::code;
use ludion_core::diagnostics::Diagnostic;
use ludion_core::ids::EntityId;
use ludion_ecs::World;
use ludion_scene::{MapOverrides, Mirror, PrefabDocument, Scene};

use super::{PrefabLibrary, Ready, spawn_expanded};
use crate::components::Transform;
use crate::skin_bone::apply_bone_poses;

/// A room's cache key: map, kit override (a content-root key), seed override and mirror.
pub(super) type RoomKey = (String, Option<String>, Option<u64>, Mirror);

/// What a map-side text is, by its `kind:`.
#[derive(Clone, Copy)]
pub(super) enum MapText {
    Map,
    Kit,
    /// A fragment: a plain scene file.
    Fragment,
}

impl MapText {
    /// The kind of `source` when it is a map, a kit or a scene; `None` for anything else (a prefab).
    pub(super) fn of(source: &str) -> Option<Self> {
        match ludion_scene::peek_kind_checked(source).ok()?.as_deref() {
            Some("map") => Some(Self::Map),
            Some("map-kit") => Some(Self::Kit),
            Some("scene") | None => Some(Self::Fragment),
            Some(_) => None,
        }
    }

    /// Parses `source` as this kind, for the diagnostics only.
    fn check(self, source: &str) -> Result<(), Vec<Diagnostic>> {
        match self {
            Self::Map => ludion_scene::parse_map(source).map(drop),
            Self::Kit => ludion_scene::parse_map_kit(source).map(drop),
            Self::Fragment => ludion_scene::parse(source).map(drop),
        }
    }
}

/// How [`spawn_map`] stamps a map. The default is the map as authored, spawned where it was
/// authored, with bare part names.
#[derive(Clone, Debug, Default)]
pub struct MapSpawn<'a> {
    /// Replaces the map's `kit:`: a `kind: map-kit` path relative to the content root, like the map's
    /// own path (not relative to the map file).
    pub kit: Option<&'a str>,
    /// Replaces the map's `seed:`, which picks its variants and rotations.
    pub seed: Option<u64>,
    /// Reflects the compiled grid and markers; a map carrying `fragments:` refuses it (`L1399`).
    pub mirror: Mirror,
    /// Baked onto every root part as `placement ∘ local`; `None` leaves the poses as authored.
    pub placement: Option<Transform>,
    /// Prefixes every part's [`Name`](crate::Name) as `name/part`.
    pub name: Option<&'a str>,
}

fn invalid(message: impl Into<String>) -> Diagnostic {
    Diagnostic::new(code::MAP_SPAWN_INVALID, message)
}

fn name_the_file(key: &str, diagnostics: Vec<Diagnostic>) -> Vec<Diagnostic> {
    diagnostics
        .into_iter()
        .map(|mut diagnostic| {
            diagnostic.message = format!("in `{key}`: {}", diagnostic.message);
            diagnostic
        })
        .collect()
}

/// The canonical content-root key of `path`, `L1406` when it is absolute or leaves the root.
fn content_key(path: &str, what: &str) -> Result<String, Diagnostic> {
    match ludion_core::paths::content_refusal(path) {
        Some(refusal) => Err(invalid(format!(
            "cannot use {what} `{path}`: the path {}",
            refusal.describe()
        ))
        .with_help(
            "name it relative to the content root, e.g. `rooms/crypt.map.yaml` or \
             `kits/ossuary.kit.yaml`",
        )),
        None => Ok(ludion_scene::normalize_prefab_path(path)),
    }
}

/// The name of `root` when it plays a clip no room can have: an `Animation` with a clip and no
/// assembly or character `Mesh` for the clip to belong to.
fn animating_root(root: &ludion_scene::SceneEntity) -> Option<String> {
    let component = |name: &str| {
        root.components
            .iter()
            .find(|(held, _)| held == name)
            .map(|(_, value)| value)
    };
    let clip = component("Animation")?.get("clip")?.as_str()?;
    let drives_a_model = component("Mesh")
        .and_then(|mesh| mesh.get("shape")?.as_object())
        .is_some_and(|shape| shape.contains_key("assembly") || shape.contains_key("character"));
    (!clip.is_empty() && !drives_a_model).then(|| root.name.clone().unwrap_or_default())
}

/// Whether the instance source `key` is a runtime room's map rather than a prefab: a room compiled
/// from it, else its held text (or, with none held, the file at `root`) is a `kind: map`.
pub(crate) fn is_room_source(library: Option<&PrefabLibrary>, root: &Path, key: &str) -> bool {
    let sniff = |text: &str| matches!(MapText::of(text), Some(MapText::Map));
    if let Some(library) = library {
        if library.rooms.keys().any(|room| room.0 == key) {
            return true;
        }
        if let Some(held) = library.texts.get(key) {
            return sniff(held);
        }
    }
    std::fs::read_to_string(root.join(key)).is_ok_and(|text| sniff(&text))
}

impl PrefabLibrary {
    /// The text of the map-side file `key`: held, else read (disk, or under replay the recording)
    /// and held and logged like a prefab read. Every refusal is `L1406`, and a failed read stays
    /// failed until a reload of `key`, so a replay (which cannot see a file appear) agrees.
    fn map_text(&mut self, key: &str) -> Result<String, Diagnostic> {
        if let Some(held) = self.texts.get(key) {
            return Ok(held.clone());
        }
        if let Some(failed) = self.unread.get(key) {
            return Err(failed.clone());
        }
        let read = match self.source.as_deref() {
            Some(source) => source
                .get(&(self.epoch, key.to_owned()))
                .cloned()
                .ok_or_else(|| {
                    invalid(format!("`{key}` is not recorded in this replay")).with_help(
                        "the recording holds the map, kit and fragment texts its session read",
                    )
                }),
            None => std::fs::read_to_string(self.root.join(key)).map_err(|found| {
                invalid(format!("cannot read `{key}`: {found}"))
                    .with_help("the path is relative to the content root")
            }),
        };
        let text = read.inspect_err(|failed| {
            self.unread.insert(key.to_owned(), failed.clone());
        })?;
        self.hold(key, &text);
        self.log
            .push((self.frame, key.to_owned(), text.clone(), false));
        Ok(text)
    }

    /// The room `map_key` compiles to under the overrides, compiling it on first use. A failed
    /// compile is cached as its diagnostics.
    pub(crate) fn ready_map(
        &mut self,
        map_key: &str,
        kit_key: Option<&str>,
        seed: Option<u64>,
        mirror: Mirror,
    ) -> Result<Arc<Ready>, Vec<Diagnostic>> {
        let room = (map_key.to_owned(), kit_key.map(str::to_owned), seed, mirror);
        if !self.rooms.contains_key(&room) {
            let mut deps = BTreeSet::new();
            let compiled = self.compile_room(map_key, kit_key, seed, mirror, &mut deps);
            self.room_deps.insert(room.clone(), deps);
            self.rooms.insert(room.clone(), compiled);
        }
        self.rooms[&room].clone()
    }

    /// Reads the map, its kit and its fragments, generates the scene and loads it as the prefab
    /// at `map_key` (the generated paths are relative to the map, which is where the prefab sits).
    fn compile_room(
        &mut self,
        map_key: &str,
        kit_override: Option<&str>,
        seed: Option<u64>,
        mirror: Mirror,
        deps: &mut BTreeSet<String>,
    ) -> Result<Arc<Ready>, Vec<Diagnostic>> {
        deps.insert(map_key.to_owned());
        let text = self.typed_text(map_key, "kind: map", "map")?;
        let mut doc =
            ludion_scene::parse_map(&text).map_err(|found| name_the_file(map_key, found))?;
        if doc.physics.is_some() {
            return Err(vec![
                invalid(format!(
                    "cannot spawn map `{map_key}`: it carries a `physics:` block"
                ))
                .with_path("/physics")
                .with_help(
                    "the scene that spawns the room owns physics; drop `physics:` from this map",
                ),
            ]);
        }
        MapOverrides {
            kit: None,
            seed,
            mirror,
        }
        .apply(&mut doc);
        doc.source = Some(map_key.rsplit('/').next().unwrap_or(map_key).to_owned());

        let mut diagnostics = Vec::new();
        let kit_key = match kit_override {
            Some(key) => Ok(key.to_owned()),
            None => content_key(&ludion_core::paths::sibling(map_key, &doc.kit), "kit"),
        };
        let kit = kit_key.map_err(|found| vec![found]).and_then(|key| {
            deps.insert(key.clone());
            let text = self.typed_text(&key, "kind: map-kit", "map-kit")?;
            ludion_scene::parse_map_kit(&text).map_err(|found| name_the_file(&key, found))
        });
        let kit = kit.map_err(|mut found| diagnostics.append(&mut found)).ok();
        let mut fragments = Vec::with_capacity(doc.fragments.len());
        for fragment in &doc.fragments {
            let loaded = content_key(&ludion_core::paths::sibling(map_key, fragment), "fragment")
                .map_err(|found| vec![found])
                .and_then(|key| {
                    deps.insert(key.clone());
                    let text = self.map_text(&key).map_err(|found| vec![found])?;
                    ludion_scene::parse(&text).map_err(|found| name_the_file(&key, found))
                });
            match loaded {
                Ok(scene) => fragments.push((fragment.clone(), scene)),
                Err(mut found) => diagnostics.append(&mut found),
            }
        }
        let Some(kit) = kit.filter(|_| diagnostics.is_empty()) else {
            return Err(diagnostics);
        };

        let scene: Scene = ludion_scene::generate(&doc, &kit, &fragments, &self.registry)
            .map_err(|found| name_the_file(map_key, found))?;
        if let Some(root) = scene.entities.first()
            && let Some(part) = animating_root(root)
        {
            return Err(vec![
                invalid(format!(
                    "cannot spawn map `{map_key}`: its root part `{part}` carries an `Animation`, but a runtime room has no clips"
                ))
                .with_help(
                    "put the animation on a part below the root, or on a mesh that is an assembly or a character",
                ),
            ]);
        }
        let prefab = PrefabDocument {
            name: scene.name,
            generated_from: scene.generated_from,
            clips: Vec::new(),
            entities: scene.entities,
        };
        let prefab_text = ludion_scene::emit_prefab(&prefab, &self.registry);
        let mut nested = Vec::new();
        let loaded = self.load(map_key, &prefab_text, &mut nested, deps);
        self.record_nested(nested);
        loaded
    }

    /// The text of `key`, which must be a `want` document.
    fn typed_text(&mut self, key: &str, want: &str, what: &str) -> Result<String, Vec<Diagnostic>> {
        let text = self.map_text(key).map_err(|found| vec![found])?;
        let kind = ludion_scene::peek_kind_checked(&text)
            .map_err(|found| name_the_file(key, vec![found]))?;
        if kind.as_deref() != want.strip_prefix("kind: ") {
            return Err(vec![
                invalid(format!(
                    "`{key}` is not a {what}: it is {}, not `{want}`",
                    kind.map_or_else(
                        || "a document with no `kind:`".to_owned(),
                        |k| format!("`kind: {k}`")
                    )
                ))
                .with_help(format!(
                    "pass a `{want}` document, relative to the content root"
                )),
            ]);
        }
        Ok(text)
    }

    /// Warms what a scene's `preload:` names, whose text the load already held: a map compiles with
    /// its authored kit and seed, a kit is already read, anything else loads as a prefab.
    pub(crate) fn preload(&mut self, key: &str) -> Result<(), Vec<Diagnostic>> {
        match self.texts.get(key).and_then(|text| MapText::of(text)) {
            Some(MapText::Map) => self.ready_map(key, None, None, Mirror::None).map(drop),
            Some(MapText::Kit) => Ok(()),
            _ => self.ready(key).map(drop),
        }
    }

    /// Drops every cached room whose compile read or resolved `key`; each recompiles on its next spawn.
    pub(super) fn drop_rooms_reading(&mut self, key: &str) {
        let stale: Vec<RoomKey> = self
            .room_deps
            .iter()
            .filter(|(_, read)| read.contains(key))
            .map(|(room, _)| room.clone())
            .collect();
        for room in stale {
            self.rooms.remove(&room);
            self.room_deps.remove(&room);
        }
    }

    /// Accepts a reload of a map, kit or fragment text: parses it, holds it in a new snapshot, logs
    /// it as a reload and drops every room that read `key`, which recompiles on its next spawn.
    pub(super) fn replace_map_text(
        &mut self,
        key: &str,
        source: &str,
        kind: MapText,
    ) -> Result<(), Vec<Diagnostic>> {
        kind.check(source)
            .map_err(|found| name_the_file(key, found))?;
        self.texts.insert(key.to_owned(), source.to_owned());
        self.unread.remove(key);
        let frozen = self.snapshots.last().expect("a snapshot").clone();
        self.snapshots.push(frozen);
        self.snapshots
            .last_mut()
            .expect("a snapshot")
            .insert(key.to_owned(), source.to_owned());
        self.drop_rooms_reading(key);
        self.log
            .push((self.frame, key.to_owned(), source.to_owned(), true));
        Ok(())
    }
}

/// Stamps the `kind: map` at `map` (relative to the content root) into `world` as one instance and
/// returns its part ids in part order; `ids[0]` is the root, to pass to
/// [`despawn_instance`](crate::despawn_instance).
///
/// The room is what `ludion generate-map <map> --out room.prefab.yaml --prefab` (with the same
/// `--kit`, `--seed` and `--mirror`) writes, spawned with [`spawn_prefab`](crate::spawn_prefab):
/// all-or-nothing, a [`PrefabInstance`](crate::PrefabInstance) on every part whose `source` is the map's key, `placement`
/// baked onto the roots and `name` prefixing the part names. Game components in the map's markers are
/// copied verbatim under a mirror.
///
/// The map, kit and fragment texts are read once per path per world and recorded inline in a
/// recording, so a replay needs none of those files; the compiled room is cached per `(map, kit,
/// seed, mirror)`, so only the first spawn of a combination pays for the compile. `asset.reload` of a
/// map, kit or fragment the world holds drops the rooms that read it; live instances never change.
/// A scene's `preload:` entry naming a `*.map.yaml` (or `*.kit.yaml`) warms them at load. Sim parts
/// only: call it from a sim system.
///
/// # Errors
/// `L1406` if a path is absolute or leaves the content root, a file cannot be read, is not a `kind:
/// map` (or the kit not a `kind: map-kit`), the map carries `physics:`, or `world` has no prefab
/// library; `L1019`–`L1026` for a map or kit that does not compile; `L1399` for a mirror on a map
/// with fragments; the prefab codes (`L1226`–`L1229`, `L1221`/`L1222`, `L1186`/`L1187`,
/// `L1151`/`L1152`/`L1209`, component errors) for a room that fails the spawn-time passes;
/// `L0028` when the spawn crosses the phase boundary. Nothing is spawned on any error.
///
/// # Example
/// ```
/// use ludion_engine::{Engine, MapSpawn, Mirror, despawn_instance, spawn_map};
///
/// let root = std::env::temp_dir().join(format!("ludion-spawn-map-doc-{}", std::process::id()));
/// std::fs::create_dir_all(root.join("rooms")).unwrap();
/// std::fs::write(
///     root.join("rooms/ossuary.kit.yaml"),
///     "format: 1\nkind: map-kit\nname: k\ntiles:\n  \".\":\n    - name: floor\n      shape: cube\n",
/// )
/// .unwrap();
/// std::fs::write(
///     root.join("rooms/crypt.map.yaml"),
///     "format: 1\nkind: map\nname: crypt\nkit: ossuary.kit.yaml\ncell_size: 2.0\norigin:\n  - 0.0\n  - 0.0\nrows:\n  - \"..\"\n",
/// )
/// .unwrap();
///
/// let mut engine = Engine::builder().project_root(&root).build().unwrap();
/// let spawn = MapSpawn { seed: Some(7), mirror: Mirror::X, name: Some("f1-r04"), ..MapSpawn::default() };
/// let ids = spawn_map(engine.world_mut(), "rooms/crypt.map.yaml", spawn).unwrap();
/// assert_eq!(ids.len(), 2);
/// despawn_instance(engine.world_mut(), ids[0]).unwrap();
/// # std::fs::remove_dir_all(&root).ok();
/// ```
pub fn spawn_map(
    world: &mut World,
    map: &str,
    spawn: MapSpawn<'_>,
) -> Result<Vec<EntityId>, Vec<Diagnostic>> {
    let ready = ready_room(world, map, &spawn)?;
    let map_key = content_key(map, "map").map_err(|found| vec![found])?;
    let ids = spawn_expanded(
        world,
        &map_key,
        &ready.expanded,
        spawn.placement,
        spawn.name,
    )?;
    apply_bone_poses(world, &ids, &ready.poses);
    Ok(ids)
}

/// Compiles and caches the room [`spawn_map`] would stamp for `(map, spawn.kit, spawn.seed,
/// spawn.mirror)`, and spawns nothing: call it a few ticks before the door so the later
/// `spawn_map` of the same combination finds the room built. `spawn.placement` and `spawn.name`
/// are ignored (they do not key the cache).
///
/// It reads and records the map, kit and fragment texts exactly as a spawn would, so a recording
/// replays the same whichever call compiled first; the library is not hashed, so a prepare leaves
/// the `WorldHash` untouched. A failed compile is cached as its diagnostics, so the later spawn
/// returns the same errors.
///
/// # Errors
/// The compile errors of [`spawn_map`] (`L1406`, `L1019`–`L1026`, `L1399`); the spawn-time passes
/// run only when the room is stamped.
///
/// # Example
/// ```
/// use ludion_engine::{Engine, MapSpawn, prepare_map, spawn_map};
///
/// let root = std::env::temp_dir().join(format!("ludion-prepare-map-doc-{}", std::process::id()));
/// std::fs::create_dir_all(root.join("rooms")).unwrap();
/// std::fs::write(
///     root.join("rooms/ossuary.kit.yaml"),
///     "format: 1\nkind: map-kit\nname: k\ntiles:\n  \".\":\n    - name: floor\n      shape: cube\n",
/// )
/// .unwrap();
/// std::fs::write(
///     root.join("rooms/crypt.map.yaml"),
///     "format: 1\nkind: map\nname: crypt\nkit: ossuary.kit.yaml\ncell_size: 2.0\norigin:\n  - 0.0\n  - 0.0\nrows:\n  - \"..\"\n",
/// )
/// .unwrap();
///
/// let mut engine = Engine::builder().project_root(&root).build().unwrap();
/// let spawn = MapSpawn { seed: Some(7), ..MapSpawn::default() };
/// prepare_map(engine.world_mut(), "rooms/crypt.map.yaml", &spawn).unwrap();
/// let ids = spawn_map(engine.world_mut(), "rooms/crypt.map.yaml", spawn).unwrap();
/// assert_eq!(ids.len(), 2);
/// # std::fs::remove_dir_all(&root).ok();
/// ```
pub fn prepare_map(
    world: &mut World,
    map: &str,
    spawn: &MapSpawn<'_>,
) -> Result<(), Vec<Diagnostic>> {
    ready_room(world, map, spawn).map(drop)
}

/// The compiled room for `(map, spawn)`: path keys, the library, the cached compile.
fn ready_room(
    world: &mut World,
    map: &str,
    spawn: &MapSpawn<'_>,
) -> Result<Arc<Ready>, Vec<Diagnostic>> {
    let map_key = content_key(map, "map").map_err(|found| vec![found])?;
    let kit_key = spawn
        .kit
        .map(|kit| content_key(kit, "kit"))
        .transpose()
        .map_err(|found| vec![found])?;
    let Some(library) = world.resource_mut::<PrefabLibrary>() else {
        return Err(vec![
            invalid(format!(
                "cannot spawn map `{map}`: this world has no prefab library"
            ))
            .with_help("spawn maps from a system of an engine-built game"),
        ]);
    };
    library.ready_map(&map_key, kit_key.as_deref(), spawn.seed, spawn.mirror)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::components::builtin_registry;
    use ludion_core::time::FrameIndex;

    #[test]
    fn a_map_missing_from_a_replay_says_map_not_prefab() {
        let mut library = PrefabLibrary::new(
            std::path::PathBuf::from("unused"),
            Arc::new(builtin_registry()),
            None,
            FrameIndex::ZERO,
            Some(Arc::new(Default::default())),
            0,
        );
        let found = library.map_text("rooms/crypt.map.yaml").unwrap_err();
        assert!(
            found.message.contains("is not recorded in this replay"),
            "{}",
            found.message
        );
        assert!(!found.message.contains("prefab"), "{}", found.message);
        assert!(
            !found.help.as_deref().unwrap_or_default().contains("prefab"),
            "{found:?}"
        );
        assert_eq!(found.code, code::MAP_SPAWN_INVALID);
    }

    #[test]
    fn a_root_animation_is_refused_unless_a_model_mesh_owns_it() {
        use serde_json::json;
        let root = |mesh: Option<serde_json::Value>| ludion_scene::SceneEntity {
            name: Some("door".to_owned()),
            presentation: false,
            components: [("Animation".to_owned(), json!({ "clip": "open" }))]
                .into_iter()
                .chain(mesh.map(|shape| ("Mesh".to_owned(), json!({ "shape": shape }))))
                .collect(),
            instance: None,
        };
        assert_eq!(animating_root(&root(None)).as_deref(), Some("door"));
        assert!(animating_root(&root(Some(json!("cube")))).is_some());
        assert!(animating_root(&root(Some(json!({ "assembly": "a.assembly.yaml" })))).is_none());
        assert!(animating_root(&root(Some(json!({ "character": "c.character.yaml" })))).is_none());
    }
}
