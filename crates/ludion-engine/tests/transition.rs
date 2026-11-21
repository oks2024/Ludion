//! In-run scene-transition tests (Phase 17 / E7, Wave 2): a game system sets the `SceneSwitch`
//! resource; the engine performs the switch at the frame boundary, rebuilding the world from the
//! target district while **continuing** the session — the frame counter keeps counting, the RNG
//! stream keeps flowing, the session logs and the in-memory profile carry, and an explicit carry set
//! (the player) travels, serialized out and respawned at a `Name`d anchor. Failures (`L3018` target
//! unloadable, `L3019` anchor missing, `L3029` anchor without a `Transform`) are transactional: the
//! live world is untouched and the sim runs on.

use ludion_core::ids::EntityId;
use ludion_core::rng::DeterministicRng;
use ludion_ecs::{Parent, SystemCtx, World};
use ludion_engine::{AudioQueue, Engine, EngineBuilder, Profile, SceneSwitch};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::{Path, PathBuf};

/// A required entity reference, for the carry remap.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Copy, Debug, PartialEq)]
struct Holds {
    #[schemars(with = "ludion_ecs::EntityRefSchema")]
    target: EntityId,
}

/// An optional entity reference, for the carry remap.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Copy, Debug, PartialEq)]
struct Eyes {
    #[schemars(with = "Option<ludion_ecs::EntityRefSchema>")]
    target: Option<EntityId>,
}

/// One element of a sanctioned reference list, its reference optional.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Copy, Debug, PartialEq)]
struct Mate {
    #[schemars(with = "Option<ludion_ecs::EntityRefSchema>")]
    who: Option<EntityId>,
}

/// A sanctioned list of optional references, for the carry remap.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq)]
struct Crew {
    #[schemars(with = "ludion_ecs::EntityRefCollection<Vec<Mate>>")]
    slots: Vec<Mate>,
}

/// One element of a sanctioned reference list, its reference required.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Copy, Debug, PartialEq)]
struct Post {
    #[schemars(with = "ludion_ecs::EntityRefSchema")]
    who: EntityId,
}

/// A sanctioned list of required references, for the carry remap.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq)]
struct Duties {
    #[schemars(with = "ludion_ecs::EntityRefCollection<Vec<Post>>")]
    posts: Vec<Post>,
}

/// The carried player's game state — never mutated after load, so it round-trips byte-equal.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Copy, Debug, PartialEq)]
struct Player {
    hp: u32,
    coins: u32,
}

/// A per-tick RNG probe: `draw_rng` stores the frame's `next_u32()` here, so the RNG stream reaches
/// the `WorldHash` and a test can read it off the carried entity.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Copy, Debug, PartialEq, Default)]
struct RngProbe {
    last: u32,
}

/// A tiny player profile: one unlock flag, set in-memory this run.
#[derive(Serialize, Deserialize, JsonSchema, Default, Clone, Debug, PartialEq)]
struct Save {
    unlocked: bool,
}

// --- Scenes -----------------------------------------------------------------------------------

/// District A (the start): seed 7, one named player carrying game state at a distinctive translation.
const START: &str = "\
format: 1
seed: 7
entities:
  - name: player
    components:
      Transform:
        translation:
          - 1.0
          - 2.0
          - 3.0
      Player:
        hp: 100
        coins: 5
      RngProbe:
        last: 0
";

/// District B (the target): seed 999 (deliberately different — a switch must ignore it), a
/// `gate-from-start` anchor at a known translation, and one native prop.
const NEXT: &str = "\
format: 1
seed: 999
entities:
  - name: gate-from-start
    components:
      Transform:
        translation:
          - 40.0
          - 0.0
          - -10.0
  - name: beacon
    components:
      Transform:
        translation:
          - 7.0
          - 0.0
          - 0.0
";

/// The anchor's translation in [`NEXT`] — where a carried entity lands.
const ANCHOR_XYZ: [f32; 3] = [40.0, 0.0, -10.0];

/// A target district whose anchor exists but carries no `Transform`, so it names no drop point →
/// `L3029`. The anchor is the same `Name` the happy path uses, so only the missing component differs.
const ANCHORLESS: &str = "\
format: 1
entities:
  - name: gate-from-start
    components:
      RngProbe:
        last: 0
";

