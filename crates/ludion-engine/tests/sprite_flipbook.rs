//! Runtime proof for flipbook playback (`Plan/sprite-engine.md` decision 10): the one `Animation`
//! component that selects an assembly clip for a `Mesh` selects an atlas flipbook for a `Sprite`,
//! `extract()` resolves which region is showing from `(clip, sim time)`, and the simulation never
//! notices.
//!
//! Two properties, pulling in opposite directions on purpose — the shape `tests/assembly_animation.rs`
//! established for the other evaluator:
//!
//! - **The sim is untouched.** An animated run's per-frame `WorldHash` trace and its recorded `.lrp`
//!   are byte-identical whether or not anything extracts a frame. The played frame is a pure function
//!   of the component and the tick count, computed at the render boundary and written back nowhere.
//! - **The evaluator actually runs.** The same world extracted at different sim times yields
//!   different UV rects *and* different quad extents (the fixture's regions are all different shapes,
//!   so a wrong region cannot pass by coincidence), while a clip name the atlas does not bind draws
//!   exactly the authored frame.
//!
//! Everything goes through the ordinary public surface — a scene document, `Engine`, `drive_hashes`,
//! `extract_at` — over a fixture atlas written to a temp directory, because the atlas pair on disk is
//! what a flipbook name resolves against.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use ludion_core::time::{SimTime, TickRate};
use ludion_engine::{Engine, QualityParams, extract_at};
use ludion_protocol::{Command, command};
use ludion_renderer::SpriteInstance;
use ludion_scene::{AtlasDocument, AtlasFilter, AtlasRegion, Flipbook};

/// The packed image the fixture atlas addresses.
const ATLAS_WIDTH: u32 = 8;
const ATLAS_HEIGHT: u32 = 8;

/// How many texels of the fixture atlas span one world unit.
const PIXELS_PER_UNIT: f64 = 4.0;

/// The fixture's clip rate. At [`TickRate::DEFAULT`] one clip frame is exactly 15 ticks, so every
/// assertion below lands on a frame boundary rather than near one.
const FPS: f64 = 4.0;

/// Ticks per clip frame at [`FPS`] — the fixture's whole reason for choosing that rate.
const TICKS_PER_FRAME: u64 = 15;

/// How many `Sprite`s the fixture scene places, in entity order.
const SPRITES: usize = 5;

/// The scene's sprite slots, by entity order.
const LOOPER: usize = 0;
const ONESHOT: usize = 1;
const UNBOUND: usize = 2;
const FROZEN: usize = 3;
const STILL: usize = 4;

/// Five quads over one atlas: a looping clip, a one-shot with holds, a clip name the atlas binds
/// nothing to, the looping clip frozen at `speed: 0`, and a sprite with no `Animation` at all. None
/// authors a `size`, so each quad's extent is its region's own — which is how a wrong region is
/// visible in the geometry as well as the UVs.
const SCENE: &str = "\
format: 1
seed: 1
entities:
  - name: looper
    components:
      Sprite:
        atlas: art/fixture.atlas.yaml
        frame: wide
      Animation:
        clip: fly
  - name: oneshot
    components:
      Sprite:
        atlas: art/fixture.atlas.yaml
        frame: wide
      Animation:
        clip: burst
  - name: unbound
    components:
      Sprite:
        atlas: art/fixture.atlas.yaml
        frame: wide
      Animation:
        clip: nonesuch
  - name: frozen
    components:
      Sprite:
        atlas: art/fixture.atlas.yaml
        frame: wide
      Animation:
        clip: fly
        speed: 0.0
  - name: still
    components:
      Sprite:
        atlas: art/fixture.atlas.yaml
        frame: wide
";

/// Three sprites playing the 15, 30 and 60 fps looping flipbooks, for the tick-exactness sweeps.
const RATES_SCENE: &str = "\
format: 1
seed: 1
entities:
  - name: f15
    components:
      Sprite:
        atlas: art/fixture.atlas.yaml
        frame: wide
      Animation:
        clip: f15
  - name: f30
    components:
      Sprite:
        atlas: art/fixture.atlas.yaml
        frame: wide
      Animation:
        clip: f30
  - name: f60
    components:
      Sprite:
        atlas: art/fixture.atlas.yaml
        frame: wide
      Animation:
        clip: f60
";

