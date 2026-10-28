//! A wireframe read of the live world: every body's collider as world-space line segments, for the
//! host's `colliders` debug-draw category. Read-only — nothing here touches solver or sim state.

use std::collections::BTreeMap;
use std::sync::Arc;

use rapier3d::parry::shape::Shape;
use rapier3d::prelude::{Pose, Vector};

use crate::world::{BodyKind, PhysicsWorld};

/// Segments per circle of a ball, capsule or cylinder outline.
const OUTLINE_SUBDIV: u32 = 16;

/// Two triangle normals at least this aligned (cosine) make their shared edge internal to a flat
/// face, so it is not drawn.
const COPLANAR_COS: f32 = 0.9999;

/// What a drawn collider's body is, which is all the host needs to colour it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WireBody {
    /// A body that never moves.
    Static,
    /// A body the ECS poses.
    Kinematic,
    /// A solver-driven body, asleep or not.
    Rigid {
        /// Whether the solver has put the body to sleep.
        sleeping: bool,
    },
    /// A sensor collider, whatever its body kind.
    Sensor {
        /// Whether any non-sensor entity is inside it as of the last step.
        occupied: bool,
    },
}

/// The edges of a triangle mesh worth drawing: every edge except those shared by exactly two
/// coplanar triangles, so a flat floor reads as its outline rather than its triangulation.
///
/// Edges are deduplicated by vertex index, so the mesh must share its vertices. A zero-area triangle
/// is never coplanar with anything. The result is sorted, hence deterministic.
///
/// # Example
/// ```
/// use ludion_physics::feature_edges;
///
/// let quad = [[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [1.0, 0.0, 1.0], [0.0, 0.0, 1.0]];
/// let edges = feature_edges(&quad, &[[0, 2, 1], [0, 3, 2]]);
/// assert_eq!(edges.len(), 4); // the diagonal is internal
/// ```
pub fn feature_edges(vertices: &[[f32; 3]], triangles: &[[u32; 3]]) -> Vec<[u32; 2]> {
    let normal = |triangle: &[u32; 3]| -> Option<[f32; 3]> {
        let [a, b, c] = triangle.map(|i| vertices[i as usize]);
        let u = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
        let v = [c[0] - a[0], c[1] - a[1], c[2] - a[2]];
        let n = [
            u[1] * v[2] - u[2] * v[1],
            u[2] * v[0] - u[0] * v[2],
            u[0] * v[1] - u[1] * v[0],
        ];
        let length = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
        (length > 0.0 && length.is_finite()).then(|| n.map(|x| x / length))
    };
    let normals: Vec<Option<[f32; 3]>> = triangles.iter().map(normal).collect();

    let mut sides: Vec<(u32, u32, usize)> = Vec::with_capacity(triangles.len() * 3);
    for (row, triangle) in triangles.iter().enumerate() {
        for k in 0..3 {
            let (a, b) = (triangle[k], triangle[(k + 1) % 3]);
            sides.push((a.min(b), a.max(b), row));
        }
    }
    sides.sort_unstable();

    let mut edges = Vec::new();
    let mut rest = sides.as_slice();
    while let Some(&(a, b, first)) = rest.first() {
        let count = rest
            .iter()
            .take_while(|side| side.0 == a && side.1 == b)
            .count();
        let flat = count == 2
            && match (normals[first], normals[rest[1].2]) {
                (Some(n1), Some(n2)) => {
                    n1[0] * n2[0] + n1[1] * n2[1] + n1[2] * n2[2] >= COPLANAR_COS
                }
                _ => false,
            };
        if !flat {
            edges.push([a, b]);
        }
        rest = &rest[count..];
    }
    edges
}

/// Merges vertices at bit-identical positions, so edges of a mesh that repeats its corners per
/// triangle (parry's heightfield) dedupe like a shared-vertex one.
fn weld(vertices: &[Vector], triangles: &[[u32; 3]]) -> (Vec<[f32; 3]>, Vec<[u32; 3]>) {
    let mut seen: BTreeMap<[u32; 3], u32> = BTreeMap::new();
    let mut welded: Vec<[f32; 3]> = Vec::new();
    let remap: Vec<u32> = vertices
        .iter()
        .map(|v| {
            *seen
                .entry([v.x.to_bits(), v.y.to_bits(), v.z.to_bits()])
                .or_insert_with(|| {
                    welded.push([v.x, v.y, v.z]);
                    (welded.len() - 1) as u32
                })
        })
        .collect();
    let triangles = triangles
        .iter()
        .map(|t| t.map(|i| remap[i as usize]))
        .collect();
    (welded, triangles)
}

/// The sink a wireframe walk emits segments into.
type Emit<'a> = &'a mut dyn FnMut(WireBody, [f32; 3], [f32; 3]);

fn emit_edges(
    pose: &Pose,
    body: WireBody,
    vertices: &[Vector],
    edges: impl Iterator<Item = [u32; 2]>,
    emit: Emit<'_>,
) {
    let point = |i: u32| {
        let p = *pose * vertices[i as usize];
        [p.x, p.y, p.z]
    };
    for [a, b] in edges {
        emit(body, point(a), point(b));
    }
}

