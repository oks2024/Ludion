//! Display-time pose blending: a pure, GPU-free function that moves the pose-driven items of the
//! newest extracted [`RenderScene`] back toward the previous tick's poses.
//!
//! The previous *scene* is never kept. Extract records, per tick, a [`BlendData`]: the world model of
//! every entity that drives a render item, plus a parallel key list per `RenderScene` list. [`blend`]
//! turns two ticks' `BlendData` and the newer scene into the scene to draw at `alpha` between them.

use ludion_core::ids::EntityId;
use ludion_core::math::glam::{Mat3, Mat4, Quat};

use crate::scene::{RenderScene, SpotLightData};
use crate::viewmodel::compose_viewmodel;

/// The poses and per-item keys one extracted tick records for [`blend`].
///
/// Each `Vec<Option<EntityId>>` is parallel to the [`RenderScene`] list of the same name: the same
/// length, or empty (every item unkeyed). `Some(key)` names the entity whose world affine moved the
/// item; `None` takes the newer tick unchanged. `poses` and `snaps` are sorted ascending.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct BlendData {
    /// Bumped when the world is replaced (restore, scene load): blending across epochs is a cut.
    pub epoch: u64,
    /// The `render_pose` model of every keyed entity, ascending by id.
    pub poses: Vec<(EntityId, Mat4)>,
    /// Keys cut this tick (the entity itself or an ancestor): they take the newer pose, ascending.
    pub snaps: Vec<EntityId>,
    /// The camera's pose and projection inputs, when the scene's camera comes from an entity.
    pub camera: Option<CameraBlend>,
    /// Keys parallel to [`RenderScene::meshes`].
    pub meshes: Vec<Option<EntityId>>,
    /// Keys parallel to [`RenderScene::sprites`].
    pub sprites: Vec<Option<EntityId>>,
    /// Keys parallel to [`RenderScene::lights`].
    pub lights: Vec<Option<EntityId>>,
    /// Keys parallel to [`RenderScene::spot_lights`].
    pub spot_lights: Vec<Option<EntityId>>,
    /// Keys parallel to [`RenderScene::texts`].
    pub texts: Vec<Option<EntityId>>,
    /// Keys parallel to [`RenderScene::ground_markers`].
    pub ground_markers: Vec<Option<EntityId>>,
    /// Keys parallel to [`RenderScene::drop_shadows`].
    pub drop_shadows: Vec<Option<EntityId>>,
    /// Keys parallel to [`RenderScene::decals`].
    pub decals: Vec<Option<EntityId>>,
    /// Instance keys for the leading [`RenderScene::particles`] (the emitters'; pool billboards
    /// follow them unkeyed), ascending; empty when uncollected. An unsorted list, or one longer
    /// than the scene's particles, is treated as unkeyed.
    pub particles: Vec<InstanceKey>,
    /// Volley bullets: each key with its index into [`RenderScene::sprites`], ascending by key.
    /// The matching `sprites` key entries stay `None`.
    pub bullets: Vec<(InstanceKey, u32)>,
    /// Projectile-pool shots: each key (`index << 32 | generation`) with the scene item that draws
    /// it, ascending by key. A shot has no owner entity, so its mesh entry in `meshes` is `None`.
    pub projectiles: Vec<(u64, ProjectileItem)>,
}

/// The scene item that draws one pool projectile.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProjectileItem {
    /// An index into [`RenderScene::meshes`].
    Mesh(u32),
    /// An index into [`RenderScene::particles`].
    Particle(u32),
}

/// The identity of one particle or bullet across ticks.
///
/// The field order is the sort order, so continuous particles (`burst == false`) sort before burst
/// ones, matching emission order. A bullet uses `burst: false` and its index in the volley; a
/// particle's `index` is its stream index, negative for an endless emitter's past.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct InstanceKey {
    /// The emitting or firing entity.
    pub owner: EntityId,
    /// When the emitter or volley started, in its own units.
    pub started: u64,
    /// Whether the particle belongs to a burst stream.
    pub burst: bool,
    /// The stream or bullet index.
    pub index: i64,
}

/// What [`blend`] needs to re-derive the camera between two ticks.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CameraBlend {
    /// The camera entity.
    pub key: EntityId,
    /// The camera's world model (camera space to world).
    pub model: Mat4,
    /// Vertical field of view in degrees; `None` for a projection that is not blended.
    pub fov_y_deg: Option<f32>,
    /// Near plane distance.
    pub near: f32,
    /// Far plane distance.
    pub far: f32,
    /// Viewport aspect ratio.
    pub aspect: f32,
}

