//! `scene.load` / `scene.save` / `scene.check` handlers.

use ludion_core::codes::code;
use ludion_core::diagnostics::Diagnostic;
use ludion_core::paths::relative_refusal;
use ludion_protocol::ResponseBody;
use ludion_protocol::command::{SceneCheckParams, SceneLoadParams, SceneSaveParams};
use ludion_protocol::event::{Event, SceneChangeKind, SceneChangedData};
use std::path::PathBuf;

use crate::commands::collapse;
use crate::engine::Engine;

/// Joins a client-supplied `path` onto the content root, refusing anything that is not a plain path
/// below it (`L3040`). Every scene command resolves through here, so every client gets the rule.
pub(crate) fn resolve_in_root(engine: &Engine, path: &str) -> Result<PathBuf, Diagnostic> {
    match relative_refusal(path) {
        None => Ok(engine.project_root().join(path)),
        Some(refusal) => Err(Diagnostic::new(
            code::WIRE_PATH_OUTSIDE_ROOT,
            format!("scene path `{path}` {}", refusal.describe()),
        )
        .with_help(
            "pass a path relative to the content root (the directory of the scene the engine was \
             started with), e.g. `levels/arena.yaml`",
        )),
    }
}

/// `scene.load` — read the file at `path` (relative to the content root — the directory of the scene
/// the engine was started with), then check → instantiate →
/// replace the world (the session's world-reset point; see
/// [`Engine::load_scene_document`](crate::engine::Engine)).
///
/// The engine's stored scene reference is the scene's **text**, inline
/// ([`SceneRef::Inline`]), not the path it was read from. Holding the canonical text (Pillar 2) is
/// what makes a recording self-contained: the replay header embeds the scene verbatim, so
/// `ludion replay` reproduces the identical world — and `WorldHash` sequence — from any directory or
/// machine, with no dependency on the recording host's paths. (A `SceneRef::Path` header is still
/// *accepted* on replay for hand-authored sessions; the engine simply never *writes* one.)
pub(crate) fn load(
    engine: &mut Engine,
    params: SceneLoadParams,
) -> Result<ResponseBody, Diagnostic> {
    let resolved = resolve_in_root(engine, &params.path)?;
    let document = std::fs::read_to_string(&resolved).map_err(|error| {
        Diagnostic::new(
            code::SCENE_IO,
            format!("could not read scene `{}`: {error}", resolved.display()),
        )
        .with_help(
            "check the path (resolved relative to the content root — the directory of the scene \
             the engine was started with) and permissions",
        )
    })?;
    let base = document.clone();
    let count = engine.load_scene_document(document).map_err(collapse)?;
    engine.set_scene_base(resolved, base);
    engine.set_current_scene_path(Some(params.path.clone()));
    // The world was replaced — notify `scene.changed` subscribers (dropped if none are open).
    engine.push_event(Event::SceneChanged(SceneChangedData {
        kind: SceneChangeKind::Load,
        path: Some(params.path),
        request_id: engine.current_request(),
    }));
    Ok(ResponseBody::SceneLoaded {
        entity_count: count,
    })
}

/// `scene.save` — write the current world to `path` through the canonical YAML writer. Every
/// refusal comes before any write, in this order: the path must stay below the content root
/// (`L3040`); the world must convert to a scene; a loaded scene bearing `generated_from:` is
/// write-only generator output (`L1267`). Then the destination is read: a `kind:` document there is
/// `L3059`, and a scene there bearing `generated_from:` is `L1267` — both even with `force`. Unless
/// `force`, a destination whose text differs from both the output and the text this session last
/// loaded or saved there is `L3058`. Checker findings over the written text ride the response and
/// never block the write: refusing would strand live work, and the file loads regardless.
pub(crate) fn save(
    engine: &mut Engine,
    params: SceneSaveParams,
) -> Result<ResponseBody, Diagnostic> {
    let resolved = resolve_in_root(engine, &params.path)?;
    let scene = engine.world_to_scene()?;
    if let Some(generator) = &scene.generated_from {
        return Err(generated_refusal(&params.path, generator));
    }
    let existing = match std::fs::read(&resolved) {
        Ok(bytes) => Some(bytes),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => return Err(write_failure(&resolved, &error)),
    };
    if let Some(Ok(text)) = existing.as_deref().map(std::str::from_utf8) {
        if let Some(kind) = ludion_scene::peek_kind(text) {
            let mut refusal = Diagnostic::new(
                code::SCENE_SAVE_WRONG_KIND,
                format!(
                    "`{}` is a `kind: {kind}` document, not a scene, and cannot be overwritten \
                     with the world",
                    params.path
                ),
            )
            .with_path("/params/path");
            refusal = refusal.with_help(if kind == "prefab" {
                "save the scene elsewhere; to change the prefab from the world use `prefab.apply`"
                    .to_string()
            } else {
                "save the scene to a different path".to_string()
            });
            return Err(refusal);
        }
        if let Ok(ludion_scene::Scene {
            generated_from: Some(generator),
            ..
        }) = ludion_scene::parse(text)
        {
            return Err(generated_refusal(&params.path, &generator));
        }
    }
    let yaml = ludion_scene::emit(&scene, engine.registry());
    if !params.force
        && let Some(bytes) = &existing
        && bytes.as_slice() != yaml.as_bytes()
    {
        match engine.scene_base(&resolved) {
            Some(base) if base.as_bytes() == bytes.as_slice() => {}
            base => {
                let why = if base.is_some() {
                    "changed on disk since this session loaded or saved it"
                } else {
                    "was never loaded by this session"
                };
                return Err(Diagnostic::new(
                    code::SCENE_SAVE_CONFLICT,
                    format!(
                        "`{}` {why}; saving would overwrite text this session has not seen",
                        params.path
                    ),
                )
                .with_path("/params/path")
                .with_help(
                    "`scene.load` the file to see it, or pass `force: true` to overwrite it",
                ));
            }
        }
    }
    let (_, diagnostics) = check_document(engine, &yaml);
    ludion_core::atomic::write_atomic(&resolved, &yaml)
        .map_err(|error| write_failure(&resolved, &error))?;
    engine.set_scene_base(resolved, yaml);
    engine.refresh_scene_session();
    engine.asset_cache_prune_requested();
    // The scene on disk changed — notify `scene.changed` subscribers (dropped if none are open).
    engine.push_event(Event::SceneChanged(SceneChangedData {
        kind: SceneChangeKind::Save,
        path: Some(params.path.clone()),
        request_id: engine.current_request(),
    }));
    Ok(ResponseBody::SceneSaved {
        path: params.path,
        diagnostics,
    })
}

