//! Runtime proof for masks, layers and blends on prefab parts (`Plan/animation-layers.md`, A3a): an
//! `AnimationLayers` on an instance's root poses its parts the way it poses an assembly's, over
//! each clip sample's delta from the part's rest.
//!
//! The fixture is `examples/gallery/prefabs/arm` (`base` ← `upper` ← `lower` ← `hand`, `lower`
//! resting turned 15° about +Z, mask `forearm` = `lower` and its descendants, blend `sway` =
//! `swing` at 0 and `reach` at 1), copied into a temp project with a few constant clips added.

use std::path::{Path, PathBuf};

use ludion_core::ids::EntityId;
use ludion_core::math::glam::{Mat4, Vec3};
use ludion_core::time::{SimTime, TickRate};
use ludion_engine::{
    Animation, AnimationEvents, BlendClock, ClipEvent, Engine, Mesh, PrefabInstance, QualityParams,
    Sprite, animation_events, blend_clocks, extract_at,
};
use ludion_protocol::command::{EngineRestoreParams, SimStepParams};
use ludion_protocol::{Command, ResponseBody};
use ludion_renderer::MeshInstance;

const PARTS: usize = 4;
const UPPER: usize = 1;
const LOWER: usize = 2;
const HAND: usize = 3;

/// A constant looping clip posing each `(part, rotation about Z, translation)`.
fn constant_clip(name: &str, tracks: &[(&str, f32, Option<[f32; 3]>)]) -> String {
    let mut out =
        format!("format: 1\nkind: animation\nname: {name}\nduration: 1.0\nloop: true\ntracks:\n");
    for (part, z, translation) in tracks {
        out.push_str(&format!(
            "  {part}:\n    rotation:\n      - t: 0.0\n        v:\n          - 0.0\n          - 0.0\n          - {z:?}\n"
        ));
        if let Some([x, y, z]) = translation {
            out.push_str(&format!(
                "    translation:\n      - t: 0.0\n        v:\n          - {x:?}\n          - {y:?}\n          - {z:?}\n"
            ));
        }
    }
    out
}

/// A fresh project carrying the arm under `arm/`, with the extra clips bound. `strip` removes the
/// prefab's `masks:` and `blends:`.
fn fixture_project(name: &str, strip: bool) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "ludion-prefab-layers-{}-{name}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    let source = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/gallery/prefabs/arm");
    let clips = dir.join("arm/clips");
    std::fs::create_dir_all(&clips).unwrap();
    for file in ["curl", "reach", "swing", "tilt"] {
        let file = format!("{file}.anim.yaml");
        std::fs::copy(source.join("clips").join(&file), clips.join(&file)).unwrap();
    }
    // `lower` rests at 15° and [0.1, 1.1, 0]: `twenty` is rest + 20° and +0.2 y, `ten` is rest +
    // 10° and +0.5 x, `still` is the rest itself.
    let extra = [
        (
            "twenty",
            constant_clip("twenty", &[("lower", 35.0, Some([0.1, 1.3, 0.0]))]),
        ),
        (
            "ten",
            constant_clip("ten", &[("lower", 25.0, Some([0.6, 1.1, 0.0]))]),
        ),
        (
            "still",
            constant_clip("still", &[("lower", 15.0, Some([0.1, 1.1, 0.0]))]),
        ),
        (
            "whole",
            constant_clip(
                "whole",
                &[
                    ("upper", 25.0, None),
                    ("lower", 50.0, None),
                    ("hand", 10.0, None),
                ],
            ),
        ),
    ];
    let mut bindings = String::from("clips:\n");
    for (clip, text) in &extra {
        std::fs::write(clips.join(format!("{clip}.anim.yaml")), text).unwrap();
        bindings.push_str(&format!("  {clip}: clips/{clip}.anim.yaml\n"));
    }
    let mut prefab = std::fs::read_to_string(source.join("arm.prefab.yaml"))
        .unwrap()
        .replace("\r\n", "\n")
        .replacen("clips:\n", &bindings, 1);
    if strip {
        let from = prefab.find("masks:\n").unwrap();
        let to = prefab.find("entities:\n").unwrap();
        prefab.replace_range(from..to, "");
    }
    std::fs::write(dir.join("arm/arm.prefab.yaml"), prefab).unwrap();
    dir
}

