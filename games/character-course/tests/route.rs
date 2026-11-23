//! The scripted route as a numeric trace: two runs print the same bytes, and the hero's records
//! show each element of the course doing its job. Compared run to run, never against a committed
//! trace.

mod common;

use std::path::PathBuf;

use common::{FRAMES, ROUTE};
use ludion_core::time::FrameIndex;
use ludion_engine::{LaneRecord, MotorRecord, TraceChannel, TraceRecord, trace_with, write_jsonl};
use ludion_scene::{PathMetric, PathSampler, parse_path};
use ludion_sim::{InputEvent, InputScript};

fn script() -> InputScript {
    let mut script = InputScript::new();
    for (frame, key, down) in ROUTE {
        let event = if down {
            InputEvent::KeyDown { key }
        } else {
            InputEvent::KeyUp { key }
        };
        script.push(FrameIndex::new(frame), event);
    }
    script
}

fn run() -> Vec<TraceRecord> {
    let scenes = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("scenes");
    trace_with(
        character_course::game().project_root(scenes.clone()),
        &scenes.join("main.yaml"),
        &script(),
        FRAMES,
        &[TraceChannel::Motor],
    )
    .expect("the course traces")
}

fn bytes(records: &[TraceRecord]) -> Vec<u8> {
    let mut out = Vec::new();
    write_jsonl(records, &mut out).unwrap();
    out
}

fn hero(records: &[TraceRecord]) -> Vec<&MotorRecord> {
    records
        .iter()
        .filter_map(|record| match record {
            TraceRecord::Motor(motor) if motor.entity == "hero" => Some(motor),
            _ => None,
        })
        .collect()
}

#[test]
fn two_runs_of_the_route_print_the_same_bytes() {
    let (first, second) = (bytes(&run()), bytes(&run()));
    assert!(!first.is_empty());
    assert!(first == second, "the route trace differs between two runs");
}

#[test]
fn the_route_clears_every_element_of_the_lane() {
    let records = run();
    let hero = hero(&records);
    let reports: Vec<_> = hero
        .iter()
        .filter_map(|m| m.report.as_ref().map(|r| (m.tick, m.feet, r)))
        .collect();

    assert!(
        reports.iter().all(|(_, _, r)| !r.stuck),
        "the hero was crushed"
    );
    // The lane section's pit kills once (x in 38.5..40, drifting on to 41.5 as it falls); nothing else falls out of the course.
    let low: Vec<_> = reports
        .iter()
        .filter(|(_, feet, _)| feet[1] <= -1.0)
        .collect();
    assert!(!low.is_empty(), "the pit never killed the hero");
    assert!(
        low.iter()
            .all(|(_, feet, _)| (38.5..41.5).contains(&feet[0])),
        "the hero fell out of the course outside the pit"
    );
    let falls = reports
        .windows(2)
        .filter(|w| w[0].1[1] > -1.0 && w[1].1[1] <= -1.0)
        .count();
    assert_eq!(falls, 1, "falls below y = -1");

    // The drop onto the spawn pad lands once, and only then does the step come.
    let first_step = reports
        .iter()
        .position(|(_, _, r)| r.stepped.is_some())
        .expect("the hero climbs the step");
    let landings = reports[..first_step]
        .iter()
        .filter(|(_, _, r)| r.landed)
        .count();
    assert_eq!(landings, 1, "landed ticks before the step");

    // Crouched the whole way through the tunnel (x 11.5..14.5), upright again soon after.
    let in_tunnel: Vec<_> = reports
        .iter()
        .filter(|(_, feet, _)| (11.8..14.2).contains(&feet[0]))
        .collect();
    assert!(in_tunnel.len() > 20, "the hero crosses the tunnel");
    assert!(in_tunnel.iter().all(|(_, _, r)| r.height == 0.6));
    let exit = reports
        .iter()
        .position(|(_, feet, _)| feet[0] > 14.5)
        .expect("the hero leaves the tunnel");
    let window = &reports[exit..(exit + 40).min(reports.len())];
    assert!(
        window.iter().any(|(_, _, r)| r.height == 0.9),
        "the hero stands up after the tunnel"
    );

    // Airborne over the pit (x 15..16.5), then landed on the far side.
    let over_pit = reports
        .iter()
        .position(|(_, feet, r)| !r.grounded && (15.0..16.5).contains(&feet[0]))
        .expect("an airborne tick over the pit");
    assert!(
        reports[over_pit..]
            .iter()
            .any(|(_, feet, r)| r.landed && feet[0] > 16.5),
        "the hero lands past the pit"
    );

    // The belt carries 1.5 m/s: at least half a second of ticks, each by exactly 1.5/60.
    let on_belt: Vec<_> = reports
        .iter()
        .filter(|(_, _, r)| r.grounded && r.platform.entity.as_deref() == Some("conveyor"))
        .collect();
    assert!(
        on_belt.len() >= 30,
        "{} ticks on the conveyor",
        on_belt.len()
    );
    for (tick, _, r) in &on_belt {
        assert!(
            (r.platform.surface[0] - 1.5 / 60.0).abs() < 1e-5,
            "tick {tick}: surface {:?}",
            r.platform.surface
        );
    }

    let last = reports.last().unwrap();
    assert!(last.1[0] >= 50.0, "the route ends at x = {}", last.1[0]);
}

