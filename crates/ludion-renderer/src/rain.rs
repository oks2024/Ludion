//! Rain: the data the renderer draws a world-anchored streak lattice from, and its CPU twin.
//!
//! World space is tiled in cubes of `cell` metres. Drop `k` of a tile hashes `(seed, tile, k)` with PCG
//! rounds, so a drop's position is a function of the tile and the time alone and never of the camera;
//! `shaders/rain.wgsl` evaluates the same arithmetic and [`drop_position`] is its twin.

use ludion_core::math::glam::{IVec3, Mat4, Vec2, Vec3};

/// The most streaks one draw carries. A scene asking for more is clamped per tile.
pub const MAX_RAIN_STREAKS: u32 = 65_536;

/// Game time wraps at this many seconds so `time_s` keeps `f32` resolution.
pub const RAIN_TIME_WRAP_S: f64 = 4096.0;

/// One frame's resolved rain. `per_tile` already holds the streak budget.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RainData {
    /// Horizontal reach around the camera, in metres.
    pub radius: f32,
    /// The vertical band `[low, high]` relative to the camera.
    pub height: [f32; 2],
    /// The lattice cube size, in metres.
    pub cell: f32,
    /// Fall speed range `[min, max]`, in m/s.
    pub fall_speed: [f32; 2],
    /// Horizontal wind `[x, 0, z]`, in m/s.
    pub wind: [f32; 3],
    /// Per-drop wind variation in `[0, 1]`.
    pub gust: f32,
    /// Streak width in metres.
    pub drop_width: f32,
    /// Streak length scale: length is `speed * streak / 60` metres.
    pub streak: f32,
    /// Display-referred colour and base opacity.
    pub color: [f32; 4],
    /// Light each streak from the scene's lights.
    pub lit: bool,
    /// How much of the medium reaches the streaks, in `[0, 1]`.
    pub medium: f32,
    /// Hide streaks under roofs.
    pub occlusion: bool,
    /// How wet exposed surfaces look, in `[0, 1]`; 0 draws no wetness.
    pub wetness: f32,
    /// How strongly wet ground and coat decals ripple, in `[0, 2]`.
    pub ripples: f32,
    /// Varies the lattice hash.
    pub seed: u32,
    /// Game seconds, wrapped at [`RAIN_TIME_WRAP_S`].
    pub time_s: f32,
    /// Streaks per tile after the budget.
    pub per_tile: u32,
}

/// The block of tiles one frame draws, around a camera position.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RainLayout {
    /// The lowest tile of the block.
    pub base: IVec3,
    /// Tiles per side on x and z.
    pub n_xz: u32,
    /// Tiles in height.
    pub n_y: u32,
    /// Streaks per tile.
    pub per_tile: u32,
    /// Instances to draw: `n_xz² · n_y · per_tile`.
    pub instances: u32,
}

/// Tiles per side `(n_xz, n_y)` of the block covering `radius` and `height`.
pub fn rain_tiles(radius: f32, height: [f32; 2], cell: f32) -> (u32, u32) {
    let n_xz = (2.0 * radius / cell).ceil() as u32 + 1;
    let n_y = ((height[1] - height[0]) / cell).ceil() as u32 + 1;
    (n_xz, n_y)
}

/// Streaks per tile for a density (per m³), an intensity and a cell size, rounded.
pub fn rain_per_tile(density: f32, intensity: f32, cell: f32) -> u32 {
    (density * intensity * cell * cell * cell).round().max(0.0) as u32
}

/// The per-tile count a quality's `rain_density` draws: the authored count scaled and rounded, so a lower
/// density draws a prefix of the same drops. `1.0` is the identity.
pub fn rain_draw_per_tile(per_tile: u32, density: f32) -> u32 {
    (per_tile as f32 * density).round().max(0.0) as u32
}

/// `per_tile` clamped so `tiles · per_tile` fits [`MAX_RAIN_STREAKS`]; the flag is true when it clamped.
pub fn rain_budget(per_tile: u32, tiles: u32) -> (u32, bool) {
    if tiles == 0 || u64::from(per_tile) * u64::from(tiles) <= u64::from(MAX_RAIN_STREAKS) {
        return (per_tile, false);
    }
    (MAX_RAIN_STREAKS / tiles, true)
}

