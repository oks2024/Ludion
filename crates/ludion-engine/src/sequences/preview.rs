//! The CLI preview (`ludion render --sequence`): plays one `kind: sequence` document over a bare-host
//! scene, bound by entity name, and stops at a chosen tick. The render side lives in
//! `crate::render::sequence_preview_world`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use ludion_core::codes::code;
use ludion_core::diagnostics::{Diagnostic, Severity};
use ludion_core::ids::EntityId;
use ludion_core::suggest::nearest;
use ludion_core::time::GameClock;
use ludion_ecs::World;
use ludion_sim::SimDriver;

use super::audio::SequenceAudio;
use super::{
    SEQUENCE_KIND, SequenceDoc, SequenceEnd, SequenceEvent, SequenceEvents, SequenceHandle, play,
    release, validate_sequence_docs,
};
use crate::{Diagnostics, DocStore, Name};

/// What `ludion render --sequence` plays: one document, its role bindings and the tick to stop at.
///
/// # Example
/// ```
/// use ludion_engine::SequencePreview;
///
/// let preview = SequencePreview {
///     doc: "sequences/intro.yaml".into(),
///     bindings: vec![("cam".into(), "camera".into())],
///     at: 12,
/// };
/// assert_eq!(preview.at, 12);
/// ```
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SequencePreview {
    /// The document's path; it must lie under the scene's folder (the content root).
    pub doc: PathBuf,
    /// `(role, entity name)` pairs; each name must match exactly one entity's `Name`.
    pub bindings: Vec<(String, String)>,
    /// The playhead tick to render: the world right after the update whose playhead is `at`, so the
    /// preview steps `at + 1` ticks.
    pub at: u64,
}

fn doc_error(message: String) -> Vec<Diagnostic> {
    vec![
        Diagnostic::new(code::SEQUENCE_PREVIEW_DOC, message)
            .with_help("a preview loads one doc from the scene's folder (its content root)"),
    ]
}

