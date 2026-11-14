//! The room-swap frame, with and without the `prewarm_prefab` upload prewarm, GI on and off.
//!
//! Two room prefabs of about 24 x 24 m, each 500 primitive cube parts, 12 point lights and 30
//! distinct textured asset meshes (256 x 256 maps), stand in for a game's room swap: a sim system
//! spawns room A, then on the swap frame despawns it and spawns room B beside a `GiVolume` with the
//! game's floor settings. Per arm the bench prints the swap tick's wall time (`advance_live`), the
//! 1080p render that follows, the extract `Gi` phase and the GPU uploads the swap frame caused.
//! The GI share is GI-on minus GI-off on the prewarmed arm.
//!
//! Ignored by default so CI stays fast; run it explicitly, in release:
//!
//! ```text
//! cargo test -p ludion-engine --release --test it -- room_prewarm_bench --ignored --nocapture
//! ```
//!
//! The 42-03 trigger: GI share > 10 ms.
//!
//! There is deliberately no budget: the bench observes, and the wall-clock spans never feed sim
//! state. Each arm builds a fresh engine per round and keeps its fastest of five rounds.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use ludion_core::ids::EntityId;
use ludion_ecs::{SystemCtx, World};
use ludion_engine::extract_observer::{ExtractObserver, ExtractPhase};
use ludion_engine::{
    Engine, PrefabInstance, Transform, despawn_instance, gpu_uploads, prewarm_prefab, spawn_prefab,
};

const ROUNDS: usize = 5;
/// The frame whose tick swaps the rooms.
const SWAP: u64 = 12;
/// The prewarm request goes out this many frames before the swap.
const AHEAD: u64 = 8;
const CUBES: usize = 500;
const LIGHTS: usize = 12;
const MESHES: usize = 30;
const MAP_SIZE: u32 = 256;
const ROOMS: [&str; 2] = ["a.prefab.yaml", "b.prefab.yaml"];

#[allow(clippy::disallowed_methods)]
fn now() -> Instant {
    Instant::now()
}

#[allow(clippy::disallowed_methods)]
fn lap(start: Instant) -> Duration {
    start.elapsed()
}

fn ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1_000.0
}

fn vec3(indent: usize, key: &str, v: [f32; 3]) -> String {
    let pad = " ".repeat(indent);
    let mut out = format!("{pad}{key}:\n");
    for x in v {
        let _ = writeln!(out, "{pad}  - {x:?}");
    }
    out
}

fn color(indent: usize, key: &str, c: [f32; 4]) -> String {
    let pad = " ".repeat(indent);
    let mut out = format!("{pad}{key}:\n");
    for x in c {
        let _ = writeln!(out, "{pad}  - {x:?}");
    }
    out
}

fn transform(translation: [f32; 3], scale: Option<[f32; 3]>) -> String {
    let mut out = String::from("      Transform:\n");
    out.push_str(&vec3(8, "translation", translation));
    if let Some(scale) = scale {
        out.push_str(&vec3(8, "scale", scale));
    }
    out
}

fn entity(out: &mut String, name: &str, components: &str) {
    let _ = writeln!(out, "  - name: {name}\n    components:");
    out.push_str(components);
}

/// One room: floor and wall cubes, point lights and textured asset meshes, tinted by `room`.
fn room_prefab(room: usize) -> String {
    let mut out = format!("format: 1\nkind: prefab\nname: room-{room}\nentities:\n");
    let tint = if room == 0 {
        [0.55, 0.57, 0.6, 1.0]
    } else {
        [0.6, 0.52, 0.45, 1.0]
    };
    // A 20 x 20 floor of tiles 1.2 m apart (24 m), then wall cubes on the perimeter.
    let mut cubes = Vec::with_capacity(CUBES);
    for i in 0..400 {
        let (col, row) = ((i % 20) as f32, (i / 20) as f32);
        cubes.push((
            [(col - 9.5) * 1.2, -0.25, (row - 9.5) * 1.2],
            [1.1, 0.5, 1.1],
        ));
    }
    let mut perimeter = Vec::new();
    for k in 0..20 {
        let edge = (k as f32 - 9.5) * 1.2;
        perimeter.push([edge, 0.0, -11.4]);
        perimeter.push([edge, 0.0, 11.4]);
        perimeter.push([-11.4, 0.0, edge]);
        perimeter.push([11.4, 0.0, edge]);
    }
    for level in 0..2 {
        for at in &perimeter {
            if cubes.len() < CUBES {
                cubes.push(([at[0], 0.6 + level as f32 * 1.1, at[2]], [1.1, 1.1, 1.1]));
            }
        }
    }
    for (i, (at, scale)) in cubes.into_iter().enumerate() {
        let mut body = transform(at, Some(scale));
        body.push_str("      Mesh:\n        shape: cube\n");
        body.push_str(&color(8, "color", tint));
        entity(&mut out, &format!("cube-{i}"), &body);
    }
    for i in 0..LIGHTS {
        let at = [
            ((i % 4) as f32 - 1.5) * 5.5,
            3.0,
            ((i / 4) as f32 - 1.0) * 7.0,
        ];
        let mut body = transform(at, None);
        body.push_str("      PointLight:\n");
        body.push_str(&vec3(8, "color", [1.0, 0.85, 0.7]));
        body.push_str("        intensity: 5.0\n        range: 9.0\n");
        entity(&mut out, &format!("light-{i}"), &body);
    }
    for i in 0..MESHES {
        let at = [
            ((i % 6) as f32 - 2.5) * 3.5,
            0.5,
            ((i / 6) as f32 - 2.0) * 3.5,
        ];
        let mut body = transform(at, None);
        let _ = writeln!(
            body,
            "      Mesh:\n        shape:\n          asset: meshes/r{room}m{i}.mesh.yaml"
        );
        body.push_str(&color(8, "color", [1.0, 1.0, 1.0, 1.0]));
        entity(&mut out, &format!("prop-{i}"), &body);
    }
    out
}

