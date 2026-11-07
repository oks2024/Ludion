//! The probe store: a content key for a probe's prefiltered cube, the KTX2 entry it is kept in, and
//! the slot readback and upload around it (`Plan/reflections.md` decisions 19 and 22).
//!
//! The key hashes exactly what `probe_capture`'s capture reads: the derived face scene (so a sprite,
//! the sun disc, the camera or the composite's `post` never moves it), the quality toggles and
//! parameters, the renderer's shader code key, the face size and [`PROBE_BAKE_VERSION`], plus the
//! probe's capture point. Shape, blend and priority shade the probe; they are not captured, so they
//! are not in the key.
//!
//! The adapter is not a key term, deliberately: `ludion export` bakes on the dev machine's GPU and
//! ships the entries, and the device looks them up by key, so an adapter term would make every
//! shipped entry miss. The bytes differ across adapters only perceptually, and `--probes rebake` is
//! the escape.

use std::fmt::Debug;
use std::fs::File;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use ludion_assets::content_hash;
use ludion_assets::cooked::{ProbeCube, cooked_file_name, read_probe_cube, write_probe_cube};
use ludion_core::diagnostics::Diagnostic;
use ludion_core::math::glam::Vec3;
use ludion_render_graph::Gpu;

use crate::quality::{Quality, QualityParams};
use crate::scene::{MeshTexture, RenderScene};

/// Bump whenever the capture or the prefilter produces different bytes for the same input; every
/// stored entry is then orphaned. `ludion-engine`'s `probe_bake_pin` test fails when the capture
/// moves without this changing: bump it when that pin moves, then re-bless the pin. 4: the capture
/// holds geometry only (sky rgb 0) and the entry carries the baked irradiance the live relight
/// divides by. 5: an emissive layer the relight leaves unscaled. 6: the baked irradiance is per
/// axis. 7: the pin's fixture gained one dominant term per face; capture code is unchanged.
pub const PROBE_BAKE_VERSION: u32 = 7;

const TEXEL_BYTES: u32 = 8;

/// What the key writer feeds the hasher: fixed-width little-endian numbers, length-prefixed strings.
struct KeyWriter(Vec<u8>);

impl KeyWriter {
    fn u32(&mut self, v: u32) {
        self.0.extend_from_slice(&v.to_le_bytes());
    }

    fn u64(&mut self, v: u64) {
        self.0.extend_from_slice(&v.to_le_bytes());
    }

    fn f32(&mut self, v: f32) {
        self.0.extend_from_slice(&v.to_le_bytes());
    }

    fn str(&mut self, s: &str) {
        self.u64(s.len() as u64);
        self.0.extend_from_slice(s.as_bytes());
    }

    /// A value through `Debug`: every field is included, so a new one enters the key unasked, and
    /// `f32` prints shortest round-trip, which is deterministic. `-0.0` prints differently from
    /// `0.0`; the cost is a spurious miss and one recapture, so it is not normalised.
    fn debug(&mut self, v: &impl Debug) {
        self.str(&format!("{v:?}"));
    }

    fn map(&mut self, map: Option<&MeshTexture>) {
        match map {
            Some(map) => {
                self.u32(1);
                self.u64(map.image.identity());
                self.debug(&map.wrap);
            }
            None => self.u32(0),
        }
    }
}

/// The key of everything a probe capture of `face_scene` reads except the capture point.
///
/// `face_scene` is the derived scene (`probe_capture::face_scene`), never the raw one, so what the
/// capture strips cannot move the key. The destructure below names every `RenderScene` field with
/// no `..`: a new field is a compile error until it is hashed or its exclusion is argued.
///
/// Small `Copy` structs are hashed through `Debug`; heavy data (meshes, maps, the voxel grid)
/// through its content key.
pub(crate) fn world_key(
    face_scene: &RenderScene,
    quality: &Quality,
    params: &QualityParams,
    code: u64,
    face: u32,
) -> u64 {
    let RenderScene {
        // per face; the capture point enters through `probe_key`
        camera: _,
        ambient,
        // only sprites, particles, text and the UI index it, and the face scene has none
        textures: _,
        mesh_assets,
        meshes,
        // the lists below are stripped from the face scene
        sprites: _,
        ground_markers: _,
        drop_shadows: _,
        decals: _,
        debug_lines: _,
        debug_view: _,
        particles: _,
        texts: _,
        ui: _,
        probes: _,
        // the capture reads the frozen world, never a nested one
        probe_world: _,
        lights,
        spot_lights,
        sun,
        environment,
        // the capture graph has no composite, and the face scene zeroes the strengths that gate the
        // screen-space chains, so no pass of it reads `post`
        post: _,
        voxel,
        gi,
        bone_palette,
        // CPU-only lens data, always `None` here: `face_scene` drops the gun and its lens
        viewmodel: _,
    } = face_scene;

    let mut w = KeyWriter(Vec::new());
    w.u32(PROBE_BAKE_VERSION);
    w.u32(face);
    w.u64(code);
    w.debug(quality);
    w.debug(params);
    w.debug(ambient);
    for asset in mesh_assets {
        w.u64(asset.hash);
        match &asset.textures {
            Some(textures) => {
                w.u32(1);
                for map in [
                    &textures.base_color,
                    &textures.metallic_roughness,
                    &textures.normal,
                    &textures.occlusion,
                    &textures.emissive,
                ] {
                    w.map(map.as_ref());
                }
                w.f32(textures.normal_scale);
                w.f32(textures.occlusion_strength);
            }
            None => w.u32(0),
        }
    }
    w.debug(meshes);
    w.debug(lights);
    w.debug(spot_lights);
    w.debug(sun);
    w.debug(environment);
    match voxel {
        Some(grid) => {
            w.u32(1);
            w.u64(grid.hash);
        }
        None => w.u32(0),
    }
    w.debug(gi);
    w.debug(bone_palette);
    content_hash(&w.0)
}

