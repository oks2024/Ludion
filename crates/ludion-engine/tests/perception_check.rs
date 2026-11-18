//! Static `L1572` checks (`Plan/perception.md` Wave 5): layer, class and noise-kind typos found
//! without a tick, agreeing with what the runtime reports, silent on a clean scene and on the gallery.

use std::path::PathBuf;

use ludion_core::codes::code;
use ludion_core::diagnostics::Diagnostic;
use ludion_engine::diagnostics::Diagnostics;
use ludion_engine::{
    Engine, PerceptionConfig, builtin_registry, dispatch, perception_update,
    scene_engine_diagnostics, scene_perception_diagnostics, scene_perception_name_diagnostics,
};
use ludion_protocol::command::SceneCheckParams;
use ludion_protocol::{Command, ResponseBody};

fn config() -> PerceptionConfig {
    PerceptionConfig {
        classes: vec!["player".into()],
        noise_kinds: vec!["footstep".into()],
        ..Default::default()
    }
}

/// `key:` and its list items, `indent` spaces deep; nothing for an empty list.
fn list(indent: usize, key: &str, items: &[&str]) -> String {
    if items.is_empty() {
        return String::new();
    }
    let pad = " ".repeat(indent);
    let mut out = format!("{pad}{key}:\n");
    for item in items {
        out += &format!("{pad}  - {item}\n");
    }
    out
}

/// The names a scene authors; every field is what one typo could land in.
#[derive(Default)]
struct Names<'a> {
    see_through: &'a [&'a str],
    detects: &'a [&'a str],
    blocked_by: &'a [&'a str],
    class: Option<&'a str>,
    hears: &'a [&'a str],
    zone_kinds: &'a [&'a str],
}

fn scene(names: &Names) -> String {
    let class = names.class.unwrap_or("player");
    let hears = if names.hears.is_empty() {
        list(10, "hears", &["footstep"])
    } else {
        list(10, "hears", names.hears)
    };
    let zone_kinds = list(8, "kinds", names.zone_kinds);
    format!(
        "format: 1\nseed: 1\nphysics:\n  space: free\n  layers:\n    - name: wall\nentities:\n  \
         - name: guard\n    components:\n      Transform: {{}}\n      Perceiver:\n        hearing:\n{hears}        \
         sight:\n          - name: main\n            range: 30.0\n            half_angle_deg: 60.0\n{}{}  \
         - name: runner\n    components:\n      Transform:\n        translation:\n          - 0.0\n          - 0.0\n          \
         - -5.0\n      SightTarget:\n        class: {class}\n        points:\n          - offset:\n              - 0.0\n              \
         - 0.0\n              - 0.0\n{}  \
         - name: din\n    components:\n      Transform: {{}}\n      NoiseZone:\n        shape:\n          sphere:\n            radius: 3.0\n{zone_kinds}  \
         - name: ground\n    components:\n      Transform: {{}}\n      Collider:\n        shape:\n          box:\n            half_extents:\n              - 9.0\n              \
         - 0.5\n              - 9.0\n        body: static\n        layer: wall\n",
        list(12, "detects", names.detects),
        list(12, "see_through", names.see_through),
        list(12, "blocked_by", names.blocked_by),
    )
}

fn parse(yaml: &str) -> ludion_scene::Scene {
    ludion_scene::check(yaml, &builtin_registry()).expect("the scene is valid")
}

fn paths(found: &[Diagnostic]) -> Vec<&str> {
    found.iter().filter_map(|d| d.path.as_deref()).collect()
}

fn help(found: &[Diagnostic], at: &str) -> String {
    found
        .iter()
        .find(|d| d.path.as_deref() == Some(at))
        .and_then(|d| d.help.clone())
        .unwrap_or_default()
}

#[test]
fn a_clean_scene_reports_nothing() {
    let scene = parse(&scene(&Names::default()));
    assert!(scene_perception_diagnostics(&scene).is_empty());
    assert!(scene_perception_name_diagnostics(&scene, &config()).is_empty());
}

#[test]
fn a_layer_typo_is_found_at_its_pointer_with_the_nearest_name() {
    let see = parse(&scene(&Names {
        see_through: &["wal"],
        ..Default::default()
    }));
    let found = scene_perception_diagnostics(&see);
    assert_eq!(
        paths(&found),
        ["/entities/0/components/Perceiver/sight/0/see_through/0"]
    );
    assert!(
        found
            .iter()
            .all(|d| d.code == code::PERCEPTION_NAME_UNKNOWN)
    );
    assert!(
        help(&found, paths(&found)[0]).contains("did you mean `wall`"),
        "{found:?}"
    );

    let blocked = parse(&scene(&Names {
        blocked_by: &["wall", "wal"],
        ..Default::default()
    }));
    let found = scene_perception_diagnostics(&blocked);
    assert_eq!(
        paths(&found),
        ["/entities/1/components/SightTarget/points/0/blocked_by/1"]
    );
}

