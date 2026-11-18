//! [`assert_components_drift_safe`] — the drift-safety probe for snapshot migration.
//!
//! `engine.restore` fills a snapshot's added fields from serde defaults during migration (Phase 7
//! decision 3, `Plan/phase-7-designer-flow.md`): a `ludion dev` reload restarts the child on the
//! *new* code, so the snapshot it restores was taken against the *old* schema. That only works if a
//! component can deserialize with a field missing — a component with a field that has no
//! `#[serde(default)]` (and no hand-written `Deserialize` that supplies one) hard-fails the moment a
//! field is added to it, instead of migrating.
//!
//! This is a designed property, not an accident: [`assert_components_drift_safe`] probes every
//! component in a registry by building it from an empty JSON object `{}` — a stand-in for "every
//! field this component has today, plus one this version hasn't been written yet, absent." A
//! component that cannot build from `{}` cannot absorb a future field addition.
//!
//! # Identity/link components are exempt, by design
//!
//! Two built-ins are deliberately skipped outright: `Name` (a reserved, field-backed wrapper over a
//! bare `String` — not an object, so `{}` is a type mismatch, not a missing-field gap) and `Parent`
//! (a required [`EntityId`](ludion_core::ids::EntityId) reference). Both differ from an ordinary
//! value component in kind, not degree: they carry no data a scene author fills in incrementally,
//! only required identity. Giving `Parent`'s entity-reference field a default would not make it
//! drift-*safe* — it would make a corrupted or partial document silently resolve a link to entity
//! `(0, 0)`, a real entity, instead of failing loudly. That is exactly the "migration can bless
//! silent wrongness" risk this whole discipline exists to avoid, so it is excluded rather than
//! weakened.
//!
//! `Joint` carries the same two required `EntityId` references (`a`/`b`) plus a required `kind`, so
//! it cannot build from `{}` either — but unlike `Name`/`Parent` it also carries genuine value
//! fields (`anchor`, `collide`, each kind's `axis`/`limits`, `break_impulse`) that a future version
//! might extend, exactly the gap this probe exists to catch. Exempting it outright would blind the
//! probe to that; instead `probe_seed` gives it the smallest document that satisfies its required
//! fields, and everything past that must
//! still default. `JointMotor` is seeded the same way: its `target`, `stiffness` and `damping` are
//! required (there is no honest default drive), `max_force` and anything added later must default.
//! `JointPoses` and `PoseBlend` are seeded likewise: `stiffness`, `damping` and `poses`, and `pose` and
//! `weight`, are required (no honest default pose or weight), and anything added later must default.
//! `SkinBone` needs the same treatment: its `skin`/`bone` name a specific joint with no honest
//! default, so it is seeded rather than exempted, and anything added later must default.
//! `ParentJoint` is seeded for the same reason: its `joint` names a specific joint, and an empty
//! default name would load without error.
//! `PrefabInstance` is seeded the same way: its `source`, `part` and `root` are provenance
//! `spawn_prefab` always writes, with no honest default.

use serde_json::{Value, json};

use ludion_ecs::{Registry, World};

/// Registered names of components structurally exempt from the drift-safety probe (see the [module
/// docs](self)): identity/link types with no value fields at all, not value bags with fields a
/// future scene might add to.
const IDENTITY_EXEMPT: &[&str] = &["Name", "Parent"];

/// The JSON every component is probed from: `{}` for everything but `Joint`, which needs its two
/// required `EntityId` references and a `kind` before it can build at all, and `JointMotor`, which
/// needs its `target`, `stiffness` and `damping` (see the [module docs](self)).
fn probe_seed(name: &str) -> Value {
    match name {
        "Joint" => json!({
            "a": { "index": 0, "generation": 0 },
            "b": { "index": 1, "generation": 0 },
            "kind": { "fixed": {} },
        }),
        "JointMotor" => json!({ "target": [0.0], "stiffness": 0.0, "damping": 0.0 }),
        "JointPoses" => json!({ "stiffness": 0.0, "damping": 0.0, "poses": {} }),
        "PoseBlend" => json!({ "pose": "p", "weight": 0.0 }),
        "SkinBone" => json!({ "skin": { "index": 0, "generation": 0 }, "bone": "b" }),
        "ParentJoint" => json!({ "joint": "j" }),
        "UiWorldAnchor" => json!({ "target": { "index": 0, "generation": 0 } }),
        "PrefabInstance" => json!({
            "source": "p.prefab.yaml",
            "part": "a",
            "root": { "index": 0, "generation": 0 },
        }),
        _ => json!({}),
    }
}

