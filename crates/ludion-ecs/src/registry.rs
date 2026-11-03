//! The component registry.
//!
//! [`Registry`] records, per component type, its **name**, its **JSON Schema** (from `schemars`),
//! and a set of **erased serde functions** that operate on a [`World`] without the caller knowing
//! the concrete type. This one table powers scene IO, snapshots, the inspector, and protocol
//! validation from a single source of truth.
//!
//! # Determinism contract
//!
//! [`Registry::iter`] yields components in **registration order**. That order is load-bearing: it
//! is the canonical key order for scene files and for the world hash. Registration is therefore
//! append-only and rejects duplicates (`L0005`) so the order and name set never change out from
//! under a document.
//!
//! # Boundary
//!
//! This module never mentions YAML. It deals in `serde_json::Value`; the scene crate is responsible
//! for translating between YAML documents and those values (architecture doc §2).

use std::any::TypeId;
use std::collections::BTreeMap;
use std::fmt;

use ludion_core::codes::code;
use ludion_core::diagnostics::Diagnostic;
use ludion_core::ids::EntityId;
use serde_json::Value;

use crate::Component;
use crate::EntityRefField;
use crate::World;

/// A component that carries **cross-field semantic invariants** JSON Schema cannot express — e.g. a
/// tilemap whose row strings must all be the same length, or whose glyphs must appear in its legend.
///
/// The registry stores this as an erased `fn(&Value) -> Vec<Diagnostic>` alongside the schema, so
/// scene validation and `component.set` can invoke it **generically, by registry entry, without
/// naming a concrete component type** (the boundary rule: this crate deals in `serde_json::Value`,
/// and the scene crate never names a component). Register such a component with
/// [`Registry::register_validated`].
///
/// # Contract
///
/// - Return an empty vector when `value` is valid (the common case).
/// - Each returned [`Diagnostic`]'s `path`, if set, is a JSON Pointer **relative to the component
///   root** (e.g. `/rows/2`), so the caller can rebase it onto the component's document position.
/// - Be defensive about shape: the value may be schema-invalid (a caller may skip the schema check,
///   or run this first). Guard with `as_object`/`as_array` and return empty on a shape mismatch —
///   the schema check reports those; this hook only adds what the schema cannot.
///
/// # Example
/// ```
/// use ludion_core::codes::code;
/// use ludion_core::diagnostics::Diagnostic;
/// use ludion_ecs::{Registry, SemanticValidate};
/// use schemars::JsonSchema;
/// use serde::{Deserialize, Serialize};
/// use serde_json::{Value, json};
///
/// // A range whose `min` must not exceed `max` — an invariant JSON Schema cannot express.
/// #[derive(Serialize, Deserialize, JsonSchema)]
/// struct Range {
///     min: i64,
///     max: i64,
/// }
///
/// impl SemanticValidate for Range {
///     fn validate_semantics(value: &Value) -> Vec<Diagnostic> {
///         let (Some(min), Some(max)) = (
///             value.get("min").and_then(Value::as_i64),
///             value.get("max").and_then(Value::as_i64),
///         ) else {
///             return Vec::new(); // shape problems are the schema check's job, not ours
///         };
///         if min > max {
///             vec![Diagnostic::new(code::SCENE_COMPONENT_MISMATCH, "min exceeds max").with_path("/min")]
///         } else {
///             Vec::new()
///         }
///     }
/// }
///
/// let mut registry = Registry::new();
/// registry.register_validated::<Range>("Range").unwrap();
/// let info = registry.get("Range").unwrap();
/// assert!(info.validate_semantics(&json!({ "min": 1, "max": 3 })).is_empty());
/// assert_eq!(info.validate_semantics(&json!({ "min": 5, "max": 2 })).len(), 1);
/// ```
pub trait SemanticValidate {
    /// Checks `value`'s cross-field invariants, returning one diagnostic per violation (relative
    /// JSON Pointers). Empty means valid.
    fn validate_semantics(value: &Value) -> Vec<Diagnostic>;
}

