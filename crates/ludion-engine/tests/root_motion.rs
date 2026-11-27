//! Runtime proof for root motion on voxel assemblies (`Plan/animation-layers.md` decisions 55-62,
//! Neon Shroud request 0007 W6a and W6b-1): a clip's reserved `$root` track is read by
//! `root_motion_delta`, never drawn, never applied by the engine.
//!
//! - **Sums.** A one-shot, a loop over 2.5 cycles and a turn-then-walk clip sum to the track's
//!   authored displacement (times the entity's scale).
//! - **Crossfade.** The incoming side counts at the fade weight, the outgoing side at its complement.
//! - **Blend spaces.** A blend sums its `$root` points at their weights over the `BlendClock` phase
//!   window, on both sides of a fade, with or without `blend_clocks`.
//! - **Refused.** An unknown clip, a non-assembly mesh and a blend with no `$root` point read `None`.
//! - **Clock.** A held frame reads zero and a scaled or held clock still tiles to the full travel.
//! - **Deterministic.** Sums and hash traces match at 1 and 8 threads.
//! - **Inert.** An unconsumed `$root` clip moves neither the part matrices nor the world hash.
//!
//! The fixture is a one-cell assembly in a scratch directory.

use std::path::{Path, PathBuf};

use ludion_core::math::glam::Mat4;
use ludion_core::time::{GameClock, SimTime, TickRate};
use ludion_ecs::{SystemCtx, World};
use ludion_engine::{
    Animation, BlendClock, Engine, Mesh, QualityParams, RootMotion, Transform, blend_clocks,
    extract_at, root_motion_delta,
};
use ludion_protocol::Command;
use ludion_protocol::command::SimStepParams;

const ASSEMBLY_REL: &str = "models/body.assembly.yaml";

/// One keyed channel of a track: `(t, [x, y, z])`.
type Keys<'a> = &'a [(f64, [f64; 3])];

fn channel(name: &str, keys: Keys) -> String {
    let mut out = format!("    {name}:\n");
    for (t, v) in keys {
        out.push_str(&format!(
            "      - t: {t:?}\n        v:\n          - {:?}\n          - {:?}\n          - {:?}\n",
            v[0], v[1], v[2]
        ));
    }
    out
}

/// A clip document. `root` is the optional `$root` track (translation, rotation); the `body` track
/// turns the one part so the clip always draws something.
fn clip(duration: f64, looped: bool, root: Option<(Keys, Keys)>, events: bool) -> String {
    let events = if events {
        format!(
            "events:\n  - t: 0.0\n    name: foot_l\n  - t: {:?}\n    name: foot_r\n",
            duration * 0.5
        )
    } else {
        String::new()
    };
    let mut text = format!(
        "format: 1\nkind: animation\nduration: {duration:?}\nloop: {looped}\n{events}tracks:\n"
    );
    text.push_str("  body:\n");
    text.push_str(&channel("rotation", &[(0.0, [0.0; 3]), (duration, [0.0, 30.0, 0.0])])[..]);
    if let Some((translation, rotation)) = root {
        text.push_str("  \"$root\":\n");
        if !translation.is_empty() {
            text.push_str(&channel("translation", translation));
        }
        if !rotation.is_empty() {
            text.push_str(&channel("rotation", rotation));
        }
    }
    text
}

fn write(root: &Path, rel: &str, contents: &str) {
    let path = root.join(rel);
    std::fs::create_dir_all(path.parent().expect("a fixture path has a parent")).unwrap();
    std::fs::write(path, contents).unwrap();
}

const WALK_Z: Keys = &[(0.0, [0.0; 3]), (1.0, [0.0, 0.0, -8.0])];
const STEP_X: Keys = &[(0.0, [0.0; 3]), (1.0, [4.0, 0.0, 0.0])];

