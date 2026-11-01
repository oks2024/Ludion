//! Prefab-instance expansion (`Plan/transform-hierarchy-and-prefabs.md` §J): flattening with
//! nesting, overrides and `removed`, the diagnostics' pointers into the right document, placement
//! composition, and `locate`.

mod common;

use std::cell::{Cell, RefCell};
use std::collections::BTreeMap;

use ludion_core::codes::code;
use ludion_core::diagnostics::Diagnostic;
use ludion_core::ids::EntityId;
use ludion_core::math::Trs;
use ludion_ecs::{Registry, World};
use ludion_scene::{
    ExpandedScene, InstanceHooks, InstantiateOptions, Scene, check, expand, instantiate_expanded,
};

/// A component with a defaulted, skip-when-default field that is not an `Option`: only absence
/// can express "unset", so a `null` override must remove the key.
#[derive(serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
struct Glow {
    level: u32,
    #[serde(default, skip_serializing_if = "is_zero")]
    fade: f32,
}

fn is_zero(value: &f32) -> bool {
    *value == 0.0
}

/// A component with a nested asset path, shaped like the engine's `Mesh.shape.asset`.
#[derive(serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
struct Mesh {
    shape: MeshShape,
}

#[derive(serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
struct MeshShape {
    #[schemars(with = "ludion_ecs::AssetPathSchema")]
    asset: String,
}

fn registry() -> Registry {
    let mut registry = common::registry();
    registry.register::<ludion_ecs::Parent>("Parent").unwrap();
    registry.register::<Glow>("Glow").unwrap();
    registry.register::<Mesh>("Mesh").unwrap();
    registry.mark_sim_only("Health").unwrap();
    registry
}

const LAMP: &str = "\
format: 1
kind: prefab
entities:
  - name: post
    components:
      Health: 3
  - name: bulb
    components:
      Parent:
        index: 0
        generation: 0
      Sprite:
        texture: glass
        layer: 1
        visible: true
";

const BARREL: &str = "\
format: 1
kind: prefab
entities:
  - name: tube
    components: {}
  - name: muzzle
    components:
      Parent:
        index: 0
        generation: 0
";

const TURRET: &str = "\
format: 1
kind: prefab
entities:
  - name: base
    components:
      Health: 1
  - name: barrel
    prefab: barrel.prefab.yaml
    transform:
      translation:
        - 0.0
        - 2.0
        - 0.0
";

struct Files {
    files: BTreeMap<&'static str, &'static str>,
    calls: Cell<usize>,
}

impl Files {
    fn new(files: &[(&'static str, &'static str)]) -> Self {
        Self {
            files: files.iter().copied().collect(),
            calls: Cell::new(0),
        }
    }

    fn expand(&self, source: &str) -> Result<ExpandedScene, Vec<Diagnostic>> {
        let scene = check(source, &registry()).unwrap_or_else(|d| panic!("{d:?}"));
        self.expand_scene(&scene)
    }

    fn expand_scene(&self, scene: &Scene) -> Result<ExpandedScene, Vec<Diagnostic>> {
        expand(scene, &registry(), &mut |path| {
            self.calls.set(self.calls.get() + 1);
            self.files
                .get(ludion_scene::normalize_prefab_path(path).as_str())
                .map(|text| (*text).to_owned())
                .ok_or_else(|| {
                    Diagnostic::new(code::PREFAB_UNRESOLVED, format!("cannot read `{path}`"))
                })
        })
    }
}

fn lamps() -> Files {
    Files::new(&[("lamp.prefab.yaml", LAMP)])
}

fn nested_files() -> Files {
    Files::new(&[
        ("turret.prefab.yaml", TURRET),
        ("barrel.prefab.yaml", BARREL),
    ])
}

fn names(expanded: &ExpandedScene) -> Vec<&str> {
    expanded
        .scene
        .entities
        .iter()
        .map(|e| e.name.as_deref().unwrap_or(""))
        .collect()
}

fn has(diagnostics: &[Diagnostic], code: &str, pointer: &str) -> bool {
    diagnostics
        .iter()
        .any(|d| d.code.as_str() == code && d.path.as_deref() == Some(pointer))
}

fn component<'a>(expanded: &'a ExpandedScene, entity: usize, name: &str) -> &'a serde_json::Value {
    &expanded.scene.entities[entity]
        .components
        .iter()
        .find(|(n, _)| n == name)
        .unwrap_or_else(|| panic!("entity {entity} has no {name}"))
        .1
}

