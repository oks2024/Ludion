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

mod ai;
mod anim_graph;
mod animation_events;
mod assembly_ik;
mod assembly_mesh;
mod asset_cache;
pub mod audio;
mod audio_occlusion;
mod audio_zones;
pub mod bisect;
mod blend_clock;
mod blend_space;
mod bounds;
pub mod builder;
mod camera_latch;
mod camera_offset;
mod camera_rig;
mod camera_zone;
mod character;
mod checkpoints;
mod collapse;
mod collider_mesh;
mod commands;
pub mod components;
mod cooked_maps;
mod cursor_lock;
mod debug_draw;
pub mod diagnostics;
pub mod dialogue;
mod dispatch;
mod docs;
pub mod egress;
mod engine;
mod extract;
#[doc(hidden)]
pub mod extract_observer;
mod field_diff;
mod font_sheet;
pub mod fonts;
mod game_clock;
mod ground_marker_clip;
pub mod hierarchy;
mod input_actions;
mod input_bindings;
mod instantiate;
pub mod interpolation;
mod lane;
mod light_exposure;
pub mod live;
mod load_observer;
pub mod localization;
pub mod manifest;
mod mesh_asset;
mod mesh_check;
pub mod model_sheet;
mod motor;
mod nav;
mod nav_blockers;
mod paired;
mod paired_doc;
mod paired_sheet;
mod parent_joint;
pub mod particles;
mod path_follow;
mod patrol;
pub mod patterns;
mod perception;
mod perception_check;
mod perception_debug;
pub mod perf_live;
mod player_profile;
mod pose_blend;
mod post_lut;
mod prefab;
mod prefab_clips;
mod prefab_ops;
mod prefab_prewarm;
mod probe_bake;
mod probe_check;
mod probe_dump;
pub mod profile;
mod projectile_draw;
mod projectiles;
mod quality;
mod query_drift;
pub mod ragdoll;
mod ragdoll_recover;
mod rain;
mod recording;
mod render;
pub mod render_extent;
mod render_layers;
mod render_size;
mod render_view;
mod replay;
mod replay_compat;
mod replay_playback;
mod residue_store;
mod root_motion;
mod rumble;
mod run_recording;
mod run_save;
mod scene_check;
mod scene_physics;
mod scene_session;
mod screen_effect;
mod sculpt_mesh;
mod secondary_view;
pub mod sequences;
mod settings;
mod shutdown;
mod silhouette;
mod skin_bone;
mod skin_ragdoll;
mod snapshot;
mod sprite_sheet;
mod static_check;
mod static_scene;
pub mod sweep;
mod tick_observer;
mod trace;
pub mod transition;
pub mod ui;
mod viewmodel;
mod views_sheet;
mod voxel_mesh;
mod voxel_sheet;
mod voxelizer;

