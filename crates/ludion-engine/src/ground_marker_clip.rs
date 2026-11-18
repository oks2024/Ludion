//! The line-of-sight profile of a clipped [`GroundMarker`]: `bins` rays fanned across the marker's
//! arc, cast through physics, and reduced to a range per bin. `extract` computes it at render time
//! (derived, render-side, never hashed or snapshotted) and the renderer clips the marker by it;
//! [`visibility_profile`] is the same cast a game or the 0001 sight pass can call. See
//! `ludion docs ground-telegraphs`.

use std::collections::BTreeMap;

pub use ludion_components::GroundMarkerClip;
use ludion_components::{GroundMarker, GroundMarkerShape, MAX_CLIP_BINS, MIN_CLIP_BINS};
use ludion_core::codes::code;
use ludion_core::diagnostics::Diagnostic;
use ludion_core::ids::EntityId;
use ludion_core::math::glam::{Quat, Vec3};
use ludion_core::math::{cos, sin};
use ludion_ecs::World;
use ludion_physics::{PhysicsError, PhysicsWorld, PrimedPhysics, QueryFilter, Sweep};

use crate::hierarchy::{hierarchy_root, world_affine};

/// One profile cast. Bin `i` looks along `rotation * (cos a, 0, sin a)` for
/// `a = arc_start + (i + 0.5) * arc_span / bins`, the angle measured in the marker's local XZ plane
/// (`atan2(z, x)`).
#[derive(Clone, Copy, Debug)]
pub struct ProfileQuery<'a> {
    /// The ray origin, world space (the marker apex lifted by the clip `height`).
    pub origin: [f32; 3],
    /// The marker's world rotation.
    pub rotation: Quat,
    /// The arc's first edge, radians.
    pub arc_start: f32,
    /// The arc's width, radians (`2π` for a full circle).
    pub arc_span: f32,
    /// The longest ray, world units.
    pub range: f32,
    /// The ray count.
    pub bins: u32,
    /// Colliders that clip: the filter is used as given.
    pub blockers: &'a QueryFilter,
    /// Colliders that only dim what lies behind them; `None` leaves `partial` equal to `radii`.
    pub partial: Option<&'a QueryFilter>,
    /// A collider every ray skips, the marker's own root.
    pub exclude: Option<EntityId>,
}

impl ProfileQuery<'_> {
    /// How many rays [`visibility_profile`] casts for this query.
    pub fn ray_count(&self) -> u32 {
        self.bins * (1 + u32::from(self.partial.is_some()))
    }
}

/// A range per bin over an arc.
#[derive(Clone, Debug, PartialEq)]
pub struct VisibilityProfile {
    /// The arc's first edge, radians.
    pub arc_start: f32,
    /// The arc's width, radians.
    pub arc_span: f32,
    /// Per bin, the distance to the first blocker, at most the query `range`.
    pub radii: Vec<f32>,
    /// Per bin, the distance to the first partial blocker, at most `radii[i]`.
    pub partial: Vec<f32>,
}

fn bin_angle(q: &ProfileQuery, i: usize) -> f32 {
    q.arc_start + (i as f32 + 0.5) * q.arc_span / q.bins as f32
}

