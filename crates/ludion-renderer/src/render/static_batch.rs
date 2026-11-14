//! The scene's statics, kept on the GPU between frames (`Plan/gpu-driven-culling.md` decision 49).
//!
//! The leading `scene.statics.count` meshes never move on their own, so their packed instances and
//! cull items live in two persistent buffers, ordered as the frame packs its world draws (bucket,
//! then scene index). Each frame the static runs are GPU-copied into the bucket-leading slots of the
//! frame's packed buffers ([`StaticCopy`]) and only the dynamics are written from the CPU, so the
//! frame layout, the cull and every direct draw are the ones a scene without statics has. A quiet
//! frame costs one key compare; a moved static is patched slot-wise; a membership or bucket change
//! rebuilds.

use std::ops::Range;

use bytemuck::Pod;

use super::mesh_batch::{MeshInstanceRaw, instance_raw, is_mirrored};
use super::{MeshAssetGpu, init_buffer};
use crate::cull::{AssetBounds, instance_bounds, instance_lod_radii};
use crate::gpu_cull::cull_item;
use crate::scene::{MeshInstance, MeshShape, RenderScene};
use crate::shader_gen::CullItem;

const RAW_BYTES: u64 = std::mem::size_of::<MeshInstanceRaw>() as u64;
const ITEM_BYTES: u64 = std::mem::size_of::<CullItem>() as u64;

/// The bucket a static run lands in: a primitive shape or an asset index, per handedness.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum RunKey {
    Prim { mirrored: bool, shape: MeshShape },
    Asset { mirrored: bool, index: usize },
}

impl RunKey {
    /// The packing order of the world draws: primitives (unmirrored, then mirrored; cube, plane,
    /// sphere), then assets (unmirrored, then mirrored; by asset index).
    pub(super) fn rank(self) -> (u8, bool, usize) {
        match self {
            RunKey::Prim { mirrored, shape } => (
                0,
                mirrored,
                match shape {
                    MeshShape::Cube => 0,
                    MeshShape::Plane => 1,
                    MeshShape::Sphere | MeshShape::Asset(_) => 2,
                },
            ),
            RunKey::Asset { mirrored, index } => (1, mirrored, index),
        }
    }
}

/// One bucket's statics: `count` consecutive slots from `first` of the static buffers.
#[derive(Clone, PartialEq, Debug)]
pub(super) struct StaticRun {
    pub(super) key: RunKey,
    pub(super) first: u32,
    pub(super) count: u32,
    /// The ascending `scene.meshes` index of each slot.
    pub(super) sources: Vec<u32>,
    /// How many of the run cast shadows.
    pub(super) casters: u32,
}

/// A draw's static run: what the batch and the cull read of it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) struct StaticSlice {
    /// The run's index in [`StaticGpu`], which is also its rank among the frame's static buckets.
    pub(crate) run: u32,
    pub(crate) first: u32,
    pub(crate) count: u32,
    pub(crate) casters: u32,
}

/// A GPU copy the frame submits before its own passes.
pub(crate) struct StaticCopy {
    pub(crate) src: wgpu::Buffer,
    pub(crate) src_offset: u64,
    pub(crate) dst: wgpu::Buffer,
    pub(crate) dst_offset: u64,
    pub(crate) size: u64,
}

impl StaticCopy {
    /// The instance copy of `run` into slot `dst_first` of the frame's packed instances.
    pub(super) fn instances(
        src: &wgpu::Buffer,
        run: &StaticRun,
        dst: &wgpu::Buffer,
        dst_first: u32,
    ) -> StaticCopy {
        StaticCopy {
            src: src.clone(),
            src_offset: u64::from(run.first) * RAW_BYTES,
            dst: dst.clone(),
            dst_offset: u64::from(dst_first) * RAW_BYTES,
            size: u64::from(run.count) * RAW_BYTES,
        }
    }

    /// The cull-item copy of `slice` into slot `dst_first` of the frame's item buffer.
    pub(crate) fn items(
        src: &wgpu::Buffer,
        slice: StaticSlice,
        dst: &wgpu::Buffer,
        dst_first: u32,
    ) -> StaticCopy {
        StaticCopy {
            src: src.clone(),
            src_offset: u64::from(slice.first) * ITEM_BYTES,
            dst: dst.clone(),
            dst_offset: u64::from(dst_first) * ITEM_BYTES,
            size: u64::from(slice.count) * ITEM_BYTES,
        }
    }

