//! Runtime proof for analytic particles (`Plan/particles-engine-and-editor.md` decisions 1, 2, 4):
//! a `ParticleEmitter` describes an effect, `extract()` computes the live set from it, and the
//! simulation never notices.
//!
//! Two properties, pulling in opposite directions on purpose — the `assembly_animation.rs` shape,
//! because the claim is the same one:
//!
//! - **The sim is untouched.** A run holding emitters produces the same per-frame `WorldHash` trace
//!   and the same recorded `.lrp` bytes whether or not anything extracts a frame. Particles are a
//!   pure function of `(emitter, sim time)` computed at the render boundary and written back
//!   nowhere, so no particle is ever a hashed term, a record, or an entity.
//! - **The evaluator actually runs.** The same world extracted at different sim times moves its
//!   particles, and an emitter-free world extracts none. Without these the first property would be
//!   satisfied by an evaluator that does nothing at all.
//!
//! Everything goes through the ordinary public surface: an inline scene, `Engine`, `drive_hashes`,
//! `extract_at`.

use std::path::PathBuf;

use ludion_core::par;
use ludion_core::time::{SimTime, TickRate};
use ludion_engine::{Engine, QualityParams, extract_at};
use ludion_protocol::{Command, command};
use ludion_renderer::ParticleInstance;

/// A camera, an endless ambient plume (in steady state from frame 0), and a burst stamped at a
/// frame the run actually reaches — so both streams are live during the drive.
const SCENE: &str = "\
format: 1
seed: 3
entities:
  - name: camera
    components:
      Transform:
        translation:
          - 0.0
          - 2.0
          - 8.0
      Camera: {}
  - name: brazier
    components:
      Transform:
        translation:
          - 0.0
          - 1.0
          - 0.0
      ParticleEmitter:
        rate: 30.0
        life:
          - 1.0
          - 2.0
        speed:
          - 0.5
          - 1.5
        gravity: -1.0
        drag: 0.5
        seed: 11
  - name: impact
    components:
      Transform:
        translation:
          - 3.0
          - 0.0
          - 0.0
      ParticleEmitter:
        rate: 0.0
        burst: 24
        started: 12
        spread_deg: 180.0
        gravity: 9.8
        blend: additive
        seed: 12
";

/// The same scene with both emitters removed — the control for "an emitter-free world draws none".
const BARE_SCENE: &str = "\
format: 1
seed: 3
entities:
  - name: camera
    components:
      Transform:
        translation:
          - 0.0
          - 2.0
          - 8.0
      Camera: {}
";

fn engine_with(scene: &str) -> Engine {
    let mut engine = Engine::new();
    engine.load_inline_scene(scene).expect("the scene loads");
    engine
}

/// One `sim.step` of `ticks` ticks.
fn step(ticks: u64) -> Command {
    Command::SimStep(command::SimStepParams { ticks, hash: false })
}

/// The particles `engine`'s world shows at `tick`, through the public extract entry point.
fn particles_at(engine: &Engine, tick: u64) -> Vec<ParticleInstance> {
    extract_at(
        engine.world(),
        (320, 180),
        engine.project_root(),
        SimTime::from_ticks(tick, TickRate::DEFAULT),
        QualityParams::FULL.particle_density,
    )
    .expect("the scene extracts")
    .particles
}

