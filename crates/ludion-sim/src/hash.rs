//! `WorldHash` — the agent-facing determinism verification primitive.
//!
//! A [`WorldHash`] is a 64-bit XXH3 digest over a **canonical byte serialization** of the world's
//! entities and component data. Same world state ⇒ same hash, across runs, processes, and
//! platforms; any divergence in entity identity or component data ⇒ (with overwhelming
//! probability) a different hash. It is the value `ludion sim-hash` prints, `sim.hash` returns
//! over the wire, and the in-process double-run CI test compares per frame.
//!
//! # Byte-level hashing rules (normative)
//!
//! The hashed byte stream is defined exactly, so independent implementations agree:
//!
//! 1. **Hash function:** XXH3-64, seed 0, over one contiguous stream.
//! 2. **Preamble:** the ASCII bytes `ludion.worldhash.v2` (domain separation + rule versioning —
//!    any change to these rules bumps the suffix and re-blesses recorded hashes).
//! 3. **Entities:** every *live* entity, in **ascending slot-index order**
//!    ([`World::entities`]): the byte `E`, then the entity's index as `u32` little-endian, then
//!    its generation as `u32` little-endian. Including the generation makes
//!    despawn-then-respawn-identically observable.
//!    Presentation entities (index at or past `PRESENTATION_BASE`) and their components are not
//!    walked: the domain tag is unchanged, and a world without them hashes as it always did.
//! 4. **Components:** for each entity, every **registered** component the entity has, in
//!    **registry registration order** ([`Registry::iter`]): the byte `C`, then the length-prefixed
//!    component name, then the canonical encoding of the component's JSON value. Unregistered
//!    component types are invisible to the hash, exactly as they are invisible to scenes and
//!    snapshots.
//! 5. **Canonical JSON-value encoding** (prefix-free; independent of `serde_json` configuration):
//!    - length prefixes and element counts are `u64` little-endian;
//!    - `null` (unit) ⇒ `n`; `None` ⇒ `o`; `false` ⇒ `f`; `true` ⇒ `t`;
//!    - number ⇒ `#` + length-prefixed decimal string as `serde_json` formats it (itoa for
//!      integers, **shortest round-trip** for floats — so every distinct finite float bit-pattern,
//!      including `-0.0` vs `0.0`, yields distinct bytes; an `f32` widens exactly to `f64` first,
//!      as a `serde_json::Value` built from the same number does);
//!    - non-finite float ⇒ `#` + length-prefixed `NaN`, `inf` or `-inf`. A finite number's text
//!      never contains those letters, so no collision; a NaN's sign and payload are deliberately
//!      not hashed, because x86 and ARM produce different default NaN bits and hashing them would
//!      break cross-arch identity;
//!    - string ⇒ `s` + length-prefixed UTF-8 bytes;
//!    - array ⇒ `[` + element count + each element's encoding, in order;
//!    - object ⇒ `{` + entry count + entries **sorted by key bytes**, each as length-prefixed key
//!      then value encoding. (Struct fields therefore hash in a fixed order regardless of
//!      declaration order or `serde_json` map internals.)
//!
//! The value being encoded is the one a component's `Serialize` describes — the same value
//! `serde_json::to_value` would have built. The private `canonical` submodule holds the encoder that
//! writes these bytes straight out of a component, and the full list of equivalences it maintains
//! with that `Value`; the rules above are what it must produce, and the oracle test below is what
//! pins it there.
//!
//! **Deliberately excluded:** resources (including the `DeterministicRng` state) and the free-list
//! shape. The hash covers observable world *content*, per the architecture's definition
//! ("canonical world serialization: registry order, entity-index order"); RNG state and allocator
//! internals are covered by snapshots, and any divergence in them that ever matters must surface
//! as diverging component data — which the per-frame hash sequence catches.

mod canonical;

use std::fmt;

use canonical::Canonical;
use ludion_core::diagnostics::Diagnostic;
use ludion_core::ids::EntityId;
use ludion_ecs::{Registry, World};
use xxhash_rust::xxh3::Xxh3;

/// The rule-versioned domain-separation preamble (rule 2 above).
const DOMAIN_TAG: &[u8] = b"ludion.worldhash.v2";

/// The first component a hash walk found holding a non-finite float, and how many it found in all.
/// See [`WorldHash::compute_with_report`].
pub struct NonFinite {
    /// The first entity found holding one.
    pub entity: EntityId,
    /// The registered name of that component.
    pub component: String,
    /// Non-finite values met in the whole walk, across every component.
    pub count: u64,
}

/// A [`WorldHash`] and, when the walk met a non-finite float, where. See
/// [`WorldHash::compute_with_report`].
pub struct HashReport {
    /// The world's hash, identical to [`WorldHash::compute`].
    pub hash: WorldHash,
    /// `None` when every float the walk met was finite.
    pub non_finite: Option<NonFinite>,
}

/// A stable 64-bit digest of the world's entities and component data.
///
/// Displayed — and serialized — as exactly 16 lowercase hex digits, so hashes compare as strings
/// everywhere (CLI output, JSON, logs) and never lose precision in JSON consumers that read
/// numbers as `f64`.
///
/// # Example
/// ```
/// use ludion_ecs::{Registry, World};
/// use ludion_sim::WorldHash;
/// use schemars::JsonSchema;
/// use serde::{Deserialize, Serialize};
///
/// #[derive(Serialize, Deserialize, JsonSchema)]
/// struct Position { x: f32, y: f32 }
///
/// let mut registry = Registry::new();
/// registry.register::<Position>("Position").unwrap();
///
/// let mut world = World::new();
/// let e = world.spawn();
/// world.insert(e, Position { x: 1.0, y: 2.0 }).unwrap();
///
/// let a = WorldHash::compute(&world, &registry).unwrap();
/// let b = WorldHash::compute(&world, &registry).unwrap();
/// assert_eq!(a, b); // same state, same hash — always
///
/// world.get_mut::<Position>(e).unwrap().unwrap().x = 1.5;
/// let c = WorldHash::compute(&world, &registry).unwrap();
/// assert_ne!(a, c); // one component field changed, the hash changed
///
/// assert_eq!(a.to_string().len(), 16); // 16 lowercase hex digits
/// ```
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct WorldHash(u64);

