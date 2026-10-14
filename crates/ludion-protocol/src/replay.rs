//! The replay session format (engine-architecture.md §6).
//!
//! A replay is everything needed to re-drive the engine through the same command queue and reach the
//! same `WorldHash` sequence: a [`ReplayHeader`] (scene reference, RNG seed, engine/schema versions)
//! followed by frame-stamped [`ReplayRecord`]s — the command/input deltas — with periodic keyframe
//! snapshots so a reader can *seek* without replaying from the start. The format lives in
//! `ludion-protocol` (not in `ludion-sim`) so there is exactly one replay format, per Pillar 6;
//! recording happens in `ludion-engine` at queue-drain time.
//!
//! # Byte layout (little-endian)
//!
//! ```text
//! [4]  magic  = REPLAY_MAGIC (b"LRP1")
//! [2]  format_version (u16) = REPLAY_FORMAT_VERSION
//! [4]  header_len (u32)
//! [..] zstd(JSON of ReplayHeader)                 -- header_len bytes
//! then records to EOF, each:
//!   [1]  kind (u8: 0=command, 1=input, 2=keyframe, 3=doc_reload, 4=scene_switch, 5=scene_load,
//!   6=prefab_text, 7=system_panic)
//!   [8]  frame (u64)
//!   [4]  body_len (u32)
//!   [..] zstd(JSON of the record body)            -- body_len bytes
//! ```
//!
//! Because each record's `frame` and `body_len` sit *in the clear*, a reader scans record boundaries
//! and skips bodies to index keyframes, decompressing only the keyframe it needs and the deltas after
//! it — the property [`ReplayReader::seek`] relies on. Each body is its own zstd frame ("zstd-framed"
//! deltas). Batching many deltas into one frame for a better ratio is a deliberate later optimization
//! (risk register #9); the framing is seekable and round-trips.
//!
//! # Format stability across a new record kind
//!
//! The `doc_reload` kind (`3`) is **additive within [`REPLAY_FORMAT_VERSION`] `1`, no bump**: it is a
//! new discriminant *value* in the existing 1-byte `kind` field — no framing byte changes. Every
//! `.lrp` recorded before it carries only kinds `0`–`2` and decodes byte-identically under this
//! reader, which is the only compatibility direction that matters (old readers seeing a new file is
//! out of scope). Bumping the version would instead force this reader to accept a *set* of versions to
//! keep old files decoding, for no compatibility gain — so the version guards the container *framing*,
//! not the record-kind vocabulary (the E2 precedent grew `InputEvent` at format `1` the same way).
//!
//! The `scene_switch` kind (`4`) grows the vocabulary the same way: every `.lrp` recorded before it
//! carries only kinds `0`–`3` and decodes byte-identically under this reader.
//!
//! The [`ReplayHeader`] grows the same way: its JSON is zstd'd, so a new `#[serde(default)]` field
//! (the E5 `profile` profile-at-start, the `render_extent` at arm time) is additive with no framing
//! change — every `.lrp` written before it decodes with the field defaulting to `None`.
//!
//! The `scene_load` kind (`5`) is additive the same way: every `.lrp` recorded before it holds a
//! mid-session `scene.load` as a plain `command` record, which still decodes and replays.
//!
//! The `prefab_text` kind (`6`) is additive the same way: a replay with no prefab spawns holds none.
//! Its body's `reload` flag is absent unless true, so a read record's bytes are the flag-free form.
//!
//! The `system_panic` kind (`7`) is additive the same way: a session in which no system panicked
//! holds none, and a replay without it accepts no `L4007` at all. A reader older than this kind
//! refuses a file that carries one (`L3010`, unknown kind) instead of replaying it strictly.

use std::io::{self, Write};

use ludion_core::codes::code;
use ludion_core::diagnostics::Diagnostic;
use ludion_core::time::FrameIndex;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::WireHash;
use crate::command::Command;

/// The 4-byte magic that opens a replay file.
pub const REPLAY_MAGIC: [u8; 4] = *b"LRP1";

/// The replay container format version (bumped on any breaking framing change).
pub const REPLAY_FORMAT_VERSION: u16 = 1;

/// The zstd compression level used for every frame (a middle-of-the-road speed/ratio default).
const ZSTD_LEVEL: i32 = 3;

const KIND_COMMAND: u8 = 0;
const KIND_INPUT: u8 = 1;
const KIND_KEYFRAME: u8 = 2;
const KIND_DOC_RELOAD: u8 = 3;
const KIND_SCENE_SWITCH: u8 = 4;
const KIND_SCENE_LOAD: u8 = 5;
const KIND_PREFAB_TEXT: u8 = 6;
const KIND_SYSTEM_PANIC: u8 = 7;
/// The highest record kind this reader understands — the scan's upper bound (see the module docs on
/// additive kinds). Bump this, never [`REPLAY_FORMAT_VERSION`], when adding a record kind.
const KIND_MAX: u8 = KIND_SYSTEM_PANIC;

/// How a replay's initial world is obtained.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SceneRef {
    /// A path the engine resolves relative to its project root.
    Path {
        /// The scene file path.
        path: String,
    },
    /// The scene document inline (raw canonical YAML text), for self-contained replays.
    Inline {
        /// The scene text.
        document: String,
    },
}

/// The header of a replay session: enough to reconstruct the starting conditions and to detect a
/// version/schema mismatch before replaying.
///
/// # Example
/// ```
/// use ludion_protocol::replay::{ReplayHeader, SceneRef, REPLAY_FORMAT_VERSION};
/// use ludion_protocol::{schema, PROTOCOL_VERSION};
///
/// let header = ReplayHeader {
///     format_version: REPLAY_FORMAT_VERSION,
///     engine_version: "0.0.0".into(),
///     protocol_version: PROTOCOL_VERSION,
///     schema_hash: schema::schema_hash(),
///     scene: SceneRef::Path { path: "level.yaml".into() },
///     seed: 7,
///     profile: None,
///     render_extent: None,
/// };
/// assert_eq!(header.seed, 7);
/// ```
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ReplayHeader {
    /// The container format version ([`REPLAY_FORMAT_VERSION`]).
    pub format_version: u16,
    /// The engine build that recorded the session.
    pub engine_version: String,
    /// The protocol version in force during recording.
    pub protocol_version: u32,
    /// The schema hash at record time; a replayer compares it to its own to detect drift.
    pub schema_hash: WireHash,
    /// Where the initial world comes from.
    pub scene: SceneRef,
    /// The seed for the deterministic RNG.
    pub seed: u64,
    /// The player profile at record time, as canonical `kind: profile` text — the profile-at-start a
    /// replay installs (overriding the boot disk-load) so a committed session reproduces bit-exactly
    /// on a machine whose `save/profile.yaml` differs or is absent. `None` when the game registered no
    /// profile. **Additive within [`REPLAY_FORMAT_VERSION`] `1`, no bump** (the header is zstd'd JSON,
    /// so an old `.lrp` header simply lacks the key → `None`), the same compatibility posture as the
    /// `doc_reload` record kind.
    #[serde(default)]
    pub profile: Option<String>,
    /// The render extent at arm time, which a replay installs (through `render.size`, so a hostile
    /// value is refused) before driving: the extent is sim input (a game's cursor unprojection reads
    /// it), so a replay must run at the extent it was recorded at. Later applied resizes are ordinary
    /// `Command` records. `None` is a `.lrp` from before the field, recorded at the default extent by
    /// convention. **Additive within [`REPLAY_FORMAT_VERSION`] `1`, no bump**, the same posture as
    /// `profile`.
    #[serde(default)]
    pub render_extent: Option<ReplayExtent>,
}

/// A render extent in pixels, as recorded in a [`ReplayHeader`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ReplayExtent {
    /// The width in pixels.
    pub width: u32,
    /// The height in pixels.
    pub height: u32,
}