/// A one-triangle mesh document binding its own base-colour map.
fn textured_mesh(room: usize, i: usize) -> String {
    format!(
        "format: 1
kind: mesh
positions:
  - 0.0
  - 0.0
  - 0.0
  - 1.0
  - 0.0
  - 0.0
  - 0.0
  - 1.0
  - 0.0
uvs:
  - 0.0
  - 0.0
  - 1.0
  - 0.0
  - 0.0
  - 1.0
base_color_texture:
  path: ../textures/r{room}m{i}.png
indices:
  - 0
  - 1
  - 2
"
    )
}

/// A noisy 256 x 256 map, distinct per room and mesh.
fn map_png(room: usize, i: usize) -> Vec<u8> {
    let mut state = 0x9e37_79b9_u32 ^ (room as u32 * 1_000 + i as u32);
    let mut pixels = Vec::with_capacity((MAP_SIZE * MAP_SIZE * 4) as usize);
    for _ in 0..MAP_SIZE * MAP_SIZE {
        state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        let [r, g, b, _] = state.to_be_bytes();
        pixels.extend_from_slice(&[r, g, b, 255]);
    }
    ludion_assets::encode_png(MAP_SIZE, MAP_SIZE, &pixels).expect("encodes")
}

fn project_root() -> PathBuf {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/tmp/room_prewarm_bench");
    let _ = std::fs::remove_dir_all(&dir);
    for sub in ["meshes", "textures"] {
        std::fs::create_dir_all(dir.join(sub)).unwrap();
    }
    for (room, prefab) in ROOMS.iter().enumerate() {
        std::fs::write(dir.join(prefab), room_prefab(room)).unwrap();
        for i in 0..MESHES {
            std::fs::write(
                dir.join(format!("meshes/r{room}m{i}.mesh.yaml")),
                textured_mesh(room, i),
            )
            .unwrap();
            std::fs::write(
                dir.join(format!("textures/r{room}m{i}.png")),
                map_png(room, i),
            )
            .unwrap();
        }
    }
    dir
}

/// A camera, a sun, a small static base room and, with `gi`, the game's floor `GiVolume`.
fn scene(gi: bool) -> String {
    let mut out = String::from("format: 1\nseed: 1\nentities:\n");
    let mut camera = String::from("      Transform:\n");
    camera.push_str(&vec3(8, "translation", [0.0, 20.0, 22.0]));
    camera.push_str(&vec3(8, "rotation", [-40.0, 0.0, 0.0]));
    camera.push_str(
        "      Camera:\n        projection:\n          type: perspective\n          fov_y_deg: 70.0\n",
    );
    entity(&mut out, "camera", &camera);
    let mut sun = transform([0.0, 10.0, 0.0], None);
    sun.push_str("      DirectionalLight:\n");
    sun.push_str(&vec3(8, "direction", [0.3, -0.8, -0.5]));
    sun.push_str("        intensity: 1.0\n        shadow_extent: 24.0\n");
    entity(&mut out, "sun", &sun);
    for (i, at) in [[-14.0, 1.0, 0.0], [14.0, 1.0, 0.0]]
        .into_iter()
        .enumerate()
    {
        let mut base = transform(at, Some([2.0, 2.0, 2.0]));
        base.push_str("      Mesh:\n        shape: cube\n");
        base.push_str(&color(8, "color", [0.5, 0.5, 0.55, 1.0]));
        entity(&mut out, &format!("base-{i}"), &base);
    }
    if gi {
        entity(
            &mut out,
            "gi-volume",
            "      GiVolume:\n        cell_size: 0.5\n        probe_spacing: 1.0\n        normal_bias: 0.5\n        bounce: 1.0\n",
        );
    }
    out
}

/// What the swap system does this arm.
struct Plan {
    prewarm: bool,
    current: Option<EntityId>,
}