/// Asserts that every component in `registry` can be built from an empty JSON object `{}` — the
/// drift-safety property snapshot migration depends on (see the [module docs](self)).
///
/// Call this once per registry, typically in its own `#[test]` fn, so a new component that forgets
/// `#[serde(default)]` on a field fails fast and by name, instead of surfacing later as an opaque
/// `L3016` during a `ludion dev` reload.
///
/// # Example
///
/// ```
/// use ludion_engine::builtin_registry;
/// use ludion_testkit::assert_components_drift_safe;
///
/// assert_components_drift_safe(&builtin_registry());
/// ```
///
/// A component missing the discipline panics, naming the field to fix:
///
/// ```should_panic
/// use ludion_ecs::Registry;
/// use ludion_testkit::assert_components_drift_safe;
/// use schemars::JsonSchema;
/// use serde::{Deserialize, Serialize};
///
/// #[derive(Serialize, Deserialize, JsonSchema)]
/// struct Health {
///     hp: u32, // no `#[serde(default)]` — cannot build from `{}`
/// }
///
/// let mut registry = Registry::new();
/// registry.register::<Health>("Health").unwrap();
/// assert_components_drift_safe(&registry); // panics: `Health` is not drift-safe
/// ```
///
/// # Panics
/// If any component fails to deserialize from its `probe_seed` (`{}` for every component but
/// `Joint` and `JointMotor`). The panic message names every offending component (with its serde error) and states
/// the fix: give every field past the seed `#[serde(default)]` (or a `Default` impl backing a
/// hand-written `Deserialize`).
pub fn assert_components_drift_safe(registry: &Registry) {
    let mut failures: Vec<String> = Vec::new();
    for info in registry.iter() {
        if IDENTITY_EXEMPT.contains(&info.name()) {
            continue;
        }
        let mut world = World::new();
        let entity = world.spawn();
        if let Err(err) = info.deserialize_insert(&probe_seed(info.name()), &mut world, entity) {
            failures.push(format!("  - `{}`: {err}", info.name()));
        }
    }
    if !failures.is_empty() {
        panic!(
            "the following components cannot be built from their drift-safety probe seed, so they \
             cannot absorb a future added field during snapshot restore migration:\n{}\n\nfix: give \
             every field past the seed `#[serde(default)]` (or back the component with a `Default` \
             impl and a matching `Deserialize`), so a snapshot taken before the field existed still \
             restores.",
            failures.join("\n")
        );
    }
}