/// The scene to draw at `alpha` between the previous tick (`prev`) and the newer one (`next`,
/// described by `next_data`).
///
/// Returns `next.clone()` bit-exactly when `alpha >= 1`, when the epochs differ, or when `prev` holds
/// no poses and no instance keys. Each keyed item moves by the delta that takes its entity's newer
/// pose to the pose interpolated (scale and translation lerped, rotation slerped) between the two
/// ticks; a snapped, newly spawned, unmoved or degenerate key gets no delta. A particle or bullet
/// found in both ticks by [`InstanceKey`] is lerped between its two samples; a birth or death takes
/// the newer tick. Everything not pose-driven comes from `next`. A keyed list whose length differs
/// from its scene list is treated as unkeyed.
///
/// ```
/// use ludion_renderer::{BlendData, RenderScene, blend};
/// let scene = RenderScene::empty((64, 64));
/// let out = blend(&scene, &BlendData::default(), &scene, &BlendData::default(), 0.5);
/// assert_eq!(out, scene);
/// ```
pub fn blend(
    prev_scene: &RenderScene,
    prev: &BlendData,
    next: &RenderScene,
    next_data: &BlendData,
    alpha: f32,
) -> RenderScene {
    let alpha = if alpha.is_nan() {
        1.0
    } else {
        alpha.clamp(0.0, 1.0)
    };
    if alpha >= 1.0
        || prev.epoch != next_data.epoch
        || (prev.poses.is_empty()
            && prev.particles.is_empty()
            && prev.bullets.is_empty()
            && prev.projectiles.is_empty())
    {
        return next.clone();
    }
    let delta = |key: Option<EntityId>| -> Option<Mat4> { delta_for(prev, next_data, key?, alpha) };
    let mut out = next.clone();

    for (inst, key) in out
        .meshes
        .iter_mut()
        .zip(keys(&next_data.meshes, next.meshes.len()))
    {
        if let Some(d) = delta(key) {
            inst.model = d * inst.model;
            let d3 = Mat3::from_mat4(d);
            if d3.determinant().abs() > DET_EPS {
                inst.normal_matrix = d3.inverse().transpose() * inst.normal_matrix;
            }
        }
    }
    for (inst, key) in out
        .sprites
        .iter_mut()
        .zip(keys(&next_data.sprites, next.sprites.len()))
    {
        if let Some(d) = delta(key) {
            inst.center = d.transform_point3(inst.center);
            inst.right = d.transform_vector3(inst.right);
            inst.up = d.transform_vector3(inst.up);
        }
    }
    for (inst, key) in out
        .lights
        .iter_mut()
        .zip(keys(&next_data.lights, next.lights.len()))
    {
        if let Some(d) = delta(key) {
            inst.position = d.transform_point3(inst.position);
        }
    }
    for (inst, key) in out
        .spot_lights
        .iter_mut()
        .zip(keys(&next_data.spot_lights, next.spot_lights.len()))
    {
        if let Some(d) = delta(key) {
            move_spot(inst, d);
        }
    }
    for (inst, key) in out
        .texts
        .iter_mut()
        .zip(keys(&next_data.texts, next.texts.len()))
    {
        if let Some(d) = delta(key) {
            inst.position = d.transform_point3(inst.position);
        }
    }
    for (inst, key) in out
        .ground_markers
        .iter_mut()
        .zip(keys(&next_data.ground_markers, next.ground_markers.len()))
    {
        if let Some(d) = delta(key) {
            // The shape is already scaled to world units: only the translation follows the entity.
            let t = d.transform_point3(inst.model.w_axis.truncate());
            inst.model.w_axis = t.extend(inst.model.w_axis.w);
        }
    }
    for (inst, key) in out
        .drop_shadows
        .iter_mut()
        .zip(keys(&next_data.drop_shadows, next.drop_shadows.len()))
    {
        if let Some(d) = delta(key) {
            inst.anchor = d.transform_point3(inst.anchor);
        }
    }
    for (inst, key) in out
        .decals
        .iter_mut()
        .zip(keys(&next_data.decals, next.decals.len()))
    {
        if let Some(d) = delta(key) {
            inst.box_to_world = d * inst.box_to_world;
            inst.world_to_box = inst.box_to_world.inverse();
        }
    }

    blend_particles(prev_scene, prev, next, next_data, alpha, &mut out);
    blend_bullets(prev_scene, prev, next_data, alpha, &mut out);
    blend_projectiles(prev_scene, prev, next_data, alpha, &mut out);

    let locals_moved = blend_viewmodel(prev_scene, prev, next_data, alpha, &mut out);

    // The drawn camera the viewmodel composes onto: the blended one, else the newer tick's when
    // only a local pose or flash centre moved.
    let mut camera_model = None;
    if let (Some(p), Some(n)) = (&prev.camera, &next_data.camera)
        && p.key == n.key
        && next_data.snaps.binary_search(&n.key).is_err()
        && (p.model != n.model || p.fov_y_deg != n.fov_y_deg)
    {
        let model = trs_lerp(p.model, n.model, alpha);
        if model.is_finite() && model.determinant().abs() > DET_EPS {
            out.camera.view = model.inverse();
            out.camera.position = model.w_axis.truncate();
            camera_model = Some(model);
            if let (Some(a), Some(b)) = (p.fov_y_deg, n.fov_y_deg) {
                let fov = a + (b - a) * alpha;
                out.camera.projection =
                    Mat4::perspective_rh(fov.to_radians(), n.aspect, n.near, n.far);
            }
        }
    }
    if camera_model.is_none() && locals_moved {
        camera_model = next_data.camera.map(|c| c.model);
    }
    if let Some(model) = camera_model {
        compose_viewmodel(&mut out, model);
    }
    out
}

