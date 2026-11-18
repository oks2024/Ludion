//! [`ColliderMeshes`]: the triangles behind `Collider.shape: { mesh: <path> }`, as a world resource.
//!
//! The library mirrors `ludion-engine`'s `AnimationLibrary` (`animation_events.rs`): a loader
//! installed by the host, lazy and memoized per path. It is a resource, so it is never hashed or
//! snapshotted; the mesh a body was built from is identified by [`TriangleMesh::hash`], which rides
//! the body's snapshot row. A host reinstalls it at every load, restore and switch, and a body whose
//! recorded hash differs from what the fresh library reads is rebuilt on the next tick.

use std::collections::BTreeMap;
use std::sync::Arc;

use ludion_core::diagnostics::Diagnostic;

/// One mesh document's triangles, as the physics engine builds a trimesh from them.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TriangleMesh {
    /// Vertex positions in the collider's local space.
    pub vertices: Vec<[f32; 3]>,
    /// Vertex-index triples, one per triangle.
    pub triangles: Vec<[u32; 3]>,
    /// A hash of the source document's content: two loads with equal hashes are the same geometry.
    pub hash: u64,
    /// The local surface names the document's `surfaces` key lists; empty when untagged.
    pub surfaces: Vec<String>,
    /// One index into [`surfaces`](Self::surfaces) per triangle, the document's runs expanded;
    /// empty when untagged.
    pub triangle_surfaces: Vec<u16>,
}

/// Reads the mesh at a content-root-relative path, or says why it cannot.
pub type MeshLoader = Box<dyn Fn(&str) -> Result<TriangleMesh, Diagnostic> + Send + Sync>;

/// The lazy, memoized mesh library a world's mesh colliders read from.
///
/// ```
/// use ludion_core::codes::code;
/// use ludion_core::diagnostics::Diagnostic;
/// use ludion_physics::{ColliderMeshes, TriangleMesh};
///
/// let mut meshes = ColliderMeshes::new(Box::new(|path| match path {
///     "floor.mesh.yaml" => Ok(TriangleMesh {
///         vertices: vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 0.0, 1.0]],
///         triangles: vec![[0, 2, 1]],
///         hash: 7,
///         ..Default::default()
///     }),
///     other => Err(Diagnostic::new(
///         code::COLLIDER_MESH_UNAVAILABLE,
///         format!("no mesh at `{other}`"),
///     )),
/// }));
/// assert_eq!(meshes.get("floor.mesh.yaml").unwrap().triangles.len(), 1);
/// assert!(meshes.get("missing.mesh.yaml").is_err());
/// ```
pub struct ColliderMeshes {
    loader: MeshLoader,
    loaded: BTreeMap<String, Result<Arc<TriangleMesh>, Diagnostic>>,
}

impl ColliderMeshes {
    /// A library that reads meshes through `loader`.
    pub fn new(loader: MeshLoader) -> Self {
        Self {
            loader,
            loaded: BTreeMap::new(),
        }
    }

    /// The mesh at `path`, loaded on first use. A failure is memoized too, so a missing document is
    /// read once, not every tick.
    pub fn get(&mut self, path: &str) -> Result<Arc<TriangleMesh>, Diagnostic> {
        if let Some(loaded) = self.loaded.get(path) {
            return loaded.clone();
        }
        let loaded = (self.loader)(path).map(Arc::new);
        self.loaded.insert(path.to_string(), loaded.clone());
        loaded
    }
}
