//! The [`AudioListener`] component: where the ear is, when it is not the camera.
//! `audio_listener` in `ludion-engine` reads it; `ludion docs audio-space` is the guide.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Whose axes place a sound across the stereo field.
///
/// ```
/// use ludion_components::ListenerOrientation;
///
/// let own: ListenerOrientation = serde_json::from_value(serde_json::json!("self")).unwrap();
/// assert_eq!(own, ListenerOrientation::Listener);
/// assert_eq!(serde_json::to_value(ListenerOrientation::Camera).unwrap(), "camera");
/// ```
#[derive(Serialize, Deserialize, JsonSchema, Debug, Clone, Copy, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum ListenerOrientation {
    /// Left and right follow the lowest-`EntityId` `Camera`, so a sound pans as the screen shows it.
    #[default]
    Camera,
    /// Left and right follow this entity's own world rotation (`+X` right, `+Y` up).
    #[serde(rename = "self")]
    Listener,
}

/// Marks the entity every positional sound is measured from: distance falls off from its world
/// position, and the stereo pan takes its axes from [`ListenerOrientation`]. Without one in the
/// world the camera is the listener. With several, the lowest `EntityId` wins and `L1650` warns.
///
/// ```yaml
/// - name: player-ear
///   components:
///     Parent: { index: 1, generation: 0 }   # the player: the second entity in the scene
///     Transform: { translation: [0.0, 1.6, 0.0] }   # head height, local to the parent
///     AudioListener: {}
/// ```
///
/// ```
/// use ludion_components::{AudioListener, ListenerOrientation};
///
/// let ear: AudioListener = serde_json::from_value(serde_json::json!({})).unwrap();
/// assert_eq!(ear.orientation, ListenerOrientation::Camera);
/// let own: AudioListener =
///     serde_json::from_value(serde_json::json!({ "orientation": "self" })).unwrap();
/// assert_eq!(own.orientation, ListenerOrientation::Listener);
/// ```
#[derive(Serialize, Deserialize, JsonSchema, Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct AudioListener {
    /// Whose axes pan the sound (default `camera`).
    #[serde(default)]
    pub orientation: ListenerOrientation,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn orientation_round_trips_as_camera_and_self() {
        for (variant, text) in [
            (ListenerOrientation::Camera, "camera"),
            (ListenerOrientation::Listener, "self"),
        ] {
            let value = serde_json::to_value(variant).unwrap();
            assert_eq!(value, text);
            assert_eq!(
                serde_json::from_value::<ListenerOrientation>(value).unwrap(),
                variant
            );
        }
    }

    #[test]
    fn empty_listener_orients_by_camera() {
        let ear: AudioListener = serde_json::from_value(serde_json::json!({})).unwrap();
        assert_eq!(ear, AudioListener::default());
        assert_eq!(ear.orientation, ListenerOrientation::Camera);
    }
}
