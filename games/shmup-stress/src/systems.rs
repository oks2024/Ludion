//! The arena's six systems, in registration order among the engine's own `spatial_rebuild`,
//! `play_patterns` and `advance_volleys`: emit, move, cancel, the brute-force hit test, and the
//! backdrop scroll.
//!
//! Four of them are the entity-per-bullet shape (`Emitter` lays `Bullet`s down, `move_bullets`
//! integrates and reaps them). Two — `cancel_bullets` and `player_hit_test` — see BOTH shapes,
//! because a scene is either volleys or entities and the same system has to price whichever it is
//! given (`Plan/projectile-patterns.md` Wave 2).

use ludion::prelude::*;

use crate::*;

/// The full turn, in radians — the ring's angular span. A constant, not a transcendental, so it does
/// not go through [`ludion::math`].
const TAU: f32 = std::f32::consts::TAU;

/// The [`Arena`] singleton, if the scene authored one. Every system below early-outs without it, so
/// a world with no arena is inert rather than wrong.
fn arena(world: &World) -> Option<Arena> {
    world.single_get::<Arena>().copied()
}

/// The [`BulletArt`] singleton, if the scene authored one **and** it names an atlas. An empty
/// `atlas` is the same answer as no singleton at all — the flat-quad archetype — so a scene cannot
/// half-configure the draw and leave every bullet pointing at a document that does not exist.
fn bullet_art(world: &World) -> Option<BulletArt> {
    world
        .single_get::<BulletArt>()
        .filter(|art| !art.atlas.is_empty())
        .cloned()
}

/// Lays down each [`Emitter`]'s volley: on a frame where `(frame + phase) % rate == 0`, `count`
/// bullets in an even ring around the emitter, the ring's start angle advanced by `spin` radians per
/// volley. No RNG — the field is a closed-form function of the frame index, which is what makes the
/// census assertable.
///
/// Order is pinned twice over: the emitters are collected in world row order *before* anything is
/// spawned (a spawn mid-iteration would both fight the query borrow and make the id sequence depend
/// on the iterator's internals), and each ring is then laid down in ascending index. Trigonometry
/// goes through [`ludion::math`] — the sim-side rule, and here also the reason two machines
/// agree on the same `WorldHash`.
pub fn emit_bullets(world: &mut World, ctx: &SystemCtx) {
    let Some(arena) = arena(world) else {
        return;
    };
    // Read once per frame rather than per bullet: the atlas path and the region prefix are owned
    // strings, and only the one region name a bullet shows is built per spawn.
    let art = bullet_art(world);
    let frame = ctx.frame.as_u64();
    let vertical = arena.plane.vertical_axis();
    let emitters: Vec<(Emitter, [f32; 2])> = world
        .query_ids::<(&Emitter, &Transform)>()
        .map(|(_, (emitter, transform))| {
            (
                *emitter,
                [transform.translation[0], transform.translation[vertical]],
            )
        })
        .collect();

    for (index, (emitter, origin)) in emitters.into_iter().enumerate() {
        if emitter.count == 0 || emitter.rate == 0 {
            continue;
        }
        let ticks = frame + u64::from(emitter.phase);
        let rate = u64::from(emitter.rate);
        if !ticks.is_multiple_of(rate) {
            continue;
        }
        // The volley ordinal, not the frame: `spin` is per volley, so the twist is independent of
        // how often the emitter fires.
        let ordinal = ticks / rate;
        let volley = ordinal as f32;
        let step = TAU / emitter.count as f32;
        let color = BULLET_COLORS[index % BULLET_COLORS.len()];
        for i in 0..emitter.count {
            let angle = emitter.spin * volley + step * i as f32;
            let velocity = [
                emitter.speed * ludion::math::cos(angle),
                emitter.speed * ludion::math::sin(angle),
            ];
            let sprite = bullet_sprite(art.as_ref(), color, ordinal, emitter.count, i);
            spawn_bullet(world, &arena, origin, velocity, emitter.life, sprite);
        }
    }
}

/// The quad one bullet is born with. Without [`BulletArt`] it is the flat tinted square the arena
/// has always drawn; with it, the same square taken from one region of one atlas — the extent
/// derived from the region rather than authored, which is the path a real game pays for (a region
/// lookup by name, per sprite, per frame, at extract).
///
/// Both forms are camera-facing and unlit: a bullet quad must read as a square under whichever
/// framing a scene authors, and three thousand of them want the no-light-lookup branch
/// (`Plan/sprite-engine.md` decision 2).
fn bullet_sprite(
    art: Option<&BulletArt>,
    color: [f32; 4],
    volley: u64,
    count: u32,
    ring_index: u32,
) -> Sprite {
    let Some(art) = art else {
        return Sprite {
            size: Some(BULLET_SIZE),
            color,
            billboard: true,
            ..Default::default()
        };
    };
    // The spawn ordinal, so which region a bullet shows is a closed form of the frame index and the
    // ring position alone — the property the census rests on, applied to the pixels.
    let ordinal = volley * u64::from(count) + u64::from(ring_index);
    let frames = u64::from(art.frames.max(1));
    let blend = if art.additive_every > 0 && ring_index.is_multiple_of(art.additive_every) {
        SpriteBlend::Additive
    } else {
        SpriteBlend::Alpha
    };
    Sprite {
        atlas: Some(art.atlas.clone()),
        frame: Some(format!("{}{:02}", art.frame_prefix, ordinal % frames)),
        color,
        billboard: true,
        blend,
        ..Default::default()
    }
}

