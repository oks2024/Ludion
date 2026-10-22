//! Scene checks for `ReflectionProbe` placement (`Plan/reflections.md` decisions 21 and 23).
//!
//! The component's own validator sees one probe's JSON; these checks need every probe's *world*
//! pose, so they instantiate a stripped copy of the scene and run the extract's own
//! [`extract_probes`](crate::extract::extract_probes) over it — the pose a check judges is the pose
//! the renderer will use.

use ludion_core::codes::code;
use ludion_core::diagnostics::Diagnostic;
use ludion_core::ids::EntityId;
use ludion_core::math::glam::{Quat, Vec3};
use ludion_renderer::{MAX_PROBES, ProbeData, ProbeVolume, probe_rank};
use ludion_scene::Scene;

use crate::hierarchy::WorldAffines;

/// Positions and sizes closer than this are the same (a copy-paste, not a near miss).
const SAME_EPSILON: f32 = 1e-3;
/// Overlap and containment slack, so touching volumes and exact fits are not flagged.
const CONTACT_EPSILON: f32 = 1e-4;

const COMPONENT: &str = "ReflectionProbe";

/// Every probe-placement diagnostic for `scene`: `L1277` duplicates, `L1275` equal-rank overlaps,
/// `L1276` shadowed probes (inside a higher-ranked probe's volume, where its weight is 1) and `L1278` for more
/// than [`MAX_PROBES`] probes. Pair diagnostics point at the lower-ranked probe and carry the other
/// as `related`.
///
/// Shadowing is detected against a single covering probe only; a union of several is not, and a
/// missed warning is safe. Probes whose values `L1274` refuses are skipped here.
///
/// ```
/// let scene = ludion_scene::check(
///     "format: 1\nentities:\n  - components:\n      ReflectionProbe: {}\n  \
///      - components:\n      ReflectionProbe: {}\n",
///     &ludion_engine::builtin_registry(),
/// )
/// .unwrap();
/// let diagnostics = ludion_engine::scene_probe_diagnostics(&scene);
/// assert_eq!(diagnostics.len(), 1);
/// assert_eq!(diagnostics[0].code.as_str(), "L1277");
/// ```
pub fn scene_probe_diagnostics(scene: &Scene) -> Vec<Diagnostic> {
    let carriers: Vec<usize> = (0..scene.entities.len())
        .filter(|&i| has_probe(scene, i))
        .collect();
    if carriers.is_empty() {
        return Vec::new();
    }
    let mut diagnostics = cap_diagnostics(scene, &carriers);

    let Some(probes) = world_probes(scene) else {
        return diagnostics;
    };
    let mut shadowed: Vec<Option<usize>> = vec![None; probes.len()];
    for a in 0..probes.len() {
        for b in a + 1..probes.len() {
            let (hi, lo) = if probe_rank(&probes[a].1, &probes[b].1).is_le() {
                (a, b)
            } else {
                (b, a)
            };
            let (hi_probe, lo_probe) = (&probes[hi].1, &probes[lo].1);
            if is_duplicate(hi_probe, lo_probe) {
                diagnostics.push(pair(
                    scene,
                    code::REFLECTION_PROBE_DUPLICATE,
                    probes[lo].0,
                    probes[hi].0,
                    "has the same shape, size and pose as the probe on",
                    "delete one of the two, or move it to where it was meant to go",
                ));
            } else if equal_rank(hi_probe, lo_probe) && overlaps(hi_probe, lo_probe) {
                diagnostics.push(pair(
                    scene,
                    code::REFLECTION_PROBE_EQUAL_RANK,
                    probes[lo].0,
                    probes[hi].0,
                    "overlaps, with equal priority and volume, the probe on",
                    "give one of the two a higher `priority`, or move them apart",
                ));
            } else if shadowed[lo].is_none() && covers(hi_probe, lo_probe) {
                shadowed[lo] = Some(hi);
            }
        }
    }
    for (lo, hi) in shadowed.iter().enumerate() {
        if let Some(hi) = *hi {
            diagnostics.push(pair(
                scene,
                code::REFLECTION_PROBE_SHADOWED,
                probes[lo].0,
                probes[hi].0,
                "lies entirely inside the volume, where the weight is 1, of the higher-ranked probe on",
                "raise its `priority`, shrink or move the other probe, or delete it",
            ));
        }
    }
    diagnostics
}

