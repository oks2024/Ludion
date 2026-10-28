//! The scripted route as a numeric trace: two runs print the same bytes, and the hero's records
//! show each element of the course doing its job. Compared run to run, never against a committed
//! trace.

mod common;

use std::path::PathBuf;

use common::{FRAMES, ROUTE};
use ludion_core::time::FrameIndex;
use ludion_engine::{MotorRecord, TraceChannel, TraceRecord, trace_with, write_jsonl};
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
    assert!(
        reports.iter().all(|(_, feet, _)| feet[1] > -1.0),
        "the hero fell out of the course"
    );

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
    assert!(last.1[0] >= 25.0, "the route ends at x = {}", last.1[0]);
}
