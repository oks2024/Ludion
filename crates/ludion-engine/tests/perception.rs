//! Runtime proof for sight (`Plan/perception.md` Wave 1): zones, sample points and batched line of
//! sight, the L1572 name checks, the sweep, and the derived-state rules (hash, threads, game clock).

use ludion_core::codes::code;
use ludion_core::ids::EntityId;
use ludion_core::math;
use ludion_core::par;
use ludion_core::time::GameClock;
use ludion_ecs::{SystemCtx, World};
use ludion_engine::diagnostics::Diagnostics;
use ludion_engine::{
    Engine, EngineBuilder, Name, Perceiver, PerceptionConfig, PerceptionResults, SightSample,
    Transform, perception_update,
};

const LAYERS: &str = "  layers:\n    - name: wall\n    - name: cover_low\n    - name: cover_high\n    - name: cover_overhead\n";

/// A static box on `layer`: centre, half extents.
struct Prop {
    name: &'static str,
    at: [f32; 3],
    half: [f32; 3],
    layer: &'static str,
}

fn prop(name: &'static str, layer: &'static str, at: [f32; 3], half: [f32; 3]) -> Prop {
    Prop {
        name,
        at,
        half,
        layer,
    }
}

/// Splits `text` at the commas outside any bracket.
fn split_top(text: &str) -> Vec<&str> {
    let (mut parts, mut depth, mut start) = (Vec::new(), 0, 0);
    for (at, c) in text.char_indices() {
        match c {
            '[' | '{' => depth += 1,
            ']' | '}' => depth -= 1,
            ',' if depth == 0 => {
                parts.push(&text[start..at]);
                start = at + 1;
            }
            _ => {}
        }
    }
    parts.push(&text[start..]);
    parts.into_iter().filter(|p| !p.trim().is_empty()).collect()
}

/// Scene files take block style only: turns `k: v, list: [a, b], map: { x: 1 }` into block YAML
/// with its keys at `indent` spaces.
fn fields(indent: usize, text: &str) -> String {
    let pad = " ".repeat(indent);
    let mut out = String::new();
    for part in split_top(text) {
        let (key, value) = part.split_once(':').expect("key: value");
        let (key, value) = (key.trim(), value.trim());
        let inner = |open: char, close: char| {
            value
                .strip_prefix(open)
                .and_then(|v| v.strip_suffix(close))
                .filter(|i| !i.trim().is_empty())
        };
        if let Some(items) = inner('[', ']') {
            out += &format!("{pad}{key}:\n");
            for item in split_top(items) {
                out += &format!("{pad}  - {}\n", item.trim());
            }
        } else if let Some(map) = inner('{', '}') {
            out += &format!("{pad}{key}:\n{}", fields(indent + 2, map));
        } else {
            out += &format!("{pad}{key}: {value}\n");
        }
    }
    out
}

/// One block-sequence item holding the map `text`, its dash at `indent` spaces.
fn item(indent: usize, text: &str) -> String {
    let body = fields(indent + 2, text);
    format!("{}- {}", " ".repeat(indent), &body[indent + 2..])
}

/// An entity with a `Transform` at `at` and the given extra component YAML (already indented to 6).
fn entity(name: &str, at: [f32; 3], components: &str) -> String {
    format!(
        "  - name: {name}\n    components:\n      Transform:\n{}{components}",
        fields(8, &format!("translation: {at:?}"))
    )
}

fn scene(entities: &[String], props: &[Prop]) -> String {
    let mut out = format!("format: 1\nseed: 1\nphysics:\n  space: free\n{LAYERS}entities:\n");
    for e in entities {
        out += e;
    }
    for p in props {
        out += &entity(
            p.name,
            p.at,
            &format!(
                "      Collider:\n        shape:\n          box:\n{}        body: static\n        layer: {}\n",
                fields(12, &format!("half_extents: {:?}", p.half)),
                p.layer
            ),
        );
    }
    out
}