#[test]
fn a_plain_scene_expands_to_an_equal_clone() {
    let source = "format: 1\nname: s\nseed: 4\nentities:\n  - name: a\n    components: {}\n  - name: b\n    components:\n      Parent:\n        index: 0\n        generation: 0\n";
    let files = Files::new(&[]);
    let scene = check(source, &registry()).unwrap();
    let expanded = files.expand_scene(&scene).unwrap();
    assert_eq!(expanded.scene, scene);
    assert!(expanded.instances.is_empty());
    assert_eq!(expanded.entry_of, [0, 1]);
    assert_eq!(files.calls.get(), 0);
}

#[test]
fn an_instance_flattens_in_order_and_entry_refs_land_on_the_first_part() {
    let source = "\
format: 1
entities:
  - name: first
    components: {}
  - name: lamp-1
    prefab: ./lamp.prefab.yaml
  - name: follower
    components:
      Parent:
        index: 1
        generation: 0
  - name: lamp-2
    prefab: lamp.prefab.yaml
";
    let files = lamps();
    let expanded = files.expand(source).unwrap();
    assert_eq!(
        names(&expanded),
        [
            "first",
            "lamp-1/post",
            "lamp-1/bulb",
            "follower",
            "lamp-2/post",
            "lamp-2/bulb"
        ]
    );
    assert_eq!(expanded.entry_of, [0, 1, 1, 2, 3, 3]);
    assert_eq!(component(&expanded, 3, "Parent")["index"], 1);
    // The prefab's own Parent (bulb -> post) is remapped by the instance's offset.
    assert_eq!(component(&expanded, 2, "Parent")["index"], 1);
    assert_eq!(component(&expanded, 5, "Parent")["index"], 4);
    assert_eq!(files.calls.get(), 2);
    let second = &expanded.instances[1];
    assert_eq!(
        (second.entry, second.depth, second.parts.clone()),
        (3, 0, 4..6)
    );
    assert_eq!(second.source, "lamp.prefab.yaml");
    assert_eq!(second.paths, ["post", "bulb"]);
}

#[test]
fn overrides_add_components_merge_fields_and_remap_refs() {
    let source = "\
format: 1
entities:
  - name: anchor
    components: {}
  - name: lamp-1
    prefab: lamp.prefab.yaml
    overrides:
      bulb:
        Sprite:
          layer: 5
        Health: 9
      post:
        Parent:
          index: 0
          generation: 0
  - name: tail
    components: {}
";
    let expanded = lamps().expand(source).unwrap();
    assert_eq!(names(&expanded).len(), 4);
    let sprite = component(&expanded, 2, "Sprite");
    assert_eq!(sprite["layer"], 5);
    assert_eq!(sprite["texture"], "glass");
    assert_eq!(component(&expanded, 2, "Health"), 9);
    assert_eq!(component(&expanded, 1, "Parent")["index"], 0);
}

#[test]
fn an_override_ref_is_a_document_position() {
    let source = "\
format: 1
entities:
  - name: lamp-1
    prefab: lamp.prefab.yaml
  - name: tail
    components: {}
  - name: lamp-2
    prefab: lamp.prefab.yaml
    overrides:
      post:
        Parent:
          index: 1
          generation: 0
";
    let expanded = lamps().expand(source).unwrap();
    // `tail` sits at flat 2 after lamp-1's two parts.
    assert_eq!(component(&expanded, 3, "Parent")["index"], 2);
}

#[test]
fn removed_drops_a_part() {
    let source = "format: 1\nentities:\n  - name: lamp-1\n    prefab: lamp.prefab.yaml\n    removed:\n      - bulb\n  - name: tail\n    components: {}\n";
    let expanded = lamps().expand(source).unwrap();
    assert_eq!(names(&expanded), ["lamp-1/post", "tail"]);
    assert_eq!(expanded.instances[0].parts, 0..1);
    assert_eq!(expanded.instances[0].paths, ["post"]);
}

const NESTED: &str = "\
format: 1
entities:
  - name: turret-1
    prefab: turret.prefab.yaml
    transform:
      translation:
        - 10.0
        - 0.0
        - 0.0
";

#[test]
fn nesting_composes_names_and_orders_instances_post_order() {
    let expanded = nested_files().expand(NESTED).unwrap();
    assert_eq!(
        names(&expanded),
        [
            "turret-1/base",
            "turret-1/barrel/tube",
            "turret-1/barrel/muzzle"
        ]
    );
    assert_eq!(component(&expanded, 2, "Parent")["index"], 1);
    let depths: Vec<_> = expanded
        .instances
        .iter()
        .map(|i| (i.name.as_str(), i.depth, i.parts.clone()))
        .collect();
    assert_eq!(depths, [("barrel", 1, 1..3), ("turret-1", 0, 0..3)]);
    assert_eq!(
        expanded.instances[1].paths,
        ["base", "barrel/tube", "barrel/muzzle"]
    );
}