/// The no-op semantic validator: the default for components registered without one.
fn no_semantic_validation(_value: &Value) -> Vec<Diagnostic> {
    Vec::new()
}

/// The erased zero-copy read behind [`ComponentInfo::serialize_erased`]: borrows a component off an
/// entity as a `serde::Serialize` trait object. Spelled as an alias because the higher-ranked bound
/// tying the borrow to the world's is unreadable inline.
type SerializeErasedFn =
    for<'w> fn(&'w World, EntityId) -> Result<Option<&'w dyn erased_serde::Serialize>, Diagnostic>;

/// Everything the registry knows about one component type.
///
/// The erased operations are exposed as methods that inject the registered component name into any
/// diagnostic, so error messages always name the offending component.
pub struct ComponentInfo {
    name: String,
    type_id: TypeId,
    schema: Value,
    /// The entity-reference fields marked in `schema`, derived once at registration.
    entity_refs: Vec<EntityRefField>,
    /// The asset-path field patterns marked in `schema`, derived once at registration.
    asset_paths: Vec<String>,
    /// Whether the component may be authored as a scene `components:` block. `false` marks a
    /// *field-backed* built-in — one a scene document projects into from a dedicated field (e.g.
    /// `Name` from an entity's `name:`) — so declaring it as a component block is rejected, keeping
    /// exactly one authoring channel (the regularity pillar). Reserved components are still ordinary
    /// registered components everywhere else: they enter the world hash and snapshots normally.
    reserved: bool,
    /// Set by [`Registry::mark_sim_only`].
    sim_only: bool,
    serialize_fn: fn(&World, EntityId) -> Result<Option<Value>, Diagnostic>,
    serialize_erased_fn: SerializeErasedFn,
    deserialize_insert_fn: fn(&Value, &mut World, EntityId) -> Result<(), Diagnostic>,
    remove_fn: fn(&mut World, EntityId) -> Result<(), Diagnostic>,
    has_fn: fn(&World, EntityId) -> bool,
    /// Cross-field semantic validation ([`SemanticValidate`]) — the no-op for components registered
    /// without one.
    validate_fn: fn(&Value) -> Vec<Diagnostic>,
}

impl ComponentInfo {
    /// The registered name.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The `TypeId` of the underlying Rust type.
    pub fn type_id(&self) -> TypeId {
        self.type_id
    }

    /// The component's JSON Schema (as produced by `schemars::schema_for!`).
    pub fn schema(&self) -> &Value {
        &self.schema
    }

    /// The component's entity-reference fields — those whose schema carries the
    /// [`ENTITY_REF_KEYWORD`](crate::ENTITY_REF_KEYWORD) marker (see
    /// [`EntityRefSchema`](crate::EntityRefSchema)), sorted by suffix. Scene validation range-checks
    /// them and `scene.save` remaps them.
    pub fn entity_refs(&self) -> &[EntityRefField] {
        &self.entity_refs
    }

    /// The component's asset-path field patterns — JSON Pointer suffixes (with `*` for every map
    /// value or list element) of fields whose schema carries the
    /// [`ASSET_PATH_KEYWORD`](crate::ASSET_PATH_KEYWORD) marker (see
    /// [`AssetPathSchema`](crate::AssetPathSchema)), sorted.
    pub fn asset_paths(&self) -> &[String] {
        &self.asset_paths
    }

    /// Hands each asset-path string present in `value` (this component's serialized value) to `f`
    /// with its concrete JSON Pointer, for in-place rewriting; see [`crate::for_each_asset_path`].
    pub fn for_each_asset_path(&self, value: &mut Value, f: &mut dyn FnMut(&str, &mut String)) {
        crate::for_each_asset_path(&self.asset_paths, value, f);
    }

    /// Whether this component is **reserved** — a field-backed built-in that must not appear as a
    /// scene `components:` block (see the [`reserved`](ComponentInfo::reserved) field docs). Scene
    /// validation and instantiation honor this; the world hash and snapshots do not (a reserved
    /// component is world content like any other).
    pub fn reserved(&self) -> bool {
        self.reserved
    }