/// Interpolates the viewmodel's camera-local poses and flash centres between the two ticks, so a
/// recoil or sway moves smoothly beside the blended camera. Meshes pair by owner (the n-th
/// occurrence with the n-th), flash particles by [`InstanceKey`]; a snapped owner takes the newer
/// local. Returns whether anything changed, which means `out` needs a fresh compose.
fn blend_viewmodel(
    prev_scene: &RenderScene,
    prev: &BlendData,
    next_data: &BlendData,
    alpha: f32,
    out: &mut RenderScene,
) -> bool {
    let (Some(prev_vm), Some(_)) = (&prev_scene.viewmodel, &out.viewmodel) else {
        return false;
    };
    let keyed_particles = prev.particles.len() <= prev_scene.particles.len()
        && next_data.particles.len() <= out.particles.len()
        && sorted(&prev.particles, |k| *k)
        && sorted(&next_data.particles, |k| *k);
    let Some(mut vm) = out.viewmodel.take() else {
        return false;
    };
    let mut changed = false;

    for i in 0..vm.locals.len() {
        let owner = vm.locals[i].owner;
        if next_data.snaps.binary_search(&owner).is_ok() {
            continue;
        }
        let nth = vm.locals[..i].iter().filter(|l| l.owner == owner).count();
        let Some(old) = prev_vm.locals.iter().filter(|l| l.owner == owner).nth(nth) else {
            continue;
        };
        let local = &mut vm.locals[i];
        if old.model == local.model {
            continue;
        }
        let model = trs_lerp(old.model, local.model, alpha);
        if !model.is_finite() {
            continue;
        }
        let (scale, rotation, _) = model.to_scale_rotation_translation();
        let inverse_scale = scale.map(|s| if s == 0.0 { 0.0 } else { 1.0 / s });
        local.model = model;
        local.normal_matrix = Mat3::from_quat(rotation) * Mat3::from_diagonal(inverse_scale);
        changed = true;
    }

    if keyed_particles {
        for local in &mut vm.particles {
            let Some(key) = next_data.particles.get(local.particle as usize) else {
                continue;
            };
            if next_data.snaps.binary_search(&key.owner).is_ok() {
                continue;
            }
            let Some(old) = prev_vm
                .particles
                .iter()
                .find(|p| prev.particles.get(p.particle as usize) == Some(key))
            else {
                continue;
            };
            if old.center != local.center {
                local.center = old.center.lerp(local.center, alpha);
                changed = true;
            }
        }
    }

    out.viewmodel = Some(vm);
    changed
}

/// Substitutes a latched camera's rotation into `scene`, keeping its drawn position.
///
/// `tick` is the tick's own camera and `latched` the same camera re-resolved with fresher look. The
/// scene's position moves by what the look changed (the eye offset turning with yaw), so a blended
/// position is kept and only the rotation is replaced; the projection is untouched. With no
/// interpolation the scene's position is `tick`'s, and the camera becomes `latched` exactly. Never
/// call it for a zero extra look: that frame draws the packet's scene as is.
pub fn latch_camera(scene: &mut RenderScene, tick: &CameraBlend, latched: &CameraBlend) {
    let mut model = latched.model;
    let position = scene.camera.position + (latched.model.w_axis - tick.model.w_axis).truncate();
    model.w_axis = position.extend(latched.model.w_axis.w);
    if model.is_finite() && model.determinant().abs() > DET_EPS {
        scene.camera.view = model.inverse();
        scene.camera.position = position;
        compose_viewmodel(scene, model);
    }
}

const DET_EPS: f32 = 1e-12;

fn sorted<T>(list: &[T], key: impl Fn(&T) -> InstanceKey) -> bool {
    list.windows(2).all(|w| key(&w[0]) < key(&w[1]))
}

