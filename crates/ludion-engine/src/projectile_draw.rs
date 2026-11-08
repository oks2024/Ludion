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
use std::path::Path;

use ludion_core::codes::code;
use ludion_core::collections::StableMap;
use ludion_core::diagnostics::Diagnostic;
use ludion_core::math::glam::{Mat4, Quat, Vec3};
use ludion_ecs::World;
use ludion_projectiles::ProjectileId;
use ludion_renderer::{MeshInstance, MeshShape, ParticleInstance, ProjectileItem, RenderScene};

use ludion_assets::AssetRoot;

use crate::extract::SceneAssets;
use crate::particles::CameraBasis;

/// How one kind of shot is drawn.
#[derive(Clone, Debug, PartialEq)]
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
    /// A `*.mesh.yaml` asset (path relative to the project root), drawn with the shot's simulated
    /// orientation: the identity is the asset's authored pose. Extents are
    /// `size × `[`scale`](ProjectileVisual::scale), with no heading stretch. The pivot is the
    /// centre of the asset's bounding box, so a gib tumbles, settles and fades about its middle. A
    /// skinned asset draws at its bind pose. A missing file is refused at `build` (`L4033`); one that fails to load
    /// draws nothing, with `L4033`.
    Mesh {
        /// Project-relative path of the mesh document.
        asset: String,
    },
    /// A `*.voxel.yaml` model (path relative to the project root), loaded by the loader a
    /// `Mesh.shape: voxel` entity uses, so a model also placed by an entity uploads once. Drawn as
    /// [`Mesh`](ProjectileLook::Mesh) is: the shot's simulated orientation, extents
    /// `size × `[`scale`](ProjectileVisual::scale), the palette and authored material as a voxel
    /// entity draws them. One voxel is one unit, so `size` is metres per voxel; the document's own
    /// `scale` is not read. The pivot is the centre of the model's occupied box, not its
    /// bottom-centre origin. A `kind: assembly` is not a single model and is not accepted. A missing file is refused at
    /// `build` (`L4033`); one that fails to load draws nothing, with `L4033`.
    Voxel {
        /// Project-relative path of the voxel document.
        model: String,
    },
    /// Not drawn (a pure-logic shot such as a hitscan marker).
    Hidden,
}

/// Which loader resolves an asset-backed look, and how its diagnostics are spelled.
#[derive(Clone, Copy)]
enum SourceKind {
    Mesh,
    Voxel,
}

impl SourceKind {
    /// The look's field naming the path.
    fn field(self) -> &'static str {
        match self {
            SourceKind::Mesh => "asset",
            SourceKind::Voxel => "model",
        }
    }

    /// The noun in diagnostics.
    fn noun(self) -> &'static str {
        match self {
            SourceKind::Mesh => "mesh",
            SourceKind::Voxel => "voxel model",
        }
    }

    /// The degraded-latch key prefix.
    fn key_prefix(self) -> &'static str {
        match self {
            SourceKind::Mesh => "projectile-mesh",
            SourceKind::Voxel => "projectile-voxel",
        }
    }

    fn glob(self) -> &'static str {
        match self {
            SourceKind::Mesh => "*.mesh.yaml",
            SourceKind::Voxel => "*.voxel.yaml",
        }
    }

    fn load(
        self,
        assets: &mut dyn SceneAssets,
        rel: &str,
    ) -> Result<std::sync::Arc<ludion_renderer::MeshData>, Vec<Diagnostic>> {
        match self {
            SourceKind::Mesh => assets.mesh(rel),
            SourceKind::Voxel => assets.voxel(rel),
        }
    }
}

