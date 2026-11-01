//! The presenter seam: a windowed host's presenter thread is fed the sim's extracted scenes and must
//! never slow, or change, the sim. A fake [`PresentBackend`] stands in for the window, so none of
//! this needs a GPU (`Plan/display-paced-present.md`).

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use crossbeam_channel::unbounded;
use ludion_core::diagnostics::Diagnostic;
use ludion_core::time::FrameIndex;
use ludion_ecs::{Registry, World};
use ludion_engine::live::{StampedCommand, run_live, shutdown_channel};
use ludion_engine::{Engine, TickObserver};
use ludion_protocol::Command;
use ludion_renderer::{PresentBackend, PresentOutcome, PresentPacket, ProbeResolve};
use ludion_sim::WorldHash;

const SCENE: &str =
    "format: 1\nentities:\n  - name: cam\n    components:\n      Transform: {}\n      Camera: {}\n";

/// Each render's frame index and whether its tick stamp was re-based.
type Stamps = Arc<Mutex<Vec<(u64, Option<bool>)>>>;

/// Each render's frame index and the x translation of its first mesh.
type Poses = Arc<Mutex<Vec<(u64, f32)>>>;

/// A cube a builder system marches +1 m along x every tick.
const CUBE_SCENE: &str = "format: 1\nentities:\n  - name: cam\n    components:\n      Transform: {}\n      Camera: {}\n  - name: cube\n    components:\n      Transform: {}\n      Mesh: {}\n";

fn march(world: &mut World, _ctx: &ludion_ecs::SystemCtx) {
    for (transform,) in world.query::<(&mut ludion_engine::Transform,)>() {
        transform.translation[0] += 1.0;
    }
}

/// Counts renders and presents; `present` sleeps `present_for`, standing in for a display's refresh.
struct Fake {
    renders: Arc<AtomicU64>,
    presents: Arc<AtomicU64>,
    stamps: Stamps,
    poses: Poses,
    present_for: Duration,
}

impl PresentBackend for Fake {
    fn render(&mut self, packet: &PresentPacket) -> Result<(), Diagnostic> {
        self.renders.fetch_add(1, Ordering::Relaxed);
        if let Some(mesh) = packet.scene.meshes.first() {
            self.poses
                .lock()
                .unwrap()
                .push((packet.frame.0, mesh.model.w_axis.x));
        }
        self.stamps
            .lock()
            .unwrap()
            .push((packet.frame.0, packet.tick.map(|tick| tick.rebased)));
        Ok(())
    }
    fn present(&mut self) -> Result<PresentOutcome, Diagnostic> {
        thread::sleep(self.present_for);
        self.presents.fetch_add(1, Ordering::Relaxed);
        Ok(PresentOutcome::Presented)
    }
    fn probe_resolve(&self) -> Option<ProbeResolve> {
        None
    }
    fn reload_shader(&mut self, _: &str, _: String) -> Result<u64, Diagnostic> {
        Ok(0)
    }
    fn idle(&self) -> bool {
        false
    }
}

struct Counts {
    renders: Arc<AtomicU64>,
    presents: Arc<AtomicU64>,
    stamps: Stamps,
    poses: Poses,
}

/// An engine over [`SCENE`], with a fake presenter attached when `present_for` is given.
fn engine(present_for: Option<Duration>) -> (Engine, Counts) {
    attach(
        Engine::builder().build().unwrap(),
        SCENE,
        present_for.map(|d| (d, false)),
    )
}

/// An engine over [`CUBE_SCENE`] with the marching system; `interpolate` is the builder flag.
fn marching(present_for: Option<Duration>, interpolate: bool) -> (Engine, Counts) {
    let engine = Engine::builder()
        .present_interpolation(interpolate)
        .system("march", march)
        .build()
        .unwrap();
    attach(engine, CUBE_SCENE, present_for.map(|d| (d, interpolate)))
}

fn attach(
    mut engine: Engine,
    scene: &str,
    presenter: Option<(Duration, bool)>,
) -> (Engine, Counts) {
    engine.load_inline_scene(scene).unwrap();
    let counts = Counts {
        renders: Arc::default(),
        presents: Arc::default(),
        stamps: Arc::default(),
        poses: Arc::default(),
    };
    if let Some((present_for, _)) = presenter {
        engine.attach_present_backend(Box::new(Fake {
            renders: Arc::clone(&counts.renders),
            presents: Arc::clone(&counts.presents),
            stamps: Arc::clone(&counts.stamps),
            poses: Arc::clone(&counts.poses),
            present_for,
        }));
    }
    (engine, counts)
}