/// A perceiver at the origin; `extra` is more `Perceiver` fields, `others` whole components.
fn perceiver_with(eye: [f32; 3], zones: &str, extra: &str, others: &str) -> String {
    entity(
        "guard",
        [0.0; 3],
        &format!(
            "      Perceiver:\n{}        sight:\n{zones}{}{others}",
            fields(8, &format!("eye: {eye:?}")),
            fields(8, extra)
        ),
    )
}

fn perceiver(eye: [f32; 3], zones: &str, extra: &str) -> String {
    perceiver_with(eye, zones, extra, "")
}

/// One zone, its fields as flow-style text.
fn zone(text: &str) -> String {
    item(10, text)
}

fn target(
    name: &str,
    at: [f32; 3],
    class: &str,
    rest: &str,
    points: &[([f32; 3], &[&str])],
) -> String {
    let mut yaml = format!(
        "      SightTarget:\n        class: {class}\n{}        points:\n",
        fields(8, rest)
    );
    for (offset, blocked) in points {
        yaml += &item(
            10,
            &format!("offset: {offset:?}, blocked_by: [{}]", blocked.join(", ")),
        );
    }
    entity(name, at, &yaml)
}

fn builder() -> EngineBuilder {
    Engine::builder()
        .perception(PerceptionConfig {
            classes: vec!["player".into(), "decoy".into()],
        })
        .system("perception_update", perception_update)
}

fn engine(text: &str) -> Engine {
    let mut engine = builder().build().expect("boots");
    engine.load_inline_scene(text).expect("the scene loads");
    engine
}

fn run(engine: &mut Engine, ticks: usize) {
    for _ in 0..ticks {
        engine.advance_live();
    }
}

fn find(engine: &mut Engine, wanted: &str) -> EntityId {
    engine
        .world_mut()
        .query_ids::<(&Name,)>()
        .find(|(_, (name,))| name.0 == wanted)
        .map(|(id, _)| id)
        .unwrap_or_else(|| panic!("no entity named {wanted}"))
}

fn results(engine: &Engine) -> &PerceptionResults {
    engine.world().resource::<PerceptionResults>().unwrap()
}

/// The sample of `perceiver`'s `zone` on `target`, after a few ticks (physics needs one to settle).
fn sample(engine: &mut Engine, zone: u8, target_name: &str) -> Option<SightSample> {
    let guard = find(engine, "guard");
    let target = find(engine, target_name);
    results(engine)
        .sight(guard)
        .iter()
        .find(|s| s.zone == zone && s.target == target)
        .cloned()
}

const STAND: ([f32; 3], &[&str]) = ([0.0, 1.6, 0.0], &["wall", "cover_high"]);
const CROUCH: ([f32; 3], &[&str]) = ([0.0, 0.7, 0.0], &["wall", "cover_low", "cover_high"]);
const PRONE: ([f32; 3], &[&str]) = ([0.0, 0.2, 0.0], &["wall", "cover_low", "cover_high"]);

/// The three postures against one cover prop 0.5 m in front of a target 10 m away.
fn cover_cell(cover: &'static str, top: f32) -> [bool; 3] {
    let entities = [
        perceiver(
            [0.0, 1.6, 0.0],
            &zone("name: main, range: 20.0, half_angle_deg: 60.0"),
            "",
        ),
        target("stand", [0.0, 0.0, -10.0], "player", "", &[STAND]),
        target("crouch", [0.0, 0.0, -10.0], "player", "", &[CROUCH]),
        target("prone", [0.0, 0.0, -10.0], "player", "", &[PRONE]),
    ];
    let props = [prop(
        "cover",
        cover,
        [0.0, top / 2.0, -9.5],
        [2.0, top / 2.0, 0.1],
    )];
    let mut engine = engine(&scene(&entities, &props));
    run(&mut engine, 4);
    ["stand", "crouch", "prone"].map(|n| sample(&mut engine, 0, n).expect("in the wedge").visible)
}

#[test]
fn the_six_posture_cells_against_low_and_high_cover() {
    assert_eq!(cover_cell("cover_low", 0.8), [true, false, false]);
    assert_eq!(cover_cell("cover_high", 2.0), [false, false, false]);
}