/// Calls `f(prev_index, next_index)` for each key present in both sorted lists, skipping snapped
/// owners; does nothing when either list is unsorted.
fn matched<T>(
    prev: &[T],
    next: &[T],
    key: impl Fn(&T) -> InstanceKey,
    snaps: &[EntityId],
    mut f: impl FnMut(usize, usize),
) {
    if !sorted(prev, &key) || !sorted(next, &key) {
        return;
    }
    let (mut i, mut j) = (0, 0);
    while i < prev.len() && j < next.len() {
        let (a, b) = (key(&prev[i]), key(&next[j]));
        match a.cmp(&b) {
            std::cmp::Ordering::Less => i += 1,
            std::cmp::Ordering::Greater => j += 1,
            std::cmp::Ordering::Equal => {
                if snaps.binary_search(&b.owner).is_err() {
                    f(i, j);
                }
                i += 1;
                j += 1;
            }
        }
    }
}

fn blend_particles(
    prev_scene: &RenderScene,
    prev: &BlendData,
    next: &RenderScene,
    next_data: &BlendData,
    alpha: f32,
    out: &mut RenderScene,
) {
    if prev.particles.len() > prev_scene.particles.len()
        || next_data.particles.len() > next.particles.len()
    {
        return;
    }
    matched(
        &prev.particles,
        &next_data.particles,
        |k| *k,
        &next_data.snaps,
        |i, j| {
            let (a, b) = (&prev_scene.particles[i], &next.particles[j]);
            let o = &mut out.particles[j];
            o.center = a.center.lerp(b.center, alpha);
            o.size = a.size + (b.size - a.size) * alpha;
            for c in 0..4 {
                o.color[c] = a.color[c] + (b.color[c] - a.color[c]) * alpha;
            }
        },
    );
}

fn blend_bullets(
    prev_scene: &RenderScene,
    prev: &BlendData,
    next_data: &BlendData,
    alpha: f32,
    out: &mut RenderScene,
) {
    let in_range =
        |list: &[(InstanceKey, u32)], len: usize| list.iter().all(|(_, s)| (*s as usize) < len);
    if !in_range(&prev.bullets, prev_scene.sprites.len())
        || !in_range(&next_data.bullets, out.sprites.len())
    {
        return;
    }
    matched(
        &prev.bullets,
        &next_data.bullets,
        |(k, _)| *k,
        &next_data.snaps,
        |i, j| {
            let a = prev_scene.sprites[prev.bullets[i].1 as usize].center;
            let o = &mut out.sprites[next_data.bullets[j].1 as usize];
            o.center = a.lerp(o.center, alpha);
        },
    );
}

/// Lerps each pool shot found in both ticks (by its `index << 32 | generation` key) between its two
/// positions; a birth or death takes the newer tick. A shot has no owner, so nothing snaps it.
fn blend_projectiles(
    prev_scene: &RenderScene,
    prev: &BlendData,
    next_data: &BlendData,
    alpha: f32,
    out: &mut RenderScene,
) {
    let in_range = |list: &[(u64, ProjectileItem)], meshes: usize, particles: usize| {
        list.iter().all(|(_, item)| match *item {
            ProjectileItem::Mesh(i) => (i as usize) < meshes,
            ProjectileItem::Particle(i) => (i as usize) < particles,
        })
    };
    if !in_range(
        &prev.projectiles,
        prev_scene.meshes.len(),
        prev_scene.particles.len(),
    ) || !in_range(
        &next_data.projectiles,
        out.meshes.len(),
        out.particles.len(),
    ) || !prev.projectiles.windows(2).all(|w| w[0].0 < w[1].0)
        || !next_data.projectiles.windows(2).all(|w| w[0].0 < w[1].0)
    {
        return;
    }
    let (mut i, mut j) = (0, 0);
    while i < prev.projectiles.len() && j < next_data.projectiles.len() {
        let ((ka, a), (kb, b)) = (prev.projectiles[i], next_data.projectiles[j]);
        match ka.cmp(&kb) {
            std::cmp::Ordering::Less => i += 1,
            std::cmp::Ordering::Greater => j += 1,
            std::cmp::Ordering::Equal => {
                match (a, b) {
                    (ProjectileItem::Mesh(a), ProjectileItem::Mesh(b)) => {
                        let from = prev_scene.meshes[a as usize].model.w_axis;
                        let o = &mut out.meshes[b as usize].model.w_axis;
                        *o = from.lerp(*o, alpha);
                    }
                    (ProjectileItem::Particle(a), ProjectileItem::Particle(b)) => {
                        let from = prev_scene.particles[a as usize].center;
                        let o = &mut out.particles[b as usize].center;
                        *o = from.lerp(*o, alpha);
                    }
                    _ => {}
                }
                i += 1;
                j += 1;
            }
        }
    }
}

/// The keys for a list of `len` items: the parallel list when it matches, otherwise all unkeyed.
fn keys(list: &[Option<EntityId>], len: usize) -> impl Iterator<Item = Option<EntityId>> + '_ {
    let keyed = list.len() == len;
    (0..len).map(move |i| if keyed { list[i] } else { None })
}

fn pose_of(poses: &[(EntityId, Mat4)], key: EntityId) -> Option<Mat4> {
    poses
        .binary_search_by_key(&key, |(id, _)| *id)
        .ok()
        .map(|i| poses[i].1)
}