/// One frame-stamped entry in a replay: a command, an injected input, or a keyframe snapshot.
///
/// The `frame` is stored in the record framing (in the clear); the `command` / `event` / `snapshot`
/// body is the zstd-compressed payload. This type is the decoded, in-memory view a reader/writer
/// works with.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ReplayRecord {
    /// A protocol command applied at `frame`.
    Command {
        /// The frame at which the command was drained and applied.
        #[schemars(with = "u64")]
        frame: FrameIndex,
        /// The command.
        command: Command,
    },
    /// An input event injected at `frame` (a `ludion-sim` input-event value, opaque here).
    Input {
        /// The frame the input applies to.
        #[schemars(with = "u64")]
        frame: FrameIndex,
        /// The input event value.
        event: Value,
    },
    /// A world snapshot at `frame`, a seek target (an `engine.snapshot` document, opaque here).
    Keyframe {
        /// The frame the snapshot captures.
        #[schemars(with = "u64")]
        frame: FrameIndex,
        /// The snapshot document.
        snapshot: Value,
    },
    /// A game data-doc reloaded at `frame`, carrying its new canonical text **inline** so a replay
    /// reproduces the swap without reading the (possibly changed or deleted) on-disk file. Unlike a
    /// texture/mesh/voxel `asset.reload` — which is render-only and never recorded — a data-doc reload
    /// is sim-visible (a game system derives components from the `DocStore`), so it must be replayed to
    /// reproduce the `WorldHash` sequence.
    DocReload {
        /// The frame the reload was drained and applied at.
        #[schemars(with = "u64")]
        frame: FrameIndex,
        /// The registered doc kind (`doc_kind`, not `kind`, to avoid the enum's internal tag key).
        doc_kind: String,
        /// The content-root-relative doc key that was reloaded (the `DocStore` entry key).
        path: String,
        /// The new canonical doc text (the `format`/`kind` envelope plus the fields).
        document: String,
    },
    /// A district scene switch performed at `frame`, carrying the target scene's canonical text
    /// **inline** so a replay resolves it off-disk. The switch itself is **regenerated** by the sim
    /// under replay (the game's switch-triggering system re-requests it at the same frame from the
    /// reproduced state) — this record supplies only the text source the regenerated switch needs,
    /// never a drive action.
    SceneSwitch {
        /// The frame the switch was drained and applied at.
        #[schemars(with = "u64")]
        frame: FrameIndex,
        /// The project-root-relative path of the target scene.
        scene_path: String,
        /// The target scene's canonical text at switch time.
        document: String,
    },
    /// A mid-session `scene.load` applied at `frame`, carrying the text it resolved **inline**. A
    /// replay loads the text and never the path, so the session reproduces without the (possibly
    /// changed or deleted) scene file. A `.lrp` recorded before this kind holds the load as a
    /// plain `Command` record, which a replay still executes against disk.
    SceneLoad {
        /// The frame the load was drained and applied at.
        #[schemars(with = "u64")]
        frame: FrameIndex,
        /// The path the command named, kept for a reader's diagnostics; replay ignores it.
        path: String,
        /// The loaded scene's canonical text.
        document: String,
    },
    /// A prefab source text carried **inline**. A read record (`reload: false`) is a text source
    /// like [`ReplayRecord::SceneSwitch`], filed per rebuild epoch (a restore, load or switch starts
    /// the next): the library resolves a path to the text its epoch read instead of reading the
    /// (possibly changed) file. A reload record (`reload: true`) is an accepted `asset.reload`,
    /// applied at its position in the stream.
    PrefabText {
        /// The frame whose tick first sees this text.
        #[schemars(with = "u64")]
        frame: FrameIndex,
        /// The prefab's project-root-relative path.
        path: String,
        /// The prefab's source text as read.
        document: String,
        /// Whether this is an applied reload rather than a first read of the epoch.
        reload: bool,
    },
    /// A system panicked on `frame`'s tick. Written right after the `Command` `sim.step` record whose
    /// last tick it is, so a replay accepts that `L4007` as the reproduction and no other.
    SystemPanic {
        /// The frame of the tick that panicked.
        #[schemars(with = "u64")]
        frame: FrameIndex,
    },
}

impl ReplayRecord {
    /// The frame this record is stamped with.
    pub fn frame(&self) -> FrameIndex {
        match self {
            ReplayRecord::Command { frame, .. }
            | ReplayRecord::Input { frame, .. }
            | ReplayRecord::Keyframe { frame, .. }
            | ReplayRecord::DocReload { frame, .. }
            | ReplayRecord::SceneSwitch { frame, .. }
            | ReplayRecord::SceneLoad { frame, .. }
            | ReplayRecord::PrefabText { frame, .. }
            | ReplayRecord::SystemPanic { frame } => *frame,
        }
    }

    /// Whether this record is a keyframe snapshot.
    pub fn is_keyframe(&self) -> bool {
        matches!(self, ReplayRecord::Keyframe { .. })
    }
}

/// Streams a replay session to any [`Write`] sink (a file, a buffer).
///
/// # Example
/// ```
/// use ludion_protocol::replay::{ReplayHeader, ReplayReader, ReplayWriter, SceneRef, REPLAY_FORMAT_VERSION};
/// use ludion_protocol::{schema, Command, PROTOCOL_VERSION};
/// use ludion_core::time::FrameIndex;
///
/// let header = ReplayHeader {
///     format_version: REPLAY_FORMAT_VERSION,
///     engine_version: "0.0.0".into(),
///     protocol_version: PROTOCOL_VERSION,
///     schema_hash: schema::schema_hash(),
///     scene: SceneRef::Path { path: "level.yaml".into() },
///     seed: 7,
///     profile: None,
///     render_extent: None,
/// };
///
/// let mut buffer = Vec::new();
/// let mut writer = ReplayWriter::new(&mut buffer, &header).unwrap();
/// writer.write_keyframe(FrameIndex::new(0), &serde_json::json!({ "entities": [] })).unwrap();
/// writer.write_command(FrameIndex::new(1), &Command::SimPause).unwrap();
///
/// let reader = ReplayReader::new(&buffer).unwrap();
/// assert_eq!(reader.header().seed, 7);
/// assert_eq!(reader.records().unwrap().len(), 2);
/// ```
pub struct ReplayWriter<W: Write> {
    inner: W,
    records: u64,
}

impl<W: Write> ReplayWriter<W> {
    /// Opens a writer, emitting the magic, version, and compressed header.
    pub fn new(mut inner: W, header: &ReplayHeader) -> io::Result<Self> {
        let header_json = serde_json::to_vec(header).map_err(io::Error::other)?;
        let compressed = zstd::encode_all(&header_json[..], ZSTD_LEVEL)?;
        check_writable("header", header_json.len(), compressed.len())?;
        inner.write_all(&REPLAY_MAGIC)?;
        inner.write_all(&REPLAY_FORMAT_VERSION.to_le_bytes())?;
        inner.write_all(&(compressed.len() as u32).to_le_bytes())?;
        inner.write_all(&compressed)?;
        Ok(ReplayWriter { inner, records: 0 })
    }

    /// Records a command applied at `frame`.
    pub fn write_command(&mut self, frame: FrameIndex, command: &Command) -> io::Result<()> {
        let body = serde_json::to_vec(command).map_err(io::Error::other)?;
        self.write_framed(KIND_COMMAND, frame, &body)
    }

    /// Records an input event injected at `frame`.
    pub fn write_input(&mut self, frame: FrameIndex, event: &Value) -> io::Result<()> {
        let body = serde_json::to_vec(event).map_err(io::Error::other)?;
        self.write_framed(KIND_INPUT, frame, &body)
    }

    /// Records a keyframe snapshot at `frame` (a seek target).
    pub fn write_keyframe(&mut self, frame: FrameIndex, snapshot: &Value) -> io::Result<()> {
        let body = serde_json::to_vec(snapshot).map_err(io::Error::other)?;
        self.write_framed(KIND_KEYFRAME, frame, &body)
    }

    /// Records a game data-doc reload applied at `frame`, carrying the new canonical `document` text
    /// inline (see [`ReplayRecord::DocReload`]).
    pub fn write_doc_reload(
        &mut self,
        frame: FrameIndex,
        doc_kind: &str,
        path: &str,
        document: &str,
    ) -> io::Result<()> {
        let body = serde_json::to_vec(&DocReloadBody {
            doc_kind,
            path,
            document,
        })
        .map_err(io::Error::other)?;
        self.write_framed(KIND_DOC_RELOAD, frame, &body)
    }

