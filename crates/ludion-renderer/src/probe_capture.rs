//! Reflection-probe capture: six world-axis faces per probe, rendered as ordinary frames of a
//! derived scene and prefiltered into one slot of a cube array (`Plan/reflections.md` decisions 16,
//! 17 and 21). The `reflections` pass samples the array (`probe_shade.rs`).

use std::cell::{Cell, RefCell};
use std::f32::consts::FRAC_PI_2;
use std::path::PathBuf;

use ludion_core::diagnostics::Diagnostic;
use ludion_core::math::glam::{Mat4, Vec3};

use crate::frame_observer::{FrameObserver, FramePhase};
use crate::ibl;
use crate::probe::{MAX_PROBES, probe_rank};
use crate::probe_store;
use crate::quality::{Quality, QualityParams};
use crate::render::Renderer;
use crate::scene::{ProbeWorld, RenderCamera, RenderScene};
use ludion_core::ids::EntityId;

const CAPTURE_NEAR: f32 = 0.05;
const CAPTURE_FAR: f32 = 1000.0;

/// Look direction and up vector per cube face, in the wgpu layer order (+X, −X, +Y, −Y, +Z, −Z).
/// The faces render unmirrored and `probe_face.wgsl` flips x at store, which is what the cube
/// convention's handedness asks for.
const FACES: [(Vec3, Vec3); 6] = [
    (Vec3::X, Vec3::Y),
    (Vec3::NEG_X, Vec3::Y),
    (Vec3::Y, Vec3::NEG_Z),
    (Vec3::NEG_Y, Vec3::Z),
    (Vec3::Z, Vec3::Y),
    (Vec3::NEG_Z, Vec3::Y),
];

fn face_camera(position: Vec3, face: u32) -> RenderCamera {
    let (dir, up) = FACES[face as usize];
    RenderCamera {
        view: Mat4::look_to_rh(position, dir, up),
        projection: Mat4::perspective_rh(FRAC_PI_2, 1.0, CAPTURE_NEAR, CAPTURE_FAR),
        position,
    }
}

/// `scene` seen from one probe face: the lit world only. The overlay content, the screen-space
/// effects and the sun disc (the IBL excludes it, so a probe does too) are dropped; the tables the
/// caches key on stay. Of the caches, only the three keyed ones that take `CacheEviction`
/// (`Renderer::resolve_textures`, `resolve_mesh_textures`, `resolve_mesh_assets`) are resolved
/// without evicting, so they keep what the main frame holds.
pub(crate) fn face_scene(scene: &RenderScene, position: Vec3, face: u32) -> RenderScene {
    let mut out = scene.clone();
    out.camera = face_camera(position, face);
    out.cull_camera = None;
    // The gun belongs to the screen, not the world: a face draws no viewmodel, and its moving pose
    // must not re-key the capture.
    out.meshes.retain(|mesh| !mesh.viewmodel);
    out.viewmodel = None;
    out.sprites.clear();
    out.particles.clear();
    out.texts.clear();
    out.ground_markers.clear();
    out.drop_shadows.clear();
    out.decals.clear();
    // Probes are lit by local lights unshadowed, like the GI bake. A face has no composite, so
    // in-scatter into the medium must not re-key it.
    for light in &mut out.lights {
        light.shadow = None;
        light.scatter = 0.0;
    }
    for light in &mut out.spot_lights {
        light.shadow = None;
        light.scatter = 0.0;
    }
    out.ui.clear();
    out.debug_lines.clear();
    out.probes.clear();
    // A face is lit with the anchored box: a fit to a 90° face camera means nothing.
    if let Some(sun) = &mut out.sun {
        sun.cascades = None;
    }
    out.probe_world = None;
    out.debug_view = None;
    let mut post = out.post.unwrap_or_default();
    post.ssao = 0.0;
    post.ssr = 0.0;
    post.shafts = 0.0;
    post.dof = 0.0;
    out.post = Some(post);
    if let Some(env) = &mut out.environment {
        env.sun_disc_size = 0.0;
        env.sun_disc_intensity = 0.0;
    }
    out
}

/// The scene a capture reads its world from: the frozen probe world when the scene carries one.
fn capture_world(scene: &RenderScene) -> &RenderScene {
    scene.probe_world.as_ref().map_or(scene, ProbeWorld::scene)
}

/// The captured, prefiltered probes: a cube array with one slot per probe.
#[derive(Clone)]
pub(crate) struct ProbeCubes {
    pub(crate) texture: wgpu::Texture,
    /// A cube-array view of every layer and mip.
    pub(crate) view: wgpu::TextureView,
    /// Probe index → slot. The identity today; the table is what lets residency change addressing.
    pub(crate) slots: Vec<u32>,
    /// Probe index → the diffuse irradiance baked at its capture point (`probe_relight.wgsl`).
    pub(crate) irradiance: Vec<AxisIrradiance>,
    /// The emissive part of each flagged probe's capture, prefiltered into the same slots: allocated
    /// only when some probe sees emissive. The relight leaves it unscaled (decision 17).
    pub(crate) emissive: Option<(wgpu::Texture, wgpu::TextureView)>,
    /// Probe index → whether its slot of `emissive` holds an emissive cube.
    pub(crate) emissive_slots: Vec<bool>,
    pub(crate) face_size: u32,
    pub(crate) max_lod: u32,
}

