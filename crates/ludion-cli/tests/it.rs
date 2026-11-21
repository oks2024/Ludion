//! One test crate for ludion-cli's integration tests: the shared generics compile once, not once
//! per file. Each `tests/<stem>.rs` is a module here, so nextest names a test `it stem::name`.
//! Filter one file with `cargo nextest run -p ludion-cli --test it -E 'test(/^stem::/)'`.
//!
//! A new test file must be declared below, or `scripts/lists-in-step.mjs` fails (with
//! `autotests = false` an undeclared file would never compile). `dev`, `new` and `phase7` (the
//! cargo-build group) and `profile_attach` (wall-clock deadlines) stay standalone binaries: under
//! plain `cargo test` they would otherwise share a process with the GPU renders.

mod bake_probes;
mod check_scene;
mod check_scene_asset_paths;
mod check_scene_character;
mod check_scene_fonts;
mod check_scene_json;
mod check_scene_mesh;
mod check_scene_paired;
mod check_scene_placement;
mod check_scene_prefab;
mod check_scene_sprite;
mod check_scene_voxel;
mod check_scene_voxel_animation;
mod common;
mod diagnostic_severity;
mod docs;
mod explain;
mod export;
mod r#gen;
mod generate_atlas;
mod generate_atlas_font;
mod hierarchy_render;
mod image_stats;
mod import;
mod import_sprite;
mod import_unsupported_set;
mod lut;
mod map;
mod map_drift;
mod map_prefab;
mod mesh;
mod mesh_pick_camera;
mod physics;
mod render;
mod render_gi;
mod render_model;
mod render_sprite;
mod render_voxels;
mod replay;
mod room_assembly;
mod room_assembly_bench;
mod room_assembly_runtime;
mod sculpt;
mod serve;
mod sim_hash;
mod sync;
mod sync_cook;
mod sync_probes;
mod test_cmd;
mod texture;
mod trace;
mod voxel_cmd;
mod window_reconnect;