/// A malformed target district (references an unregistered component) — fails validation → `L3018`.
const BAD: &str = "\
format: 1
entities:
  - components:
      NotARealComponent:
        x: 1
";

// --- Systems ----------------------------------------------------------------------------------

/// Draws one `u32` from the world RNG each tick and stores it on every [`RngProbe`], so the RNG
/// stream is hash-visible and readable off the carried player.
fn draw_rng(world: &mut World, _ctx: &SystemCtx) {
    let value = world
        .resource_mut::<DeterministicRng>()
        .map(DeterministicRng::next_u32)
        .unwrap_or(0);
    for (probe,) in world.query::<(&mut RngProbe,)>() {
        probe.last = value;
    }
}

/// On frame 0 only: push an audio event and set + save the profile in-memory — the session state that
/// must survive a later switch.
fn frame0_side_effects(world: &mut World, ctx: &SystemCtx) {
    if ctx.frame.as_u64() != 0 {
        return;
    }
    if let Some(queue) = world.resource_mut::<AudioQueue>() {
        queue.play("ding.wav", 1.0);
    }
    if let Some(profile) = world.resource_mut::<Profile<Save>>() {
        profile.get_mut().unlocked = true;
        profile.save();
    }
}

/// Requests a switch to `path`/`anchor` on frame 2, carrying the single `Player` entity — the gate
/// system every failure/happy variant shares.
fn request_switch(world: &mut World, ctx: &SystemCtx, path: &str, anchor: &str) {
    if ctx.frame.as_u64() != 2 {
        return;
    }
    let ids: Vec<EntityId> = world.entities().collect();
    let Some(player) = ids
        .into_iter()
        .find(|&id| matches!(world.get::<Player>(id), Ok(Some(_))))
    else {
        return;
    };
    if let Some(switch) = world.resource_mut::<SceneSwitch>() {
        switch.request(path.to_string(), anchor.to_string(), vec![player]);
    }
}

fn switch_to_next(world: &mut World, ctx: &SystemCtx) {
    request_switch(world, ctx, "next.yaml", "gate-from-start");
}

fn switch_bad_anchor(world: &mut World, ctx: &SystemCtx) {
    request_switch(world, ctx, "next.yaml", "no-such-anchor");
}

fn switch_missing_target(world: &mut World, ctx: &SystemCtx) {
    request_switch(world, ctx, "does-not-exist.yaml", "gate-from-start");
}

fn switch_malformed_target(world: &mut World, ctx: &SystemCtx) {
    request_switch(world, ctx, "bad.yaml", "gate-from-start");
}

fn switch_anchorless_target(world: &mut World, ctx: &SystemCtx) {
    request_switch(world, ctx, "anchorless.yaml", "gate-from-start");
}

/// On frame 2, requests a switch carrying the live player **plus** a fabricated (never-allocated) id
/// — the stale-`EntityId` game bug the `L3020` skip-and-report path guards.
fn switch_with_dead_carry(world: &mut World, ctx: &SystemCtx) {
    if ctx.frame.as_u64() != 2 {
        return;
    }
    let ids: Vec<EntityId> = world.entities().collect();
    let Some(player) = ids
        .into_iter()
        .find(|&id| matches!(world.get::<Player>(id), Ok(Some(_))))
    else {
        return;
    };
    let dead = EntityId::new(9999, 0);
    if let Some(switch) = world.resource_mut::<SceneSwitch>() {
        switch.request(
            "next.yaml".to_string(),
            "gate-from-start".to_string(),
            vec![player, dead],
        );
    }
}

// --- Harness ----------------------------------------------------------------------------------

fn fresh_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("ludion-transition-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    // Every district-switch test resolves target districts under the project root.
    std::fs::write(dir.join("next.yaml"), NEXT).unwrap();
    std::fs::write(dir.join("bad.yaml"), BAD).unwrap();
    std::fs::write(dir.join("anchorless.yaml"), ANCHORLESS).unwrap();
    dir
}

/// The base game: the built-in `draw_rng` + `frame0_side_effects`, `Player`/`RngProbe` components, and
/// a `Save` profile. A caller adds a switch system.
fn base_game(root: &Path) -> EngineBuilder {
    Engine::builder()
        .components(|reg| {
            reg.register::<Player>("Player")?;
            reg.register::<RngProbe>("RngProbe")?;
            reg.register::<Holds>("Holds")?;
            reg.register::<Eyes>("Eyes")?;
            reg.register::<Crew>("Crew")?;
            reg.register::<Duties>("Duties")?;
            Ok(())
        })
        .system("draw_rng", draw_rng)
        .system("frame0_side_effects", frame0_side_effects)
        .profile::<Save>()
        .project_root(root)
}

