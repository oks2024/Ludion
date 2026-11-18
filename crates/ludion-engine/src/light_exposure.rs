//! Sim-side light exposure: how much authored light reaches a set of points, answered from sim
//! state with the renderer's own falloff, cone, clamping and sky-irradiance formulas
//! (`ludion_renderer::light_model`, pinned to the WGSL by a drift test). No rendered pixel is read.
//!
//! Counted lights are the sim entities' `PointLight`, `SpotLight` and the lowest-`EntityId`
//! `DirectionalLight` (the sun, which ignores `Visibility` as the renderer does); a hidden light, or
//! one under a hidden `Parent`, is absent, and so is a point or spot light with no `Transform`.
//! Occlusion is opt-in per light (`cast_shadows`, and always for the sun) and uses physics rays
//! through the caller's filter, as given. See `ludion docs light-exposure`.

use std::fmt;

use ludion_core::codes::code;
use ludion_core::diagnostics::Diagnostic;
use ludion_core::ids::EntityId;
use ludion_core::math::glam::Vec3;
use ludion_ecs::World;
use ludion_physics::{PhysicsError, PhysicsWorld, QueryFilter};
use ludion_renderer::DEFAULT_AMBIENT;
use ludion_renderer::light_model::{
    LocalLight, atten, local_light, sky_irradiance, sky_sphere_mean, spot_cone, spot_cosines,
};

use crate::components::{DirectionalLight, Environment, PointLight, SpotLight, Visibility};
use crate::extract::local_shadow;
use crate::hierarchy::{inherited_visibility, world_transform};

/// The most points one query may carry.
pub const MAX_EXPOSURE_POINTS: usize = 16;

/// The surface orientation a point is lit as.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub enum Facing {
    /// `n·l = 1` for every light: the light a body of unknown orientation could receive at most.
    #[default]
    Omni,
    /// A surface with this normal (any non-zero length): `max(n·l, 0)`.
    Normal([f32; 3]),
}

/// How several points collapse into one reading.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Combine {
    /// The point with the largest `total`, whole; ties go to the first in authored order.
    #[default]
    Max,
    /// The componentwise mean of the per-point readings.
    Mean,
}

/// One query: where, how the surface faces, what may occlude, how to combine.
#[derive(Debug, Clone, Copy)]
pub struct ExposureQuery<'a> {
    /// 1 to [`MAX_EXPOSURE_POINTS`] finite world-space points.
    pub points: &'a [[f32; 3]],
    /// The surface orientation at every point.
    pub facing: Facing,
    /// The filter every shadow ray runs through, as given: nothing is added implicitly.
    pub occluders: &'a QueryFilter,
    /// How the points collapse.
    pub combine: Combine,
}

/// Which kind of light a [`LightContribution`] came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LightKind {
    /// The sun.
    Directional,
    /// A `PointLight`.
    Point,
    /// A `SpotLight`.
    Spot,
}

/// One light's share of a reading.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LightContribution {
    /// The light's entity.
    pub light: EntityId,
    /// Its kind.
    pub kind: LightKind,
    /// Rec.709 luminance of what it adds after occlusion.
    pub luminance: f32,
    /// The same without occlusion.
    pub unoccluded: f32,
    /// A shadow ray blocked it (at every point it reached, under [`Combine::Mean`]).
    pub occluded: bool,
}

/// The answer: linear light, as the renderer would sum it onto a white diffuse surface.
#[derive(Debug, Clone, PartialEq)]
pub struct Exposure {
    /// Rec.709 luminance of `rgb`.
    pub total: f32,
    /// Ambient plus every light's contribution, linear.
    pub rgb: [f32; 3],
    /// The ambient term alone.
    pub ambient: [f32; 3],
    /// Every light that reached a point (`unoccluded > 0`), ascending `EntityId`.
    pub lights: Vec<LightContribution>,
}

/// Why a query failed.
#[derive(Debug)]
pub enum ExposureError {
    /// The query is malformed (`L4250`).
    Invalid(Diagnostic),
    /// A shadow ray's filter was refused, e.g. an undeclared layer.
    Query(PhysicsError),
}