/// The world-space delta taking `key`'s newer pose to its pose at `alpha`; `None` when the item
/// should stay exactly where the newer tick put it.
fn delta_for(prev: &BlendData, next: &BlendData, key: EntityId, alpha: f32) -> Option<Mat4> {
    if next.snaps.binary_search(&key).is_ok() {
        return None;
    }
    let w_prev = pose_of(&prev.poses, key)?;
    let w_next = pose_of(&next.poses, key)?;
    if w_prev == w_next {
        return None;
    }
    let det = w_next.determinant();
    if !det.is_finite() || det.abs() < DET_EPS {
        return None;
    }
    let d = trs_lerp(w_prev, w_next, alpha) * w_next.inverse();
    d.is_finite().then_some(d)
}

/// Scale and translation lerped, rotation slerped (shortest arc), recomposed.
fn trs_lerp(a: Mat4, b: Mat4, t: f32) -> Mat4 {
    let (sa, ra, ta) = a.to_scale_rotation_translation();
    let (sb, rb, tb) = b.to_scale_rotation_translation();
    let r: Quat = ra.slerp(rb, t);
    Mat4::from_scale_rotation_translation(sa.lerp(sb, t), r, ta.lerp(tb, t))
}

fn move_spot(inst: &mut SpotLightData, d: Mat4) {
    inst.position = d.transform_point3(inst.position);
    let dir = d.transform_vector3(inst.direction);
    inst.direction = if dir.length_squared() > 0.0 {
        dir.normalize()
    } else {
        inst.direction
    };
}

#[cfg(test)]
mod tests {
    use super::*;
    use ludion_core::math::glam::Vec3;

    use crate::scene::{
        DecalBlend, DecalInstance, DecalMaps, DecalReceivers, DropShadowInstance,
        GroundMarkerInstance, GroundMarkerShape, MeshInstance, MeshShape, PointLightData,
        SpriteInstance, TextRun,
    };

    const E1: EntityId = EntityId::new(1, 0);
    const E2: EntityId = EntityId::new(2, 0);

    fn at(x: f32) -> Mat4 {
        Mat4::from_translation(Vec3::new(x, 0.0, 0.0))
    }

    fn mesh(model: Mat4) -> MeshInstance {
        MeshInstance {
            medium: 1.0,
            shape: MeshShape::Cube,
            model,
            normal_matrix: Mat3::from_mat4(model),
            color: [1.0, 0.0, 0.0, 1.0],
            material: [0.5, 0.0, 0.0],
            use_vertex_material: false,
            fade: 1.0,
            fade_height_min: -f32::MAX,
            flash: 0.0,
            flash_color: [1.0; 3],
            wrap: 0.0,
            scatter_color: [1.0; 3],
            bone_offset: 0,
            cast_shadows: true,
            viewmodel: false,
            rim: [0.0; 4],
        }
    }

    fn sprite(center: Vec3) -> SpriteInstance {
        SpriteInstance {
            center,
            right: Vec3::X,
            up: Vec3::Y,
            color: [1.0; 4],
            texture: None,
            uv: SpriteInstance::FULL_UV,
            lit: false,
            layer: 0,
            additive: false,
            flash: 0.0,
            flash_color: [1.0; 3],
            uv_repeat: [1.0, 1.0],
            uv_offset: [0.0, 0.0],
        }
    }

    fn point(position: Vec3) -> PointLightData {
        PointLightData {
            position,
            color: Vec3::ONE,
            intensity: 1.0,
            range: 5.0,
            dynamic: false,
            shadow: None,
            scatter: 0.0,
        }
    }

    fn spot(position: Vec3) -> SpotLightData {
        SpotLightData {
            position,
            direction: Vec3::X,
            color: Vec3::ONE,
            intensity: 1.0,
            range: 5.0,
            cos_inner: 0.9,
            cos_outer: 0.8,
            shadow: None,
            scatter: 0.0,
        }
    }

    fn text(position: Vec3) -> TextRun {
        TextRun {
            position,
            size: 1.0,
            color: [1.0; 4],
            text: String::new(),
            quads: Vec::new(),
            extent: [0.0, 0.0],
            font: None,
        }
    }

    fn marker(model: Mat4) -> GroundMarkerInstance {
        GroundMarkerInstance {
            shape: GroundMarkerShape::Disc { radius: 2.0 },
            model,
            color: [1.0; 4],
        }
    }

    fn shadow(anchor: Vec3) -> DropShadowInstance {
        DropShadowInstance {
            shape: GroundMarkerShape::Disc { radius: 1.0 },
            anchor,
            yaw: 0.0,
            opacity: 0.5,
            softness: 0.5,
            max_distance: 10.0,
            fade_distance: 2.0,
            exclude: Vec::new(),
        }
    }

