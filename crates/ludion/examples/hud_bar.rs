//! A screen-space HP bar that tracks player health — the copyable HUD pattern (Phase 14's
//! screen-space UI wave).
//!
//! This is the runnable proof of the UI facade surface: a [`UiRect`] frame + fill pair (anchored
//! `bottom_left`, per the coordinate model in the engine guide's "Screen-space UI" chapter) and a
//! `Center`-anchored [`UiText`] label that switches on only once health reaches zero. The fill's
//! `size[0]` is driven every tick from a plain `Health` game component through ordinary typed world
//! access — the same shape as any other system — and both `UiRect`/`UiText` are hashed sim state, so
//! the bar's width is exactly as reproducible as a `Transform`.
//!
//! Everything above the `main` divider is copyable into a real game as-is. A real game drains
//! `Health` from combat or hazards; `drain_health` here is a deliberately trivial stand-in so the HUD
//! wiring stays the point.
//!
//! Run it headless (drains health over a few ticks and prints the resolved bar width + label):
//!
//! ```text
//! cargo run -p ludion --example hud_bar
//! ```
//!
//! To host it live for a viewer or a windowed play instead, `main`'s body would just be
//! `ludion::run(game())` — the same three-line shape every Ludion game's binary uses.

use ludion::prelude::*;
use std::process::ExitCode;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

// --- The game (copyable) -------------------------------------------------------------------------

/// The player's health. A real game drains this from combat/hazards; `drain_health` is the only
/// writer here.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Copy, Debug)]
#[serde(default)]
struct Health {
    current: f32,
    max: f32,
}

impl Default for Health {
    fn default() -> Self {
        Health {
            current: 100.0,
            max: 100.0,
        }
    }
}

/// Marks the `UiRect` whose `size[0]` tracks the health fraction — the bar's fill, layered over a
/// static frame `UiRect` by `order` (see the scene below).
#[derive(Serialize, Deserialize, JsonSchema, Clone, Copy, Debug, Default)]
struct HpFill {}

/// Marks the `Center`-anchored `UiText` that shows "GAME OVER" once health reaches zero, and is
/// empty — a legal, deliberate `UiText` state that draws nothing — otherwise.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Copy, Debug, Default)]
struct GameOverLabel {}

/// The fill `UiRect`'s width in canvas units at full health — must match `hp-fill`'s authored
/// `size[0]` in the scene below, since that is what `drive_hp_bar` scales down from.
const MAX_FILL_WIDTH: f32 = 188.0;

/// A constant drain, standing in for whatever really damages the player in a full game — keeps this
/// example self-contained without a combat system.
const DAMAGE_PER_SECOND: f32 = 20.0;

/// Drains `Health.current` toward zero at a fixed rate. Ordinary `ctx.dt`-scaled sim math — no wall
/// clock, deterministic like every other system.
fn drain_health(world: &mut World, ctx: &SystemCtx) {
    for (health,) in world.query::<(&mut Health,)>() {
        health.current = (health.current - DAMAGE_PER_SECOND * ctx.dt).max(0.0);
    }
}

/// Drives the HUD from `Health`: the fill `UiRect`'s `size[0]` tracks the health fraction, and the
/// `GameOverLabel` text switches on at zero health. Plain typed world access — the anchor/canvas math
/// lives entirely in `resolve_ui_rect` at extract time, not here.
fn drive_hp_bar(world: &mut World, _ctx: &SystemCtx) {
    let Some(fraction) = world
        .query::<(&Health,)>()
        .next()
        .map(|(health,)| (health.current / health.max).clamp(0.0, 1.0))
    else {
        return; // no Health entity this frame — nothing to drive
    };

    for (_marker, fill) in world.query::<(&HpFill, &mut UiRect)>() {
        fill.size[0] = MAX_FILL_WIDTH * fraction;
    }
    for (_marker, label) in world.query::<(&GameOverLabel, &mut UiText)>() {
        label.text = if fraction <= 0.0 {
            "GAME OVER".to_string()
        } else {
            String::new()
        };
    }
}