/// Records each tick's frame and `WorldHash`.
struct Hashes(Arc<Mutex<BTreeMap<u64, WorldHash>>>);

impl TickObserver for Hashes {
    fn after_tick(&mut self, frame: FrameIndex, world: &World, registry: &Registry) {
        if let Ok(hash) = WorldHash::compute(world, registry) {
            self.0.lock().unwrap().insert(frame.0, hash);
        }
    }
}

struct Ticks(Arc<AtomicU64>);

impl TickObserver for Ticks {
    fn after_tick(&mut self, _: FrameIndex, _: &World, _: &Registry) {
        self.0.fetch_add(1, Ordering::Relaxed);
    }
}

/// A live loop on its own thread; `stop` ends it and joins.
struct Live {
    intake: crossbeam_channel::Sender<StampedCommand>,
    shutdown: ludion_engine::live::ShutdownSignal,
    handle: thread::JoinHandle<Result<(), Diagnostic>>,
}

fn run(engine: Engine) -> Live {
    let (intake, intake_rx) = unbounded();
    let (shutdown, shutdown_rx) = shutdown_channel();
    let handle = thread::spawn(move || run_live(engine, intake_rx, shutdown_rx, None));
    Live {
        intake,
        shutdown,
        handle,
    }
}

impl Live {
    fn send(&self, command: Command) {
        let (stamped, reply) = StampedCommand::new(command);
        self.intake.send(stamped).unwrap();
        let _ = reply.recv_timeout(Duration::from_secs(5)).unwrap();
    }

    fn stop(self) {
        self.shutdown.signal();
        self.handle.join().unwrap().unwrap();
    }
}

fn until(what: &str, mut done: impl FnMut() -> bool) {
    for _ in 0..2000 {
        if done() {
            return;
        }
        thread::sleep(Duration::from_millis(5));
    }
    panic!("timed out waiting for {what}");
}

#[test]
fn a_slow_display_never_slows_the_tick() {
    let (mut engine, counts) = engine(Some(Duration::from_millis(50)));
    let ticks = Arc::new(AtomicU64::new(0));
    engine.set_tick_observer(Ticks(Arc::clone(&ticks)));
    let live = run(engine);
    thread::sleep(Duration::from_millis(1200));
    live.stop();
    let ticks = ticks.load(Ordering::Relaxed);
    let presents = counts.presents.load(Ordering::Relaxed);
    let renders = counts.renders.load(Ordering::Relaxed);
    // A sim coupled to the 20 Hz display would reach ~24 ticks in 1.2 s, about one per present;
    // the floor and the ratio sit between that and the nominal 72, so a loaded box still passes.
    assert!(
        ticks >= 36,
        "the sim kept its pace: {ticks} ticks, {presents} presents, {renders} renders"
    );
    assert!(
        presents <= 30,
        "the display ran at its own rate: {ticks} ticks, {presents} presents, {renders} renders"
    );
    assert!(
        renders <= presents + 1,
        "{ticks} ticks, {renders} renders, {presents} presents"
    );
    assert!(
        ticks as f64 > 2.0 * presents as f64,
        "{ticks} ticks, {presents} presents, {renders} renders"
    );
}

#[test]
fn a_presenter_leaves_the_world_hash_unchanged() {
    let hashes = |present_for| {
        let (mut engine, _) = engine(present_for);
        let seen = Arc::new(Mutex::new(BTreeMap::new()));
        engine.set_tick_observer(Hashes(Arc::clone(&seen)));
        let live = run(engine);
        thread::sleep(Duration::from_millis(600));
        live.stop();
        Arc::try_unwrap(seen).unwrap().into_inner().unwrap()
    };
    let with = hashes(Some(Duration::from_millis(20)));
    let without = hashes(None);
    let common: Vec<_> = with.keys().filter(|f| without.contains_key(f)).collect();
    assert!(
        common.len() >= 10,
        "the runs overlap on {} frames",
        common.len()
    );
    for frame in common {
        assert_eq!(with[frame], without[frame], "frame {frame} diverged");
    }
}

