//! Camera zones: a [`CameraZone`] on a sensor volume swaps the [`CameraRig`]'s configuration while
//! the rig's target stands inside it. [`resolve_zone`] is the pure switch; the engine finds the
//! zone each tick from the target's position.

use ludion_core::diagnostics::Diagnostic;
use ludion_core::ids::EntityId;
use ludion_ecs::SemanticValidate;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::rig::{CameraRig, RigConfig};

/// A sensor volume that drives the scene's [`CameraRig`] while the rig's target origin is inside
/// it. Put it beside a `Collider` with `sensor: true`; the highest `priority` among the zones the
/// target stands in wins, the lowest entity id breaking a tie.
///
/// ```
/// use ludion_camera::CameraZone;
///
/// let zone: CameraZone = serde_json::from_value(serde_json::json!({
///     "priority": 2,
///     "rig": { "focus_offset": [0.0, 1.5, 0.0] },
/// }))
/// .unwrap();
/// assert_eq!(zone.priority, 2);
/// assert_eq!(zone.rig.focus_offset, [0.0, 1.5, 0.0]);
/// ```
#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, Default, PartialEq)]
#[serde(default)]
pub struct CameraZone {
    /// Higher wins where zones overlap.
    pub priority: i32,
    /// The configuration the camera's `CameraRig` takes while this zone is active; it blends over
    /// its own `blend_in`.
    pub rig: RigConfig,
}

impl SemanticValidate for CameraZone {
    /// Delegates `rig` to the rig's range rules (`L1304`), pointers under `/rig`.
    fn validate_semantics(value: &serde_json::Value) -> Vec<Diagnostic> {
        let rig = value.get("rig").cloned().unwrap_or_default();
        RigConfig::validate_semantics(&rig)
            .into_iter()
            .map(|mut diagnostic| {
                diagnostic.path = diagnostic.path.map(|path| format!("/rig{path}"));
                diagnostic
            })
            .collect()
    }
}

/// Applies the active zone to `rig` before [`step_rig`](crate::step_rig): `active` is the zone the
/// target stands in (its entity and `rig`), or `None` outside every zone.
///
/// Entering from no zone remembers the configuration in force as the base; a change of zone copies
/// the zone's `rig` into [`CameraRig::rig`], which the next step blends over its `blend_in`;
/// leaving every zone restores the base. A game write to `rig` while a zone is active becomes the
/// base and is never fought. An unchanged zone does nothing, so a live edit to the active zone's
/// `rig` applies at the next zone change. A rig outside every zone is untouched.
pub fn resolve_zone(rig: &mut CameraRig, active: Option<(EntityId, &RigConfig)>) {
    if rig.state.zone.is_some() && rig.state.last_applied.as_ref() != Some(&rig.rig) {
        rig.state.base = Some(rig.rig.clone());
    }
    match active {
        Some((id, zone_rig)) if rig.state.zone != Some(id) => {
            if rig.state.zone.is_none() {
                rig.state.base = Some(rig.rig.clone());
            }
            rig.rig = zone_rig.clone();
            rig.state.zone = Some(id);
        }
        None if rig.state.zone.is_some() => {
            if let Some(base) = rig.state.base.take() {
                rig.rig = base;
            }
            rig.state.zone = None;
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rig::BlendIn;

    fn config(fov: f32) -> RigConfig {
        RigConfig {
            fov_y_deg: Some(fov),
            blend_in: BlendIn::default(),
            ..RigConfig::default()
        }
    }

    /// A rig that has applied `rig` this tick, as after `step_rig`.
    fn applied(rig: &mut CameraRig) {
        rig.state.last_applied = Some(rig.rig.clone());
    }

    fn id(n: u32) -> EntityId {
        EntityId::new(n, 0)
    }

    #[test]
    fn a_rig_outside_every_zone_is_untouched() {
        let mut rig = CameraRig {
            rig: config(60.0),
            ..CameraRig::default()
        };
        applied(&mut rig);
        let before = rig.clone();
        resolve_zone(&mut rig, None);
        assert_eq!(rig, before);
    }

    #[test]
    fn enter_nested_zone_then_exit_back_through_the_lower_zone_to_the_base() {
        let (low, high) = (config(70.0), config(40.0));
        let mut rig = CameraRig {
            rig: config(60.0),
            ..CameraRig::default()
        };
        applied(&mut rig);

        resolve_zone(&mut rig, Some((id(1), &low)));
        assert_eq!(rig.rig, low);
        assert_eq!(rig.state.zone, Some(id(1)));
        assert_eq!(rig.state.base, Some(config(60.0)));
        applied(&mut rig);

        resolve_zone(&mut rig, Some((id(2), &high)));
        assert_eq!(rig.rig, high);
        assert_eq!(
            rig.state.base,
            Some(config(60.0)),
            "the base is not the lower zone"
        );
        applied(&mut rig);

        resolve_zone(&mut rig, Some((id(1), &low)));
        assert_eq!(rig.rig, low);
        applied(&mut rig);

        resolve_zone(&mut rig, None);
        assert_eq!(rig.rig, config(60.0));
        assert_eq!(rig.state.zone, None);
        assert_eq!(rig.state.base, None);
    }

    #[test]
    fn a_game_write_inside_a_zone_becomes_the_base() {
        let zone = config(40.0);
        let mut rig = CameraRig {
            rig: config(60.0),
            ..CameraRig::default()
        };
        applied(&mut rig);
        resolve_zone(&mut rig, Some((id(1), &zone)));
        applied(&mut rig);

        rig.rig = config(90.0);
        resolve_zone(&mut rig, Some((id(1), &zone)));
        assert_eq!(
            rig.rig,
            config(90.0),
            "the write is not fought inside the zone"
        );
        assert_eq!(rig.state.base, Some(config(90.0)));
        applied(&mut rig);

        resolve_zone(&mut rig, None);
        assert_eq!(rig.rig, config(90.0));
    }

    #[test]
    fn an_unchanged_zone_ignores_a_live_edit_to_its_rig() {
        let mut zone = config(40.0);
        let mut rig = CameraRig::default();
        applied(&mut rig);
        resolve_zone(&mut rig, Some((id(1), &zone)));
        applied(&mut rig);
        zone.fov_y_deg = Some(30.0);
        resolve_zone(&mut rig, Some((id(1), &zone)));
        assert_eq!(rig.rig.fov_y_deg, Some(40.0));
    }

    #[test]
    fn the_zone_state_round_trips_through_serde() {
        let mut rig = CameraRig::default();
        applied(&mut rig);
        resolve_zone(&mut rig, Some((id(3), &config(40.0))));
        let back: CameraRig = serde_json::from_value(serde_json::to_value(&rig).unwrap()).unwrap();
        assert_eq!(back, rig);
        let plain = serde_json::to_value(CameraRig::default()).unwrap();
        assert!(plain["state"].get("zone").is_none());
        assert!(plain["state"].get("base").is_none());
    }

    #[test]
    fn a_bad_zone_rig_is_l1304_under_rig() {
        let found = CameraZone::validate_semantics(&serde_json::json!({
            "rig": { "springs": { "position_half_life": -1.0 } }
        }));
        assert!(!found.is_empty());
        assert!(
            found
                .iter()
                .all(|d| d.path.as_deref().is_some_and(|p| p.starts_with("/rig/")))
        );
    }
}
