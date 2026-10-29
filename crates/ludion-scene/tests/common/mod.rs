//! Shared test components and registry for the ludion-scene integration suites.
//!
//! These stand in for real game components; they cover the shapes the milestone needs: flat
//! float structs, strings/ints/bools, `Vec`, `Option`, an externally-tagged enum (a `oneOf`
//! schema), and a newtype scalar with an integer range.

use ludion_ecs::Registry;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, JsonSchema, Debug, PartialEq)]
pub struct Transform {
    pub x: f32,
    pub y: f32,
    pub z: f32,
}

#[derive(Serialize, Deserialize, JsonSchema, Debug, PartialEq)]
pub struct Sprite {
    #[schemars(with = "ludion_ecs::AssetPathSchema")]
    pub texture: String,
    pub layer: i32,
    pub visible: bool,
}

#[derive(Serialize, Deserialize, JsonSchema, Debug, PartialEq)]
pub struct Palette {
    pub colors: Vec<String>,
    pub emissive: Option<f64>,
}

#[derive(Serialize, Deserialize, JsonSchema, Debug, PartialEq)]
pub enum Shape {
    Circle { radius: f64 },
    Rect { w: f64, h: f64 },
}

#[derive(Serialize, Deserialize, JsonSchema, Debug, PartialEq)]
pub struct Health(pub u32);

/// A reserved (field-backed) component, standing in for the engine's `Name`: authored through a
/// dedicated scene field, never as a `components:` block.
#[derive(Serialize, Deserialize, JsonSchema, Debug, PartialEq)]
pub struct Label(pub String);

/// A registry with all test components, in a fixed registration order (the canonical component
/// order for emitted scenes).
pub fn registry() -> Registry {
    let mut registry = Registry::new();
    registry.register::<Transform>("Transform").unwrap();
    registry.register::<Sprite>("Sprite").unwrap();
    registry.register::<Palette>("Palette").unwrap();
    registry.register::<Shape>("Shape").unwrap();
    registry.register::<Health>("Health").unwrap();
    registry.register_reserved::<Label>("Label").unwrap();
    registry
}
