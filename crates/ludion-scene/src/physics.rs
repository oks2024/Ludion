//! The scene's optional top-level `physics:` block (`Plan/physics.md` W2): present, every host that
//! builds a world from the scene installs the rigid-body write/step/mirror triple; absent, exactly
//! today. Mirrors `seed`'s presence-is-the-signal shape (`lib.rs`'s `Scene::seed`).
//!
//! `ludion-scene` does not depend on `ludion-physics` (crate-boundary rule,
//! `engine-architecture.md` §2: this crate never learns a solver type exists), so [`ScenePhysics`]
//! is its own plain-data mirror of `ludion_physics::PhysicsSettings`; `ludion-engine` — the one
//! crate that depends on both — maps one onto the other.

use serde_json::Value;

use ludion_core::codes::code;
use ludion_core::diagnostics::Diagnostic;
use ludion_core::suggest;

use crate::envelope;
use crate::parse::expect_u64;
use crate::physics_layers::{SceneLayer, check_layers};
use crate::yaml::{MapEntry, Node, NodeValue};

/// The keys a scene's `physics:` block accepts, in canonical emission order.
pub(crate) const PHYSICS_KEYS: [&str; 6] = [
    "space",
    "gravity",
    "solver_iterations",
    "stabilization_iterations",
    "sleep_after_ticks",
    "layers",
];

/// The keys one `physics.layers` entry accepts.
const LAYER_KEYS: [&str; 2] = ["name", "ignores"];

/// The inclusive range a `solver_iterations`/`stabilization_iterations` count must fall in: below
/// it the solver would not run at all, above it a runaway count stalls every host's tick.
const ITERATIONS_RANGE: std::ops::RangeInclusive<u64> = 1..=64;

/// Whether a scene's rigid bodies simulate with the engine's `Plane` ignored ([`Free`](Self::Free)) or its
/// locks applied ([`Planar`](Self::Planar)) — the scene-authored form of `ludion_physics::PhysicsSpace`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScenePhysicsSpace {
    /// `space: free` — full three-dimensional motion.
    Free,
    /// `space: planar` — the engine's `Plane` locks every rigid body.
    Planar,
}

impl ScenePhysicsSpace {
    /// The canonical spelling, written by the emitter and read back by the parser.
    pub fn as_str(self) -> &'static str {
        match self {
            ScenePhysicsSpace::Free => "free",
            ScenePhysicsSpace::Planar => "planar",
        }
    }
}

/// A scene's `physics:` block.
///
/// Present on [`crate::Scene::physics`] means every host that builds a world from the scene
/// installs the rigid-body write/step/mirror triple, in `space`, with the solver tunables below —
/// absent behind their pinned defaults when a field is `None`.
#[derive(Clone, Debug, PartialEq)]
pub struct ScenePhysics {
    /// The space rigid bodies simulate in — required inside the block, since the engine's default
    /// `Plane` would otherwise silently freeze the axis gravity points down.
    pub space: ScenePhysicsSpace,
    /// The constant acceleration applied to every rigid body, as authored (`x`, `y`, `z`), or
    /// `None` for the pinned default (one standard gravity down `y`). A top-down game sets
    /// `[0, 0, 0]`.
    pub gravity: Option<[f64; 3]>,
    /// `rapier`'s solver iteration count (`1..=64`), or `None` for the pinned default.
    pub solver_iterations: Option<u32>,
    /// `rapier`'s internal stabilization iteration count (`1..=64`), or `None` for the pinned
    /// default.
    pub stabilization_iterations: Option<u32>,
    /// The scene-wide sleep default a rigid body falls back to when it omits its own
    /// `sleep_after_ticks`, or `None` for the pinned default.
    pub sleep_after_ticks: Option<u32>,
    /// The named collision layers a collider's `layer` may name, in declaration order; empty when
    /// the scene declares none. [`ScenePhysics::layer_matrix`] resolves them.
    pub layers: Vec<SceneLayer>,
}

