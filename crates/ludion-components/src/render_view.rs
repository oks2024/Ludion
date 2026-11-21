//! The [`RenderView`] component: makes the `Camera` on its entity a secondary view that renders into a
//! texture a `UiRect.view` shows. Render-only; `ludion docs render-views` is the guide.

use ludion_core::codes::code;
use ludion_core::diagnostics::Diagnostic;
use ludion_ecs::SemanticValidate;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// The smallest and largest view size per axis, in texels.
pub const RENDER_VIEW_SIZE_RANGE: std::ops::RangeInclusive<u32> = 16..=2048;

/// Renders the world through the `Camera` on this entity into its own texture of `size` texels, every
/// frame it is `enabled`, in the same frame. A `UiRect` shows it by naming it in `view`. A `Camera`
/// beside a `RenderView` is never the main camera. At most 4 enabled views render per frame.
///
/// ```
/// use ludion_components::RenderView;
///
/// let view: RenderView = serde_json::from_value(serde_json::json!({ "name": "radar" })).unwrap();
/// assert_eq!(view.size, [256, 256]);
/// assert!(view.enabled);
/// assert_eq!(view.order, 0);
/// assert!(view.sun && view.lights.is_none());
/// ```
#[derive(Serialize, Deserialize, JsonSchema, Debug, Clone, PartialEq, Eq)]
pub struct RenderView {
    /// The name a `UiRect.view` uses; unique per scene, not empty.
    pub name: String,
    /// The texture size `[width, height]` in texels, each axis in `16..=2048` (default `[256, 256]`).
    #[serde(default = "default_size")]
    pub size: [u32; 2],
    /// Whether the view renders (default true).
    #[serde(default = "default_enabled", skip_serializing_if = "is_true")]
    pub enabled: bool,
    /// Draw order among views, lowest first; ties go by entity (default 0).
    #[serde(default, skip_serializing_if = "is_zero")]
    pub order: i32,
    /// The layers the view draws (default none, meaning `[default]`). Absent is omitted from the document.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub layers: Option<Vec<String>>,
    /// The `PointLight`s and `SpotLight`s that light the view, by `RenderLayers` (default none, meaning the
    /// view's own `layers`). A light the view drops casts no shadow in it. Absent is omitted from the document.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lights: Option<Vec<String>>,
    /// Whether the scene's `DirectionalLight` lights the view (default true). `false` drops the sun from the
    /// view only; it needs no layer.
    #[serde(default = "default_enabled", skip_serializing_if = "is_true")]
    pub sun: bool,
}

fn default_size() -> [u32; 2] {
    [256, 256]
}

fn default_enabled() -> bool {
    true
}

fn is_true(v: &bool) -> bool {
    *v
}

fn is_zero(v: &i32) -> bool {
    *v == 0
}

impl SemanticValidate for RenderView {
    /// Rejects what a schema cannot (`L2370`, pointer into the field): an empty `name` and a `size` axis
    /// outside `16..=2048`. A wrong *shape* is the schema's job.
    fn validate_semantics(value: &Value) -> Vec<Diagnostic> {
        let mut out = crate::render_layers::field_layer_diagnostics(value, "layers");
        out.extend(crate::render_layers::field_layer_diagnostics(
            value, "lights",
        ));
        let mut reject = |path: String, what: String| {
            out.push(
                Diagnostic::new(code::RENDER_VIEW_INVALID, format!("`RenderView` {what}"))
                    .with_path(path)
                    .with_help("see `ludion docs render-views`; `ludion explain L2370`"),
            );
        };
        if value.get("name").and_then(Value::as_str) == Some("") {
            reject(
                "/name".into(),
                "`name` is empty; a `UiRect.view` needs it to find the view".into(),
            );
        }
        if let Some(size) = value.get("size").and_then(Value::as_array) {
            for (i, axis) in size.iter().enumerate() {
                let Some(n) = axis.as_i64() else { continue };
                if !(i64::from(*RENDER_VIEW_SIZE_RANGE.start())
                    ..=i64::from(*RENDER_VIEW_SIZE_RANGE.end()))
                    .contains(&n)
                {
                    reject(
                        format!("/size/{i}"),
                        format!(
                            "`size[{i}]` is {n}; it takes a value in {}..={}",
                            RENDER_VIEW_SIZE_RANGE.start(),
                            RENDER_VIEW_SIZE_RANGE.end()
                        ),
                    );
                }
            }
        }
        out
    }
}