/// A probe's store key: the world key and its capture point. The capture is world-axis-aligned, so
/// the rotation is not an input.
pub(crate) fn probe_key(world: u64, position: Vec3) -> u64 {
    let mut bytes = Vec::with_capacity(20);
    bytes.extend_from_slice(&world.to_le_bytes());
    for c in position.to_array() {
        bytes.extend_from_slice(&c.to_le_bytes());
    }
    content_hash(&bytes)
}

/// The entry path of `key` under `dir`.
pub(crate) fn entry_path(dir: &Path, key: u64) -> PathBuf {
    dir.join(cooked_file_name(key))
}

/// The stored cube for `key`, or `None` for a missing, unreadable or invalid entry (a miss).
pub(crate) fn read_entry(dir: &Path, key: u64, face: u32, level_count: u32) -> Option<ProbeCube> {
    let bytes = std::fs::read(entry_path(dir, key)).ok()?;
    read_probe_cube(&bytes, face, level_count).ok()
}

/// Writes the entry atomically (a temp file, then a rename). A failure is dropped: the store is a
/// cache and the slot is already correct.
pub(crate) fn write_entry(
    dir: &Path,
    key: u64,
    face: u32,
    levels: &[Vec<u8>],
    emissive: Option<&[Vec<u8>]>,
    irradiance: [[f32; 3]; 6],
) {
    let path = entry_path(dir, key);
    let temp = path.with_extension(format!("{}.tmp", std::process::id()));
    let write = || -> std::io::Result<()> {
        std::fs::create_dir_all(dir)?;
        std::fs::write(&temp, write_probe_cube(face, levels, emissive, irradiance))?;
        std::fs::rename(&temp, &path)
    };
    if write().is_err() {
        let _ = std::fs::remove_file(&temp);
    }
}

/// Stamps `paths` as used now, on a short-lived named thread: a store hit lands on the sim thread
/// inside a frame, which reads no clock and waits on no stamp I/O. Best-effort and silent: a failed
/// spawn, a read-only store or a process that exits first only loses a stamp, which means an earlier
/// eviction and one recapture.
pub(crate) fn stamp_used(paths: Vec<PathBuf>) -> Option<std::thread::JoinHandle<()>> {
    std::thread::Builder::new()
        .name("probe-store-stamp".to_owned())
        .spawn(move || {
            // The clock is read here and nowhere else: the stamp is a cache-eviction hint and no
            // time reaches a pixel, the sim or a frame.
            #[allow(clippy::disallowed_methods)]
            let now = SystemTime::now();
            for path in paths {
                let _ = File::options()
                    .write(true)
                    .open(&path)
                    .and_then(|file| file.set_modified(now));
            }
        })
        .ok()
}

/// What [`prune_store`] removed and what the store holds afterwards.
#[derive(Debug, Default, PartialEq, Eq, Clone, Copy)]
pub struct ProbePrune {
    /// Entries removed.
    pub entries: usize,
    /// Dead temp files removed.
    pub temps: usize,
    /// Bytes those removals freed.
    pub bytes: u64,
    /// Bytes the store's entries and temps hold after the prune.
    pub store_bytes: u64,
    /// The store still exceeds the cap: everything left was used recently or belongs to this warm.
    pub over_cap: bool,
}

struct Limits {
    unused_ttl: Duration,
    recent_floor: Duration,
    cap: u64,
    temp_ttl: Duration,
}

impl Limits {
    const DEFAULT: Limits = Limits {
        unused_ttl: UNUSED_TTL,
        recent_floor: RECENT_FLOOR,
        cap: STORE_CAP,
        temp_ttl: TEMP_TTL,
    };
}

/// An entry unused this long is removed.
const UNUSED_TTL: Duration = Duration::from_secs(14 * 24 * 3600);
/// The store's size cap; the oldest entries go first once it is exceeded.
const STORE_CAP: u64 = 4 << 30;
/// An entry used within this window is never removed to meet the cap.
const RECENT_FLOOR: Duration = Duration::from_secs(24 * 3600);
/// A temp file older than this is the debris of a dead writer.
const TEMP_TTL: Duration = Duration::from_secs(3600);

