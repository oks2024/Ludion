//! The persistent static scene on the CPU (`Plan/gpu-driven-culling.md` W3, Serious Isaac 0039):
//! extract keeps the placed instances of every `Static` entity between frames and invalidates them
//! through the ECS write journal. The one acceptance is that the kept scene equals the rebuilt one
//! (`Engine::set_static_scene(false)`) after every kind of edit, and none of it needs a GPU.

use ludion_components::{Animation, Mesh, MeshSource, Name, PrefabInstance, Static, Transform};
use ludion_core::codes::code;
use ludion_core::diagnostics::DiagnosticCode;
use ludion_core::ids::EntityId;
use ludion_ecs::Parent;
use ludion_engine::{Diagnostics, Engine};
use ludion_protocol::Command;
use ludion_protocol::command::{AssetReloadParams, ComponentSetParams};
use ludion_renderer::{BlendData, RenderScene};

use crate::culling_bench::gallery;

const HEAD: &str = "format: 1
seed: 1
entities:
  - name: cam
    components:
      Transform:
        translation:
          - 0.0
          - 3.0
          - 12.0
      Camera:
        projection:
          type: perspective
          fov_y_deg: 60.0
";

const CRYSTAL: &str = "meshes/crystal.mesh.yaml";

fn entity(name: &str, x: f32, shape: &str, extra: &str) -> String {
    format!(
        "  - name: {name}
    components:
      Transform:
        translation:
          - {x:?}
          - 0.0
          - 0.0
{extra}      Mesh:
        shape: {shape}
"
    )
}

fn marked(name: &str, x: f32, shape: &str) -> String {
    entity(name, x, shape, "      Static: {}\n")
}

fn dynamic(name: &str, x: f32, shape: &str) -> String {
    entity(name, x, shape, "")
}

fn engine_for(entities: &[String]) -> Engine {
    let mut engine = Engine::builder()
        .project_root(gallery())
        .build()
        .expect("boots");
    let text = format!("{HEAD}{}", entities.concat());
    engine.load_inline_scene(&text).expect("the scene loads");
    engine
}

fn named(engine: &Engine, wanted: &str) -> EntityId {
    let world = engine.world();
    world
        .entities()
        .find(|&e| {
            world
                .get::<Name>(e)
                .ok()
                .flatten()
                .is_some_and(|n| n.0 == wanted)
        })
        .unwrap_or_else(|| panic!("the scene has a `{wanted}`"))
}

fn extract(engine: &mut Engine) -> (RenderScene, BlendData) {
    engine.advance_live();
    engine.extract_present_scene().expect("extracts")
}

/// The scene the kept static scene produces now, asserted equal to the one a rebuild from scratch
/// produces. Switching the host switch re-arms the kept scene, so the cache under test is the one
/// every edit before this call fed; the rebuild after it keeps the next edit incremental.
fn on_equals_off(engine: &mut Engine) -> (RenderScene, BlendData) {
    let kept = extract(engine);
    engine.set_static_scene(false);
    let rebuilt = engine.extract_present_scene().expect("extracts").0;
    engine.set_static_scene(true);
    engine.extract_present_scene().expect("extracts");
    assert_eq!(kept.0, rebuilt, "the kept static scene equals a rebuild");
    kept
}

fn reported(engine: &Engine, code: DiagnosticCode) -> Vec<String> {
    engine
        .world()
        .resource::<Diagnostics>()
        .map(|d| d.reported(code).map(str::to_string).collect())
        .unwrap_or_default()
}

fn drawn_at(scene: &RenderScene, x: f32) -> bool {
    scene.meshes.iter().any(|m| m.model.w_axis.x == x)
}

fn leading_unkeyed(blend: &BlendData) -> usize {
    blend.meshes.iter().take_while(|k| k.is_none()).count()
}

fn set_x(engine: &mut Engine, id: EntityId, x: f32) {
    engine
        .world_mut()
        .get_mut::<Transform>(id)
        .expect("transform")
        .expect("transform")
        .translation[0] = x;
}

