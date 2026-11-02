//! `scene.load` / `scene.save` / `scene.check` handlers.

use ludion_core::codes::code;
use ludion_core::diagnostics::Diagnostic;
use ludion_core::paths::relative_refusal;
use ludion_protocol::ResponseBody;
use ludion_protocol::command::{SceneCheckParams, SceneLoadParams, SceneSaveParams};
use ludion_protocol::event::{Event, SceneChangedData};
use std::path::PathBuf;

use crate::commands::collapse;
use crate::engine::Engine;

/// Joins a client-supplied `path` onto the content root, refusing anything that is not a plain path
/// below it (`L3040`). Every scene command resolves through here, so every client gets the rule.
fn resolve_in_root(engine: &Engine, path: &str) -> Result<PathBuf, Diagnostic> {
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
    let count = engine.load_scene_document(document).map_err(collapse)?;
    // The world was replaced — notify `scene.changed` subscribers (dropped if none are open).
    engine.push_event(Event::SceneChanged(SceneChangedData {
        path: Some(params.path),
    }));
    Ok(ResponseBody::SceneLoaded {
        entity_count: count,
    })
}

/// `scene.save` — write the current world to `path` through the canonical YAML writer. A scene
/// bearing `generated_from:` is write-only generator output and is refused (`L1267`).
pub(crate) fn save(
    engine: &mut Engine,
    params: SceneSaveParams,
) -> Result<ResponseBody, Diagnostic> {
    let resolved = resolve_in_root(engine, &params.path)?;
    let scene = engine.world_to_scene()?;
    if let Some(generator) = &scene.generated_from {
        return Err(Diagnostic::new(
            code::SCENE_SAVE_GENERATED,
            format!(
                "scene `{}` is generated from `{generator}` and cannot be saved",
                params.path
            ),
        )
        .with_path("/generated_from")
        .with_help(format!(
            "edit the map source and run `ludion generate-map {generator}`; a generated scene is \
             write-only"
        )));
    }
    let yaml = ludion_scene::emit(&scene, engine.registry());
    ludion_core::atomic::write_atomic(&resolved, yaml).map_err(|error| {
        Diagnostic::new(
            code::OUTPUT_IO,
            format!("could not write scene `{}`: {error}", resolved.display()),
        )
        .with_help(
            "check the directory exists below the content root (the directory of the scene the \
             engine was started with) and is writable",
        )
    })?;
    engine.asset_cache_prune_requested();
    // The scene on disk changed — notify `scene.changed` subscribers (dropped if none are open).
    engine.push_event(Event::SceneChanged(SceneChangedData {
        path: Some(params.path.clone()),
    }));
    Ok(ResponseBody::SceneSaved { path: params.path })
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
    let checked = ludion_scene::check(&document, engine.registry()).and_then(|scene| {
        crate::scene_check::expand_scene(engine.project_root(), engine.registry(), &scene)
    });
    let (valid, diagnostics) = match checked {
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
    };
    Ok(ResponseBody::SceneChecked { valid, diagnostics })
}