/// The `player` entity (the carried one), located by `Name`.
fn player_of(engine: &Engine) -> EntityId {
    engine
        .world()
        .entities()
        .find(|&id| {
            engine
                .world()
                .get::<ludion_engine::Name>(id)
                .ok()
                .flatten()
                .is_some_and(|name| name.0 == "player")
        })
        .expect("the player entity is present")
}

fn serialized(engine: &Engine, id: EntityId, component: &str) -> Option<Value> {
    engine
        .registry()
        .serialize(component, engine.world(), id)
        .unwrap()
}

// --- Tests ------------------------------------------------------------------------------------

#[test]
fn a_switch_rebuilds_into_the_target_district_carrying_the_player_to_the_anchor() {
    let dir = fresh_dir("happy");
    let mut engine = base_game(&dir)
        .system("switch_to_next", switch_to_next)
        .build()
        .expect("boots");
    engine.load_inline_scene(START).unwrap();

    // Capture the player's carried state BEFORE the switch (frame 1, pre-gate).
    engine.advance_live(); // frame 0
    engine.advance_live(); // frame 1
    let before_player = serialized(&engine, player_of(&engine), "Player");
    let before_name = serialized(&engine, player_of(&engine), "Name");

    // Frame 2 requests the switch; the drain performs it. Two more ticks run in district B.
    engine.advance_live(); // frame 2 -> switch performed, driver resumes at frame 3
    engine.advance_live(); // frame 3
    engine.advance_live(); // frame 4

    assert!(
        engine.last_transition_error().is_none(),
        "a valid switch reports no error: {:?}",
        engine.last_transition_error()
    );

    // The world is now district B: its natives (gate-from-start, beacon) plus the carried player.
    let names: Vec<String> = engine
        .world()
        .entities()
        .filter_map(|id| {
            engine
                .world()
                .get::<ludion_engine::Name>(id)
                .ok()
                .flatten()
                .map(|n| n.0.clone())
        })
        .collect();
    assert!(names.contains(&"gate-from-start".to_string()));
    assert!(names.contains(&"beacon".to_string()));
    assert!(names.contains(&"player".to_string()));
    assert_eq!(
        engine.world().entity_count(),
        3,
        "district B's two natives plus the one carried player"
    );

    // The carried player landed at the anchor's translation (translation only — decision 8).
    let player = player_of(&engine);
    let transform = serialized(&engine, player, "Transform").unwrap();
    let translation = &transform["translation"];
    assert_eq!(translation[0], ANCHOR_XYZ[0] as f64);
    assert_eq!(translation[1], ANCHOR_XYZ[1] as f64);
    assert_eq!(translation[2], ANCHOR_XYZ[2] as f64);

    // The carried game state is byte-equal across the switch (Player + Name unchanged).
    assert_eq!(
        serialized(&engine, player, "Player"),
        before_player,
        "the carried Player component round-trips byte-equal"
    );
    assert_eq!(
        serialized(&engine, player, "Name"),
        before_name,
        "the carried Name round-trips byte-equal"
    );
}

#[test]
fn the_frame_counter_and_logs_and_profile_continue_across_a_switch() {
    let dir = fresh_dir("continuity");
    let mut engine = base_game(&dir)
        .system("switch_to_next", switch_to_next)
        .build()
        .expect("boots");
    engine.load_inline_scene(START).unwrap();

    for _ in 0..5 {
        engine.advance_live();
    }

    // The run clock kept ticking — the switch did not reset the frame to 0.
    assert_eq!(
        engine.frame().as_u64(),
        5,
        "FrameIndex is continuous across the switch (5 ticks, no reset)"
    );

    // The session logs survive the switch (a switch is mid-session, unlike scene.load). The switch
    // adds one entry of its own: the blanket loop stop every world rebuild emits.
    assert_eq!(
        engine.audio_log().len(),
        2,
        "the frame-0 audio event survives the switch, which appends its blanket loop stop"
    );
    assert_eq!(engine.audio_log()[0].sound(), Some("ding.wav"));
    assert_eq!(
        engine.audio_log()[1].message,
        ludion_engine::AudioMessage::StopAllLoops
    );
    assert_eq!(
        engine.profile_save_log().len(),
        1,
        "the frame-0 profile-save entry survives the switch"
    );

    // The in-memory profile is carried (not re-read from the absent save/profile.yaml, which would
    // default to unlocked:false).
    assert!(
        engine
            .world()
            .resource::<Profile<Save>>()
            .unwrap()
            .get()
            .unlocked,
        "the mid-run unlock is carried across the district change"
    );
}

