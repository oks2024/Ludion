//! The IK pass over a posed assembly: look-at, then two-part chains (`Plan/animation-layers.md`
//! decision 37).
//!
//! It is a post-pass over the placements `part_placements_layered` composed (clip, crossfade,
//! layers), so it never touches the model sheet or the ragdoll switch. Every turn is the
//! pre-multiplication `T(C) · R(Q) · T(−C)` of part `i` and **every descendant** — the parts are
//! rigid, so a turn moves the subtree. The maths runs in `f64` on arrays, through the libm-only
//! `ludion_core::math::f64` and `ludion_scene::slerp`, and only the finished rotation becomes an
//! `f32` matrix.
//!
//! The goals arrive already resolved ([`assembly_ik_goals`]): a world target in the assembly frame
//! and a weight `k` in `(0, 1]`.

use ludion_components::{IkTargets, ik_target_weight_at};
use ludion_core::ids::EntityId;
use ludion_core::math::f64::{acos, asin, atan2, cos, sin, sqrt};
use ludion_core::math::glam::{Mat3, Mat4, Vec3, Vec4};
use ludion_core::time::GameTime;
use ludion_ecs::World;
use ludion_physics::QueryFilter;

use crate::assembly_mesh::{AssemblyData, AssemblyIkKind};

type V3 = [f64; 3];
type Quat = [f64; 4];

const IDENTITY: Quat = [0.0, 0.0, 0.0, 1.0];
const NEG_Z: V3 = [0.0, 0.0, -1.0];

/// Where one goal aims, in the assembly frame.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum IkAim {
    /// A point. A look-at faces it from the aim part's pivot; a chain's effector reaches it.
    Point(V3),
    /// A look-at direction in `Look`'s convention, relative to the look-at's `forward`.
    Direction { yaw_deg: f64, pitch_deg: f64 },
}

/// One resolved target: the `ik:` entry it drives (an index into [`AssemblyData::ik`]), where it
/// aims and its weight in `(0, 1]`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct IkGoal {
    pub entry: usize,
    pub aim: IkAim,
    pub weight: f64,
}

/// The goals of `id`'s `IkTargets` against `data`'s `ik:` entries at `now`: a target whose name
/// matches an entry and whose weight is above `0`. A direction on a chain is ignored. Empty — without
/// allocating — when there is no component or the assembly has no `ik:`.
pub(crate) fn assembly_ik_goals(
    world: &World,
    id: EntityId,
    data: &AssemblyData,
    model: Mat4,
    now: GameTime,
) -> Vec<IkGoal> {
    if data.ik.is_empty() {
        return Vec::new();
    }
    let Some(targets) = world.get::<IkTargets>(id).ok().flatten() else {
        return Vec::new();
    };
    let inverse = ludion_core::math::mat4_inverse(model);
    let to_assembly = |world_point: V3| {
        let p = inverse
            * Vec4::new(
                world_point[0] as f32,
                world_point[1] as f32,
                world_point[2] as f32,
                1.0,
            );
        [f64::from(p.x), f64::from(p.y), f64::from(p.z)]
    };
    let mut goals = Vec::new();
    for (entry, ik) in data.ik.iter().enumerate() {
        let Some(target) = targets.targets.get(&ik.name) else {
            continue;
        };
        let weight = f64::from(ik_target_weight_at(target, now));
        if weight.is_nan() || weight <= 0.0 {
            continue;
        }
        let aim = if let Some(point) = target.point {
            IkAim::Point(to_assembly(point.map(f64::from)))
        } else if let Some(look) = target.direction {
            if matches!(ik.kind, AssemblyIkKind::Chain { .. }) {
                continue;
            }
            IkAim::Direction {
                yaw_deg: f64::from(look.yaw_deg),
                pitch_deg: f64::from(look.pitch_deg),
            }
        } else {
            continue;
        };
        goals.push(IkGoal {
            entry,
            aim,
            weight: weight.min(1.0),
        });
    }
    goals
}