/// What the renderer's probe resolves did: how many ran GPU work, and for the last one each probe's
/// store key and whether its cube came from the store.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct ProbeResolve {
    /// Increments on each resolve that does GPU work.
    pub resolve: u64,
    /// The last resolve's probes, in slot (entity) order.
    pub probes: Vec<ResolvedProbe>,
}

/// One probe of the last resolve.
#[derive(Clone, Debug, PartialEq)]
pub struct ResolvedProbe {
    /// The probe's entity.
    pub entity: ludion_core::ids::EntityId,
    /// The store key; `None` without a store.
    pub key: Option<u64>,
    /// The cube came from the store rather than a capture.
    pub loaded: bool,
}

/// What the world key was computed from: the probe world, the quality toggles and parameters, the
/// shader code key and the face size.
type WorldMemo = (u64, Quality, QualityParams, u64, u32);

/// The renderer's probe captures.
pub(crate) struct Probes {
    /// Keyed by the probes' store keys (see [`Probes::resident_key`]): anything that can change a
    /// capture's bytes recaptures.
    cache: RefCell<Option<(u64, ProbeCubes)>>,
    /// The world key of the last probe world, so a live frame pays a compare, not a hash of every mesh.
    world_memo: RefCell<Option<(WorldMemo, u64)>>,
    /// The capture targets the faces are copied into (lit, emissive), kept for the face size they
    /// were built for.
    source: RefCell<Option<(u32, wgpu::Texture, wgpu::Texture)>>,
    /// The probe store directory, when the host set one.
    store: RefCell<Option<PathBuf>>,
    report: RefCell<ProbeResolve>,
    /// Set while a readback runs: each captured probe's raw faces are kept before the shared
    /// source cube is overwritten by the next one.
    retain: Cell<bool>,
    /// One entry per slot of the retained resolve; `None` for a slot the store supplied.
    captured: RefCell<Vec<Option<Vec<u8>>>>,
    /// The last store-hit stamp thread (`probe_store::stamp_used`); production never joins it.
    pub(crate) stamping: RefCell<Option<std::thread::JoinHandle<()>>>,
}

/// One probe as `Renderer::read_probes` returns it, for a dump or a test.
#[derive(Clone, Debug)]
pub struct ProbeReadback {
    /// The probe's entity.
    pub entity: EntityId,
    /// The probe's cube-array index: its position in entity order.
    pub slot: u32,
    /// The probe's position in [`probe_rank`] order: 0 is shaded first.
    pub rank: u32,
    /// The store key at this renderer's quality, with or without a store.
    pub key: u64,
    /// The cube's edge in texels.
    pub face_size: u32,
    /// The raw capture, mip 0 RGBA, faces in layer order (+X −X +Y −Y +Z −Z).
    pub captured: Option<Vec<f32>>,
    /// The prefiltered slot, one entry per mip, same layout.
    pub levels: Vec<Vec<f32>>,
}

/// A point's diffuse irradiance per cube axis, in face order (+X −X +Y −Y +Z −Z).
pub type AxisIrradiance = [[f32; 3]; 6];

/// One probe's baked and live diffuse irradiance as `Renderer::read_probe_irradiance` returns them.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ProbeIrradiance {
    /// E stored with the capture, measured at the capture point.
    pub baked: AxisIrradiance,
    /// E the relight kernel evaluates for the current frame at the same point.
    pub live: AxisIrradiance,
}

fn f16_texels(bytes: &[u8]) -> Vec<f32> {
    bytes
        .chunks_exact(2)
        .map(|b| crate::gi_bake::f16_to_f32(u16::from_le_bytes([b[0], b[1]])))
        .collect()
}

/// The store key of each probe of `scene` at `r`'s quality: the world key once, then the capture
/// point. The one place keys are computed, so a dump can never disagree with a host.
fn store_keys(r: &Renderer, scene: &RenderScene, face_size: u32) -> Vec<u64> {
    let world = r.probes.world_key(r, scene, face_size);
    scene.probes[..scene.probes.len().min(MAX_PROBES)]
        .iter()
        .map(|probe| probe_store::probe_key(world, probe.position))
        .collect()
}

impl Renderer {
    /// The store key of each of `scene`'s probes at this renderer's quality, in slot order: what a
    /// `Store` host loading `scene` would look up.
    pub fn probe_keys(&self, scene: &RenderScene) -> Vec<u64> {
        if scene.probes.is_empty() {
            return Vec::new();
        }
        store_keys(self, scene, self.ibl_specular_size())
    }

    /// Captures `scene`'s probes fresh and reads them back, in slot order. A dump path: it captures
    /// every probe rather than loading from the store, and leaves the resident cubes a fresh capture
    /// of the same probe set.
    ///
    /// # Errors
    /// `L2010` / `L2011` if a capture met a GPU fault.
    pub fn read_probes(
        &self,
        scene: &RenderScene,
        mut observer: Option<&mut dyn FrameObserver>,
    ) -> Result<Vec<ProbeReadback>, Diagnostic> {
        let count = scene.probes.len().min(MAX_PROBES);
        if count == 0 {
            return Ok(Vec::new());
        }
        let probes = &self.probes;
        probes.retain.set(true);
        probes.recapture();
        let resolved = probes.resolve(self, scene, &mut observer);
        probes.retain.set(false);
        resolved?;
        let captured = std::mem::take(&mut *probes.captured.borrow_mut());
        let cubes = probes.cubes().expect("the scene has probes");
        let keys = store_keys(self, scene, cubes.face_size);
        let mut by_rank: Vec<usize> = (0..count).collect();
        by_rank.sort_by(|&a, &b| probe_rank(&scene.probes[a], &scene.probes[b]));
        let mut rank_of = vec![0u32; count];
        for (rank, &slot) in by_rank.iter().enumerate() {
            rank_of[slot] = rank as u32;
        }
        (0..count)
            .map(|slot| {
                Ok(ProbeReadback {
                    entity: scene.probes[slot].entity,
                    slot: slot as u32,
                    rank: rank_of[slot],
                    key: keys[slot],
                    face_size: cubes.face_size,
                    captured: captured
                        .get(slot)
                        .and_then(|bytes| bytes.as_deref())
                        .map(f16_texels),
                    levels: probe_store::read_slot(self.gpu(), &cubes.texture, slot as u32)?
                        .iter()
                        .map(|level| f16_texels(level))
                        .collect(),
                })
            })
            .collect()
    }
}

