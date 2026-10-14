//! Implementation of `ludion check-scene --placement`: warnings for placed props that float, sink
//! or overlap another prop's collider.
//!
//! The unit is a *placement group*: a depth-0 prefab instance (all its parts), or else a hierarchy
//! root with every `Parent` descendant. Parts of one group never support or overlap each other.
//! Vertical rules (floating, sunk, contact) read the group's world mesh box, where a protrusion can
//! only add support. Horizontal overlap reads the static in-plane `Collider`s, the authored solid
//! footprint, through the same `spatial::overlaps` the game's collision uses. All three are
//! heuristics, so warnings: they never change the exit code.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use ludion_core::codes::code;
use ludion_core::diagnostics::Diagnostic;
use ludion_core::math::Affine3;
use ludion_core::time::{FrameIndex, TickRate};
use ludion_ecs::{SystemCtx, World};
use ludion_engine::{Mesh, MeshGrid, MeshSource, Transform};
use ludion_scene::{ExpandedScene, InstantiateOptions, Scene, SceneEntity, instantiate_expanded};
use ludion_spatial::{Body, Collider, Shape};
use serde_json::Value;

/// The height of the ground the props stand on, in metres.
pub const DEFAULT_GROUND_Y: f64 = 0.0;

/// The slack for floating, contact and overlap, in metres.
pub const DEFAULT_TOLERANCE: f64 = 0.05;

/// How far below the ground a prop may sit before it is sunk, in metres. Ground tiles are embedded
/// by design (a cobble's bottom sits a few centimetres under the surface).
pub const DEFAULT_SINK: f64 = 0.1;

/// The resolved lint tunables.
#[derive(Debug, Clone, Copy)]
pub struct PlacementOptions {
    pub ground_y: f64,
    pub tolerance: f64,
    pub sink: f64,
}

impl PlacementOptions {
    /// The flags' values, each defaulting to its named constant.
    pub fn new(ground_y: Option<f64>, tolerance: Option<f64>, sink: Option<f64>) -> Self {
        Self {
            ground_y: ground_y.unwrap_or(DEFAULT_GROUND_Y),
            tolerance: tolerance.unwrap_or(DEFAULT_TOLERANCE),
            sink: sink.unwrap_or(DEFAULT_SINK),
        }
    }
}

/// What the lint found.
pub struct PlacementReport {
    /// The warnings, already located in the document.
    pub diagnostics: Vec<Diagnostic>,
    /// Groups with at least one measured mesh.
    pub groups: usize,
    /// Mesh entities whose geometry could not be measured (voxel, sculpt, unreadable document...).
    pub unmeasured: usize,
}

impl PlacementReport {
    /// The one-line stdout summary.
    pub fn summary(&self, options: &PlacementOptions) -> String {
        format!(
            "placement: {} groups checked, {} unmeasured, {} warnings (ground y {}, tolerance {} m, sink {} m)",
            self.groups,
            self.unmeasured,
            self.diagnostics.len(),
            options.ground_y,
            options.tolerance,
            options.sink
        )
    }
}

type Box3 = ([f32; 3], [f32; 3]);

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum GroupKey {
    Instance(usize),
    Entity(usize),
}

struct Group {
    key: GroupKey,
    members: Vec<usize>,
    bounds: Option<Box3>,
}

