---
description: The scaffolded starter read end to end: one component, one system, the wiring root, its proofs
---

# Worked mini-example: the starter, end to end

**Scope:** The smallest complete game, file by file, as `ludion new` writes it. What each file is for is project-anatomy; the APIs it uses are components and systems.

The starter is one component, one input-driven system, wired into `game()` — laid out across the
three `src/` files so the shape scales. Definitions live in modules; the wiring lives at the root.

`src/components.rs` — the data (and, at this scale, its tunable):

```rust
use ludion::prelude::*;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

pub const PLAYER_SPEED: f32 = 6.0;

#[derive(Serialize, Deserialize, JsonSchema, Clone, Copy, Debug, PartialEq)]
pub struct Player { #[serde(default)] pub x: f32 }
```

`src/systems.rs` — the behavior:

```rust
use ludion::prelude::*;
use crate::{PLAYER_SPEED, Player};

pub fn move_player(world: &mut World, ctx: &SystemCtx) {
    if !world.resource::<InputState>().unwrap().is_held(Key::Right) {
        return;
    }
    for (player, transform) in world.query::<(&mut Player, &mut Transform)>() {
        player.x += PLAYER_SPEED * ctx.dt;
        transform.translation[0] = player.x;
    }
}
```

`src/lib.rs` — the wiring root: re-export the modules, list the components, order the systems:

```rust
use ludion::prelude::*;

mod components;
mod systems;
pub use components::*;
pub use systems::*;

pub fn register(reg: &mut Registry) -> Result<(), Diagnostic> { reg.register::<Player>("Player") }

pub fn game() -> EngineBuilder {
    Engine::builder()
        .components(register)
        .system("move_player", move_player)
        .default_scene("scenes/main.yaml")
}
```

`scenes/main.yaml` puts a `Player` on a `Sprite`, adds a `Camera` and a `PointLight`, and the golden
proves it renders. The gameplay test proves the paddle slides 3.0 units in 30 ticks with the key held.

## Next

- `ludion docs project-anatomy` — what `ludion new` scaffolds, file by file.
- `ludion docs components` — declaring components, and the built-in visual set.
- `ludion docs systems` — the system contract and the determinism rules.
- `ludion docs input` — reading keyboard, mouse and gamepad input.
- `ludion docs verify-loop` — the `cargo test` verification loop.