/// Casts `q.bins` rays through `physics` in one batch (the result is the same at every thread
/// count) and reduces them to a [`VisibilityProfile`]. Rays go through `PhysicsWorld::sweep_filter` and
/// `first_hits`, the path 0001 sight uses.
///
/// # Errors
/// [`PhysicsError::QueryInput`] when a filter names a layer the world does not declare.
///
/// # Example
/// ```
/// use ludion_core::math::glam::Quat;
/// use ludion_core::time::TickRate;
/// use ludion_engine::{ProfileQuery, visibility_profile};
/// use ludion_physics::{PhysicsWorld, QueryFilter};
///
/// let physics = PhysicsWorld::new(TickRate::DEFAULT);
/// let filter = QueryFilter::new();
/// let profile = visibility_profile(
///     &physics,
///     &ProfileQuery {
///         origin: [0.0, 1.6, 0.0],
///         rotation: Quat::IDENTITY,
///         arc_start: -std::f32::consts::PI,
///         arc_span: std::f32::consts::TAU,
///         range: 6.0,
///         bins: 16,
///         blockers: &filter,
///         partial: None,
///         exclude: None,
///     },
/// )
/// .unwrap();
/// assert_eq!(profile.radii, vec![6.0; 16]);
/// ```
pub fn visibility_profile(
    physics: &PhysicsWorld,
    q: &ProfileQuery,
) -> Result<VisibilityProfile, PhysicsError> {
    let bins = q.bins as usize;
    let sweeps: Vec<Sweep> = (0..bins)
        .map(|i| {
            let a = bin_angle(q, i);
            let dir = q.rotation * Vec3::new(cos(a), 0.0, sin(a));
            let sweep = Sweep::new(q.origin, (dir * q.range).to_array(), 0.0);
            q.exclude.map_or(sweep, |e| sweep.excluding(e))
        })
        .collect();
    let first =
        |filter: &QueryFilter| -> Result<Vec<Option<ludion_physics::SweepHit>>, PhysicsError> {
            let filter = physics.sweep_filter(filter)?;
            let mut hits = vec![None; bins];
            physics.first_hits(&sweeps, &filter, &mut hits);
            Ok(hits)
        };
    let radii: Vec<f32> = first(q.blockers)?
        .into_iter()
        .map(|hit| hit.map_or(q.range, |h| h.distance.min(q.range)))
        .collect();
    let partial = match q.partial {
        None => radii.clone(),
        Some(filter) => first(filter)?
            .into_iter()
            .zip(&radii)
            .map(|(hit, &r)| hit.map_or(r, |h| h.distance.min(r)))
            .collect(),
    };
    Ok(VisibilityProfile {
        arc_start: q.arc_start,
        arc_span: q.arc_span,
        radii,
        partial,
    })
}

/// The arc and range a marker's clip casts over, in world units for the uniform `scale`: `(arc_start,
/// arc_span, range)`. A segment has no apex and returns `None`.
pub(crate) fn marker_arc(shape: GroundMarkerShape, scale: f32) -> Option<(f32, f32, f32)> {
    let scale = scale.max(0.0);
    let full = (-std::f32::consts::PI, std::f32::consts::TAU);
    Some(match shape {
        GroundMarkerShape::Disc { radius } | GroundMarkerShape::Ring { radius, .. } => {
            (full.0, full.1, radius * scale)
        }
        GroundMarkerShape::Wedge {
            half_angle_deg,
            outer_radius,
            ..
        } => {
            let half = half_angle_deg.to_radians();
            (-half, 2.0 * half, outer_radius * scale)
        }
        GroundMarkerShape::Segment { .. } => return None,
    })
}

/// The blocker and partial filters of `clip`: its layers (empty means every layer), static colliders
/// only unless `dynamic`. The same construction the sight pass uses for a zone's blockers.
pub(crate) fn clip_filters(clip: &GroundMarkerClip) -> (QueryFilter, Option<QueryFilter>) {
    let build = |set: &ludion_physics::LayerSet| {
        let filter = QueryFilter::new().include_set(set);
        if clip.dynamic {
            filter
        } else {
            filter.only_fixed()
        }
    };
    (
        build(&clip.layers),
        (!clip.partial_layers.is_empty()).then(|| build(&clip.partial_layers)),
    )
}

/// The world's physics for a cast: the live [`PhysicsWorld`], or before the first tick the one the
/// scene's load primed, so a tick-0 render already casts through the static colliders.
pub(crate) fn physics_of(world: &World) -> Option<&PhysicsWorld> {
    world.resource::<PhysicsWorld>().or_else(|| {
        world
            .resource::<PrimedPhysics>()
            .map(|primed| &primed.world)
    })
}

