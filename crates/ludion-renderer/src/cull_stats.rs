//! The cull's per-view counts, read back from its indirect args through a small non-blocking ring.
//!
//! A frame files one copy of the args buffer into a free slot, or skips its sample when none is
//! free; nothing waits inside a frame. A slot's map is requested at the start of the next `file` or
//! `collect`, because the copy's encoder is submitted only after `record_cull` returns. Only
//! [`CullStatsRing::collect`] with `wait` blocks, and that is for a caller that has finished its
//! measured loop.

use std::sync::mpsc::{Receiver, TryRecvError};

/// One view's counts for one frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CullViewCount {
    /// `camera | sun | cascade_N | local_N`.
    pub name: String,
    /// Instances the view tested.
    pub instances: u32,
    /// Instances the view kept to draw: the sum of its buckets' visible counts over every level.
    pub drawn: u32,
    /// Instances drawn at each LOD level, level 0 first; empty when the frame drew no `lods:` chain.
    pub levels_drawn: Vec<u32>,
}

/// What a filed copy needs to be read back.
pub(crate) struct SampleMeta {
    pub(crate) names: Vec<String>,
    pub(crate) tested: Vec<u32>,
    pub(crate) buckets: u32,
    pub(crate) bucket_cap: u32,
    /// Lanes per view: a view's args are `levels` consecutive slot rows, one per LOD level.
    pub(crate) levels: u32,
}

impl SampleMeta {
    fn bytes(&self) -> u64 {
        self.names.len() as u64 * u64::from(self.levels) * u64::from(self.bucket_cap) * ARGS_SLOT
    }

    fn zeros(&self) -> Vec<CullViewCount> {
        self.counts(|_, _| 0)
    }

    /// `drawn(view, level)` is the instances a view kept at one level.
    fn counts(&self, drawn: impl Fn(usize, usize) -> u32) -> Vec<CullViewCount> {
        let levels = self.levels as usize;
        self.names
            .iter()
            .enumerate()
            .map(|(v, name)| {
                let per_level: Vec<u32> = (0..levels).map(|l| drawn(v, l)).collect();
                CullViewCount {
                    name: name.clone(),
                    instances: self.tested[v],
                    drawn: per_level.iter().sum(),
                    levels_drawn: if levels > 1 { per_level } else { Vec::new() },
                }
            })
            .collect()
    }
}

const ARGS_SLOT: u64 = 20;

type MapResult = Receiver<Result<(), wgpu::BufferAsyncError>>;

enum State {
    Free,
    /// The copy is encoded; the map waits for its submit.
    Encoded(SampleMeta),
    Mapping(SampleMeta, MapResult),
}

struct Slot {
    buffer: Option<wgpu::Buffer>,
    state: State,
}

pub(crate) struct CullStatsRing {
    slots: [Slot; Self::DEPTH],
    ready: Vec<Vec<CullViewCount>>,
}

impl CullStatsRing {
    const DEPTH: usize = 3;

    pub(crate) fn new() -> CullStatsRing {
        CullStatsRing {
            slots: std::array::from_fn(|_| Slot {
                buffer: None,
                state: State::Free,
            }),
            ready: Vec::new(),
        }
    }

    /// Files one sample: copies the args into a free slot, or skips the sample when none is free.
    /// An empty cull wrote no args and files its zero-drawn sample directly. Never waits.
    pub(crate) fn file(
        &mut self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        args: &wgpu::Buffer,
        meta: SampleMeta,
        empty: bool,
    ) {
        self.map_encoded();
        if empty {
            self.ready.push(meta.zeros());
            return;
        }
        let bytes = meta.bytes();
        if bytes == 0 {
            return;
        }
        let Some(slot) = self
            .slots
            .iter_mut()
            .find(|slot| matches!(slot.state, State::Free))
        else {
            return;
        };
        if slot.buffer.as_ref().is_none_or(|b| b.size() < bytes) {
            slot.buffer = Some(device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("cull stats staging"),
                size: bytes,
                usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            }));
        }
        let staging = slot
            .buffer
            .as_ref()
            .expect("the staging buffer was just sized");
        encoder.copy_buffer_to_buffer(args, 0, staging, 0, bytes);
        slot.state = State::Encoded(meta);
    }

    /// Requests the map of every slot whose copy has been submitted since it was filed.
    fn map_encoded(&mut self) {
        for slot in &mut self.slots {
            match std::mem::replace(&mut slot.state, State::Free) {
                State::Encoded(meta) => {
                    let buffer = slot
                        .buffer
                        .as_ref()
                        .expect("an encoded slot holds its buffer");
                    let (tx, rx) = std::sync::mpsc::channel();
                    buffer
                        .slice(..meta.bytes())
                        .map_async(wgpu::MapMode::Read, move |result| {
                            let _ = tx.send(result);
                        });
                    slot.state = State::Mapping(meta, rx);
                }
                other => slot.state = other,
            }
        }
    }

    /// Drains the finished samples. With `wait`, blocks until every filed sample has finished;
    /// without, takes only those the GPU already completed. A failed map drops its sample.
    pub(crate) fn collect(&mut self, device: &wgpu::Device, wait: bool) -> Vec<Vec<CullViewCount>> {
        self.map_encoded();
        let _ = device.poll(if wait {
            wgpu::PollType::wait_indefinitely()
        } else {
            wgpu::PollType::Poll
        });
        let mut out = std::mem::take(&mut self.ready);
        for slot in &mut self.slots {
            let State::Mapping(meta, rx) = std::mem::replace(&mut slot.state, State::Free) else {
                // `map_encoded` left only `Free` and `Mapping`, and `Free` was just rewritten as itself.
                continue;
            };
            match rx.try_recv() {
                Err(TryRecvError::Empty) => slot.state = State::Mapping(meta, rx),
                Err(TryRecvError::Disconnected) | Ok(Err(_)) => {}
                Ok(Ok(())) => {
                    let buffer = slot
                        .buffer
                        .as_ref()
                        .expect("a mapped slot holds its buffer");
                    if let Ok(range) = buffer.slice(..meta.bytes()).get_mapped_range() {
                        let words: &[u32] = bytemuck::cast_slice(&range);
                        let stride = meta.bucket_cap as usize * 5;
                        let levels = meta.levels as usize;
                        out.push(meta.counts(|v, l| {
                            (0..meta.buckets as usize)
                                .map(|b| words[(v * levels + l) * stride + b * 5 + 1])
                                .sum()
                        }));
                    }
                    buffer.unmap();
                }
            }
        }
        out
    }
}
