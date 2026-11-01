//! The host quality setting on the `Engine`: it is render-only state, so it must move no sim bit,
//! and it must be visible to a client through `engine.describe`.
//!
//! No GPU — quality reaches the renderer at construction (`gpu_render.rs` covers what it does to
//! pixels); what is testable here is everything the setting must *not* touch, plus the report.

use std::path::Path;

use ludion_core::time::{SimTime, TickRate};
use ludion_engine::{Engine, Quality, QualityParams, QualitySetting, RenderScale};
use ludion_protocol::{Command, ResponseBody};

/// A scene with content to move: the hash sequence below is meaningless over an empty world.
const SCENE: &str = "format: 1\nseed: 7\nentities:\n  - components:\n      Transform:\n        translation:\n          - 1.0\n          - 2.0\n          - 3.0\n";

fn engine(setting: Option<QualitySetting>) -> Engine {
    let mut engine = Engine::new();
    engine.load_inline_scene(SCENE).expect("scene loads");
    if let Some(setting) = setting {
        engine.set_quality(setting);
    }
    engine
}

/// The quality setting never enters the world: an engine told to render nothing optional produces
/// the same `WorldHash` at every frame as one that was never told anything. Two machines at
/// different qualities run the same simulation and differ only in pixels.
#[test]
fn quality_never_affects_the_world_hash() {
    fn run(setting: Option<QualitySetting>) -> Vec<u64> {
        let mut engine = engine(setting);
        let mut hashes = vec![engine.world_hash().unwrap().as_u64()];
        for _ in 0..5 {
            engine.advance_live();
            hashes.push(engine.world_hash().unwrap().as_u64());
        }
        hashes
    }
    let low = QualitySetting {
        quality: Quality::LOW,
        params: QualityParams::LOW,
        label: "low".to_string(),
    };
    assert_eq!(
        run(Some(low)),
        run(None),
        "quality must not perturb the WorldHash sequence"
    );
}

/// An engine that was never told a quality renders everything the scene authors and says so.
#[test]
fn the_default_quality_is_full() {
    let engine = engine(None);
    assert_eq!(engine.quality(), &QualitySetting::default());
    assert_eq!(engine.quality().quality, Quality::FULL);
    assert_eq!(engine.quality().label, "full");
}

/// `engine.describe` carries this host's quality: the label of the choice plus the toggles it
/// resolved to, so a client can read what the machine renders.
#[test]
fn describe_carries_the_default_quality() {
    let mut engine = engine(None);
    match dispatched(&mut engine) {
        ResponseBody::EngineDescription { description } => {
            let quality = &description["quality"];
            assert_eq!(quality["preset"], "full");
            assert_eq!(quality["ssao"], true);
            assert_eq!(quality["shafts"], true);
            assert_eq!(quality["dof"], true);
            assert_eq!(quality["gi_dynamic"], true);
            assert_eq!(quality["shadows"], true);
        }
        other => panic!("{other:?}"),
    }
}

/// A set quality reaches the report with its own label — including a label no preset produces, which
/// is the reason the setting carries one at all.
#[test]
fn describe_carries_a_set_quality_and_its_label() {
    let mut engine = engine(Some(QualitySetting {
        quality: Quality {
            ssao: false,
            dof: false,
            ..Quality::FULL
        },
        params: QualityParams::MEDIUM,
        label: QualitySetting::CUSTOM_LABEL.to_string(),
    }));
    match dispatched(&mut engine) {
        ResponseBody::EngineDescription { description } => {
            let quality = &description["quality"];
            assert_eq!(quality["preset"], "custom");
            assert_eq!(quality["ssao"], false);
            assert_eq!(quality["shafts"], true);
            assert_eq!(quality["dof"], false);
            assert_eq!(quality["gi_dynamic"], true);
            assert_eq!(quality["shadows"], true);
        }
        other => panic!("{other:?}"),
    }
}