/// Parses a scene's `physics:` block. `None` on a structural or value failure — always after
/// pushing at least one diagnostic, so the caller never has to explain the loss on its own.
pub(crate) fn parse_physics(
    node: &Node,
    diagnostics: &mut Vec<Diagnostic>,
) -> Option<ScenePhysics> {
    let NodeValue::Mapping(entries) = &node.value else {
        diagnostics.push(
            Diagnostic::new(
                code::SCENE_TYPE_MISMATCH,
                format!(
                    "`physics` must be a mapping, found {} (line {})",
                    node.value.type_phrase(),
                    node.line
                ),
            )
            .with_path("/physics")
            .with_help("write `physics` as a mapping:\n  physics:\n    space: free"),
        );
        return None;
    };

    let mut space_seen = false;
    let mut space = None;
    let mut gravity = None;
    let mut solver_iterations = None;
    let mut stabilization_iterations = None;
    let mut sleep_after_ticks = None;
    let mut layers = Vec::new();
    let mut layers_ok = true;

    for entry in entries {
        match entry.key.as_str() {
            "space" => {
                space_seen = true;
                space = parse_space(entry, diagnostics);
            }
            "gravity" => {
                gravity = parse_gravity(entry, diagnostics);
            }
            "solver_iterations" => {
                solver_iterations = parse_iterations(entry, "solver_iterations", diagnostics);
            }
            "stabilization_iterations" => {
                stabilization_iterations =
                    parse_iterations(entry, "stabilization_iterations", diagnostics);
            }
            "sleep_after_ticks" => {
                sleep_after_ticks = parse_sleep_after_ticks(entry, diagnostics);
            }
            "layers" => match parse_layers(entry, diagnostics) {
                Some(parsed) => layers = parsed,
                None => layers_ok = false,
            },
            unknown => diagnostics.push(envelope::unknown_key(
                unknown,
                entry,
                "/physics",
                "the physics block",
                PHYSICS_KEYS,
                code::SCENE_UNKNOWN_KEY,
            )),
        }
    }

    if !space_seen {
        diagnostics.push(
            Diagnostic::new(
                code::SCENE_MISSING_KEY,
                "the `physics` block is missing the required key `space`",
            )
            .with_path("/physics")
            .with_help(
                "add `space: free` (3D) or `space: planar` (the engine's plane locks every rigid \
                 body)",
            ),
        );
    }

    let checked = check_layers(&layers, "/physics");
    if !checked.is_empty() {
        diagnostics.extend(checked);
        return None;
    }

    if !layers_ok {
        return None;
    }
    Some(ScenePhysics {
        space: space?,
        gravity,
        solver_iterations,
        stabilization_iterations,
        sleep_after_ticks,
        layers,
    })
}

/// Parses `physics.layers`: a sequence of mappings, each with a required string `name` and an
/// optional sequence of strings `ignores`. Structural errors only — the value checks run in
/// `check_layers` over the parsed list.
fn parse_layers(entry: &MapEntry, diagnostics: &mut Vec<Diagnostic>) -> Option<Vec<SceneLayer>> {
    let NodeValue::Sequence(items) = &entry.node.value else {
        diagnostics.push(
            Diagnostic::new(
                code::SCENE_TYPE_MISMATCH,
                format!(
                    "`physics.layers` must be a sequence of mappings, found {} (line {})",
                    entry.node.value.type_phrase(),
                    entry.node.line
                ),
            )
            .with_path("/physics/layers")
            .with_help(
                "write each layer as a mapping:\n  layers:\n    - name: dummy\n      ignores:\n        \
                 - dummy",
            ),
        );
        return None;
    };
    let mut out = Vec::with_capacity(items.len());
    let mut ok = true;
    for (index, item) in items.iter().enumerate() {
        match parse_layer(item, index, diagnostics) {
            Some(layer) => out.push(layer),
            None => ok = false,
        }
    }
    ok.then_some(out)
}