impl ProjectileLook {
    /// The project-relative path and loader of an asset-backed look.
    fn source(&self) -> Option<(&str, SourceKind)> {
        match self {
            ProjectileLook::Mesh { asset } => Some((asset, SourceKind::Mesh)),
            ProjectileLook::Voxel { model } => Some((model, SourceKind::Voxel)),
            _ => None,
        }
    }
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
    /// Scale in `[0, 1]` on the medium's optical depth along the view ray to this shot (default
    /// `1.0`, today's look): `0` shows no air between the eye and the shot. Render-only.
    pub medium: f32,
    /// Per-axis multiplier on the drawn extents of a `Cube`, `Sphere`, `Mesh` or `Voxel` look, each finite
    /// and > 0 (default `[1.0; 3]`). A heading-aimed shot draws `(size·x, size·y, length·z)`; an
    /// oriented draw, a `Mesh` or a `Voxel` look draws `size·scale`. Billboards ignore it.
    pub scale: [f32; 3],
    /// Whether a `Cube`, `Sphere`, `Mesh` or `Voxel` shot casts a shadow (default `false`: tracers do not).
    pub cast_shadows: bool,
    /// Seconds before expiry over which the drawn extents shrink linearly to nothing; `0`
    /// (default) is off. Opaque and scale only.
    pub fade_out: f32,
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
            medium: 1.0,
            scale: [1.0; 3],
            cast_shadows: false,
            fade_out: 0.0,
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
    /// `color`, `emissive`, `fade_out` or `converge_distance`, a non-positive or non-finite `scale`
    /// component, an empty `Mesh` asset or `Voxel` model, or a `softness` or `medium` outside `[0, 1]`; the path
    /// names the field (`/tags/3/size`, `/fallback/color`).
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

