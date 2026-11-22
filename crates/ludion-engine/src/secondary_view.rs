//! Secondary views at extract: which `RenderView` cameras render this frame, and the table a
//! `UiRect.view` resolves its name through. The static half, [`scene_render_view_diagnostics`], reports
//! the same mistakes from scene text. Not `render_view.rs`: that file is the host `render.view` override.

use std::collections::BTreeMap;

use ludion_core::codes::code;
use ludion_core::diagnostics::Diagnostic;
use ludion_core::ids::EntityId;
use ludion_core::time::GameTime;
use ludion_ecs::World;
use ludion_renderer::{MAX_RENDER_VIEWS, RenderViewData, ViewPost};
use ludion_scene::{Scene, SceneEntity};
use serde_json::Value;

use crate::components::{
    Camera, PostProcess, PostProfile, RenderView, ViewBackground, ViewShading,
};
use crate::extract::{SceneAssets, lut_for, post_data};
use ludion_assets::AssetRoot;

/// The frame's views and the name each `UiRect.view` resolves to.
#[derive(Default)]
pub(crate) struct ViewTable {
    /// The views that render, in `(order, entity)` order.
    pub(crate) views: Vec<RenderViewData>,
    /// The entity behind each of `views`, parallel to it.
    pub(crate) entities: Vec<EntityId>,
    /// Every authored name: its index in `views`, or `None` when the view is disabled or over budget.
    pub(crate) by_name: BTreeMap<String, Option<u32>>,
    /// The profile each rendered view names, as `(slot in views, name, entity index)`, until
    /// [`resolve_view_posts`] fills the view's look.
    profile_refs: Vec<(usize, String, usize)>,
}

/// Resolves the world's `RenderView`s. A duplicate name or a view without a `Camera` is an error in
/// `errors`; a view past [`MAX_RENDER_VIEWS`] is a warning in `degraded` and is not rendered. A view's
/// camera ignores the presentation `CameraOffset`, which belongs to the main camera.
pub(crate) fn extract_views(
    world: &World,
    ids: &[EntityId],
    layers: Option<&crate::render_layers::FrameLayers>,
    errors: &mut Vec<Diagnostic>,
    degraded: &mut Vec<(String, Diagnostic)>,
) -> ViewTable {
    let mut table = ViewTable::default();
    if world.component_count::<RenderView>() == 0 {
        return table;
    }
    let mut candidates: Vec<(i32, EntityId, &RenderView)> = Vec::new();
    for (index, &id) in ids.iter().enumerate() {
        let Some(view) = world.get::<RenderView>(id).ok().flatten() else {
            continue;
        };
        let at = format!("/entities/{index}/components/RenderView");
        if !world.has::<Camera>(id) {
            errors.push(
                Diagnostic::new(
                    code::RENDER_VIEW_INVALID,
                    format!("{id} has a `RenderView` but no `Camera` to render through"),
                )
                .with_path(at)
                .with_help("add a `Camera` to the same entity; `ludion explain L2370`"),
            );
            continue;
        }
        if table.by_name.contains_key(&view.name) {
            errors.push(
                Diagnostic::new(
                    code::RENDER_VIEW_NAME_DUPLICATE,
                    format!("a second `RenderView` is named `{}`", view.name),
                )
                .with_path(format!("{at}/name"))
                .with_help("give every view its own name; `ludion explain L2371`"),
            );
            continue;
        }
        table.by_name.insert(view.name.clone(), None);
        if view.enabled {
            candidates.push((view.order, id, view));
        }
    }
    candidates.sort_by_key(|&(order, id, _)| (order, id));
    for (slot, &(_, id, view)) in candidates.iter().enumerate() {
        if slot >= MAX_RENDER_VIEWS {
            degraded.push((
                format!(
                    "{}/render-view-budget",
                    crate::diagnostics::offender_key(world, id)
                ),
                Diagnostic::new(
                    code::RENDER_VIEW_BUDGET,
                    format!(
                        "view `{}` is not rendered: only {MAX_RENDER_VIEWS} views render per frame",
                        view.name
                    ),
                )
                .with_help(
                    "set `enabled: false` on views that are not on screen; `ludion explain L2374`",
                ),
            ));
            continue;
        }
        let size = (view.size[0], view.size[1]);
        table.by_name.insert(view.name.clone(), Some(slot as u32));
        if let Some(name) = view.post.as_deref().filter(|name| *name != "none") {
            let index = ids.iter().position(|&other| other == id).unwrap_or(0);
            table.profile_refs.push((slot, name.to_string(), index));
        }
        table.entities.push(id);
        table.views.push(RenderViewData {
            name: view.name.clone(),
            size,
            camera: crate::extract::build_camera(world, &[], id, size),
            layers: layers.map_or(u32::MAX, |layers| layers.view_mask(view)),
            lights: layers.map_or(u32::MAX, |layers| layers.view_lights_mask(view)),
            sun: view.sun,
            background: match view.background {
                ViewBackground::Sky => None,
                ViewBackground::Color(color) => Some(color),
            },
            unlit: view.shading == ViewShading::Unlit,
            fog: view.fog,
            post: match view.post.as_deref() {
                Some("none") => ViewPost::Passthrough,
                _ => ViewPost::Scene,
            },
            screen_effects: Vec::new(),
        });
    }
    table
}