#[test]
fn an_override_addresses_a_nested_part_by_its_path() {
    let source = "format: 1\nentities:\n  - name: turret-1\n    prefab: turret.prefab.yaml\n    overrides:\n      barrel/muzzle:\n        Health: 7\n";
    let expanded = nested_files().expand(source).unwrap();
    assert_eq!(component(&expanded, 2, "Health"), 7);
}

#[test]
fn removing_a_nested_part_shrinks_the_nested_range() {
    let source = "format: 1\nentities:\n  - name: turret-1\n    prefab: turret.prefab.yaml\n    removed:\n      - barrel/muzzle\n";
    let expanded = nested_files().expand(source).unwrap();
    assert_eq!(names(&expanded), ["turret-1/base", "turret-1/barrel/tube"]);
    assert_eq!(expanded.instances[0].parts, 1..2);
    assert_eq!(expanded.instances[0].paths, ["tube"]);
}

#[derive(Default)]
struct Recording {
    placed: RefCell<Vec<(EntityId, Trs)>>,
}

impl InstanceHooks for Recording {
    fn name(&self, _: &mut World, _: EntityId, _: &str) {}
    fn place(&self, _: &mut World, root: EntityId, placement: &Trs) {
        self.placed.borrow_mut().push((root, *placement));
    }
}

#[test]
fn placement_composes_inner_then_outer_on_roots_only() {
    let registry = registry();
    let expanded = nested_files().expand(NESTED).unwrap();
    let hooks = Recording::default();
    let mut world = World::new();
    let options = InstantiateOptions {
        placement: Some(Trs {
            translation: [0.0, 0.0, 100.0],
            ..Trs::IDENTITY
        }),
        name_prefix: None,
        hooks: &hooks,
    };
    let ids = instantiate_expanded(&mut world, &expanded, &registry, &options).unwrap();
    assert_eq!(ids.len(), 3);
    let order: Vec<(EntityId, [f32; 3])> = hooks
        .placed
        .borrow()
        .iter()
        .map(|(id, trs)| (*id, trs.translation))
        .collect();
    // The barrel's root, then the turret's roots (base and tube — muzzle links inside), then the group.
    assert_eq!(
        order,
        [
            (ids[1], [0.0, 2.0, 0.0]),
            (ids[0], [10.0, 0.0, 0.0]),
            (ids[1], [10.0, 0.0, 0.0]),
            (ids[0], [0.0, 0.0, 100.0]),
            (ids[1], [0.0, 0.0, 100.0]),
        ]
    );
}

#[test]
fn a_prefab_loop_is_l1232_in_the_closing_file_with_related_entries() {
    let files = Files::new(&[
        (
            "a.prefab.yaml",
            "format: 1\nkind: prefab\nentities:\n  - name: x\n    components: {}\n  - name: nb\n    prefab: b.prefab.yaml\n",
        ),
        (
            "b.prefab.yaml",
            "format: 1\nkind: prefab\nentities:\n  - name: na\n    prefab: a.prefab.yaml\n",
        ),
    ]);
    let source = "format: 1\nentities:\n  - name: s\n    prefab: a.prefab.yaml\n";
    let diagnostics = files.expand(source).unwrap_err();
    assert_eq!(diagnostics.len(), 1);
    let looped = &diagnostics[0];
    assert_eq!(looped.code.as_str(), "L1232");
    assert_eq!(looped.path.as_deref(), Some("/entities/0/prefab"));
    assert!(
        looped.message.contains("in `b.prefab.yaml`"),
        "{}",
        looped.message
    );
    assert!(
        looped
            .message
            .contains("a.prefab.yaml -> b.prefab.yaml -> a.prefab.yaml"),
        "{}",
        looped.message
    );
    let related: Vec<_> = looped
        .related
        .iter()
        .map(|r| r.path.as_deref().unwrap())
        .collect();
    assert_eq!(related, ["/entities/1", "/entities/0"]);
    assert!(looped.related[0].message.contains("`s/nb`"));
}

#[test]
fn a_bad_override_path_is_l1233_in_the_document_with_a_suggestion() {
    let source = "format: 1\nentities:\n  - name: plain\n    components: {}\n  - name: lamp-1\n    prefab: lamp.prefab.yaml\n    overrides:\n      bulbb:\n        Health: 1\n";
    let diagnostics = lamps().expand(source).unwrap_err();
    let bad = diagnostics
        .iter()
        .find(|d| d.path.as_deref() == Some("/entities/1/overrides/bulbb"))
        .expect("pointer into the document");
    assert_eq!(bad.code.as_str(), "L1233");
    assert_eq!(bad.help.as_deref(), Some("nearest match is `bulb`"));
    assert!(!bad.message.starts_with("in `"));
    assert!(bad.related.is_empty());
}