/// Runs the lint over a scene's expanded entities, mesh documents read relative to `root`.
pub(crate) fn lint(
    root: &Path,
    expanded: &ExpandedScene,
    options: &PlacementOptions,
) -> PlacementReport {
    let entities = &expanded.scene.entities;
    let stripped = stripped(expanded);
    let empty = || PlacementReport {
        diagnostics: Vec::new(),
        groups: 0,
        unmeasured: 0,
    };
    let mut world = World::new();
    let Ok(ids) = instantiate_expanded(
        &mut world,
        &stripped,
        &ludion_engine::builtin_registry(),
        &InstantiateOptions {
            placement: None,
            name_prefix: None,
            hooks: &ludion_engine::EngineHooks,
        },
    ) else {
        return empty();
    };
    ludion_spatial::spatial::rebuild::<Transform>(
        &mut world,
        &SystemCtx::new(FrameIndex::ZERO, TickRate::DEFAULT),
    );

    let mut instance_of = vec![None; entities.len()];
    for (k, instance) in expanded.instances.iter().enumerate() {
        if instance.depth == 0 {
            for part in instance.parts.clone() {
                instance_of[part] = Some(k);
            }
        }
    }
    let mut groups: Vec<Group> = Vec::new();
    let mut by_key: BTreeMap<GroupKey, usize> = BTreeMap::new();
    let mut group_of = Vec::with_capacity(entities.len());
    for i in 0..entities.len() {
        let top = top(entities, i);
        let key = instance_of[top].map_or(GroupKey::Entity(top), GroupKey::Instance);
        let id = *by_key.entry(key).or_insert_with(|| {
            groups.push(Group {
                key,
                members: Vec::new(),
                bounds: None,
            });
            groups.len() - 1
        });
        groups[id].members.push(i);
        group_of.push(id);
    }

    let boxes = read_boxes(root, entities);
    let mut unmeasured = 0;
    for group in &mut groups {
        for &i in &group.members {
            let affine = ludion_engine::world_affine(&world, ids[i]);
            let mut place = |measured: Option<Box3>| match measured {
                Some(found) => group.bounds = Some(union(group.bounds, found)),
                None => unmeasured += 1,
            };
            if let Some(value) = component(&entities[i], "Mesh") {
                place(
                    serde_json::from_value::<Mesh>(value.clone())
                        .ok()
                        .and_then(|mesh| local_box(&boxes, &mesh))
                        .zip(affine)
                        .map(|(local, affine)| world_box(&affine, local)),
                );
            }
            let grid = component(&entities[i], "MeshGrid")
                .and_then(|value| serde_json::from_value::<MeshGrid>(value.clone()).ok());
            if let Some(grid) = grid {
                let mut glyph_buf = [0u8; 4];
                for (row, line) in grid.rows.iter().enumerate() {
                    for (col, glyph) in line.chars().enumerate() {
                        let Some(mesh) = grid.legend.get(glyph.encode_utf8(&mut glyph_buf)) else {
                            continue;
                        };
                        let cell = ludion_engine::mesh_grid_cell(&grid, col, row)
                            .0
                            .to_cols_array_2d();
                        let cell = Affine3 {
                            cols: [
                                [cell[0][0], cell[0][1], cell[0][2]],
                                [cell[1][0], cell[1][1], cell[1][2]],
                                [cell[2][0], cell[2][1], cell[2][2]],
                                [cell[3][0], cell[3][1], cell[3][2]],
                            ],
                        };
                        place(
                            local_box(&boxes, mesh)
                                .zip(affine)
                                .map(|(local, affine)| world_box(&affine.mul_affine(&cell), local)),
                        );
                    }
                }
            }
        }
    }

    let label = |group: &Group| -> String {
        match group.key {
            GroupKey::Instance(k) => expanded.instances[k].name.clone(),
            GroupKey::Entity(top) => entities[top]
                .name
                .clone()
                .unwrap_or_else(|| format!("entity {}", expanded.entry_of[top])),
        }
    };
    let locate = |group: &Group, diagnostic: Diagnostic| -> Diagnostic {
        match group.key {
            GroupKey::Instance(k) => {
                diagnostic.with_path(format!("/entities/{}", expanded.instances[k].entry))
            }
            GroupKey::Entity(top) => {
                let tail = if component(&entities[top], "Transform").is_some() {
                    "/components/Transform"
                } else {
                    ""
                };
                expanded.locate(diagnostic.with_path(format!("/entities/{top}{tail}")))
            }
        }
    };

    let tolerance = options.tolerance;
    let mut diagnostics = Vec::new();
    for (g, group) in groups.iter().enumerate() {
        let Some((lo, hi)) = group.bounds else {
            continue;
        };
        let name = label(group);
        let bottom = f64::from(lo[1]);
        if bottom < options.ground_y - options.sink {
            diagnostics.push(locate(
                group,
                Diagnostic::new(
                    code::PLACEMENT_SUNK,
                    format!(
                        "`{name}` is sunk {:.2} m below the ground (y {})",
                        options.ground_y - bottom,
                        options.ground_y
                    ),
                )
                .with_help("raise it; ground tiles embed less than --sink"),
            ));
        } else if bottom > options.ground_y + tolerance {
            let supported = groups.iter().enumerate().any(|(h, other)| {
                h != g
                    && other
                        .bounds
                        .is_some_and(|(olo, ohi)| touches((lo, hi), (olo, ohi), tolerance))
            });
            if !supported {
                diagnostics.push(locate(
                    group,
                    Diagnostic::new(
                        code::PLACEMENT_FLOATING,
                        format!(
                            "`{name}` floats {:.2} m above the ground (y {}) and touches no other prop",
                            bottom - options.ground_y,
                            options.ground_y
                        ),
                    )
                    .with_help("lower it onto the ground or a supporting prop, or raise --tolerance"),
                ));
            }
        }
    }

    let flat_of: BTreeMap<u32, usize> = ids
        .iter()
        .enumerate()
        .map(|(i, id)| (id.index(), i))
        .collect();
    let mut pairs: BTreeSet<(usize, usize)> = BTreeSet::new();
    for (i, entity) in entities.iter().enumerate() {
        let Some((shape, center)) = static_footprint(entity).and_then(|shape| {
            let affine = ludion_engine::world_affine(&world, ids[i])?;
            Some((
                shrunk(shape, tolerance),
                [affine.cols[3][0], affine.cols[3][2]],
            ))
        }) else {
            continue;
        };
        for hit in
            ludion_spatial::spatial::overlaps(&world, shape, center, Some(Body::new_static()))
        {
            if let Some(&j) = flat_of.get(&hit.index())
                && group_of[j] > group_of[i]
            {
                pairs.insert((group_of[i], group_of[j]));
            }
        }
    }
    for (a, b) in pairs {
        diagnostics.push(locate(
            &groups[a],
            Diagnostic::new(
                code::PLACEMENT_OVERLAP,
                format!(
                    "`{}`'s collider overlaps `{}`'s",
                    label(&groups[a]),
                    label(&groups[b])
                ),
            )
            .with_help("move one apart; colliders are the solid footprints the game collides with"),
        ));
    }

    PlacementReport {
        diagnostics,
        groups: groups.iter().filter(|group| group.bounds.is_some()).count(),
        unmeasured,
    }
}

