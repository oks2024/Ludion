//! Gates `Plan/bullet-hell-stress.md` and `Plan/projectile-patterns.md` Wave 2: the fast,
//! ordinary-lane half of the arc's proof obligation. Runs under the workspace's default
//! `cargo nextest run --workspace` (no `--release`, no `--ignored`) — the *measurement* half (frame
//! cost) is `tests/profile.rs`, ignored by design.
//!
//! Each of the four scenes must hold two properties: **determinism** (two independent drives over
//! the same ticks produce identical `WorldHash` traces — there is no RNG anywhere in this crate, so
//! any divergence is a bug, not sampling noise) and **the closed-form census** each scene's own
//! header derives (`steady`/`steady-drawn`/`steady-indexed`: `live(N) = 25 * min(N+1, 119)`;
//! `burst`: `live(N) = 50*(N+1)` for `N<60`, `0` on `N%60==0` (`N>0`), else `50*(N%60)`).
//!
//! **The census counts BULLETS, not entities**, which is the whole point of the migration: in an
//! entity scene one live bullet is one entity with a [`shmup_stress::Bullet`]; in a volley scene it
//! is one index of a `Volley` that [`bullet_is_alive`] says is on the field. [`live_bullets`] below
//! evaluates both, so one closed form gates both shapes — and
//! [`steady_and_steady_drawn_agree_frame_by_frame`] asserts the two shapes agree at *every* frame,
//! which is what makes their profile reports an A-versus-B rather than two unrelated numbers.
//!
//! `steady-drawn` adds the same discipline to what the field *shows*: the regions it draws from and
//! the share of it that composites additively are closed forms too, which is the only way a
//! measurement scene can promise "3,000 sprites over 40 regions, 200 of them additive" without a run
//! being asked what it happened to produce.
//!
//! **Frame convention**, matching the scene headers: a census is always read *after* the frame's
//! systems have all run (`spatial_rebuild` -> `play_patterns` -> `emit_bullets` -> `move_bullets` ->
//! `advance_volleys` -> `cancel_bullets` -> `player_hit_test` -> `scroll_backdrop`), i.e. "the live
//! count at the end of frame N" — never mid-frame. `Engine::frame` counts **ticks already
//! executed**, not the index of the one that just ran (the tick that runs with `ctx.frame == N` is
//! the engine's `(N + 1)`th), so `end of frame N` requires `N + 1` executed ticks — see `step_to`
//! below, and the `SimTime` [`live_bullets`] evaluates volleys at is frame `N`'s, the one the last
//! tick's systems saw.
//!
//! Every scene is built **from its file**, never from an inlined copy: a scene's own directory is
//! the content root, and both the atlas `steady-drawn` samples and the `kind: pattern` documents
//! `steady`/`burst` play resolve through it.

use std::collections::BTreeSet;
use std::path::Path;

use ludion::prelude::*;

/// Ticks driven by the determinism check and the top of every census sample list — enough to reach
/// deep into `burst`'s third cancel cycle (frame 180) while costing well under a second per scene at
/// these scenes' well-under-1-ms-per-frame sim time.
const FRAMES: u64 = 200;

/// Builds a fresh engine over `scenes/<name>.yaml`, resolved against this crate's own directory
/// (cargo runs an integration test with the package root as its working directory).
fn build(name: &str) -> Engine {
    shmup_stress::game()
        .load_scene_file(Path::new(&format!("scenes/{name}.yaml")))
        .expect("the scene loads against the game registry")
}

/// The live-bullet census in `engine`'s current world at the end of frame `n`, **over both shapes**:
/// one per entity carrying a [`shmup_stress::Bullet`], plus every index of every [`Volley`] that
/// [`bullet_is_alive`] places on the field at frame `n`'s own [`SimTime`].
///
/// A scene is one shape or the other, so exactly one of the two terms is ever non-zero — but the
/// function does not care which, which is what lets one closed form gate both.
fn live_bullets(engine: &mut Engine, n: u64) -> u64 {
    let now = SimTime::from_ticks(n, TickRate::DEFAULT);
    let world = engine.world_mut();
    let ids: Vec<EntityId> = world.entities().collect();
    let store = world.resource::<DocStore<PatternDoc>>();
    let mut live = 0u64;
    for id in ids {
        if world.has::<shmup_stress::Bullet>(id) {
            live += 1;
            continue;
        }
        let Some(volley) = world.get::<Volley>(id).ok().flatten() else {
            continue;
        };
        let Some(kind) = store
            .and_then(|store| store.get(&volley.pattern))
            .and_then(|doc| doc.bullets.get(&volley.bullet))
        else {
            continue;
        };
        live += (0..volley.count)
            .filter(|&index| bullet_is_alive(volley, kind, index, now))
            .count() as u64;
    }
    live
}

/// How many [`Volley`] entities hold that census — the migration's other half: `steady` carries the
/// same 2,975 bullets in 119 entities that `steady-drawn` needs 2,975 for.
fn volley_entities(engine: &mut Engine) -> u64 {
    let world = engine.world_mut();
    world.entities().filter(|&e| world.has::<Volley>(e)).count() as u64
}

