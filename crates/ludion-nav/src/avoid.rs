//! Reciprocal velocity-obstacle avoidance in the xz plane (ORCA, van den Berg et al. 2011,
//! "Reciprocal n-body collision avoidance").
//!
//! One half-plane of permitted velocities per neighbour, then a 2D linear program picks the permitted
//! velocity closest to the preferred one. When the half-planes are jointly infeasible, a second program
//! minimises the worst violation instead. Pure and allocation-free given the caller's `lines` scratch, so a
//! parallel phase can call it per agent.
//!
//! ```
//! use ludion_nav::{AvoidAgent, avoid_velocity};
//!
//! let agent = AvoidAgent {
//!     position: [0.0, 0.0], velocity: [0.0, 0.0], preferred: [3.0, 4.0],
//!     radius: 0.5, max_speed: 2.5, horizon_s: 1.5, dt: 0.02,
//! };
//! let mut lines = Vec::new();
//! let v = avoid_velocity(&agent, &[], &mut lines);
//! assert!((v[0] - 1.5).abs() < 1e-5 && (v[1] - 2.0).abs() < 1e-5); // preferred, clamped to max_speed
//! ```

use ludion_core::math::sqrt;

/// The sideways share of the preferred velocity added when there are neighbours: it breaks the exact
/// head-on symmetry, and each agent steers to its own right, so a pair part to opposite sides.
const RIGHT_BIAS: f32 = 0.01;

/// Below this a length, determinant or denominator counts as zero.
const EPSILON: f32 = 1e-6;

/// A neighbour as the avoiding agent sees it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AvoidNeighbor {
    /// World (x, z), metres.
    pub position: [f32; 2],
    /// Its current velocity, metres per second.
    pub velocity: [f32; 2],
    /// Body radius, metres.
    pub radius: f32,
    /// The share of the avoidance this agent takes: 0.5 for a reciprocal pair, 1 when the neighbour will not
    /// move aside, 0 for none (the neighbour is skipped).
    pub responsibility: f32,
}

/// The agent being steered.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AvoidAgent {
    /// World (x, z), metres.
    pub position: [f32; 2],
    /// Its current velocity, metres per second.
    pub velocity: [f32; 2],
    /// The velocity it would take with nobody around.
    pub preferred: [f32; 2],
    /// Body radius, metres.
    pub radius: f32,
    /// The longest velocity it may take.
    pub max_speed: f32,
    /// How far ahead, in seconds, a predicted collision is avoided.
    pub horizon_s: f32,
    /// The step length; the cut-off of the already-colliding case.
    pub dt: f32,
}

/// The half-plane `{ v : dot(normal, v - point) >= 0 }` of permitted velocities.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct OrcaLine {
    /// A velocity on the boundary.
    pub point: [f32; 2],
    /// Unit length, pointing into the permitted side.
    pub normal: [f32; 2],
}

#[derive(Clone, Copy)]
enum Objective {
    /// The permitted velocity nearest this one.
    Closest([f32; 2]),
    /// The permitted velocity furthest along this unit direction.
    Direction([f32; 2]),
}

fn dot(a: [f32; 2], b: [f32; 2]) -> f32 {
    a[0] * b[0] + a[1] * b[1]
}

fn cross(a: [f32; 2], b: [f32; 2]) -> f32 {
    a[0] * b[1] - a[1] * b[0]
}

fn sub(a: [f32; 2], b: [f32; 2]) -> [f32; 2] {
    [a[0] - b[0], a[1] - b[1]]
}

fn scale(a: [f32; 2], s: f32) -> [f32; 2] {
    [a[0] * s, a[1] * s]
}

fn clamp_length(a: [f32; 2], max: f32) -> [f32; 2] {
    let len_sq = dot(a, a);
    if len_sq > max * max {
        scale(a, max / sqrt(len_sq))
    } else {
        a
    }
}

/// The ORCA velocity for `agent` among `neighbors`: the permitted velocity closest to `agent.preferred`, at
/// most `agent.max_speed` long. With neighbours present the preferred velocity is nudged 1% to the agent's
/// right, so exactly head-on pairs part instead of deadlocking. `lines` is scratch, cleared on entry.
pub fn avoid_velocity(
    agent: &AvoidAgent,
    neighbors: &[AvoidNeighbor],
    lines: &mut Vec<OrcaLine>,
) -> [f32; 2] {
    lines.clear();
    for n in neighbors {
        if n.responsibility <= 0.0 {
            continue;
        }
        lines.push(neighbor_line(agent, n));
    }
    let count = lines.len();
    let preferred = if count > 0 {
        [
            agent.preferred[0] + agent.preferred[1] * RIGHT_BIAS,
            agent.preferred[1] - agent.preferred[0] * RIGHT_BIAS,
        ]
    } else {
        agent.preferred
    };
    let mut result = [0.0; 2];
    let failed = linear_program_2(
        lines,
        agent.max_speed,
        Objective::Closest(preferred),
        &mut result,
    );
    if failed < count {
        linear_program_3(lines, failed, agent.max_speed, &mut result);
    }
    result
}

