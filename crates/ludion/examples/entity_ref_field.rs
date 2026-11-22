//! **Marking an entity-reference field** — `Leash.holder` names another entity, and
//! `#[schemars(with = "Option<EntityRefSchema>")]` is what makes the engine know it: scene load
//! range-checks the reference (`L1017`) and `scene.save` remaps it to document positions. Without
//! the marker the field is just two integers.
//!
//! Everything above the `main` divider is copyable into a real game. Registered on the built-in
//! registry, `entity_refs()` lists the marked field; the example exits non-zero if it is missing.
//!
//! Run it headless (prints the marked suffix):
//!
//! ```text
//! cargo run -p ludion --example entity_ref_field
//! ```

use ludion::prelude::*;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::process::ExitCode;

// --- The game (copyable) -------------------------------------------------------------------------

/// A tether to the entity holding it; absent when the leash is loose.
#[derive(Serialize, Deserialize, JsonSchema, Debug, Clone)]
struct Leash {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Option<EntityRefSchema>")]
    holder: Option<EntityId>,
}

// --- main (not copyable) -------------------------------------------------------------------------

fn main() -> ExitCode {
    let mut registry = builtin_registry();
    registry
        .register::<Leash>("Leash")
        .expect("Leash registers");
    let fields = registry.get("Leash").expect("registered").entity_refs();
    let Some(field) = fields.first() else {
        eprintln!("Leash.holder is not marked as an entity reference");
        return ExitCode::FAILURE;
    };
    println!(
        "marked: {} (optional: {}, nullable: {})",
        field.suffix, field.optional, field.nullable
    );
    ExitCode::SUCCESS
}
