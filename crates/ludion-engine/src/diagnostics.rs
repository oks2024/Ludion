//! The diagnostic channel game systems report through: the [`Diagnostics`] world resource and the
//! [`report`] free function a system calls with nothing but its `&mut World`.
//!
//! A system that notices something wrong — a data-doc naming a model that is not there, a spawn
//! point outside the map, a tunable read as zero — has one place to say so. It pushes a
//! `(code, key, message)` — sugar for a whole [`Diagnostic`], which [`report_full`] takes directly
//! when the fault has a path or help. The engine drains the queue at the frame boundary into an
//! `Event::Log` on the `log` channel *and* the host's stderr, rendering each diagnostic's `Display`
//! at its severity's level, so a client sees it and a headless or windowed run with no client still
//! prints it.
//!
//! Two properties make it safe to call from inside a system that runs every tick:
//!
//! - **Once per `(code, key)` per session.** The resource latches every pair it has already
//!   reported, so a system reporting the same fault sixty times a second prints one line. `key` is
//!   the offender's identity — a doc name, a scene name, or for an entity `offender_key`, never
//!   the id with its generation (a respawn takes a new generation) — so the same code still prints
//!   once for each distinct offender.
//! - **Never hashed, never serialized.** The resource is engine-owned world state like
//!   [`AudioQueue`](crate::AudioQueue): outside the component registry, so it reaches neither
//!   `WorldHash` nor a snapshot, and reporting cannot move a replay.
//!
//! The latch is moved onto the world a district switch builds, so "once per session" survives walking
//! through a gate. A `scene.load` or an `engine.restore` starts a fresh session with an empty latch,
//! so a reload of a still-broken scene reports again.

use ludion_core::codes::code;
use ludion_core::collections::{StableMap, StableSet};
use ludion_core::diagnostics::{Diagnostic, DiagnosticCode};
use ludion_core::ids::EntityId;
use ludion_ecs::{Registry, World, is_presentation};

use crate::components::Name;
use crate::prefab::PrefabInstance;

/// The engine-owned queue a game system reports diagnostics into, drained at the frame boundary.
///
/// Reach it from a system as `world.resource_mut::<Diagnostics>()`, or call the module's
/// [`report`] free function, which is the same thing in one line.
///
/// # Example
/// ```
/// use ludion_engine::diagnostics::{self, Diagnostics};
/// use ludion_core::diagnostics::DiagnosticCode;
/// use ludion_ecs::World;
///
/// const ASSET_FILE_IO: DiagnosticCode = DiagnosticCode::new("L1101");
///
/// let mut world = World::new();
/// world.insert_resource(Diagnostics::default());
/// diagnostics::report(
///     &mut world,
///     ASSET_FILE_IO,
///     "spawner-a",
///     "nest doc `spawner-a` names a model that is not in the content root",
/// );
/// ```
#[derive(Default, Debug)]
pub struct Diagnostics {
    /// Reported this frame, in report order, waiting for the boundary drain.
    pending: Vec<Diagnostic>,
    /// Every `(code, key)` already reported this session — the once-per-offender latch.
    /// Keyed by code so a probe by `&str` allocates nothing.
    latched: StableMap<DiagnosticCode, StableSet<String>>,
    /// The current scene's identity, prefixed onto an entity key so an offender in a new scene
    /// (whose ids restart at `e0v0`) cannot match an old scene's key.
    scope: String,
}

impl Diagnostics {
    /// Reports `message` under `code` for the offender named by `key`, unless that exact
    /// `(code, key)` has already been reported this session — in which case this is a no-op.
    ///
    /// `key` is the per-instance identity, not the message: two nests with the same fault report
    /// under the same code and different keys, and both print.
    pub fn report(
        &mut self,
        code: DiagnosticCode,
        key: impl Into<String>,
        message: impl Into<String>,
    ) {
        // The latch is consulted before the message is materialized, so a per-tick reporter pays
        // only for its key after the first time.
        if !self.latch(code, key.into()) {
            return;
        }
        self.pending.push(Diagnostic::new(code, message));
    }

    /// Reports a whole `diagnostic` (with its path, help and related) for the offender named by
    /// `key`, latched on `(diagnostic.code, key)` exactly like [`report`](Self::report).
    pub fn report_full(&mut self, key: impl Into<String>, diagnostic: Diagnostic) {
        if !self.latch(diagnostic.code, key.into()) {
            return;
        }
        self.pending.push(diagnostic);
    }

    /// Latches `(code, key)`; `false` when it already was.
    fn latch(&mut self, code: DiagnosticCode, key: String) -> bool {
        self.latched.entry(code).or_default().insert(key)
    }

    /// The diagnostics reported since the last boundary drain.
    #[cfg(test)]
    pub(crate) fn pending(&self) -> &[Diagnostic] {
        &self.pending
    }

    /// Whether `(code, key)` has already been reported this session. Allocation-free.
    pub(crate) fn is_latched(&self, code: DiagnosticCode, key: &str) -> bool {
        self.latched
            .get(&code)
            .is_some_and(|keys| keys.as_inner().contains(key))
    }

    /// The keys already reported this session under `code`, in set order.
    pub fn reported(&self, code: DiagnosticCode) -> impl Iterator<Item = &str> {
        self.latched
            .get(&code)
            .into_iter()
            .flat_map(|keys| keys.as_inner().iter().map(String::as_str))
    }

    /// Whether anything is waiting for the next boundary drain.
    pub(crate) fn is_empty(&self) -> bool {
        self.pending.is_empty()
    }