/// The chase section past the exit pad: the route walks the hero under the porch (x 61..65) and stops
/// beyond it, still on the chase floor.
#[test]
fn the_route_ends_past_the_chase_porch() {
    let records = run();
    let hero = hero(&records);
    let last = hero.last().unwrap();
    assert!(
        last.feet[0] > 64.0,
        "the route ends at x = {}",
        last.feet[0]
    );
    assert!(last.feet[1] > 1.0, "the hero left the chase floor");
}

/// The lane section: engage, hold, the death in the pit, the respawn back on the lane, the belt,
/// the release.
#[test]
fn the_lane_section_holds_the_hero() {
    let records = run();
    let hero = hero(&records);
    let lanes: Vec<(usize, &LaneRecord)> = hero
        .iter()
        .enumerate()
        .filter_map(|(i, m)| Some((i, m.report.as_ref()?.lane.as_ref()?)))
        .collect();
    assert!(!lanes.is_empty(), "the lane never engaged");
    let first = lanes[0].0;
    assert!(
        hero[first].feet[0] >= 28.95,
        "engaged at x = {}",
        hero[first].feet[0]
    );

    // After the ease the hero sits on the centre line.
    for (i, lane) in &lanes {
        if lane.weight == 1.0 {
            assert!(
                lane.lateral_error.abs() < 0.05,
                "tick {}: lateral error {}",
                hero[*i].tick,
                lane.lateral_error
            );
        }
    }

    // A transition moves the feet toward the lane no faster than the pull speed.
    let (mut engaged, mut released) = (false, false);
    for pair in lanes.windows(2) {
        let ((ia, a), (ib, b)) = (pair[0], pair[1]);
        if ib != ia + 1
            || !((0.0 < a.weight && a.weight < 1.0) || (0.0 < b.weight && b.weight < 1.0))
        {
            continue;
        }
        engaged |= b.weight > a.weight;
        released |= b.weight < a.weight;
        assert!(
            (b.lateral_error - a.lateral_error).abs() <= 4.0 / 60.0 + 1e-4,
            "tick {}: lateral error {} -> {}",
            hero[ib].tick,
            a.lateral_error,
            b.lateral_error
        );
    }
    assert!(
        engaged && released,
        "engage {engaged}, disengage {released}"
    );

    // Reached the exit straight and left the lane there.
    assert!(
        lanes
            .iter()
            .any(|(i, l)| l.weight == 1.0 && hero[*i].feet[0] > 44.0),
        "the lane held the hero past the drop"
    );
    assert!(
        hero.last()
            .unwrap()
            .report
            .as_ref()
            .is_some_and(|r| r.lane.is_none()),
        "the exit sensor never released the lane"
    );

    // The respawn lands on the lane at the checkpoint: the first lane report after the teleport.
    let teleport = hero
        .windows(2)
        .position(|w| w[1].feet[0] < w[0].feet[0] - 1.0)
        .expect("the pit kills the hero")
        + 1;
    let after = lanes
        .iter()
        .find(|(i, _)| *i > teleport)
        .expect("a lane report after the respawn");
    let text = std::fs::read_to_string(
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("scenes/paths/lane.path.yaml"),
    )
    .unwrap();
    let sampler = PathSampler::new(&parse_path(&text).unwrap());
    let s_cp = sampler.nearest_s_global([37.0, 2.421875, -0.25], PathMetric::Full) as f32;
    assert!(
        (after.1.s - s_cp).abs() < 0.1,
        "re-acquired s = {}, checkpoint s = {s_cp}",
        after.1.s
    );
    assert!(
        after.1.lateral_error.abs() < 0.05,
        "lateral error {}",
        after.1.lateral_error
    );

    // The belt carries a lane rider by its 1.5 m/s on top of the 5 m/s run.
    let belt: Vec<_> = lanes
        .iter()
        .filter(|(i, l)| {
            let r = hero[*i].report.as_ref().unwrap();
            r.grounded && r.platform.entity.as_deref() == Some("lane-belt") && l.weight == 1.0
        })
        .collect();
    assert!(belt.len() >= 20, "{} ticks on the lane belt", belt.len());
    // `ds` is the change of a nearest-point `s`, resolved to a few millimetres a tick; the mean over
    // the belt is exact.
    let expected = (5.0 + 1.5) / 60.0;
    for (i, l) in &belt {
        assert!(
            (l.ds - expected).abs() < 5e-3,
            "tick {}: ds {}",
            hero[*i].tick,
            l.ds
        );
    }
    let mean = belt.iter().map(|(_, l)| l.ds).sum::<f32>() / belt.len() as f32;
    assert!((mean - expected).abs() < 1e-3, "mean ds {mean}");

    // The jump clears the pit: landed on the far side after the respawn.
    assert!(
        hero[teleport..].iter().any(|m| {
            m.report.as_ref().is_some_and(|r| r.landed) && (40.0..42.0).contains(&m.feet[0])
        }),
        "no landing past the pit after the respawn"
    );
}
