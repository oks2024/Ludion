//! Replay compatibility: the content hash a replay header records, and the gate that compares it.
//!
//! A replay holds three schemas beyond the wire: its record shape (every [`Command`] it embeds), the
//! input events, and the keyframe snapshot document. [`replay_content_hash`] covers all three, with
//! prose stripped, so editing a doc comment never invalidates a recording but a new field does. One
//! exception: a *first* doc comment on a unit variant of a string enum moves its schema from `enum`
//! to `oneOf`, which is a shape change and re-records. The header also carries the
//! description-stripped wire hash ([`schema::replay_schema_hash`]).
//!
//! The gate refuses a replay whose protocol version, content hash or wire schema hash differs from
//! this engine's, or whose header carries no content hash, all as `L3010`. The game half
//! ([`check_game`]) runs after the game's engine is built, since it needs the game's registry.

use std::sync::OnceLock;

use ludion_core::codes::code;
use ludion_core::diagnostics::Diagnostic;
use ludion_protocol::replay::{GameIdentity, ReplayHeader};
use ludion_protocol::{PROTOCOL_VERSION, WireHash, schema};
use ludion_sim::InputEvent;
use serde_json::json;

/// The hash of everything a replay holds: record schema, input event schema and snapshot document
/// schema, prose stripped. Cached: the schemas are fixed for the process.
///
/// # Example
/// ```
/// let hash = ludion_engine::replay_content_hash();
/// assert_eq!(hash, ludion_engine::replay_content_hash());
/// ```
pub fn replay_content_hash() -> WireHash {
    static HASH: OnceLock<WireHash> = OnceLock::new();
    *HASH.get_or_init(|| schema::canonical_hash(&schema::strip_prose(content_schema())))
}

/// The unhashed document [`replay_content_hash`] covers. The rapier pin stays out on purpose: the
/// pin ritual runs the replay gates across a pin move as its verdict, so the pin bites only when a
/// physics blob is decoded.
fn content_schema() -> serde_json::Value {
    json!({
        "record": schema::replay_record_schema(),
        "input_event": schema::schema_of::<InputEvent>(),
        "snapshot": crate::snapshot::snapshot_schema(),
        "physics_blob": ludion_physics::PHYSICS_BLOB_VERSION,
    })
}

/// Rejects a replay this engine cannot trust to reproduce: see [`compatibility`].
pub(crate) fn check_compatible(header: &ReplayHeader) -> Result<(), Diagnostic> {
    compatibility(header, schema::replay_schema_hash(), replay_content_hash())
}

/// The gate. A `protocol_version` mismatch, an absent content hash, a content-hash mismatch or a
/// schema-hash mismatch refuses, all as `L3010`.
fn compatibility(
    header: &ReplayHeader,
    schema_hash: WireHash,
    content_hash: WireHash,
) -> Result<(), Diagnostic> {
    const RERECORD: &str = "re-record the session with this engine build: run the owning test \
                            with LUDION_BLESS=1, or record again with --record";
    if header.protocol_version != PROTOCOL_VERSION {
        return Err(Diagnostic::new(
            code::REPLAY_FORMAT,
            format!(
                "replay was recorded against protocol version {}, but this engine speaks \
                 version {PROTOCOL_VERSION}",
                header.protocol_version
            ),
        )
        .with_help(RERECORD));
    }
    let Some(recorded) = header.replay_content_hash else {
        return Err(Diagnostic::new(
            code::REPLAY_FORMAT,
            format!(
                "replay (engine {}) predates the replay content hash, so this engine cannot tell \
                 whether its records and snapshots still decode as recorded; this is not a \
                 determinism failure",
                header.engine_version
            ),
        )
        .with_help(RERECORD));
    };
    if recorded != content_hash {
        return Err(Diagnostic::new(
            code::REPLAY_FORMAT,
            format!(
                "replay content hash {recorded} differs from this engine's {content_hash}: the \
                 record schema, the input events or the snapshot document changed since \
                 recording (engine {})",
                header.engine_version
            ),
        )
        .with_help(RERECORD));
    }
    if header.schema_hash != schema_hash {
        return Err(Diagnostic::new(
            code::REPLAY_FORMAT,
            format!(
                "replay was recorded against wire schema {}, but this engine speaks {schema_hash}",
                header.schema_hash
            ),
        )
        .with_help(RERECORD));
    }
    Ok(())
}