    /// Records a district scene switch applied at `frame`, carrying the target scene's canonical
    /// `document` text inline (see [`ReplayRecord::SceneSwitch`]).
    pub fn write_scene_switch(
        &mut self,
        frame: FrameIndex,
        scene_path: &str,
        document: &str,
    ) -> io::Result<()> {
        let body = serde_json::to_vec(&SceneSwitchBody {
            scene_path,
            document,
        })
        .map_err(io::Error::other)?;
        self.write_framed(KIND_SCENE_SWITCH, frame, &body)
    }

    /// Records a mid-session `scene.load` applied at `frame`, carrying the resolved `document` text
    /// inline (see [`ReplayRecord::SceneLoad`]).
    pub fn write_scene_load(
        &mut self,
        frame: FrameIndex,
        path: &str,
        document: &str,
    ) -> io::Result<()> {
        let body =
            serde_json::to_vec(&SceneLoadBody { path, document }).map_err(io::Error::other)?;
        self.write_framed(KIND_SCENE_LOAD, frame, &body)
    }

    /// Records a prefab source text read from disk at `frame`, carrying the `document` inline (see
    /// [`ReplayRecord::PrefabText`]).
    pub fn write_prefab_text(
        &mut self,
        frame: FrameIndex,
        path: &str,
        document: &str,
        reload: bool,
    ) -> io::Result<()> {
        let body = serde_json::to_vec(&PrefabTextBody {
            path,
            document,
            reload,
        })
        .map_err(io::Error::other)?;
        self.write_framed(KIND_PREFAB_TEXT, frame, &body)
    }

    /// Records that a system panicked on `frame`'s tick (see [`ReplayRecord::SystemPanic`]).
    pub fn write_system_panic(&mut self, frame: FrameIndex) -> io::Result<()> {
        self.write_framed(KIND_SYSTEM_PANIC, frame, b"{}")
    }

    /// Records an already-decoded [`ReplayRecord`].
    pub fn write_record(&mut self, record: &ReplayRecord) -> io::Result<()> {
        match record {
            ReplayRecord::Command { frame, command } => self.write_command(*frame, command),
            ReplayRecord::Input { frame, event } => self.write_input(*frame, event),
            ReplayRecord::Keyframe { frame, snapshot } => self.write_keyframe(*frame, snapshot),
            ReplayRecord::DocReload {
                frame,
                doc_kind,
                path,
                document,
            } => self.write_doc_reload(*frame, doc_kind, path, document),
            ReplayRecord::SceneSwitch {
                frame,
                scene_path,
                document,
            } => self.write_scene_switch(*frame, scene_path, document),
            ReplayRecord::SceneLoad {
                frame,
                path,
                document,
            } => self.write_scene_load(*frame, path, document),
            ReplayRecord::PrefabText {
                frame,
                path,
                document,
                reload,
            } => self.write_prefab_text(*frame, path, document, *reload),
            ReplayRecord::SystemPanic { frame } => self.write_system_panic(*frame),
        }
    }

    /// How many records have been written after the header.
    pub fn records_written(&self) -> u64 {
        self.records
    }

    /// Flushes and returns the underlying sink.
    pub fn into_inner(mut self) -> io::Result<W> {
        self.inner.flush()?;
        Ok(self.inner)
    }

    fn write_framed(&mut self, kind: u8, frame: FrameIndex, body_json: &[u8]) -> io::Result<()> {
        let compressed = zstd::encode_all(body_json, ZSTD_LEVEL)?;
        check_writable(kind_name(kind), body_json.len(), compressed.len())?;
        self.inner.write_all(&[kind])?;
        self.inner.write_all(&frame.as_u64().to_le_bytes())?;
        self.inner
            .write_all(&(compressed.len() as u32).to_le_bytes())?;
        self.inner.write_all(&compressed)?;
        self.records += 1;
        Ok(())
    }
}

/// A header or record whose JSON is longer than the reader's ceiling for its frame: the ceiling is
/// per frame and scales with how well the frame compresses (256x its compressed size, within 1 MiB
/// and 256 MiB). The writer refuses it, because the reader would refuse the file as `L3017`.
/// Carried inside the [`io::Error`] the writer returns (`ErrorKind::InvalidInput`), recoverable
/// with [`io::Error::get_ref`] so a host can report it as a diagnostic rather than an I/O failure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecordTooLarge {
    /// What was being written: `"header"` or a record kind such as `"scene_load"`.
    pub what: &'static str,
    /// The JSON body's length in bytes.
    pub len: u64,
    /// The largest body the reader admits for this frame, in bytes.
    pub max: u64,
}

impl RecordTooLarge {
    /// The `L3017` this refusal reports, worded for the writing side.
    pub fn to_diagnostic(&self) -> Diagnostic {
        Diagnostic::new(
            code::REPLAY_DECOMPRESSED_TOO_LARGE,
            format!(
                "the {} to record is {} KiB, past its {} KiB replay ceiling ({MAX_RATIO}x its \
                 compressed size, at most {} MiB), so the replay could not be read back",
                self.what,
                self.len / 1024,
                self.max / 1024,
                HARD_CEILING / (1024 * 1024)
            ),
        )
        .with_help(format!(
            "a replay frame holds one record whole; a keyframe past {} MiB, or JSON compressing \
             better than {MAX_RATIO}:1, cannot be recorded — record a smaller scene",
            HARD_CEILING / (1024 * 1024)
        ))
    }
}

impl std::fmt::Display for RecordTooLarge {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "replay {} of {} bytes is past the {}-byte ceiling",
            self.what, self.len, self.max
        )
    }
}

impl std::error::Error for RecordTooLarge {}

/// Refuses a JSON body the reader would refuse, given its compressed length, before anything is
/// written.
fn check_writable(what: &'static str, json_len: usize, compressed_len: usize) -> io::Result<()> {
    let len = json_len as u64;
    let max = frame_ceiling(compressed_len);
    if len > max {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            RecordTooLarge { what, len, max },
        ));
    }
    Ok(())
}

/// The record kind's name, for [`RecordTooLarge`].
fn kind_name(kind: u8) -> &'static str {
    match kind {
        KIND_COMMAND => "command",
        KIND_INPUT => "input",
        KIND_KEYFRAME => "keyframe",
        KIND_DOC_RELOAD => "doc_reload",
        KIND_SCENE_SWITCH => "scene_switch",
        KIND_SCENE_LOAD => "scene_load",
        KIND_PREFAB_TEXT => "prefab_text",
        KIND_SYSTEM_PANIC => "system_panic",
        _ => "record",
    }
}

/// The empty JSON body of a [`ReplayRecord::SystemPanic`]; decoding it keeps non-JSON an error.
#[derive(Deserialize)]
struct SystemPanicBody {}

/// The JSON body of a [`ReplayRecord::DocReload`], written borrowed (zero-copy) and read owned.
#[derive(Serialize)]
struct DocReloadBody<'a> {
    doc_kind: &'a str,
    path: &'a str,
    document: &'a str,
}

/// The owned counterpart of [`DocReloadBody`], used when decoding a doc-reload record body.
#[derive(Deserialize)]
struct DocReloadBodyOwned {
    doc_kind: String,
    path: String,
    document: String,
}

/// The JSON body of a [`ReplayRecord::SceneSwitch`], written borrowed (zero-copy) and read owned.
#[derive(Serialize)]
struct SceneSwitchBody<'a> {
    scene_path: &'a str,
    document: &'a str,
}

/// The owned counterpart of [`SceneSwitchBody`], used when decoding a scene-switch record body.
#[derive(Deserialize)]
struct SceneSwitchBodyOwned {
    scene_path: String,
    document: String,
}

/// The JSON body of a [`ReplayRecord::SceneLoad`], written borrowed (zero-copy) and read owned.
#[derive(Serialize)]
struct SceneLoadBody<'a> {
    path: &'a str,
    document: &'a str,
}

/// The owned counterpart of [`SceneLoadBody`], used when decoding a scene-load record body.
#[derive(Deserialize)]
struct SceneLoadBodyOwned {
    path: String,
    document: String,
}

/// The JSON body of a [`ReplayRecord::PrefabText`], written borrowed (zero-copy) and read owned.
#[derive(Serialize)]
struct PrefabTextBody<'a> {
    path: &'a str,
    document: &'a str,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    reload: bool,
}