#[test]
fn bad_removed_entries_are_l1233() {
    let unknown = "format: 1\nentities:\n  - name: lamp-1\n    prefab: lamp.prefab.yaml\n    removed:\n      - bulbb\n";
    let diagnostics = lamps().expand(unknown).unwrap_err();
    assert!(has(&diagnostics, "L1233", "/entities/0/removed/0"));

    let referenced = "format: 1\nentities:\n  - name: lamp-1\n    prefab: lamp.prefab.yaml\n    removed:\n      - post\n";
    let diagnostics = lamps().expand(referenced).unwrap_err();
    assert!(has(&diagnostics, "L1233", "/entities/0/removed/0"));
    assert!(diagnostics[0].message.contains("`bulb` refers to it"));

    let everything = "format: 1\nentities:\n  - name: lamp-1\n    prefab: lamp.prefab.yaml\n    removed:\n      - bulb\n      - post\n";
    let diagnostics = lamps().expand(everything).unwrap_err();
    assert!(has(&diagnostics, "L1233", "/entities/0/removed"));

    let both = "format: 1\nentities:\n  - name: lamp-1\n    prefab: lamp.prefab.yaml\n    overrides:\n      bulb:\n        Health: 1\n    removed:\n      - bulb\n";
    let diagnostics = lamps().expand(both).unwrap_err();
    assert!(has(&diagnostics, "L1233", "/entities/0/overrides/bulb"));
}

#[test]
fn a_bad_merged_override_value_is_l1014_at_the_override() {
    let source = "format: 1\nentities:\n  - name: lamp-1\n    prefab: lamp.prefab.yaml\n    overrides:\n      bulb:\n        Sprite:\n          layer: high\n";
    let diagnostics = lamps().expand(source).unwrap_err();
    assert!(has(
        &diagnostics,
        "L1014",
        "/entities/0/overrides/bulb/Sprite/layer"
    ));
}

#[test]
fn a_bad_value_in_a_prefab_file_points_into_that_file() {
    let files = Files::new(&[(
        "props/bad.prefab.yaml",
        "format: 1\nkind: prefab\nentities:\n  - name: p\n    components:\n      Health: -1\n",
    )]);
    let source = "format: 1\nentities:\n  - name: plain\n    components: {}\n  - name: bad-1\n    prefab: ./props\\bad.prefab.yaml\n";
    let diagnostics = files.expand(source).unwrap_err();
    let bad = &diagnostics[0];
    assert_eq!(bad.code.as_str(), "L1014");
    assert_eq!(bad.path.as_deref(), Some("/entities/0/components/Health"));
    assert!(
        bad.message.starts_with("in `props/bad.prefab.yaml`: "),
        "{}",
        bad.message
    );
    assert_eq!(bad.related.len(), 1);
    assert_eq!(bad.related[0].path.as_deref(), Some("/entities/1"));
}

#[test]
fn a_resolver_error_points_at_the_entry() {
    let diagnostics = Files::new(&[]).expand(NESTED).unwrap_err();
    assert!(has(&diagnostics, "L1230", "/entities/0/prefab"));
}

#[test]
fn locate_maps_flat_pointers_back_to_the_document() {
    let source = "format: 1\nentities:\n  - name: lamp-1\n    prefab: lamp.prefab.yaml\n  - name: tail\n    components:\n      Health: 1\n";
    let expanded = lamps().expand(source).unwrap();
    let health = Diagnostic::new(code::SCENE_COMPONENT_MISMATCH, "bad")
        .with_path("/entities/2/components/Health");
    assert_eq!(
        expanded.locate(health).path.as_deref(),
        Some("/entities/1/components/Health")
    );
    let sprite = Diagnostic::new(code::SCENE_COMPONENT_MISMATCH, "bad")
        .with_path("/entities/1/components/Sprite/layer");
    let located = expanded.locate(sprite);
    assert_eq!(located.path.as_deref(), Some("/entities/0"));
    assert_eq!(
        located.message,
        "part `bulb` of instance `lamp-1` (`lamp.prefab.yaml`), at /components/Sprite/layer: bad"
    );
    let whole = expanded
        .locate(Diagnostic::new(code::SCENE_COMPONENT_MISMATCH, "bad").with_path("/entities/0"));
    assert!(
        whole
            .message
            .starts_with("part `post` of instance `lamp-1` (`lamp.prefab.yaml`): ")
    );
    let elsewhere = Diagnostic::new(code::SCENE_COMPONENT_MISMATCH, "bad").with_path("/physics");
    assert_eq!(expanded.locate(elsewhere).path.as_deref(), Some("/physics"));
}

