//! The scene checks that need the filesystem, which `ludion-scene` (which does no IO) cannot make.
//!
//! [`ludion_scene::check`] answers everything the text can answer on its own. An atlas reference is
//! the one thing it cannot: an `atlas` + `frame` pair names a region of a *document on disk*, and a
//! pair that does not resolve draws nothing at all. This module is where that second pass lives, so
//! every client asking "is this scene valid" — `ludion check-scene`, a game binary's own
//! `--check-scene`, the wire's `scene.check` — runs the same one and cannot answer differently.

use std::collections::BTreeMap;
use std::path::Path;

use ludion_core::codes::code;
use ludion_core::diagnostics::Diagnostic;
use ludion_core::suggest;
use ludion_ecs::Registry;
use ludion_scene::{ExpandedScene, Scene};

use crate::components::pointer_token;
use crate::prefab::{prefab_key, read_prefab_text};

/// Expands the prefab instances of a checked `scene`, reading each prefab file from `project_root`.
///
/// The flat [`ExpandedScene::scene`] is what loads: run the engine's other passes over it and map
/// their diagnostics back to the document with [`ExpandedScene::locate`]. A scene with no instance
/// entries comes back equal, with no instances.
///
/// # Errors
/// `L1230` for an unreadable or absolute prefab path, `L1231`–`L1233` for a malformed instance, a
/// loop or a bad override, and the diagnostics of an invalid prefab file.
///
/// # Example
/// ```
/// use ludion_engine::expand_scene;
///
/// let root = std::env::temp_dir().join(format!("ludion-expand-scene-doc-{}", std::process::id()));
/// std::fs::create_dir_all(&root).unwrap();
/// std::fs::write(
///     root.join("crate.prefab.yaml"),
///     "format: 1\nkind: prefab\nname: crate\nentities:\n  - name: lid\n    components: {}\n",
/// )
/// .unwrap();
/// let registry = ludion_engine::builtin_registry();
/// let scene = ludion_scene::check(
///     "format: 1\nname: doc\nentities:\n  - name: crate-1\n    prefab: crate.prefab.yaml\n",
///     &registry,
/// )
/// .unwrap();
/// let expanded = expand_scene(&root, &registry, &scene).unwrap();
/// assert_eq!(expanded.scene.entities[0].name.as_deref(), Some("crate-1/lid"));
/// # std::fs::remove_dir_all(&root).ok();
/// ```
pub fn expand_scene(
    project_root: &Path,
    registry: &Registry,
    scene: &Scene,
) -> Result<ExpandedScene, Vec<Diagnostic>> {
    expand_scene_with(registry, scene, &mut |path| {
        let key = prefab_key(path).map_err(|mut found| found.swap_remove(0))?;
        read_prefab_text(project_root, &key)
    })
}

/// [`expand_scene`] over any `resolve`, which gets each prefab path as written.
pub(crate) fn expand_scene_with(
    registry: &Registry,
    scene: &Scene,
    resolve: &mut dyn FnMut(&str) -> Result<String, Diagnostic>,
) -> Result<ExpandedScene, Vec<Diagnostic>> {
    let expanded = ludion_scene::expand(scene, registry, resolve)?;
    let singletons = instance_singleton_diagnostics(&expanded);
    if singletons.is_empty() {
        Ok(expanded)
    } else {
        Err(singletons)
    }
}

/// Every `L1229` an expanded scene's instance parts report: a part, authored or overridden, that
/// carries a scene-wide singleton. Located at the instance entry, naming the part.
fn instance_singleton_diagnostics(expanded: &ExpandedScene) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();
    for instance in expanded.instances.iter().filter(|i| i.depth == 0) {
        for flat in instance.parts.clone() {
            for (name, _) in &expanded.scene.entities[flat].components {
                if !PREFAB_SINGLETONS.contains(&name.as_str()) {
                    continue;
                }
                diagnostics.push(
                    expanded.locate(
                        Diagnostic::new(
                            code::PREFAB_SINGLETON,
                            format!("a prefab part carries `{name}`, of which a scene has one"),
                        )
                        .with_path(format!("/entities/{flat}/components/{name}"))
                        .with_help("a prefab is spawned many times; put it in the scene instead"),
                    ),
                );
            }
        }
    }
    diagnostics
}

/// Every `L1151`/`L1152` a checked scene's atlas references report — one pass over each entity's
/// `Sprite`, each `UiRect`, and each `Tilemap` legend entry, through the same
/// [`check_atlas_reference`](crate::check_atlas_reference) `extract` resolves with, so a scene that
/// checks clean cannot fail to draw for a reason this pass could have named.
///
/// `project_root` is the content root the references resolve against — the scene file's own parent
/// directory, exactly as a host roots one.
pub fn scene_atlas_diagnostics(project_root: &Path, scene: &Scene) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();
    for (index, entity) in scene.entities.iter().enumerate() {
        for (name, value) in &entity.components {
            match name.as_str() {
                "Sprite" => diagnostics.extend(atlas_reference(
                    project_root,
                    &format!("/entities/{index}/components/Sprite"),
                    value,
                )),
                "UiRect" => diagnostics.extend(atlas_reference(
                    project_root,
                    &format!("/entities/{index}/components/UiRect"),
                    value,
                )),
                "Tilemap" => {
                    let Some(legend) = value.get("legend").and_then(serde_json::Value::as_object)
                    else {
                        continue;
                    };
                    for (glyph, tile) in legend {
                        diagnostics.extend(atlas_reference(
                            project_root,
                            &format!(
                                "/entities/{index}/components/Tilemap/legend/{}",
                                pointer_token(glyph)
                            ),
                            tile,
                        ));
                    }
                }
                _ => {}
            }
        }
        diagnostics.extend(sprite_crossfade(index, &entity.components));
    }
    diagnostics
}