impl fmt::Display for ExposureError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Invalid(d) => write!(f, "{}", d.message),
            Self::Query(e) => write!(f, "light exposure occlusion query failed: {e}"),
        }
    }
}

impl std::error::Error for ExposureError {}

impl From<PhysicsError> for ExposureError {
    fn from(e: PhysicsError) -> Self {
        Self::Query(e)
    }
}

/// A light's finished shadow ray: bias to step off the surface and the length to cast.
#[derive(Clone, Copy)]
struct Ray {
    bias: f32,
    /// For a local light, `bias + near` is subtracted from the distance per point.
    near: f32,
    far: f32,
}

struct Lamp {
    id: EntityId,
    kind: LightKind,
    pos: Vec3,
    color: Vec3,
    range: f32,
    cone: Option<(Vec3, f32, f32)>,
    shadow: Option<Ray>,
    /// The sun: unit surface-to-light vector, no attenuation.
    sun_l: Option<Vec3>,
}

fn luminance(c: Vec3) -> f32 {
    0.2126 * c.x + 0.7152 * c.y + 0.0722 * c.z
}

fn invalid(message: String, path: String) -> ExposureError {
    ExposureError::Invalid(
        Diagnostic::new(code::LIGHT_EXPOSURE_INVALID, message)
            .with_path(path)
            .with_help("see `ludion docs light-exposure`"),
    )
}

fn validate(query: &ExposureQuery) -> Result<Option<Vec3>, ExposureError> {
    let n = query.points.len();
    if n == 0 || n > MAX_EXPOSURE_POINTS {
        return Err(invalid(
            format!("a light exposure query takes 1 to {MAX_EXPOSURE_POINTS} points, got {n}"),
            "/points".into(),
        ));
    }
    if let Some(i) = query
        .points
        .iter()
        .position(|p| !p.iter().all(|c| c.is_finite()))
    {
        return Err(invalid(
            format!("point {i} has a non-finite coordinate"),
            format!("/points/{i}"),
        ));
    }
    match query.facing {
        Facing::Omni => Ok(None),
        Facing::Normal(v) => Vec3::from(v).try_normalize().map(Some).ok_or_else(|| {
            invalid(
                "a Normal facing must be non-zero and finite".into(),
                "/facing".into(),
            )
        }),
    }
}

/// Collects the counted lamps: points, then spots, each ascending by `EntityId`, then the sun.
fn gather(world: &World) -> Vec<Lamp> {
    let (mut points, mut spots, mut sun) = (Vec::new(), Vec::new(), None);
    for id in world.entities() {
        if sun.is_none()
            && let Some(l) = world.get::<DirectionalLight>(id).ok().flatten()
        {
            sun = Some((id, l));
        }
        let point = world.get::<PointLight>(id).ok().flatten();
        let spot = world.get::<SpotLight>(id).ok().flatten();
        if point.is_none() && spot.is_none() {
            continue;
        }
        if !inherited_visibility(world, id).visible {
            continue;
        }
        let Some(pose) = world_transform(world, id) else {
            continue;
        };
        let pos = Vec3::from(pose.trs.translation);
        if let Some(l) = point {
            let Some(base) = local_light(l.color, l.intensity, l.range) else {
                continue;
            };
            let shadow = local_ray(l.cast_shadows, l.shadow_bias, l.shadow_near, l.range);
            points.push(lamp(id, LightKind::Point, pos, base, None, shadow));
        }
        if let Some(l) = spot {
            let Some(base) = local_light(l.color, l.intensity, l.range) else {
                continue;
            };
            let Some(axis) = Vec3::from(l.direction).try_normalize() else {
                continue;
            };
            let (ci, co) = spot_cosines(l.inner_angle, l.outer_angle);
            let shadow = local_ray(l.cast_shadows, l.shadow_bias, l.shadow_near, l.range);
            spots.push(lamp(
                id,
                LightKind::Spot,
                pos,
                base,
                Some((axis, ci, co)),
                shadow,
            ));
        }
    }
    points.extend(spots);
    if let Some((id, l)) = sun {
        points.push(sun_lamp(id, l));
    }
    points
}

fn local_ray(cast: bool, bias: f32, near: f32, range: f32) -> Option<Ray> {
    local_shadow(cast, bias, near, range, None, 0.0).map(|s| Ray {
        bias: s.bias,
        near: s.near,
        far: 0.0,
    })
}

