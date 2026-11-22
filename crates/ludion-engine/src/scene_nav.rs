//! The scene's top-level `nav:` block (`Plan/debug-draw.md` decision 8): the built-in hosts read it
//! into a [`NavConfig`] and install [`Nav`](crate::Nav), so `ludion render --debug-draw nav` draws a
//! scene's grid with no game. A game's engine configures nav with `.nav(..)` and ignores the block.

use ludion_core::codes::code;
use ludion_core::diagnostics::Diagnostic;
use ludion_core::suggest;
use ludion_ecs::World;
use ludion_scene::{NAV_CLASS_KEY_ORDER, NAV_KEY_ORDER, Scene};
use serde_json::{Map, Value};

use crate::nav::{Nav, NavClass, NavConfig};

/// The scene's nav config: `None` when it authors no `nav:` block. The authored mapping is merged
/// over [`NavConfig::default`], so a scene writes only what differs.
///
/// # Errors
/// `L2330` for an unknown key, a wrong type or a class missing a field (at `/nav/...`); `L4032` for
/// a config [`NavConfig::validate`] refuses; `L2331` when the scene has no `physics:` block.
pub(crate) fn scene_nav_config(scene: &Scene) -> Result<Option<NavConfig>, Vec<Diagnostic>> {
    let Some(authored) = &scene.nav else {
        return Ok(None);
    };
    let mut diagnostics = Vec::new();
    let Some(authored) = authored.as_object() else {
        return Err(vec![invalid(
            "/nav",
            "`nav` must be a mapping".into(),
            None,
        )]);
    };
    let mut merged = match serde_json::to_value(NavConfig::default()) {
        Ok(Value::Object(map)) => map,
        _ => Map::new(),
    };
    for (key, value) in authored {
        let path = format!("/nav/{key}");
        if !NAV_KEY_ORDER.contains(&key.as_str()) {
            diagnostics.push(unknown_key(&path, "nav", key, &NAV_KEY_ORDER));
        } else if key == "classes" {
            class_diagnostics(value, &mut diagnostics);
        } else {
            let probe = Value::Object(Map::from_iter([(key.clone(), value.clone())]));
            if let Err(error) = from_merged(&merged, probe) {
                diagnostics.push(invalid(
                    &path,
                    format!("`nav.{key}` is the wrong type: {error}"),
                    None,
                ));
            }
        }
        merged.insert(key.clone(), value.clone());
    }
    if diagnostics.is_empty() {
        match serde_json::from_value::<NavConfig>(Value::Object(merged)) {
            Ok(config) => {
                if let Err(d) = config.validate() {
                    let path = format!("/nav{}", d.path.as_deref().unwrap_or(""));
                    diagnostics.push(d.with_path(path));
                }
                if diagnostics.is_empty() {
                    if scene.physics.is_none() {
                        diagnostics.push(needs_physics());
                    } else {
                        return Ok(Some(config));
                    }
                }
            }
            Err(error) => diagnostics.push(invalid("/nav", error.to_string(), None)),
        }
    } else if scene.physics.is_none() {
        diagnostics.push(needs_physics());
    }
    Err(diagnostics)
}

/// [`scene_nav_config`]'s diagnostics, for `check-scene`.
pub(crate) fn scene_nav_diagnostics(scene: &Scene) -> Vec<Diagnostic> {
    scene_nav_config(scene).err().unwrap_or_default()
}

/// Inserts [`Nav`] into `world` when the scene authors a `nav:` block.
pub(crate) fn install(world: &mut World, scene: &Scene) -> Result<(), Vec<Diagnostic>> {
    if let Some(config) = scene_nav_config(scene)? {
        world.insert_resource(Nav::new(config).map_err(|d| vec![d])?);
    }
    Ok(())
}

fn from_merged(base: &Map<String, Value>, probe: Value) -> Result<NavConfig, serde_json::Error> {
    let mut map = base.clone();
    if let Value::Object(probe) = probe {
        map.extend(probe);
    }
    serde_json::from_value(Value::Object(map))
}

