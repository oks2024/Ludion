//! `paired_fit` (Neon Shroud request 0008 W5a, `Plan/paired-animations.md` decisions 22–26): whether
//! the gallery's two-role `choke` document can start from where two motor entities stand.
//!
//! The instigator is the reference and moves; the target stands at `(3, 0, 2)` facing `-Z`, so the
//! aligned instigator is 0.5 m ahead of it, at `(3, 0, 1.5)` facing `+Z` (yaw 180 degrees).

use std::path::{Path, PathBuf};

use ludion_components::Name;
use ludion_core::codes::code;
use ludion_core::diagnostics::Diagnostic;
use ludion_core::ids::EntityId;
use ludion_ecs::Parent;
use ludion_engine::{Engine, PairedFit, Warp, paired_fit};
use ludion_protocol::command::EngineRestoreParams;
use ludion_protocol::{Command, ResponseBody};

const DOC: &str = "voxels/choke.paired.yaml";
const WARP: Warp = Warp {
    distance: 0.6,
    yaw_deg: 45.0,
};

fn gallery() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/gallery")
}

/// A scene vector in block style, `key` indented by `indent` spaces.
fn vec3(indent: usize, key: &str, v: [f32; 3]) -> String {
    let pad = " ".repeat(indent);
    format!(
        "{pad}{key}:
{pad}  - {v0:?}
{pad}  - {v1:?}
{pad}  - {v2:?}
",
        v0 = v[0],
        v1 = v[1],
        v2 = v[2]
    )
}

fn motor_entity(name: &str, at: [f32; 3], yaw_deg: f32, scale: f32, shape: &str) -> String {
    format!(
        "  - name: {name}
    components:
      Transform:
{}{}{}      Mesh:
        shape:
          {shape}
      CharacterMotor:
        radius: 0.35
        height: 1.7
",
        vec3(8, "translation", at),
        vec3(8, "rotation", [0.0, yaw_deg, 0.0]),
        vec3(8, "scale", [scale; 3]),
    )
}

fn plain_entity(name: &str, at: [f32; 3], scale: [f32; 3], rest: &str) -> String {
    format!(
        "  - name: {name}
    components:
      Transform:
{}{}{rest}",
        vec3(8, "translation", at),
        vec3(8, "scale", scale),
    )
}

const ASSEMBLY: &str = "assembly: voxels/humanoid.assembly.yaml";

fn scene(extra: &str) -> String {
    format!(
        "format: 1
seed: 5
physics:
  space: free
entities:
  - name: floor
    components:
      Transform:
        translation:
          - 0.0
          - -0.5
          - 0.0
      Collider:
        shape:
          box:
            half_extents:
              - 20.0
              - 0.5
              - 20.0
        body: static
{}{}{extra}",
        // 0.4 m and 20 degrees off its aligned pose.
        motor_entity("instigator", [3.4, 0.0, 1.5], 160.0, 0.125, ASSEMBLY),
        motor_entity("target", [3.0, 0.0, 2.0], 0.0, 0.125, ASSEMBLY),
    )
}

fn boot(root: &Path, text: &str) -> Engine {
    let mut engine = Engine::builder()
        .project_root(root)
        .build()
        .expect("the game builds");
    engine.load_inline_scene(text).expect("the scene loads");
    for _ in 0..3 {
        engine.advance_live();
    }
    engine
}

fn named(engine: &mut Engine, name: &str) -> EntityId {
    engine
        .world_mut()
        .query_ids::<(&Name,)>()
        .find(|(_, (n,))| n.0 == name)
        .map(|(id, _)| id)
        .unwrap_or_else(|| panic!("no entity `{name}`"))
}

struct Rig {
    engine: Engine,
    instigator: EntityId,
    target: EntityId,
}

fn rig(extra: &str) -> Rig {
    let mut engine = boot(&gallery(), &scene(extra));
    let instigator = named(&mut engine, "instigator");
    let target = named(&mut engine, "target");
    Rig {
        engine,
        instigator,
        target,
    }
}

impl Rig {
    fn fit(&self, warp: Warp) -> Result<PairedFit, Vec<Diagnostic>> {
        paired_fit(
            self.engine.world(),
            DOC,
            &[("instigator", self.instigator), ("target", self.target)],
            warp,
        )
    }
}

fn codes(errors: &[Diagnostic]) -> Vec<String> {
    errors.iter().map(|d| d.code.to_string()).collect()
}

fn close(a: f32, b: f32) -> bool {
    (a - b).abs() < 1e-4
}

