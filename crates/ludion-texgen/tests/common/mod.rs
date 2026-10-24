//! Shared helpers: an in-memory resolver and a one-line recipe wrapper.

#![allow(dead_code)]

use std::collections::BTreeMap;

use ludion_core::diagnostics::Diagnostic;
use ludion_texgen::resolve::normalize;
use ludion_texgen::{Resolver, Source};

/// Documents and image files by normalized id.
#[derive(Default)]
pub struct MemResolver {
    pub documents: BTreeMap<String, String>,
    pub images: BTreeMap<String, Vec<u8>>,
}

impl MemResolver {
    pub fn with(files: &[(&str, &str)]) -> Self {
        MemResolver {
            documents: files
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
            images: BTreeMap::new(),
        }
    }

    pub fn with_image(mut self, id: &str, bytes: Vec<u8>) -> Self {
        self.images.insert(id.to_string(), bytes);
        self
    }
}

impl Resolver for MemResolver {
    fn read(&self, reference: &str, from: Option<&str>) -> Result<Source, String> {
        if reference.starts_with("std:") {
            return Err("the embedded stdlib (std:) is not available in this build".into());
        }
        let id = normalize(reference, from);
        self.documents
            .get(&id)
            .map(|text| Source {
                id: id.clone(),
                text: text.clone(),
            })
            .ok_or_else(|| format!("no such recipe `{id}`"))
    }

    fn read_bytes(&self, reference: &str, from: Option<&str>) -> Result<(String, Vec<u8>), String> {
        let id = normalize(reference, from);
        self.images
            .get(&id)
            .map(|bytes| (id.clone(), bytes.clone()))
            .ok_or_else(|| format!("no such image `{id}`"))
    }
}

/// `steps` and `outputs` bodies wrapped in the envelope; every output names `out`.
pub fn recipe(steps: &str, outputs: &str) -> String {
    format!("format: 1\nkind: texture\nsteps:\n{steps}outputs:\n{outputs}")
}

/// The first diagnostic of a failing build.
pub fn first_error(source: &str, resolver: &MemResolver) -> Diagnostic {
    ludion_texgen::load(source, resolver)
        .expect_err("the recipe should be rejected")
        .remove(0)
}

/// An 8-bit RGB PNG of a deterministic diagonal pattern.
pub fn pattern_png(side: u32) -> Vec<u8> {
    let mut raw = Vec::new();
    for y in 0..side {
        for x in 0..side {
            raw.extend([
                (x * 255 / side) as u8,
                (y * 255 / side) as u8,
                ((x + y) * 127 / side) as u8,
            ]);
        }
    }
    let mut out = Vec::new();
    let mut enc = png::Encoder::new(&mut out, side, side);
    enc.set_color(png::ColorType::Rgb);
    enc.set_depth(png::BitDepth::Eight);
    enc.write_header().unwrap().write_image_data(&raw).unwrap();
    out
}