/// The profile of the clipped marker `id`, cast fresh from the world's current physics state (never
/// cached), or `None` when `id` has no `GroundMarker` plus `GroundMarkerClip`, no pose, a segment
/// shape or the world has no physics. Marker and clip are read as `extract` reads them, so the
/// answer is the profile the renderer would draw.
pub fn ground_marker_profile(
    world: &World,
    id: EntityId,
) -> Option<Result<VisibilityProfile, PhysicsError>> {
    let marker = world.get::<GroundMarker>(id).ok().flatten()?;
    let clip = world.get::<GroundMarkerClip>(id).ok().flatten()?;
    let physics = physics_of(world)?;
    let pose = world_affine(world, id)?;
    let (arc_start, arc_span, range) = marker_arc(marker.shape, pose.lossy_scale()[0])?;
    let (blockers, partial) = clip_filters(clip);
    let rotation = Quat::from_array(pose.to_trs_lossy().rotation);
    let origin = (Vec3::from(pose.cols[3]) + Vec3::Y * clip.height).to_array();
    Some(visibility_profile(
        physics,
        &ProfileQuery {
            origin,
            rotation,
            arc_start,
            arc_span,
            range,
            bins: clip.bins,
            blockers: &blockers,
            partial: partial.as_ref(),
            exclude: Some(hierarchy_root(world, id)),
        },
    ))
}

/// The memo of per-entity profiles held by the loader. An entry is reused while its key (the pose,
/// the shape, the clip spec and, for a static clip, a fingerprint of the static colliders) is
/// byte-identical, so a restored run casts what an uninterrupted one cast; a `dynamic` clip never
/// caches. Entries not used by an extract are dropped at its end.
#[derive(Default)]
pub(crate) struct ClipCache {
    entries: BTreeMap<EntityId, Entry>,
    stamp: u64,
    rays: u64,
}

struct Entry {
    key: Vec<u8>,
    stamp: u64,
    profile: VisibilityProfile,
}

impl ClipCache {
    /// An extract starts.
    pub(crate) fn begin(&mut self) {
        self.stamp += 1;
    }

    /// The extract ends: entries it did not use are dropped.
    pub(crate) fn end(&mut self) {
        let stamp = self.stamp;
        self.entries.retain(|_, e| e.stamp == stamp);
    }

    #[cfg(test)]
    /// Rays cast since the cache was made.
    pub(crate) fn rays_cast(&self) -> u64 {
        self.rays
    }

    /// The profile of `id` under `key`: the held one when `key` is `Some` and unchanged, else
    /// `cast`'s answer, held under `key` for next time.
    pub(crate) fn profile(
        &mut self,
        id: EntityId,
        key: Option<Vec<u8>>,
        rays: u32,
        cast: impl FnOnce() -> Result<VisibilityProfile, PhysicsError>,
    ) -> Result<VisibilityProfile, PhysicsError> {
        let Some(key) = key else {
            self.rays += u64::from(rays);
            self.entries.remove(&id);
            return cast();
        };
        if let Some(entry) = self.entries.get_mut(&id)
            && entry.key == key
        {
            entry.stamp = self.stamp;
            return Ok(entry.profile.clone());
        }
        self.rays += u64::from(rays);
        let profile = cast()?;
        self.entries.insert(
            id,
            Entry {
                key,
                stamp: self.stamp,
                profile: profile.clone(),
            },
        );
        Ok(profile)
    }
}

/// A fingerprint of the world's static colliders and the layer inputs a visibility cast reads, so a
/// moved, added, removed or re-layered static changes it.
pub(crate) fn static_fingerprint(physics: &PhysicsWorld) -> u64 {
    physics.static_query_fingerprint()
}