/// The owned counterpart of [`PrefabTextBody`], used when decoding a prefab-text record body.
#[derive(Deserialize)]
struct PrefabTextBodyOwned {
    path: String,
    document: String,
    #[serde(default)]
    reload: bool,
}

/// One record's framing metadata plus a borrowed slice of its (still-compressed) body.
struct RawRecord<'a> {
    kind: u8,
    frame: FrameIndex,
    /// Byte offset of the compressed body within the file (for diagnostics that name a location).
    body_offset: usize,
    body: &'a [u8],
}

/// Reads a replay session from an in-memory byte slice, with seek support.
#[derive(Debug)]
pub struct ReplayReader<'a> {
    bytes: &'a [u8],
    header: ReplayHeader,
    records_start: usize,
}

/// The result of [`ReplayReader::seek`]: the latest keyframe at or before the target, and the
/// command/input deltas that advance from it to the target (in record order).
#[derive(Clone, Debug, PartialEq)]
pub struct Seek {
    /// The nearest keyframe `(frame, snapshot)` with `frame <= target`, if any exists.
    pub keyframe: Option<(FrameIndex, Value)>,
    /// The command/input records to apply after the keyframe (or from the start, if none) to reach
    /// the target frame, in original order.
    pub deltas: Vec<ReplayRecord>,
}

impl<'a> ReplayReader<'a> {
    /// Parses the header and validates the container framing.
    ///
    /// # Errors
    /// - `L3010` if the magic/version is wrong, the file is truncated, or a record/header is not
    ///   valid JSON.
    /// - `L3011` if a zstd frame fails to decompress.
    pub fn new(bytes: &'a [u8]) -> Result<Self, Diagnostic> {
        if bytes.len() < 10 {
            return Err(format_err("replay is shorter than a header"));
        }
        if bytes[0..4] != REPLAY_MAGIC {
            return Err(format_err("replay does not start with the LRP1 magic"));
        }
        let version = u16::from_le_bytes(bytes[4..6].try_into().unwrap());
        if version != REPLAY_FORMAT_VERSION {
            return Err(format_err(format!(
                "replay format version {version} is not supported (this engine reads version \
                 {REPLAY_FORMAT_VERSION})"
            )));
        }
        let header_len = u32::from_le_bytes(bytes[6..10].try_into().unwrap()) as usize;
        let header_end = 10usize
            .checked_add(header_len)
            .filter(|end| *end <= bytes.len())
            .ok_or_else(|| format_err("replay header length runs past the end of the file"))?;
        let header_json = decompress(&bytes[10..header_end], || {
            "the replay header (at byte offset 10)".to_string()
        })?;
        let header: ReplayHeader = serde_json::from_slice(&header_json)
            .map_err(|e| format_err(format!("replay header is not valid JSON: {e}")))?;
        Ok(ReplayReader {
            bytes,
            header,
            records_start: header_end,
        })
    }

    /// The parsed header.
    pub fn header(&self) -> &ReplayHeader {
        &self.header
    }

    /// Decodes every record in order.
    pub fn records(&self) -> Result<Vec<ReplayRecord>, Diagnostic> {
        self.scan()?.iter().map(|raw| decode_record(raw)).collect()
    }

    /// Finds the latest keyframe at or before `target` and the deltas that reach `target` from it.
    ///
    /// The deltas are the command/input records written after the chosen keyframe in the stream
    /// with `frame <= target`; when no keyframe is at or before `target`, `keyframe` is `None` and
    /// they are every such record (replay from the initial scene). Records written before the
    /// keyframe are already in its state — a future-stamped input rides its `pending_input` — so
    /// stream order decides, not the frame stamp. Keyframe records after the chosen one are ignored.
    /// A keyframe without a `pending_input` key (an empty script, or one written before the field
    /// existed) also gets the inputs written before it and stamped in `(keyframe, target]`, first,
    /// since an older keyframe lost exactly those.
    pub fn seek(&self, target: FrameIndex) -> Result<Seek, Diagnostic> {
        let raw = self.scan()?;

        // Latest keyframe with frame <= target.
        let keyframe_index = raw
            .iter()
            .enumerate()
            .rfind(|(_, r)| r.kind == KIND_KEYFRAME && r.frame <= target)
            .map(|(i, _)| i);

        let keyframe = match keyframe_index {
            Some(i) => match decode_record(&raw[i])? {
                ReplayRecord::Keyframe { frame, snapshot } => Some((frame, snapshot)),
                _ => unreachable!("index selected a keyframe"),
            },
            None => None,
        };

        let mut deltas = Vec::new();
        if let (Some(i), Some((keyframe_frame, snapshot))) = (keyframe_index, &keyframe)
            && snapshot.get("pending_input").is_none()
        {
            for r in raw[..i].iter().filter(|r| r.kind == KIND_INPUT) {
                if r.frame > *keyframe_frame && r.frame <= target {
                    deltas.push(decode_record(r)?);
                }
            }
        }
        for r in raw.iter().skip(keyframe_index.map_or(0, |i| i + 1)) {
            // `continue`, not `break`: an input is stamped at its apply frame, which can be later
            // than records that follow it in the stream.
            if r.kind == KIND_KEYFRAME || r.frame > target {
                continue;
            }
            deltas.push(decode_record(r)?);
        }

        Ok(Seek { keyframe, deltas })
    }

    /// Walks the record framing, borrowing each still-compressed body without decompressing it.
    fn scan(&self) -> Result<Vec<RawRecord<'a>>, Diagnostic> {
        let mut out = Vec::new();
        let mut offset = self.records_start;
        while offset < self.bytes.len() {
            if offset + 13 > self.bytes.len() {
                return Err(format_err("replay is truncated inside a record header"));
            }
            let kind = self.bytes[offset];
            if kind > KIND_MAX {
                return Err(format_err(format!(
                    "replay has an unknown record kind {kind}"
                )));
            }
            let frame = FrameIndex::new(u64::from_le_bytes(
                self.bytes[offset + 1..offset + 9].try_into().unwrap(),
            ));
            let body_len =
                u32::from_le_bytes(self.bytes[offset + 9..offset + 13].try_into().unwrap())
                    as usize;
            let body_start = offset + 13;
            let body_end = body_start
                .checked_add(body_len)
                .filter(|end| *end <= self.bytes.len())
                .ok_or_else(|| format_err("replay record body runs past the end of the file"))?;
            out.push(RawRecord {
                kind,
                frame,
                body_offset: body_start,
                body: &self.bytes[body_start..body_end],
            });
            offset = body_end;
        }
        Ok(out)
    }
}

/// Decompresses one raw record body and reconstructs the typed [`ReplayRecord`].
fn decode_record(raw: &RawRecord<'_>) -> Result<ReplayRecord, Diagnostic> {
    let body = decompress(raw.body, || {
        format!("the replay record body at byte offset {}", raw.body_offset)
    })?;
    let record = match raw.kind {
        KIND_COMMAND => ReplayRecord::Command {
            frame: raw.frame,
            command: serde_json::from_slice(&body)
                .map_err(|e| format_err(format!("replay command record is not valid JSON: {e}")))?,
        },
        KIND_INPUT => ReplayRecord::Input {
            frame: raw.frame,
            event: serde_json::from_slice(&body)
                .map_err(|e| format_err(format!("replay input record is not valid JSON: {e}")))?,
        },
        KIND_KEYFRAME => ReplayRecord::Keyframe {
            frame: raw.frame,
            snapshot: serde_json::from_slice(&body).map_err(|e| {
                format_err(format!("replay keyframe record is not valid JSON: {e}"))
            })?,
        },
        KIND_DOC_RELOAD => {
            let payload: DocReloadBodyOwned = serde_json::from_slice(&body).map_err(|e| {
                format_err(format!("replay doc-reload record is not valid JSON: {e}"))
            })?;
            ReplayRecord::DocReload {
                frame: raw.frame,
                doc_kind: payload.doc_kind,
                path: payload.path,
                document: payload.document,
            }
        }
        KIND_SCENE_SWITCH => {
            let payload: SceneSwitchBodyOwned = serde_json::from_slice(&body).map_err(|e| {
                format_err(format!("replay scene-switch record is not valid JSON: {e}"))
            })?;
            ReplayRecord::SceneSwitch {
                frame: raw.frame,
                scene_path: payload.scene_path,
                document: payload.document,
            }
        }
        KIND_SCENE_LOAD => {
            let payload: SceneLoadBodyOwned = serde_json::from_slice(&body).map_err(|e| {
                format_err(format!("replay scene-load record is not valid JSON: {e}"))
            })?;
            ReplayRecord::SceneLoad {
                frame: raw.frame,
                path: payload.path,
                document: payload.document,
            }
        }
        KIND_PREFAB_TEXT => {
            let payload: PrefabTextBodyOwned = serde_json::from_slice(&body).map_err(|e| {
                format_err(format!("replay prefab-text record is not valid JSON: {e}"))
            })?;
            ReplayRecord::PrefabText {
                frame: raw.frame,
                path: payload.path,
                document: payload.document,
                reload: payload.reload,
            }
        }
        KIND_SYSTEM_PANIC => {
            serde_json::from_slice::<SystemPanicBody>(&body).map_err(|e| {
                format_err(format!("replay system-panic record is not valid JSON: {e}"))
            })?;
            ReplayRecord::SystemPanic { frame: raw.frame }
        }
        other => {
            return Err(format_err(format!(
                "replay has an unknown record kind {other}"
            )));
        }
    };
    Ok(record)
}

