//! [`AiFaults`]: runtime faults queued for the engine to report.
//!
//! This crate cannot see the engine's `Diagnostics` resource, so a fault is queued here and the
//! engine drains the queue once a tick. The resource is never hashed, snapshotted or registered as a
//! component.

use ludion_core::diagnostics::DiagnosticCode;
use ludion_core::ids::EntityId;
use ludion_ecs::World;

/// One runtime fault of an agent: the entity, its `L####` code and a message naming the cause.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AiFault {
    /// The offending entity (the agent, or the target of a refused send).
    pub entity: EntityId,
    /// The registered code: `L4120`–`L4124`.
    pub code: DiagnosticCode,
    /// What happened, in terms a level designer can act on. It does not name `entity`: the drain
    /// prefixes the entity's name.
    pub message: String,
}

/// The queue of [`AiFault`]s awaiting the engine's drain. Inserted into the world lazily, by the
/// first fault.
#[derive(Debug, Default)]
pub struct AiFaults {
    pending: Vec<AiFault>,
}

impl AiFaults {
    /// Queues a fault unless the same `(entity, code)` is already pending, so a standing fault
    /// that repeats every tick cannot grow the queue between drains.
    pub fn push(&mut self, entity: EntityId, code: DiagnosticCode, message: impl Into<String>) {
        if self
            .pending
            .iter()
            .any(|f| f.entity == entity && f.code == code)
        {
            return;
        }
        self.pending.push(AiFault {
            entity,
            code,
            message: message.into(),
        });
    }

    /// Takes every pending fault, in the order they were raised.
    pub fn take(&mut self) -> Vec<AiFault> {
        std::mem::take(&mut self.pending)
    }
}

/// Queues a fault on the world's [`AiFaults`], inserting the resource on first use.
pub(crate) fn fault(
    world: &mut World,
    entity: EntityId,
    code: DiagnosticCode,
    message: impl Into<String>,
) {
    if world.resource::<AiFaults>().is_none() {
        world.insert_resource(AiFaults::default());
    }
    if let Some(faults) = world.resource_mut::<AiFaults>() {
        faults.push(entity, code, message);
    }
}