impl WorldHash {
    /// Computes the hash of `world`'s live entities and their registered components, per the
    /// module-level byte rules.
    ///
    /// # Errors
    /// Propagates a component's own serialization failure (`L0007`). Dead-entity errors are
    /// impossible here: enumeration only yields live entities.
    pub fn compute(world: &World, registry: &Registry) -> Result<WorldHash, Diagnostic> {
        Self::compute_with_report(world, registry).map(|report| report.hash)
    }

    /// [`compute`](Self::compute), plus where the walk first met a non-finite float. The hash
    /// encodes NaN, `+inf` and `-inf` distinctly (rule 5) and never fails on them; the report is how
    /// a caller learns one reached sim state.
    ///
    /// # Errors
    /// As [`compute`](Self::compute).
    ///
    /// # Example
    /// ```
    /// use ludion_ecs::{Registry, World};
    /// use ludion_sim::WorldHash;
    /// use schemars::JsonSchema;
    /// use serde::{Deserialize, Serialize};
    ///
    /// #[derive(Serialize, Deserialize, JsonSchema)]
    /// struct Speed(f32);
    ///
    /// let mut registry = Registry::new();
    /// registry.register::<Speed>("Speed").unwrap();
    /// let mut world = World::new();
    /// let e = world.spawn();
    /// world.insert(e, Speed(1.0)).unwrap();
    /// assert!(WorldHash::compute_with_report(&world, &registry).unwrap().non_finite.is_none());
    ///
    /// world.get_mut::<Speed>(e).unwrap().unwrap().0 = f32::NAN;
    /// let report = WorldHash::compute_with_report(&world, &registry).unwrap();
    /// let site = report.non_finite.unwrap();
    /// assert_eq!((site.entity, site.component.as_str(), site.count), (e, "Speed", 1));
    /// ```
    pub fn compute_with_report(
        world: &World,
        registry: &Registry,
    ) -> Result<HashReport, Diagnostic> {
        let mut non_finite: Option<NonFinite> = None;
        let mut hasher = Xxh3::new();
        // One encoder for the whole world: its buffers grow to the largest component and are then
        // reused, so encoding a world allocates nothing after the first few components.
        let mut encoder = Canonical::default();
        hasher.update(DOMAIN_TAG);
        // Presence is read from one ascending cursor per registered component
        // ([`World::entity_indices_with`]), advanced in step with the entity walk, instead of
        // probing every storage for every entity: the emission order is unchanged (entity-major,
        // components in registration order) but a pair that is absent costs an integer comparison
        // rather than a map probe, and only components actually present are looked up.
        let mut cursors: Vec<_> = registry
            .iter()
            .map(|info| world.entity_indices_with(info.type_id()).peekable())
            .collect();
        for entity in world.entities() {
            hasher.update(b"E");
            hasher.update(&entity.index().to_le_bytes());
            hasher.update(&entity.generation().to_le_bytes());
            for (info, cursor) in registry.iter().zip(cursors.iter_mut()) {
                // Both sequences ascend, so a row below the current entity can never be reached
                // again. (Despawn drops rows, so this loop is normally a no-op.)
                while cursor.next_if(|&index| index < entity.index()).is_some() {}
                if cursor.next_if_eq(&entity.index()).is_none() {
                    continue;
                }
                if let Some(component) = info.serialize_erased(world, entity)? {
                    hasher.update(b"C");
                    update_bytes(&mut hasher, info.name().as_bytes());
                    let bytes = encoder
                        .encode(component)
                        .map_err(|err| info.serialize_failed(entity, &err))?;
                    hasher.update(bytes);
                    let found = encoder.non_finite();
                    if found > 0 {
                        match &mut non_finite {
                            Some(site) => site.count += found,
                            None => {
                                non_finite = Some(NonFinite {
                                    entity,
                                    component: info.name().to_string(),
                                    count: found,
                                });
                            }
                        }
                    }
                }
            }
        }
        Ok(HashReport {
            hash: WorldHash(hasher.digest()),
            non_finite,
        })
    }

    /// The raw 64-bit digest.
    pub const fn as_u64(self) -> u64 {
        self.0
    }

    /// Parses the canonical 16-hex-digit form (as produced by `Display`), returning `None` for
    /// anything else.
    ///
    /// ```
    /// use ludion_sim::WorldHash;
    /// let h = WorldHash::parse("00000000deadbeef").unwrap();
    /// assert_eq!(h.as_u64(), 0xdead_beef);
    /// assert!(WorldHash::parse("0xdeadbeef").is_none());
    /// ```
    pub fn parse(text: &str) -> Option<WorldHash> {
        if text.len() != 16 || !text.bytes().all(|b| b.is_ascii_hexdigit()) {
            return None;
        }
        u64::from_str_radix(text, 16).ok().map(WorldHash)
    }
}

impl fmt::Display for WorldHash {
    /// Exactly 16 lowercase hex digits, zero-padded — e.g. `04c11db700000042`.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:016x}", self.0)
    }
}

impl fmt::Debug for WorldHash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "WorldHash({self})")
    }
}

impl serde::Serialize for WorldHash {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.to_string())
    }
}

