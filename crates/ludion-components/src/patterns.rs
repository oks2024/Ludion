//! The [`Volley`] and [`PatternPlayer`] components.

use ludion_core::codes::code;
use ludion_core::diagnostics::Diagnostic;
use ludion_core::ids::EntityId;
use ludion_core::time::FrameIndex;
use ludion_ecs::SemanticValidate;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// The most bullets one [`Volley`] may hold: the width of its `killed` bitset.
///
/// A volley past it cannot record its own kills, so the cap is a real bound rather than a
/// formality — `L1149` at document time, `L2108` on a component built in Rust.
pub const VOLLEY_MAX_BULLETS: u32 = 256;

/// How a volley lays its bullets out around its angle.
///
/// **Every variant carries a doc comment** for [`crate::EmitShape`]'s reason: it is what makes
/// `schemars` emit the `oneOf`-of-`const` shape a schema-driven form renders as a dropdown.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Copy, Debug, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum VolleyShape {
    /// `count` bullets evenly over the full turn, the first at the volley's angle.
    #[default]
    Ring,
    /// `count` bullets evenly over `spread_deg`, centred on the volley's angle (a single bullet
    /// sits at the centre).
    Arc,
    /// Each bullet's angle drawn uniformly inside the cone and its speed uniformly from the
    /// volley's range, from a stream named by the bullet's own index.
    Random,
}

/// One fired volley: everything bullet `i`'s trajectory, life and hit circle follow from
/// (`Plan/projectile-patterns.md` decision 1).
///
/// The entity carrying it is the volley; its `Transform`'s translation is the origin captured at
/// fire time and never moves afterward. `angle_deg` is already **absolute** — an aimed volley folded
/// its aim in when it fired — so nothing here needs to be re-resolved against the world, and the
/// same `(volley, now)` draws the same field in any process.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq)]
pub struct Volley {
    /// Who fired: the game reads the source's own components for side, damage and score. `None`
    /// when authored without one or saved after the shooter died.
    #[serde(default)]
    #[schemars(with = "Option<ludion_ecs::EntityRefSelfSchema>")]
    pub source: Option<EntityId>,
    /// The `DocStore` key of the `PatternDoc` holding [`bullet`](Volley::bullet).
    #[serde(default)]
    #[schemars(with = "ludion_ecs::AssetPathSchema")]
    pub pattern: String,
    /// Which of that document's `bullets:` these are.
    #[serde(default)]
    pub bullet: String,
    /// The frame the volley fired on; every bullet's age is measured from it.
    #[serde(default = "frame_zero")]
    #[schemars(with = "u64")]
    pub started: FrameIndex,
    /// How the bullets are laid out around [`angle_deg`](Volley::angle_deg).
    #[serde(default)]
    pub shape: VolleyShape,
    /// How many bullets, at most [`VOLLEY_MAX_BULLETS`] (`L2108`).
    #[serde(default)]
    pub count: u32,
    /// The volley's world angle in degrees: `0` along the plane's first axis, `90` along its second.
    #[serde(default)]
    pub angle_deg: f32,
    /// The cone width in degrees for [`VolleyShape::Arc`] and [`VolleyShape::Random`].
    #[serde(default)]
    pub spread_deg: f32,
    /// The resolved per-bullet-index speed range in units per second: bullet `i` of `count` fires at
    /// `lerp(speed[0], speed[1], i / (count − 1))`, a single bullet at `speed[0]`, and a
    /// [`VolleyShape::Random`] bullet uniformly inside it.
    #[serde(default)]
    pub speed: [f32; 2],
    /// Names this volley's random stream — only [`VolleyShape::Random`] draws from it.
    #[serde(default)]
    pub seed: u64,
    /// The kill bitset: bit `i` set means bullet `i` is gone. The game's hit systems set it
    /// ([`Volley::kill`]); the engine only reads it.
    #[serde(default)]
    pub killed: [u64; 4],
}

impl Default for Volley {
    fn default() -> Self {
        Volley {
            source: None,
            pattern: String::new(),
            bullet: String::new(),
            started: FrameIndex::ZERO,
            shape: VolleyShape::Ring,
            count: 0,
            angle_deg: 0.0,
            spread_deg: 0.0,
            speed: [0.0, 0.0],
            seed: 0,
            killed: [0; 4],
        }
    }
}

