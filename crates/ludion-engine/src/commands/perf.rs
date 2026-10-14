//! The `profile.capture` handler.
//!
//! Arming a capture is all this handler does. The *reply* it produces is a placeholder: a capture
//! measures live frames that have not happened yet, so the report cannot exist at dispatch time.
//! [`run_live`](crate::live::run_live) holds the request's reply channel open and answers it with the
//! finished [`ResponseBody::ProfileReport`] once the window closes — see `pending_capture` there.
//! Everything this handler can decide *now*, it decides now: an unmeasurable frame count (`L4012`)
//! and a capture already in flight (`L4013`) are refused before anything is armed, and so is a
//! window that cannot close at the current run state, speed and tick rate (`L4023`, see
//! [`cannot_close`], which [`run_live`](crate::live::run_live) re-asks while the capture is held).

use std::time::Duration;

use ludion_core::codes::code;
use ludion_core::diagnostics::Diagnostic;
use ludion_protocol::ResponseBody;
use ludion_protocol::perf::{MAX_CAPTURE_FRAMES, MAX_CAPTURE_WALL_SECONDS, ProfileCaptureParams};
use ludion_sim::RunState;

use crate::engine::Engine;

/// `profile.capture` — arm a capture over the next `frames` live frames.
///
/// Returns [`ResponseBody::Ack`] as the *internal* result; a live host replaces it with the report.
/// A host that dispatches this command without a live loop (a lockstep driver, a direct
/// [`dispatch`](crate::dispatch) call) gets the ack and an armed capture whose window will close but
/// whose report nobody takes — profiling frames a live loop is producing is the whole point of the
/// method.
///
/// # Errors
/// `L4012` if `frames` is zero or above [`MAX_CAPTURE_FRAMES`]; `L4023` if the sim is paused or the
/// window cannot close within [`MAX_CAPTURE_WALL_SECONDS`] at the current speed; `L4013` if a
/// capture is already in flight or its report has not been taken.
pub(crate) fn capture(
    engine: &mut Engine,
    params: ProfileCaptureParams,
) -> Result<ResponseBody, Diagnostic> {
    if params.frames == 0 || params.frames > MAX_CAPTURE_FRAMES {
        return Err(Diagnostic::new(
            code::PROFILE_CAPTURE_INVALID,
            format!(
                "profile.capture asked for {} frames; the range is 1..={MAX_CAPTURE_FRAMES}",
                params.frames
            ),
        )
        .with_path("/params/frames")
        .with_help(
            "ask for a frame count in range — 60 is one second at the default tick rate, and is \
             the default when `frames` is omitted; subscribe to the `profile` channel to watch \
             cost continuously instead of capturing a fixed window",
        ));
    }
    if let Some(diag) = cannot_close(
        engine,
        params.frames,
        Duration::from_secs_f64(MAX_CAPTURE_WALL_SECONDS),
        false,
    ) {
        return Err(diag);
    }
    if !engine.perf_begin_capture(params.frames) {
        return Err(Diagnostic::new(
            code::PROFILE_CAPTURE_BUSY,
            "a profile capture is already in flight",
        )
        .with_help(
            "wait for the in-flight capture's reply — it arrives when its window closes — before \
             issuing the next one; any number of clients can subscribe to the `profile` channel at \
             once if a running picture is what is wanted",
        ));
    }
    Ok(ResponseBody::Ack)
}