#[test]
fn a_clean_fit_ignores_the_partner_and_carries_the_exact_target() {
    let rig = rig("");
    let fit = rig.fit(WARP).expect("a valid cast");
    assert!(fit.fits, "{fit:?}");
    assert_eq!(fit.blocked_by, None);
    assert_eq!(fit.over_warp, None);
    let [instigator, target] = &fit.roles[..] else {
        panic!("two roles: {fit:?}");
    };
    assert_eq!(
        (instigator.role.as_str(), target.role.as_str()),
        ("instigator", "target")
    );
    assert_eq!(instigator.entity, rig.instigator);
    assert!(
        close(instigator.feet[0], 3.0) && close(instigator.feet[2], 1.5),
        "{instigator:?}"
    );
    assert_eq!(instigator.feet[1], 0.0);
    assert!(
        close(instigator.yaw.abs(), std::f32::consts::PI),
        "{instigator:?}"
    );
    assert!(close(instigator.distance, 0.4), "{instigator:?}");
    assert!(close(instigator.yaw_deg, 20.0), "{instigator:?}");
    assert_eq!(target.feet, [3.0, 0.0, 2.0]);
    assert_eq!((target.distance, target.yaw_deg), (0.0, 0.0));
}

#[test]
fn a_wall_at_the_target_pose_blocks_the_instigator() {
    let wall = "  - name: wall
    components:
      Transform:
        translation:
          - 3.0
          - 1.0
          - 1.5
      Collider:
        shape:
          box:
            half_extents:
              - 0.1
              - 1.0
              - 0.1
        body: static
";
    let mut rig = rig(wall);
    let wall = named(&mut rig.engine, "wall");
    let fit = rig.fit(WARP).expect("a valid cast");
    assert_eq!(fit.blocked_by, Some(("instigator".to_owned(), wall)));
    assert!(!fit.fits);
}

#[test]
fn a_warp_over_the_limit_does_not_fit() {
    let rig = rig("");
    let fit = rig
        .fit(Warp {
            distance: 0.3,
            yaw_deg: 45.0,
        })
        .expect("a valid cast");
    assert_eq!(fit.over_warp.as_deref(), Some("instigator"));
    assert_eq!(fit.blocked_by, None);
    assert!(!fit.fits);
    let turn = rig
        .fit(Warp {
            distance: 0.6,
            yaw_deg: 10.0,
        })
        .expect("a valid cast");
    assert_eq!(turn.over_warp.as_deref(), Some("instigator"));
}

#[test]
fn an_unknown_document_is_l4370() {
    let rig = rig("");
    let cast = [("instigator", rig.instigator), ("target", rig.target)];
    let errors =
        paired_fit(rig.engine.world(), "voxels/nope.paired.yaml", &cast, WARP).unwrap_err();
    assert_eq!(errors[0].code, code::PAIRED_UNKNOWN, "{errors:?}");
    assert!(errors.len() > 1, "the loader's diagnostics follow");
}

#[test]
fn a_bad_cast_is_l4371_with_every_problem_collected() {
    let rig = rig("");
    let (a, b) = (rig.instigator, rig.target);
    let world = rig.engine.world();
    let wrong = paired_fit(world, DOC, &[("instigator", a), ("victim", b)], WARP).unwrap_err();
    assert_eq!(codes(&wrong), ["L4371", "L4371"], "{wrong:?}");
    assert_eq!(wrong[0].path.as_deref(), Some("/roles/victim"));
    assert_eq!(wrong[1].path.as_deref(), Some("/roles/target"));
    let twice = paired_fit(world, DOC, &[("instigator", a), ("instigator", b)], WARP).unwrap_err();
    assert_eq!(codes(&twice), ["L4371", "L4371"], "{twice:?}");
    let missing = paired_fit(world, DOC, &[("instigator", a)], WARP).unwrap_err();
    assert_eq!(missing[0].path.as_deref(), Some("/roles/target"));
}

#[test]
fn a_dead_parented_or_wrong_entity_is_l4371() {
    let small = [0.125; 3];
    let prop = plain_entity(
        "prop",
        [8.0, 0.0, 8.0],
        small,
        "      Mesh:
        shape: cube
",
    ) + &plain_entity(
        "wrong",
        [9.0, 0.0, 8.0],
        small,
        "      Mesh:
        shape:
          assembly: voxels/choke-other.assembly.yaml
",
    );
    let mut rig = rig(&prop);
    let prop = named(&mut rig.engine, "prop");
    let wrong = named(&mut rig.engine, "wrong");
    let cast = |target| [("instigator", rig.instigator), ("target", target)];
    for entity in [prop, wrong] {
        let errors = paired_fit(rig.engine.world(), DOC, &cast(entity), WARP).unwrap_err();
        assert_eq!(codes(&errors), ["L4371"], "{errors:?}");
        assert_eq!(errors[0].path.as_deref(), Some("/roles/target"));
    }
    let root = rig.target;
    rig.engine.world_mut().insert(prop, Parent(root)).unwrap();
    let errors = paired_fit(rig.engine.world(), DOC, &cast(prop), WARP).unwrap_err();
    assert!(
        codes(&errors).iter().all(|c| c == "L4371") && errors.len() >= 2,
        "{errors:?}"
    );
    rig.engine.world_mut().despawn(prop).unwrap();
    let errors = paired_fit(rig.engine.world(), DOC, &cast(prop), WARP).unwrap_err();
    assert_eq!(codes(&errors), ["L4371"], "{errors:?}");
}

