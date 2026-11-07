//! Drawing the projectile pool: the game's visual table and the extract step that turns live shots
//! into scene instances.
//!
//! **No new render pass.** A billboard look becomes a [`ParticleInstance`] (the analytic soft-disc
//! pipeline: HDR colour, additive or alpha, sorted, instanced) and a mesh look becomes a
//! [`MeshInstance`] (a `Cube` or `Sphere` primitive, instanced per shape). Both are drawn by the
//! scene pass's existing paths, so tracers share the fog, bloom, depth and viewmodel rules of
//! particles and meshes, and the frame graph, every shader and every golden stay as they are.
//! `particle.wgsl` places a disc's corners at `center + corner.x * size * right + corner.y * size * up`,
//! so a non-unit `right` stretches the disc into a streak along the heading.
//!
//! The table is a **builder host setting** ([`EngineBuilder::projectile_visuals`](crate::EngineBuilder::projectile_visuals)),
//! the twin of `.projectiles(config)`: a shot's `tag` is an opaque Rust-side `u32`, so the table is
//! neither a component nor a document kind and adds nothing to the wire schema.

use std::collections::BTreeMap;

use ludion_core::codes::code;
use ludion_core::diagnostics::Diagnostic;
use ludion_core::math::glam::{Mat4, Quat, Vec3};
use ludion_ecs::World;
use ludion_projectiles::ProjectileId;
use ludion_renderer::{MeshInstance, MeshShape, ParticleInstance, ProjectileItem, RenderScene};

use crate::particles::CameraBasis;

/// How one kind of shot is drawn.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ProjectileLook {
    /// A soft glowing disc stretched along the heading (a particle instance).
    Billboard {
        /// `0` is a hard anti-aliased edge, `1` a gaussian falloff; in `[0, 1]`.
        softness: f32,
    },
    /// A unit cube scaled to `size × size × length` and aimed along the heading.
    Cube,
    /// A unit sphere scaled to `size × size × length` and aimed along the heading.
    Sphere,
    /// Not drawn (a pure-logic shot such as a hitscan marker).
    Hidden,
}

/// The look of one shot tag.
#[derive(Clone, Debug, PartialEq)]
pub struct ProjectileVisual {
    /// What to draw.
    pub look: ProjectileLook,
    /// Width in metres.
    pub size: f32,
    /// Seconds of travel the streak covers: the drawn length is `max(size, speed * stretch)`.
    pub stretch: f32,
    /// Linear RGBA.
    pub color: [f32; 4],
    /// A mesh's emissive strength (its material's emissive channel), or a billboard's RGB multiplier.
    pub emissive: f32,
    /// Billboard only: additive blending (the usual tracer) rather than alpha.
    pub additive: bool,
    /// Draw new shots of this tag leaving [`ProjectileVisuals::muzzle`] and converge them; `false`
    /// (default) draws them at their simulated position — enemy shots, for example.
    pub from_muzzle: bool,
}

impl Default for ProjectileVisual {
    fn default() -> Self {
        Self {
            look: ProjectileLook::Billboard { softness: 1.0 },
            size: 0.05,
            stretch: 0.0,
            color: [1.0; 4],
            emissive: 4.0,
            additive: true,
            from_muzzle: false,
        }
    }
}