/// The live bullet count restricted to entities that also carry a [`Collider`] — the
/// `steady-indexed` scene's extra assertion (every bullet is spawned with one once
/// `Arena.bullets_collide` is set).
fn bullet_with_collider_count(engine: &mut Engine) -> u64 {
    let world = engine.world_mut();
    world
        .entities()
        .filter(|&e| world.has::<shmup_stress::Bullet>(e) && world.has::<Collider>(e))
        .count() as u64
}

/// The hit tally, which both shapes write into.
fn hit_count(engine: &mut Engine) -> u64 {
    let world = engine.world_mut();
    world
        .single_get::<shmup_stress::HitCount>()
        .expect("the scene has a HitCount singleton")
        .hits
}

/// Steps `engine` so that its world reflects the state **at the end of frame `target`** (the scene
/// headers' `live(N)` convention), updating `*ticks_run` — the count of ticks already executed —
/// to match.
///
/// `Engine::frame()`/`ctx.frame` count **ticks executed so far**: the tick that runs with
/// `ctx.frame == N` is the `(N + 1)`th tick, so reaching "the end of frame `target`" means the
/// engine must have executed `target + 1` ticks in total, not `target`.
fn step_to(engine: &mut Engine, ticks_run: &mut u64, target: u64) {
    let want = target + 1;
    let ticks = want - *ticks_run;
    engine
        .execute(Command::SimStep(command::SimStepParams {
            ticks,
            hash: false,
        }))
        .expect("sim steps");
    *ticks_run = want;
}

/// `steady.yaml` / `steady-drawn.yaml` / `steady-indexed.yaml`'s closed form:
/// `live(N) = 25 * min(N + 1, 119)`.
fn steady_live(n: u64) -> u64 {
    25 * (n + 1).min(119)
}

/// `steady-drawn.yaml`'s additive share: `BulletArt.additive_every: 12` marks ring indices 0, 12 and
/// 24 of each 25-bullet volley, so three of every volley that is still alive.
fn drawn_additive(n: u64) -> u64 {
    3 * (n + 1).min(119)
}

/// The number of distinct atlas regions `steady-drawn.yaml`'s bullet field is drawn from, and how
/// many of its bullets composite additively.
fn drawn_census(engine: &mut Engine) -> (usize, u64) {
    let world = engine.world_mut();
    let mut frames = BTreeSet::new();
    let mut additive = 0;
    for (_, sprite) in world.query::<(&shmup_stress::Bullet, &Sprite)>() {
        if let Some(frame) = sprite.frame.as_deref() {
            frames.insert(frame.to_string());
        }
        if sprite.blend == SpriteBlend::Additive {
            additive += 1;
        }
    }
    (frames.len(), additive)
}

/// The backdrop grid's position along the arena plane's vertical axis.
fn backdrop_y(engine: &mut Engine) -> f32 {
    let world = engine.world_mut();
    world
        .query::<(&shmup_stress::Backdrop, &Transform)>()
        .map(|(_, transform)| transform.translation[1])
        .next()
        .expect("the scene has a backdrop")
}

/// `burst.yaml`'s closed form, from its header: a sawtooth that resets to zero on every cancel frame
/// (a multiple of 60, frame 0 exempt) and otherwise counts up from the last cancel.
fn burst_live(n: u64) -> u64 {
    if n < 60 {
        50 * (n + 1)
    } else if n.is_multiple_of(60) {
        0
    } else {
        50 * (n % 60)
    }
}

/// Drives two independent engines over `scenes/<name>.yaml` for [`FRAMES`] ticks and asserts their
/// per-tick `WorldHash` traces are identical — the field's spawn/reap/cancel timing must be a pure
/// function of the frame index alone.
fn assert_deterministic(name: &str) {
    let run = || {
        let mut engine = build(name);
        ludion_engine::drive_hashes(
            &mut engine,
            vec![Command::SimStep(command::SimStepParams {
                ticks: FRAMES,
                hash: false,
            })],
        )
        .expect("drive succeeds")
    };
    assert_eq!(
        run(),
        run(),
        "the scene must simulate deterministically — no RNG or wall clock anywhere in this crate"
    );
}

#[test]
fn steady_is_deterministic_and_holds_its_census() {
    assert_deterministic("steady");

    let mut engine = build("steady");
    let mut frame = 0u64;
    // Before the first full ramp, exactly at it (frame 118), one tick past it (still full — the
    // "constant from there on" claim), and late in the steady state.
    for &n in &[0, 50, 118, 119, 199] {
        step_to(&mut engine, &mut frame, n);
        assert_eq!(
            live_bullets(&mut engine, n),
            steady_live(n),
            "live census at the end of frame {n}"
        );
    }

    // The migration's headline: one entity per volley, not per bullet. Four sources, one ring every
    // four frames each, 119 frames of life — 119 volleys hold all 2,975 bullets.
    assert_eq!(
        volley_entities(&mut engine),
        119,
        "the full field is held by 119 Volley entities"
    );

    // The hit test is live on the volley half too: the player sits at the arena centre, where every
    // source's ring converges, so over 200 frames `volley_hits` must have found overlaps.
    assert!(
        hit_count(&mut engine) > 0,
        "the volley hit test never registered a hit over {FRAMES} frames"
    );
}