fn neighbor_line(agent: &AvoidAgent, n: &AvoidNeighbor) -> OrcaLine {
    let p = sub(n.position, agent.position);
    let v = sub(agent.velocity, n.velocity);
    let radius = agent.radius + n.radius;
    let dist_sq = dot(p, p);
    let radius_sq = radius * radius;

    // `u` is the shortest change to `v` that reaches the boundary of the velocity obstacle; `normal` points
    // out of it.
    let (u, normal);
    if dist_sq > radius_sq {
        let inv_tau = 1.0 / agent.horizon_s;
        let w = sub(v, scale(p, inv_tau));
        let w_len_sq = dot(w, w);
        let along = dot(w, p);
        if along < 0.0 && along * along > radius_sq * w_len_sq {
            // Closest to the cut-off circle.
            let w_len = sqrt(w_len_sq);
            normal = scale(w, 1.0 / w_len);
            u = scale(normal, radius * inv_tau - w_len);
        } else {
            let leg = sqrt(dist_sq - radius_sq);
            let (tangent, out) = if cross(p, w) > 0.0 {
                let t = scale(
                    [p[0] * leg - p[1] * radius, p[0] * radius + p[1] * leg],
                    1.0 / dist_sq,
                );
                (t, [-t[1], t[0]])
            } else {
                let t = scale(
                    [p[0] * leg + p[1] * radius, -p[0] * radius + p[1] * leg],
                    1.0 / dist_sq,
                );
                (t, [t[1], -t[0]])
            };
            normal = out;
            u = sub(scale(tangent, dot(v, tangent)), v);
        }
    } else {
        // Already overlapping: resolve within one step.
        let inv_dt = 1.0 / agent.dt;
        let w = sub(v, scale(p, inv_dt));
        let w_len = sqrt(dot(w, w));
        normal = if w_len > EPSILON {
            scale(w, 1.0 / w_len)
        } else {
            [1.0, 0.0]
        };
        u = scale(normal, radius * inv_dt - w_len);
    }
    OrcaLine {
        point: [
            agent.velocity[0] + u[0] * n.responsibility,
            agent.velocity[1] + u[1] * n.responsibility,
        ],
        normal,
    }
}

/// Sequential 2D program: returns the index of the first line that cannot be satisfied, or `lines.len()`.
fn linear_program_2(
    lines: &[OrcaLine],
    max_speed: f32,
    objective: Objective,
    result: &mut [f32; 2],
) -> usize {
    *result = match objective {
        Objective::Closest(preferred) => clamp_length(preferred, max_speed),
        Objective::Direction(dir) => scale(dir, max_speed),
    };
    for i in 0..lines.len() {
        if dot(lines[i].normal, sub(*result, lines[i].point)) < 0.0 {
            let before = *result;
            if !linear_program_1(lines, i, max_speed, objective, result) {
                *result = before;
                return i;
            }
        }
    }
    lines.len()
}

/// The best point on line `i` that satisfies lines `0..i` inside the speed disc; false when there is none.
fn linear_program_1(
    lines: &[OrcaLine],
    i: usize,
    max_speed: f32,
    objective: Objective,
    result: &mut [f32; 2],
) -> bool {
    let line = lines[i];
    let dir = [-line.normal[1], line.normal[0]];
    let along = dot(line.point, dir);
    let disc = along * along + max_speed * max_speed - dot(line.point, line.point);
    if disc < 0.0 {
        return false;
    }
    let root = sqrt(disc);
    let (mut t_min, mut t_max) = (-along - root, -along + root);
    for other in &lines[..i] {
        let denom = dot(other.normal, dir);
        let num = dot(other.normal, sub(line.point, other.point));
        if denom.abs() <= EPSILON {
            if num < 0.0 {
                return false;
            }
            continue;
        }
        let bound = -num / denom;
        if denom > 0.0 {
            t_min = t_min.max(bound);
        } else {
            t_max = t_max.min(bound);
        }
        if t_min > t_max {
            return false;
        }
    }
    let t = match objective {
        Objective::Closest(preferred) => dot(sub(preferred, line.point), dir).clamp(t_min, t_max),
        Objective::Direction(wanted) => {
            if dot(wanted, dir) > 0.0 {
                t_max
            } else {
                t_min
            }
        }
    };
    *result = [line.point[0] + dir[0] * t, line.point[1] + dir[1] * t];
    true
}

/// The lines are jointly infeasible from `begin` on: minimise the worst violation. Projected lines are pushed
/// after the originals and truncated again.
fn linear_program_3(
    lines: &mut Vec<OrcaLine>,
    begin: usize,
    max_speed: f32,
    result: &mut [f32; 2],
) {
    let count = lines.len();
    let mut worst = 0.0f32;
    for i in begin..count {
        let line = lines[i];
        if -dot(line.normal, sub(*result, line.point)) <= worst {
            continue;
        }
        lines.truncate(count);
        let dir = [-line.normal[1], line.normal[0]];
        for k in 0..i {
            let other = lines[k];
            let denom = dot(other.normal, dir);
            let point = if denom.abs() <= EPSILON {
                if dot(line.normal, other.normal) > 0.0 {
                    continue;
                }
                scale(
                    [
                        line.point[0] + other.point[0],
                        line.point[1] + other.point[1],
                    ],
                    0.5,
                )
            } else {
                let s = -dot(other.normal, sub(line.point, other.point)) / denom;
                [line.point[0] + dir[0] * s, line.point[1] + dir[1] * s]
            };
            let diff = sub(other.normal, line.normal);
            let len = sqrt(dot(diff, diff));
            let normal = if len > EPSILON {
                scale(diff, 1.0 / len)
            } else {
                other.normal
            };
            lines.push(OrcaLine { point, normal });
        }
        let mut candidate = *result;
        let failed = linear_program_2(
            &lines[count..],
            max_speed,
            Objective::Direction(line.normal),
            &mut candidate,
        );
        if failed >= lines.len() - count {
            *result = candidate;
        }
        lines.truncate(count);
        worst = (-dot(line.normal, sub(*result, line.point))).max(0.0);
    }
}