/// The game's table of shot looks, keyed by `ProjectileDesc::tag`.
///
/// A tag with no entry draws as [`fallback`](ProjectileVisuals::fallback), so a pool with no table
/// still draws. The default has no entries and a small white additive billboard as the fallback.
///
/// # Example
/// ```
/// use ludion_engine::{ProjectileLook, ProjectileVisual, ProjectileVisuals};
///
/// let mut visuals = ProjectileVisuals::default();
/// visuals.tags.insert(
///     1,
///     ProjectileVisual {
///         look: ProjectileLook::Billboard { softness: 0.6 },
///         size: 0.08,
///         stretch: 0.02,
///         color: [1.0, 0.7, 0.2, 1.0],
///         ..ProjectileVisual::default()
///     },
/// );
/// assert!(visuals.validate().is_ok());
/// visuals.tags.get_mut(&1).unwrap().size = 0.0;
/// assert!(visuals.validate().is_err());
/// ```
#[derive(Clone, Debug, PartialEq)]
pub struct ProjectileVisuals {
    /// The look of each tag.
    pub tags: BTreeMap<u32, ProjectileVisual>,
    /// The look of a tag with no entry.
    pub fallback: ProjectileVisual,
    /// `Name` of the entity a shot is drawn from: a `Viewmodel` (or a child of one) under a
    /// first-person camera. A shot of a [`from_muzzle`](ProjectileVisual::from_muzzle) tag, first
    /// seen by extract, is drawn at that point and converges onto its simulated path over
    /// [`converge_distance`](Self::converge_distance); the offset is render-only and never reaches
    /// the sim or the hash. `None` draws shots at their simulated position; a name that cannot be
    /// resolved does too, with `L4029`.
    ///
    /// Convergence needs extracts across ticks (the store remembers each shot's first sighting): a
    /// one-shot render, a restore or the first frame after a load extracts unprimed, so shots in
    /// flight draw at their simulated position.
    pub muzzle: Option<String>,
    /// Metres of straight-line travel from first sight over which a shot converges, eased by
    /// smoothstep; `0` disables convergence. A ricochet inside the distance converges a little later.
    pub converge_distance: f32,
}

impl Default for ProjectileVisuals {
    fn default() -> Self {
        Self {
            tags: BTreeMap::new(),
            fallback: ProjectileVisual::default(),
            muzzle: None,
            converge_distance: 4.0,
        }
    }
}

impl ProjectileVisuals {
    /// Checks the table.
    ///
    /// # Errors
    /// `L4028` for a non-finite or non-positive `size`, a negative or non-finite `stretch`,
    /// `color`, `emissive` or `converge_distance`, or a `softness` outside `[0, 1]`; the path names
    /// the field (`/tags/3/size`, `/fallback/color`).
    pub fn validate(&self) -> Result<(), Diagnostic> {
        self.fallback.validate("/fallback")?;
        for (tag, visual) in &self.tags {
            visual.validate(&format!("/tags/{tag}"))?;
        }
        if !self.converge_distance.is_finite() || self.converge_distance < 0.0 {
            return Err(invalid(
                "/converge_distance",
                format!(
                    "projectile `converge_distance` is {}, not a finite number >= 0",
                    self.converge_distance
                ),
            ));
        }
        Ok(())
    }

    /// The visual of `tag`: its entry, else the fallback.
    fn of(&self, tag: u32) -> &ProjectileVisual {
        self.tags.get(&tag).unwrap_or(&self.fallback)
    }
}

impl ProjectileVisual {
    fn validate(&self, at: &str) -> Result<(), Diagnostic> {
        let field = |name: &str, value: f32, ok: bool, want: &str| {
            if ok && value.is_finite() {
                Ok(())
            } else {
                Err(invalid(
                    &format!("{at}/{name}"),
                    format!("projectile `{name}` is {value}, {want}"),
                ))
            }
        };
        field(
            "size",
            self.size,
            self.size > 0.0,
            "not a finite number > 0",
        )?;
        field(
            "stretch",
            self.stretch,
            self.stretch >= 0.0,
            "not a finite number >= 0",
        )?;
        field(
            "emissive",
            self.emissive,
            self.emissive >= 0.0,
            "not a finite number >= 0",
        )?;
        for (i, c) in self.color.iter().enumerate() {
            field(
                &format!("color/{i}"),
                *c,
                *c >= 0.0,
                "not a finite number >= 0",
            )?;
        }
        if let ProjectileLook::Billboard { softness } = self.look {
            field(
                "look/softness",
                softness,
                (0.0..=1.0).contains(&softness),
                "outside 0..=1",
            )?;
        }
        Ok(())
    }
}