#[test]
fn burst_is_deterministic_and_cancels_whole() {
    assert_deterministic("burst");

    let mut engine = build("burst");
    let mut frame = 0u64;
    // The pre-cancel peak, the cancel frame itself (zero), the first tick of the refill, and the
    // same triple one cancel cycle later.
    for &n in &[59, 60, 61, 119, 120, 179] {
        step_to(&mut engine, &mut frame, n);
        assert_eq!(
            live_bullets(&mut engine, n),
            burst_live(n),
            "live census at the end of frame {n}"
        );
    }

    // The mass despawn priced: the 3,000-bullet field a cancel destroys is 60 entities here, and
    // the frame after a cancel holds exactly the one volley the refill has stood up.
    step_to(&mut engine, &mut frame, 239);
    assert_eq!(
        volley_entities(&mut engine),
        59,
        "the 59 volleys standing one frame before the next cancel hold 2,950 bullets"
    );
}

#[test]
fn steady_indexed_matches_steady_census_with_colliders() {
    assert_deterministic("steady-indexed");

    let mut engine = build("steady-indexed");
    let mut frame = 0u64;
    for &n in &[0, 50, 118, 119, 199] {
        step_to(&mut engine, &mut frame, n);
        let expected = steady_live(n);
        assert_eq!(
            live_bullets(&mut engine, n),
            expected,
            "live census (identical to `steady-drawn.yaml`) at the end of frame {n}"
        );
        assert_eq!(
            bullet_with_collider_count(&mut engine),
            expected,
            "every live bullet carries a Collider once `bullets_collide` is set (end of frame {n})"
        );
    }
}

#[test]
fn steady_drawn_matches_steady_census_over_forty_atlas_regions() {
    assert_deterministic("steady-drawn");

    let mut engine = build("steady-drawn");
    let mut frame = 0u64;
    for &n in &[0, 50, 118, 119, 199] {
        step_to(&mut engine, &mut frame, n);
        let expected = steady_live(n);
        assert_eq!(
            live_bullets(&mut engine, n),
            expected,
            "live census (identical to `steady.yaml`) at the end of frame {n}"
        );

        let (regions, additive) = drawn_census(&mut engine);
        assert_eq!(
            additive,
            drawn_additive(n),
            "additive share at the end of frame {n}"
        );
        // The exit line asks for 40 distinct regions of one atlas and 200 additive bullets.
        // Both are properties of the FULL field, so they are only claimed once it is full.
        if n >= 118 {
            assert_eq!(
                regions, 40,
                "the full field draws every one of the atlas's 40 bullet regions (frame {n})"
            );
            assert!(
                additive >= 200,
                "the full field holds at least 200 additive bullets, not {additive} (frame {n})"
            );
        }
    }

    // The backdrop's position is a closed form of the frame index, not an accumulation: 2.0 u/s for
    // `frame % 120` frames at the 60 Hz tick, back to its origin on every 120th.
    for &(n, expected) in &[
        (0u64, 10.0f32),
        (59, 10.0 - 2.0 * 59.0 / 60.0),
        (119, 10.0 - 2.0 * 119.0 / 60.0),
        (120, 10.0),
    ] {
        let mut engine = build("steady-drawn");
        let mut frame = 0u64;
        step_to(&mut engine, &mut frame, n);
        let y = backdrop_y(&mut engine);
        assert!(
            (y - expected).abs() < 1e-4,
            "backdrop at the end of frame {n}: {y} vs the closed form's {expected}"
        );
    }
}

/// **The A-versus-B gate.** `steady` (volleys) and `steady-drawn` (one entity per bullet) are the
/// same measurement taken twice, so their reports are only comparable while their fields are the
/// same size at *every* frame — not merely at the five the census tests sample.
///
/// Frame 130 is past the ramp (118) and a full cancel period beyond it, so it covers the fill, the
/// boundary and the steady state.
#[test]
fn steady_and_steady_drawn_agree_frame_by_frame() {
    let mut volleys = build("steady");
    let mut entities = build("steady-drawn");
    let (mut volley_frame, mut entity_frame) = (0u64, 0u64);

    for n in 0..=130 {
        step_to(&mut volleys, &mut volley_frame, n);
        step_to(&mut entities, &mut entity_frame, n);
        let (a, b) = (
            live_bullets(&mut volleys, n),
            live_bullets(&mut entities, n),
        );
        assert_eq!(
            a, b,
            "the two shapes must hold the same field at the end of frame {n}"
        );
        assert_eq!(a, steady_live(n), "both shapes hold the closed form at {n}");
    }
}

#[test]
fn committed_scenes_respell_nothing_on_save() {
    ludion_testkit::assert_saves_respell_nothing(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("scenes"),
        &["."],
        shmup_stress::game(),
    );
}