    /// Records the copies into one encoder; `None` when there are none.
    pub(super) fn encode(
        device: &wgpu::Device,
        copies: &[StaticCopy],
    ) -> Option<wgpu::CommandBuffer> {
        if copies.is_empty() {
            return None;
        }
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("ludion static copies"),
        });
        for c in copies {
            encoder.copy_buffer_to_buffer(&c.src, c.src_offset, &c.dst, c.dst_offset, c.size);
        }
        Some(encoder.finish())
    }
}

/// What the last [`StaticGpu::update`] and [`StaticGpu::remap`] did, for tests.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub(crate) enum StaticUpdate {
    /// The static key was unchanged: nothing was built or written.
    #[default]
    Quiet,
    /// The layout was unchanged: this many slots were rewritten in place.
    Patched { slots: u32 },
    /// Membership or a bucket changed: both buffers were recreated.
    Rebuilt,
    /// Only the frame's bucket numbering moved: the cull items were rewritten.
    Remapped,
}

/// The persistent static buffers and their CPU mirrors.
pub(super) struct StaticGpu {
    key: u64,
    count: usize,
    runs: Vec<StaticRun>,
    raws: Vec<MeshInstanceRaw>,
    items: Vec<CullItem>,
    instances: wgpu::Buffer,
    items_buf: wgpu::Buffer,
    /// The frame bucket id stamped into each run's items; empty until the first [`remap`](Self::remap).
    buckets: Vec<u32>,
}

/// The runs, packed instances and cull items of `statics`, in frame packing order. An out-of-range
/// asset, or an asset with nothing to draw, is dropped exactly as the dynamic batch drops it.
fn build(
    statics: &[MeshInstance],
    assets: &[MeshAssetGpu],
) -> (Vec<StaticRun>, Vec<MeshInstanceRaw>, Vec<CullItem>) {
    let asset_bounds: Vec<AssetBounds> = assets.iter().map(|a| a.bounds.clone()).collect();
    let bounds = instance_bounds(statics, &asset_bounds, &[]);
    let radii = instance_lod_radii(statics, &asset_bounds);
    let mut order: Vec<(RunKey, usize)> = statics
        .iter()
        .enumerate()
        .filter_map(|(i, m)| {
            debug_assert!(!m.viewmodel && m.bone_offset == 0, "an ineligible static");
            let mirrored = is_mirrored(&m.model);
            let key = match m.shape {
                MeshShape::Asset(index) => {
                    let a = assets.get(index)?;
                    if a.vertex_count == 0 || a.index_count == 0 {
                        return None;
                    }
                    RunKey::Asset { mirrored, index }
                }
                shape => RunKey::Prim { mirrored, shape },
            };
            Some((key, i))
        })
        .collect();
    // Stable: scene order inside a run.
    order.sort_by_key(|&(key, _)| key.rank());
    let mut runs: Vec<StaticRun> = Vec::new();
    let mut raws = Vec::with_capacity(order.len());
    let mut items = Vec::with_capacity(order.len());
    for (key, i) in order {
        let m = &statics[i];
        if runs.last().is_none_or(|r| r.key != key) {
            runs.push(StaticRun {
                key,
                first: raws.len() as u32,
                count: 0,
                sources: Vec::new(),
                casters: 0,
            });
        }
        let run = runs.last_mut().expect("a run was just pushed");
        let lane_bounds: &[AssetBounds] = match key {
            RunKey::Prim { .. } => &[],
            RunKey::Asset { .. } => &asset_bounds,
        };
        raws.push(instance_raw(m, lane_bounds));
        items.push(cull_item(m, 0, bounds[i], radii[i]));
        run.count += 1;
        run.sources.push(i as u32);
        run.casters += u32::from(m.cast_shadows);
    }
    (runs, raws, items)
}

/// The maximal runs of slots where `new` differs bytewise from `old` (equal lengths).
pub(super) fn changed_spans<T: Pod>(old: &[T], new: &[T]) -> Vec<Range<usize>> {
    debug_assert_eq!(old.len(), new.len());
    let mut spans: Vec<Range<usize>> = Vec::new();
    for (i, (a, b)) in old.iter().zip(new).enumerate() {
        if bytemuck::bytes_of(a) == bytemuck::bytes_of(b) {
            continue;
        }
        match spans.last_mut() {
            Some(span) if span.end == i => span.end = i + 1,
            _ => spans.push(i..i + 1),
        }
    }
    spans
}

fn layout_eq(a: &[StaticRun], b: &[StaticRun]) -> bool {
    a.len() == b.len()
        && a.iter()
            .zip(b)
            .all(|(x, y)| x.key == y.key && x.count == y.count && x.sources == y.sources)
}

