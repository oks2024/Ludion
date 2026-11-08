//! Mesh LOD chains from `ludion_import::import`: generated levels (`--lod <screen>:<ratio>`) and
//! authored `<name>_LOD<n>` sibling nodes.

use ludion_import::{ExternalFiles, Import, ImportOptions, LodSpec, import};

/// One mesh of a test glTF: an `n × n` quad grid, optionally on a double-sided material.
struct Grid {
    n: usize,
    double_sided: bool,
}

fn grid_bytes(n: usize) -> (Vec<u8>, usize, usize) {
    let side = n + 1;
    let mut bytes = Vec::new();
    for y in 0..side {
        for x in 0..side {
            let (fx, fy) = (x as f32 / n as f32, y as f32 / n as f32);
            for v in [fx, fy, 0.05 * (fx * 6.0).sin() * (fy * 5.0).cos()] {
                bytes.extend(v.to_le_bytes());
            }
        }
    }
    let vertex_bytes = bytes.len();
    let mut indices = 0;
    for y in 0..n {
        for x in 0..n {
            let a = (y * side + x) as u32;
            for i in [
                a,
                a + 1,
                a + side as u32,
                a + 1,
                a + side as u32 + 1,
                a + side as u32,
            ] {
                bytes.extend(i.to_le_bytes());
                indices += 1;
            }
        }
    }
    (bytes, vertex_bytes, indices)
}