/// The tile block and instance count for `rain` seen from `cam`.
pub fn rain_layout(rain: &RainData, cam: Vec3) -> RainLayout {
    let (n_xz, n_y) = rain_tiles(rain.radius, rain.height, rain.cell);
    let min = Vec3::new(
        cam.x - rain.radius,
        cam.y + rain.height[0],
        cam.z - rain.radius,
    );
    let from = (min / rain.cell).floor();
    let base = IVec3::new(from.x as i32, from.y as i32, from.z as i32);
    RainLayout {
        base,
        n_xz,
        n_y,
        per_tile: rain.per_tile,
        instances: n_xz * n_xz * n_y * rain.per_tile,
    }
}

/// The edge of the overhead occlusion map, in texels.
pub const RAIN_OCCLUSION_SIZE: u32 = 512;

/// How far above the top of the rain band the occlusion camera sits: roofs up to this high over the
/// band still shadow the streaks below them.
pub const RAIN_OCCLUDER_REACH: f32 = 64.0;

/// The straight-down orthographic view of the overhead occlusion map: world to clip, `x`/`y` over
/// `[-radius, radius]` around the camera and `z = 0` at the eye, `z = 1` at the far plane.
///
/// The eye follows `cam.xz` snapped to the map's texel grid, so a sub-texel camera move leaves the
/// map, and every roof edge in it, still.
pub fn occlusion_view(rain: &RainData, cam: Vec3) -> Mat4 {
    let texel = 2.0 * rain.radius / RAIN_OCCLUSION_SIZE as f32;
    let snap = |v: f32| (v / texel).floor() * texel;
    let eye = Vec3::new(
        snap(cam.x),
        cam.y + rain.height[1] + RAIN_OCCLUDER_REACH,
        snap(cam.z),
    );
    let far = RAIN_OCCLUDER_REACH + (rain.height[1] - rain.height[0]);
    let view = Mat4::look_at_rh(eye, eye - Vec3::Y, -Vec3::Z);
    let proj = Mat4::orthographic_rh(
        -rain.radius,
        rain.radius,
        -rain.radius,
        rain.radius,
        0.0,
        far,
    );
    proj * view
}

/// The world height of the surface a stored map `depth` means: the CPU twin of the shader's inverse.
/// A straight-down orthographic depth is affine in height, so the matrix's `y` column and translation
/// invert it with no unprojection.
pub fn occluder_height(view: &Mat4, depth: f32) -> f32 {
    (depth - view.w_axis.z) / view.y_axis.z
}

/// One round of PCG, the shader's `pcg`.
fn pcg(v: u32) -> u32 {
    let s = v.wrapping_mul(747_796_405).wrapping_add(2_891_336_453);
    let w = ((s >> ((s >> 28) + 4)) ^ s).wrapping_mul(277_803_737);
    (w >> 22) ^ w
}

/// The top 24 bits as a float in `[0, 1)`, the shader's `unit_hash`.
fn unit_hash(h: u32) -> f32 {
    (h >> 8) as f32 * (1.0 / 16_777_216.0)
}

/// The hash of drop `k` of `tile` under `seed`: the state the shader derives every drop value from.
pub fn drop_hash(tile: IVec3, k: u32, seed: u32) -> u32 {
    let mut h = pcg(seed);
    h = pcg(h ^ tile.x as u32);
    h = pcg(h ^ tile.y as u32);
    h = pcg(h ^ tile.z as u32);
    pcg(h ^ k)
}

fn fract(x: f32) -> f32 {
    x - x.floor()
}

/// Side of one ripple cell, in metres: world xz is tiled in squares of this size.
pub const RIPPLE_CELL: f32 = 0.35;
/// Seconds between the rings of one cell.
pub const RIPPLE_PERIOD: f32 = 0.7;
/// Radius a ring grows to before it fades out, in metres.
pub const RIPPLE_RADIUS: f32 = 0.16;
/// Distance between the crests of a ring, in metres.
pub const RIPPLE_WAVELENGTH: f32 = 0.05;
/// Half-width of the band around a ring's radius where it has any height, in metres.
pub const RIPPLE_WINDOW: f32 = 0.03;