/// Layer `face` of a capture cube's mip 0, the copy destination of one rendered face.
fn face_layer(cube: &wgpu::Texture, face: u32) -> wgpu::TexelCopyTextureInfo<'_> {
    wgpu::TexelCopyTextureInfo {
        texture: cube,
        mip_level: 0,
        origin: wgpu::Origin3d {
            x: 0,
            y: 0,
            z: face,
        },
        aspect: wgpu::TextureAspect::All,
    }
}

impl Probes {
    pub(crate) fn new() -> Probes {
        Probes {
            cache: RefCell::new(None),
            world_memo: RefCell::new(None),
            source: RefCell::new(None),
            store: RefCell::new(None),
            report: RefCell::new(ProbeResolve::default()),
            retain: Cell::new(false),
            captured: RefCell::new(Vec::new()),
            stamping: RefCell::new(None),
        }
    }

    /// Drops the resident captures so the next resolve captures again.
    pub(crate) fn recapture(&self) {
        self.cache.borrow_mut().take();
    }

    /// Sets the store directory; the resident captures are dropped so the next resolve consults it.
    pub(crate) fn set_store(&self, dir: Option<PathBuf>) {
        if *self.store.borrow() != dir {
            *self.store.borrow_mut() = dir;
            self.cache.borrow_mut().take();
        }
    }

    /// What the resolves did so far.
    pub(crate) fn report(&self) -> ProbeResolve {
        self.report.borrow().clone()
    }

    /// The world key of `scene`'s capture world at `r`'s quality and code. Memoized while the scene
    /// carries the same probe world; a scene without one is hashed on every call.
    fn world_key(&self, r: &Renderer, scene: &RenderScene, face_size: u32) -> u64 {
        let quality = r.quality();
        let params = r.quality_params();
        let code = r.capture_code_key();
        let memo = scene
            .probe_world
            .as_ref()
            .map(|world| (world.id(), quality, params, code, face_size));
        if let Some((held, key)) = &*self.world_memo.borrow()
            && memo.as_ref() == Some(held)
        {
            return *key;
        }
        let key = probe_store::world_key(
            &face_scene(capture_world(scene), Vec3::ZERO, 0),
            &quality,
            &params,
            code,
            face_size,
        );
        *self.world_memo.borrow_mut() = memo.map(|memo| (memo, key));
        key
    }

    /// The resident cubes' key: the face size and each probe's entity and store key, so the probe
    /// set, the capture world and everything the store key hashes decide a recapture.
    fn resident_key(probes: &[crate::probe::ProbeData], keys: &[u64], face_size: u32) -> u64 {
        let mut bytes = Vec::with_capacity(4 + probes.len() * 16);
        bytes.extend_from_slice(&face_size.to_le_bytes());
        for (probe, key) in probes.iter().zip(keys) {
            bytes.extend_from_slice(&probe.entity.index().to_le_bytes());
            bytes.extend_from_slice(&probe.entity.generation().to_le_bytes());
            bytes.extend_from_slice(&key.to_le_bytes());
        }
        ludion_assets::content_hash(&bytes)
    }

