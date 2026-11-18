//! Secondary views at extract: which `RenderView` cameras render this frame, and the table a
//! `UiRect.view` resolves its name through. The static half, [`scene_render_view_diagnostics`], reports
//! the same mistakes from scene text. Not `render_view.rs`: that file is the host `render.view` override.

use std::collections::BTreeMap;

use ludion_core::codes::code;
use ludion_core::diagnostics::Diagnostic;
use ludion_core::ids::EntityId;
use ludion_ecs::World;
use ludion_renderer::{MAX_RENDER_VIEWS, RenderViewData};
use ludion_scene::{Scene, SceneEntity};
use serde_json::Value;

use crate::components::{Camera, RenderView};

/// The frame's views and the name each `UiRect.view` resolves to.
#[derive(Default)]
pub(crate) struct ViewTable {
    /// The views that render, in `(order, entity)` order.
    pub(crate) views: Vec<RenderViewData>,
    /// Every authored name: its index in `views`, or `None` when the view is disabled or over budget.
    pub(crate) by_name: BTreeMap<String, Option<u32>>,
}

/// Resolves the world's `RenderView`s. A duplicate name or a view without a `Camera` is an error in
/// `errors`; a view past [`MAX_RENDER_VIEWS`] is a warning in `degraded` and is not rendered. A view's
/// camera ignores the presentation `CameraOffset`, which belongs to the main camera.
pub(crate) fn extract_views(
    world: &World,
    ids: &[EntityId],
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
        table.views.push(RenderViewData {
            name: view.name.clone(),
            size,
            camera: crate::extract::build_camera(world, &[], id, size),
        });
    }
    table
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
/// `UiRect.view` no `RenderView` carries, each at its JSON Pointer. Silent for a scene with no
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
    out
}
