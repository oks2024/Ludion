//! The `shape: mesh` conformance proof: a triangle soup's field is a pure function of the mesh
//! document's bytes, so the pebble fixture meshes to one committed hash on every machine and at
//! every thread count.

use ludion_core::par;
use ludion_scene::{
    MeshDocument, SculptDocument, check_mesh, check_sculpt, emit_mesh, emit_sculpt,
};
use ludion_sculpt::{SculptMesh, SculptSources, mesh_paths, mesh_with};

const PEBBLE_MESH: &str = include_str!("fixtures/pebble.mesh.yaml");
const SHELL_MESH: &str = include_str!("fixtures/shell.mesh.yaml");
const PEBBLE_SCULPT: &str = include_str!("fixtures/pebble.sculpt.yaml");

/// XXH3 of the pebble's mesh bytes (see [`mesh_bytes`]), landed from the authoring machine
/// (Windows x86-64). The two pinned boxes must agree on it: a mismatch on the other one is a
/// determinism bug in the field, never a per-OS constant.
const PEBBLE_MESH_HASH: u64 = 0x4e99_6a69_41d6_e7c9;

fn document(source: &str) -> SculptDocument {
    check_sculpt(source).unwrap_or_else(|d| panic!("{d:#?}"))
}

fn source_document(source: &str) -> MeshDocument {
    check_mesh(source).unwrap_or_else(|d| panic!("{d:#?}"))
}

/// The pebble sculpt meshed against a mesh source — the fixture's own by default.
fn pebble_with(source: &str) -> SculptMesh {
    let doc = document(PEBBLE_SCULPT);
    let mut sources = SculptSources::new();
    sources.insert_mesh("pebble.mesh.yaml", &source_document(source));
    mesh_with(&doc, &sources).expect("the pebble meshes")
}

/// Every flat list concatenated little-endian in `SculptMesh` field order, the `idol.rs` shape.
fn mesh_bytes(mesh: &SculptMesh) -> Vec<u8> {
    let floats = [
        &mesh.positions,
        &mesh.normals,
        &mesh.colors,
        &mesh.materials,
    ];
    let mut bytes: Vec<u8> = floats
        .iter()
        .flat_map(|list| list.iter())
        .flat_map(|v| v.to_le_bytes())
        .collect();
    bytes.extend(mesh.indices.iter().flat_map(|i| i.to_le_bytes()));
    bytes.extend(mesh.weights.iter().flat_map(|w| w.to_le_bytes()));
    bytes
}

#[test]
fn the_fixtures_are_canonical() {
    assert_eq!(emit_sculpt(&document(PEBBLE_SCULPT)), PEBBLE_SCULPT);
    assert_eq!(emit_mesh(&source_document(PEBBLE_MESH)), PEBBLE_MESH);
    assert_eq!(emit_mesh(&source_document(SHELL_MESH)), SHELL_MESH);
}

#[test]
fn the_document_names_its_source() {
    assert_eq!(mesh_paths(&document(PEBBLE_SCULPT)), ["pebble.mesh.yaml"]);
    // Without it the same document refuses, naming the part that needs one.
    let error = ludion_sculpt::mesh(&document(PEBBLE_SCULPT)).unwrap_err();
    assert_eq!(error.path.as_deref(), Some("/parts/0/path"));
    assert!(error.message.contains("shape: mesh"), "{}", error.message);
}

#[test]
fn the_pebble_meshes_to_the_conformance_hash() {
    let mesh = pebble_with(PEBBLE_MESH);
    let hash = ludion_assets::content_hash(&mesh_bytes(&mesh));
    assert_eq!(
        hash, PEBBLE_MESH_HASH,
        "the pebble's mesh bytes hash to {hash:#018x}; a change here is a determinism break unless \
         the field changed on purpose"
    );
}

/// The lattice a mesh source is sampled on is filled in parallel, so the one property that makes
/// that legal is asserted directly: the same bytes at one worker and at eight.
#[test]
fn the_field_is_the_same_at_every_thread_count() {
    let bytes = |threads| par::with_threads(threads, || mesh_bytes(&pebble_with(PEBBLE_MESH)));
    let one = bytes(1);
    assert_eq!(one, bytes(2));
    assert_eq!(one, bytes(8));
}

#[test]
fn meshing_twice_is_bit_identical() {
    let first = pebble_with(PEBBLE_MESH);
    let second = pebble_with(PEBBLE_MESH);
    assert_eq!(mesh_bytes(&first), mesh_bytes(&second));
    assert_eq!(first.stats, second.stats);
}

/// The part is placed by its `at` at `size` world units per document unit, and the sphere blended
/// onto it belongs to the same surface — one piece, not two.
#[test]
fn the_part_carries_the_meshs_own_extent() {
    let mesh = pebble_with(PEBBLE_MESH);
    // The fixture's ellipsoid spans 1.0 x 0.7 x 0.8 document units, so at `size: 1.5` the part's own
    // surface is 1.5 x 1.05 x 1.2 world units, give or take the cell the lattice resolves it to and
    // the blend the bead melts into it.
    let (lo, hi) = mesh.stats.parts[0]
        .bounds
        .expect("the mesh part has surface");
    let span = [0, 1, 2].map(|axis| hi[axis] - lo[axis]);
    for (axis, expected) in [1.5f32, 1.05, 1.2].iter().enumerate() {
        assert!(
            (span[axis] - expected).abs() < 0.1,
            "axis {axis}: {span:?} against {expected}"
        );
    }
    assert_eq!(mesh.stats.components, 1, "{:#?}", mesh.stats);
    assert!(
        mesh.stats
            .parts
            .iter()
            .all(|part| part.surface_vertices > 0)
    );
}

/// The sign comes from the generalised winding number, so a source with its cap cut away — what a
/// generator hands back on a bad day — still meshes to a solid rather than to nothing.
#[test]
fn an_open_source_still_meshes_to_a_solid() {
    let closed = pebble_with(PEBBLE_MESH);
    let open = pebble_with(SHELL_MESH);
    assert!(open.stats.triangles_after > 0, "{:#?}", open.stats);
    assert!(
        open.stats.vertices as f32 > 0.5 * closed.stats.vertices as f32,
        "open {} against closed {}",
        open.stats.vertices,
        closed.stats.vertices
    );
    assert!(open.stats.parts[0].surface_vertices > 0);
}

// The wall clock measures the mesher from outside it; nothing it reads feeds the mesh.
#[allow(clippy::disallowed_methods)]
#[test]
fn mesh_source_timing() {
    let doc = source_document(PEBBLE_MESH);
    let start = std::time::Instant::now();
    let mut sources = SculptSources::new();
    sources.insert_mesh("pebble.mesh.yaml", &doc);
    let sampled = start.elapsed();
    let folded = std::time::Instant::now();
    let mesh = mesh_with(&document(PEBBLE_SCULPT), &sources).expect("the pebble meshes");
    println!(
        "pebble: {} triangles sampled in {:.2} ms, folded in {:.2} ms ({} cells, {} -> {} \
         triangles)",
        doc.triangle_count(),
        sampled.as_secs_f64() * 1000.0,
        folded.elapsed().as_secs_f64() * 1000.0,
        mesh.stats.cells,
        mesh.stats.triangles_before,
        mesh.stats.triangles_after,
    );
}
