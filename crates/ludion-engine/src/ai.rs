//! The engine's half of `ludion-ai`: faults queued by `ai_tick` become diagnostics.
//!
//! `ludion-ai` cannot see the `Diagnostics` resource (it does not depend on this crate), so its
//! faults wait in an `AiFaults` resource and this drain reports each through
//! [`crate::diagnostics::report_full`], once per `(code, entity)` by the engine's own latch. A no-op
//! on a world where no machine ever faulted.

use ludion_ai::{AiFaults, AiRoster};
use ludion_core::diagnostics::Diagnostic;
use ludion_ecs::World;
use ludion_protocol::response::{AiAgent, AiMachine, AiState};
use serde_json::{Value, json};

use crate::components::Name;
use crate::scene_physics::entity_label;

/// Reports every fault `ai_tick` and `send` queued since the last drain.
pub(crate) fn report_faults(world: &mut World) {
    let Some(faults) = world.resource_mut::<AiFaults>() else {
        return;
    };
    for fault in faults.take() {
        let named = entity_label(world, fault.entity);
        let key = crate::diagnostics::offender_key(world, fault.entity);
        let diagnostic = Diagnostic::new(fault.code, format!("{named}: {}", fault.message));
        crate::diagnostics::report_full(world, key, diagnostic);
    }
}

/// The `ai` key of `engine.describe`: every machine `ai_tick` has run, with its hierarchy and live
/// agents (`entity`, `name`, `path`, `entered`, `ticks_in` = the machine's `game_tick` minus each
/// `entered`, `inbox`, `dropped`). `None` while no machine has ticked, so a world without ai
/// describes byte-identically to one before this key existed.
pub(crate) fn describe_section(world: &World) -> Option<Value> {
    let machines = machines(world)?;
    Some(json!({ "machines": machines }))
}

/// The typed machines behind both `engine.describe`'s `ai` key and `ai.describe`; `None` while no
/// machine has ticked.
pub(crate) fn machines(world: &World) -> Option<Vec<AiMachine>> {
    let roster = world.resource::<AiRoster>()?;
    let machines = roster
        .machines
        .iter()
        .map(|machine| {
            let agents = machine
                .agents(world)
                .into_iter()
                .map(|agent| {
                    let name = match world.get::<Name>(agent.entity) {
                        Ok(Some(name)) => Some(name.0.clone()),
                        _ => None,
                    };
                    let ticks_in = agent
                        .entered
                        .iter()
                        .map(|entered| machine.game_tick.saturating_sub(*entered))
                        .collect();
                    AiAgent {
                        entity: agent.entity,
                        name,
                        path: agent.path,
                        entered: agent.entered,
                        ticks_in,
                        inbox: agent.inbox as u64,
                        dropped: agent.dropped,
                    }
                })
                .collect();
            let states = machine
                .states
                .iter()
                .map(|s| AiState {
                    state: s.state.clone(),
                    parent: s.parent.clone(),
                    initial: s.initial.clone(),
                })
                .collect();
            AiMachine {
                machine: machine.machine.to_string(),
                component: machine.component.clone(),
                root: machine.root.clone(),
                leaf_first: machine.leaf_first,
                inbox_capacity: machine.inbox_capacity as u64,
                game_tick: machine.game_tick,
                states,
                agents,
            }
        })
        .collect();
    Some(machines)
}

#[cfg(test)]
mod tests {
    use ludion_ai::{Cx, Machine, ai_tick, register_machine};
    use ludion_protocol::event::Event;
    use schemars::JsonSchema;
    use serde::{Deserialize, Serialize};

    use crate::Engine;

    #[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
    enum Flip {
        Root,
        A,
        B,
    }

    impl Machine for Flip {
        type Blackboard = ();
        type Event = ();
        const NAME: &'static str = "Flip";
        const ROOT: Self = Flip::Root;
        const ALL: &'static [Self] = &[Flip::Root, Flip::A, Flip::B];
        fn parent(s: Self) -> Option<Self> {
            (s != Flip::Root).then_some(Flip::Root)
        }
        fn initial(s: Self) -> Option<Self> {
            (s == Flip::Root).then_some(Flip::A)
        }
        fn enter(cx: &mut Cx<'_, Self>, s: Self) {
            match s {
                Flip::A => cx.go(Flip::B),
                Flip::B => cx.go(Flip::A),
                Flip::Root => {}
            }
        }
    }

    #[test]
    fn describe_carries_an_ai_key_only_once_a_machine_has_ticked() {
        let mut engine = Engine::builder()
            .components(register_machine::<Flip>)
            .system("flip_ai", ai_tick::<Flip>)
            .build()
            .expect("boots");
        engine
            .load_inline_scene(
                "format: 1
seed: 1
entities:
  - name: flipper
    components:
      FlipMind: {}
",
            )
            .expect("loads");
        assert!(engine.describe_document().get("ai").is_none());
        engine.advance_live();
        let doc = engine.describe_document();
        let machine = &doc["ai"]["machines"][0];
        assert_eq!(machine["machine"], "Flip");
        assert_eq!(machine["agents"][0]["name"], "flipper");
    }

    #[test]
    fn a_runaway_chain_surfaces_as_an_l4120_diagnostic_once() {
        let mut engine = Engine::builder()
            .components(register_machine::<Flip>)
            .system("flip_ai", ai_tick::<Flip>)
            .build()
            .expect("boots");
        engine
            .load_inline_scene(
                "format: 1\nseed: 1\nentities:\n  - name: flipper\n    components:\n      FlipMind: {}\n",
            )
            .expect("loads");
        let reported = |engine: &mut Engine| {
            engine
                .take_events()
                .into_iter()
                .filter_map(|event| match event {
                    Event::Log(log) => log.diagnostic,
                    _ => None,
                })
                .filter(|d| d.code.as_str() == "L4120")
                .count()
        };
        engine.advance_live();
        assert_eq!(reported(&mut engine), 1);
        engine.advance_live();
        assert_eq!(reported(&mut engine), 0, "latched: once per entity");
    }
}
