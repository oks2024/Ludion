//! Bounded reconnect + frame-streaming, shared by the window and driveable headlessly.
//!
//! A lost connection is **not** fatal here: the blink at the heart of `ludion dev` (snapshot →
//! shutdown → relaunch on the same port → restore) drops the socket for a second or two, and the
//! viewer is supposed to ride it out — hold the last frame, flag the window title, and re-subscribe
//! the moment the freshly relaunched child is listening again. [`run_frame_stream`] is that loop,
//! factored out of the window's stream thread so it can be exercised without opening a window (the
//! integration test drives it against a real `ludion serve` it kills and restarts).
//!
//! # Bounds (the build-vs-blink reasoning)
//!
//! Retries start quick (the same-port blink is ~1–3 s) and back off, giving up only after
//! [`GIVE_UP_BUDGET`] of *continuous* failure — a genuinely dead server, not a reload. This budget
//! is deliberately about **connection** duration, not build duration: during a long rebuild (even a
//! cold one over 30 s) the *old* child is still serving, so the stream never drops and the budget is
//! never touched; the socket only drops for the seconds between the old child's shutdown and the new
//! child's listen. So a slow build and a short give-up budget do not fight — the window gives up only
//! when nothing is listening for [`GIVE_UP_BUDGET`], which under `ludion dev` means the supervisor
//! itself failed, not that a build is slow.
//!
//! A schema/version mismatch is the one connect error treated as fatal on sight ([`is_fatal`]): it
//! means a *different engine build* is now on the port, never a transient blink, so retrying would
//! only mask it.

use std::time::Duration;

use ludion_client::Client;
use ludion_core::codes::code;
use ludion_core::diagnostics::Diagnostic;
use ludion_protocol::frame::FrameFormat;
use ludion_protocol::{Channel, SubId, SubscribeMode};

use crate::frame::{DecodedFrame, decode_frame};

/// The first reconnect delay — short, because the same-port blink usually clears in ~1–3 s.
const INITIAL_RETRY_DELAY: Duration = Duration::from_millis(100);
/// The ceiling each backoff step approaches, so late retries stay responsive without busy-spinning.
const MAX_RETRY_DELAY: Duration = Duration::from_secs(2);
/// Total cumulative retry wait before giving up and surfacing the connection as permanently lost.
/// See the module docs: this bounds *connection outage*, not build time.
const GIVE_UP_BUDGET: Duration = Duration::from_secs(30);
/// How long one streaming read (or one slice of a backoff sleep) blocks before re-checking the
/// caller's stop signal, so teardown never waits longer than this on a quiet stream or a long sleep.
const STREAM_POLL: Duration = Duration::from_millis(200);

/// An exponential backoff with a give-up budget. Pure and clock-free (it only produces the *sequence*
/// of sleep durations and decides when to stop), so the schedule is unit-testable without sleeping.
pub(crate) struct Backoff {
    /// The delay the next failure will hand out.
    delay: Duration,
    /// Cumulative delay already handed out this outage — compared against [`GIVE_UP_BUDGET`].
    spent: Duration,
}

impl Backoff {
    /// A fresh schedule, starting at [`INITIAL_RETRY_DELAY`] with an empty spend.
    pub(crate) fn new() -> Backoff {
        Backoff {
            delay: INITIAL_RETRY_DELAY,
            spent: Duration::ZERO,
        }
    }

    /// The delay to wait before the next reconnect attempt, or `None` once the cumulative wait has
    /// reached [`GIVE_UP_BUDGET`] — at which point the caller treats the connection as permanently
    /// lost. Each call doubles the delay (capped at [`MAX_RETRY_DELAY`]) and adds it to the spend.
    pub(crate) fn next_delay(&mut self) -> Option<Duration> {
        if self.spent >= GIVE_UP_BUDGET {
            return None;
        }
        let delay = self.delay;
        self.spent += delay;
        self.delay = (self.delay * 2).min(MAX_RETRY_DELAY);
        Some(delay)
    }

    /// Resets the schedule after a successful (re)connect, so a later outage gets the full budget
    /// and the quick first retries again.
    pub(crate) fn reset(&mut self) {
        self.delay = INITIAL_RETRY_DELAY;
        self.spent = Duration::ZERO;
    }
}

/// Whether a connect-time diagnostic is a *fatal* mismatch (a different engine build on the port)
/// rather than a transient outage worth retrying. Only protocol/schema-drift qualifies; a plain
/// "engine unavailable" (`L3009`) is the blink, and everything else is treated as retryable too.
pub(crate) fn is_fatal(diagnostic: &Diagnostic) -> bool {
    diagnostic.code == code::PROTOCOL_UNSUPPORTED_VERSION
}

