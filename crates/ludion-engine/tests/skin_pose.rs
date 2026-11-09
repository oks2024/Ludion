//! Skin posing on the `crowd-animation` gallery scene: 500 skinned figures in six groups (plain
//! `run`, `run` on twos, `jump` on fours, a mid-blend crossfade, a frozen `idle`, `idle` on
//! threes). Two fast tests pin the palette bits across thread counts and the skin counts; an
//! ignored benchmark measures the `Skin` phase, CPU only:
//!
//! ```text
//! cargo test -p ludion-engine --release --test it -- skin_pose --ignored --nocapture
//! ```
//!
//! There is deliberately no inline budget: the figures are unpinned until recorded on the pinned
//! machines.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

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

#[derive(Default)]
struct Clock {
    open: Option<Instant>,
    skin: Duration,
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
        if phase == ExtractPhase::Skin {
            self.0.lock().unwrap().open = Some(now());
        }
    }
    fn end(&mut self, phase: ExtractPhase) {
        let mut clock = self.0.lock().unwrap();
        if phase == ExtractPhase::Skin
            && let Some(start) = clock.open.take()
        {
            clock.skin += lap(start);
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
