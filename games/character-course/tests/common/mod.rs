//! The scripted route every test shares: hold Right down the main lane, crouch under the tunnel,
//! jump the pit, ride the conveyor and cross the end pad into the lane section: the first pass of its
//! pit is the deliberate death, the respawn lands on the lane at the checkpoint, the second pass jumps
//! the pit, drops onto the belt and stops on the exit pad, then walks on into the chase section: round the
//! pillar (a short strafe out and back) and under the porch. Frames are sim ticks.

use ludion_sim::Key;

/// How many ticks the route runs.
pub const FRAMES: u64 = 960;

/// `(frame, key, down)`: the key transitions of the route, in order.
pub const ROUTE: [(u64, Key, bool); 16] = [
    (2, Key::Up, true),
    (104, Key::C, true),
    (166, Key::Space, true),
    (172, Key::C, false),
    (190, Key::Space, false),
    (334, Key::Up, false),
    (334, Key::Right, true),
    (514, Key::Space, true),
    (538, Key::Space, false),
    (655, Key::Right, false),
    (680, Key::Up, true),
    (744, Key::Right, true),
    (756, Key::Right, false),
    (785, Key::Left, true),
    (797, Key::Left, false),
    (880, Key::Up, false),
];