fn lamp(
    id: EntityId,
    kind: LightKind,
    pos: Vec3,
    base: LocalLight,
    cone: Option<(Vec3, f32, f32)>,
    shadow: Option<Ray>,
) -> Lamp {
    Lamp {
        id,
        kind,
        pos,
        color: base.color * base.intensity,
        range: base.range,
        cone,
        shadow,
        sun_l: None,
    }
}

fn sun_lamp(id: EntityId, sun: &DirectionalLight) -> Lamp {
    let defaults = DirectionalLight::default();
    let dir = Vec3::from(sun.direction)
        .try_normalize()
        .unwrap_or_else(|| Vec3::from(defaults.direction).normalize());
    let far = if sun.shadow_near.is_finite()
        && sun.shadow_far.is_finite()
        && sun.shadow_near < sun.shadow_far
    {
        sun.shadow_far
    } else {
        defaults.shadow_far
    };
    Lamp {
        id,
        kind: LightKind::Directional,
        pos: Vec3::ZERO,
        color: (Vec3::from(sun.color) * sun.intensity).max(Vec3::ZERO),
        range: 0.0,
        cone: None,
        shadow: Some(Ray {
            bias: sun.shadow_bias.max(0.0),
            near: 0.0,
            far,
        }),
        sun_l: Some(-dir),
    }
}

fn ambient(world: &World, facing: Option<Vec3>) -> Vec3 {
    let env = world
        .entities()
        .find_map(|id| world.get::<Environment>(id).ok().flatten());
    let Some(env) = env else {
        return Vec3::from(DEFAULT_AMBIENT);
    };
    let (z, h, g) = (
        Vec3::from(env.sky_zenith),
        Vec3::from(env.sky_horizon),
        Vec3::from(env.sky_ground),
    );
    let e = match facing {
        Some(n) => sky_irradiance(n, z, h, g),
        None => sky_sphere_mean(z, h, g),
    };
    Vec3::from(env.ambient) + env.ibl_intensity * e
}

/// One light at one point.
#[derive(Clone, Copy, Default)]
struct Share {
    reached: bool,
    blocked: bool,
    luminance: f32,
    unoccluded: f32,
}

struct PointReading {
    rgb: Vec3,
    shares: Vec<Share>,
}

/// Whether a physics ray from `origin` along `dir` for `length` is stopped by an occluder that
/// casts a shadow (a hit whose inherited `Visibility` hides it or turns `cast_shadows` off is
/// skipped).
fn blocked(
    world: &World,
    physics: Option<&PhysicsWorld>,
    filter: &QueryFilter,
    origin: Vec3,
    dir: Vec3,
    length: f32,
) -> Result<bool, ExposureError> {
    let Some(physics) = physics else {
        return Ok(false);
    };
    if length.is_nan() || length <= 0.0 {
        return Ok(false);
    }
    let hits = physics.ray_cast(origin.to_array(), dir.to_array(), length, filter)?;
    Ok(hits.iter().any(|hit| {
        let v: Visibility = inherited_visibility(world, hit.entity);
        v.visible && v.cast_shadows
    }))
}

fn read_point(
    world: &World,
    physics: Option<&PhysicsWorld>,
    lamps: &[Lamp],
    ambient: Vec3,
    p: Vec3,
    n: Option<Vec3>,
    filter: &QueryFilter,
) -> Result<PointReading, ExposureError> {
    let mut rgb = ambient;
    let mut shares = vec![Share::default(); lamps.len()];
    for (lamp, share) in lamps.iter().zip(&mut shares) {
        let (l, dist, falloff) = match lamp.sun_l {
            Some(l) => (l, 0.0, 1.0),
            None => {
                let to_light = lamp.pos - p;
                let dist = to_light.length();
                let l = if dist > 0.0 {
                    to_light / dist
                } else {
                    Vec3::ZERO
                };
                let a = atten(dist, lamp.range);
                (l, dist, a * a)
            }
        };
        let cone = lamp
            .cone
            .map_or(1.0, |(axis, ci, co)| spot_cone((-l).dot(axis), ci, co));
        let ndl = n.map_or(1.0, |n| n.dot(l).max(0.0));
        let base = lamp.color * cone * ndl * falloff;
        let reached = base.max_element() > 0.0;
        let mut vis = 1.0;
        if reached && let Some(ray) = lamp.shadow {
            let length = if lamp.sun_l.is_some() {
                ray.far - ray.bias
            } else {
                dist - ray.bias - ray.near
            };
            if blocked(world, physics, filter, p + l * ray.bias, l, length)? {
                vis = 0.0;
            }
        }
        let lit = base * vis;
        rgb += lit;
        *share = Share {
            reached,
            blocked: reached && vis == 0.0,
            luminance: luminance(lit),
            unoccluded: luminance(base),
        };
    }
    Ok(PointReading { rgb, shares })
}