/// The persistent buffers' usage; a zero-size buffer cannot be bound or copied, so nothing is empty.
fn persistent(device: &wgpu::Device, bytes: &[u8]) -> wgpu::Buffer {
    let usage = wgpu::BufferUsages::COPY_SRC | wgpu::BufferUsages::COPY_DST;
    if bytes.is_empty() {
        init_buffer(device, usage, &[0; 16])
    } else {
        init_buffer(device, usage, bytes)
    }
}

impl StaticGpu {
    /// Brings `slot` up to `scene.statics`: creates it, patches it or rebuilds it, and does nothing
    /// when the static key is unchanged.
    pub(super) fn update(
        slot: &mut Option<StaticGpu>,
        scene: &RenderScene,
        assets: &[MeshAssetGpu],
        device: &wgpu::Device,
        queue: &wgpu::Queue,
    ) -> StaticUpdate {
        let range = scene.statics;
        if let Some(gpu) = slot.as_ref()
            && gpu.key == range.key
            && gpu.count == range.count
        {
            return StaticUpdate::Quiet;
        }
        let (runs, raws, mut items) = build(&scene.meshes[..range.count], assets);
        if let Some(gpu) = slot.as_mut()
            && layout_eq(&gpu.runs, &runs)
        {
            // The same layout keeps the frame buckets the old items were stamped with.
            for (run, &bucket) in runs.iter().zip(&gpu.buckets) {
                let span = run.first as usize..(run.first + run.count) as usize;
                items[span].iter_mut().for_each(|item| item.bucket = bucket);
            }
            let raw_spans = changed_spans(&gpu.raws, &raws);
            let item_spans = changed_spans(&gpu.items, &items);
            let mut changed = vec![false; raws.len()];
            for span in raw_spans.iter().chain(&item_spans) {
                changed[span.clone()].fill(true);
            }
            for span in raw_spans {
                queue.write_buffer(
                    &gpu.instances,
                    span.start as u64 * RAW_BYTES,
                    bytemuck::cast_slice(&raws[span]),
                );
            }
            for span in item_spans {
                queue.write_buffer(
                    &gpu.items_buf,
                    span.start as u64 * ITEM_BYTES,
                    bytemuck::cast_slice(&items[span]),
                );
            }
            gpu.key = range.key;
            gpu.count = range.count;
            gpu.runs = runs;
            gpu.raws = raws;
            gpu.items = items;
            return StaticUpdate::Patched {
                slots: changed.iter().filter(|&&c| c).count() as u32,
            };
        }
        *slot = Some(StaticGpu {
            key: range.key,
            count: range.count,
            instances: persistent(device, bytemuck::cast_slice(&raws)),
            items_buf: persistent(device, bytemuck::cast_slice(&items)),
            runs,
            raws,
            items,
            buckets: Vec::new(),
        });
        StaticUpdate::Rebuilt
    }

    /// Stamps the frame's bucket ids (one per run, in run order) into the items when they moved.
    /// Returns whether it rewrote them.
    pub(super) fn remap(&mut self, frame_buckets: &[u32], queue: &wgpu::Queue) -> bool {
        debug_assert_eq!(frame_buckets.len(), self.runs.len());
        if self.buckets == frame_buckets {
            return false;
        }
        for (run, &bucket) in self.runs.iter().zip(frame_buckets) {
            let span = run.first as usize..(run.first + run.count) as usize;
            self.items[span]
                .iter_mut()
                .for_each(|item| item.bucket = bucket);
        }
        queue.write_buffer(&self.items_buf, 0, bytemuck::cast_slice(&self.items));
        self.buckets = frame_buckets.to_vec();
        true
    }

    /// The run of `key` and its index.
    pub(super) fn run(&self, key: RunKey) -> Option<(usize, &StaticRun)> {
        self.runs.iter().enumerate().find(|(_, r)| r.key == key)
    }

    pub(super) fn run_by_index(&self, run: usize) -> &StaticRun {
        &self.runs[run]
    }

    pub(super) fn run_count(&self) -> usize {
        self.runs.len()
    }

    /// The scene's leading static meshes this holds, dropped or not.
    pub(super) fn count(&self) -> usize {
        self.count
    }

    pub(super) fn instances(&self) -> &wgpu::Buffer {
        &self.instances
    }

    pub(crate) fn items_buffer(&self) -> &wgpu::Buffer {
        &self.items_buf
    }
}

impl StaticRun {
    pub(super) fn slice(&self, run: usize) -> StaticSlice {
        StaticSlice {
            run: run as u32,
            first: self.first,
            count: self.count,
            casters: self.casters,
        }
    }
}
