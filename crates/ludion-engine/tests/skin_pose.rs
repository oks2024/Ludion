//! Skin posing on the `crowd-animation` gallery scene: 500 skinned figures in six groups (plain
//! `run`, `run` on twos, `jump` on fours, a mid-blend crossfade, a frozen `idle`, `idle` on
//! threes). Two fast tests pin the palette bits across thread counts and the skin counts; an
//! ignored benchmark measures the `Skin` phase, CPU only:
//!
//! ```text
//! cargo test -p ludion-engine --release --test it -- skin_pose --ignored --nocapture
//! cargo test -p ludion-engine --release --test it -- socket_pose --ignored --nocapture
//! ```
//!
//! There is deliberately no inline budget: the figures are unpinned until recorded on the pinned
//! machines.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use ludion_components::{Mesh, MeshSource, ParentJoint, Transform};
use ludion_core::ids::EntityId;
use ludion_ecs::Parent;
use ludion_engine::Engine;
use ludion_engine::extract_observer::{ExtractObserver, ExtractPhase, SkinCounts};
use ludion_protocol::{Command, command};
use ludion_renderer::RenderScene;

fn gallery() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/gallery")
}

fn crowd() -> Engine {
    let mut engine = Engine::builder()
        .project_root(gallery())
        .build()
        .expect("boots");
    let text = std::fs::read_to_string(gallery().join("crowd-animation.yaml")).expect("scene file");
    engine.load_inline_scene(&text).expect("the crowd loads");
    engine
}

/// `crowd()` with a socket prop (a small cube on `lower_arm_l`) under every figure.
fn socketed_crowd() -> Engine {
    let mut engine = crowd();
    let figures: Vec<EntityId> = engine
        .world()
        .entities()
        .filter(|&id| {
            matches!(
                engine.world().get::<Mesh>(id),
                Ok(Some(Mesh {
                    shape: MeshSource::Character(_),
                    ..
                }))
            )
        })
        .collect();
    let world = engine.world_mut();
    for figure in figures {
        let prop = world.spawn();
        world.insert(prop, Parent(figure)).unwrap();
        world
            .insert(
                prop,
                ParentJoint {
                    joint: "lower_arm_l".into(),
                },
            )
            .unwrap();
        world
            .insert(
                prop,
                Transform {
                    translation: [0.0, 0.2, 0.0],
                    rotation: [0.0; 3],
                    scale: [0.08, 0.25, 0.08],
                },
            )
            .unwrap();
        world.insert(prop, Mesh::default()).unwrap();
    }
    engine
}

fn step(engine: &mut Engine, ticks: u64) {
    engine
        .execute(Command::SimStep(command::SimStepParams {
            ticks,
            hash: false,
        }))
        .expect("steps");
}

fn palette_bits(scene: &RenderScene) -> (Vec<[u32; 16]>, Vec<u32>) {
    let bits = scene
        .bone_palette
        .iter()
        .map(|m| m.to_cols_array().map(f32::to_bits))
        .collect();
    let offsets = scene.meshes.iter().map(|m| m.bone_offset).collect();
    (bits, offsets)
}

/// The same 12 ticks run at 1 thread and at 8 pose the same
/// palette bits at every extract (and the same offsets), and end on the same world hash.
#[test]
fn crowd_palettes_are_byte_identical_at_1_and_8_threads() {
    let run = |threads: usize| {
        ludion_core::par::with_threads(threads, || {
            let mut engine = crowd();
            step(&mut engine, 70);
            let mut frames = Vec::new();
            for _ in 0..12 {
                step(&mut engine, 1);
                frames.push(palette_bits(
                    &engine.extract_present_scene().expect("extracts").0,
                ));
            }
            (frames, engine.world_hash().unwrap())
        })
    };
    assert!(
        run(1) == run(8),
        "palettes or hash differ between 1 and 8 threads"
    );
}

/// Every `scene.meshes[i].model` as bits.
fn model_bits(scene: &RenderScene) -> Vec<[u32; 16]> {
    scene
        .meshes
        .iter()
        .map(|m| m.model.to_cols_array().map(f32::to_bits))
        .collect()
}

/// The socketed crowd extracts the same palettes, models and hash at 1 and 8 threads.
#[test]
fn socketed_crowd_is_byte_identical_at_1_and_8_threads() {
    let run = |threads: usize| {
        ludion_core::par::with_threads(threads, || {
            let mut engine = socketed_crowd();
            step(&mut engine, 70);
            let mut frames = Vec::new();
            for _ in 0..12 {
                step(&mut engine, 1);
                let scene = engine.extract_present_scene().expect("extracts").0;
                frames.push((palette_bits(&scene), model_bits(&scene)));
            }
            (frames, engine.world_hash().unwrap())
        })
    };
    assert!(
        run(1) == run(8),
        "socketed palettes, models or hash differ between 1 and 8 threads"
    );
}

#[derive(Clone, Default)]
struct Recorder(Arc<Mutex<SkinCounts>>);

impl ExtractObserver for Recorder {
    fn begin(&mut self, _: ExtractPhase) {}
    fn end(&mut self, _: ExtractPhase) {}
    fn skin(&mut self, counts: SkinCounts) {
        *self.0.lock().unwrap() = counts;
    }
}

