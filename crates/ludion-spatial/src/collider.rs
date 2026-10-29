//! The [`Collider`] builtin component and its [`Shape`] / [`Body`] enums.

use ludion_core::codes::code;
use ludion_core::diagnostics::Diagnostic;
use ludion_ecs::SemanticValidate;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// A collision shape, in-plane (2D, on [`Plane`](crate::Plane); x/z by default) or 3D.
///
/// Serialized with serde's external tagging (a one-key map whose key is the snake_case variant
/// name), mirroring the engine's `MeshSource`:
///
/// ```yaml
/// shape:
///   circle:
///     radius: 0.5
/// shape:
///   aabb:
///     half_extents:
///       - 0.5
///       - 0.5
/// shape:
///   sphere:
///     radius: 0.5
/// shape:
///   box:
///     half_extents:
///       - 0.5
///       - 0.5
///       - 0.5
/// shape:
///   capsule:
///     half_height: 0.5
///     radius: 0.25
/// shape:
///   cylinder:
///     half_height: 0.5
///     radius: 0.25
/// shape:
///   convex:
///     points:
///       - [0.0, 0.0, 0.0]
///       - [1.0, 0.0, 0.0]
///       - [0.0, 1.0, 0.0]
///       - [0.0, 0.0, 1.0]
/// shape:
///   heightfield:
///     rows: 2
///     cols: 2
///     heights: [0.0, 0.0, 0.0, 0.0]
///     scale: [1.0, 1.0, 1.0]
/// shape:
///   compound:
///     parts:
///       - shape:
///           box:
///             half_extents: [0.5, 0.5, 0.5]
///       - shape:
///           sphere:
///             radius: 0.25
///         translation: [0.0, -0.75, 0.0]
///         rotation: [0.0, 0.0, 0.0]   # Euler XYZ, degrees
/// ```
///
/// `circle` and `aabb` are the in-plane forms — `half_extents` is a 2-vector on the two in-plane
/// axes, there is no off-plane extent, because `ludion-spatial`'s own collision is 2D
/// ([`spatial::rebuild`](crate::spatial::rebuild) indexes only these two). `sphere`, `box`,
/// `capsule`, `cylinder` (axis along up, like `capsule`), `convex` (hand-authored points, or the
/// 8-corner part-bounds box `switch_to_ragdoll` writes; no hull generator), `heightfield`,
/// `compound` and `mesh` are 3D shapes for `ludion-physics`'s rigid bodies
/// ([`is_3d`](Shape::is_3d)); `ludion-spatial` carries them as pure data and never queries against
/// them itself. Every size must be a strictly positive, finite number as an `f32` (`L1116`; a
/// negative one is additionally rejected by the schema as `L1014`); `convex`'s `points` must
/// enclose a volume (`L1181`) and `heightfield`'s `rows`/`cols`/`heights` must describe a
/// consistent grid (`L1182`). A `compound` is one body, one collider and one mass spread over its
/// parts by volume, so its centre of mass may sit off the entity origin; its parts are
/// `sphere`/`box`/`capsule`/`cylinder`/`convex` only, and an empty `parts`, a nested `compound`, a
/// `heightfield`, `circle` or `aabb` part, or a part `translation`/`rotation` beyond `f32` range is
/// `L1189`. A `mesh` is the triangles of a `kind: mesh` document at `path` (content-root relative),
/// for level geometry: `static` or `kinematic` only (`L1211` on `rigid`), a blank `path` is
/// `L1212`, and a mesh is never a `compound` part.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum Shape {
    /// A circle of the given radius, centered on the collider's in-plane position.
    Circle {
        /// Radius in world units (must be `> 0`; default `0.5`).
        #[serde(default = "half")]
        #[schemars(range(min = 0.0))]
        radius: f32,
    },
    /// An axis-aligned box with half-extents on the two in-plane axes ([`Plane`](crate::Plane);
    /// x/z by default) about the collider's position.
    Aabb {
        /// Half-extents on the two in-plane axes, in world units (each must be `> 0`; default
        /// `[0.5, 0.5]`).
        #[serde(default = "half2")]
        half_extents: [f32; 2],
    },
    /// A 3D sphere of the given radius, centered on the collider's position.
    Sphere {
        /// Radius in world units (must be `> 0`; default `0.5`).
        #[serde(default = "half")]
        #[schemars(range(min = 0.0))]
        radius: f32,
    },
    /// An axis-aligned 3D box with half-extents on all three axes about the collider's position.
    Box {
        /// Half-extents on all three axes, in world units (each must be `> 0`; default
        /// `[0.5, 0.5, 0.5]`).
        #[serde(default = "half3")]
        half_extents: [f32; 3],
    },
    /// A capsule: a cylinder of `half_height` capped with hemispheres of `radius`, its axis along
    /// the collider's up axis.
    Capsule {
        /// Half the cylinder segment's length, in world units, excluding the end caps (must be
        /// `> 0`; default `0.5`).
        #[serde(default = "half")]
        #[schemars(range(min = 0.0))]
        half_height: f32,
        /// The cap and cylinder radius, in world units (must be `> 0`; default `0.5`).
        #[serde(default = "half")]
        #[schemars(range(min = 0.0))]
        radius: f32,
    },
    /// A cylinder: flat ends of `radius`, `2 * half_height` long, its axis along the collider's up
    /// axis.
    Cylinder {
        /// Half the cylinder's length, in world units (must be `> 0`; default `0.5`).
        #[serde(default = "half")]
        #[schemars(range(min = 0.0))]
        half_height: f32,
        /// The end-cap radius, in world units (must be `> 0`; default `0.5`).
        #[serde(default = "half")]
        #[schemars(range(min = 0.0))]
        radius: f32,
    },
    /// A convex hull over `points` — hand-authored scene data, or the 8-corner part-bounds box
    /// `switch_to_ragdoll` writes; no hull generator.
    Convex {
        /// The hull's points, in the collider's local space.
        #[serde(default)]
        points: Vec<[f32; 3]>,
    },
    /// A regular grid of heights, `rows` by `cols`, scaled by `scale`.
    Heightfield {
        /// The number of rows in the height grid.
        #[serde(default)]
        rows: u32,
        /// The number of columns in the height grid.
        #[serde(default)]
        cols: u32,
        /// The grid's heights, row-major, `rows * cols` of them.
        #[serde(default)]
        heights: Vec<f32>,
        /// Per-axis scale applied to the grid (each must be `> 0`; default `[1.0, 1.0, 1.0]`).
        #[serde(default = "one3")]
        scale: [f32; 3],
    },
    /// Several shapes rigidly joined into one collider: one body, one mass (spread over the parts
    /// by volume), a centre of mass that comes from the geometry.
    Compound {
        /// The parts, in authored order (at least one; no nested `compound`, `heightfield`,
        /// `mesh`, `circle` or `aabb`).
        #[serde(default)]
        parts: Vec<CompoundPart>,
    },
    /// The triangles of a `kind: mesh` document, for level geometry: a trimesh with its internal
    /// edges fixed, on a `static` or `kinematic` body only (`L1211` on `rigid`).
    ///
    /// ```yaml
    /// - Collider:
    ///     shape:
    ///       mesh:
    ///         path: meshes/floor.mesh.yaml
    ///     body: static
    /// ```
    Mesh {
        /// The mesh document, relative to the content root (the same path as `Mesh.shape: { asset }`).
        #[serde(default)]
        #[schemars(with = "ludion_ecs::AssetPathSchema")]
        path: String,
    },
}

/// One part of a [`Shape::Compound`]: a shape placed relative to the collider's origin.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq)]
pub struct CompoundPart {
    /// The part's shape: `sphere`, `box`, `capsule`, `cylinder` or `convex` (never a `mesh`).
    pub shape: Shape,
    /// The part's offset from the collider's origin, in the collider's local space (default
    /// `[0, 0, 0]`).
    #[serde(default)]
    pub translation: [f32; 3],
    /// The part's rotation: Euler XYZ in degrees, as `Transform` (default `[0, 0, 0]`).
    #[serde(default)]
    pub rotation: [f32; 3],
}

impl Shape {
    /// Whether `self` is a 3D shape (`sphere`, `box`, `capsule`, `cylinder`, `convex`,
    /// `heightfield`, `compound`, `mesh`) rather than one of the in-plane 2D forms (`circle`, `aabb`).
    ///
    /// [`spatial::rebuild`](crate::spatial::rebuild) skips any collider this returns `true` for (and
    /// any `body: rigid` collider regardless of shape): `ludion-physics` owns 3D shapes and rigid
    /// bodies, so the 2D index never carries an entry it cannot correctly query.
    pub fn is_3d(&self) -> bool {
        matches!(
            self,
            Shape::Sphere { .. }
                | Shape::Box { .. }
                | Shape::Capsule { .. }
                | Shape::Cylinder { .. }
                | Shape::Convex { .. }
                | Shape::Heightfield { .. }
                | Shape::Compound { .. }
                | Shape::Mesh { .. }
        )
    }
}