#[test]
fn a_parent_cycle_across_levels_is_reported_at_the_instance() {
    let source = "\
format: 1
entities:
  - name: lamp-1
    prefab: lamp.prefab.yaml
    overrides:
      post:
        Parent:
          index: 1
          generation: 0
  - name: tail
    components:
      Parent:
        index: 0
        generation: 0
";
    let diagnostics = lamps().expand(source).unwrap_err();
    assert_eq!(diagnostics.len(), 1);
    assert_eq!(diagnostics[0].code.as_str(), "L1035");
    assert_eq!(diagnostics[0].path.as_deref(), Some("/entities/0"));
    assert!(
        diagnostics[0]
            .message
            .starts_with("part `post` of instance `lamp-1`"),
        "{}",
        diagnostics[0].message
    );
    assert!(diagnostics[0].message.contains("lamp-1/post"));
}

#[test]
fn a_removed_pointer_follows_the_authored_order() {
    let source = "format: 1\nentities:\n  - name: lamp-1\n    prefab: lamp.prefab.yaml\n    removed:\n      - zz\n      - bulbb\n";
    let diagnostics = lamps().expand(source).unwrap_err();
    assert!(has(&diagnostics, "L1233", "/entities/0/removed/0"));
    assert!(has(&diagnostics, "L1233", "/entities/0/removed/1"));
    assert!(diagnostics[1].message.contains("`bulbb`"));
}

/// `post`, then an unparented `bulb`.
const PAIR: &str = "format: 1
kind: prefab
entities:
  - name: post
    components: {}
  - name: bulb
    components: {}
";

fn pairs() -> Files {
    Files::new(&[("pair.prefab.yaml", PAIR)])
}

#[test]
fn an_override_ref_to_its_own_entry_means_the_first_part() {
    let source = "format: 1
entities:
  - name: lamp-1
    prefab: pair.prefab.yaml
    overrides:
      bulb:
        Parent:
          index: 0
          generation: 0
";
    let expanded = pairs().expand(source).unwrap();
    assert_eq!(component(&expanded, 1, "Parent")["index"], 0);
}

#[test]
fn an_override_ref_from_the_first_part_to_its_own_entry_is_l1017() {
    let source = "format: 1
entities:
  - name: lamp-1
    prefab: pair.prefab.yaml
    overrides:
      post:
        Parent:
          index: 0
          generation: 0
";
    let scene = check(source, &registry()).unwrap_or_else(|d| panic!("{d:?}"));
    let diagnostics = pairs().expand_scene(&scene).unwrap_err();
    assert!(
        has(
            &diagnostics,
            "L1017",
            "/entities/0/overrides/post/Parent/index"
        ),
        "{diagnostics:?}"
    );
}

#[test]
fn a_ref_to_an_entry_lands_on_its_first_surviving_part() {
    let source = "format: 1
entities:
  - name: lamp-1
    prefab: pair.prefab.yaml
    removed:
      - post
  - name: pet
    components:
      Parent:
        index: 0
        generation: 0
";
    let expanded = pairs().expand(source).unwrap();
    assert_eq!(names(&expanded), ["lamp-1/bulb", "pet"]);
    assert_eq!(component(&expanded, 1, "Parent")["index"], 0);
}

#[test]
fn a_null_override_removes_a_defaulted_field() {
    const GLOWING: &str = "format: 1
kind: prefab
entities:
  - name: lamp
    components:
      Glow:
        level: 2
        fade: 0.5
";
    let source = "format: 1
entities:
  - name: i
    prefab: glowing.prefab.yaml
    overrides:
      lamp:
        Glow:
          fade: null
";
    let files = Files::new(&[("glowing.prefab.yaml", GLOWING)]);
    let expanded = files.expand(source).unwrap_or_else(|d| panic!("{d:?}"));
    let glow = component(&expanded, 0, "Glow");
    assert_eq!(glow["level"], 2);
    assert!(glow.get("fade").is_none(), "{glow}");
}

