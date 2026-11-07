#![forbid(unsafe_code)]
#![warn(missing_docs)]

//! `ludion-engine` — the [`Engine`], the exhaustive [`dispatch`] parity gate, and the
//! recording/replay layer, on top of the built-in components and the world→render `extract()`.
//!
//! Phase 3 grows this crate from its Phase 2 form (the built-in visual [`components`] and their
//! [`builtin_registry`], the [`extract`] step, and the lockstep [`render_scene`] driver) into the
//! engine core: the [`Engine`] owns the world (by value, never `Arc<Mutex<_>>`), the registry, run
//! state, a subscription table, and an optional recording sink; [`dispatch`] is the single, exhaustive
//! command surface (the parity gate — a command with no handler is a compile error); and
//! [`replay_hashes`] re-drives a recorded session through that same surface to reproduce its `WorldHash`
//! sequence. It depends on `ecs`, `scene`, `assets`, `sim`, `renderer`, and `protocol` — **not** on
//! `ludion-server` (the server plugs into the engine, not the reverse; architecture §2).
//!
//! The pure [`dispatch`] path and [`Engine::execute`] are single-threaded and sequential; the
//! [`live`] module adds the threaded seam — the intake/reply channels and the [`run_live`](live::run_live)
//! sim-thread loop — that `ludion-server` plugs its WebSocket transport into. The engine stays
//! tokio-free: the seam is plain [`crossbeam_channel`]s, and the server is the peer that owns the
//! async runtime.
//!
//! # Example
//! ```
//! use ludion_engine::{dispatch, Engine};
//! use ludion_protocol::Command;
//! use ludion_protocol::command::SimStepParams;
//!
//! let mut engine = Engine::new();
//! engine.load_inline_scene("format: 1\nentities: []\n").unwrap();
//! let stepped = dispatch(&mut engine, Command::SimStep(SimStepParams { ticks: 3, hash: true })).unwrap();
//! assert_eq!(stepped.frame.as_u64(), 3);
//! ```

mod animation_events;
mod assembly_mesh;
mod asset_cache;
pub mod audio;
pub mod bisect;
mod bounds;
pub mod builder;
mod camera_latch;
mod camera_offset;
mod camera_rig;
mod character;
mod collapse;
mod collider_mesh;
mod commands;
pub mod components;
mod cooked_maps;
mod cursor_lock;
mod debug_draw;
pub mod diagnostics;
mod dispatch;
mod docs;
pub mod egress;
mod engine;
mod extract;
mod field_diff;
mod font_sheet;
pub mod fonts;
pub mod hierarchy;
mod instantiate;
pub mod interpolation;
mod lane;
pub mod live;
mod load_observer;
pub mod manifest;
mod mesh_asset;
mod mesh_check;
pub mod model_sheet;
mod motor;
mod parent_joint;
pub mod particles;
mod path_follow;
pub mod patterns;
pub mod perf_live;
mod player_profile;
mod prefab;
mod prefab_clips;
mod prefab_ops;
mod probe_bake;
mod probe_check;
mod probe_dump;
pub mod profile;
mod projectiles;
mod quality;
mod query_drift;
pub mod ragdoll;
mod recording;
mod render;
pub mod render_extent;
mod render_settings;
mod render_size;
mod render_view;
mod replay;
mod replay_compat;
mod residue_store;
mod scene_check;
mod scene_physics;
mod scene_session;
mod sculpt_mesh;
mod shutdown;
mod skin_bone;
mod snapshot;
mod sprite_sheet;
pub mod sweep;
mod tick_observer;
mod trace;
pub mod transition;
pub mod ui;
mod viewmodel;
mod voxel_mesh;
mod voxel_sheet;
mod voxelizer;