/// Poses `placed` (one `(model, normal)` pair per part, assembly frame) toward `goals`: every
/// look-at first, then every chain, each in name order. A goal whose aim does not fit its entry (a
/// direction on a chain) is skipped.
pub(crate) fn apply_assembly_ik(
    data: &AssemblyData,
    goals: &[IkGoal],
    placed: &mut [(Mat4, Mat3)],
) {
    let mut ordered: Vec<&IkGoal> = goals.iter().collect();
    ordered.sort_by_key(|goal| {
        (
            matches!(data.ik[goal.entry].kind, AssemblyIkKind::Chain { .. }),
            goal.entry,
        )
    });
    for goal in ordered {
        match &data.ik[goal.entry].kind {
            AssemblyIkKind::LookAt {
                weights,
                forward,
                limits_deg,
            } => look_at(data, placed, weights, *forward, *limits_deg, goal),
            AssemblyIkKind::Chain {
                parts,
                effector,
                axis,
                bend,
                limits,
                ..
            } => {
                if let IkAim::Point(target) = goal.aim {
                    chain(
                        data,
                        placed,
                        *parts,
                        *effector,
                        *axis,
                        *bend,
                        *limits,
                        target,
                        goal.weight,
                    );
                }
            }
        }
    }
}

/// Puts the effector of every `ground: true` chain without an active goal on the static ground
/// under it (`Plan/animation-layers.md` decisions 45–48), on the placements after targeted IK.
/// Returns the number of ray casts. Without physics, or a foot over no ground within reach, the
/// placements are untouched; a move under a thousandth of a voxel is no move, so flat ground at the
/// assembly's origin poses bit for bit as without `ground`.
pub(crate) fn apply_assembly_ground(
    world: &World,
    id: EntityId,
    data: &AssemblyData,
    model: Mat4,
    goals: &[IkGoal],
    placed: &mut [(Mat4, Mat3)],
) -> usize {
    let Some(ground) = data.ground else {
        return 0;
    };
    let Some(physics) = crate::ground_marker_clip::physics_of(world) else {
        return 0;
    };
    let cols = model.to_cols_array().map(f64::from);
    let voxel = length([cols[4], cols[5], cols[6]]);
    let band = 1e-3 * voxel;
    let reach = ground.max_adjust * voxel;
    let floor = cols[13];
    let filter = QueryFilter::new().only_fixed().exclude(id);

    struct Foot {
        parts: [usize; 2],
        effector: V3,
        axis: V3,
        bend: f64,
        limits: Option<[f64; 2]>,
        target: V3,
        delta: f64,
    }
    let mut casts = 0;
    let mut feet: Vec<Foot> = Vec::new();
    for (entry, ik) in data.ik.iter().enumerate() {
        let AssemblyIkKind::Chain {
            parts,
            effector,
            axis,
            bend,
            limits,
            ground: true,
        } = &ik.kind
        else {
            continue;
        };
        if goals.iter().any(|goal| goal.entry == entry) {
            continue;
        }
        let local = point_of(&placed[parts[1]].0, effector.map(|c| c as f32));
        let e = transform_point(&cols, local);
        let origin = [e[0] as f32, (floor + reach) as f32, e[2] as f32];
        casts += 1;
        let Ok(hits) = physics.ray_cast(origin, [0.0, -1.0, 0.0], (2.0 * reach) as f32, &filter)
        else {
            continue;
        };
        let Some(hit) = hits.first() else {
            continue;
        };
        if hit.distance <= 0.0 {
            continue;
        }
        let delta = (reach - f64::from(hit.distance)).clamp(-reach, reach);
        feet.push(Foot {
            parts: *parts,
            effector: *effector,
            axis: *axis,
            bend: *bend,
            limits: *limits,
            target: [e[0], e[1] + delta, e[2]],
            delta,
        });
    }
    let inverse = ludion_core::math::mat4_inverse(model);
    let drop = if ground.root_drop {
        feet.iter().map(|foot| foot.delta).fold(0.0, f64::min)
    } else {
        0.0
    };
    let dropped = drop < -band;
    if dropped {
        let down = inverse * Vec4::new(0.0, drop as f32, 0.0, 0.0);
        let shift = Mat4::from_translation(Vec3::new(down.x, down.y, down.z));
        for pair in placed.iter_mut() {
            pair.0 = shift * pair.0;
        }
    }
    for foot in feet {
        let residual = if dropped {
            foot.delta - drop
        } else {
            foot.delta
        };
        if residual.abs() < band {
            continue;
        }
        let t = inverse
            * Vec4::new(
                foot.target[0] as f32,
                foot.target[1] as f32,
                foot.target[2] as f32,
                1.0,
            );
        let target = [f64::from(t.x), f64::from(t.y), f64::from(t.z)];
        chain(
            data,
            placed,
            foot.parts,
            foot.effector,
            foot.axis,
            foot.bend,
            foot.limits,
            target,
            1.0,
        );
    }
    casts
}

