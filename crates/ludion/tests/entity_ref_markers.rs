//! A game marks an entity-reference field with the prelude alone, and the marker reaches scene
//! validation through the facade: a holder pointing out of range is `L1017`.

use ludion::prelude::*;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, JsonSchema)]
struct Leash {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Option<EntityRefSchema>")]
    holder: Option<EntityId>,
}

fn engine() -> Engine {
    Engine::builder()
        .components(|registry| registry.register::<Leash>("Leash"))
        .build()
        .expect("the engine builds")
}

#[test]
fn a_reference_out_of_range_is_refused_by_scene_validation() {
    let mut engine = engine();
    let diagnostics = engine
        .load_inline_scene(
            "format: 1\nentities:\n  - components:\n      Leash:\n        holder:\n          index: 9\n          generation: 0\n",
        )
        .expect_err("an out-of-range holder is refused");
    assert!(
        diagnostics.iter().any(|d| d.code.as_str() == "L1017"),
        "{diagnostics:?}"
    );
}

#[test]
fn a_reference_in_range_loads() {
    let mut engine = engine();
    engine
        .load_inline_scene(
            "format: 1\nentities:\n  - name: a\n    components: {}\n  - components:\n      Leash:\n        holder:\n          index: 0\n          generation: 0\n",
        )
        .expect("an in-range holder loads");
}
