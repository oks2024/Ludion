//! **A ragdoll cap for a horde game** — 300 enemies die at 25 a second, and at most 120 ragdolls are
//! ever simulated at once.
//!
//! Each kill spawns a skinned enemy and switches it with `switch_skin_to_ragdoll`. A group is
//! *retired* — its `Collider`, `Joint`, `JointMotor` and `Velocity` stripped, its `Transform` and
//! `SkinBone` kept, so the skin stays drawn in its settled pose at no solver cost — when every part
//! is asleep, when it has been simulated for `max_sim_ticks` (some parts never sleep), or when an
//! admission over the cap evicts it: the oldest unpinned simulated group first, pinned ones (a torso
//! joined to a static anchor) last. Retired groups are capped too; beyond
//! `max_retired` the oldest unpinned is despawned.
//!
//! **The pool is registered components**, never a resource: `RagdollBudget` (authored in the scene),
//! `RagdollGroup` on each skin and `RagdollPin` on each pin carrier. They ride `WorldHash` and
//! `engine.snapshot`, so a restored run continues the same admissions. Admission order is
//! `frame * ADMISSIONS_PER_FRAME + index in frame`, which needs no counter.
//!
//! Everything above the `main` divider is copyable into a real game. See `ludion docs physics-joints`
//! "Ragdoll budget".
//!
//! Run it headless (prints the counts and checks the cap held every tick):
//!
//! ```text
//! cargo run -p ludion --example ragdoll_budget
//! ```

use ludion::prelude::*;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::process::ExitCode;

// --- The game (copyable) -------------------------------------------------------------------------

/// The parts prefab each kill switches its skin through, relative to the content root.
const PARTS_PATH: &str = "prefabs/mannequin-ragdoll.prefab.yaml";
/// The skinned mesh every enemy wears.
const SKIN_PATH: &str = "characters/mannequin.assembly.yaml";

/// Enemies killed over the run.
pub const KILLS: u64 = 300;
/// Kills per second of gameplay time.
pub const KILLS_PER_SECOND: u64 = 25;
/// Admission slots per frame in the order key; `kill_enemies` asserts a frame stays under it.
pub const ADMISSIONS_PER_FRAME: u64 = 1 << 16;
/// Ticks the demo runs: all kills land by tick 720, the rest is settling and retiring.
pub const RUN_TICKS: usize = 1200;

/// The tunables, authored on one scene entity.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq)]
pub struct RagdollBudget {
    /// Most groups simulated at once; an admission over it evicts.
    pub max_simulated: u32,
    /// Most retired groups kept drawn; beyond it the oldest is despawned.
    pub max_retired: u32,
    /// Longest a group may stay simulated, in ticks: some parts never fall asleep, so settling alone
    /// would leave their groups in the solver forever.
    pub max_sim_ticks: u32,
}

/// Where a group is in its life; retired states record why.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum GroupState {
    /// Bodies and joints live in the solver.
    Simulated,
    /// Retired because every part fell asleep.
    Settled,
    /// Retired because an admission over the cap forced it.
    Evicted,
    /// Retired because it stayed simulated for `max_sim_ticks` without settling.
    Expired,
}

/// On the skin of a switched enemy: its place in the pool.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq)]
pub struct RagdollGroup {
    /// `frame * ADMISSIONS_PER_FRAME + index in frame`: admission order.
    pub admitted: u64,
    /// Whether the torso is joined to a static anchor; pinned groups are evicted and despawned last.
    pub pinned: bool,
    pub state: GroupState,
}

/// On a pin's joint carrier: the skin whose torso it holds, so a retire finds it.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq)]
pub struct RagdollPin {
    #[schemars(with = "EntityRefSchema")]
    pub group: EntityId,
}

/// Every part of every group, by skin, ids ascending.
fn parts_by_skin(world: &mut World) -> BTreeMap<EntityId, Vec<EntityId>> {
    let mut map: BTreeMap<EntityId, Vec<EntityId>> = BTreeMap::new();
    for (id, (bone,)) in world.query_ids::<(&SkinBone,)>() {
        map.entry(bone.skin).or_default().push(id);
    }
    for parts in map.values_mut() {
        parts.sort();
    }
    map
}

/// Every group, oldest admission first.
pub fn groups(world: &mut World) -> Vec<(EntityId, RagdollGroup)> {
    let mut groups: Vec<(EntityId, RagdollGroup)> = world
        .query_ids::<(&RagdollGroup,)>()
        .map(|(id, (group,))| (id, group.clone()))
        .collect();
    groups.sort_by_key(|(_, group)| group.admitted);
    groups
}

