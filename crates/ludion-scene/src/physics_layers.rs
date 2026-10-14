//! The `physics.layers` list (`Plan/physics-collision-layers.md`): named collision layers, each
//! optionally ignoring others. This module holds the authored form ([`SceneLayer`]), the value
//! checks the parser and the snapshot restore share ([`check_layers`]), and the resolved
//! [`LayerMatrix`] the solver mapping and the `check-scene` verb read.

use ludion_core::codes::code;
use ludion_core::diagnostics::Diagnostic;
use ludion_core::suggest;

use crate::physics::ScenePhysics;

/// The implicit layer every unlayered collider belongs to. Reserved: a scene cannot declare it, but
/// any layer may ignore it.
pub const DEFAULT_LAYER: &str = "default";

/// The most layers a scene may declare: the solver keeps one 32-bit group mask per collider and
/// `default` takes the first bit.
pub const MAX_LAYERS: usize = 31;

/// One authored layer: its name and the layers it never collides with, unresolved.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SceneLayer {
    /// The layer's name — what a collider's `layer` field spells.
    pub name: String,
    /// The layers this one ignores, as authored. Ignoring is symmetric once resolved
    /// ([`LayerMatrix::collides`]).
    pub ignores: Vec<String>,
}

/// The mask bit for layer `index`; zero past the 32 a validated block can reach.
fn bit(index: usize) -> u32 {
    u32::try_from(index)
        .ok()
        .and_then(|shift| 1u32.checked_shl(shift))
        .unwrap_or(0)
}

/// The resolved layer table: `default` at index 0, then the declared layers in declaration order —
/// the order the solver's group bits follow.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LayerMatrix {
    names: Vec<String>,
    masks: Vec<u32>,
}

impl LayerMatrix {
    /// Every layer name, `default` first.
    pub fn names(&self) -> &[String] {
        &self.names
    }

    /// The index (bit position) of `name`, or `None` when the scene does not declare it.
    pub fn index_of(&self, name: &str) -> Option<usize> {
        self.names.iter().position(|candidate| candidate == name)
    }

    /// The bitmask of layers `index` never collides with: bit `j` is set when either layer ignores
    /// the other.
    pub fn ignores_mask(&self, index: usize) -> u32 {
        self.masks[index]
    }

    /// Whether layers `a` and `b` collide.
    pub fn collides(&self, a: usize, b: usize) -> bool {
        self.masks.get(a).is_none_or(|mask| mask & bit(b) == 0)
    }
}

impl ScenePhysics {
    /// Resolves the authored layers into a [`LayerMatrix`]. Assumes a validated block: an
    /// `ignores` name that matches no layer is skipped.
    pub fn layer_matrix(&self) -> LayerMatrix {
        let mut names = vec![DEFAULT_LAYER.to_string()];
        names.extend(self.layers.iter().map(|layer| layer.name.clone()));
        let mut masks = vec![0u32; names.len()];
        for (offset, layer) in self.layers.iter().enumerate() {
            let i = offset + 1;
            for ignored in &layer.ignores {
                if let Some(j) = names.iter().position(|name| name == ignored) {
                    masks[i] |= bit(j);
                    masks[j] |= bit(i);
                }
            }
        }
        LayerMatrix { names, masks }
    }
}