#[test]
fn a_null_override_on_a_missing_component_drops_the_field() {
    const BARE: &str = "format: 1
kind: prefab
entities:
  - name: lamp
    components: {}
";
    let source = "format: 1
entities:
  - name: i
    prefab: bare.prefab.yaml
    overrides:
      lamp:
        Glow:
          level: 2
          fade: null
";
    let files = Files::new(&[("bare.prefab.yaml", BARE)]);
    let expanded = files.expand(source).unwrap_or_else(|d| panic!("{d:?}"));
    let glow = component(&expanded, 0, "Glow");
    assert_eq!(glow["level"], 2);
    assert!(glow.get("fade").is_none(), "{glow}");
}

#[test]
fn an_instance_of_an_empty_prefab_is_one_diagnostic_at_the_prefab() {
    const EMPTY: &str = "format: 1\nkind: prefab\nentities: []\n";
    let source = "format: 1\nentities:\n  - name: i\n    prefab: empty.prefab.yaml\n";
    let files = Files::new(&[("empty.prefab.yaml", EMPTY)]);
    let diagnostics = files.expand(source).unwrap_err();
    assert!(
        has(&diagnostics, "L1233", "/entities/0/prefab"),
        "{diagnostics:?}"
    );
    assert!(
        diagnostics.iter().all(|d| !d.message.contains("removed")),
        "{diagnostics:?}"
    );
}

const DRESSED: &str = "\
format: 1
kind: prefab
entities:
  - name: post
    components:
      Health: 3
  - name: bulb
    presentation: true
    components: {}
  - name: stump
    components:
      Health: 1
";

const GLOW_FIRST: &str = "\
format: 1
kind: prefab
entities:
  - name: glow
    presentation: true
    components: {}
  - name: post
    components:
      Health: 3
";

const PROP: &str = "\
format: 1
kind: prefab
entities:
  - name: a
    presentation: true
    components: {}
  - name: b
    presentation: true
    components: {}
";

#[test]
fn a_presentation_first_prefab_with_a_sim_part_is_l1036() {
    let diagnostics = ludion_scene::check_prefab(GLOW_FIRST, &registry()).unwrap_err();
    assert!(has(&diagnostics, "L1036", "/entities/0/presentation"));
    assert!(diagnostics[0].message.contains("`glow`"));
    assert!(ludion_scene::check_prefab(PROP, &registry()).is_ok());
    assert!(ludion_scene::check_prefab(DRESSED, &registry()).is_ok());
}

#[test]
fn an_instance_of_a_presentation_first_prefab_is_l1036_at_the_entry() {
    let files = Files::new(&[("glow.prefab.yaml", GLOW_FIRST), ("prop.prefab.yaml", PROP)]);
    let source = "format: 1\nentities:\n  - name: g\n    prefab: glow.prefab.yaml\n";
    let diagnostics = files.expand(source).unwrap_err();
    assert!(diagnostics.iter().any(|d| d.code.as_str() == "L1036"));
    let clean = "format: 1\nentities:\n  - name: p\n    prefab: prop.prefab.yaml\n";
    assert!(files.expand(clean).is_ok());
}

#[test]
fn removing_the_sim_first_part_can_leave_a_presentation_root_l1036() {
    let files = Files::new(&[("dressed.prefab.yaml", DRESSED)]);
    let keeps = "format: 1\nentities:\n  - name: d\n    prefab: dressed.prefab.yaml\n";
    assert!(files.expand(keeps).is_ok());
    let strips = "format: 1\nentities:\n  - name: d\n    prefab: dressed.prefab.yaml\n    removed:\n      - post\n";
    let diagnostics = files.expand(strips).unwrap_err();
    assert!(has(&diagnostics, "L1036", "/entities/0"));
}

const INSTANCE_BARREL: &str = "\
format: 1
entities:
  - name: b
    prefab: barrel.prefab.yaml
";

const POST_AND_BARREL: &str = "\
format: 1
kind: prefab
entities:
  - name: base
    components: {}
  - name: gun
    prefab: barrel.prefab.yaml
";

const POST_AND_PRESENTATION_BARREL: &str = "\
format: 1
kind: prefab
entities:
  - name: base
    components: {}
  - name: gun
    prefab: barrel.prefab.yaml
    presentation: true
";

fn presentation_flags(expanded: &ExpandedScene) -> Vec<bool> {
    expanded
        .scene
        .entities
        .iter()
        .map(|entity| entity.presentation)
        .collect()
}

#[test]
fn a_presentation_instance_spawns_every_part_presentation() {
    let files = Files::new(&[("barrel.prefab.yaml", BARREL)]);
    let source = format!("{INSTANCE_BARREL}    presentation: true\n");
    let expanded = files.expand(&source).unwrap();
    assert_eq!(presentation_flags(&expanded), [true, true]);
}