/// `L2373` for a `UiRect` that sets `view` beside another texture source, pushed onto `out`. The pointer
/// is relative to the component (`/view`).
pub(crate) fn ui_rect_view_diagnostics(
    object: &serde_json::Map<String, Value>,
    out: &mut Vec<Diagnostic>,
) {
    let present = |field: &str| object.get(field).is_some_and(|v| !v.is_null());
    if !present("view") {
        return;
    }
    let beside: Vec<&str> = ["texture", "atlas", "frame", "slice", "uv_rect"]
        .into_iter()
        .filter(|f| present(f))
        .collect();
    if !beside.is_empty() {
        out.push(
            Diagnostic::new(
                code::UI_RECT_VIEW_CONFLICT,
                format!(
                    "this `UiRect` has `view` beside {}; a view is the rect's whole image",
                    beside
                        .iter()
                        .map(|f| format!("`{f}`"))
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
            )
            .with_path("/view")
            .with_help(
                "keep `view` and remove the other fields, or drop `view`; `ludion explain L2373`",
            ),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn paths(value: Value) -> Vec<String> {
        RenderView::validate_semantics(&value)
            .into_iter()
            .map(|d| d.path.unwrap())
            .collect()
    }

    #[test]
    fn a_named_view_takes_the_defaults_and_writes_minimally() {
        let v: RenderView = serde_json::from_value(json!({ "name": "radar" })).unwrap();
        assert_eq!(v.size, [256, 256]);
        assert!(v.enabled);
        assert_eq!(
            serde_json::to_value(&v).unwrap(),
            json!({ "name": "radar", "size": [256, 256] })
        );
        assert!(serde_json::from_value::<RenderView>(json!({})).is_err());
        assert!(paths(json!({ "name": "radar" })).is_empty());
    }

    #[test]
    fn a_full_view_round_trips() {
        let value = json!({ "name": "cam", "size": [512, 128], "enabled": false, "order": -2 });
        let v: RenderView = serde_json::from_value(value.clone()).unwrap();
        assert_eq!(serde_json::to_value(&v).unwrap(), value);
    }

    #[test]
    fn each_bad_field_names_its_pointer() {
        assert_eq!(paths(json!({ "name": "" })), ["/name"]);
        assert_eq!(
            paths(json!({ "name": "a", "size": [15, 256] })),
            ["/size/0"]
        );
        assert_eq!(
            paths(json!({ "name": "a", "size": [256, 2049] })),
            ["/size/1"]
        );
        assert_eq!(
            paths(json!({ "name": "", "size": [0, 0] })),
            ["/name", "/size/0", "/size/1"]
        );
        assert!(paths(json!({ "name": "a", "size": [16, 2048] })).is_empty());
    }

    #[test]
    fn lights_and_sun_round_trip_and_validate() {
        let value = json!({ "name": "p", "size": [64, 64], "layers": ["portrait"], "lights": ["portrait", "default"], "sun": false });
        let v: RenderView = serde_json::from_value(value.clone()).unwrap();
        assert!(!v.sun);
        assert_eq!(serde_json::to_value(&v).unwrap(), value);
        assert!(paths(value).is_empty());
        assert_eq!(
            paths(json!({ "name": "p", "lights": ["a", "a"] })),
            ["/lights/1"]
        );
        assert_eq!(paths(json!({ "name": "p", "lights": [] })), ["/lights"]);
    }

    #[test]
    fn a_ui_rect_view_beside_a_texture_source_is_l2373() {
        let check = |value: Value| {
            let mut out = Vec::new();
            ui_rect_view_diagnostics(value.as_object().unwrap(), &mut out);
            out
        };
        assert!(check(json!({ "view": "radar" })).is_empty());
        assert!(check(json!({ "texture": "a.png" })).is_empty());
        let found = check(json!({ "view": "radar", "texture": "a.png", "uv_rect": [0, 0, 1, 1] }));
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].path.as_deref(), Some("/view"));
        assert_eq!(found[0].code, code::UI_RECT_VIEW_CONFLICT);
    }

    #[test]
    fn a_view_rect_does_not_trip_the_missing_texture_warning() {
        use crate::UiRect;
        let diags = UiRect::validate_semantics(
            &json!({ "view": "radar", "uv_rect": [0.0, 0.0, 1.0, 1.0] }),
        );
        assert_eq!(diags.len(), 1);
        assert_eq!(diags[0].code, code::UI_RECT_VIEW_CONFLICT);
    }
}