/// Bounds the store at `dir` by last-used age (an entry's mtime: a write sets it, a store hit
/// refreshes it): entries unused past `UNUSED_TTL` go, then the oldest until the store fits
/// `STORE_CAP`, never one used within `RECENT_FLOOR`. An entry whose key is in `keep` (what the
/// caller just resolved) is never removed. A temp older than `TEMP_TTL` goes; a live writer's is
/// fresh. Any other name is left alone. A cache: failures are dropped.
pub fn prune_store(dir: &Path, keep: &[u64], now: SystemTime) -> ProbePrune {
    prune_with(dir, keep, now, &Limits::DEFAULT)
}

struct Held {
    path: PathBuf,
    name: String,
    key: u64,
    size: u64,
    age: Duration,
}

fn prune_with(dir: &Path, keep: &[u64], now: SystemTime, limits: &Limits) -> ProbePrune {
    let mut report = ProbePrune::default();
    let Ok(read) = std::fs::read_dir(dir) else {
        return report;
    };
    let mut entries: Vec<Held> = Vec::new();
    for item in read.flatten() {
        let Ok(meta) = item.metadata() else { continue };
        if !meta.is_file() {
            continue;
        }
        let name = item.file_name().to_string_lossy().into_owned();
        let size = meta.len();
        let Ok(modified) = meta.modified() else {
            report.store_bytes += size;
            continue;
        };
        // A future mtime (a clock step) reads as just used.
        let age = now.duration_since(modified).unwrap_or(Duration::ZERO);
        if is_temp_name(&name) {
            if age > limits.temp_ttl && std::fs::remove_file(item.path()).is_ok() {
                report.temps += 1;
                report.bytes += size;
            } else {
                report.store_bytes += size;
            }
        } else if let Some(key) = entry_key(&name) {
            entries.push(Held {
                path: item.path(),
                name,
                key,
                size,
                age,
            });
        }
    }
    let removable = |held: &Held| !keep.contains(&held.key);
    let mut survivors = Vec::new();
    for held in entries {
        if held.age > limits.unused_ttl
            && removable(&held)
            && std::fs::remove_file(&held.path).is_ok()
        {
            report.entries += 1;
            report.bytes += held.size;
        } else {
            survivors.push(held);
        }
    }
    evict_over_cap(&mut survivors, &mut report, limits, removable);
    report.store_bytes += survivors.iter().map(|held| held.size).sum::<u64>();
    report.over_cap = report.store_bytes > limits.cap;
    report
}

/// Removes the oldest of `held` that are removable and not recent until the store fits the cap, or
/// nothing eligible is left. `report.store_bytes` carries the temps' bytes on entry.
fn evict_over_cap(
    held: &mut Vec<Held>,
    report: &mut ProbePrune,
    limits: &Limits,
    removable: impl Fn(&Held) -> bool,
) {
    let mut total = report.store_bytes + held.iter().map(|h| h.size).sum::<u64>();
    if total <= limits.cap {
        return;
    }
    held.sort_by(|a, b| b.age.cmp(&a.age).then_with(|| a.name.cmp(&b.name)));
    let mut kept = Vec::with_capacity(held.len());
    for entry in held.drain(..) {
        if total > limits.cap
            && entry.age >= limits.recent_floor
            && removable(&entry)
            && std::fs::remove_file(&entry.path).is_ok()
        {
            total -= entry.size;
            report.entries += 1;
            report.bytes += entry.size;
        } else {
            kept.push(entry);
        }
    }
    *held = kept;
}

/// `<16 lowercase hex>` and the rest of the name, when the name starts that way.
fn hex_stem(name: &str) -> Option<(u64, &str)> {
    let (stem, rest) = name.split_at_checked(16)?;
    stem.bytes()
        .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        .then(|| u64::from_str_radix(stem, 16).ok().map(|key| (key, rest)))?
}

/// The key of an entry named exactly `<16 hex>.ktx2`.
fn entry_key(name: &str) -> Option<u64> {
    hex_stem(name).and_then(|(key, rest)| (rest == ".ktx2").then_some(key))
}

/// A writer's temp: exactly `<16 hex>.<digits>.tmp`.
fn is_temp_name(name: &str) -> bool {
    hex_stem(name).is_some_and(|(_, rest)| {
        rest.strip_prefix('.')
            .and_then(|r| r.strip_suffix(".tmp"))
            .is_some_and(|pid| !pid.is_empty() && pid.bytes().all(|b| b.is_ascii_digit()))
    })
}