#[test]
fn statics_lead_the_mesh_lists_in_both_modes() {
    let mut engine = engine_for(&[
        dynamic("a", -6.0, "cube"),
        marked("b", -4.0, "sphere"),
        dynamic("c", -2.0, CRYSTAL_SHAPE),
        marked("d", 0.0, CRYSTAL_SHAPE),
        marked("e", 2.0, "cube"),
    ]);
    let (scene, blend) = on_equals_off(&mut engine);
    assert_eq!(scene.meshes.len(), 5);
    assert_eq!(
        leading_unkeyed(&blend),
        3,
        "the three statics lead, unkeyed"
    );
    let xs: Vec<f32> = scene.meshes.iter().map(|m| m.model.w_axis.x).collect();
    assert_eq!(xs, [-4.0, 0.0, 2.0, -6.0, -2.0]);
    assert_eq!(scene.mesh_assets.len(), 1);
}

const CRYSTAL_SHAPE: &str = "\n          asset: meshes/crystal.mesh.yaml";

#[test]
fn a_wire_edit_moves_a_static_the_next_frame() {
    let mut engine = engine_for(&[marked("wall", -2.0, "cube"), marked("post", 2.0, "cube")]);
    extract(&mut engine);
    let wall = named(&engine, "wall");
    engine
        .execute(Command::ComponentSet(ComponentSetParams {
            entity: wall,
            component: "Transform".into(),
            value: serde_json::json!({"translation": [5.0, 0.0, 0.0]}),
        }))
        .expect("the wire edit applies");
    let (scene, _) = on_equals_off(&mut engine);
    assert!(drawn_at(&scene, 5.0), "the wall moved");
    assert!(!drawn_at(&scene, -2.0), "and is not where it was");
}

#[test]
fn a_direct_write_to_any_component_of_a_static_re_places_it() {
    let mut engine = engine_for(&[marked("wall", -2.0, "cube")]);
    extract(&mut engine);
    let wall = named(&engine, "wall");
    engine
        .world_mut()
        .get_mut::<Mesh>(wall)
        .expect("mesh")
        .expect("mesh")
        .shape = MeshSource::Sphere;
    let (scene, _) = on_equals_off(&mut engine);
    assert_eq!(scene.meshes[0].shape, ludion_renderer::MeshShape::Sphere);
}

#[test]
fn removing_static_sends_the_entity_down_the_dynamic_path() {
    let mut engine = engine_for(&[
        marked("a", -2.0, "cube"),
        marked("b", 0.0, "cube"),
        dynamic("c", 2.0, "cube"),
    ]);
    extract(&mut engine);
    let a = named(&engine, "a");
    engine.world_mut().remove::<Static>(a).expect("removes");
    let (scene, blend) = on_equals_off(&mut engine);
    assert_eq!(scene.meshes.len(), 3);
    assert_eq!(leading_unkeyed(&blend), 1);
    // The dynamics follow in ascending entity order: a, then c.
    let xs: Vec<f32> = scene.meshes.iter().map(|m| m.model.w_axis.x).collect();
    assert_eq!(xs, [0.0, -2.0, 2.0]);
}

#[test]
fn despawning_a_static_evicts_it() {
    let mut engine = engine_for(&[marked("a", -2.0, "cube"), marked("b", 0.0, "cube")]);
    extract(&mut engine);
    let a = named(&engine, "a");
    engine.world_mut().despawn(a).expect("despawns");
    let (scene, _) = on_equals_off(&mut engine);
    assert_eq!(scene.meshes.len(), 1);
    assert!(!drawn_at(&scene, -2.0));
}