impl Default for Shape {
    fn default() -> Self {
        Shape::Circle { radius: 0.5 }
    }
}

/// Per-axis translation/rotation locks on a [`Body::Rigid`] body: `true` freezes that axis.
///
/// A 2D game locks the off-plane translation axis and the two in-plane rotation axes on every rigid
/// body, so a 2D game is the 3D solver with axis locks, not a second engine (`Plan/physics.md`).
#[derive(Serialize, Deserialize, JsonSchema, Clone, Copy, Debug, PartialEq, Default)]
pub struct AxisLock {
    /// Which translation axes (x, y, z) are frozen. Default: none.
    #[serde(default)]
    pub translation: [bool; 3],
    /// Which rotation axes (x, y, z) are frozen. Default: none.
    #[serde(default)]
    pub rotation: [bool; 3],
}

/// A [`Body::Rigid`] body's tunables: mass, friction, restitution, damping, gravity scale, axis
/// locks, and the sleep threshold.
///
/// `ludion-spatial` carries this as pure data; `ludion-physics` reads it and maps it onto the
/// solver. Sleeping is authored in ticks, not seconds — `ludion-physics` converts
/// `sleep_after_ticks` once at world-build time, so the tunable stays a fixed-timestep-native
/// integer here (`Plan/physics.md` §Why it is compatible with determinism).
#[derive(Serialize, Deserialize, JsonSchema, Clone, Copy, Debug, PartialEq)]
pub struct RigidBody {
    /// Mass in kilograms (must be `> 0`; default `1.0`).
    #[serde(default = "one")]
    #[schemars(range(min = 0.0))]
    pub mass: f32,
    /// Coulomb friction coefficient (must be `>= 0`; default `0.5`).
    #[serde(default = "half")]
    #[schemars(range(min = 0.0))]
    pub friction: f32,
    /// Restitution (bounciness), `0.0` inelastic to `1.0` fully elastic (must be `>= 0`; default
    /// `0.0`).
    #[serde(default)]
    #[schemars(range(min = 0.0))]
    pub restitution: f32,
    /// How this body's friction combines with the other collider's (default `average`); see
    /// [`CombineRule`].
    #[serde(default, skip_serializing_if = "is_average")]
    pub friction_combine: CombineRule,
    /// How this body's restitution combines with the other collider's (default `average`); see
    /// [`CombineRule`].
    #[serde(default, skip_serializing_if = "is_average")]
    pub restitution_combine: CombineRule,
    /// Linear velocity damping per second (must be `>= 0`; default `0.0`).
    #[serde(default)]
    #[schemars(range(min = 0.0))]
    pub linear_damping: f32,
    /// Angular velocity damping per second (must be `>= 0`; default `0.0`).
    #[serde(default)]
    #[schemars(range(min = 0.0))]
    pub angular_damping: f32,
    /// Multiplier on the scene's gravity for this body alone: `1.0` (default) falls normally, `0.0`
    /// floats, a negative value rises. Any finite value. A live edit applies in place — see
    /// `ludion docs physics` §Live edits.
    #[serde(default = "one", skip_serializing_if = "is_one")]
    pub gravity_scale: f32,
    /// Per-axis translation/rotation locks (default: none locked).
    #[serde(default)]
    pub lock: AxisLock,
    /// Ticks of near-rest motion before the body may sleep; absent ⇒ the scene's
    /// `physics.sleep_after_ticks`, else 30.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sleep_after_ticks: Option<u32>,
}

impl Default for RigidBody {
    fn default() -> Self {
        RigidBody {
            mass: one(),
            friction: half(),
            restitution: 0.0,
            friction_combine: CombineRule::Average,
            restitution_combine: CombineRule::Average,
            linear_damping: 0.0,
            angular_damping: 0.0,
            gravity_scale: one(),
            lock: AxisLock::default(),
            sleep_after_ticks: None,
        }
    }
}

/// How two touching colliders' friction (or restitution) values combine into the one the solver
/// uses. When the two colliders' rules differ the stronger wins, in rapier's priority order
/// **max > multiply > min > average**, so a bouncy part with `max` beats a ground left at `average`.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Copy, Debug, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum CombineRule {
    /// The mean of the two values.
    #[default]
    Average,
    /// The smaller of the two values.
    Min,
    /// The product of the two values.
    Multiply,
    /// The larger of the two values.
    Max,
}

/// The contact material of a `static` or `kinematic` collider: friction, restitution and their
/// combine rules. A [`Body::Rigid`] body carries the same four fields on [`RigidBody`].
///
/// ```yaml
/// body: { static: { restitution: 0.3 } }
/// body: { kinematic: { friction_combine: max } }
/// ```
#[derive(Serialize, Deserialize, JsonSchema, Clone, Copy, Debug, PartialEq)]
pub struct ContactMaterial {
    /// Coulomb friction coefficient (must be `>= 0`; default `0.5`).
    #[serde(default = "half", skip_serializing_if = "is_half")]
    #[schemars(range(min = 0.0))]
    pub friction: f32,
    /// Restitution (bounciness), `0.0` inelastic to `1.0` fully elastic (must be `>= 0`; default
    /// `0.0`).
    #[serde(default, skip_serializing_if = "is_zero")]
    #[schemars(range(min = 0.0))]
    pub restitution: f32,
    /// How the friction combines with the other collider's (default `average`).
    #[serde(default, skip_serializing_if = "is_average")]
    pub friction_combine: CombineRule,
    /// How the restitution combines with the other collider's (default `average`).
    #[serde(default, skip_serializing_if = "is_average")]
    pub restitution_combine: CombineRule,
}

impl Default for ContactMaterial {
    fn default() -> Self {
        ContactMaterial {
            friction: half(),
            restitution: 0.0,
            friction_combine: CombineRule::Average,
            restitution_combine: CombineRule::Average,
        }
    }
}

/// Whether a collider ever moves, and how.
///
/// `move_and_slide` resolves against `Static` colliders only; `Kinematic` moves under game
/// code (the character controller case — `Dynamic` is accepted on read as an alias for it, so a
/// scene written before the rename still loads, but the canonical writer always emits `kinematic`).
/// `Rigid` carries the solver tunables and moves under `ludion-physics`'s dynamics; a collider whose
/// body is `Rigid`, like one whose shape is 3D, is skipped by
/// [`spatial::rebuild`](crate::spatial::rebuild) (`Plan/physics.md`).
///
/// ```yaml
/// body: static
/// body: kinematic
/// body: { static: { restitution: 0.3 } }
/// body:
///   rigid:
///     mass: 2.0
/// body:
///   rigid: {}   # every RigidBody field defaults
/// ```
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq)]
#[serde(from = "BodyWire", into = "BodyWire")]
pub enum Body {
    /// Never moves — walls, props, level geometry.
    Static(ContactMaterial),
    /// Moves under game control — players, enemies, projectiles. Reads the legacy `dynamic` name
    /// too; the canonical writer emits `kinematic`.
    Kinematic(ContactMaterial),
    /// Moves under `ludion-physics`'s rigid-body dynamics.
    Rigid(RigidBody),
}

impl Default for Body {
    fn default() -> Self {
        Body::new_static()
    }
}

impl Body {
    /// A static body with the default contact material.
    pub fn new_static() -> Body {
        Body::Static(ContactMaterial::default())
    }

    /// A kinematic body with the default contact material.
    pub fn new_kinematic() -> Body {
        Body::Kinematic(ContactMaterial::default())
    }

    /// The body's contact material; a rigid body's is built from its four fields.
    pub fn material(&self) -> ContactMaterial {
        match self {
            Body::Static(material) | Body::Kinematic(material) => *material,
            Body::Rigid(rigid) => ContactMaterial {
                friction: rigid.friction,
                restitution: rigid.restitution,
                friction_combine: rigid.friction_combine,
                restitution_combine: rigid.restitution_combine,
            },
        }
    }
}

/// The serialized shape of [`Body`]: a bare name while the material is the default, else a map.
#[derive(Serialize)]
#[serde(untagged)]
enum BodyWire {
    Bare(BareBody),
    Tagged(TaggedBody),
}

/// Hand-written rather than `untagged`: that reports every malformed body as "did not match any
/// variant", where this keeps serde's field- and variant-specific message from the arm that was taken.
impl<'de> Deserialize<'de> for BodyWire {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        use serde::de::{self, IntoDeserializer, MapAccess, Visitor, value::MapAccessDeserializer};

        struct WireVisitor;

