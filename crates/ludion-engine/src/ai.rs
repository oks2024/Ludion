//! The engine's half of `ludion-ai`: faults queued by `ai_tick` become diagnostics.
//!
//! `ludion-ai` cannot see the `Diagnostics` resource (it does not depend on this crate), so its
//! faults wait in an `AiFaults` resource and this drain reports each through
//! [`crate::diagnostics::report_full`], once per `(code, entity)` by the engine's own latch. A no-op
//! on a world where no machine ever faulted.

use ludion_ai::AiFaults;
use ludion_core::diagnostics::Diagnostic;
use ludion_ecs::World;

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