/// The most a replay frame may inflate relative to its compressed size. A decompression bomb is a
/// ratio attack (the fuzz reproducer `fuzz/findings/replay_reader/zstd-decompression-bomb.lrp` is
/// 274 bytes inflating to 8 MiB, 30,615:1); no committed replay compresses better than ~26:1, so 256
/// sits an order of magnitude over real data and far under any bomb.
const MAX_RATIO: u64 = 256;

/// The smallest ceiling a frame gets: a small frame is never judged by ratio, since zstd framing
/// overhead dominates its size.
const FLOOR: u64 = 1024 * 1024;

/// The absolute ceiling for one frame, whatever its compressed size — the bound on memory a reader
/// spends per frame.
const HARD_CEILING: u64 = 256 * 1024 * 1024;

// The reproducer's 274-byte frame must stay refused, and a keyframe must have real room.
const _: () = assert!(FLOOR < 8 * 1024 * 1024);
const _: () = assert!(HARD_CEILING >= 64 * 1024 * 1024);

/// The most a frame of `compressed_len` bytes may decompress to. The reader and the writer both use
/// it, so the writer refuses exactly what the reader would.
fn frame_ceiling(compressed_len: usize) -> u64 {
    (compressed_len as u64)
        .saturating_mul(MAX_RATIO)
        .clamp(FLOOR, HARD_CEILING)
}

/// zstd-decompresses one frame under a hard output ceiling (`frame_ceiling` of its length).
///
/// Every body in a replay is its own zstd frame, and the in-the-clear `body_len`/`header_len` bound
/// only the *compressed* bytes on disk, so without a ceiling a tiny frame could inflate without limit
/// and exhaust memory before the JSON parse even runs. Streams the frame and refuses at the moment
/// the output would exceed the cap, so a hostile frame never drives an allocation larger than the
/// ceiling regardless of what its bytes claim. `location` is evaluated only on the over-cap path, to
/// name the offending frame in the `L3017` diagnostic.
///
/// # Errors
/// - `L3011` if the bytes are not a decodable zstd frame.
/// - `L3017` if the frame decompresses to more than its ceiling.
fn decompress(frame: &[u8], location: impl FnOnce() -> String) -> Result<Vec<u8>, Diagnostic> {
    use std::io::Read;

    let ceiling = frame_ceiling(frame.len());
    let decoder = zstd::stream::read::Decoder::new(frame).map_err(compression_err)?;
    // Read at most ceiling + 1 bytes: yielding ceiling + 1 proves the frame exceeds the cap, and the
    // buffer never grows past that regardless of the frame's advertised content size.
    let mut out = Vec::new();
    decoder
        .take(ceiling + 1)
        .read_to_end(&mut out)
        .map_err(compression_err)?;
    if out.len() as u64 > ceiling {
        return Err(Diagnostic::new(
            code::REPLAY_DECOMPRESSED_TOO_LARGE,
            format!(
                "{} decompresses to more than its {} KiB replay ceiling ({MAX_RATIO}x its {} \
                 compressed bytes, at most {} MiB)",
                location(),
                ceiling / 1024,
                frame.len(),
                HARD_CEILING / (1024 * 1024)
            ),
        )
        .with_help(
            "this is a decompression bomb or a corrupt frame; re-record the session or verify the \
             .lrp came from a trusted `ludion` run",
        ));
    }
    Ok(out)
}

/// Maps a zstd decode failure to `L3011`.
fn compression_err(e: io::Error) -> Diagnostic {
    Diagnostic::new(
        code::REPLAY_COMPRESSION,
        format!("a replay zstd frame could not be decompressed: {e}"),
    )
    .with_help("the replay is corrupt or was written by an incompatible tool; re-record it")
}

