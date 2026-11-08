//! `CameraOffset`: a presentation entity moves the rendered camera and nothing the sim sees. Every
//! case extracts through `Engine::extract_present_scene` (the scene a host presents) and none needs
//! a GPU: the acceptance is that a scene with an offset extracts exactly what the same scene with the
//! camera moved by hand does.

use ludion_core::codes::code;
use ludion_core::ids::EntityId;
use ludion_ecs::World;
use ludion_engine::{
    Camera, CameraOffset, Diagnostics, Engine, Name, Projection, Transform, dispatch,
};
use ludion_protocol::command::{RenderViewParams, ViewPose};
use ludion_protocol::{Command, ResponseBody};
use ludion_renderer::RenderScene;

/// The rig's camera position and the offset the acceptance composes onto it. Every value is exact
/// in `f32`, so the composed pose is bit-exact.
const EYE: [f32; 3] = [1.0, 2.0, 10.0];
const SHIFT: [f32; 3] = [0.5, -0.25, 4.0];
const COMPOSED: [f32; 3] = [1.5, 1.75, 14.0];

const BODY: &str = "  - name: board
    components:
      Transform:
        translation:
          - 0.0
          - 1.0
          - 0.0
      Sprite:
        billboard: true
  - name: label
    components:
      Transform:
        translation:
          - 2.0
          - 0.0
          - 0.0
      Text:
        text: hello
  - name: brazier
    components:
      Transform: {}
      ParticleEmitter:
        rate: 30.0
        life:
          - 1.0
          - 2.0
        speed:
          - 0.5
          - 1.5
        gravity: -1.0
        seed: 11
  - name: crate
    components:
      Transform:
        translation:
          - -2.0
          - 0.0
          - 0.0
      Mesh:
        shape: cube
";

fn scene(camera: &str, extra: &str) -> String {
    format!("format: 1\nseed: 3\nentities:\n  - name: cam\n    components:\n{camera}{BODY}{extra}")
}

fn camera_block(eye: [f32; 3], fov: f32) -> String {
    format!(
        "      Transform:\n        translation:\n          - {:?}\n          - {:?}\n          - {:?}\n      Camera:\n        projection:\n          type: perspective\n          fov_y_deg: {fov:?}\n",
        eye[0], eye[1], eye[2]
    )
}

fn offset_entity(fields: &str) -> String {
    format!(
        "  - name: intro\n    presentation: true\n    components:\n      CameraOffset:{fields}\n",
    )
}

const PULL_IN: &str = "\n        translation:\n          - 0.5\n          - -0.25\n          - 4.0\n        fov_delta: -10.0";

fn engine_for(document: &str) -> Engine {
    let mut engine = Engine::builder().build().expect("the game builds");
    engine.load_inline_scene(document).expect("the scene loads");
    engine
}

fn extract(engine: &mut Engine) -> RenderScene {
    engine.advance_live();
    engine.extract_present_scene().expect("extracts").0
}

fn named(world: &World, wanted: &str) -> EntityId {
    world
        .entities()
        .chain(world.presentation_entities())
        .find(|&id| {
            world
                .get::<Name>(id)
                .ok()
                .flatten()
                .is_some_and(|name| name.0 == wanted)
        })
        .unwrap_or_else(|| panic!("the scene has a `{wanted}`"))
}

fn reported(engine: &Engine, code: ludion_core::diagnostics::DiagnosticCode) -> Vec<String> {
    engine
        .world()
        .resource::<Diagnostics>()
        .map(|d| d.reported(code).map(str::to_string).collect())
        .unwrap_or_default()
}

fn baseline() -> RenderScene {
    extract(&mut engine_for(&scene(&camera_block(EYE, 60.0), "")))
}

/// T2, the acceptance: the offset scene and the hand-moved camera extract the same `RenderScene`
/// (billboard sprite, world text, particles and a mesh included), and the baseline differs.
#[test]
fn an_offset_extracts_what_a_hand_moved_camera_does() {
    let offset = extract(&mut engine_for(&scene(
        &camera_block(EYE, 60.0),
        &offset_entity(PULL_IN),
    )));
    let moved = extract(&mut engine_for(&scene(&camera_block(COMPOSED, 50.0), "")));
    assert_eq!(offset, moved);
    assert_ne!(offset.camera, baseline().camera);
    assert!(!offset.sprites.is_empty() && !offset.particles.is_empty());
}

/// A world without the component, and one with `CameraOffset: {}`, extract the baseline.
#[test]
fn no_offset_and_an_identity_offset_extract_the_baseline() {
    let identity = extract(&mut engine_for(&scene(
        &camera_block(EYE, 60.0),
        &offset_entity(" {}"),
    )));
    assert_eq!(identity, baseline());
}