/// `expanded` reduced to what the lint reads: every entity kept (document positions and `Parent`
/// indices stay aligned), each holding only `Transform`, `Parent` and `Collider`, all sim.
pub(crate) fn stripped(expanded: &ExpandedScene) -> ExpandedScene {
    let entities = expanded
        .scene
        .entities
        .iter()
        .map(|entity| SceneEntity {
            presentation: false,
            name: entity.name.clone(),
            components: entity
                .components
                .iter()
                .filter(|(name, _)| matches!(name.as_str(), "Transform" | "Parent" | "Collider"))
                .cloned()
                .collect(),
            instance: None,
        })
        .collect();
    ExpandedScene {
        scene: Scene {
            entities,
            ..Scene::default()
        },
        instances: expanded.instances.clone(),
        entry_of: expanded.entry_of.clone(),
    }
}

fn component<'a>(entity: &'a SceneEntity, wanted: &str) -> Option<&'a Value> {
    entity
        .components
        .iter()
        .find(|(name, _)| name == wanted)
        .map(|(_, value)| value)
}

/// The hierarchy root above entity `i`.
fn top(entities: &[SceneEntity], mut i: usize) -> usize {
    for _ in 0..entities.len() {
        let parent = component(&entities[i], "Parent")
            .and_then(|value| value.get("index"))
            .and_then(Value::as_u64)
            .and_then(|parent| usize::try_from(parent).ok())
            .filter(|&parent| parent < entities.len());
        match parent {
            Some(parent) => i = parent,
            None => break,
        }
    }
    i
}

/// The mesh's model-space box; `None` for geometry the lint does not measure.
fn local_box(boxes: &BTreeMap<String, Option<Box3>>, mesh: &Mesh) -> Option<Box3> {
    match &mesh.shape {
        MeshSource::Cube | MeshSource::Sphere => Some(([-0.5; 3], [0.5; 3])),
        MeshSource::Plane => Some(([-0.5, 0.0, -0.5], [0.5, 0.0, 0.5])),
        MeshSource::Asset(path) => boxes.get(path).copied().flatten(),
        _ => None,
    }
}