/// Builds one bullet: the quad `sprite` describes, placed on the arena's plane with its own velocity
/// and countdown, plus — only when the arena asked for it — a dynamic circle [`Collider`], which is
/// the single difference between the `steady-drawn` and `steady-indexed` scenes.
fn spawn_bullet(
    world: &mut World,
    arena: &Arena,
    from: [f32; 2],
    velocity: [f32; 2],
    life: u32,
    sprite: Sprite,
) {
    let bullet = world.spawn();
    let mut translation = [0.0; 3];
    translation[0] = from[0];
    translation[arena.plane.vertical_axis()] = from[1];
    translation[arena.plane.off_axis()] = BULLET_DEPTH;
    let _ = world.insert(
        bullet,
        Transform {
            translation,
            ..Transform::default()
        },
    );
    let _ = world.insert(bullet, sprite);
    let _ = world.insert(bullet, Bullet { velocity, life });
    if arena.bullets_collide {
        let _ = world.insert(
            bullet,
            Collider {
                shape: Shape::Circle {
                    radius: BULLET_RADIUS,
                },
                body: Body::new_kinematic(),
                layer: None,
                sensor: false,
            },
        );
    }
}

/// Integrates every bullet and reaps the spent ones — the churn term the whole arena exists to
/// measure. A bullet dies two ways: its `life` reaches zero, or it leaves the [`Arena`]'s square
/// bounds. (A well-dimensioned scene never takes the second path — see the scene headers — so the
/// live census stays a closed form in the emitters alone.)
pub fn move_bullets(world: &mut World, ctx: &SystemCtx) {
    let Some(arena) = arena(world) else {
        return;
    };
    match arena.plane {
        ArenaPlane::Ground => integrate::<2>(world, ctx.dt, arena.half_extent),
        ArenaPlane::Upright => integrate::<1>(world, ctx.dt, arena.half_extent),
    }
}

/// The integration pass, with the plane's vertical axis as a **const** parameter `V` rather than a
/// runtime index: each plane gets its own loop with constant array offsets, so the ground path costs
/// what it cost before the upright plane existed and two scenes' `move_bullets` lines stay
/// comparable.
///
/// Sequential by construction: the despawns are collected during the pass and applied after it, in
/// the same ascending row order, so the entity free-list order — and with it the next frame's ids —
/// is a function of the sim alone.
fn integrate<const V: usize>(world: &mut World, dt: f32, half: f32) {
    let mut spent: Vec<EntityId> = Vec::new();
    for (bullet, (state, transform)) in world.query_ids::<(&mut Bullet, &mut Transform)>() {
        transform.translation[0] += state.velocity[0] * dt;
        transform.translation[V] += state.velocity[1] * dt;
        state.life = state.life.saturating_sub(1);
        if state.life == 0
            || transform.translation[0].abs() > half
            || transform.translation[V].abs() > half
        {
            spent.push(bullet);
        }
    }
    let _ = world.despawn_many(&spent);
}

/// The bullet-cancel worst case: on every [`Arena::cancel_every`]-th frame the whole field is
/// despawned at once, and the sources refill it over the frames that follow. Frame 0 is exempt (the
/// field is empty there anyway, and a cancel on the boot frame would only measure the empty case).
/// Inert when `cancel_every` is `0`, which is what makes it a per-scene switch rather than a mode.
///
/// It wipes **both shapes**, and the pair of numbers is the point: an entity scene destroys one
/// entity per bullet (~3,000 in one frame), a volley scene destroys one entity per volley (60 for
/// the same 3,000 bullets). Cancel is game code in both — the engine ships no cancel
/// (`Plan/projectile-patterns.md` decision 6).
pub fn cancel_bullets(world: &mut World, ctx: &SystemCtx) {
    let Some(arena) = arena(world) else {
        return;
    };
    let frame = ctx.frame.as_u64();
    if arena.cancel_every == 0 || frame == 0 || !frame.is_multiple_of(u64::from(arena.cancel_every))
    {
        return;
    }
    // Collected in two passes rather than one chained iterator so each borrow of the world ends
    // before the next begins; the ids stay in ascending row order within each shape, so the entity
    // free-list order after the wipe is a function of the sim alone.
    let mut spent: Vec<EntityId> = world.query_ids::<(&Bullet,)>().map(|(id, _)| id).collect();
    spent.extend(world.query_ids::<(&Volley,)>().map(|(id, _)| id));
    let _ = world.despawn_many(&spent);
}