/// Parses one `physics.layers` entry.
fn parse_layer(item: &Node, index: usize, diagnostics: &mut Vec<Diagnostic>) -> Option<SceneLayer> {
    let base = format!("/physics/layers/{index}");
    let NodeValue::Mapping(entries) = &item.value else {
        diagnostics.push(
            Diagnostic::new(
                code::SCENE_TYPE_MISMATCH,
                format!(
                    "`physics.layers[{index}]` must be a mapping, found {} (line {})",
                    item.value.type_phrase(),
                    item.line
                ),
            )
            .with_path(base)
            .with_help("write the layer as `name: <layer>` with an optional `ignores` list"),
        );
        return None;
    };
    let mut name = None;
    let mut name_seen = false;
    let mut ignores = Vec::new();
    let mut ok = true;
    for entry in entries {
        match entry.key.as_str() {
            "name" => {
                name_seen = true;
                name = envelope::expect_string(
                    &entry.node,
                    &format!("{base}/name"),
                    &format!("`physics.layers[{index}].name`"),
                    code::SCENE_TYPE_MISMATCH,
                    diagnostics,
                );
            }
            "ignores" => match parse_ignores(entry, index, &base, diagnostics) {
                Some(parsed) => ignores = parsed,
                None => ok = false,
            },
            unknown => {
                diagnostics.push(envelope::unknown_key(
                    unknown,
                    entry,
                    &base,
                    "a physics layer",
                    LAYER_KEYS,
                    code::SCENE_UNKNOWN_KEY,
                ));
                ok = false;
            }
        }
    }
    if !name_seen {
        diagnostics.push(
            Diagnostic::new(
                code::SCENE_MISSING_KEY,
                format!("`physics.layers[{index}]` is missing the required key `name`"),
            )
            .with_path(base)
            .with_help("add `name: <layer>` — the spelling a collider's `layer` uses"),
        );
    }
    let name = name?;
    ok.then_some(SceneLayer { name, ignores })
}

/// Parses one layer's `ignores`: a sequence of strings.
fn parse_ignores(
    entry: &MapEntry,
    index: usize,
    base: &str,
    diagnostics: &mut Vec<Diagnostic>,
) -> Option<Vec<String>> {
    let pointer = format!("{base}/ignores");
    let NodeValue::Sequence(items) = &entry.node.value else {
        diagnostics.push(
            Diagnostic::new(
                code::SCENE_TYPE_MISMATCH,
                format!(
                    "`physics.layers[{index}].ignores` must be a sequence of layer names, found {} \
                     (line {})",
                    entry.node.value.type_phrase(),
                    entry.node.line
                ),
            )
            .with_path(pointer)
            .with_help("write the layers as a list:\n  ignores:\n    - sling"),
        );
        return None;
    };
    let mut out = Vec::with_capacity(items.len());
    let mut ok = true;
    for (j, item) in items.iter().enumerate() {
        match envelope::expect_string(
            item,
            &format!("{pointer}/{j}"),
            &format!("`physics.layers[{index}].ignores[{j}]`"),
            code::SCENE_TYPE_MISMATCH,
            diagnostics,
        ) {
            Some(name) => out.push(name),
            None => ok = false,
        }
    }
    ok.then_some(out)
}