/// Fills each view's `post` that names a `PostProfile` with that profile's imaging and LUT. A name no
/// profile (a `PostProfile` beside a `PostProcess`) carries is `L2378` at `/post` of the `RenderView`; a
/// LUT that does not load is reported as the main frame's is.
pub(crate) fn resolve_view_posts(
    table: &mut ViewTable,
    world: &World,
    ids: &[EntityId],
    now: GameTime,
    project_root: AssetRoot<'_>,
    assets: &mut dyn SceneAssets,
) -> Result<(), Vec<Diagnostic>> {
    if table.profile_refs.is_empty() {
        return Ok(());
    }
    let mut profiles: BTreeMap<&str, (usize, &PostProcess)> = BTreeMap::new();
    for (index, &id) in ids.iter().enumerate() {
        if let (Some(profile), Some(post)) = (
            world.get::<PostProfile>(id).ok().flatten(),
            world.get::<PostProcess>(id).ok().flatten(),
        ) {
            profiles
                .entry(profile.name.as_str())
                .or_insert((index, post));
        }
    }
    let mut errors = Vec::new();
    for (slot, name, entity) in std::mem::take(&mut table.profile_refs) {
        let Some(&(index, post)) = profiles.get(name.as_str()) else {
            errors.push(unknown_profile(
                &name,
                format!("/entities/{entity}/components/RenderView/post"),
                profiles.keys().copied(),
            ));
            continue;
        };
        match lut_for(index, post, project_root, assets) {
            Ok(lut) => {
                table.views[slot].post = ViewPost::Profile {
                    post: Box::new(post_data(post, now)),
                    lut,
                };
            }
            Err(found) => errors.extend(found),
        }
    }
    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors)
    }
}

/// `L2378` for a `RenderView.post` naming no profile, with the nearest of `known` as the help.
fn unknown_profile<'a>(
    name: &str,
    at: String,
    known: impl IntoIterator<Item = &'a str>,
) -> Diagnostic {
    let help = match ludion_core::suggest::nearest(name, known) {
        Some(near) => format!("did you mean `{near}`? `ludion explain L2378`"),
        None => "add a `PostProfile` with that name beside a `PostProcess`, or use `none`; `ludion explain L2378`".to_string(),
    };
    Diagnostic::new(
        code::POST_PROFILE_INVALID,
        format!("this `RenderView` takes post `{name}`, which no `PostProfile` names"),
    )
    .with_path(at)
    .with_help(help)
}