#[test]
fn the_rng_stream_continues_across_a_switch_and_ignores_the_target_seed() {
    let switch_dir = fresh_dir("rng-switch");
    let mut switching = base_game(&switch_dir)
        .system("switch_to_next", switch_to_next)
        .build()
        .expect("boots");
    switching.load_inline_scene(START).unwrap();
    for _ in 0..5 {
        switching.advance_live();
    }
    let switched = engine_probe(&switching);

    // Control A: a seed-7 run that never switches (one uninterrupted stream).
    let ctrl_dir = fresh_dir("rng-control-7");
    let mut control = base_game(&ctrl_dir).build().expect("boots");
    control.load_inline_scene(START).unwrap();
    for _ in 0..5 {
        control.advance_live();
    }
    let continuous = engine_probe(&control);

    // Control B: a seed-999 run (what a switch that WRONGLY honored the target seed would resemble).
    let reseed_dir = fresh_dir("rng-control-999");
    let start_999 = START.replace("seed: 7", "seed: 999");
    let mut reseeded_run = base_game(&reseed_dir).build().expect("boots");
    reseeded_run.load_inline_scene(&start_999).unwrap();
    for _ in 0..5 {
        reseeded_run.advance_live();
    }
    let reseeded = engine_probe(&reseeded_run);

    assert_ne!(
        continuous, reseeded,
        "seed 7 and seed 999 produce different RNG streams (the probe has teeth)"
    );
    assert_eq!(
        switched, continuous,
        "the switch carried the RNG stream forward — the target scene's seed: 999 is ignored"
    );
}

/// The `player`'s current [`RngProbe`] value.
fn engine_probe(engine: &Engine) -> u32 {
    engine
        .world()
        .get::<RngProbe>(player_of(engine))
        .ok()
        .flatten()
        .expect("the player carries an RngProbe")
        .last
}

#[test]
fn two_identical_drives_across_a_switch_produce_the_same_worldhash_trace() {
    let drive = |name: &str| -> Vec<u64> {
        let dir = fresh_dir(name);
        let mut engine = base_game(&dir)
            .system("switch_to_next", switch_to_next)
            .build()
            .expect("boots");
        engine.load_inline_scene(START).unwrap();
        let mut trace = Vec::new();
        for _ in 0..6 {
            engine.advance_live();
            trace.push(engine.world_hash().unwrap().as_u64());
        }
        trace
    };
    assert_eq!(
        drive("trace-a"),
        drive("trace-b"),
        "the switch is fully deterministic — two identical drives hash identically across the gate"
    );
}

#[test]
fn a_missing_anchor_rejects_the_switch_and_the_sim_continues() {
    let dir = fresh_dir("missing-anchor");
    let mut engine = base_game(&dir)
        .system("switch_bad_anchor", switch_bad_anchor)
        .build()
        .expect("boots");
    engine.load_inline_scene(START).unwrap();

    for _ in 0..5 {
        engine.advance_live();
    }

    // L3019, and the live world is untouched: still in district A with the player at its original
    // translation, and the frame counter kept advancing (the failed switch was a no-op on the world).
    let error = engine
        .last_transition_error()
        .expect("a missing anchor is reported");
    assert_eq!(error.code.as_str(), "L3019");
    assert_eq!(engine.frame().as_u64(), 5, "the sim continued");
    let player = player_of(&engine);
    let transform = serialized(&engine, player, "Transform").unwrap();
    assert_eq!(
        transform["translation"][0], 1.0,
        "the live world is untouched — the player never moved to any anchor"
    );
    assert_eq!(
        engine.world().entity_count(),
        1,
        "still just district A's single entity — no target district was swapped in"
    );
}