/// Parses `physics.space`: `free` or `planar`, with a nearest-match suggestion for anything else.
fn parse_space(entry: &MapEntry, diagnostics: &mut Vec<Diagnostic>) -> Option<ScenePhysicsSpace> {
    let value = envelope::expect_string(
        &entry.node,
        "/physics/space",
        "`physics.space`",
        code::SCENE_TYPE_MISMATCH,
        diagnostics,
    )?;
    match value.as_str() {
        "free" => Some(ScenePhysicsSpace::Free),
        "planar" => Some(ScenePhysicsSpace::Planar),
        other => {
            let mut diagnostic = Diagnostic::new(
                code::SCENE_PHYSICS_INVALID,
                format!("`physics.space` must be `free` or `planar`, found `{other}`"),
            )
            .with_path("/physics/space");
            diagnostic = match suggest::nearest(other, ["free", "planar"]) {
                Some(candidate) => diagnostic.with_help(format!("nearest match is `{candidate}`")),
                None => diagnostic.with_help("valid values are `free` and `planar`"),
            };
            diagnostics.push(diagnostic);
            None
        }
    }
}

/// Parses `physics.gravity`: a sequence of exactly three numbers, each representable as `f32`.
///
/// A non-sequence or non-number element is `L1009` (mirroring [`expect_u64`]'s type-mismatch
/// convention); a wrong length or an element outside f32's finite range is `L1033` — a well-typed
/// value that is still not a legal one. Non-finite floats (`.inf`, `.nan`) never reach here: the
/// scalar resolver rejects them upstream as unrepresentable.
fn parse_gravity(entry: &MapEntry, diagnostics: &mut Vec<Diagnostic>) -> Option<[f64; 3]> {
    let NodeValue::Sequence(items) = &entry.node.value else {
        diagnostics.push(
            Diagnostic::new(
                code::SCENE_TYPE_MISMATCH,
                format!(
                    "`physics.gravity` must be a sequence of three numbers, found {} (line {})",
                    entry.node.value.type_phrase(),
                    entry.node.line
                ),
            )
            .with_path("/physics/gravity")
            .with_help(
                "write three numbers (x, y, z):\n  gravity:\n    - 0.0\n    - -9.81\n    - 0.0",
            ),
        );
        return None;
    };
    if items.len() != 3 {
        diagnostics.push(
            Diagnostic::new(
                code::SCENE_PHYSICS_INVALID,
                format!(
                    "`physics.gravity` must have exactly 3 components (x, y, z), found {}",
                    items.len()
                ),
            )
            .with_path("/physics/gravity")
            .with_help("supply exactly three numbers"),
        );
        return None;
    }
    let mut out = [0.0f64; 3];
    let mut ok = true;
    for (index, item) in items.iter().enumerate() {
        let pointer = format!("/physics/gravity/{index}");
        match &item.value {
            NodeValue::Scalar(Value::Number(n)) => {
                let value = crate::fields::number(n);
                match check_gravity_component(index, value, &pointer) {
                    None => out[index] = value,
                    Some(diagnostic) => {
                        diagnostics.push(diagnostic);
                        ok = false;
                    }
                }
            }
            other => {
                diagnostics.push(
                    Diagnostic::new(
                        code::SCENE_TYPE_MISMATCH,
                        format!(
                            "`physics.gravity[{index}]` must be a number, found {} (line {})",
                            other.type_phrase(),
                            item.line
                        ),
                    )
                    .with_path(pointer),
                );
                ok = false;
            }
        }
    }
    ok.then_some(out)
}

/// `L1033` at `pointer` if `value` falls outside [`ITERATIONS_RANGE`], naming `key`
/// (`solver_iterations` or `stabilization_iterations`) in the message. Shared by the parser
/// ([`parse_iterations`]) and [`validate_physics`], so a bad count reads identically whether it
/// came from a fresh scene or a restored snapshot.
fn check_iterations(key: &str, value: u64, pointer: &str) -> Option<Diagnostic> {
    if ITERATIONS_RANGE.contains(&value) {
        None
    } else {
        Some(
            Diagnostic::new(
                code::SCENE_PHYSICS_INVALID,
                format!(
                    "`physics.{key}` must be between {} and {}, found {value}",
                    ITERATIONS_RANGE.start(),
                    ITERATIONS_RANGE.end()
                ),
            )
            .with_path(pointer)
            .with_help(
                "the range is capped at 64 because a runaway iteration count stalls every host's \
                 tick",
            ),
        )
    }
}