/// One layer entry: `clip` plus any further `key: value` fields.
fn layer(name: &str, clip: &str, rest: &[&str]) -> String {
    let mut out = format!(
        "            - name: {name}\n              clip: {clip}\n              weight: 1.0\n"
    );
    for field in rest {
        out.push_str(&format!("              {field}\n"));
    }
    out
}

/// One arm instance: its base `Animation.clip` (none for no `Animation`), further `Animation`
/// lines, and its layers.
fn instance(name: &str, clip: &str, animation: &[&str], layers: &[String]) -> String {
    let mut out = format!("  - name: {name}\n    prefab: arm/arm.prefab.yaml\n");
    if clip.is_empty() && layers.is_empty() {
        return out;
    }
    out.push_str("    overrides:\n      base:\n");
    if !clip.is_empty() {
        out.push_str(&format!("        Animation:\n          clip: {clip}\n"));
        for line in animation {
            out.push_str(&format!("          {line}\n"));
        }
    }
    if !layers.is_empty() {
        out.push_str("        AnimationLayers:\n          layers:\n");
        out.push_str(&layers.concat());
    }
    out
}

fn scene(instances: &[String]) -> String {
    format!("format: 1\nseed: 1\nentities:\n{}", instances.concat())
}

fn engine_with(root: &Path, scene: &str) -> Engine {
    let mut engine = Engine::with_project_root(root);
    engine.load_inline_scene(scene).expect("the scene loads");
    engine
}

fn posed_at(engine: &Engine, tick: u64) -> Vec<MeshInstance> {
    extract_at(
        engine.world(),
        (320, 180),
        ludion_assets::AssetRoot::content(engine.project_root()),
        SimTime::from_ticks(tick, TickRate::DEFAULT),
        QualityParams::FULL.particle_density,
    )
    .expect("the fixture world extracts")
    .meshes
}

/// `part`'s matrix of instance `index`, relative to its parent part.
fn local(posed: &[MeshInstance], index: usize, part: usize) -> Mat4 {
    let first = index * PARTS;
    ludion_core::math::mat4_inverse(posed[first + part - 1].model) * posed[first + part].model
}

fn assert_same_local(posed: &[MeshInstance], a: usize, b: usize, part: usize, what: &str) {
    let (x, y) = (local(posed, a, part), local(posed, b, part));
    assert!(
        x.abs_diff_eq(y, 1e-5),
        "{what}, part {part}: {x:?} vs {y:?}"
    );
}

/// `part`'s local turn about +Z in degrees, and its local translation.
fn turn_and_offset(posed: &[MeshInstance], index: usize, part: usize) -> (f32, Vec3) {
    let m = local(posed, index, part);
    (
        ludion_core::math::atan2(m.x_axis.y, m.x_axis.x).to_degrees(),
        m.w_axis.truncate(),
    )
}

#[test]
fn declared_masks_and_blends_leave_a_plain_clip_bit_identical() {
    let text = scene(&[
        instance("plain", "swing", &[], &[]),
        instance("fading", "curl", &["from: swing", "fade: 1.0"], &[]),
    ]);
    let (keyed, stripped) = (
        fixture_project("keyed", false),
        fixture_project("stripped", true),
    );
    let (a, b) = (engine_with(&keyed, &text), engine_with(&stripped, &text));
    for tick in [0, 18, 30, 75] {
        for (x, y) in posed_at(&a, tick).iter().zip(&posed_at(&b, tick)) {
            assert_eq!(x.model, y.model, "tick {tick}");
            assert_eq!(x.normal_matrix, y.normal_matrix, "tick {tick}");
        }
    }
    let _ = std::fs::remove_dir_all(&keyed);
    let _ = std::fs::remove_dir_all(&stripped);
}