#[test]
fn a_rebased_deadline_is_stamped() {
    let (engine, counts) = engine(Some(Duration::from_millis(1)));
    let live = run(engine);
    let rendered = || counts.stamps.lock().unwrap().len();
    until("steady packets", || rendered() >= 5);
    live.send(Command::SimPause);
    thread::sleep(Duration::from_millis(200));
    let paused_at = rendered();
    live.send(Command::SimResume);
    until("a packet after resume", || rendered() > paused_at + 3);
    live.stop();
    let stamps = counts.stamps.lock().unwrap();
    assert_eq!(
        stamps[paused_at].1,
        Some(true),
        "the first tick after a pause is re-based"
    );
    assert!(
        stamps[..paused_at]
            .iter()
            .any(|(_, rebased)| *rebased == Some(false)),
        "steady ticks are not"
    );
}

/// The x offset of each render from its frame index: constant while every render shows a whole
/// tick pose, within one tick's step while blended.
fn offsets(counts: &Counts) -> Vec<f32> {
    counts
        .poses
        .lock()
        .unwrap()
        .iter()
        .map(|(frame, x)| x - *frame as f32)
        .collect()
}

fn ticks_in_a_second_of(engine: Engine) -> u64 {
    let ticks = Arc::new(AtomicU64::new(0));
    let mut engine = engine;
    engine.set_tick_observer(Ticks(Arc::clone(&ticks)));
    let live = run(engine);
    thread::sleep(Duration::from_millis(1200));
    live.stop();
    ticks.load(Ordering::Relaxed)
}

#[test]
fn interpolation_draws_between_ticks() {
    let (engine, counts) = marching(Some(Duration::from_millis(4)), true);
    let ticks = ticks_in_a_second_of(engine);
    assert!(ticks >= 36, "the sim kept its pace: {ticks} ticks");
    let offsets = offsets(&counts);
    let hi = offsets.iter().fold(f32::MIN, |hi, o| hi.max(*o));
    // A render blends from the last pose the presenter saw, which is at most as many ticks back as
    // the frames the presenter skipped since its previous render (a loaded box skips some): it
    // never draws past its own tick, nor before the previous render's tick.
    assert!(
        hi <= 1e-3,
        "no extrapolation: a render drew {hi} past its tick"
    );
    let frames: Vec<u64> = counts.poses.lock().unwrap().iter().map(|p| p.0).collect();
    let mut earlier = 0; // the last distinct frame rendered; a re-render of one frame re-blends it
    let mut current = 0;
    for (i, offset) in offsets.iter().enumerate() {
        if frames[i] != current {
            earlier = current;
            current = frames[i];
        }
        let gap = frames[i] - earlier;
        assert!(
            *offset >= hi - gap as f32 - 1e-3,
            "no extrapolation: render {i} (frame {}) sits {} ticks before its pose, over a {gap}-frame gap",
            frames[i],
            hi - offset
        );
    }
    assert!(
        offsets.iter().any(|o| (o - hi).abs() > 1e-3),
        "at least one render sits between two poses: {} renders",
        offsets.len()
    );
    let presents = counts.presents.load(Ordering::Relaxed);
    assert!(
        counts.renders.load(Ordering::Relaxed) > ticks.min(presents) / 2,
        "renders happen per refresh"
    );
}

#[test]
fn without_interpolation_every_render_is_a_whole_tick_pose() {
    let (engine, counts) = marching(Some(Duration::from_millis(4)), false);
    let _ = ticks_in_a_second_of(engine);
    let offsets = offsets(&counts);
    assert!(!offsets.is_empty());
    assert!(
        offsets.iter().all(|o| *o == offsets[0]),
        "whole poses only: {offsets:?}"
    );
    let renders = counts.renders.load(Ordering::Relaxed);
    let presents = counts.presents.load(Ordering::Relaxed);
    assert!(
        renders <= presents + 1,
        "{renders} renders, {presents} presents"
    );
}

#[test]
fn interpolation_leaves_the_world_hash_unchanged() {
    let hashes = |interpolate| {
        let (mut engine, _) = marching(Some(Duration::from_millis(4)), interpolate);
        let seen = Arc::new(Mutex::new(BTreeMap::new()));
        engine.set_tick_observer(Hashes(Arc::clone(&seen)));
        let live = run(engine);
        thread::sleep(Duration::from_millis(600));
        live.stop();
        Arc::try_unwrap(seen).unwrap().into_inner().unwrap()
    };
    let on = hashes(true);
    let off = hashes(false);
    let common: Vec<_> = on.keys().filter(|f| off.contains_key(f)).collect();
    assert!(
        common.len() >= 10,
        "the runs overlap on {} frames",
        common.len()
    );
    for frame in common {
        assert_eq!(on[frame], off[frame], "frame {frame} diverged");
    }
}