/// The cache key of one static-clip cast: every input the profile reads, as bytes.
pub(crate) fn cache_key(q: &ProfileQuery, clip: &GroundMarkerClip, fingerprint: u64) -> Vec<u8> {
    let mut key = Vec::with_capacity(96);
    let put = |key: &mut Vec<u8>, bits: u32| key.extend_from_slice(&bits.to_le_bytes());
    for v in q.origin {
        put(&mut key, v.to_bits());
    }
    for v in q.rotation.to_array() {
        put(&mut key, v.to_bits());
    }
    for v in [q.arc_start, q.arc_span, q.range, clip.partial_alpha] {
        put(&mut key, v.to_bits());
    }
    put(&mut key, q.bins);
    put(&mut key, q.exclude.map_or(u32::MAX, |e| e.index()));
    put(&mut key, q.exclude.map_or(0, |e| e.generation()));
    for set in [&clip.layers, &clip.partial_layers] {
        put(&mut key, set.as_slice().len() as u32);
        for name in set.iter() {
            key.extend_from_slice(name.as_bytes());
            key.push(0);
        }
    }
    key.extend_from_slice(&fingerprint.to_le_bytes());
    key
}

/// One extract's clipping: resolves each clipped marker's profile (through the loader's cache when
/// it has one) and packs it into the scene's profile floats.
pub(crate) struct ClipPass<'a> {
    world: &'a World,
    physics: Option<&'a PhysicsWorld>,
    cache: Option<&'a mut ClipCache>,
    fingerprint: Option<u64>,
    profiles: Vec<f32>,
    reported_no_physics: bool,
    reported_segment: bool,
}

/// Floats in a packed profile header: `[arc_start, arc_span, partial_alpha, bins]`.
const HEADER: usize = 4;

impl<'a> ClipPass<'a> {
    pub(crate) fn new(world: &'a World, mut cache: Option<&'a mut ClipCache>) -> Self {
        if let Some(cache) = cache.as_deref_mut() {
            cache.begin();
        }
        Self {
            world,
            physics: physics_of(world),
            cache,
            fingerprint: None,
            profiles: Vec::new(),
            reported_no_physics: false,
            reported_segment: false,
        }
    }

    /// Resolves and packs the profile of the clipped marker `id`, returning its float offset, or
    /// `None` (the marker draws unclipped) with the reason in `degraded`. `pose` is the marker's
    /// world `(translation, rotation, uniform scale)`.
    pub(crate) fn clip(
        &mut self,
        id: EntityId,
        shape: GroundMarkerShape,
        clip: &GroundMarkerClip,
        pose: ([f32; 3], Quat, f32),
        degraded: &mut Vec<(String, Diagnostic)>,
    ) -> Option<u32> {
        let (translation, rotation, scale) = pose;
        let Some((arc_start, arc_span, range)) = marker_arc(shape, scale) else {
            if !std::mem::replace(&mut self.reported_segment, true) {
                degraded.push((
                    "ground marker clip: segment".into(),
                    Diagnostic::new(
                        code::GROUND_MARKER_CLIP_INVALID,
                        format!("{id}'s `GroundMarkerClip` sits on a segment, which has no apex to cast from; it is drawn unclipped"),
                    )
                    .with_help("clip a `disc`, `ring` or `wedge`; `ludion explain L2342`"),
                ));
            }
            return None;
        };
        let Some(physics) = self.physics else {
            if !std::mem::replace(&mut self.reported_no_physics, true) {
                degraded.push((
                    "ground marker clip: no physics".into(),
                    Diagnostic::new(
                        code::GROUND_MARKER_CLIP_NO_PHYSICS,
                        format!("{id}'s `GroundMarkerClip` has no physics world to cast through; it is drawn unclipped"),
                    )
                    .with_help("declare a `physics:` block in the scene; `ludion explain L2343`"),
                ));
            }
            return None;
        };
        let (blockers, partial) = clip_filters(clip);
        let bins = clip.bins.clamp(MIN_CLIP_BINS, MAX_CLIP_BINS);
        let query = ProfileQuery {
            origin: (Vec3::from(translation) + Vec3::Y * clip.height).to_array(),
            rotation,
            arc_start,
            arc_span,
            range,
            bins,
            blockers: &blockers,
            partial: partial.as_ref(),
            exclude: Some(hierarchy_root(self.world, id)),
        };
        let key = (!clip.dynamic).then(|| {
            let fingerprint = *self
                .fingerprint
                .get_or_insert_with(|| static_fingerprint(physics));
            cache_key(&query, clip, fingerprint)
        });
        let cast = || visibility_profile(physics, &query);
        let rays = query.ray_count();
        let profile = match self.cache.as_deref_mut() {
            Some(cache) => cache.profile(id, key, rays, cast),
            None => cast(),
        };
        let profile = match profile {
            Ok(profile) => profile,
            Err(err) => {
                degraded.push((
                    format!("ground marker clip: {id}"),
                    Diagnostic::new(
                        code::GROUND_MARKER_CLIP_INVALID,
                        format!(
                            "{id}'s `GroundMarkerClip` could not cast: {err}; it is drawn unclipped"
                        ),
                    )
                    .with_help(
                        "name only layers declared under `physics.layers`; `ludion explain L2342`",
                    ),
                ));
                return None;
            }
        };
        let offset = self.profiles.len();
        if offset + HEADER + 2 * bins as usize >= 1 << 24 {
            return None;
        }
        self.profiles.extend_from_slice(&[
            profile.arc_start,
            profile.arc_span,
            clip.partial_alpha,
            bins as f32,
        ]);
        self.profiles.extend_from_slice(&profile.radii);
        self.profiles.extend_from_slice(&profile.partial);
        Some(offset as u32)
    }