/// `m · (p, 1)` in `f64` from a column-major matrix.
fn transform_point(c: &[f64; 16], p: V3) -> V3 {
    [
        c[0] * p[0] + c[4] * p[1] + c[8] * p[2] + c[12],
        c[1] * p[0] + c[5] * p[1] + c[9] * p[2] + c[13],
        c[2] * p[0] + c[6] * p[1] + c[10] * p[2] + c[14],
    ]
}

/// Turns the aim parts together so `forward` faces the goal, root-first, each by its share of the
/// mask's total weight. The yaw and pitch limits are measured in the frame where `forward` is `−Z`.
fn look_at(
    data: &AssemblyData,
    placed: &mut [(Mat4, Mat3)],
    weights: &[f32],
    forward: V3,
    limits_deg: [f64; 2],
    goal: &IkGoal,
) {
    let mut parts: Vec<(usize, f64)> = (0..placed.len())
        .filter_map(|i| {
            let w = f64::from(weights.get(i).copied().unwrap_or(0.0));
            (w > 0.0).then_some((i, w))
        })
        .collect();
    parts.sort_by_key(|&(i, _)| (depth(data, i), i));
    let Some(&(deepest, _)) = parts.last() else {
        return;
    };
    let total: f64 = parts.iter().map(|&(_, w)| w).sum();
    let forward = normalized(forward);
    let to_neg_z = arc(forward, NEG_Z);
    let posed = normalized(rotate_mat3(&placed[deepest].1, forward));
    let (yaw_f, pitch_f) = yaw_pitch(rotate_quat(to_neg_z, posed));
    let (yaw_t, pitch_t) = match goal.aim {
        IkAim::Direction { yaw_deg, pitch_deg } => (yaw_deg.to_radians(), pitch_deg.to_radians()),
        IkAim::Point(target) => {
            let from = point_of(&placed[deepest].0, data.parts[deepest].pivot);
            let t = sub(target, from);
            if length(t) < 1e-9 {
                return;
            }
            yaw_pitch(rotate_quat(to_neg_z, normalized(t)))
        }
    };
    let yaw_c = yaw_t.clamp(-limits_deg[0].to_radians(), limits_deg[0].to_radians());
    let pitch_c = pitch_t.clamp(-limits_deg[1].to_radians(), limits_deg[1].to_radians());
    let in_frame = quat_mul(
        look_quat(yaw_c, pitch_c),
        quat_conj(look_quat(yaw_f, pitch_f)),
    );
    let turn = quat_mul(quat_mul(quat_conj(to_neg_z), in_frame), to_neg_z);
    let whole = ludion_scene::slerp(IDENTITY, turn, goal.weight);
    for (part, w) in parts {
        let share = ludion_scene::slerp(IDENTITY, whole, w / total);
        let pivot = point_of(&placed[part].0, data.parts[part].pivot);
        turn_subtree(data, placed, part, pivot, share);
    }
}