/// `L1033` for every way a `layers` list can be wrong: an empty or duplicate name, `default`
/// redeclared, more than [`MAX_LAYERS`], an `ignores` entry naming no layer, a repeated `ignores`
/// entry. Shared by the parser and [`crate::validate_physics`]; `base` is the pointer prefix
/// (`/physics`, or `/scene_physics` from a restore).
pub(crate) fn check_layers(layers: &[SceneLayer], base: &str) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();
    if layers.len() > MAX_LAYERS {
        diagnostics.push(
            Diagnostic::new(
                code::SCENE_PHYSICS_INVALID,
                format!(
                    "`physics.layers` declares {} layers, more than the {MAX_LAYERS} the solver's \
                     32-bit group mask has room for",
                    layers.len()
                ),
            )
            .with_path(format!("{base}/layers"))
            .with_help(format!(
                "declare at most {MAX_LAYERS} layers; `{DEFAULT_LAYER}` takes the remaining bit"
            )),
        );
    }
    let known: Vec<&str> = std::iter::once(DEFAULT_LAYER)
        .chain(layers.iter().map(|layer| layer.name.as_str()))
        .collect();
    for (i, layer) in layers.iter().enumerate() {
        let name_pointer = format!("{base}/layers/{i}/name");
        if layer.name.is_empty() {
            diagnostics.push(
                Diagnostic::new(
                    code::SCENE_PHYSICS_INVALID,
                    format!("`physics.layers[{i}].name` is empty"),
                )
                .with_path(name_pointer),
            );
        } else if layer.name == DEFAULT_LAYER {
            diagnostics.push(
                Diagnostic::new(
                    code::SCENE_PHYSICS_INVALID,
                    format!("`physics.layers[{i}]` redeclares `{DEFAULT_LAYER}`"),
                )
                .with_path(name_pointer)
                .with_help(format!(
                    "`{DEFAULT_LAYER}` is reserved: it is the implicit layer of every collider \
                     that names none, and any layer may already ignore it"
                )),
            );
        } else if layers[..i].iter().any(|earlier| earlier.name == layer.name) {
            diagnostics.push(
                Diagnostic::new(
                    code::SCENE_PHYSICS_INVALID,
                    format!(
                        "`physics.layers[{i}]` declares `{}` a second time",
                        layer.name
                    ),
                )
                .with_path(name_pointer)
                .with_help("give each layer one entry and list everything it ignores under it"),
            );
        }
        for (j, ignored) in layer.ignores.iter().enumerate() {
            let pointer = format!("{base}/layers/{i}/ignores/{j}");
            if !known.contains(&ignored.as_str()) {
                let help = match suggest::nearest(ignored, known.iter().copied()) {
                    Some(candidate) => format!("nearest match is `{candidate}`"),
                    None => "declare it under `physics.layers`".to_string(),
                };
                diagnostics.push(
                    Diagnostic::new(
                        code::SCENE_PHYSICS_INVALID,
                        format!(
                            "`physics.layers[{i}].ignores[{j}]` names `{ignored}`, which no layer \
                             declares"
                        ),
                    )
                    .with_path(pointer)
                    .with_help(help),
                );
            } else if layer.ignores[..j].contains(ignored) {
                diagnostics.push(
                    Diagnostic::new(
                        code::SCENE_PHYSICS_INVALID,
                        format!("`physics.layers[{i}].ignores` lists `{ignored}` a second time"),
                    )
                    .with_path(pointer)
                    .with_help("list each ignored layer once"),
                );
            }
        }
    }
    diagnostics
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::physics::ScenePhysicsSpace;

    fn block(layers: Vec<SceneLayer>) -> ScenePhysics {
        ScenePhysics {
            space: ScenePhysicsSpace::Free,
            gravity: None,
            solver_iterations: None,
            stabilization_iterations: None,
            sleep_after_ticks: None,
            layers,
        }
    }

    fn layer(name: &str, ignores: &[&str]) -> SceneLayer {
        SceneLayer {
            name: name.to_string(),
            ignores: ignores.iter().map(|s| s.to_string()).collect(),
        }
    }

    #[test]
    fn unlayered_block_has_only_default() {
        let matrix = block(Vec::new()).layer_matrix();
        assert_eq!(matrix.names(), [DEFAULT_LAYER]);
        assert_eq!(matrix.ignores_mask(0), 0);
        assert!(matrix.collides(0, 0));
    }

    #[test]
    fn one_sided_ignore_is_symmetric() {
        let matrix = block(vec![layer("dummy", &["sling"]), layer("sling", &[])]).layer_matrix();
        let dummy = matrix.index_of("dummy").unwrap();
        let sling = matrix.index_of("sling").unwrap();
        assert_eq!((dummy, sling), (1, 2));
        assert!(!matrix.collides(dummy, sling));
        assert!(!matrix.collides(sling, dummy));
        assert_eq!(matrix.ignores_mask(dummy), 1 << sling);
        assert_eq!(matrix.ignores_mask(sling), 1 << dummy);
        assert!(matrix.collides(dummy, dummy));
        assert!(matrix.collides(0, dummy));
    }

    #[test]
    fn default_can_be_ignored() {
        let matrix = block(vec![layer("ghost", &[DEFAULT_LAYER, "ghost"])]).layer_matrix();
        assert!(!matrix.collides(0, 1));
        assert!(!matrix.collides(1, 1));
        assert!(matrix.collides(0, 0));
    }

    #[test]
    fn unknown_ignore_is_skipped_not_fatal() {
        let matrix = block(vec![layer("a", &["nope"])]).layer_matrix();
        assert_eq!(matrix.ignores_mask(1), 0);
    }

    #[test]
    fn check_layers_flags_each_case() {
        let layers = vec![
            layer("a", &["b", "b", "zzz"]),
            layer("a", &[]),
            layer(DEFAULT_LAYER, &[]),
            layer("", &[]),
            layer("b", &["c"]),
        ];
        let pointers: Vec<String> = check_layers(&layers, "/physics")
            .into_iter()
            .map(|d| d.path.unwrap_or_default())
            .collect();
        assert_eq!(
            pointers,
            [
                "/physics/layers/0/ignores/1",
                "/physics/layers/0/ignores/2",
                "/physics/layers/1/name",
                "/physics/layers/2/name",
                "/physics/layers/3/name",
                "/physics/layers/4/ignores/0",
            ]
        );
    }

    #[test]
    fn an_unvalidated_oversized_list_resolves_without_panicking() {
        let layers: Vec<SceneLayer> = (0..33)
            .map(|i| layer(&format!("l{i}"), &["l32", "l0"]))
            .collect();
        let matrix = block(layers).layer_matrix();
        assert_eq!(matrix.names().len(), 34);
        assert!(!matrix.collides(2, 1));
        assert!(matrix.collides(1, 33));
        assert!(matrix.collides(33, 40));
    }

    #[test]
    fn check_layers_caps_the_count() {
        let layers: Vec<SceneLayer> = (0..=MAX_LAYERS)
            .map(|i| layer(&format!("l{i}"), &[]))
            .collect();
        let diagnostics = check_layers(&layers, "/scene_physics");
        assert_eq!(diagnostics.len(), 1);
        assert_eq!(
            diagnostics[0].path.as_deref(),
            Some("/scene_physics/layers")
        );
        assert!(check_layers(&layers[..MAX_LAYERS], "/physics").is_empty());
    }
}
