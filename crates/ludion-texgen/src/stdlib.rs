//! The embedded `std:` recipes: style-agnostic macros a recipe references as `path: std:<name>`.
//!
//! A macro declares the images it receives with `op: input` steps and the tunables it exposes
//! under `params:`; the consumer binds them with `inputs:` and `params:` on a `recipe` step.
//! Style lives in those bindings, never in the engine. A std recipe may reference only other std
//! recipes.
//!
//! # Example
//! ```
//! let recipe = "format: 1
//! kind: texture
//! steps:
//!   h:
//!     op: noise
//!     type: gradient
//!     scale: 4
//!     octaves: 3
//!   wear:
//!     op: recipe
//!     path: std:edge_wear
//!     output: mask
//!     inputs:
//!       height: h
//!     params:
//!       amount: 0.7
//! outputs:
//!   base_color: wear
//!   roughness: wear
//!   height: h
//! ";
//! let resolver = ludion_texgen::FsResolver::new(".");
//! let maps = ludion_texgen::build(recipe, &resolver, 16).unwrap();
//! assert_eq!(&maps.height[1..4], b"PNG");
//! assert!(ludion_texgen::stdlib::names().any(|n| n == "edge_wear"));
//! ```

/// Every embedded recipe as `(name, document text)`.
const FILES: &[(&str, &str)] = &[
    ("edge_wear", include_str!("../std/edge_wear.texture.yaml")),
    (
        "cavity_grime",
        include_str!("../std/cavity_grime.texture.yaml"),
    ),
    ("cobbles", include_str!("../std/cobbles.texture.yaml")),
    ("planks", include_str!("../std/planks.texture.yaml")),
    ("rust_spots", include_str!("../std/rust_spots.texture.yaml")),
];

/// The names of the embedded recipes, without the `std:` prefix.
pub fn names() -> impl Iterator<Item = &'static str> {
    FILES.iter().map(|(name, _)| *name)
}

/// The document text of the embedded recipe `name` (no `std:` prefix).
pub fn get(name: &str) -> Option<&'static str> {
    FILES
        .iter()
        .find(|(n, _)| *n == name)
        .map(|(_, text)| *text)
}

/// Whether a document id names an embedded recipe.
pub(crate) fn is_std(id: &str) -> bool {
    id.starts_with("std:")
}

/// A document whose id starts with `std:` may reference only `std:` recipes.
pub(crate) fn std_policy(from: Option<&str>, path: &str) -> Result<(), String> {
    match from {
        Some(from) if is_std(from) && !is_std(path) => Err(format!(
            "the stdlib recipe `{from}` can reference only `std:` recipes, not `{path}`"
        )),
        _ => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn std_documents_reference_only_std() {
        assert!(std_policy(None, "wall.texture.yaml").is_ok());
        assert!(std_policy(Some("wall.texture.yaml"), "std:cobbles").is_ok());
        assert!(std_policy(Some("std:planks"), "std:cobbles").is_ok());
        let err = std_policy(Some("std:planks"), "../mine.texture.yaml").unwrap_err();
        assert!(err.contains("std:planks") && err.contains("../mine.texture.yaml"));
    }

    #[test]
    fn every_file_on_disk_is_embedded_and_back() {
        let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/std");
        let mut on_disk: Vec<String> = std::fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().into_string().unwrap())
            .filter_map(|f| f.strip_suffix(".texture.yaml").map(str::to_string))
            .collect();
        on_disk.sort();
        let mut embedded: Vec<String> = names().map(str::to_string).collect();
        embedded.sort();
        assert_eq!(on_disk, embedded);
    }
}
