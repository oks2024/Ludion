//! **A list of entity references** — `Squad.members` holds several `Member` elements, each naming
//! another entity. `#[schemars(with = "EntityRefCollection<Vec<Member>>")]` is what lets the engine
//! follow every element: scene load range-checks each one (`L1017`), `scene.save` remaps each to a
//! document position, and a scene carry moves each with its target. `Option<EntityRefSchema>` on the
//! element makes a despawned target null only that element instead of refusing the save.
//!
//! Everything above the `main` divider is copyable into a real game. Registered on the built-in
//! registry, `entity_refs()` lists the pattern suffix and `entity_ref_sites` the concrete ones of a
//! value; the example exits non-zero if either is missing.
//!
//! Run it headless (prints the `/*` suffix and the concrete sites):
//!
//! ```text
//! cargo run -p ludion --example entity_ref_collection
//! ```

use ludion::prelude::*;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::process::ExitCode;

// --- The game (copyable) -------------------------------------------------------------------------

/// One squad slot; empty when its member is gone.
#[derive(Serialize, Deserialize, JsonSchema, Debug)]
struct Member {
    #[schemars(with = "Option<EntityRefSchema>")]
    who: Option<EntityId>,
}

/// The entities a squad commands, in order.
#[derive(Serialize, Deserialize, JsonSchema, Debug)]
struct Squad {
    #[schemars(with = "EntityRefCollection<Vec<Member>>")]
    members: Vec<Member>,
}

// --- main (not copyable) -------------------------------------------------------------------------

fn main() -> ExitCode {
    let mut registry = builtin_registry();
    registry
        .register::<Squad>("Squad")
        .expect("Squad registers");
    let info = registry.get("Squad").expect("registered");
    let Some(pattern) = info.entity_refs().first() else {
        eprintln!("Squad.members is not marked as an entity-reference collection");
        return ExitCode::FAILURE;
    };
    println!("pattern: {}", pattern.suffix);
    let value = serde_json::json!({
        "members": [
            {"who": {"index": 1, "generation": 0}},
            {"who": null},
            {"who": {"index": 2, "generation": 0}}
        ]
    });
    let sites = info.entity_ref_sites(&value);
    if !pattern.suffix.contains("/*") || sites.len() != 3 {
        eprintln!("expected a `/*` pattern and 3 sites, found {}", sites.len());
        return ExitCode::FAILURE;
    }
    for site in &sites {
        println!("site: {}", site.suffix);
    }
    ExitCode::SUCCESS
}