/// Builds an `L3010` replay-format diagnostic.
fn format_err(message: impl Into<String>) -> Diagnostic {
    Diagnostic::new(code::REPLAY_FORMAT, message).with_help(
        "a replay is the LRP1 magic, a u16 format version, a length-prefixed zstd header, then \
         zstd-framed {kind, frame, body} records; re-record the session if it is corrupt",
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every committed replay under the named top-level folders of the repo, as `(display path,
    /// bytes)`, sorted. Empty outside a checkout (a packaged crate has no `games/`), which the
    /// callers treat as "nothing to measure". The decompression-bomb reproducers under `fuzz/` and
    /// `crates/*/tests/` are hostile on purpose and are not corpus.
    fn committed_replays(tops: &[&str]) -> Vec<(String, Vec<u8>)> {
        fn walk(dir: &std::path::Path, display: &str, found: &mut Vec<(String, Vec<u8>)>) {
            let Ok(entries) = std::fs::read_dir(dir) else {
                return;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                let shown = format!("{display}/{}", entry.file_name().to_string_lossy());
                if path.is_dir() {
                    walk(&path, &shown, found);
                } else if path.extension().is_some_and(|ext| ext == "lrp")
                    && let Ok(bytes) = std::fs::read(&path)
                {
                    found.push((shown, bytes));
                }
            }
        }
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("..");
        let mut found = Vec::new();
        for top in tops {
            walk(&root.join(top), top, &mut found);
        }
        found.sort_by(|a, b| a.0.cmp(&b.0));
        found
    }

    /// `(compressed, decompressed)` byte lengths of the header frame and every record in `bytes`,
    /// measured with no ceiling — the committed replays are trusted input.
    fn frame_sizes(bytes: &[u8]) -> Vec<(usize, u64)> {
        let reader = ReplayReader::new(bytes).expect("a committed replay parses");
        let header_len = u32::from_le_bytes(bytes[6..10].try_into().unwrap()) as usize;
        let mut frames = vec![&bytes[10..10 + header_len]];
        frames.extend(
            reader
                .scan()
                .expect("a committed replay's framing scans")
                .iter()
                .map(|raw| raw.body),
        );
        frames
            .into_iter()
            .map(|frame| {
                let inflated = zstd::decode_all(frame).expect("frame decompresses");
                (frame.len(), inflated.len() as u64)
            })
            .collect()
    }

    /// **The committed-corpus gate.** Every committed replay decodes through the real reader, and
    /// every frame sits at least 4x under its ceiling, so a corpus drifting toward the ratio bound
    /// fails here, naming the frame, long before a legitimate replay is refused as `L3017`.
    #[test]
    fn committed_replays_decode_well_under_the_ceiling() {
        for (path, bytes) in committed_replays(&["games", "examples"]) {
            let reader = ReplayReader::new(&bytes).expect("a committed replay parses");
            assert!(
                reader.records().is_ok(),
                "{path} no longer decodes under its frame ceilings"
            );
            for (compressed, decompressed) in frame_sizes(&bytes) {
                assert!(
                    decompressed * 4 <= frame_ceiling(compressed),
                    "{path} has a frame of {compressed} compressed bytes inflating to \
                     {decompressed} (ratio {:.1}:1), within 4x of its {}-byte ceiling",
                    decompressed as f64 / compressed.max(1) as f64,
                    frame_ceiling(compressed)
                );
            }
        }
    }

    fn header() -> ReplayHeader {
        ReplayHeader {
            format_version: REPLAY_FORMAT_VERSION,
            engine_version: "0.0.0".into(),
            protocol_version: crate::PROTOCOL_VERSION,
            schema_hash: WireHash::from_u64(0xabcd),
            scene: SceneRef::Path {
                path: "level.yaml".into(),
            },
            seed: 7,
            profile: None,
            render_extent: None,
        }
    }

    /// Writes a session with keyframes at 0 and 10 and commands/inputs in between.
    fn sample_session() -> Vec<u8> {
        let mut buffer = Vec::new();
        let mut writer = ReplayWriter::new(&mut buffer, &header()).unwrap();
        writer
            .write_keyframe(FrameIndex::new(0), &serde_json::json!({ "k": 0 }))
            .unwrap();
        writer
            .write_command(FrameIndex::new(2), &Command::SimResume)
            .unwrap();
        writer
            .write_input(
                FrameIndex::new(5),
                &serde_json::json!({ "kind": "key_down", "key": "W" }),
            )
            .unwrap();
        writer
            .write_keyframe(FrameIndex::new(10), &serde_json::json!({ "k": 10 }))
            .unwrap();
        writer
            .write_command(FrameIndex::new(12), &Command::SimPause)
            .unwrap();
        writer.into_inner().unwrap();
        buffer
    }

    #[test]
    fn header_and_records_round_trip() {
        let bytes = sample_session();
        let reader = ReplayReader::new(&bytes).unwrap();
        assert_eq!(reader.header(), &header());
        let records = reader.records().unwrap();
        assert_eq!(records.len(), 5);
        assert_eq!(
            records[1],
            ReplayRecord::Command {
                frame: FrameIndex::new(2),
                command: Command::SimResume
            }
        );
        assert!(records[3].is_keyframe());
    }

    #[test]
    fn seek_picks_latest_keyframe_and_following_deltas() {
        let bytes = sample_session();
        let reader = ReplayReader::new(&bytes).unwrap();

        // Target frame 12: latest keyframe <= 12 is the one at frame 10; only the command at 12
        // follows it.
        let seek = reader.seek(FrameIndex::new(12)).unwrap();
        assert_eq!(seek.keyframe.as_ref().unwrap().0, FrameIndex::new(10));
        assert_eq!(seek.deltas.len(), 1);
        assert_eq!(seek.deltas[0].frame(), FrameIndex::new(12));

        // Target frame 7: latest keyframe <= 7 is the one at frame 0; the command at 2 and input at
        // 5 follow it, the command at 12 does not.
        let seek = reader.seek(FrameIndex::new(7)).unwrap();
        assert_eq!(seek.keyframe.as_ref().unwrap().0, FrameIndex::new(0));
        assert_eq!(seek.deltas.len(), 2);
        assert!(seek.deltas.iter().all(|d| !d.is_keyframe()));
    }

    #[test]
    fn seek_falls_back_to_frame_stamps_for_a_keyframe_without_pending_input() {
        let step = |ticks| Command::SimStep(crate::command::SimStepParams { ticks, hash: false });
        let key = |name: &str| serde_json::json!({ "kind": "key_down", "key": name });
        let mut buffer = Vec::new();
        let mut writer = ReplayWriter::new(&mut buffer, &header()).unwrap();
        writer
            .write_keyframe(FrameIndex::new(0), &serde_json::json!({ "k": 0 }))
            .unwrap();
        writer.write_input(FrameIndex::new(120), &key("W")).unwrap();
        writer.write_command(FrameIndex::new(0), &step(60)).unwrap();
        writer
            .write_keyframe(FrameIndex::new(60), &serde_json::json!({ "k": 60 }))
            .unwrap();
        writer
            .write_command(FrameIndex::new(60), &step(10))
            .unwrap();
        writer.into_inner().unwrap();

        let reader = ReplayReader::new(&buffer).unwrap();
        let w = ReplayRecord::Input {
            frame: FrameIndex::new(120),
            event: key("W"),
        };

        let seek = reader.seek(FrameIndex::new(130)).unwrap();
        assert_eq!(seek.keyframe.as_ref().unwrap().0, FrameIndex::new(60));
        assert_eq!(seek.deltas.first(), Some(&w));
        assert_eq!(seek.deltas.iter().filter(|d| **d == w).count(), 1);

        let seek = reader.seek(FrameIndex::new(70)).unwrap();
        assert!(!seek.deltas.contains(&w), "120 > 70: {:?}", seek.deltas);
    }

    #[test]
    fn seek_picks_deltas_by_stream_position_after_the_keyframe() {
        let step = |ticks| Command::SimStep(crate::command::SimStepParams { ticks, hash: false });
        let key = |name: &str| serde_json::json!({ "kind": "key_down", "key": name });
        let mut buffer = Vec::new();
        let mut writer = ReplayWriter::new(&mut buffer, &header()).unwrap();
        writer
            .write_keyframe(FrameIndex::new(0), &serde_json::json!({ "k": 0 }))
            .unwrap();
        // Stamped for a later frame but written before the next keyframe: that keyframe carries it.
        writer.write_input(FrameIndex::new(120), &key("W")).unwrap();
        writer.write_command(FrameIndex::new(0), &step(60)).unwrap();
        writer
            .write_keyframe(
                FrameIndex::new(60),
                &serde_json::json!({ "k": 60, "pending_input": [{ "frame": 120 }] }),
            )
            .unwrap();
        // Stamped at the keyframe's own frame but written after it: a delta.
        writer
            .write_input(FrameIndex::new(60), &key("Space"))
            .unwrap();
        writer
            .write_command(FrameIndex::new(60), &step(10))
            .unwrap();
        writer.into_inner().unwrap();

        let reader = ReplayReader::new(&buffer).unwrap();

        let seek = reader.seek(FrameIndex::new(70)).unwrap();
        assert_eq!(seek.keyframe.as_ref().unwrap().0, FrameIndex::new(60));
        assert_eq!(
            seek.deltas,
            vec![
                ReplayRecord::Input {
                    frame: FrameIndex::new(60),
                    event: key("Space")
                },
                ReplayRecord::Command {
                    frame: FrameIndex::new(60),
                    command: step(10)
                },
            ]
        );

        let seek = reader.seek(FrameIndex::new(130)).unwrap();
        assert_eq!(seek.keyframe.as_ref().unwrap().0, FrameIndex::new(60));
        assert!(
            !seek
                .deltas
                .iter()
                .any(|d| d.frame() == FrameIndex::new(120)),
            "the keyframe already carries the W input: {:?}",
            seek.deltas
        );
    }

    #[test]
    fn seek_before_first_keyframe_has_none() {
        // A session whose first keyframe is at frame 10, with a delta at frame 1.
        let mut buffer = Vec::new();
        let mut writer = ReplayWriter::new(&mut buffer, &header()).unwrap();
        writer
            .write_command(FrameIndex::new(1), &Command::SimResume)
            .unwrap();
        writer
            .write_keyframe(FrameIndex::new(10), &serde_json::json!({ "k": 10 }))
            .unwrap();
        writer.into_inner().unwrap();

        let reader = ReplayReader::new(&buffer).unwrap();
        let seek = reader.seek(FrameIndex::new(5)).unwrap();
        assert!(seek.keyframe.is_none());
        assert_eq!(seek.deltas.len(), 1);
        assert_eq!(seek.deltas[0].frame(), FrameIndex::new(1));
    }

    #[test]
    fn corrupt_inputs_are_diagnosed() {
        assert_eq!(
            ReplayReader::new(b"nope").unwrap_err().code,
            code::REPLAY_FORMAT
        );

        let mut bad_magic = sample_session();
        bad_magic[0] = b'X';
        assert_eq!(
            ReplayReader::new(&bad_magic).unwrap_err().code,
            code::REPLAY_FORMAT
        );

        // Truncate mid-body: header parses, but scanning the records fails.
        let bytes = sample_session();
        let truncated = &bytes[..bytes.len() - 3];
        let reader = ReplayReader::new(truncated).unwrap();
        assert_eq!(reader.records().unwrap_err().code, code::REPLAY_FORMAT);
    }

    #[test]
    fn doc_reload_record_round_trips() {
        // A doc-reload record (kind 3) writes and reads back intact, carrying its inline canonical
        // text — the additive record the E4 replay story depends on.
        let mut buffer = Vec::new();
        let mut writer = ReplayWriter::new(&mut buffer, &header()).unwrap();
        writer
            .write_command(FrameIndex::new(1), &Command::SimResume)
            .unwrap();
        writer
            .write_doc_reload(
                FrameIndex::new(4),
                "weapon",
                "weapons/crossbow.yaml",
                "format: 1\nkind: weapon\ndamage: 20.0\n",
            )
            .unwrap();
        writer.into_inner().unwrap();

        let reader = ReplayReader::new(&buffer).unwrap();
        let records = reader.records().unwrap();
        assert_eq!(records.len(), 2);
        assert_eq!(
            records[1],
            ReplayRecord::DocReload {
                frame: FrameIndex::new(4),
                doc_kind: "weapon".into(),
                path: "weapons/crossbow.yaml".into(),
                document: "format: 1\nkind: weapon\ndamage: 20.0\n".into(),
            }
        );
        // The doc-reload record is a delta (a seek at frame 5 with no keyframe returns both records).
        let seek = reader.seek(FrameIndex::new(5)).unwrap();
        assert!(seek.keyframe.is_none());
        assert_eq!(seek.deltas.len(), 2);
        assert_eq!(seek.deltas[1].frame(), FrameIndex::new(4));
    }

    #[test]
    fn write_record_round_trips_a_doc_reload() {
        // The already-decoded `write_record` path handles the new variant too.
        let record = ReplayRecord::DocReload {
            frame: FrameIndex::new(7),
            doc_kind: "enemy".into(),
            path: "enemies/goblin.yaml".into(),
            document: "format: 1\nkind: enemy\nhp: 10\n".into(),
        };
        let mut buffer = Vec::new();
        let mut writer = ReplayWriter::new(&mut buffer, &header()).unwrap();
        writer.write_record(&record).unwrap();
        writer.into_inner().unwrap();

        let reader = ReplayReader::new(&buffer).unwrap();
        assert_eq!(reader.records().unwrap(), vec![record]);
    }

    #[test]
    fn scene_switch_record_round_trips() {
        // A scene-switch record (kind 4) writes and reads back intact, carrying its inline canonical
        // district text — the additive record the E7 replay story depends on.
        let mut buffer = Vec::new();
        let mut writer = ReplayWriter::new(&mut buffer, &header()).unwrap();
        writer
            .write_command(FrameIndex::new(1), &Command::SimResume)
            .unwrap();
        writer
            .write_scene_switch(
                FrameIndex::new(4),
                "scenes/district-2.yaml",
                "format: 1\nkind: scene\nseed: 0\nentities: []\n",
            )
            .unwrap();
        writer.into_inner().unwrap();

        let reader = ReplayReader::new(&buffer).unwrap();
        let records = reader.records().unwrap();
        assert_eq!(records.len(), 2);
        assert_eq!(
            records[1],
            ReplayRecord::SceneSwitch {
                frame: FrameIndex::new(4),
                scene_path: "scenes/district-2.yaml".into(),
                document: "format: 1\nkind: scene\nseed: 0\nentities: []\n".into(),
            }
        );
        // The scene-switch record is a delta (a seek at frame 5 with no keyframe returns both
        // records) — it carries no snapshot, so seek treats it like any other delta at its frame.
        let seek = reader.seek(FrameIndex::new(5)).unwrap();
        assert!(seek.keyframe.is_none());
        assert_eq!(seek.deltas.len(), 2);
        assert_eq!(seek.deltas[1].frame(), FrameIndex::new(4));
    }

    #[test]
    fn write_record_round_trips_a_scene_switch() {
        // The already-decoded `write_record` path handles the new variant too.
        let record = ReplayRecord::SceneSwitch {
            frame: FrameIndex::new(7),
            scene_path: "scenes/district-3.yaml".into(),
            document: "format: 1\nkind: scene\nseed: 0\nentities: []\n".into(),
        };
        let mut buffer = Vec::new();
        let mut writer = ReplayWriter::new(&mut buffer, &header()).unwrap();
        writer.write_record(&record).unwrap();
        writer.into_inner().unwrap();

        let reader = ReplayReader::new(&buffer).unwrap();
        assert_eq!(reader.records().unwrap(), vec![record]);
    }

    #[test]
    fn pre_e7_lrp_with_only_kinds_0_to_3_decodes_unchanged() {
        // Old-`.lrp` compatibility direction: a stream written before E7 (no scene-switch records)
        // still decodes every record — the doc-reload/command/input/keyframe mix is unaffected by the
        // new kind's addition (the `a_header_without_a_profile_key_decodes_as_none` posture, applied
        // to the record vocabulary rather than the header).
        let bytes = sample_session();
        let reader = ReplayReader::new(&bytes).unwrap();
        let records = reader.records().unwrap();
        assert_eq!(records.len(), 5);
        assert!(records.iter().all(|r| !matches!(
            r,
            ReplayRecord::DocReload { .. }
                | ReplayRecord::SceneSwitch { .. }
                | ReplayRecord::SceneLoad { .. }
        )));
    }

    #[test]
    fn scene_load_record_round_trips() {
        // A scene-load record (kind 5) writes and reads back intact, carrying the text the
        // mid-session `scene.load` resolved — so a replay never re-reads the path off disk.
        let mut buffer = Vec::new();
        let mut writer = ReplayWriter::new(&mut buffer, &header()).unwrap();
        writer
            .write_command(FrameIndex::new(1), &Command::SimResume)
            .unwrap();
        writer
            .write_scene_load(
                FrameIndex::new(4),
                "scenes/arena.yaml",
                "format: 1\nkind: scene\nseed: 0\nentities: []\n",
            )
            .unwrap();
        writer.into_inner().unwrap();

        let reader = ReplayReader::new(&buffer).unwrap();
        let records = reader.records().unwrap();
        assert_eq!(records.len(), 2);
        assert_eq!(
            records[1],
            ReplayRecord::SceneLoad {
                frame: FrameIndex::new(4),
                path: "scenes/arena.yaml".into(),
                document: "format: 1\nkind: scene\nseed: 0\nentities: []\n".into(),
            }
        );
        let seek = reader.seek(FrameIndex::new(5)).unwrap();
        assert!(seek.keyframe.is_none());
        assert_eq!(seek.deltas.len(), 2);
        assert_eq!(seek.deltas[1].frame(), FrameIndex::new(4));
    }

    #[test]
    fn write_record_round_trips_a_scene_load() {
        let record = ReplayRecord::SceneLoad {
            frame: FrameIndex::new(7),
            path: "scenes/arena.yaml".into(),
            document: "format: 1\nkind: scene\nseed: 0\nentities: []\n".into(),
        };
        let mut buffer = Vec::new();
        let mut writer = ReplayWriter::new(&mut buffer, &header()).unwrap();
        writer.write_record(&record).unwrap();
        writer.into_inner().unwrap();

        let reader = ReplayReader::new(&buffer).unwrap();
        assert_eq!(reader.records().unwrap(), vec![record]);
    }

    #[test]
    fn prefab_text_record_round_trips() {
        let mut buffer = Vec::new();
        let mut writer = ReplayWriter::new(&mut buffer, &header()).unwrap();
        writer
            .write_command(FrameIndex::new(1), &Command::SimResume)
            .unwrap();
        writer
            .write_prefab_text(
                FrameIndex::new(2),
                "prefabs/crate.prefab.yaml",
                "format: 1\nkind: prefab\nname: crate\nentities: []\n",
                false,
            )
            .unwrap();
        writer
            .write_prefab_text(
                FrameIndex::new(3),
                "prefabs/crate.prefab.yaml",
                "edited",
                true,
            )
            .unwrap();
        writer.into_inner().unwrap();

        let reader = ReplayReader::new(&buffer).unwrap();
        let records = reader.records().unwrap();
        assert_eq!(records.len(), 3);
        assert_eq!(
            records[1],
            ReplayRecord::PrefabText {
                frame: FrameIndex::new(2),
                path: "prefabs/crate.prefab.yaml".into(),
                document: "format: 1\nkind: prefab\nname: crate\nentities: []\n".into(),
                reload: false,
            }
        );
        assert_eq!(
            records[2],
            ReplayRecord::PrefabText {
                frame: FrameIndex::new(3),
                path: "prefabs/crate.prefab.yaml".into(),
                document: "edited".into(),
                reload: true,
            }
        );
    }

    #[test]
    fn a_read_record_body_has_no_reload_key() {
        let read = serde_json::to_value(PrefabTextBody {
            path: "p",
            document: "d",
            reload: false,
        })
        .unwrap();
        assert_eq!(read, serde_json::json!({"path": "p", "document": "d"}));
        let reload = serde_json::to_value(PrefabTextBody {
            path: "p",
            document: "d",
            reload: true,
        })
        .unwrap();
        assert_eq!(reload["reload"], serde_json::json!(true));
    }

    #[test]
    fn write_record_round_trips_a_prefab_text() {
        let record = ReplayRecord::PrefabText {
            frame: FrameIndex::new(7),
            path: "prefabs/crate.prefab.yaml".into(),
            document: "format: 1\nkind: prefab\nname: crate\nentities: []\n".into(),
            reload: true,
        };
        let mut buffer = Vec::new();
        let mut writer = ReplayWriter::new(&mut buffer, &header()).unwrap();
        writer.write_record(&record).unwrap();
        writer.into_inner().unwrap();

        let reader = ReplayReader::new(&buffer).unwrap();
        assert_eq!(reader.records().unwrap(), vec![record]);
    }

    /// The writer refuses what the reader would refuse as `L3017` — and writes nothing of it.
    #[test]
    fn the_writer_refuses_a_record_past_the_reader_ceiling() {
        let mut buffer = Vec::new();
        let written = {
            let mut writer = ReplayWriter::new(&mut buffer, &header()).unwrap();
            let oversized = "x".repeat(FLOOR as usize + 1);
            let error = writer
                .write_scene_load(FrameIndex::new(1), "arena.yaml", &oversized)
                .unwrap_err();
            let refusal = error
                .get_ref()
                .and_then(|inner| inner.downcast_ref::<RecordTooLarge>())
                .expect("the refusal is a RecordTooLarge");
            assert_eq!(refusal.what, "scene_load");
            assert_eq!(
                refusal.to_diagnostic().code,
                code::REPLAY_DECOMPRESSED_TOO_LARGE
            );
            writer.into_inner().unwrap().len()
        };
        assert_eq!(
            written,
            ReplayReader::new(&buffer).unwrap().records_start,
            "a refused record leaves the buffer at the header"
        );
        assert!(
            ReplayReader::new(&buffer)
                .unwrap()
                .records()
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn the_writer_refuses_a_header_past_the_reader_ceiling() {
        let mut oversized = header();
        oversized.scene = SceneRef::Inline {
            document: "x".repeat(FLOOR as usize + 1),
        };
        let mut buffer = Vec::new();
        let error = ReplayWriter::new(&mut buffer, &oversized)
            .err()
            .expect("an over-ceiling header is refused");
        assert!(
            error
                .get_ref()
                .is_some_and(|inner| inner.is::<RecordTooLarge>())
        );
        assert!(buffer.is_empty(), "nothing is written for a refused header");
    }

    #[test]
    fn frame_ceiling_bounds() {
        // The fuzz reproducer's 274-byte body (8 MiB inflated) stays refused.
        assert!(frame_ceiling(274) < 8 * 1024 * 1024);
        assert_eq!(frame_ceiling(10), FLOOR);
        assert_eq!(frame_ceiling(usize::MAX), HARD_CEILING);
        // A 64 MiB keyframe at a realistic 30:1 ratio is admitted.
        let sixty_four_mib = 64 * 1024 * 1024;
        assert!(frame_ceiling(sixty_four_mib / 30) >= sixty_four_mib as u64);
    }

    #[test]
    fn a_keyframe_past_the_old_ceiling_round_trips() {
        let mut state = 0x2545_f491_4f6c_dd1d_u64;
        let hex: String = (0..10 * 1024 * 1024)
            .map(|_| {
                state = state
                    .wrapping_mul(6_364_136_223_846_793_005)
                    .wrapping_add(1_442_695_040_888_963_407);
                char::from_digit((state >> 60) as u32, 16).unwrap()
            })
            .collect();
        let snapshot = Value::String(hex);
        let mut buffer = Vec::new();
        let mut writer = ReplayWriter::new(&mut buffer, &header()).unwrap();
        writer
            .write_keyframe(FrameIndex::new(300), &snapshot)
            .unwrap();
        writer.into_inner().unwrap();

        let reader = ReplayReader::new(&buffer).unwrap();
        assert_eq!(
            reader.records().unwrap(),
            vec![ReplayRecord::Keyframe {
                frame: FrameIndex::new(300),
                snapshot,
            }]
        );
    }

    #[test]
    fn write_record_round_trips_a_system_panic() {
        let record = ReplayRecord::SystemPanic {
            frame: FrameIndex::new(9),
        };
        let mut buffer = Vec::new();
        let mut writer = ReplayWriter::new(&mut buffer, &header()).unwrap();
        writer.write_record(&record).unwrap();
        writer.into_inner().unwrap();

        let reader = ReplayReader::new(&buffer).unwrap();
        assert_eq!(reader.records().unwrap(), vec![record]);
    }

    #[test]
    fn unknown_record_kind_8_is_rejected() {
        // KIND_MAX is now 7 (system_panic); a kind byte of 8 is still unknown and must be rejected,
        // not silently accepted as a future kind.
        let mut bytes = sample_session();
        let records_start = ReplayReader::new(&bytes).unwrap().records_start;
        bytes[records_start] = 8; // clobber the first record's kind byte
        let reader = ReplayReader::new(&bytes).unwrap();
        let err = reader.records().unwrap_err();
        assert_eq!(err.code, code::REPLAY_FORMAT);
    }

    #[test]
    fn header_profile_round_trips_some_and_none() {
        // A header carrying an embedded profile-at-start round-trips through the writer/reader intact,
        // and a header without one comes back `None` — the additive E5 field.
        for profile in [
            None,
            Some("format: 1\nkind: profile\nunlocked_crossbow: true\nruns: 7\n".to_string()),
        ] {
            let header = ReplayHeader {
                profile: profile.clone(),
                ..header()
            };
            let mut buffer = Vec::new();
            let writer = ReplayWriter::new(&mut buffer, &header).unwrap();
            writer.into_inner().unwrap();

            let reader = ReplayReader::new(&buffer).unwrap();
            assert_eq!(reader.header().profile, profile);
            assert_eq!(reader.header(), &header);
        }
    }

    #[test]
    fn a_header_without_a_profile_key_decodes_as_none() {
        // The old-`.lrp` compatibility direction: a header JSON that predates the `profile` field
        // (the key simply absent) still parses, with `profile` defaulting to `None`.
        let json = serde_json::json!({
            "format_version": REPLAY_FORMAT_VERSION,
            "engine_version": "0.0.0",
            "protocol_version": crate::PROTOCOL_VERSION,
            "schema_hash": WireHash::from_u64(0xabcd),
            "scene": { "kind": "path", "path": "level.yaml" },
            "seed": 7,
        });
        let decoded: ReplayHeader = serde_json::from_value(json).unwrap();
        assert_eq!(decoded.profile, None);
        assert_eq!(decoded.render_extent, None);
        assert_eq!(decoded, header());
    }

    #[test]
    fn header_render_extent_round_trips_some_and_none() {
        // The render extent at arm time round-trips intact; a header without one comes back `None`.
        for render_extent in [
            None,
            Some(ReplayExtent {
                width: 960,
                height: 540,
            }),
        ] {
            let header = ReplayHeader {
                render_extent: render_extent.clone(),
                ..header()
            };
            let mut buffer = Vec::new();
            let writer = ReplayWriter::new(&mut buffer, &header).unwrap();
            writer.into_inner().unwrap();

            let reader = ReplayReader::new(&buffer).unwrap();
            assert_eq!(reader.header().render_extent, render_extent);
            assert_eq!(reader.header(), &header);
        }
    }

    #[test]
    fn wrong_version_is_rejected() {
        let mut bytes = sample_session();
        bytes[4] = 99; // clobber the format version's low byte
        assert_eq!(
            ReplayReader::new(&bytes).unwrap_err().code,
            code::REPLAY_FORMAT
        );
    }
}