    /// Whether only the sim reads this component (see [`Registry::mark_sim_only`]).
    pub fn sim_only(&self) -> bool {
        self.sim_only
    }

    /// Serializes this component off `entity` to a JSON value, or `Ok(None)` if the entity does not
    /// have it.
    ///
    /// # Errors
    /// `L0002` if `entity` is not alive; `L0007` if the component's own serialization fails.
    pub fn serialize(&self, world: &World, entity: EntityId) -> Result<Option<Value>, Diagnostic> {
        (self.serialize_fn)(world, entity).map_err(|d| self.tag(d))
    }

    /// Borrows this component off `entity` as a **type-erased `serde::Serialize`**, or `Ok(None)` if
    /// the entity does not have it.
    ///
    /// The zero-copy counterpart to [`serialize`](ComponentInfo::serialize): the caller drives its
    /// own `serde::Serializer` over the borrowed component instead of receiving a
    /// `serde_json::Value` built for it. Reach for it when the JSON tree is a pure intermediate that
    /// would be walked and dropped — the world hash serializes every component of every entity every
    /// frame, so building one is its dominant cost. Anything that needs an owned, inspectable tree
    /// (scene IO, snapshots, the inspector) wants `serialize`.
    ///
    /// Serialization has not run yet when this returns, so a component whose own `Serialize` fails
    /// reports through the caller's serializer; [`serialize_failed`](ComponentInfo::serialize_failed)
    /// turns that error into the same `L0007` `serialize` would have raised.
    ///
    /// # Errors
    /// `L0002` if `entity` is not alive.
    pub fn serialize_erased<'w>(
        &self,
        world: &'w World,
        entity: EntityId,
    ) -> Result<Option<&'w dyn erased_serde::Serialize>, Diagnostic> {
        (self.serialize_erased_fn)(world, entity).map_err(|d| self.tag(d))
    }

    /// The `L0007` this component's serialization failure raises, naming the component and `entity`.
    ///
    /// One owner for that message, shared by [`serialize`](ComponentInfo::serialize) (which raises it
    /// from inside its erased body) and by callers that drive their own serializer over
    /// [`serialize_erased`](ComponentInfo::serialize_erased) and must report the identical
    /// diagnostic.
    pub fn serialize_failed(&self, entity: EntityId, error: &dyn fmt::Display) -> Diagnostic {
        self.tag(serialize_failure(entity, error))
    }

    /// Deserializes `value` into this component and inserts it onto `entity`.
    ///
    /// # Errors
    /// `L0002` if `entity` is not alive; `L0006` if `value` does not match the component's shape.
    pub fn deserialize_insert(
        &self,
        value: &Value,
        world: &mut World,
        entity: EntityId,
    ) -> Result<(), Diagnostic> {
        (self.deserialize_insert_fn)(value, world, entity).map_err(|d| self.tag(d))
    }

    /// Removes this component from `entity`.
    ///
    /// # Errors
    /// `L0002` if `entity` is not alive.
    pub fn remove(&self, world: &mut World, entity: EntityId) -> Result<(), Diagnostic> {
        (self.remove_fn)(world, entity).map_err(|d| self.tag(d))
    }

    /// Whether `entity` has this component.
    pub fn has(&self, world: &World, entity: EntityId) -> bool {
        (self.has_fn)(world, entity)
    }

    /// Runs this component's cross-field semantic validation ([`SemanticValidate`]) against a JSON
    /// value, returning one diagnostic per violation (empty = valid, and the default for any
    /// component registered without a validator). Diagnostic paths are JSON Pointers relative to the
    /// component root — the caller rebases them onto the document position.
    pub fn validate_semantics(&self, value: &Value) -> Vec<Diagnostic> {
        (self.validate_fn)(value)
    }

    /// Prepends the registered component name to a diagnostic's message.
    fn tag(&self, mut diagnostic: Diagnostic) -> Diagnostic {
        diagnostic.message = format!("component `{}`: {}", self.name, diagnostic.message);
        diagnostic
    }
}