/// Bends the lower part about the hinge so the effector is at the right distance from the upper
/// pivot, then turns the upper part (and so the lower) to point it at the target.
#[allow(clippy::too_many_arguments)]
fn chain(
    data: &AssemblyData,
    placed: &mut [(Mat4, Mat3)],
    parts: [usize; 2],
    effector: V3,
    axis: V3,
    bend: f64,
    limits: Option<[f64; 2]>,
    target: V3,
    k: f64,
) {
    let [a, b] = parts;
    let pa = point_of(&placed[a].0, data.parts[a].pivot);
    let pb = point_of(&placed[b].0, data.parts[b].pivot);
    let effector = effector.map(|c| c as f32);
    let e = point_of(&placed[b].0, effector);
    let u = sub(pa, pb);
    let v = sub(e, pb);
    let h = normalized(rotate_mat3(&placed[a].1, axis));
    let (u_par, v_par) = (dot(u, h), dot(v, h));
    let u_perp = sub(u, scale(h, u_par));
    let v_perp = sub(v, scale(h, v_par));
    let (u_len, v_len) = (length(u_perp), length(v_perp));
    if u_len * v_len >= 1e-9 {
        let from = scale(u_perp, -1.0);
        let psi = atan2(dot(h, cross(from, v_perp)), dot(from, v_perp));
        let d = length(sub(target, pa));
        let cos_star =
            (d * d - dot(u, u) - dot(v, v) + 2.0 * u_par * v_par) / (2.0 * u_len * v_len);
        let mut star = bend * acos(cos_star.clamp(-1.0, 1.0));
        if let Some([lo, hi]) = limits {
            star = star.clamp(lo.to_radians(), hi.to_radians());
        }
        let turn = k * (star - psi);
        turn_subtree(data, placed, b, pb, axis_angle(h, turn));
    }
    let e = point_of(&placed[b].0, effector);
    let want = sub(target, pa);
    let have = sub(e, pa);
    if length(want) < 1e-9 || length(have) < 1e-9 {
        return;
    }
    let swing = ludion_scene::slerp(IDENTITY, arc(normalized(have), normalized(want)), k);
    turn_subtree(data, placed, a, pa, swing);
}

/// Pre-multiplies `T(c) · R(q) · T(−c)` onto part `root` and every descendant of it.
fn turn_subtree(data: &AssemblyData, placed: &mut [(Mat4, Mat3)], root: usize, c: V3, q: Quat) {
    let r = quat_to_rows(q);
    let moved_c = [dot(r[0], c), dot(r[1], c), dot(r[2], c)];
    let t = sub(c, moved_c);
    let rotation = Mat3::from_cols(
        glam_col(r, 0, 0.0).truncate(),
        glam_col(r, 1, 0.0).truncate(),
        glam_col(r, 2, 0.0).truncate(),
    );
    let about = Mat4::from_cols(
        glam_col(r, 0, 0.0),
        glam_col(r, 1, 0.0),
        glam_col(r, 2, 0.0),
        Vec4::new(t[0] as f32, t[1] as f32, t[2] as f32, 1.0),
    );
    for (i, pair) in placed.iter_mut().enumerate() {
        if is_in_subtree(data, i, root) {
            pair.0 = about * pair.0;
            pair.1 = rotation * pair.1;
        }
    }
}

fn glam_col(rows: [V3; 3], column: usize, w: f32) -> Vec4 {
    Vec4::new(
        rows[0][column] as f32,
        rows[1][column] as f32,
        rows[2][column] as f32,
        w,
    )
}

/// Whether part `i` is `root` or one of its descendants (authored order is not topological, so it
/// walks `parent`; a cycle ends the walk).
fn is_in_subtree(data: &AssemblyData, i: usize, root: usize) -> bool {
    let mut at = Some(i);
    for _ in 0..=data.parts.len() {
        match at {
            Some(part) if part == root => return true,
            Some(part) => at = data.parts[part].parent,
            None => return false,
        }
    }
    false
}