    /// Checks that every `Mesh` and `Voxel` look names a file under `root`.
    ///
    /// # Errors
    /// `L4033` at `/tags/<t>/look/asset` or `…/look/model` (or under `/fallback`), with the nearest existing
    /// name as the help when there is one.
    pub(crate) fn check_assets(&self, root: &Path) -> Result<(), Diagnostic> {
        let mut sites = vec![("/fallback".to_string(), &self.fallback)];
        sites.extend(self.tags.iter().map(|(t, v)| (format!("/tags/{t}"), v)));
        for (at, visual) in sites {
            let Some((asset, kind)) = visual.look.source() else {
                continue;
            };
            let root = AssetRoot::content(root);
            if ludion_assets::resolve(root, asset).is_ok_and(|p| p.is_file()) {
                continue;
            }
            let (field, noun) = (kind.field(), kind.noun());
            let help = match crate::extract::suggest_asset(root, asset) {
                Some(near) => format!("did you mean `{near}`?"),
                None => format!(
                    "point `{field}` at an existing `{}` under the project root",
                    kind.glob()
                ),
            };
            return Err(Diagnostic::new(
                code::PROJECTILE_LOOK_ASSET,
                format!("projectile look {noun} `{asset}` is not a file under the project root"),
            )
            .with_path(format!("{at}/look/{field}"))
            .with_help(help));
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
        field(
            "medium",
            self.medium,
            (0.0..=1.0).contains(&self.medium),
            "outside 0..=1",
        )?;
        field(
            "fade_out",
            self.fade_out,
            self.fade_out >= 0.0,
            "not a finite number >= 0",
        )?;
        for (i, c) in self.scale.iter().enumerate() {
            field(
                &format!("scale/{i}"),
                *c,
                *c > 0.0,
                "not a finite number > 0",
            )?;
        }
        match &self.look {
            ProjectileLook::Billboard { softness } => field(
                "look/softness",
                *softness,
                (0.0..=1.0).contains(softness),
                "outside 0..=1",
            )?,
            look => {
                if let Some((_, kind)) = look.source().filter(|(path, _)| path.is_empty()) {
                    let field = kind.field();
                    return Err(invalid(
                        &format!("{at}/look/{field}"),
                        format!("projectile {} look `{field}` is empty", kind.noun()),
                    ));
                }
            }
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
    assets: &mut dyn SceneAssets,
    mesh_path_index: &mut StableMap<String, usize>,
    degraded: &mut Vec<(String, Diagnostic)>,
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
    let looks = resolve_looks(&draw.visuals, scene, assets, mesh_path_index, degraded);
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
        let mesh_look = matches!(
            visual.look,
            ProjectileLook::Cube
                | ProjectileLook::Sphere
                | ProjectileLook::Mesh { .. }
                | ProjectileLook::Voxel { .. }
        );
        // A mesh-instanced look draws its simulated orientation when the shot has any.
        let oriented = mesh_look && {
            let spin = pool.spin(id).unwrap_or([0.0; 3]);
            let orient = pool.orientation(id).unwrap_or([0.0, 0.0, 0.0, 1.0]);
            spin.map(f32::to_bits) != [0; 3] || orient.map(f32::to_bits) != IDENTITY_BITS
        };
        let fade = if visual.fade_out > 0.0 {
            (pool.life(id).unwrap_or(0.0) / visual.fade_out).clamp(0.0, 1.0)
        } else {
            1.0
        };
        let posed = |oriented: bool| oriented || visual.fade_out > 0.0;
        let sim_rotation = || {
            let [x, y, z, w] = pool.orientation(id).unwrap_or([0.0, 0.0, 0.0, 1.0]);
            Quat::from_xyzw(x, y, z, w).normalize()
        };
        let [sx, sy, sz] = visual.scale;
        match &visual.look {
            ProjectileLook::Hidden => {}
            _ if mesh_look && fade <= 0.0 => {}
            ProjectileLook::Cube
            | ProjectileLook::Sphere
            | ProjectileLook::Mesh { .. }
            | ProjectileLook::Voxel { .. } => {
                let (shape, authored, rim, rotation, extents, pivot) = match &visual.look {
                    ProjectileLook::Mesh { .. } | ProjectileLook::Voxel { .. } => {
                        let Some(&Some((index, pivot))) =
                            visual.look.source().and_then(|(path, _)| looks.get(path))
                        else {
                            continue;
                        };
                        (
                            MeshShape::Asset(index),
                            scene.mesh_assets[index].authored_material,
                            crate::extract::resolve_rim_lane(
                                &scene.mesh_assets,
                                Some(index),
                                [0.0; 4],
                            ),
                            sim_rotation(),
                            Vec3::new(visual.size * sx, visual.size * sy, visual.size * sz),
                            pivot,
                        )
                    }
                    look => {
                        let shape = if *look == ProjectileLook::Cube {
                            MeshShape::Cube
                        } else {
                            MeshShape::Sphere
                        };
                        let (rotation, extents) = if oriented {
                            (
                                sim_rotation(),
                                Vec3::new(visual.size * sx, visual.size * sy, visual.size * sz),
                            )
                        } else {
                            let rotation = if speed > 0.0 {
                                Quat::from_rotation_arc(Vec3::Z, dir)
                            } else {
                                Quat::IDENTITY
                            };
                            (
                                rotation,
                                Vec3::new(visual.size * sx, visual.size * sy, length * sz),
                            )
                        };
                        (shape, false, [0.0; 4], rotation, extents, Vec3::ZERO)
                    }
                };
                let scale = extents * fade;
                let mut model = Mat4::from_scale_rotation_translation(scale, rotation, position);
                if pivot != Vec3::ZERO {
                    model *= Mat4::from_translation(-pivot);
                }
                let item = if posed(
                    oriented
                        || matches!(
                            visual.look,
                            ProjectileLook::Mesh { .. } | ProjectileLook::Voxel { .. }
                        ),
                ) {
                    ProjectileItem::Posed(scene.meshes.len() as u32)
                } else {
                    ProjectileItem::Mesh(scene.meshes.len() as u32)
                };
                keys.push((key, item));
                scene.meshes.push(MeshInstance {
                    shape,
                    model,
                    normal_matrix: crate::extract::normal_matrix(model, scale.to_array()),
                    color: visual.color,
                    material: [0.5, 0.0, visual.emissive],
                    use_vertex_material: authored,
                    fade: 1.0,
                    fade_height_min: -f32::MAX,
                    flash: 0.0,
                    flash_color: [1.0; 3],
                    wrap: 0.0,
                    scatter_color: [1.0; 3],
                    bone_offset: 0,
                    cast_shadows: visual.cast_shadows,
                    viewmodel: false,
                    rim,
                    medium: visual.medium,
                });
            }
            ProjectileLook::Billboard { softness } => {
                let softness = *softness;
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
                    medium: visual.medium,
                });
            }
        }
    }
    keys.sort_unstable_by_key(|(key, _)| *key);
    keys
}