/// Counts the bullets overlapping the player's hitbox this frame into [`HitCount`] — **both
/// shapes**, tallied into the one counter.
///
/// **Deliberately not a spatial-index probe.** The genre's hypothesis is that bullets must never
/// enter the index — rebuilding it at ~3,000 entries costs more than the one scan a single tiny
/// hitbox needs — so the entity half is a plain squared-distance test over `(&Bullet, &Transform)`,
/// and its cost in the profile report is exactly the number that hypothesis is judged on.
///
/// The volley half is [`volley_hits`], the engine's pure overlap query: it evaluates the very
/// closed form that draws the bullet, so what is hit and what is seen cannot drift
/// (`Plan/projectile-patterns.md` decision 6). Its cost against the entity scan's, at the same
/// census, is the other half of the migration's A-versus-B.
pub fn player_hit_test(world: &mut World, ctx: &SystemCtx) {
    let Some(arena) = arena(world) else {
        return;
    };
    let Some(player) = world.single::<Player>() else {
        return;
    };
    let Some(hitbox) = world
        .get::<Player>(player)
        .ok()
        .flatten()
        .map(|p| p.hitbox_radius)
    else {
        return;
    };
    let vertical = arena.plane.vertical_axis();
    let Some(at) = world
        .get::<Transform>(player)
        .ok()
        .flatten()
        .map(|t| [t.translation[0], t.translation[vertical]])
    else {
        return;
    };

    let entity_hits = match arena.plane {
        ArenaPlane::Ground => scan_bullets::<2>(world, at, hitbox),
        ArenaPlane::Upright => scan_bullets::<1>(world, at, hitbox),
    };
    // Gameplay time, the clock every volley evaluator takes: a hit test on sim time would score a
    // field the frozen frame is not drawing.
    let volley_hit_count = scan_volleys(world, at, hitbox, ctx.game_time);
    let hits = entity_hits.saturating_add(volley_hit_count);
    for (count,) in world.query::<(&mut HitCount,)>() {
        count.hits = count.hits.saturating_add(hits);
    }
}

/// The entity half's scan, split on the plane's vertical axis for the reason [`integrate`] is.
fn scan_bullets<const V: usize>(world: &mut World, at: [f32; 2], hitbox: f32) -> u64 {
    let reach = hitbox + BULLET_RADIUS;
    let reach_sq = reach * reach;
    world
        .query::<(&Bullet, &Transform)>()
        .filter(|(_, transform)| {
            let dx = transform.translation[0] - at[0];
            let dv = transform.translation[V] - at[1];
            dx * dx + dv * dv <= reach_sq
        })
        .count() as u64
}

/// The volley half: [`volley_hits`] over every live [`Volley`], at the game's own
/// [`Plane`] and at gameplay time — the engine query, evaluated on the same closed form extract
/// draws.
///
/// It sets no kill bit. This arena counts overlaps to prove the test runs and costs what it costs;
/// killing a bullet would move the census a scene header promises, and the census is the
/// instrument.
fn scan_volleys(world: &mut World, at: [f32; 2], hitbox: f32, game_now: SimTime) -> u64 {
    let plane = Plane::of(world);
    let ids: Vec<EntityId> = world.entities().collect();
    let Some(store) = world.resource::<DocStore<PatternDoc>>() else {
        return 0;
    };
    let mut hits = 0u64;
    for id in ids {
        let Some(volley) = world.get::<Volley>(id).ok().flatten() else {
            continue;
        };
        let Some(origin) = world.get::<Transform>(id).ok().flatten() else {
            continue;
        };
        let Some(kind) = store
            .get(&volley.pattern)
            .and_then(|doc| doc.bullets.get(&volley.bullet))
        else {
            continue;
        };
        hits = hits.saturating_add(
            volley_hits(volley, origin, kind, game_now, plane, at, hitbox).count() as u64,
        );
    }
    hits
}

/// Slides every [`Backdrop`] grid along the arena plane's vertical axis and snaps it back after its
/// own `period` — the scrolling-stage idiom before `uv_scroll` exists: one `Tilemap` entity moved as
/// a whole, its cells expanded into the same sprite batch the bullets ride in.
///
/// The offset is computed from the frame index, never accumulated, so the grid's position at tick N
/// is the same number however the run reached it.
pub fn scroll_backdrop(world: &mut World, ctx: &SystemCtx) {
    let Some(arena) = arena(world) else {
        return;
    };
    let vertical = arena.plane.vertical_axis();
    let frame = ctx.frame.as_u64();
    let dt = ctx.dt;
    for (backdrop, transform) in world.query::<(&Backdrop, &mut Transform)>() {
        if backdrop.period == 0 {
            continue;
        }
        let elapsed = (frame % u64::from(backdrop.period)) as f32 * dt;
        transform.translation[vertical] = backdrop.origin - backdrop.speed * elapsed;
    }
}