/// A quality that draws below native supplies the host's render scale, and an explicit one beats it
/// — **in either setting order**. The two live as separate values resolved on read, so neither
/// setter can overwrite the other's; a host that configures itself in the order its flags happened
/// to parse in gets the same frame as one that does it the other way round.
#[test]
fn the_render_scale_is_the_explicit_one_else_the_qualitys_whatever_order_they_are_set_in() {
    let low = || QualitySetting {
        quality: Quality::LOW,
        params: QualityParams::LOW,
        label: "low".to_string(),
    };
    let explicit = RenderScale::new(2.0).expect("an in-range factor");

    assert!(
        engine(None).render_scale().is_native(),
        "a machine that was never told anything draws every output pixel",
    );
    assert_eq!(
        engine(Some(low())).render_scale().factor(),
        QualityParams::LOW.render_scale,
        "`low` draws below native, and nothing else had to be said for it to",
    );

    let mut quality_first = engine(None);
    quality_first.set_quality(low());
    quality_first.set_render_scale(explicit);

    let mut scale_first = engine(None);
    scale_first.set_render_scale(explicit);
    scale_first.set_quality(low());

    assert_eq!(quality_first.render_scale(), explicit);
    assert_eq!(scale_first.render_scale(), quality_first.render_scale());
}

/// **The particle axis reaches the frame, and it is the only place it reaches.** `particle_density`
/// is spent in extract rather than in `ludion-renderer`, so the only observable it has is how many
/// particles a `RenderScene` carries — and the gallery's committed three-emitter scene is where that
/// is counted, on real authored rates rather than a fixture's.
///
/// The quarter is approximate on purpose: each emitter's live window is derived from its own thinned
/// rate and rounds independently, so three emitters give three roundings and the total lands *near*
/// a quarter rather than on it. Asserting the exact count would pin the emitters' authored numbers
/// instead of the axis. `low > 0` is the half that keeps this from passing vacuously — the plumes go
/// sparser, never silent.
///
/// The density is read back off the engine's own resolved setting, which is the expression the
/// engine's cached extract passes: what is under test is that a machine's `low` reaches the frame's
/// particle list at all, and the setting is the only place it can come from. The world is never
/// ticked, so tick 0 is its time — and the gallery's three emitters are endless ones, already in
/// steady state there.
#[test]
fn low_quality_thins_the_particle_population() {
    fn particles(setting: Option<QualitySetting>) -> usize {
        let path =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/gallery/particles.yaml");
        let mut engine = Engine::with_project_root(path.parent().expect("a scene has a parent"));
        let source = std::fs::read_to_string(&path).expect("the committed gallery scene reads");
        engine
            .load_inline_scene(&source)
            .expect("the committed gallery scene validates");
        if let Some(setting) = setting {
            engine.set_quality(setting);
        }
        ludion_engine::extract_at(
            engine.world(),
            (960, 540),
            ludion_assets::AssetRoot::content(engine.project_root()),
            SimTime::from_ticks(0, TickRate::DEFAULT),
            engine.quality().params.particle_density,
        )
        .expect("the committed gallery scene extracts")
        .particles
        .len()
    }

    let full = particles(None);
    let low = particles(Some(QualitySetting {
        quality: Quality::LOW,
        params: QualityParams::LOW,
        label: "low".to_string(),
    }));
    assert!(
        full > 0,
        "the gallery's three endless emitters are in steady state at frame 0, so the full \
         population is non-empty or this test measures nothing",
    );
    assert!(
        low > 0 && low < full,
        "`low` must thin the plumes rather than silence or keep them: {low} of {full}",
    );
    let share = low as f64 / full as f64;
    assert!(
        (0.2..=0.3).contains(&share),
        "`low` emits {share:.3} of the authored population ({low} of {full}), not the quarter \
         `QualityParams::LOW.particle_density` asks for",
    );
}

fn dispatched(engine: &mut Engine) -> ResponseBody {
    ludion_engine::dispatch(engine, Command::EngineDescribe)
        .expect("engine.describe answers")
        .body
}