fn has_probe(scene: &Scene, index: usize) -> bool {
    scene.entities[index]
        .components
        .iter()
        .any(|(name, _)| name == COMPONENT)
}

fn pointer(index: usize) -> String {
    format!("/entities/{index}/components/{COMPONENT}")
}

fn label(scene: &Scene, index: usize) -> String {
    match scene.entities[index].name.as_ref() {
        Some(name) => format!("entity {index} (`{name}`)"),
        None => format!("entity {index}"),
    }
}

/// `L1278`: one error at the first probe past the cap, a related diagnostic per probe after it.
fn cap_diagnostics(scene: &Scene, carriers: &[usize]) -> Vec<Diagnostic> {
    let Some((&first, rest)) = carriers[MAX_PROBES.min(carriers.len())..].split_first() else {
        return Vec::new();
    };
    let mut diagnostic = Diagnostic::new(
        code::REFLECTION_PROBE_OVER_CAP,
        format!(
            "the scene has {} reflection probes, over the cap of {MAX_PROBES}; {} is the first \
             past it",
            carriers.len(),
            label(scene, first)
        ),
    )
    .with_path(pointer(first))
    .with_help("merge neighbouring probes; streaming probes in and out is not supported");
    for &index in rest {
        diagnostic = diagnostic.with_related(
            Diagnostic::new(
                code::REFLECTION_PROBE_OVER_CAP,
                format!("{} is also past the cap", label(scene, index)),
            )
            .with_path(pointer(index)),
        );
    }
    vec![diagnostic]
}

/// A pair diagnostic at `lo` (the lower-ranked probe) with `hi` as its related entry.
fn pair(
    scene: &Scene,
    diagnostic_code: ludion_core::diagnostics::DiagnosticCode,
    lo: usize,
    hi: usize,
    relation: &str,
    help: &str,
) -> Diagnostic {
    Diagnostic::new(
        diagnostic_code,
        format!(
            "reflection probe on {} {relation} {}",
            label(scene, lo),
            label(scene, hi)
        ),
    )
    .with_path(pointer(lo))
    .with_help(help)
    .with_related(Diagnostic::new(diagnostic_code, "the other probe").with_path(pointer(hi)))
}

/// Each probe with its document index, posed exactly as the extract poses it: a scene copy keeping
/// only `Transform`, `Parent` and `ReflectionProbe` (every entity stays so `Parent` indices hold) is
/// instantiated against the built-in registry, which a game's own components would not satisfy.
fn world_probes(scene: &Scene) -> Option<Vec<(usize, ProbeData)>> {
    let mut stripped = scene.clone();
    stripped.physics = None;
    for entity in &mut stripped.entities {
        entity
            .components
            .retain(|(name, _)| matches!(name.as_str(), "Transform" | "Parent" | COMPONENT));
    }
    let world = ludion_scene::instantiate(&stripped, &crate::builtin_registry()).ok()?;
    let ids: Vec<EntityId> = ludion_scene::instantiated_ids(&stripped);
    let probes = crate::extract::extract_probes(&world, &ids, &mut WorldAffines::new());
    Some(
        probes
            .into_iter()
            .filter_map(|probe| {
                let index = ids.iter().position(|&id| id == probe.entity)?;
                Some((index, probe))
            })
            .collect(),
    )
}

fn is_duplicate(a: &ProbeData, b: &ProbeData) -> bool {
    let near = |x: f32, y: f32| (x - y).abs() <= SAME_EPSILON;
    if (a.position - b.position).abs().max_element() > SAME_EPSILON {
        return false;
    }
    match (a.volume, b.volume) {
        (ProbeVolume::Box { extent: ea }, ProbeVolume::Box { extent: eb }) => {
            (ea - eb).abs().max_element() <= SAME_EPSILON
                && quat_dot(a.rotation, b.rotation).abs() >= 1.0 - 1e-6
        }
        (ProbeVolume::Sphere { radius: ra }, ProbeVolume::Sphere { radius: rb }) => near(ra, rb),
        _ => false,
    }
}

/// Quaternion dot, component by component in a fixed order (glam's `dot` rounds per backend).
fn quat_dot(a: Quat, b: Quat) -> f32 {
    a.x * b.x + a.y * b.y + a.z * b.z + a.w * b.w
}

fn equal_rank(a: &ProbeData, b: &ProbeData) -> bool {
    a.priority == b.priority
        && a.volume.is_sphere() == b.volume.is_sphere()
        && (a.size() - b.size()).abs() <= 1e-4 * a.size().max(b.size())
}