/// A prone target under a table (`cover_overhead`) 3.3 m away, with the inner zone's `see_through`.
fn under_the_table(see_through_max_range: f32) -> bool {
    let zones = zone(&format!(
        "name: inner, range: 6.0, half_angle_deg: 60.0, see_through: [cover_overhead], see_through_max_range: {see_through_max_range:?}"
    ));
    let entities = [
        perceiver([0.0, 1.6, 0.0], &zones, ""),
        target(
            "prone",
            [0.0, 0.0, -3.0],
            "player",
            "",
            &[([0.0, 0.2, 0.0], &["wall", "cover_overhead"])],
        ),
    ];
    let props = [prop(
        "table",
        "cover_overhead",
        [0.0, 0.7, -2.0],
        [2.0, 0.2, 0.1],
    )];
    let mut engine = engine(&scene(&entities, &props));
    run(&mut engine, 4);
    sample(&mut engine, 0, "prone")
        .expect("in the wedge")
        .visible
}

#[test]
fn see_through_applies_only_within_its_range() {
    assert!(under_the_table(3.5), "the point is 3.3 m away, inside 3.5");
    assert!(!under_the_table(3.0), "beyond 3.0 the table blocks again");
    assert!(!under_the_table(0.0), "0 disables see_through");
}

#[test]
fn the_outer_zone_scales_with_visibility_and_the_inner_does_not() {
    let zones = zone("name: inner, range: 15.0, half_angle_deg: 60.0")
        + &zone("name: outer, range: 20.0, half_angle_deg: 30.0, range_by_visibility: true");
    let dim = "visibility: 0.5";
    let entities = [
        perceiver([0.0; 3], &zones, ""),
        target(
            "dim_far",
            [0.0, 0.0, -12.0],
            "player",
            dim,
            &[([0.0; 3], &[])],
        ),
        target(
            "lit_far",
            [0.0, 0.0, -12.0],
            "player",
            "",
            &[([0.0; 3], &[])],
        ),
        target(
            "dim_near",
            [0.0, 0.0, -8.0],
            "player",
            dim,
            &[([0.0; 3], &[])],
        ),
    ];
    let mut engine = engine(&scene(&entities, &[]));
    run(&mut engine, 3);
    for name in ["dim_far", "lit_far", "dim_near"] {
        assert_eq!(
            sample(&mut engine, 0, name).unwrap().effective_range,
            15.0,
            "{name}"
        );
    }
    assert!(
        sample(&mut engine, 1, "dim_far").is_none(),
        "20 * 0.5 = 10 < 12"
    );
    assert_eq!(
        sample(&mut engine, 1, "lit_far").unwrap().effective_range,
        20.0
    );
    assert_eq!(
        sample(&mut engine, 1, "dim_near").unwrap().effective_range,
        10.0
    );
}

#[test]
fn the_half_angle_edges_hold_in_both_axes() {
    let zones = zone("name: z, range: 20.0, half_angle_deg: 30.0, vertical_half_angle_deg: 20.0");
    let at = |h: f32, v: f32| {
        let (h, v) = (h.to_radians(), v.to_radians());
        [
            5.0 * math::cos(v) * math::sin(h),
            5.0 * math::sin(v),
            -5.0 * math::cos(v) * math::cos(h),
        ]
    };
    let entities = [
        perceiver([0.0; 3], &zones, ""),
        target("h_in", at(29.0, 0.0), "player", "", &[([0.0; 3], &[])]),
        target("h_out", at(31.0, 0.0), "player", "", &[([0.0; 3], &[])]),
        target("v_in", at(0.0, 19.0), "player", "", &[([0.0; 3], &[])]),
        target("v_out", at(0.0, 21.0), "player", "", &[([0.0; 3], &[])]),
        target(
            "v_down_out",
            at(0.0, -21.0),
            "player",
            "",
            &[([0.0; 3], &[])],
        ),
        target("behind", [0.0, 0.0, 5.0], "player", "", &[([0.0; 3], &[])]),
    ];
    let mut engine = engine(&scene(&entities, &[]));
    run(&mut engine, 2);
    let seen = |engine: &mut Engine, n: &str| sample(engine, 0, n).is_some();
    assert!(seen(&mut engine, "h_in") && seen(&mut engine, "v_in"));
    for n in ["h_out", "v_out", "v_down_out", "behind"] {
        assert!(!seen(&mut engine, n), "{n}");
    }
    let sample = sample(&mut engine, 0, "h_in").unwrap();
    assert!((sample.angle_deg - 29.0).abs() < 0.01 && (sample.distance - 5.0).abs() < 1e-4);
    assert!(sample.in_zone && sample.visible && sample.points_visible == 1);
}