/// The fixture atlas: three regions of three different shapes, one looping flipbook over all three,
/// and a one-shot whose middle frame is held twice as long as its neighbours.
fn doc() -> AtlasDocument {
    let mut regions = BTreeMap::new();
    for (name, rect, pivot) in [
        ("wide", [0, 0, 4, 2], [2.0, 1.0]),
        ("tall", [4, 0, 2, 3], [1.0, 1.5]),
        ("dot", [0, 4, 1, 1], [0.5, 0.5]),
    ] {
        regions.insert(name.to_string(), AtlasRegion { rect, pivot });
    }
    let frames = ["wide", "tall", "dot"].map(str::to_string).to_vec();
    AtlasDocument {
        generated_from: "fixture.atlas-source.yaml".to_string(),
        pixels_per_unit: PIXELS_PER_UNIT,
        color_map: "fixture.png".to_string(),
        filter: AtlasFilter::Nearest,
        regions,
        flipbooks: vec![
            Flipbook {
                name: "fly".to_string(),
                frames: frames.clone(),
                fps: FPS,
                looped: true,
                hold: None,
            },
            Flipbook {
                name: "f15".to_string(),
                frames: frames.clone(),
                fps: 15.0,
                looped: true,
                hold: None,
            },
            Flipbook {
                name: "f30".to_string(),
                frames: frames.clone(),
                fps: 30.0,
                looped: true,
                hold: None,
            },
            Flipbook {
                name: "f60".to_string(),
                frames: frames.clone(),
                fps: 60.0,
                looped: true,
                hold: None,
            },
            Flipbook {
                name: "burst".to_string(),
                frames,
                fps: FPS,
                looped: false,
                hold: Some(vec![1, 2, 1]),
            },
        ],
    }
}

/// A fresh project directory unique to this process + `label`, carrying the fixture atlas pair.
fn fixture_project(label: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "ludion-sprite-flipbook-{label}-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("art")).unwrap();

    let rgba: Vec<u8> = (0..ATLAS_WIDTH * ATLAS_HEIGHT * 4)
        .map(|index| (index % 251) as u8)
        .collect();
    let png = ludion_assets::encode_png(ATLAS_WIDTH, ATLAS_HEIGHT, &rgba).unwrap();
    std::fs::write(dir.join("art/fixture.png"), &png).unwrap();
    std::fs::write(
        dir.join("art/fixture.atlas.yaml"),
        ludion_scene::emit_atlas(&doc()),
    )
    .unwrap();
    dir
}

fn engine_at(root: &Path) -> Engine {
    let mut engine = Engine::with_project_root(root);
    engine.load_inline_scene(SCENE).expect("the scene loads");
    engine
}

/// One `sim.step` of `ticks` ticks.
fn step(ticks: u64) -> Command {
    Command::SimStep(command::SimStepParams { ticks, hash: false })
}

/// The sprite instances of `engine`'s world at `tick`, through the public extract entry point.
fn sprites_at(engine: &Engine, tick: u64) -> Vec<SpriteInstance> {
    let sprites = extract_at(
        engine.world(),
        (320, 180),
        ludion_assets::AssetRoot::content(engine.project_root()),
        SimTime::from_ticks(tick, TickRate::DEFAULT),
        QualityParams::FULL.particle_density,
    )
    .expect("the fixture world extracts")
    .sprites;
    assert_eq!(sprites.len(), SPRITES, "every fixture quad places");
    sprites
}

/// The sim half of decision 10: the per-frame `WorldHash` trace and the recorded `.lrp` bytes of a
/// run playing flipbooks are identical whether or not frames are extracted along the way.
///
/// Not vacuous — `a_mid_clip_time_moves_the_frame` proves the quads really do change — so this says
/// the render boundary is a read: a flipbook adds nothing to the world, nothing to the record
/// stream, and nothing to a replay.
#[test]
fn extracting_an_animated_run_changes_neither_its_hashes_nor_its_recording() {
    let root = fixture_project("hash-neutral");

    let run = |tag: &str, extract_each_tick: bool| {
        let lrp = root.join(format!("{tag}.lrp"));
        let mut engine = engine_at(&root);
        engine
            .set_recording(std::fs::File::create(&lrp).unwrap())
            .expect("recording starts");

        let mut trace = Vec::new();
        for _ in 0..40 {
            trace.extend(
                ludion_engine::drive_hashes(&mut engine, vec![step(1)])
                    .expect("the session drives")
                    .into_iter()
                    // `drive_hashes` re-samples the current frame first; keep only the new tick.
                    .skip(1),
            );
            if extract_each_tick {
                let _ = sprites_at(&engine, engine.frame().as_u64());
            }
        }
        engine.stop_recording().expect("recording stops");
        (trace, std::fs::read(&lrp).unwrap())
    };

    let (quiet_trace, quiet_bytes) = run("quiet", false);
    let (drawn_trace, drawn_bytes) = run("drawn", true);

    assert_eq!(quiet_trace.len(), 40, "the run actually advanced 40 frames");
    assert_eq!(
        quiet_trace, drawn_trace,
        "extracting a flipbook frame must not move a single world hash"
    );
    assert_eq!(
        quiet_bytes, drawn_bytes,
        "extracting a flipbook frame must not write a byte into the recording"
    );

    let _ = std::fs::remove_dir_all(&root);
}

