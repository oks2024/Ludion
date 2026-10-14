//! The conformance proof: the idol fixture meshes to one committed hash on every machine.

use ludion_scene::{SculptDocument, check_sculpt, emit_sculpt};
use ludion_sculpt::{SculptMesh, SculptSources, mesh, unresolved_parts};

const IDOL: &str = include_str!("fixtures/idol.sculpt.yaml");

/// XXH3 of the idol's mesh bytes (see [`mesh_bytes`]), landed from the authoring machine.
const IDOL_MESH_HASH: u64 = 0xc944_f5e5_b55f_9b79;

fn idol() -> SculptDocument {
    check_sculpt(IDOL).unwrap_or_else(|d| panic!("{d:#?}"))
}

/// Every flat list concatenated little-endian in `SculptMesh` field order: positions, normals,
/// colors, materials, indices, weights.
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
fn idol_fixture_is_canonical() {
    assert_eq!(emit_sculpt(&idol()), IDOL);
}

#[test]
fn idol_meshes_to_the_conformance_hash() {
    let mesh = mesh(&idol()).expect("the idol meshes");
    let hash = ludion_assets::content_hash(&mesh_bytes(&mesh));
    assert_eq!(
        hash, IDOL_MESH_HASH,
        "the idol's mesh bytes hash to {hash:#018x}; a change here is a determinism break unless \
         the mesher changed on purpose"
    );
}

#[test]
fn meshing_twice_is_bit_identical() {
    let doc = idol();
    let first = mesh(&doc).unwrap();
    let second = mesh(&doc).unwrap();
    assert_eq!(mesh_bytes(&first), mesh_bytes(&second));
    assert_eq!(first.stats, second.stats);
}

#[test]
fn idol_reports_its_thin_mouth() {
    let doc = idol();
    let mesh = mesh(&doc).unwrap();
    let diagnostics = unresolved_parts(&doc, &SculptSources::new(), &mesh);
    let paths: Vec<_> = diagnostics.iter().map(|d| d.path.as_deref()).collect();
    assert_eq!(paths, vec![Some("/parts/4")], "{diagnostics:#?}");
}

// The wall clock measures the mesher from outside it; nothing it reads feeds the mesh.
#[allow(clippy::disallowed_methods)]
#[test]
fn idol_mesh_timing() {
    let doc = idol();
    let start = std::time::Instant::now();
    let mesh = mesh(&doc).unwrap();
    let elapsed = start.elapsed();
    println!(
        "idol meshed in {:.2} ms: {} cells, {} surface cells, {} -> {} triangles, {} vertices",
        elapsed.as_secs_f64() * 1000.0,
        mesh.stats.cells,
        mesh.stats.surface_cells,
        mesh.stats.triangles_before,
        mesh.stats.triangles_after,
        mesh.stats.vertices
    );
    println!("{:#?}", mesh.stats);
}
