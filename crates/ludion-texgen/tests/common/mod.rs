//! Shared helpers: an in-memory resolver and a one-line recipe wrapper.

#![allow(dead_code)]

use std::collections::BTreeMap;

use ludion_core::diagnostics::Diagnostic;
use ludion_texgen::resolve::normalize;
use ludion_texgen::{Resolver, Source};

/// Documents by normalized id.
#[derive(Default)]
pub struct MemResolver(pub BTreeMap<String, String>);

impl MemResolver {
    pub fn with(files: &[(&str, &str)]) -> Self {
        MemResolver(
            files
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
        )
    }
}

impl Resolver for MemResolver {
    fn read(&self, reference: &str, from: Option<&str>) -> Result<Source, String> {
        if reference.starts_with("std:") {
            return Err("the embedded stdlib (std:) is not available in this build".into());
        }
        let id = normalize(reference, from);
        self.0
            .get(&id)
            .map(|text| Source {
                id: id.clone(),
                text: text.clone(),
            })
            .ok_or_else(|| format!("no such recipe `{id}`"))
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