pub use animation_events::{AnimationEvents, ClipEvent, animation_events};
pub use assembly_mesh::{
    AssemblyData, AssemblyPartData, MeshedAssemblyDocuments, load as load_assembly,
    load_meshed_documents as load_assembly_meshed_documents, mesh_assembly,
};
pub use audio::{
    AudioCategory, AudioDistance, AudioEvent, AudioLogEntry, AudioMessage, AudioPan, AudioQueue,
    CategoryGains, LoopId, play_sound,
};
pub use bisect::{
    BisectReport, ComponentDiff, DriveDone, EntityDiff, KeyframeDivergence, KeyframeDriftReport,
    RunConfig, WorldCapture, WorldDiff, bisect_divergence, first_divergent_frame,
    localize_divergence, verify_replay_keyframes, world_diff,
};
pub use builder::{CursorMode, EngineBuilder};
pub use character::{
    CharacterClipData, CharacterData, ClipContact, JointHeight, contact_heights,
    load as load_character,
};
pub use commands::sim::step_in_batches;
pub use components::{
    Anchor, Animation, AudioEmitter, Camera, CameraOffset, ClipAnchor, Decal, DecalBlend, DecalMap,
    DecalReceiverSet, DirectionalLight, DropShadow, EmitShape, Environment, Fade, GiDynamic,
    GiVolume, GroundMarker, GroundMarkerShape, MapWrap, Mesh, MeshGrid, MeshSource, Name,
    PARTICLE_MAX_LIVE_PER_EMITTER, ParentJoint, ParticleBlend, ParticleEmitter, PointLight,
    PostProcess, ProbeShape, Projection, ReflectionProbe, Residue, SkinBone, SpotLight, Sprite,
    SpriteBlend, Text, TileAppearance, Tilemap, Transform, UiList, UiListItem, UiRect, UiText,
    Viewmodel, ViewmodelLens, Visibility, builtin_registry, crossfade,
};
pub use cooked_maps::TextureSource;
pub use cursor_lock::CursorLock;
pub use debug_draw::{DebugCategories, DebugDraw, DebugPen, debug_draw};
pub use diagnostics::{
    Diagnostics, report as report_diagnostic, report_full as report_full_diagnostic,
};
pub use dispatch::{Dispatched, dispatch};
pub use docs::{DocStore, content_root_yaml, is_engine_kind};
pub use egress::{Egress, EgressChannels, EgressEvent, EgressFrame};
pub use engine::{ENGINE_VERSION, Engine, GiRebakeMode, KEYFRAME_INTERVAL};
pub use extract::set_clip_speed;
pub use extract::{check_atlas_reference, extract, extract_at, mesh_grid_cell};
pub use font_sheet::render_font_sheet;
pub use fonts::FontLibrary;
pub use hierarchy::{
    HierarchyFault, Keep, MAX_HIERARCHY_DEPTH, TransformHierarchy, WorldPose, reparent,
    world_affine, world_pose_transform, world_transform,
};
pub use instantiate::EngineHooks;
pub use interpolation::NoInterpolate;
pub use lane::{
    LANE_ANCHOR, LANE_ERROR_FULL, LANE_FORWARD, LANE_LATERAL, LANE_OFF, LANE_ON, LANE_PATH,
    LaneConstraint, LaneEnds, LaneInput, lane_error_colour, lane_frame, lane_relative,
};
pub use ludion_camera::{
    Aim, BlendCurve, BlendIn, CameraRig, FirstPersonRig, KickConfig, LookAhead, RigConfig, RigMode,
    RigState, ShakeConfig, Springs, TrackRig, VerticalAnchor, add_fov_kick, add_kick, add_look,
    add_look_counts, add_trauma, camera_relative, camera_relative_yaw, first_person_eye,
    look_from_counts, set_look, snap_rig,
};
pub use ludion_scene::{
    PathDocument, PathFrame, PathInterpolation, PathMetric, PathSampler, check_path,
};
pub use ludion_sim::WorldHash;
pub use mesh_check::scene_mesh_asset_diagnostics;
pub use model_sheet::{
    ClipCrossfade, ClipOutcome, CrossfadeSpec, DEFAULT_CROSSFADE_PHASES, MAX_LINEUP_MODELS,
    MAX_STRIP_PHASES, MIN_STRIP_PHASES, ModelCamera, ModelChannel, ModelSheetRequest, ModelSweep,
    ModelSweepOutcome, ModelView, PhaseGap, PhaseReport, PhaseStray, SheetOutcome, TrueSizeFraming,
    fit_distance_for_aspect, model_camera, parse_crossfade, render_model_sheet,
};
pub use motor::{
    MOTOR_AIRBORNE, MOTOR_CARRY, MOTOR_CEILING, MOTOR_GROUND_NORMAL, MOTOR_GROUNDED, MOTOR_PROBE,
    MOTOR_STUCK, MOTOR_WALL, fits_character, move_character, move_character_with,
    probe_character_ground, resize_character, teleport_character,
};
pub use parent_joint::{posed_joint_world, scene_parent_joint_diagnostics};
pub use path_follow::{
    FollowerMotion, PathFollower, PathMode, PathOrient, path_follow, path_follower_motion,
};
pub use patterns::{
    BulletKind, MotionSegment, PatternDoc, PatternPlayer, RampSpec, TimelineEntry, TurnSpec,
    VOLLEY_MAX_BULLETS, VOLLEY_SALT, Volley, VolleyCenter, VolleyShape, advance_volleys,
    bullet_age, bullet_is_alive, bullet_position, play_patterns, validate_pattern_docs,
    volley_hits,
};
pub use perf_live::{
    FrameSample, LIVE_SEQUENTIAL_NAMES, LiveGpuProfile, LivePerf, PROFILE_WINDOW_FRAMES,
    ProfileSamples, ProfileWindowData,
};
pub use player_profile::{Profile, ProfileSaveEntry};
pub use prefab::{PrefabInstance, spawn_prefab};
pub use prefab_clips::{
    PrefabClipData, load as load_prefab_clips, prefab_clip_target_diagnostics, prefab_part_names,
};
pub use prefab_ops::{PrefabApplied, PrefabReverted, revert_prefab, unpack_prefab};
pub use probe_bake::{ProbeBake, ProbeBakeReport, ProbeOutcome, ProbeSource, prune_probe_store};
pub use probe_check::scene_probe_diagnostics;
pub use probe_dump::{DumpedProbe, ProbeDumpOutcome, render_probe_dump};
pub use profile::{
    Budget, GpuProfile, PassTiming, ProfileReport, QualityProfile, SEQUENTIAL_NAMES, SimProfile,
    SystemTiming, TimingTail, check_budget, load_budget, profile_engine, profile_replay,
    profile_scene, warmup_for,
};
pub use projectiles::{ProjectileConfig, Projectiles, projectiles_step, spawn_projectile};
pub use quality::{QUALITY_ENV, QualitySetting, resolve_render_scale, resolve_startup_quality};
pub use query_drift::{QueryDriftReport, check_query_drift_replay, check_query_drift_with};
pub use ragdoll::switch_to_ragdoll;
pub use render::{
    BuilderRenderOutcome, RenderOutcome, dump_scene_graph_json, engine_at_frame, gpu_uploads,
    render_scene, render_scene_with_builder, render_scene_with_registry,
};
pub use render_extent::RenderExtent;
pub use render_settings::{RenderSettings, resolve_anti_aliasing};
pub use render_size::{
    MAX_HEIGHT as MAX_RENDER_HEIGHT, MAX_WIDTH as MAX_RENDER_WIDTH, RENDER_SIZE_ENV,
    resolve_startup_render_size,
};
pub use replay::{
    ReplayFramesOutcome, ReplayFramesRequest, drive_hashes, replay_hashes, replay_hashes_with,
    replay_render_frames, replay_with,
};
pub use replay_compat::replay_content_hash;
pub use residue_store::ResidueStore;
pub use scene_check::{
    expand_scene, font_table_faults, prefab_singleton_diagnostics, scene_atlas_diagnostics,
    scene_audio_diagnostics, scene_camera_offset_diagnostics, scene_camera_rig_diagnostics,
    scene_engine_diagnostics, scene_font_diagnostics, scene_hierarchy_body_diagnostics,
    scene_layer_diagnostics, scene_local_light_diagnostics, scene_motor_diagnostics,
    scene_physics_diagnostics, scene_prefab_clip_diagnostics, scene_sensor_diagnostics,
    scene_sun_diagnostics,
};
pub use sculpt_mesh::{SculptStats, read_sculpt_sources, sculpt_budget_warning};
pub use shutdown::Shutdown;
pub use skin_bone::scene_skin_bone_diagnostics;
pub use sprite_sheet::{
    AtlasClipOutcome, AtlasClipRequest, SpriteSheetOutcome, SpriteSheetRequest, SpriteSteps,
    check_atlas_clip_flags, render_atlas_clip, render_atlas_sheet, render_sprite_sheet,
    resolve_sprite_palette,
};
pub use sweep::{
    FrameCell, FrameStripOutcome, MAX_SWEEP_CELLS, SweepCell, SweepOutcome, render_frame_strip,
    render_sweep_sheet,
};
pub use tick_observer::TickObserver;
pub use trace::{
    GroundRecord, LaneRecord, MotorRecord, PlatformRecord, ReportRecord, SensorRecord,
    SurfaceRecord, TraceChannel, TraceRecord, load_input_script, trace, trace_with, write_jsonl,
};
pub use transition::{RESIDUE_MAX_RECORDS_PER_SCENE, RESIDUE_MAX_SCENES, SceneSwitch, SwitchTo};
pub use voxel_mesh::{VoxelMeshStats, voxel_mesh_stats};
pub use voxel_sheet::{GiSheetOutcome, VoxelSheetOutcome, render_gi_sheet, render_voxel_sheet};

// Re-exported so the CLI's `dump-graph` gets the real frame-graph JSON without depending on
// `ludion-renderer` directly (CPU-only, no GPU), and so every caller of a render entry point can
// name the frame size and the client's quality setting from this crate alone. `PresentSize` rides
// along because [`Engine::attach_surface`] returns one: a windowed host must be able to name what
// it holds.
pub use ludion_renderer::{
    AntiAliasing, DebugView, FrameSize, GpuUploads, LookMotion, PresentSize, ProbePrune, Quality,
    QualityParams, RenderScale, dump_frame_graph_json,
};

#[cfg(test)]
mod animation_clock_tests;
#[cfg(test)]
mod probe_bake_pin_tests;
#[cfg(test)]
mod snapshot_integrity_tests;