/// A guard (eye 1.5 m up), a target 10 m away with head, chest and foot points, and a 0.6 m wall
/// 0.5 m in front of it that hides the foot only.
fn three_points(rest: &str) -> Engine {
    let points: [([f32; 3], &[&str]); 3] = [
        ([0.0, 1.5, 0.0], &["wall"]),
        ([0.0, 1.0, 0.0], &["wall"]),
        ([0.0, 0.2, 0.0], &["wall"]),
    ];
    let entities = [
        perceiver(
            [0.0, 1.5, 0.0],
            &zone("name: z, range: 20.0, half_angle_deg: 60.0"),
            "",
        ),
        target("t", [0.0, 0.0, -10.0], "player", rest, &points),
    ];
    let props = [prop("low_wall", "wall", [0.0, 0.3, -9.5], [2.0, 0.3, 0.1])];
    let mut engine = engine(&scene(&entities, &props));
    run(&mut engine, 4);
    engine
}

#[test]
fn require_any_all_and_fraction_read_the_visible_points() {
    for (rest, visible) in [
        ("", true),
        ("require: any", true),
        ("require: all", false),
        ("require: { fraction: 0.66 }", true),
        ("require: { fraction: 0.67 }", false),
        ("require: { fraction: 1.0 }", false),
    ] {
        let mut engine = three_points(rest);
        let s = sample(&mut engine, 0, "t").unwrap();
        assert_eq!((s.points_visible, s.visible), (0b011, visible), "{rest}");
        assert_eq!(s.blocker, Some(find(&mut engine, "low_wall")), "{rest}");
    }
}

#[test]
fn the_blocker_is_the_nearest_prop_on_the_ray() {
    let entities = [
        perceiver(
            [0.0; 3],
            &zone("name: z, range: 30.0, half_angle_deg: 30.0"),
            "",
        ),
        target(
            "t",
            [0.0, 0.0, -20.0],
            "player",
            "",
            &[([0.0; 3], &["wall"])],
        ),
    ];
    let props = [
        prop("far", "wall", [0.0, 0.0, -15.0], [1.0, 1.0, 0.1]),
        prop("near", "wall", [0.0, 0.0, -5.0], [1.0, 1.0, 0.1]),
    ];
    let mut engine = engine(&scene(&entities, &props));
    run(&mut engine, 3);
    let s = sample(&mut engine, 0, "t").unwrap();
    assert!(!s.visible);
    assert_eq!(s.blocker, Some(find(&mut engine, "near")));
}

#[test]
fn an_empty_blocked_by_casts_no_ray_and_sees_through_a_wall() {
    let entities = [
        perceiver(
            [0.0; 3],
            &zone("name: z, range: 30.0, half_angle_deg: 30.0"),
            "",
        ),
        target("open", [0.0, 0.0, -10.0], "player", "", &[([0.0; 3], &[])]),
        target(
            "walled",
            [0.0, 0.0, -10.0],
            "player",
            "",
            &[([0.0; 3], &["wall"])],
        ),
    ];
    let props = [prop("wall", "wall", [0.0, 0.0, -5.0], [2.0, 2.0, 0.1])];
    let mut engine = engine(&scene(&entities, &props));
    run(&mut engine, 3);
    let open = sample(&mut engine, 0, "open").unwrap();
    assert!(open.visible && open.blocker.is_none());
    assert!(!sample(&mut engine, 0, "walled").unwrap().visible);
}