#[test]
fn class_and_kind_typos_need_the_config_and_are_silent_without_it() {
    let typos = parse(&scene(&Names {
        detects: &["plyer"],
        class: Some("plyer"),
        hears: &["footsteps"],
        zone_kinds: &["footstp"],
        ..Default::default()
    }));
    assert!(
        scene_perception_diagnostics(&typos).is_empty(),
        "the layer pass has no names to check"
    );
    assert!(
        scene_engine_diagnostics(&std::env::temp_dir(), &typos)
            .iter()
            .all(|d| d.code != code::PERCEPTION_NAME_UNKNOWN),
        "the engine pass has no config"
    );
    let found = scene_perception_name_diagnostics(&typos, &config());
    assert_eq!(
        paths(&found),
        [
            "/entities/0/components/Perceiver/sight/0/detects/0",
            "/entities/0/components/Perceiver/hearing/hears/0",
            "/entities/1/components/SightTarget/class",
            "/entities/2/components/NoiseZone/kinds/0",
        ]
    );
    assert!(help(&found, paths(&found)[0]).contains("did you mean `player`"));
    assert!(help(&found, paths(&found)[1]).contains("did you mean `footstep`"));
}

#[test]
fn the_engine_pass_reports_layer_typos() {
    let typos = parse(&scene(&Names {
        blocked_by: &["wal"],
        ..Default::default()
    }));
    let found = scene_engine_diagnostics(&std::env::temp_dir(), &typos);
    assert_eq!(
        found
            .iter()
            .filter(|d| d.code == code::PERCEPTION_NAME_UNKNOWN)
            .count(),
        1
    );
}

/// The `kind:name` keys the runtime latches for layers and classes, from a one-tick run.
fn runtime_keys(yaml: &str) -> Vec<String> {
    let mut engine = Engine::builder()
        .perception(config())
        .system("perception_update", perception_update)
        .build()
        .unwrap();
    engine.load_inline_scene(yaml).unwrap();
    for _ in 0..5 {
        engine.advance_live();
    }
    let mut keys: Vec<String> = engine
        .world()
        .resource::<Diagnostics>()
        .unwrap()
        .reported(code::PERCEPTION_NAME_UNKNOWN)
        .map(String::from)
        .filter(|key| key.starts_with("layer:") || key.starts_with("class:"))
        .collect();
    keys.sort();
    keys
}

/// The same keys from the static passes: the second word of the message is `layer` or `class`.
fn static_keys(yaml: &str) -> Vec<String> {
    let scene = parse(yaml);
    let mut found = scene_perception_diagnostics(&scene);
    found.extend(scene_perception_name_diagnostics(&scene, &config()));
    let mut keys: Vec<String> = found
        .iter()
        .filter_map(|d| {
            let mut words = d.message.split('`');
            let what = words.next()?.trim().rsplit(' ').next()?;
            let name = words.next()?;
            matches!(what, "layer" | "class").then(|| format!("{what}:{name}"))
        })
        .collect();
    keys.sort();
    keys.dedup();
    keys
}

#[test]
fn the_static_passes_agree_with_the_runtime_on_which_names_are_unknown() {
    let cases = [
        Names {
            see_through: &["wal"],
            ..Default::default()
        },
        Names {
            blocked_by: &["wal"],
            ..Default::default()
        },
        Names {
            detects: &["plyer"],
            ..Default::default()
        },
        Names {
            class: Some("ghost"),
            ..Default::default()
        },
        Names::default(),
    ];
    for (case, names) in cases.iter().enumerate() {
        let yaml = scene(names);
        assert_eq!(static_keys(&yaml), runtime_keys(&yaml), "case {case}");
    }
    assert_eq!(
        static_keys(&scene(&cases[0])),
        ["layer:wal"],
        "the comparison is not vacuous"
    );
}

fn check_over_the_wire(engine: &mut Engine, root: &std::path::Path, yaml: &str) -> Vec<Diagnostic> {
    std::fs::write(root.join("level.yaml"), yaml).unwrap();
    match dispatch(
        engine,
        Command::SceneCheck(SceneCheckParams {
            path: "level.yaml".into(),
        }),
    )
    .expect("scene.check answers")
    .body
    {
        ResponseBody::SceneChecked { diagnostics, .. } => diagnostics,
        other => panic!("expected SceneChecked, got {other:?}"),
    }
}

#[test]
fn scene_check_reports_class_typos_only_on_a_perception_configured_engine() {
    let root = std::env::temp_dir().join(format!("ludion-perception-check-{}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    let yaml = scene(&Names {
        class: Some("plyer"),
        ..Default::default()
    });
    let count = |diagnostics: &[Diagnostic]| {
        diagnostics
            .iter()
            .filter(|d| d.code == code::PERCEPTION_NAME_UNKNOWN)
            .count()
    };

    let mut configured = Engine::builder()
        .project_root(&root)
        .perception(config())
        .build()
        .unwrap();
    assert_eq!(
        count(&check_over_the_wire(&mut configured, &root, &yaml)),
        1
    );

    let mut plain = Engine::with_project_root(&root);
    assert_eq!(count(&check_over_the_wire(&mut plain, &root, &yaml)), 0);
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn the_gallery_scene_is_clean_under_both_passes() {
    let path =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/gallery/perception.yaml");
    let scene = parse(&std::fs::read_to_string(path).unwrap());
    assert!(scene_perception_diagnostics(&scene).is_empty());
    assert!(scene_perception_name_diagnostics(&scene, &config()).is_empty());
}
