//! Every registered component's own serialized value passes the wire schema check, so the editor's
//! `component.get` → `component.set` loop never meets an `L1014` on a value the engine produced.

use ludion_ecs::World;
use ludion_engine::Engine;
use serde_json::json;

#[test]
fn every_default_constructible_component_serializes_into_its_own_schema() {
    let engine = Engine::builder()
        .build()
        .expect("the default engine builds");
    let registry = engine.registry();
    let mut world = World::new();
    let mut checked = 0usize;
    let mut skipped = Vec::new();
    let mut failures = Vec::new();
    for info in registry.iter() {
        let entity = world.spawn();
        // A component with required fields has no constructible default; it is skipped, not failed.
        if info
            .deserialize_insert(&json!({}), &mut world, entity)
            .is_err()
        {
            skipped.push(info.name().to_string());
            continue;
        }
        let value = info
            .serialize(&world, entity)
            .expect("a present component serializes")
            .expect("the inserted component is present");
        checked += 1;
        let diagnostics = ludion_scene::check_component(info.name(), &value, registry);
        if !diagnostics.is_empty() {
            let messages: Vec<String> = diagnostics.iter().map(ToString::to_string).collect();
            failures.push(format!(
                "{}: {value} — {}",
                info.name(),
                messages.join("; ")
            ));
        }
    }
    println!("skipped (no default value): {skipped:?}");
    assert!(checked > 0, "no component was constructible from `{{}}`");
    assert!(
        failures.is_empty(),
        "components whose serialized value fails their own schema ({checked} checked, skipped: \
         {skipped:?}):\n{}",
        failures.join("\n")
    );
}