fn depth(data: &AssemblyData, i: usize) -> usize {
    let mut at = data.parts[i].parent;
    let mut depth = 0;
    while let Some(part) = at {
        depth += 1;
        if depth > data.parts.len() {
            break;
        }
        at = data.parts[part].parent;
    }
    depth
}

// --- f64 vector and quaternion helpers -------------------------------------------------------------

fn sub(a: V3, b: V3) -> V3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn scale(a: V3, s: f64) -> V3 {
    [a[0] * s, a[1] * s, a[2] * s]
}

fn dot(a: V3, b: V3) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn cross(a: V3, b: V3) -> V3 {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

fn length(a: V3) -> f64 {
    sqrt(dot(a, a))
}

fn normalized(a: V3) -> V3 {
    let len = length(a);
    if len == 0.0 { a } else { scale(a, 1.0 / len) }
}

/// `m · (p, 1)` in `f64` from the `f32` matrix.
fn point_of(m: &Mat4, p: [f32; 3]) -> V3 {
    let c = m.to_cols_array().map(f64::from);
    let p = p.map(f64::from);
    [
        c[0] * p[0] + c[4] * p[1] + c[8] * p[2] + c[12],
        c[1] * p[0] + c[5] * p[1] + c[9] * p[2] + c[13],
        c[2] * p[0] + c[6] * p[1] + c[10] * p[2] + c[14],
    ]
}

fn rotate_mat3(m: &Mat3, v: V3) -> V3 {
    let c = m.to_cols_array().map(f64::from);
    [
        c[0] * v[0] + c[3] * v[1] + c[6] * v[2],
        c[1] * v[0] + c[4] * v[1] + c[7] * v[2],
        c[2] * v[0] + c[5] * v[1] + c[8] * v[2],
    ]
}

fn quat_mul(a: Quat, b: Quat) -> Quat {
    [
        a[3] * b[0] + a[0] * b[3] + a[1] * b[2] - a[2] * b[1],
        a[3] * b[1] - a[0] * b[2] + a[1] * b[3] + a[2] * b[0],
        a[3] * b[2] + a[0] * b[1] - a[1] * b[0] + a[2] * b[3],
        a[3] * b[3] - a[0] * b[0] - a[1] * b[1] - a[2] * b[2],
    ]
}

fn quat_conj(q: Quat) -> Quat {
    [-q[0], -q[1], -q[2], q[3]]
}

/// The rotation matrix of a unit quaternion, as rows.
fn quat_to_rows(q: Quat) -> [V3; 3] {
    let [x, y, z, w] = q;
    [
        [
            1.0 - 2.0 * (y * y + z * z),
            2.0 * (x * y - z * w),
            2.0 * (x * z + y * w),
        ],
        [
            2.0 * (x * y + z * w),
            1.0 - 2.0 * (x * x + z * z),
            2.0 * (y * z - x * w),
        ],
        [
            2.0 * (x * z - y * w),
            2.0 * (y * z + x * w),
            1.0 - 2.0 * (x * x + y * y),
        ],
    ]
}

fn rotate_quat(q: Quat, v: V3) -> V3 {
    let r = quat_to_rows(q);
    [dot(r[0], v), dot(r[1], v), dot(r[2], v)]
}

fn axis_angle(axis: V3, angle: f64) -> Quat {
    let (s, c) = (sin(angle * 0.5), cos(angle * 0.5));
    [axis[0] * s, axis[1] * s, axis[2] * s, c]
}

/// The shortest rotation taking unit `a` to unit `b`; a half-turn about any perpendicular when
/// they are opposite.
fn arc(a: V3, b: V3) -> Quat {
    let d = dot(a, b);
    if d > 1.0 - 1e-12 {
        return IDENTITY;
    }
    if d < -1.0 + 1e-12 {
        let probe = if a[0].abs() < 0.9 {
            [1.0, 0.0, 0.0]
        } else {
            [0.0, 1.0, 0.0]
        };
        return axis_angle(normalized(cross(a, probe)), std::f64::consts::PI);
    }
    let c = cross(a, b);
    let q = [c[0], c[1], c[2], 1.0 + d];
    let len = sqrt(q[0] * q[0] + q[1] * q[1] + q[2] * q[2] + q[3] * q[3]);
    q.map(|component| component / len)
}

/// `R_y(yaw) · R_x(−pitch)`: `Look`'s convention — positive yaw turns left, positive pitch looks
/// down — taking `−Z` to the look direction.
fn look_quat(yaw: f64, pitch: f64) -> Quat {
    quat_mul(
        axis_angle([0.0, 1.0, 0.0], yaw),
        axis_angle([1.0, 0.0, 0.0], -pitch),
    )
}

/// The yaw and pitch of a unit direction, in radians: the inverse of [`look_quat`] on `−Z`.
fn yaw_pitch(v: V3) -> (f64, f64) {
    (atan2(-v[0], -v[2]), asin((-v[1]).clamp(-1.0, 1.0)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::assembly_mesh::{AssemblyIkData, AssemblyMaskData, AssemblyPartData};
    use ludion_renderer::MeshData;
    use std::sync::Arc;

    fn mesh() -> Arc<MeshData> {
        Arc::new(MeshData {
            positions: Vec::new(),
            normals: Vec::new(),
            colors: Vec::new(),
            materials: Vec::new(),
            uvs: Vec::new(),
            tangents: Vec::new(),
            weights: Vec::new(),
            bone_indices: Vec::new(),
            skin_joints: Vec::new(),
            textures: None,
            authored_material: false,
            base_color_mean: [1.0; 4],
            rim: None,
            double_sided: false,
            indices: Vec::new(),
            hash: 0,
            lods: Vec::new(),
        })
    }

    fn part(name: &str, offset: [f32; 3], parent: Option<usize>) -> AssemblyPartData {
        AssemblyPartData {
            name: name.to_string(),
            mesh: mesh(),
            size: [1, 1, 1],
            offset,
            pivot: [0.0; 3],
            parent,
        }
    }

    fn rest(data: &AssemblyData) -> Vec<(Mat4, Mat3)> {
        data.parts
            .iter()
            .map(|p| (Mat4::from_translation(p.offset.into()), Mat3::IDENTITY))
            .collect()
    }

    /// A limb hanging down −Y: the upper part's pivot is the origin, the lower's `(0, −2, 0)`, and
    /// the effector `(0, −2, 0)` in the lower part's frame, so it rests at `(0, −4, 0)`.
    fn limb(bend: f64, limits: Option<[f64; 2]>) -> AssemblyData {
        AssemblyData {
            size: [1, 1, 1],
            parts: vec![
                part("upper", [0.0; 3], None),
                part("lower", [0.0, -2.0, 0.0], Some(0)),
                part("hand", [0.0, -4.0, 0.0], Some(1)),
            ],
            clips: Vec::new(),
            joints: Vec::new(),
            masks: Vec::new(),
            blends: Vec::new(),
            ik: vec![AssemblyIkData {
                name: "reach".to_string(),
                kind: AssemblyIkKind::Chain {
                    parts: [0, 1],
                    effector: [0.0, -2.0, 0.0],
                    axis: [1.0, 0.0, 0.0],
                    bend,
                    limits,
                    ground: false,
                },
            }],
            ground: None,
        }
    }

    fn effector(data: &AssemblyData, placed: &[(Mat4, Mat3)]) -> V3 {
        let AssemblyIkKind::Chain {
            parts, effector, ..
        } = &data.ik[0].kind
        else {
            panic!("a chain");
        };
        point_of(&placed[parts[1]].0, effector.map(|c| c as f32))
    }

    fn goal(target: V3, weight: f64) -> IkGoal {
        IkGoal {
            entry: 0,
            aim: IkAim::Point(target),
            weight,
        }
    }

    fn near(a: V3, b: V3) {
        assert!(length(sub(a, b)) < 1e-4, "{a:?} vs {b:?}");
    }

    #[test]
    fn a_reachable_target_puts_the_effector_on_it() {
        let data = limb(1.0, None);
        for target in [[0.0, -2.0, 2.0], [1.0, -3.0, 0.5], [0.0, -1.0, -1.5]] {
            let mut placed = rest(&data);
            apply_assembly_ik(&data, &[goal(target, 1.0)], &mut placed);
            near(effector(&data, &placed), target);
        }
    }

    #[test]
    fn descendants_follow_their_parent() {
        let data = limb(1.0, None);
        let mut placed = rest(&data);
        apply_assembly_ik(&data, &[goal([0.0, -2.0, 2.0], 1.0)], &mut placed);
        // The hand is rigid on the lower part: it stays 2 below the lower pivot along the limb.
        let lower = point_of(&placed[1].0, [0.0; 3]);
        let hand = point_of(&placed[2].0, [0.0; 3]);
        assert!((length(sub(hand, lower)) - 2.0).abs() < 1e-4);
        assert!((length(lower) - 2.0).abs() < 1e-4);
    }

    #[test]
    fn an_unreachable_target_gives_a_straight_limb_aimed_at_it() {
        let data = limb(1.0, None);
        let mut placed = rest(&data);
        apply_assembly_ik(&data, &[goal([0.0, -10.0, 10.0], 1.0)], &mut placed);
        let dir = normalized([0.0, -1.0, 1.0]);
        near(effector(&data, &placed), scale(dir, 4.0));
        near(point_of(&placed[1].0, [0.0; 3]), scale(dir, 2.0));
    }

    #[test]
    fn the_bend_sign_picks_the_side() {
        let target = [0.0, -2.0, 2.0];
        let side = |bend| {
            let data = limb(bend, None);
            let mut placed = rest(&data);
            apply_assembly_ik(&data, &[goal(target, 1.0)], &mut placed);
            near(effector(&data, &placed), target);
            point_of(&placed[1].0, [0.0; 3])[0..3].to_vec()
        };
        let (plus, minus) = (side(1.0), side(-1.0));
        // The elbow sits on opposite sides of the line from the shoulder to the target.
        let line = normalized(target);
        let off = |p: &[f64]| dot([p[0], p[1], p[2]], cross(line, [1.0, 0.0, 0.0]));
        assert!(off(&plus) * off(&minus) < 0.0, "{plus:?} {minus:?}");
    }

    #[test]
    fn limits_clamp_the_flex() {
        // The target asks for a 90 degree flex; the limit allows 30.
        let data = limb(1.0, Some([0.0, 30.0]));
        let mut placed = rest(&data);
        apply_assembly_ik(&data, &[goal([0.0, -2.0, 2.0], 1.0)], &mut placed);
        let upper = point_of(&placed[0].0, [0.0; 3]);
        let lower = point_of(&placed[1].0, [0.0; 3]);
        let hand = point_of(&placed[2].0, [0.0; 3]);
        let a = normalized(sub(lower, upper));
        let b = normalized(sub(hand, lower));
        let flex = acos(dot(a, b));
        assert!((flex - 30f64.to_radians()).abs() < 1e-4, "{flex}");
    }

    #[test]
    fn a_half_weight_lands_between_rest_and_solved() {
        let data = limb(1.0, None);
        let target = [0.0, -2.0, 2.0];
        let mut solved = rest(&data);
        apply_assembly_ik(&data, &[goal(target, 1.0)], &mut solved);
        let mut half = rest(&data);
        apply_assembly_ik(&data, &[goal(target, 0.5)], &mut half);
        let rest_pose = rest(&data);
        let rest_e = effector(&data, &rest_pose);
        let half_e = effector(&data, &half);
        assert!(length(sub(half_e, rest_e)) > 1e-3);
        assert!(length(sub(half_e, target)) > 1e-3);
        // Weight 0 is the rest pose to the bit.
        let mut none = rest(&data);
        apply_assembly_ik(&data, &[], &mut none);
        assert_eq!(none, rest_pose);
    }

    /// A neck and a head hanging under a body: pivots at `(0, 2, 0)` and `(0, 3, 0)`, facing `−Z`.
    fn head(weights: Vec<f32>, limits_deg: [f64; 2]) -> AssemblyData {
        let mut parts = vec![
            part("body", [0.0; 3], None),
            part("neck", [0.0; 3], Some(0)),
            part("head", [0.0; 3], Some(1)),
        ];
        parts[1].pivot = [0.0, 2.0, 0.0];
        parts[2].pivot = [0.0, 3.0, 0.0];
        AssemblyData {
            size: [1, 1, 1],
            parts,
            clips: Vec::new(),
            joints: Vec::new(),
            masks: vec![AssemblyMaskData {
                name: "aim".to_string(),
                weights: weights.clone(),
            }],
            blends: Vec::new(),
            ik: vec![AssemblyIkData {
                name: "look".to_string(),
                kind: AssemblyIkKind::LookAt {
                    weights,
                    forward: [0.0, 0.0, -1.0],
                    limits_deg,
                },
            }],
            ground: None,
        }
    }

    fn facing(placed: &[(Mat4, Mat3)], part: usize) -> V3 {
        rotate_mat3(&placed[part].1, NEG_Z)
    }

    fn look(yaw_deg: f64, pitch_deg: f64) -> IkGoal {
        IkGoal {
            entry: 0,
            aim: IkAim::Direction { yaw_deg, pitch_deg },
            weight: 1.0,
        }
    }

    #[test]
    fn a_look_reaches_an_in_limit_direction_exactly() {
        let data = head(vec![0.0, 0.0, 1.0], [70.0, 40.0]);
        let mut placed = rest(&data);
        apply_assembly_ik(&data, &[look(30.0, 20.0)], &mut placed);
        let want = rotate_quat(look_quat(30f64.to_radians(), 20f64.to_radians()), NEG_Z);
        near(facing(&placed, 2), want);
        // Only the aim part turned.
        near(facing(&placed, 0), NEG_Z);
    }

    #[test]
    fn a_look_stops_at_its_yaw_limit() {
        let data = head(vec![0.0, 0.0, 1.0], [70.0, 40.0]);
        let mut placed = rest(&data);
        apply_assembly_ik(&data, &[look(120.0, 0.0)], &mut placed);
        let want = rotate_quat(look_quat(70f64.to_radians(), 0.0), NEG_Z);
        near(facing(&placed, 2), want);
    }

    #[test]
    fn a_point_look_faces_the_point() {
        let data = head(vec![0.0, 0.0, 1.0], [180.0, 180.0]);
        let mut placed = rest(&data);
        let goal = IkGoal {
            entry: 0,
            aim: IkAim::Point([-3.0, 3.0, 0.0]),
            weight: 1.0,
        };
        apply_assembly_ik(&data, &[goal], &mut placed);
        near(facing(&placed, 2), [-1.0, 0.0, 0.0]);
    }

    #[test]
    fn two_aim_parts_split_the_turn_by_weight() {
        let data = head(vec![0.0, 1.0, 3.0], [70.0, 40.0]);
        let mut placed = rest(&data);
        apply_assembly_ik(&data, &[look(40.0, 0.0)], &mut placed);
        // The neck takes a quarter, the head, on top of it, the rest.
        near(
            facing(&placed, 1),
            rotate_quat(look_quat(10f64.to_radians(), 0.0), NEG_Z),
        );
        near(
            facing(&placed, 2),
            rotate_quat(look_quat(40f64.to_radians(), 0.0), NEG_Z),
        );
    }
}