/// The game: a bare `Health` state entity plus the two HUD-driving systems, in order (drain first,
/// so the HUD reflects this tick's damage). A real game would point `.default_scene(...)` at an
/// on-disk file; `SCENE` is inlined below so this stays a single, copyable file.
pub fn game() -> EngineBuilder {
    Engine::builder()
        .components(|reg| {
            reg.register::<Health>("Health")?;
            reg.register::<HpFill>("HpFill")?;
            reg.register::<GameOverLabel>("GameOverLabel")
        })
        .system("drain_health", drain_health)
        .system("drive_hp_bar", drive_hp_bar)
}

/// The HUD: a bottom-left frame/fill pair (`fill.order` over `frame.order` keeps the fill drawn on
/// top of the frame) and a `Center`-anchored label — plus the bare `Health` entity `drive_hp_bar`
/// reads from.
const SCENE: &str = concat!(
    "format: 1\n",
    "entities:\n",
    "  - name: player\n",
    "    components:\n",
    "      Health:\n",
    "        current: 100.0\n",
    "        max: 100.0\n",
    "  - name: hp-frame\n",
    "    components:\n",
    "      UiRect:\n",
    "        anchor: bottom_left\n",
    "        offset:\n",
    "          - 24.0\n",
    "          - -24.0\n",
    "        size:\n",
    "          - 200.0\n",
    "          - 32.0\n",
    "        color:\n",
    "          - 0.05\n",
    "          - 0.05\n",
    "          - 0.06\n",
    "          - 1.0\n",
    "        order: 0\n",
    "  - name: hp-fill\n",
    "    components:\n",
    "      UiRect:\n",
    "        anchor: bottom_left\n",
    "        offset:\n",
    "          - 30.0\n",
    "          - -30.0\n",
    "        size:\n",
    "          - 188.0\n",
    "          - 20.0\n",
    "        color:\n",
    "          - 0.2\n",
    "          - 0.85\n",
    "          - 0.3\n",
    "          - 1.0\n",
    "        order: 1\n",
    "      HpFill: {}\n",
    "  - name: game-over-label\n",
    "    components:\n",
    "      UiText:\n",
    "        anchor: center\n",
    "        text: \"\"\n",
    "        size: 48.0\n",
    "        color:\n",
    "          - 0.95\n",
    "          - 0.2\n",
    "          - 0.2\n",
    "          - 1.0\n",
    "        order: 0\n",
    "      GameOverLabel: {}\n",
);

// --- Headless demo `main` ------------------------------------------------------------------------

/// Steps the game in one-second bursts, printing the resolved fill width and label after each — no
/// window, no wire client, deterministic. (A real game's `main` is just `ludion::run(game())`.)
fn main() -> ExitCode {
    let mut engine = match game().build() {
        Ok(engine) => engine,
        Err(diagnostic) => {
            eprintln!("{diagnostic}");
            return ExitCode::FAILURE;
        }
    };
    if let Err(diagnostics) = engine.load_inline_scene(SCENE) {
        for diagnostic in diagnostics {
            eprintln!("{diagnostic}");
        }
        return ExitCode::FAILURE;
    }

    // 100 HP at 20/s drains to 0 in 300 ticks (5s at the fixed 60Hz timestep); sample every second.
    for _ in 0..5 {
        for _ in 0..60 {
            step(&mut engine);
        }
        let (fill_width, label) = hud_state(&mut engine);
        println!("fill width {fill_width:6.1} / {MAX_FILL_WIDTH:.1}   label {label:?}");
    }

    ExitCode::SUCCESS
}

/// Advances one fixed tick.
fn step(engine: &mut Engine) {
    engine
        .execute(Command::SimStep(command::SimStepParams {
            ticks: 1,
            hash: false,
        }))
        .expect("the sim steps");
}

/// The fill `UiRect`'s current `size[0]` and the `GameOverLabel`'s current text.
fn hud_state(engine: &mut Engine) -> (f32, String) {
    let world = engine.world_mut();
    let fill_width = world
        .single::<HpFill>()
        .map(|e| world.get::<UiRect>(e).unwrap().unwrap().size[0])
        .expect("the scene has an HpFill entity");
    let label = world
        .single::<GameOverLabel>()
        .map(|e| world.get::<UiText>(e).unwrap().unwrap().text.clone())
        .expect("the scene has a GameOverLabel entity");
    (fill_width, label)
}