/// Whether every part is asleep. A body created this tick reads `None`, so a fresh group never
/// counts as settled.
fn all_asleep(world: &World, parts: &[EntityId]) -> bool {
    let Some(physics) = world.resource::<PhysicsWorld>() else {
        return false;
    };
    !parts.is_empty()
        && parts
            .iter()
            .all(|&part| physics.is_sleeping(part) == Some(true))
}

/// The pin carriers of `skin` and their static anchors.
fn pin_entities(world: &mut World, skin: EntityId) -> Vec<EntityId> {
    let mut doomed = Vec::new();
    for (carrier, (pin, joint)) in world.query_ids::<(&RagdollPin, &Joint)>() {
        if pin.group == skin {
            doomed.extend([carrier, joint.a]);
        }
    }
    doomed
}

/// Drops `skin`'s group from the solver in one tick. The parts keep `Transform` and `SkinBone`, so the
/// skin stays drawn in its pose; the pin goes with them, or its joint would go inert.
fn retire(world: &mut World, skin: EntityId, parts: &[EntityId], reason: GroupState) {
    for &part in parts {
        let _ = world.remove::<Collider>(part);
        let _ = world.remove::<Joint>(part);
        let _ = world.remove::<JointMotor>(part);
        let _ = world.remove::<Velocity>(part);
    }
    let pins = pin_entities(world, skin);
    let errors = world.despawn_many(&pins);
    debug_assert!(errors.is_empty(), "{errors:?}");
    if let Ok(Some(group)) = world.get_mut::<RagdollGroup>(skin) {
        group.state = reason;
    }
}

/// Removes a retired group entirely: pins, parts and skin.
fn despawn_group(world: &mut World, skin: EntityId, parts: &[EntityId]) {
    let mut doomed = pin_entities(world, skin);
    doomed.extend_from_slice(parts);
    doomed.push(skin);
    let errors = world.despawn_many(&doomed);
    debug_assert!(errors.is_empty(), "{errors:?}");
}

/// Retires every simulated group whose parts have all fallen asleep, and force-retires those older
/// than `max_sim_ticks`. A group's age is the frame minus its admission frame: no counter.
fn retire_settled(world: &mut World, ctx: &SystemCtx) {
    let Some(budget) = world
        .single::<RagdollBudget>()
        .and_then(|id| world.get::<RagdollBudget>(id).ok().flatten().cloned())
    else {
        return;
    };
    let frame = ctx.game_frame.as_u64();
    let parts = parts_by_skin(world);
    for (skin, group) in groups(world) {
        let Some(parts) = parts.get(&skin) else {
            continue;
        };
        if group.state != GroupState::Simulated {
            continue;
        }
        if all_asleep(world, parts) {
            retire(world, skin, parts, GroupState::Settled);
        } else if frame.saturating_sub(group.admitted / ADMISSIONS_PER_FRAME)
            >= u64::from(budget.max_sim_ticks)
        {
            retire(world, skin, parts, GroupState::Expired);
        }
    }
}

/// Kills due by `frame`: a steady rate, capped at [`KILLS`].
fn due(frame: u64) -> u64 {
    (frame * KILLS_PER_SECOND / 60).min(KILLS)
}

/// Admits this frame's kills, evicting to stay under `max_simulated`.
fn kill_enemies(world: &mut World, ctx: &SystemCtx) {
    let Some(budget) = world
        .single::<RagdollBudget>()
        .and_then(|id| world.get::<RagdollBudget>(id).ok().flatten().cloned())
    else {
        return;
    };
    let frame = ctx.game_frame.as_u64();
    let first = if frame == 0 { 0 } else { due(frame - 1) };
    for (index, kill) in (first..due(frame)).enumerate() {
        evict_for_room(world, &budget);
        assert!(
            (index as u64) < ADMISSIONS_PER_FRAME,
            "more than {ADMISSIONS_PER_FRAME} admissions in one frame"
        );
        admit(world, kill, frame * ADMISSIONS_PER_FRAME + index as u64);
    }
}

/// Retires simulated groups, the oldest unpinned first and pinned last, until one more fits.
fn evict_for_room(world: &mut World, budget: &RagdollBudget) {
    loop {
        let simulated: Vec<(EntityId, RagdollGroup)> = groups(world)
            .into_iter()
            .filter(|(_, group)| group.state == GroupState::Simulated)
            .collect();
        if (simulated.len() as u32) < budget.max_simulated {
            return;
        }
        let victim = simulated
            .iter()
            .find(|(_, group)| !group.pinned)
            .or(simulated.first());
        let Some((skin, _)) = victim else {
            return;
        };
        let parts = parts_by_skin(world).remove(skin).unwrap_or_default();
        retire(world, *skin, &parts, GroupState::Evicted);
    }
}