    /// Ends the pass, dropping cache entries it did not use, and hands back the packed profiles.
    pub(crate) fn finish(mut self) -> Vec<f32> {
        if let Some(cache) = self.cache.as_deref_mut() {
            cache.end();
        }
        self.profiles
    }
}

#[cfg(test)]
mod tests {
    use std::time::Instant;

    use ludion_core::math::cos;

    use crate::{Engine, Name, Transform};

    const HEAD: &str = "format: 1\nseed: 1\nphysics:\n  space: free\n  layers:\n    - name: sight_blocker\n    - name: low_cover\nentities:\n";

    fn seq(indent: usize, v: &[f32]) -> String {
        v.iter()
            .map(|c| format!("{:indent$}- {c:?}\n", ""))
            .collect()
    }

    fn wall(name: &str, at: [f32; 3], half: [f32; 3], layer: &str) -> String {
        format!(
            "  - name: {name}\n    components:\n      Transform:\n        translation:\n{}      Collider:\n        shape:\n          box:\n            half_extents:\n{}        body: static\n        layer: {layer}\n",
            seq(10, &at),
            seq(14, &half)
        )
    }

    fn marker(name: &str, at: [f32; 3], bins: u32, tail: &str) -> String {
        format!(
            "  - name: {name}\n    components:\n      Transform:\n        translation:\n{}      GroundMarker:\n        shape:\n          type: disc\n          radius: 12.0\n      GroundMarkerClip:\n        bins: {bins}\n        layers:\n          - sight_blocker\n{tail}",
            seq(10, &at)
        )
    }

    fn engine(entities: &[String]) -> Engine {
        let mut engine = Engine::builder().build().expect("boots");
        engine
            .load_inline_scene(&(HEAD.to_string() + &entities.concat()))
            .expect("loads");
        engine
    }

    fn extract(engine: &mut Engine) -> ludion_renderer::RenderScene {
        engine.extract_render_scene(64, 64).expect("extracts")
    }

    fn static_wall() -> String {
        wall("wall", [5.5, 0.0, 0.0], [0.5, 2.0, 100.0], "sight_blocker")
    }

