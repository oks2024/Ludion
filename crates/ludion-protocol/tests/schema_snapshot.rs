//! Snapshot + hash-stability test for the exported protocol schema.
//!
//! The generated schema document and its hash are the drift tripwire (engine-architecture.md §6, risk
//! register #4): a client is generated from this document, and the handshake announces its hash. If
//! the wire types change, this test fails so the snapshot and hash are re-blessed deliberately —
//! never silently.
//!
//! To re-bless after an intentional protocol change:
//!
//! ```text
//! BLESS_SCHEMA=1 cargo test -p ludion-protocol --test schema_snapshot -- --nocapture
//! ```
//!
//! The bless run writes the snapshot and then fails until `EXPECTED_SCHEMA_HASH` below holds the printed
//! hash; set it, re-run without `BLESS_SCHEMA`, and commit both files.

use ludion_protocol::schema;

/// The committed canonical schema document.
const SNAPSHOT: &str = include_str!("protocol_schema.snapshot.json");

/// The committed schema hash (16 hex digits). Re-bless when the schema changes.
const EXPECTED_SCHEMA_HASH: &str = "c3dddcfcd8111902";

#[test]
fn schema_matches_committed_snapshot() {
    let document = schema::schema_document();
    let hash = schema::schema_hash().to_string();

    if ludion_core::bless::switch_set(ludion_core::bless::BLESS_SCHEMA_ENV) {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/protocol_schema.snapshot.json"
        );
        std::fs::write(path, &document).expect("write schema snapshot");
        eprintln!("blessed protocol schema; EXPECTED_SCHEMA_HASH = {hash}");
        // `SNAPSHOT` is the pre-bless `include_str!` copy, so only the hash constant is checked.
    } else {
        assert_eq!(
            document, SNAPSHOT,
            "protocol schema changed; re-bless with `BLESS_SCHEMA=1 cargo test -p ludion-protocol \
             --test schema_snapshot -- --nocapture` and commit the snapshot + hash",
        );
    }
    assert_eq!(
        hash, EXPECTED_SCHEMA_HASH,
        "protocol schema hash changed; set EXPECTED_SCHEMA_HASH = \"{hash}\" in \
         crates/ludion-protocol/tests/schema_snapshot.rs (a BLESS_SCHEMA run has written the snapshot)",
    );
}

#[test]
fn schema_hash_is_the_hash_of_the_committed_document() {
    // Independent of BLESS: the announced hash must be exactly the digest of the committed bytes, so
    // a client that hashes the document it was generated from computes the same value.
    use ludion_protocol::WireHash;
    assert_eq!(
        schema::schema_hash(),
        WireHash::of_bytes(schema::schema_document().as_bytes())
    );
}