impl PhysicsWorld {
    /// Calls `emit(body, from, to)` once per wireframe segment of every body's collider, in the
    /// handle table's creation order, in world space.
    ///
    /// Boxes, balls, capsules and cylinders draw their parry outlines; meshes, convex hulls and
    /// heightfields draw their triangle edges minus the coplanar internal ones (a mesh's edges are
    /// cached per content hash); a compound draws each part in its part pose. Output is a pure
    /// function of the world, and reading it never changes the world or its snapshot.
    ///
    /// # Example
    /// ```
    /// use ludion_core::time::TickRate;
    /// use ludion_physics::PhysicsWorld;
    ///
    /// let world = PhysicsWorld::new(TickRate::DEFAULT);
    /// let mut segments = 0;
    /// world.collider_wireframe(&mut |_, _, _| segments += 1);
    /// assert_eq!(segments, 0);
    /// ```
    pub fn collider_wireframe(&self, emit: &mut dyn FnMut(WireBody, [f32; 3], [f32; 3])) {
        for entry in &self.table {
            let Some(collider) = self.rapier.colliders.get(entry.collider) else {
                continue;
            };
            let body = match entry.kind {
                _ if entry.source.sensor => WireBody::Sensor {
                    occupied: !self.sensor_occupants(entry.entity).is_empty(),
                },
                BodyKind::Fixed => WireBody::Static,
                BodyKind::Kinematic => WireBody::Kinematic,
                BodyKind::Rigid => WireBody::Rigid {
                    sleeping: self
                        .rapier
                        .bodies
                        .get(entry.body)
                        .is_some_and(|body| body.is_sleeping()),
                },
            };
            self.shape_wireframe(
                collider.shape(),
                collider.position(),
                body,
                entry.mesh_hash,
                emit,
            );
        }
    }

    fn shape_wireframe(
        &self,
        shape: &dyn Shape,
        pose: &Pose,
        body: WireBody,
        mesh_hash: Option<u64>,
        emit: Emit<'_>,
    ) {
        if let Some(ball) = shape.as_ball() {
            let (vertices, edges) = ball.to_outline(OUTLINE_SUBDIV);
            emit_edges(pose, body, &vertices, edges.into_iter(), emit);
        } else if let Some(cuboid) = shape.as_cuboid() {
            let (vertices, edges) = cuboid.to_outline();
            emit_edges(pose, body, &vertices, edges.into_iter(), emit);
        } else if let Some(capsule) = shape.as_capsule() {
            let (vertices, edges) = capsule.to_outline(OUTLINE_SUBDIV);
            emit_edges(pose, body, &vertices, edges.into_iter(), emit);
        } else if let Some(cylinder) = shape.as_cylinder() {
            let (vertices, edges) = cylinder.to_outline(OUTLINE_SUBDIV);
            emit_edges(pose, body, &vertices, edges.into_iter(), emit);
        } else if let Some(mesh) = shape.as_trimesh() {
            let edges = self.mesh_edges(mesh_hash, || {
                let vertices: Vec<[f32; 3]> =
                    mesh.vertices().iter().map(|v| [v.x, v.y, v.z]).collect();
                feature_edges(&vertices, mesh.indices())
            });
            emit_edges(pose, body, mesh.vertices(), edges.iter().copied(), emit);
        } else if let Some(convex) = shape.as_convex_polyhedron() {
            let (vertices, triangles) = convex.to_trimesh();
            let vertices: Vec<Vector> = vertices
                .iter()
                .map(|v| Vector::new(v.x, v.y, v.z))
                .collect();
            self.welded_wireframe(pose, body, &vertices, &triangles, emit);
        } else if let Some(heightfield) = shape.as_heightfield() {
            let (vertices, triangles) = heightfield.to_trimesh();
            let vertices: Vec<Vector> = vertices
                .iter()
                .map(|v| Vector::new(v.x, v.y, v.z))
                .collect();
            self.welded_wireframe(pose, body, &vertices, &triangles, emit);
        } else if let Some(compound) = shape.as_compound() {
            for (part_pose, part) in compound.shapes() {
                self.shape_wireframe(&**part, &(*pose * *part_pose), body, None, emit);
            }
        }
    }

    fn welded_wireframe(
        &self,
        pose: &Pose,
        body: WireBody,
        vertices: &[Vector],
        triangles: &[[u32; 3]],
        emit: Emit<'_>,
    ) {
        let (welded, triangles) = weld(vertices, triangles);
        let edges = feature_edges(&welded, &triangles);
        let vertices: Vec<Vector> = welded
            .iter()
            .map(|v| Vector::new(v[0], v[1], v[2]))
            .collect();
        emit_edges(pose, body, &vertices, edges.into_iter(), emit);
    }

    /// The feature edges of the mesh content `hash`, computed by `build` on a miss.
    fn mesh_edges(
        &self,
        hash: Option<u64>,
        build: impl FnOnce() -> Vec<[u32; 2]>,
    ) -> Arc<[[u32; 2]]> {
        let Some(hash) = hash else {
            return build().into();
        };
        let mut cache = self
            .wire_edges
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        Arc::clone(cache.entry(hash).or_insert_with(|| build().into()))
    }
}