fn invalid(path: &str, what: String) -> Diagnostic {
    Diagnostic::new(code::PROJECTILE_VISUALS_INVALID, what)
        .with_path(path)
        .with_help(
            "fix the field in the `ProjectileVisuals` given to `EngineBuilder::projectile_visuals`",
        )
}

/// The engine's per-host projectile drawing state: the visual table and the convergence store.
#[derive(Clone, Debug, Default)]
pub(crate) struct ProjectileDraw {
    pub(crate) visuals: ProjectileVisuals,
    /// Whether the store has seen a pool extract since the last reset; shots already in flight at the
    /// first sight are recorded with no offset.
    primed: bool,
    /// Per pool slot: the shot last seen there.
    seen: Vec<Option<Seen>>,
}

/// A shot's first sighting.
#[derive(Clone, Copy, Debug)]
struct Seen {
    generation: u32,
    /// Simulated position at first sight.
    origin: Vec3,
    /// Muzzle minus `origin` (zero when the muzzle is unresolved or the shot was in flight).
    offset: Vec3,
}

impl ProjectileDraw {
    /// Forgets every sighting: the next extract with a pool is a first sight.
    pub(crate) fn reset_convergence(&mut self) {
        self.primed = false;
        self.seen.clear();
    }

    /// The offset to add to the shot's simulated `position` this frame.
    fn offset(
        &mut self,
        id: ProjectileId,
        position: Vec3,
        muzzle: Option<Vec3>,
        priming: bool,
    ) -> Vec3 {
        let slot = id.index as usize;
        if self.seen.len() <= slot {
            self.seen.resize(slot + 1, None);
        }
        let entry = match self.seen[slot] {
            Some(seen) if seen.generation == id.generation => seen,
            _ => {
                let seen = Seen {
                    generation: id.generation,
                    origin: position,
                    offset: if priming {
                        Vec3::ZERO
                    } else {
                        muzzle.map_or(Vec3::ZERO, |m| m - position)
                    },
                };
                self.seen[slot] = Some(seen);
                seen
            }
        };
        let distance = self.visuals.converge_distance;
        let w = if distance > 0.0 {
            let t = ((position - entry.origin).length() / distance).clamp(0.0, 1.0);
            1.0 - t * t * (3.0 - 2.0 * t)
        } else {
            0.0
        };
        entry.offset * w
    }
}

