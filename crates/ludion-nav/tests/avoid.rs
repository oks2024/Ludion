//! ORCA unit tests: the pure avoidance rule, no ECS.

use ludion_nav::{AvoidAgent, AvoidNeighbor, avoid_velocity};

fn agent(position: [f32; 2], velocity: [f32; 2], preferred: [f32; 2]) -> AvoidAgent {
    AvoidAgent {
        position,
        velocity,
        preferred,
        radius: 0.5,
        max_speed: 1.0,
        horizon_s: 3.0,
        dt: 0.1,
    }
}

fn len(v: [f32; 2]) -> f32 {
    (v[0] * v[0] + v[1] * v[1]).sqrt()
}

#[test]
fn no_neighbors_returns_preferred_clamped() {
    let mut lines = Vec::new();
    let a = agent([0.0, 0.0], [0.0, 0.0], [3.0, 4.0]);
    let v = avoid_velocity(&a, &[], &mut lines);
    assert!(
        (v[0] - 0.6).abs() < 1e-5 && (v[1] - 0.8).abs() < 1e-5,
        "{v:?}"
    );
    let slow = agent([0.0, 0.0], [0.0, 0.0], [0.3, 0.0]);
    assert_eq!(avoid_velocity(&slow, &[], &mut lines), [0.3, 0.0]);
}

#[test]
fn head_on_pair_swaps_sides_without_overlap() {
    let mut lines = Vec::new();
    let dt = 0.1;
    let goals = [[5.0, 0.05], [-5.0, -0.05]];
    let mut pos = [[-5.0f32, 0.0], [5.0, 0.0]];
    let mut vel = [[0.0f32; 2]; 2];
    let mut min_dist = f32::MAX;
    for _ in 0..200 {
        let mut next = [[0.0f32; 2]; 2];
        for i in 0..2 {
            let j = 1 - i;
            let to_goal = [goals[i][0] - pos[i][0], goals[i][1] - pos[i][1]];
            let l = len(to_goal);
            let preferred = if l > 1e-4 {
                [to_goal[0] / l * l.min(1.0), to_goal[1] / l * l.min(1.0)]
            } else {
                [0.0; 2]
            };
            let a = agent(pos[i], vel[i], preferred);
            let n = AvoidNeighbor {
                position: pos[j],
                velocity: vel[j],
                radius: 0.5,
                responsibility: 0.5,
            };
            next[i] = avoid_velocity(&a, &[n], &mut lines);
        }
        vel = next;
        for i in 0..2 {
            pos[i] = [pos[i][0] + vel[i][0] * dt, pos[i][1] + vel[i][1] * dt];
        }
        min_dist = min_dist.min(len([pos[0][0] - pos[1][0], pos[0][1] - pos[1][1]]));
    }
    assert!(min_dist >= 1.0 - 1e-3, "min distance {min_dist}");
    assert!(
        pos[0][0] > 4.0 && pos[1][0] < -4.0,
        "did not swap sides: {pos:?}"
    );
}

#[test]
fn full_responsibility_passes_a_static_neighbor() {
    let mut lines = Vec::new();
    let dt = 0.1;
    let mut pos = [-5.0f32, 0.0];
    let mut vel = [0.0f32; 2];
    let obstacle = [0.0f32, 0.0];
    let mut min_dist = f32::MAX;
    for _ in 0..200 {
        let to_goal = [5.0 - pos[0], 0.05 - pos[1]];
        let l = len(to_goal).max(1e-4);
        let preferred = [to_goal[0] / l * l.min(1.0), to_goal[1] / l * l.min(1.0)];
        let a = agent(pos, vel, preferred);
        let n = AvoidNeighbor {
            position: obstacle,
            velocity: [0.0; 2],
            radius: 0.5,
            responsibility: 1.0,
        };
        vel = avoid_velocity(&a, &[n], &mut lines);
        pos = [pos[0] + vel[0] * dt, pos[1] + vel[1] * dt];
        min_dist = min_dist.min(len([pos[0] - obstacle[0], pos[1] - obstacle[1]]));
    }
    assert!(min_dist >= 1.0 - 1e-3, "min distance {min_dist}");
    assert!(pos[0] > 4.0, "did not pass: {pos:?}");
}

#[test]
fn zero_responsibility_is_ignored() {
    let mut lines = Vec::new();
    let a = agent([0.0, 0.0], [1.0, 0.0], [1.0, 0.0]);
    let n = AvoidNeighbor {
        position: [1.2, 0.0],
        velocity: [0.0; 2],
        radius: 0.5,
        responsibility: 0.0,
    };
    assert_eq!(avoid_velocity(&a, &[n], &mut lines), [1.0, 0.0]);
}

#[test]
fn deterministic_and_order_independent() {
    let mut lines = Vec::new();
    let a = agent([0.0, 0.0], [1.0, 0.0], [1.0, 0.0]);
    let ns = [
        AvoidNeighbor {
            position: [2.0, 0.4],
            velocity: [-1.0, 0.0],
            radius: 0.5,
            responsibility: 0.5,
        },
        AvoidNeighbor {
            position: [2.5, -0.6],
            velocity: [-0.5, 0.2],
            radius: 0.4,
            responsibility: 1.0,
        },
        AvoidNeighbor {
            position: [-1.0, 1.5],
            velocity: [0.0, -0.5],
            radius: 0.5,
            responsibility: 0.5,
        },
    ];
    let first = avoid_velocity(&a, &ns, &mut lines);
    assert_eq!(first, avoid_velocity(&a, &ns, &mut lines));
    let mut reversed = ns;
    reversed.reverse();
    let second = avoid_velocity(&a, &reversed, &mut lines);
    assert!(
        (first[0] - second[0]).abs() < 1e-5 && (first[1] - second[1]).abs() < 1e-5,
        "{first:?} {second:?}"
    );
}
