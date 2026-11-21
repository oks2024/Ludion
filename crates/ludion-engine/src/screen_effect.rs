//! Screen effects at extract: each applied `ScreenEffect` becomes a [`ScreenEffectData`] with its
//! output-pixel rectangle, UI split index, resolved stages and game time. The static half,
//! [`scene_screen_effect_diagnostics`], reports the cap from scene text.

use ludion_components::{
    ScreenEffect, ScreenEffectTarget, ScreenMaskKind, screen_effect_target_problems,
};
use ludion_core::codes::code;
use ludion_core::diagnostics::Diagnostic;
use ludion_core::ids::EntityId;
use ludion_core::time::GameTime;
use ludion_ecs::World;
use ludion_renderer::{MAX_SCREEN_EFFECTS, RAIN_TIME_WRAP_S, ScreenEffectData, ScreenMask};
use ludion_scene::Scene;

/// The frame's screen effects, in draw order: the (at most [`MAX_SCREEN_EFFECTS`]) lowest
/// `(order, EntityId)` effects that change a pixel. An effect with every stage off is neither applied
/// nor counted; one with an unusable region is `L1761`; each effect past the cap is `L2392`.
/// `ui_orders` is `RenderScene.ui`'s sorted order column. `time_s` and `frame` are game time, so a
/// held `GameClock` holds the effect.
pub(crate) fn extract_screen_effects(
    world: &World,
    ids: &[EntityId],
    now: GameTime,
    viewport: (u32, u32),
    ui_orders: &[i32],
    degraded: &mut Vec<(String, Diagnostic)>,
) -> Vec<ScreenEffectData> {
    if world.component_count::<ScreenEffect>() == 0 {
        return Vec::new();
    }
    let mut candidates: Vec<(i32, EntityId, &ScreenEffect)> = Vec::new();
    for &id in ids {
        let Some(effect) = world.get::<ScreenEffect>(id).ok().flatten() else {
            continue;
        };
        if effect.stages().is_neutral() {
            continue;
        }
        if let Some((path, what)) = screen_effect_target_problems(&effect.target)
            .into_iter()
            .next()
        {
            degraded.push((
                format!(
                    "{}/screen-effect-target",
                    crate::diagnostics::offender_key(world, id)
                ),
                Diagnostic::new(
                    code::SCREEN_EFFECT_INVALID_TARGET,
                    format!("`ScreenEffect` region {what}"),
                )
                .with_path(path)
                .with_help("see `ludion explain L1761` and `ludion docs screen-effects`"),
            ));
            continue;
        }
        candidates.push((effect.order, id, effect));
    }
    candidates.sort_by_key(|(order, id, _)| (*order, *id));
    for (_, id, _) in candidates.iter().skip(MAX_SCREEN_EFFECTS) {
        degraded.push((
            format!(
                "{}/screen-effect-cap",
                crate::diagnostics::offender_key(world, *id)
            ),
            cap_diagnostic(format!("{id} is past the cap")),
        ));
    }
    let time_s = (now.seconds() % RAIN_TIME_WRAP_S) as f32;
    let frame = now.ticks_floor() as u32;
    candidates
        .into_iter()
        .take(MAX_SCREEN_EFFECTS)
        .map(|(order, _, effect)| {
            let s = effect.stages();
            ScreenEffectData {
                rect_px: rect_px(&effect.target, viewport),
                ui_start: ui_orders.partition_point(|&o| o < order) as u32,
                time_s,
                frame,
                seed: effect.seed,
                scanlines: s.scanlines,
                mask_strength: s.mask_strength,
                curvature: s.curvature,
                glow: s.glow,
                chroma_bleed: s.chroma_bleed,
                noise: s.noise,
                tracking: s.tracking,
                roll: s.roll,
                jitter: s.jitter,
                flicker: s.flicker,
                sweep: s.sweep,
                sweep_period: s.sweep_period,
                scanline_count: s.scanline_count,
                posterize: s.posterize,
                mask: match s.mask {
                    ScreenMaskKind::None => ScreenMask::None,
                    ScreenMaskKind::Aperture => ScreenMask::Aperture,
                    ScreenMaskKind::Slot => ScreenMask::Slot,
                    ScreenMaskKind::Shadow => ScreenMask::Shadow,
                },
                phosphor: s.phosphor,
                interlace: s.interlace,
            }
        })
        .collect()
}