/// Appends the live shots to `scene` (meshes, then particles) and returns their blend keys,
/// ascending. A world with no pool, or an empty one, returns nothing and leaves `scene` untouched.
/// `muzzle` is the world point a new shot starts drawing from (see
/// [`ProjectileVisuals::muzzle`]).
///
/// Called after the scene's own lists are built and before the viewmodel is composed: appending
/// after decals and drop shadows keeps them off the shots, and GI (which reads the placed meshes)
/// never sees them.
#[allow(clippy::disallowed_methods)] // render-side: GPU instance data, never reaches a hash
pub(crate) fn extract_projectiles(
    scene: &mut RenderScene,
    world: &World,
    draw: &mut ProjectileDraw,
    muzzle: Option<Vec3>,
) -> Vec<(u64, ProjectileItem)> {
    let Some(res) = world.resource::<crate::Projectiles>() else {
        draw.reset_convergence();
        return Vec::new();
    };
    let pool = res.pool();
    // An extract of an empty pool primes too: only shots already in flight at the first sight draw
    // with no offset.
    let priming = !draw.primed;
    draw.primed = true;
    if pool.is_empty() {
        return Vec::new();
    }
    let eye = scene.camera.position;
    let camera_basis = CameraBasis::from_view(scene.camera.view);
    let mut keys = Vec::with_capacity(pool.len());
    for id in pool.ids() {
        let (Some(position), Some(velocity), Some(tag)) =
            (pool.position(id), pool.velocity(id), pool.tag(id))
        else {
            continue;
        };
        let (position, velocity) = (Vec3::from(position), Vec3::from(velocity));
        let from = muzzle.filter(|_| draw.visuals.of(tag).from_muzzle);
        let position = position + draw.offset(id, position, from, priming);
        let visual = draw.visuals.of(tag);
        let speed = velocity.length();
        let dir = if speed > 0.0 {
            velocity / speed
        } else {
            Vec3::ZERO
        };
        let length = if speed > 0.0 {
            visual.size.max(speed * visual.stretch)
        } else {
            visual.size
        };
        let key = (u64::from(id.index) << 32) | u64::from(id.generation);
        match visual.look {
            ProjectileLook::Hidden => {}
            ProjectileLook::Cube | ProjectileLook::Sphere => {
                let rotation = if speed > 0.0 {
                    Quat::from_rotation_arc(Vec3::Z, dir)
                } else {
                    Quat::IDENTITY
                };
                let scale = Vec3::new(visual.size, visual.size, length);
                let model = Mat4::from_scale_rotation_translation(scale, rotation, position);
                let shape = if visual.look == ProjectileLook::Cube {
                    MeshShape::Cube
                } else {
                    MeshShape::Sphere
                };
                keys.push((key, ProjectileItem::Mesh(scene.meshes.len() as u32)));
                scene.meshes.push(MeshInstance {
                    shape,
                    model,
                    normal_matrix: crate::extract::normal_matrix(model, scale.to_array()),
                    color: visual.color,
                    material: [0.5, 0.0, visual.emissive],
                    use_vertex_material: false,
                    fade: 1.0,
                    fade_height_min: -f32::MAX,
                    flash: 0.0,
                    flash_color: [1.0; 3],
                    wrap: 0.0,
                    scatter_color: [1.0; 3],
                    bone_offset: 0,
                    cast_shadows: false,
                    viewmodel: false,
                });
            }
            ProjectileLook::Billboard { softness } => {
                let (right, up) = if speed > 0.0 {
                    let view = (position - eye).normalize_or_zero();
                    let up = dir.cross(view);
                    if up.length_squared() > 1e-8 {
                        (dir * (length / visual.size), up.normalize())
                    } else {
                        (camera_basis.right, camera_basis.up)
                    }
                } else {
                    (camera_basis.right, camera_basis.up)
                };
                let [r, g, b, a] = visual.color;
                keys.push((key, ProjectileItem::Particle(scene.particles.len() as u32)));
                scene.particles.push(ParticleInstance {
                    center: position,
                    right,
                    up,
                    size: visual.size,
                    color: [
                        r * visual.emissive,
                        g * visual.emissive,
                        b * visual.emissive,
                        a,
                    ],
                    softness,
                    additive: visual.additive,
                    lit: false,
                    viewmodel: false,
                });
            }
        }
    }
    keys.sort_unstable_by_key(|(key, _)| *key);
    keys
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Engine, ProjectileConfig, Projectiles};
    use ludion_projectiles::ProjectileDesc;

    const SCENE: &str = "\
format: 1
seed: 1
entities:
  - name: camera
    components:
      Transform:
        translation:
          - 0.0
          - 0.0
          - 10.0
      Camera: {}
  - name: block
    components:
      Transform:
        translation:
          - 0.0
          - 0.0
          - 0.0
      Mesh: {}