    /// Captures and prefilters `scene`'s probes unless the probe set is the one already resident.
    /// An `observer` brackets each captured probe as `ProbeBake`; loaded probes are not bracketed.
    /// A scene with no probes frees the array and does no GPU work.
    ///
    /// # Errors
    /// A GPU fault during a capture (`L2010` / `L2011`). Nothing is cached and no store entry is
    /// written for the failed probe, so the next call recaptures it.
    pub(crate) fn resolve(
        &self,
        r: &Renderer,
        scene: &RenderScene,
        observer: &mut Option<&mut dyn FrameObserver>,
    ) -> Result<(), Diagnostic> {
        debug_assert!(scene.probes.len() <= MAX_PROBES, "13 caps the probe list");
        let probes = &scene.probes[..scene.probes.len().min(MAX_PROBES)];
        if probes.is_empty() {
            self.cache.borrow_mut().take();
            self.report.borrow_mut().probes.clear();
            return Ok(());
        }
        let face_size = r.ibl_specular_size();
        let keys = store_keys(r, scene, face_size);
        let key = Self::resident_key(probes, &keys, face_size);
        if self
            .cache
            .borrow()
            .as_ref()
            .is_some_and(|(cached, _)| *cached == key)
        {
            return Ok(());
        }

        let gpu = r.gpu();
        let array = ibl::probe_array(gpu.device(), face_size, probes.len() as u32);
        let (source, emissive_source) = {
            let mut held = self.source.borrow_mut();
            if held.as_ref().is_none_or(|(size, ..)| *size != face_size) {
                *held = Some((
                    face_size,
                    ibl::source_cube(gpu.device(), face_size),
                    ibl::source_cube(gpu.device(), face_size),
                ));
            }
            let (_, lit, emissive) = held.as_ref().expect("just filled");
            (lit.clone(), emissive.clone())
        };
        // Allocated by the first probe that sees emissive; a probe set without any costs nothing.
        let mut emissive_array: Option<wgpu::Texture> = None;
        let mut emissive_slots = Vec::with_capacity(probes.len());
        let emissive_slot = |array: &Option<wgpu::Texture>| -> wgpu::Texture {
            array
                .clone()
                .unwrap_or_else(|| ibl::probe_array(gpu.device(), face_size, probes.len() as u32))
        };
        let store = self.store.borrow().clone();
        let retain = self.retain.get();
        if retain {
            self.captured.borrow_mut().clear();
        }
        let mut resolved = Vec::with_capacity(probes.len());
        let mut irradiance = Vec::with_capacity(probes.len());
        let mut hits = Vec::new();
        // No borrow of the cache is held below: a face's own `prepare_frame` resolves its
        // probe-less scene back through here.
        for (slot, probe) in probes.iter().enumerate() {
            let slot = slot as u32;
            let entry = store.as_deref().map(|dir| (dir, keys[slot as usize]));
            let loaded = entry.and_then(|(dir, key)| {
                probe_store::read_entry(dir, key, face_size, array.mip_level_count())
            });
            if let Some(cube) = loaded {
                if let Some((dir, key)) = entry {
                    hits.push(probe_store::entry_path(dir, key));
                }
                probe_store::write_slot(gpu, &array, slot, &cube.levels);
                if let Some(levels) = &cube.emissive {
                    let target = emissive_slot(&emissive_array);
                    probe_store::write_slot(gpu, &target, slot, levels);
                    emissive_array = Some(target);
                }
                emissive_slots.push(cube.emissive.is_some());
                irradiance.push(cube.irradiance);
                if retain {
                    self.captured.borrow_mut().push(None);
                }
                resolved.push(ResolvedProbe {
                    entity: probe.entity,
                    key: entry.map(|(_, key)| key),
                    loaded: true,
                });
                continue;
            }
            let mut baked = [[0.0; 3]; 6];
            let captured = r.observed_phase(observer, FramePhase::ProbeBake, || {
                for face in 0..6 {
                    let derived = face_scene(capture_world(scene), probe.position, face);
                    // The irradiance is a property of the capture point, not of a face: face 0 reads it.
                    let measured = r.capture_probe_face(
                        &derived,
                        face_size,
                        face == 0,
                        face_layer(&source, face),
                        face_layer(&emissive_source, face),
                    )?;
                    if let Some(e) = measured {
                        baked = e;
                    }
                }
                if retain {
                    let faces = probe_store::read_slot(gpu, &source, 0)?.swap_remove(0);
                    self.captured.borrow_mut().push(Some(faces));
                }
                r.ibl().prefilter_probe(gpu, &source, &array, slot);
                let sees_emissive = probe_store::read_slot(gpu, &emissive_source, 0)?[0]
                    .iter()
                    .any(|&b| b != 0);
                if sees_emissive {
                    let target = emissive_slot(&emissive_array);
                    r.ibl()
                        .prefilter_probe(gpu, &emissive_source, &target, slot);
                    emissive_array = Some(target);
                }
                // The prefilter has no error return of its own: a validation fault it raised is
                // pending here, and must not reach the resident cache or the store.
                if let Some(fault) = gpu.take_fault() {
                    return Err(fault);
                }
                emissive_slots.push(sees_emissive);
                if let Some((dir, key)) = entry {
                    let levels = probe_store::read_slot(gpu, &array, slot)?;
                    let emissive = emissive_array
                        .as_ref()
                        .filter(|_| sees_emissive)
                        .map(|target| probe_store::read_slot(gpu, target, slot))
                        .transpose()?;
                    // The emissive prefilter and these readbacks may have faulted since the check
                    // above; nothing bad reaches the content-addressed store.
                    if let Some(fault) = gpu.take_fault() {
                        return Err(fault);
                    }
                    probe_store::write_entry(
                        dir,
                        key,
                        face_size,
                        &levels,
                        emissive.as_deref(),
                        baked,
                    );
                }
                Ok::<(), Diagnostic>(())
            });
            if let Err(fault) = captured {
                // Retained captures are all-or-nothing, like the resident cache.
                self.captured.borrow_mut().clear();
                return Err(fault);
            }
            irradiance.push(baked);
            resolved.push(ResolvedProbe {
                entity: probe.entity,
                key: entry.map(|(_, key)| key),
                loaded: false,
            });
        }
        if !hits.is_empty() {
            *self.stamping.borrow_mut() = probe_store::stamp_used(hits);
        }
        {
            let mut report = self.report.borrow_mut();
            report.resolve += 1;
            report.probes = resolved;
        }
        let view = array.create_view(&wgpu::TextureViewDescriptor {
            label: Some("probe cube array"),
            dimension: Some(wgpu::TextureViewDimension::CubeArray),
            ..Default::default()
        });
        let emissive = emissive_array.map(|texture| {
            let view = texture.create_view(&wgpu::TextureViewDescriptor {
                label: Some("probe emissive cube array"),
                dimension: Some(wgpu::TextureViewDimension::CubeArray),
                ..Default::default()
            });
            (texture, view)
        });
        let cubes = ProbeCubes {
            max_lod: array.mip_level_count() - 1,
            emissive,
            emissive_slots,
            texture: array,
            view,
            slots: (0..probes.len() as u32).collect(),
            irradiance,
            face_size,
        };
        *self.cache.borrow_mut() = Some((key, cubes));
        Ok(())
    }