fn class_diagnostics(value: &Value, diagnostics: &mut Vec<Diagnostic>) {
    let Some(items) = value.as_array() else {
        diagnostics.push(invalid(
            "/nav/classes",
            "`nav.classes` must be a list of classes".into(),
            None,
        ));
        return;
    };
    for (index, item) in items.iter().enumerate() {
        let path = format!("/nav/classes/{index}");
        let Some(map) = item.as_object() else {
            diagnostics.push(invalid(
                &path,
                format!("nav class {index} must be a mapping"),
                None,
            ));
            continue;
        };
        let mut ok = true;
        for key in map
            .keys()
            .filter(|key| !NAV_CLASS_KEY_ORDER.contains(&key.as_str()))
        {
            diagnostics.push(unknown_key(
                &format!("{path}/{key}"),
                "nav class",
                key,
                &NAV_CLASS_KEY_ORDER,
            ));
            ok = false;
        }
        if ok && let Err(error) = serde_json::from_value::<NavClass>(item.clone()) {
            let help = format!("a class sets all of {}", NAV_CLASS_KEY_ORDER.join(", "));
            diagnostics.push(invalid(
                &path,
                format!("nav class {index} is invalid: {error}"),
                Some(help),
            ));
        }
    }
}

fn unknown_key(path: &str, what: &str, key: &str, known: &[&str]) -> Diagnostic {
    let help = match suggest::nearest(key, known.iter().copied()) {
        Some(near) => format!("did you mean `{near}`? the keys are {}", known.join(", ")),
        None => format!("the keys are {}", known.join(", ")),
    };
    invalid(path, format!("`{key}` is not a {what} key"), Some(help))
}

fn invalid(path: &str, message: String, help: Option<String>) -> Diagnostic {
    let d = Diagnostic::new(code::SCENE_NAV_INVALID, message).with_path(path);
    d.with_help(help.unwrap_or_else(|| "see `ludion docs nav` for the nav keys".into()))
}