/// The target's rectangle `[x, y, w, h]` in output pixels, clamped to the viewport (an empty clamp
/// is zero area).
fn rect_px(target: &ScreenEffectTarget, viewport: (u32, u32)) -> [f32; 4] {
    let (vw, vh) = (viewport.0 as f32, viewport.1 as f32);
    let [x, y, w, h] = match *target {
        ScreenEffectTarget::Fill => return [0.0, 0.0, vw, vh],
        ScreenEffectTarget::Region {
            anchor,
            offset,
            size,
        } => crate::ui::resolve_ui_rect(anchor, offset, size, viewport),
    };
    let (x0, y0) = (x.clamp(0.0, vw), y.clamp(0.0, vh));
    let (x1, y1) = ((x + w).clamp(0.0, vw), (y + h).clamp(0.0, vh));
    [x0, y0, (x1 - x0).max(0.0), (y1 - y0).max(0.0)]
}

fn cap_diagnostic(what: String) -> Diagnostic {
    Diagnostic::new(
        code::SCREEN_EFFECT_CAP,
        format!(
            "{what}: at most {MAX_SCREEN_EFFECTS} `ScreenEffect`s are applied, lowest `order` then lowest entity id first"
        ),
    )
    .with_help("remove an effect or leave its stages off; `ludion explain L2392` and `ludion docs screen-effects`")
}

