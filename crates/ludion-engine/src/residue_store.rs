//! The [`ResidueStore`] resource: a read-only mirror of what every scene this session visited kept
//! behind, readable from whichever district the run is standing in.
//!
//! [`crate::transition`] owns residue's authority — the session map only the engine writes, updated
//! at the frame boundary when a switch commits. This is its readout. A system that wants to *show* a
//! sector's state before the run enters it has to read residue for a scene it is not in, and world
//! resources are wiped at every world reset, so the answer is the [`DocStore`](crate::DocStore) mold:
//! a per-world copy rebuilt from the engine-owned authority at every world (re)build.
//!
//! The one thing a game can write is [`ResidueStore::clear`] — a request drained at the frame
//! boundary, because a run restarting in place (no `scene.load`) must be able to forget the city.

use std::any::TypeId;

use ludion_core::collections::StableMap;
use ludion_core::time::FrameIndex;
use ludion_ecs::Registry;
use serde::de::DeserializeOwned;

use crate::transition::SceneResidue;

/// An ECS resource: what each scene the session has left kept behind, readable from anywhere.
///
/// **Read-only, and one frame behind nothing.** The engine installs a fresh copy of the session's
/// residue into every world it builds — construction, `scene.load`, `engine.restore`, and the scene
/// switch itself, where the reinstall happens *after* the departing scene's capture has been
/// committed, so the district you just left already reads back from the world you just arrived in.
/// Like [`SceneSwitch`](crate::SceneSwitch) and [`AudioQueue`](crate::AudioQueue) it is a plain
/// resource: excluded from the `WorldHash` and from snapshots (the session map behind it is what a
/// snapshot carries), so it registers no component and trips no drift gate.
///
/// The scene keys are scene `name:`s — the same identity residue itself is keyed by, so a scene that
/// declares no `name:` never appears here.
///
/// [`clear`](ResidueStore::clear) is the only write, and it is a *request*: the engine drains it at
/// the next frame boundary, empties the session's map, and reinstalls this mirror emptied.
///
/// # Example
/// ```
/// use ludion_engine::{Engine, ResidueStore};
///
/// let engine = Engine::builder().build().unwrap();
/// let store = engine.world().resource::<ResidueStore>().unwrap();
/// // A session that has left no scene remembers nothing.
/// assert_eq!(store.scenes().count(), 0);
/// assert!(store.last_visit("district-a").is_none());
/// assert!(store.record_count("district-a").is_none());
/// ```
#[derive(Debug)]
pub struct ResidueStore {
    /// The session's residue map as it stood when this world was built.
    residue: StableMap<String, SceneResidue>,
    /// The registry's `(type, registered name)` pairs, so [`get`](ResidueStore::get) can resolve a
    /// component type to the name its bag entry is keyed by. A linear scan over the registered set
    /// (a few dozen entries) — no ordering is read off it, only equality on the type.
    names: Vec<(TypeId, String)>,
    /// Whether a system asked the session to forget every scene; drained at the frame boundary.
    clear_requested: bool,
}

impl ResidueStore {
    /// Copies the session's residue map into a fresh mirror, capturing the registry's component
    /// naming alongside it — the engine's `install_residue_store`.
    pub(crate) fn new(residue: &StableMap<String, SceneResidue>, registry: &Registry) -> Self {
        ResidueStore {
            residue: residue.clone(),
            names: registry
                .iter()
                .map(|info| (info.type_id(), info.name().to_string()))
                .collect(),
            clear_requested: false,
        }
    }

    /// Every scene the session has left something behind in, in ascending scene-name order.
    pub fn scenes(&self) -> impl Iterator<Item = &str> {
        self.residue.iter().map(|(scene, _)| scene.as_str())
    }

    /// The frame the run last left `scene` on, or `None` if it has never left it. The same value the
    /// engine evicts on, so two machines and a replay read one number.
    pub fn last_visit(&self, scene: &str) -> Option<FrameIndex> {
        self.residue
            .as_inner()
            .get(scene)
            .map(|residue| residue.last_visit)
    }

    /// How many records `scene` kept, or `None` if it kept nothing — the cheap readout for a sector
    /// summary that only needs a count.
    pub fn record_count(&self, scene: &str) -> Option<usize> {
        self.residue
            .as_inner()
            .get(scene)
            .map(|residue| residue.records.len())
    }

    /// The component `T` of the record named `name` in `scene`, as that scene kept it.
    ///
    /// `name` is the entity's `Name` — a record's identity, and the only handle a reader has on it
    /// (anonymous records are unaddressable by construction). `None` for a scene that kept nothing, a
    /// record that is not there, an entity that did not carry `T`, or a stored value that no longer
    /// deserializes into `T`: a read of another district's memory never panics and never fails a
    /// frame.
    pub fn get<T: DeserializeOwned + 'static>(&self, scene: &str, name: &str) -> Option<T> {
        let component = self.registered_name::<T>()?;
        let value = self
            .residue
            .as_inner()
            .get(scene)?
            .records
            .iter()
            .find(|record| record.name.as_deref() == Some(name))?
            .bag
            .iter()
            .find(|(stored, _)| stored == component)
            .map(|(_, value)| value)?;
        serde_json::from_value(value.clone()).ok()
    }

    /// Asks the engine to forget every scene's residue, honoured at the next frame boundary.
    ///
    /// The city-forgetting verb a run restarting **in place** needs: a `scene.load` resets the
    /// session for free, but a game that rebuilds its run without one (vigil's `reset_run`) would
    /// otherwise walk back into districts that remember the previous run. Last request wins; a clear
    /// requested in the same frame as a switch runs first.
    pub fn clear(&mut self) {
        self.clear_requested = true;
    }

    /// Takes the pending clear request, leaving none (the engine's drain path) — the
    /// [`SceneSwitch::take`](crate::SceneSwitch) mold.
    pub(crate) fn take_clear(&mut self) -> bool {
        std::mem::take(&mut self.clear_requested)
    }

    /// The registered component name for `T`, or `None` if `T` is not a registered component (a bag
    /// never holds one, so the read is simply empty).
    fn registered_name<T: 'static>(&self) -> Option<&str> {
        let wanted = TypeId::of::<T>();
        self.names
            .iter()
            .find(|(type_id, _)| *type_id == wanted)
            .map(|(_, name)| name.as_str())
    }
}