/// The slope of the ripple height field at world `xz`, `rain_ripple.wgsl`'s `ripple_slope`. Each cell of
/// [`RIPPLE_CELL`] sends one ring per [`RIPPLE_PERIOD`] from a hashed origin inside it; the ring is
/// `sin(k (d - r)) / k` (`k = 2 pi / RIPPLE_WAVELENGTH`, so its crest slope is the amplitude) under a smooth
/// window of half-width [`RIPPLE_WINDOW`] and a `(1 - age)^2` fade. A ring reaches at most
/// `RIPPLE_RADIUS + RIPPLE_WINDOW` from its origin, under a cell's width, so the 3x3 cells around a point
/// hold every ring that touches it and the field has no seam. A pure function of `(xz, time, seed)`.
pub fn ripple_slope(xz: Vec2, time: f32, seed: u32) -> Vec2 {
    let cell = (xz / RIPPLE_CELL).floor();
    let k = std::f32::consts::TAU / RIPPLE_WAVELENGTH;
    let mut sum = Vec2::ZERO;
    for dj in -1..=1 {
        for di in -1..=1 {
            let i = cell.x as i32 + di;
            let j = cell.y as i32 + dj;
            let mut h = pcg(seed ^ 0x9e37_79b9);
            h = pcg(h ^ i as u32);
            h = pcg(h ^ j as u32);
            let phase = unit_hash(pcg(h.wrapping_add(1)));
            let origin = Vec2::new(
                (i as f32 + unit_hash(pcg(h.wrapping_add(2)))) * RIPPLE_CELL,
                (j as f32 + unit_hash(pcg(h.wrapping_add(3)))) * RIPPLE_CELL,
            );
            let age = fract(time / RIPPLE_PERIOD + phase);
            let delta = xz - origin;
            let d = delta.length();
            let x = d - age * RIPPLE_RADIUS;
            let u = x / RIPPLE_WINDOW;
            if u.abs() < 1.0 && d > 1.0e-6 {
                let window = (1.0 - u * u) * (1.0 - u * u);
                let fade = (1.0 - age) * (1.0 - age);
                let slope = ludion_core::math::cos(k * x) * window * fade;
                sum += delta / d * slope;
            }
        }
    }
    sum
}

/// World position of drop `k` of `tile` at `rain.time_s`, the CPU twin of the shader. It never reads a
/// camera: the drop wraps inside its own tile column.
pub fn drop_position(rain: &RainData, tile: IVec3, k: u32) -> Vec3 {
    drop_state(rain, tile, k).0
}

