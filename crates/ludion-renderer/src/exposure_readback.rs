//! The metered exposure read back to the CPU: texel 64 of the `exposure_meter` table, through the
//! same small non-blocking ring as [`crate::cull_stats`].
//!
//! A frame files one 1x1 copy into a free slot, or skips when none is free; nothing waits inside a
//! frame. A slot's map is requested at the start of the next `file` or `collect`, because the copy's
//! encoder is submitted only after the pass returns. Only [`MeterRing::collect`] with `wait` blocks.

use std::sync::mpsc::{Receiver, TryRecvError};

/// One metered frame's verdict, as the `exposure_meter` pass wrote it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MeterReading {
    /// The exposure correction the frame applied, in EV (the table's `corr`, inside the range).
    pub correction_ev: f32,
    /// The trimmed mean of the metered log2 luminance.
    pub mean_log2: f32,
    /// The total weight the histogram binned.
    pub weight: f32,
    /// The correction sits on an end of the authored `exposure_range`: the scene wanted more.
    pub clamped: bool,
}

/// The texel's origin in the table, and the bytes one slot holds.
pub(crate) const VERDICT_TEXEL: u32 = 64;
const SLOT_BYTES: u64 = 256;

impl MeterReading {
    /// Decodes the verdict texel (`corr, mean_log2, total_weight, gain`); `range` is the frame's
    /// `[min, max]` correction limits, which decide `clamped`.
    pub(crate) fn decode(texel: [f32; 4], range: [f32; 2]) -> MeterReading {
        MeterReading {
            correction_ev: texel[0],
            mean_log2: texel[1],
            weight: texel[2],
            clamped: texel[0] == range[0] || texel[0] == range[1],
        }
    }
}

type MapResult = Receiver<Result<(), wgpu::BufferAsyncError>>;

/// What a filed copy needs to be decoded: its order and the frame's range.
#[derive(Clone, Copy)]
struct Meta {
    seq: u64,
    range: [f32; 2],
}

enum State {
    Free,
    /// The copy is encoded; the map waits for its submit.
    Encoded(Meta),
    Mapping(Meta, MapResult),
}

struct Slot {
    buffer: Option<wgpu::Buffer>,
    state: State,
}

pub(crate) struct MeterRing {
    slots: [Slot; Self::DEPTH],
    next_seq: u64,
}

impl MeterRing {
    const DEPTH: usize = 3;

    pub(crate) fn new() -> MeterRing {
        MeterRing {
            slots: std::array::from_fn(|_| Slot {
                buffer: None,
                state: State::Free,
            }),
            next_seq: 0,
        }
    }

    /// Files one sample: copies the verdict texel of `table` into a free slot, or skips the sample
    /// when none is free. Never waits.
    pub(crate) fn file(
        &mut self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        table: &wgpu::Texture,
        range: [f32; 2],
    ) {
        self.map_encoded();
        let Some(slot) = self
            .slots
            .iter_mut()
            .find(|slot| matches!(slot.state, State::Free))
        else {
            return;
        };
        let staging = slot.buffer.get_or_insert_with(|| {
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("exposure meter staging"),
                size: SLOT_BYTES,
                usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            })
        });
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: table,
                mip_level: 0,
                origin: wgpu::Origin3d {
                    x: VERDICT_TEXEL,
                    y: 0,
                    z: 0,
                },
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: staging,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(SLOT_BYTES as u32),
                    rows_per_image: None,
                },
            },
            wgpu::Extent3d {
                width: 1,
                height: 1,
                depth_or_array_layers: 1,
            },
        );
        slot.state = State::Encoded(Meta {
            seq: self.next_seq,
            range,
        });
        self.next_seq += 1;
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
                        .slice(..16)
                        .map_async(wgpu::MapMode::Read, move |result| {
                            let _ = tx.send(result);
                        });
                    slot.state = State::Mapping(meta, rx);
                }
                other => slot.state = other,
            }
        }
    }

    /// Drains the finished samples in filing order. With `wait`, blocks until every filed sample
    /// has finished; without, takes only those the GPU already completed.
    pub(crate) fn collect(&mut self, device: &wgpu::Device, wait: bool) -> Vec<MeterReading> {
        self.map_encoded();
        let _ = device.poll(if wait {
            wgpu::PollType::wait_indefinitely()
        } else {
            wgpu::PollType::Poll
        });
        let mut out: Vec<(u64, MeterReading)> = Vec::new();
        for slot in &mut self.slots {
            let State::Mapping(meta, rx) = std::mem::replace(&mut slot.state, State::Free) else {
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
                    if let Ok(range) = buffer.slice(..16).get_mapped_range() {
                        let words: &[f32] = bytemuck::cast_slice(&range);
                        out.push((
                            meta.seq,
                            MeterReading::decode(
                                [words[0], words[1], words[2], words[3]],
                                meta.range,
                            ),
                        ));
                    }
                    buffer.unmap();
                }
            }
        }
        out.sort_by_key(|(seq, _)| *seq);
        out.into_iter().map(|(_, reading)| reading).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decoding_reads_the_four_texel_words_in_order() {
        let r = MeterReading::decode([0.5, -2.5, 9216.0, 1.41], [-2.0, 2.0]);
        assert_eq!(r.correction_ev, 0.5);
        assert_eq!(r.mean_log2, -2.5);
        assert_eq!(r.weight, 9216.0);
        assert!(!r.clamped);
    }

    #[test]
    fn a_correction_on_either_end_of_the_range_is_clamped() {
        assert!(MeterReading::decode([2.0, -9.0, 1.0, 4.0], [-2.0, 2.0]).clamped);
        assert!(MeterReading::decode([-2.0, 5.0, 1.0, 0.25], [-2.0, 2.0]).clamped);
        assert!(!MeterReading::decode([0.0, -2.47, 1.0, 1.0], [-2.0, 2.0]).clamped);
    }
}
