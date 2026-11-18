//! The occluded-silhouette extract: which meshes draw into the `silhouette_mask` pass and with what
//! style. `ludion docs occluded-silhouette` is the guide; `Plan/occluded-silhouette.md` the design.

use ludion_components::{OccludedSilhouette, SilhouetteWhen};
use ludion_core::codes::code;
use ludion_core::diagnostics::Diagnostic;
use ludion_core::ids::EntityId;
use ludion_ecs::World;
use ludion_renderer::{
    MAX_SILHOUETTE_STYLES, MeshInstance, SilhouetteMesh, SilhouetteStyle, Silhouettes,
};

/// The exact bits of a style: two entities share a table slot only when every field is identical.
type StyleKey = ([u32; 4], u32, u32, u32, bool);

fn style_of(silhouette: &OccludedSilhouette) -> SilhouetteStyle {
    let (outline_px, outline_alpha) = silhouette
        .outline
        .map_or((0.0, 1.0), |outline| (outline.px, outline.alpha));
    SilhouetteStyle {
        color: silhouette.color,
        outline_px,
        outline_alpha,
        bias: silhouette.bias,
        always: silhouette.when == SilhouetteWhen::Always,
    }
}

fn key_of(style: &SilhouetteStyle) -> StyleKey {
    (
        style.color.map(f32::to_bits),
        style.outline_px.to_bits(),
        style.outline_alpha.to_bits(),
        style.bias.to_bits(),
        style.always,
    )
}

/// The silhouette table for this frame's `meshes`; `owners` is parallel to it. Each owner resolves its
/// own `OccludedSilhouette`, else its nearest ancestor's. A `Viewmodel` mesh is skipped and warns
/// (`L2351`); a ninth distinct style is not drawn and warns once (`L2352`). Empty, with no walk, when
/// no entity carries the component.
pub(crate) fn extract_silhouettes(
    world: &World,
    meshes: &[MeshInstance],
    owners: &[EntityId],
    degraded: &mut Vec<(String, Diagnostic)>,
) -> Silhouettes {
    debug_assert_eq!(meshes.len(), owners.len());
    let mut out = Silhouettes::default();
    if world.component_count::<OccludedSilhouette>() == 0 {
        return out;
    }
    let mut memo = crate::hierarchy::InheritMemo::<Option<OccludedSilhouette>>::new();
    let mut keys: Vec<StyleKey> = Vec::new();
    let mut last: Option<(EntityId, Option<u8>)> = None;
    let mut viewmodel_reported: Vec<EntityId> = Vec::new();
    let mut limit_reported = false;
    for (index, (mesh, &owner)) in meshes.iter().zip(owners).enumerate() {
        let resolved = match last {
            Some((id, resolved)) if id == owner => resolved,
            _ => {
                let silhouette = memo.resolve(
                    world,
                    owner,
                    |id| world.get::<OccludedSilhouette>(id).ok().flatten().copied(),
                    |parent, own| own.or(parent),
                    None,
                );
                let slot = silhouette.and_then(|silhouette| {
                    let style = style_of(&silhouette);
                    let key = key_of(&style);
                    if let Some(at) = keys.iter().position(|k| *k == key) {
                        return Some(at as u8 + 1);
                    }
                    if keys.len() == MAX_SILHOUETTE_STYLES {
                        if !limit_reported {
                            limit_reported = true;
                            let at = crate::diagnostics::offender_key(world, owner);
                            degraded.push((
                                format!("{at}/silhouette-style-limit"),
                                Diagnostic::new(
                                    code::OCCLUDED_SILHOUETTE_STYLE_LIMIT,
                                    format!(
                                        "{owner} resolves a ninth distinct `OccludedSilhouette` style; \
                                         a frame draws at most {MAX_SILHOUETTE_STYLES}, so it gets none"
                                    ),
                                )
                                .with_help(
                                    "share styles between entities, for example on a common `Parent`",
                                ),
                            ));
                        }
                        return None;
                    }
                    keys.push(key);
                    out.styles.push(style);
                    Some(keys.len() as u8)
                });
                last = Some((owner, slot));
                slot
            }
        };
        let Some(style) = resolved else { continue };
        if mesh.viewmodel {
            if !viewmodel_reported.contains(&owner) {
                viewmodel_reported.push(owner);
                let at = crate::diagnostics::offender_key(world, owner);
                degraded.push((
                    format!("{at}/silhouette-on-viewmodel"),
                    Diagnostic::new(
                        code::OCCLUDED_SILHOUETTE_ON_VIEWMODEL,
                        format!(
                            "{owner} resolves an `OccludedSilhouette` on a `Viewmodel` mesh; the viewmodel \
                             is never silhouetted"
                        ),
                    )
                    .with_help("remove the `OccludedSilhouette` from the viewmodel or its ancestors"),
                ));
            }
            continue;
        }
        out.meshes.push(SilhouetteMesh {
            mesh: index as u32,
            style,
        });
    }
    out
}
