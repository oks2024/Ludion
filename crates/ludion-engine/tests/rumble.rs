//! Gamepad rumble: the outbox drains at the frame boundary, validated and scaled, to an armed
//! channel, and is never sim state (`Plan/input-actions.md`, W4).

use ludion_core::codes::code;
use ludion_core::rumble::RumbleCommand;
use ludion_core::time::FrameIndex;
use ludion_ecs::{SystemCtx, World};
use ludion_engine::diagnostics::Diagnostics;
use ludion_engine::{Engine, EngineBuilder, Rumble, RumbleEffect};
use ludion_protocol::command::EngineRestoreParams;
use ludion_protocol::{Command, ResponseBody};
use serde_json::Value;

fn effect(low: f32, high: f32, duration: f32) -> RumbleEffect {
    RumbleEffect {
        low,
        high,
        duration,
    }
}

/// Plays on frame 1 only.
fn play_on_one(world: &mut World, ctx: &SystemCtx) {
    if ctx.frame == FrameIndex(1)
        && let Some(rumble) = world.resource_mut::<Rumble>()
    {
        rumble.play(effect(0.8, 0.3, 0.18));
    }
}

fn play_out_of_range(world: &mut World, ctx: &SystemCtx) {
    if ctx.frame == FrameIndex(0)
        && let Some(rumble) = world.resource_mut::<Rumble>()
    {
        rumble.play(effect(1.5, f32::NAN, 5.0));
    }
}

fn play_then_stop(world: &mut World, ctx: &SystemCtx) {
    if ctx.frame == FrameIndex(0)
        && let Some(rumble) = world.resource_mut::<Rumble>()
    {
        rumble.play(effect(1.0, 1.0, 1.0));
        rumble.stop_all();
    }
}

fn play_half(world: &mut World, ctx: &SystemCtx) {
    if ctx.frame == FrameIndex(0)
        && let Some(rumble) = world.resource_mut::<Rumble>()
    {
        rumble.play(effect(0.8, 0.4, 1.0));
    }
}

fn quiet(_world: &mut World, _ctx: &SystemCtx) {}

#[test]
fn a_play_arrives_once_after_its_tick_unchanged() {
    let (tx, rx) = crossbeam_channel::unbounded();
    let mut engine = Engine::builder()
        .system("play", play_on_one)
        .build()
        .unwrap();
    engine.set_rumble_sender(tx);
    engine.advance_live();
    assert_eq!(rx.try_iter().count(), 0);
    engine.advance_live();
    assert_eq!(
        rx.try_iter().collect::<Vec<_>>(),
        [RumbleCommand::Play {
            low: 0.8,
            high: 0.3,
            seconds: 0.18
        }]
    );
    engine.advance_live();
    assert_eq!(rx.try_iter().count(), 0);
}

#[test]
fn values_clamp_and_a_non_finite_one_reports_l4279() {
    let (tx, rx) = crossbeam_channel::unbounded();
    let mut engine = Engine::builder()
        .system("play", play_out_of_range)
        .build()
        .unwrap();
    engine.set_rumble_sender(tx);
    engine.advance_live();
    assert_eq!(
        rx.try_iter().collect::<Vec<_>>(),
        [RumbleCommand::Play {
            low: 1.0,
            high: 0.0,
            seconds: 2.0
        }]
    );
    let diagnostics = engine.world().resource::<Diagnostics>().unwrap();
    assert_eq!(diagnostics.reported(code::RUMBLE_EFFECT_INVALID).count(), 1);
}

#[test]
fn stop_all_after_a_play_sends_only_the_stop() {
    let (tx, rx) = crossbeam_channel::unbounded();
    let mut engine = Engine::builder()
        .system("play", play_then_stop)
        .build()
        .unwrap();
    engine.set_rumble_sender(tx);
    engine.advance_live();
    assert_eq!(rx.try_iter().collect::<Vec<_>>(), [RumbleCommand::StopAll]);
}

#[test]
fn intensity_scales_both_motors_and_zero_sends_nothing() {
    let (tx, rx) = crossbeam_channel::unbounded();
    let mut engine = Engine::builder().system("play", play_half).build().unwrap();
    engine.set_rumble_sender(tx.clone());
    engine.set_rumble_intensity(0.5);
    engine.advance_live();
    assert_eq!(
        rx.try_iter().collect::<Vec<_>>(),
        [RumbleCommand::Play {
            low: 0.4,
            high: 0.2,
            seconds: 1.0
        }]
    );

    let mut off = Engine::builder().system("play", play_half).build().unwrap();
    off.set_rumble_sender(tx);
    off.set_rumble_intensity(0.0);
    off.advance_live();
    assert_eq!(rx.try_iter().count(), 0);
    assert_eq!(off.rumble_intensity(), 0.0);
}

#[test]
fn without_a_sender_the_outbox_still_empties() {
    let mut engine = Engine::builder().system("play", play_half).build().unwrap();
    engine.advance_live();
    assert!(!engine.world().resource::<Rumble>().unwrap().is_pending());
}

fn trace(builder: EngineBuilder) -> Vec<u64> {
    let mut engine = builder.build().unwrap();
    (0..6)
        .map(|_| {
            engine.advance_live();
            engine.world_hash().unwrap().as_u64()
        })
        .collect()
}

#[test]
fn rumble_is_not_sim_state() {
    let with = trace(Engine::builder().system("play", play_on_one));
    let without = trace(Engine::builder().system("play", quiet));
    assert_eq!(with, without);
}

fn snapshot_of(engine: &mut Engine) -> Value {
    match engine.execute(Command::EngineSnapshot).unwrap().body {
        ResponseBody::Snapshot { snapshot } => snapshot,
        other => panic!("expected a snapshot, got {other:?}"),
    }
}

#[test]
fn a_restore_or_rebuild_leaves_an_empty_outbox() {
    let mut engine = Engine::builder().system("play", play_half).build().unwrap();
    let snapshot = snapshot_of(&mut engine);
    engine
        .world_mut()
        .resource_mut::<Rumble>()
        .unwrap()
        .play(effect(1.0, 1.0, 1.0));
    assert!(engine.world().resource::<Rumble>().unwrap().is_pending());
    match engine
        .execute(Command::EngineRestore(EngineRestoreParams { snapshot }))
        .unwrap()
        .body
    {
        ResponseBody::Restored { .. } => {}
        other => panic!("expected a restore, got {other:?}"),
    }
    assert!(!engine.world().resource::<Rumble>().unwrap().is_pending());

    engine
        .world_mut()
        .resource_mut::<Rumble>()
        .unwrap()
        .play(effect(1.0, 1.0, 1.0));
    engine
        .load_inline_scene("format: 1\nentities: []\n")
        .expect("scene loads");
    assert!(!engine.world().resource::<Rumble>().unwrap().is_pending());
}