/// `L1209` for a `Sprite` drawn from an atlas whose `Animation` authors a crossfade: a flipbook
/// hard-cuts, so `fade` would silently do nothing.
fn sprite_crossfade(
    index: usize,
    components: &[(String, serde_json::Value)],
) -> Option<Diagnostic> {
    let find = |wanted: &str| {
        components
            .iter()
            .find(|(name, _)| name == wanted)
            .map(|(_, value)| value)
    };
    find("Sprite")?.get("atlas")?.as_str()?;
    let fade = find("Animation")?.get("fade")?.as_f64()?;
    (fade > 0.0).then(|| {
        Diagnostic::new(
            code::ANIMATION_SPRITE_CROSSFADE,
            "a sprite flipbook cannot crossfade: `Animation.fade` is ignored and the sprite hard-cuts to `clip`",
        )
        .with_path(format!("/entities/{index}/components/Animation/fade"))
        .with_help("drop `fade` and `from`, or use a skinned character or an assembly")
    })
}

/// Every `L1293` a checked scene reports: a `sensor: true` collider the 3D solver does not claim,
/// where the flag would silently do nothing.
pub fn scene_sensor_diagnostics(scene: &Scene) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();
    for (index, entity) in scene.entities.iter().enumerate() {
        let Some((_, value)) = entity
            .components
            .iter()
            .find(|(name, _)| name == "Collider")
        else {
            continue;
        };
        let Ok(collider) = serde_json::from_value::<ludion_spatial::Collider>(value.clone()) else {
            continue;
        };
        if collider.sensor && !ludion_physics::claims(&collider) {
            diagnostics.push(
                Diagnostic::new(
                    code::COLLIDER_SENSOR_UNCLAIMED,
                    "collider `sensor` is ignored: the 3D solver does not claim this collider"
                        .to_string(),
                )
                .with_path(format!("/entities/{index}/components/Collider/sensor"))
                .with_help("give the collider a 3D shape (`sphere`, `box`, ...), or drop `sensor`"),
            );
        }
    }
    diagnostics
}

/// Every `L1186`/`L1187` a checked scene's collider `layer` fields report: a layer the scene's
/// `physics.layers` does not declare (with the nearest declared name), and a layer on a collider
/// the 3D solver never claims, where it would silently do nothing.
///
/// It lives here rather than in `ludion-scene` because the claim rule is
/// [`ludion_physics::claims`], and the scene crate names no spatial or physics type.
/// `layer: default` is always accepted; a scene with no `physics:` block declares nothing else.
pub fn scene_layer_diagnostics(scene: &Scene) -> Vec<Diagnostic> {
    let matrix = scene.physics.as_ref().map(|physics| physics.layer_matrix());
    let declared: Vec<&str> = match &matrix {
        Some(matrix) => matrix.names().iter().map(String::as_str).collect(),
        None => vec![ludion_scene::DEFAULT_LAYER],
    };
    let mut diagnostics = Vec::new();
    for (index, entity) in scene.entities.iter().enumerate() {
        if let Some((_, value)) = entity
            .components
            .iter()
            .find(|(name, _)| name == "CharacterMotor")
            && let Some(name) = value.get("layer").and_then(serde_json::Value::as_str)
            && !declared.contains(&name)
        {
            diagnostics.push(unknown_layer(
                name,
                &declared,
                format!("/entities/{index}/components/CharacterMotor/layer"),
                "character motor",
            ));
        }
        let Some((_, value)) = entity
            .components
            .iter()
            .find(|(name, _)| name == "Collider")
        else {
            continue;
        };
        let Ok(collider) = serde_json::from_value::<ludion_spatial::Collider>(value.clone()) else {
            continue;
        };
        let Some(name) = collider.layer.as_deref() else {
            continue;
        };
        let pointer = format!("/entities/{index}/components/Collider/layer");
        if !ludion_physics::claims(&collider) {
            diagnostics.push(
                Diagnostic::new(
                    code::COLLIDER_LAYER_UNCLAIMED,
                    format!(
                        "collider layer `{name}` is ignored: the 3D solver does not claim this \
                         collider"
                    ),
                )
                .with_path(pointer)
                .with_help(
                    "remove `layer`, or give the collider a 3D shape or a `rigid` body; the 2D \
                     tier has no filtering",
                ),
            );
        } else if !declared.contains(&name) {
            diagnostics.push(unknown_layer(name, &declared, pointer, "collider"));
        }
    }
    diagnostics
}

/// The `L1186` for a layer `name` the scene does not declare, with the nearest declared name.
fn unknown_layer(name: &str, declared: &[&str], pointer: String, owner: &str) -> Diagnostic {
    let help = match suggest::nearest(name, declared.iter().copied()) {
        Some(candidate) => format!("nearest match is `{candidate}`"),
        None => "declare it under `physics.layers`".to_string(),
    };
    Diagnostic::new(
        code::COLLIDER_LAYER_UNKNOWN,
        format!("{owner} layer `{name}` is not declared under `physics.layers`"),
    )
    .with_path(pointer)
    .with_help(help)
}