fn needs_physics() -> Diagnostic {
    Diagnostic::new(
        code::SCENE_NAV_NEEDS_PHYSICS,
        "the scene authors a `nav:` block but no `physics:` block, so its grid would sample no colliders",
    )
    .with_path("/nav")
    .with_help("add `physics: {space: free}` (or the space the scene uses) beside `nav:`")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::debug_draw::DebugCategories;
    use ludion_renderer::{FrameSize, QualityParams};

    const ENTITIES: &str = "entities:
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
              - 3.0
              - 0.5
              - 3.0
        body: static
  - name: ladder
    components:
      NavLink:
        from:
          - 0.0
          - 0.0
          - 0.0
        to:
          - 1.0
          - 1.0
          - 0.0
  - name: door
    components:
      Transform:
        translation:
          - -1.0
          - 1.0
          - 0.0
      NavBlocker:
        mode: interact
";

    const NAV: &str = "nav:
  origin:
    - -3.0
    - -3.0
  cell_size: 0.5
  nx: 12
  nz: 12
  y_min: -1.0
  y_max: 3.0
";

    fn scene(nav: &str, physics: bool) -> String {
        let physics = if physics {
            "physics:\n  space: free\n"
        } else {
            ""
        };
        format!("format: 1\nseed: 1\n{physics}{nav}{ENTITIES}")
    }

    fn diagnostics(text: &str) -> Vec<Diagnostic> {
        scene_nav_diagnostics(&ludion_scene::parse(text).unwrap())
    }

    fn extract(
        text: &str,
        frames: u64,
    ) -> (
        (
            ludion_renderer::RenderScene,
            Vec<ludion_scene::AppliedOverride>,
            Vec<&'static str>,
        ),
        std::path::PathBuf,
    ) {
        let dir = std::env::temp_dir().join(format!(
            "ludion-scene-nav-{}-{frames}-{}",
            std::process::id(),
            text.len()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("scene.yaml");
        std::fs::write(&path, text).unwrap();
        let extracted = crate::render::extract_scene_file(
            &crate::builtin_registry(),
            &path,
            FrameSize::native(64, 64),
            &[],
            frames,
            &QualityParams::default(),
            &DebugCategories::parse("nav"),
            None,
        )
        .unwrap_or_else(|d| panic!("{d:?}"));
        (extracted, dir)
    }

    #[test]
    fn a_nav_block_draws_grid_links_and_blockers() {
        for frames in [0, 2] {
            let ((render_scene, _, drawn), dir) = extract(&scene(NAV, true), frames);
            assert!(!render_scene.debug_lines.is_empty(), "frames {frames}");
            for category in ["nav:grid", "nav:links", "nav:blockers"] {
                assert!(drawn.contains(&category), "frames {frames}: {drawn:?}");
            }
            let _ = std::fs::remove_dir_all(dir);
        }
    }

    #[test]
    fn no_nav_block_draws_no_grid() {
        let ((render_scene, _, drawn), dir) = extract(&scene("", true), 0);
        assert!(!render_scene.debug_lines.is_empty());
        assert!(!drawn.contains(&"nav:grid"), "{drawn:?}");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn install_inserts_nav_only_when_authored() {
        let mut world = World::new();
        let parsed = ludion_scene::parse(&scene(NAV, true)).unwrap();
        install(&mut world, &parsed).unwrap();
        assert!(world.resource::<Nav>().is_some());
        let mut bare = World::new();
        install(&mut bare, &ludion_scene::parse(&scene("", true)).unwrap()).unwrap();
        assert!(bare.resource::<Nav>().is_none());
    }

    #[test]
    fn diagnostics_are_located_and_coded() {
        let hit = |text: &str, code: &str, path: &str| {
            let found = diagnostics(text);
            assert!(
                found
                    .iter()
                    .any(|d| d.code.as_str() == code && d.path.as_deref() == Some(path)),
                "{text}: {found:?}"
            );
            found
        };
        let found = hit(
            &scene("nav:\n  cel_size: 0.5\n", true),
            "L2330",
            "/nav/cel_size",
        );
        assert!(
            found[0].help.as_deref().unwrap().contains("cell_size"),
            "{found:?}"
        );
        hit(&scene("nav:\n  nx: a\n", true), "L2330", "/nav/nx");
        hit(
            &scene("nav:\n  cell_size: -1.0\n", true),
            "L4032",
            "/nav/cell_size",
        );
        hit(
            &scene("nav:\n  classes:\n    - name: a\n      height: 1.0\n", true),
            "L2330",
            "/nav/classes/0",
        );
        hit(&scene(NAV, false), "L2331", "/nav");
        assert!(diagnostics(&scene(NAV, true)).is_empty());
        assert!(diagnostics(&scene("", false)).is_empty());
        let scene = ludion_scene::parse(&scene("nav:\n  nx: a\n", true)).unwrap();
        let surfaced =
            crate::scene_check::scene_engine_diagnostics(std::path::Path::new("."), &scene);
        assert!(
            surfaced.iter().any(|d| d.code.as_str() == "L2330"),
            "{surfaced:?}"
        );
    }

    #[test]
    fn key_order_lists_match_the_config_structs() {
        let order = |schema: schemars::Schema| -> Vec<String> {
            let value = serde_json::to_value(schema).unwrap();
            value["properties"]
                .as_object()
                .unwrap()
                .keys()
                .cloned()
                .collect()
        };
        assert_eq!(order(schemars::schema_for!(NavConfig)), NAV_KEY_ORDER);
        assert_eq!(order(schemars::schema_for!(NavClass)), NAV_CLASS_KEY_ORDER);
    }

    #[test]
    fn scene_text_keeps_the_nav_block() {
        let mut engine = crate::Engine::new();
        engine.load_inline_scene(&scene(NAV, true)).unwrap();
        let text = engine.scene_text().unwrap();
        assert!(
            text.contains(
                "nav:\n  origin:\n    - -3.0\n    - -3.0\n  cell_size: 0.5\n  nx: 12\n  nz: 12\n  y_min: -1.0\n  y_max: 3.0\n"
            ),
            "{text}"
        );
    }
}