fn generated_refusal(path: &str, generator: &str) -> Diagnostic {
    Diagnostic::new(
        code::SCENE_SAVE_GENERATED,
        format!("scene `{path}` is generated from `{generator}` and cannot be saved"),
    )
    .with_path("/generated_from")
    .with_help(format!(
        "edit the map source and run `ludion generate-map {generator}`; a generated scene is \
         write-only"
    ))
}

fn write_failure(resolved: &std::path::Path, error: &std::io::Error) -> Diagnostic {
    Diagnostic::new(
        code::OUTPUT_IO,
        format!("could not write scene `{}`: {error}", resolved.display()),
    )
    .with_help(
        "check the directory exists below the content root (the directory of the scene the \
         engine was started with) and is writable",
    )
}

/// `scene.check` — validate the scene file at `path` without loading it; returns its diagnostics in
/// the response body (this command never errors — an invalid scene is a successful check with
/// `valid: false`). It runs everything `ludion check-scene` does: parse, prefab expansion, the font
/// table's faults and every engine pass over the expanded scene. Warnings ride along with
/// `valid: true`.
pub(crate) fn check(
    engine: &mut Engine,
    params: SceneCheckParams,
) -> Result<ResponseBody, Diagnostic> {
    let resolved = match resolve_in_root(engine, &params.path) {
        Ok(resolved) => resolved,
        Err(diagnostic) => {
            return Ok(ResponseBody::SceneChecked {
                valid: false,
                diagnostics: vec![diagnostic],
            });
        }
    };
    let document = match std::fs::read_to_string(&resolved) {
        Ok(document) => document,
        Err(error) => {
            return Ok(ResponseBody::SceneChecked {
                valid: false,
                diagnostics: vec![
                    Diagnostic::new(
                        code::SCENE_IO,
                        format!("could not read scene `{}`: {error}", resolved.display()),
                    )
                    .with_help(
                        "check the path (resolved relative to the content root — the directory \
                         of the scene the engine was started with)",
                    ),
                ],
            });
        }
    };
    let (valid, diagnostics) = check_document(engine, &document);
    Ok(ResponseBody::SceneChecked { valid, diagnostics })
}

/// What `scene.check` reports over `document`: whether it is free of errors, and every diagnostic.
fn check_document(engine: &Engine, document: &str) -> (bool, Vec<Diagnostic>) {
    let checked = ludion_scene::check(document, engine.registry()).and_then(|scene| {
        crate::scene_check::expand_scene(engine.project_root(), engine.registry(), &scene)
    });
    match checked {
        Ok(expanded) => {
            let root = engine.project_root();
            let mut diagnostics: Vec<Diagnostic> = crate::scene_check::font_table_faults(root)
                .into_iter()
                .map(|(_, fault)| fault)
                .collect();
            diagnostics.extend(
                crate::scene_check::scene_engine_diagnostics(root, &expanded.scene)
                    .into_iter()
                    .map(|d| expanded.locate(d)),
            );
            if let Some(config) = engine.world().resource::<crate::PerceptionConfig>() {
                diagnostics.extend(
                    crate::perception_check::scene_perception_name_diagnostics(
                        &expanded.scene,
                        config,
                    )
                    .into_iter()
                    .map(|d| expanded.locate(d)),
                );
            }
            diagnostics.extend(crate::scene_check::scene_prefab_clip_diagnostics(
                root,
                engine.registry(),
                &expanded,
            ));
            (
                !diagnostics.iter().any(|d| d.severity.is_error()),
                diagnostics,
            )
        }
        Err(diagnostics) => (false, diagnostics),
    }
}
