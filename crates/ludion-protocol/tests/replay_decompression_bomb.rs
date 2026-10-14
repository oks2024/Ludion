//! Regression test for the LRP1 replay-reader decompression bomb (Phase 9 Wave 3 fuzz finding
//! `replay_reader/zstd-decompression-bomb`).
//!
//! The committed fixture is a well-formed 404-byte LRP1 file whose single keyframe body is a valid
//! zstd frame that inflates to 8 MiB of zeros (a 30,615x expansion) — far past the reader's ratio
//! bound (a 274-byte body gets a 1 MiB ceiling). Before the fix, `records()` called
//! `zstd::decode_all` with no cap and allocated the full 8 MiB before the downstream JSON parse
//! failed (`L3010`); scaling the frame scaled RAM without bound. The fix streams each frame under
//! its ceiling and refuses once the output would exceed it, so this asserts the reader returns
//! `L3017` instead of inflating.

use ludion_core::codes::code;
use ludion_protocol::replay::ReplayReader;

/// The minimized reproducer, committed alongside this test.
const BOMB: &[u8] = include_bytes!("zstd-decompression-bomb.lrp");

#[test]
fn decompression_bomb_is_capped_not_inflated() {
    // The header is legitimate and tiny, so opening the reader succeeds — the bomb is in a record.
    let reader = ReplayReader::new(BOMB).expect("header parses; the bomb is in the keyframe body");

    // Decoding the records must refuse the over-ceiling keyframe with the structured L3017
    // diagnostic, rather than allocating the full 8 MiB decompressed body.
    let err = reader
        .records()
        .expect_err("the keyframe body exceeds the decompressed-size ceiling");
    assert_eq!(
        err.code,
        code::REPLAY_DECOMPRESSED_TOO_LARGE,
        "expected L3017 (decompressed too large), got: {err:?}",
    );

    // `seek` walks the same decode path, so it is guarded too.
    let err = reader
        .seek(ludion_core::time::FrameIndex::new(u64::MAX))
        .expect_err("seek decodes the same over-ceiling keyframe");
    assert_eq!(err.code, code::REPLAY_DECOMPRESSED_TOO_LARGE);
}

#[test]
fn a_pre_e5_committed_replay_header_decodes_with_no_profile() {
    // This fixture was committed before the additive E5 `ReplayHeader.profile` field existed, so its
    // header JSON has no `profile` key. It must still decode, with `profile` defaulting to `None` —
    // the old-`.lrp`-still-decodes compatibility direction the additive field must preserve.
    let reader = ReplayReader::new(BOMB).expect("the pre-E5 header still parses");
    assert_eq!(
        reader.header().profile,
        None,
        "an old header lacking the key decodes as a profile-free session"
    );
}