/// An anchor that exists but carries no `Transform` names no drop point, so it is refused exactly as
/// a missing one is. The alternative — the origin — is a plausible-looking place, so the run would
/// land somewhere real and the scene file that caused it would never be suspected.
#[test]
fn an_anchor_without_a_transform_rejects_the_switch() {
    let dir = fresh_dir("anchorless");
    let mut engine = base_game(&dir)
        .system("switch_anchorless_target", switch_anchorless_target)
        .build()
        .expect("boots");
    engine.load_inline_scene(START).unwrap();

    for _ in 0..5 {
        engine.advance_live();
    }

    let error = engine
        .last_transition_error()
        .expect("a transform-less anchor is reported");
    assert_eq!(error.code.as_str(), "L3029", "{error}");
    assert!(
        error.message.contains("gate-from-start"),
        "the rejection names the anchor: {error}"
    );
    let transform = serialized(&engine, player_of(&engine), "Transform").unwrap();
    assert_eq!(
        transform["translation"][0], 1.0,
        "the live world is untouched — the player never moved to the origin"
    );
    assert_eq!(
        engine.world().entity_count(),
        1,
        "no target district was swapped in"
    );
}

#[test]
fn a_dead_carry_id_is_skipped_and_reported_but_the_switch_succeeds() {
    let dir = fresh_dir("dead-carry");
    let mut engine = base_game(&dir)
        .system("switch_with_dead_carry", switch_with_dead_carry)
        .build()
        .expect("boots");
    engine.load_inline_scene(START).unwrap();

    for _ in 0..5 {
        engine.advance_live();
    }

    // The switch STILL happened: district B is present with the live player carried to the anchor.
    let names: Vec<String> = engine
        .world()
        .entities()
        .filter_map(|id| {
            engine
                .world()
                .get::<ludion_engine::Name>(id)
                .ok()
                .flatten()
                .map(|n| n.0.clone())
        })
        .collect();
    assert!(names.contains(&"gate-from-start".to_string()));
    assert!(names.contains(&"player".to_string()));
    assert_eq!(
        engine.world().entity_count(),
        3,
        "district B's two natives plus the one live carried player (the dead id carried nothing)"
    );
    let player = player_of(&engine);
    let transform = serialized(&engine, player, "Transform").unwrap();
    assert_eq!(transform["translation"][0], ANCHOR_XYZ[0] as f64);

    // ...but the dead carry id was reported (L3020, naming the skipped id), and the sim continues.
    let warning = engine
        .last_transition_error()
        .expect("the dead carry id is reported, not silently dropped");
    assert_eq!(warning.code.as_str(), "L3020");
    assert!(
        warning.message.contains("9999"),
        "the warning names the skipped id: {}",
        warning.message
    );
    assert_eq!(engine.frame().as_u64(), 5, "the sim continued");
}

#[test]
fn a_missing_target_file_rejects_the_switch_with_l3018() {
    let dir = fresh_dir("missing-target");
    let mut engine = base_game(&dir)
        .system("switch_missing_target", switch_missing_target)
        .build()
        .expect("boots");
    engine.load_inline_scene(START).unwrap();

    for _ in 0..5 {
        engine.advance_live();
    }

    let error = engine
        .last_transition_error()
        .expect("a missing target district is reported");
    assert_eq!(error.code.as_str(), "L3018");
    assert_eq!(engine.frame().as_u64(), 5, "the sim continued");
    assert_eq!(
        engine.world().entity_count(),
        1,
        "the live world is untouched"
    );
}

const NO_MESH_DISTRICT: &str = "\
format: 1
physics:
  space: free
entities:
  - name: gate-from-start
    components:
      Transform: {}
  - name: floor
    components:
      Transform: {}
      Collider:
        shape:
          mesh:
            path: gone.mesh.yaml
        body: static
";

fn switch_to_missing_collider_mesh(world: &mut World, ctx: &SystemCtx) {
    request_switch(world, ctx, "no-mesh.yaml", "gate-from-start");
}

/// The control's twin of [`switch_to_missing_collider_mesh`]: same system name and slot, no request.
fn never_switch(_world: &mut World, _ctx: &SystemCtx) {}

fn run_with_switch_system(name: &str, system: fn(&mut World, &SystemCtx)) -> (Engine, u64) {
    let dir = fresh_dir(name);
    std::fs::write(dir.join("no-mesh.yaml"), NO_MESH_DISTRICT).unwrap();
    let mut engine = base_game(&dir)
        .system("switch_request", system)
        .build()
        .expect("boots");
    engine.load_inline_scene(START).unwrap();
    for _ in 0..8 {
        engine.advance_live();
    }
    let hash = engine.world_hash().expect("the world hashes").as_u64();
    (engine, hash)
}