/// The component registry: name/type ⇒ schema + erased serde functions, in registration order.
///
/// # Example
/// ```
/// use ludion_ecs::{Registry, World};
/// use serde::{Deserialize, Serialize};
/// use schemars::JsonSchema;
///
/// #[derive(Serialize, Deserialize, JsonSchema, PartialEq, Debug)]
/// struct Health(u32);
///
/// let mut registry = Registry::new();
/// registry.register::<Health>("Health").unwrap();
///
/// let mut world = World::new();
/// let e = world.spawn();
/// world.insert(e, Health(100)).unwrap();
///
/// // Round-trip a live component through JSON via the registry.
/// let value = registry.serialize("Health", &world, e).unwrap().unwrap();
/// let mut fresh = World::new();
/// let e2 = fresh.spawn();
/// registry.deserialize_insert("Health", &value, &mut fresh, e2).unwrap();
/// assert_eq!(fresh.get::<Health>(e2).unwrap(), Some(&Health(100)));
/// ```
#[derive(Default)]
pub struct Registry {
    entries: Vec<ComponentInfo>,
    by_name: BTreeMap<String, usize>,
    by_type: BTreeMap<TypeId, usize>,
}

impl Registry {
    /// Creates an empty registry.
    pub fn new() -> Self {
        Self::default()
    }

    /// Registers component `T` under `name`.
    ///
    /// # Errors
    /// `L0005` if `name` is already taken, or if `T` is already registered (under any name).
    /// `L1392` if a marked entity reference sits inside a list or map of `T`.
    pub fn register<T: Component>(&mut self, name: &str) -> Result<(), Diagnostic> {
        self.register_with::<T>(name, false, no_semantic_validation)
    }

    /// Registers component `T` under `name` **with a semantic validator** — a component whose
    /// [`SemanticValidate`] impl carries cross-field invariants JSON Schema cannot express (see
    /// [`SemanticValidate`]). Identical to [`register`](Registry::register) in every other respect.
    ///
    /// # Errors
    /// `L0005` if `name` is already taken, or if `T` is already registered (under any name).
    /// `L1392` if a marked entity reference sits inside a list or map of `T`.
    pub fn register_validated<T: Component + SemanticValidate>(
        &mut self,
        name: &str,
    ) -> Result<(), Diagnostic> {
        self.register_with::<T>(name, false, T::validate_semantics)
    }

    /// Registers a **reserved** component `T` under `name` — a field-backed built-in that a scene
    /// document projects into from a dedicated field, never from a `components:` block (see
    /// [`ComponentInfo::reserved`]). Identical to [`register`](Registry::register) in every other
    /// respect: `T` still hashes and snapshots like any registered component.
    ///
    /// # Errors
    /// `L0005` if `name` is already taken, or if `T` is already registered (under any name).
    /// `L1392` if a marked entity reference sits inside a list or map of `T`.
    pub fn register_reserved<T: Component>(&mut self, name: &str) -> Result<(), Diagnostic> {
        self.register_with::<T>(name, true, no_semantic_validation)
    }

    fn register_with<T: Component>(
        &mut self,
        name: &str,
        reserved: bool,
        validate_fn: fn(&Value) -> Vec<Diagnostic>,
    ) -> Result<(), Diagnostic> {
        let type_id = TypeId::of::<T>();
        if self.by_name.contains_key(name) {
            return Err(duplicate_name(name));
        }
        if let Some(&existing) = self.by_type.get(&type_id) {
            return Err(duplicate_type(name, &self.entries[existing].name));
        }
        let schema = serde_json::to_value(schemars::schema_for!(T))
            .expect("a schemars JSON Schema always serializes to a JSON value");
        if let Some(suffix) = crate::entity_refs_in_collections(&schema).first() {
            return Err(entity_ref_in_collection(name, suffix));
        }
        let entity_refs = crate::entity_ref_fields(&schema);
        let asset_paths = crate::asset_path_fields(&schema);
        let index = self.entries.len();
        self.entries.push(ComponentInfo {
            name: name.to_string(),
            type_id,
            schema,
            entity_refs,
            asset_paths,
            reserved,
            sim_only: false,
            serialize_fn: erased_serialize::<T>,
            serialize_erased_fn: erased_serialize_ref::<T>,
            deserialize_insert_fn: erased_deserialize_insert::<T>,
            remove_fn: erased_remove::<T>,
            has_fn: erased_has::<T>,
            validate_fn,
        });
        self.by_name.insert(name.to_string(), index);
        self.by_type.insert(type_id, index);
        Ok(())
    }

