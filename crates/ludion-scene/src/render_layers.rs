//! The scene's top-level `render_layers:` block (`Plan/secondary-views.md` D14): the render layer
//! names a scene authors, declared once in the `physics.layers` idiom. Declaration is validation
//! only — extract still interns names per frame — so a name `RenderLayers`, `Camera.layers`,
//! `Camera.lights`, `RenderView.layers` or `RenderView.lights` spells that the block does not list
//! is `L2376` from `check-scene`.

use ludion_core::codes::code;
use ludion_core::diagnostics::Diagnostic;

use crate::envelope;
use crate::yaml::{Node, NodeValue};

/// The implicit layer every unlayered item belongs to. Reserved: a scene cannot declare it.
pub const DEFAULT_RENDER_LAYER: &str = "default";

/// The most layers a scene may declare: a frame's layer mask is 32 bits and `default` takes one.
pub const MAX_RENDER_LAYERS: usize = 31;

const LAYER_KEYS: [&str; 1] = ["name"];

/// One declared render layer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SceneRenderLayer {
    /// The layer's name: what a `RenderLayers` or `Camera.layers` entry spells.
    pub name: String,
}

/// Parses the `render_layers:` sequence: mappings with a required string `name`, each name
/// non-empty, not `default`, and unique, at most [`MAX_RENDER_LAYERS`] entries. `None` when any
/// entry is invalid (the diagnostics say which).
pub(crate) fn parse_render_layers(
    node: &Node,
    diagnostics: &mut Vec<Diagnostic>,
) -> Option<Vec<SceneRenderLayer>> {
    let NodeValue::Sequence(items) = &node.value else {
        diagnostics.push(
            Diagnostic::new(
                code::SCENE_TYPE_MISMATCH,
                format!(
                    "`render_layers` must be a sequence of mappings, found {} (line {})",
                    node.value.type_phrase(),
                    node.line
                ),
            )
            .with_path("/render_layers")
            .with_help("write each layer as a mapping:\n  render_layers:\n    - name: radar"),
        );
        return None;
    };
    if items.len() > MAX_RENDER_LAYERS {
        diagnostics.push(
            Diagnostic::new(
                code::RENDER_LAYERS_INVALID,
                format!(
                    "`render_layers` declares {} layers, more than the {MAX_RENDER_LAYERS} a frame's \
                     32-bit layer mask has room for",
                    items.len()
                ),
            )
            .with_path("/render_layers")
            .with_help(format!(
                "declare at most {MAX_RENDER_LAYERS} layers; `{DEFAULT_RENDER_LAYER}` takes the remaining bit"
            )),
        );
        return None;
    }
    let mut out: Vec<SceneRenderLayer> = Vec::with_capacity(items.len());
    let mut ok = true;
    for (index, item) in items.iter().enumerate() {
        match parse_layer(item, index, &out, diagnostics) {
            Some(layer) => out.push(layer),
            None => ok = false,
        }
    }
    ok.then_some(out)
}

/// Parses one entry; `earlier` is the layers accepted so far, for the duplicate check.
fn parse_layer(
    item: &Node,
    index: usize,
    earlier: &[SceneRenderLayer],
    diagnostics: &mut Vec<Diagnostic>,
) -> Option<SceneRenderLayer> {
    let base = format!("/render_layers/{index}");
    let NodeValue::Mapping(entries) = &item.value else {
        diagnostics.push(
            Diagnostic::new(
                code::SCENE_TYPE_MISMATCH,
                format!(
                    "`render_layers[{index}]` must be a mapping, found {} (line {})",
                    item.value.type_phrase(),
                    item.line
                ),
            )
            .with_path(base)
            .with_help("write the layer as `name: <layer>`"),
        );
        return None;
    };
    let mut name = None;
    let mut name_seen = false;
    let mut ok = true;
    for entry in entries {
        if entry.key == "name" {
            name_seen = true;
            name = envelope::expect_string(
                &entry.node,
                &format!("{base}/name"),
                &format!("`render_layers[{index}].name`"),
                code::SCENE_TYPE_MISMATCH,
                diagnostics,
            );
        } else {
            diagnostics.push(envelope::unknown_key(
                &entry.key,
                entry,
                &base,
                "a render layer",
                LAYER_KEYS,
                code::SCENE_UNKNOWN_KEY,
            ));
            ok = false;
        }
    }
    if !name_seen {
        diagnostics.push(
            Diagnostic::new(
                code::SCENE_MISSING_KEY,
                format!("`render_layers[{index}]` is missing the required key `name`"),
            )
            .with_path(&base)
            .with_help("add `name: <layer>`: the spelling a `RenderLayers` entry uses"),
        );
    }
    let name = name?;
    let pointer = format!("{base}/name");
    let problem = if name.is_empty() {
        Some((
            format!("`render_layers[{index}].name` is empty"),
            "name the layer, e.g. `name: radar`".to_string(),
        ))
    } else if name == DEFAULT_RENDER_LAYER {
        Some((
            format!("`render_layers[{index}]` redeclares `{DEFAULT_RENDER_LAYER}`"),
            format!(
                "`{DEFAULT_RENDER_LAYER}` is reserved: it is the implicit layer of every item that names none"
            ),
        ))
    } else if earlier.iter().any(|layer| layer.name == name) {
        Some((
            format!("`render_layers[{index}]` declares `{name}` a second time"),
            "declare each layer once".to_string(),
        ))
    } else {
        None
    };
    if let Some((message, help)) = problem {
        diagnostics.push(
            Diagnostic::new(code::RENDER_LAYERS_INVALID, message)
                .with_path(pointer)
                .with_help(help),
        );
        return None;
    }
    ok.then_some(SceneRenderLayer { name })
}