    fn decal(box_to_world: Mat4) -> DecalInstance {
        DecalInstance {
            box_to_world,
            world_to_box: box_to_world.inverse(),
            blend: DecalBlend::Multiply,
            color: [1.0; 4],
            opacity: 1.0,
            roughness: None,
            metallic: 0.0,
            uv_rect: [0.0, 0.0, 1.0, 1.0],
            angle_fade: [60.0, 80.0],
            layer: 0,
            maps: DecalMaps::default(),
            receivers: DecalReceivers::Exclude(Vec::new()),
        }
    }

    /// One item of every kind keyed to `E1`, at x = `x`.
    fn world(x: f32) -> (RenderScene, BlendData) {
        let mut s = RenderScene::empty((64, 64));
        s.meshes.push(mesh(at(x)));
        s.sprites.push(sprite(Vec3::new(x, 0.0, 0.0)));
        s.lights.push(point(Vec3::new(x, 0.0, 0.0)));
        s.spot_lights.push(spot(Vec3::new(x, 0.0, 0.0)));
        s.texts.push(text(Vec3::new(x, 0.0, 0.0)));
        s.ground_markers.push(marker(at(x)));
        s.drop_shadows.push(shadow(Vec3::new(x, 0.0, 0.0)));
        s.decals.push(decal(at(x)));
        let k = vec![Some(E1)];
        let d = BlendData {
            epoch: 1,
            poses: vec![(E1, at(x))],
            meshes: k.clone(),
            sprites: k.clone(),
            lights: k.clone(),
            spot_lights: k.clone(),
            texts: k.clone(),
            ground_markers: k.clone(),
            drop_shadows: k.clone(),
            decals: k,
            ..BlendData::default()
        };
        (s, d)
    }

    fn assert_x(s: &RenderScene, x: f32) {
        assert_eq!(s.meshes[0].model.w_axis.x, x);
        assert_eq!(s.sprites[0].center.x, x);
        assert_eq!(s.lights[0].position.x, x);
        assert_eq!(s.spot_lights[0].position.x, x);
        assert_eq!(s.texts[0].position.x, x);
        assert_eq!(s.ground_markers[0].model.w_axis.x, x);
        assert_eq!(s.drop_shadows[0].anchor.x, x);
        assert_eq!(s.decals[0].box_to_world.w_axis.x, x);
    }

    #[test]
    fn midpoint_lands_halfway() {
        let (_, p) = world(0.0);
        let (s, n) = world(1.0);
        assert_x(&blend(&s, &p, &s, &n, 0.5), 0.5);
    }

    #[test]
    fn alpha_one_is_next_and_zero_is_previous() {
        let (_, p) = world(0.0);
        let (s, n) = world(1.0);
        assert_eq!(blend(&s, &p, &s, &n, 1.0), s);
        assert_eq!(blend(&s, &p, &s, &n, 3.0), s);
        assert_x(&blend(&s, &p, &s, &n, 0.0), 0.0);
        assert_x(&blend(&s, &p, &s, &n, -1.0), 0.0);
    }

    #[test]
    fn snap_spawn_and_epoch_take_next() {
        let (_, p) = world(0.0);
        let (s, mut n) = world(1.0);
        n.snaps = vec![E1];
        assert_eq!(blend(&s, &p, &s, &n, 0.5), s);

        let (_, mut p2) = world(0.0);
        p2.poses = vec![(E2, at(0.0))];
        let (_, n2) = world(1.0);
        assert_eq!(blend(&s, &p2, &s, &n2, 0.5), s);

        let (_, mut p3) = world(0.0);
        p3.epoch = 0;
        let (_, n3) = world(1.0);
        assert_eq!(blend(&s, &p3, &s, &n3, 0.5), s);
    }

    #[test]
    fn unkeyed_and_non_pose_fields_come_from_next() {
        let (_, p) = world(0.0);
        let (mut s, mut n) = world(1.0);
        s.meshes.push(mesh(at(7.0)));
        n.meshes.push(None);
        s.meshes[0].color = [0.0, 1.0, 0.0, 1.0];
        s.meshes[0].fade = 0.25;
        let out = blend(&s, &p, &s, &n, 0.5);
        assert_eq!(out.meshes[1], s.meshes[1]);
        assert_eq!(out.meshes[0].color, [0.0, 1.0, 0.0, 1.0]);
        assert_eq!(out.meshes[0].fade, 0.25);
        assert_eq!(out.meshes[0].model.w_axis.x, 0.5);
    }