/// Asserts that every entity-reference field of every component in `registry` carries the
/// `EntityRefSchema` / `EntityRefSelfSchema` marker (both are in `ludion::prelude`).
///
/// An `EntityId` field borrows a proxy schema through `#[schemars(with = …)]`; one that is left as
/// a plain `{index, generation}` struct (or sits in a list or map) is invisible to `scene.check`'s
/// range check and to `scene.save`'s remap, and nothing else reports it. This compares each
/// component's `{index, generation}` shapes ([`ludion_ecs::entity_ref_shapes`]) with the fields the
/// registry derived from the marker ([`ComponentInfo::entity_refs`](ludion_ecs::ComponentInfo::entity_refs)).
///
/// # Example
///
/// ```
/// use ludion_engine::builtin_registry;
/// use ludion_testkit::assert_entity_refs_marked;
///
/// assert_entity_refs_marked(&builtin_registry());
/// ```
///
/// A hand-written reference shape panics, naming the component and the suffix to mark:
///
/// ```should_panic
/// use ludion_ecs::Registry;
/// use ludion_testkit::assert_entity_refs_marked;
/// use schemars::JsonSchema;
/// use serde::{Deserialize, Serialize};
///
/// #[derive(Serialize, Deserialize, JsonSchema)]
/// struct Id {
///     index: u32,
///     generation: u32,
/// }
///
/// #[derive(Serialize, Deserialize, JsonSchema)]
/// struct Leash {
///     holder: Id, // not `#[schemars(with = "EntityRefSchema")]`
/// }
///
/// let mut registry = Registry::new();
/// registry.register::<Leash>("Leash").unwrap();
/// assert_entity_refs_marked(&registry); // panics: `Leash` holds an unmarked reference at `/holder`
/// ```
///
/// # Panics
/// If any component holds an entity-reference shape its registry entry does not mark. The message
/// names each component and the unmarked suffixes, and states the fix.
pub fn assert_entity_refs_marked(registry: &Registry) {
    let mut failures: Vec<String> = Vec::new();
    for info in registry.iter() {
        let marked: std::collections::BTreeSet<&str> = info
            .entity_refs()
            .iter()
            .map(|field| field.suffix.as_str())
            .collect();
        let unmarked: Vec<String> = ludion_ecs::entity_ref_shapes(info.schema())
            .into_iter()
            .filter(|suffix| !marked.contains(suffix.as_str()))
            .collect();
        if !unmarked.is_empty() {
            failures.push(format!(
                "  - `{}` holds an entity reference at {} that is not marked",
                info.name(),
                unmarked.join(", ")
            ));
        }
    }
    if !failures.is_empty() {
        panic!(
            "these components hold entity references the engine cannot see, so scene.check's range \
             check and scene.save's remap skip them:\n{}\n\nfix: mark each field with \
             `#[schemars(with = \"EntityRefSchema\")]` (or `EntityRefSelfSchema`, or \
             `Option<…>` of either), both in `ludion::prelude`. A list, map or tuple of references \
             is not supported: hold each in its own field.",
            failures.join("\n")
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use schemars::JsonSchema;
    use serde::{Deserialize, Serialize};

    #[derive(Serialize, Deserialize, JsonSchema)]
    struct DriftSafe {
        #[serde(default)]
        #[allow(dead_code)]
        hp: u32,
    }

    #[derive(Serialize, Deserialize, JsonSchema)]
    struct DriftUnsafe {
        #[allow(dead_code)]
        hp: u32,
    }

    #[test]
    fn passes_for_an_all_defaulted_registry() {
        let mut registry = Registry::new();
        registry.register::<DriftSafe>("DriftSafe").unwrap();
        assert_components_drift_safe(&registry);
    }

    #[test]
    #[should_panic(expected = "`DriftUnsafe`")]
    fn panics_naming_the_offending_component() {
        let mut registry = Registry::new();
        registry.register::<DriftUnsafe>("DriftUnsafe").unwrap();
        assert_components_drift_safe(&registry);
    }

    #[test]
    fn builtin_registry_is_drift_safe() {
        assert_components_drift_safe(&ludion_engine::builtin_registry());
    }

    #[test]
    fn builtin_registry_marks_every_entity_reference() {
        assert_entity_refs_marked(&ludion_engine::builtin_registry());
    }

    #[derive(Serialize, Deserialize, JsonSchema)]
    struct Id {
        index: u32,
        generation: u32,
    }

    fn unmarked_panics<T: ludion_ecs::Component>() -> String {
        let mut registry = Registry::new();
        registry.register::<T>("Holder").unwrap();
        let caught = std::panic::catch_unwind(|| assert_entity_refs_marked(&registry))
            .expect_err("an unmarked reference must panic");
        caught
            .downcast_ref::<String>()
            .cloned()
            .expect("a formatted message")
    }

    #[derive(Serialize, Deserialize, JsonSchema)]
    struct InField {
        holder: Id,
    }
    #[derive(Serialize, Deserialize, JsonSchema)]
    struct InVec {
        holders: Vec<Id>,
    }
    #[derive(Serialize, Deserialize, JsonSchema)]
    struct InMap {
        holders: std::collections::BTreeMap<String, Id>,
    }
    #[derive(Serialize, Deserialize, JsonSchema)]
    struct InTuple {
        pair: (u32, Id),
    }

    #[test]
    fn an_unmarked_proxy_panics_in_a_field_a_list_a_map_and_a_tuple() {
        assert!(
            unmarked_panics::<InField>().contains("`Holder` holds an entity reference at /holder")
        );
        assert!(unmarked_panics::<InVec>().contains("/holders/*"));
        assert!(unmarked_panics::<InMap>().contains("/holders/*"));
        assert!(unmarked_panics::<InTuple>().contains("/pair/1"));
    }

    #[derive(Serialize, Deserialize, JsonSchema)]
    struct Marked {
        #[schemars(with = "ludion_ecs::EntityRefSchema")]
        holder: ludion_core::ids::EntityId,
        #[schemars(with = "Option<ludion_ecs::EntityRefSelfSchema>")]
        maybe: Option<ludion_core::ids::EntityId>,
    }

    #[test]
    fn marked_fields_pass() {
        let mut registry = Registry::new();
        registry.register::<Marked>("Marked").unwrap();
        assert_entity_refs_marked(&registry);
    }
}