#[test]
fn an_omitted_presentation_key_leaves_every_part_sim() {
    let files = Files::new(&[("barrel.prefab.yaml", BARREL)]);
    let expanded = files.expand(INSTANCE_BARREL).unwrap();
    assert_eq!(presentation_flags(&expanded), [false, false]);
    let source = format!("{INSTANCE_BARREL}    presentation: false\n");
    let expanded = files.expand(&source).unwrap();
    assert_eq!(presentation_flags(&expanded), [false, false]);
}

#[test]
fn an_outer_presentation_flag_is_sticky_over_nested_instances() {
    let files = Files::new(&[
        ("barrel.prefab.yaml", BARREL),
        ("turret.prefab.yaml", POST_AND_BARREL),
    ]);
    let source = "format: 1\nentities:\n  - name: t\n    prefab: turret.prefab.yaml\n    presentation: true\n";
    let expanded = files.expand(source).unwrap();
    assert_eq!(presentation_flags(&expanded), [true, true, true]);
}

#[test]
fn an_inner_presentation_flag_marks_only_the_inner_parts() {
    let files = Files::new(&[
        ("barrel.prefab.yaml", BARREL),
        ("turret.prefab.yaml", POST_AND_PRESENTATION_BARREL),
    ]);
    let source = "format: 1\nentities:\n  - name: t\n    prefab: turret.prefab.yaml\n";
    let expanded = files.expand(source).unwrap();
    assert_eq!(presentation_flags(&expanded), [false, true, true]);
}

#[test]
fn a_presentation_instance_refuses_an_authored_sim_only_part_l1320() {
    let files = Files::new(&[("lamp.prefab.yaml", LAMP)]);
    let source =
        "format: 1\nentities:\n  - name: l\n    prefab: lamp.prefab.yaml\n    presentation: true\n";
    let diagnostics = files.expand(source).unwrap_err();
    assert!(
        has(&diagnostics, "L1320", "/entities/0/presentation"),
        "{diagnostics:?}"
    );
    let message = &diagnostics[0].message;
    assert!(
        message.contains("`post`") && message.contains("`Health`"),
        "{message}"
    );
}

#[test]
fn a_presentation_instance_refuses_a_sim_only_override_l1320() {
    let files = Files::new(&[("barrel.prefab.yaml", BARREL)]);
    let source = "format: 1\nentities:\n  - name: b\n    prefab: barrel.prefab.yaml\n    presentation: true\n    overrides:\n      tube:\n        Health: 1\n";
    let diagnostics = files.expand(source).unwrap_err();
    assert!(
        has(&diagnostics, "L1320", "/entities/0/presentation"),
        "{diagnostics:?}"
    );
    let message = &diagnostics[0].message;
    assert!(
        message.contains("`tube`") && message.contains("`Health`"),
        "{message}"
    );
}

#[test]
fn a_sim_only_part_the_prefab_marks_presentation_itself_is_not_l1320() {
    let prefab = "\
format: 1
kind: prefab
entities:
  - name: post
    presentation: true
    components:
      Health: 3
";
    let files = Files::new(&[("odd.prefab.yaml", prefab)]);
    let source =
        "format: 1\nentities:\n  - name: o\n    prefab: odd.prefab.yaml\n    presentation: true\n";
    assert!(files.expand(source).is_ok());
}

#[test]
fn a_sim_entity_referencing_a_presentation_instance_is_l1036() {
    let files = Files::new(&[("barrel.prefab.yaml", BARREL)]);
    let source = "format: 1\nentities:\n  - name: s\n    components:\n      Parent:\n        index: 1\n        generation: 0\n  - name: b\n    prefab: barrel.prefab.yaml\n    presentation: true\n";
    let diagnostics = files.expand(source).unwrap_err();
    assert!(
        has(&diagnostics, "L1036", "/entities/0/components/Parent/index"),
        "{diagnostics:?}"
    );
}

const SIGN: &str = "\
format: 1
kind: prefab
entities:
  - name: board
    components:
      Mesh:
        shape:
          asset: meshes/l.mesh.yaml
      Sprite:
        texture: ../tex/t.png
        layer: 1
        visible: true
";

#[test]
fn a_prefabs_asset_paths_resolve_against_its_directory() {
    let files = Files::new(&[("props/lamp.prefab.yaml", SIGN)]);
    let source = "format: 1\nentities:\n  - name: lamp\n    prefab: props/lamp.prefab.yaml\n";
    let expanded = files.expand(source).unwrap();
    assert_eq!(
        component(&expanded, 0, "Mesh")["shape"]["asset"],
        "props/meshes/l.mesh.yaml"
    );
    assert_eq!(component(&expanded, 0, "Sprite")["texture"], "tex/t.png");
}