/// One slot's six layers of every mip as raw `Rgba16Float` bytes: per mip, the faces back to back
/// with tight rows (the layout of a stored entry).
///
/// # Errors
/// A GPU fault (`L2010` / `L2011`), or `L2005`-class failure to poll or map the readback: a fault
/// the device reported is the cause and wins.
pub(crate) fn read_slot(
    gpu: &Gpu,
    array: &wgpu::Texture,
    slot: u32,
) -> Result<Vec<Vec<u8>>, Diagnostic> {
    let device = gpu.device();
    (0..array.mip_level_count())
        .map(|mip| {
            let edge = (array.width() >> mip).max(1);
            let tight = edge * TEXEL_BYTES;
            let row = tight.div_ceil(256) * 256;
            let buffer = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("probe slot readback"),
                size: u64::from(row * edge * 6),
                usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                mapped_at_creation: false,
            });
            let mut encoder =
                device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
            encoder.copy_texture_to_buffer(
                slot_copy(array, slot, mip),
                wgpu::TexelCopyBufferInfo {
                    buffer: &buffer,
                    layout: wgpu::TexelCopyBufferLayout {
                        offset: 0,
                        bytes_per_row: Some(row),
                        rows_per_image: Some(edge),
                    },
                },
                wgpu::Extent3d {
                    width: edge,
                    height: edge,
                    depth_or_array_layers: 6,
                },
            );
            gpu.queue().submit(Some(encoder.finish()));
            let (tx, rx) = std::sync::mpsc::channel();
            buffer.slice(..).map_async(wgpu::MapMode::Read, move |r| {
                let _ = tx.send(r);
            });
            let poll = device.poll(wgpu::PollType::wait_indefinitely());
            let mapped = match poll {
                Ok(_) => rx
                    .recv()
                    .map_err(|e| e.to_string())
                    .and_then(|r| r.map_err(|e| e.to_string()))
                    .and_then(|()| {
                        buffer
                            .slice(..)
                            .get_mapped_range()
                            .map_err(|e| e.to_string())
                    }),
                Err(e) => Err(e.to_string()),
            };
            let mapped = mapped.map_err(|reason| {
                gpu.take_fault().unwrap_or_else(|| {
                    Diagnostic::new(
                        ludion_core::codes::code::RENDER_GPU_ERROR,
                        format!("the probe slot readback failed: {reason}"),
                    )
                })
            })?;
            let mut out = Vec::with_capacity((tight * edge * 6) as usize);
            for rows in mapped.chunks_exact(row as usize) {
                out.extend_from_slice(&rows[..tight as usize]);
            }
            drop(mapped);
            buffer.unmap();
            Ok(out)
        })
        .collect()
}

/// Uploads `levels` (as [`read_slot`] returns them) into `slot`'s six layers.
pub(crate) fn write_slot(gpu: &Gpu, array: &wgpu::Texture, slot: u32, levels: &[Vec<u8>]) {
    for (mip, data) in levels.iter().enumerate() {
        let edge = (array.width() >> mip).max(1);
        gpu.queue().write_texture(
            slot_copy(array, slot, mip as u32),
            data,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(edge * TEXEL_BYTES),
                rows_per_image: Some(edge),
            },
            wgpu::Extent3d {
                width: edge,
                height: edge,
                depth_or_array_layers: 6,
            },
        );
    }
}

fn slot_copy(array: &wgpu::Texture, slot: u32, mip: u32) -> wgpu::TexelCopyTextureInfo<'_> {
    wgpu::TexelCopyTextureInfo {
        texture: array,
        mip_level: mip,
        origin: wgpu::Origin3d {
            x: 0,
            y: 0,
            z: slot * 6,
        },
        aspect: wgpu::TextureAspect::All,
    }
}