";

    fn engine(config: bool, visuals: ProjectileVisuals) -> Engine {
        let mut builder = Engine::builder();
        if config {
            builder = builder
                .projectiles(ProjectileConfig {
                    capacity: 8,
                    ..ProjectileConfig::default()
                })
                .projectile_visuals(visuals);
        }
        let mut engine = builder.build().expect("boots");
        engine.load_inline_scene(SCENE).unwrap();
        engine
    }

    fn fire(engine: &mut Engine, velocity: [f32; 3], tag: u32) {
        let mut desc = ProjectileDesc::new([1.0, 2.0, 0.0], velocity);
        desc.tag = tag;
        engine
            .world_mut()
            .resource_mut::<Projectiles>()
            .unwrap()
            .spawn(desc)
            .unwrap();
    }

    fn visual(look: ProjectileLook) -> ProjectileVisual {
        ProjectileVisual {
            look,
            size: 0.1,
            stretch: 0.01,
            color: [0.5, 0.25, 1.0, 1.0],
            emissive: 3.0,
            additive: true,
            from_muzzle: false,
        }
    }

    #[test]
    fn no_pool_or_an_empty_pool_leaves_the_scene_unchanged() {
        let plain = engine(false, ProjectileVisuals::default())
            .extract_present_scene()
            .unwrap();
        let pooled = engine(true, ProjectileVisuals::default())
            .extract_present_scene()
            .unwrap();
        assert_eq!(plain.0, pooled.0);
        assert!(pooled.1.projectiles.is_empty());
    }

    #[test]
    fn a_tag_picks_its_visual_and_unknown_tags_fall_back() {
        let mut visuals = ProjectileVisuals::default();
        visuals.tags.insert(1, visual(ProjectileLook::Cube));
        visuals.tags.insert(2, visual(ProjectileLook::Hidden));
        let mut engine = engine(true, visuals);
        for tag in [1, 2, 9] {
            fire(&mut engine, [100.0, 0.0, 0.0], tag);
        }
        let (scene, blend) = engine.extract_present_scene().unwrap();
        // Entity mesh + the cube shot; the fallback billboard is the only particle; tag 2 is hidden.
        assert_eq!(scene.meshes.len(), 2);
        assert_eq!(scene.meshes[1].shape, MeshShape::Cube);
        assert_eq!(scene.particles.len(), 1);
        assert_eq!(blend.projectiles.len(), 2);
        assert_eq!(blend.meshes.len(), scene.meshes.len());
        assert_eq!(blend.meshes[1], None);
    }

    #[test]
    fn a_mesh_shot_aims_along_its_velocity_and_stretches_with_speed() {
        let mut visuals = ProjectileVisuals::default();
        visuals.tags.insert(1, visual(ProjectileLook::Sphere));
        let mut engine = engine(true, visuals);
        fire(&mut engine, [0.0, 200.0, 0.0], 1);
        let (scene, _) = engine.extract_present_scene().unwrap();
        let shot = &scene.meshes[1];
        let z = shot.model.z_axis.truncate();
        assert!(
            (z.length() - 200.0 * 0.01).abs() < 1e-4,
            "length {}",
            z.length()
        );
        assert!(z.normalize().dot(Vec3::Y) > 0.9999);
        assert!((shot.model.x_axis.truncate().length() - 0.1).abs() < 1e-5);
        assert_eq!(shot.model.w_axis.truncate(), Vec3::new(1.0, 2.0, 0.0));
        assert_eq!(shot.material[2], 3.0);
        assert!(!shot.cast_shadows);
    }

    #[test]
    fn a_billboard_streaks_along_its_velocity_and_carries_emissive() {
        let mut visuals = ProjectileVisuals::default();
        visuals
            .tags
            .insert(1, visual(ProjectileLook::Billboard { softness: 0.5 }));
        let mut engine = engine(true, visuals);
        fire(&mut engine, [0.0, 50.0, 0.0], 1);
        let (scene, _) = engine.extract_present_scene().unwrap();
        let p = scene.particles.last().unwrap();
        assert!(p.right.normalize().dot(Vec3::Y) > 0.9999);
        assert!((p.right.length() * p.size - 0.5).abs() < 1e-4);
        assert!((p.up.length() - 1.0).abs() < 1e-5);
        assert_eq!(p.color, [1.5, 0.75, 3.0, 1.0]);
        assert_eq!(p.softness, 0.5);
        assert!(!p.lit);
    }

    #[test]
    fn invalid_visuals_are_refused_at_build_with_a_pointer() {
        let mut visuals = ProjectileVisuals::default();
        visuals.tags.insert(
            3,
            ProjectileVisual {
                size: 0.0,
                ..ProjectileVisual::default()
            },
        );
        let error = Engine::builder()
            .projectile_visuals(visuals)
            .build()
            .err()
            .expect("size 0 is refused");
        assert_eq!(error.code, code::PROJECTILE_VISUALS_INVALID);
        assert_eq!(error.path.as_deref(), Some("/tags/3/size"));

        let softness = ProjectileVisuals {
            fallback: visual(ProjectileLook::Billboard { softness: 2.0 }),
            ..ProjectileVisuals::default()
        };
        assert_eq!(
            softness.validate().unwrap_err().path.as_deref(),
            Some("/fallback/look/softness")
        );
    }

    const MUZZLE_SCENE: &str = "\
format: 1
seed: 1
entities:
  - name: camera
    components:
      Transform:
        translation:
          - 0.0
          - 1.6
          - 0.0
      Camera: {}
      CameraRig:
        target:
          index: 1
          generation: 0
        rig:
          mode:
            first_person: {}
  - name: hero
    components:
      Transform: {}
  - name: muzzle
    presentation: true
    components:
      Parent:
        index: 0
        generation: 0
      Transform:
        translation:
          - 0.2
          - -0.2
          - -0.7
      Viewmodel: {}
  - name: loose
    components:
      Transform: {}
