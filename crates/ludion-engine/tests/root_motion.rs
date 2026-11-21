//! Runtime proof for root motion on voxel assemblies (`Plan/animation-layers.md` decisions 55-60,
//! Neon Shroud request 0007 W6a): a clip's reserved `$root` track is read by `root_motion_delta`,
//! never drawn, never applied by the engine.
//!
//! - **Sums.** A one-shot, a loop over 2.5 cycles and a turn-then-walk clip sum to the track's
//!   authored displacement (times the entity's scale).
//! - **Crossfade.** The incoming side counts at the fade weight, the outgoing side at its complement.
//! - **Refused.** A blend name, an unknown clip and a non-assembly mesh read `None`.
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
    Animation, Engine, Mesh, QualityParams, RootMotion, Transform, extract_at, root_motion_delta,
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

/// The assembly with a clip per behaviour under test and a `stride` blend over two looped clips.
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
blends:
  stride:
    sync: foot_l
    points:
      - clip: stride_a
        at: 0.0
      - clip: stride_b
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

/// Everything the driver saw: one record per entity per tick, and the running sums.
#[derive(Default)]
struct Seen {
    records: Vec<(u64, bool, Option<RootMotion>)>,
    sum: [f64; 3],
    yaw: f64,
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
        if world.resource::<Seen>().is_none() {
            world.insert_resource(Seen::default());
        }
        let seen = world.resource_mut::<Seen>().expect("just inserted");
        seen.records
            .push((ctx.game_instant.ticks_floor(), ctx.game_advanced(), delta));
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
    let root = project(name, with_root);
    let mut builder = Engine::builder().project_root(&root);
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
fn a_blend_an_unknown_clip_and_a_non_assembly_mesh_read_none() {
    let mut entities = runner("blend", "stride", "");
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