/// What a `UiRect.view` resolves to.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum RectView {
    /// The rect names no view: it draws as any rect.
    None,
    /// The view is disabled, over budget or unknown: the rect draws nothing.
    Hidden,
    /// The rect shows this index of the frame's views.
    Shows(u32),
}

impl RectView {
    /// The view index a drawn rect carries.
    pub(crate) fn slot(self) -> Option<u32> {
        match self {
            RectView::Shows(slot) => Some(slot),
            _ => None,
        }
    }
}

fn unresolved(name: &str, at: String, help: String) -> Diagnostic {
    Diagnostic::new(
        code::UI_RECT_VIEW_UNRESOLVED,
        format!("this `UiRect` shows view `{name}`, which no `RenderView` names"),
    )
    .with_path(at)
    .with_help(help)
}

/// Resolves a rect's `view` name through `table`; an unknown name is `L2372` in `errors` and the rect
/// is hidden, so the entity's other components still draw.
pub(crate) fn resolve_rect_view(
    table: &ViewTable,
    rect_view: Option<&str>,
    index: usize,
    errors: &mut Vec<Diagnostic>,
) -> RectView {
    let Some(name) = rect_view else {
        return RectView::None;
    };
    match table.by_name.get(name) {
        Some(Some(slot)) => RectView::Shows(*slot),
        Some(None) => RectView::Hidden,
        None => {
            errors.push(unresolved(
                name,
                format!("/entities/{index}/components/UiRect/view"),
                "add a `RenderView` with that name; `ludion explain L2372`".to_string(),
            ));
            RectView::Hidden
        }
    }
}

fn component<'a>(entity: &'a SceneEntity, wanted: &str) -> Option<&'a Value> {
    entity
        .components
        .iter()
        .find(|(name, _)| name == wanted)
        .map(|(_, value)| value)
}

/// `L2370` for a `RenderView` beside no `Camera`, `L2371` for a repeated name and `L2372` for a
/// `UiRect.view` no `RenderView` carries, `L2378` for a `PostProfile` that is not usable or a
/// `RenderView.post` naming none, and `L2377` for a profile field a view ignores, each at its JSON Pointer. Silent for a scene with no
/// `RenderView` and no `UiRect.view`.
///
/// # Example
/// ```
/// use ludion_engine::scene_render_view_diagnostics;
///
/// let scene = ludion_scene::check(
///     "format: 1\nentities:\n  - components:\n      UiRect:\n        view: radar\n",
///     &ludion_engine::builtin_registry(),
/// )
/// .unwrap();
/// let found = scene_render_view_diagnostics(&scene);
/// assert_eq!(found.len(), 1);
/// assert_eq!(found[0].path.as_deref(), Some("/entities/0/components/UiRect/view"));
/// ```
pub fn scene_render_view_diagnostics(scene: &Scene) -> Vec<Diagnostic> {
    let mut out = Vec::new();
    let mut named: BTreeMap<&str, usize> = BTreeMap::new();
    for (index, entity) in scene.entities.iter().enumerate() {
        let Some(view) = component(entity, "RenderView") else {
            continue;
        };
        let at = format!("/entities/{index}/components/RenderView");
        if component(entity, "Camera").is_none() {
            out.push(
                Diagnostic::new(
                    code::RENDER_VIEW_INVALID,
                    "this `RenderView` has no `Camera` beside it to render through",
                )
                .with_path(at.clone())
                .with_help("add a `Camera` to the same entity; `ludion explain L2370`"),
            );
        }
        if let Some(name) = view.get("name").and_then(Value::as_str)
            && !name.is_empty()
            && named.insert(name, index).is_some()
        {
            out.push(
                Diagnostic::new(
                    code::RENDER_VIEW_NAME_DUPLICATE,
                    format!("a second `RenderView` is named `{name}`"),
                )
                .with_path(format!("{at}/name"))
                .with_help("give every view its own name; `ludion explain L2371`"),
            );
        }
    }
    for (index, entity) in scene.entities.iter().enumerate() {
        let Some(name) = component(entity, "UiRect")
            .and_then(|rect| rect.get("view"))
            .and_then(Value::as_str)
        else {
            continue;
        };
        if !named.contains_key(name) {
            let help = match ludion_core::suggest::nearest(name, named.keys().copied()) {
                Some(near) => format!("did you mean `{near}`? `ludion explain L2372`"),
                None => "add a `RenderView` with that name; `ludion explain L2372`".to_string(),
            };
            out.push(unresolved(
                name,
                format!("/entities/{index}/components/UiRect/view"),
                help,
            ));
        }
    }
    post_profile_diagnostics(scene, &mut out);
    out
}

