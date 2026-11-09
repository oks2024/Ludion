//! In-window replay playback: a game system plays a recorded `.lrp` on its own live engine.
//!
//! A game system calls [`ReplayPlayback::play`]; the engine applies the request at a free-running
//! frame boundary, in a persisting host only (the [`RunRecording`](crate::RunRecording) mold). The
//! live engine becomes the replay's frame 0 in place, the same setup a fresh replay engine gets, and
//! every later live tick performs one tick of the shared replay driver. When the file ends, or on
//! [`Engine::stop_playback`](crate::Engine::stop_playback), the engine reloads the `return_to` scene
//! and carries the pre-play live profile with it. The resource is undigested, invisible to the
//! `WorldHash` and to snapshots.

use std::collections::BTreeMap;
use std::path::PathBuf;

use ludion_core::diagnostics::Diagnostic;
use ludion_core::time::FrameIndex;
use ludion_sim::Speed;

use crate::docs::InlineDoc;
use crate::replay::ActionCursor;

/// What [`EngineBuilder::replay_playback`](crate::EngineBuilder::replay_playback) configures.
///
/// # Example
/// ```
/// use ludion_engine::ReplayPlaybackConfig;
///
/// assert_eq!(ReplayPlaybackConfig::default().dir, "save/replays");
/// ```
#[derive(Clone, Debug, PartialEq)]
pub struct ReplayPlaybackConfig {
    /// Where recordings live, relative to the save root. The default matches
    /// [`RunRecordingConfig`](crate::RunRecordingConfig), so both use one folder.
    pub dir: String,
}

impl Default for ReplayPlaybackConfig {
    fn default() -> Self {
        Self {
            dir: "save/replays".to_string(),
        }
    }
}

/// How the last playback ended.
#[derive(Clone, Debug, PartialEq)]
pub enum PlaybackOutcome {
    /// The file ran to its end after `frames` ticks.
    Finished {
        /// The recording played.
        name: String,
        /// Ticks the replay ran.
        frames: u64,
    },
    /// [`Engine::stop_playback`](crate::Engine::stop_playback) ended it at replay frame `frame`.
    Stopped {
        /// The recording played.
        name: String,
        /// The replay frame it stopped at.
        frame: FrameIndex,
    },
    /// The request was refused before anything changed: `L1405`, or `L3010` / `L1404` for a file
    /// this build cannot replay.
    Refused {
        /// The recording asked for.
        name: String,
        /// Why.
        diagnostic: Diagnostic,
    },
    /// The replay hit a driver error and the engine returned to the `return_to` scene.
    Failed {
        /// The recording played.
        name: String,
        /// Why.
        diagnostic: Diagnostic,
    },
}

/// One recording in the replays directory, as [`ReplayPlayback::entries`] lists it.
#[derive(Clone, Debug, PartialEq)]
pub struct ReplayEntry {
    /// The file stem: what [`ReplayPlayback::play`] takes.
    pub name: String,
    /// The file size in bytes.
    pub bytes: u64,
    /// Why this build cannot play it (`L3010`, `L1404`), or `None` if it can.
    pub refusal: Option<Diagnostic>,
}

/// The play/list/delete handle a game system holds, installed by
/// [`EngineBuilder::replay_playback`](crate::EngineBuilder::replay_playback).
///
/// Requests are applied at the end of a free-running tick, so a request made during a lockstep
/// `sim.step` waits for the next free-running boundary, and the last `play` in a frame wins. Only a
/// persisting host plays, lists or deletes; a test without `persist_profile`, or a replayed world, only
/// logs them. [`entries`](Self::entries) is disk input a replay never sees: read it from a system that
/// is not part of the replayed sim, or only to present a menu.
///
/// # Example
/// ```
/// use ludion_engine::{Engine, ReplayPlayback, ReplayPlaybackConfig};
///
/// let engine = Engine::builder()
///     .replay_playback(ReplayPlaybackConfig::default())
///     .build()
///     .unwrap();
/// let playback = engine.world().resource::<ReplayPlayback>().unwrap();
/// assert!(playback.playing().is_none() && playback.entries().is_empty());
/// ```
pub struct ReplayPlayback {
    playing: Option<String>,
    last_outcome: Option<PlaybackOutcome>,
    entries: Vec<ReplayEntry>,
    pub(crate) play: Option<(String, String)>,
    pub(crate) refresh: bool,
    pub(crate) delete: Vec<String>,
}

impl ReplayPlayback {
    /// Requests that `<save_root>/<dir>/<name>.lrp` play, then return to the scene file `return_to`
    /// (relative to the content root). A refused file leaves the world untouched; read the verdict in
    /// [`last_outcome`](Self::last_outcome).
    pub fn play(&mut self, name: impl Into<String>, return_to: impl Into<String>) {
        self.play = Some((name.into(), return_to.into()));
    }

    /// Requests a fresh listing of the replays directory into [`entries`](Self::entries).
    pub fn refresh(&mut self) {
        self.refresh = true;
    }

    /// Requests that `<name>.lrp` be deleted. Deletes apply before a refresh, which applies before a
    /// play.
    pub fn delete(&mut self, name: impl Into<String>) {
        self.delete.push(name.into());
    }

    /// The name of the recording playing.
    pub fn playing(&self) -> Option<&str> {
        self.playing.as_deref()
    }

    /// How the last playback ended, or was refused.
    pub fn last_outcome(&self) -> Option<&PlaybackOutcome> {
        self.last_outcome.as_ref()
    }

    /// The recordings found by the last [`refresh`](Self::refresh) (and by every delete), sorted by
    /// name.
    pub fn entries(&self) -> &[ReplayEntry] {
        &self.entries
    }

    pub(crate) fn mirror(state: &ReplayPlaybackState, playing: Option<&str>) -> Self {
        ReplayPlayback {
            playing: playing.map(str::to_owned),
            last_outcome: state.last_outcome.clone(),
            entries: state.entries.clone(),
            play: None,
            refresh: false,
            delete: Vec::new(),
        }
    }
}

/// The engine's persistent playback state; it outlives every world rebuild and the
/// [`ReplayPlayback`] resource mirrors it.
pub(crate) struct ReplayPlaybackState {
    pub(crate) config: crate::ReplayPlaybackConfig,
    pub(crate) last_outcome: Option<PlaybackOutcome>,
    pub(crate) entries: Vec<ReplayEntry>,
    pub(crate) error: Option<Diagnostic>,
}

/// The host state a playback replaces and the return restores.
pub(crate) struct SavedHost {
    pub(crate) doc_texts: BTreeMap<String, InlineDoc>,
    pub(crate) profile_off_disk: bool,
    pub(crate) profile_text: Option<String>,
    pub(crate) speed: Speed,
    pub(crate) render_size: (u32, u32),
    pub(crate) return_to: String,
    pub(crate) resolved: PathBuf,
    pub(crate) return_text: String,
    pub(crate) fallback_path: Option<String>,
    pub(crate) fallback_text: Option<String>,
}

/// A playback in progress.
pub(crate) struct ActivePlayback {
    pub(crate) name: String,
    pub(crate) cursor: ActionCursor,
    pub(crate) frames: u64,
    pub(crate) saved: SavedHost,
}

/// What a start has verified before it may touch the world.
pub(crate) struct PlaybackPlan {
    pub(crate) header: ludion_protocol::replay::ReplayHeader,
    pub(crate) scan: crate::replay::ReplayScan,
    pub(crate) doc_texts: BTreeMap<String, InlineDoc>,
    pub(crate) resolved: PathBuf,
    pub(crate) return_text: String,
}
