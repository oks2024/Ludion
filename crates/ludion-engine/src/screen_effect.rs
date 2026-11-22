//! Screen effects at extract: each applied `ScreenEffect` becomes a [`ScreenEffectData`] with its
//! output-pixel rectangle, UI split index, resolved stages and game time, routed to the frame or to
//! the `RenderView` it targets. The static half, [`scene_screen_effect_diagnostics`], reports the cap
//! and unknown view names from scene text.

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

use crate::secondary_view::ViewTable;

/// The applied effects, split by where they draw.
#[derive(Default)]
pub(crate) struct ScreenEffectsOut {
    /// Fill and region effects, in draw order: `RenderScene.screen_effects`.
    pub(crate) frame: Vec<ScreenEffectData>,
    /// Each rendered view's effects, indexed like `ViewTable.views`.
    pub(crate) per_view: Vec<Vec<ScreenEffectData>>,
}

/// The frame's screen effects, in draw order: the (at most [`MAX_SCREEN_EFFECTS`]) lowest
/// `(order, EntityId)` effects that change a pixel, fill, region and view targets together. An effect
/// with every stage off is neither applied nor counted; one with an unusable region or a view naming
/// no `RenderView` is `L1761`; a view that exists but does not render drops its effects silently;
/// each effect past the cap is `L2392`. `ui_orders` is `RenderScene.ui`'s sorted order column.
/// `time_s` and `frame` are game time, so a held `GameClock` holds the effect.
pub(crate) fn extract_screen_effects(
    world: &World,
    ids: &[EntityId],
    now: GameTime,
    viewport: (u32, u32),
    ui_orders: &[i32],
    views: &ViewTable,
    degraded: &mut Vec<(String, Diagnostic)>,
) -> ScreenEffectsOut {
    let mut out = ScreenEffectsOut {
        frame: Vec::new(),
        per_view: vec![Vec::new(); views.views.len()],
    };
    if world.component_count::<ScreenEffect>() == 0 {
        return out;
    }
    let mut candidates: Vec<(i32, EntityId, &ScreenEffect, Option<usize>)> = Vec::new();
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
                    format!("`ScreenEffect` target {what}"),
                )
                .with_path(path)
                .with_help("see `ludion explain L1761` and `ludion docs screen-effects`"),
            ));
            continue;
        }
        let mut slot = None;
        if let ScreenEffectTarget::View(name) = &effect.target {
            match views.by_name.get(name) {
                Some(Some(index)) => slot = Some(*index as usize),
                Some(None) => continue,
                None => {
                    degraded.push((
                        format!(
                            "{}/screen-effect-target",
                            crate::diagnostics::offender_key(world, id)
                        ),
                        unknown_view_diagnostic(name).with_path("/target/view"),
                    ));
                    continue;
                }
            }
        }
        candidates.push((effect.order, id, effect, slot));
    }
    candidates.sort_by_key(|(order, id, _, _)| (*order, *id));
    for (_, id, _, _) in candidates.iter().skip(MAX_SCREEN_EFFECTS) {
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
    for (order, _, effect, slot) in candidates.into_iter().take(MAX_SCREEN_EFFECTS) {
        let (rect_px, ui_start) = match slot {
            Some(slot) => {
                let (w, h) = views.views[slot].size;
                ([0.0, 0.0, w as f32, h as f32], 0)
            }
            None => (
                rect_px(&effect.target, viewport),
                ui_orders.partition_point(|&o| o < order) as u32,
            ),
        };
        let data = {
            let s = effect.stages();
            ScreenEffectData {
                rect_px,
                ui_start,
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
        };
        match slot {
            Some(slot) => out.per_view[slot].push(data),
            None => out.frame.push(data),
        }
    }
    out
}

fn unknown_view_diagnostic(name: &str) -> Diagnostic {
    Diagnostic::new(
        code::SCREEN_EFFECT_INVALID_TARGET,
        format!("`ScreenEffect` target view `{name}` is named by no `RenderView`"),
    )
    .with_help("see `ludion explain L1761` and `ludion docs screen-effects`")
}

/// The target's rectangle `[x, y, w, h]` in output pixels, clamped to the viewport (an empty clamp
/// is zero area).
fn rect_px(target: &ScreenEffectTarget, viewport: (u32, u32)) -> [f32; 4] {
    let (vw, vh) = (viewport.0 as f32, viewport.1 as f32);
    let [x, y, w, h] = match *target {
        ScreenEffectTarget::Fill | ScreenEffectTarget::View(_) => return [0.0, 0.0, vw, vh],
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

/// `L1761` at each `ScreenEffect` whose `view` target names no `RenderView`, and `L2392` at each
/// non-neutral effect past the cap, from scene text, in the order extract drops them (highest `order`,
/// then highest entity position). An effect with every stage off, an unusable target (`L1761`, raised
/// by component validation), an unknown view or a view with `enabled: false` is not counted.
pub fn scene_screen_effect_diagnostics(scene: &Scene) -> Vec<Diagnostic> {
    let mut views: std::collections::BTreeMap<&str, bool> = std::collections::BTreeMap::new();
    for entity in &scene.entities {
        let Some((_, view)) = entity
            .components
            .iter()
            .find(|(name, _)| name == "RenderView")
        else {
            continue;
        };
        if let Some(name) = view.get("name").and_then(serde_json::Value::as_str) {
            let enabled = view
                .get("enabled")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(true);
            views.entry(name).or_insert(enabled);
        }
    }
    let mut out = Vec::new();
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
        if let ScreenEffectTarget::View(name) = &effect.target {
            match views.get(name.as_str()) {
                Some(true) => {}
                Some(false) => continue,
                None => {
                    out.push(unknown_view_diagnostic(name).with_path(format!(
                        "/entities/{index}/components/ScreenEffect/target/view"
                    )));
                    continue;
                }
            }
        }
        applied.push((effect.order, index));
    }
    applied.sort();
    out.extend(
        applied
            .into_iter()
            .skip(MAX_SCREEN_EFFECTS)
            .map(|(_, index)| {
                cap_diagnostic(format!("the `ScreenEffect` at /entities/{index}"))
                    .with_path(format!("/entities/{index}/components/ScreenEffect"))
            }),
    );
    out
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
        let (out, degraded) = run_views(world, ids, ui_orders, &ViewTable::default());
        (out.frame, degraded)
    }

    fn run_views(
        world: &World,
        ids: &[EntityId],
        ui_orders: &[i32],
        views: &ViewTable,
    ) -> (ScreenEffectsOut, Vec<(String, Diagnostic)>) {
        let mut degraded = Vec::new();
        let now = GameTime::from_ticks(90, TickRate::DEFAULT);
        let out =
            extract_screen_effects(world, ids, now, VIEWPORT, ui_orders, views, &mut degraded);
        (out, degraded)
    }

    /// A table of views `(name, size, rendered)`; an unrendered one is authored but maps to `None`.
    fn table(views: &[(&str, (u32, u32), bool)]) -> ViewTable {
        let mut table = ViewTable::default();
        for &(name, size, rendered) in views {
            if rendered {
                table
                    .by_name
                    .insert(name.into(), Some(table.views.len() as u32));
                table.views.push(ludion_renderer::RenderViewData {
                    name: name.into(),
                    size,
                    camera: ludion_renderer::RenderScene::empty(size).camera,
                    layers: u32::MAX,
                    lights: u32::MAX,
                    sun: true,
                    background: None,
                    unlit: false,
                    fog: true,
                    post: ludion_renderer::ViewPost::Scene,
                    screen_effects: Vec::new(),
                });
            } else {
                table.by_name.insert(name.into(), None);
            }
        }
        table
    }

    fn on_view(name: &str, order: i32) -> ScreenEffect {
        ScreenEffect {
            target: ScreenEffectTarget::View(name.into()),
            ..crt(order)
        }
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
        let views = ViewTable::default();
        let a = extract_screen_effects(&world, &ids, held, VIEWPORT, &[], &views, &mut degraded);
        let b = extract_screen_effects(&world, &ids, held, VIEWPORT, &[], &views, &mut degraded);
        assert_eq!(a.frame, b.frame);
        assert_eq!(a.frame[0].frame, 90);
        assert!((a.frame[0].time_s - 1.5).abs() < 1e-4);
    }

    #[test]
    fn a_view_effect_lands_in_that_views_list_over_its_whole_texture() {
        let (world, ids) = world_of(vec![on_view("radar", 7)]);
        let views = table(&[("hud", (64, 64), true), ("radar", (256, 128), true)]);
        let (out, degraded) = run_views(&world, &ids, &[0, 5], &views);
        assert!(degraded.is_empty() && out.frame.is_empty());
        assert!(out.per_view[0].is_empty());
        assert_eq!(out.per_view[1].len(), 1);
        assert_eq!(out.per_view[1][0].rect_px, [0.0, 0.0, 256.0, 128.0]);
        assert_eq!(out.per_view[1][0].ui_start, 0);
        assert_eq!(out.per_view[1][0].scanlines, 0.35);
    }

    #[test]
    fn an_unknown_view_is_l1761_and_applied_nowhere() {
        let (world, ids) = world_of(vec![on_view("nope", 0)]);
        let views = table(&[("radar", (64, 64), true)]);
        let (out, degraded) = run_views(&world, &ids, &[], &views);
        assert!(out.frame.is_empty() && out.per_view[0].is_empty());
        assert_eq!(degraded.len(), 1);
        assert_eq!(degraded[0].1.code, code::SCREEN_EFFECT_INVALID_TARGET);
        assert!(degraded[0].1.message.contains("nope"));
        assert!(degraded[0].0.ends_with("/screen-effect-target"));
    }

    #[test]
    fn a_view_that_does_not_render_drops_its_effect_silently_and_uncounted() {
        let mut effects = vec![on_view("off", 0)];
        effects.extend((1..5).map(crt));
        let (world, ids) = world_of(effects);
        let views = table(&[("off", (64, 64), false)]);
        let (out, degraded) = run_views(&world, &ids, &[], &views);
        assert_eq!(out.frame.len(), 4);
        assert!(degraded.is_empty());
    }

    #[test]
    fn the_cap_counts_fill_and_view_effects_together() {
        let (world, ids) = world_of(vec![
            crt(0),
            on_view("radar", 1),
            crt(2),
            on_view("radar", 3),
            crt(4),
        ]);
        let views = table(&[("radar", (64, 64), true)]);
        let (out, degraded) = run_views(&world, &ids, &[], &views);
        assert_eq!(out.frame.len(), 2);
        assert_eq!(out.per_view[0].len(), 2);
        assert_eq!(degraded.len(), 1);
        assert_eq!(degraded[0].1.code, code::SCREEN_EFFECT_CAP);
        assert!(degraded[0].0.contains(&format!("e{}", ids[4].index())));
    }

    fn scene_of(text: &str) -> Scene {
        ludion_scene::check(text, &crate::builtin_registry()).unwrap_or_else(|d| panic!("{d:?}"))
    }

    #[test]
    fn check_scene_flags_an_unknown_view_at_its_pointer() {
        let scene = scene_of(
            "format: 1
seed: 1
entities:
  - name: v
    components:
      RenderView:
        name: radar
      Camera: {}
  - name: ok
    components:
      ScreenEffect:
        target:
          view: radar
        preset: crt
  - name: bad
    components:
      ScreenEffect:
        target:
          view: radr
        preset: crt
",
        );
        let found = scene_screen_effect_diagnostics(&scene);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].code, code::SCREEN_EFFECT_INVALID_TARGET);
        assert_eq!(
            found[0].path.as_deref(),
            Some("/entities/2/components/ScreenEffect/target/view")
        );
    }

    #[test]
    fn check_scene_cap_mirrors_extract_for_views() {
        let mut text = String::from(
            "format: 1
seed: 1
entities:
  - name: v
    components:
      RenderView:
        name: radar
      Camera: {}
  - name: off
    components:
      RenderView:
        name: dark
        enabled: false
      Camera: {}
",
        );
        for (i, view) in ["radar", "dark", "radar", "radar", "radar", "radar", "radar"]
            .iter()
            .enumerate()
        {
            text.push_str(&format!(
                "  - name: fx{i}
    components:
      ScreenEffect:
        target:
          view: {view}
        preset: crt
        order: {i}
"
            ));
        }
        let found = scene_screen_effect_diagnostics(&scene_of(&text));
        let caps: Vec<_> = found
            .iter()
            .filter(|d| d.code == code::SCREEN_EFFECT_CAP)
            .collect();
        assert_eq!(found.len(), caps.len());
        assert_eq!(caps.len(), 2);
        assert_eq!(
            caps[0].path.as_deref(),
            Some("/entities/7/components/ScreenEffect")
        );
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