const IDENTITY_BITS: [u32; 4] = [0, 0, 0, 0x3f80_0000];

/// The `mesh_assets` index and pivot of each distinct `Mesh` and `Voxel` look in the table (`None`
/// when it failed to load, latched in `degraded` once per asset). The pivot is the centre of the
/// asset's bounding box, for both looks. Resolved once per extract, before the shot loop,
/// through the same path index entity meshes use, so a shared asset uploads once.
fn resolve_looks(
    visuals: &ProjectileVisuals,
    scene: &mut RenderScene,
    assets: &mut dyn SceneAssets,
    mesh_path_index: &mut StableMap<String, usize>,
    degraded: &mut Vec<(String, Diagnostic)>,
) -> BTreeMap<String, Option<(usize, Vec3)>> {
    let mut out = BTreeMap::new();
    let sites = std::iter::once(("/fallback".to_string(), &visuals.fallback)).chain(
        visuals
            .tags
            .iter()
            .map(|(tag, visual)| (format!("/tags/{tag}"), visual)),
    );
    for (at, visual) in sites {
        let Some((asset, kind)) = visual.look.source() else {
            continue;
        };
        if out.contains_key(asset) {
            continue;
        }
        let index = if let Some(&index) = mesh_path_index.as_inner().get(asset) {
            Some(index)
        } else {
            match kind.load(assets, asset) {
                Ok(mesh) => {
                    let index = scene.mesh_assets.len();
                    scene.mesh_assets.push(mesh);
                    mesh_path_index.insert(asset.to_string(), index);
                    Some(index)
                }
                Err(diagnostics) => {
                    let first = diagnostics.into_iter().next();
                    let reason = first.map_or_else(String::new, |d| format!(": {}", d.message));
                    let (field, noun) = (kind.field(), kind.noun());
                    degraded.push((
                        format!("{}/{asset}", kind.key_prefix()),
                        Diagnostic::new(
                            code::PROJECTILE_LOOK_ASSET,
                            format!("projectile look {noun} `{asset}` cannot be drawn{reason}"),
                        )
                        .with_path(format!("{at}/look/{field}"))
                        .with_help(match kind {
                            SourceKind::Mesh => {
                                "repair the mesh document or point the look at another asset"
                            }
                            SourceKind::Voxel => {
                                "repair the voxel document or point the look at another model"
                            }
                        }),
                    ));
                    None
                }
            }
        };
        out.insert(
            asset.to_string(),
            index.map(|i| (i, bounds_centre(&scene.mesh_assets[i]))),
        );
    }
    out
}

/// The centre of a mesh's positions' bounding box; zero for an empty mesh.
fn bounds_centre(mesh: &ludion_renderer::MeshData) -> Vec3 {
    let mut points = mesh
        .positions
        .chunks_exact(3)
        .map(|p| Vec3::new(p[0], p[1], p[2]));
    let Some(first) = points.next() else {
        return Vec3::ZERO;
    };
    let (min, max) = points.fold((first, first), |(lo, hi), p| (lo.min(p), hi.max(p)));
    (min + max) * 0.5
}

#[cfg(test)]
#[allow(clippy::disallowed_methods)] // assertions on render-side instance data
mod tests {
    use super::*;
    use crate::{Engine, ProjectileConfig, Projectiles};
    use ludion_projectiles::ProjectileDesc;
    use std::path::Path;

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
            medium: 1.0,
            ..ProjectileVisual::default()
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
    fn a_tags_medium_reaches_its_billboard_and_its_mesh() {
        for (look, scene_has_it) in [
            (ProjectileLook::Billboard { softness: 0.5 }, true),
            (ProjectileLook::Cube, false),
        ] {
            let mut visuals = ProjectileVisuals::default();
            visuals.tags.insert(
                1,
                ProjectileVisual {
                    medium: 0.25,
                    ..visual(look)
                },
            );
            let mut engine = engine(true, visuals);
            fire(&mut engine, [0.0, 50.0, 0.0], 1);
            let (scene, _) = engine.extract_present_scene().unwrap();
            if scene_has_it {
                assert_eq!(scene.particles.last().unwrap().medium, 0.25);
            } else {
                assert_eq!(scene.meshes.last().unwrap().medium, 0.25);
            }
        }
    }