#[test]
fn a_scale_that_is_not_the_roles_is_l4372() {
    let big = motor_entity("big", [8.0, 0.0, 8.0], 0.0, 0.25, ASSEMBLY);
    let skew = plain_entity(
        "skew",
        [9.0, 0.0, 8.0],
        [0.125, 0.25, 0.125],
        "      Mesh:
        shape:
          assembly: voxels/humanoid.assembly.yaml
",
    );
    let mut rig = rig(&format!("{big}{skew}"));
    let big = named(&mut rig.engine, "big");
    let skew = named(&mut rig.engine, "skew");
    for entity in [big, skew] {
        let errors = paired_fit(
            rig.engine.world(),
            DOC,
            &[("instigator", rig.instigator), ("target", entity)],
            WARP,
        )
        .unwrap_err();
        assert_eq!(codes(&errors), ["L4372"], "{errors:?}");
        assert_eq!(errors[0].path.as_deref(), Some("/roles/target"));
    }
}

fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let target = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_dir(&entry.path(), &target);
        } else {
            std::fs::copy(entry.path(), target).unwrap();
        }
    }
}

#[test]
fn three_roles_are_l4373() {
    let root = std::env::temp_dir().join(format!("ludion-paired-fit-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    copy_dir(&gallery().join("voxels"), &root.join("voxels"));
    let fixture =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/paired/three-roles.paired.yaml");
    std::fs::copy(fixture, root.join("voxels/three-roles.paired.yaml")).unwrap();
    let mut engine = boot(&root, &scene(""));
    let (a, b) = (
        named(&mut engine, "instigator"),
        named(&mut engine, "target"),
    );
    let errors = paired_fit(
        engine.world(),
        "voxels/three-roles.paired.yaml",
        &[("a", a), ("b", b)],
        WARP,
    )
    .unwrap_err();
    assert_eq!(codes(&errors), ["L4373"], "{errors:?}");
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn an_engine_without_a_project_gives_l4370() {
    let mut engine = Engine::builder().build().expect("the game builds");
    engine.load_inline_scene(&scene("")).unwrap();
    let (a, b) = (
        named(&mut engine, "instigator"),
        named(&mut engine, "target"),
    );
    let errors = paired_fit(
        engine.world(),
        DOC,
        &[("instigator", a), ("target", b)],
        WARP,
    )
    .unwrap_err();
    assert_eq!(errors[0].code, code::PAIRED_UNKNOWN, "{errors:?}");
}

#[test]
fn a_fit_mutates_nothing_and_is_the_same_at_every_thread_count() {
    let fits: Vec<PairedFit> = [1, 2, 8]
        .into_iter()
        .map(|n| {
            ludion_core::par::with_threads(n, || {
                let mut rig = rig("");
                let before = rig.engine.world_hash().unwrap();
                let fit = rig.fit(WARP).expect("a valid cast");
                assert_eq!(
                    rig.engine.world_hash().unwrap(),
                    before,
                    "a fit changed the world"
                );
                fit
            })
        })
        .collect();
    assert_eq!(fits[0], fits[1], "1 vs 2 threads");
    assert_eq!(fits[0], fits[2], "1 vs 8 threads");
}

#[test]
fn a_restore_reloads_the_library_and_the_fit_is_unchanged() {
    let mut rig = rig("");
    let before = rig.fit(WARP).expect("a valid cast");
    let snapshot = match rig.engine.execute(Command::EngineSnapshot).unwrap().body {
        ResponseBody::Snapshot { snapshot } => snapshot,
        other => panic!("expected a snapshot, got {other:?}"),
    };
    rig.engine
        .execute(Command::EngineRestore(EngineRestoreParams { snapshot }))
        .unwrap();
    let instigator = named(&mut rig.engine, "instigator");
    let target = named(&mut rig.engine, "target");
    let after = paired_fit(
        rig.engine.world(),
        DOC,
        &[("instigator", instigator), ("target", target)],
        WARP,
    )
    .expect("a valid cast");
    assert_eq!(before, after);
}