/// The side effects the reconnecting frame stream performs, abstracted so [`run_frame_stream`] can be
/// driven either by the window (push frames into the shared slot, wake the event loop, flag the
/// title) or by a headless test (record what happened). Every method is called from the stream
/// thread, never the main thread.
pub trait StreamSink {
    /// A freshly decoded frame arrived — take ownership (no copy).
    fn frame(&self, frame: DecodedFrame);
    /// The connection is down and the loop is now retrying: hold the last frame and flag the visible
    /// state (the window's title). May be called repeatedly across one outage; treat it idempotently.
    fn reconnecting(&self);
    /// The connection was (re)established and frames are resuming: clear the reconnect flag.
    fn reconnected(&self);
    /// Whether the loop should keep running. Returning `false` asks it to stop promptly (within one
    /// streaming-poll interval); it then unsubscribes best-effort and returns `None`.
    fn keep_running(&self) -> bool;
}

/// Runs the reconnecting frame stream against `url`, driving `sink`, until `sink.keep_running()`
/// goes false (returns `None`, a clean stop) or the connection is *permanently* lost — the give-up
/// budget is exhausted, or a fatal schema mismatch is seen (returns `Some(diagnostic)`).
///
/// On every successful (re)establish it resets the backoff and calls [`StreamSink::reconnected`]; on
/// a drop it silently re-establishes (flagging [`StreamSink::reconnecting`] only once it actually has
/// to wait, so a momentary reconnect never flickers the title).
pub fn run_frame_stream(url: &str, sink: &impl StreamSink) -> Option<Diagnostic> {
    let mut backoff = Backoff::new();
    'reconnect: loop {
        if !sink.keep_running() {
            return None;
        }
        // Establish: connect (which runs the schema handshake) + subscribe to frames.
        let (mut client, sub) = match connect_and_subscribe(url) {
            Ok(pair) => pair,
            Err(diagnostic) => {
                if is_fatal(&diagnostic) {
                    return Some(diagnostic); // a different engine build: never a retry.
                }
                sink.reconnecting();
                match backoff.next_delay() {
                    Some(delay) => {
                        interruptible_sleep(delay, sink);
                        continue 'reconnect;
                    }
                    None => return Some(diagnostic), // gave up after the outage budget.
                }
            }
        };
        backoff.reset();
        sink.reconnected();

        // Stream frames until the connection drops or a stop is requested.
        loop {
            if !sink.keep_running() {
                let _ = client.unsubscribe(sub); // best-effort on the clean way out
                return None;
            }
            match client.next_frame(STREAM_POLL) {
                Ok(Some(received)) => {
                    // A malformed frame is dropped; latest-wins means the next good one lands.
                    if let Ok(decoded) = decode_frame(&received) {
                        sink.frame(decoded);
                    }
                }
                Ok(None) => {} // timeout: loop to re-check the stop signal
                Err(_dropped) => continue 'reconnect, // connection lost: go re-establish
            }
        }
    }
}

/// The frame codec to subscribe with for a server at `url`, chosen by where the server is.
///
/// **Loopback gets [`FrameFormat::RawRgba`]**: over a local socket the payload is effectively free
/// while codec time is not, and raw is the only format that costs nothing at *both* ends — the
/// server's "encode" is one copy and the viewer's "decode" is none. Everything else gets
/// [`FrameFormat::Qoi`], because raw is 3.7 MB a frame at 720p (8.3 at 1080p) and dead on a real
/// network. QOI rather than LZ4 for that remote case: measured on engine content it produces the
/// smaller payload *and* the cheaper encode, so LZ4 is opt-in only, never a default.
///
/// The parse is deliberately shallow — host up to the port, matched against the three loopback
/// spellings. A hostname that resolves to a loopback address by other means (an `/etc/hosts` alias)
/// reads as remote and merely gets QOI: the cost of guessing wrong here is a codec, never a failure.
pub(crate) fn default_stream_format(url: &str) -> FrameFormat {
    // Strip the scheme, then the path/query, leaving `host[:port]` (possibly with userinfo).
    let after_scheme = url.split_once("://").map_or(url, |(_, rest)| rest);
    let authority = after_scheme
        .split(['/', '?', '#'])
        .next()
        .unwrap_or(after_scheme);
    let host_port = authority.rsplit_once('@').map_or(authority, |(_, h)| h);
    // A bracketed IPv6 literal keeps its brackets; otherwise the port is whatever follows the colon.
    let host = match host_port.strip_prefix('[') {
        Some(rest) => rest.split_once(']').map_or(host_port, |(h, _)| h),
        None => host_port.split_once(':').map_or(host_port, |(h, _)| h),
    };
    if host.eq_ignore_ascii_case("localhost") || host == "127.0.0.1" || host == "::1" {
        FrameFormat::RawRgba
    } else {
        FrameFormat::Qoi
    }
}

