#![forbid(unsafe_code)]
#![warn(missing_docs)]

//! `sokoban` — a grid-pushing puzzle built on Ludion.
//!
//! The level is one built-in [`Tilemap`]: walls (`#`), floor (`.`), and targets (`x`) are glyphs,
//! so the picture and the rules read the same grid. The player and the crates are separate sprite
//! entities carrying a [`GridPos`]; movement is discrete (one cell per key press), pushing follows
//! the classic Sokoban rule (one crate, never into a wall or another crate), and the game is won
//! when every crate rests on a target glyph.

use ludion::prelude::*;

/// Crate color while off a target (matches the scene's initial crate color).
pub const CRATE_COLOR: [f32; 4] = [0.78, 0.55, 0.25, 1.0];
/// Crate color once it rests on a target glyph.
pub const CRATE_ON_TARGET_COLOR: [f32; 4] = [1.0, 0.85, 0.3, 1.0];
/// The scene light's base intensity; the win celebration pulses above it. Keep in step with the
/// scene's `PointLight.intensity` so the first rendered frame matches what the sim then holds.
pub const LIGHT_BASE_INTENSITY: f32 = 0.7;

/// A discrete cell position on the level's tilemap grid, plus the cell the entity started in
/// (so `R` can reset the level without reloading the scene).
#[derive(
    serde::Serialize,
    serde::Deserialize,
    schemars::JsonSchema,
    Clone,
    Copy,
    Debug,
    PartialEq,
    Eq,
    Default,
)]
pub struct GridPos {
    /// Current column (increases along +X).
    #[serde(default)]
    pub col: i32,
    /// Current row (increases downward, matching tilemap reading order).
    #[serde(default)]
    pub row: i32,
    /// The column this entity starts (and resets) in.
    #[serde(default)]
    pub start_col: i32,
    /// The row this entity starts (and resets) in.
    #[serde(default)]
    pub start_row: i32,
}

/// Marker: the one entity the arrow keys move.
#[derive(
    serde::Serialize,
    serde::Deserialize,
    schemars::JsonSchema,
    Clone,
    Copy,
    Debug,
    PartialEq,
    Default,
)]
pub struct Player {}

/// Marker: a crate the player can push, one cell at a time.
#[derive(
    serde::Serialize,
    serde::Deserialize,
    schemars::JsonSchema,
    Clone,
    Copy,
    Debug,
    PartialEq,
    Default,
)]
pub struct Pushable {}

/// The game's mutable state, riding on the status-text entity so it snapshots and hashes with
/// everything else.
#[derive(
    serde::Serialize,
    serde::Deserialize,
    schemars::JsonSchema,
    Clone,
    Copy,
    Debug,
    PartialEq,
    Default,
)]
pub struct GameState {
    /// True while every crate rests on a target glyph.
    #[serde(default)]
    pub won: bool,
    /// Successful moves so far (blocked presses don't count).
    #[serde(default)]
    pub moves: u32,
}

/// True when `(col, row)` is a cell the player or a crate may occupy: floor (`.`) or target (`x`).
fn walkable(map: &Tilemap, col: i32, row: i32) -> bool {
    if col < 0 || row < 0 {
        return false;
    }
    matches!(
        map.glyph_at(col as usize, row as usize),
        Some('.') | Some('x')
    )
}

/// The entity carrying the level [`Tilemap`], if the scene has one.
fn level_entity(world: &mut World) -> Option<EntityId> {
    world.single::<Tilemap>()
}

/// The entity carrying the [`GameState`], if the scene has one.
fn state_entity(world: &mut World) -> Option<EntityId> {
    world.single::<GameState>()
}

/// Puts every [`GridPos`] entity back in its start cell and clears the [`GameState`].
fn reset_level(world: &mut World) {
    let gridded: Vec<EntityId> = world
        .entities()
        .filter(|&e| world.has::<GridPos>(e))
        .collect();
    for e in gridded {
        let pos = world.get_mut::<GridPos>(e).unwrap().unwrap();
        pos.col = pos.start_col;
        pos.row = pos.start_row;
    }
    if let Some(e) = state_entity(world) {
        let state = world.get_mut::<GameState>(e).unwrap().unwrap();
        state.won = false;
        state.moves = 0;
    }
}

/// One discrete Sokoban move per key-press edge: step onto floor, or push a single crate one cell
/// — never into a wall or a second crate. `R` resets the level; after a win only `R` is heard.
pub fn player_move(world: &mut World, _ctx: &SystemCtx) {
    let (dir, reset) = {
        let input = world.resource::<InputState>().unwrap();
        let dir = if input.was_pressed(Key::Up) {
            Some((0i32, -1i32))
        } else if input.was_pressed(Key::Down) {
            Some((0, 1))
        } else if input.was_pressed(Key::Left) {
            Some((-1, 0))
        } else if input.was_pressed(Key::Right) {
            Some((1, 0))
        } else {
            None
        };
        (dir, input.was_pressed(Key::R))
    };
    if reset {
        reset_level(world);
        return;
    }
    let Some((dc, dr)) = dir else { return };

    let state = state_entity(world);
    if let Some(e) = state
        && world.get::<GameState>(e).unwrap().unwrap().won
    {
        return;
    }

    let Some(level) = level_entity(world) else {
        return;
    };
    let map = world.get::<Tilemap>(level).unwrap().unwrap().clone();

    let Some(player) = world.single::<Player>() else {
        return;
    };
    let pos = *world.get::<GridPos>(player).unwrap().unwrap();
    let (nc, nr) = (pos.col + dc, pos.row + dr);
    if !walkable(&map, nc, nr) {
        return;
    }

    let crates: Vec<EntityId> = world
        .entities()
        .filter(|&e| world.has::<Pushable>(e))
        .collect();
    let at = |world: &mut World, col: i32, row: i32| -> Option<EntityId> {
        crates.iter().copied().find(|&e| {
            let p = world.get::<GridPos>(e).unwrap().unwrap();
            p.col == col && p.row == row
        })
    };
    if let Some(pushed) = at(world, nc, nr) {
        let (bc, br) = (nc + dc, nr + dr);
        if !walkable(&map, bc, br) || at(world, bc, br).is_some() {
            return;
        }
        let crate_pos = world.get_mut::<GridPos>(pushed).unwrap().unwrap();
        crate_pos.col = bc;
        crate_pos.row = br;
    }

    let player_pos = world.get_mut::<GridPos>(player).unwrap().unwrap();
    player_pos.col = nc;
    player_pos.row = nr;
    if let Some(e) = state {
        world.get_mut::<GameState>(e).unwrap().unwrap().moves += 1;
    }
}