    /// Marks the registered component `name` **sim-only**: the sim must see it, so a presentation
    /// entity carrying it would be dead weight — no sim system can read it (`L0028`); a presentation
    /// prefab instance refuses it (`L1320`). Games mark their own sim-only components the same way.
    ///
    /// # Errors
    /// `L0004` if `name` is not registered.
    ///
    /// # Example
    /// ```
    /// use ludion_ecs::Registry;
    /// use serde::{Deserialize, Serialize};
    /// use schemars::JsonSchema;
    ///
    /// #[derive(Serialize, Deserialize, JsonSchema)]
    /// struct Body(u32);
    ///
    /// let mut registry = Registry::new();
    /// registry.register::<Body>("Body").unwrap();
    /// assert!(!registry.get("Body").unwrap().sim_only());
    /// registry.mark_sim_only("Body").unwrap();
    /// assert!(registry.get("Body").unwrap().sim_only());
    /// assert!(registry.mark_sim_only("Nope").is_err());
    /// ```
    pub fn mark_sim_only(&mut self, name: &str) -> Result<(), Diagnostic> {
        let Some(&index) = self.by_name.get(name) else {
            return Err(self.unknown_component(name));
        };
        self.entries[index].sim_only = true;
        Ok(())
    }

    /// Looks up a component by name.
    pub fn get(&self, name: &str) -> Option<&ComponentInfo> {
        self.by_name.get(name).map(|&i| &self.entries[i])
    }

    /// Looks up a component by a runtime [`TypeId`] — the resolution seam for a declared query shape,
    /// whose element types are known only as `TypeId`s (from [`QueryElemShape`](crate::QueryElemShape)),
    /// not as concrete `T`s, at the point they must map to registry names.
    pub fn get_by_type_id(&self, type_id: TypeId) -> Option<&ComponentInfo> {
        self.by_type.get(&type_id).map(|&i| &self.entries[i])
    }

    /// Iterates components in **registration order** (the canonical key order).
    pub fn iter(&self) -> impl Iterator<Item = &ComponentInfo> {
        self.entries.iter()
    }

    /// The `help` line for a name this registry does not know: the nearest registered name when one
    /// is close enough, otherwise the registered names themselves (truncated past eight).
    ///
    /// One owner, because the same "you named a component that is not registered" moment is reached
    /// from three directions — the registry's own lookups (`L0004`), the dispatcher's query filters
    /// (`L0004`), and scene validation (`L1013`) — and a reader who typo'd `Trasnform` deserves the
    /// same suggestion from all three.
    pub fn name_suggestion(&self, name: &str) -> String {
        match ludion_core::suggest::nearest(name, self.iter().map(ComponentInfo::name)) {
            Some(candidate) => format!("nearest match is `{candidate}`"),
            None => {
                let mut names: Vec<&str> = self.iter().map(ComponentInfo::name).collect();
                let listed = if names.len() > 8 {
                    names.truncate(8);
                    format!("{}, …", names.join(", "))
                } else {
                    names.join(", ")
                };
                format!("registered components are: {listed}")
            }
        }
    }

    /// The `L0004` diagnostic for `name`, carrying [`name_suggestion`](Self::name_suggestion)'s
    /// help. Every `L0004` raise site builds it here, so one code means one message.
    pub fn unknown_component(&self, name: &str) -> Diagnostic {
        Diagnostic::new(
            code::UNKNOWN_COMPONENT,
            format!("no component is registered under the name `{name}`"),
        )
        .with_help(self.name_suggestion(name))
    }

