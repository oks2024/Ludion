//! A one-call text trace of a sequence run (`ludion docs sequences`).
//!
//! [`sequence_trace`] steps an engine and returns what its sequences did, one frame-stamped line
//! per thing: every key applied, event fired and end, plus each wait the playhead parks on and
//! resumes from. Text, so a regression is a diff; integer ticks, so it holds on every machine.
//!
//! Line grammar (`{player}` is the sequence's entity, `e7v0`; the tick is the frame the step ran
//! from, zero-padded to four digits):
//!
//! ```text
//! 0005 e7v0 key camera at 5
//! 0005 e7v0 fire radar_online
//! 0030 e7v0 wait line_end at 30
//! 0060 e7v0 resume at 30
//! 0070 e7v0 end completed
//! ```

use std::collections::BTreeMap;

use ludion_core::ids::EntityId;
use ludion_engine::sequences::{
    PendingWait, SequenceEnd, SequenceEvent, SequencePlayer, SequenceTrack, WaitKind,
};
use ludion_engine::{Engine, SequenceEvents};

/// Steps `engine` `ticks` ticks and returns the sequence log, in order: per tick, every
/// [`SequenceEvents`] entry as it happened, then wait transitions by ascending player id.
///
/// A wait parks the playhead until its line ends or the game releases it; a wait is a state, so it
/// is logged as the tick it began (`wait`) and the tick it cleared (`resume`). The waits the
/// sequences already hold are read from the world first, so `trace(0..k)` followed by `trace(k..n)`
/// equals `trace(0..n)`, also across a snapshot restore. An engine that did not opt into
/// `.sequences()` steps and returns an empty log.
///
/// # Panics
/// If the sim step fails.
///
/// ```no_run
/// # use ludion_engine::Engine;
/// # let mut engine = Engine::new();
/// let lines = ludion_testkit::sequence::sequence_trace(&mut engine, 120);
/// // assert_eq!(lines[0], "0000 e3v0 key camera at 0");
/// ```
pub fn sequence_trace(engine: &mut Engine, ticks: u64) -> Vec<String> {
    let mut parked = waits(engine);
    let mut lines = Vec::new();
    for _ in 0..ticks {
        let tick = engine.frame().as_u64();
        crate::driver::step(engine, 1);
        let Some(events) = engine.world().resource::<SequenceEvents>() else {
            continue;
        };
        for event in events.iter() {
            lines.push(match event {
                SequenceEvent::KeyApplied { handle, track, at } => {
                    format!("{tick:04} {} key {} at {at}", handle.0, track_name(*track))
                }
                SequenceEvent::Fired { handle, name } => {
                    format!("{tick:04} {} fire {name}", handle.0)
                }
                SequenceEvent::Ended { handle, end } => {
                    format!("{tick:04} {} end {}", handle.0, end_name(*end))
                }
            });
        }
        let now = waits(engine);
        parked.retain(|player, _| now.contains_key(player) || alive(engine, *player));
        for (player, wait) in &now {
            let before = parked.get(player);
            if before == Some(wait) {
                continue;
            }
            if let Some(old) = before {
                lines.push(format!("{tick:04} {player} resume at {}", old.at));
            }
            lines.push(format!(
                "{tick:04} {player} wait {} at {}",
                wait_name(wait.kind),
                wait.at
            ));
        }
        for (player, old) in &parked {
            if !now.contains_key(player) {
                lines.push(format!("{tick:04} {player} resume at {}", old.at));
            }
        }
        parked = now;
    }
    lines
}

/// The wait each live sequence player is parked on, by player.
fn waits(engine: &Engine) -> BTreeMap<EntityId, PendingWait> {
    let world = engine.world();
    world
        .entities()
        .filter_map(|id| {
            let player = world.get::<SequencePlayer>(id).ok()??;
            Some((id, player.wait.clone()?))
        })
        .collect()
}

/// Whether `player` still carries a `SequencePlayer`: a parked wait that cleared on a live player is
/// a resume, one on a player that ended is not.
fn alive(engine: &Engine, player: EntityId) -> bool {
    matches!(engine.world().get::<SequencePlayer>(player), Ok(Some(_)))
}