/// A box's local axes in world space.
fn box_axes(rotation: Quat) -> [Vec3; 3] {
    [rotation * Vec3::X, rotation * Vec3::Y, rotation * Vec3::Z]
}

/// Whether two probes' influence regions (each volume grown by its `blend`) overlap with positive
/// depth; touching is not overlap. They meet when the volumes are closer than the summed blends.
fn overlaps(a: &ProbeData, b: &ProbeData) -> bool {
    let margin = a.blend + b.blend;
    match (a.volume, b.volume) {
        (ProbeVolume::Sphere { radius: ra }, ProbeVolume::Sphere { radius: rb }) => {
            a.position.distance(b.position) < ra + rb + margin - CONTACT_EPSILON
        }
        (ProbeVolume::Box { extent }, ProbeVolume::Sphere { radius }) => {
            box_sphere_overlap(a, extent, b, radius, margin)
        }
        (ProbeVolume::Sphere { radius }, ProbeVolume::Box { extent }) => {
            box_sphere_overlap(b, extent, a, radius, margin)
        }
        (ProbeVolume::Box { extent: ea }, ProbeVolume::Box { extent: eb }) => {
            box_box_overlap(a, ea, b, eb, margin)
        }
    }
}

fn box_sphere_overlap(
    boxed: &ProbeData,
    extent: Vec3,
    sphere: &ProbeData,
    radius: f32,
    margin: f32,
) -> bool {
    let local = boxed.rotation.inverse() * (sphere.position - boxed.position);
    let closest = local.clamp(-extent, extent);
    local.distance(closest) < radius + margin - CONTACT_EPSILON
}

/// Separating-axis test over the 3 + 3 face axes and 9 edge cross axes. The `margin` slack on every
/// axis is conservative: it can over-report near edges and corners of the grown boxes.
fn box_box_overlap(a: &ProbeData, ea: Vec3, b: &ProbeData, eb: Vec3, margin: f32) -> bool {
    let (axes_a, axes_b) = (box_axes(a.rotation), box_axes(b.rotation));
    let offset = b.position - a.position;
    let radius = |axes: &[Vec3; 3], extent: Vec3, axis: Vec3| {
        axes[0].dot(axis).abs() * extent.x
            + axes[1].dot(axis).abs() * extent.y
            + axes[2].dot(axis).abs() * extent.z
    };
    let mut candidates = Vec::with_capacity(15);
    candidates.extend(axes_a);
    candidates.extend(axes_b);
    for axis_a in axes_a {
        for axis_b in axes_b {
            candidates.push(axis_a.cross(axis_b));
        }
    }
    candidates.into_iter().all(|axis| {
        if axis.length_squared() < 1e-12 {
            return true;
        }
        let axis = axis.normalize();
        let depth = radius(&axes_a, ea, axis) + radius(&axes_b, eb, axis) - offset.dot(axis).abs();
        depth + margin > CONTACT_EPSILON
    })
}