    /// The number of registered components.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether nothing is registered.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Serializes component `name` off `entity`.
    ///
    /// # Errors
    /// `L0004` if `name` is not registered; otherwise as [`ComponentInfo::serialize`].
    pub fn serialize(
        &self,
        name: &str,
        world: &World,
        entity: EntityId,
    ) -> Result<Option<Value>, Diagnostic> {
        self.get(name)
            .ok_or_else(|| self.unknown_component(name))?
            .serialize(world, entity)
    }

    /// Deserializes `value` into component `name` and inserts it onto `entity`.
    ///
    /// # Errors
    /// `L0004` if `name` is not registered; otherwise as [`ComponentInfo::deserialize_insert`].
    pub fn deserialize_insert(
        &self,
        name: &str,
        value: &Value,
        world: &mut World,
        entity: EntityId,
    ) -> Result<(), Diagnostic> {
        self.get(name)
            .ok_or_else(|| self.unknown_component(name))?
            .deserialize_insert(value, world, entity)
    }
}

fn erased_serialize<T: Component>(
    world: &World,
    entity: EntityId,
) -> Result<Option<Value>, Diagnostic> {
    match world.get::<T>(entity)? {
        Some(component) => serde_json::to_value(component)
            .map(Some)
            .map_err(|err| serialize_failure(entity, &err)),
        None => Ok(None),
    }
}

fn erased_serialize_ref<T: Component>(
    world: &World,
    entity: EntityId,
) -> Result<Option<&dyn erased_serde::Serialize>, Diagnostic> {
    Ok(world
        .get::<T>(entity)?
        .map(|component| component as &dyn erased_serde::Serialize))
}

/// The untagged `L0007` body; [`ComponentInfo::tag`] adds the component name.
fn serialize_failure(entity: EntityId, error: &dyn fmt::Display) -> Diagnostic {
    Diagnostic::new(
        code::COMPONENT_SERIALIZE_FAILED,
        format!("failed to serialize for entity {entity}: {error}"),
    )
}

fn erased_deserialize_insert<T: Component>(
    value: &Value,
    world: &mut World,
    entity: EntityId,
) -> Result<(), Diagnostic> {
    let component: T = T::deserialize(value).map_err(|err| {
        Diagnostic::new(
            code::COMPONENT_DESERIALIZE_FAILED,
            format!("failed to deserialize for entity {entity}: {err}"),
        )
        .with_help(
            "check the value against the component's JSON Schema (field names, types, required fields)",
        )
    })?;
    world.insert::<T>(entity, component).map(|_| ())
}

fn erased_remove<T: Component>(world: &mut World, entity: EntityId) -> Result<(), Diagnostic> {
    world.remove::<T>(entity).map(|_| ())
}

fn erased_has<T: Component>(world: &World, entity: EntityId) -> bool {
    world.has::<T>(entity)
}

fn duplicate_name(name: &str) -> Diagnostic {
    Diagnostic::new(
        code::DUPLICATE_COMPONENT_REGISTRATION,
        format!("a component is already registered under the name `{name}`"),
    )
    .with_help("choose a unique component name; each name maps to exactly one component")
}

fn entity_ref_in_collection(name: &str, suffix: &str) -> Diagnostic {
    Diagnostic::new(
        code::ENTITY_REF_IN_COLLECTION,
        format!("component `{name}` marks an entity reference inside a list or map, at `{suffix}`"),
    )
    .with_help(
        "a list or map of entity references is not supported: hold each reference in its own \
         field, or derive the collection from a query",
    )
}

fn duplicate_type(new_name: &str, existing_name: &str) -> Diagnostic {
    Diagnostic::new(
        code::DUPLICATE_COMPONENT_REGISTRATION,
        format!(
            "this component type is already registered as `{existing_name}`; it cannot also be \
             registered as `{new_name}`"
        ),
    )
    .with_help("register each component type exactly once")
}