/// The fields of a `PostProcess` a view never runs, each a gate that is on above 0.
const VIEW_IGNORED_POST_FIELDS: [&str; 5] = ["ssao", "ssr", "dof", "shafts", "fog_density"];

/// `L2378` for a `PostProfile` beside no `PostProcess`, a repeated profile name and a `RenderView.post`
/// naming no profile; `L2377` for each field a view-named profile authors that no view runs.
fn post_profile_diagnostics(scene: &Scene, out: &mut Vec<Diagnostic>) {
    // Name -> the entity index of the first profile carrying it (beside a `PostProcess`).
    let mut profiles: BTreeMap<&str, usize> = BTreeMap::new();
    for (index, entity) in scene.entities.iter().enumerate() {
        let Some(profile) = component(entity, "PostProfile") else {
            continue;
        };
        let at = format!("/entities/{index}/components/PostProfile");
        if component(entity, "PostProcess").is_none() {
            out.push(
                Diagnostic::new(
                    code::POST_PROFILE_INVALID,
                    "this `PostProfile` has no `PostProcess` beside it to name",
                )
                .with_path(at.clone())
                .with_help("add a `PostProcess` to the same entity; `ludion explain L2378`"),
            );
            continue;
        }
        let Some(name) = profile
            .get("name")
            .and_then(Value::as_str)
            .filter(|name| !name.is_empty() && *name != "none")
        else {
            continue;
        };
        if profiles.contains_key(name) {
            out.push(
                Diagnostic::new(
                    code::POST_PROFILE_INVALID,
                    format!("a second `PostProfile` is named `{name}`"),
                )
                .with_path(format!("{at}/name"))
                .with_help("give every profile its own name; `ludion explain L2378`"),
            );
        } else {
            profiles.insert(name, index);
        }
    }
    let mut named_by_views: BTreeMap<&str, usize> = BTreeMap::new();
    for (index, entity) in scene.entities.iter().enumerate() {
        let Some(name) = component(entity, "RenderView")
            .and_then(|view| view.get("post"))
            .and_then(Value::as_str)
            .filter(|name| *name != "none")
        else {
            continue;
        };
        if let Some(&at) = profiles.get(name) {
            named_by_views.entry(name).or_insert(at);
        } else {
            out.push(unknown_profile(
                name,
                format!("/entities/{index}/components/RenderView/post"),
                profiles.keys().copied(),
            ));
        }
    }
    for &index in named_by_views.values() {
        let Some(post) = component(&scene.entities[index], "PostProcess") else {
            continue;
        };
        for field in VIEW_IGNORED_POST_FIELDS {
            if post
                .get(field)
                .and_then(Value::as_f64)
                .is_some_and(|v| v > 0.0)
            {
                out.push(
                    Diagnostic::new(
                        code::POST_PROFILE_VIEW_IGNORED,
                        format!(
                            "this `PostProfile` authors `{field}`, which a view never takes from a profile"
                        ),
                    )
                    .with_path(format!("/entities/{index}/components/PostProcess/{field}"))
                    .with_help(
                        "remove the field; a view runs no ssao, ssr, dof or shafts and takes its fog from the scene; `ludion explain L2377`",
                    ),
                );
            }
        }
    }
}