/// Every `L1295`/`L1296`/`L1297` a checked scene's character motors report: a `Collider` beside the
/// motor (the motor owns its capsule), a scene with no `physics:` block (the body is never built),
/// and a `Parent` (a motor writes world-space feet). The field rules (`L1294`) and the layer
/// (`L1186`) are reported elsewhere.
pub fn scene_motor_diagnostics(scene: &Scene) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();
    for (index, entity) in scene.entities.iter().enumerate() {
        let has = |wanted: &str| entity.components.iter().any(|(name, _)| name == wanted);
        if !has("CharacterMotor") {
            continue;
        }
        let at = format!("/entities/{index}/components");
        if has("Collider") {
            diagnostics.push(
                Diagnostic::new(
                    code::CHARACTER_MOTOR_WITH_COLLIDER,
                    "this entity carries a `Collider` beside its `CharacterMotor`; the motor owns \
                     its capsule and the `Collider` would be ignored",
                )
                .with_path(format!("{at}/Collider"))
                .with_help("remove the `Collider`; the motor owns its capsule"),
            );
        }
        if scene.physics.is_none() {
            diagnostics.push(
                Diagnostic::new(
                    code::CHARACTER_MOTOR_NO_PHYSICS_SCENE,
                    "this character motor belongs to the physics solver, but the scene has no \
                     `physics:` block to install it",
                )
                .with_path(format!("{at}/CharacterMotor"))
                .with_help("add `physics: { space: free }` to the scene"),
            );
        }
        if has("Parent") {
            diagnostics.push(
                Diagnostic::new(
                    code::CHARACTER_MOTOR_PARENTED,
                    "this character motor has a `Parent`; a motor writes world-space feet, so it \
                     must be a root entity",
                )
                .with_path(format!("{at}/Parent"))
                .with_help("remove the `Parent` and follow the mover from game code"),
            );
        }
    }
    diagnostics
}

/// Every `L1261` a checked scene's solver colliders report when the scene has no `physics:` block:
/// the solver is never installed, so a 3D shape or a rigid body would silently do nothing. The
/// claim rule is [`ludion_physics::claims`].
pub fn scene_physics_diagnostics(scene: &Scene) -> Vec<Diagnostic> {
    if scene.physics.is_some() {
        return Vec::new();
    }
    let mut diagnostics = Vec::new();
    for (index, entity) in scene.entities.iter().enumerate() {
        let Some(Ok(collider)) = entity
            .components
            .iter()
            .find(|(name, _)| name == "Collider")
            .map(|(_, value)| serde_json::from_value::<ludion_spatial::Collider>(value.clone()))
        else {
            continue;
        };
        if !ludion_physics::claims(&collider) {
            continue;
        }
        diagnostics.push(
            Diagnostic::new(
                code::COLLIDER_NO_PHYSICS_SCENE,
                "this collider belongs to the physics solver, but the scene has no `physics:` block \
                 to install it",
            )
            .with_path(format!("/entities/{index}/components/Collider"))
            .with_help(
                "add `physics: { space: free }` to the scene, or use a static or kinematic 2D \
                 `circle`/`aabb` collider",
            ),
        );
    }
    diagnostics
}

/// Every `L1221`/`L1222` a checked scene's parented colliders report: a rigid body or a static
/// solver collider on an entity with a `Parent` gets no physics body at runtime. A kinematic body
/// and a collider the solver does not claim (an in-plane static or kinematic `circle`/`aabb`) are
/// accepted. The claim rule is [`ludion_physics::claims`], which is why this lives here and not in
/// `ludion-scene`.
pub fn scene_hierarchy_body_diagnostics(scene: &Scene) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();
    for (index, entity) in scene.entities.iter().enumerate() {
        let find = |wanted: &str| {
            entity
                .components
                .iter()
                .find(|(name, _)| name == wanted)
                .map(|(_, value)| value)
        };
        let Some(parent) = find("Parent")
            .and_then(|value| value.get("index"))
            .and_then(serde_json::Value::as_u64)
            .and_then(|parent| usize::try_from(parent).ok())
        else {
            continue;
        };
        let Some(Ok(collider)) = find("Collider")
            .map(|value| serde_json::from_value::<ludion_spatial::Collider>(value.clone()))
        else {
            continue;
        };
        if !ludion_physics::claims(&collider) {
            continue;
        }
        let (diagnostic, kind, help) = match collider.body {
            ludion_spatial::Body::Rigid(_) => (
                code::HIERARCHY_RIGID_BODY,
                "a rigid body",
                "un-parent it, or join it to the parent with a `Joint`",
            ),
            ludion_spatial::Body::Static(_) => (
                code::HIERARCHY_STATIC_COLLIDER,
                "a static solver collider",
                "make it `body: kinematic`, or un-parent it",
            ),
            ludion_spatial::Body::Kinematic(_) => continue,
        };
        let parent_label = match scene.entities.get(parent).and_then(|p| p.name.as_ref()) {
            Some(name) => format!("entity {parent} (`{name}`)"),
            None => format!("entity {parent}"),
        };
        diagnostics.push(
            Diagnostic::new(
                diagnostic,
                format!("{kind} sits under {parent_label} and would get no physics body"),
            )
            .with_path(format!("/entities/{index}/components/Collider/body"))
            .with_help(help),
        );
    }
    diagnostics
}

/// The components a scene has one of: a prefab is spawned many times, so a part carrying one is
/// refused (`L1229`).
const PREFAB_SINGLETONS: [&str; 5] = [
    "Camera",
    "DirectionalLight",
    "Environment",
    "PostProcess",
    "GiVolume",
];

