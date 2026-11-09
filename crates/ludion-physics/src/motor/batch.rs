//! [`move_characters`]: many motors moved in one call, their casts run in parallel.

use ludion_core::collections::StableSet;
use ludion_core::ids::EntityId;
use ludion_core::par;
use ludion_ecs::World;
use ludion_spatial::ParentPose;

use super::mover::{Outcome, Prepared, Solver, apply, platform_after, prepare};
use super::report::{MotorError, MoveReport};
use crate::pose::PhysicsPose;
use crate::world::PhysicsWorld;

/// Motors solved per parallel task. A contract constant: results involve no reduction, so they do
/// not depend on it.
const MOTOR_CHUNK: usize = 16;

struct Slot {
    index: usize,
    entity: EntityId,
    prepared: Prepared,
    outcome: Option<Outcome>,
}

/// Moves every `(entity, displacement)` in `moves` as [`move_character`](crate::move_character)
/// would, with the casts of all of them run in parallel over fixed chunks; `out[i]` is the result of
/// `moves[i]`.
///
/// **The carry rule.** Every support pose a call reads (the carry it is due and the support it
/// records as `platform`) is the **tick-start pose**: the world as it was when the batch began.
/// Writes apply afterwards, in slice order. With no support moving during the batch, the results are
/// bit-identical to calling `move_character` once per entry in slice order, at any thread count,
/// and do not depend on the slice order. A motor standing on another motor is therefore carried by
/// that support's motion over the previous tick: a one-tick lag that loses no motion.
///
/// **Duplicates.** An entity listed twice moves on its first entry only; later entries get
/// [`MotorError::DuplicateInBatch`]. Merge the displacements instead.
///
/// There is no [`MoveConstraint`](crate::MoveConstraint) in a batch: a motor that needs one (a
/// lane) goes through `move_character_with`. Call it from a system registered **before**
/// [`install`](crate::install)'s, as for `move_character`.
///
/// # Panics
/// Panics if `moves` and `out` differ in length.
///
/// ```no_run
/// # use ludion_ecs::{SystemCtx, World};
/// # use ludion_physics::{move_characters, MotorError, MoveReport, NoParent, PhysicsPose};
/// # use ludion_core::ids::EntityId;
/// fn walk<P: PhysicsPose>(world: &mut World, _ctx: &SystemCtx, crowd: &[EntityId]) {
///     let moves: Vec<_> = crowd.iter().map(|id| (*id, [0.05, -0.2, 0.0])).collect();
///     let mut out = vec![Err(MotorError::NoPhysicsWorld); moves.len()];
///     move_characters::<P, NoParent>(world, &moves, &mut out);
///     let landed = out.iter().flatten().filter(|report| report.landed).count();
/// }
/// ```
pub fn move_characters<P: PhysicsPose, R: ParentPose>(
    world: &mut World,
    moves: &[(EntityId, [f32; 3])],
    out: &mut [Result<MoveReport, MotorError>],
) {
    assert_eq!(
        moves.len(),
        out.len(),
        "move_characters: {} moves but {} output slots",
        moves.len(),
        out.len()
    );

    let mut seen = StableSet::new();
    let mut slots = Vec::with_capacity(moves.len());
    for (index, &(entity, displacement)) in moves.iter().enumerate() {
        if !seen.insert(entity) {
            out[index] = Err(MotorError::DuplicateInBatch(entity));
            continue;
        }
        match prepare::<P, R>(world, entity, displacement) {
            Ok(prepared) => slots.push(Slot {
                index,
                entity,
                prepared,
                outcome: None,
            }),
            Err(error) => out[index] = Err(error),
        }
    }

    let platforms: Vec<_> = {
        let Some(physics) = world.resource::<PhysicsWorld>() else {
            return;
        };
        par::par_chunks(&mut slots, MOTOR_CHUNK, |_, chunk| {
            for slot in chunk {
                let prepared = &slot.prepared;
                slot.outcome = Some(Solver::new(physics, slot.entity, &prepared.checked).solve(
                    prepared.feet,
                    prepared.displacement,
                    prepared.was_grounded,
                    None,
                    prepared.carry.as_ref(),
                ));
            }
        });
        slots
            .iter()
            .map(|slot| {
                slot.outcome
                    .as_ref()
                    .and_then(|outcome| platform_after::<P, R>(world, physics, outcome))
            })
            .collect()
    };

    for (slot, platform_now) in slots.into_iter().zip(platforms) {
        if let Some(outcome) = slot.outcome {
            out[slot.index] = Ok(apply::<P>(
                world,
                slot.entity,
                slot.prepared.tick,
                outcome,
                platform_now,
            ));
        }
    }
}