fn track_name(track: SequenceTrack) -> &'static str {
    match track {
        SequenceTrack::Camera => "camera",
        SequenceTrack::Transform => "transform",
        SequenceTrack::Animation => "animation",
        SequenceTrack::Ui => "ui",
        SequenceTrack::Audio => "audio",
        SequenceTrack::Dialogue => "dialogue",
        SequenceTrack::Event => "event",
    }
}

fn end_name(end: SequenceEnd) -> &'static str {
    match end {
        SequenceEnd::Completed => "completed",
        SequenceEnd::Skipped => "skipped",
        SequenceEnd::Cancelled => "cancelled",
    }
}

fn wait_name(kind: WaitKind) -> &'static str {
    match kind {
        WaitKind::LineEnd { .. } => "line_end",
        WaitKind::Release => "release",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ludion_engine::CameraRig;
    use ludion_engine::sequences;

    const BEAT: &str = "format: 1
kind: sequence
roles:
  - cam
length_ticks: 12
tracks:
  camera:
    - at: 1
      role: cam
      rig:
        focus_offset:
          - 0.0
          - 2.0
          - 0.0
  event:
    - at: 3
      name: go
      wait: release
    - at: 5
      name: later
";

    /// An engine with the beat playing, nothing stepped yet; the handle's entity prints as `player`.
    fn engine(name: &str) -> (Engine, EntityId) {
        let dir =
            std::env::temp_dir().join(format!("ludion-testkit-seq-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let mut engine = Engine::builder()
            .project_root(dir)
            .sequences()
            .doc_text("sequences/beat.yaml", BEAT)
            .build()
            .expect("the beat is valid");
        engine
            .load_inline_scene("format: 1\nseed: 1\nentities: []\n")
            .unwrap();
        let world = engine.world_mut();
        let cam = world.spawn();
        world.insert(cam, CameraRig::default()).unwrap();
        let handle = sequences::play(world, "sequences/beat.yaml", &[("cam", cam)]).unwrap();
        (engine, handle.0)
    }

    fn release(engine: &mut Engine) {
        let handle = {
            let world = engine.world();
            let id = world
                .entities()
                .find(|id| matches!(world.get::<SequencePlayer>(*id), Ok(Some(_))))
                .expect("a live player");
            sequences::SequenceHandle(id)
        };
        assert!(sequences::release(engine.world_mut(), handle));
    }

    #[test]
    fn the_trace_logs_keys_fires_waits_resumes_and_the_end() {
        let (mut engine, player) = engine("grammar");
        let mut lines = sequence_trace(&mut engine, 8);
        release(&mut engine);
        lines.extend(sequence_trace(&mut engine, 12));
        let p = player.to_string();
        let expected = [
            format!("0001 {p} key camera at 1"),
            format!("0003 {p} key event at 3"),
            format!("0003 {p} fire go"),
            format!("0003 {p} wait release at 3"),
        ];
        assert_eq!(&lines[..4], &expected, "{lines:#?}");
        let tail = &lines[4..];
        assert!(tail[0].ends_with(&format!("{p} resume at 3")), "{lines:#?}");
        assert!(tail.iter().any(|l| l.ends_with("fire later")), "{lines:#?}");
        assert!(
            tail.last()
                .unwrap()
                .ends_with(&format!("{p} end completed")),
            "{lines:#?}"
        );
    }

    #[test]
    fn two_calls_concatenate_to_one() {
        let whole = {
            let (mut engine, _) = engine("whole");
            let mut lines = sequence_trace(&mut engine, 8);
            release(&mut engine);
            lines.extend(sequence_trace(&mut engine, 12));
            lines
        };
        // Split mid-wait (tick 5 sits inside the park that began at tick 3).
        let (mut engine, _) = engine("split");
        let mut lines = sequence_trace(&mut engine, 5);
        lines.extend(sequence_trace(&mut engine, 3));
        release(&mut engine);
        lines.extend(sequence_trace(&mut engine, 12));
        assert_eq!(lines, whole);
    }

    #[test]
    fn an_engine_without_sequences_traces_nothing() {
        let mut engine = Engine::new();
        assert!(sequence_trace(&mut engine, 3).is_empty());
    }
}