/// Recomputes the win condition and its presentation: crates on targets turn gold, the status
/// [`Text`] shows the move count (or the solve), and [`GameState::won`] tracks the crates.
pub fn update_status(world: &mut World, _ctx: &SystemCtx) {
    let Some(level) = level_entity(world) else {
        return;
    };
    let map = world.get::<Tilemap>(level).unwrap().unwrap().clone();

    let crates: Vec<EntityId> = world
        .entities()
        .filter(|&e| world.has::<Pushable>(e))
        .collect();
    let mut all_on_target = !crates.is_empty();
    for &e in &crates {
        let pos = *world.get::<GridPos>(e).unwrap().unwrap();
        let on_target = pos.col >= 0
            && pos.row >= 0
            && map.glyph_at(pos.col as usize, pos.row as usize) == Some('x');
        all_on_target &= on_target;
        if let Ok(Some(sprite)) = world.get_mut::<Sprite>(e) {
            sprite.color = if on_target {
                CRATE_ON_TARGET_COLOR
            } else {
                CRATE_COLOR
            };
        }
    }

    let Some(state_e) = state_entity(world) else {
        return;
    };
    let moves = {
        let state = world.get_mut::<GameState>(state_e).unwrap().unwrap();
        state.won = all_on_target;
        state.moves
    };
    let message = if all_on_target {
        format!("Solved in {moves} moves!")
    } else {
        format!("Moves: {moves}")
    };
    if let Ok(Some(text)) = world.get_mut::<Text>(state_e)
        && text.text != message
    {
        text.text = message;
    }
}

/// Keeps every gridded entity's [`Transform`] snapped to its cell center (x/y only — z stays the
/// scene's layering choice), so game logic never re-derives the grid-to-world math.
pub fn sync_grid_transforms(world: &mut World, _ctx: &SystemCtx) {
    let Some(level) = level_entity(world) else {
        return;
    };
    let map = world.get::<Tilemap>(level).unwrap().unwrap().clone();
    let origin = world.get::<Transform>(level).unwrap().unwrap().translation;

    let gridded: Vec<EntityId> = world
        .entities()
        .filter(|&e| world.has::<GridPos>(e) && world.has::<Transform>(e))
        .collect();
    for e in gridded {
        let pos = *world.get::<GridPos>(e).unwrap().unwrap();
        let center = map.cell_to_world(origin, pos.col, pos.row);
        let transform = world.get_mut::<Transform>(e).unwrap().unwrap();
        transform.translation[0] = center[0];
        transform.translation[1] = center[1];
    }
}

/// Pulses the scene light while the level is solved; holds it at the base intensity otherwise.
/// The pulse reads time only through `ctx.time`, so it is deterministic and hashed.
pub fn celebrate_win(world: &mut World, ctx: &SystemCtx) {
    let won = state_entity(world)
        .map(|e| world.get::<GameState>(e).unwrap().unwrap().won)
        .unwrap_or(false);
    let t = ctx.time.seconds() as f32;
    for (light,) in world.query::<(&mut PointLight,)>() {
        light.intensity = if won {
            LIGHT_BASE_INTENSITY + 1.5 * (t * 2.0).fract()
        } else {
            LIGHT_BASE_INTENSITY
        };
    }
}

/// Registers this game's components on top of a registry that already holds the built-ins.
///
/// # Errors
/// An `L0005` diagnostic if a name collides with a built-in or another game component.
pub fn register(registry: &mut Registry) -> Result<(), Diagnostic> {
    registry.register::<GridPos>("GridPos")?;
    registry.register::<Player>("Player")?;
    registry.register::<Pushable>("Pushable")?;
    registry.register::<GameState>("GameState")
}

/// This game's full component registry: the built-ins plus this game's components.
///
/// # Panics
/// If [`register`] fails — a build-time bug (a name colliding with a built-in), not a runtime
/// state.
pub fn registry() -> Registry {
    let mut registry = builtin_registry();
    register(&mut registry).expect("components register cleanly on top of the built-ins");
    registry
}

/// Describes the engine that hosts this game. Registration order is execution order:
/// move first, then judge the board, then snap visuals, then light the celebration.
pub fn game() -> EngineBuilder {
    Engine::builder()
        .components(register)
        .system("player_move", player_move)
        .system("update_status", update_status)
        .system("sync_grid_transforms", sync_grid_transforms)
        .system("celebrate_win", celebrate_win)
        .default_scene("scenes/main.yaml")
}
