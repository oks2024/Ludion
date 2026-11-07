//! Shared fixtures: a closure-backed column source and parameter helpers.
#![allow(dead_code)]

use ludion_nav::{ColumnSource, FloorSample, GridParams};

pub struct FnSource<F: Fn(u32, u32, &mut Vec<FloorSample>)>(pub F);

impl<F: Fn(u32, u32, &mut Vec<FloorSample>)> ColumnSource for FnSource<F> {
    fn column(&self, i: u32, j: u32, _center: [f32; 2], out: &mut Vec<FloorSample>) {
        (self.0)(i, j, out)
    }
}

pub fn params(nx: u32, nz: u32) -> GridParams {
    GridParams {
        origin: [0.0, 0.0],
        cell_size: 1.0,
        nx,
        nz,
        agent_height: 1.8,
        max_step: 0.4,
        max_slope_deg: 45.0,
    }
}

pub fn flat(y: f32) -> FloorSample {
    FloorSample {
        y,
        normal: [0.0, 1.0, 0.0],
        headroom: f32::INFINITY,
    }
}

/// A surface tilted `deg` degrees about the z axis.
pub fn tilted(y: f32, deg: f32) -> FloorSample {
    let r = deg.to_radians();
    FloorSample {
        y,
        normal: [-ludion_core::math::sin(r), ludion_core::math::cos(r), 0.0],
        headroom: f32::INFINITY,
    }
}

/// The world point at the centre of column `(i, j)` at height `y`.
pub fn at(i: u32, j: u32, y: f32) -> [f32; 3] {
    [i as f32 + 0.5, y, j as f32 + 0.5]
}