        impl<'de> Visitor<'de> for WireVisitor {
            type Value = BodyWire;

            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str(
                    "a body name (`static`, `kinematic`) or a map with one of `static`, `kinematic`, `rigid`",
                )
            }

            fn visit_str<E: de::Error>(self, name: &str) -> Result<BodyWire, E> {
                BareBody::deserialize(name.into_deserializer()).map(BodyWire::Bare)
            }

            fn visit_map<A: MapAccess<'de>>(self, map: A) -> Result<BodyWire, A::Error> {
                TaggedBody::deserialize(MapAccessDeserializer::new(map)).map(BodyWire::Tagged)
            }
        }

        deserializer.deserialize_any(WireVisitor)
    }
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum BareBody {
    Static,
    #[serde(alias = "dynamic")]
    Kinematic,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum TaggedBody {
    Static(ContactMaterial),
    Kinematic(ContactMaterial),
    Rigid(RigidBody),
}

impl From<Body> for BodyWire {
    fn from(body: Body) -> Self {
        let default = ContactMaterial::default();
        match body {
            Body::Static(m) if m == default => BodyWire::Bare(BareBody::Static),
            Body::Kinematic(m) if m == default => BodyWire::Bare(BareBody::Kinematic),
            Body::Static(m) => BodyWire::Tagged(TaggedBody::Static(m)),
            Body::Kinematic(m) => BodyWire::Tagged(TaggedBody::Kinematic(m)),
            Body::Rigid(r) => BodyWire::Tagged(TaggedBody::Rigid(r)),
        }
    }
}

impl From<BodyWire> for Body {
    fn from(wire: BodyWire) -> Self {
        match wire {
            BodyWire::Bare(BareBody::Static) => Body::new_static(),
            BodyWire::Bare(BareBody::Kinematic) => Body::new_kinematic(),
            BodyWire::Tagged(TaggedBody::Static(m)) => Body::Static(m),
            BodyWire::Tagged(TaggedBody::Kinematic(m)) => Body::Kinematic(m),
            BodyWire::Tagged(TaggedBody::Rigid(r)) => Body::Rigid(r),
        }
    }
}

/// Schema-only proxy for [`Body`] (the `EntityIdSchema` precedent, `ludion-engine`'s
/// `patterns.rs`): adds the legacy `dynamic` spelling and the contact-material map forms to the
/// *generated* schema. `ludion_scene::check` validates a scene against this schema *before*
/// `Deserialize` ever runs, so it must accept every form `Body` reads. Never constructed;
/// `#[schemars(with = "BodySchema")]` on [`Collider::body`] redirects only the generated schema,
/// not `Body`'s own (de)serialization.
#[derive(JsonSchema)]
#[serde(rename_all = "snake_case")]
#[allow(dead_code)]
enum BodySchema {
    Static,
    Kinematic,
    /// Accepted for backward compatibility only; the canonical writer never emits it.
    Dynamic,
    #[serde(rename = "static")]
    StaticMaterial(ContactMaterial),
    #[serde(rename = "kinematic")]
    KinematicMaterial(ContactMaterial),
    Rigid(RigidBody),
}

/// A collision shape, with a static/kinematic/rigid classification.
///
/// A `Collider` carries only authored *shape* configuration; the world-space position it sits at
/// comes from a companion position component via [`GroundPosition`](crate::GroundPosition) (the
/// engine's `Transform`). This is a registered builtin component: it round-trips through the scene
/// format, publishes a schema for `check-scene`, and enters the world hash and snapshots like any
/// other component. Every field defaults, so `{}` deserializes to a `0.5`-radius static circle
/// (drift-safe; see `ludion_testkit::assert_components_drift_safe`).
#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq, Default)]
pub struct Collider {
    /// The collision shape (default a `0.5`-radius circle).
    #[serde(default)]
    pub shape: Shape,
    /// Whether the collider ever moves (default a static body).
    #[serde(default)]
    #[schemars(with = "BodySchema")]
    pub body: Body,
    /// The collision layer, named in the scene's `physics.layers`; `None` is the implicit
    /// `default` layer, and the field is omitted from the saved text.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub layer: Option<String>,
    /// Detects overlaps without ever colliding or pushing (default `false`, omitted from the
    /// saved text). Queries skip a sensor unless the filter opts in. `static` or `kinematic` only,
    /// and a volume shape only (not `mesh` or `heightfield`).
    #[serde(default, skip_serializing_if = "is_false")]
    pub sensor: bool,
}

impl Collider {
    /// Whether the physics solver owns this collider — a 3D shape or a rigid body — rather than the
    /// 2D [`SpatialIndex`](crate::SpatialIndex).
    ///
    /// ```
    /// use ludion_spatial::{Body, Collider, RigidBody, Shape};
    ///
    /// let wall = Collider { shape: Shape::Aabb { half_extents: [1.0, 1.0] }, ..Collider::default() };
    /// assert!(!wall.is_solver_owned());
    /// let puck = Collider { body: Body::Rigid(RigidBody::default()), ..Collider::default() };
    /// assert!(puck.is_solver_owned());
    /// ```
    pub fn is_solver_owned(&self) -> bool {
        self.shape.is_3d() || matches!(self.body, Body::Rigid(_))
    }
}

impl SemanticValidate for Collider {
    /// Rejects a size that is not a strictly positive, finite number as an `f32` (`L1116`, covering
    /// a `rigid` body's `mass` too), a degenerate `convex` hull (`L1181`), a malformed
    /// `heightfield` grid (`L1182`), a structurally invalid `compound` (`L1189`), a `mesh` or
    /// `heightfield` on a `rigid` body (`L1211`, `/body`) or with a blank path (`L1212`,
    /// `/shape/mesh/path`), with a JSON Pointer relative to the component root
    /// (`/shape/circle/radius`, `/shape/aabb/half_extents/<i>`, `/shape/sphere/radius`,
    /// `/shape/box/half_extents/<i>`, `/shape/capsule/half_height`, `/shape/capsule/radius`,
    /// `/shape/cylinder/half_height`, `/shape/cylinder/radius`, `/shape/convex/points`,
    /// `/shape/heightfield/rows`, `/shape/heightfield/cols`, `/shape/heightfield/heights`,
    /// `/shape/heightfield/scale/<i>`, `/shape/compound/parts`, `/shape/compound/parts/<i>/shape`
    /// (with any of the above nested under it), `/shape/compound/parts/<i>/translation/<j>`,
    /// `/shape/compound/parts/<i>/rotation/<j>`, `/body/rigid/mass`, `/body/rigid/gravity_scale`).
    /// JSON Schema's `range(min)` is inclusive and does not reach array elements, so these
    /// invariants live here rather than in the schema. A hull that passes this check but
    /// `ludion-physics`'s parry builder still rejects gets no body, and is not retried until its
    /// `Collider` changes; this validator cannot see parry's own tolerance. Defensive: runs on a
    /// schema-clean value and adds nothing to a shape mismatch (that is `L1014`'s job).
    /// A `sensor: true` on a `rigid` body is `L1291` and on a `mesh` or `heightfield` is
    /// `L1292`, both at `/sensor`.
    fn validate_semantics(value: &Value) -> Vec<Diagnostic> {
        let mut out = Vec::new();
        if let Some(shape) = value.get("shape") {
            validate_shape(shape, "/shape", &mut out);
        }
        if value.pointer("/body/rigid").is_some() {
            let refused = [
                ("mesh", "a mesh collider", "a triangle mesh"),
                ("heightfield", "a heightfield collider", "a heightfield"),
            ]
            .into_iter()
            .find(|(key, ..)| value.pointer(&format!("/shape/{key}")).is_some());
            if let Some((_, kind, why)) = refused {
                out.push(
                    Diagnostic::new(
                        code::COLLIDER_MESH_ON_RIGID,
                        format!("{kind} cannot be on a rigid body: {why} has no interior"),
                    )
                    .with_path("/body".to_string())
                    .with_help(
                        "set `body: static` (or `kinematic`), or give the rigid body a `convex` \
                         shape or a `compound` of convex parts",
                    ),
                );
            }
        }
        if value.get("sensor") == Some(&Value::Bool(true)) {
            if value.pointer("/body/rigid").is_some() {
                out.push(
                    Diagnostic::new(
                        code::COLLIDER_SENSOR_ON_RIGID,
                        "a sensor cannot be a rigid body: it detects overlaps and never collides"
                            .to_string(),
                    )
                    .with_path("/sensor".to_string())
                    .with_help("use `body: kinematic` and move it, or drop `sensor`"),
                );
            }
            if value.pointer("/shape/mesh").is_some()
                || value.pointer("/shape/heightfield").is_some()
            {
                out.push(
                    Diagnostic::new(
                        code::COLLIDER_SENSOR_NO_INTERIOR,
                        "a mesh or heightfield sensor has no interior, so it could only fire on a surface crossing"
                            .to_string(),
                    )
                    .with_path("/sensor".to_string())
                    .with_help("use a `box`, `sphere`, `capsule`, `cylinder`, `convex` or `compound` shape"),
                );
            }
        }
        if let Some(mass) = value.pointer("/body/rigid/mass").and_then(Value::as_f64) {
            require_positive_finite_mass(mass, &mut out);
        }
        if let Some(scale) = value
            .pointer("/body/rigid/gravity_scale")
            .and_then(Value::as_f64)
        {
            require_finite_gravity_scale(scale, &mut out);
        }
        out
    }
}

/// Validates one shape value, its pointers rooted at `base` (`/shape`, or a compound part's
/// `/shape/compound/parts/<i>/shape`).
fn validate_shape(shape: &Value, base: &str, out: &mut Vec<Diagnostic>) {
    if let Some(radius) = shape.pointer("/circle/radius").and_then(Value::as_f64) {
        require_positive_finite(radius, &format!("{base}/circle/radius"), out);
    } else if let Some(extents) = shape
        .pointer("/aabb/half_extents")
        .and_then(Value::as_array)
    {
        require_positive_finite_elements(extents, &format!("{base}/aabb/half_extents"), out);
    } else if let Some(radius) = shape.pointer("/sphere/radius").and_then(Value::as_f64) {
        require_positive_finite(radius, &format!("{base}/sphere/radius"), out);
    } else if let Some(extents) = shape.pointer("/box/half_extents").and_then(Value::as_array) {
        require_positive_finite_elements(extents, &format!("{base}/box/half_extents"), out);
    } else if shape.get("capsule").is_some() {
        validate_round_shape(shape, "capsule", base, out);
    } else if shape.get("cylinder").is_some() {
        validate_round_shape(shape, "cylinder", base, out);
    } else if let Some(convex) = shape.get("convex") {
        let no_points = Vec::new();
        let points = convex
            .get("points")
            .and_then(Value::as_array)
            .unwrap_or(&no_points);
        validate_convex_hull(points, &format!("{base}/convex/points"), out);
    } else if let Some(heightfield) = shape.get("heightfield") {
        validate_heightfield(heightfield, &format!("{base}/heightfield"), out);
    } else if let Some(compound) = shape.get("compound") {
        validate_compound(compound, &format!("{base}/compound"), out);
    } else if let Some(mesh) = shape.get("mesh") {
        let blank = mesh
            .get("path")
            .and_then(Value::as_str)
            .is_none_or(|path| path.trim().is_empty());
        if blank {
            out.push(
                Diagnostic::new(
                    code::COLLIDER_MESH_UNAVAILABLE,
                    "a mesh collider needs the path of a `kind: mesh` document".to_string(),
                )
                .with_path(format!("{base}/mesh/path"))
                .with_help("set `path` to the mesh document, relative to the content root"),
            );
        }
    }
}

/// `capsule` and `cylinder` share their two sizes: `half_height` and `radius`, both `> 0`.
fn validate_round_shape(shape: &Value, kind: &str, base: &str, out: &mut Vec<Diagnostic>) {
    for field in ["half_height", "radius"] {
        if let Some(size) = shape
            .pointer(&format!("/{kind}/{field}"))
            .and_then(Value::as_f64)
        {
            require_positive_finite(size, &format!("{base}/{kind}/{field}"), out);
        }
    }
}

/// Rejects a `compound` whose structure `ludion-physics` cannot build (`L1189`): no parts, a part
/// that is itself a `compound`, `heightfield`, `mesh`, `circle` or `aabb`, or a part `translation`/
/// `rotation` element beyond `f32` range. Every other part shape is validated as it would be on its
/// own, nested under `<compound>/parts/<i>/shape`.
fn validate_compound(compound: &Value, base: &str, out: &mut Vec<Diagnostic>) {
    let parts = compound
        .get("parts")
        .and_then(Value::as_array)
        .map_or(&[][..], Vec::as_slice);
    if parts.is_empty() {
        out.push(compound_invalid(
            "a compound needs at least one part".to_string(),
            &format!("{base}/parts"),
        ));
        return;
    }
    for (i, part) in parts.iter().enumerate() {
        let part_base = format!("{base}/parts/{i}");
        if let Some(shape) = part.get("shape") {
            let disallowed = ["compound", "heightfield", "mesh", "circle", "aabb"]
                .into_iter()
                .find(|kind| shape.get(kind).is_some());
            match disallowed {
                Some(kind) => out.push(compound_invalid(
                    format!("a compound part cannot be a `{kind}`"),
                    &format!("{part_base}/shape"),
                )),
                None => validate_shape(shape, &format!("{part_base}/shape"), out),
            }
        }
        for field in ["translation", "rotation"] {
            let Some(elements) = part.get(field).and_then(Value::as_array) else {
                continue;
            };
            for (j, element) in elements.iter().enumerate() {
                if let Some(v) = element
                    .as_f64()
                    .filter(|v| !ludion_core::number::fits_f32(*v))
                {
                    out.push(compound_invalid(
                        format!("a compound part's {field} value {v} is beyond f32 range"),
                        &format!("{part_base}/{field}/{j}"),
                    ));
                }
            }
        }
    }
}

/// `L1189` — a structurally invalid compound collider.
fn compound_invalid(message: String, pointer: &str) -> Diagnostic {
    Diagnostic::new(code::COLLIDER_COMPOUND_INVALID, message)
        .with_path(pointer.to_string())
        .with_help(
            "give the compound at least one `sphere`/`box`/`capsule`/`cylinder`/`convex` part; \
             move a `heightfield`, `mesh` or a nested `compound` to its own entity, or flatten nested \
             parts into one list",
        )
}

/// Rejects a `convex` shape's `points` that enclose no volume (`L1181`): fewer than four points, or
/// all of them coincident, collinear, or coplanar.
///
/// The tolerance is `1e-4` of the point cloud's own bounding-box diagonal, so the check has no fixed
/// unit. Every comparison is squared to avoid a square root: coincidence and the point-line/
/// point-plane tests all reduce to `numerator <= tolerance² * scale`, where `scale` is `1` for the
/// coincidence test, `|p0a|²` for the line test, and `|n|²` for the plane test.
fn validate_convex_hull(raw_points: &[Value], pointer: &str, out: &mut Vec<Diagnostic>) {
    let points: Vec<[f64; 3]> = raw_points.iter().filter_map(as_point3).collect();
    if points.len() < 4 {
        out.push(degenerate_hull(
            format!(
                "a convex hull needs at least four points, found {}",
                points.len()
            ),
            pointer,
        ));
        return;
    }

    let mut min = points[0];
    let mut max = points[0];
    for &p in &points[1..] {
        for axis in 0..3 {
            min[axis] = min[axis].min(p[axis]);
            max[axis] = max[axis].max(p[axis]);
        }
    }
    let diagonal2 = dot(sub(max, min), sub(max, min));
    let tol2 = 1e-8 * diagonal2; // (1e-4 * scale)^2, scale = sqrt(diagonal2)

    let p0 = points[0];
    let (a, a_dist2) = farthest(&points, |p| dist2(p0, p));
    if a_dist2 <= tol2 {
        out.push(degenerate_hull(
            "a convex hull's points are all coincident; they enclose no volume".to_string(),
            pointer,
        ));
        return;
    }

    let ab = sub(a, p0);
    let ab_len2 = dot(ab, ab);
    let (b, b_metric) = farthest(&points, |p| {
        let cross_ = cross(ab, sub(p, p0));
        dot(cross_, cross_)
    });
    if b_metric <= tol2 * ab_len2 {
        out.push(degenerate_hull(
            "a convex hull's points are all collinear; they enclose no volume".to_string(),
            pointer,
        ));
        return;
    }

    let n = cross(ab, sub(b, p0));
    let n_len2 = dot(n, n);
    let (_, c_metric) = farthest(&points, |p| {
        let v = dot(sub(p, p0), n);
        v * v
    });
    if c_metric <= tol2 * n_len2 {
        out.push(degenerate_hull(
            "a convex hull's points are all coplanar; they enclose no volume".to_string(),
            pointer,
        ));
    }
}

/// `L1181` — a degenerate convex hull.
fn degenerate_hull(message: String, pointer: &str) -> Diagnostic {
    Diagnostic::new(code::COLLIDER_DEGENERATE_HULL, message)
        .with_path(pointer.to_string())
        .with_help(
            "list at least four points that are not all on one plane — e.g. the corners of the \
             prop's bounding shape — or use `box`/`sphere`/`capsule` for a flat or thin prop",
        )
}

/// A JSON array `[x, y, z]` as a point, or `None` if it is not shaped that way.
fn as_point3(value: &Value) -> Option<[f64; 3]> {
    let array = value.as_array()?;
    if array.len() != 3 {
        return None;
    }
    Some([array[0].as_f64()?, array[1].as_f64()?, array[2].as_f64()?])
}

/// The point in `points` maximizing `metric`, and that maximum value.
fn farthest(points: &[[f64; 3]], metric: impl Fn([f64; 3]) -> f64) -> ([f64; 3], f64) {
    points.iter().copied().map(|p| (p, metric(p))).fold(
        ([0.0; 3], f64::NEG_INFINITY),
        |best, cur| {
            if cur.1 > best.1 { cur } else { best }
        },
    )
}

fn sub(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn dist2(a: [f64; 3], b: [f64; 3]) -> f64 {
    dot(sub(b, a), sub(b, a))
}

fn dot(a: [f64; 3], b: [f64; 3]) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn cross(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

/// Rejects a `heightfield` shape whose grid does not hang together (`L1182`): `rows`/`cols` below 2
/// (an omitted field reads as its serialized default, `0`, which is itself below 2), or
/// `heights.len() != rows * cols`. The length check only runs once both `rows` and `cols` are
/// individually valid, so a single malformed field raises one diagnostic, not two. Keeps the
/// existing `scale` extent check (`L1116`) in the same branch.
fn validate_heightfield(heightfield: &Value, base: &str, out: &mut Vec<Diagnostic>) {
    let rows = heightfield.get("rows").and_then(Value::as_u64).unwrap_or(0);
    let cols = heightfield.get("cols").and_then(Value::as_u64).unwrap_or(0);
    let mut grid_ok = true;
    if rows < 2 {
        out.push(heightfield_grid(
            format!("`rows` must be at least 2, found {rows}"),
            &format!("{base}/rows"),
        ));
        grid_ok = false;
    }
    if cols < 2 {
        out.push(heightfield_grid(
            format!("`cols` must be at least 2, found {cols}"),
            &format!("{base}/cols"),
        ));
        grid_ok = false;
    }
    if grid_ok {
        let heights_len = heightfield
            .get("heights")
            .and_then(Value::as_array)
            .map_or(0, |heights| heights.len() as u64);
        let expected = rows * cols;
        if heights_len != expected {
            out.push(heightfield_grid(
                format!("found {heights_len} heights, a {rows}\u{d7}{cols} grid needs {expected}"),
                &format!("{base}/heights"),
            ));
        }
    }

    if let Some(scale) = heightfield.get("scale").and_then(Value::as_array) {
        require_positive_finite_elements(scale, &format!("{base}/scale"), out);
    }
}

/// `L1182` — a malformed heightfield grid.
fn heightfield_grid(message: String, pointer: &str) -> Diagnostic {
    Diagnostic::new(code::COLLIDER_HEIGHTFIELD_GRID, message)
        .with_path(pointer.to_string())
        .with_help("give `rows`/`cols` at least 2 each, and supply exactly rows * cols heights")
}

/// Runs [`require_positive_finite`] over every element of a JSON array, at `<base>/<index>`.
fn require_positive_finite_elements(elements: &[Value], base: &str, out: &mut Vec<Diagnostic>) {
    for (index, element) in elements.iter().enumerate() {
        if let Some(value) = element.as_f64() {
            require_positive_finite(value, &format!("{base}/{index}"), out);
        }
    }
}

/// Pushes an `L1116` when `value` is not greater than zero as the `f32` the engine stores (zero,
/// negative, non-finite, past `f32`'s range, or so small it narrows to `0.0`). The judgement is on
/// the narrowed value, not the `f64` the document yields, so the query math never sees a bad size.
fn require_positive_finite(value: f64, pointer: &str, out: &mut Vec<Diagnostic>) {
    if !ludion_core::number::positive_f32(value) {
        out.push(non_positive_extent(value, pointer));
    }
}

/// `L1116` — a collider size is not a positive `f32`.
fn non_positive_extent(value: f64, pointer: &str) -> Diagnostic {
    Diagnostic::new(
        code::COLLIDER_NON_POSITIVE_EXTENT,
        format!(
            "collider size {value} is not valid; a radius or half-extent must be greater than 0 \
             as an f32 (at least ~1.4e-45, at most ~3.4e38)"
        ),
    )
    .with_path(pointer.to_string())
    .with_help("give every collider radius and half-extent a value greater than 0")
}

/// Pushes an `L1116` when a `rigid` body's `mass` is not greater than zero as an `f32`.
fn require_positive_finite_mass(value: f64, out: &mut Vec<Diagnostic>) {
    if !ludion_core::number::positive_f32(value) {
        out.push(
            Diagnostic::new(
                code::COLLIDER_NON_POSITIVE_EXTENT,
                format!(
                    "rigid body mass {value} is not valid; mass must be greater than 0 as an \
                     f32 (at least ~1.4e-45, at most ~3.4e38)"
                ),
            )
            .with_path("/body/rigid/mass")
            .with_help("give the rigid body's mass a value greater than 0"),
        );
    }
}

/// Pushes an `L1116` when a `rigid` body's `gravity_scale` is not finite as an `f32` (a JSON number
/// beyond `f32`'s range deserializes to infinity). Zero and negative scales are valid.
fn require_finite_gravity_scale(value: f64, out: &mut Vec<Diagnostic>) {
    if !ludion_core::number::fits_f32(value) {
        out.push(
            Diagnostic::new(
                code::COLLIDER_NON_POSITIVE_EXTENT,
                format!(
                    "rigid body gravity_scale {value} is not valid; it must be a finite number \
                     (0 floats, a negative value rises)"
                ),
            )
            .with_path("/body/rigid/gravity_scale")
            .with_help(
                "give gravity_scale a finite value, e.g. 0.0 to float or 1.0 for normal gravity",
            ),
        );
    }
}

fn half() -> f32 {
    0.5
}

fn half2() -> [f32; 2] {
    [0.5, 0.5]
}

fn half3() -> [f32; 3] {
    [0.5, 0.5, 0.5]
}

fn one() -> f32 {
    1.0
}

fn is_one(value: &f32) -> bool {
    *value == 1.0
}

fn is_half(value: &f32) -> bool {
    *value == 0.5
}

fn is_false(value: &bool) -> bool {
    !*value
}

fn is_zero(value: &f32) -> bool {
    *value == 0.0
}

fn is_average(rule: &CombineRule) -> bool {
    *rule == CombineRule::Average
}

fn one3() -> [f32; 3] {
    [1.0, 1.0, 1.0]
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn an_absent_layer_is_omitted_and_defaults() {
        let json = serde_json::to_value(Collider::default()).unwrap();
        assert!(json.get("layer").is_none());
        assert_eq!(
            serde_json::from_value::<Collider>(json!({})).unwrap().layer,
            None
        );
        let layered: Collider = serde_json::from_value(json!({ "layer": "dummy" })).unwrap();
        assert_eq!(layered.layer.as_deref(), Some("dummy"));
        assert_eq!(serde_json::to_value(&layered).unwrap()["layer"], "dummy");
    }

    #[test]
    fn circle_round_trips_externally_tagged() {
        let collider = Collider {
            shape: Shape::Circle { radius: 1.25 },
            body: Body::new_kinematic(),
            layer: None,
            sensor: false,
        };
        let json = serde_json::to_value(&collider).unwrap();
        assert_eq!(
            json,
            json!({ "shape": { "circle": { "radius": 1.25 } }, "body": "kinematic" })
        );
        assert_eq!(serde_json::from_value::<Collider>(json).unwrap(), collider);
    }

    #[test]
    fn aabb_round_trips_externally_tagged() {
        let collider = Collider {
            shape: Shape::Aabb {
                half_extents: [0.5, 2.0],
            },
            body: Body::new_static(),
            layer: None,
            sensor: false,
        };
        let json = serde_json::to_value(&collider).unwrap();
        assert_eq!(
            json,
            json!({ "shape": { "aabb": { "half_extents": [0.5, 2.0] } }, "body": "static" })
        );
        assert_eq!(serde_json::from_value::<Collider>(json).unwrap(), collider);
    }

    /// The legacy `dynamic` spelling still deserializes, to [`Body::Kinematic`] — the migration
    /// this rename carries so a scene written before it still loads.
    #[test]
    fn dynamic_is_accepted_as_an_alias_for_kinematic() {
        let collider: Collider = serde_json::from_value(
            json!({ "shape": { "circle": { "radius": 0.5 } }, "body": "dynamic" }),
        )
        .unwrap();
        assert_eq!(collider.body, Body::new_kinematic());
    }

    /// The canonical writer always emits `kinematic`, never the legacy `dynamic` spelling — `alias`
    /// is deserialize-only.
    #[test]
    fn kinematic_always_serializes_as_kinematic() {
        let json = serde_json::to_value(Body::new_kinematic()).unwrap();
        assert_eq!(json, json!("kinematic"));
    }

    #[test]
    fn rigid_round_trips_with_every_field_defaulted() {
        let collider = Collider {
            shape: Shape::Sphere { radius: 1.0 },
            body: Body::Rigid(RigidBody::default()),
            layer: None,
            sensor: false,
        };
        let json = serde_json::to_value(&collider).unwrap();
        assert_eq!(
            json,
            json!({
                "shape": { "sphere": { "radius": 1.0 } },
                "body": { "rigid": {
                    "mass": 1.0,
                    "friction": 0.5,
                    "restitution": 0.0,
                    "linear_damping": 0.0,
                    "angular_damping": 0.0,
                    "lock": { "translation": [false, false, false], "rotation": [false, false, false] },
                } }
            })
        );
        assert_eq!(serde_json::from_value::<Collider>(json).unwrap(), collider);
        // `body: { rigid: {} }` — every field defaults, drift-safe.
        let collider: Collider = serde_json::from_value(json!({
            "shape": { "sphere": { "radius": 1.0 } },
            "body": { "rigid": {} },
        }))
        .unwrap();
        assert_eq!(collider.body, Body::Rigid(RigidBody::default()));
    }

    #[test]
    fn every_3d_shape_round_trips_externally_tagged() {
        let cases = [
            (
                Shape::Sphere { radius: 0.75 },
                json!({ "sphere": { "radius": 0.75 } }),
            ),
            (
                Shape::Box {
                    half_extents: [0.5, 1.0, 1.5],
                },
                json!({ "box": { "half_extents": [0.5, 1.0, 1.5] } }),
            ),
            (
                Shape::Capsule {
                    half_height: 0.5,
                    radius: 0.25,
                },
                json!({ "capsule": { "half_height": 0.5, "radius": 0.25 } }),
            ),
            (
                Shape::Cylinder {
                    half_height: 0.5,
                    radius: 0.25,
                },
                json!({ "cylinder": { "half_height": 0.5, "radius": 0.25 } }),
            ),
            (
                Shape::Compound {
                    parts: vec![CompoundPart {
                        shape: Shape::Sphere { radius: 0.5 },
                        translation: [0.0, -1.0, 0.0],
                        rotation: [0.0, 45.0, 0.0],
                    }],
                },
                json!({ "compound": { "parts": [{
                    "shape": { "sphere": { "radius": 0.5 } },
                    "translation": [0.0, -1.0, 0.0],
                    "rotation": [0.0, 45.0, 0.0],
                }] } }),
            ),
            (
                Shape::Convex {
                    points: vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0]],
                },
                json!({ "convex": { "points": [[0.0, 0.0, 0.0], [1.0, 0.0, 0.0]] } }),
            ),
            (
                Shape::Heightfield {
                    rows: 2,
                    cols: 2,
                    heights: vec![0.0, 1.0, 2.0, 3.0],
                    scale: [1.0, 1.0, 1.0],
                },
                json!({ "heightfield": {
                    "rows": 2, "cols": 2, "heights": [0.0, 1.0, 2.0, 3.0], "scale": [1.0, 1.0, 1.0],
                } }),
            ),
        ];
        for (shape, expected) in cases {
            let json = serde_json::to_value(&shape).unwrap();
            assert_eq!(json, expected);
            assert_eq!(serde_json::from_value::<Shape>(json).unwrap(), shape);
            assert!(shape.is_3d(), "{shape:?} must report is_3d()");
        }
        assert!(!Shape::Circle { radius: 0.5 }.is_3d());
        assert!(
            !Shape::Aabb {
                half_extents: [0.5, 0.5]
            }
            .is_3d()
        );
    }

    #[test]
    fn empty_object_is_the_drift_safe_default() {
        let collider: Collider = serde_json::from_value(json!({})).unwrap();
        assert_eq!(collider, Collider::default());
        assert_eq!(collider.shape, Shape::Circle { radius: 0.5 });
        assert_eq!(collider.body, Body::new_static());
    }

    #[test]
    fn semantic_validation_flags_zero_negative_and_non_finite() {
        // Zero and negative radii, and each non-finite value, are rejected with the pointer at the
        // exact offending size. (A negative radius is reachable through the semantic hook only when
        // the schema does not run; the schema catches it as `L1014` first in a real scene — this
        // unit-tests the guard directly, including `NaN`/`inf`, which no JSON value can carry.)
        for bad in [0.0, -1.0, f64::NAN, f64::INFINITY] {
            let mut out = Vec::new();
            require_positive_finite(bad, "/shape/circle/radius", &mut out);
            assert_eq!(out.len(), 1, "expected one diagnostic for {bad}");
            assert_eq!(out[0].code.as_str(), "L1116");
            assert_eq!(out[0].path.as_deref(), Some("/shape/circle/radius"));
        }
    }

    #[test]
    fn semantic_validation_accepts_positive_sizes() {
        let ok = json!({ "shape": { "aabb": { "half_extents": [0.5, 3.0] } }, "body": "static" });
        assert!(Collider::validate_semantics(&ok).is_empty());
        let ok = json!({ "shape": { "circle": { "radius": 0.5 } }, "body": "kinematic" });
        assert!(Collider::validate_semantics(&ok).is_empty());
        let ok = json!({ "shape": { "sphere": { "radius": 0.5 } } });
        assert!(Collider::validate_semantics(&ok).is_empty());
        let ok = json!({ "shape": { "box": { "half_extents": [0.5, 1.0, 1.5] } } });
        assert!(Collider::validate_semantics(&ok).is_empty());
        let ok = json!({ "shape": { "capsule": { "half_height": 0.5, "radius": 0.25 } } });
        assert!(Collider::validate_semantics(&ok).is_empty());
        let ok = json!({ "shape": { "heightfield": {
            "rows": 2, "cols": 2, "heights": [0.0, 0.0, 0.0, 0.0], "scale": [1.0, 1.0, 1.0],
        } } });
        assert!(Collider::validate_semantics(&ok).is_empty());
    }

    #[test]
    fn semantic_validation_reports_the_offending_aabb_element() {
        let bad = json!({ "shape": { "aabb": { "half_extents": [1.0, 0.0] } } });
        let out = Collider::validate_semantics(&bad);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].code.as_str(), "L1116");
        assert_eq!(out[0].path.as_deref(), Some("/shape/aabb/half_extents/1"));
    }

    #[test]
    fn semantic_validation_covers_every_3d_size() {
        let bad = json!({ "shape": { "sphere": { "radius": 0.0 } } });
        let out = Collider::validate_semantics(&bad);
        assert_eq!(out[0].path.as_deref(), Some("/shape/sphere/radius"));

        let bad = json!({ "shape": { "box": { "half_extents": [1.0, -1.0, 1.0] } } });
        let out = Collider::validate_semantics(&bad);
        assert_eq!(out[0].path.as_deref(), Some("/shape/box/half_extents/1"));

        let bad = json!({ "shape": { "capsule": { "half_height": 0.0, "radius": 1.0 } } });
        let out = Collider::validate_semantics(&bad);
        assert_eq!(out[0].path.as_deref(), Some("/shape/capsule/half_height"));

        let bad = json!({ "shape": { "capsule": { "half_height": 1.0, "radius": -1.0 } } });
        let out = Collider::validate_semantics(&bad);
        assert_eq!(out[0].path.as_deref(), Some("/shape/capsule/radius"));

        let bad = json!({ "shape": { "heightfield": {
            "rows": 2, "cols": 2, "heights": [0.0, 0.0, 0.0, 0.0], "scale": [1.0, 1.0, 0.0],
        } } });
        let out = Collider::validate_semantics(&bad);
        assert_eq!(out[0].path.as_deref(), Some("/shape/heightfield/scale/2"));
    }

    #[test]
    fn a_convex_hull_with_fewer_than_four_points_is_l1181() {
        let bad = json!({ "shape": { "convex": { "points": [[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]] } } });
        let out = Collider::validate_semantics(&bad);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].code.as_str(), "L1181");
        assert_eq!(out[0].path.as_deref(), Some("/shape/convex/points"));
        assert!(out[0].message.contains("found 3"), "{}", out[0].message);
    }

    #[test]
    fn a_convex_hull_of_coincident_points_is_l1181() {
        let bad = json!({ "shape": { "convex": { "points": [
            [1.0, 1.0, 1.0], [1.0, 1.0, 1.0], [1.0, 1.0, 1.0], [1.0, 1.0, 1.0],
        ] } } });
        let out = Collider::validate_semantics(&bad);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].code.as_str(), "L1181");
        assert!(out[0].message.contains("coincident"), "{}", out[0].message);
    }

    #[test]
    fn a_convex_hull_of_collinear_points_is_l1181() {
        let bad = json!({ "shape": { "convex": { "points": [
            [0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [2.0, 0.0, 0.0], [3.0, 0.0, 0.0],
        ] } } });
        let out = Collider::validate_semantics(&bad);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].code.as_str(), "L1181");
        assert!(out[0].message.contains("collinear"), "{}", out[0].message);
    }

    #[test]
    fn a_convex_hull_of_coplanar_points_is_l1181() {
        let bad = json!({ "shape": { "convex": { "points": [
            [0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [1.0, 1.0, 0.0],
        ] } } });
        let out = Collider::validate_semantics(&bad);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].code.as_str(), "L1181");
        assert!(out[0].message.contains("coplanar"), "{}", out[0].message);
    }

    #[test]
    fn a_unit_tetrahedron_and_cube_corners_are_accepted() {
        let tetrahedron = json!({ "shape": { "convex": { "points": [
            [0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0],
        ] } } });
        assert!(Collider::validate_semantics(&tetrahedron).is_empty());

        let cube = json!({ "shape": { "convex": { "points": [
            [0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0],
            [1.0, 1.0, 0.0], [1.0, 0.0, 1.0], [0.0, 1.0, 1.0], [1.0, 1.0, 1.0],
        ] } } });
        assert!(Collider::validate_semantics(&cube).is_empty());
    }

    #[test]
    fn a_3_by_2_heightfield_grid_with_6_heights_is_accepted() {
        let ok = json!({ "shape": { "heightfield": {
            "rows": 3, "cols": 2, "heights": [0.0, 0.0, 0.0, 0.0, 0.0, 0.0],
        } } });
        assert!(Collider::validate_semantics(&ok).is_empty());
    }

    #[test]
    fn a_ragged_heightfield_grid_is_l1182() {
        let bad = json!({ "shape": { "heightfield": { "rows": 1, "cols": 4, "heights": [0.0, 0.0, 0.0, 0.0] } } });
        let out = Collider::validate_semantics(&bad);
        assert_eq!(out[0].code.as_str(), "L1182");
        assert_eq!(out[0].path.as_deref(), Some("/shape/heightfield/rows"));

        let bad = json!({ "shape": { "heightfield": { "rows": 3, "cols": 3, "heights": [1.0] } } });
        let out = Collider::validate_semantics(&bad);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].code.as_str(), "L1182");
        assert_eq!(out[0].path.as_deref(), Some("/shape/heightfield/heights"));
        assert!(
            out[0].message.contains("found 1 heights") && out[0].message.contains("needs 9"),
            "{}",
            out[0].message
        );
    }

    #[test]
    fn a_zero_rigid_mass_is_l1116_at_body_rigid_mass() {
        let bad = json!({ "shape": { "sphere": { "radius": 0.5 } }, "body": { "rigid": { "mass": 0.0 } } });
        let out = Collider::validate_semantics(&bad);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].code.as_str(), "L1116");
        assert_eq!(out[0].path.as_deref(), Some("/body/rigid/mass"));
    }

    #[test]
    fn a_size_or_mass_that_narrows_to_zero_is_l1116_and_the_smallest_f32_is_accepted() {
        let radius = json!({ "shape": { "circle": { "radius": 1e-46 } } });
        let half = json!({ "shape": { "box": { "half_extents": [0.5, 0.5, 1e-46] } } });
        let mass = json!({ "shape": { "sphere": { "radius": 0.5 } }, "body": { "rigid": { "mass": 1e-46 } } });
        for (doc, pointer) in [
            (radius, "/shape/circle/radius"),
            (half, "/shape/box/half_extents/2"),
            (mass, "/body/rigid/mass"),
        ] {
            let out = Collider::validate_semantics(&doc);
            assert_eq!(out.len(), 1, "{pointer}");
            assert_eq!(out[0].code.as_str(), "L1116");
            assert_eq!(out[0].path.as_deref(), Some(pointer));
        }
        let boundary = json!({ "shape": { "circle": { "radius": 1.5e-45 } } });
        assert!(Collider::validate_semantics(&boundary).is_empty());
    }

    #[test]
    fn a_gravity_scale_beyond_f32_is_l1116_at_body_rigid_gravity_scale() {
        let bad = json!({ "shape": { "sphere": { "radius": 0.5 } }, "body": { "rigid": { "gravity_scale": 1e39 } } });
        let out = Collider::validate_semantics(&bad);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].code.as_str(), "L1116");
        assert_eq!(out[0].path.as_deref(), Some("/body/rigid/gravity_scale"));
    }

    #[test]
    fn zero_and_negative_gravity_scales_are_accepted() {
        for scale in [0.0, -1.0, -0.5, 2.5] {
            let ok = json!({ "shape": { "sphere": { "radius": 0.5 } }, "body": { "rigid": { "gravity_scale": scale } } });
            assert!(Collider::validate_semantics(&ok).is_empty(), "{scale}");
        }
    }

    #[test]
    fn gravity_scale_round_trips_when_set() {
        for scale in [0.0_f32, -0.5] {
            let collider = Collider {
                shape: Shape::Sphere { radius: 1.0 },
                body: Body::Rigid(RigidBody {
                    gravity_scale: scale,
                    ..RigidBody::default()
                }),
                layer: None,
                sensor: false,
            };
            let json = serde_json::to_value(&collider).unwrap();
            assert_eq!(
                json.pointer("/body/rigid/gravity_scale"),
                Some(&json!(scale))
            );
            assert_eq!(serde_json::from_value::<Collider>(json).unwrap(), collider);
        }
    }

    #[test]
    fn an_empty_rigid_body_is_accepted() {
        let ok = json!({ "shape": { "sphere": { "radius": 0.5 } }, "body": { "rigid": {} } });
        assert!(Collider::validate_semantics(&ok).is_empty());
    }

    #[test]
    fn a_compound_part_serializes_its_default_pose_and_reads_it_back() {
        let part: CompoundPart =
            serde_json::from_value(json!({ "shape": { "sphere": { "radius": 0.5 } } })).unwrap();
        assert_eq!(part.translation, [0.0; 3]);
        assert_eq!(part.rotation, [0.0; 3]);
        assert_eq!(
            serde_json::to_value(&part).unwrap(),
            json!({
                "shape": { "sphere": { "radius": 0.5 } },
                "translation": [0.0, 0.0, 0.0],
                "rotation": [0.0, 0.0, 0.0],
            })
        );
        assert!(serde_json::from_value::<CompoundPart>(json!({})).is_err());
    }

    fn only(bad: Value, code: &str, pointer: &str) {
        let out = Collider::validate_semantics(&bad);
        assert_eq!(out.len(), 1, "{out:?}");
        assert_eq!(out[0].code.as_str(), code);
        assert_eq!(out[0].path.as_deref(), Some(pointer));
    }

    #[test]
    fn a_zero_cylinder_radius_is_l1116() {
        only(
            json!({ "shape": { "cylinder": { "half_height": 0.5, "radius": 0.0 } } }),
            "L1116",
            "/shape/cylinder/radius",
        );
        let ok = json!({ "shape": { "cylinder": { "half_height": 0.5, "radius": 0.25 } } });
        assert!(Collider::validate_semantics(&ok).is_empty());
    }

    #[test]
    fn an_empty_compound_is_l1189() {
        only(
            json!({ "shape": { "compound": { "parts": [] } } }),
            "L1189",
            "/shape/compound/parts",
        );
        only(
            json!({ "shape": { "compound": {} } }),
            "L1189",
            "/shape/compound/parts",
        );
    }

    #[test]
    fn a_disallowed_compound_part_is_l1189_at_its_shape() {
        let box_part = json!({ "shape": { "box": { "half_extents": [0.5, 0.5, 0.5] } } });
        let heightfield = json!({ "shape": { "heightfield": {
            "rows": 2, "cols": 2, "heights": [0.0, 0.0, 0.0, 0.0],
        } } });
        let nested = json!({ "shape": { "compound": { "parts": [box_part.clone()] } } });
        let circle = json!({ "shape": { "circle": { "radius": 0.5 } } });
        for (index, bad_part) in [heightfield, nested, circle].into_iter().enumerate() {
            let parts = if index == 0 {
                vec![bad_part]
            } else {
                vec![box_part.clone(), bad_part]
            };
            let pointer = format!("/shape/compound/parts/{}/shape", parts.len() - 1);
            only(
                json!({ "shape": { "compound": { "parts": parts } } }),
                "L1189",
                &pointer,
            );
        }
    }

    #[test]
    fn a_mesh_round_trips_and_is_3d() {
        let collider: Collider = serde_json::from_value(
            json!({ "shape": { "mesh": { "path": "meshes/floor.mesh.yaml" } }, "body": "static" }),
        )
        .unwrap();
        assert_eq!(
            collider.shape,
            Shape::Mesh {
                path: "meshes/floor.mesh.yaml".into()
            }
        );
        assert!(collider.shape.is_3d());
        let back: Collider =
            serde_json::from_value(serde_json::to_value(&collider).unwrap()).unwrap();
        assert_eq!(back, collider);
        assert!(Collider::validate_semantics(&serde_json::to_value(&collider).unwrap()).is_empty());
    }

    #[test]
    fn a_mesh_on_a_rigid_body_is_l1211() {
        only(
            json!({ "shape": { "mesh": { "path": "a.mesh.yaml" } }, "body": { "rigid": {} } }),
            "L1211",
            "/body",
        );
    }

    #[test]
    fn a_heightfield_on_a_rigid_body_is_l1211() {
        only(
            json!({
                "shape": { "heightfield": { "rows": 2, "cols": 2, "heights": [0.0, 0.0, 0.0, 0.0] } },
                "body": { "rigid": {} }
            }),
            "L1211",
            "/body",
        );
    }

    #[test]
    fn a_rigid_sensor_is_l1291() {
        only(
            json!({ "shape": { "sphere": { "radius": 1.0 } }, "body": { "rigid": {} }, "sensor": true }),
            "L1291",
            "/sensor",
        );
    }

    #[test]
    fn the_l1292_message_has_no_double_spaces() {
        let found = Collider::validate_semantics(
            &json!({ "shape": { "heightfield": { "rows": 2, "cols": 2, "heights": [0.0, 0.0, 0.0, 0.0] } }, "sensor": true }),
        );
        assert_eq!(
            found[0].message,
            "a mesh or heightfield sensor has no interior, so it could only fire on a surface crossing"
        );
    }

    #[test]
    fn a_heightfield_or_mesh_sensor_is_l1292() {
        only(
            json!({
                "shape": { "heightfield": { "rows": 2, "cols": 2, "heights": [0.0, 0.0, 0.0, 0.0] } },
                "sensor": true
            }),
            "L1292",
            "/sensor",
        );
        only(
            json!({ "shape": { "mesh": { "path": "a.mesh.yaml" } }, "body": "kinematic", "sensor": true }),
            "L1292",
            "/sensor",
        );
    }

    #[test]
    fn a_volume_sensor_is_clean() {
        for value in [
            json!({ "shape": { "box": { "half_extents": [1.0, 1.0, 1.0] } }, "sensor": true }),
            json!({
                "shape": { "convex": { "points": [[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]] } },
                "body": "kinematic",
                "sensor": true
            }),
            json!({
                "shape": { "compound": { "parts": [{ "shape": { "sphere": { "radius": 1.0 } } }] } },
                "sensor": true
            }),
        ] {
            assert!(Collider::validate_semantics(&value).is_empty(), "{value}");
        }
    }

    #[test]
    fn sensor_serializes_only_when_true() {
        let off = serde_json::to_value(Collider::default()).unwrap();
        assert!(off.get("sensor").is_none());
        let on = Collider {
            sensor: true,
            ..Collider::default()
        };
        let value = serde_json::to_value(&on).unwrap();
        assert_eq!(value["sensor"], json!(true));
        assert_eq!(serde_json::from_value::<Collider>(value).unwrap(), on);
    }

    #[test]
    fn a_blank_mesh_path_is_l1212() {
        only(
            json!({ "shape": { "mesh": { "path": "  " } } }),
            "L1212",
            "/shape/mesh/path",
        );
        only(
            json!({ "shape": { "mesh": {} } }),
            "L1212",
            "/shape/mesh/path",
        );
    }

    #[test]
    fn a_mesh_compound_part_is_l1189() {
        only(
            json!({ "shape": { "compound": { "parts": [
                { "shape": { "mesh": { "path": "a.mesh.yaml" } } },
            ] } } }),
            "L1189",
            "/shape/compound/parts/0/shape",
        );
    }

    #[test]
    fn a_compound_part_size_is_checked_under_its_pointer() {
        only(
            json!({ "shape": { "compound": { "parts": [
                { "shape": { "sphere": { "radius": 0.5 } } },
                { "shape": { "cylinder": { "half_height": -1.0, "radius": 0.5 } } },
            ] } } }),
            "L1116",
            "/shape/compound/parts/1/shape/cylinder/half_height",
        );
        only(
            json!({ "shape": { "compound": { "parts": [
                { "shape": { "convex": { "points": [[0.0, 0.0, 0.0], [1.0, 0.0, 0.0]] } } },
            ] } } }),
            "L1181",
            "/shape/compound/parts/0/shape/convex/points",
        );
    }

    #[test]
    fn a_compound_part_pose_beyond_f32_is_l1189() {
        only(
            json!({ "shape": { "compound": { "parts": [
                { "shape": { "sphere": { "radius": 0.5 } }, "translation": [1e39, 0.0, 0.0] },
            ] } } }),
            "L1189",
            "/shape/compound/parts/0/translation/0",
        );
        only(
            json!({ "shape": { "compound": { "parts": [
                { "shape": { "sphere": { "radius": 0.5 } }, "rotation": [0.0, 0.0, -1e39] },
            ] } } }),
            "L1189",
            "/shape/compound/parts/0/rotation/2",
        );
        let ok = json!({ "shape": { "compound": { "parts": [
            { "shape": { "sphere": { "radius": 0.5 } }, "translation": [1.0, 2.0, 3.0] },
            { "shape": { "box": { "half_extents": [0.5, 0.5, 0.5] } }, "rotation": [0.0, 90.0, 0.0] },
        ] } } });
        assert!(Collider::validate_semantics(&ok).is_empty());
    }

    fn body_json(body: Body) -> serde_json::Value {
        serde_json::to_value(body).unwrap()
    }

    fn body_from(json: serde_json::Value) -> Body {
        serde_json::from_value(json).unwrap()
    }

    #[test]
    fn a_malformed_body_keeps_serdes_specific_message() {
        let message = |json: serde_json::Value| {
            serde_json::from_value::<Body>(json)
                .unwrap_err()
                .to_string()
        };
        let bad_field = message(json!({ "rigid": { "mass": "heavy" } }));
        assert!(bad_field.contains("string \"heavy\""), "{bad_field}");
        let bad_rule = message(json!({ "static": { "friction_combine": "bogus" } }));
        assert!(bad_rule.contains("bogus"), "{bad_rule}");
        let bad_name = message(json!("wobbly"));
        assert!(bad_name.contains("unknown variant `wobbly`"), "{bad_name}");
        for text in [bad_field, bad_rule, bad_name] {
            assert!(!text.contains("did not match any variant"), "{text}");
        }
    }

    #[test]
    fn contact_material_maps_round_trip_verbatim() {
        for json in [
            json!({ "static": { "restitution": 0.25 } }),
            json!({ "kinematic": { "friction_combine": "max" } }),
        ] {
            assert_eq!(body_json(body_from(json.clone())), json);
        }
        let Body::Static(m) = body_from(json!({ "static": { "restitution": 0.25 } })) else {
            panic!("not static");
        };
        assert_eq!(m.restitution, 0.25);
        assert_eq!(m.friction, 0.5);
    }

    #[test]
    fn default_material_reserializes_as_the_bare_name() {
        assert_eq!(
            body_json(body_from(json!({ "static": {} }))),
            json!("static")
        );
        assert_eq!(
            body_json(body_from(json!({ "static": { "friction": 0.5 } }))),
            json!("static")
        );
        assert_eq!(
            body_json(body_from(json!({ "kinematic": {} }))),
            json!("kinematic")
        );
        assert_eq!(body_from(json!("static")), Body::new_static());
    }

    #[test]
    fn rigid_combine_rules_round_trip_and_omit_the_default() {
        let body = body_from(json!({ "rigid": { "restitution_combine": "max" } }));
        let Body::Rigid(rigid) = body else {
            panic!("not rigid")
        };
        assert_eq!(rigid.restitution_combine, CombineRule::Max);
        assert_eq!(rigid.friction_combine, CombineRule::Average);
        let out = body_json(body);
        assert_eq!(out["rigid"]["restitution_combine"], json!("max"));
        assert!(out["rigid"].get("friction_combine").is_none());
    }

    #[test]
    fn material_reads_each_body_kind() {
        let m = ContactMaterial {
            friction: 0.2,
            restitution: 0.7,
            friction_combine: CombineRule::Min,
            restitution_combine: CombineRule::Max,
        };
        assert_eq!(Body::Static(m).material(), m);
        assert_eq!(Body::Kinematic(m).material(), m);
        assert_eq!(Body::new_static().material(), ContactMaterial::default());
        let rigid = Body::Rigid(RigidBody {
            friction: 0.2,
            restitution: 0.7,
            friction_combine: CombineRule::Min,
            restitution_combine: CombineRule::Max,
            ..RigidBody::default()
        });
        assert_eq!(rigid.material(), m);
    }
}