    #[test]
    fn an_out_of_range_medium_is_refused_with_its_pointer() {
        for bad in [-0.1, 1.5, f32::NAN] {
            let mut visuals = ProjectileVisuals::default();
            visuals.tags.insert(
                2,
                ProjectileVisual {
                    medium: bad,
                    ..ProjectileVisual::default()
                },
            );
            let error = visuals.validate().unwrap_err();
            assert_eq!(error.code, code::PROJECTILE_VISUALS_INVALID);
            assert_eq!(error.path.as_deref(), Some("/tags/2/medium"));
        }
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

    const CRYSTAL: &str = include_str!("../../../examples/gallery/meshes/crystal.mesh.yaml");

    /// One triangle whose bounding-box centre, (1, 2, 0), is off its origin.
    const OFF_CENTRE: &str = "\
format: 1
kind: mesh
name: off-centre
positions:
  - 0.0
  - 0.0
  - 0.0
  - 2.0
  - 0.0
  - 0.0
  - 0.0
  - 4.0
  - 0.0
indices:
  - 0
  - 1
  - 2
";

    const VOXEL_GIB: &str = "\
format: 1
kind: voxel
name: gib
size:
  - 2
  - 2
  - 2
scale: 1.0
palette:
  E:
    color:
      - 0.4
      - 0.8
      - 1.0
      - 1.0
    emissive: 3.0
  T:
    color:
      - 0.85
      - 0.6
      - 0.25
      - 1.0
slices:
  -
    - \"EE\"
    - \"TT\"
  -
    - \"EE\"
    - \"TT\"
";

    fn project_with(name: &str, mesh: Option<&str>) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("ludion-gibs-{}-{name}", std::process::id()));
        std::fs::create_dir_all(dir.join("meshes")).unwrap();
        std::fs::create_dir_all(dir.join("voxels")).unwrap();
        if let Some(text) = mesh {
            std::fs::write(dir.join("meshes/gib.mesh.yaml"), text).unwrap();
        }
        std::fs::write(dir.join("voxels/gib.voxel.yaml"), VOXEL_GIB).unwrap();
        dir
    }

    fn voxel_look() -> ProjectileLook {
        ProjectileLook::Voxel {
            model: "voxels/gib.voxel.yaml".into(),
        }
    }

    fn gib_engine(root: &Path, visuals: ProjectileVisuals) -> Engine {
        let mut engine = Engine::builder()
            .project_root(root)
            .projectiles(ProjectileConfig {
                capacity: 8,
                ..ProjectileConfig::default()
            })
            .projectile_visuals(visuals)
            .build()
            .expect("boots");
        engine.load_inline_scene(SCENE).unwrap();
        engine
    }

    fn gib(tag_look: ProjectileLook) -> ProjectileVisual {
        ProjectileVisual {
            scale: [1.0, 2.0, 3.0],
            cast_shadows: true,
            ..visual(tag_look)
        }
    }

    fn mesh_look() -> ProjectileLook {
        ProjectileLook::Mesh {
            asset: "meshes/gib.mesh.yaml".into(),
        }
    }

    fn spin_shot(engine: &mut Engine, tag: u32, spin: [f32; 3], orientation: [f32; 4], life: f32) {
        let mut desc = ProjectileDesc::new([1.0, 2.0, 0.0], [0.0; 3]);
        desc.tag = tag;
        desc.spin = spin;
        desc.orientation = orientation;
        desc.lifetime = life;
        engine
            .world_mut()
            .resource_mut::<Projectiles>()
            .unwrap()
            .spawn(desc)
            .unwrap();
    }

    #[test]
    fn an_unoriented_cube_draws_exactly_as_before() {
        let mut visuals = ProjectileVisuals::default();
        visuals.tags.insert(1, visual(ProjectileLook::Cube));
        let mut engine = engine(true, visuals);
        fire(&mut engine, [100.0, 0.0, 0.0], 1);
        let (scene, blend) = engine.extract_present_scene().unwrap();
        let rotation = Quat::from_rotation_arc(Vec3::Z, Vec3::X);
        let scale = Vec3::new(0.1, 0.1, 1.0);
        let model =
            Mat4::from_scale_rotation_translation(scale, rotation, Vec3::new(1.0, 2.0, 0.0));
        assert_eq!(scene.meshes[1].model, model);
        assert!(!scene.meshes[1].cast_shadows);
        assert!(matches!(blend.projectiles[0].1, ProjectileItem::Mesh(1)));
    }

    #[test]
    fn a_mesh_look_draws_the_sim_orientation_per_axis_and_shares_its_asset() {
        let root = project_with("look", Some(OFF_CENTRE));
        let mut visuals = ProjectileVisuals::default();
        visuals.tags.insert(1, gib(mesh_look()));
        visuals.tags.insert(2, gib(mesh_look()));
        let mut engine = gib_engine(&root, visuals);
        let q = Quat::from_rotation_y(1.0);
        spin_shot(&mut engine, 1, [0.0; 3], q.to_array(), 5.0);
        spin_shot(&mut engine, 2, [0.0; 3], Quat::IDENTITY.to_array(), 5.0);
        let (scene, blend) = engine.extract_present_scene().unwrap();
        assert_eq!(scene.mesh_assets.len(), 1);
        let m = &scene.meshes[1];
        assert_eq!(m.shape, MeshShape::Asset(0));
        assert!(m.cast_shadows);
        let (s, r, _) = m.model.to_scale_rotation_translation();
        assert!((s - Vec3::new(0.1, 0.2, 0.3)).abs().max_element() < 1e-5);
        assert!(r.dot(q).abs() > 0.99999);
        let centre = bounds_centre(&scene.mesh_assets[0]);
        assert!(centre != Vec3::ZERO, "an off-centre asset");
        for mesh in &scene.meshes[1..] {
            let at = mesh.model.transform_point3(centre);
            assert!((at - Vec3::new(1.0, 2.0, 0.0)).length() < 1e-5, "{at:?}");
        }
        assert_eq!(scene.meshes[2].shape, MeshShape::Asset(0));
        assert!(
            blend
                .projectiles
                .iter()
                .all(|(_, item)| matches!(item, ProjectileItem::Posed(_)))
        );
        assert_eq!(blend.meshes.len(), scene.meshes.len());
    }

    #[test]
    fn a_spun_cube_draws_its_orientation_and_an_unspun_one_faces_its_heading() {
        let mut visuals = ProjectileVisuals::default();
        visuals.tags.insert(1, gib(ProjectileLook::Cube));
        let mut engine = engine(true, visuals);
        let q = Quat::from_rotation_x(0.8);
        spin_shot(&mut engine, 1, [0.0, 2.0, 0.0], q.to_array(), 5.0);
        fire(&mut engine, [100.0, 0.0, 0.0], 1);
        let (scene, blend) = engine.extract_present_scene().unwrap();
        let (s, r, _) = scene.meshes[1].model.to_scale_rotation_translation();
        assert!((s - Vec3::new(0.1, 0.2, 0.3)).abs().max_element() < 1e-5);
        assert!(r.dot(q).abs() > 0.99999);
        assert!(matches!(blend.projectiles[0].1, ProjectileItem::Posed(1)));
        let (s, r, _) = scene.meshes[2].model.to_scale_rotation_translation();
        assert!(r.dot(Quat::from_rotation_arc(Vec3::Z, Vec3::X)).abs() > 0.99999);
        assert!(
            (s.z - 3.0).abs() < 1e-5,
            "heading stretch keeps length * scale.z"
        );
        assert!(matches!(blend.projectiles[1].1, ProjectileItem::Mesh(2)));
    }

    #[test]
    fn fade_out_shrinks_the_last_seconds() {
        let mut visuals = ProjectileVisuals::default();
        visuals.tags.insert(
            1,
            ProjectileVisual {
                fade_out: 2.0,
                ..visual(ProjectileLook::Cube)
            },
        );
        let mut engine = engine(true, visuals);
        spin_shot(&mut engine, 1, [0.0, 1.0, 0.0], [0.0, 0.0, 0.0, 1.0], 1.0);
        spin_shot(&mut engine, 1, [0.0, 1.0, 0.0], [0.0, 0.0, 0.0, 1.0], 5.0);
        let (scene, _) = engine.extract_present_scene().unwrap();
        assert!((scene.meshes[1].model.x_axis.length() - 0.05).abs() < 1e-5);
        assert!((scene.meshes[2].model.x_axis.length() - 0.1).abs() < 1e-5);
    }

    #[test]
    fn the_new_fields_are_validated_with_their_pointers() {
        let at = |v: ProjectileVisual| {
            let mut visuals = ProjectileVisuals::default();
            visuals.tags.insert(4, v);
            visuals.validate().unwrap_err()
        };
        for bad in [0.0, f32::NAN, -1.0] {
            let e = at(ProjectileVisual {
                scale: [1.0, bad, 1.0],
                ..ProjectileVisual::default()
            });
            assert_eq!(e.code, code::PROJECTILE_VISUALS_INVALID);
            assert_eq!(e.path.as_deref(), Some("/tags/4/scale/1"));
        }
        let e = at(ProjectileVisual {
            fade_out: -0.5,
            ..ProjectileVisual::default()
        });
        assert_eq!(e.path.as_deref(), Some("/tags/4/fade_out"));
        let e = at(ProjectileVisual {
            look: ProjectileLook::Mesh {
                asset: String::new(),
            },
            ..ProjectileVisual::default()
        });
        assert_eq!(e.path.as_deref(), Some("/tags/4/look/asset"));
    }

    #[test]
    fn a_missing_mesh_file_is_refused_at_build_and_a_broken_one_degrades() {
        let root = project_with("missing", None);
        let mut visuals = ProjectileVisuals::default();
        visuals.tags.insert(7, gib(mesh_look()));
        let error = Engine::builder()
            .project_root(&root)
            .projectile_visuals(visuals.clone())
            .build()
            .err()
            .expect("missing asset is refused");
        assert_eq!(error.code, code::PROJECTILE_LOOK_ASSET);
        assert_eq!(error.path.as_deref(), Some("/tags/7/look/asset"));

        let root = project_with("broken", Some("format: 1\nkind: mesh\npositions: nope\n"));
        let mut engine = gib_engine(&root, visuals);
        spin_shot(&mut engine, 7, [0.0, 1.0, 0.0], [0.0, 0.0, 0.0, 1.0], 5.0);
        for _ in 0..2 {
            let (scene, blend) = engine.extract_present_scene().unwrap();
            assert_eq!(scene.meshes.len(), 1, "nothing drawn for the broken tag");
            assert!(blend.projectiles.is_empty());
        }
        let sink = engine.world().resource::<crate::Diagnostics>().unwrap();
        assert_eq!(sink.reported(code::PROJECTILE_LOOK_ASSET).count(), 1);
        let pending: Vec<_> = sink
            .pending()
            .iter()
            .filter(|d| d.code == code::PROJECTILE_LOOK_ASSET)
            .collect();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].path.as_deref(), Some("/tags/7/look/asset"));
    }

    #[test]
    fn a_voxel_look_pivots_on_its_box_centre_and_shares_its_model() {
        let root = project_with("voxel", Some(CRYSTAL));
        let mut visuals = ProjectileVisuals::default();
        visuals.tags.insert(1, gib(voxel_look()));
        visuals.tags.insert(2, gib(voxel_look()));
        let mut engine = gib_engine(&root, visuals);
        let q = Quat::from_rotation_y(1.0);
        spin_shot(&mut engine, 1, [0.0; 3], q.to_array(), 5.0);
        spin_shot(&mut engine, 2, [0.0; 3], Quat::IDENTITY.to_array(), 5.0);
        let (scene, blend) = engine.extract_present_scene().unwrap();
        assert_eq!(scene.mesh_assets.len(), 1, "one upload for two tags");
        let data = &scene.mesh_assets[0];
        assert!(data.authored_material);
        let centre = bounds_centre(data);
        assert!(
            centre != Vec3::ZERO,
            "the mesher origin is not the box centre"
        );
        let m = &scene.meshes[1];
        assert_eq!(m.shape, MeshShape::Asset(0));
        assert!(m.use_vertex_material && m.cast_shadows);
        let (s, r, _) = m.model.to_scale_rotation_translation();
        assert!((s - Vec3::new(0.1, 0.2, 0.3)).abs().max_element() < 1e-5);
        assert!(r.dot(q).abs() > 0.99999);
        for mesh in &scene.meshes[1..] {
            let at = mesh.model.transform_point3(centre);
            assert!((at - Vec3::new(1.0, 2.0, 0.0)).length() < 1e-5, "{at:?}");
        }
        assert!(
            blend
                .projectiles
                .iter()
                .all(|(_, item)| matches!(item, ProjectileItem::Posed(_)))
        );
    }

    #[test]
    fn a_voxel_look_fades_out_and_refuses_bad_models() {
        let root = project_with("voxel-fade", None);
        let mut visuals = ProjectileVisuals::default();
        visuals.tags.insert(
            1,
            ProjectileVisual {
                fade_out: 2.0,
                ..visual(voxel_look())
            },
        );
        let mut engine = gib_engine(&root, visuals);
        spin_shot(&mut engine, 1, [0.0; 3], [0.0, 0.0, 0.0, 1.0], 1.0);
        let (scene, _) = engine.extract_present_scene().unwrap();
        assert!((scene.meshes[1].model.x_axis.length() - 0.05).abs() < 1e-5);

        let mut empty = ProjectileVisuals::default();
        empty.tags.insert(
            4,
            ProjectileVisual {
                look: ProjectileLook::Voxel {
                    model: String::new(),
                },
                ..ProjectileVisual::default()
            },
        );
        let e = empty.validate().unwrap_err();
        assert_eq!(e.code, code::PROJECTILE_VISUALS_INVALID);
        assert_eq!(e.path.as_deref(), Some("/tags/4/look/model"));

        let mut missing = ProjectileVisuals::default();
        missing.tags.insert(
            7,
            ProjectileVisual {
                look: ProjectileLook::Voxel {
                    model: "voxels/nope.voxel.yaml".into(),
                },
                ..ProjectileVisual::default()
            },
        );
        let error = Engine::builder()
            .project_root(&root)
            .projectile_visuals(missing)
            .build()
            .err()
            .expect("missing model is refused");
        assert_eq!(error.code, code::PROJECTILE_LOOK_ASSET);
        assert_eq!(error.path.as_deref(), Some("/tags/7/look/model"));

        std::fs::write(
            root.join("voxels/gib.voxel.yaml"),
            "format: 1\nkind: voxel\nsize: nope\n",
        )
        .unwrap();
        let mut broken = ProjectileVisuals::default();
        broken.tags.insert(7, gib(voxel_look()));
        let mut engine = gib_engine(&root, broken);
        spin_shot(&mut engine, 7, [0.0, 1.0, 0.0], [0.0, 0.0, 0.0, 1.0], 5.0);
        for _ in 0..2 {
            let (scene, blend) = engine.extract_present_scene().unwrap();
            assert_eq!(scene.meshes.len(), 1, "nothing drawn for the broken tag");
            assert!(blend.projectiles.is_empty());
        }
        let sink = engine.world().resource::<crate::Diagnostics>().unwrap();
        assert_eq!(sink.reported(code::PROJECTILE_LOOK_ASSET).count(), 1);
        let pending: Vec<_> = sink
            .pending()
            .iter()
            .filter(|d| d.code == code::PROJECTILE_LOOK_ASSET)
            .collect();
        assert_eq!(pending[0].path.as_deref(), Some("/tags/7/look/model"));
    }
}