/// A glTF with one mesh per grid and one root node per `(name, mesh)`.
fn gltf(grids: &[Grid], nodes: &[(&str, usize)]) -> String {
    let mut buffer = Vec::new();
    let (mut views, mut accessors, mut meshes, mut materials) =
        (Vec::new(), Vec::new(), Vec::new(), Vec::new());
    for (mesh, grid) in grids.iter().enumerate() {
        let (bytes, vertex_bytes, indices) = grid_bytes(grid.n);
        let at = buffer.len();
        buffer.extend(&bytes);
        let side = grid.n + 1;
        views.push(format!(
            r#"{{"buffer":0,"byteOffset":{at},"byteLength":{vertex_bytes},"target":34962}}"#
        ));
        views.push(format!(
            r#"{{"buffer":0,"byteOffset":{},"byteLength":{},"target":34963}}"#,
            at + vertex_bytes,
            bytes.len() - vertex_bytes
        ));
        accessors.push(format!(
            r#"{{"bufferView":{},"componentType":5126,"count":{},"type":"VEC3","min":[0,0,-0.1],"max":[1,1,0.1]}}"#,
            mesh * 2,
            side * side
        ));
        accessors.push(format!(
            r#"{{"bufferView":{},"componentType":5125,"count":{indices},"type":"SCALAR"}}"#,
            mesh * 2 + 1
        ));
        materials.push(format!(r#"{{"doubleSided":{}}}"#, grid.double_sided));
        meshes.push(format!(
            r#"{{"name":"m{mesh}","primitives":[{{"attributes":{{"POSITION":{}}},"indices":{},"material":{mesh}}}]}}"#,
            mesh * 2,
            mesh * 2 + 1
        ));
    }
    let nodes: Vec<String> = nodes
        .iter()
        .map(|(name, mesh)| format!(r#"{{"name":"{name}","mesh":{mesh}}}"#))
        .collect();
    format!(
        r#"{{"asset":{{"version":"2.0"}},"nodes":[{}],"meshes":[{}],"materials":[{}],"accessors":[{}],"bufferViews":[{}],"buffers":[{{"byteLength":{},"uri":"data:application/octet-stream;base64,{}"}}]}}"#,
        nodes.join(","),
        meshes.join(","),
        materials.join(","),
        accessors.join(","),
        views.join(","),
        buffer.len(),
        base64::encode(&buffer)
    )
}

fn run(json: &str, lods: &[LodSpec]) -> Result<Import, Vec<ludion_core::diagnostics::Diagnostic>> {
    import(
        json.as_bytes(),
        ExternalFiles::Refused,
        &ImportOptions {
            allow_lossy: false,
            lods: lods.to_vec(),
        },
    )
}

fn spec(screen_size: f64, ratio: Option<f64>) -> LodSpec {
    LodSpec { screen_size, ratio }
}

fn messages(errors: &[ludion_core::diagnostics::Diagnostic]) -> String {
    errors
        .iter()
        .map(|e| format!("{} {}", e.code.as_str(), e.message))
        .collect::<Vec<_>>()
        .join("\n")
}

fn plain(n: usize) -> Grid {
    Grid {
        n,
        double_sided: false,
    }
}

#[test]
fn no_lod_options_write_no_chain() {
    let import = run(&gltf(&[plain(8)], &[("rock", 0)]), &[]).unwrap();
    assert_eq!(import.meshes.len(), 1);
    assert!(import.meshes[0].doc.lods.is_empty());
}

#[test]
fn generated_levels_are_written_beside_the_base_and_listed_in_it() {
    let json = gltf(&[plain(16)], &[("rock", 0)]);
    let import = run(&json, &[spec(0.25, Some(0.5)), spec(0.1, Some(0.25))]).unwrap();
    let paths: Vec<&str> = import.meshes.iter().map(|m| m.rel_path.as_str()).collect();
    assert_eq!(
        paths,
        [
            "meshes/m0.mesh.yaml",
            "meshes/m0.lod1.mesh.yaml",
            "meshes/m0.lod2.mesh.yaml"
        ]
    );
    let base = &import.meshes[0].doc;
    assert_eq!(
        base.lods
            .iter()
            .map(|l| (l.mesh.as_str(), l.screen_size))
            .collect::<Vec<_>>(),
        [("m0.lod1.mesh.yaml", 0.25), ("m0.lod2.mesh.yaml", 0.1)]
    );
    assert!(import.meshes[1].doc.indices.len() < base.indices.len());
    assert!(import.meshes[2].doc.indices.len() < import.meshes[1].doc.indices.len());
    for level in &import.meshes[1..] {
        assert!(level.doc.lods.is_empty());
    }
    // The whole plan stays valid, as the tree writer checks it.
    for mesh in &import.meshes {
        ludion_scene::check_mesh(&ludion_scene::emit_mesh(&mesh.doc)).expect("a valid document");
    }
}

#[test]
fn a_generated_chain_is_byte_reproducible() {
    let json = gltf(&[plain(16)], &[("rock", 0)]);
    let emit = || -> Vec<String> {
        run(&json, &[spec(0.25, Some(0.5))])
            .unwrap()
            .meshes
            .iter()
            .map(|m| format!("{}\n{}", m.rel_path, ludion_scene::emit_mesh(&m.doc)))
            .collect()
    };
    assert_eq!(emit(), emit());
}

#[test]
fn bad_level_requests_are_l1395() {
    let json = gltf(&[plain(8)], &[("rock", 0)]);
    for (lods, needle) in [
        (vec![spec(0.25, Some(1.5))], "ratio"),
        (
            vec![spec(0.1, Some(0.5)), spec(0.2, Some(0.25))],
            "strictly decreasing",
        ),
        (
            vec![spec(0.5, Some(0.5)), spec(0.4, None)],
            "some LOD levels carry a ratio",
        ),
        (
            vec![
                spec(0.5, Some(0.5)),
                spec(0.4, Some(0.4)),
                spec(0.3, Some(0.3)),
                spec(0.2, Some(0.2)),
            ],
            "at most 3",
        ),
        (vec![spec(0.5, None)], "this glTF has none"),
    ] {
        let errors = run(&json, &lods).expect_err("refused");
        let text = messages(&errors);
        assert!(text.contains("L1395") && text.contains(needle), "{text}");
    }
}

#[test]
fn authored_level_nodes_become_level_documents_and_no_parts() {
    let json = gltf(
        &[plain(8), plain(4), plain(2)],
        &[("rock", 0), ("rock_LOD1", 1), ("rock_LOD2", 2)],
    );
    let import = run(&json, &[]).unwrap();
    assert_eq!(import.prefab.entities.len(), 1, "level nodes are no parts");
    let paths: Vec<&str> = import.meshes.iter().map(|m| m.rel_path.as_str()).collect();
    assert_eq!(
        paths,
        [
            "meshes/m0.mesh.yaml",
            "meshes/m0.lod1.mesh.yaml",
            "meshes/m0.lod2.mesh.yaml"
        ]
    );
    assert_eq!(
        import.meshes[0]
            .doc
            .lods
            .iter()
            .map(|l| l.screen_size)
            .collect::<Vec<_>>(),
        [0.5, 0.25]
    );
    assert_eq!(import.meshes[1].doc.indices.len(), 4 * 4 * 6);
}

#[test]
fn authored_thresholds_come_from_ratio_free_specs() {
    let json = gltf(&[plain(8), plain(4)], &[("rock_LOD0", 0), ("rock_LOD1", 1)]);
    let import = run(&json, &[spec(0.3, None)]).unwrap();
    assert_eq!(import.meshes[0].doc.lods[0].screen_size, 0.3);
    let errors = run(&json, &[spec(0.3, None), spec(0.1, None)]).expect_err("count differs");
    assert!(messages(&errors).contains("1 authored LOD levels but 2"));
}

#[test]
fn authored_chain_errors_are_readable() {
    let gap = gltf(&[plain(8), plain(4)], &[("rock", 0), ("rock_LOD2", 1)]);
    let errors = run(&gap, &[]).expect_err("a gap");
    assert!(
        messages(&errors).contains("no gap or repeat"),
        "{}",
        messages(&errors)
    );

    let orphan = gltf(&[plain(4)], &[("rock_LOD1", 0)]);
    let errors = run(&orphan, &[]).expect_err("no base");
    assert!(
        messages(&errors).contains("no node `rock`"),
        "{}",
        messages(&errors)
    );

    let mismatch = gltf(
        &[
            plain(8),
            Grid {
                n: 4,
                double_sided: true,
            },
        ],
        &[("rock", 0), ("rock_LOD1", 1)],
    );
    let errors = run(&mismatch, &[]).expect_err("a draw-key mismatch");
    let text = messages(&errors);
    assert!(
        text.contains("L1396") && text.contains("double_sided"),
        "{text}"
    );

    let mixed = gltf(&[plain(8), plain(4)], &[("rock", 0), ("rock_LOD1", 1)]);
    let errors = run(&mixed, &[spec(0.3, Some(0.5))]).expect_err("two sources");
    assert!(
        messages(&errors).contains("one source") || messages(&errors).contains("also requested")
    );

    let too_far = gltf(&[plain(8), plain(4)], &[("rock", 0), ("rock_LOD4", 1)]);
    let errors = run(&too_far, &[]).expect_err("level 4");
    assert!(
        messages(&errors).contains("at most 3"),
        "{}",
        messages(&errors)
    );
}

#[test]
fn a_name_that_only_looks_like_a_level_is_an_ordinary_node() {
    let json = gltf(&[plain(4), plain(4)], &[("rock", 0), ("rock_LODge", 1)]);
    let import = run(&json, &[]).unwrap();
    assert_eq!(import.prefab.entities.len(), 2);
}
