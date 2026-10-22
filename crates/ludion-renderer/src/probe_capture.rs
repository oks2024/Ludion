//! Reflection-probe capture: six world-axis faces per probe, rendered as ordinary frames of a
//! derived scene and prefiltered into one slot of a cube array (`Plan/reflections.md` decisions 16,
//! 17 and 21). The `reflections` pass samples the array (`probe_shade.rs`).

use std::cell::{Cell, RefCell};
use std::f32::consts::FRAC_PI_2;
use std::path::PathBuf;

use ludion_core::math::glam::{Mat4, Vec3};

use crate::frame_observer::{FrameObserver, FramePhase};
use crate::ibl;
use crate::probe::{MAX_PROBES, probe_rank};
use crate::probe_store;
use crate::render::Renderer;
use crate::scene::{RenderCamera, RenderScene};
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
/// caches key on stay, so the capture hits every cache the main frame filled.
pub(crate) fn face_scene(scene: &RenderScene, position: Vec3, face: u32) -> RenderScene {
    let mut out = scene.clone();
    out.camera = face_camera(position, face);
    out.sprites.clear();
    out.particles.clear();
    out.texts.clear();
    out.ground_markers.clear();
    out.ui.clear();
    out.debug_lines.clear();
    out.probes.clear();
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

/// The captured, prefiltered probes: a cube array with one slot per probe.
#[derive(Clone)]
pub(crate) struct ProbeCubes {
    pub(crate) texture: wgpu::Texture,
    /// A cube-array view of every layer and mip.
    pub(crate) view: wgpu::TextureView,
    /// Probe index → slot. The identity today; the table is what lets residency change addressing.
    pub(crate) slots: Vec<u32>,
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

/// The renderer's probe captures.
pub(crate) struct Probes {
    /// Keyed by the probe set (face size, entity, position): the world is captured once, and a
    /// change to the meshes or the sky does not recapture.
    cache: RefCell<Option<(u64, ProbeCubes)>>,
    /// The capture target the faces are copied into, kept for the face size it was built for.
    source: RefCell<Option<(u32, wgpu::Texture)>>,
    /// The probe store directory, when the host set one.
    store: RefCell<Option<PathBuf>>,
    report: RefCell<ProbeResolve>,
    /// Set while a readback runs: each captured probe's raw faces are kept before the shared
    /// source cube is overwritten by the next one.
    retain: Cell<bool>,
    /// One entry per slot of the retained resolve; `None` for a slot the store supplied.
    captured: RefCell<Vec<Option<Vec<u8>>>>,
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

fn f16_texels(bytes: &[u8]) -> Vec<f32> {
    bytes
        .chunks_exact(2)
        .map(|b| crate::gi_bake::f16_to_f32(u16::from_le_bytes([b[0], b[1]])))
        .collect()
}

/// The store key of each probe of `scene` at `r`'s quality: the world key once, then the capture
/// point. The one place keys are computed, so a dump can never disagree with a host.
fn store_keys(r: &Renderer, scene: &RenderScene, face_size: u32) -> Vec<u64> {
    let world = probe_store::world_key(
        &face_scene(scene, Vec3::ZERO, 0),
        &r.quality_params(),
        face_size,
    );
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
    pub fn read_probes(
        &self,
        scene: &RenderScene,
        mut observer: Option<&mut dyn FrameObserver>,
    ) -> Vec<ProbeReadback> {
        let count = scene.probes.len().min(MAX_PROBES);
        if count == 0 {
            return Vec::new();
        }
        let probes = &self.probes;
        probes.retain.set(true);
        probes.recapture();
        probes.resolve(self, scene, &mut observer);
        probes.retain.set(false);
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
            .map(|slot| ProbeReadback {
                entity: scene.probes[slot].entity,
                slot: slot as u32,
                rank: rank_of[slot],
                key: keys[slot],
                face_size: cubes.face_size,
                captured: captured
                    .get(slot)
                    .and_then(|bytes| bytes.as_deref())
                    .map(f16_texels),
                levels: probe_store::read_slot(self.gpu(), &cubes.texture, slot as u32)
                    .iter()
                    .map(|level| f16_texels(level))
                    .collect(),
            })
            .collect()
    }
}

impl Probes {
    pub(crate) fn new() -> Probes {
        Probes {
            cache: RefCell::new(None),
            source: RefCell::new(None),
            store: RefCell::new(None),
            report: RefCell::new(ProbeResolve::default()),
            retain: Cell::new(false),
            captured: RefCell::new(Vec::new()),
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

    fn key(probes: &[crate::probe::ProbeData], face_size: u32) -> u64 {
        let mut bytes = Vec::with_capacity(4 + probes.len() * 20);
        bytes.extend_from_slice(&face_size.to_le_bytes());
        for probe in probes {
            bytes.extend_from_slice(&probe.entity.index().to_le_bytes());
            bytes.extend_from_slice(&probe.entity.generation().to_le_bytes());
            for c in probe.position.to_array() {
                bytes.extend_from_slice(&c.to_le_bytes());
            }
        }
        ludion_assets::content_hash(&bytes)
    }

    /// Captures and prefilters `scene`'s probes unless the probe set is the one already resident.
    /// An `observer` brackets each captured probe as `ProbeBake`; loaded probes are not bracketed.
    /// A scene with no probes frees the array and does no GPU work.
    pub(crate) fn resolve(
        &self,
        r: &Renderer,
        scene: &RenderScene,
        observer: &mut Option<&mut dyn FrameObserver>,
    ) {
        debug_assert!(scene.probes.len() <= MAX_PROBES, "13 caps the probe list");
        let probes = &scene.probes[..scene.probes.len().min(MAX_PROBES)];
        if probes.is_empty() {
            self.cache.borrow_mut().take();
            self.report.borrow_mut().probes.clear();
            return;
        }
        let face_size = r.ibl_specular_size();
        let key = Self::key(probes, face_size);
        if self
            .cache
            .borrow()
            .as_ref()
            .is_some_and(|(cached, _)| *cached == key)
        {
            return;
        }

        let gpu = r.gpu();
        let array = ibl::probe_array(gpu.device(), face_size, probes.len() as u32);
        let source = {
            let mut held = self.source.borrow_mut();
            if held.as_ref().is_none_or(|(size, _)| *size != face_size) {
                *held = Some((face_size, ibl::source_cube(gpu.device(), face_size)));
            }
            held.as_ref().expect("just filled").1.clone()
        };
        // The world key is taken once per resolve: a load-time event, the world at frame 0.
        let store = self.store.borrow().clone();
        let keys = store.as_ref().map(|_| store_keys(r, scene, face_size));
        let retain = self.retain.get();
        if retain {
            self.captured.borrow_mut().clear();
        }
        let mut resolved = Vec::with_capacity(probes.len());
        // No borrow of the cache is held below: a face's own `prepare_frame` resolves its
        // probe-less scene back through here.
        for (slot, probe) in probes.iter().enumerate() {
            let slot = slot as u32;
            let entry = store
                .as_deref()
                .zip(keys.as_ref())
                .map(|(dir, keys)| (dir, keys[slot as usize]));
            let loaded = entry.and_then(|(dir, key)| {
                probe_store::read_entry(dir, key, face_size, array.mip_level_count())
            });
            if let Some(levels) = loaded {
                probe_store::write_slot(gpu, &array, slot, &levels);
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
            r.observed_phase(observer, FramePhase::ProbeBake, || {
                for face in 0..6 {
                    let derived = face_scene(scene, probe.position, face);
                    r.capture_probe_face(
                        &derived,
                        face_size,
                        wgpu::TexelCopyTextureInfo {
                            texture: &source,
                            mip_level: 0,
                            origin: wgpu::Origin3d {
                                x: 0,
                                y: 0,
                                z: face,
                            },
                            aspect: wgpu::TextureAspect::All,
                        },
                    );
                }
                if retain {
                    let faces = probe_store::read_slot(gpu, &source, 0).swap_remove(0);
                    self.captured.borrow_mut().push(Some(faces));
                }
                r.ibl().prefilter_probe(gpu, &source, &array, slot);
                if let Some((dir, key)) = entry {
                    let levels = probe_store::read_slot(gpu, &array, slot);
                    probe_store::write_entry(dir, key, face_size, &levels);
                }
            });
            resolved.push(ResolvedProbe {
                entity: probe.entity,
                key: entry.map(|(_, key)| key),
                loaded: false,
            });
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
        let cubes = ProbeCubes {
            max_lod: array.mip_level_count() - 1,
            texture: array,
            view,
            slots: (0..probes.len() as u32).collect(),
            face_size,
        };
        *self.cache.borrow_mut() = Some((key, cubes));
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
            bone_offset: 0,
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
        r.probes.resolve(r, scene, &mut None);
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

    /// With no geometry the probe sees only the sky: every mip of slot 0 matches the sky cube
    /// within the bound `ibl`'s sky-filled-source test records, and alpha is 1.
    #[test]
    fn needs_gpu_a_sky_only_probe_matches_the_sky_cube() {
        let Some(r) = renderer_or_skip("the sky-only probe") else {
            return;
        };
        let mut scene = RenderScene::empty((64, 64));
        scene.environment = Some(test_env());
        scene.probes = vec![probe_at(1, Vec3::ZERO)];
        resolve(&r, &scene);
        let cubes = r.probes.cubes().expect("captured");
        let probe_mips = read_rgba16_mips(r.gpu(), &cubes.texture);
        let sky = r
            .ibl()
            .sky_specular_texture(r.gpu(), &test_env(), cubes.face_size);
        let sky_mips = read_rgba16_mips(r.gpu(), &sky);
        assert_eq!(sky_mips.len(), probe_mips.len());
        for (mip, (sky, probe)) in sky_mips.iter().zip(&probe_mips).enumerate() {
            let mut errors = Vec::new();
            for (i, (&want, &got)) in sky.iter().zip(probe).enumerate() {
                if i % 4 == 3 {
                    assert!((got - 1.0).abs() < 1e-3, "mip {mip} texel {i}: alpha {got}");
                } else {
                    errors.push((got - want).abs() / want.abs().max(1e-3));
                }
            }
            errors.sort_by(f32::total_cmp);
            let (median, worst) = (errors[errors.len() / 2], errors[errors.len() - 1]);
            let (median_bound, worst_bound) = if mip == 0 { (0.01, 0.05) } else { (0.06, 0.35) };
            assert!(
                median <= median_bound && worst <= worst_bound,
                "mip {mip}: median {median}, worst {worst} vs the sky cube"
            );
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

        let readbacks = dumped.read_probes(&scene, None);
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
        let again = dumped.read_probes(&scene, None);
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
}