#[test]
fn a_scene_override_of_a_prefab_path_stays_content_root() {
    let files = Files::new(&[("props/lamp.prefab.yaml", SIGN)]);
    let source = "format: 1\nentities:\n  - name: lamp\n    prefab: props/lamp.prefab.yaml\n    overrides:\n      board:\n        Mesh:\n          shape:\n            asset: level/x.mesh.yaml\n";
    let expanded = files.expand(source).unwrap();
    assert_eq!(
        component(&expanded, 0, "Mesh")["shape"]["asset"],
        "level/x.mesh.yaml"
    );
    assert_eq!(component(&expanded, 0, "Sprite")["texture"], "tex/t.png");
}

const OUTER: &str = "\
format: 1
kind: prefab
entities:
  - name: inner
    prefab: ../b/inner.prefab.yaml
    overrides:
      leaf:
        Mesh:
          shape:
            asset: own.mesh.yaml
";

const INNER: &str = "\
format: 1
kind: prefab
entities:
  - name: leaf
    components:
      Mesh:
        shape:
          asset: deep.mesh.yaml
";

#[test]
fn a_nested_prefab_resolves_against_the_prefab_that_names_it() {
    let asked = RefCell::new(Vec::new());
    let scene = check(
        "format: 1\nentities:\n  - name: o\n    prefab: a/outer.prefab.yaml\n",
        &registry(),
    )
    .unwrap();
    let expanded = expand(&scene, &registry(), &mut |path| {
        asked.borrow_mut().push(path.to_owned());
        match ludion_scene::normalize_prefab_path(path).as_str() {
            "a/outer.prefab.yaml" => Ok(OUTER.to_owned()),
            "b/inner.prefab.yaml" => Ok(INNER.to_owned()),
            other => Err(Diagnostic::new(
                code::PREFAB_UNRESOLVED,
                format!("cannot read `{other}`"),
            )),
        }
    })
    .unwrap();
    assert_eq!(
        *asked.borrow(),
        ["a/outer.prefab.yaml", "b/inner.prefab.yaml"]
    );
    // The outer's override is written against `a/`; the inner's own path against `b/`, and the
    // override wins.
    assert_eq!(
        component(&expanded, 0, "Mesh")["shape"]["asset"],
        "a/own.mesh.yaml"
    );
    let inner_only = Files::new(&[("b/inner.prefab.yaml", INNER)])
        .expand("format: 1\nentities:\n  - name: i\n    prefab: b/inner.prefab.yaml\n")
        .unwrap();
    assert_eq!(
        component(&inner_only, 0, "Mesh")["shape"]["asset"],
        "b/deep.mesh.yaml"
    );
}

#[test]
fn a_cycle_spelled_through_relative_paths_is_still_l1232() {
    let x = "format: 1\nkind: prefab\nentities:\n  - name: y\n    prefab: ../b/y.prefab.yaml\n";
    let y = "format: 1\nkind: prefab\nentities:\n  - name: x\n    prefab: ../a/x.prefab.yaml\n";
    let files = Files::new(&[("a/x.prefab.yaml", x), ("b/y.prefab.yaml", y)]);
    let diagnostics = files
        .expand("format: 1\nentities:\n  - name: s\n    prefab: a/x.prefab.yaml\n")
        .unwrap_err();
    assert!(
        diagnostics.iter().any(|d| d.code.as_str() == "L1232"),
        "{diagnostics:?}"
    );
}

#[test]
fn a_malformed_path_in_a_prefab_is_l1311_naming_the_file() {
    let bad = SIGN.replace("../tex/t.png", "/abs/t.png");
    let diagnostics = ludion_scene::check_prefab(&bad, &registry()).unwrap_err();
    assert!(has(
        &diagnostics,
        "L1311",
        "/entities/0/components/Sprite/texture"
    ));
    assert!(ludion_scene::check_prefab(SIGN, &registry()).is_ok());

    let nested = OUTER.replace("../b/inner", "/b/inner");
    let diagnostics = ludion_scene::check_prefab(&nested, &registry()).unwrap_err();
    assert!(has(&diagnostics, "L1311", "/entities/0/prefab"));

    let files = Files::new(&[("props/lamp.prefab.yaml", Box::leak(bad.into_boxed_str()))]);
    let source = "format: 1\nentities:\n  - name: lamp\n    prefab: props/lamp.prefab.yaml\n";
    let diagnostics = files.expand(source).unwrap_err();
    assert!(
        diagnostics
            .iter()
            .any(|d| d.code.as_str() == "L1311" && d.message.contains("props/lamp.prefab.yaml")),
        "{diagnostics:?}"
    );
}