    #[test]
    fn an_unmoved_static_clip_casts_once_across_extracts() {
        let mut engine = engine(&[static_wall(), marker("m", [0.0; 3], 64, "")]);
        extract(&mut engine);
        assert_eq!(engine.ground_marker_clip_rays(), 64);
        engine.advance_live();
        let first = extract(&mut engine);
        let second = extract(&mut engine);
        assert_eq!(
            engine.ground_marker_clip_rays(),
            64,
            "held across ticks and extracts"
        );
        assert_eq!(first.ground_marker_profiles, second.ground_marker_profiles);
    }

    #[test]
    fn a_moved_static_wall_recasts() {
        let mut engine = engine(&[static_wall(), marker("m", [0.0; 3], 64, "")]);
        let before = extract(&mut engine);
        engine.advance_live();
        let wall_id = engine
            .world_mut()
            .query_ids::<(&Name,)>()
            .find(|(_, (name,))| name.0 == "wall")
            .map(|(id, _)| id)
            .unwrap();
        engine
            .world_mut()
            .get_mut::<Transform>(wall_id)
            .unwrap()
            .unwrap()
            .translation = [8.5, 0.0, 0.0];
        engine.advance_live();
        let after = extract(&mut engine);
        assert_eq!(
            engine.ground_marker_clip_rays(),
            128,
            "the move recast once"
        );
        assert_ne!(before.ground_marker_profiles, after.ground_marker_profiles);
    }

    fn wall_id(engine: &mut Engine) -> ludion_core::ids::EntityId {
        engine
            .world_mut()
            .query_ids::<(&Name,)>()
            .find(|(_, (name,))| name.0 == "wall")
            .map(|(id, _)| id)
            .unwrap()
    }

    fn set_wall_layer(engine: &mut Engine, layer: &str) {
        let id = wall_id(engine);
        engine
            .world_mut()
            .get_mut::<ludion_spatial::Collider>(id)
            .unwrap()
            .unwrap()
            .layer = Some(layer.to_string());
        engine.advance_live();
    }

    #[test]
    fn a_collider_layer_change_recasts_and_back() {
        let mut engine = engine(&[static_wall(), marker("m", [0.0; 3], 64, "")]);
        engine.advance_live();
        let blocked = extract(&mut engine);
        assert_eq!(engine.ground_marker_clip_rays(), 64);

        set_wall_layer(&mut engine, "low_cover");
        let open = extract(&mut engine);
        assert_eq!(
            engine.ground_marker_clip_rays(),
            128,
            "the layer edit recast once"
        );
        assert_ne!(blocked.ground_marker_profiles, open.ground_marker_profiles);
        assert!(
            open.ground_marker_profiles[4..]
                .iter()
                .all(|&r| r >= 12.0 - 1e-3),
            "no clipping bin: {:?}",
            &open.ground_marker_profiles[4..]
        );

        set_wall_layer(&mut engine, "sight_blocker");
        let again = extract(&mut engine);
        assert_eq!(
            engine.ground_marker_clip_rays(),
            192,
            "blocking again recast once"
        );
        assert_eq!(blocked.ground_marker_profiles, again.ground_marker_profiles);

        engine.advance_live();
        extract(&mut engine);
        extract(&mut engine);
        assert_eq!(
            engine.ground_marker_clip_rays(),
            192,
            "an unrelated tick casts nothing"
        );
    }

    #[test]
    fn a_query_layers_edit_recasts_and_back() {
        let mut engine = engine(&[
            wall("wall", [5.5, 0.0, 0.0], [0.5, 2.0, 100.0], "low_cover"),
            marker("m", [0.0; 3], 64, ""),
        ]);
        engine.advance_live();
        let open = extract(&mut engine);
        assert_eq!(engine.ground_marker_clip_rays(), 64);

        let id = wall_id(&mut engine);
        engine
            .world_mut()
            .insert(
                id,
                ludion_physics::QueryLayers {
                    layers: ludion_physics::LayerSet::from(vec!["sight_blocker".to_string()]),
                },
            )
            .unwrap();
        engine.advance_live();
        let blocked = extract(&mut engine);
        assert_eq!(
            engine.ground_marker_clip_rays(),
            128,
            "the query mask recast once"
        );
        assert_ne!(open.ground_marker_profiles, blocked.ground_marker_profiles);

        engine
            .world_mut()
            .remove::<ludion_physics::QueryLayers>(id)
            .unwrap();
        engine.advance_live();
        let again = extract(&mut engine);
        assert_eq!(engine.ground_marker_clip_rays(), 192);
        assert_eq!(open.ground_marker_profiles, again.ground_marker_profiles);
    }