#[test]
fn a_static_follows_a_static_ancestor_and_falls_back_when_it_loses_static() {
    let mut engine = engine_for(&[marked("root", 1.0, "cube"), marked("leaf", 2.0, "sphere")]);
    let (root, leaf) = (named(&engine, "root"), named(&engine, "leaf"));
    engine
        .world_mut()
        .insert(leaf, Parent(root))
        .expect("parents");
    let (scene, blend) = on_equals_off(&mut engine);
    assert_eq!(leading_unkeyed(&blend), 2);
    assert!(
        drawn_at(&scene, 3.0),
        "the leaf sits at the root plus its local offset"
    );

    set_x(&mut engine, root, 4.0);
    let (scene, _) = on_equals_off(&mut engine);
    assert!(
        drawn_at(&scene, 6.0),
        "a moved ancestor moves its static child"
    );

    engine.world_mut().remove::<Static>(root).expect("removes");
    let (_, blend) = on_equals_off(&mut engine);
    assert_eq!(
        leading_unkeyed(&blend),
        0,
        "the child left the static path with its parent"
    );
    assert_eq!(reported(&engine, code::STATIC_PARENT_NOT_STATIC).len(), 1);
}

#[test]
fn reparenting_a_static_under_a_dynamic_entity_rejects_it() {
    let mut engine = engine_for(&[dynamic("root", 1.0, "cube"), marked("leaf", 2.0, "sphere")]);
    extract(&mut engine);
    let (root, leaf) = (named(&engine, "root"), named(&engine, "leaf"));
    engine
        .world_mut()
        .insert(leaf, Parent(root))
        .expect("parents");
    let (scene, blend) = on_equals_off(&mut engine);
    assert_eq!(leading_unkeyed(&blend), 0);
    assert!(drawn_at(&scene, 3.0));
    assert_eq!(reported(&engine, code::STATIC_PARENT_NOT_STATIC).len(), 1);
}

#[test]
fn an_ineligible_static_reports_l1341_and_draws_dynamically() {
    let sprite = "  - name: flag
    components:
      Transform: {}
      Static: {}
      Sprite: {}
";
    let skinned = marked(
        "walker",
        0.0,
        "\n          asset: meshes/lod/skin-mannequin.mesh.yaml",
    );
    let mut engine = engine_for(&[sprite.to_string(), skinned]);
    let (scene, blend) = on_equals_off(&mut engine);
    extract(&mut engine);
    assert_eq!(leading_unkeyed(&blend), 0);
    assert_eq!(scene.sprites.len(), 1);
    assert_eq!(scene.meshes.len(), 1);
    assert_eq!(reported(&engine, code::STATIC_INELIGIBLE).len(), 2);
}

#[test]
fn a_static_with_a_gi_volume_voxelizes_the_same_either_way() {
    let volume = "  - name: gi
    components:
      GiVolume:
        cell_size: 1.0
        probe_spacing: 4.0
";
    let mut engine = engine_for(&[
        volume.to_string(),
        marked("floor", 0.0, "cube"),
        dynamic("crate", 2.0, "cube"),
        marked("pillar", -2.0, "cube"),
    ]);
    let (scene, _) = on_equals_off(&mut engine);
    assert!(scene.voxel.is_some(), "the volume voxelizes");
    let pillar = named(&engine, "pillar");
    set_x(&mut engine, pillar, -3.0);
    on_equals_off(&mut engine);
}

#[test]
fn a_static_written_every_frame_reports_l1343_once() {
    let mut engine = engine_for(&[marked("spinner", 0.0, "cube"), marked("post", 2.0, "cube")]);
    extract(&mut engine);
    let spinner = named(&engine, "spinner");
    for frame in 0..40 {
        set_x(&mut engine, spinner, frame as f32);
        extract(&mut engine);
    }
    assert_eq!(reported(&engine, code::STATIC_REWRITTEN).len(), 1);
    assert!(
        drawn_at(&extract(&mut engine).0, 39.0),
        "the instance follows"
    );
}

#[test]
fn a_quiet_static_scene_reports_nothing_and_stays_put() {
    let mut engine = engine_for(&[marked("post", 2.0, "cube")]);
    for _ in 0..45 {
        extract(&mut engine);
    }
    assert!(reported(&engine, code::STATIC_REWRITTEN).is_empty());
    assert!(reported(&engine, code::STATIC_INELIGIBLE).is_empty());
}

