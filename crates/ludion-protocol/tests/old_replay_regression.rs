//! Phase 13 (E2 input) Wave 2, decision 4: old-replay compatibility is automatic.
//!
//! `InputEvent` grew mouse/gamepad variants (`crates/ludion-sim/src/input.rs`) with analog fields
//! encoded as IEEE-754 bit patterns (Wave 1). The replay container framing did not change:
//! `REPLAY_FORMAT_VERSION` stays `1`, and an input event is (and was) stored as an opaque
//! `serde_json::Value` (`ReplayRecord::Input`) with `key_down`/`key_up` bodies byte-identical
//! before and after E2. This test is the workspace-level half of that guarantee: every committed
//! `.lrp` still opens under format version 1 and its records decode intact.
//!
//! The other half — that each file's own `WorldHash` trace is unchanged when replayed — is proven
//! by each game's own test suite: `games/breakout/tests/replay.rs` (and vigil's own suite, now
//! private) record a fresh session and assert it matches the committed `run.lrp`'s replayed trace
//! exactly.
//! `games/sokoban` and `games/hello-paddle` prove record/replay the same way but in-process, with
//! no committed `.lrp` (see each game's `CLAUDE.md` / `tests/replay.rs` / `tests/gameplay.rs`) — so
//! there is nothing for this file to decode for those two. Running `cargo test -p breakout`,
//! `-p sokoban`, `-p hello-paddle`, `-p vigil` green *is* that other half of the proof.

use ludion_protocol::replay::{REPLAY_FORMAT_VERSION, ReplayReader};

/// Every `.lrp` committed in the public tree, relative to the workspace root (breakout's, and a
/// frozen copy of vigil's `stress.lrp` — see the module doc above), paired with whether its header
/// must carry an embedded E5 profile. Breakout's predates E5 (and the game registers no profile),
/// so it is the standing pre-E5 fixture proving the old-`.lrp`-decodes-as-`None` direction; the
/// vigil one is a frozen post-G5 recording that embeds its profile-at-start (the additive-field new
/// direction), whose live trace is proven by vigil's own suite (now private).
const COMMITTED_REPLAYS: &[(&str, bool)] = &[
    ("games/breakout/run.lrp", false),
    (
        "crates/ludion-protocol/tests/fixtures/vigil-stress.lrp",
        true,
    ),
];

/// Resolves a workspace-root-relative path. `CARGO_MANIFEST_DIR` is `crates/ludion-protocol`, two
/// levels below the workspace root.
fn workspace_root_relative(path: &str) -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(path)
}

#[test]
fn committed_replays_still_decode_under_format_version_1() {
    for &(relative, has_profile) in COMMITTED_REPLAYS {
        let path = workspace_root_relative(relative);
        let bytes = std::fs::read(&path)
            .unwrap_or_else(|e| panic!("{} must be readable: {e}", path.display()));

        let reader = ReplayReader::new(&bytes)
            .unwrap_or_else(|e| panic!("{} must still parse as LRP1: {e:?}", path.display()));
        assert_eq!(
            reader.header().format_version,
            REPLAY_FORMAT_VERSION,
            "{} was recorded at a format version the current reader no longer expects",
            path.display()
        );
        assert_eq!(
            REPLAY_FORMAT_VERSION, 1,
            "the container framing did not change in E2 — this must still be version 1"
        );
        assert_eq!(
            reader.header().profile.is_some(),
            has_profile,
            "{}'s header must {} an embedded E5 profile — a pre-E5 artifact decodes as `None` \
             (the old-`.lrp`-still-decodes direction), a post-G5 vigil recording embeds its \
             profile-at-start (the additive-field direction)",
            path.display(),
            if has_profile {
                "carry"
            } else {
                "decode without"
            }
        );

        let records = reader
            .records()
            .unwrap_or_else(|e| panic!("{}'s records must decode intact: {e:?}", path.display()));
        assert!(
            !records.is_empty(),
            "{} must contain at least one record",
            path.display()
        );
    }
}