impl<'de> serde::Deserialize<'de> for WorldHash {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s = String::deserialize(deserializer)?;
        WorldHash::parse(&s).ok_or_else(|| {
            serde::de::Error::custom(format!("invalid world hash '{s}', expected 16 hex digits"))
        })
    }
}

/// Feeds a `u64`-little-endian length prefix followed by the bytes themselves (rule 5).
fn update_bytes(hasher: &mut Xxh3, bytes: &[u8]) {
    hasher.update(&(bytes.len() as u64).to_le_bytes());
    hasher.update(bytes);
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;
    use serde::Serialize;
    use serde_json::{Value, json};

    /// Hashes anything serializable through the production encoder (no world involved).
    fn digest(value: &dyn erased_serde::Serialize) -> u64 {
        let mut hasher = Xxh3::new();
        let mut encoder = Canonical::default();
        hasher.update(encoder.encode(value).expect("the corpus serializes"));
        hasher.digest()
    }

    /// Hashes a JSON value through the production encoder. `Value`'s own `Serialize` emits exactly
    /// the events that rebuild it, so this is the encoder meeting the oracle on the oracle's terms.
    fn value_digest(value: &Value) -> u64 {
        digest(value)
    }

    /// The encoder as it stood before it became a streaming `Serializer` — a walk over a
    /// materialized `serde_json::Value`, kept verbatim as this module's oracle, since a one-bit
    /// change here breaks every committed replay and the cross-architecture determinism guarantee.
    fn update_value_oracle(hasher: &mut Xxh3, value: &Value) {
        match value {
            Value::Null => hasher.update(b"n"),
            Value::Bool(false) => hasher.update(b"f"),
            Value::Bool(true) => hasher.update(b"t"),
            Value::Number(number) => {
                hasher.update(b"#");
                update_bytes(hasher, number.to_string().as_bytes());
            }
            Value::String(string) => {
                hasher.update(b"s");
                update_bytes(hasher, string.as_bytes());
            }
            Value::Array(items) => {
                hasher.update(b"[");
                hasher.update(&(items.len() as u64).to_le_bytes());
                for item in items {
                    update_value_oracle(hasher, item);
                }
            }
            Value::Object(map) => {
                hasher.update(b"{");
                hasher.update(&(map.len() as u64).to_le_bytes());
                let mut entries: Vec<(&String, &Value)> = map.iter().collect();
                entries.sort_by(|a, b| a.0.cmp(b.0));
                for (key, entry) in entries {
                    update_bytes(hasher, key.as_bytes());
                    update_value_oracle(hasher, entry);
                }
            }
        }
    }

    fn oracle_digest(value: &Value) -> u64 {
        let mut hasher = Xxh3::new();
        update_value_oracle(&mut hasher, value);
        hasher.digest()
    }

    /// Every JSON shape the encoder can meet hashes to the same bytes as the oracle.
    ///
    /// Covers each arm, and leans on what the change actually touched: the encoder no longer walks a
    /// `serde_json::Value` at all, it consumes serde events and buffers/sorts object entries itself.
    /// So the corpus carries every number kind (integers at the `i64`/`u64` edges, `-0.0` vs `0.0`,
    /// adjacent float bit patterns, exponent-form floats) and nests deeply enough that a leaked
    /// buffer or a mis-scoped frame would show up. It also pins the surrounding rules the oracle
    /// encodes: key order (the same object authored three ways), and strings needing JSON escaping —
    /// which the encoder deliberately hashes raw.
    #[test]
    fn the_encoder_is_bit_identical_to_the_pre_optimization_oracle() {
        let x = 2.0_f32;
        let corpus = vec![
            json!(null),
            json!(true),
            json!(false),
            json!(0),
            json!(-1),
            json!(i64::MIN),
            json!(u64::MAX),
            json!(0.0_f64),
            json!(-0.0_f64),
            json!(x),
            json!(f32::from_bits(x.to_bits() + 1)),
            json!(1.0e300_f64),
            json!(-1.5e-7_f64),
            json!(f64::MIN_POSITIVE),
            json!(""),
            json!("plain"),
            json!("needs \"escaping\"\n\t\\ and non-ascii: héllo ☃"),
            json!([]),
            json!({}),
            json!([1, "two", 3.5, null, true, [], {}]),
            json!({"a": 1, "b": 2, "c": 3}),
            // The same object, keys authored in reverse: both must reach the oracle's sorted bytes.
            json!({"c": 3, "b": 2, "a": 1}),
            json!({"b": 2, "a": 1, "c": 3}),
            // Keys that are not sorted by naive length or case-insensitive order.
            json!({"B": 1, "a": 2, "aa": 3, "A": 4, "": 5}),
            // Deep nesting with numbers at every level: numbers are leaves, so one scratch buffer
            // must serve them all without bleeding between siblings.
            json!({
                "outer": {
                    "z": [1.25, {"y": -0.0, "x": [0.0, {"w": 1.0e300}]}],
                    "a": {"nested": {"deep": [[[[42]]]], "float": -1.5e-7}},
                },
                "list": [{"k": 1}, {"k": 2}, {"k": 3}],
            }),
        ];
        for value in &corpus {
            assert_eq!(
                value_digest(value),
                oracle_digest(value),
                "encoder drifted from the oracle on {value}",
            );
        }
        // The corpus is not trivially all-equal — a broken encoder could hide behind that. Every
        // entry is distinct except the three deliberate permutations of `{a,b,c}`, which must
        // collapse to one digest: that collapse IS the sorted-key rule.
        let digests: std::collections::BTreeSet<u64> = corpus.iter().map(value_digest).collect();
        assert_eq!(digests.len(), corpus.len() - 2);
        assert_eq!(
            value_digest(&json!({"a": 1, "b": 2, "c": 3})),
            value_digest(&json!({"c": 3, "b": 2, "a": 1})),
        );
    }

    /// `WorldHash::compute`, verbatim as it stood before the optimization: probe every registered
    /// component of every live entity, materialize its `serde_json::Value`, walk that with the
    /// oracle encoder. The second half of this module's oracle — it pins the *walk* (entity order,
    /// registration order, presence) the way `update_value_oracle` pins the *encoding*.
    fn world_hash_oracle(world: &World, registry: &Registry) -> WorldHash {
        let mut hasher = Xxh3::new();
        hasher.update(DOMAIN_TAG);
        for entity in world.entities() {
            hasher.update(b"E");
            hasher.update(&entity.index().to_le_bytes());
            hasher.update(&entity.generation().to_le_bytes());
            for info in registry.iter() {
                if let Some(value) = info.serialize(world, entity).unwrap() {
                    hasher.update(b"C");
                    update_bytes(&mut hasher, info.name().as_bytes());
                    update_value_oracle(&mut hasher, &value);
                }
            }
        }
        WorldHash(hasher.digest())
    }

    /// Asserts, for each value given, that hashing it *through its own `Serialize`* equals the
    /// oracle's hash of the `serde_json::Value` it would have been turned into first.
    ///
    /// This is the bit-compatibility property the whole optimization rests on, stated once: the
    /// encoder consumes serde events, the frozen byte stream is defined over the `Value` those
    /// events would have built, and the two must not disagree anywhere.
    macro_rules! assert_matches_value_oracle {
        ($($value:expr),* $(,)?) => {
            $({
                let value = $value;
                let json = serde_json::to_value(&value).expect("the corpus serializes");
                assert_eq!(
                    digest(&value),
                    oracle_digest(&json),
                    "encoder drifted from `to_value` + oracle on {json}",
                );
            })*
        };
    }

    #[derive(Serialize)]
    struct Transform {
        translation: [f32; 3],
        rotation: [f32; 4],
        scale: [f32; 3],
    }

    #[derive(Serialize)]
    struct Label {
        text: String,
        tags: Vec<String>,
    }

    #[derive(Serialize)]
    struct Flag(bool);

    #[derive(Serialize)]
    struct Unit;

    #[derive(Serialize)]
    struct Pair(i8, u64);

    #[derive(Serialize)]
    enum Shape {
        Dot,
        Radius(f32),
        Segment(f64, f64),
        Box { w: f32, h: f32 },
    }

    #[derive(Serialize)]
    struct Kitchen {
        maybe: Option<u8>,
        unit: (),
        tuple: (i32, char, bool),
        by_name: std::collections::BTreeMap<String, f32>,
        by_number: std::collections::BTreeMap<u32, i16>,
        nested: Vec<Vec<Option<f32>>>,
        shapes: Vec<Shape>,
    }

    /// Every serde shape a component can take encodes to the `Value` path's bytes.
    ///
    /// The corpus is chosen to hit the places the two paths *could* disagree: the number widenings
    /// (`f32` → `f64`, small ints → `i64`/`u64`), the four enum representations, `Option`, maps with
    /// keys that are not `&'static str`, tuples, empty containers, and the recursion.
    #[test]
    fn encoding_a_component_matches_encoding_its_json_value() {
        assert_matches_value_oracle!(
            Transform {
                translation: [-0.5, 0.0, 1.0e30],
                rotation: [0.0, -0.0, 1.0, f32::from_bits(0x3f80_0001)],
                scale: [1.0; 3],
            },
            Label {
                text: "entity \"0\"\n\t héllo ☃".to_string(),
                tags: vec!["a".into(), "ż".into(), String::new()],
            },
            Flag(true),
            Unit,
            Pair(i8::MIN, u64::MAX),
            Shape::Dot,
            Shape::Radius(-0.0),
            Shape::Segment(f64::MIN_POSITIVE, -1.5e-7),
            Shape::Box { w: 1.5, h: 2.5 },
            Kitchen {
                maybe: Some(7),
                unit: (),
                tuple: (-1, 'ż', false),
                by_name: [("b".to_string(), 1.25_f32), ("a".to_string(), -0.0_f32)]
                    .into_iter()
                    .collect(),
                by_number: [(10_u32, -3_i16), (2, 4)].into_iter().collect(),
                nested: vec![
                    vec![Some(0.1), Some(0.2)],
                    vec![],
                    vec![Some(f32::MIN_POSITIVE)]
                ],
                shapes: vec![
                    Shape::Dot,
                    Shape::Radius(f32::MAX),
                    Shape::Box { w: -0.0, h: 0.0 }
                ],
            },
            // Bare scalars, as a `newtype`-free component would emit them.
            (),
            true,
            0_u8,
            i64::MIN,
            u64::MAX,
            -0.0_f32,
            0.0_f32,
            f32::MIN_POSITIVE,
            f32::MAX,
            -0.0_f64,
            1.0e300_f64,
            "".to_string(),
            'a',
            Vec::<u8>::new(),
            std::collections::BTreeMap::<String, u8>::new(),
        );
    }

    /// The v2 stream for a non-finite float, built by hand: `#`, a `u64` LE length, the text.
    fn non_finite_bytes(text: &str) -> Vec<u8> {
        let mut bytes = vec![b'#'];
        bytes.extend_from_slice(&(text.len() as u64).to_le_bytes());
        bytes.extend_from_slice(text.as_bytes());
        bytes
    }

    fn bytes_digest(bytes: &[u8]) -> u64 {
        let mut hasher = Xxh3::new();
        hasher.update(bytes);
        hasher.digest()
    }

    /// What a float must hash to: the `Value` oracle when finite, the hand-built token otherwise.
    fn expected_float_digest(value: f64) -> u64 {
        if value.is_finite() {
            return oracle_digest(&serde_json::to_value(value).unwrap());
        }
        let text = if value.is_nan() {
            "NaN"
        } else if value > 0.0 {
            "inf"
        } else {
            "-inf"
        };
        bytes_digest(&non_finite_bytes(text))
    }

    /// An independent byte oracle for the v2 tokens that the `Value` path cannot express.
    #[test]
    fn non_finite_floats_and_none_encode_to_their_own_bytes() {
        fn encoded(value: &dyn erased_serde::Serialize) -> Vec<u8> {
            Canonical::default().encode(value).unwrap().to_vec()
        }
        let nan = non_finite_bytes("NaN");
        assert_eq!(encoded(&f32::NAN), nan);
        assert_eq!(encoded(&-f32::NAN), nan);
        assert_eq!(encoded(&f32::from_bits(0x7fc0_0001)), nan);
        assert_eq!(encoded(&f64::NAN), nan);
        assert_eq!(encoded(&f32::INFINITY), non_finite_bytes("inf"));
        assert_eq!(encoded(&f64::INFINITY), non_finite_bytes("inf"));
        assert_eq!(encoded(&f32::NEG_INFINITY), non_finite_bytes("-inf"));
        assert_eq!(encoded(&f64::NEG_INFINITY), non_finite_bytes("-inf"));
        assert_eq!(encoded(&None::<u8>), b"o");
        assert_eq!(encoded(&()), b"n");
        assert_eq!(encoded(&Unit), b"n");
        assert_eq!(encoded(&Value::Null), b"n");
    }

    /// NaN, `+inf`, `-inf`, `None` and unit are five different values, and `Some` wraps nothing.
    #[test]
    fn non_finite_floats_none_and_unit_hash_distinctly() {
        let digests = [
            digest(&f32::NAN),
            digest(&f32::INFINITY),
            digest(&f32::NEG_INFINITY),
            digest(&None::<u8>),
            digest(&()),
        ];
        let distinct: std::collections::BTreeSet<u64> = digests.iter().copied().collect();
        assert_eq!(distinct.len(), digests.len());
        assert_ne!(digest(&Some(f32::NAN)), digest(&None::<f32>));
        assert_ne!(digest(&Some(())), digest(&None::<()>));
        assert_eq!(digest(&()), digest(&Unit));
        assert_eq!(digest(&()), digest(&Value::Null));
    }

    /// A world with an `Option<f32>` and an `f32` component, hashed against a stream written out by
    /// hand — the independent oracle for the whole v2 walk.
    #[test]
    fn a_world_with_non_finite_floats_and_none_matches_a_hand_built_stream() {
        use schemars::JsonSchema;
        use serde::Deserialize;

        #[derive(Serialize, Deserialize, JsonSchema)]
        struct Maybe(Option<f32>);
        #[derive(Serialize, Deserialize, JsonSchema)]
        struct Level(f32);

        let mut registry = Registry::new();
        registry.register::<Maybe>("Maybe").unwrap();
        registry.register::<Level>("Level").unwrap();
        let mut world = World::new();
        let a = world.spawn();
        let b = world.spawn();
        world.insert(a, Maybe(None)).unwrap();
        world.insert(a, Level(f32::NEG_INFINITY)).unwrap();
        world.insert(b, Maybe(Some(f32::NAN))).unwrap();

        let mut stream = b"ludion.worldhash.v2".to_vec();
        let component = |stream: &mut Vec<u8>, name: &str, value: &[u8]| {
            stream.push(b'C');
            stream.extend_from_slice(&(name.len() as u64).to_le_bytes());
            stream.extend_from_slice(name.as_bytes());
            stream.extend_from_slice(value);
        };
        for (entity, parts) in [
            (
                a,
                vec![
                    ("Maybe", b"o".to_vec()),
                    ("Level", non_finite_bytes("-inf")),
                ],
            ),
            (b, vec![("Maybe", non_finite_bytes("NaN"))]),
        ] {
            stream.push(b'E');
            stream.extend_from_slice(&entity.index().to_le_bytes());
            stream.extend_from_slice(&entity.generation().to_le_bytes());
            for (name, value) in parts {
                component(&mut stream, name, &value);
            }
        }
        assert_eq!(
            WorldHash::compute(&world, &registry).unwrap().as_u64(),
            bytes_digest(&stream),
        );
    }

    /// The report names the first entity and component holding a non-finite float and counts every
    /// non-finite value in the walk; a finite world reports nothing.
    #[test]
    fn compute_with_report_names_the_first_site_and_counts_all() {
        use schemars::JsonSchema;
        use serde::Deserialize;

        #[derive(Serialize, Deserialize, JsonSchema)]
        struct Pos([f32; 3]);
        #[derive(Serialize, Deserialize, JsonSchema)]
        struct Level(f32);

        let mut registry = Registry::new();
        registry.register::<Pos>("Pos").unwrap();
        registry.register::<Level>("Level").unwrap();
        let mut world = World::new();
        let a = world.spawn();
        let b = world.spawn();
        let c = world.spawn();
        world.insert(a, Pos([0.0; 3])).unwrap();
        world
            .insert(b, Pos([f32::NAN, 1.0, f32::INFINITY]))
            .unwrap();
        world.insert(c, Level(f32::NEG_INFINITY)).unwrap();

        let report = WorldHash::compute_with_report(&world, &registry).unwrap();
        let site = report
            .non_finite
            .expect("a non-finite float is in the world");
        assert_eq!(
            (site.entity, site.component.as_str(), site.count),
            (b, "Pos", 3)
        );
        assert_eq!(report.hash, WorldHash::compute(&world, &registry).unwrap());

        world.get_mut::<Pos>(b).unwrap().unwrap().0 = [0.0; 3];
        world.get_mut::<Level>(c).unwrap().unwrap().0 = 1.0;
        let report = WorldHash::compute_with_report(&world, &registry).unwrap();
        assert!(report.non_finite.is_none());
    }

    /// Adjacent `f32` bit patterns, subnormals, and exponent extremes all reach the oracle's bytes.
    ///
    /// `f32` is the subtle one: a `Value` widens it to `f64` before formatting, so an encoder that
    /// formatted the `f32` directly would produce shorter — and different — text for most values.
    #[test]
    fn adversarial_f32_bit_patterns_match_the_value_oracle() {
        let mut seeds: Vec<u32> = vec![
            0x0000_0000, // 0.0
            0x8000_0000, // -0.0
            0x0000_0001, // smallest subnormal
            0x007f_ffff, // largest subnormal
            0x0080_0000, // smallest normal
            0x3f80_0000, // 1.0
            0x3f80_0001, // 1.0 + 1 ulp
            0x3dcc_cccd, // 0.1
            0x7f7f_ffff, // f32::MAX
            0x4b7f_ffff,
            0xc07f_ffff,
        ];
        // A deterministic spread over the exponent range, each with a ragged mantissa.
        for exponent in 0..=254_u32 {
            seeds.push((exponent << 23) | 0x0055_5555);
        }
        for bits in seeds {
            for value in [f32::from_bits(bits), -f32::from_bits(bits)] {
                assert_eq!(
                    digest(&value),
                    expected_float_digest(f64::from(value)),
                    "encoder drifted on f32 bits {bits:#010x} ({value:e})",
                );
            }
        }
    }

    /// The whole-world entry point is oracle-identical too, over a world exercising several
    /// entities, several component shapes, and a recycled entity slot — what `WorldHash::compute`
    /// actually walks.
    #[test]
    fn world_hash_is_bit_identical_to_the_pre_optimization_oracle() {
        use ludion_ecs::{Registry, World};
        use schemars::JsonSchema;
        use serde::Deserialize;

        #[derive(Serialize, Deserialize, JsonSchema)]
        struct Transform {
            translation: [f32; 3],
            rotation: [f32; 4],
            scale: [f32; 3],
        }
        #[derive(Serialize, Deserialize, JsonSchema)]
        struct Label {
            text: String,
            tags: Vec<String>,
        }
        #[derive(Serialize, Deserialize, JsonSchema)]
        struct Flag(bool);
        #[derive(Serialize, Deserialize, JsonSchema)]
        enum Mode {
            Idle,
            Chasing { speed: f32 },
        }
        #[derive(Serialize, Deserialize, JsonSchema)]
        struct Brain {
            mode: Mode,
            memory: std::collections::BTreeMap<String, i64>,
            target: Option<u32>,
        }

        let mut registry = Registry::new();
        registry.register::<Transform>("Transform").unwrap();
        registry.register::<Label>("Label").unwrap();
        registry.register::<Flag>("Flag").unwrap();
        registry.register::<Brain>("Brain").unwrap();

        let mut world = World::new();
        for i in 0..8 {
            let e = world.spawn();
            world
                .insert(
                    e,
                    Transform {
                        translation: [i as f32 * -0.5, 0.0, 1.0e30],
                        rotation: [0.0, -0.0, 1.0, f32::from_bits(0x3f80_0001)],
                        scale: [1.0; 3],
                    },
                )
                .unwrap();
            if i % 2 == 0 {
                world
                    .insert(
                        e,
                        Label {
                            text: format!("entity \"{i}\"\n"),
                            tags: vec!["a".into(), "ż".into()],
                        },
                    )
                    .unwrap();
            }
            if i % 3 == 0 {
                world.insert(e, Flag(i % 2 == 0)).unwrap();
            }
            if i % 4 == 0 {
                world
                    .insert(
                        e,
                        Brain {
                            mode: Mode::Chasing { speed: 0.1 },
                            memory: [("seen".to_string(), -1_i64), ("hp".to_string(), 3)]
                                .into_iter()
                                .collect(),
                            target: Some(i as u32),
                        },
                    )
                    .unwrap();
            } else if i % 5 == 0 {
                world
                    .insert(
                        e,
                        Brain {
                            mode: Mode::Idle,
                            memory: std::collections::BTreeMap::new(),
                            target: Some(0),
                        },
                    )
                    .unwrap();
            }
        }
        // A despawn-then-respawn, so generations differ and the free list has been exercised.
        let doomed = world.spawn();
        world.despawn(doomed).unwrap();
        let reborn = world.spawn();
        world.insert(reborn, Flag(true)).unwrap();

        assert_eq!(
            WorldHash::compute(&world, &registry).unwrap(),
            world_hash_oracle(&world, &registry),
        );
    }

    /// The restructured walk (one presence cursor per component, advanced with the entity scan)
    /// still emits exactly what probing every storage per entity emitted, across the shapes that
    /// stress a cursor: sparse components, gaps from despawns, a component nothing carries, and a
    /// registered component whose storage the world has never created.
    #[test]
    fn the_component_walk_matches_the_oracle_over_sparse_and_recycled_worlds() {
        use ludion_ecs::{Registry, World};
        use schemars::JsonSchema;
        use serde::Deserialize;

        #[derive(Serialize, Deserialize, JsonSchema)]
        struct A(u32);
        #[derive(Serialize, Deserialize, JsonSchema)]
        struct B(u32);
        #[derive(Serialize, Deserialize, JsonSchema)]
        struct C(u32);
        #[derive(Serialize, Deserialize, JsonSchema)]
        struct Never(u32);

        let mut registry = Registry::new();
        registry.register::<A>("A").unwrap();
        registry.register::<B>("B").unwrap();
        registry.register::<C>("C").unwrap();
        // Registered but never inserted anywhere: its storage does not exist, so its cursor is empty.
        registry.register::<Never>("Never").unwrap();

        let mut world = World::new();
        let entities: Vec<_> = (0..64).map(|_| world.spawn()).collect();
        for (i, &e) in entities.iter().enumerate() {
            // Deliberately uneven: `A` on a prefix, `B` on a suffix, `C` on a sparse scatter, so the
            // three cursors are never in step with each other or with the entity walk.
            if i < 20 {
                world.insert(e, A(i as u32)).unwrap();
            }
            if i > 40 {
                world.insert(e, B(i as u32)).unwrap();
            }
            if i % 7 == 3 {
                world.insert(e, C(i as u32)).unwrap();
            }
        }
        assert_eq!(
            WorldHash::compute(&world, &registry).unwrap(),
            world_hash_oracle(&world, &registry),
        );

        // Punch holes, then refill some of them: live slots stop being contiguous and generations
        // diverge, which is exactly where a mis-advanced cursor would desynchronize.
        for (i, &e) in entities.iter().enumerate() {
            if i % 3 == 0 {
                world.despawn(e).unwrap();
            }
        }
        assert_eq!(
            WorldHash::compute(&world, &registry).unwrap(),
            world_hash_oracle(&world, &registry),
        );
        for i in 0..10 {
            let e = world.spawn();
            world.insert(e, C(1000 + i)).unwrap();
        }
        assert_eq!(
            WorldHash::compute(&world, &registry).unwrap(),
            world_hash_oracle(&world, &registry),
        );

        // An empty world and a world of bare entities are the degenerate ends of the same walk.
        let empty = World::new();
        assert_eq!(
            WorldHash::compute(&empty, &registry).unwrap(),
            world_hash_oracle(&empty, &registry),
        );
        let mut bare = World::new();
        for _ in 0..5 {
            bare.spawn();
        }
        assert_eq!(
            WorldHash::compute(&bare, &registry).unwrap(),
            world_hash_oracle(&bare, &registry),
        );
    }

    /// A component whose `Serialize` fails still surfaces as the same `L0007` the `Value` path
    /// raised — same code, same message, same component name.
    #[test]
    fn a_serialization_failure_still_reports_l0007() {
        use ludion_ecs::{Registry, World};
        use schemars::JsonSchema;
        use serde::Deserialize;

        /// A 128-bit integer past `u64`/`i64` is the one value JSON cannot hold, so both paths must
        /// refuse it.
        #[derive(Deserialize, JsonSchema)]
        struct Huge(i128);

        impl Serialize for Huge {
            fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
                serializer.serialize_i128(self.0)
            }
        }

        let mut registry = Registry::new();
        registry.register::<Huge>("Huge").unwrap();
        let mut world = World::new();
        let e = world.spawn();
        world.insert(e, Huge(i128::MIN)).unwrap();

        let streamed = WorldHash::compute(&world, &registry).unwrap_err();
        let from_value = registry.serialize("Huge", &world, e).unwrap_err();
        assert_eq!(streamed.code, from_value.code);
        assert_eq!(streamed.message, from_value.message);
        assert!(
            streamed.message.contains("component `Huge`"),
            "{streamed:?}"
        );
    }

    /// A `Serialize` impl that writes the same key twice collapses to one entry holding the **last**
    /// value — what `serde_json::Map::insert` does, and therefore what the frozen byte stream says.
    /// Nothing in the engine authors such a component, but the encoder decides this on its own (the
    /// `Value` path used to decide it inside `serde_json`), so the rule is pinned rather than
    /// assumed.
    #[test]
    fn a_repeated_object_key_keeps_the_last_value_like_the_value_path() {
        struct Repeated;

        impl Serialize for Repeated {
            fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
                use serde::ser::SerializeMap as _;
                let mut map = serializer.serialize_map(Some(3))?;
                map.serialize_entry("k", &1)?;
                map.serialize_entry("other", &2)?;
                map.serialize_entry("k", &3)?;
                map.end()
            }
        }

        assert_matches_value_oracle!(Repeated);
        assert_eq!(
            digest(&Repeated),
            value_digest(&json!({"k": 3, "other": 2})),
        );
    }

    /// One encoder serves a whole world, reusing its output buffer, its frame stack, and both of its
    /// pools across components. A value hashed after a deeply nested, key-heavy neighbour must still
    /// hash exactly as it does alone — a leaked frame or a pooled buffer returned dirty would show
    /// up here and nowhere else, because every other test hands each value a fresh encoder.
    #[test]
    fn reusing_one_encoder_never_bleeds_between_components() {
        let corpus = [
            json!({"deep": {"a": [[[{"z": [1.25, -0.0, {"": []}]}]]], "b": {"c": {"d": 1}}}}),
            json!(null),
            json!({"b": 2, "a": 1}),
            json!([]),
            json!({}),
            json!([{"k": [1, 2, 3]}, {"k": []}]),
            json!(-0.0),
            json!("tail"),
        ];
        let mut shared = Canonical::default();
        // Twice through, so every value is preceded by a different neighbour on the second pass.
        for value in corpus.iter().chain(corpus.iter().rev()) {
            let mut fresh = Canonical::default();
            assert_eq!(
                shared.encode(value).unwrap(),
                fresh.encode(value).unwrap(),
                "a reused encoder drifted on {value}",
            );
        }
    }

    /// A recursive JSON strategy: every scalar kind (including arbitrary finite `f64` bit patterns
    /// and arbitrary UTF-8 strings) under nesting deep and wide enough to exercise the frame stack
    /// and the buffer pools.
    fn arbitrary_json() -> impl Strategy<Value = Value> {
        let leaf = prop_oneof![
            Just(Value::Null),
            any::<bool>().prop_map(Value::Bool),
            any::<i64>().prop_map(|n| json!(n)),
            any::<u64>().prop_map(|n| json!(n)),
            any::<u64>()
                .prop_map(f64::from_bits)
                .prop_filter("JSON numbers are finite", |f| f.is_finite())
                .prop_map(|f| json!(f)),
            ".*".prop_map(Value::String),
        ];
        leaf.prop_recursive(4, 48, 5, |inner| {
            prop_oneof![
                proptest::collection::vec(inner.clone(), 0..5).prop_map(Value::Array),
                proptest::collection::vec((".*", inner), 0..5)
                    .prop_map(|entries| Value::Object(entries.into_iter().collect())),
            ]
        })
    }

    proptest! {
        /// A wide, unbiased draw over JSON shapes: whatever a component's `Serialize` describes, the
        /// encoder and the pre-optimization `Value` walker must agree on it.
        #[test]
        fn arbitrary_json_values_encode_to_the_oracle_bytes(value in arbitrary_json()) {
            prop_assert_eq!(value_digest(&value), oracle_digest(&value));
        }

        /// The whole `f32` bit-pattern space — subnormals, infinities, NaNs and every ordinary value
        /// alike — reaches the same bytes as widening it into a `Value` first (finite values) or
        /// the hand-built non-finite token. This is the single subtlest equivalence in the encoder,
        /// so it gets the widest draw.
        #[test]
        fn arbitrary_f32_bit_patterns_encode_like_their_json_value(bits in any::<u32>()) {
            let value = f32::from_bits(bits);
            prop_assert_eq!(digest(&value), expected_float_digest(f64::from(value)));
        }

        /// The same draw over `f64`, where no widening happens but the formatting still must match.
        #[test]
        fn arbitrary_f64_bit_patterns_encode_like_their_json_value(bits in any::<u64>()) {
            let value = f64::from_bits(bits);
            prop_assert_eq!(digest(&value), expected_float_digest(value));
        }
    }

    #[test]
    fn length_prefixes_prevent_concatenation_collisions() {
        assert_ne!(
            value_digest(&json!(["ab"])),
            value_digest(&json!(["a", "b"]))
        );
        assert_ne!(value_digest(&json!("12")), value_digest(&json!(12)));
    }

    #[test]
    fn object_key_order_does_not_matter() {
        // serde_json may or may not canonicalize map order itself depending on features; the
        // encoder's explicit sort makes the question moot.
        assert_eq!(
            value_digest(&json!({"a": 1, "b": 2})),
            value_digest(&json!({"b": 2, "a": 1}))
        );
    }

    #[test]
    fn float_bit_patterns_are_distinguished() {
        assert_ne!(
            value_digest(&json!(0.0_f64)),
            value_digest(&json!(-0.0_f64))
        );
        let x = 2.0_f32;
        let x_next = f32::from_bits(x.to_bits() + 1); // smallest possible f32 change
        assert_ne!(
            value_digest(&json!(x)),
            value_digest(&json!(x_next)),
            "adjacent f32 bit patterns must hash differently"
        );
    }

    #[test]
    fn scalar_types_are_domain_separated() {
        assert_ne!(value_digest(&json!(true)), value_digest(&json!("t")));
        assert_ne!(value_digest(&json!(null)), value_digest(&json!("n")));
        assert_ne!(value_digest(&json!(1)), value_digest(&json!(1.0)));
    }

    #[test]
    fn display_and_parse_round_trip() {
        let h = WorldHash(0x0123_4567_89ab_cdef);
        assert_eq!(h.to_string(), "0123456789abcdef");
        assert_eq!(WorldHash::parse(&h.to_string()), Some(h));
        assert!(WorldHash::parse("").is_none());
        assert!(WorldHash::parse("0123456789abcde").is_none()); // 15 digits
        assert!(WorldHash::parse("0123456789abcdeg").is_none()); // non-hex
    }

    #[test]
    fn presentation_entities_never_reach_the_hash() {
        #[derive(Serialize, serde::Deserialize, schemars::JsonSchema, Clone, Copy)]
        struct Tint(u32);

        let mut registry = Registry::new();
        registry.register::<Tint>("Tint").unwrap();

        let mut plain = World::new();
        let a = plain.spawn();
        plain.insert(a, Tint(1)).unwrap();
        let before = WorldHash::compute(&plain, &registry).unwrap();

        let mut world = World::new();
        let a = world.spawn();
        world.insert(a, Tint(1)).unwrap();
        let popups: Vec<_> = (0..3).map(|_| world.spawn_presentation()).collect();
        for (i, &p) in popups.iter().enumerate() {
            world.insert(p, Tint(100 + i as u32)).unwrap();
        }
        assert_eq!(WorldHash::compute(&world, &registry).unwrap(), before);

        // Despawning and respawning presentation entities moves nothing either.
        world.despawn(popups[1]).unwrap();
        let again = world.spawn_presentation();
        world.insert(again, Tint(7)).unwrap();
        assert_eq!(WorldHash::compute(&world, &registry).unwrap(), before);

        // A sim spawn after all that still gets the id a world without popups would hand out.
        assert_eq!(world.spawn(), plain.spawn());
    }

    #[test]
    fn serde_uses_the_hex_string_form() {
        let h = WorldHash(42);
        let json = serde_json::to_string(&h).unwrap();
        assert_eq!(json, "\"000000000000002a\"");
        let back: WorldHash = serde_json::from_str(&json).unwrap();
        assert_eq!(h, back);
        assert!(serde_json::from_str::<WorldHash>("\"xyz\"").is_err());
    }
}
