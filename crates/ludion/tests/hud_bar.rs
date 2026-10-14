//! Behavioral proof for the Phase 14 screen-space UI facade wave (E3): a scripted HP-drain session
//! driving the `hud_bar` HUD pattern, verified two ways — the fill `UiRect`'s resolved `size[0]` (it
//! actually tracks health, and the "GAME OVER" label switches on exactly at zero) and a two-run
//! `WorldHash` trace (`UiRect`/`UiText` are ordinary hashed component state, so two identical runs
//! produce identical traces).
//!
//! The game here mirrors the `hud_bar` example: a `Health` state entity drained by a constant-rate
//! system, and `drive_hp_bar` scaling the fill `UiRect` + switching the `GameOverLabel` `UiText`.

use ludion::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, schemars::JsonSchema, Clone, Copy, Debug)]
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

#[derive(Serialize, Deserialize, schemars::JsonSchema, Clone, Copy, Debug, Default)]
struct HpFill {}

#[derive(Serialize, Deserialize, schemars::JsonSchema, Clone, Copy, Debug, Default)]
struct GameOverLabel {}

const MAX_FILL_WIDTH: f32 = 188.0;
const DAMAGE_PER_SECOND: f32 = 20.0;
const DT: f32 = 1.0 / 60.0;

fn drain_health(world: &mut World, ctx: &SystemCtx) {
    for (health,) in world.query::<(&mut Health,)>() {
        health.current = (health.current - DAMAGE_PER_SECOND * ctx.dt).max(0.0);
    }
}

fn drive_hp_bar(world: &mut World, _ctx: &SystemCtx) {
    let Some(fraction) = world
        .query::<(&Health,)>()
        .next()
        .map(|(health,)| (health.current / health.max).clamp(0.0, 1.0))
    else {
        return;
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

fn game() -> EngineBuilder {
    Engine::builder()
        .components(|reg| {
            reg.register::<Health>("Health")?;
            reg.register::<HpFill>("HpFill")?;
            reg.register::<GameOverLabel>("GameOverLabel")
        })
        .system("drain_health", drain_health)
        .system("drive_hp_bar", drive_hp_bar)
}

/// A bottom-left frame/fill pair and a `Center`-anchored label, plus the `Health` entity the systems
/// read/write — the same shape as the `hud_bar` example's scene.
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
    "        order: 1\n",
    "      HpFill: {}\n",
    "  - name: game-over-label\n",
    "    components:\n",
    "      UiText:\n",
    "        anchor: center\n",
    "        text: \"\"\n",
    "        size: 48.0\n",
    "        order: 0\n",
    "      GameOverLabel: {}\n",
);

fn engine_with_scene() -> Engine {
    let mut engine = game().build().expect("the game builds");
    engine
        .load_inline_scene(SCENE)
        .expect("the HUD scene loads");
    engine
}

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

/// The fill bar's width tracks `Health` deterministically tick over tick, and the "GAME OVER" label
/// switches on exactly when health reaches zero — not before, not after.
#[test]
fn fill_width_tracks_health_deterministically() {
    let mut engine = engine_with_scene();

    // Partway drained: 90 ticks at 20 HP/s off a max of 100 HP.
    for _ in 0..90 {
        step(&mut engine);
    }
    let (fill_width, label) = hud_state(&mut engine);
    let expected_fraction = (100.0 - DAMAGE_PER_SECOND * DT * 90.0) / 100.0;
    assert!(
        (fill_width - MAX_FILL_WIDTH * expected_fraction).abs() < 1e-3,
        "fill width {fill_width} should track the drained fraction {expected_fraction} \
         (expected {})",
        MAX_FILL_WIDTH * expected_fraction
    );
    assert_eq!(label, "", "health is still positive: no GAME OVER yet");

    // Fully drained: 100 HP / (20 HP/s) = 5s = 300 ticks total; step the remaining 210.
    for _ in 0..210 {
        step(&mut engine);
    }
    let (fill_width, label) = hud_state(&mut engine);
    assert_eq!(fill_width, 0.0, "a fully drained bar has zero fill width");
    assert_eq!(label, "GAME OVER", "zero health shows the GAME OVER label");
}

/// Two identical scripted runs produce identical per-frame `WorldHash` traces: the whole
/// drain-and-drive path — `Health` decay, the fill's `size[0]`, the label's text — is deterministic.
#[test]
fn two_runs_produce_identical_worldhash_traces() {
    let run = || {
        let mut engine = engine_with_scene();
        ludion_engine::drive_hashes(
            &mut engine,
            vec![Command::SimStep(command::SimStepParams {
                ticks: 120,
                hash: false,
            })],
        )
        .expect("the session drives")
    };
    assert_eq!(
        run(),
        run(),
        "the HUD drain-and-drive path must be deterministic"
    );
}