/// `L1033` at `pointer` if `value` does not fit `f32`, naming `physics.gravity[index]`. Shared by
/// the parser ([`parse_gravity`]) and [`validate_physics`].
fn check_gravity_component(index: usize, value: f64, pointer: &str) -> Option<Diagnostic> {
    if ludion_core::number::fits_f32(value) {
        None
    } else {
        Some(
            Diagnostic::new(
                code::SCENE_PHYSICS_INVALID,
                format!("`physics.gravity[{index}]` does not fit in f32, found {value}"),
            )
            .with_path(pointer)
            .with_help("use a value within f32's finite range"),
        )
    }
}

/// Re-runs `check_iterations`, `check_gravity_component` and `check_layers` over an
/// already-typed [`ScenePhysics`] — the checks a fresh scene's `physics:` block gets from
/// `parse_physics`, applied again to a value that skipped the parser, such as a restored snapshot's
/// `scene_physics` field a hand-edited or foreign document could carry anything in. `base` is the
/// JSON Pointer prefix each diagnostic's path is written under: `/physics` from the parser's own
/// point of view, `/scene_physics` from restore's. Structural checks (a field of the wrong shape,
/// a wrong-length `gravity`) do not need re-running: the typed struct cannot represent them.
pub fn validate_physics(physics: &ScenePhysics, base: &str) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();
    if let Some(value) = physics.solver_iterations
        && let Some(diagnostic) = check_iterations(
            "solver_iterations",
            u64::from(value),
            &format!("{base}/solver_iterations"),
        )
    {
        diagnostics.push(diagnostic);
    }
    if let Some(value) = physics.stabilization_iterations
        && let Some(diagnostic) = check_iterations(
            "stabilization_iterations",
            u64::from(value),
            &format!("{base}/stabilization_iterations"),
        )
    {
        diagnostics.push(diagnostic);
    }
    if let Some(gravity) = physics.gravity {
        for (index, component) in gravity.into_iter().enumerate() {
            if let Some(diagnostic) =
                check_gravity_component(index, component, &format!("{base}/gravity/{index}"))
            {
                diagnostics.push(diagnostic);
            }
        }
    }
    diagnostics.extend(check_layers(&physics.layers, base));
    diagnostics
}

/// Parses `physics.solver_iterations`/`physics.stabilization_iterations`: an integer in
/// [`ITERATIONS_RANGE`].
fn parse_iterations(entry: &MapEntry, key: &str, diagnostics: &mut Vec<Diagnostic>) -> Option<u32> {
    let pointer = format!("/physics/{key}");
    let what = format!("`physics.{key}`");
    let value = expect_u64(&entry.node, &pointer, &what, diagnostics)?;
    match check_iterations(key, value, &pointer) {
        None => Some(value as u32),
        Some(diagnostic) => {
            diagnostics.push(diagnostic);
            None
        }
    }
}

/// Parses `physics.sleep_after_ticks`: an integer that fits in `u32`.
fn parse_sleep_after_ticks(entry: &MapEntry, diagnostics: &mut Vec<Diagnostic>) -> Option<u32> {
    let value = expect_u64(
        &entry.node,
        "/physics/sleep_after_ticks",
        "`physics.sleep_after_ticks`",
        diagnostics,
    )?;
    match u32::try_from(value) {
        Ok(ticks) => Some(ticks),
        Err(_) => {
            diagnostics.push(
                Diagnostic::new(
                    code::SCENE_PHYSICS_INVALID,
                    format!(
                        "`physics.sleep_after_ticks` must be at most {}, found {value}",
                        u32::MAX
                    ),
                )
                .with_path("/physics/sleep_after_ticks")
                .with_help(format!("use a value at most {}", u32::MAX)),
            );
            None
        }
    }
}