/// How much authored light reaches the query's points, from sim state.
///
/// ```
/// use ludion_ecs::World;
/// use ludion_engine::{ExposureQuery, Facing, Combine, light_exposure};
/// use ludion_engine::components::{PointLight, Transform};
/// use ludion_physics::QueryFilter;
///
/// let mut world = World::new();
/// let lamp = world.spawn();
/// world.insert(lamp, Transform { translation: [0.0, 2.0, 0.0], ..Transform::default() }).unwrap();
/// world.insert(lamp, PointLight { range: 4.0, ..PointLight::default() }).unwrap();
///
/// let filter = QueryFilter::new();
/// let query = ExposureQuery {
///     points: &[[0.0, 0.0, 0.0]],
///     facing: Facing::Normal([0.0, 1.0, 0.0]),
///     occluders: &filter,
///     combine: Combine::Max,
/// };
/// let exposure = light_exposure(&world, &query).unwrap();
/// assert_eq!(exposure.lights.len(), 1);
/// assert!(exposure.total > 0.0);
/// ```
pub fn light_exposure(world: &World, query: &ExposureQuery) -> Result<Exposure, ExposureError> {
    let n = validate(query)?;
    let lamps = gather(world);
    let ambient = ambient(world, n);
    let physics = world.resource::<PhysicsWorld>();
    let readings = query
        .points
        .iter()
        .map(|&p| {
            read_point(
                world,
                physics,
                &lamps,
                ambient,
                Vec3::from(p),
                n,
                query.occluders,
            )
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok(combine(&lamps, &readings, ambient, query.combine))
}

fn combine(lamps: &[Lamp], readings: &[PointReading], ambient: Vec3, how: Combine) -> Exposure {
    let (rgb, shares): (Vec3, Vec<Share>) = match how {
        Combine::Max => {
            let mut best = &readings[0];
            for r in &readings[1..] {
                if luminance(r.rgb) > luminance(best.rgb) {
                    best = r;
                }
            }
            (best.rgb, best.shares.clone())
        }
        Combine::Mean => {
            let count = readings.len() as f32;
            let rgb = readings.iter().fold(Vec3::ZERO, |a, r| a + r.rgb) / count;
            let shares = (0..lamps.len())
                .map(|i| {
                    let (mut lum, mut un) = (0.0, 0.0);
                    let (mut reached, mut all_blocked) = (false, true);
                    for r in readings {
                        let s = r.shares[i];
                        lum += s.luminance;
                        un += s.unoccluded;
                        if s.reached {
                            reached = true;
                            all_blocked &= s.blocked;
                        }
                    }
                    Share {
                        reached,
                        blocked: reached && all_blocked,
                        luminance: lum / count,
                        unoccluded: un / count,
                    }
                })
                .collect();
            (rgb, shares)
        }
    };
    let mut lights: Vec<LightContribution> = lamps
        .iter()
        .zip(&shares)
        .filter(|(_, s)| s.unoccluded > 0.0)
        .map(|(lamp, s)| LightContribution {
            light: lamp.id,
            kind: lamp.kind,
            luminance: s.luminance,
            unoccluded: s.unoccluded,
            occluded: s.blocked,
        })
        .collect();
    lights.sort_by_key(|l| l.light);
    Exposure {
        total: luminance(rgb),
        rgb: rgb.to_array(),
        ambient: ambient.to_array(),
        lights,
    }
}