/// Spawns enemy number `kill` on the grid and switches it to its ragdoll. Every tenth is pinned by
/// the torso to a static anchor.
fn admit(world: &mut World, kill: u64, admitted: u64) {
    let cell = (kill % 120) as f32;
    let (column, row) = (cell % 12.0, (cell / 12.0).floor());
    let at = Transform {
        translation: [(column - 5.5) * 1.4, 0.0, (row - 4.5) * 1.4],
        ..Transform::default()
    };
    let skin = world.spawn();
    let inserted = world.insert(skin, at).is_ok()
        && world
            .insert(
                skin,
                Mesh {
                    shape: MeshSource::Assembly(SKIN_PATH.into()),
                    ..Mesh::default()
                },
            )
            .is_ok()
        && world
            .insert(
                skin,
                Velocity {
                    linear: [((kill % 5) as f32 - 2.0) * 0.6, 0.0, -1.5],
                    angular: [0.0; 3],
                },
            )
            .is_ok();
    debug_assert!(inserted);
    let parts = match switch_skin_to_ragdoll(world, skin, PARTS_PATH) {
        Ok(parts) => parts,
        Err(diagnostics) => {
            for diagnostic in diagnostics {
                report_full_diagnostic(world, "ragdoll", diagnostic);
            }
            let _ = world.despawn(skin);
            return;
        }
    };
    let pinned = kill.is_multiple_of(10);
    if pinned {
        pin_torso(world, skin, parts[1]);
    }
    let _ = world.insert(
        skin,
        RagdollGroup {
            admitted,
            pinned,
            state: GroupState::Simulated,
        },
    );
}

/// Joins `torso` to a fresh static anchor at the torso's own pose through a dedicated carrier.
fn pin_torso(world: &mut World, skin: EntityId, torso: EntityId) {
    let Ok(Some(&at)) = world.get::<Transform>(torso) else {
        return;
    };
    let anchor = world.spawn();
    let _ = world.insert(anchor, at);
    let _ = world.insert(
        anchor,
        Collider {
            shape: Shape::Sphere { radius: 0.05 },
            body: Body::new_static(),
            layer: None,
            sensor: false,
        },
    );
    let carrier = world.spawn();
    let _ = world.insert(carrier, RagdollPin { group: skin });
    let _ = world.insert(
        carrier,
        Joint {
            a: anchor,
            b: torso,
            kind: JointKind::Ball { limits: None },
            anchor: [0.0; 3],
            break_impulse: None,
            collide: false,
            zero: None,
        },
    );
}

/// Despawns the oldest retired groups (unpinned first) past `max_retired`.
fn cap_retired(world: &mut World, _ctx: &SystemCtx) {
    let Some(budget) = world
        .single::<RagdollBudget>()
        .and_then(|id| world.get::<RagdollBudget>(id).ok().flatten().cloned())
    else {
        return;
    };
    let mut retired: Vec<(EntityId, RagdollGroup)> = groups(world)
        .into_iter()
        .filter(|(_, group)| group.state != GroupState::Simulated)
        .collect();
    if retired.len() as u32 <= budget.max_retired {
        return;
    }
    let mut parts = parts_by_skin(world);
    while retired.len() as u32 > budget.max_retired {
        let at = retired
            .iter()
            .position(|(_, group)| !group.pinned)
            .unwrap_or(0);
        let (skin, _) = retired.remove(at);
        despawn_group(world, skin, &parts.remove(&skin).unwrap_or_default());
    }
}