    /// The resident captures, or `None` for a scene without probes.
    pub(crate) fn cubes(&self) -> Option<ProbeCubes> {
        self.cache.borrow().as_ref().map(|(_, cubes)| cubes.clone())
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::ibl::tests::{face_dir, read_rgba16_mips, test_env};
    use crate::probe::{ProbeData, ProbeVolume};
    use crate::scene::{MeshInstance, MeshShape, SunData};
    use ludion_core::math::glam::{Mat3, Quat};

    #[test]
    fn a_face_scene_carries_no_viewmodel() {
        let mut scene = RenderScene::empty((64, 64));
        let world = mesh(MeshShape::Cube, Mat4::IDENTITY, [1.0; 4], [1.0, 0.0, 1.0]);
        let mut gun = world;
        gun.viewmodel = true;
        scene.meshes = vec![world, gun];
        scene.viewmodel = Some(crate::viewmodel::ViewmodelData {
            fov_y_deg: 55.0,
            near: 0.01,
            depth_slice: 0.01,
            projection: Mat4::IDENTITY,
            locals: Vec::new(),
            particles: Vec::new(),
        });
        let face = face_scene(&scene, Vec3::ZERO, 0);
        assert_eq!(face.meshes.len(), 1);
        assert!(face.meshes.iter().all(|m| !m.viewmodel));
        assert!(face.viewmodel.is_none());
        let key = |scene: &RenderScene| {
            probe_store::world_key(
                &face_scene(scene, Vec3::ZERO, 0),
                &crate::quality::Quality::FULL,
                &crate::quality::QualityParams::MEDIUM,
                0,
                16,
            )
        };
        let mut without = scene.clone();
        without.meshes.pop();
        without.viewmodel = None;
        assert_eq!(key(&scene), key(&without));
    }

    /// Every face's camera maps the cube direction `face_dir` gives to the mirrored NDC point
    /// (−u, −v), which `probe_face.wgsl`'s x flip turns into the cube texel (u, v).
    #[test]
    fn capture_faces_match_the_cube_convention() {
        let position = Vec3::new(3.0, -2.0, 5.0);
        for face in 0..6 {
            let camera = face_camera(position, face);
            let clip_from_world = camera.projection * camera.view;
            for (u, v) in [
                (0.0, 0.0),
                (0.5, 0.5),
                (-0.5, 0.5),
                (0.5, -0.5),
                (-0.5, -0.5),
            ] {
                let dir = Vec3::from(face_dir(face, u, v));
                let clip = clip_from_world * (position + dir).extend(1.0);
                let ndc = clip.truncate() / clip.w;
                assert!(clip.w > 0.0, "face {face} ({u}, {v}) is behind the camera");
                assert!(
                    (ndc.x + u).abs() < 1e-4 && (ndc.y + v).abs() < 1e-4,
                    "face {face} ({u}, {v}): ndc {ndc:?}"
                );
            }
        }
    }

    pub(crate) fn probe_at(entity: u32, position: Vec3) -> ProbeData {
        ProbeData {
            entity: EntityId::new(entity, 0),
            position,
            rotation: Quat::IDENTITY,
            volume: ProbeVolume::Sphere { radius: 5.0 },
            blend: 0.5,
            priority: 0,
        }
    }

    pub(crate) fn mesh(
        shape: MeshShape,
        model: Mat4,
        color: [f32; 4],
        material: [f32; 3],
    ) -> MeshInstance {
        MeshInstance {
            medium: 1.0,
            shape,
            model,
            normal_matrix: Mat3::IDENTITY,
            color,
            material,
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

    pub(crate) fn renderer_or_skip(what: &str) -> Option<Renderer> {
        match Renderer::new() {
            Ok(r) => Some(r),
            Err(d) if ludion_core::require_gpu::is_no_adapter(std::slice::from_ref(&d)) => {
                eprintln!("SKIPPED {what}: no GPU adapter");
                None
            }
            Err(d) => panic!("renderer init failed: {d:?}"),
        }
    }

    /// The array's bytes, every mip and layer.
    pub(crate) fn array_bytes(r: &Renderer) -> Vec<f32> {
        let cubes = r.probes.cubes().expect("the scene has a probe");
        read_rgba16_mips(r.gpu(), &cubes.texture)
            .into_iter()
            .flatten()
            .collect()
    }

    pub(crate) fn resolve(r: &Renderer, scene: &RenderScene) {
        r.probes.resolve(r, scene, &mut None).unwrap();
    }

    pub(crate) fn lit_scene() -> RenderScene {
        let mut scene = RenderScene::empty((64, 64));
        scene.environment = Some(test_env());
        let dir = Vec3::new(-0.4, -1.0, -0.3).normalize();
        scene.sun = Some(SunData {
            direction: dir,
            color: Vec3::splat(2.0),
            light_space: Mat4::orthographic_rh(-20.0, 20.0, -20.0, 20.0, 1.0, 40.0)
                * Mat4::look_to_rh(-dir * 20.0, dir, Vec3::Y),
            bias: 0.002,
            angular_radius: 0.0,
            cascades: None,
        });
        scene.meshes = vec![
            mesh(
                MeshShape::Plane,
                Mat4::from_scale(Vec3::new(30.0, 1.0, 30.0)),
                [0.6, 0.6, 0.6, 1.0],
                [0.2, 0.0, 0.0],
            ),
            mesh(
                MeshShape::Cube,
                Mat4::from_translation(Vec3::new(3.0, 1.0, 0.0))
                    * Mat4::from_scale(Vec3::splat(2.0)),
                [0.9, 0.1, 0.1, 1.0],
                [0.6, 0.0, 0.0],
            ),
            mesh(
                MeshShape::Cube,
                Mat4::from_translation(Vec3::new(-2.0, 1.0, 3.0))
                    * Mat4::from_scale(Vec3::splat(2.0)),
                [0.1, 0.8, 0.2, 1.0],
                [0.6, 0.0, 0.0],
            ),
        ];
        scene.probes = vec![probe_at(1, Vec3::new(0.0, 1.5, 0.0))];
        scene
    }

    #[test]
    fn needs_gpu_probe_capture_is_deterministic() {
        let (Some(a), Some(b)) = (
            renderer_or_skip("probe capture determinism"),
            renderer_or_skip("probe capture determinism"),
        ) else {
            return;
        };
        let scene = lit_scene();
        resolve(&a, &scene);
        resolve(&b, &scene);
        let (bytes_a, bytes_b) = (array_bytes(&a), array_bytes(&b));
        assert!(bytes_a.iter().any(|&v| v > 0.0), "the capture is empty");
        assert!(bytes_a.iter().all(|v| v.is_finite()));
        assert!(
            bytes_a == bytes_b,
            "two renderers captured different probe bytes"
        );
    }

    /// With no geometry the probe sees only the sky: every mip of slot 0 holds rgb 0 and alpha 1,
    /// because the capture stores geometry only and the sky comes from the live sky cube.
    #[test]
    fn needs_gpu_a_sky_only_probe_stores_no_radiance() {
        let Some(r) = renderer_or_skip("the sky-only probe") else {
            return;
        };
        let mut scene = RenderScene::empty((64, 64));
        scene.environment = Some(test_env());
        scene.probes = vec![probe_at(1, Vec3::ZERO)];
        resolve(&r, &scene);
        let cubes = r.probes.cubes().expect("captured");
        for (mip, texels) in read_rgba16_mips(r.gpu(), &cubes.texture).iter().enumerate() {
            for (i, texel) in texels.chunks_exact(4).enumerate() {
                assert!(
                    texel[..3] == [0.0; 3] && (texel[3] - 1.0).abs() < 1e-3,
                    "mip {mip} texel {i}: {texel:?}"
                );
            }
        }
    }

    /// Boxes on ±X and ±Z show on their own faces with alpha 0; the faces toward empty space are
    /// all sky.
    #[test]
    fn needs_gpu_probe_faces_land_on_their_axes() {
        let Some(r) = renderer_or_skip("probe faces land on their axes") else {
            return;
        };
        let mut scene = RenderScene::empty((64, 64));
        let emissive = [1.0, 0.0, 4.0];
        let at = |p: Vec3, color: [f32; 4]| {
            mesh(
                MeshShape::Cube,
                Mat4::from_translation(p) * Mat4::from_scale(Vec3::splat(2.0)),
                color,
                emissive,
            )
        };
        scene.meshes = vec![
            at(Vec3::new(4.0, 0.0, 0.0), [1.0, 0.0, 0.0, 1.0]),
            at(Vec3::new(-4.0, 0.0, 0.0), [0.0, 1.0, 0.0, 1.0]),
            at(Vec3::new(0.0, 0.0, 4.0), [0.0, 0.0, 1.0, 1.0]),
            at(Vec3::new(0.0, 0.0, -4.0), [1.0, 1.0, 0.0, 1.0]),
        ];
        scene.probes = vec![probe_at(1, Vec3::ZERO)];
        resolve(&r, &scene);
        let cubes = r.probes.cubes().expect("captured");
        let mip0 = &read_rgba16_mips(r.gpu(), &cubes.texture)[0];
        let edge = cubes.face_size as usize;
        let centre = |face: usize| -> [f32; 4] {
            let at = ((face * edge + edge / 2) * edge + edge / 2) * 4;
            std::array::from_fn(|c| mip0[at + c])
        };
        // face order: +X red, −X green, +Y sky, −Y sky, +Z blue, −Z yellow
        let dominant = |t: [f32; 4], channels: [bool; 3]| {
            let on = (0..3)
                .filter(|&c| channels[c])
                .map(|c| t[c])
                .fold(f32::MAX, f32::min);
            let off = (0..3)
                .filter(|&c| !channels[c])
                .map(|c| t[c])
                .fold(0.0, f32::max);
            on > 4.0 * off.max(1e-3)
        };
        for (face, channels) in [
            (0, [true, false, false]),
            (1, [false, true, false]),
            (4, [false, false, true]),
            (5, [true, true, false]),
        ] {
            let t = centre(face);
            assert_eq!(t[3], 0.0, "face {face} sees a box: {t:?}");
            assert!(
                dominant(t, channels),
                "face {face} has the wrong colour: {t:?}"
            );
        }
        for face in [2, 3] {
            assert_eq!(centre(face)[3], 1.0, "face {face} sees only sky");
        }
    }

    /// A face copied into a cube smaller than the face is a GPU validation fault: the capture
    /// returns it as `L2010` instead of panicking the sim thread.
    #[test]
    fn needs_gpu_a_faulting_capture_is_an_error_not_a_panic() {
        let Some(r) = renderer_or_skip("probe capture fault") else {
            return;
        };
        let scene = lit_scene();
        let face_size = r.ibl_specular_size();
        let small = ibl::source_cube(r.gpu().device(), face_size / 2);
        let derived = face_scene(capture_world(&scene), Vec3::new(0.0, 1.5, 0.0), 0);
        let err = r
            .capture_probe_face(
                &derived,
                face_size,
                true,
                face_layer(&small, 0),
                face_layer(&small, 0),
            )
            .expect_err("the copy overruns the destination");
        assert_eq!(err.code.as_str(), "L2010", "{err:?}");
        assert!(
            r.probes.cubes().is_none(),
            "a failed capture caches nothing"
        );
    }

    /// `prefilter_probe` has no error return: a slot past the array is a validation fault the
    /// caller must find through the readback or `take_fault` before storing anything. Neither path
    /// panics, and the fault surfaces as `L2010`.
    #[test]
    fn needs_gpu_a_faulting_prefilter_surfaces_before_any_store() {
        let Some(r) = renderer_or_skip("probe prefilter fault") else {
            return;
        };
        let face_size = r.ibl_specular_size();
        let source = ibl::source_cube(r.gpu().device(), face_size);
        let array = ibl::probe_array(r.gpu().device(), face_size, 1);
        r.ibl().prefilter_probe(r.gpu(), &source, &array, 3);
        let fault = probe_store::read_slot(r.gpu(), &array, 0)
            .err()
            .or_else(|| r.gpu().take_fault())
            .expect("the out-of-range slot is reported, not swallowed");
        assert_eq!(fault.code.as_str(), "L2010", "{fault:?}");
    }

    #[test]
    fn needs_gpu_read_probes_returns_every_slot() {
        let (Some(mut stored), Some(dumped), Some(fresh)) = (
            renderer_or_skip("probe readback"),
            renderer_or_skip("probe readback"),
            renderer_or_skip("probe readback"),
        ) else {
            return;
        };
        let mut scene = lit_scene();
        scene.probes = vec![
            probe_at(1, Vec3::new(0.0, 1.5, 0.0)),
            probe_at(2, Vec3::new(-4.0, 1.5, 2.0)),
        ];
        let dir =
            std::env::temp_dir().join(format!("ludion-probe-readback-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        stored.set_probe_store(Some(dir.clone()));
        resolve(&stored, &scene);
        let recorded: Vec<_> = stored
            .probe_resolve()
            .probes
            .iter()
            .map(|p| p.key.expect("a store-backed resolve records keys"))
            .collect();

        let readbacks = dumped.read_probes(&scene, None).unwrap();
        assert_eq!(readbacks.len(), 2);
        let max_lod = dumped.probes.cubes().expect("resident").max_lod;
        for (slot, probe) in readbacks.iter().enumerate() {
            let edge = probe.face_size as usize;
            assert_eq!(probe.slot, slot as u32);
            assert_eq!(probe.entity, scene.probes[slot].entity);
            assert_eq!(probe.key, recorded[slot]);
            assert_eq!(
                probe.captured.as_ref().expect("fresh").len(),
                6 * edge * edge * 4
            );
            assert_eq!(probe.levels.len() as u32, max_lod + 1);
        }
        assert_ne!(readbacks[0].key, readbacks[1].key);
        let again = dumped.read_probes(&scene, None).unwrap();
        for (a, b) in readbacks.iter().zip(&again) {
            assert!(
                a.captured == b.captured && a.levels == b.levels,
                "a second readback differs"
            );
        }

        let size = crate::FrameSize::native(64, 64);
        let after = dumped.render(&scene, size).expect("renders");
        let direct = fresh.render(&scene, size).expect("renders");
        assert!(
            after.rgba == direct.rgba,
            "a render after a dump differs from a fresh one"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn needs_gpu_no_probes_leaves_no_cubes() {
        let Some(r) = renderer_or_skip("no probes") else {
            return;
        };
        let mut scene = lit_scene();
        scene.probes.clear();
        resolve(&r, &scene);
        assert!(r.probes.cubes().is_none());
        scene.probes = vec![probe_at(1, Vec3::new(0.0, 1.5, 0.0))];
        resolve(&r, &scene);
        assert!(r.probes.cubes().is_some());
        scene.probes.clear();
        resolve(&r, &scene);
        assert!(r.probes.cubes().is_none());
    }

    fn nudge(scene: &mut RenderScene) {
        scene.meshes[1].model *= Mat4::from_translation(Vec3::new(0.0, 0.0, 2.0));
    }

    #[test]
    fn needs_gpu_the_resident_cubes_follow_the_store_key() {
        let Some(mut r) = renderer_or_skip("the resident cubes") else {
            return;
        };
        let mut scene = lit_scene();
        resolve(&r, &scene);
        let count = |r: &Renderer| r.probe_resolve().resolve;
        assert_eq!(count(&r), 1);
        resolve(&r, &scene);
        assert_eq!(count(&r), 1, "the same scene resolved again");

        let key = r.probe_keys(&scene);
        nudge(&mut scene);
        assert_ne!(r.probe_keys(&scene), key);
        resolve(&r, &scene);
        assert_eq!(count(&r), 2, "a moved mesh");

        let key = r.probe_keys(&scene);
        r.set_quality(
            Quality {
                shadows: false,
                ..r.quality()
            },
            r.quality_params(),
        );
        assert_ne!(r.probe_keys(&scene), key);
        resolve(&r, &scene);
        assert_eq!(count(&r), 3, "a quality toggle");

        let key = r.probe_keys(&scene);
        let edited = format!("{}\n// edit\n", include_str!("shaders/mesh.wgsl"));
        r.reload_shader("mesh.wgsl", edited).unwrap();
        assert_ne!(r.probe_keys(&scene), key);
        resolve(&r, &scene);
        assert_eq!(count(&r), 4, "a shader reload");
    }

    #[test]
    fn needs_gpu_a_probe_world_freezes_the_capture() {
        let Some(r) = renderer_or_skip("the probe world") else {
            return;
        };
        let mut scene = lit_scene();
        scene.probe_world = Some(ProbeWorld::new(scene.clone()));
        resolve(&r, &scene);
        let frozen = array_bytes(&r);
        let key = r.probe_keys(&scene);
        assert_eq!(r.probe_resolve().resolve, 1);

        nudge(&mut scene);
        resolve(&r, &scene);
        assert_eq!(r.probe_resolve().resolve, 1, "a live mesh edit recaptured");
        assert!(array_bytes(&r) == frozen);
        assert_eq!(r.probe_keys(&scene), key);

        scene.probes[0].position.x += 1.0;
        resolve(&r, &scene);
        assert_eq!(r.probe_resolve().resolve, 2, "a moved probe");
    }

    /// A probe recapture reads the frozen world, which names neither the sprite texture nor the decal
    /// map the live scene holds; it must leave both resident, so the next main frame uploads nothing.
    #[test]
    fn needs_gpu_a_recapture_uploads_nothing_the_main_frame_holds() {
        use crate::scene::{
            DecalBlend, DecalInstance, DecalMaps, DecalReceivers, MapImage, MeshTexture,
            SpriteInstance, TextureImage, TextureWrap,
        };
        use std::sync::Arc;
        let Some(r) = renderer_or_skip("the recapture's caches") else {
            return;
        };
        let image = |hash: u64| {
            Arc::new(TextureImage {
                width: 1,
                height: 1,
                rgba: vec![200, 100, 50, 255],
                hash,
                filter_linear: false,
                premultiplied_on_load: false,
                mean: [1.0; 4],
            })
        };
        let mut scene = lit_scene();
        scene.probe_world = Some(ProbeWorld::new(scene.clone()));
        scene.textures = vec![image(0x5E1)];
        scene.sprites = vec![SpriteInstance {
            center: Vec3::new(0.0, 1.0, -4.0),
            right: Vec3::new(2.0, 0.0, 0.0),
            up: Vec3::new(0.0, 2.0, 0.0),
            color: [1.0; 4],
            texture: Some(0),
            uv: SpriteInstance::FULL_UV,
            lit: false,
            layer: 0,
            additive: false,
            flash: 0.0,
            flash_color: [1.0; 3],
            uv_repeat: SpriteInstance::NO_TILING,
            uv_offset: [0.0; 2],
        }];
        let box_to_world = Mat4::from_scale(Vec3::splat(4.0));
        scene.decals = vec![DecalInstance {
            box_to_world,
            world_to_box: box_to_world.inverse(),
            blend: DecalBlend::Alpha,
            color: [1.0; 4],
            opacity: 1.0,
            roughness: None,
            metallic: 0.0,
            uv_rect: [0.0, 0.0, 1.0, 1.0],
            angle_fade: [60.0, 80.0],
            layer: 0,
            maps: DecalMaps {
                base_color: Some(MeshTexture {
                    image: MapImage::Source(image(0xDECA)),
                    wrap: TextureWrap::Clamp,
                }),
                ..DecalMaps::default()
            },
            receivers: DecalReceivers::Exclude(Vec::new()),
        }];
        let size = crate::FrameSize::native(64, 64);

        r.render(&scene, size).unwrap();
        let held = r.uploads();
        assert!(held.textures >= 1 && held.mesh_maps >= 1, "{held:?}");
        assert_eq!(r.probe_resolve().resolve, 1);

        scene.probes[0].position.x += 1.0;
        r.render(&scene, size).unwrap();
        assert_eq!(r.probe_resolve().resolve, 2, "the probe was not recaptured");
        assert_eq!(
            r.uploads(),
            held,
            "a recapture evicted what the main frame holds"
        );
    }

    #[test]
    fn needs_gpu_post_does_not_reach_the_capture() {
        let (Some(a), Some(b)) = (
            renderer_or_skip("post and the capture"),
            renderer_or_skip("post and the capture"),
        ) else {
            return;
        };
        let plain = lit_scene();
        let mut graded = lit_scene();
        graded.post = Some(crate::scene::PostData {
            exposure: 2.5,
            fog_density: 0.3,
            bloom_strength: 0.9,
            ..Default::default()
        });
        assert_eq!(a.probe_keys(&plain), b.probe_keys(&graded));
        resolve(&a, &plain);
        resolve(&b, &graded);
        assert!(array_bytes(&a) == array_bytes(&b));
    }
}