/// The document's content-root key: `doc` made absolute, relative to the absolute `root`, `/`-joined.
fn key_of(root: &Path, doc: &Path) -> Result<String, Vec<Diagnostic>> {
    let absolute = |path: &Path| {
        let path = if path.as_os_str().is_empty() {
            Path::new(".")
        } else {
            path
        };
        std::path::absolute(path)
    };
    let (Ok(root_abs), Ok(doc_abs)) = (absolute(root), absolute(doc)) else {
        return Err(doc_error(format!("could not resolve `{}`", doc.display())));
    };
    let relative = doc_abs.strip_prefix(&root_abs).map_err(|_| {
        doc_error(format!(
            "`{}` is not under the scene's folder `{}`",
            doc.display(),
            root.display()
        ))
    })?;
    Ok(relative
        .components()
        .map(|c| c.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/"))
}

fn read_doc(path: &Path) -> Result<SequenceDoc, Vec<Diagnostic>> {
    let source = std::fs::read_to_string(path)
        .map_err(|error| doc_error(format!("could not read `{}`: {error}", path.display())))?;
    match ludion_scene::peek_kind_checked(&source) {
        Ok(Some(kind)) if kind == SEQUENCE_KIND => {}
        Ok(other) => {
            return Err(doc_error(format!(
                "`{}` is not a `kind: {SEQUENCE_KIND}` document (its kind is {})",
                path.display(),
                other.map_or_else(|| "absent".to_owned(), |kind| format!("`{kind}`")),
            )));
        }
        Err(diagnostic) => return Err(vec![diagnostic]),
    }
    let schema = serde_json::to_value(schemars::schema_for!(SequenceDoc))
        .expect("a schemars JSON Schema always serializes");
    let fields = ludion_scene::check_doc(&source, SEQUENCE_KIND, &schema)?;
    ludion_scene::deserialize_doc::<SequenceDoc>(fields).map_err(|d| vec![d])
}

fn find_entity(world: &mut World, name: &str) -> Result<EntityId, Vec<Diagnostic>> {
    let named: Vec<(EntityId, String)> = world
        .query_ids::<(&Name,)>()
        .map(|(id, (n,))| (id, n.0.clone()))
        .collect();
    let matches: Vec<EntityId> = named
        .iter()
        .filter(|(_, n)| n == name)
        .map(|(id, _)| *id)
        .collect();
    match matches[..] {
        [one] => Ok(one),
        [] => {
            let mut diagnostic = Diagnostic::new(
                code::SEQUENCE_PREVIEW_BIND,
                format!("no entity is named `{name}`"),
            );
            diagnostic = match nearest(name, named.iter().map(|(_, n)| n.as_str())) {
                Some(close) => diagnostic.with_help(format!("did you mean `{close}`?")),
                None => diagnostic.with_help("bind a role to the `name:` of a scene entity"),
            };
            Err(vec![diagnostic])
        }
        _ => Err(vec![
            Diagnostic::new(
                code::SEQUENCE_PREVIEW_BIND,
                format!("the name `{name}` names {} entities", matches.len()),
            )
            .with_help("give the entity a unique `name:` in the scene"),
        ]),
    }
}

/// Loads `preview.doc` into `world` as the only sequence document, binds each role to the entity of
/// that name and plays it. Returns the document's key and the handle.
///
/// # Errors
/// `L4355` for a document outside `root`, unreadable or of another kind (a document that fails its own
/// checks returns those diagnostics), `L4356` for a bind name that matches no entity or several, `L4351`
/// for an undeclared role.
pub(crate) fn install(
    world: &mut World,
    root: &Path,
    preview: &SequencePreview,
) -> Result<(String, SequenceHandle), Vec<Diagnostic>> {
    let key = key_of(root, &preview.doc)?;
    let doc = read_doc(&preview.doc)?;
    world.insert_resource(DocStore::<SequenceDoc>::new(BTreeMap::from([(
        key.clone(),
        doc,
    )])));
    world.insert_resource(SequenceEvents::default());
    world.insert_resource(SequenceAudio::default());
    if world.resource::<GameClock>().is_none() {
        world.insert_resource(GameClock::default());
    }
    if world.resource::<Diagnostics>().is_none() {
        world.insert_resource(Diagnostics::default());
    }
    let errors: Vec<Diagnostic> = validate_sequence_docs(world)
        .into_iter()
        .filter(|d| d.severity == Severity::Error)
        .collect();
    if !errors.is_empty() {
        return Err(errors);
    }
    let mut pairs: Vec<(&str, EntityId)> = Vec::new();
    for (role, name) in &preview.bindings {
        pairs.push((role, find_entity(world, name)?));
    }
    let handle = play(world, &key, &pairs).map_err(|d| vec![d])?;
    Ok((key, handle))
}

/// Steps `preview.at + 1` ticks, releasing the player before each so a `wait: release` clears the tick
/// it is reached. Returns how the player ended, `None` while it is still playing.
pub(crate) fn run(
    driver: &mut SimDriver,
    handle: SequenceHandle,
    preview: &SequencePreview,
) -> Option<SequenceEnd> {
    let mut ended = None;
    for _ in 0..=preview.at {
        release(driver.world_mut(), handle);
        driver.step(1);
        driver.advance();
        if ended.is_none()
            && let Some(events) = driver.world().resource::<SequenceEvents>()
        {
            ended = events.iter().find_map(|event| match event {
                SequenceEvent::Ended { handle: h, end } if *h == handle => Some(*end),
                _ => None,
            });
        }
    }
    ended
}

/// The warnings the run reported (`L4352`, `L4353`, ...), in report order.
pub(crate) fn warnings(world: &mut World) -> Vec<Diagnostic> {
    world
        .resource_mut::<Diagnostics>()
        .map(Diagnostics::take)
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Transform, builtin_registry, sequence_preview_world};

    const SCENE: &str = "format: 1
seed: 0
entities:
  - name: crate
    components:
      Transform:
        translation:
          - 0.0
          - 0.0
          - 0.0
";

    fn doc(extra: &str) -> String {
        format!(
            "format: 1
kind: sequence
roles:
  - mover
length_ticks: 20
tracks:
  transform:
    - at: 0
      role: mover
      over: 10
      to:
        offset:
          - 4.0
          - 0.0
          - 0.0
{extra}"
        )
    }

    /// A scene folder holding `scene.yaml` and `sequences/move.yaml`.
    fn project(name: &str, sequence: &str) -> (PathBuf, PathBuf) {
        let root = std::env::temp_dir().join(format!(
            "ludion-sequence-preview-{name}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("sequences")).unwrap();
        std::fs::write(root.join("scene.yaml"), SCENE).unwrap();
        std::fs::write(root.join("sequences/move.yaml"), sequence).unwrap();
        (root.join("scene.yaml"), root.join("sequences/move.yaml"))
    }

    fn preview(doc: PathBuf, bind: &str, at: u64) -> SequencePreview {
        SequencePreview {
            doc,
            bindings: vec![("mover".into(), bind.into())],
            at,
        }
    }

    fn x_at(scene: &Path, preview: &SequencePreview) -> f32 {
        let mut run = sequence_preview_world(&builtin_registry(), scene, &[], preview)
            .unwrap_or_else(|d| panic!("{d:?}"));
        let world = run.driver.world_mut();
        let id = world
            .query_ids::<(&Name,)>()
            .find_map(|(id, (n,))| (n.0 == "crate").then_some(id))
            .unwrap();
        world.get::<Transform>(id).unwrap().unwrap().translation[0]
    }

    #[test]
    fn at_names_the_playhead_tick() {
        let (scene, doc_path) = project("at", &doc(""));
        for (at, x) in [(0, 0.0), (5, 2.0), (10, 4.0)] {
            assert_eq!(x_at(&scene, &preview(doc_path.clone(), "crate", at)), x);
        }
    }

    #[test]
    fn an_unknown_name_is_l4356_and_suggests_the_nearest() {
        let (scene, doc_path) = project("name", &doc(""));
        let errors = sequence_preview_world(
            &builtin_registry(),
            &scene,
            &[],
            &preview(doc_path, "crat", 0),
        )
        .err()
        .expect("no entity is named crat");
        assert_eq!(errors[0].code, code::SEQUENCE_PREVIEW_BIND);
        assert!(
            errors[0].help.as_deref().unwrap_or("").contains("`crate`"),
            "{errors:?}"
        );
    }

    #[test]
    fn a_name_on_two_entities_is_l4356() {
        let (scene, doc_path) = project("twice", &doc(""));
        let twice = format!("{SCENE}  - name: crate\n");
        std::fs::write(&scene, twice).unwrap();
        let errors = sequence_preview_world(
            &builtin_registry(),
            &scene,
            &[],
            &preview(doc_path, "crate", 0),
        )
        .err()
        .expect("two entities share the name");
        assert_eq!(errors[0].code, code::SEQUENCE_PREVIEW_BIND);
        assert!(errors[0].message.contains("2 entities"), "{errors:?}");
    }

    #[test]
    fn a_doc_outside_the_scene_folder_is_l4355() {
        let (scene, _) = project("outside", &doc(""));
        let (_, elsewhere) = project("elsewhere", &doc(""));
        let errors = sequence_preview_world(
            &builtin_registry(),
            &scene,
            &[],
            &preview(elsewhere, "crate", 0),
        )
        .err()
        .expect("the doc is not under the scene's folder");
        assert_eq!(errors[0].code, code::SEQUENCE_PREVIEW_DOC);
    }

    #[test]
    fn a_doc_of_another_kind_is_l4355() {
        let (scene, doc_path) = project(
            "kind",
            "format: 1\nkind: strings\nlocale: en\nstrings: {}\n",
        );
        let errors = sequence_preview_world(
            &builtin_registry(),
            &scene,
            &[],
            &preview(doc_path, "crate", 0),
        )
        .err()
        .expect("not a sequence");
        assert_eq!(errors[0].code, code::SEQUENCE_PREVIEW_DOC);
    }

    #[test]
    fn an_undeclared_role_is_l4351() {
        let (scene, doc_path) = project("role", &doc(""));
        let mut wrong = preview(doc_path, "crate", 0);
        wrong.bindings = vec![("ghost".into(), "crate".into())];
        let errors = sequence_preview_world(&builtin_registry(), &scene, &[], &wrong)
            .err()
            .expect("`ghost` is not a role");
        assert_eq!(errors[0].code, code::SEQUENCE_BIND_INVALID);
    }

    #[test]
    fn a_release_wait_does_not_stall_the_preview() {
        let waiting = doc("  event:
    - at: 3
      name: door
      wait: release
");
        let (scene, doc_path) = project("wait", &waiting);
        let (free_scene, free_doc) = project("no-wait", &doc(""));
        let stalled = x_at(&scene, &preview(doc_path, "crate", 8));
        let free = x_at(&free_scene, &preview(free_doc, "crate", 8));
        assert_eq!(stalled, free);
    }

    #[test]
    fn a_finished_sequence_reports_how_it_ended() {
        let (scene, doc_path) = project("ended", &doc(""));
        let run = sequence_preview_world(
            &builtin_registry(),
            &scene,
            &[],
            &preview(doc_path, "crate", 25),
        )
        .unwrap();
        assert_eq!(run.ended, Some(SequenceEnd::Completed));
    }
}