#[test]
fn an_eye_inside_its_own_blocking_collider_is_recast_past_it() {
    let own = "      Collider:\n        shape:\n          box:\n            half_extents:\n              - 0.5\n              - 0.5\n              - 0.5\n        body: static\n        layer: wall\n";
    let entities = [
        perceiver_with(
            [0.0; 3],
            &zone("name: z, range: 30.0, half_angle_deg: 60.0"),
            "",
            own,
        ),
        target(
            "clear",
            [0.0, 0.0, -10.0],
            "player",
            "",
            &[([0.0; 3], &["wall"])],
        ),
        target(
            "walled",
            [6.0, 0.0, -10.0],
            "player",
            "",
            &[([0.0; 3], &["wall"])],
        ),
    ];
    let props = [prop("wall", "wall", [3.0, 0.0, -5.0], [1.0, 2.0, 0.1])];
    let mut engine = engine(&scene(&entities, &props));
    run(&mut engine, 4);
    let clear = sample(&mut engine, 0, "clear").unwrap();
    assert!(clear.visible && clear.blocker.is_none(), "{clear:?}");
    let walled = sample(&mut engine, 0, "walled").unwrap();
    assert_eq!(
        walled.blocker,
        Some(find(&mut engine, "wall")),
        "{walled:?}"
    );
}

fn reported(engine: &Engine) -> Vec<String> {
    engine
        .world()
        .resource::<Diagnostics>()
        .unwrap()
        .reported(code::PERCEPTION_NAME_UNKNOWN)
        .map(String::from)
        .collect()
}

#[test]
fn unknown_names_report_l1572_once_and_see_nothing() {
    let zones = zone("name: z, range: 30.0, half_angle_deg: 60.0, detects: [plyer]")
        + &zone("name: all, range: 30.0, half_angle_deg: 60.0");
    let entities = [
        perceiver([0.0; 3], &zones, ""),
        target("a", [0.0, 0.0, -5.0], "player", "", &[([0.0; 3], &[])]),
        target("ghost", [0.0, 0.0, -5.0], "ghost", "", &[([0.0; 3], &[])]),
        target(
            "typo",
            [0.0, 0.0, -5.0],
            "player",
            "",
            &[([0.0; 3], &["wal"])],
        ),
    ];
    let ground = [prop("ground", "wall", [0.0, -1.0, 0.0], [20.0, 0.5, 20.0])];
    let mut engine = engine(&scene(&entities, &ground));
    run(&mut engine, 5);
    let mut keys = reported(&engine);
    keys.sort();
    assert_eq!(keys, ["class:ghost", "class:plyer", "layer:wal"]);
    assert!(
        sample(&mut engine, 0, "a").is_none(),
        "`plyer` matches nothing"
    );
    assert!(
        sample(&mut engine, 1, "ghost").is_none(),
        "an undeclared class is detected by no zone"
    );
    let typo = sample(&mut engine, 1, "typo").unwrap();
    assert!(!typo.visible && typo.points_visible == 0);
    assert!(sample(&mut engine, 1, "a").unwrap().visible);
    run(&mut engine, 5);
    assert_eq!(reported(&engine).len(), 3, "once per name");
}

#[test]
fn a_disabled_perceiver_has_no_samples() {
    let on = [
        perceiver(
            [0.0; 3],
            &zone("name: z, range: 30.0, half_angle_deg: 60.0"),
            "",
        ),
        target("t", [0.0, 0.0, -5.0], "player", "", &[([0.0; 3], &[])]),
    ];
    let mut engine = engine(&scene(&on, &[]));
    run(&mut engine, 2);
    assert_eq!(results(&engine).samples().len(), 1);
    let guard = find(&mut engine, "guard");
    engine
        .world_mut()
        .get_mut::<Perceiver>(guard)
        .unwrap()
        .unwrap()
        .enabled = false;
    run(&mut engine, 1);
    assert!(results(&engine).samples().is_empty());
    assert!(results(&engine).zone_facing(guard, 0).is_none());
}