/// Every `L1229` a prefab body reports: one per part component that is a scene-wide singleton
/// (`Camera`, `DirectionalLight`, `Environment`, `PostProcess`, `GiVolume`). Takes the body as a
/// [`Scene`] (`PrefabDocument::to_scene`) like its siblings; it names concrete components, which is
/// why it lives here and not in `ludion-scene`.
pub fn prefab_singleton_diagnostics(scene: &Scene) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();
    for (index, entity) in scene.entities.iter().enumerate() {
        for (name, _) in &entity.components {
            if !PREFAB_SINGLETONS.contains(&name.as_str()) {
                continue;
            }
            diagnostics.push(
                Diagnostic::new(
                    code::PREFAB_SINGLETON,
                    format!("a prefab part carries `{name}`, of which a scene has one"),
                )
                .with_path(format!("/entities/{index}/components/{name}"))
                .with_help("a prefab is spawned many times; put it in the scene instead"),
            );
        }
    }
    diagnostics
}

/// The diagnostics one component value's atlas reference reports; empty when it carries none (a
/// loose `texture:` or an untextured quad has no atlas to resolve).
fn atlas_reference(root: &Path, pointer: &str, value: &serde_json::Value) -> Vec<Diagnostic> {
    let Some(atlas) = value.get("atlas").and_then(serde_json::Value::as_str) else {
        return Vec::new();
    };
    crate::check_atlas_reference(
        root,
        pointer,
        atlas,
        value.get("frame").and_then(serde_json::Value::as_str),
    )
}

/// The load faults of the font table at `project_root` (`L1253` for `fonts.yaml` itself, `L1255` for
/// a face), each paired with the root-relative file it is in; their pointers are into that file,
/// never the scene.
pub fn font_table_faults(project_root: &Path) -> Vec<(String, Diagnostic)> {
    crate::FontLibrary::load(project_root)
        .faults()
        .map(|(file, d)| (file.to_string(), d.clone()))
        .collect()
}

/// Every font diagnostic a checked scene's text components report, against the `fonts.yaml` at
/// `project_root`: a `font` naming a face the table lacks (`L1254`, with the nearest name), and one
/// `L1256` **warning** per text field whose chars the resolved table face has no glyph for (line
/// breaks are not chars and a tab lays out as a space). Coverage is checked for table faces only,
/// never the embedded 8×8 face, whose gaps draw blank by design. The table's own faults come from
/// [`font_table_faults`].
///
/// Errors and warnings come back together; `Diagnostic::severity` tells them apart.
pub fn scene_font_diagnostics(project_root: &Path, scene: &Scene) -> Vec<Diagnostic> {
    use crate::fonts::TableState;

    let library = crate::FontLibrary::load(project_root);
    let mut diagnostics: Vec<Diagnostic> = Vec::new();
    for (index, entity) in scene.entities.iter().enumerate() {
        for (name, value) in &entity.components {
            let fields: Vec<(String, &str)> = match name.as_str() {
                "Text" => text_field(value, "text", "text").into_iter().collect(),
                "UiText" => text_field(value, "text", "text").into_iter().collect(),
                "UiList" => {
                    let mut fields: Vec<(String, &str)> =
                        text_field(value, "title", "title").into_iter().collect();
                    if let Some(items) = value.get("items").and_then(serde_json::Value::as_array) {
                        for (k, item) in items.iter().enumerate() {
                            fields.extend(text_field(item, "text", &format!("items/{k}/text")));
                        }
                    }
                    fields
                }
                _ => continue,
            };
            let base = format!("/entities/{index}/components/{name}");
            let font = value.get("font").and_then(serde_json::Value::as_str);
            if let Some(font) = font.filter(|f| !library.contains(f)) {
                let help = match suggest::nearest(font, library.names()) {
                    Some(near) => format!("did you mean `{near}`?"),
                    None if library.names().next().is_some() => {
                        format!(
                            "`fonts.yaml` defines: {}",
                            library.names().collect::<Vec<_>>().join(", ")
                        )
                    }
                    None => match library.table_state() {
                        TableState::Absent => {
                            "add a `fonts.yaml` (`kind: fonts`) at the content root".to_string()
                        }
                        TableState::Faulted => {
                            "fix `fonts.yaml` first (its error is reported above)".to_string()
                        }
                        TableState::Loaded => "`fonts.yaml` defines no faces".to_string(),
                    },
                };
                diagnostics.push(
                    Diagnostic::new(
                        code::FONT_NAME_UNKNOWN,
                        format!(
                            "`{name}` names the font `{font}`, which `fonts.yaml` does not define"
                        ),
                    )
                    .with_path(format!("{base}/font"))
                    .with_help(help),
                );
                continue;
            }
            let Some(face) = library.table_face(font) else {
                continue;
            };
            for (field, text) in fields {
                let mut missing: Vec<char> = text
                    .chars()
                    .map(|c| if c == '\t' { ' ' } else { c })
                    .filter(|&c| c != '\n' && c != '\r' && face.glyph(c).is_none())
                    .collect();
                missing.sort_unstable();
                missing.dedup();
                if missing.is_empty() {
                    continue;
                }
                let listed: String = missing.iter().collect();
                let dir = library
                    .paths(font)
                    .and_then(|(doc, _)| doc.rsplit_once('/'))
                    .map_or("", |(dir, _)| dir);
                let help = match face.document().generated_from.as_deref() {
                    Some(source) => {
                        let source = if dir.is_empty() {
                            source.to_string()
                        } else {
                            format!("{dir}/{source}")
                        };
                        format!(
                            "add them to `charset.chars` in `{source}` and re-run `ludion generate-atlas {source}`"
                        )
                    }
                    None => "add them to the face's `charset.chars` and re-bake it".to_string(),
                };
                diagnostics.push(
                    Diagnostic::new(
                        code::FONT_GLYPH_MISSING,
                        format!("the text has characters the font lacks: {listed:?}"),
                    )
                    .with_path(format!("{base}/{field}"))
                    .with_help(help),
                );
            }
        }
    }
    diagnostics
}