#[test]
fn an_override_layer_takes_the_masked_parts_only() {
    let root = fixture_project("override", false);
    let text = scene(&[
        instance(
            "layered",
            "swing",
            &[],
            &[layer("arm", "whole", &["mask: forearm"])],
        ),
        instance("layer_as_base", "whole", &[], &[]),
        instance("base_only", "swing", &[], &[]),
    ]);
    let posed = posed_at(&engine_with(&root, &text), 18);
    for part in [LOWER, HAND] {
        assert_same_local(&posed, 0, 1, part, "a masked part plays the layer");
    }
    assert_same_local(&posed, 0, 2, UPPER, "an unmasked part keeps the base");
    assert!(
        !local(&posed, 0, UPPER).abs_diff_eq(local(&posed, 1, UPPER), 1e-3),
        "the layer clip does turn `upper`"
    );
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn an_additive_layer_adds_its_delta_from_the_parts_rest() {
    let root = fixture_project("additive", false);
    let additive = |clip: &str| layer("add", clip, &["mode: additive"]);
    let text = scene(&[
        instance("added", "twenty", &[], &[additive("ten")]),
        instance("unchanged", "twenty", &[], &[additive("still")]),
        instance("base_only", "twenty", &[], &[]),
    ]);
    let posed = posed_at(&engine_with(&root, &text), 18);

    let (turn, offset) = turn_and_offset(&posed, 0, LOWER);
    assert!((turn - 45.0).abs() < 1e-4, "rest 15 + 20 + 10, got {turn}");
    assert!(
        offset.abs_diff_eq(Vec3::new(0.6, 1.3, 0.0), 1e-5),
        "{offset:?}"
    );
    assert_same_local(&posed, 1, 2, LOWER, "a layer at the rest adds nothing");
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_reference_clip_cancels_an_identical_layer() {
    let root = fixture_project("reference", false);
    let text = scene(&[
        instance(
            "cancelled",
            "twenty",
            &[],
            &[layer("add", "ten", &["mode: additive", "reference: ten"])],
        ),
        instance("base_only", "twenty", &[], &[]),
    ]);
    let posed = posed_at(&engine_with(&root, &text), 18);
    assert_same_local(&posed, 0, 1, LOWER, "ten measured from ten");
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_blend_at_a_points_param_plays_that_clip() {
    let root = fixture_project("blend", false);
    let at = |x: f32| format!("params:\n            - {x:?}\n            - 0.0");
    let layer_at = |x: f32| format!("params:\n                - {x:?}\n                - 0.0");
    let (zero, one, half) = (at(0.0), at(1.0), at(0.5));
    let text = scene(&[
        instance("blend_zero", "sway", &[&zero], &[]),
        instance("swing", "swing", &[], &[]),
        instance("blend_one", "sway", &[&one], &[]),
        instance("reach", "reach", &[], &[]),
        instance("blend_half", "sway", &[&half], &[]),
        instance(
            "layer_blend",
            "whole",
            &[],
            &[layer("arm", "sway", &[&layer_at(1.0)])],
        ),
        instance("layer_clip", "whole", &[], &[layer("arm", "reach", &[])]),
    ]);
    let engine = engine_with(&root, &text);
    for tick in [9, 18, 75] {
        let posed = posed_at(&engine, tick);
        assert_same_local(&posed, 0, 1, UPPER, "param 0 is `swing`");
        assert_same_local(&posed, 2, 3, UPPER, "param 1 is `reach`");
        assert_same_local(&posed, 5, 6, UPPER, "a layer blend at 1 is `reach`");
        for other in [1, 3] {
            assert!(
                !local(&posed, 4, UPPER).abs_diff_eq(local(&posed, other, UPPER), 1e-3),
                "tick {tick}: halfway is neither point"
            );
        }
    }
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_placed_instance_layers_relative_to_its_placement() {
    let root = fixture_project("placed", false);
    let arm = |name: &str| {
        instance(
            name,
            "swing",
            &[],
            &[layer("add", "ten", &["mode: additive"])],
        )
    };
    let placed = arm("placed").replace(
        "    overrides:\n",
        "    transform:\n      translation:\n        - 5.0\n        - 0.0\n        - 2.0\n      \
         rotation:\n        - 0.0\n        - 90.0\n        - 0.0\n    overrides:\n",
    );
    let engine = engine_with(&root, &scene(&[arm("plain"), placed]));
    let placement = Mat4::from_translation(Vec3::new(5.0, 0.0, 2.0))
        * Mat4::from_rotation_y(90f32.to_radians());
    for tick in [0, 18, 36] {
        let posed = posed_at(&engine, tick);
        for part in 0..PARTS {
            let want = placement * posed[part].model;
            let got = posed[PARTS + part].model;
            assert!(
                want.abs_diff_eq(got, 1e-4),
                "tick {tick}, part {part}: {want:?} vs {got:?}"
            );
        }
    }
    let _ = std::fs::remove_dir_all(&root);
}

/// Layers need no base `Animation`, as on an assembly.
#[test]
fn layers_alone_pose_the_parts_over_their_rest() {
    let root = fixture_project("layers-only", false);
    let text = scene(&[
        instance("layered", "", &[], &[layer("arm", "twenty", &[])]),
        instance("as_base", "twenty", &[], &[]),
        instance("still", "", &[], &[]),
    ]);
    let posed = posed_at(&engine_with(&root, &text), 18);
    assert_same_local(&posed, 0, 1, LOWER, "the layer poses over the rest");
    assert_same_local(&posed, 0, 2, UPPER, "an unkeyed part rests");
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn an_unknown_mask_clip_or_reference_leaves_the_base_pose() {
    let root = fixture_project("unknown", false);
    let text = scene(&[
        instance(
            "unknown",
            "swing",
            &[],
            &[
                layer("mask", "whole", &["mask: elbow"]),
                layer("clip", "wave", &[]),
                layer("reference", "whole", &["mode: additive", "reference: wave"]),
            ],
        ),
        instance("base_only", "swing", &[], &[]),
    ]);
    let posed = posed_at(&engine_with(&root, &text), 18);
    for part in UPPER..PARTS {
        assert_eq!(local(&posed, 0, part), local(&posed, 1, part), "{part}");
    }
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn extracting_a_layered_prefab_run_changes_neither_its_hashes_nor_its_recording() {
    let root = fixture_project("hash-neutral", false);
    let text = scene(&[
        instance(
            "layered",
            "sway",
            &[],
            &[
                layer("arm", "curl", &["mask: forearm"]),
                layer("add", "tilt", &["mode: additive"]),
            ],
        ),
        instance("layers_only", "", &[], &[layer("arm", "sway", &[])]),
    ]);

    let run = |tag: &str, extract_each_tick: bool| {
        let lrp = root.join(format!("{tag}.lrp"));
        let mut engine = engine_with(&root, &text);
        engine
            .set_recording(std::fs::File::create(&lrp).unwrap())
            .expect("recording starts");
        let mut trace = Vec::new();
        for _ in 0..60 {
            let step = Command::SimStep(SimStepParams {
                ticks: 1,
                hash: false,
            });
            trace.extend(
                ludion_engine::drive_hashes(&mut engine, vec![step])
                    .expect("the session drives")
                    .into_iter()
                    // `drive_hashes` re-samples the current frame first; keep only the new tick.
                    .skip(1),
            );
            if extract_each_tick {
                let posed = posed_at(&engine, engine.frame().as_u64());
                assert_eq!(posed.len(), 2 * PARTS);
            }
        }
        engine.stop_recording().expect("recording stops");
        (trace, std::fs::read(&lrp).unwrap())
    };

    let (quiet_trace, quiet_bytes) = run("quiet", false);
    let (drawn_trace, drawn_bytes) = run("drawn", true);
    assert_eq!(quiet_trace.len(), 60);
    assert_eq!(quiet_trace, drawn_trace);
    assert_eq!(quiet_bytes, drawn_bytes);
    let _ = std::fs::remove_dir_all(&root);
}

// --- The `BlendClock` and events --------------------------------------------------------------

/// [`fixture_project`] plus what the clock and event proofs need: `swing` fires `step` and `reach`
/// fires `grab`, each a quarter in; `slow` is `swing` stretched to three seconds with no events, and
/// the blend `gait` runs from `swing` at 0 to `slow` at 1, so only its params change the cadence.
fn clock_project(name: &str) -> PathBuf {
    let dir = fixture_project(name, false);
    let clips = dir.join("arm/clips");
    let read = |clip: &str| {
        std::fs::read_to_string(clips.join(format!("{clip}.anim.yaml")))
            .unwrap()
            .replace("\r\n", "\n")
    };
    let slow = read("swing")
        .replace("name: swing", "name: slow")
        .replace("duration: 1.0", "duration: 3.0")
        .replace("t: 1.0", "t: 3.0")
        .replace("t: 0.5", "t: 1.5");
    std::fs::write(clips.join("slow.anim.yaml"), slow).unwrap();
    for (clip, event) in [("swing", "step"), ("reach", "grab")] {
        let text = read(clip).replacen(
            "tracks:\n",
            &format!("events:\n  - t: 0.25\n    name: {event}\ntracks:\n"),
            1,
        );
        std::fs::write(clips.join(format!("{clip}.anim.yaml")), text).unwrap();
    }
    let prefab = dir.join("arm/arm.prefab.yaml");
    let text = std::fs::read_to_string(&prefab)
        .unwrap()
        .replacen("clips:\n", "clips:\n  slow: clips/slow.anim.yaml\n", 1)
        .replacen(
            "blends:\n",
            "blends:\n  gait:\n    points:\n      - clip: swing\n        at: 0.0\n      \
             - clip: slow\n        at: 1.0\n",
            1,
        );
    std::fs::write(prefab, text).unwrap();
    dir
}

/// The events every tick fired, in firing order.
#[derive(Default)]
struct EventLog(Vec<(u64, ClipEvent)>);

fn record(world: &mut ludion_ecs::World, ctx: &ludion_ecs::SystemCtx) {
    let fired: Vec<ClipEvent> = world
        .resource::<AnimationEvents>()
        .map(|events| events.iter().cloned().collect())
        .unwrap_or_default();
    if world.resource::<EventLog>().is_none() {
        world.insert_resource(EventLog::default());
    }
    let log = world.resource_mut::<EventLog>().expect("just inserted");
    log.0
        .extend(fired.into_iter().map(|event| (ctx.frame.as_u64(), event)));
}

type Driver = fn(&mut ludion_ecs::World, &ludion_ecs::SystemCtx);

/// The game: an optional driver, then `blend_clocks` when `clocked`, the events and the log.
fn running(root: &Path, scene: &str, clocked: bool, driver: Option<Driver>) -> Engine {
    let mut builder = Engine::builder().project_root(root);
    if let Some(driver) = driver {
        builder = builder
            .system("driver", driver)
            .query::<(&mut Animation,)>();
    }
    if clocked {
        builder = builder
            .system("blend_clocks", blend_clocks)
            .query::<(&Animation, &Mesh)>()
            .query::<(&Animation, &PrefabInstance)>()
            .query::<(&mut BlendClock,)>();
    }
    let mut engine = builder
        .system("animation_events", animation_events)
        .query::<(&Animation, &Mesh)>()
        .query::<(&Animation, &Sprite)>()
        .query::<(&Animation, &PrefabInstance)>()
        .query::<(&BlendClock,)>()
        .system("record", record)
        .build()
        .expect("the game builds");
    engine.load_inline_scene(scene).expect("the scene loads");
    engine
}

fn step(engine: &mut Engine, ticks: u64) {
    engine
        .execute(Command::SimStep(SimStepParams { ticks, hash: false }))
        .expect("steps");
}

fn log(engine: &Engine) -> Vec<(u64, ClipEvent)> {
    engine
        .world()
        .resource::<EventLog>()
        .map(|log| log.0.clone())
        .unwrap_or_default()
}

/// The instance roots, in spawn order.
fn roots(engine: &Engine) -> Vec<EntityId> {
    let world = engine.world();
    world
        .entities()
        .filter(|&id| {
            world
                .component::<PrefabInstance>(id)
                .is_some_and(|instance| instance.root == id)
        })
        .collect()
}

fn clock_of(engine: &Engine, id: EntityId) -> Option<BlendClock> {
    engine.world().component::<BlendClock>(id).cloned()
}

fn params(x: f32) -> String {
    format!("params:\n            - {x:?}\n            - 0.0")
}

fn layer_params(x: f32) -> String {
    format!("params:\n                - {x:?}\n                - 0.0")
}

#[test]
fn a_blend_gets_a_clock_and_a_plain_clip_none() {
    let root = clock_project("clock");
    let text = scene(&[
        instance("blended", "sway", &[], &[]),
        instance("plain", "swing", &[], &[]),
        instance("layered", "swing", &[], &[layer("arm", "sway", &[])]),
    ]);
    let mut engine = running(&root, &text, true, None);
    step(&mut engine, 30);
    let [blended, plain, layered] = roots(&engine)[..] else {
        panic!("three instances");
    };
    let clock = clock_of(&engine, blended).expect("a base blend holds a clock");
    assert_eq!(clock.blend, "sway");
    assert!(clock.phase > 0.0);
    assert!(clock_of(&engine, plain).is_none(), "no blend, no clock");
    let clock = clock_of(&engine, layered).expect("a layer blend holds a clock");
    assert_eq!((clock.blend.as_str(), clock.layers.len()), ("", 1));
    assert_eq!(clock.layers[0].blend, "sway");
    let mut animation = engine
        .world()
        .component::<Animation>(blended)
        .cloned()
        .expect("animates");
    animation.clip = "swing".into();
    engine
        .world_mut()
        .insert(blended, animation)
        .expect("switches");
    step(&mut engine, 2);
    assert!(clock_of(&engine, blended).is_none(), "the blend is gone");
    let _ = std::fs::remove_dir_all(&root);
}

/// Switches `gait` from the fast point to the slow one at frame 240.
fn slow_down(world: &mut ludion_ecs::World, ctx: &ludion_ecs::SystemCtx) {
    if ctx.game_frame.as_u64() == 240 {
        for (_, (animation,)) in world.query_ids::<(&mut Animation,)>() {
            animation.params = [1.0, 0.0];
        }
    }
}

#[test]
fn a_param_change_keeps_a_clocked_pose_continuous() {
    let root = clock_project("continuous");
    let zero = params(0.0);
    let text = scene(&[instance("arm", "gait", &[&zero], &[])]);
    // The largest change of `upper`'s local matrix over the two ticks around the switch.
    let jump = |clocked: bool| {
        let mut engine = running(&root, &text, clocked, Some(slow_down));
        step(&mut engine, 239);
        let before = local(&posed_at(&engine, 239), 0, UPPER);
        step(&mut engine, 2);
        let after = local(&posed_at(&engine, 241), 0, UPPER);
        (after - before)
            .to_cols_array()
            .iter()
            .fold(0.0f32, |most, v| most.max(v.abs()))
    };
    let (clocked, stateless) = (jump(true), jump(false));
    assert!(clocked < 0.06, "the clock carries the phase: {clocked}");
    assert!(stateless > 0.2, "the stateless phase jumps: {stateless}");
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_base_blend_fires_the_dominant_points_events() {
    let root = clock_project("base-events");
    let (low, high) = (params(0.25), params(0.75));
    let text = scene(&[
        instance("swings", "sway", &[&low], &[]),
        instance("reaches", "sway", &[&high], &[]),
    ]);
    let mut engine = running(&root, &text, true, None);
    step(&mut engine, 150);
    let [swings, reaches] = roots(&engine)[..] else {
        panic!("two instances");
    };
    let events = log(&engine);
    for (id, name) in [(swings, "step"), (reaches, "grab")] {
        let mine: Vec<&ClipEvent> = events
            .iter()
            .map(|(_, e)| e)
            .filter(|e| e.entity == id)
            .collect();
        // A one-second cycle crosses a quarter at 0.25, 1.25 and 2.25 s.
        assert_eq!(mine.len(), 3, "{mine:?}");
        assert!(
            mine.iter()
                .all(|e| e.name == name && e.clip == "sway" && e.layer.is_empty()),
            "{mine:?}"
        );
    }
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn event_layers_fire_on_a_based_and_a_baseless_prefab_root() {
    let root = clock_project("layer-events");
    let high = layer_params(1.0);
    let layers = [
        layer("clip", "swing", &["events: true"]),
        layer("blend", "sway", &["events: true", &high]),
        layer("mute", "reach", &[]),
    ];
    let text = scene(&[
        instance("based", "curl", &[], &layers),
        instance("baseless", "\"\"", &[], &layers),
    ]);
    let mut engine = running(&root, &text, true, None);
    step(&mut engine, 90);
    let events = log(&engine);
    let ids = roots(&engine);
    assert_eq!(ids.len(), 2);
    for id in ids {
        let fired: Vec<(&str, &str, &str)> = events
            .iter()
            .map(|(_, e)| e)
            .filter(|e| e.entity == id)
            .map(|e| (e.layer.as_str(), e.clip.as_str(), e.name.as_str()))
            .collect();
        // Both cross at 0.25 and 1.25 s; the layers fire in list order within a tick.
        assert_eq!(
            fired,
            [
                ("clip", "swing", "step"),
                ("blend", "sway", "grab"),
                ("clip", "swing", "step"),
                ("blend", "sway", "grab"),
            ],
            "{id}"
        );
    }
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn registering_blend_clocks_moves_no_hash_without_a_prefab_blend() {
    let root = clock_project("clock-inert");
    let text = scene(&[
        instance("plain", "swing", &[], &[]),
        instance("fading", "curl", &["from: swing", "fade: 1.0"], &[]),
        instance("layered", "swing", &[], &[layer("arm", "curl", &[])]),
        instance("baseless", "", &[], &[layer("arm", "reach", &[])]),
    ]);
    let run = |clocked: bool| {
        let mut engine = running(&root, &text, clocked, None);
        let mut trace = Vec::new();
        for _ in 0..120 {
            let one = Command::SimStep(SimStepParams {
                ticks: 1,
                hash: false,
            });
            trace.extend(
                ludion_engine::drive_hashes(&mut engine, vec![one])
                    .expect("the session drives")
                    .into_iter()
                    .skip(1),
            );
        }
        for id in roots(&engine) {
            assert!(clock_of(&engine, id).is_none());
        }
        trace
    };
    let (clocked, unclocked) = (run(true), run(false));
    assert_eq!(clocked.len(), 120);
    assert_eq!(clocked, unclocked);
    let _ = std::fs::remove_dir_all(&root);
}

/// Writes `params = [tick / 120, 0]` (0 to 1 over 120 ticks, then held).
fn sweep_gait(world: &mut ludion_ecs::World, ctx: &ludion_ecs::SystemCtx) {
    let x = (ctx.game_frame.as_u64() as f32 / 120.0).min(1.0);
    for (_, (animation,)) in world.query_ids::<(&mut Animation,)>() {
        animation.params = [x, 0.0];
    }
}

#[test]
fn a_restored_prefab_blend_resumes_the_same_phase() {
    let root = clock_project("restore");
    let quarter = layer_params(0.25);
    let text = scene(&[instance(
        "arm",
        "gait",
        &[],
        &[layer(
            "add",
            "sway",
            &["events: true", "mode: additive", &quarter],
        )],
    )]);
    let mut engine = running(&root, &text, true, Some(sweep_gait));
    step(&mut engine, 70);
    let snapshot = match engine.execute(Command::EngineSnapshot).unwrap().body {
        ResponseBody::Snapshot { snapshot } => snapshot,
        other => panic!("expected a snapshot, got {other:?}"),
    };
    step(&mut engine, 100);
    let outcome = |engine: &Engine| {
        let events: Vec<(u64, ClipEvent)> = log(engine)
            .into_iter()
            .filter(|(tick, _)| *tick >= 70)
            .collect();
        let matrices: Vec<[u32; 16]> = posed_at(engine, engine.frame().as_u64())
            .iter()
            .map(|mesh| mesh.model.to_cols_array().map(f32::to_bits))
            .collect();
        (clock_of(engine, roots(engine)[0]), events, matrices)
    };
    let want = outcome(&engine);
    assert!(want.0.is_some() && !want.1.is_empty());
    match engine
        .execute(Command::EngineRestore(EngineRestoreParams { snapshot }))
        .unwrap()
        .body
    {
        ResponseBody::Restored { .. } => {}
        other => panic!("expected a restore, got {other:?}"),
    }
    engine.world_mut().remove_resource::<EventLog>();
    step(&mut engine, 100);
    assert_eq!(outcome(&engine), want);
    let _ = std::fs::remove_dir_all(&root);
}