#[test]
fn a_switch_to_a_missing_collider_mesh_is_rejected_and_moves_no_hash() {
    let (refused, refused_hash) =
        run_with_switch_system("collider-mesh-refused", switch_to_missing_collider_mesh);
    let (_control, control_hash) = run_with_switch_system("collider-mesh-control", never_switch);

    let error = refused
        .last_transition_error()
        .expect("the refused district is reported");
    assert_eq!(error.code.as_str(), "L3018");
    assert!(
        error.related.iter().any(|d| d.code.as_str() == "L1212"),
        "the L1212 rides along: {error:?}"
    );
    assert_eq!(
        refused.frame().as_u64(),
        8,
        "the sim stayed in its district"
    );
    assert_eq!(refused.world().entity_count(), 1);
    assert_eq!(refused_hash, control_hash, "a refused switch moves no hash");
}

#[test]
fn a_malformed_target_file_rejects_the_switch_with_l3018() {
    let dir = fresh_dir("malformed-target");
    let mut engine = base_game(&dir)
        .system("switch_malformed_target", switch_malformed_target)
        .build()
        .expect("boots");
    engine.load_inline_scene(START).unwrap();

    for _ in 0..5 {
        engine.advance_live();
    }

    let error = engine
        .last_transition_error()
        .expect("a malformed target district is reported");
    assert_eq!(error.code.as_str(), "L3018");
    assert!(
        !error.related.is_empty(),
        "the underlying scene-validation diagnostic rides along as `related`"
    );
    assert_eq!(
        engine.world().entity_count(),
        1,
        "the live world is untouched"
    );
}

// --- Subtree carry ------------------------------------------------------------------------------

/// District A with a hierarchy: the player holds `held` (local offset), which holds `charm`; `rider`
/// sits under a scaled `mount` that stays behind unless carried.
const START_TREE: &str = "\
format: 1
seed: 7
entities:
  - name: player
    components:
      Transform:
        translation:
          - 1.0
          - 2.0
          - 3.0
      Player:
        hp: 100
        coins: 5
      RngProbe:
        last: 0
  - name: held
    components:
      Parent:
        index: 0
        generation: 0
      Transform:
        translation:
          - 0.0
          - 1.0
          - 0.0
  - name: charm
    components:
      Parent:
        index: 1
        generation: 0
      Transform:
        translation:
          - 0.0
          - 0.5
          - 0.0
  - name: mount
    components:
      Transform:
        translation:
          - 10.0
          - 0.0
          - 0.0
        scale:
          - 2.0
          - 2.0
          - 2.0
  - name: rider
    components:
      Parent:
        index: 3
        generation: 0
      Transform:
        translation:
          - 0.0
          - 1.0
          - 0.0
";

/// On frame 2, requests the switch to `next.yaml` carrying the entities named `names`.
fn request_carry(world: &mut World, ctx: &SystemCtx, names: &[&str]) {
    if ctx.frame.as_u64() != 2 {
        return;
    }
    let carry: Vec<EntityId> = names
        .iter()
        .map(|wanted| {
            world
                .entities()
                .find(|&id| {
                    world
                        .get::<ludion_engine::Name>(id)
                        .ok()
                        .flatten()
                        .is_some_and(|name| name.0 == *wanted)
                })
                .expect("the carried entity exists")
        })
        .collect();
    if let Some(switch) = world.resource_mut::<SceneSwitch>() {
        switch.request(
            "next.yaml".to_string(),
            "gate-from-start".to_string(),
            carry,
        );
    }
}

fn switch_player(world: &mut World, ctx: &SystemCtx) {
    request_carry(world, ctx, &["player"]);
}

fn switch_rider(world: &mut World, ctx: &SystemCtx) {
    request_carry(world, ctx, &["rider"]);
}

fn switch_player_and_held(world: &mut World, ctx: &SystemCtx) {
    request_carry(world, ctx, &["player", "held"]);
}

fn named_of(engine: &Engine, wanted: &str) -> EntityId {
    engine
        .world()
        .entities()
        .find(|&id| {
            engine
                .world()
                .get::<ludion_engine::Name>(id)
                .ok()
                .flatten()
                .is_some_and(|name| name.0 == wanted)
        })
        .unwrap_or_else(|| panic!("`{wanted}` is present"))
}