/// The evaluator half: a looping clip walks its three regions, one clip frame per 15 ticks, and each
/// step moves the UV rect *and* the quad's extent (the regions are three different shapes). The
/// sprite with no `Animation` never moves, and the loop wraps back onto its first frame.
#[test]
fn a_mid_clip_time_moves_the_frame() {
    let root = fixture_project("mid-clip");
    let engine = engine_at(&root);

    let shown: Vec<SpriteInstance> = (0..4)
        .map(|frame| sprites_at(&engine, frame * TICKS_PER_FRAME)[LOOPER])
        .collect();

    for (index, pair) in shown.windows(2).take(2).enumerate() {
        assert_ne!(
            pair[0].uv,
            pair[1].uv,
            "clip frame {index} to {}",
            index + 1
        );
        assert_ne!(pair[0].right, pair[1].right, "the derived width follows");
        assert_ne!(pair[0].up, pair[1].up, "and so does the derived height");
    }
    assert_eq!(
        shown[0], shown[3],
        "three frames in, a looping clip is back where it started"
    );

    let still = sprites_at(&engine, 0)[STILL];
    for frame in 0..4 {
        assert_eq!(
            sprites_at(&engine, frame * TICKS_PER_FRAME)[STILL],
            still,
            "a sprite with no `Animation` is the same quad at every time"
        );
    }

    let _ = std::fs::remove_dir_all(&root);
}

/// A one-shot clamps: far past its last clip frame it still draws that frame — the region the
/// looping clip shows at its own third frame.
#[test]
fn a_one_shot_clip_holds_its_last_frame() {
    let root = fixture_project("one-shot");
    let engine = engine_at(&root);

    let last = sprites_at(&engine, 2 * TICKS_PER_FRAME)[LOOPER];
    for tick in [4 * TICKS_PER_FRAME, 600] {
        let held = sprites_at(&engine, tick)[ONESHOT];
        assert_eq!(
            held.uv, last.uv,
            "the one-shot holds its last frame at {tick}"
        );
        assert_eq!(held.right, last.right);
        assert_eq!(held.up, last.up);
    }

    let _ = std::fs::remove_dir_all(&root);
}

/// `hold: [1, 2, 1]` stretches the middle frame over two clip frames: the one-shot shows it from
/// tick 15 through tick 44 and only turns over at 45, while the looping clip — same frames, no holds
/// — has already moved on by 30.
#[test]
fn hold_counts_stretch_a_frame() {
    let root = fixture_project("holds");
    let engine = engine_at(&root);

    let held = sprites_at(&engine, TICKS_PER_FRAME)[ONESHOT];
    for tick in [
        TICKS_PER_FRAME + 14,
        2 * TICKS_PER_FRAME,
        3 * TICKS_PER_FRAME - 1,
    ] {
        assert_eq!(
            sprites_at(&engine, tick)[ONESHOT],
            held,
            "the held frame is still showing at tick {tick}"
        );
    }
    assert_ne!(
        sprites_at(&engine, 3 * TICKS_PER_FRAME)[ONESHOT],
        held,
        "and turns over the tick its hold runs out"
    );
    assert_ne!(
        sprites_at(&engine, 2 * TICKS_PER_FRAME)[LOOPER],
        held,
        "the un-held clip over the same frames has already moved on"
    );

    let _ = std::fs::remove_dir_all(&root);
}

/// One sprite playing the one-frame-per-tick `f60` flipbook with `hold: 5`.
const HOLD_SCENE: &str = "\
format: 1
seed: 1
entities:
  - name: held
    components:
      Sprite:
        atlas: art/fixture.atlas.yaml
        frame: wide
      Animation:
        clip: f60
        hold: 5
";

/// `hold: 5` shows a new flipbook frame only on multiples of 5: ticks 11..=14 draw tick 10's frame
/// (`tall`), and tick 15 turns over to `wide`.
#[test]
fn a_hold_steps_the_flipbook_on_a_global_grid() {
    let root = fixture_project("hold-grid");
    let mut engine = Engine::with_project_root(&root);
    engine.load_inline_scene(HOLD_SCENE).expect("scene loads");
    let at = |tick: u64| {
        let sprites = extract_at(
            engine.world(),
            (320, 180),
            ludion_assets::AssetRoot::content(engine.project_root()),
            SimTime::from_ticks(tick, TickRate::DEFAULT),
            QualityParams::FULL.particle_density,
        )
        .expect("the hold world extracts")
        .sprites;
        assert_eq!(sprites.len(), 1);
        sprites[0]
    };
    let held = at(10);
    assert_eq!(held.uv, region_uv(10));
    for tick in 11..=14 {
        assert_eq!(at(tick), held, "tick {tick} still shows tick 10's frame");
    }
    let next = at(15);
    assert_ne!(next, held, "tick 15 turns over");
    assert_eq!(next.uv, region_uv(15));
    let _ = std::fs::remove_dir_all(&root);
}

