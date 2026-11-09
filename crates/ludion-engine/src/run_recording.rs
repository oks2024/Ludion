//! In-game run recording: a game system starts and stops an ordinary `.lrp` recording of one run.
//!
//! A game system calls [`RunRecording::start`] or [`RunRecording::stop`]; the engine applies the
//! request at a free-running frame boundary, in a persisting host only (the [`RunSave`](crate::RunSave)
//! mold). A start makes the run a fresh session: the current scene's inline text is reloaded into
//! frame 0 (only the [`Profile`](crate::Profile) and the run-save state carry across), then the
//! ordinary recorder is armed, so the file replays from its own header like any recording. The
//! resource is undigested, invisible to the `WorldHash` and to snapshots.

use std::collections::VecDeque;
use std::path::PathBuf;

use ludion_core::diagnostics::Diagnostic;
use ludion_core::time::FrameIndex;

/// The most [`RunRecordingEntry`]s the log keeps, the oldest dropped first.
pub(crate) const RUN_RECORDING_LOG_CAPACITY: usize = 4096;

/// What [`EngineBuilder::run_recordings`](crate::EngineBuilder::run_recordings) configures.
///
/// # Example
/// ```
/// use ludion_engine::RunRecordingConfig;
///
/// let config = RunRecordingConfig::default();
/// assert_eq!((config.dir.as_str(), config.keyframe_every), ("save/replays", 3600));
/// ```
#[derive(Clone, Debug, PartialEq)]
pub struct RunRecordingConfig {
    /// Where finished recordings go, relative to the save root.
    pub dir: String,
    /// Ticks between keyframes (at least 1). A larger interval is a smaller file.
    pub keyframe_every: u64,
}

impl Default for RunRecordingConfig {
    fn default() -> Self {
        Self {
            dir: "save/replays".to_string(),
            keyframe_every: 3600,
        }
    }
}

/// One run-recording request a game system made.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RunRecordingRequest {
    /// [`RunRecording::start`] with this name.
    Start(String),
    /// [`RunRecording::stop`].
    Stop,
}

/// One drained request, stamped with the frame whose systems made it (before any reset).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RunRecordingEntry {
    /// The frame whose systems asked.
    pub frame: FrameIndex,
    /// What was asked.
    pub request: RunRecordingRequest,
}

/// The file being written for an active run recording.
pub(crate) struct ActiveRun {
    pub(crate) name: String,
    pub(crate) temp: PathBuf,
    pub(crate) path: PathBuf,
}

/// The engine's persistent run-recording state; it outlives every world rebuild and the
/// [`RunRecording`] resource mirrors it.
pub(crate) struct RunRecordingState {
    pub(crate) config: RunRecordingConfig,
    pub(crate) active: Option<ActiveRun>,
    pub(crate) last_finished: Option<String>,
    pub(crate) log: VecDeque<RunRecordingEntry>,
    pub(crate) error: Option<Diagnostic>,
}

/// The start/stop handle a game system holds, installed by
/// [`EngineBuilder::run_recordings`](crate::EngineBuilder::run_recordings).
///
/// Requests are applied at the end of a free-running tick, so a request made during a lockstep
/// `sim.step` waits for the next free-running boundary, and the last `start` in a frame wins. Only a
/// persisting host resets the run and writes the file; a test without `persist_profile` or a replay
/// only fills the [log](crate::Engine::run_recording_log). A `start` while a `--record` session is
/// running is refused with `L1402`.
///
/// # Example
/// ```
/// use ludion_engine::{Engine, RunRecording, RunRecordingConfig};
///
/// let engine = Engine::builder()
///     .run_recordings(RunRecordingConfig::default())
///     .build()
///     .unwrap();
/// assert!(engine.world().resource::<RunRecording>().unwrap().active().is_none());
/// ```
pub struct RunRecording {
    active: Option<String>,
    last_finished: Option<String>,
    pub(crate) start: Option<String>,
    pub(crate) stop: bool,
}

impl RunRecording {
    /// Requests a recording named `name` (`<save_root>/<dir>/<name>.lrp`): the run restarts from the
    /// current scene's text at frame 0 and records from there. A recording already active finishes
    /// first.
    pub fn start(&mut self, name: impl Into<String>) {
        self.start = Some(name.into());
    }

    /// Requests that the active recording be closed and published.
    pub fn stop(&mut self) {
        self.stop = true;
    }

    /// The name of the recording in progress.
    pub fn active(&self) -> Option<&str> {
        self.active.as_deref()
    }

    /// The name of the last recording that finished.
    pub fn last_finished(&self) -> Option<&str> {
        self.last_finished.as_deref()
    }

    pub(crate) fn mirror(state: &RunRecordingState) -> Self {
        RunRecording {
            active: state.active.as_ref().map(|run| run.name.clone()),
            last_finished: state.last_finished.clone(),
            start: None,
            stop: false,
        }
    }
}

/// `Ok` for a name safe as a file stem: 1–128 of `[A-Za-z0-9._-]`, not starting with `.`.
pub(crate) fn validate_name(name: &str) -> Result<(), String> {
    if name.is_empty() || name.len() > 128 {
        return Err(format!(
            "the recording name must be 1 to 128 characters, not {}",
            name.chars().count()
        ));
    }
    if name.starts_with('.') {
        return Err(format!(
            "the recording name `{name}` must not start with `.`"
        ));
    }
    if let Some(bad) = name
        .chars()
        .find(|c| !(c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-')))
    {
        return Err(format!(
            "the recording name `{name}` holds `{bad}`; use only A-Z, a-z, 0-9, `.`, `_` and `-`"
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::validate_name;

    #[test]
    fn plain_names_pass() {
        for name in ["run-1", "A.b_c", "x", &"a".repeat(128)] {
            assert!(validate_name(name).is_ok(), "{name}");
        }
    }

    #[test]
    fn unsafe_names_are_refused() {
        for name in ["", ".hidden", "a/b", "a b", "..", "é", &"a".repeat(129)] {
            assert!(validate_name(name).is_err(), "{name}");
        }
    }
}