fn parent_of(engine: &Engine, id: EntityId) -> Option<EntityId> {
    engine
        .world()
        .get::<Parent>(id)
        .ok()
        .flatten()
        .map(|Parent(parent)| *parent)
}

fn tree_engine(dir: &Path, system: fn(&mut World, &SystemCtx)) -> Engine {
    let mut engine = base_game(dir)
        .system("switch", system)
        .build()
        .expect("boots");
    engine.load_inline_scene(START_TREE).unwrap();
    for _ in 0..5 {
        engine.advance_live();
    }
    engine
}

#[test]
fn a_switch_carries_the_whole_subtree_and_anchors_only_the_root() {
    let dir = fresh_dir("subtree");
    let engine = tree_engine(&dir, switch_player);
    assert!(
        engine.last_transition_error().is_none(),
        "{:?}",
        engine.last_transition_error()
    );
    // Two natives plus the player, `held` and `charm`; `mount` and `rider` stayed in district A.
    assert_eq!(engine.world().entity_count(), 5);
    let player = named_of(&engine, "player");
    let held = named_of(&engine, "held");
    let charm = named_of(&engine, "charm");
    assert_eq!(parent_of(&engine, held), Some(player));
    assert_eq!(parent_of(&engine, charm), Some(held));
    assert_eq!(
        serialized(&engine, player, "Transform").unwrap()["translation"],
        serde_json::json!(ANCHOR_XYZ)
    );
    assert_eq!(
        serialized(&engine, held, "Transform").unwrap()["translation"],
        serde_json::json!([0.0, 1.0, 0.0]),
        "a carried child keeps its local Transform"
    );
    assert_eq!(
        serialized(&engine, charm, "Transform").unwrap()["translation"],
        serde_json::json!([0.0, 0.5, 0.0])
    );
}

#[test]
fn a_carried_child_whose_parent_stays_behind_is_unparented_keeping_its_world_pose() {
    let dir = fresh_dir("orphan");
    let engine = tree_engine(&dir, switch_rider);
    let rider = named_of(&engine, "rider");
    assert_eq!(parent_of(&engine, rider), None);
    let transform = serialized(&engine, rider, "Transform").unwrap();
    assert_eq!(
        transform["scale"],
        serde_json::json!([2.0, 2.0, 2.0]),
        "the scale it had in the world, composed through the mount"
    );
    assert_eq!(
        transform["translation"],
        serde_json::json!(ANCHOR_XYZ),
        "a carried root takes the anchor"
    );
    let warning = engine
        .last_transition_error()
        .expect("the orphaning is reported");
    assert_eq!(warning.code.as_str(), "L3043", "{warning}");
}

#[test]
fn a_child_carried_beside_its_parent_travels_once() {
    let dir = fresh_dir("once");
    let engine = tree_engine(&dir, switch_player_and_held);
    assert!(
        engine.last_transition_error().is_none(),
        "{:?}",
        engine.last_transition_error()
    );
    assert_eq!(engine.world().entity_count(), 5);
    let held = named_of(&engine, "held");
    assert_eq!(parent_of(&engine, held), Some(named_of(&engine, "player")));
}

#[test]
fn two_identical_drives_across_a_subtree_switch_hash_identically() {
    let drive = |name: &str| -> Vec<u64> {
        let dir = fresh_dir(name);
        let mut engine = base_game(&dir)
            .system("switch", switch_player)
            .build()
            .expect("boots");
        engine.load_inline_scene(START_TREE).unwrap();
        (0..6)
            .map(|_| {
                engine.advance_live();
                engine.world_hash().unwrap().as_u64()
            })
            .collect()
    };
    assert_eq!(drive("tree-a"), drive("tree-b"));
}

// --- Entity references across the carry ---------------------------------------------------------

/// [`START_TREE`] with references: `player` looks at `mount` (optional, stays behind), `charm` holds
/// `held` (required, carried with it) and `rider` holds `mount` (required, stays behind).
fn refs_scene() -> String {
    let at = |index: u32| {
        format!(
            "        target:
          index: {index}
          generation: 0
"
        )
    };
    START_TREE
        .replace(
            "        last: 0
  - name: held",
            &format!(
                "        last: 0
      Eyes:
{}  - name: held",
                at(3)
            ),
        )
        .replace(
            "          - 0.5
          - 0.0
  - name: mount",
            &format!(
                "          - 0.5
          - 0.0
      Holds:
{}  - name: mount",
                at(1)
            ),
        )
        + &format!(
            "      Holds:
{}",
            at(3)
        )
}