";

    fn muzzle_engine(muzzle: &str, scene: &str) -> Engine {
        let visuals = ProjectileVisuals {
            muzzle: Some(muzzle.to_string()),
            fallback: ProjectileVisual {
                from_muzzle: true,
                ..ProjectileVisual::default()
            },
            ..ProjectileVisuals::default()
        };
        let mut engine = Engine::builder()
            .projectiles(ProjectileConfig {
                capacity: 8,
                ..ProjectileConfig::default()
            })
            .projectile_visuals(visuals)
            .build()
            .expect("boots");
        engine.load_inline_scene(scene).unwrap();
        engine
    }

    fn drawn(engine: &mut Engine) -> Vec3 {
        let (scene, _) = engine.extract_present_scene().unwrap();
        scene.particles.last().expect("a shot draws").center
    }

    #[test]
    fn a_new_shot_draws_at_the_muzzle_and_an_unprimed_one_at_its_sim_position() {
        let mut engine = muzzle_engine("muzzle", MUZZLE_SCENE);
        // Empty pool extract primes the store.
        engine.extract_present_scene().unwrap();
        fire(&mut engine, [0.0, 0.0, -50.0], 0);
        let at = drawn(&mut engine);
        // Camera at (0, 1.6, 0) looking down -Z; muzzle local (0.2, -0.2, -0.7), rescaled by the lens.
        assert!((at.z - -0.7).abs() < 1e-3, "{at:?}");
        assert!(at.x > 0.0 && at.y < 1.6, "{at:?}");

        let mut cold = muzzle_engine("muzzle", MUZZLE_SCENE);
        fire(&mut cold, [0.0, 0.0, -50.0], 0);
        assert_eq!(
            drawn(&mut cold),
            Vec3::new(1.0, 2.0, 0.0),
            "unprimed: sim position"
        );
        // A world replacement unprimes again.
        let mut engine = muzzle_engine("muzzle", MUZZLE_SCENE);
        engine.extract_present_scene().unwrap();
        engine.load_inline_scene(MUZZLE_SCENE).unwrap();
        fire(&mut engine, [0.0, 0.0, -50.0], 0);
        assert_eq!(drawn(&mut engine), Vec3::new(1.0, 2.0, 0.0));
    }

    #[test]
    fn only_a_from_muzzle_tag_converges() {
        let mut visuals = ProjectileVisuals {
            muzzle: Some("muzzle".to_string()),
            ..ProjectileVisuals::default()
        };
        for (tag, from_muzzle) in [(0, true), (1, false)] {
            visuals.tags.insert(
                tag,
                ProjectileVisual {
                    from_muzzle,
                    ..ProjectileVisual::default()
                },
            );
        }
        let mut engine = Engine::builder()
            .projectiles(ProjectileConfig {
                capacity: 8,
                ..ProjectileConfig::default()
            })
            .projectile_visuals(visuals)
            .build()
            .expect("boots");
        engine.load_inline_scene(MUZZLE_SCENE).unwrap();
        engine.extract_present_scene().unwrap();
        fire(&mut engine, [0.0, 0.0, -50.0], 0);
        fire(&mut engine, [0.0, 0.0, -50.0], 1);
        let (scene, _) = engine.extract_present_scene().unwrap();
        let mut centers: Vec<Vec3> = scene.particles.iter().map(|p| p.center).collect();
        centers.sort_by(|a, b| a.z.total_cmp(&b.z));
        assert_eq!(centers.len(), 2);
        assert!((centers[0].z - -0.7).abs() < 1e-3, "{centers:?}");
        assert_eq!(centers[1], Vec3::new(1.0, 2.0, 0.0), "{centers:?}");
    }

    #[test]
    fn convergence_eases_to_the_sim_position_at_the_distance() {
        let mut draw = ProjectileDraw {
            visuals: ProjectileVisuals::default(),
            ..ProjectileDraw::default()
        };
        draw.primed = true;
        let id = ProjectileId {
            index: 3,
            generation: 1,
        };
        let muzzle = Some(Vec3::new(0.0, -1.0, 0.0));
        let start = Vec3::ZERO;
        assert_eq!(
            draw.offset(id, start, muzzle, false),
            Vec3::new(0.0, -1.0, 0.0)
        );
        let half = draw.offset(id, Vec3::new(0.0, 0.0, -2.0), muzzle, false);
        assert!(
            (half - Vec3::new(0.0, -0.5, 0.0)).length() < 1e-6,
            "{half:?}"
        );
        assert_eq!(
            draw.offset(id, Vec3::new(0.0, 0.0, -4.0), muzzle, false),
            Vec3::ZERO
        );
        assert_eq!(
            draw.offset(id, Vec3::new(0.0, 0.0, -9.0), muzzle, false),
            Vec3::ZERO
        );
        // A reused slot with a new generation is a new shot.
        let next = ProjectileId {
            index: 3,
            generation: 2,
        };
        assert_eq!(
            draw.offset(next, start, muzzle, false),
            Vec3::new(0.0, -1.0, 0.0)
        );
        // A shot first seen while priming has none; D = 0 disables convergence.
        let other = ProjectileId {
            index: 4,
            generation: 0,
        };
        assert_eq!(draw.offset(other, start, muzzle, true), Vec3::ZERO);
        draw.visuals.converge_distance = 0.0;
        let last = ProjectileId {
            index: 5,
            generation: 0,
        };
        assert_eq!(draw.offset(last, start, muzzle, false), Vec3::ZERO);
    }

    #[test]
    fn an_unresolved_muzzle_warns_and_draws_at_the_sim_position() {
        for (name, scene) in [("nobody", MUZZLE_SCENE), ("loose", MUZZLE_SCENE)] {
            let mut engine = muzzle_engine(name, scene);
            engine.extract_present_scene().unwrap();
            fire(&mut engine, [0.0, 0.0, -50.0], 0);
            assert_eq!(drawn(&mut engine), Vec3::new(1.0, 2.0, 0.0), "{name}");
            let sink = engine.world().resource::<crate::Diagnostics>().unwrap();
            assert_eq!(
                sink.reported(code::PROJECTILE_MUZZLE_UNRESOLVED).count(),
                1,
                "{name}: L4029 latched once"
            );
        }
    }
}