fn crowd() -> String {
    let zones = zone("name: a, range: 40.0, half_angle_deg: 90.0")
        + &zone("name: b, range: 25.0, half_angle_deg: 40.0, range_by_visibility: true");
    let mut entities = vec![
        perceiver([0.0, 1.6, 0.0], &zones, ""),
        entity(
            "guard2",
            [3.0, 0.0, 3.0],
            &format!(
                "      Perceiver:\n        eye:\n          - 0.0\n          - 1.6\n          - 0.0\n        sight:\n{zones}"
            ),
        ),
    ];
    for n in 0..40 {
        let x = (n % 8) as f32 * 3.0 - 10.0;
        let z = -4.0 - (n / 8) as f32 * 4.0;
        entities.push(target(
            &format!("t{n:02}"),
            [x, 0.0, z],
            if n % 5 == 0 { "decoy" } else { "player" },
            "visibility: 0.8",
            &[
                ([0.0, 1.6, 0.0], &["wall", "cover_high"]),
                ([0.0, 0.4, 0.0], &["wall", "cover_low"]),
            ],
        ));
    }
    let props = [
        prop("w1", "wall", [-4.0, 1.0, -6.0], [2.0, 1.0, 0.2]),
        prop("w2", "cover_high", [4.0, 1.0, -10.0], [3.0, 1.0, 0.2]),
        prop("w3", "cover_low", [0.0, 0.4, -14.0], [6.0, 0.4, 0.2]),
    ];
    scene(&entities, &props)
}

fn all_samples(threads: usize) -> Vec<SightSample> {
    par::with_threads(threads, || {
        let mut engine = engine(&crowd());
        run(&mut engine, 4);
        results(&engine).samples().to_vec()
    })
}

#[test]
fn samples_are_sorted_and_identical_at_any_thread_count() {
    let one = all_samples(1);
    assert!(one.len() > 40, "the crowd is seen: {}", one.len());
    assert!(
        one.windows(2)
            .all(|w| (w[0].perceiver, w[0].zone, w[0].target)
                < (w[1].perceiver, w[1].zone, w[1].target)),
        "sorted (perceiver, zone, target)"
    );
    assert!(one.iter().any(|s| s.visible) && one.iter().any(|s| !s.visible));
    assert_eq!(all_samples(2), one);
    assert_eq!(all_samples(8), one);
}

#[test]
fn sight_slices_one_perceivers_samples() {
    let mut engine = engine(&crowd());
    run(&mut engine, 3);
    let (g1, g2) = (find(&mut engine, "guard"), find(&mut engine, "guard2"));
    let all = results(&engine).samples().len();
    let (a, b) = (
        results(&engine).sight(g1).len(),
        results(&engine).sight(g2).len(),
    );
    assert!(a > 0 && b > 0 && a + b == all);
    assert!(results(&engine).sight(g1).iter().all(|s| s.perceiver == g1));
    assert!(results(&engine).sight(EntityId::new(999, 0)).is_empty());
}

/// Walks `walker` along +x by 0.1 m a tick.
fn walk(world: &mut World, _ctx: &SystemCtx) {
    let ids: Vec<EntityId> = world
        .query_ids::<(&Name, &Transform)>()
        .filter(|(_, (n, _))| n.0 == "walker")
        .map(|(id, _)| id)
        .collect();
    for id in ids {
        world.get_mut::<Transform>(id).unwrap().unwrap().translation[0] += 0.1;
    }
}