    #[test]
    fn yaw_slerps_to_forty_five_degrees() {
        let prev = Mat4::IDENTITY;
        let next = Mat4::from_rotation_y(90f32.to_radians());
        let mut s = RenderScene::empty((64, 64));
        s.meshes.push(mesh(next));
        let p = BlendData {
            epoch: 1,
            poses: vec![(E1, prev)],
            ..BlendData::default()
        };
        let n = BlendData {
            epoch: 1,
            poses: vec![(E1, next)],
            meshes: vec![Some(E1)],
            ..BlendData::default()
        };
        let out = blend(&s, &p, &s, &n, 0.5);
        let want = Mat4::from_rotation_y(45f32.to_radians());
        assert!(out.meshes[0].model.abs_diff_eq(want, 1e-5));
        assert!(
            out.meshes[0]
                .normal_matrix
                .abs_diff_eq(Mat3::from_mat4(want), 1e-5)
        );
    }

    #[test]
    fn ground_marker_moves_by_translation_only() {
        let prev = Mat4::from_scale(Vec3::splat(1.0));
        let next = Mat4::from_scale_rotation_translation(Vec3::splat(3.0), Quat::IDENTITY, Vec3::X);
        let m = marker(at(1.0));
        let mut s = RenderScene::empty((64, 64));
        s.ground_markers.push(m);
        let p = BlendData {
            epoch: 1,
            poses: vec![(E1, prev)],
            ..BlendData::default()
        };
        let n = BlendData {
            epoch: 1,
            poses: vec![(E1, next)],
            ground_markers: vec![Some(E1)],
            ..BlendData::default()
        };
        let out = blend(&s, &p, &s, &n, 0.5);
        assert_eq!(out.ground_markers[0].model.x_axis, m.model.x_axis);
        assert_eq!(out.ground_markers[0].model.w_axis.x, 0.5);
    }

    #[test]
    fn decal_inverse_follows_the_blended_box() {
        let (_, p) = world(0.0);
        let (s, n) = world(1.0);
        let out = blend(&s, &p, &s, &n, 0.5);
        let d = &out.decals[0];
        assert!((d.world_to_box * d.box_to_world).abs_diff_eq(Mat4::IDENTITY, 1e-6));
    }

    fn camera_data(x: f32, fov: f32, key: EntityId) -> BlendData {
        BlendData {
            epoch: 1,
            poses: vec![(E1, at(x))],
            camera: Some(CameraBlend {
                key,
                model: at(x),
                fov_y_deg: Some(fov),
                near: 0.1,
                far: 100.0,
                aspect: 1.0,
            }),
            ..BlendData::default()
        }
    }

    #[test]
    fn camera_midpoint_position_and_fov() {
        let s = RenderScene::empty((64, 64));
        let out = blend(
            &s,
            &camera_data(0.0, 60.0, E1),
            &s,
            &camera_data(2.0, 90.0, E1),
            0.5,
        );
        assert_eq!(out.camera.position, Vec3::new(1.0, 0.0, 0.0));
        assert!(out.camera.view.abs_diff_eq(at(1.0).inverse(), 1e-6));
        let want = Mat4::perspective_rh(75f32.to_radians(), 1.0, 0.1, 100.0);
        assert!(out.camera.projection.abs_diff_eq(want, 1e-5));
    }

    #[test]
    fn a_different_camera_key_takes_next() {
        let s = RenderScene::empty((64, 64));
        let out = blend(
            &s,
            &camera_data(0.0, 60.0, E1),
            &s,
            &camera_data(2.0, 90.0, E2),
            0.5,
        );
        assert_eq!(out.camera, s.camera);
    }

    #[test]
    fn a_mismatched_key_list_is_untouched() {
        let (_, p) = world(0.0);
        let (mut s, mut n) = world(1.0);
        s.meshes.push(mesh(at(1.0)));
        assert_eq!(n.meshes.len(), 1);
        let out = blend(&s, &p, &s, &n, 0.5);
        assert_eq!(out.meshes, s.meshes);
        n.meshes.clear();
        assert_eq!(blend(&s, &p, &s, &n, 0.5).meshes, s.meshes);
    }

    fn key(owner: EntityId, index: i64) -> InstanceKey {
        InstanceKey {
            owner,
            started: 0,
            burst: false,
            index,
        }
    }

    fn particle(x: f32, alpha: f32) -> crate::scene::ParticleInstance {
        crate::scene::ParticleInstance {
            center: Vec3::new(x, 0.0, 0.0),
            right: Vec3::X,
            up: Vec3::Y,
            size: 1.0,
            color: [1.0, 1.0, 1.0, alpha],
            softness: 0.0,
            additive: false,
            lit: false,
            viewmodel: false,
        }
    }

    fn particles(items: &[(InstanceKey, f32, f32)]) -> (RenderScene, BlendData) {
        let mut s = RenderScene::empty((64, 64));
        let mut d = BlendData {
            epoch: 1,
            ..BlendData::default()
        };
        for (k, x, a) in items {
            s.particles.push(particle(*x, *a));
            d.particles.push(*k);
        }
        (s, d)
    }