/// T3: a presentation scene saves the offset back, defaults omitted, and reloads it.
#[test]
fn a_scene_round_trips_the_offset_with_defaults_omitted() {
    let root = std::env::temp_dir().join(format!("ludion-camera-offset-{}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    let mut engine = Engine::with_project_root(&root);
    engine
        .load_inline_scene(&scene(
            &camera_block(EYE, 60.0),
            &offset_entity("\n        fov_delta: -5.0"),
        ))
        .unwrap();
    dispatch(
        &mut engine,
        Command::SceneSave(ludion_protocol::command::SceneSaveParams {
            path: "out.yaml".into(),
            force: false,
        }),
    )
    .unwrap();
    let saved = std::fs::read_to_string(root.join("out.yaml")).unwrap();
    assert!(saved.contains("presentation: true"), "{saved}");
    assert!(saved.contains("fov_delta: -5.0"), "{saved}");
    assert!(!saved.contains("rotation_deg"), "{saved}");
    let mut reloaded = Engine::with_project_root(&root);
    reloaded.load_inline_scene(&saved).unwrap();
    let id = named(reloaded.world(), "intro");
    assert_eq!(
        reloaded.world().get::<CameraOffset>(id).unwrap().unwrap(),
        &CameraOffset {
            fov_delta: -5.0,
            ..CameraOffset::default()
        }
    );
}

/// A runtime `CameraOffset` on a sim entity is never applied; it reports `L1333` once.
#[test]
fn an_offset_on_a_sim_entity_is_ignored_and_reports_l1333() {
    let mut engine = engine_for(&scene(&camera_block(EYE, 60.0), ""));
    let crate_id = named(engine.world(), "crate");
    engine
        .world_mut()
        .insert(
            crate_id,
            CameraOffset {
                translation: SHIFT,
                ..CameraOffset::default()
            },
        )
        .unwrap();
    assert_eq!(extract(&mut engine), baseline());
    extract(&mut engine);
    assert_eq!(
        reported(&engine, code::CAMERA_OFFSET_ON_SIM_ENTITY).len(),
        1
    );
}

/// Two offsets on presentation entities: the lowest id wins and the second reports `L1334` once.
#[test]
fn the_lowest_of_two_offsets_wins_and_the_second_reports_l1334() {
    let mut engine = engine_for(&scene(&camera_block(EYE, 60.0), &offset_entity(PULL_IN)));
    let first = named(engine.world(), "intro");
    let second = engine.world_mut().spawn_presentation();
    assert!(second > first);
    engine
        .world_mut()
        .insert(
            second,
            CameraOffset {
                translation: [9.0, 9.0, 9.0],
                ..CameraOffset::default()
            },
        )
        .unwrap();
    let moved = extract(&mut engine_for(&scene(&camera_block(COMPOSED, 50.0), "")));
    assert_eq!(extract(&mut engine), moved);
    extract(&mut engine);
    assert_eq!(reported(&engine, code::CAMERA_OFFSET_DUPLICATE).len(), 1);
}

/// A non-finite runtime offset is ignored whole: the rig's pose is drawn, `L1332` once.
#[test]
fn a_non_finite_offset_is_ignored_and_reports_l1332() {
    let mut engine = engine_for(&scene(&camera_block(EYE, 60.0), &offset_entity(PULL_IN)));
    let intro = named(engine.world(), "intro");
    engine
        .world_mut()
        .get_mut::<CameraOffset>(intro)
        .unwrap()
        .unwrap()
        .translation[1] = f32::NAN;
    assert_eq!(extract(&mut engine), baseline());
    extract(&mut engine);
    assert_eq!(reported(&engine, code::CAMERA_OFFSET_INVALID).len(), 1);
}

/// A scene with a non-finite offset field is refused at the field (the schema's f32-range check,
/// `L1014`, answers before the component validator's `L1332`).
#[test]
fn a_scene_with_a_non_finite_offset_is_refused() {
    let mut engine = Engine::builder().build().unwrap();
    let errors = engine
        .load_inline_scene(&scene(
            &camera_block(EYE, 60.0),
            &offset_entity(
                "
        translation:
          - 0.0
          - 1.0e300
          - 0.0",
            ),
        ))
        .expect_err("the scene is refused");
    assert!(
        errors.iter().any(|d| d
            .path
            .as_deref()
            .is_some_and(|p| p.ends_with("/CameraOffset/translation/1"))),
        "{errors:?}"
    );
}

/// `scene.check` and `check-scene` refuse an offset on a sim entity (`L1333`) and name the second
/// presentation offset (`L1334`).
#[test]
fn scene_check_reports_a_sim_offset_and_a_duplicate() {
    let document = scene(
        &camera_block(EYE, 60.0),
        &format!(
            "{}{}  - name: bad
    components:
      CameraOffset: {{}}
",
            offset_entity(" {}"),
            offset_entity(" {}")
        ),
    );
    let parsed = ludion_scene::check(&document, &ludion_engine::builtin_registry()).expect("valid");
    let found = ludion_engine::scene_engine_diagnostics(std::path::Path::new("."), &parsed);
    let codes: Vec<_> = found.iter().map(|d| d.code).collect();
    assert!(
        codes.contains(&code::CAMERA_OFFSET_ON_SIM_ENTITY),
        "{found:?}"
    );
    assert!(codes.contains(&code::CAMERA_OFFSET_DUPLICATE), "{found:?}");
}

/// A `fov_delta` past the range clamps to 179 degrees and reports the `L1335` warning once.
#[test]
fn a_fov_delta_out_of_range_clamps_with_one_warning() {
    let mut engine = engine_for(&scene(
        &camera_block(EYE, 60.0),
        &offset_entity("\n        fov_delta: 400.0"),
    ));
    let clamped = extract(&mut engine);
    let max = extract(&mut engine_for(&scene(&camera_block(EYE, 179.0), "")));
    assert_eq!(clamped, max);
    extract(&mut engine);
    assert_eq!(reported(&engine, code::CAMERA_OFFSET_FOV_CLAMPED).len(), 1);
    assert!(reported(&engine, code::CAMERA_OFFSET_INVALID).is_empty());
    assert_eq!(
        ludion_core::codes::severity(code::CAMERA_OFFSET_FOV_CLAMPED),
        ludion_core::diagnostics::Severity::Warning
    );
}

/// An orthographic camera ignores `fov_delta`, and still moves.
#[test]
fn an_orthographic_camera_ignores_fov_delta_and_moves() {
    let ortho = |eye: [f32; 3]| {
        format!(
            "      Transform:\n        translation:\n          - {:?}\n          - {:?}\n          - {:?}\n      Camera:\n        projection:\n          type: orthographic\n          height: 8.0\n",
            eye[0], eye[1], eye[2]
        )
    };
    let offset = extract(&mut engine_for(&scene(
        &ortho(EYE),
        &offset_entity(PULL_IN),
    )));
    let moved = extract(&mut engine_for(&scene(&ortho(COMPOSED), "")));
    assert_eq!(offset, moved);
    assert_eq!(
        offset.camera.projection,
        baseline_ortho(&ortho(EYE)).camera.projection
    );
}

fn baseline_ortho(camera: &str) -> RenderScene {
    extract(&mut engine_for(&scene(camera, "")))
}

/// With no camera the offset does nothing: the default framing is untouched.
#[test]
fn an_offset_without_a_camera_is_inert() {
    let bare = |extra: &str| format!("format: 1\nseed: 3\nentities:\n{BODY}{extra}");
    let with = extract(&mut engine_for(&bare(&offset_entity(PULL_IN))));
    let without = extract(&mut engine_for(&bare("")));
    assert_eq!(with, without);
}

/// `render.view` replaces the viewpoint: the offset does not touch the override camera.
#[test]
fn a_render_view_override_ignores_the_offset() {
    let pose = ViewPose {
        translation: [0.0, 3.0, 8.0],
        rotation: [0.0, 0.0, 0.0],
        fov_y_deg: None,
    };
    let set_view = |engine: &mut Engine| match dispatch(
        engine,
        Command::RenderView(RenderViewParams {
            pose: Some(Some(pose)),
            ..Default::default()
        }),
    )
    .unwrap()
    .body
    {
        ResponseBody::RenderView { .. } => {}
        other => panic!("{other:?}"),
    };
    let mut with = engine_for(&scene(&camera_block(EYE, 60.0), &offset_entity(PULL_IN)));
    let mut without = engine_for(&scene(&camera_block(EYE, 60.0), ""));
    set_view(&mut with);
    set_view(&mut without);
    assert_eq!(extract(&mut with).camera, extract(&mut without).camera);
}

/// A transform on the offset entity, and the rig's own `Transform`/`Camera`, are untouched.
#[test]
fn the_rigs_components_keep_their_values() {
    let mut engine = engine_for(&scene(&camera_block(EYE, 60.0), &offset_entity(PULL_IN)));
    extract(&mut engine);
    let cam = named(engine.world(), "cam");
    let world = engine.world();
    assert_eq!(
        world.get::<Transform>(cam).unwrap().unwrap().translation,
        EYE
    );
    assert_eq!(
        world.get::<Camera>(cam).unwrap().unwrap().projection,
        Projection::Perspective { fov_y_deg: 60.0 }
    );
}
