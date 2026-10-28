//! The scripted route every test shares: hold Right down the main lane, crouch under the tunnel,
//! jump the pit, ride the conveyor and stop on the end pad. Frames are sim ticks.

use ludion_sim::Key;

/// How many ticks the route runs.
pub const FRAMES: u64 = 330;

/// `(frame, key, down)`: the key transitions of the route, in order.
pub const ROUTE: [(u64, Key, bool); 6] = [
    (2, Key::Right, true),
    (104, Key::C, true),
    (166, Key::Space, true),
    (172, Key::C, false),
    (190, Key::Space, false),
    (290, Key::Right, false),
];
