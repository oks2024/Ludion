//! The `static-kit` room: the acceptance scene for the persistent static scene
//! (`Plan/gpu-driven-culling.md` W3, Serious Isaac 0039) — a 64 x 64 m dungeon block of about 3,000
//! unscaled kit-style statics (a 1,024-cell `MeshGrid` floor — one entity — perimeter and interior walls with
//! doorway gaps, pillars of three stacked pieces, crate piles, voxel lanterns and trees) on 2 m
//! cells, interleaved in document order with about 220 dynamics (voxel figures, primitives, eight
//! skinned mannequins) and one decal.
//!
//! **The committed scene is generated, never hand-edited.** [`generate_scene_text`] builds the source
//! document and re-emits it through the scene crate's canonical writer, so the committed text is
//! canonical by construction. It lives in
//! `examples/gallery/staged/` (outside the gallery root, so the golden suite does not discover it)
//! and carries `generated_from:`. Regenerate it with
//!
//! ```text
//! LUDION_BLESS=1 cargo nextest run -p ludion-engine --test it \
//!   -E 'test(=static_kit::the_committed_static_kit_scene_matches_a_fresh_generation)'
//! ```
//!
//! The ignored release bench `needs_gpu_static_kit_bench` reports `extract_ms` per phase and the GPU
//! frame, full against the kit ablated, from an elevated overview and from eye height in a corridor:
//!
//! ```text
//! cargo test -p ludion-engine --release --test it -- static_kit::needs_gpu_static_kit_bench --ignored --nocapture
//! ```
//!
//! Like `culling_bench`, it carries no `budgets/*.toml` entry and reuses that bench's measuring helpers.
//! Its inline budget names the instance count ([`MESH_INSTANCES`], asserted every push) and an
//! `extract_ms` ceiling for the statics-on arm ([`STATIC_KIT_EXTRACT_BUDGET_MS`]); there is no GPU ceiling.

use std::fmt::Write as _;
use std::path::PathBuf;

use ludion_components::{Name, Transform};
use ludion_core::ids::EntityId;
use ludion_engine::Engine;

use crate::culling_bench::{gallery, measure_engine, print_ablation, print_arm};

/// [`ludion_scene::Scene::generated_from`], resolved relative to
/// `examples/gallery/staged/static-kit.yaml`.
const GENERATED_FROM: &str = "../../../crates/ludion-engine/tests/static_kit.rs";
/// Cells per side; each cell is [`CELL`] metres square and the room is centred on the origin.
const CELLS: usize = 32;
const CELL: f32 = 2.0;
/// Entities named `kit-…` other than the floor (every one comes from [`kit_piece`]).
const KIT_PIECES: usize = 1_943;
/// Entities named `dyn-…`, 8 of them skinned mannequins.
const DYNAMICS: usize = 219;
const MANNEQUINS: usize = 8;
/// The fixed entities: camera, sun, two point lights, the ground, one decal.
const FIXED: usize = 6;
const EXPECTED_ENTITIES: usize = FIXED + KIT_PIECES + DYNAMICS;
/// Mesh instances one extract emits (camera-independent); the extract budget is calibrated at this count.
const MESH_INSTANCES: usize = 3_186;
/// Ceiling for one extract of the room with the static scene on, calibrated at [`MESH_INSTANCES`]:
/// measured median 0.31 ms, about 3x, on an unpinned M5 laptop (not authoritative), 2026-11-14.
/// Pinned M4 / RTX 3090 figures are owed (`Plan/owed.md`).
const STATIC_KIT_EXTRACT_BUDGET_MS: f64 = 1.0;
/// The committed scene must stay under the 1 MiB size gate with room to spare.
const MAX_SCENE_BYTES: usize = 900_000;

fn scene_path() -> PathBuf {
    gallery().join("staged/static-kit.yaml")
}