/// Whether `container`'s full-weight region (its volume itself) holds all of `inner`'s influence
/// region (its volume grown by its own `blend`).
fn covers(container: &ProbeData, inner: &ProbeData) -> bool {
    // Points of `inner` that must lie inside, with the extra slack a sphere and the blend need.
    let (points, slack): (Vec<Vec3>, f32) = match inner.volume {
        ProbeVolume::Box { extent } => (
            (0..8)
                .map(|corner| {
                    let sign = |bit: u32| if corner & bit == 0 { -1.0 } else { 1.0 };
                    inner.position
                        + inner.rotation
                            * Vec3::new(sign(1) * extent.x, sign(2) * extent.y, sign(4) * extent.z)
                })
                .collect(),
            inner.blend,
        ),
        ProbeVolume::Sphere { radius } => (vec![inner.position], radius + inner.blend),
    };
    match container.volume {
        ProbeVolume::Box { extent } => {
            let inverse = container.rotation.inverse();
            points.iter().all(|&point| {
                let local = (inverse * (point - container.position)).abs() + Vec3::splat(slack);
                local.cmple(extent + Vec3::splat(CONTACT_EPSILON)).all()
            })
        }
        ProbeVolume::Sphere { radius } => points
            .iter()
            .all(|&point| point.distance(container.position) + slack <= radius + CONTACT_EPSILON),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A cube shape of the given half-extent.
    fn cube(half: f32) -> String {
        format!("box:\n            extent: {}", seq(12, [half; 3]))
    }

    fn sphere(radius: f32) -> String {
        format!("sphere:\n            radius: {radius:?}")
    }

    /// A block-style sequence (scene files forbid flow sequences), indented under its key.
    fn seq(indent: usize, value: [f32; 3]) -> String {
        let pad = " ".repeat(indent + 2);
        format!(
            "\n{pad}- {:?}\n{pad}- {:?}\n{pad}- {:?}",
            value[0], value[1], value[2]
        )
    }

    /// One probe entity. `extra` holds field-level lines (`priority`, `blend`), already indented.
    fn probe(name: &str, at: [f32; 3], shape: &str, extra: &str) -> String {
        format!(
            "  - name: {name}\n    components:\n      Transform:\n        translation: {}\n      \
             ReflectionProbe:\n        shape:\n          {shape}\n{extra}",
            seq(8, at)
        )
    }

    fn check(entities: &str) -> Vec<Diagnostic> {
        let source = format!("format: 1\nentities:\n{entities}");
        let scene = ludion_scene::check(&source, &crate::builtin_registry())
            .unwrap_or_else(|errors| panic!("fixture scene invalid: {errors:?}"));
        scene_probe_diagnostics(&scene)
    }

    fn codes(diagnostics: &[Diagnostic]) -> Vec<&str> {
        diagnostics.iter().map(|d| d.code.as_str()).collect()
    }

    const PRIORITY_1: &str = "        priority: 1\n";
    const PRIORITY_3: &str = "        priority: 3\n";
    const NO_BLEND: &str = "        blend: 0.0\n";

    #[test]
    fn a_copy_pasted_pair_is_one_duplicate_error() {
        let diagnostics = check(&format!(
            "{}{}",
            probe("a", [0.0; 3], &cube(2.0), ""),
            probe("b", [0.0; 3], &cube(2.0), PRIORITY_3)
        ));
        assert_eq!(codes(&diagnostics), ["L1277"]);
        // `b` has the higher priority, so `a` is the lower-ranked one the path names.
        assert_eq!(
            diagnostics[0].path.as_deref(),
            Some("/entities/0/components/ReflectionProbe")
        );
        assert_eq!(diagnostics[0].related.len(), 1);
    }

    #[test]
    fn equal_size_overlap_at_equal_priority_is_a_warning_with_both_pointers() {
        let diagnostics = check(&format!(
            "{}{}",
            probe("a", [0.0; 3], &cube(2.0), ""),
            probe("b", [1.0, 0.0, 0.0], &cube(2.0), "")
        ));
        assert_eq!(codes(&diagnostics), ["L1275"]);
        assert_eq!(
            diagnostics[0].path.as_deref(),
            Some("/entities/1/components/ReflectionProbe")
        );
        assert_eq!(
            diagnostics[0].related[0].path.as_deref(),
            Some("/entities/0/components/ReflectionProbe")
        );
        assert!(diagnostics[0].message.contains("`a`") && diagnostics[0].message.contains("`b`"));
    }

    #[test]
    fn the_same_pair_at_different_priority_is_clean() {
        let diagnostics = check(&format!(
            "{}{}",
            probe("a", [0.0; 3], &cube(2.0), ""),
            probe("b", [1.0, 0.0, 0.0], &cube(2.0), PRIORITY_1)
        ));
        assert!(diagnostics.is_empty(), "{diagnostics:?}");
    }

    #[test]
    fn touching_boxes_without_a_blend_are_clean() {
        let diagnostics = check(&format!(
            "{}{}",
            probe("a", [0.0; 3], &cube(2.0), NO_BLEND),
            probe("b", [4.0, 0.0, 0.0], &cube(2.0), NO_BLEND)
        ));
        assert!(diagnostics.is_empty(), "{diagnostics:?}");
    }

    #[test]
    fn touching_boxes_with_a_blend_overlap_in_their_bands() {
        let diagnostics = check(&format!(
            "{}{}",
            probe("a", [0.0; 3], &cube(2.0), ""),
            probe("b", [4.0, 0.0, 0.0], &cube(2.0), "")
        ));
        assert_eq!(codes(&diagnostics), ["L1275"]);
    }

    #[test]
    fn a_small_probe_inside_a_big_one_ranks_first_and_is_clean() {
        let diagnostics = check(&format!(
            "{}{}",
            probe("big", [0.0; 3], &cube(10.0), ""),
            probe("small", [0.0; 3], &cube(1.0), "")
        ));
        assert!(diagnostics.is_empty(), "{diagnostics:?}");
    }

    #[test]
    fn a_big_probe_at_higher_priority_shadows_the_small_one() {
        let diagnostics = check(&format!(
            "{}{}",
            probe("big", [0.0; 3], &cube(10.0), PRIORITY_1),
            probe("small", [0.0; 3], &cube(1.0), "")
        ));
        assert_eq!(codes(&diagnostics), ["L1276"]);
        assert_eq!(
            diagnostics[0].path.as_deref(),
            Some("/entities/1/components/ReflectionProbe")
        );
    }

    #[test]
    fn a_small_sphere_inside_a_big_box_is_shadowed_at_equal_priority() {
        let diagnostics = check(&format!(
            "{}{}",
            probe("room", [0.0; 3], &cube(10.0), ""),
            probe("orb", [0.0; 3], &sphere(1.0), "")
        ));
        assert_eq!(codes(&diagnostics), ["L1276"]);
        assert_eq!(
            diagnostics[0].path.as_deref(),
            Some("/entities/1/components/ReflectionProbe")
        );
    }

    #[test]
    fn a_wide_blend_on_the_small_probe_uncovers_it() {
        let diagnostics = check(&format!(
            "{}{}",
            probe("big", [0.0; 3], &cube(10.0), PRIORITY_1),
            probe("small", [0.0; 3], &cube(1.0), "        blend: 10.0\n")
        ));
        assert!(diagnostics.is_empty(), "{diagnostics:?}");
    }

    #[test]
    fn a_wide_blend_on_the_container_still_shadows_the_small_probe() {
        let diagnostics = check(&format!(
            "{}{}",
            probe(
                "big",
                [0.0; 3],
                &cube(10.0),
                "        priority: 1\n        blend: 10.0\n"
            ),
            probe("small", [0.0; 3], &cube(1.0), "")
        ));
        assert_eq!(codes(&diagnostics), ["L1276"]);
    }

    #[test]
    fn a_rotated_parent_probe_duplicating_a_root_probe_is_found_by_world_pose() {
        // The pivot turns 90° about Y, so the child's local +X lands on world -Z with the same
        // world rotation as the root probe, which is authored there directly.
        let shape = format!("box:\n            extent: {}", seq(12, [1.0, 2.0, 3.0]));
        let entities = format!(
            "  - name: root\n    components:\n      Transform:\n        translation: {}\n        \
             rotation: {}\n      ReflectionProbe:\n        shape:\n          {shape}\n  \
             - name: pivot\n    components:\n      Transform:\n        rotation: {}\n  \
             - name: child\n    components:\n      Parent:\n        index: 1\n        \
             generation: 0\n      Transform:\n        translation: {}\n      \
             ReflectionProbe:\n        shape:\n          {shape}\n",
            seq(8, [0.0, 0.0, -3.0]),
            seq(8, [0.0, 90.0, 0.0]),
            seq(8, [0.0, 90.0, 0.0]),
            seq(8, [3.0, 0.0, 0.0]),
        );
        let diagnostics = check(&entities);
        assert_eq!(codes(&diagnostics), ["L1277"], "{diagnostics:?}");
    }

    #[test]
    fn over_the_cap_is_one_error_with_a_related_per_further_probe() {
        let many = |count: usize| {
            let entities: String = (0..count)
                .map(|i| {
                    probe(
                        &format!("p{i}"),
                        [i as f32 * 10.0, 0.0, 0.0],
                        &cube(1.0),
                        "",
                    )
                })
                .collect();
            check(&entities)
        };
        assert!(many(MAX_PROBES).is_empty());
        let diagnostics = many(MAX_PROBES + 1);
        assert_eq!(codes(&diagnostics), ["L1278"]);
        assert_eq!(
            diagnostics[0].path.as_deref(),
            Some("/entities/32/components/ReflectionProbe")
        );
        assert!(diagnostics[0].related.is_empty());
        assert_eq!(many(MAX_PROBES + 2)[0].related.len(), 1);
    }

    #[test]
    fn a_scene_without_probes_is_clean() {
        assert!(check("  - components: {}\n").is_empty());
    }
}