/// The assembly with a clip per behaviour under test and three 1D blends: `stride` over two looped
/// `$root` clips (points at 0 and 2), `mixed` over a `$root` clip and one without (0 and 2), and
/// `still` over two without.
fn project(name: &str, with_root: bool) -> PathBuf {
    let dir =
        std::env::temp_dir().join(format!("ludion-root-motion-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    fn walk(keys: Keys<'_>, with_root: bool) -> Option<(Keys<'_>, Keys<'_>)> {
        with_root.then_some((keys, &[]))
    }
    let r = |keys| walk(keys, with_root);
    write(
        &dir,
        ASSEMBLY_REL,
        "format: 1
kind: assembly
name: body
size:
  - 1
  - 1
  - 1
palette:
  A:
    color:
      - 1.0
      - 0.0
      - 0.0
      - 1.0
parts:
  - name: body
    file: parts/cell.part.yaml
    place:
      - 0
      - 0
      - 0
clips:
  idle: anims/idle.anim.yaml
  lunge: anims/lunge.anim.yaml
  loopwalk: anims/loopwalk.anim.yaml
  sidestep: anims/sidestep.anim.yaml
  turnwalk: anims/turnwalk.anim.yaml
  stride_a: anims/stride_a.anim.yaml
  stride_b: anims/stride_b.anim.yaml
  stride_c: anims/stride_c.anim.yaml
blends:
  stride:
    sync: foot_l
    points:
      - clip: stride_a
        at: 0.0
      - clip: stride_b
        at: 2.0
  mixed:
    points:
      - clip: stride_a
        at: 0.0
      - clip: stride_c
        at: 2.0
  still:
    points:
      - clip: idle
        at: 0.0
      - clip: stride_c
        at: 2.0
",
    );
    write(
        &dir,
        "models/parts/cell.part.yaml",
        "format: 1\nkind: part\nsize:\n  - 1\n  - 1\n  - 1\nslices:\n  -\n    - \"A\"\n",
    );
    let turn_rot: Keys = &[
        (0.0, [0.0; 3]),
        (0.5, [0.0, 90.0, 0.0]),
        (1.0, [0.0, 90.0, 0.0]),
    ];
    let turn_pos: Keys = &[(0.0, [0.0; 3]), (0.5, [0.0; 3]), (1.0, [0.0, 0.0, -8.0])];
    let clips = [
        ("idle", clip(1.0, true, None, false)),
        ("lunge", clip(1.0, false, r(WALK_Z), false)),
        (
            "loopwalk",
            clip(
                0.5,
                true,
                r(&[(0.0, [0.0; 3]), (0.5, [0.0, 0.0, -4.0])]),
                false,
            ),
        ),
        ("sidestep", clip(1.0, false, r(STEP_X), false)),
        (
            "turnwalk",
            clip(1.0, false, with_root.then_some((turn_pos, turn_rot)), false),
        ),
        ("stride_a", clip(1.0, true, r(WALK_Z), true)),
        (
            "stride_b",
            clip(
                0.5,
                true,
                r(&[(0.0, [0.0; 3]), (0.5, [0.0, 0.0, -4.0])]),
                true,
            ),
        ),
        ("stride_c", clip(1.0, true, None, true)),
    ];
    for (stem, text) in clips {
        write(&dir, &format!("models/anims/{stem}.anim.yaml"), &text);
    }
    dir
}

fn scene(entities: &str) -> String {
    format!("format: 1\nseed: 1\nentities:\n{entities}")
}

fn runner(name: &str, animation: &str, transform: &str) -> String {
    let origin = "        translation:
          - 0.0
          - 0.0
          - 0.0
";
    let transform = if transform.is_empty() {
        origin
    } else {
        transform
    };
    format!(
        "  - name: {name}
    components:
      Transform:
{transform}      Mesh:
        shape:
          assembly: {ASSEMBLY_REL}
      Animation:
        clip: {animation}
"
    )
}

/// A [`runner`] playing `blend` at the 1D parameter `param`.
fn blend_runner(name: &str, blend: &str, param: f64) -> String {
    format!(
        "{}        params:\n          - {param:?}\n          - 0.0\n",
        runner(name, blend, "")
    )
}

/// Everything the driver saw: one record per entity per tick, and the running sums.
#[derive(Default)]
struct Seen {
    records: Vec<(u64, bool, Option<RootMotion>)>,
    sum: [f64; 3],
    yaw: f64,
    /// The `BlendClock` last read beside a delta.
    clock: Option<BlendClock>,
}

type Driver = fn(&mut World, &SystemCtx);

/// The game's own consumer: applies the delta to `Transform` (translation, then yaw about Y).
fn consume(world: &mut World, ctx: &SystemCtx) {
    let ids: Vec<_> = world
        .query_ids::<(&Animation, &Mesh, &Transform)>()
        .map(|(id, _)| id)
        .collect();
    for id in ids {
        let delta = root_motion_delta(&*world, id, ctx);
        let clock = world
            .get_unrecorded::<BlendClock>(id)
            .ok()
            .flatten()
            .cloned();
        if world.resource::<Seen>().is_none() {
            world.insert_resource(Seen::default());
        }
        let seen = world.resource_mut::<Seen>().expect("just inserted");
        seen.records
            .push((ctx.game_instant.ticks_floor(), ctx.game_advanced(), delta));
        seen.clock = clock;
        let Some(rm) = delta else { continue };
        for axis in 0..3 {
            seen.sum[axis] += f64::from(rm.translation[axis]);
        }
        seen.yaw += f64::from(rm.yaw);
        let transform = world.get_mut::<Transform>(id).unwrap().unwrap();
        for axis in 0..3 {
            transform.translation[axis] += rm.translation[axis];
        }
        transform.rotation[1] += rm.yaw.to_degrees();
    }
}

fn build(name: &str, with_root: bool, text: &str, driver: Option<Driver>) -> (Engine, PathBuf) {
    build_scheduled(name, with_root, text, None, true, driver)
}

/// `writer` (an `Animation` writer), then `blend_clocks` when `clocked`, then `driver`: the order
/// a game registers them in.
fn build_scheduled(
    name: &str,
    with_root: bool,
    text: &str,
    writer: Option<Driver>,
    clocked: bool,
    driver: Option<Driver>,
) -> (Engine, PathBuf) {
    let root = project(name, with_root);
    let mut builder = Engine::builder().project_root(&root);
    if let Some(writer) = writer {
        builder = builder
            .system("writer", writer)
            .query::<(&mut Animation,)>();
    }
    if clocked {
        builder = builder
            .system("blend_clocks", blend_clocks)
            .query::<(&Animation, &Mesh)>()
            .query::<(&ludion_engine::Animation, &ludion_engine::PrefabInstance)>()
            .query::<(&mut BlendClock,)>();
    }
    if let Some(driver) = driver {
        builder = builder
            .system("driver", driver)
            .query::<(&mut Animation,)>()
            .query::<(&Animation, &Mesh, &mut Transform)>();
    }
    let mut engine = builder.build().expect("the game builds");
    engine.load_inline_scene(text).expect("the scene loads");
    (engine, root)
}

fn step(engine: &mut Engine, ticks: u64) {
    engine
        .execute(Command::SimStep(SimStepParams { ticks, hash: false }))
        .expect("steps");
}

fn seen(engine: &Engine) -> &Seen {
    engine.world().resource::<Seen>().expect("the driver ran")
}

fn close(a: f64, b: f64, what: &str) {
    assert!((a - b).abs() < 1e-4, "{what}: {a} vs {b}");
}

/// `consume` after a driver `before` that runs first.
fn with(world: &mut World, ctx: &SystemCtx, before: fn(&mut World, &SystemCtx)) {
    before(world, ctx);
    consume(world, ctx);
}

#[test]
fn a_one_shot_sums_to_the_authored_travel_times_the_scale() {
    let text = scene(&runner(
        "a",
        "lunge",
        "        scale:\n          - 2.0\n          - 1.0\n          - 3.0\n",
    ));
    let (mut engine, root) = build("one-shot", true, &text, Some(consume));
    step(&mut engine, 90);
    let s = seen(&engine);
    close(s.sum[0], 0.0, "x");
    close(s.sum[1], 0.0, "y is never driven");
    close(s.sum[2], -8.0 * 3.0, "z over the whole clip");
    close(s.yaw, 0.0, "no yaw authored");
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn a_loop_over_two_and_a_half_cycles_sums_whole_cycles_plus_the_half() {
    let (mut engine, root) = build(
        "loop",
        true,
        &scene(&runner("a", "loopwalk", "")),
        Some(consume),
    );
    step(&mut engine, 76);
    close(seen(&engine).sum[2], -4.0 * 2.5, "2.5 cycles of -4");
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn a_turn_then_walk_ends_where_the_analytic_path_does() {
    let (mut engine, root) = build(
        "turn",
        true,
        &scene(&runner("a", "turnwalk", "")),
        Some(consume),
    );
    step(&mut engine, 90);
    let s = seen(&engine);
    // The keys are in the frame at the clip's start, so the entity ends at the key difference.
    close(s.sum[0], 0.0, "x");
    close(s.sum[2], -8.0, "z");
    close(
        s.yaw,
        std::f64::consts::FRAC_PI_2,
        "the authored 90 degrees",
    );
    let _ = std::fs::remove_dir_all(root);
}

/// Crossfades into `into` at game tick 10.
fn fade_at_10(world: &mut World, ctx: &SystemCtx, into: &str) {
    if ctx.game_instant.ticks_floor() == 10 {
        for (_, (animation,)) in world.query_ids::<(&mut Animation,)>() {
            ludion_engine::crossfade(animation, into, 0.5, ctx.game_frame);
        }
    }
}

fn from_idle_into_lunge(world: &mut World, ctx: &SystemCtx) {
    with(world, ctx, |w, c| fade_at_10(w, c, "lunge"));
}

fn from_lunge_into_sidestep(world: &mut World, ctx: &SystemCtx) {
    with(world, ctx, |w, c| fade_at_10(w, c, "sidestep"));
}

#[test]
fn a_fade_from_a_clip_without_root_gives_the_fade_weight_times_the_incoming_travel() {
    let (mut engine, root) = build(
        "fade-in",
        true,
        &scene(&runner("a", "idle", "")),
        Some(from_idle_into_lunge),
    );
    step(&mut engine, 60);
    let mut checked = 0;
    for (tick, _, rm) in &seen(&engine).records {
        let k = tick.saturating_sub(10);
        if (1..30).contains(&k) {
            let rm = rm.expect("the incoming side carries $root");
            let w = k as f64 / 30.0;
            close(f64::from(rm.weight), w, "weight");
            close(
                f64::from(rm.translation[2]),
                -8.0 / 60.0 * w,
                "delta is w times the incoming",
            );
            checked += 1;
        } else if *tick <= 10 {
            assert!(rm.is_none(), "no root side before the fade at tick {tick}");
        }
    }
    assert_eq!(checked, 29);
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn a_fade_between_two_root_clips_blends_both_sides() {
    let (mut engine, root) = build(
        "fade-both",
        true,
        &scene(&runner("a", "lunge", "")),
        Some(from_lunge_into_sidestep),
    );
    step(&mut engine, 60);
    for (tick, _, rm) in &seen(&engine).records {
        let k = tick.saturating_sub(10);
        if (1..30).contains(&k) {
            let rm = rm.expect("both sides carry $root");
            let w = k as f64 / 30.0;
            close(f64::from(rm.weight), 1.0, "both sides contribute");
            close(f64::from(rm.translation[0]), 4.0 / 60.0 * w, "incoming x");
            close(
                f64::from(rm.translation[2]),
                -8.0 / 60.0 * (1.0 - w),
                "outgoing z",
            );
        }
    }
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn a_blend_with_root_points_reads_some() {
    let (mut engine, root) = build(
        "blend-some",
        true,
        &scene(&runner("a", "stride", "")),
        Some(consume),
    );
    step(&mut engine, 10);
    let s = seen(&engine);
    assert_eq!(s.records.len(), 10);
    for (tick, _, rm) in &s.records {
        let rm = rm.unwrap_or_else(|| panic!("tick {tick} read None"));
        assert_eq!(rm.weight, 1.0);
    }
    assert!(s.sum[2] < -1.0, "the blend travels: {:?}", s.sum);
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn an_unknown_clip_a_non_assembly_mesh_and_a_blend_without_root_read_none() {
    let mut entities = runner("still", "still", "");
    entities.push_str(&runner("unknown", "nope", ""));
    entities.push_str("  - name: cube\n    components:\n      Transform:\n        translation:\n          - 0.0\n          - 0.0\n          - 0.0\n      Mesh:\n        shape: cube\n      Animation:\n        clip: lunge\n");
    let (mut engine, root) = build("none", true, &scene(&entities), Some(consume));
    step(&mut engine, 10);
    let s = seen(&engine);
    assert_eq!(s.records.len(), 30, "every entity is read every tick");
    assert!(
        s.records.iter().all(|(_, _, rm)| rm.is_none()),
        "{:?}",
        s.records
    );
    let _ = std::fs::remove_dir_all(root);
}

fn blend_clock(engine: &Engine) -> &BlendClock {
    seen(engine)
        .clock
        .as_ref()
        .expect("the entity carries a BlendClock")
}

/// `stride`'s travel per shared cycle at `param`: 8 and 4 voxels toward `-Z`, mixed linearly.
fn stride_cycle(param: f64) -> f64 {
    let b = param / 2.0;
    (1.0 - b) * -8.0 + b * -4.0
}

#[test]
fn a_blend_on_one_point_sums_that_clips_travel() {
    // Two seconds are 2 cycles of `stride_a` (8 each) or 4 of `stride_b` (4 each).
    for (name, param) in [("point-a", 0.0), ("point-b", 2.0)] {
        let text = scene(&blend_runner("a", "stride", param));
        let (mut engine, root) = build(name, true, &text, Some(consume));
        step(&mut engine, 121);
        let z = seen(&engine).sum[2];
        // `blend_clocks` integrates the `f32` step, 5e-8 off a sixtieth.
        assert!((z - -16.0).abs() < 2e-6, "{name}: {z}");
        let _ = std::fs::remove_dir_all(root);
    }
}

#[test]
fn a_blend_between_points_sums_weight_times_phase_times_cycle_travel() {
    let text = scene(&blend_runner("a", "stride", 0.5));
    let (mut engine, root) = build("mid", true, &text, Some(consume));
    step(&mut engine, 121);
    // The clock started at phase 0.
    let delta_phase = blend_clock(&engine).phase;
    assert!((delta_phase - 2.5).abs() < 1e-5, "{delta_phase}");
    let expected = delta_phase * (0.75 * -8.0 + 0.25 * -4.0);
    let z = seen(&engine).sum[2];
    assert!((z - expected).abs() < 1e-6, "{z} vs {expected}");
    let _ = std::fs::remove_dir_all(root);
}

/// The `(prev, phase, params[0])` of every tick's clock, as the root system finds it.
#[derive(Default)]
struct Windows(Vec<(f64, f64, f32)>);

fn record_windows_then_consume(world: &mut World, ctx: &SystemCtx) {
    let ids: Vec<_> = world
        .query_ids::<(&Animation, &Mesh, &Transform)>()
        .map(|(id, (animation, _, _))| (id, animation.params[0]))
        .collect();
    let read: Vec<_> = ids
        .into_iter()
        .filter_map(|(id, param)| {
            let clock = world.get_unrecorded::<BlendClock>(id).ok().flatten()?;
            Some((clock.prev, clock.phase, param))
        })
        .collect();
    if world.resource::<Windows>().is_none() {
        world.insert_resource(Windows::default());
    }
    world
        .resource_mut::<Windows>()
        .expect("just inserted")
        .0
        .extend(read);
    consume(world, ctx);
}

fn change_params(world: &mut World, ctx: &SystemCtx) {
    let param = match ctx.game_instant.ticks_floor() {
        30 => 2.0,
        70 => 0.7,
        _ => return,
    };
    for (_, (animation,)) in world.query_ids::<(&mut Animation,)>() {
        animation.params[0] = param;
    }
}

#[test]
fn a_params_change_mid_run_tiles() {
    let text = scene(&blend_runner("a", "stride", 0.0));
    let (mut engine, root) = build_scheduled(
        "params",
        true,
        &text,
        Some(change_params),
        true,
        Some(record_windows_then_consume),
    );
    step(&mut engine, 100);
    let windows = &engine.world().resource::<Windows>().expect("recorded").0;
    assert_eq!(windows.len(), 100);
    let (mut previous, mut expected) = (0.0, 0.0);
    for &(prev, phase, param) in windows.iter().skip(1) {
        assert_eq!(prev, previous, "each window opens where the last closed");
        expected += (phase - prev) * stride_cycle(f64::from(param));
        previous = phase;
    }
    let z = seen(&engine).sum[2];
    assert!((z - expected).abs() < 1e-5, "{z} vs {expected}");
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn a_partly_rooted_blend_weighs_only_its_root_points() {
    let text = scene(&blend_runner("a", "mixed", 0.5));
    let (mut engine, root) = build("mixed", true, &text, Some(consume));
    step(&mut engine, 61);
    let s = seen(&engine);
    for (tick, _, rm) in &s.records {
        let rm = rm.expect("one point carries $root");
        assert_eq!(rm.weight, 0.75, "tick {tick}");
    }
    // Only `stride_a` travels: its 8 voxels a cycle at weight 0.75.
    let expected = 0.75 * -8.0 * blend_clock(&engine).phase;
    close(s.sum[2], expected, "z");
    let _ = std::fs::remove_dir_all(root);
}

fn idle_into_stride(world: &mut World, ctx: &SystemCtx) {
    fade_at_10(world, ctx, "stride");
}

fn stride_into_sidestep(world: &mut World, ctx: &SystemCtx) {
    fade_at_10(world, ctx, "sidestep");
}

fn stride_into_idle(world: &mut World, ctx: &SystemCtx) {
    fade_at_10(world, ctx, "idle");
}

#[test]
fn a_fade_from_a_clip_into_a_blend_weighs_the_incoming_share() {
    let (mut engine, root) = build_scheduled(
        "clip-into-blend",
        true,
        &scene(&runner("a", "idle", "")),
        Some(idle_into_stride),
        true,
        Some(consume),
    );
    step(&mut engine, 60);
    let mut checked = 0;
    for (tick, _, rm) in &seen(&engine).records {
        let k = tick.saturating_sub(10);
        if (1..30).contains(&k) {
            let rm = rm.expect("the incoming blend carries $root");
            let w = k as f64 / 30.0;
            close(f64::from(rm.weight), w, "weight");
            close(
                f64::from(rm.translation[2]),
                -8.0 / 60.0 * w,
                "delta is w times the blend",
            );
            checked += 1;
        } else if *tick <= 10 {
            assert!(rm.is_none(), "no root side before the fade at tick {tick}");
        }
    }
    assert_eq!(checked, 29);
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn a_fade_from_a_blend_moves_the_outgoing_side_on_its_carried_phase() {
    let (mut engine, root) = build_scheduled(
        "blend-into-clip",
        true,
        &scene(&runner("a", "stride", "")),
        Some(stride_into_sidestep),
        true,
        Some(consume),
    );
    step(&mut engine, 60);
    let mut checked = 0;
    for (tick, _, rm) in &seen(&engine).records {
        let k = tick.saturating_sub(10);
        if *tick == 10 {
            let rm = rm.expect("the outgoing side");
            assert_eq!(rm.weight, 1.0);
            assert_eq!(rm.translation, [0.0; 3], "the carried phase does not move");
        } else if (1..30).contains(&k) {
            let rm = rm.expect("both sides carry $root");
            let w = k as f64 / 30.0;
            close(f64::from(rm.weight), 1.0, "the two shares sum to 1");
            close(f64::from(rm.translation[0]), 4.0 / 60.0 * w, "incoming x");
            close(
                f64::from(rm.translation[2]),
                -8.0 / 60.0 * (1.0 - w),
                "outgoing z",
            );
            checked += 1;
        }
    }
    assert_eq!(checked, 29);
    let _ = std::fs::remove_dir_all(root);

    let (mut engine, root) = build_scheduled(
        "blend-into-idle",
        true,
        &scene(&runner("a", "stride", "")),
        Some(stride_into_idle),
        true,
        Some(consume),
    );
    step(&mut engine, 60);
    for (tick, _, rm) in &seen(&engine).records {
        let k = tick.saturating_sub(10);
        if (1..30).contains(&k) {
            let rm = rm.expect("the outgoing side carries $root");
            close(f64::from(rm.weight), 1.0 - k as f64 / 30.0, "weight");
        } else if *tick > 40 {
            assert!(rm.is_none(), "the fade is over at tick {tick}");
        }
    }
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn without_blend_clocks_the_stateless_window_still_tiles() {
    let text = scene(&blend_runner("a", "stride", 1.0));
    let (mut engine, root) = build_scheduled("stateless", true, &text, None, false, Some(consume));
    step(&mut engine, 121);
    assert!(seen(&engine).clock.is_none());
    // At 1.0 the shared phase runs 1.5 cycles a second at 6 voxels each.
    close(seen(&engine).sum[2], -18.0, "two seconds of the blend");
    let _ = std::fs::remove_dir_all(root);
}

fn hold_20_to_29_then_slow(world: &mut World, ctx: &SystemCtx) {
    let raw = ctx.frame.as_u64();
    let clock = world.resource_mut::<GameClock>().unwrap();
    if (20..30).contains(&raw) {
        clock.hold();
    }
    if raw == 40 {
        clock.scale(0.5);
    }
    consume(world, ctx);
}

#[test]
fn a_held_frame_reads_zero_and_a_scaled_clock_still_tiles() {
    let (mut engine, root) = build(
        "clock",
        true,
        &scene(&runner("a", "lunge", "")),
        Some(hold_20_to_29_then_slow),
    );
    step(&mut engine, 400);
    let s = seen(&engine);
    let held: Vec<_> = s
        .records
        .iter()
        .filter(|(_, advanced, _)| !advanced)
        .collect();
    assert!(!held.is_empty(), "the hold held some frames");
    for (_, _, rm) in held {
        if let Some(rm) = rm {
            assert_eq!(rm.translation, [0.0; 3], "a held frame moves nothing");
        }
    }
    close(
        s.sum[2],
        -8.0,
        "held and slowed, the clip still travels its full distance once",
    );
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn a_held_blend_reads_zero_and_a_scaled_clock_still_tiles() {
    let (mut engine, root) = build(
        "blend-clock",
        true,
        &scene(&runner("a", "stride", "")),
        Some(hold_20_to_29_then_slow),
    );
    step(&mut engine, 200);
    let s = seen(&engine);
    let held: Vec<_> = s
        .records
        .iter()
        .filter(|(_, advanced, _)| !advanced)
        .collect();
    assert!(!held.is_empty(), "the hold held some frames");
    for (_, _, rm) in held {
        let rm = rm.expect("a held blend still carries $root");
        assert_eq!(rm.translation, [0.0; 3], "a held frame moves nothing");
    }
    close(
        s.sum[2],
        -8.0 * blend_clock(&engine).phase,
        "the sum follows the phase",
    );
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn blend_sums_and_hash_traces_match_at_1_and_8_threads() {
    let mut entities = String::new();
    for i in 0..40 {
        let blend = ["stride", "mixed"][i % 2];
        entities.push_str(&blend_runner(&format!("m{i}"), blend, i as f64 / 20.0));
    }
    let text = scene(&entities);
    let runs: Vec<_> = [1usize, 8]
        .into_iter()
        .map(|n| {
            ludion_core::par::with_threads(n, || {
                let (mut engine, root) =
                    build(&format!("blend-threads{n}"), true, &text, Some(consume));
                let trace = trace_of(&mut engine, 30);
                let out = (trace, bits(seen(&engine)));
                let _ = std::fs::remove_dir_all(root);
                out
            })
        })
        .collect();
    assert!(runs[0].1.iter().all(|(_, _, rm)| rm.is_some()));
    assert_eq!(runs[0], runs[1], "1 vs 8 threads");
}

fn many(count: usize) -> String {
    let mut entities = String::new();
    for i in 0..count {
        let clip = ["lunge", "loopwalk", "turnwalk", "sidestep"][i % 4];
        entities.push_str(&runner(&format!("m{i}"), clip, ""));
    }
    scene(&entities)
}

fn bits(seen: &Seen) -> Vec<(u64, bool, Option<[u32; 5]>)> {
    seen.records
        .iter()
        .map(|&(t, adv, rm)| {
            (
                t,
                adv,
                rm.map(|r| {
                    [
                        r.translation[0].to_bits(),
                        r.translation[1].to_bits(),
                        r.translation[2].to_bits(),
                        r.yaw.to_bits(),
                        r.weight.to_bits(),
                    ]
                }),
            )
        })
        .collect()
}

fn trace_of(
    engine: &mut Engine,
    ticks: u64,
) -> Vec<(ludion_core::time::FrameIndex, ludion_sim::WorldHash)> {
    let mut trace = Vec::new();
    for _ in 0..ticks {
        let one = Command::SimStep(SimStepParams {
            ticks: 1,
            hash: false,
        });
        trace.extend(
            ludion_engine::drive_hashes(engine, vec![one])
                .expect("the session drives")
                .into_iter()
                .skip(1),
        );
    }
    trace
}

#[test]
fn sums_and_hash_traces_match_at_1_and_8_threads() {
    let runs: Vec<_> = [1usize, 8]
        .into_iter()
        .map(|n| {
            ludion_core::par::with_threads(n, || {
                let (mut engine, root) =
                    build(&format!("threads{n}"), true, &many(40), Some(consume));
                let trace = trace_of(&mut engine, 30);
                let out = (trace, bits(seen(&engine)));
                let _ = std::fs::remove_dir_all(root);
                out
            })
        })
        .collect();
    assert_eq!(runs[0], runs[1], "1 vs 8 threads");
}

fn matrices_at(engine: &Engine, tick: u64) -> Vec<[u32; 16]> {
    let extracted = extract_at(
        engine.world(),
        (320, 180),
        ludion_assets::AssetRoot::content(engine.project_root()),
        SimTime::from_ticks(tick, TickRate::DEFAULT),
        QualityParams::FULL.particle_density,
    )
    .expect("the fixture world extracts");
    extracted
        .meshes
        .iter()
        .map(|mesh| Mat4::to_cols_array(&mesh.model).map(f32::to_bits))
        .collect()
}

#[test]
fn an_unconsumed_root_track_moves_neither_the_matrices_nor_the_hash() {
    let text = scene(&runner("a", "turnwalk", ""));
    let run = |name: &str, with_root: bool| {
        let (mut engine, root) = build(name, with_root, &text, None);
        let trace = trace_of(&mut engine, 45);
        let matrices = matrices_at(&engine, 45);
        let _ = std::fs::remove_dir_all(root);
        (trace, matrices)
    };
    let (with_root, without) = (run("inert-with", true), run("inert-without", false));
    assert_eq!(with_root.0.len(), 45);
    assert_eq!(with_root.0, without.0, "the world hash moved");
    assert_eq!(with_root.1, without.1, "the drawn matrices moved");
}