/// Wall-clock-free hash of a cell, so placement is a fixed function of `(row, col, salt)`.
fn hash(row: usize, col: usize, salt: usize) -> usize {
    (row * 73 + col * 151 + row * col * 31 + salt * 17) % 97
}

fn vec3(indent: usize, key: &str, v: [f32; 3]) -> String {
    let pad = " ".repeat(indent);
    let mut out = format!("{pad}{key}:\n");
    for x in v {
        let _ = writeln!(out, "{pad}  - {x:?}");
    }
    out
}

fn color(indent: usize, c: [f32; 3]) -> String {
    let pad = " ".repeat(indent);
    let mut out = format!("{pad}color:\n");
    for x in [c[0], c[1], c[2], 1.0] {
        let _ = writeln!(out, "{pad}  - {x:?}");
    }
    out
}

fn transform(translation: [f32; 3], yaw: f32, scale: Option<[f32; 3]>) -> String {
    let mut out = String::from("      Transform:\n");
    out.push_str(&vec3(8, "translation", translation));
    if yaw != 0.0 {
        out.push_str(&vec3(8, "rotation", [0.0, yaw, 0.0]));
    }
    if let Some(scale) = scale {
        out.push_str(&vec3(8, "scale", scale));
    }
    out
}

/// What a `Mesh` draws.
#[derive(Clone, Copy)]
enum Shape {
    Asset(&'static str),
    Voxel(&'static str),
    Cube,
    Sphere,
    Character(&'static str),
}

fn mesh(shape: Shape, tint: [f32; 3], roughness: f32, metallic: f32) -> String {
    let mut out = String::from("      Mesh:\n");
    match shape {
        Shape::Asset(path) => {
            let _ = writeln!(out, "        shape:\n          asset: {path}");
        }
        Shape::Voxel(path) => {
            let _ = writeln!(out, "        shape:\n          voxel: {path}");
        }
        Shape::Character(path) => {
            let _ = writeln!(out, "        shape:\n          character: {path}");
        }
        Shape::Cube => out.push_str("        shape: cube\n"),
        Shape::Sphere => out.push_str("        shape: sphere\n"),
    }
    out.push_str(&color(8, tint));
    let _ = writeln!(out, "        roughness: {roughness:?}");
    if metallic != 0.0 {
        let _ = writeln!(out, "        metallic: {metallic:?}");
    }
    out
}

/// The document under construction: entity count and text.
struct Doc {
    text: String,
    entities: usize,
}

impl Doc {
    fn entity(&mut self, name: &str, components: &str) {
        let _ = writeln!(self.text, "  - name: {name}\n    components:");
        self.text.push_str(components);
        self.entities += 1;
    }

    /// The single kit emitter: every `kit-…` piece except the ground goes through here, unscaled.
    #[allow(clippy::too_many_arguments)]
    fn kit_piece(
        &mut self,
        name: &str,
        at: [f32; 3],
        yaw: f32,
        shape: Shape,
        tint: [f32; 3],
        roughness: f32,
        metallic: f32,
    ) {
        let mut body = transform(at, yaw, None);
        body.push_str(&mesh(shape, tint, roughness, metallic));
        body.push_str("      Static: {}\n");
        self.entity(name, &body);
    }
}

/// World position of a cell's centre.
fn cell_centre(row: usize, col: usize) -> (f32, f32) {
    let half = CELLS as f32 / 2.0 - 0.5;
    ((col as f32 - half) * CELL, (row as f32 - half) * CELL)
}

/// What stands in a cell, decided before emission so the document can walk row-major.
#[derive(Clone, Copy, PartialEq)]
enum Cell {
    Free,
    /// A wall along X (`false`) or Z (`true`).
    Wall(bool),
    Pillar,
}

fn classify(row: usize, col: usize) -> Cell {
    let last = CELLS - 1;
    if row == 0 || row == last {
        return Cell::Wall(false);
    }
    if col == 0 || col == last {
        return Cell::Wall(true);
    }
    let row_line = row.is_multiple_of(4) && (4..=28).contains(&row);
    let col_line = col.is_multiple_of(4) && (4..=28).contains(&col);
    if row_line && col_line {
        return Cell::Pillar;
    }
    if row_line {
        return if col % 8 <= 2 && col % 8 >= 1 {
            Cell::Free
        } else {
            Cell::Wall(false)
        };
    }
    if col_line {
        return if row % 8 <= 2 && row % 8 >= 1 {
            Cell::Free
        } else {
            Cell::Wall(true)
        };
    }
    if row % 4 == 2 && col % 4 == 2 && row != 2 {
        return Cell::Pillar;
    }
    Cell::Free
}

fn room_tint(row: usize, col: usize) -> [f32; 3] {
    match (row / 4 + col / 4) % 3 {
        0 => [0.45, 0.41, 0.36],
        1 => [0.34, 0.39, 0.45],
        _ => [0.42, 0.34, 0.32],
    }
}

fn kit_ground() -> String {
    let mut out = String::from("      MeshGrid:\n        cell_size: 2.0\n        legend:\n");
    for (glyph, tint, roughness) in [
        ('a', [0.5, 0.48, 0.44], 0.9),
        ('b', [0.38, 0.4, 0.45], 0.75),
        ('c', [0.45, 0.36, 0.32], 0.85),
    ] {
        let _ = writeln!(
            out,
            "          {glyph}:\n            shape:\n              asset: meshes/kit/floor-tile.mesh.yaml"
        );
        out.push_str(&color(12, tint));
        let _ = writeln!(out, "            roughness: {roughness:?}");
    }
    out.push_str("        rows:\n");
    for row in 0..CELLS {
        let line: String = (0..CELLS)
            .map(|col| ['a', 'b', 'c'][(row / 2 + col / 2 + row * col % 3) % 3])
            .collect();
        let _ = writeln!(out, "          - {line}");
    }
    out.push_str("        orient:\n");
    for row in 0..CELLS {
        let line: String = (0..CELLS)
            .map(|col| char::from(b'0' + ((row * 3 + col * 5 + row * col / 7) % 8) as u8))
            .collect();
        let _ = writeln!(out, "          - \"{line}\"");
    }
    out
}

/// The source document, walked row-major so kit and dynamic ids interleave in document order.
fn source() -> String {
    let mut doc = Doc {
        text: String::from(
            "format: 1\nname: \"static-kit - a dungeon block of ~3k unscaled kit statics interleaved with dynamics\"\nentities:\n",
        ),
        entities: 0,
    };

    let mut camera = String::from("      Transform:\n");
    camera.push_str(&vec3(8, "translation", [-20.0, 52.0, 56.0]));
    camera.push_str(&vec3(8, "rotation", [-41.0, -19.6, 0.0]));
    camera.push_str(
        "      Camera:\n        projection:\n          type: perspective\n          fov_y_deg: 50.0\n        far: 160.0\n",
    );
    doc.entity("camera", &camera);

    let mut sun = String::from("      DirectionalLight:\n");
    sun.push_str(&vec3(8, "direction", [0.3, -0.8, -0.5]));
    sun.push_str(
        "        shadow_extent: 24.0\n        shadow_near: 1.0\n        shadow_far: 140.0\n",
    );
    sun.push_str(&vec3(8, "color", [1.0, 0.96, 0.9]));
    sun.push_str("        intensity: 1.8\n");
    doc.entity("sun", &sun);
    for (i, at) in [[-12.0, 3.5, -8.0], [14.0, 3.5, 10.0]]
        .into_iter()
        .enumerate()
    {
        let mut light = transform(at, 0.0, None);
        light.push_str("      PointLight:\n");
        light.push_str(&vec3(8, "color", [1.0, 0.85, 0.7]));
        light.push_str("        intensity: 4.0\n        range: 22.0\n");
        doc.entity(&format!("light-{i}"), &light);
    }

    let half = CELLS as f32 * CELL / 2.0;
    let mut ground = transform([-half, 0.0, -half], 0.0, None);
    ground.push_str(&kit_ground());
    ground.push_str("      Static: {}\n");
    doc.entity("kit-ground", &ground);

    let mut decal = transform([-5.0, 0.3, 3.0], 0.0, None);
    decal.push_str("      Decal:\n");
    decal.push_str(&vec3(8, "size", [4.0, 1.0, 4.0]));
    decal.push_str(
        "        base_color_texture:\n          path: assets/decal-hazard-stripes.png\n          wrap: clamp\n",
    );
    doc.entity("decal-hazard", &decal);

    const Y: f32 = 0.1;
    let mut dynamics = 0;
    let mut mannequins = 0;
    for row in 0..CELLS {
        for col in 0..CELLS {
            let (x, z) = cell_centre(row, col);
            let tint = room_tint(row, col);
            let kind = classify(row, col);
            let tag = format!("{row:02}-{col:02}");
            match kind {
                Cell::Wall(along_z) => {
                    let yaw = if along_z { 90.0 } else { 0.0 };
                    doc.kit_piece(
                        &format!("kit-wall-{tag}"),
                        [x, Y, z],
                        yaw,
                        Shape::Asset("meshes/kit/wall.mesh.yaml"),
                        tint,
                        0.9,
                        0.0,
                    );
                    doc.kit_piece(
                        &format!("kit-trim-{tag}"),
                        [x, Y, z],
                        yaw,
                        Shape::Asset("meshes/kit/wall-trim.mesh.yaml"),
                        [tint[0] * 0.6, tint[1] * 0.6, tint[2] * 0.6],
                        0.6,
                        0.2,
                    );
                }
                Cell::Pillar => {
                    for (part, asset, rough, metal) in [
                        ("base", "meshes/kit/pillar-base.mesh.yaml", 0.8, 0.0),
                        ("shaft", "meshes/kit/pillar.mesh.yaml", 0.7, 0.1),
                        ("cap", "meshes/kit/pillar-cap.mesh.yaml", 0.5, 0.3),
                    ] {
                        doc.kit_piece(
                            &format!("kit-pillar-{part}-{tag}"),
                            [x, Y, z],
                            0.0,
                            Shape::Asset(asset),
                            tint,
                            rough,
                            metal,
                        );
                    }
                }
                Cell::Free if row != 2 => {
                    let h = hash(row, col, 1);
                    if h.is_multiple_of(3) {
                        let crate_tint = [0.55 + (h % 5) as f32 * 0.04, 0.4, 0.25];
                        let asset = Shape::Asset("meshes/kit/crate.mesh.yaml");
                        for (i, (dx, dz, dy, yaw)) in [
                            (-0.5, -0.4, 0.0, 0.0),
                            (0.5, 0.3, 0.0, 20.0),
                            (-0.5, -0.4, 0.9, 10.0),
                            (0.5, -0.5, 0.0, 70.0),
                            (0.5, 0.3, 0.9, 5.0),
                        ]
                        .into_iter()
                        .enumerate()
                        {
                            doc.kit_piece(
                                &format!("kit-crate-{tag}-{i}"),
                                [x + dx, Y + dy, z + dz],
                                yaw,
                                asset,
                                crate_tint,
                                0.85,
                                0.0,
                            );
                        }
                    } else if h % 23 == 3 {
                        let (shape, name) = if h.is_multiple_of(2) {
                            (Shape::Voxel("voxels/tree.voxel.yaml"), "tree")
                        } else {
                            (Shape::Voxel("voxels/lantern.voxel.yaml"), "lantern")
                        };
                        doc.kit_piece(
                            &format!("kit-{name}-{tag}"),
                            [x, Y, z],
                            (h * 37 % 360) as f32,
                            shape,
                            [1.0, 1.0, 1.0],
                            0.8,
                            0.0,
                        );
                    }
                }
                Cell::Free => {}
            }

            if kind != Cell::Free || row == 2 || hash(row, col, 5) % 10 >= 4 {
                continue;
            }
            let k = dynamics;
            dynamics += 1;
            let at = |dy: f32| [x + 0.6, dy, z - 0.6];
            let mannequin = mannequins < MANNEQUINS && k % 30 == 7;
            let mut body;
            if mannequin {
                mannequins += 1;
                body = transform(at(Y), 0.0, None);
                body.push_str(&mesh(
                    Shape::Character("characters/mannequin.character.yaml"),
                    [0.6, 0.45, 0.35],
                    0.8,
                    0.0,
                ));
                body.push_str("      Animation:\n        clip: idle\n");
            } else {
                body = match k % 3 {
                    0 => {
                        let mut b = transform(at(Y), (k * 41 % 360) as f32, Some([0.045; 3]));
                        b.push_str(&mesh(
                            Shape::Voxel("voxels/horde-figure.voxel.yaml"),
                            [0.6, 0.45, 0.35],
                            0.8,
                            0.0,
                        ));
                        b
                    }
                    1 => {
                        let mut b = transform(at(0.4), 0.0, Some([0.6; 3]));
                        b.push_str(&mesh(Shape::Cube, [0.3, 0.6, 0.8], 0.4, 0.0));
                        b
                    }
                    _ => {
                        let mut b = transform(at(0.4), 0.0, Some([0.7; 3]));
                        b.push_str(&mesh(Shape::Sphere, [0.8, 0.3, 0.3], 0.3, 0.5));
                        b
                    }
                };
            }
            doc.entity(&format!("dyn-{k}"), &body);
        }
    }
    doc.text
}

/// Generates the canonical `static-kit.yaml` text: the source document checked and re-emitted by
/// the scene writer, which keeps exactly the fields the source states (the engine's own save would
/// spell every default and triple the file).
fn generate_scene_text() -> String {
    let engine = Engine::with_project_root(gallery());
    let mut scene = ludion_scene::check(&source(), engine.registry())
        .unwrap_or_else(|diagnostics| panic!("the source is not a valid scene: {diagnostics:?}"));
    scene.generated_from = Some(GENERATED_FROM.to_string());
    ludion_scene::emit(&scene, engine.registry())
}

#[test]
fn the_committed_static_kit_scene_matches_a_fresh_generation() {
    let generated = generate_scene_text();
    assert!(
        generated.len() < MAX_SCENE_BYTES,
        "static-kit.yaml grew to {} bytes; keep it under {MAX_SCENE_BYTES}",
        generated.len()
    );
    let path = scene_path();

    if ludion_core::bless::bless_requested() {
        std::fs::create_dir_all(path.parent().expect("a parent directory"))
            .expect("the staged directory is creatable");
        std::fs::write(&path, &generated).expect("the staged directory is writable");
        return;
    }

    let committed = std::fs::read_to_string(&path).expect("the committed static-kit scene");
    assert_eq!(
        committed, generated,
        "examples/gallery/staged/static-kit.yaml has drifted from a fresh generation; \
         re-bless with `LUDION_BLESS=1 cargo nextest run -p ludion-engine --test it \
         -E 'test(=static_kit::the_committed_static_kit_scene_matches_a_fresh_generation)'`"
    );
}

/// A bare engine rooted at the gallery with the committed scene loaded.
fn build() -> Engine {
    let mut engine = Engine::builder()
        .project_root(gallery())
        .build()
        .expect("boots");
    let text = std::fs::read_to_string(scene_path()).expect("the committed static-kit scene");
    engine
        .load_inline_scene(&text)
        .expect("the static-kit scene loads");
    engine
}

fn named(engine: &Engine, prefix: &str) -> Vec<EntityId> {
    let world = engine.world();
    world
        .entities()
        .filter(|&e| {
            world
                .get::<Name>(e)
                .ok()
                .flatten()
                .is_some_and(|n| n.0.starts_with(prefix))
        })
        .collect()
}

#[test]
fn static_kit_scene_matches_its_counts() {
    let mut engine = build();
    assert_eq!(engine.world().entity_count(), EXPECTED_ENTITIES);
    assert_eq!(named(&engine, "kit-").len(), KIT_PIECES + 1);
    assert_eq!(named(&engine, "dyn-").len(), DYNAMICS);
    let instances = engine
        .extract_present_scene()
        .expect("extracts")
        .0
        .meshes
        .len();
    assert_eq!(
        instances, MESH_INSTANCES,
        "MESH_INSTANCES is the count STATIC_KIT_EXTRACT_BUDGET_MS is calibrated at; a changed room re-calibrates it"
    );
}

/// Eye height inside the row-2 corridor, looking down its length (+X).
fn enter_corridor(engine: &mut Engine) {
    let camera = named(engine, "camera")[0];
    let t = engine
        .world_mut()
        .get_mut::<Transform>(camera)
        .expect("the camera has a transform")
        .expect("the camera has a transform");
    t.translation = [-29.0, 1.7, -27.0];
    t.rotation = [0.0, -90.0, 0.0];
}

fn ablate_kit(engine: &mut Engine) {
    for id in named(engine, "kit-") {
        engine.world_mut().despawn(id).expect("despawns");
    }
}

#[test]
#[ignore = "release-mode benchmark; run explicitly (see the module docs)"]
fn needs_gpu_static_kit_bench() {
    for (view, corridor) in [("Overview", false), ("Floor", true)] {
        let prepare = |ablated: bool| {
            let mut engine = build();
            if ablated {
                ablate_kit(&mut engine);
            }
            if corridor {
                enter_corridor(&mut engine);
            }
            engine
        };
        let instances = |engine: &mut Engine| {
            engine
                .extract_present_scene()
                .expect("extracts")
                .0
                .meshes
                .len()
        };
        let mut full_engine = prepare(false);
        let full_instances = instances(&mut full_engine);
        let full = measure_engine(&format!("{view} / full"), full_engine);
        let mut ablated_engine = prepare(true);
        let ablated_instances = instances(&mut ablated_engine);
        let ablated = measure_engine(&format!("{view} / kit ablated"), ablated_engine);
        print_arm(&full);
        println!("mesh instances {full_instances}");
        print_arm(&ablated);
        println!("mesh instances {ablated_instances}");
        println!();
        print_ablation("kit", &full, &ablated);
        let mut off_engine = prepare(false);
        off_engine.set_static_scene(false);
        let off = measure_engine(&format!("{view} / statics off"), off_engine);
        print_arm(&off);
        print_ablation("static scene (off minus on)", &off, &full);
        assert!(
            ablated_instances < full_instances,
            "{view}: ablating the kit must draw fewer instances"
        );
        assert_eq!(
            full_instances, MESH_INSTANCES,
            "{view}: the budget is calibrated at {MESH_INSTANCES} instances"
        );
        assert!(
            full.extract_total_ms <= STATIC_KIT_EXTRACT_BUDGET_MS,
            "{view}: extract {:.3} ms over the {STATIC_KIT_EXTRACT_BUDGET_MS} ms budget at {MESH_INSTANCES} instances",
            full.extract_total_ms
        );
        for arm in [&full, &ablated] {
            assert!(
                !arm.report.gpu.available || arm.gpu_ms().is_some(),
                "{}: an adapter exists, so the GPU passes must have run",
                arm.label
            );
        }
    }
}

#[test]
fn the_committed_static_kit_scene_has_no_static_diagnostics() {
    let text = std::fs::read_to_string(scene_path()).expect("the committed static-kit scene");
    let engine = Engine::with_project_root(gallery());
    let scene = ludion_scene::check(&text, engine.registry()).expect("the scene checks");
    let found = ludion_engine::scene_static_diagnostics(&gallery(), &scene);
    assert!(found.is_empty(), "{found:?}");
}