    #[test]
    fn a_dynamic_clip_casts_every_extract() {
        let mut engine = engine(&[
            static_wall(),
            marker("m", [0.0; 3], 64, "        dynamic: true\n"),
        ]);
        for _ in 0..3 {
            extract(&mut engine);
        }
        assert_eq!(engine.ground_marker_clip_rays(), 3 * 64);
    }

    /// 30 dynamic clipped markers at 128 bins (10 with a partial layer) over static walls: the ray
    /// count per extract is exact, every profile matches the wall geometry, and the median wall time
    /// is printed (never asserted).
    #[test]
    #[allow(clippy::disallowed_methods)] // wall time is only printed, never asserted
    fn thirty_dynamic_markers_cost_a_known_ray_count() {
        let mut entities = vec![
            static_wall(),
            wall(
                "far-x",
                [-20.0, 0.0, 0.0],
                [0.5, 2.0, 100.0],
                "sight_blocker",
            ),
            wall(
                "far-z",
                [0.0, 0.0, 40.0],
                [100.0, 2.0, 0.5],
                "sight_blocker",
            ),
            wall("cover", [-3.0, 0.0, 0.0], [0.25, 2.0, 1.0], "low_cover"),
        ];
        for n in 0..30 {
            let partial = if n % 3 == 0 {
                "        partial_layers:\n          - low_cover\n"
            } else {
                ""
            };
            entities.push(marker(
                &format!("m{n}"),
                [0.0, 0.0, n as f32 - 15.0],
                128,
                &format!("        dynamic: true\n{partial}"),
            ));
        }
        let mut engine = engine(&entities);
        engine.advance_live();
        let mut times = Vec::new();
        let mut last = extract(&mut engine);
        let base = engine.ground_marker_clip_rays();
        assert_eq!(
            base,
            30 * 128 + 10 * 128,
            "one extract: 30 casts plus 10 partial casts"
        );
        for _ in 0..20 {
            let started = Instant::now();
            last = extract(&mut engine);
            times.push(started.elapsed().as_secs_f64() * 1e3);
        }
        assert_eq!(engine.ground_marker_clip_rays(), base * 21);
        times.sort_by(f64::total_cmp);
        println!(
            "ground marker clip cost: median {:.3} ms per extract of 30 dynamic markers at 128 bins (min {:.3}, max {:.3})",
            times[10], times[0], times[19]
        );
        let floats = &last.ground_marker_profiles;
        assert_eq!(floats.len(), 30 * (4 + 256));
        for (n, marker) in last.ground_markers.iter().enumerate() {
            let at = marker.clip.expect("clipped") as usize;
            assert_eq!(at, n * 260);
            let radii = &floats[at + 4..at + 4 + 128];
            for (i, &r) in radii.iter().enumerate() {
                let a = -std::f32::consts::PI + (i as f32 + 0.5) * std::f32::consts::TAU / 128.0;
                let toward = 5.0 / cos(a);
                if cos(a) > 0.0 && toward < 11.9 {
                    assert!(
                        (r - toward).abs() < 1e-3,
                        "marker {n} bin {i}: {r} vs {toward}"
                    );
                } else if cos(a) > 0.0 && toward > 12.1 {
                    assert_eq!(r, 12.0, "marker {n} bin {i} sees past the wall's reach");
                }
            }
        }
    }
}