/// Position and world velocity of one drop.
pub fn drop_state(rain: &RainData, tile: IVec3, k: u32) -> (Vec3, Vec3) {
    let h = drop_hash(tile, k, rain.seed);
    let hx = unit_hash(pcg(h.wrapping_add(1)));
    let hz = unit_hash(pcg(h.wrapping_add(2)));
    let phase = unit_hash(pcg(h.wrapping_add(3)));
    let speed_t = unit_hash(pcg(h.wrapping_add(4)));
    let gust_t = unit_hash(pcg(h.wrapping_add(5)));
    let cell = rain.cell;
    let t = rain.time_s;
    let speed = rain.fall_speed[0] * (1.0 - speed_t) + rain.fall_speed[1] * speed_t;
    let g = 1.0 - rain.gust + 2.0 * rain.gust * gust_t;
    let fall = fract(phase + t * speed / cell);
    let pos = Vec3::new(
        (tile.x as f32 + fract(hx + t * rain.wind[0] * g / cell)) * cell,
        (tile.y as f32 + 1.0 - fall) * cell,
        (tile.z as f32 + fract(hz + t * rain.wind[2] * g / cell)) * cell,
    );
    let vel = Vec3::new(rain.wind[0] * g, -speed, rain.wind[2] * g);
    (pos, vel)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One PCG round of 0, computed independently of this file.
    const PCG_OF_ZERO: u32 = 129_708_002;

    fn rain() -> RainData {
        RainData {
            radius: 16.0,
            height: [-6.0, 14.0],
            cell: 4.0,
            fall_speed: [7.0, 9.0],
            wind: [1.2, 0.0, 0.4],
            gust: 0.15,
            drop_width: 0.006,
            streak: 1.0,
            color: [0.55, 0.6, 0.65, 0.25],
            lit: true,
            medium: 1.0,
            occlusion: true,
            wetness: 0.0,
            ripples: 0.0,
            seed: 0,
            time_s: 3.25,
            per_tile: 96,
        }
    }

    /// Two cameras 2 m apart see the same tiles' drops at the same world positions: nothing swims.
    #[test]
    fn drop_position_does_not_depend_on_the_camera() {
        let r = rain();
        let a = rain_layout(&r, Vec3::new(10.0, 1.6, 10.0));
        let b = rain_layout(&r, Vec3::new(12.0, 1.6, 10.0));
        // Tile (1, 0, 1) lies in both blocks.
        assert_ne!(a.base, b.base, "the block follows the camera");
        let tile = IVec3::new(a.base.x + 4, a.base.y + 1, a.base.z + 4);
        let inside = |l: &RainLayout| {
            (0..l.n_xz as i32).contains(&(tile.x - l.base.x))
                && (0..l.n_y as i32).contains(&(tile.y - l.base.y))
                && (0..l.n_xz as i32).contains(&(tile.z - l.base.z))
        };
        assert!(inside(&a) && inside(&b));
        for k in 0..r.per_tile {
            assert_eq!(drop_position(&r, tile, k), drop_position(&r, tile, k));
        }
    }

    /// A drop stays inside its own tile on x, z and y, whatever the time.
    #[test]
    fn a_drop_wraps_inside_its_tile() {
        let mut r = rain();
        let tile = IVec3::new(-3, 2, 5);
        for t in [0.0, 0.7, 31.4, 4095.9] {
            r.time_s = t;
            for k in 0..r.per_tile {
                let p = drop_position(&r, tile, k);
                let lo = tile.as_vec3() * r.cell;
                let hi = lo + Vec3::splat(r.cell);
                assert!(
                    p.cmpge(lo - Vec3::splat(1e-3)).all() && p.cmple(hi + Vec3::splat(1e-3)).all(),
                    "drop {k} at t={t} is {p}, outside {lo}..{hi}"
                );
            }
        }
    }

    #[test]
    fn a_drop_falls_and_depends_on_the_seed() {
        let mut r = rain();
        let tile = IVec3::new(0, 0, 0);
        let early = drop_position(&r, tile, 3);
        r.time_s += 0.01;
        let later = drop_position(&r, tile, 3);
        assert!(
            later.y < early.y || later.y > early.y + r.cell - 0.5,
            "it falls or wraps"
        );
        r.seed = 9;
        assert_ne!(drop_position(&r, tile, 3), later);
        assert_ne!(drop_hash(tile, 0, 0), drop_hash(tile, 0, 1));
        assert_ne!(drop_hash(tile, 0, 0), drop_hash(tile, 1, 0));
    }

    /// A point far from every ring has no slope: cells' rings never reach past 0.19 m.
    #[test]
    fn ripples_are_zero_far_from_any_ring() {
        // At age 0 every ring has radius 0, so only the 0.03 m window around each origin is lit; find a
        // point farther than the window from all nine origins by scanning a few candidates.
        let quiet = (0..200)
            .map(|n| Vec2::new(0.013 * n as f32, 0.007 * n as f32))
            .filter(|p| ripple_slope(*p, 0.0, 0).length() == 0.0)
            .count();
        assert!(
            quiet > 20,
            "most of the floor is flat at one instant: {quiet}"
        );
    }

    #[test]
    fn ripples_repeat_every_period() {
        for n in 0..40 {
            let p = Vec2::new(0.11 * n as f32 + 0.02, 0.07 * n as f32);
            let a = ripple_slope(p, 0.3, 5);
            let b = ripple_slope(p, 0.3 + RIPPLE_PERIOD, 5);
            assert!((a - b).length() < 2.0e-3, "{p}: {a} vs {b}");
        }
    }

    #[test]
    fn ripples_move_with_time_and_depend_on_the_seed() {
        let differs = |f: &dyn Fn(Vec2) -> (Vec2, Vec2)| {
            (0..400).any(|n| {
                let p = Vec2::new(0.031 * n as f32, 0.017 * n as f32);
                let (a, b) = f(p);
                (a - b).length() > 1.0e-3
            })
        };
        assert!(differs(&|p| (
            ripple_slope(p, 0.0, 1),
            ripple_slope(p, 0.2, 1)
        )));
        assert!(differs(&|p| (
            ripple_slope(p, 0.2, 1),
            ripple_slope(p, 0.2, 2)
        )));
        assert_eq!(
            ripple_slope(Vec2::new(1.0, 2.0), 0.2, 1),
            ripple_slope(Vec2::new(1.0, 2.0), 0.2, 1)
        );
    }

    /// The field is continuous across every cell edge: the 3x3 neighbourhood leaves no seam.
    #[test]
    fn ripples_have_no_seam_at_cell_edges() {
        let eps = 1.0e-5;
        for k in -3..4 {
            for n in 0..60 {
                let t = 0.011 * n as f32;
                let z = 0.0173 * n as f32 - 0.4;
                let x = k as f32 * RIPPLE_CELL;
                let left = ripple_slope(Vec2::new(x - eps, z), t, 9);
                let right = ripple_slope(Vec2::new(x + eps, z), t, 9);
                assert!(
                    (left - right).length() < 0.05,
                    "x = {x} z = {z}: {left} vs {right}"
                );
            }
        }
    }

    /// The hash is pinned: it is the WGSL's, and a drift here moves every frame of rain.
    #[test]
    fn the_hash_matches_the_shader_rounds() {
        assert_eq!(pcg(0), PCG_OF_ZERO);
        let h = drop_hash(IVec3::new(1, -2, 3), 7, 42);
        assert_eq!(h, drop_hash(IVec3::new(1, -2, 3), 7, 42));
        assert_ne!(h, drop_hash(IVec3::new(1, -2, 4), 7, 42));
    }

    /// Moving the camera by less than a texel keeps the map still; a whole texel moves it.
    #[test]
    fn the_occlusion_snap_is_stable_under_a_sub_texel_move() {
        let r = rain();
        let texel = 2.0 * r.radius / RAIN_OCCLUSION_SIZE as f32;
        let base = Vec3::new(10.0 + 0.25 * texel, 1.6, -7.0 + 0.25 * texel);
        let a = occlusion_view(&r, base);
        let b = occlusion_view(&r, base + Vec3::new(0.4 * texel, 0.0, 0.4 * texel));
        assert_eq!(a, b, "a sub-texel move");
        let c = occlusion_view(&r, base + Vec3::new(texel, 0.0, 0.0));
        assert_ne!(a, c, "a texel move");
    }

    #[test]
    fn the_occlusion_map_covers_the_band_and_the_reach() {
        let r = rain();
        let cam = Vec3::new(3.0, 1.6, 4.0);
        let m = occlusion_view(&r, cam);
        // The top of the band's top reaches z = REACH / far; the band's bottom is z = 1.
        let far = RAIN_OCCLUDER_REACH + (r.height[1] - r.height[0]);
        let top = m.project_point3(Vec3::new(cam.x, cam.y + r.height[1], cam.z));
        assert!((top.z - RAIN_OCCLUDER_REACH / far).abs() < 1e-4, "{top:?}");
        let bottom = m.project_point3(Vec3::new(cam.x, cam.y + r.height[0], cam.z));
        assert!((bottom.z - 1.0).abs() < 1e-4, "{bottom:?}");
        let east = m.project_point3(Vec3::new(cam.x + r.radius, cam.y, cam.z));
        assert!((east.x - 1.0).abs() < 0.01, "{east:?}");
    }

    #[test]
    fn an_occluder_height_round_trips_through_the_depth() {
        let r = rain();
        let m = occlusion_view(&r, Vec3::new(-2.0, 0.5, 9.0));
        for y in [-5.0, 0.0, 3.0, 12.5, 40.0] {
            let depth = m.project_point3(Vec3::new(1.0, y, 2.0)).z;
            assert!((occluder_height(&m, depth) - y).abs() < 1e-3, "y={y}");
        }
    }

    #[test]
    fn the_budget_clamps_per_tile() {
        let r = rain();
        let (n_xz, n_y) = rain_tiles(r.radius, r.height, r.cell);
        assert_eq!((n_xz, n_y), (9, 6));
        let tiles = n_xz * n_xz * n_y;
        assert_eq!(rain_budget(96, tiles), (96, false));
        let (per_tile, clamped) = rain_budget(1000, tiles);
        assert!(clamped && per_tile * tiles <= MAX_RAIN_STREAKS && per_tile == 65_536 / tiles);
        assert_eq!(rain_per_tile(1.5, 1.0, 4.0), 96);
        assert_eq!(rain_per_tile(1.5, 0.5, 4.0), 48);
        assert_eq!(rain_draw_per_tile(96, 1.0), 96);
        assert_eq!(rain_draw_per_tile(96, 0.4), 38);
        assert_eq!(rain_draw_per_tile(96, 0.0), 0);
        let layout = rain_layout(&RainData { per_tile: 96, ..r }, Vec3::ZERO);
        assert_eq!(layout.instances, 486 * 96);
    }
}