/// The game: three systems, in this order. A real game's `main` is `ludion::run(game())`.
pub fn game() -> EngineBuilder {
    Engine::builder()
        .components(|registry| {
            registry.register::<RagdollBudget>("RagdollBudget")?;
            registry.register::<RagdollGroup>("RagdollGroup")?;
            registry.register::<RagdollPin>("RagdollPin")
        })
        .system("retire_settled", retire_settled)
        .query::<(&RagdollBudget,)>()
        .query::<(&mut Collider,)>()
        .query::<(&mut Joint,)>()
        .query::<(&mut JointMotor,)>()
        .query::<(&mut Velocity,)>()
        .query::<(&mut RagdollGroup,)>()
        .query::<(&mut RagdollPin,)>()
        .query::<(&SkinBone,)>()
        .system("kill_enemies", kill_enemies)
        .query::<(&mut Collider,)>()
        .query::<(&mut Joint,)>()
        .query::<(&mut JointMotor,)>()
        .query::<(&mut Velocity,)>()
        .query::<(&mut RagdollGroup,)>()
        .query::<(&mut RagdollPin,)>()
        .query::<(&mut SkinBone,)>()
        .query::<(&mut Mesh,)>()
        .query::<(&PrefabInstance,)>()
        .query::<(&mut PrefabInstance,)>()
        .query::<(&mut Name,)>()
        .query::<(&RagdollBudget,)>()
        .query::<(&mut Transform,)>()
        .system("cap_retired", cap_retired)
        .query::<(&mut RagdollGroup,)>()
        .query::<(&mut RagdollPin,)>()
        .query::<(&mut Joint,)>()
        .query::<(&SkinBone,)>()
        .query::<(&RagdollBudget,)>()
}

/// A camera over the field, a key light, a static floor (24 × 24 m) and the budget.
pub const SCENE: &str = "\
format: 1
seed: 1
physics:
  space: free
entities:
  - name: camera
    components:
      Transform:
        translation:
          - 0.0
          - 13.0
          - 17.0
        rotation:
          - -38.0
          - 0.0
          - 0.0
      Camera:
        projection:
          type: perspective
          fov_y_deg: 50.0
  - name: key-light
    components:
      Transform:
        translation:
          - 0.0
          - 12.0
          - 4.0
      PointLight:
        color:
          - 1.0
          - 0.96
          - 0.9
        intensity: 200.0
        range: 40.0
  - name: floor
    components:
      Transform:
        translation:
          - 0.0
          - -0.5
          - 0.0
      Collider:
        shape:
          box:
            half_extents:
              - 12.0
              - 0.5
              - 12.0
        body: static
  - name: floor-look
    components:
      Transform:
        translation:
          - 0.0
          - -0.5
          - 0.0
        scale:
          - 24.0
          - 1.0
          - 24.0
      Mesh:
        shape: cube
        color:
          - 0.55
          - 0.57
          - 0.6
          - 1.0
  - name: ragdoll-budget
    components:
      RagdollBudget:
        max_simulated: 120
        max_retired: 300
        max_sim_ticks: 300
";

// --- Headless demo `main` ------------------------------------------------------------------------

/// Runs [`RUN_TICKS`] ticks and checks the simulated count never passed the cap and every group kept
/// all its parts. Deterministic, no window.
fn main() -> ExitCode {
    let root = std::path::Path::new(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../examples/gallery"
    ));
    let mut engine = match game().project_root(root).build() {
        Ok(engine) => engine,
        Err(diagnostic) => {
            eprintln!("{diagnostic}");
            return ExitCode::FAILURE;
        }
    };
    if let Err(diagnostics) = engine.load_inline_scene(SCENE) {
        for diagnostic in diagnostics {
            eprintln!("{diagnostic}");
        }
        return ExitCode::FAILURE;
    }
    let max_simulated = {
        let world = engine.world();
        world
            .single_get::<RagdollBudget>()
            .map_or(0, |budget| budget.max_simulated)
    };
    let mut seen = std::collections::BTreeSet::new();
    let mut peak = 0;
    let mut broken = false;
    for _ in 0..RUN_TICKS {
        engine.advance_live();
        let world = engine.world_mut();
        let parts = parts_by_skin(world);
        let all = groups(world);
        let simulated = all
            .iter()
            .filter(|(_, group)| group.state == GroupState::Simulated)
            .count();
        peak = peak.max(simulated);
        broken |= all
            .iter()
            .any(|(skin, _)| parts.get(skin).is_none_or(|parts| parts.len() != 11));
        seen.extend(all.iter().map(|(_, group)| group.admitted));
    }
    let world = engine.world_mut();
    let all = groups(world);
    let count = |state| all.iter().filter(|(_, group)| group.state == state).count();
    let bodies = world
        .resource::<PhysicsWorld>()
        .map_or(0, |physics| physics.body_count());
    println!(
        "switched {} · simulated {} · retired {} · despawned {} · peak simulated {peak} · bodies {bodies}",
        seen.len(),
        count(GroupState::Simulated),
        count(GroupState::Settled) + count(GroupState::Evicted) + count(GroupState::Expired),
        seen.len() - all.len(),
    );
    if peak > max_simulated as usize || broken || seen.len() != KILLS as usize {
        eprintln!("the cap did not hold, or a group lost parts");
        return ExitCode::FAILURE;
    }
    ExitCode::SUCCESS
}