#[test]
fn reloading_a_held_static_asset_re_places_it() {
    let root = std::env::temp_dir().join(format!("ludion-static-scene-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("meshes")).unwrap();
    let source = std::fs::read_to_string(gallery().join(CRYSTAL)).unwrap();
    let path = root.join("meshes/a.mesh.yaml");
    std::fs::write(&path, &source).unwrap();
    let mut engine = Engine::builder()
        .project_root(&root)
        .build()
        .expect("boots");
    let text = format!(
        "{HEAD}{}",
        marked("gem", 0.0, "\n          asset: meshes/a.mesh.yaml")
    );
    engine.load_inline_scene(&text).expect("the scene loads");
    let (before, _) = on_equals_off(&mut engine);

    std::fs::write(&path, source.replace("1.1", "2.2")).unwrap();
    engine
        .execute(Command::AssetReload(AssetReloadParams {
            path: Some("meshes/a.mesh.yaml".into()),
        }))
        .expect("reloads");
    let (after, _) = on_equals_off(&mut engine);
    assert_ne!(
        before.mesh_assets[0], after.mesh_assets[0],
        "the new geometry is drawn"
    );
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_scene_without_statics_extracts_the_same_either_way() {
    let mut engine = engine_for(&[dynamic("a", 0.0, "cube"), dynamic("b", 2.0, "sphere")]);
    let (scene, blend) = on_equals_off(&mut engine);
    assert_eq!(scene.meshes.len(), 2);
    assert_eq!(leading_unkeyed(&blend), 0);
}

#[test]
fn the_static_kit_room_extracts_the_same_kept_and_rebuilt() {
    let mut engine = Engine::builder()
        .project_root(gallery())
        .build()
        .expect("boots");
    let text = std::fs::read_to_string(gallery().join("staged/static-kit.yaml"))
        .expect("the committed static-kit scene");
    engine.load_inline_scene(&text).expect("the scene loads");
    let (scene, blend) = on_equals_off(&mut engine);
    let kept = leading_unkeyed(&blend);
    assert!(kept > 1_900, "the kit is held ({kept} instances)");
    assert!(kept < scene.meshes.len(), "the dynamics follow the kit");
    for _ in 0..3 {
        on_equals_off(&mut engine);
    }
    assert!(reported(&engine, code::STATIC_REWRITTEN).is_empty());
    assert!(reported(&engine, code::STATIC_INELIGIBLE).is_empty());
}

#[test]
fn a_static_group_root_is_silent_and_its_static_children_stay_static() {
    let group = "  - name: group
    components:
      Transform:
        translation:
          - 10.0
          - 0.0
          - 0.0
      Static: {}
";
    let mut engine = engine_for(&[group.to_string(), marked("leaf", 1.0, "cube")]);
    let (group, leaf) = (named(&engine, "group"), named(&engine, "leaf"));
    engine
        .world_mut()
        .insert(leaf, Parent(group))
        .expect("parents");
    let (scene, blend) = on_equals_off(&mut engine);
    assert_eq!(leading_unkeyed(&blend), 1, "the leaf is held");
    assert_eq!(scene.statics.count, 1);
    assert!(drawn_at(&scene, 11.0), "it follows the group's transform");
    assert!(reported(&engine, code::STATIC_INELIGIBLE).is_empty());
    assert!(reported(&engine, code::STATIC_PARENT_NOT_STATIC).is_empty());

    set_x(&mut engine, group, 20.0);
    let (scene, _) = on_equals_off(&mut engine);
    assert!(drawn_at(&scene, 21.0), "a moved group moves its children");
}

#[test]
fn the_static_range_describes_the_prefix_and_its_key_follows_the_content() {
    let mut engine = engine_for(&[
        marked("a", 0.0, "cube"),
        dynamic("crate", 1.0, "cube"),
        marked("b", 2.0, "sphere"),
    ]);
    let (first, _) = on_equals_off(&mut engine);
    assert_eq!(first.statics.count, 2);
    assert_ne!(first.statics.key, 0);
    for _ in 0..3 {
        let (quiet, _) = on_equals_off(&mut engine);
        assert_eq!(quiet.statics, first.statics, "a quiet frame keeps the key");
    }
    let a = named(&engine, "a");
    engine
        .execute(Command::ComponentSet(ComponentSetParams {
            entity: a,
            component: "Transform".into(),
            value: serde_json::json!({"translation": [7.0, 0.0, 0.0]}),
        }))
        .expect("the wire edit applies");
    let (moved, _) = on_equals_off(&mut engine);
    assert_eq!(moved.statics.count, 2);
    assert_ne!(moved.statics.key, first.statics.key, "a wire edit moves it");

    let crate_ = named(&engine, "crate");
    set_x(&mut engine, crate_, 9.0);
    let (after, _) = on_equals_off(&mut engine);
    assert_eq!(
        after.statics, moved.statics,
        "a dynamic's motion never moves it"
    );
}

#[test]
fn a_scene_without_statics_has_the_default_range() {
    let mut engine = engine_for(&[dynamic("a", 0.0, "cube")]);
    let (scene, _) = on_equals_off(&mut engine);
    assert_eq!(scene.statics, Default::default());
}

#[test]
fn the_static_kit_range_covers_the_kit_and_stays_put() {
    let mut engine = Engine::builder()
        .project_root(gallery())
        .build()
        .expect("boots");
    let text = std::fs::read_to_string(gallery().join("staged/static-kit.yaml"))
        .expect("the committed static-kit scene");
    engine.load_inline_scene(&text).expect("the scene loads");
    let (scene, blend) = on_equals_off(&mut engine);
    assert_eq!(scene.statics.count, leading_unkeyed(&blend));
    let key = scene.statics.key;
    for _ in 0..3 {
        assert_eq!(on_equals_off(&mut engine).0.statics.key, key);
    }
}

#[test]
fn the_gi_grid_is_kept_on_quiet_frames_and_rebuilt_when_a_static_moves() {
    let volume = "  - name: gi
    components:
      GiVolume:
        cell_size: 1.0
        probe_spacing: 4.0
";
    let mut engine = engine_for(&[
        volume.to_string(),
        marked("floor", 0.0, "cube"),
        dynamic("crate", 2.0, "cube"),
        marked("pillar", -2.0, "cube"),
    ]);
    let (first, _) = extract(&mut engine);
    let (second, _) = extract(&mut engine);
    let (grid, again) = (first.voxel.expect("voxel"), second.voxel.expect("voxel"));
    assert!(
        std::sync::Arc::ptr_eq(&grid, &again),
        "a quiet frame keeps the grid"
    );

    let pillar = named(&engine, "pillar");
    set_x(&mut engine, pillar, -3.0);
    let (moved, _) = extract(&mut engine);
    let moved = moved.voxel.expect("voxel");
    assert_ne!(moved.hash, grid.hash, "a moved static rebuilds the grid");
    on_equals_off(&mut engine);
}

/// **The static-kit room renders the same through the persistent static buffers.** With the range and
/// without it the room's pixels are equal, 20 repeats on one renderer agree, and a moved static
/// renders as the zeroed scene does on a fresh renderer.
#[test]
fn needs_gpu_the_static_kit_renders_the_same_through_the_static_buffers() {
    use ludion_renderer::{FrameSize, Renderer};

    let renderer = match Renderer::new() {
        Ok(renderer) => renderer,
        Err(d) if ludion_core::require_gpu::is_no_adapter(std::slice::from_ref(&d)) => {
            eprintln!("SKIPPED the_static_kit_renders_the_same: no GPU adapter");
            return;
        }
        Err(d) => panic!("renderer init failed: {d:?}"),
    };
    let size = FrameSize::native(640, 360);
    let zeroed = |scene: &RenderScene| {
        let mut plain = scene.clone();
        plain.statics = Default::default();
        plain
    };
    let mut engine = Engine::builder()
        .project_root(gallery())
        .build()
        .expect("boots");
    let text = std::fs::read_to_string(gallery().join("staged/static-kit.yaml"))
        .expect("the committed static-kit scene");
    engine.load_inline_scene(&text).expect("the scene loads");
    let (scene, _) = extract(&mut engine);
    assert!(scene.statics.count > 1_900, "the kit is the static prefix");

    let plain = renderer.render(&zeroed(&scene), size).expect("plain").rgba;
    let with_statics = renderer.render(&scene, size).expect("statics").rgba;
    assert_eq!(with_statics, plain, "the static buffers changed the room");
    for run in 1..20 {
        let again = renderer.render(&scene, size).expect("repeat").rgba;
        assert_eq!(again, with_statics, "repeat {run} differs");
    }

    let wall = named(&engine, "kit-wall-00-00");
    let x = engine
        .world()
        .get::<Transform>(wall)
        .expect("transform")
        .expect("transform")
        .translation[0];
    set_x(&mut engine, wall, x + 0.7);
    let (moved, _) = extract(&mut engine);
    assert_ne!(moved.statics.key, scene.statics.key, "the move re-keys");
    let patched = renderer.render(&moved, size).expect("patched").rgba;
    let fresh = Renderer::new().expect("a second renderer");
    let expected = fresh.render(&zeroed(&moved), size).expect("fresh").rgba;
    assert_eq!(
        patched, expected,
        "the patched static renders as a fresh one"
    );
    assert_ne!(patched, with_statics, "the moved wall shows");
}

/// The mannequin, a socket on its `lower_arm_l` joint and the socket's own child, all `Static`.
fn socket_scene() -> String {
    format!(
        "{HEAD}  - name: skin
    components:
      Static: {{}}
      Mesh:
        shape:
          character: characters/mannequin.character.yaml
      Animation:
        clip: run
  - name: socket
    components:
      Static: {{}}
      Mesh:
        shape: cube
      Parent:
        index: 1
        generation: 0
      ParentJoint:
        joint: lower_arm_l
  - name: grandchild
    components:
      Static: {{}}
      Mesh:
        shape: cube
      Transform:
        translation:
          - 0.0
          - 1.0
          - 0.0
      Parent:
        index: 2
        generation: 0
"
    )
}

#[test]
fn a_static_under_a_socket_takes_the_dynamic_path() {
    let mut engine = Engine::builder()
        .project_root(gallery())
        .build()
        .expect("boots");
    engine
        .load_inline_scene(&socket_scene())
        .expect("the scene loads");
    for _ in 0..5 {
        let (scene, _) = on_equals_off(&mut engine);
        assert_eq!(scene.statics.count, 0, "nothing in the chain is held");
        assert!(!scene.meshes.is_empty());
    }
    let keys = reported(&engine, code::STATIC_INELIGIBLE);
    assert!(
        keys.iter().any(|k| k.contains("/grandchild/")),
        "the posed grandchild is reported: {keys:?}"
    );
}

fn hinge_project(name: &str) -> std::path::PathBuf {
    let dir =
        std::env::temp_dir().join(format!("ludion-static-hinge-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let source = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../examples/import-node-clips/expected");
    for file in [
        "model.prefab.yaml",
        "clips/open.anim.yaml",
        "clips/snap.anim.yaml",
        "meshes/base.mesh.yaml",
        "meshes/lid.mesh.yaml",
        "meshes/latch.mesh.yaml",
    ] {
        let to = dir.join("hinge").join(file);
        std::fs::create_dir_all(to.parent().unwrap()).unwrap();
        std::fs::copy(source.join(file), to).unwrap();
    }
    dir
}

fn hinge_engine(name: &str, base_overrides: &str) -> Engine {
    let text = format!(
        "{HEAD}  - name: door
    prefab: hinge/model.prefab.yaml
    overrides:
      base:
{base_overrides}      lid:
        Static: {{}}
      latch:
        Static: {{}}
"
    );
    let mut engine = Engine::builder()
        .project_root(hinge_project(name))
        .build()
        .expect("boots");
    engine.load_inline_scene(&text).expect("the scene loads");
    engine
}

fn part(engine: &Engine, wanted: &str) -> EntityId {
    let world = engine.world();
    world
        .entities()
        .find(|&e| {
            world
                .get::<PrefabInstance>(e)
                .ok()
                .flatten()
                .is_some_and(|i| i.part == wanted)
        })
        .unwrap_or_else(|| panic!("the instance has a `{wanted}`"))
}

#[test]
fn an_animated_prefab_part_takes_the_dynamic_path() {
    let mut engine = hinge_engine(
        "animated",
        "        Static: {}\n        Animation:\n          clip: open\n",
    );
    for _ in 0..5 {
        let (scene, _) = on_equals_off(&mut engine);
        assert_eq!(scene.statics.count, 0, "the posed parts are not held");
    }
    assert!(!reported(&engine, code::STATIC_INELIGIBLE).is_empty());
}

#[test]
fn animating_a_held_prefab_part_at_runtime_releases_it() {
    let mut engine = hinge_engine("runtime", "        Static: {}\n");
    let (scene, _) = on_equals_off(&mut engine);
    assert_eq!(scene.statics.count, 3, "the still instance is held");
    let base = part(&engine, "base");
    engine
        .world_mut()
        .insert(
            base,
            Animation {
                clip: "open".into(),
                ..Animation::default()
            },
        )
        .expect("animates");
    for _ in 0..5 {
        let (scene, _) = on_equals_off(&mut engine);
        assert_eq!(scene.statics.count, 0, "the animated instance is released");
    }
    assert!(!reported(&engine, code::STATIC_INELIGIBLE).is_empty());
}

#[test]
fn l1343_counts_sim_frames_not_extracts() {
    let mut engine = engine_for(&[marked("spinner", 0.0, "cube")]);
    let spinner = named(&engine, "spinner");
    extract(&mut engine);
    let mut frames = Vec::new();
    for frame in 0..30 {
        engine.advance_live();
        frames.push(engine.frame());
        set_x(&mut engine, spinner, frame as f32);
        for _ in 0..3 {
            engine.extract_present_scene().expect("extracts");
        }
    }
    assert!(
        frames.windows(2).all(|w| w[0] < w[1]),
        "a frame per advance"
    );
    assert_eq!(reported(&engine, code::STATIC_REWRITTEN).len(), 1);

    let mut engine = engine_for(&[marked("blinker", 0.0, "cube")]);
    let blinker = named(&engine, "blinker");
    extract(&mut engine);
    for frame in 0..60 {
        engine.advance_live();
        if frame % 2 == 0 {
            set_x(&mut engine, blinker, frame as f32);
        }
        engine.extract_present_scene().expect("extracts");
    }
    assert!(reported(&engine, code::STATIC_REWRITTEN).is_empty());
}

#[test]
fn turning_the_static_scene_off_disarms_the_journal() {
    let mut engine = engine_for(&[marked("post", 2.0, "cube"), marked("pillar", 4.0, "cube")]);
    extract(&mut engine);
    assert!(engine.world().is_watched::<Static>());
    engine.set_static_scene(false);
    assert!(!engine.world().is_watched::<Static>());
    let pillar = named(&engine, "pillar");
    set_x(&mut engine, pillar, -3.0);
    engine.extract_present_scene().expect("extracts");
    assert!(
        !engine.world().is_watched::<Static>(),
        "off stays unwatched"
    );
    engine.set_static_scene(true);
    extract(&mut engine);
    assert!(engine.world().is_watched::<Static>());
    set_x(&mut engine, pillar, 5.0);
    let (scene, _) = on_equals_off(&mut engine);
    assert!(drawn_at(&scene, 5.0));
}