impl Volley {
    /// Whether bullet `index` has been killed. An index past [`VOLLEY_MAX_BULLETS`] reads as killed
    /// — there is no bit for it, so it can never be alive.
    pub fn is_killed(&self, index: u32) -> bool {
        if index >= VOLLEY_MAX_BULLETS {
            return true;
        }
        let (word, bit) = (index as usize / 64, index % 64);
        self.killed[word] & (1u64 << bit) != 0
    }

    /// Marks bullet `index` gone — what a game's hit system calls on a `volley_hits` result. Out
    /// of range is a no-op.
    pub fn kill(&mut self, index: u32) {
        if index >= VOLLEY_MAX_BULLETS {
            return;
        }
        let (word, bit) = (index as usize / 64, index % 64);
        self.killed[word] |= 1u64 << bit;
    }

    /// Whether every one of this volley's bullets carries a kill bit — the despawn condition
    /// `advance_volleys` checks before age.
    pub fn all_killed(&self) -> bool {
        (0..self.count.min(VOLLEY_MAX_BULLETS)).all(|index| self.is_killed(index))
    }
}

impl SemanticValidate for Volley {
    /// Rejects exactly what the schema cannot: a `count` past the kill bitset's width, and a
    /// non-finite angle, spread or speed — each of which would poison every bullet's position with
    /// a NaN rather than fail loudly.
    fn validate_semantics(value: &Value) -> Vec<Diagnostic> {
        let mut out = Vec::new();
        if let Some(count) = value.get("count").and_then(Value::as_u64)
            && count > u64::from(VOLLEY_MAX_BULLETS)
        {
            out.push(volley_invalid(
                format!(
                    "Volley count {count} is above the kill bitset's width of \
                     {VOLLEY_MAX_BULLETS}, so its highest bullets could never be killed"
                ),
                "/count",
                "fire several volleys of 256 or fewer instead",
            ));
        }
        for field in ["angle_deg", "spread_deg"] {
            if let Some(number) = value.get(field).and_then(crate::as_stored_f32)
                && !number.is_finite()
            {
                out.push(volley_invalid(
                    format!("Volley `{field}` {number} is not a finite number"),
                    &format!("/{field}"),
                    "give the field a finite value in degrees",
                ));
            }
        }
        for index in 0..2 {
            if let Some(number) = value
                .get("speed")
                .and_then(|v| v.get(index))
                .and_then(crate::as_stored_f32)
                && !number.is_finite()
            {
                out.push(volley_invalid(
                    format!("Volley speed[{index}] {number} is not a finite number"),
                    &format!("/speed/{index}"),
                    "give both ends of the range a finite speed in units per second",
                ));
            }
        }
        out
    }
}

/// Plays a `PatternDoc`'s timeline: `play_patterns` walks it from `started` every tick and
/// stands a [`Volley`] up on each entry's frame (`Plan/projectile-patterns.md` decision 5).
///
/// The entity's world-space translation is the origin every volley it fires captures, so a moving
/// turret lays its pattern down where it currently is. Re-stamping `started` restarts the pattern —
/// [`Animation`](crate::Animation)'s own warning.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq)]
pub struct PatternPlayer {
    /// The `DocStore` key of the `PatternDoc` to play (default `""` — plays nothing).
    #[serde(default)]
    #[schemars(with = "ludion_ecs::AssetPathSchema")]
    pub pattern: String,
    /// The frame playback started at; the timeline's `at` offsets are counted from it.
    #[serde(default = "frame_zero")]
    #[schemars(with = "u64")]
    pub started: FrameIndex,
    /// The entity an `arc { center: aim }` volley aims at, read at fire time. A missing or dead
    /// target degrades to the authored absolute angle with one `L2108`.
    #[serde(default)]
    #[schemars(with = "Option<ludion_ecs::EntityRefSchema>")]
    pub target: Option<EntityId>,
}

impl Default for PatternPlayer {
    fn default() -> Self {
        PatternPlayer {
            pattern: String::new(),
            started: FrameIndex::ZERO,
            target: None,
        }
    }
}

fn frame_zero() -> FrameIndex {
    FrameIndex::ZERO
}

/// `L2108` — a [`PatternPlayer`] or [`Volley`] cannot be evaluated as authored. A **warning**: the
/// sim degrades once per offender and keeps running.
fn volley_invalid(message: String, pointer: &str, help: &str) -> Diagnostic {
    Diagnostic::new(code::VOLLEY_INVALID, message)
        .with_path(pointer.to_string())
        .with_help(help.to_string())
}