fn refs_engine(dir: &Path, system: fn(&mut World, &SystemCtx)) -> Engine {
    let mut engine = base_game(dir)
        .system("switch", system)
        .build()
        .expect("boots");
    engine.load_inline_scene(&refs_scene()).unwrap();
    for _ in 0..5 {
        engine.advance_live();
    }
    engine
}

/// [`refs_scene`] with reference lists: `player` crews `[held, mount]` (optional elements, `held`
/// carried, `mount` left behind) and `rider` has the duty `[mount]` (required, left behind).
fn collection_engine(dir: &Path, system: fn(&mut World, &SystemCtx)) -> Engine {
    let element = |index: u32| {
        format!(
            "          - who:
              index: {index}
              generation: 0
"
        )
    };
    let scene = refs_scene().replace(
        "  - name: held",
        &format!(
            "      Crew:
        slots:
{}{}  - name: held",
            element(1),
            element(3)
        ),
    ) + &format!(
        "      Duties:
        posts:
{}",
        element(3)
    );
    let mut engine = base_game(dir)
        .system("switch", system)
        .build()
        .expect("boots");
    engine.load_inline_scene(&scene).unwrap();
    for _ in 0..5 {
        engine.advance_live();
    }
    engine
}

#[test]
fn a_carried_list_element_follows_its_carried_target_and_a_left_behind_one_is_cleared() {
    let dir = fresh_dir("collection-optional");
    let engine = collection_engine(&dir, switch_player);
    let held = named_of(&engine, "held");
    let crew = serialized(&engine, named_of(&engine, "player"), "Crew").unwrap();
    assert_eq!(
        crew["slots"][0]["who"],
        serde_json::to_value(held).unwrap(),
        "{crew}"
    );
    assert!(crew["slots"][1]["who"].is_null(), "{crew}");
    let warning = engine.last_transition_error().expect("reported");
    assert_eq!(warning.code.as_str(), "L3043", "{warning}");
    assert!(
        warning.to_string().contains("`Crew`/slots/1/who"),
        "{warning}"
    );
}

#[test]
fn a_carried_required_list_element_left_behind_drops_its_component() {
    let dir = fresh_dir("collection-required");
    let engine = collection_engine(&dir, switch_rider);
    let rider = named_of(&engine, "rider");
    assert!(serialized(&engine, rider, "Duties").is_none());
    let warning = engine.last_transition_error().expect("reported");
    assert_eq!(warning.code.as_str(), "L3043", "{warning}");
    assert!(warning.to_string().contains("`Duties`"), "{warning}");
}

#[test]
fn a_carried_reference_to_a_carried_entity_follows_it() {
    let dir = fresh_dir("ref-follows");
    let engine = refs_engine(&dir, switch_player);
    let held = named_of(&engine, "held");
    let holds = serialized(&engine, named_of(&engine, "charm"), "Holds").unwrap();
    assert_eq!(holds["target"], serde_json::to_value(held).unwrap());
}

#[test]
fn a_carried_optional_reference_to_an_entity_left_behind_is_cleared_and_reported() {
    let dir = fresh_dir("ref-optional");
    let engine = refs_engine(&dir, switch_player);
    let eyes = serialized(&engine, named_of(&engine, "player"), "Eyes").unwrap();
    assert!(eyes["target"].is_null(), "{eyes}");
    let warning = engine.last_transition_error().expect("reported");
    assert_eq!(warning.code.as_str(), "L3043", "{warning}");
    assert!(warning.to_string().contains("`Eyes`"), "{warning}");
}

#[test]
fn a_carried_required_reference_to_an_entity_left_behind_drops_its_component() {
    let dir = fresh_dir("ref-required");
    let engine = refs_engine(&dir, switch_rider);
    let rider = named_of(&engine, "rider");
    assert!(serialized(&engine, rider, "Holds").is_none());
    let warning = engine.last_transition_error().expect("reported");
    assert_eq!(warning.code.as_str(), "L3043", "{warning}");
    assert!(warning.to_string().contains("`Holds`"), "{warning}");
}