    /// Takes the pending queue, leaving the latch behind.
    pub(crate) fn take(&mut self) -> Vec<Diagnostic> {
        std::mem::take(&mut self.pending)
    }

    /// The resource the next world receives when a district switch rebuilds one: the latch moved over,
    /// the queue empty (the boundary drain has already emptied it, and a half-drained queue must never
    /// cross a world rebuild). A load or restore starts from `Diagnostics::default()` instead.
    pub(crate) fn carried(&mut self) -> Diagnostics {
        Diagnostics {
            pending: Vec::new(),
            latched: std::mem::take(&mut self.latched),
            scope: String::new(),
        }
    }

    /// Sets the scene identity `offender_key` prefixes onto entity keys.
    pub(crate) fn set_scope(&mut self, scope: String) {
        self.scope = scope;
    }
}

/// The latch key for a report about `id`, stable across respawns. First match wins:
///
/// - a [`PrefabInstance`] part: `prefab:{source}#{part}`, not scene-scoped — the fault belongs to
///   the prefab, so it reports once per `(code, source, part)` per session;
/// - a [`Name`]d entity: `{scope}/{name}`, so same-named entities of one scene share a line;
/// - anonymous: `{scope}#e{index}`, the slot index without its generation, so serial respawn in one
///   slot is one key while two live anonymous offenders stay distinct.
///
/// A presentation entity is not read (a sim-phase read of it is refused) and keys as anonymous.
pub(crate) fn offender_key(world: &World, id: EntityId) -> String {
    let scope = world
        .resource::<Diagnostics>()
        .map_or("", |diagnostics| diagnostics.scope.as_str());
    if !is_presentation(id) {
        if let Ok(Some(instance)) = world.get::<PrefabInstance>(id) {
            return format!("prefab:{}#{}", instance.source, instance.part);
        }
        if let Ok(Some(name)) = world.get::<Name>(id) {
            return format!("{scope}/{}", name.0);
        }
    }
    format!("{scope}#e{}", id.index())
}

/// Reports one diagnostic from a system that holds only `&mut World` — the one-line form of
/// [`Diagnostics::report`]. A no-op on a world with no [`Diagnostics`] resource (an engine builds
/// one into every world it owns; a bare `World` in a test may not have one).
pub fn report(
    world: &mut World,
    code: DiagnosticCode,
    key: impl Into<String>,
    message: impl Into<String>,
) {
    if let Some(diagnostics) = world.resource_mut::<Diagnostics>() {
        diagnostics.report(code, key, message);
    }
}

/// Whether [`report`] under `(code, key)` would be a no-op, so a per-tick reporter can skip building
/// its message.
pub(crate) fn is_latched(world: &World, code: DiagnosticCode, key: &str) -> bool {
    world
        .resource::<Diagnostics>()
        .is_none_or(|diagnostics| diagnostics.is_latched(code, key))
}

/// Reports a whole [`Diagnostic`] from a system that holds only `&mut World` — the one-line form of
/// [`Diagnostics::report_full`]. The log line's level follows the diagnostic's severity.
///
/// # Example
/// ```
/// use ludion_engine::diagnostics::{self, Diagnostics};
/// use ludion_core::diagnostics::{Diagnostic, DiagnosticCode};
/// use ludion_ecs::World;
///
/// let mut world = World::new();
/// world.insert_resource(Diagnostics::default());
/// let diagnostic = Diagnostic::new(DiagnosticCode::new("L1101"), "tunable `damage` reads as zero")
///     .with_path("weapon/crossbow/damage")
///     .with_help("set a positive damage in weapons/crossbow.yaml");
/// diagnostics::report_full(&mut world, "crossbow", diagnostic);
/// ```
pub fn report_full(world: &mut World, key: impl Into<String>, diagnostic: Diagnostic) {
    if let Some(diagnostics) = world.resource_mut::<Diagnostics>() {
        diagnostics.report_full(key, diagnostic);
    }
}

/// Reports the `L0028` refusals the world queued since the last drain (see
/// [`World::take_boundary_crossings`]), once per `"{system}/{op}/{Component}"` key per session.
/// Sorted by key: refusals from `par_for_each` chunks queue in nondeterministic order.
pub(crate) fn report_boundary_crossings(world: &mut World) {
    let mut crossings = world.take_boundary_crossings();
    crossings.sort_by(|a, b| a.key.cmp(&b.key));
    for crossing in crossings {
        report_full(world, crossing.key, crossing.diagnostic);
    }
}