/// A clip name the atlas binds no flipbook to is a silent degrade to the authored frame —
/// byte-identical to the sprite carrying no `Animation` at all, and never a diagnostic (the extract
/// above would have returned `Err`). Clip names are chosen at runtime, so an unknown one cannot be
/// an error.
#[test]
fn an_unbound_clip_name_shows_the_authored_frame() {
    let root = fixture_project("unbound");
    let engine = engine_at(&root);

    for tick in [0, TICKS_PER_FRAME, 7 * TICKS_PER_FRAME] {
        let sprites = sprites_at(&engine, tick);
        assert_eq!(
            sprites[UNBOUND], sprites[STILL],
            "an unbound clip draws the authored frame at tick {tick}"
        );
    }

    let _ = std::fs::remove_dir_all(&root);
}

/// `speed: 0.0` freezes a clip on its first frame — the authored one here, forever.
#[test]
fn a_speed_of_zero_freezes_the_first_frame() {
    let root = fixture_project("frozen");
    let engine = engine_at(&root);

    for tick in [0, TICKS_PER_FRAME, 40 * TICKS_PER_FRAME] {
        let sprites = sprites_at(&engine, tick);
        assert_eq!(
            sprites[FROZEN], sprites[STILL],
            "a frozen clip never leaves its first frame (tick {tick})"
        );
    }

    let _ = std::fs::remove_dir_all(&root);
}

/// The uv rect of the region `frames[index % 3]` names, over the fixture image.
fn region_uv(index: u64) -> [f32; 4] {
    let name = ["wide", "tall", "dot"][(index % 3) as usize];
    let [x, y, w, h] = doc().regions[name].rect.map(|v| v as f32);
    let (iw, ih) = (ATLAS_WIDTH as f32, ATLAS_HEIGHT as f32);
    [x / iw, y / ih, (x + w) / iw, (y + h) / ih]
}

/// Checks the three rate sprites at `tick` against `frames[(num * fps / den) % 3]` in integers.
fn assert_rates_at(engine: &Engine, tick: u64, num: u64, den: u64) {
    let instances = extract_at(
        engine.world(),
        (320, 180),
        ludion_assets::AssetRoot::content(engine.project_root()),
        SimTime::from_ticks(tick, TickRate::DEFAULT),
        QualityParams::FULL.particle_density,
    )
    .expect("the rates world extracts")
    .sprites;
    for (instance, fps) in instances.iter().zip([15, 30, 60]) {
        assert_eq!(
            instance.uv,
            region_uv(num * fps / den),
            "{fps} fps at tick {tick} (clip ticks {num}/{den})"
        );
    }
}

fn rates_engine(root: &Path) -> Engine {
    let mut engine = Engine::with_project_root(root);
    engine.load_inline_scene(RATES_SCENE).expect("scene loads");
    engine
}

/// A frame boundary that lands on a tick turns over on that tick: 15, 30 and 60 fps at 60 Hz, every
/// tick of five seconds.
#[test]
fn flipbook_frames_are_exact_on_every_tick() {
    let root = fixture_project("every-tick");
    let engine = rates_engine(&root);
    for tick in 0..=300 {
        assert_rates_at(&engine, tick, tick, 60);
    }
    let _ = std::fs::remove_dir_all(&root);
}

/// A `set_clip_speed` anchor keeps the boundary exact: from tick 62 at half speed the clip has counted
/// `tick + 62` half-ticks, so its frame is `(tick + 62) * fps / 120`.
#[test]
fn an_anchored_flipbook_is_exact_across_set_speed() {
    let root = fixture_project("anchored");
    let mut engine = rates_engine(&root);
    for (_, (animation,)) in engine
        .world_mut()
        .query_ids::<(&mut ludion_engine::Animation,)>()
    {
        ludion_engine::set_clip_speed(animation, 0.5, SimTime::from_ticks(62, TickRate::DEFAULT));
    }
    for tick in 62..=362 {
        assert_rates_at(&engine, tick, tick + 62, 120);
    }
    let _ = std::fs::remove_dir_all(&root);
}