/// `None` if `remaining` frames can close within `budget_left` at the engine's current run-state,
/// speed and tick rate; otherwise the `L4023` diagnostic saying why. `armed` is whether the capture
/// is already in flight (the message then says it is cancelled, and names no request parameter).
pub(crate) fn cannot_close(
    engine: &Engine,
    remaining: u64,
    budget_left: Duration,
    armed: bool,
) -> Option<Diagnostic> {
    let help = "resume the sim (`sim.resume`), capture, then `sim.pause` again; raise the speed \
                with `sim.set_speed`; ask for fewer frames; or subscribe to the `profile` channel \
                to watch cost without a fixed window";
    let cancelled = if armed {
        "; the capture is cancelled"
    } else {
        ""
    };
    if engine.run_state() == RunState::Paused {
        let when = if armed {
            "was paused before the window closed"
        } else {
            "the sim is paused, so the capture's window can never close"
        };
        // Paused is not a frames problem, so it carries no request path even when arming.
        let diag = Diagnostic::new(
            code::PROFILE_CAPTURE_CANNOT_CLOSE,
            format!("profile.capture {when}{cancelled}"),
        )
        .with_help(help);
        return Some(diag);
    }
    let seconds_per_tick = engine.seconds_per_tick();
    let speed = engine.speed().factor();
    let budget = budget_left.as_secs_f64();
    // The epsilon absorbs float error at the exact boundary (3600 frames at 60 Hz is 60 s).
    let fits = (budget * speed / seconds_per_tick + 1e-6).floor() as u64;
    if remaining <= fits {
        return None;
    }
    let needed = remaining as f64 * seconds_per_tick / speed;
    let diag = Diagnostic::new(
        code::PROFILE_CAPTURE_CANNOT_CLOSE,
        format!(
            "profile.capture needs {needed:.1} s of wall time for {remaining} frames at speed \
             {speed}, over the {budget:.1} s left; at this speed and tick rate {fits} frames fit\
             {cancelled}"
        ),
    )
    .with_help(help);
    Some(if armed {
        diag
    } else {
        diag.with_path("/params/frames")
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use ludion_core::codes::code;

    fn engine() -> Engine {
        let mut engine = Engine::new();
        engine
            .load_inline_scene("format: 1\nseed: 7\nentities: []\n")
            .unwrap();
        engine
    }

    fn params(frames: u64) -> ProfileCaptureParams {
        ProfileCaptureParams {
            frames,
            include_samples: false,
        }
    }

    #[test]
    fn an_unmeasurable_frame_count_is_refused_without_arming() {
        let mut engine = engine();
        for frames in [0, MAX_CAPTURE_FRAMES + 1] {
            let diag = capture(&mut engine, params(frames)).expect_err("refused");
            assert_eq!(diag.code, code::PROFILE_CAPTURE_INVALID);
            assert_eq!(diag.path.as_deref(), Some("/params/frames"));
        }
        assert!(
            !engine.perf_armed(),
            "a refused capture must leave the profiler exactly as it found it"
        );
    }

    #[test]
    fn a_capture_armed_while_paused_is_refused_without_arming() {
        let mut engine = engine();
        engine.driver_mut().pause();
        let diag = capture(&mut engine, params(60)).expect_err("refused");
        assert_eq!(diag.code, code::PROFILE_CAPTURE_CANNOT_CLOSE);
        assert!(!engine.perf_capture_in_flight());
        assert!(!engine.perf_armed());
    }

    #[test]
    fn a_window_too_long_for_the_speed_is_refused_naming_the_largest_that_fits() {
        let mut engine = engine();
        engine
            .driver_mut()
            .set_speed(ludion_sim::Speed::new(1.0 / 64.0).unwrap());
        // 60 s x 60 Hz x 1/64 = 56.25 frames.
        let diag = capture(&mut engine, params(57)).expect_err("refused");
        assert_eq!(diag.code, code::PROFILE_CAPTURE_CANNOT_CLOSE);
        assert_eq!(diag.path.as_deref(), Some("/params/frames"));
        assert!(diag.message.contains("56"), "{}", diag.message);
        assert!(!engine.perf_armed());
        capture(&mut engine, params(56)).expect("56 frames fit");
    }

    #[test]
    fn the_longest_capture_at_normal_speed_still_arms() {
        let mut engine = engine();
        capture(&mut engine, params(MAX_CAPTURE_FRAMES)).expect("3600 frames is exactly 60 s");
    }

    #[test]
    fn a_second_capture_is_busy_until_the_first_report_is_taken() {
        let mut engine = engine();
        capture(&mut engine, params(4)).expect("the first capture arms");
        assert!(engine.perf_armed());

        let diag = capture(&mut engine, params(4)).expect_err("the second is refused");
        assert_eq!(diag.code, code::PROFILE_CAPTURE_BUSY);

        // Still busy once the window has closed — the report has not been handed back yet.
        for _ in 0..16 {
            engine.advance_live();
        }
        let diag = capture(&mut engine, params(4)).expect_err("still busy with an untaken report");
        assert_eq!(diag.code, code::PROFILE_CAPTURE_BUSY);

        // Taking it frees the slot.
        assert!(engine.perf_take_capture().is_some());
        capture(&mut engine, params(4)).expect("the next capture arms");
    }
}