/// Every `L1101`/`L1104` a checked scene's `AudioEmitter.sound` paths report: each is decoded with
/// [`ludion_assets::load_wav`], the call the audio sink makes, so a sound that checks clean cannot
/// fail to play for a reason this pass could have named. An empty `sound` is `L1101` too. Check-time
/// only: audio never moves the sim, so scene loading does not run it.
///
/// `project_root` is the content root the paths resolve against.
///
/// # Example
/// ```
/// use ludion_engine::scene_audio_diagnostics;
///
/// let root = std::env::temp_dir().join(format!("ludion-audio-diag-doc-{}", std::process::id()));
/// std::fs::create_dir_all(&root).unwrap();
/// let scene = ludion_scene::check(
///     "format: 1\nentities:\n  - name: lamp\n    components:\n      AudioEmitter:\n        sound: hum.wav\n",
///     &ludion_engine::builtin_registry(),
/// )
/// .unwrap();
/// let found = scene_audio_diagnostics(&root, &scene);
/// assert_eq!(found[0].code.as_str(), "L1101");
/// assert_eq!(
///     found[0].path.as_deref(),
///     Some("/entities/0/components/AudioEmitter/sound")
/// );
/// # std::fs::remove_dir_all(&root).ok();
/// ```
pub fn scene_audio_diagnostics(project_root: &Path, scene: &Scene) -> Vec<Diagnostic> {
    let mut decoded: BTreeMap<String, Option<Diagnostic>> = BTreeMap::new();
    let mut diagnostics = Vec::new();
    for (index, entity) in scene.entities.iter().enumerate() {
        let Some((_, emitter)) = entity
            .components
            .iter()
            .find(|(name, _)| name == "AudioEmitter")
        else {
            continue;
        };
        let sound = emitter
            .get("sound")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("");
        let pointer = format!("/entities/{index}/components/AudioEmitter/sound");
        let fault = if sound.is_empty() {
            Some(
                Diagnostic::new(
                    code::ASSET_FILE_IO,
                    "this `AudioEmitter` names no sound, so it plays nothing",
                )
                .with_help("set `sound` to a WAV file under the content root, e.g. `sfx/hum.wav`"),
            )
        } else {
            decoded
                .entry(sound.to_string())
                .or_insert_with(|| ludion_assets::load_wav(project_root, sound).err())
                .clone()
        };
        diagnostics.extend(fault.map(|d| d.with_path(pointer)));
    }
    diagnostics
}

/// Every engine pass over an expanded scene, in one list: atlas, font, collider layer, parented
/// body, physics, preload, skin bone, audio and reflection probe diagnostics, errors and warnings together
/// (`severity` tells them apart). `ludion check-scene`, a game's `--check-scene` and `scene.check` all run exactly
/// this, so no client answers "is this scene valid" differently.
///
/// The diagnostics are unlocated: map each through [`ExpandedScene::locate`] to point into the
/// authored document. The font table's own faults point into `fonts.yaml`, not the scene, and
/// come from [`font_table_faults`].
///
/// # Example
/// ```
/// use ludion_engine::scene_engine_diagnostics;
///
/// let root = std::env::temp_dir().join(format!("ludion-engine-diag-doc-{}", std::process::id()));
/// std::fs::create_dir_all(&root).unwrap();
/// let scene = ludion_scene::check("format: 1\nentities: []\n", &ludion_engine::builtin_registry())
///     .unwrap();
/// assert!(scene_engine_diagnostics(&root, &scene).is_empty());
/// # std::fs::remove_dir_all(&root).ok();
/// ```
pub fn scene_engine_diagnostics(project_root: &Path, scene: &Scene) -> Vec<Diagnostic> {
    let mut diagnostics = scene_atlas_diagnostics(project_root, scene);
    diagnostics.extend(scene_font_diagnostics(project_root, scene));
    diagnostics.extend(scene_layer_diagnostics(scene));
    diagnostics.extend(scene_sensor_diagnostics(scene));
    diagnostics.extend(scene_hierarchy_body_diagnostics(scene));
    diagnostics.extend(scene_physics_diagnostics(scene));
    diagnostics.extend(scene_motor_diagnostics(scene));
    diagnostics.extend(scene_preload_diagnostics(project_root, scene));
    diagnostics.extend(crate::skin_bone::scene_skin_bone_diagnostics(
        project_root,
        scene,
    ));
    diagnostics.extend(scene_audio_diagnostics(project_root, scene));
    diagnostics.extend(crate::probe_check::scene_probe_diagnostics(scene));
    diagnostics
}

/// `L1230` at `/preload/{i}` for each preload entry whose prefab text cannot be read. Existence
/// only: the full prefab passes need the registry's spawn path and run at load.
fn scene_preload_diagnostics(project_root: &Path, scene: &Scene) -> Vec<Diagnostic> {
    let mut found = Vec::new();
    for (index, path) in scene.preload.iter().enumerate() {
        let read = crate::prefab::prefab_key(path)
            .map_err(|mut found| found.swap_remove(0))
            .and_then(|key| crate::prefab::read_prefab_text(project_root, &key));
        if let Err(mut error) = read {
            error.message = format!("the scene preloads `{path}`: {}", error.message);
            found.push(error.with_path(format!("/preload/{index}")));
        }
    }
    found
}