/// `L2392` at each non-neutral `ScreenEffect` past the cap, from scene text, in the order extract
/// drops them (highest `order`, then highest entity position). An effect with every stage off or an
/// unusable region (`L1761`, raised by component validation) is not counted.
pub fn scene_screen_effect_diagnostics(scene: &Scene) -> Vec<Diagnostic> {
    let mut applied: Vec<(i32, usize)> = Vec::new();
    for (index, entity) in scene.entities.iter().enumerate() {
        let Some((_, value)) = entity
            .components
            .iter()
            .find(|(name, _)| name == "ScreenEffect")
        else {
            continue;
        };
        let Ok(effect) = serde_json::from_value::<ScreenEffect>(value.clone()) else {
            continue;
        };
        if effect.stages().is_neutral() || !screen_effect_target_problems(&effect.target).is_empty()
        {
            continue;
        }
        applied.push((effect.order, index));
    }
    applied.sort();
    applied
        .into_iter()
        .skip(MAX_SCREEN_EFFECTS)
        .map(|(_, index)| {
            cap_diagnostic(format!("the `ScreenEffect` at /entities/{index}"))
                .with_path(format!("/entities/{index}/components/ScreenEffect"))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use ludion_components::{Anchor, ScreenEffectPreset};
    use ludion_core::time::TickRate;

    const VIEWPORT: (u32, u32) = (1920, 1080);

    fn crt(order: i32) -> ScreenEffect {
        ScreenEffect {
            order,
            preset: ScreenEffectPreset::Crt,
            ..ScreenEffect::default()
        }
    }

    fn world_of(effects: Vec<ScreenEffect>) -> (World, Vec<EntityId>) {
        let mut world = World::new();
        let ids: Vec<EntityId> = effects
            .into_iter()
            .map(|fx| {
                let id = world.spawn();
                world.insert(id, fx).unwrap();
                id
            })
            .collect();
        (world, ids)
    }

    fn run(
        world: &World,
        ids: &[EntityId],
        ui_orders: &[i32],
    ) -> (Vec<ScreenEffectData>, Vec<(String, Diagnostic)>) {
        let mut degraded = Vec::new();
        let now = GameTime::from_ticks(90, TickRate::DEFAULT);
        let out = extract_screen_effects(world, ids, now, VIEWPORT, ui_orders, &mut degraded);
        (out, degraded)
    }

    #[test]
    fn no_component_yields_nothing() {
        let world = World::new();
        let (out, degraded) = run(&world, &[], &[]);
        assert!(out.is_empty() && degraded.is_empty());
    }

    #[test]
    fn fill_covers_the_viewport_and_carries_resolved_stages() {
        let (world, ids) = world_of(vec![crt(0)]);
        let (out, _) = run(&world, &ids, &[]);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].rect_px, [0.0, 0.0, 1920.0, 1080.0]);
        assert_eq!(out[0].scanlines, 0.35);
        assert_eq!(out[0].mask, ScreenMask::Aperture);
        assert_eq!(out[0].phosphor, [0.25, 1.0, 0.45]);
    }

    #[test]
    fn a_region_matches_resolve_ui_rect() {
        let target = ScreenEffectTarget::Region {
            anchor: Anchor::TopRight,
            offset: [-32.0, 32.0],
            size: [320.0, 320.0],
        };
        let (world, ids) = world_of(vec![ScreenEffect { target, ..crt(0) }]);
        let (out, _) = run(&world, &ids, &[]);
        let want =
            crate::ui::resolve_ui_rect(Anchor::TopRight, [-32.0, 32.0], [320.0, 320.0], VIEWPORT);
        assert_eq!(out[0].rect_px, want);
    }

    #[test]
    fn a_region_off_the_frame_is_clamped() {
        let target = ScreenEffectTarget::Region {
            anchor: Anchor::TopLeft,
            offset: [-100.0, 0.0],
            size: [200.0, 100.0],
        };
        let (world, ids) = world_of(vec![ScreenEffect { target, ..crt(0) }]);
        let (out, _) = run(&world, &ids, &[]);
        assert_eq!(out[0].rect_px, [0.0, 0.0, 100.0, 100.0]);
    }

    #[test]
    fn ui_start_counts_the_elements_under_the_effect() {
        let (world, ids) = world_of(vec![crt(10)]);
        let (out, _) = run(&world, &ids, &[0, 5, 5, 10, 20]);
        assert_eq!(out[0].ui_start, 3);
    }

    #[test]
    fn the_cap_keeps_the_lowest_orders_and_reports_once_per_offender() {
        let (world, ids) = world_of(vec![crt(4), crt(1), crt(3), crt(2), crt(0)]);
        let (out, degraded) = run(&world, &ids, &[]);
        assert_eq!(out.len(), 4);
        assert_eq!(out.iter().map(|d| d.ui_start).sum::<u32>(), 0);
        assert_eq!(degraded.len(), 1);
        assert_eq!(degraded[0].1.code, code::SCREEN_EFFECT_CAP);
        assert!(degraded[0].0.ends_with("/screen-effect-cap"));
        assert!(degraded[0].0.contains(&format!("e{}", ids[0].index())));
    }

    #[test]
    fn a_neutral_effect_is_dropped_silently_and_not_counted() {
        let mut effects = vec![ScreenEffect::default()];
        effects.extend((0..4).map(crt));
        let (world, ids) = world_of(effects);
        let (out, degraded) = run(&world, &ids, &[]);
        assert_eq!(out.len(), 4);
        assert!(degraded.is_empty());
    }

    #[test]
    fn an_unusable_region_is_l1761_and_not_applied() {
        let target = ScreenEffectTarget::Region {
            anchor: Anchor::Center,
            offset: [0.0, 0.0],
            size: [0.0, 10.0],
        };
        let (world, ids) = world_of(vec![ScreenEffect { target, ..crt(0) }]);
        let (out, degraded) = run(&world, &ids, &[]);
        assert!(out.is_empty());
        assert_eq!(degraded[0].1.code, code::SCREEN_EFFECT_INVALID_TARGET);
    }

    #[test]
    fn a_held_game_clock_gives_the_same_time_and_frame() {
        let (world, ids) = world_of(vec![crt(0)]);
        let held = GameTime::from_ticks(90, TickRate::DEFAULT);
        let mut degraded = Vec::new();
        let a = extract_screen_effects(&world, &ids, held, VIEWPORT, &[], &mut degraded);
        let b = extract_screen_effects(&world, &ids, held, VIEWPORT, &[], &mut degraded);
        assert_eq!(a, b);
        assert_eq!(a[0].frame, 90);
        assert!((a[0].time_s - 1.5).abs() < 1e-4);
    }

    #[test]
    fn check_scene_warns_on_the_fifth_effect() {
        let mut text = String::from("format: 1\nseed: 1\nentities:\n");
        for i in 0..5 {
            text.push_str(&format!(
                "  - name: fx{i}\n    components:\n      ScreenEffect:\n        preset: crt\n        order: {i}\n"
            ));
        }
        text.push_str("  - name: off\n    components:\n      ScreenEffect: {}\n");
        let scene = ludion_scene::check(&text, &crate::builtin_registry())
            .unwrap_or_else(|d| panic!("{d:?}"));
        let diagnostics = scene_screen_effect_diagnostics(&scene);
        assert_eq!(diagnostics.len(), 1);
        assert_eq!(diagnostics[0].code, code::SCREEN_EFFECT_CAP);
        assert_eq!(
            diagnostics[0].path.as_deref(),
            Some("/entities/4/components/ScreenEffect")
        );
    }
}
