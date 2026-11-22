//! Rain at extract: the lowest-id `Rain` becomes the frame's [`RainData`]. The static half,
//! [`scene_rain_diagnostics`], reports the duplicate and the streak budget from scene text.

use ludion_components::{Rain, RainOccluder, rain_problems};
use ludion_core::codes::code;
use ludion_core::diagnostics::Diagnostic;
use ludion_core::ids::EntityId;
use ludion_core::time::GameTime;
use ludion_ecs::World;
use ludion_renderer::{RAIN_TIME_WRAP_S, RainData, rain_budget, rain_per_tile, rain_tiles};
use ludion_scene::Scene;

/// The streaks per tile `rain` asks for, and what fits the budget: `(authored, drawn, clamped)`.
fn streaks_per_tile(rain: &Rain) -> (u32, u32, bool) {
    let (n_xz, n_y) = rain_tiles(rain.radius, rain.height, rain.cell);
    let authored = rain_per_tile(rain.density, rain.intensity, rain.cell);
    let (drawn, clamped) = rain_budget(authored, n_xz * n_xz * n_y);
    (authored, drawn, clamped)
}

/// Whether a placed mesh writes the rain occlusion map: the nearest `RainOccluder` on its entity or a
/// `Parent` ancestor decides, and with none the mesh occludes when it casts a shadow. A world with no
/// `RainOccluder` answers `cast_shadows` after one O(1) count.
pub(crate) fn rain_occluder(world: &World, id: EntityId, cast_shadows: bool) -> bool {
    if world.component_count::<RainOccluder>() == 0 {
        return cast_shadows;
    }
    crate::hierarchy::nearest_component::<RainOccluder>(world, id)
        .map_or(cast_shadows, |occluder| occluder.occludes)
}

/// `L2383` when a mesh routed as blended carries a `RainOccluder` with `occludes: true` (its own or an
/// ancestor's): blended meshes are drawn outside the opaque list and never write the map. Keyed per
/// entity, so an assembly's parts and each frame report once.
pub(crate) fn report_blended_occluder(
    world: &World,
    id: EntityId,
    degraded: &mut Vec<(String, Diagnostic)>,
) {
    if world.component_count::<RainOccluder>() == 0 {
        return;
    }
    if !crate::hierarchy::nearest_component::<RainOccluder>(world, id).is_some_and(|o| o.occludes) {
        return;
    }
    degraded.push((
        format!(
            "{}/rain-occluder-blended",
            crate::diagnostics::offender_key(world, id)
        ),
        Diagnostic::new(
            code::RAIN_OCCLUDER_BLENDED,
            format!("{id} carries a `RainOccluder` but its mesh is blended, which never writes the rain map"),
        )
        .with_help(
            "blended meshes never write the rain map; put the occluder on an opaque mesh (`ludion docs weather`)",
        ),
    ));
}

/// The frame's rain, `None` when no entity carries one, its intensity is 0, or a field is out of range
/// (`L2380`, which load-time validation already reports). The lowest entity id wins; each further
/// `Rain` is `L2381`. `time_s` is game time, so a held `GameClock` holds the rain.
pub(crate) fn extract_rain(
    world: &World,
    ids: &[EntityId],
    now: GameTime,
    degraded: &mut Vec<(String, Diagnostic)>,
) -> Option<RainData> {
    if world.component_count::<Rain>() == 0 {
        return None;
    }
    let mut chosen: Option<Rain> = None;
    for &id in ids {
        let Some(rain) = world.get::<Rain>(id).ok().flatten() else {
            continue;
        };
        if chosen.is_some() {
            degraded.push((
                format!(
                    "{}/rain-duplicate",
                    crate::diagnostics::offender_key(world, id)
                ),
                Diagnostic::new(
                    code::RAIN_DUPLICATE,
                    format!("{id} carries a second `Rain`; the lowest entity id is used"),
                )
                .with_help("keep one `Rain` entity; `ludion explain L2381`"),
            ));
            continue;
        }
        if let Some((path, what)) = rain_problems(rain).into_iter().next() {
            degraded.push((
                format!(
                    "{}/rain-invalid",
                    crate::diagnostics::offender_key(world, id)
                ),
                Diagnostic::new(code::RAIN_INVALID, format!("`Rain` {what}"))
                    .with_path(path)
                    .with_help("see `ludion docs weather`; `ludion explain L2380`"),
            ));
            continue;
        }
        // A silent rain still claims the singleton, so a second one is reported.
        chosen = Some(*rain);
        let (authored, drawn, clamped) = streaks_per_tile(rain);
        if clamped {
            degraded.push((
                format!(
                    "{}/rain-budget",
                    crate::diagnostics::offender_key(world, id)
                ),
                budget_diagnostic(authored, drawn),
            ));
        }
    }
    let rain = chosen.filter(|r| r.intensity > 0.0)?;
    let (_, per_tile, _) = streaks_per_tile(&rain);
    Some(RainData {
        radius: rain.radius,
        height: rain.height,
        cell: rain.cell,
        fall_speed: rain.fall_speed,
        wind: rain.wind,
        gust: rain.gust,
        drop_width: rain.drop_width,
        streak: rain.streak,
        color: rain.color,
        lit: rain.lit,
        medium: rain.medium,
        occlusion: rain.occlusion,
        wetness: rain.wetness,
        ripples: rain.ripples,
        splash_rate: rain.intensity * rain.splashes,
        shadowed: rain.shadowed,
        backlight: rain.backlight,
        seed: rain.seed,
        time_s: (now.seconds() % RAIN_TIME_WRAP_S) as f32,
        per_tile,
    })
}

fn budget_diagnostic(authored: u32, drawn: u32) -> Diagnostic {
    Diagnostic::new(
        code::RAIN_STREAK_BUDGET,
        format!(
            "`Rain` asks for {authored} streaks per tile; {drawn} fit the {} streak budget",
            ludion_renderer::MAX_RAIN_STREAKS
        ),
    )
    .with_help("lower `density`, `radius` or the `height` span; `ludion explain L2382`")
}

/// `L2381` at the second and later `Rain` entities and `L2382` at a `Rain` over the streak budget,
/// from scene text. Invalid field values are `L2380`, raised by component validation.
pub fn scene_rain_diagnostics(scene: &Scene) -> Vec<Diagnostic> {
    let mut out = Vec::new();
    let mut first: Option<usize> = None;
    for (index, entity) in scene.entities.iter().enumerate() {
        let Some((_, value)) = entity.components.iter().find(|(name, _)| name == "Rain") else {
            continue;
        };
        let at = format!("/entities/{index}/components/Rain");
        if let Some(first) = first {
            out.push(
                Diagnostic::new(
                    code::RAIN_DUPLICATE,
                    format!("a second `Rain`; the one at /entities/{first} is used"),
                )
                .with_path(at)
                .with_help("keep one `Rain` entity; `ludion explain L2381`"),
            );
            continue;
        }
        first = Some(index);
        let Ok(rain) = serde_json::from_value::<Rain>(value.clone()) else {
            continue;
        };
        if !rain_problems(&rain).is_empty() || rain.intensity <= 0.0 {
            continue;
        }
        let (authored, drawn, clamped) = streaks_per_tile(&rain);
        if clamped {
            out.push(budget_diagnostic(authored, drawn).with_path(format!("{at}/density")));
        }
    }
    out
}