/// The box of every distinct mesh document the scene names, read and validated in parallel: a
/// town's few hundred props share a few dozen documents, and validating each is the lint's cost.
fn read_boxes(root: &Path, entities: &[SceneEntity]) -> BTreeMap<String, Option<Box3>> {
    let paths: BTreeSet<String> = entities
        .iter()
        .flat_map(|entity| {
            let single = component(entity, "Mesh")
                .and_then(|value| serde_json::from_value::<Mesh>(value.clone()).ok());
            let grid = component(entity, "MeshGrid")
                .and_then(|value| serde_json::from_value::<MeshGrid>(value.clone()).ok());
            single
                .into_iter()
                .chain(grid.into_iter().flat_map(|grid| grid.legend.into_values()))
        })
        .filter_map(|mesh| match mesh.shape {
            MeshSource::Asset(path) => Some(path),
            _ => None,
        })
        .collect();
    let paths: Vec<String> = paths.into_iter().collect();
    let threads = std::thread::available_parallelism().map_or(1, |n| n.get());
    let chunk = paths.len().div_ceil(threads).max(1);
    std::thread::scope(|scope| {
        let handles: Vec<_> = paths
            .chunks(chunk)
            .map(|paths| {
                scope.spawn(move || {
                    paths
                        .iter()
                        .map(|path| (path.clone(), read_box(&root.join(path))))
                        .collect::<Vec<_>>()
                })
            })
            .collect();
        handles
            .into_iter()
            .flat_map(|handle| handle.join().expect("a mesh read does not panic"))
            .collect()
    })
}

fn read_box(path: &Path) -> Option<Box3> {
    let text = std::fs::read_to_string(path).ok()?;
    let document = ludion_scene::check_mesh(&text).ok()?;
    let (lo, hi) = crate::check_scene::mesh_box(&document)?;
    Some((lo.map(|v| v as f32), hi.map(|v| v as f32)))
}

fn world_box(affine: &ludion_core::math::Affine3, (lo, hi): Box3) -> Box3 {
    let mut min = [f32::INFINITY; 3];
    let mut max = [f32::NEG_INFINITY; 3];
    for corner in 0..8 {
        let point = affine.transform_point([
            if corner & 1 == 0 { lo[0] } else { hi[0] },
            if corner & 2 == 0 { lo[1] } else { hi[1] },
            if corner & 4 == 0 { lo[2] } else { hi[2] },
        ]);
        for axis in 0..3 {
            min[axis] = min[axis].min(point[axis]);
            max[axis] = max[axis].max(point[axis]);
        }
    }
    (min, max)
}

fn union(current: Option<Box3>, (lo, hi): Box3) -> Box3 {
    match current {
        None => (lo, hi),
        Some((clo, chi)) => (
            std::array::from_fn(|k| clo[k].min(lo[k])),
            std::array::from_fn(|k| chi[k].max(hi[k])),
        ),
    }
}

/// Whether `a` grown by `tolerance` meets `b`.
fn touches(a: Box3, b: Box3, tolerance: f64) -> bool {
    let t = tolerance as f32;
    (0..3).all(|k| a.0[k] - t <= b.1[k] && b.0[k] <= a.1[k] + t)
}

/// The entity's static in-plane collider shape; 3D, rigid and kinematic colliders are not
/// footprints.
fn static_footprint(entity: &SceneEntity) -> Option<Shape> {
    let collider: Collider = serde_json::from_value(component(entity, "Collider")?.clone()).ok()?;
    (matches!(collider.body, Body::Static(_)) && !collider.shape.is_3d()).then_some(collider.shape)
}

/// `shape` shrunk by `tolerance` on every side, so props that merely touch do not overlap.
fn shrunk(shape: Shape, tolerance: f64) -> Shape {
    let t = tolerance as f32;
    match shape {
        Shape::Circle { radius } => Shape::Circle {
            radius: (radius - t).max(1e-4),
        },
        Shape::Aabb { half_extents } => Shape::Aabb {
            half_extents: half_extents.map(|h| (h - t).max(1e-4)),
        },
        other => other,
    }
}
