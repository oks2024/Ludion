//! Every registered component's own serialized value passes the wire schema check, so the editor's
//! `component.get` → `component.set` loop never meets an `L1014` on a value the engine produced.
//! Each component is built from `{}` when it has a default, from its required-shape seed when it
//! does not, and once more per data-carrying enum variant.

use ludion_ecs::World;
use ludion_engine::Engine;
use serde_json::{Value, json};

fn entity_ref() -> Value {
    json!({ "index": 0, "generation": 0 })
}

/// The values to build `name` from: `{}` unless it has required fields or data-carrying enum
/// variants (shapes from the component schema snapshot).
fn fixtures(name: &str) -> Vec<Value> {
    let joint = |kind: Value| json!({ "a": entity_ref(), "b": entity_ref(), "kind": kind });
    let material = json!({ "friction": 0.5, "restitution": 0.25 });
    let half3 = json!([0.5, 0.5, 0.5]);
    match name {
        "Name" => vec![json!("n")],
        "Parent" => vec![entity_ref()],
        "JointMotor" => vec![json!({ "target": [0.0], "stiffness": 0.0, "damping": 0.0 })],
        "JointPoses" => vec![json!({ "stiffness": 0.0, "damping": 0.0, "poses": { "a": [0.0] } })],
        "PoseBlend" => vec![json!({ "pose": "a", "weight": 0.0 })],
        "ParentJoint" => vec![json!({ "joint": "j" })],
        "PrefabInstance" => vec![json!({
            "source": "p.prefab.yaml",
            "part": "a",
            "root": entity_ref(),
        })],
        "SkinBone" => vec![json!({ "skin": entity_ref(), "bone": "b" })],
        "UiWorldAnchor" => vec![json!({ "target": entity_ref() })],
        "SightTarget" => vec![json!({ "class": "c" })],
        "Joint" => vec![
            joint(json!({ "fixed": {} })),
            joint(json!({ "hinge": { "axis": [0.0, 1.0, 0.0], "limits": [-1.0, 1.0] } })),
            joint(json!({ "ball": {} })),
            joint(json!({ "slider": { "axis": [1.0, 0.0, 0.0], "limits": [0.0, 2.0] } })),
        ],
        "Collider" => vec![
            json!({ "body": { "static": material } }),
            json!({ "body": { "kinematic": material } }),
            json!({ "body": { "rigid": { "mass": 2.0, "sleep_after_ticks": 30 } } }),
            json!({ "shape": { "circle": { "radius": 1.0 } } }),
            json!({ "shape": { "aabb": { "half_extents": [0.5, 0.5] } } }),
            json!({ "shape": { "sphere": { "radius": 1.0 } } }),
            json!({ "shape": { "box": { "half_extents": half3 } } }),
            json!({ "shape": { "capsule": { "half_height": 1.0, "radius": 0.5 } } }),
            json!({ "shape": { "cylinder": { "half_height": 1.0, "radius": 0.5 } } }),
            json!({ "shape": { "convex": { "points": [[0.0, 0.0, 0.0], [1.0, 0.0, 0.0]] } } }),
            json!({ "shape": { "heightfield": {
                "rows": 2, "cols": 2, "heights": [0.0, 0.0, 0.0, 0.0], "scale": [1.0, 1.0, 1.0],
            } } }),
            json!({ "shape": { "compound": { "parts": [{ "shape": { "sphere": { "radius": 1.0 } } }] } } }),
            json!({ "shape": { "mesh": { "path": "m.glb" } } }),
        ],
        "Mesh" => vec![
            json!({ "shape": { "asset": "a.glb" } }),
            json!({ "shape": { "voxel": "v.vox" } }),
            json!({ "shape": { "assembly": "a.assembly.yaml" } }),
            json!({ "shape": { "sculpt": "s.sculpt.yaml" } }),
            json!({ "shape": { "character": "c.character.yaml" } }),
            json!({ "shape": { "assembly_part": { "assembly": "a.assembly.yaml", "part": "p" } } }),
        ],
        "Camera" => vec![
            json!({ "projection": { "type": "perspective", "fov_y_deg": 60.0 } }),
            json!({ "projection": { "type": "orthographic", "height": 10.0, "pixel_snap": true } }),
        ],
        "DropShadow" | "GroundMarker" => vec![
            json!({ "shape": { "type": "disc", "radius": 1.0 } }),
            json!({ "shape": { "type": "segment", "length": 2.0, "width": 1.0 } }),
        ],
        "ReflectionProbe" => vec![
            json!({ "shape": { "box": { "extent": [1.0, 1.0, 1.0] } } }),
            json!({ "shape": { "sphere": { "radius": 2.0 } } }),
        ],
        _ => vec![json!({})],
    }
}

#[test]
fn every_component_serializes_into_its_own_schema() {
    let engine = Engine::builder()
        .build()
        .expect("the default engine builds");
    let registry = engine.registry();
    let mut world = World::new();
    let mut checked = 0usize;
    let mut failures = Vec::new();
    for info in registry.iter() {
        for fixture in fixtures(info.name()) {
            let entity = world.spawn();
            if let Err(error) = info.deserialize_insert(&fixture, &mut world, entity) {
                failures.push(format!(
                    "{}: the fixture {fixture} does not build ({error}); a component with \
                     required fields needs a fixture in `fixtures`",
                    info.name()
                ));
                continue;
            }
            let value = info
                .serialize(&world, entity)
                .expect("a present component serializes")
                .expect("the inserted component is present");
            checked += 1;
            let diagnostics = ludion_scene::check_component(info.name(), &value, registry);
            if !diagnostics.is_empty() {
                let messages: Vec<String> = diagnostics.iter().map(ToString::to_string).collect();
                failures.push(format!(
                    "{}: {value} — {}",
                    info.name(),
                    messages.join("; ")
                ));
            }
        }
    }
    assert!(checked > 0, "no component was checked");
    assert!(
        failures.is_empty(),
        "components whose fixture fails to build or whose serialized value fails their own \
         schema ({checked} checked):\n{}",
        failures.join("\n")
    );
}