/// A fresh, process-unique temp directory for the two recordings.
fn recording_dir() -> PathBuf {
    let dir = std::env::temp_dir().join(format!("ludion-particles-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("a temp directory");
    dir
}

/// The sim half of decision 1, both ways at once: the per-frame `WorldHash` trace and the recorded
/// `.lrp` bytes of a run holding two emitters are identical whether or not frames are extracted
/// along the way.
///
/// The extracting run really does draw particles (asserted inside the loop, and
/// `particles_move_with_sim_time` proves they move), so this is not vacuous: it says the render
/// boundary is a read, and that an effect adds nothing to the world, nothing to the record stream,
/// and nothing to a replay.
#[test]
fn extracting_particles_changes_neither_the_hashes_nor_the_recording() {
    let dir = recording_dir();

    let run = |tag: &str, extract_each_tick: bool| {
        let lrp = dir.join(format!("{tag}.lrp"));
        let mut engine = engine_with(SCENE);
        engine
            .set_recording(std::fs::File::create(&lrp).unwrap())
            .expect("recording starts");

        let mut trace = Vec::new();
        let mut drawn = 0usize;
        for _ in 0..40 {
            trace.extend(
                ludion_engine::drive_hashes(&mut engine, vec![step(1)])
                    .expect("the session drives")
                    .into_iter()
                    // `drive_hashes` re-samples the current frame first; keep only the new tick.
                    .skip(1),
            );
            if extract_each_tick {
                drawn += particles_at(&engine, engine.frame().as_u64()).len();
            }
        }
        engine.stop_recording().expect("recording stops");
        (trace, std::fs::read(&lrp).unwrap(), drawn)
    };

    let (quiet_trace, quiet_bytes, _) = run("quiet", false);
    let (drawn_trace, drawn_bytes, drawn) = run("drawn", true);

    assert_eq!(quiet_trace.len(), 40, "the run actually advanced 40 frames");
    assert!(drawn > 0, "the extracting run actually drew particles");
    assert_eq!(
        quiet_trace, drawn_trace,
        "extracting particles must not move a single world hash"
    );
    assert_eq!(
        quiet_bytes, drawn_bytes,
        "extracting particles must not write a byte into the recording"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// The evaluator half: the same world extracted at three sim times shows three different pictures,
/// and re-extracting one of them is identical — a pure function of `(world, now)`.
#[test]
fn particles_move_with_sim_time() {
    let engine = engine_with(SCENE);

    let (start, quarter, half) = (
        particles_at(&engine, 0),
        particles_at(&engine, 15),
        particles_at(&engine, 30),
    );
    assert!(
        !start.is_empty(),
        "the endless emitter is in steady state at tick 0"
    );
    assert_ne!(start, quarter, "a quarter second in");
    assert_ne!(quarter, half, "and again by the half");
    assert_eq!(quarter, particles_at(&engine, 15), "same instant, same set");
}

/// The burst is stamped at frame 12, so it is absent before that frame and present after — the
/// `started` convention as an observable.
#[test]
fn a_burst_appears_only_after_its_started_frame() {
    let engine = engine_with(SCENE);

    // The additive flag is the burst emitter's alone, so it identifies its particles in the set.
    let burst_at = |tick: u64| {
        particles_at(&engine, tick)
            .into_iter()
            .filter(|p| p.additive)
            .count()
    };
    assert_eq!(burst_at(0), 0, "nothing before `started`");
    assert!(burst_at(12) > 0, "the whole burst releases at `started`");
    assert_eq!(burst_at(600), 0, "and it is spent long after");
}

/// A world with no emitter draws no particles at all — the invisibility half: every scene in the
/// repo that authors none extracts exactly the list it did before the component existed.
#[test]
fn an_emitter_free_world_draws_no_particles() {
    let engine = engine_with(BARE_SCENE);
    assert!(particles_at(&engine, 0).is_empty());
    assert!(particles_at(&engine, 300).is_empty());
}

/// The list `engine` shows at `tick`, extracted with `threads` workers.
fn particles_at_threads(engine: &Engine, tick: u64, threads: usize) -> Vec<ParticleInstance> {
    par::with_threads(threads, || particles_at(engine, tick))
}

/// Parallel extract is the same list at any thread count: on the committed stress scene (far over
/// the sequential gate, mid-flight at tick 37) and on the small in-repo scene (under it).
#[test]
fn the_particle_list_is_identical_at_every_thread_count() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/gallery");
    let source = std::fs::read_to_string(root.join("particles-stress.yaml"))
        .expect("the committed stress scene reads");
    let mut stress = Engine::with_project_root(&root);
    stress
        .load_inline_scene(&source)
        .expect("the stress scene validates");

    let one = particles_at_threads(&stress, 37, 1);
    // The gate is 4096 live particles; this population must sit well over it.
    assert!(one.len() > 20_000, "only {} particles", one.len());
    assert_eq!(one, particles_at_threads(&stress, 37, 2), "1 vs 2 threads");
    assert_eq!(one, particles_at_threads(&stress, 37, 8), "1 vs 8 threads");

    let small = engine_with(SCENE);
    let one = particles_at_threads(&small, 90, 1);
    assert!(!one.is_empty());
    assert_eq!(one, particles_at_threads(&small, 90, 8), "small scene");
}