    #[test]
    fn particles_found_in_both_ticks_lerp_and_births_deaths_step() {
        let (ps, pd) = particles(&[(key(E1, 4), 0.0, 1.0), (key(E1, 5), 0.0, 1.0)]);
        let (ns, nd) = particles(&[(key(E1, 5), 1.0, 0.0), (key(E1, 6), 3.0, 1.0)]);
        let out = blend(&ps, &pd, &ns, &nd, 0.5);
        assert_eq!(out.particles.len(), 2);
        assert_eq!(out.particles[0].center.x, 0.5);
        assert_eq!(out.particles[0].color[3], 0.5);
        assert_eq!(out.particles[1], ns.particles[1]);
        assert_eq!(blend(&ps, &pd, &ns, &nd, 1.0), ns);
    }

    #[test]
    fn a_snapped_or_unsorted_particle_list_is_untouched() {
        let (ps, pd) = particles(&[(key(E1, 5), 0.0, 1.0)]);
        let (ns, mut nd) = particles(&[(key(E1, 5), 1.0, 1.0)]);
        nd.snaps = vec![E1];
        assert_eq!(blend(&ps, &pd, &ns, &nd, 0.5), ns);

        let (ps, pd) = particles(&[(key(E1, 6), 0.0, 1.0), (key(E1, 5), 0.0, 1.0)]);
        let (ns, nd) = particles(&[(key(E1, 5), 1.0, 1.0)]);
        assert_eq!(blend(&ps, &pd, &ns, &nd, 0.5), ns);
    }

    /// A scene with entity mesh `E1` at `mx`, an emitter particle at `px` (keyed), and pool shots
    /// appended: a mesh shot and a billboard shot at `sx` for each key in `shots`.
    fn with_shots(mx: f32, px: f32, sx: f32, shots: &[u64]) -> (RenderScene, BlendData) {
        let mut s = RenderScene::empty((64, 64));
        let mut d = BlendData {
            epoch: 1,
            poses: vec![(E1, at(mx))],
            ..BlendData::default()
        };
        s.meshes.push(mesh(at(mx)));
        d.meshes.push(Some(E1));
        s.particles.push(particle(px, 1.0));
        d.particles.push(key(E1, 0));
        for &k in shots {
            d.projectiles
                .push((k, ProjectileItem::Mesh(s.meshes.len() as u32)));
            s.meshes.push(mesh(at(sx)));
            d.meshes.push(None);
            d.projectiles
                .push((k + 1, ProjectileItem::Particle(s.particles.len() as u32)));
            s.particles.push(particle(sx, 1.0));
        }
        (s, d)
    }

    #[test]
    fn matched_projectiles_lerp_and_births_take_the_newer_tick() {
        let (ps, pd) = with_shots(0.0, 0.0, 0.0, &[10]);
        let (ns, nd) = with_shots(2.0, 2.0, 4.0, &[10, 20]);
        let out = blend(&ps, &pd, &ns, &nd, 0.5);
        // The entity mesh and the emitter particle still blend beside the appended shots.
        assert_eq!(out.meshes[0].model.w_axis.x, 1.0);
        assert_eq!(out.particles[0].center.x, 1.0);
        // The matched shot lerps (mesh and billboard); the birth is as extracted.
        assert_eq!(out.meshes[1].model.w_axis.x, 2.0);
        assert_eq!(out.particles[1].center.x, 2.0);
        assert_eq!(out.meshes[2], ns.meshes[2]);
        assert_eq!(out.particles[2], ns.particles[2]);
    }

    #[test]
    fn a_pool_only_scene_still_interpolates() {
        let mk = |x: f32| {
            let mut s = RenderScene::empty((64, 64));
            s.particles.push(particle(x, 1.0));
            let d = BlendData {
                epoch: 1,
                projectiles: vec![(7, ProjectileItem::Particle(0))],
                ..BlendData::default()
            };
            (s, d)
        };
        let (ps, pd) = mk(0.0);
        let (ns, nd) = mk(2.0);
        assert_eq!(blend(&ps, &pd, &ns, &nd, 0.5).particles[0].center.x, 1.0);
    }

    #[test]
    fn a_bullet_sprite_lerps_and_an_entity_sprite_does_not() {
        let build = |x: f32| {
            let mut s = RenderScene::empty((64, 64));
            s.sprites.push(sprite(Vec3::new(x, 0.0, 0.0)));
            s.sprites.push(sprite(Vec3::new(x, 1.0, 0.0)));
            let d = BlendData {
                epoch: 1,
                sprites: vec![None, None],
                bullets: vec![(key(E1, 0), 1)],
                ..BlendData::default()
            };
            (s, d)
        };
        let (ps, pd) = build(0.0);
        let (ns, nd) = build(2.0);
        let out = blend(&ps, &pd, &ns, &nd, 0.5);
        assert_eq!(out.sprites[1].center.x, 1.0);
        assert_eq!(out.sprites[0], ns.sprites[0]);
    }
}