/// A string field of a text component with the pointer suffix it reports under.
fn text_field<'a>(
    value: &'a serde_json::Value,
    key: &str,
    pointer: &str,
) -> Option<(String, &'a str)> {
    value
        .get(key)
        .and_then(serde_json::Value::as_str)
        .map(|text| (pointer.to_string(), text))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unknown_font_help(table: Option<&str>) -> String {
        let dir = std::env::temp_dir().join(format!(
            "ludion-scene-check-fonts-{}-{}",
            table.is_some(),
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("scratch dir");
        if let Some(table) = table {
            std::fs::write(dir.join("fonts.yaml"), table).expect("table");
        }
        let scene = ludion_scene::check(
            "format: 1\nseed: 1\nentities:\n  - name: t\n    components:\n      Text:\n        \
             text: HI\n        font: body\n",
            &crate::builtin_registry(),
        )
        .expect("fixture scene");
        let found = scene_font_diagnostics(&dir, &scene);
        assert_eq!(found.len(), 1, "{found:?}");
        assert_eq!(found[0].code, code::FONT_NAME_UNKNOWN);
        found[0].help.clone().expect("a help")
    }

    #[test]
    fn the_unknown_font_help_tells_the_three_table_states_apart() {
        assert!(unknown_font_help(None).contains("add a `fonts.yaml`"));
        assert!(
            unknown_font_help(Some("format: 1\nkind: fonts\nfonts: 3\n"))
                .contains("fix `fonts.yaml` first")
        );
        assert!(
            unknown_font_help(Some("format: 1\nkind: fonts\nfonts: {}\n"))
                .contains("defines no faces")
        );
    }

    const LAYERS: &str = concat!(
        "physics:\n  space: free\n",
        "  layers:\n    - name: dummy\n      ignores:\n        - dummy\n"
    );

    fn collider(shape: &str, body: &str, layer: &str) -> String {
        format!(
            "entities:\n  - name: part\n    components:\n      Collider:\n        shape:\n\
             {pad}{shape}\n        body: {body}\n        layer: {layer}\n",
            pad = " ".repeat(10)
        )
    }

    fn diagnostics(physics: &str, entities: &str) -> Vec<Diagnostic> {
        let source = format!("format: 1\nseed: 1\n{physics}{entities}");
        let scene = ludion_scene::check(&source, &crate::builtin_registry())
            .unwrap_or_else(|errors| panic!("fixture scene invalid: {errors:?}"));
        scene_layer_diagnostics(&scene)
    }

    fn sensor_diagnostics(shape: &str, body: &str) -> Vec<Diagnostic> {
        let source = format!(
            "format: 1
seed: 1
entities:
  - name: zone
    components:
      Collider:
        shape:
          {shape}
        body: {body}
        sensor: true
"
        );
        let scene = ludion_scene::check(&source, &crate::builtin_registry())
            .unwrap_or_else(|errors| panic!("fixture scene invalid: {errors:?}"));
        scene_sensor_diagnostics(&scene)
    }

    #[test]
    fn a_sensor_on_an_unclaimed_collider_is_l1293_and_a_3d_one_is_clean() {
        let found = sensor_diagnostics(
            "circle:
            radius: 0.5",
            "static",
        );
        assert_eq!(found.len(), 1, "{found:?}");
        assert_eq!(found[0].code.as_str(), "L1293");
        assert_eq!(
            found[0].path.as_deref(),
            Some("/entities/0/components/Collider/sensor")
        );
        let clean = sensor_diagnostics(
            "sphere:
            radius: 0.5",
            "static",
        );
        assert!(clean.is_empty(), "{clean:?}");
    }

    fn parented_collider(shape: &str, body: &str) -> Vec<Diagnostic> {
        let source = format!(
            "format: 1\nseed: 1\nentities:\n  - name: platform\n    components: {{}}\n  \
             - name: part\n    components:\n      Parent:\n        index: 0\n        generation: 0\n      \
             Collider:\n        shape:\n          {shape}\n        body: {body}\n"
        );
        let scene = ludion_scene::check(&source, &crate::builtin_registry())
            .unwrap_or_else(|errors| panic!("fixture scene invalid: {errors:?}"));
        scene_hierarchy_body_diagnostics(&scene)
    }

    const BOX: &str = "box:
            half_extents:
              - 0.5
              - 0.5
              - 0.5";
    const AABB: &str = "aabb:
            half_extents:
              - 0.5
              - 0.5";

    #[test]
    fn a_rigid_child_is_l1221_and_a_static_solver_child_is_l1222() {
        let rigid = parented_collider(
            BOX,
            "
          rigid: {}",
        );
        assert_eq!(rigid.len(), 1);
        assert_eq!(rigid[0].code, code::HIERARCHY_RIGID_BODY);
        assert_eq!(
            rigid[0].path.as_deref(),
            Some("/entities/1/components/Collider/body")
        );
        assert!(rigid[0].message.contains("`platform`"));
        let fixed = parented_collider(BOX, "static");
        assert_eq!(fixed.len(), 1);
        assert_eq!(fixed[0].code, code::HIERARCHY_STATIC_COLLIDER);
    }

    #[test]
    fn a_solver_collider_without_a_physics_block_is_l1261() {
        let entities = "entities:\n  - name: ball\n    components:\n      Collider:\n        \
                        shape:\n          sphere:\n            radius: 0.5\n        body:\n          \
                        rigid: {}\n";
        let check = |physics: &str| {
            let source = format!("format: 1\nseed: 1\n{physics}{entities}");
            let scene = ludion_scene::check(&source, &crate::builtin_registry())
                .unwrap_or_else(|errors| panic!("fixture scene invalid: {errors:?}"));
            scene_physics_diagnostics(&scene)
        };
        let missing = check("");
        assert_eq!(missing.len(), 1);
        assert_eq!(missing[0].code, code::COLLIDER_NO_PHYSICS_SCENE);
        assert_eq!(
            missing[0].path.as_deref(),
            Some("/entities/0/components/Collider")
        );
        assert!(check("physics:\n  space: free\n").is_empty());
    }

    #[test]
    fn a_kinematic_child_and_a_2d_tier_child_are_accepted() {
        assert!(parented_collider(BOX, "kinematic").is_empty());
        assert!(parented_collider(AABB, "static").is_empty());
    }

    fn prefab_body(component: &str) -> Vec<Diagnostic> {
        let source = format!(
            "format: 1\nkind: prefab\nentities:\n  - name: part\n    components:\n      {component}\n"
        );
        let doc = ludion_scene::check_prefab(&source, &crate::builtin_registry())
            .unwrap_or_else(|errors| panic!("fixture prefab invalid: {errors:?}"));
        prefab_singleton_diagnostics(&doc.to_scene())
    }

    #[test]
    fn a_camera_part_is_l1229_and_a_point_light_part_is_accepted() {
        let refused = prefab_body("Camera: {}");
        assert_eq!(refused.len(), 1);
        assert_eq!(refused[0].code, code::PREFAB_SINGLETON);
        assert_eq!(
            refused[0].path.as_deref(),
            Some("/entities/0/components/Camera")
        );
        assert!(prefab_body("PointLight: {}").is_empty());
    }

    #[test]
    fn an_atlas_sprite_crossfade_is_l1209() {
        let scene_with = |animation: &str| {
            let source = format!(
                "format: 1\nseed: 1\nentities:\n  - name: hero\n    components:\n      Sprite:\n        \
                 atlas: atlas.yaml\n        frame: idle\n      Animation:\n{animation}"
            );
            ludion_scene::check(&source, &crate::builtin_registry())
                .unwrap_or_else(|errors| panic!("fixture scene invalid: {errors:?}"))
        };
        let count = |scene: &Scene| {
            scene_atlas_diagnostics(Path::new("."), scene)
                .into_iter()
                .filter(|d| d.code == code::ANIMATION_SPRITE_CROSSFADE)
                .collect::<Vec<_>>()
        };
        let fading = count(&scene_with(
            "        clip: run\n        from: walk\n        fade: 0.5\n",
        ));
        assert_eq!(fading.len(), 1);
        assert_eq!(
            fading[0].path.as_deref(),
            Some("/entities/0/components/Animation/fade")
        );
        assert!(count(&scene_with("        clip: run\n")).is_empty());
    }

    #[test]
    fn a_typo_names_the_nearest_declared_layer() {
        let found = diagnostics(
            LAYERS,
            &collider("sphere:\n            radius: 0.5", "static", "dummi"),
        );
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].code, code::COLLIDER_LAYER_UNKNOWN);
        assert_eq!(
            found[0].path.as_deref(),
            Some("/entities/0/components/Collider/layer")
        );
        assert_eq!(found[0].help.as_deref(), Some("nearest match is `dummy`"));
    }

    fn motor_scene(physics: &str, components: &str) -> Scene {
        let source = format!(
            "format: 1\nseed: 1\n{physics}entities:\n  - name: hero\n    components:\n{components}  - name: base\n    components: {{}}\n"
        );
        ludion_scene::check(&source, &crate::builtin_registry())
            .unwrap_or_else(|errors| panic!("fixture scene invalid: {errors:?}"))
    }

    #[test]
    fn a_motor_layer_typo_names_the_nearest_declared_layer() {
        let scene = motor_scene(LAYERS, "      CharacterMotor:\n        layer: dummi\n");
        let found = scene_layer_diagnostics(&scene);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].code, code::COLLIDER_LAYER_UNKNOWN);
        assert_eq!(
            found[0].path.as_deref(),
            Some("/entities/0/components/CharacterMotor/layer")
        );
        assert_eq!(found[0].help.as_deref(), Some("nearest match is `dummy`"));
    }

    #[test]
    fn a_clean_motor_scene_reports_nothing() {
        let scene = motor_scene(LAYERS, "      CharacterMotor:\n        layer: dummy\n");
        assert!(scene_layer_diagnostics(&scene).is_empty());
        assert!(scene_motor_diagnostics(&scene).is_empty());
    }

    #[test]
    fn a_motor_beside_a_collider_is_l1295() {
        let scene = motor_scene(
            LAYERS,
            "      CharacterMotor: {}\n      Collider:\n        shape:\n          sphere:\n            radius: 0.5\n",
        );
        let found = scene_motor_diagnostics(&scene);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].code, code::CHARACTER_MOTOR_WITH_COLLIDER);
        assert_eq!(
            found[0].path.as_deref(),
            Some("/entities/0/components/Collider")
        );
    }

    #[test]
    fn a_motor_without_a_physics_block_is_l1296() {
        let scene = motor_scene("", "      CharacterMotor: {}\n");
        let found = scene_motor_diagnostics(&scene);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].code, code::CHARACTER_MOTOR_NO_PHYSICS_SCENE);
        assert_eq!(
            found[0].path.as_deref(),
            Some("/entities/0/components/CharacterMotor")
        );
    }

    #[test]
    fn a_parented_motor_is_l1297() {
        let scene = motor_scene(
            LAYERS,
            "      CharacterMotor: {}\n      Parent:\n        index: 1\n        generation: 0\n",
        );
        let found = scene_motor_diagnostics(&scene);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].code, code::CHARACTER_MOTOR_PARENTED);
        assert_eq!(
            found[0].path.as_deref(),
            Some("/entities/0/components/Parent")
        );
    }

    #[test]
    fn a_2d_static_circle_with_a_layer_is_unclaimed() {
        let found = diagnostics(
            LAYERS,
            &collider("circle:\n            radius: 0.5", "static", "dummy"),
        );
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].code, code::COLLIDER_LAYER_UNCLAIMED);
    }

    #[test]
    fn a_rigid_circle_with_a_declared_layer_is_clean() {
        let found = diagnostics(
            LAYERS,
            &collider(
                "circle:\n            radius: 0.5",
                "\n          rigid:\n            mass: 1.0",
                "dummy",
            ),
        );
        assert!(found.is_empty(), "{found:?}");
    }

    #[test]
    fn default_is_always_declared() {
        let found = diagnostics(
            "",
            &collider("sphere:\n            radius: 0.5", "static", "default"),
        );
        assert!(found.is_empty(), "{found:?}");
    }

    fn tiny_wav() -> Vec<u8> {
        let samples = [0i16, 1000, -1000, 0];
        let data_len = samples.len() * 2;
        let mut bytes = Vec::new();
        bytes.extend_from_slice(b"RIFF");
        bytes.extend_from_slice(&(36 + data_len as u32).to_le_bytes());
        bytes.extend_from_slice(b"WAVEfmt ");
        bytes.extend_from_slice(&16u32.to_le_bytes());
        bytes.extend_from_slice(&1u16.to_le_bytes()); // PCM
        bytes.extend_from_slice(&1u16.to_le_bytes()); // mono
        bytes.extend_from_slice(&8000u32.to_le_bytes());
        bytes.extend_from_slice(&16000u32.to_le_bytes());
        bytes.extend_from_slice(&2u16.to_le_bytes());
        bytes.extend_from_slice(&16u16.to_le_bytes());
        bytes.extend_from_slice(b"data");
        bytes.extend_from_slice(&(data_len as u32).to_le_bytes());
        for s in samples {
            bytes.extend_from_slice(&s.to_le_bytes());
        }
        bytes
    }

    /// The audio diagnostics of a scene with one `AudioEmitter` per `sounds` entry, against a scratch
    /// root holding `good.wav` (valid) and `bad.wav` (not a WAV).
    fn audio_diagnostics(sounds: &[&str]) -> Vec<Diagnostic> {
        static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "ludion-scene-check-audio-{}-{n}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).expect("scratch dir");
        std::fs::write(dir.join("good.wav"), tiny_wav()).expect("good.wav");
        std::fs::write(dir.join("bad.wav"), b"not a wav").expect("bad.wav");
        let mut source = String::from("format: 1\nseed: 1\nentities:\n");
        for sound in sounds {
            source.push_str(&format!(
                "  - components:\n      AudioEmitter:\n        sound: \"{sound}\"\n"
            ));
        }
        let scene = ludion_scene::check(&source, &crate::builtin_registry())
            .unwrap_or_else(|errors| panic!("fixture scene invalid: {errors:?}"));
        let found = scene_audio_diagnostics(&dir, &scene);
        std::fs::remove_dir_all(&dir).ok();
        found
    }

    #[test]
    fn a_missing_sound_is_l1101_at_its_pointer() {
        let found = audio_diagnostics(&["good.wav", "gone.wav"]);
        assert_eq!(found.len(), 1, "{found:?}");
        assert_eq!(found[0].code, code::ASSET_FILE_IO);
        assert_eq!(
            found[0].path.as_deref(),
            Some("/entities/1/components/AudioEmitter/sound")
        );
    }

    #[test]
    fn a_non_wav_sound_is_l1104() {
        let found = audio_diagnostics(&["bad.wav"]);
        assert_eq!(found.len(), 1, "{found:?}");
        assert_eq!(found[0].code, code::ASSET_WAV_DECODE);
    }

    #[test]
    fn an_empty_sound_is_l1101_with_its_own_message() {
        let found = audio_diagnostics(&[""]);
        assert_eq!(found.len(), 1, "{found:?}");
        assert_eq!(found[0].code, code::ASSET_FILE_IO);
        assert!(
            found[0].message.contains("names no sound"),
            "{}",
            found[0].message
        );
    }

    #[test]
    fn two_emitters_sharing_a_bad_path_each_report_it() {
        let found = audio_diagnostics(&["bad.wav", "good.wav", "bad.wav"]);
        let pointers: Vec<_> = found.iter().filter_map(|d| d.path.as_deref()).collect();
        assert_eq!(
            pointers,
            [
                "/entities/0/components/AudioEmitter/sound",
                "/entities/2/components/AudioEmitter/sound"
            ]
        );
    }

    #[test]
    fn a_valid_sound_is_clean() {
        assert!(audio_diagnostics(&["good.wav"]).is_empty());
    }

    #[test]
    fn no_physics_block_declares_nothing() {
        let found = diagnostics(
            "",
            &collider("sphere:\n            radius: 0.5", "static", "dummy"),
        );
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].code, code::COLLIDER_LAYER_UNKNOWN);
    }
}