/// Connects and opens a latest-wins `frames` subscription, the pair the stream loop needs. The
/// codec comes from [`default_stream_format`] — there is no negotiation to do (the window and the
/// server it talks to ship from the same repo in lockstep), only the where-is-the-server question —
/// and [`decode_frame`] handles every format regardless of which one was asked for.
fn connect_and_subscribe(url: &str) -> Result<(Client, SubId), Diagnostic> {
    let mut client = Client::connect(url)?;
    let sub = client.subscribe(
        Channel::Frames,
        SubscribeMode::LatestWins { max_fps: None },
        default_stream_format(url),
    )?;
    Ok((client, sub))
}

/// Sleeps up to `total`, in [`STREAM_POLL`] slices, returning early if the sink asks the loop to
/// stop — so a backoff wait never delays teardown by more than one slice.
fn interruptible_sleep(total: Duration, sink: &impl StreamSink) {
    let mut remaining = total;
    while remaining > Duration::ZERO {
        if !sink.keep_running() {
            return;
        }
        let chunk = remaining.min(STREAM_POLL);
        std::thread::sleep(chunk);
        remaining = remaining.saturating_sub(chunk);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backoff_starts_quick_then_doubles_to_the_ceiling() {
        let mut backoff = Backoff::new();
        assert_eq!(backoff.next_delay(), Some(Duration::from_millis(100)));
        assert_eq!(backoff.next_delay(), Some(Duration::from_millis(200)));
        assert_eq!(backoff.next_delay(), Some(Duration::from_millis(400)));
        assert_eq!(backoff.next_delay(), Some(Duration::from_millis(800)));
        assert_eq!(backoff.next_delay(), Some(Duration::from_millis(1600)));
        // Capped at the ceiling from here on.
        assert_eq!(backoff.next_delay(), Some(MAX_RETRY_DELAY));
        assert_eq!(backoff.next_delay(), Some(MAX_RETRY_DELAY));
    }

    #[test]
    fn backoff_gives_up_after_the_budget_and_covers_a_multi_second_blink_first() {
        let mut backoff = Backoff::new();
        let mut total = Duration::ZERO;
        let mut steps = 0u32;
        while let Some(delay) = backoff.next_delay() {
            total += delay;
            steps += 1;
            assert!(steps < 100, "backoff must terminate well within 100 steps");
        }
        // It kept retrying until at least the give-up budget of cumulative wait.
        assert!(
            total >= GIVE_UP_BUDGET,
            "must retry for at least the give-up budget, got {total:?}",
        );
        // And the first few retries alone cover a several-second same-port blink.
        let mut quick = Backoff::new();
        let mut early = Duration::ZERO;
        for _ in 0..5 {
            early += quick.next_delay().expect("early retries exist");
        }
        assert!(
            early >= Duration::from_secs(3),
            "the first five retries should span a multi-second blink, got {early:?}",
        );
    }

    #[test]
    fn reset_restores_the_full_budget_and_quick_start() {
        let mut backoff = Backoff::new();
        // Burn most of the budget.
        for _ in 0..30 {
            backoff.next_delay();
        }
        backoff.reset();
        assert_eq!(
            backoff.next_delay(),
            Some(INITIAL_RETRY_DELAY),
            "after a successful reconnect the schedule starts over",
        );
    }

    #[test]
    fn loopback_urls_stream_raw_and_everything_else_streams_qoi() {
        for url in [
            "ws://127.0.0.1:9411",
            "ws://localhost:9411",
            "ws://LocalHost:9411/stream",
            "ws://[::1]:9411",
            "ws://127.0.0.1",
        ] {
            assert_eq!(
                default_stream_format(url),
                FrameFormat::RawRgba,
                "{url} is loopback: raw is free at both ends there",
            );
        }
        for url in [
            "ws://203.0.113.5:9411",
            "ws://myhost:9411",
            "wss://viewer.example.com:9411/stream",
            "ws://127.0.0.1.example.com:9411",
        ] {
            assert_eq!(
                default_stream_format(url),
                FrameFormat::Qoi,
                "{url} is remote: 8 MB a frame is not sendable, so a codec is",
            );
        }
    }

    #[test]
    fn schema_mismatch_is_fatal_but_a_lost_connection_is_not() {
        let mismatch = Diagnostic::new(code::PROTOCOL_UNSUPPORTED_VERSION, "schema drift");
        let lost = Diagnostic::new(code::PROTOCOL_ENGINE_UNAVAILABLE, "server gone");
        assert!(is_fatal(&mismatch), "a schema mismatch must not be retried");
        assert!(
            !is_fatal(&lost),
            "a lost connection is the blink — retry it"
        );
    }
}