fn swap(world: &mut World, ctx: &SystemCtx) {
    let frame = ctx.frame.as_u64();
    let prewarm = world.resource::<Plan>().is_some_and(|p| p.prewarm);
    if prewarm && frame == SWAP - AHEAD {
        prewarm_prefab(world, ROOMS[1], None).expect("room B prewarms");
    }
    let next = match frame {
        1 => ROOMS[0],
        SWAP => ROOMS[1],
        _ => return,
    };
    if let Some(old) = world.resource_mut::<Plan>().and_then(|p| p.current.take()) {
        despawn_instance(world, old).expect("room A's root is alive");
    }
    let ids = spawn_prefab(world, next, None::<Transform>, Some("room"), false).expect("spawns");
    world.resource_mut::<Plan>().unwrap().current = ids.first().copied();
}

struct GiClock(Arc<Mutex<(Option<Instant>, Duration)>>);

impl ExtractObserver for GiClock {
    fn begin(&mut self, phase: ExtractPhase) {
        if phase == ExtractPhase::Gi {
            self.0.lock().unwrap().0 = Some(now());
        }
    }

    fn end(&mut self, phase: ExtractPhase) {
        if phase == ExtractPhase::Gi {
            let mut state = self.0.lock().unwrap();
            if let Some(start) = state.0.take() {
                state.1 += lap(start);
            }
        }
    }
}

#[derive(Clone, Copy)]
struct Row {
    tick_ms: f64,
    render_ms: f64,
    gi_ms: f64,
    textures: u64,
    mesh_maps: u64,
    mesh_assets: u64,
}

impl Row {
    fn total(&self) -> f64 {
        self.tick_ms + self.render_ms
    }
}

/// One round: a fresh engine, rooms swapped on frame `SWAP`; `None` with no GPU adapter.
fn round(root: &Path, prewarm: bool, gi: bool) -> Option<Row> {
    let mut engine = Engine::builder()
        .project_root(root)
        .system("swap", swap)
        .query::<(&PrefabInstance,)>()
        .build()
        .expect("boots");
    engine.load_inline_scene(&scene(gi)).expect("loads");
    engine.world_mut().insert_resource(Plan {
        prewarm,
        current: None,
    });
    let render = |engine: &mut Engine| engine.render_png(1920, 1080).map(drop);
    while engine.frame().as_u64() < SWAP {
        engine.advance_live();
        render(&mut engine).ok()?;
    }
    let before = gpu_uploads(&mut engine).ok()?;
    let clock = Arc::new(Mutex::new((None, Duration::ZERO)));
    engine.set_extract_observer(Some(Box::new(GiClock(Arc::clone(&clock)))));
    let start = now();
    engine.advance_live();
    let tick_ms = ms(lap(start));
    let start = now();
    render(&mut engine).ok()?;
    let render_ms = ms(lap(start));
    engine.set_extract_observer(None);
    let after = gpu_uploads(&mut engine).ok()?;
    let gi_ms = ms(clock.lock().unwrap().1);
    Some(Row {
        tick_ms,
        render_ms,
        gi_ms,
        textures: after.textures - before.textures,
        mesh_maps: after.mesh_maps - before.mesh_maps,
        mesh_assets: after.mesh_assets - before.mesh_assets,
    })
}

fn fastest(root: &Path, prewarm: bool, gi: bool) -> Option<Row> {
    let mut best: Option<Row> = None;
    for _ in 0..ROUNDS {
        let row = round(root, prewarm, gi)?;
        if best.is_none_or(|b| row.total() < b.total()) {
            best = Some(row);
        }
    }
    best
}

#[test]
#[ignore = "release-mode benchmark; run explicitly (see the module docs)"]
fn needs_gpu_room_swap_bench() {
    let root = project_root();
    let mut rows = Vec::new();
    for (label, prewarm, gi) in [
        ("no prewarm / GI on", false, true),
        ("no prewarm / GI off", false, false),
        ("prewarm / GI on", true, true),
        ("prewarm / GI off", true, false),
    ] {
        let Some(row) = fastest(&root, prewarm, gi) else {
            ludion_core::require_gpu::skip_no_adapter(true);
            return;
        };
        rows.push((label, row));
    }
    println!(
        "\n{:<22} {:>9} {:>10} {:>8} {:>9} {:>10} {:>12}",
        "arm", "tick ms", "render ms", "Gi ms", "textures", "mesh_maps", "mesh_assets"
    );
    for (label, row) in &rows {
        println!(
            "{label:<22} {:>9.2} {:>10.2} {:>8.2} {:>9} {:>10} {:>12}",
            row.tick_ms, row.render_ms, row.gi_ms, row.textures, row.mesh_maps, row.mesh_assets
        );
    }
    let share = rows[2].1.total() - rows[3].1.total();
    println!(
        "\nGI share (prewarmed, tick + render, GI on - GI off): {share:.2} ms | extract Gi phase {:.2} ms",
        rows[2].1.gi_ms
    );
    println!("42-03 trigger (GI share > 10 ms): {}", share > 10.0);
}