/// Reports `L0027` for every component type that holds values but is missing from `registry`, once
/// per type per session. The hash, snapshots, saves and the wire all walk the registry, so such
/// values are invisible to them; only Rust code can create the storage, so a per-tick check sees it
/// before any later hash or save.
pub(crate) fn report_unregistered(world: &mut World, registry: &Registry) {
    let mut names: Vec<&'static str> = world
        .component_types()
        .filter(|(id, _)| {
            registry.get_by_type_id(*id).is_none()
                && *id != std::any::TypeId::of::<crate::NoInterpolate>()
        })
        .map(|(_, name)| name)
        .collect();
    if names.is_empty() {
        return;
    }
    // Storages are keyed by `TypeId`, whose order differs between builds.
    names.sort_unstable();
    for name in names {
        report(
            world,
            code::COMPONENT_NOT_REGISTERED,
            name,
            format!(
                "component type `{name}` has values in the world but is not registered, so the \
                 world hash, snapshots, saves and the wire cannot see it (register it with \
                 `EngineBuilder::components`, or keep it as a resource if it is not per-entity \
                 state)"
            ),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::{Diagnostics, report, report_full};
    use crate::{Engine, SceneSwitch, spawn_prefab};
    use ludion_core::diagnostics::{Diagnostic, DiagnosticCode};
    use ludion_core::ids::EntityId;
    use ludion_ecs::{SystemCtx, World};
    use ludion_protocol::command::EngineRestoreParams;
    use ludion_protocol::event::{Event, LogEvent, LogLevel};
    use ludion_protocol::{Command, ResponseBody};
    use serde_json::Value;
    use std::path::{Path, PathBuf};

    const NAGGY: DiagnosticCode = DiagnosticCode::new("L1101");

    const SCENE: &str = "\
format: 1
entities:
  - name: player
    components:
      Transform:
        translation:
          - 1.0
          - 2.0
          - 3.0
";

    const NEXT: &str = "\
format: 1
entities:
  - name: gate-from-start
    components:
      Transform:
        translation:
          - 4.0
          - 0.0
          - 0.0
";

    /// Reports the same `(code, key)` every single tick — the shape a per-frame content check has.
    fn nag(world: &mut World, _ctx: &SystemCtx) {
        report(world, NAGGY, "spawner-a", "the nest names no model");
    }

    /// Reports the same code for two different offenders, every tick.
    fn nag_twice(world: &mut World, _ctx: &SystemCtx) {
        report(world, NAGGY, "spawner-a", "the nest names no model");
        report(world, NAGGY, "spawner-b", "the nest names no model");
    }

    /// Requests a district switch on frame 2 (the `tests/transition.rs` gate mold), carrying nothing.
    fn switch_on_frame_2(world: &mut World, ctx: &SystemCtx) {
        if ctx.frame.as_u64() != 2 {
            return;
        }
        if let Some(switch) = world.resource_mut::<SceneSwitch>() {
            switch.request(
                "next.yaml".to_string(),
                "gate-from-start".to_string(),
                Vec::<EntityId>::new(),
            );
        }
    }

    /// The diagnostic-carrying log records drained since the last call.
    fn logged(engine: &mut Engine) -> Vec<LogEvent> {
        engine
            .take_events()
            .into_iter()
            .filter_map(|event| match event {
                Event::Log(log) if log.diagnostic.is_some() => Some(log),
                _ => None,
            })
            .collect()
    }

    fn code_of(log: &LogEvent) -> Option<&str> {
        log.diagnostic.as_ref().map(|d| d.code.as_str())
    }

    /// Reports a whole diagnostic with a path and help, once.
    fn report_whole(world: &mut World, _ctx: &SystemCtx) {
        report_full(world, "crossbow", whole_diagnostic());
    }

    fn whole_diagnostic() -> Diagnostic {
        Diagnostic::new(NAGGY, "tunable `damage` reads as zero")
            .with_path("weapon/x/damage")
            .with_help("set a positive damage")
    }

    /// A scratch directory removed when dropped, so a test run leaves nothing under the temp dir.
    struct ScratchDir(PathBuf);

    impl std::ops::Deref for ScratchDir {
        type Target = Path;
        fn deref(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for ScratchDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// Asks for a restart on every third frame.
    fn restart_every_third(world: &mut World, ctx: &SystemCtx) {
        if ctx.frame.as_u64() % 3 == 2
            && let Some(switch) = world.resource_mut::<SceneSwitch>()
        {
            switch.restart(Vec::<EntityId>::new());
        }
    }

    /// Advances until a `scene.changed` arrives and returns it.
    fn next_scene_changed(engine: &mut Engine) -> ludion_protocol::event::SceneChangedData {
        for _ in 0..10 {
            engine.advance_live();
            for event in engine.take_events() {
                if let Event::SceneChanged(data) = event {
                    return data;
                }
            }
        }
        panic!("no scene.changed within 10 frames");
    }

    #[test]
    fn a_restart_names_the_scene_it_rebuilt_when_it_knows_one() {
        let dir = fresh_dir("restart-event");
        let mut engine = Engine::builder()
            .system("restart_every_third", restart_every_third)
            .project_root(&*dir)
            .build()
            .expect("the test engine builds");
        engine.load_inline_scene(SCENE).expect("the scene loads");
        engine.take_events();
        let inline = next_scene_changed(&mut engine);
        assert_eq!(inline.kind, ludion_protocol::event::SceneChangeKind::Switch);
        assert_eq!(inline.path, None, "an inline scene has no path");

        engine
            .execute(ludion_protocol::Command::SceneLoad(
                ludion_protocol::command::SceneLoadParams {
                    path: "next.yaml".to_string(),
                },
            ))
            .expect("the scene loads");
        engine.take_events();
        let loaded = next_scene_changed(&mut engine);
        assert_eq!(loaded.path.as_deref(), Some("next.yaml"));
    }

    #[test]
    fn a_live_switch_leaves_the_target_as_the_save_base() {
        let dir = fresh_dir("switch-base");
        // Hand-written bytes the canonical writer would not reproduce.
        let hand = format!("# hand note\n{NEXT}");
        std::fs::write(dir.join("next.yaml"), &hand).unwrap();
        let mut engine = Engine::builder()
            .system("switch_on_frame_2", switch_on_frame_2)
            .project_root(&*dir)
            .build()
            .expect("the test engine builds");
        engine.load_inline_scene(SCENE).expect("the scene loads");
        for _ in 0..4 {
            engine.advance_live();
        }
        engine
            .execute(ludion_protocol::Command::SceneSave(
                ludion_protocol::command::SceneSaveParams {
                    path: "next.yaml".to_string(),
                    force: false,
                },
            ))
            .expect("the switch noted the base, so the save is no conflict");
        assert_ne!(
            std::fs::read_to_string(dir.join("next.yaml")).unwrap(),
            hand
        );
    }

    fn scene_load(engine: &mut Engine, path: &str) {
        engine
            .execute(Command::SceneLoad(
                ludion_protocol::command::SceneLoadParams {
                    path: path.to_string(),
                },
            ))
            .expect("the scene loads");
        engine.take_events();
    }

    fn scene_save(engine: &mut Engine, path: &str, force: bool) -> Result<(), Diagnostic> {
        engine
            .execute(Command::SceneSave(
                ludion_protocol::command::SceneSaveParams {
                    path: path.to_string(),
                    force,
                },
            ))
            .map(|_| ())
    }

    fn conflict(result: Result<(), Diagnostic>) -> bool {
        result.is_err_and(|diagnostic| diagnostic.code.as_str() == "L3058")
    }

    /// A first engine that writes its scene session to `dir/session.json` when `engine.snapshot` runs.
    fn first_engine(dir: &Path, system: fn(&mut World, &SystemCtx)) -> Engine {
        let mut engine = Engine::builder()
            .system("first", system)
            .project_root(dir)
            .build()
            .expect("the test engine builds");
        engine.set_scene_session_file(Some(dir.join("session.json")));
        engine
    }

    fn snapshot_of(engine: &mut Engine) -> Value {
        match engine.execute(Command::EngineSnapshot).unwrap().body {
            ResponseBody::Snapshot { snapshot } => snapshot,
            other => panic!("expected a snapshot, got {other:?}"),
        }
    }

    /// What a `--restore` boot does: a fresh engine restores `snapshot`, then adopts the session file.
    fn restored_engine(dir: &Path, snapshot: Value) -> Engine {
        let mut engine = Engine::builder()
            .system("restart_every_third", restart_every_third)
            .project_root(dir)
            .build()
            .expect("the test engine builds");
        engine
            .execute(Command::EngineRestore(EngineRestoreParams { snapshot }))
            .expect("the snapshot restores");
        engine.take_events();
        engine
    }

    fn adopt(engine: &mut Engine, dir: &Path) -> Result<(), Diagnostic> {
        engine.adopt_scene_session(&dir.join("session.json"))
    }

    fn idle(_world: &mut World, _ctx: &SystemCtx) {}

    #[test]
    fn a_save_after_the_snapshot_is_the_base_a_restart_adopts() {
        let dir = fresh_dir("session-save-window");
        std::fs::write(dir.join("next.yaml"), NEXT).unwrap();
        let mut first = first_engine(&dir, idle);
        scene_load(&mut first, "next.yaml");
        let snapshot = snapshot_of(&mut first);
        // Another client edits and saves while the child runs on, before dev kills it.
        first.world_mut().spawn();
        scene_save(&mut first, "next.yaml", true).expect("a forced save writes");
        let saved = std::fs::read_to_string(dir.join("next.yaml")).unwrap();

        let mut second = restored_engine(&dir, snapshot);
        adopt(&mut second, &dir).expect("the frames match");
        assert!(
            !conflict(scene_save(&mut second, "next.yaml", false)),
            "the file holds exactly what the session saved: {saved}"
        );
    }

    #[test]
    fn a_load_and_save_after_the_snapshot_keep_the_snapshots_path() {
        let dir = fresh_dir("session-save-other-path");
        std::fs::write(dir.join("next.yaml"), NEXT).unwrap();
        std::fs::write(dir.join("town.yaml"), SCENE).unwrap();
        let mut first = first_engine(&dir, idle);
        scene_load(&mut first, "next.yaml");
        let snapshot = snapshot_of(&mut first);
        // The window after the snapshot: another client switches scene and saves it.
        scene_load(&mut first, "town.yaml");
        scene_save(&mut first, "town.yaml", true).expect("a forced save writes");

        let mut second = restored_engine(&dir, snapshot);
        adopt(&mut second, &dir).expect("the frames match");
        let changed = next_scene_changed(&mut second);
        assert_eq!(
            changed.path.as_deref(),
            Some("next.yaml"),
            "the restored world is the snapshot's, so a restart names its path"
        );
    }

    #[test]
    fn a_restored_boot_adopts_the_session_path_and_bases() {
        let dir = fresh_dir("session-adopt");
        let hand = format!("# hand note\n{NEXT}");
        std::fs::write(dir.join("next.yaml"), &hand).unwrap();
        let mut first = first_engine(&dir, idle);
        scene_load(&mut first, "next.yaml");
        let snapshot = snapshot_of(&mut first);
        // The file changes by hand after the snapshot: the session never saw this text.
        let edited = format!("# edited\n{NEXT}");
        std::fs::write(dir.join("next.yaml"), &edited).unwrap();

        let mut second = restored_engine(&dir, snapshot);
        adopt(&mut second, &dir).expect("the frames match");
        assert!(conflict(scene_save(&mut second, "next.yaml", false)));
        assert_eq!(
            std::fs::read_to_string(dir.join("next.yaml")).unwrap(),
            edited
        );
        let changed = next_scene_changed(&mut second);
        assert_eq!(changed.path.as_deref(), Some("next.yaml"));

        // Without the hand edit the adopted base matches the disk and the save goes through.
        std::fs::write(dir.join("next.yaml"), &hand).unwrap();
        scene_save(&mut second, "next.yaml", false).expect("the base is the text on disk");
    }

    #[test]
    fn a_restored_boot_adopts_a_switched_scene_and_keeps_the_old_one_guarded() {
        let dir = fresh_dir("session-switch");
        std::fs::write(dir.join("town.yaml"), SCENE).unwrap();
        let mut first = first_engine(&dir, switch_on_frame_2);
        scene_load(&mut first, "town.yaml");
        for _ in 0..4 {
            first.advance_live();
        }
        first.take_events();
        let snapshot = snapshot_of(&mut first);
        std::fs::write(dir.join("town.yaml"), format!("# edited\n{SCENE}")).unwrap();

        let mut second = restored_engine(&dir, snapshot);
        adopt(&mut second, &dir).expect("the frames match");
        let changed = next_scene_changed(&mut second);
        assert_eq!(changed.path.as_deref(), Some("next.yaml"));
        scene_save(&mut second, "next.yaml", false).expect("the switch noted the base");
        assert!(conflict(scene_save(&mut second, "town.yaml", false)));
    }

    #[test]
    fn a_save_updates_the_base_a_restored_boot_adopts() {
        let dir = fresh_dir("session-saved");
        let mut first = first_engine(&dir, idle);
        scene_load(&mut first, "next.yaml");
        first
            .execute(Command::EntityDestroy(
                ludion_protocol::command::EntityDestroyParams {
                    entity: EntityId::new(0, 0),
                },
            ))
            .expect("the entity is destroyed");
        scene_save(&mut first, "next.yaml", false).expect("the save passes");
        let saved = std::fs::read_to_string(dir.join("next.yaml")).unwrap();
        let snapshot = snapshot_of(&mut first);
        // The restored world is the destroyed one, so only the adopted base lets this pass.
        let mut second = restored_engine(&dir, snapshot);
        std::fs::write(dir.join("next.yaml"), NEXT).unwrap();
        second.take_events();
        assert!(
            conflict(scene_save(&mut second, "next.yaml", false)),
            "the file on disk is not the saved text"
        );
        std::fs::write(dir.join("next.yaml"), &saved).unwrap();
        adopt(&mut second, &dir).expect("the frames match");
        scene_save(&mut second, "next.yaml", false).expect("the adopted base is the saved text");
    }

    #[test]
    fn a_session_that_does_not_match_leaves_the_path_unset_and_the_save_guarded() {
        let dir = fresh_dir("session-mismatch");
        let hand = format!("# hand note\n{NEXT}");
        std::fs::write(dir.join("next.yaml"), &hand).unwrap();
        let mut first = first_engine(&dir, idle);
        scene_load(&mut first, "next.yaml");
        let snapshot = snapshot_of(&mut first);

        let mut stale = restored_engine(&dir, snapshot.clone());
        stale.advance_live();
        let refused = adopt(&mut stale, &dir).expect_err("the frame moved");
        assert_eq!(refused.code.as_str(), "L0033");
        assert!(conflict(scene_save(&mut stale, "next.yaml", false)));
        scene_save(&mut stale, "next.yaml", true).expect("force skips the check");

        let mut missing = restored_engine(&dir, snapshot);
        let refused = missing
            .adopt_scene_session(&dir.join("absent.json"))
            .expect_err("no file");
        assert_eq!(refused.code.as_str(), "L0033");
        let changed = next_scene_changed(&mut missing);
        assert_eq!(changed.path, None);
    }

    #[test]
    fn a_session_written_for_another_scene_at_the_same_frame_is_refused() {
        let dir = fresh_dir("session-other-scene");
        std::fs::write(dir.join("a.yaml"), SCENE).unwrap();
        std::fs::write(dir.join("b.yaml"), NEXT).unwrap();
        let mut a = first_engine(&dir, idle);
        scene_load(&mut a, "a.yaml");
        let snapshot_a = snapshot_of(&mut a);
        // Another engine overwrites the sidecar: same frame, different scene.
        let mut b = first_engine(&dir, idle);
        scene_load(&mut b, "b.yaml");
        snapshot_of(&mut b);
        assert_eq!(a.frame(), b.frame());

        let mut restored = restored_engine(&dir, snapshot_a);
        let refused = adopt(&mut restored, &dir).expect_err("the sidecar is b's");
        assert_eq!(refused.code.as_str(), "L0033");
        assert!(refused.message.contains("scene"), "{}", refused.message);
        let changed = next_scene_changed(&mut restored);
        assert_eq!(changed.path, None);
        assert!(conflict(scene_save(&mut restored, "b.yaml", false)));
    }

    #[test]
    fn a_restore_and_an_inline_load_forget_the_scene_path() {
        let dir = fresh_dir("path-forgotten");
        let mut other = first_engine(&dir, idle);
        other.load_inline_scene(SCENE).expect("the scene loads");
        let snapshot = snapshot_of(&mut other);

        let mut restored = restored_engine(&dir, snapshot.clone());
        scene_load(&mut restored, "next.yaml");
        restored
            .execute(Command::EngineRestore(EngineRestoreParams { snapshot }))
            .expect("the snapshot restores");
        restored.take_events();
        assert_eq!(next_scene_changed(&mut restored).path, None);

        scene_load(&mut restored, "next.yaml");
        restored.load_inline_scene(SCENE).expect("the scene loads");
        restored.take_events();
        assert_eq!(next_scene_changed(&mut restored).path, None);
    }

    #[test]
    fn two_spellings_of_one_file_share_a_save_base() {
        let dir = fresh_dir("base-key");
        std::fs::write(dir.join("Town.yaml"), format!("# hand note\n{SCENE}")).unwrap();
        if !dir.join("town.yaml").exists() {
            return; // a case-sensitive file system: the two spellings are two files
        }
        let mut engine = first_engine(&dir, idle);
        scene_load(&mut engine, "Town.yaml");
        scene_save(&mut engine, "town.yaml", false).expect("one file, one base");
    }

    fn fresh_dir(name: &str) -> ScratchDir {
        let dir =
            std::env::temp_dir().join(format!("ludion-diagnostics-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("next.yaml"), NEXT).unwrap();
        ScratchDir(dir)
    }

    fn engine_with(system: fn(&mut World, &SystemCtx), root: &Path) -> Engine {
        let mut engine = Engine::builder()
            .system("nag", system)
            .project_root(root)
            .build()
            .expect("the test engine builds");
        engine.load_inline_scene(SCENE).expect("the scene loads");
        // The load itself queues events (a blanket loop stop); start the assertions from a clean slate.
        engine.take_events();
        engine
    }

    #[test]
    fn a_per_frame_report_reaches_the_log_exactly_once() {
        let dir = fresh_dir("once");
        let mut engine = engine_with(nag, &dir);
        engine.advance_live();
        engine.advance_live();

        let messages = logged(&mut engine);
        assert_eq!(
            messages.len(),
            1,
            "a system reporting every tick prints once per session: {messages:?}"
        );
        assert_eq!(code_of(&messages[0]), Some("L1101"));
        assert!(
            messages[0].message.contains("names no model"),
            "the log line renders the diagnostic: {}",
            messages[0].message
        );
        assert_eq!(
            messages[0].level,
            LogLevel::Error,
            "L1101 is an error-severity code, so it logs at Error"
        );
    }

    #[test]
    fn a_full_diagnostic_reaches_the_log_whole() {
        let dir = fresh_dir("whole");
        let mut engine = engine_with(report_whole, &dir);
        engine.advance_live();

        let messages = logged(&mut engine);
        assert_eq!(messages.len(), 1, "{messages:?}");
        let expected = whole_diagnostic();
        assert_eq!(messages[0].diagnostic, Some(expected.clone()));
        assert_eq!(messages[0].message, expected.to_string());
        assert_eq!(messages[0].level, LogLevel::Error);
        assert!(messages[0].message.contains("weapon/x/damage"));
        assert!(messages[0].message.contains("help: set a positive damage"));
    }

    #[test]
    fn two_keys_under_one_code_both_report() {
        let dir = fresh_dir("keys");
        let mut engine = engine_with(nag_twice, &dir);
        engine.advance_live();
        engine.advance_live();

        let messages = logged(&mut engine);
        assert_eq!(
            messages.len(),
            2,
            "the latch is per `(code, key)`, not per code: {messages:?}"
        );
    }

    #[test]
    fn a_silent_run_reports_nothing() {
        let dir = fresh_dir("silent");
        let mut engine = engine_with(|_, _| {}, &dir);
        engine.advance_live();
        engine.advance_live();

        assert!(
            logged(&mut engine).is_empty(),
            "nothing reported means nothing logged and nothing printed"
        );
    }

    /// Presentation: a discarded write to a sim entity (the `Result` is thrown away).
    fn trespass(world: &mut World, _ctx: &SystemCtx) {
        let Some((sim, _)) = world.query_ids::<(&crate::Transform,)>().next() else {
            return;
        };
        let _ = world.get_mut::<crate::Transform>(sim);
    }

    #[test]
    fn a_discarded_boundary_crossing_logs_one_l0028() {
        let dir = fresh_dir("crossing");
        let mut engine = Engine::builder()
            .presentation_system("trespass", trespass)
            .project_root(&*dir)
            .build()
            .expect("the test engine builds");
        engine.load_inline_scene(SCENE).expect("the scene loads");
        engine.take_events();
        for _ in 0..10 {
            engine.advance_live();
        }
        let logs = logged(&mut engine);
        assert_eq!(logs.len(), 1, "{logs:?}");
        assert_eq!(code_of(&logs[0]), Some("L0028"));
        assert!(logs[0].message.contains("trespass"), "{}", logs[0].message);
    }

    #[test]
    fn the_latch_survives_a_district_switch() {
        let dir = fresh_dir("switch");
        let mut engine = Engine::builder()
            .system("nag", nag)
            .system("switch_on_frame_2", switch_on_frame_2)
            .project_root(&*dir)
            .build()
            .expect("the test engine builds");
        engine.load_inline_scene(SCENE).expect("the scene loads");
        engine.take_events();

        engine.advance_live(); // frame 0 — reports
        engine.advance_live(); // frame 1
        assert_eq!(logged(&mut engine).len(), 1);

        engine.advance_live(); // frame 2 — the switch is performed at the boundary
        engine.advance_live(); // frame 3, in the new district
        engine.advance_live(); // frame 4
        assert!(
            engine.last_transition_error().is_none(),
            "the switch is clean: {:?}",
            engine.last_transition_error()
        );
        assert!(
            engine.world().resource::<Diagnostics>().is_some(),
            "the rebuilt district carries the resource"
        );
        assert!(
            logged(&mut engine).is_empty(),
            "the latch moved onto the new world, so the same fault stays quiet across the gate"
        );
    }

    #[test]
    fn a_scene_switch_pushes_a_scene_changed_naming_its_target() {
        let dir = fresh_dir("switch-event");
        let mut engine = Engine::builder()
            .system("switch_on_frame_2", switch_on_frame_2)
            .project_root(&*dir)
            .build()
            .expect("the test engine builds");
        engine.load_inline_scene(SCENE).expect("the scene loads");
        engine.take_events();

        for _ in 0..4 {
            engine.advance_live();
        }
        let changed: Vec<_> = engine
            .take_events()
            .into_iter()
            .filter_map(|event| match event {
                Event::SceneChanged(data) => Some(data),
                _ => None,
            })
            .collect();
        assert_eq!(changed.len(), 1, "one switch, one event: {changed:?}");
        assert_eq!(
            changed[0].kind,
            ludion_protocol::event::SceneChangeKind::Switch
        );
        assert_eq!(changed[0].path.as_deref(), Some("next.yaml"));
        assert_eq!(changed[0].request_id, None);
    }

    const FAULTY_PREFAB: &str = "\
format: 1
kind: prefab
name: rail
entities:
  - name: runner
    components:
      PathFollower:
        path: missing.path.yaml
";

    /// Logs of `code` drained since the last call.
    fn logged_code(engine: &mut Engine, code: &str) -> usize {
        logged(engine)
            .iter()
            .filter(|log| code_of(log) == Some(code))
            .count()
    }

    fn engine_with_prefab(name: &str) -> (ScratchDir, Engine) {
        let dir = fresh_dir(name);
        std::fs::write(dir.join("rail.prefab.yaml"), FAULTY_PREFAB).unwrap();
        let mut engine = Engine::builder()
            .project_root(&*dir)
            .build()
            .expect("the test engine builds");
        engine.load_inline_scene(SCENE).expect("the scene loads");
        engine.take_events();
        (dir, engine)
    }

    #[test]
    fn a_faulty_prefab_respawned_many_times_logs_one_line() {
        let (_dir, mut engine) = engine_with_prefab("prefab-respawn");
        for round in 0..25 {
            let ids = spawn_prefab(
                engine.world_mut(),
                "rail.prefab.yaml",
                None,
                Some(&format!("rail-{round}")),
                false,
            )
            .expect("the prefab spawns");
            engine.advance_live();
            for id in ids {
                engine.world_mut().despawn(id).unwrap();
            }
        }
        let count = logged_code(&mut engine, "L1215");
        let latched = engine
            .world()
            .resource::<Diagnostics>()
            .unwrap()
            .reported(DiagnosticCode::new("L1215"))
            .count();
        assert_eq!(count, 1, "25 respawns of one faulty part log one line");
        assert_eq!(latched, 1, "and the latch holds one key, not 25");
    }

    #[test]
    fn many_live_instances_of_a_faulty_prefab_log_one_line() {
        let (_dir, mut engine) = engine_with_prefab("prefab-live");
        for round in 0..10 {
            spawn_prefab(
                engine.world_mut(),
                "rail.prefab.yaml",
                None,
                Some(&format!("rail-{round}")),
                false,
            )
            .expect("the prefab spawns");
        }
        engine.advance_live();
        engine.advance_live();
        assert_eq!(logged_code(&mut engine, "L1215"), 1);
    }

    /// An anonymous `PathFollower` naming a missing path.
    fn spawn_faulty_anonymous(engine: &mut Engine) -> EntityId {
        let world = engine.world_mut();
        let id = world.spawn();
        world
            .insert(
                id,
                crate::PathFollower {
                    path: "missing.path.yaml".to_string(),
                    ..Default::default()
                },
            )
            .unwrap();
        id
    }

    #[test]
    fn two_live_anonymous_offenders_log_two_lines_and_respawning_adds_none() {
        let dir = fresh_dir("anonymous");
        let mut engine = engine_with(|_, _| {}, &dir);
        let first = spawn_faulty_anonymous(&mut engine);
        let second = spawn_faulty_anonymous(&mut engine);
        engine.advance_live();
        assert_eq!(logged_code(&mut engine, "L1215"), 2);

        for victim in [first, second] {
            engine.world_mut().despawn(victim).unwrap();
            spawn_faulty_anonymous(&mut engine);
            engine.advance_live();
        }
        assert_eq!(
            logged_code(&mut engine, "L1215"),
            0,
            "a respawn in a reported slot is the same offender"
        );
    }

    /// Switches to `next.yaml` on frame 2 and back to `first.yaml` on frame 5.
    fn switch_there_and_back(world: &mut World, ctx: &SystemCtx) {
        let target = match ctx.frame.as_u64() {
            2 => "next.yaml",
            5 => "first.yaml",
            _ => return,
        };
        if let Some(switch) = world.resource_mut::<SceneSwitch>() {
            switch.request(
                target.to_string(),
                "gate".to_string(),
                Vec::<EntityId>::new(),
            );
        }
    }

    #[test]
    fn an_anonymous_offender_in_a_second_scene_reports_and_returning_adds_none() {
        let dir = fresh_dir("scoped");
        let named = |name: &str| {
            format!(
                "format: 1\nname: {name}\nentities:\n  - name: gate\n    components:\n      Transform: {{}}\n"
            )
        };
        std::fs::write(dir.join("first.yaml"), named("first")).unwrap();
        std::fs::write(dir.join("next.yaml"), named("second")).unwrap();
        let mut engine = Engine::builder()
            .system("switch_there_and_back", switch_there_and_back)
            .project_root(&*dir)
            .build()
            .expect("the test engine builds");
        engine.load_inline_scene(&named("first")).expect("loads");
        engine.take_events();

        spawn_faulty_anonymous(&mut engine);
        engine.advance_live(); // frame 0
        engine.advance_live(); // frame 1
        assert_eq!(logged_code(&mut engine, "L1215"), 1, "first scene");

        engine.advance_live(); // frame 2: the switch to `second`
        engine.advance_live(); // frame 3
        assert!(engine.last_transition_error().is_none());
        spawn_faulty_anonymous(&mut engine);
        engine.advance_live(); // frame 4
        assert_eq!(
            logged_code(&mut engine, "L1215"),
            1,
            "the same slot in a new scene is a new offender"
        );

        engine.advance_live(); // frame 5: the switch back to `first`
        engine.advance_live(); // frame 6
        assert!(engine.last_transition_error().is_none());
        spawn_faulty_anonymous(&mut engine);
        engine.advance_live();
        assert_eq!(
            logged_code(&mut engine, "L1215"),
            0,
            "returning to a visited scene stays quiet"
        );
    }

    #[test]
    fn the_latch_resets_on_scene_load() {
        let dir = fresh_dir("load");
        let mut engine = engine_with(nag, &dir);
        engine.advance_live();
        assert_eq!(logged(&mut engine).len(), 1);

        engine.load_inline_scene(SCENE).expect("the scene reloads");
        engine.take_events();
        engine.advance_live();
        assert_eq!(
            logged(&mut engine).len(),
            1,
            "a reload is a fresh session, so a still-broken scene reports again"
        );
    }

    #[test]
    fn the_latch_resets_on_restore() {
        let dir = fresh_dir("restore");
        let mut engine = engine_with(nag, &dir);
        let snapshot = match engine.execute(Command::EngineSnapshot).unwrap().body {
            ResponseBody::Snapshot { snapshot } => snapshot,
            other => panic!("expected a snapshot, got {other:?}"),
        };
        engine.advance_live();
        assert_eq!(logged(&mut engine).len(), 1);

        engine
            .execute(Command::EngineRestore(EngineRestoreParams {
                snapshot: Value::clone(&snapshot),
            }))
            .expect("the snapshot restores");
        engine.take_events();
        engine.advance_live();
        assert_eq!(
            logged(&mut engine).len(),
            1,
            "a restore is a fresh session, so a still-broken world reports again"
        );
    }

    /// A component type no registry knows: only Rust code can put its values in the world.
    #[derive(serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
    struct DebugOnly(u32);

    /// Spawns an entity carrying an unregistered component every tick.
    fn stash_unregistered(world: &mut World, _ctx: &SystemCtx) {
        let entity = world.spawn();
        world.insert(entity, DebugOnly(7)).unwrap();
    }

    /// The same spawn without the unregistered component — the hash reference.
    fn spawn_only(world: &mut World, _ctx: &SystemCtx) {
        world.spawn();
    }

    #[test]
    fn an_unregistered_component_with_values_warns_once_and_never_moves_the_hash() {
        let dir = fresh_dir("unregistered");
        let mut stashing = engine_with(stash_unregistered, &dir);
        let mut reference = engine_with(spawn_only, &dir);
        for _ in 0..3 {
            stashing.advance_live();
            reference.advance_live();
        }

        let messages = logged(&mut stashing);
        assert_eq!(
            messages.len(),
            1,
            "one warning per type across several ticks: {messages:?}"
        );
        assert_eq!(code_of(&messages[0]), Some("L0027"));
        assert!(
            messages[0].message.contains("DebugOnly"),
            "the line names the Rust type: {}",
            messages[0].message
        );
        assert!(
            logged(&mut reference).is_empty(),
            "a world with only registered components reports nothing"
        );
        assert_eq!(
            stashing.world_hash().unwrap(),
            reference.world_hash().unwrap(),
            "the warning is a report, not a hash input: the unregistered values stay invisible"
        );
    }

    /// Spawns an entity with a NaN `Transform`.
    fn poison_transform(world: &mut World, _ctx: &SystemCtx) {
        let entity = world.spawn();
        let transform = crate::Transform {
            translation: [f32::NAN, 0.0, 0.0],
            ..Default::default()
        };
        world.insert(entity, transform).unwrap();
    }

    #[test]
    fn a_non_finite_float_in_a_hash_warns_once_per_component_and_emits_at_once() {
        let dir = fresh_dir("non-finite");
        let mut poisoned = engine_with(poison_transform, &dir);
        poisoned.advance_live();
        poisoned.take_events();
        let first = poisoned.world_hash().unwrap();
        // Emitted by the hash itself, with no further frame boundary.
        let messages = logged(&mut poisoned);
        assert_eq!(messages.len(), 1, "{messages:?}");
        assert_eq!(code_of(&messages[0]), Some("L4024"));
        assert!(
            messages[0].message.contains("`Transform`"),
            "{}",
            messages[0].message
        );
        assert_eq!(poisoned.world_hash().unwrap(), first);
        assert!(
            logged(&mut poisoned).is_empty(),
            "the latch holds per component type"
        );

        let mut finite = engine_with(|_, _| {}, &dir);
        finite.advance_live();
        finite.world_hash().unwrap();
        assert!(logged(&mut finite).is_empty());
    }

    #[test]
    fn reporting_never_moves_the_world_hash() {
        let dir = fresh_dir("hash");
        let mut reporting = engine_with(nag, &dir);
        let mut silent = engine_with(|_, _| {}, &dir);
        for _ in 0..3 {
            reporting.advance_live();
            silent.advance_live();
        }
        assert_eq!(
            reporting.world_hash().unwrap(),
            silent.world_hash().unwrap(),
            "the diagnostics resource is outside the registry, so it cannot reach a WorldHash"
        );
    }
}