#[cfg(test)]
#[allow(clippy::disallowed_methods)] // tests set and read mtimes on the wall clock
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::probe::{ProbeData, ProbeVolume};
    use crate::probe_capture::face_scene;
    use crate::probe_capture::tests::{array_bytes, lit_scene, probe_at, resolve};
    use crate::render::Renderer;
    use crate::scene::{
        EnvironmentData, GiData, MapImage, MeshData, MeshTextures, PointLightData, PostData,
        SpotLightData, SpriteInstance, TextureImage, TextureWrap, TonemapOperator, WorldVoxelGrid,
    };
    use ludion_core::math::glam::{Mat4, Quat};

    struct Fixture {
        scene: RenderScene,
        toggles: Quality,
        quality: QualityParams,
        code: u64,
        face: u32,
        position: Vec3,
    }

    fn source_map(hash: u64) -> MeshTexture {
        MeshTexture {
            image: MapImage::Source(Arc::new(TextureImage {
                width: 1,
                height: 1,
                rgba: vec![255; 4],
                hash,
                filter_linear: false,
                premultiplied_on_load: false,
                mean: [1.0; 4],
            })),
            wrap: TextureWrap::Repeat,
        }
    }

    fn asset(hash: u64) -> MeshData {
        MeshData {
            positions: vec![0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0],
            normals: vec![0.0, 0.0, 1.0, 0.0, 0.0, 1.0, 0.0, 0.0, 1.0],
            colors: vec![1.0; 12],
            materials: vec![1.0, 0.0, 0.0, 1.0, 0.0, 0.0, 1.0, 0.0, 0.0],
            uvs: vec![0.0; 6],
            tangents: vec![1.0, 0.0, 0.0, 1.0, 1.0, 0.0, 0.0, 1.0, 1.0, 0.0, 0.0, 1.0],
            weights: vec![1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0],
            bone_indices: vec![0; 12],
            skin_joints: Vec::new(),
            textures: Some(MeshTextures {
                base_color: Some(source_map(5)),
                metallic_roughness: None,
                normal: None,
                occlusion: None,
                emissive: None,
                normal_scale: 1.0,
                occlusion_strength: 1.0,
                alpha_cutoff: None,
            }),
            authored_material: false,
            base_color_mean: [1.0; 4],
            double_sided: false,
            indices: vec![0, 1, 2],
            hash,
        }
    }

    fn fixture() -> Fixture {
        let mut scene = lit_scene();
        scene.probes.clear();
        scene.mesh_assets = vec![Arc::new(asset(11))];
        scene.lights = vec![PointLightData {
            position: Vec3::new(1.0, 2.0, 3.0),
            color: Vec3::ONE,
            intensity: 5.0,
            range: 8.0,
            dynamic: false,
            shadow: None,
        }];
        scene.spot_lights = vec![SpotLightData {
            position: Vec3::new(0.0, 4.0, 0.0),
            direction: Vec3::NEG_Y,
            color: Vec3::ONE,
            intensity: 5.0,
            range: 9.0,
            cos_inner: 0.9,
            cos_outer: 0.8,
            shadow: None,
        }];
        scene.voxel = Some(Arc::new(
            WorldVoxelGrid {
                origin: Vec3::ZERO,
                cell_size: 1.0,
                extent: [1, 1, 1],
                occupancy: vec![true],
                albedo: vec![[1.0; 4]],
                emissive: vec![0.0],
                hash: 0,
            }
            .sealed(),
        ));
        scene.gi = Some(GiData {
            probe_spacing: 1.0,
            normal_bias: 0.1,
            sky_visibility: 1.0,
            bounce: 0.5,
            count: [1, 1, 1],
            origin: Vec3::ZERO,
            span: Vec3::ONE,
        });
        scene.bone_palette = vec![Mat4::IDENTITY];
        scene.post = Some(PostData::default());
        Fixture {
            scene,
            toggles: Quality::FULL,
            quality: QualityParams::MEDIUM,
            code: 7,
            face: 64,
            position: Vec3::new(0.0, 1.5, 0.0),
        }
    }

    /// The key as `Probes::resolve` computes it: through the face scene.
    fn key_of(f: &Fixture) -> u64 {
        let world = world_key(
            &face_scene(&f.scene, Vec3::ZERO, 0),
            &f.toggles,
            &f.quality,
            f.code,
            f.face,
        );
        probe_key(world, f.position)
    }

    #[test]
    fn the_same_input_gives_the_same_key() {
        assert_eq!(key_of(&fixture()), key_of(&fixture()));
    }

    #[test]
    fn each_capture_input_moves_the_key() {
        type Case = (&'static str, fn(&mut Fixture));
        let cases: &[Case] = &[
            ("mesh transform", |f| {
                f.scene.meshes[1].model *= Mat4::from_translation(Vec3::X)
            }),
            ("mesh colour", |f| f.scene.meshes[1].color[0] = 0.5),
            ("mesh material", |f| f.scene.meshes[1].material[0] = 0.9),
            ("mesh wrap", |f| f.scene.meshes[1].wrap = 0.5),
            ("mesh data hash", |f| {
                let mut mesh = (*f.scene.mesh_assets[0]).clone();
                mesh.hash += 1;
                f.scene.mesh_assets[0] = Arc::new(mesh);
            }),
            ("map identity", |f| {
                let mut mesh = (*f.scene.mesh_assets[0]).clone();
                let textures = mesh.textures.as_mut().unwrap();
                textures.base_color.as_mut().unwrap().image =
                    MapImage::Cooked(Arc::new(crate::scene::CookedMap {
                        key: 6,
                        image: ludion_assets::cooked::CompressedImage {
                            format: ludion_assets::cooked::CompressedFormat::Bc7,
                            srgb: true,
                            width: 4,
                            height: 4,
                            levels: vec![vec![0; 16]],
                            mean: [1.0; 4],
                        },
                    }));
                f.scene.mesh_assets[0] = Arc::new(mesh);
            }),
            ("map wrap", |f| {
                let mut mesh = (*f.scene.mesh_assets[0]).clone();
                mesh.textures
                    .as_mut()
                    .unwrap()
                    .base_color
                    .as_mut()
                    .unwrap()
                    .wrap = TextureWrap::Clamp;
                f.scene.mesh_assets[0] = Arc::new(mesh);
            }),
            ("point light", |f| f.scene.lights[0].intensity = 6.0),
            ("spot light", |f| f.scene.spot_lights[0].range = 10.0),
            ("sun direction", |f| {
                f.scene.sun.as_mut().unwrap().direction = Vec3::NEG_Y
            }),
            ("sky colour", |f| {
                f.scene.environment.as_mut().unwrap().sky_zenith[0] = 0.9
            }),
            ("ambient", |f| f.scene.ambient[0] += 0.1),
            ("gi", |f| f.scene.gi.as_mut().unwrap().bounce = 0.9),
            ("voxel hash", |f| {
                let mut grid = (**f.scene.voxel.as_ref().unwrap()).clone();
                grid.albedo[0] = [0.5; 4];
                f.scene.voxel = Some(Arc::new(grid.sealed()));
            }),
            ("bone palette", |f| {
                f.scene.bone_palette[0] = Mat4::from_translation(Vec3::Y)
            }),
            ("quality", |f| f.quality = QualityParams::LOW),
            ("shadows toggle", |f| f.toggles.shadows = false),
            ("dynamic gi toggle", |f| f.toggles.gi_dynamic = false),
            ("shader code", |f| f.code += 1),
            ("face size", |f| f.face = 128),
            ("position", |f| f.position.x += 1.0),
        ];
        let base = key_of(&fixture());
        let mut seen = vec![base];
        for (name, mutate) in cases {
            let mut f = fixture();
            mutate(&mut f);
            let key = key_of(&f);
            assert!(!seen.contains(&key), "{name} did not move the key");
            seen.push(key);
        }
    }

    #[test]
    fn non_capture_inputs_leave_the_key() {
        let base = key_of(&fixture());
        type Case = (&'static str, fn(&mut Fixture));
        let cases: &[Case] = &[
            ("a sprite", |f| {
                f.scene.sprites.push(SpriteInstance {
                    center: Vec3::ZERO,
                    right: Vec3::X,
                    up: Vec3::Y,
                    color: [1.0; 4],
                    texture: None,
                    uv: SpriteInstance::FULL_UV,
                    lit: true,
                    layer: 0,
                    additive: false,
                    flash: 0.0,
                    flash_color: [1.0; 3],
                    uv_repeat: SpriteInstance::NO_TILING,
                    uv_offset: [0.0; 2],
                })
            }),
            ("the sun disc", |f| {
                let env: &mut EnvironmentData = f.scene.environment.as_mut().unwrap();
                env.sun_disc_size = 0.05;
                env.sun_disc_intensity = 10.0;
            }),
            ("ssr", |f| f.scene.post.as_mut().unwrap().ssr = 0.7),
            ("post", |f| {
                let post = f.scene.post.as_mut().unwrap();
                post.exposure = 2.0;
                post.bloom_strength = 0.9;
                post.tonemap = TonemapOperator::Aces;
                post.fog_density = 0.2;
            }),
            ("the camera", |f| {
                f.scene.camera.position += Vec3::new(5.0, 1.0, 2.0)
            }),
            ("probe priority, blend and shape", |f| {
                f.scene.probes = vec![ProbeData {
                    entity: ludion_core::ids::EntityId::new(3, 0),
                    position: f.position,
                    rotation: Quat::from_rotation_y(0.5),
                    volume: ProbeVolume::Box {
                        extent: Vec3::splat(4.0),
                    },
                    blend: 2.0,
                    priority: 9,
                }];
            }),
        ];
        for (name, mutate) in cases {
            let mut f = fixture();
            mutate(&mut f);
            assert_eq!(key_of(&f), base, "{name} moved the key");
        }
    }

    fn temp_store(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("ludion-probe-store-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    fn two_probe_scene() -> RenderScene {
        let mut scene = lit_scene();
        scene.probes = vec![
            probe_at(1, Vec3::new(0.0, 1.5, 0.0)),
            probe_at(2, Vec3::new(-5.0, 1.0, -4.0)),
        ];
        scene
    }

    fn with_store(dir: Option<&PathBuf>, what: &str) -> Option<Renderer> {
        let mut r = match Renderer::new() {
            Ok(r) => r,
            Err(d) if ludion_core::require_gpu::is_no_adapter(std::slice::from_ref(&d)) => {
                eprintln!("SKIPPED {what}: no GPU adapter");
                return None;
            }
            Err(d) => panic!("renderer init failed: {d:?}"),
        };
        r.set_probe_store(dir.cloned());
        Some(r)
    }

    /// (loaded, captured) of the renderer's last resolve.
    fn loaded_captured(r: &Renderer) -> (usize, usize) {
        let probes = r.probe_resolve().probes;
        let loaded = probes.iter().filter(|p| p.loaded).count();
        (loaded, probes.len() - loaded)
    }

    fn entries(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(dir)
            .map(|rd| {
                rd.map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
                    .collect()
            })
            .unwrap_or_default();
        names.sort();
        names
    }

    const DAY: Duration = Duration::from_secs(24 * 3600);

    fn hex(key: u64) -> String {
        format!("{key:016x}")
    }

    /// Writes `name` of `size` bytes, last used `age` before `now`.
    fn put(dir: &Path, name: &str, size: usize, now: SystemTime, age: Duration) {
        std::fs::create_dir_all(dir).unwrap();
        let path = dir.join(name);
        std::fs::write(&path, vec![0u8; size]).unwrap();
        File::options()
            .write(true)
            .open(&path)
            .unwrap()
            .set_modified(now - age)
            .unwrap();
    }

    fn present(dir: &Path) -> Vec<String> {
        entries(dir)
    }

    fn limits(cap: u64) -> Limits {
        Limits {
            unused_ttl: 14 * DAY,
            recent_floor: DAY,
            cap,
            temp_ttl: Duration::from_secs(3600),
        }
    }

    #[test]
    fn prune_removes_the_old_and_unreferenced() {
        let dir = temp_store("prune-old");
        let now = SystemTime::now();
        put(&dir, &format!("{}.ktx2", hex(1)), 10, now, 15 * DAY);
        put(&dir, &format!("{}.ktx2", hex(2)), 10, now, 13 * DAY);
        let report = prune_store(&dir, &[], now);
        assert_eq!(present(&dir), [format!("{}.ktx2", hex(2))]);
        assert_eq!(
            (report.entries, report.bytes, report.store_bytes),
            (1, 10, 10)
        );
        assert!(!report.over_cap);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn this_warms_key_survives_at_any_age_and_over_the_cap() {
        let dir = temp_store("prune-keep");
        let now = SystemTime::now();
        put(&dir, &format!("{}.ktx2", hex(1)), 100, now, 400 * DAY);
        put(&dir, &format!("{}.ktx2", hex(2)), 100, now, 300 * DAY);
        let report = prune_with(&dir, &[1], now, &limits(50));
        assert_eq!(present(&dir), [format!("{}.ktx2", hex(1))]);
        assert_eq!(report.entries, 1);
        assert!(report.over_cap, "the kept entry alone exceeds the cap");
        assert_eq!(report.store_bytes, 100);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_recent_entry_not_in_keep_survives() {
        let dir = temp_store("prune-recent");
        let now = SystemTime::now();
        put(&dir, &format!("{}.ktx2", hex(3)), 10, now, 2 * DAY);
        prune_store(&dir, &[], now);
        assert_eq!(present(&dir).len(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_future_mtime_and_foreign_names_are_kept() {
        let dir = temp_store("prune-foreign");
        let now = SystemTime::now();
        put(&dir, &format!("{}.ktx2", hex(4)), 10, now, Duration::ZERO);
        File::options()
            .write(true)
            .open(dir.join(format!("{}.ktx2", hex(4))))
            .unwrap()
            .set_modified(now + 5 * DAY)
            .unwrap();
        for name in ["notes.ktx2", "junk.tmp", ".x.tmp", "ABCDEF0123456789.ktx2"] {
            put(&dir, name, 10, now, 900 * DAY);
        }
        prune_store(&dir, &[], now);
        assert_eq!(present(&dir).len(), 5, "{:?}", present(&dir));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_live_writers_temp_survives_and_an_aged_one_goes() {
        let dir = temp_store("prune-temp");
        let now = SystemTime::now();
        let fresh = format!("{}.123.tmp", hex(5));
        let aged = format!("{}.456.tmp", hex(6));
        put(&dir, &fresh, 10, now, Duration::from_secs(60));
        put(&dir, &aged, 10, now, Duration::from_secs(2 * 3600));
        let report = prune_store(&dir, &[], now);
        assert_eq!(present(&dir), [fresh]);
        assert_eq!((report.temps, report.entries, report.bytes), (1, 0, 10));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_cap_evicts_oldest_first() {
        let dir = temp_store("prune-cap");
        let now = SystemTime::now();
        for (key, days) in [(1, 10), (2, 8), (3, 6), (4, 4)] {
            put(&dir, &format!("{}.ktx2", hex(key)), 100, now, days * DAY);
        }
        let report = prune_with(&dir, &[], now, &limits(250));
        assert_eq!(
            present(&dir),
            [format!("{}.ktx2", hex(3)), format!("{}.ktx2", hex(4))]
        );
        assert_eq!((report.entries, report.store_bytes), (2, 200));
        assert!(!report.over_cap);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_entry_under_24_hours_survives_over_the_cap() {
        let dir = temp_store("prune-floor");
        let now = SystemTime::now();
        put(&dir, &format!("{}.ktx2", hex(1)), 100, now, 5 * DAY);
        put(&dir, &format!("{}.ktx2", hex(2)), 100, now, DAY / 2);
        put(&dir, &format!("{}.ktx2", hex(3)), 100, now, DAY / 4);
        let report = prune_with(&dir, &[], now, &limits(100));
        assert_eq!(present(&dir).len(), 2);
        assert!(report.over_cap);
        assert_eq!(report.store_bytes, 200);
        assert_eq!(report.entries, 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_missing_store_prunes_nothing() {
        let dir = temp_store("prune-missing");
        assert_eq!(
            prune_store(&dir, &[], SystemTime::now()),
            ProbePrune::default()
        );
    }

    #[test]
    fn stamping_refreshes_the_mtime_and_tolerates_bad_paths() {
        let dir = temp_store("stamp");
        let now = SystemTime::now();
        let name = format!("{}.ktx2", hex(9));
        put(&dir, &name, 10, now, 30 * DAY);
        let path = dir.join(&name);
        let readonly = dir.join("ro.ktx2");
        put(&dir, "ro.ktx2", 1, now, 30 * DAY);
        let mut permissions = std::fs::metadata(&readonly).unwrap().permissions();
        permissions.set_readonly(true);
        std::fs::set_permissions(&readonly, permissions).unwrap();
        let handle = stamp_used(vec![
            dir.join("absent.ktx2"),
            readonly.clone(),
            path.clone(),
        ])
        .unwrap();
        handle.join().unwrap();
        let age = SystemTime::now()
            .duration_since(std::fs::metadata(&path).unwrap().modified().unwrap())
            .unwrap_or_default();
        assert!(age < DAY, "the hit was not stamped: {age:?}");
        let mut permissions = std::fs::metadata(&readonly).unwrap().permissions();
        #[allow(clippy::permissions_set_readonly_false)]
        permissions.set_readonly(false);
        std::fs::set_permissions(&readonly, permissions).unwrap();
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn needs_gpu_a_store_miss_then_hit_gives_identical_bytes() {
        let dir = temp_store("hit");
        let (Some(a), Some(b), Some(c)) = (
            with_store(Some(&dir), "store miss then hit"),
            with_store(Some(&dir), "store miss then hit"),
            with_store(None, "store miss then hit"),
        ) else {
            return;
        };
        let scene = two_probe_scene();
        resolve(&a, &scene);
        assert_eq!(loaded_captured(&a), (0, 2));
        let names = entries(&dir);
        assert_eq!(names.len(), 2, "{names:?}");
        assert!(names.iter().all(|n| n.ends_with(".ktx2")), "{names:?}");

        let old = SystemTime::now() - 30 * DAY;
        for name in &names {
            File::options()
                .write(true)
                .open(dir.join(name))
                .unwrap()
                .set_modified(old)
                .unwrap();
        }
        resolve(&b, &scene);
        assert_eq!(loaded_captured(&b), (2, 0));
        let stamping = b.probes.stamping.borrow_mut().take().expect("a hit stamps");
        stamping.join().unwrap();
        for name in &names {
            let modified = std::fs::metadata(dir.join(name))
                .unwrap()
                .modified()
                .unwrap();
            assert!(modified > old + DAY, "{name} was not stamped on a hit");
        }
        resolve(&c, &scene);
        assert_eq!(loaded_captured(&c), (0, 2));
        let (bytes_a, bytes_b, bytes_c) = (array_bytes(&a), array_bytes(&b), array_bytes(&c));
        assert!(bytes_a.iter().any(|&v| v > 0.0), "the capture is empty");
        assert!(bytes_a == bytes_b, "a store hit differs from the miss");
        assert!(bytes_a == bytes_c, "a store-less capture differs");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn needs_gpu_a_corrupt_entry_is_recaptured() {
        let dir = temp_store("corrupt");
        let (Some(a), Some(b)) = (
            with_store(Some(&dir), "corrupt entry"),
            with_store(Some(&dir), "corrupt entry"),
        ) else {
            return;
        };
        let scene = two_probe_scene();
        resolve(&a, &scene);
        let name = entries(&dir).remove(0);
        let path = dir.join(&name);
        let original = std::fs::read(&path).unwrap();
        std::fs::write(&path, &original[..original.len() / 2]).unwrap();

        resolve(&b, &scene);
        assert_eq!(loaded_captured(&b), (1, 1));
        assert_eq!(std::fs::read(&path).unwrap(), original);
        assert!(array_bytes(&a) == array_bytes(&b));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn needs_gpu_a_changed_world_misses() {
        let dir = temp_store("changed");
        let (Some(a), Some(b)) = (
            with_store(Some(&dir), "changed world"),
            with_store(Some(&dir), "changed world"),
        ) else {
            return;
        };
        let mut scene = two_probe_scene();
        resolve(&a, &scene);
        let before = entries(&dir);
        scene.meshes[1].model *= Mat4::from_translation(Vec3::new(0.0, 0.0, 2.0));
        resolve(&b, &scene);
        assert_eq!(loaded_captured(&b), (0, 2));
        let after = entries(&dir);
        assert_eq!(after.len(), 4, "{after:?}");
        assert!(before.iter().all(|n| after.contains(n)));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