/// Every figure poses its own palette each extract: 500 sampled, none reused or shared, and the
/// 83 figures mid-crossfade (group 3) sample both clips.
#[test]
fn every_figure_samples_its_own_palette() {
    let mut engine = crowd();
    let recorder = Recorder::default();
    engine.set_extract_observer(Some(Box::new(recorder.clone())));
    step(&mut engine, 73);
    engine.extract_present_scene().expect("extracts");
    let counts = *recorder.0.lock().unwrap();
    assert_eq!(counts.instances, 500);
    assert_eq!(counts.palettes_sampled, 500);
    assert_eq!(counts.palettes_reused, 0);
    assert_eq!(counts.palettes_shared, 0);
    assert_eq!(counts.joints_sampled, 11 * (500 + 83));
}

/// Sockets ride their skins; they never change a skin's palette or its counts.
#[test]
fn sockets_do_not_change_the_skin_palettes() {
    let run = |mut engine: Engine| {
        let recorder = Recorder::default();
        engine.set_extract_observer(Some(Box::new(recorder.clone())));
        step(&mut engine, 73);
        let scene = engine.extract_present_scene().expect("extracts").0;
        let counts = *recorder.0.lock().unwrap();
        (palette_bits(&scene).0, counts)
    };
    let (plain_bits, plain) = run(crowd());
    let (socket_bits, socketed) = run(socketed_crowd());
    assert!(plain_bits == socket_bits, "sockets moved a palette");
    assert_eq!(plain, socketed, "sockets moved the skin counts");
}

#[derive(Default)]
struct Clock {
    open: Option<Instant>,
    skin: Duration,
    poses: Duration,
}

#[derive(Clone, Default)]
struct SkinClock(Arc<Mutex<Clock>>);

#[allow(clippy::disallowed_methods)]
fn now() -> Instant {
    Instant::now()
}

#[allow(clippy::disallowed_methods)]
fn lap(start: Instant) -> Duration {
    start.elapsed()
}

impl ExtractObserver for SkinClock {
    fn begin(&mut self, phase: ExtractPhase) {
        if matches!(phase, ExtractPhase::Skin | ExtractPhase::Poses) {
            self.0.lock().unwrap().open = Some(now());
        }
    }
    fn end(&mut self, phase: ExtractPhase) {
        let mut clock = self.0.lock().unwrap();
        if matches!(phase, ExtractPhase::Skin | ExtractPhase::Poses)
            && let Some(start) = clock.open.take()
        {
            let spent = lap(start);
            if phase == ExtractPhase::Skin {
                clock.skin += spent;
            } else {
                clock.poses += spent;
            }
        }
    }
}

const ROUNDS: usize = 5;
const ITERATIONS: usize = 120;

fn median(samples: &mut [f64]) -> f64 {
    samples.sort_by(f64::total_cmp);
    samples[samples.len() / 2]
}

#[test]
#[ignore = "release-mode benchmark; run explicitly"]
fn skin_pose_bench() {
    let mut engine = crowd();
    let clock = SkinClock::default();
    engine.set_extract_observer(Some(Box::new(clock.clone())));
    step(&mut engine, 60);
    let mut best = f64::INFINITY;
    for _ in 0..ROUNDS {
        let mut samples = Vec::new();
        for _ in 0..ITERATIONS {
            step(&mut engine, 1);
            engine.extract_present_scene().expect("extracts");
            let mut clock = clock.0.lock().unwrap();
            samples.push(std::mem::take(&mut clock.skin).as_secs_f64() * 1_000.0);
        }
        best = best.min(median(&mut samples));
    }
    println!("skin pose: 500 figures, Skin-phase median {best:.3} ms (best of {ROUNDS} rounds)");
}

/// `Poses`, `Skin` and their sum on the socketed crowd, at 1 and 8 threads.
#[test]
#[ignore = "release-mode benchmark; run explicitly"]
fn socket_pose_bench() {
    for threads in [1, 8] {
        ludion_core::par::with_threads(threads, || {
            let mut engine = socketed_crowd();
            let clock = SkinClock::default();
            engine.set_extract_observer(Some(Box::new(clock.clone())));
            step(&mut engine, 60);
            let mut best = [f64::INFINITY; 3];
            for _ in 0..ROUNDS {
                let mut samples: [Vec<f64>; 3] = Default::default();
                for _ in 0..ITERATIONS {
                    step(&mut engine, 1);
                    engine.extract_present_scene().expect("extracts");
                    let mut clock = clock.0.lock().unwrap();
                    let poses = std::mem::take(&mut clock.poses).as_secs_f64() * 1_000.0;
                    let skin = std::mem::take(&mut clock.skin).as_secs_f64() * 1_000.0;
                    samples[0].push(poses);
                    samples[1].push(skin);
                    samples[2].push(poses + skin);
                }
                for (b, s) in best.iter_mut().zip(samples.iter_mut()) {
                    *b = b.min(median(s));
                }
            }
            println!(
                "socket pose T={threads}: 500 socketed figures, Poses {:.3} ms, Skin {:.3} ms, Poses+Skin {:.3} ms (best of {ROUNDS} rounds)",
                best[0], best[1], best[2]
            );
        });
    }
}