/// The game half of the gate: a header carrying a game identity that differs from the replaying
/// build's, in schema or version, is refused with `L1404`. An absent identity (a session recording
/// or an older file) passes.
pub(crate) fn check_game(header: &ReplayHeader, current: &GameIdentity) -> Result<(), Diagnostic> {
    let Some(recorded) = &header.game else {
        return Ok(());
    };
    if recorded == current {
        return Ok(());
    }
    let version = |v: &Option<String>| match v {
        Some(v) => format!("game version {v}"),
        None => "an unversioned game".to_string(),
    };
    Err(Diagnostic::new(
        code::REPLAY_GAME_MISMATCH,
        format!(
            "replay was recorded by {} (schema {}, engine {}); this build is {} (schema {}, engine {})",
            version(&recorded.version),
            recorded.schema,
            header.engine_version,
            version(&current.version),
            current.schema,
            crate::ENGINE_VERSION,
        ),
    )
    .with_help(
        "this build cannot replay the file: list it as recorded by another version, or re-record \
         it under this build (the schema covers engine built-ins too, so an engine component \
         change also reads as a mismatch)",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use ludion_protocol::replay::{ReplayReader, SceneRef};

    fn header() -> ReplayHeader {
        ReplayHeader {
            format_version: ludion_protocol::replay::REPLAY_FORMAT_VERSION,
            engine_version: "0.0.0".into(),
            protocol_version: PROTOCOL_VERSION,
            schema_hash: schema::replay_schema_hash(),
            scene: SceneRef::Inline {
                document: String::new(),
            },
            seed: 1,
            profile: None,
            render_extent: None,
            replay_content_hash: Some(replay_content_hash()),
            game: None,
        }
    }

    fn check(h: &ReplayHeader) -> Result<(), Diagnostic> {
        compatibility(h, schema::replay_schema_hash(), replay_content_hash())
    }

    #[test]
    fn matching_header_passes() {
        assert!(check(&header()).is_ok());
    }

    #[test]
    fn protocol_mismatch_refuses() {
        let h = ReplayHeader {
            protocol_version: PROTOCOL_VERSION + 1,
            ..header()
        };
        assert_eq!(check(&h).unwrap_err().code, code::REPLAY_FORMAT);
    }

    #[test]
    fn each_arm_refuses_with_its_own_message() {
        let absent = ReplayHeader {
            replay_content_hash: None,
            ..header()
        };
        let message = check(&absent).unwrap_err().message;
        assert!(
            message.contains("predates the replay content hash"),
            "{message}"
        );
        assert!(message.contains("0.0.0") && message.contains("not a determinism failure"));

        let content = ReplayHeader {
            replay_content_hash: Some(WireHash::from_u64(2)),
            ..header()
        };
        let message = check(&content).unwrap_err().message;
        assert!(
            message.contains("content hash")
                && message.contains(&replay_content_hash().to_string())
        );

        let wire = ReplayHeader {
            schema_hash: WireHash::from_u64(1),
            ..header()
        };
        let message = check(&wire).unwrap_err().message;
        assert!(
            message.contains("wire schema") && message.contains(&WireHash::from_u64(1).to_string())
        );
    }

    #[test]
    fn content_covers_snapshot_applied_and_input_events() {
        let document = content_schema();
        let text = document["snapshot"].to_string();
        assert!(
            text.contains("\"applied\""),
            "DocReloadDoc.applied must be covered"
        );
        assert!(
            text.contains("\"render_extent\""),
            "the keyframe render extent must be covered"
        );
        assert_eq!(
            document["physics_blob"],
            ludion_physics::PHYSICS_BLOB_VERSION
        );
        assert!(document["input_event"].to_string().contains("\"Left\""));
    }

    #[test]
    fn a_fresh_recording_carries_the_stripped_hash_and_content_hash() {
        let engine = crate::Engine::new();
        let header = engine.replay_header();
        assert_eq!(header.schema_hash, schema::replay_schema_hash());
        assert_eq!(header.replay_content_hash, Some(replay_content_hash()));
    }

    #[test]
    fn old_format_fixture_gets_the_old_format_message() {
        let bytes = std::fs::read(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../ludion-protocol/tests/fixtures/vigil-stress.lrp"
        ))
        .expect("the committed old-header fixture");
        let reader = ReplayReader::new(&bytes).expect("the fixture decodes");
        let error = check(reader.header()).unwrap_err();
        assert!(
            error.message.contains("predates the replay content hash"),
            "{}",
            error.message
        );
    }

    fn identity(schema: u64, version: Option<&str>) -> GameIdentity {
        GameIdentity {
            schema: WireHash::from_u64(schema),
            version: version.map(String::from),
        }
    }

    fn with_game(game: Option<GameIdentity>) -> ReplayHeader {
        ReplayHeader { game, ..header() }
    }

    #[test]
    fn game_absent_or_equal_passes() {
        let now = identity(1, Some("1.0"));
        assert!(check_game(&with_game(None), &now).is_ok());
        assert!(check_game(&with_game(Some(now.clone())), &now).is_ok());
    }

    #[test]
    fn game_schema_difference_refuses_naming_both_hashes() {
        let err = check_game(&with_game(Some(identity(1, None))), &identity(2, None)).unwrap_err();
        assert_eq!(err.code, code::REPLAY_GAME_MISMATCH);
        assert!(err.message.contains(&WireHash::from_u64(1).to_string()));
        assert!(err.message.contains(&WireHash::from_u64(2).to_string()));
    }

    #[test]
    fn game_version_difference_refuses_naming_both_versions() {
        let err = check_game(
            &with_game(Some(identity(1, Some("1.2")))),
            &identity(1, Some("1.3")),
        )
        .unwrap_err();
        assert_eq!(err.code, code::REPLAY_GAME_MISMATCH);
        assert!(err.message.contains("1.2") && err.message.contains("1.3"));
    }

    #[test]
    fn game_version_against_none_refuses_as_unversioned() {
        let err = check_game(
            &with_game(Some(identity(1, Some("1.2")))),
            &identity(1, None),
        )
        .unwrap_err();
        assert_eq!(err.code, code::REPLAY_GAME_MISMATCH);
        assert!(err.message.contains("unversioned") && err.message.contains("1.2"));
    }
}