#[test]
fn a_walker_appears_and_disappears_behind_a_wall_on_the_expected_ticks() {
    let entities = [
        perceiver(
            [0.0, 1.6, 0.0],
            &zone("name: z, range: 30.0, half_angle_deg: 80.0"),
            "",
        ),
        target(
            "walker",
            [-4.15, 0.0, -10.0],
            "player",
            "",
            &[([0.0, 1.0, 0.0], &["cover_high"])],
        ),
    ];
    // The wall's half width is 1 at z = -5; the ray to the walker crosses it at x / 2.
    let props = [prop(
        "wall",
        "cover_high",
        [0.0, 1.5, -5.0],
        [1.0, 1.5, 0.1],
    )];
    // `walk` runs before `perception_update`, so the samples match the walker's current position.
    let mut engine = Engine::builder()
        .perception(PerceptionConfig {
            classes: vec!["player".into()],
        })
        .system("walk", walk)
        .system("perception_update", perception_update)
        .build()
        .expect("boots");
    engine.load_inline_scene(&scene(&entities, &props)).unwrap();
    let walker = find(&mut engine, "walker");
    let mut trace = Vec::new();
    for _ in 0..90 {
        engine.advance_live();
        let x = engine
            .world()
            .get::<Transform>(walker)
            .unwrap()
            .unwrap()
            .translation[0];
        let seen = results(&engine)
            .samples()
            .iter()
            .find(|s| s.target == walker)
            .map(|s| s.visible);
        trace.push((x, seen));
    }
    let mut states = Vec::new();
    for (x, visible) in &trace[3..] {
        let expected = x.abs() > 2.0;
        assert_eq!(*visible, Some(expected), "x = {x}");
        if states.last() != Some(&expected) {
            states.push(expected);
        }
    }
    assert_eq!(
        states,
        [true, false, true],
        "seen, hidden behind the wall, seen again"
    );
}

fn facing_yaw(engine: &Engine, guard: EntityId) -> (u64, f32) {
    let f = results(engine).zone_facing(guard, 0).unwrap();
    (
        results(engine).tick(),
        math::atan2(-f[0], -f[2]).to_degrees(),
    )
}

fn hold_window(world: &mut World, ctx: &SystemCtx) {
    if (30..=59).contains(&ctx.frame.as_u64()) {
        world.resource_mut::<GameClock>().unwrap().hold();
    }
}

#[test]
fn the_sweep_follows_its_closed_form_and_freezes_under_a_hold() {
    let sweep = "sweep: { amplitude_deg: 30.0, period_s: 4.0, shape: sine }";
    let entities = [perceiver(
        [0.0; 3],
        &zone("name: z, range: 10.0, half_angle_deg: 40.0"),
        sweep,
    )];
    let mut engine = Engine::builder()
        .perception(PerceptionConfig {
            classes: vec!["player".into()],
        })
        .system("hold_window", hold_window)
        .system("perception_update", perception_update)
        .build()
        .expect("boots");
    engine.load_inline_scene(&scene(&entities, &[])).unwrap();
    let guard = find(&mut engine, "guard");
    let mut yaws = Vec::new();
    for _ in 0..90 {
        engine.advance_live();
        yaws.push(facing_yaw(&engine, guard));
    }
    for (tick, yaw) in &yaws {
        let t = *tick as f64 / 60.0;
        let want = 30.0 * math::f64::sin(std::f64::consts::TAU * t / 4.0);
        assert!(
            (f64::from(*yaw) - want).abs() < 1e-2,
            "tick {tick}: {yaw} vs {want}"
        );
    }
    let held_tick = yaws[40].0;
    let held: Vec<_> = yaws.iter().filter(|(tick, _)| *tick == held_tick).collect();
    assert!(
        held.len() >= 30,
        "the facing sits still while the game holds: {}",
        held.len()
    );
    assert!(held.iter().all(|y| y.1 == held[0].1));
    assert!(
        yaws.last().unwrap().0 < 90,
        "gameplay time lost the held frames"
    );
}

fn trace(mut engine: Engine) -> Vec<u64> {
    (0..40)
        .map(|_| {
            engine.advance_live();
            engine.world_hash().unwrap().as_u64()
        })
        .collect()
}

#[test]
fn two_runs_hash_the_same_every_tick() {
    let a = trace(engine(&crowd()));
    let b = trace(engine(&crowd()));
    assert_eq!(a, b);
}

#[test]
fn installing_perception_does_not_move_a_world_without_it() {
    let plain_scene = scene(
        &[entity("a", [1.0, 0.0, 0.0], "")],
        &[prop("wall", "wall", [0.0, 0.0, -5.0], [1.0, 1.0, 1.0])],
    );
    let mut plain = Engine::builder().build().unwrap();
    plain.load_inline_scene(&plain_scene).unwrap();
    let mut with = builder().build().unwrap();
    with.load_inline_scene(&plain_scene).unwrap();
    assert_eq!(trace(with), trace(plain));
}