pub use anim_graph::{AnimEdge, AnimGate, AnimGraph, AnimState};
pub use animation_events::{AnimationEvents, ClipEvent, animation_events};
pub use assembly_mesh::{
    AssemblyData, AssemblyGroundData, AssemblyIkData, AssemblyIkKind, AssemblyPartData,
    MeshedAssemblyDocuments, load as load_assembly,
    load_meshed_documents as load_assembly_meshed_documents, mesh_assembly,
};
pub use audio::{
    AudioCategory, AudioDistance, AudioEvent, AudioLogEntry, AudioMessage, AudioPan, AudioQueue,
    Bus, CategoryGains, DUCK_FLOOR_DB, DuckRule, LoopId, MixLayer, VoiceFilter, VoiceId,
    VoiceSpatial, play_sound,
};
pub use audio_occlusion::AudioOcclusion;
pub use bisect::{
    BisectReport, ComponentDiff, DriveDone, EntityDiff, KeyframeDivergence, KeyframeDriftReport,
    RunConfig, WorldCapture, WorldDiff, bisect_divergence, first_divergent_frame,
    localize_divergence, verify_replay_keyframes, world_diff,
};
pub use blend_clock::blend_clocks;
pub use blend_space::{BlendPointData, BlendSpaceData};
pub use builder::{CursorMode, EngineBuilder};
pub use camera_zone::camera_occluders;
pub use character::{
    CharacterClipData, CharacterData, CharacterMaskData, ClipContact, JointHeight, contact_heights,
    load as load_character,
};
pub use checkpoints::{CheckpointConfig, CheckpointInfo, Checkpoints, RestoreOutcome};
pub use commands::sim::step_in_batches;
pub use components::PairedAction;
pub use components::SubtitleView;
pub use components::{AimKind, RadialEvent, UiRadial, UiRadialItem, UiRadialNav};
pub use components::{
    Anchor, Animation, AudioEmitter, Camera, CameraOffset, ClipAnchor, Decal, DecalBlend, DecalMap,
    DecalReceiverSet, DirectionalLight, DropShadow, EmitShape, Environment, Fade, GiDynamic,
    GiVolume, GroundMarker, GroundMarkerGrid, GroundMarkerShape, GroundMarkerStyle, MapWrap, Mesh,
    MeshGrid, MeshSource, Name, NavAgent, NavAvoidance, NavBlocker, NavBlockerMode,
    NavBlockerShape, NavGoal, NavLink, NavObstacle, NavObstacleState, NavReplan, NavRoute,
    NavRouteStatus, NavSteering, PARTICLE_MAX_LIVE_PER_EMITTER, ParentJoint, ParticleBlend,
    ParticleEmitter, Perceiver, PointLight, PostProcess, ProbeShape, Projection, ReflectionProbe,
    Require, Residue, SightPoint, SightTarget, SightZone, SkinBone, SpotLight, Sprite, SpriteBlend,
    Static, Text, TextOutline, TextShadow, TileAppearance, Tilemap, Transform, UiAnchorBehind,
    UiAnchorShow, UiClipShape, UiClock, UiEase, UiGroup, UiList, UiListItem, UiRect, UiText,
    UiTween, UiTweenField, UiTweenRepeat, UiTweenTrack, UiWorldAnchor, Viewmodel, ViewmodelLens,
    Visibility, builtin_registry, crossfade,
};
pub use components::{AnimGraphState, PoseSnapshot, SnapshotPart};
pub use components::{
    AnimationLayer, AnimationLayers, BlendClock, LayerMode, LayerPhase, fade_layer, layer_weight,
    play_layer, set_layer_params,
};
pub use components::{AudioListener, ListenerOrientation};
pub use components::{
    AudioZone, AudioZoneBed, AudioZoneBusGains, AudioZoneReverb, AudioZoneShape, ReverbPreset,
};
pub use components::{
    Hearing, Look, NoiseZone, NoiseZoneShape, PendingNoise, PerceiverMemory, PerceiverStatus,
    PerceptionState, Sense, SightSampleRecord, StimulusRecord, SweepShape, ZoneSweep,
};
pub use components::{IkTarget, IkTargets, fade_ik_target, ik_target_weight, ik_target_weight_at};
pub use components::{UiDash, UiLineCap, UiLineJoin, UiShape, UiShapeType, UiStroke};
pub use cooked_maps::TextureSource;
pub use cursor_lock::CursorLock;
pub use debug_draw::{DebugCategories, DebugDraw, DebugPen, debug_draw};
pub use diagnostics::{
    Diagnostics, report as report_diagnostic, report_full as report_full_diagnostic,
};
pub use dialogue::{DialogueConfig, DialogueEvents, Subtitles, dialogue_update};
pub use dispatch::{Dispatched, dispatch};
pub use docs::{DocStore, content_root_yaml, is_engine_kind};
pub use egress::{Egress, EgressChannels, EgressEvent, EgressFrame};
pub use engine::{ENGINE_VERSION, Engine, GiRebakeMode, KEYFRAME_INTERVAL};
pub use extract::set_clip_speed;
pub use extract::{check_atlas_reference, extract, extract_at, mesh_grid_cell};
pub use font_sheet::render_font_sheet;
pub use fonts::FontLibrary;
pub use ground_marker_clip::{
    GroundMarkerClip, ProfileQuery, VisibilityProfile, ground_marker_profile, visibility_profile,
};
pub use hierarchy::{
    HierarchyFault, Keep, MAX_HIERARCHY_DEPTH, TransformHierarchy, WorldPose, reparent,
    world_affine, world_pose_transform, world_transform,
};
pub use input_actions::{INPUT_ACTIONS_KIND, validate_input_actions};
pub use instantiate::EngineHooks;
pub use interpolation::NoInterpolate;
pub use lane::{
    LANE_ANCHOR, LANE_ERROR_FULL, LANE_FORWARD, LANE_LATERAL, LANE_OFF, LANE_ON, LANE_PATH,
    LaneConstraint, LaneEnds, LaneInput, lane_error_colour, lane_frame, lane_relative,
};
pub use light_exposure::{
    Combine, Exposure, ExposureError, ExposureQuery, Facing, LightContribution, LightExposure,
    LightExposureResults, LightKind, MAX_EXPOSURE_POINTS, light_exposure, light_exposure_update,
};
pub use ludion_camera::{
    Aim, BlendCurve, BlendIn, CameraRig, CameraZone, ChaseRig, FirstPersonRig, FollowRig, Framing,
    KickConfig, LookAhead, Recenter, RigCollision, RigCollisionConfig, RigConfig, RigMode,
    RigState, ShakeConfig, Springs, TrackRig, VerticalAnchor, add_fov_kick, add_kick, add_look,
    add_look_counts, add_trauma, camera_relative, camera_relative_yaw, first_person_eye,
    look_from_counts, orbit_eye, resolve_zone, set_look, snap_rig,
};
pub use ludion_components::Rain;
pub use ludion_components::RainOccluder;
pub use ludion_components::RenderLayers;
pub use ludion_components::RenderView;
pub use ludion_components::{BlendDepth, MeshBlend};
pub use ludion_components::{
    OccludedSilhouette, SilhouetteOutline, SilhouettePattern, SilhouettePatternKind, SilhouetteWhen,
};
pub use ludion_components::{PostProfile, ViewBackground, ViewShading};
pub use ludion_components::{ScreenEffect, ScreenEffectPreset, ScreenEffectTarget, ScreenMaskKind};
pub use ludion_nav::PathStatus;
pub use ludion_protocol::replay::GameIdentity;
pub use ludion_scene::Mirror;
pub use ludion_scene::{
    PathDocument, PathFrame, PathInterpolation, PathMetric, PathSampler, check_path,
};
pub use ludion_sim::WorldHash;
pub use mesh_check::scene_mesh_asset_diagnostics;
pub use model_sheet::{
    BlendSweepSpec, ClipCrossfade, ClipOutcome, CrossfadeSpec, DEFAULT_CROSSFADE_PHASES, IkSpec,
    LayerSpec, LookSpec, MAX_LINEUP_MODELS, MAX_STRIP_PHASES, MIN_STRIP_PHASES, ModelCamera,
    ModelChannel, ModelSheetRequest, ModelSweep, ModelSweepOutcome, ModelView, PhaseGap,
    PhaseReport, PhaseStray, SheetOutcome, Sweep, TrueSizeFraming, fit_distance_for_aspect,
    model_camera, parse_blend, parse_crossfade, parse_ik, parse_layer, parse_look,
    render_model_sheet,
};
pub use motor::{
    MOTOR_AIRBORNE, MOTOR_CARRY, MOTOR_CEILING, MOTOR_GROUND_NORMAL, MOTOR_GROUNDED, MOTOR_PROBE,
    MOTOR_STUCK, MOTOR_WALL, character_fit, character_fit_ignoring, fits_character, move_character,
    move_character_with, move_characters, probe_character_ground, resize_character,
    resume_character, set_character_posture, suspend_character, teleport_character, turn_character,
};
pub use nav::{
    Nav, NavClass, NavConfig, NavPath, PathRequest, nav_distance, nav_link_done, nav_path,
    nav_project, nav_random_point, nav_raycast, nav_reachable, nav_steer, nav_update,
};
pub use paired::{
    PairedFit, PairedMotion, PairedRoleFit, Warp, abort_paired, paired_fit, paired_motion,
    paired_update, start_paired,
};
pub use paired_doc::{PairedContactReport, PairedReport, PairedRoleReport, check_paired_files};
pub use paired_sheet::{
    MAX_PAIRED_STRIP, MIN_PAIRED_STRIP, PairedPhase, PairedSheetOutcome, PairedSheetRequest,
    render_paired_sheet,
};
pub use parent_joint::{posed_joint_world, scene_parent_joint_diagnostics};
pub use path_follow::{
    FollowerMotion, PathFollower, PathMode, PathOrient, path_follow, path_follower_motion,
};
pub use patrol::{
    PatrolFollower, PatrolMotion, PatrolPhase, PatrolResume, PatrolState, patrol_motion,
    patrol_update,
};
pub use patterns::{
    BulletKind, MotionSegment, PatternDoc, PatternPlayer, RampSpec, TimelineEntry, TurnSpec,
    VOLLEY_MAX_BULLETS, VOLLEY_SALT, Volley, VolleyCenter, VolleyShape, advance_volleys,
    bullet_age, bullet_is_alive, bullet_position, play_patterns, validate_pattern_docs,
    volley_hits,
};
pub use perception::{
    HeardNoise, Noise, NoiseId, NoiseRecord, PerceptionConfig, PerceptionResults, SightSample,
    Stimulus, SurfaceNoise, emit_noise, forget, forget_all, perception_update, report_stimulus,
    stimuli,
};
pub use perception_check::{scene_perception_diagnostics, scene_perception_name_diagnostics};
pub use perf_live::{
    FrameSample, LIVE_SEQUENTIAL_NAMES, LiveGpuProfile, LivePerf, PROFILE_WINDOW_FRAMES,
    ProfileSamples, ProfileWindowData,
};
pub use player_profile::{Profile, ProfileSaveEntry};
pub use post_lut::scene_lut_diagnostics;
pub use prefab::{
    MapSpawn, PrefabInstance, despawn_instance, prepare_map, prepare_prefab, spawn_map,
    spawn_prefab,
};
pub use prefab_clips::{
    PrefabClipData, load as load_prefab_clips, prefab_clip_target_diagnostics, prefab_part_names,
};
pub use prefab_ops::{PrefabApplied, PrefabReverted, revert_prefab, unpack_prefab};
pub use prefab_prewarm::{PrefabPrewarm, prewarm_prefab};
pub use probe_bake::{
    ProbeBake, ProbeBakeReport, ProbeCoverage, ProbeCoverageEntry, ProbeOutcome, ProbeRefreeze,
    ProbeSource, prune_probe_store,
};
pub use probe_check::scene_probe_diagnostics;
pub use probe_dump::{DumpedProbe, ProbeDumpOutcome, render_probe_dump};
pub use profile::{
    Budget, GpuProfile, PassTiming, ProfileReport, QualityProfile, SEQUENTIAL_NAMES, SimProfile,
    SystemTiming, TimingTail, check_budget, load_budget, profile_engine, profile_replay,
    profile_scene, warmup_for,
};
pub use projectile_draw::{ProjectileLook, ProjectileVisual, ProjectileVisuals};
pub use projectiles::{ProjectileConfig, Projectiles, projectiles_step, spawn_projectile};
pub use quality::{
    QUALITY_ENV, QualitySetting, resolve_render_scale, resolve_startup_quality,
    startup_quality_named,
};
pub use query_drift::{QueryDriftReport, check_query_drift_replay, check_query_drift_with};
pub use ragdoll::{LimitsFrom, RagdollSwitch, switch_to_ragdoll, switch_to_ragdoll_with};
pub use ragdoll_recover::{RagdollToAssembly, ragdoll_landing, switch_ragdoll_to_assembly};
pub use rain::scene_rain_diagnostics;
pub use render::{
    BuilderRenderOutcome, RenderOutcome, SequencePreviewOutcome, SequencePreviewWorld,
    ViewsOutcome, dump_scene_graph_json, engine_at_frame, find_view, gpu_uploads, render_scene,
    render_scene_views, render_scene_with_builder, render_scene_with_registry,
    render_sequence_preview, sequence_preview_world,
};
pub use render_extent::RenderExtent;
pub use render_layers::scene_render_layer_diagnostics;
pub use render_size::{
    MAX_HEIGHT as MAX_RENDER_HEIGHT, MAX_WIDTH as MAX_RENDER_WIDTH, RENDER_SIZE_ENV,
    resolve_startup_render_size,
};
pub use replay::{
    ReplayFramesOutcome, ReplayFramesRequest, drive_hashes, replay_hashes, replay_hashes_with,
    replay_render_frames, replay_with,
};
pub use replay_compat::replay_content_hash;
pub use replay_playback::{
    PlaybackControls, PlaybackInput, PlaybackOutcome, PlaybackSpeed, ReplayEntry, ReplayPlayback,
    ReplayPlaybackConfig,
};
pub use residue_store::ResidueStore;
pub use root_motion::{RootMotion, root_motion_delta};
pub use rumble::{Rumble, RumbleEffect};
pub use run_recording::{RunRecording, RunRecordingConfig, RunRecordingEntry, RunRecordingRequest};
pub use run_save::{BuildStamp, ResumeOutcome, RunSave, RunSaveEntry, RunSaveRequest, SuspendInfo};
pub use scene_check::{
    expand_scene, font_table_faults, prefab_singleton_diagnostics, scene_atlas_diagnostics,
    scene_audio_diagnostics, scene_camera_offset_diagnostics, scene_camera_rig_diagnostics,
    scene_engine_diagnostics, scene_font_diagnostics, scene_ground_marker_clip_diagnostics,
    scene_hierarchy_body_diagnostics, scene_layer_diagnostics, scene_light_exposure_diagnostics,
    scene_local_light_diagnostics, scene_motor_diagnostics, scene_physics_diagnostics,
    scene_prefab_clip_diagnostics, scene_sensor_diagnostics, scene_sun_diagnostics,
    scene_surface_diagnostics,
};
pub use screen_effect::scene_screen_effect_diagnostics;
pub use sculpt_mesh::{SculptStats, read_sculpt_sources, sculpt_budget_warning};
pub use secondary_view::scene_render_view_diagnostics;
pub use sequences::{SequenceEvents, SequencePreview};
pub use settings::{
    QualityPreset, Settings, SettingsView, WindowMode, resolve_anti_aliasing,
    settings_window_mode_at,
};
pub use shutdown::Shutdown;
pub use skin_bone::scene_skin_bone_diagnostics;
pub use skin_ragdoll::switch_skin_to_ragdoll;
pub use sprite_sheet::{
    AtlasClipOutcome, AtlasClipRequest, SpriteSheetOutcome, SpriteSheetRequest, SpriteSteps,
    check_atlas_clip_flags, render_atlas_clip, render_atlas_sheet, render_sprite_sheet,
    resolve_sprite_palette,
};
pub use static_check::{HOT_STATIC_FRAMES, HotStatic, StaticIneligible, scene_static_diagnostics};
pub use sweep::{
    FrameCell, FrameStripOutcome, MAX_SWEEP_CELLS, SweepCell, SweepOutcome, render_frame_strip,
    render_sweep_sheet,
};
pub use tick_observer::TickObserver;
pub use trace::{
    AiStatesRecord, AiTransitionRecord, ForgetTraceRecord, GroundRecord, LaneRecord, MotorRecord,
    NavPlanRecord, NavStateRecord, NoiseTraceRecord, PerceptionRecord, PlatformRecord,
    RememberTraceRecord, ReportRecord, SensorRecord, SightTraceRecord, SurfaceRecord, TraceChannel,
    TraceRecord, load_input_script, trace, trace_with, write_jsonl,
};
pub use transition::{RESIDUE_MAX_RECORDS_PER_SCENE, RESIDUE_MAX_SCENES, SceneSwitch, SwitchTo};
pub use views_sheet::views_sheet;
pub use voxel_mesh::{VoxelMeshStats, voxel_empty_lod_levels, voxel_mesh_stats};
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
// The metered exposure `ludion render` prints ([`RenderOutcome::metered`]).
pub use ludion_renderer::MeterReading;

#[cfg(test)]
mod animation_clock_tests;
#[cfg(test)]
mod probe_bake_pin_tests;
#[cfg(test)]
mod snapshot_integrity_tests;
