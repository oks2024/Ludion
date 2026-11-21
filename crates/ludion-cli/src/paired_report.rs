//! The report `ludion check-scene` prints for a `kind: paired` document: each role, the align (warp)
//! block, the events and every contact's gap measured on the posed parts. The cross-file checks live
//! in `ludion_engine::check_paired_files`; this renders its [`PairedReport`] as text or JSON.

use std::path::Path;
use std::process::ExitCode;

use ludion_engine::{PairedReport, check_paired_files};
use ludion_scene::PairedMoves;
use serde_json::json;

use crate::check_scene::Reporter;

/// Runs the paired report over the document at the reporter's path; its roles' assemblies resolve
/// relative to that file (the `run_assembly` rooting).
pub(crate) fn run(mut reporter: Reporter) -> ExitCode {
    let path = reporter.path;
    let root = path.parent().unwrap_or_else(|| Path::new("."));
    let rel = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string());
    let report = match check_paired_files(
        ludion_assets::AssetRoot::local_document_unconfined(root),
        &rel,
    ) {
        Ok(report) => report,
        Err(diagnostics) => return reporter.fail(diagnostics),
    };
    for warning in &report.warnings {
        reporter.warn(warning.clone());
    }
    let fields = to_json(&report);
    reporter.ok(fields, || print(path, &report))
}

fn mover(report: &PairedReport) -> String {
    match &report.doc.align.moves {
        PairedMoves::Split => "split".to_string(),
        PairedMoves::Role(role) => role.clone(),
    }
}

fn to_json(report: &PairedReport) -> serde_json::Value {
    let roles: Vec<_> = report
        .roles
        .iter()
        .map(|role| {
            let spec = report.doc.roles.iter().find(|r| r.name == role.name);
            json!({
                "name": role.name,
                "assembly": role.assembly,
                "clip": role.clip,
                "duration": role.duration,
                "align_only": spec.is_some_and(|r| r.align_only),
                "posture": spec.and_then(|r| r.posture.clone()),
                "scale": spec.map_or(1.0, |r| r.scale),
                "reference": role.name == report.doc.align.reference,
                "offset": role.offset,
                "yaw_deg": role.yaw_deg,
                "distance": role.distance,
            })
        })
        .collect();
    let events: Vec<_> = report
        .doc
        .events
        .iter()
        .map(|event| json!({ "name": event.name, "t": event.t }))
        .collect();
    let contacts: Vec<_> = report
        .contacts
        .iter()
        .map(|contact| {
            json!({
                "index": contact.index,
                "a": contact.a,
                "b": contact.b,
                "at": contact.at,
                "t": contact.t,
                "gap_m": contact.gap_m,
                "gap_cells": contact.gap_cells,
                "max_gap": contact.max_gap,
            })
        })
        .collect();
    json!({
        "name": report.doc.name,
        "roles": roles,
        "align": {
            "reference": report.doc.align.reference,
            "moves": mover(report),
            "ticks": report.doc.align.ticks,
        },
        "events": events,
        "contacts": contacts,
    })
}

fn print(path: &Path, report: &PairedReport) {
    let doc = &report.doc;
    let plural = |n: usize| if n == 1 { "" } else { "s" };
    println!(
        "{}: OK (paired {}, {} role{}, {} event{}, {} contact{})",
        path.display(),
        doc.name,
        doc.roles.len(),
        plural(doc.roles.len()),
        doc.events.len(),
        plural(doc.events.len()),
        doc.contacts.len(),
        plural(doc.contacts.len()),
    );
    for role in &report.roles {
        let spec = doc.roles.iter().find(|r| r.name == role.name);
        let mut line = format!("role {}: assembly {}, ", role.name, role.assembly);
        match (&role.clip, role.duration) {
            (Some(clip), Some(duration)) => line.push_str(&format!("clip {clip} ({duration} s)")),
            _ => line.push_str("align only"),
        }
        if let Some(posture) = spec.and_then(|r| r.posture.as_deref()) {
            line.push_str(&format!(", posture {posture}"));
        }
        if let Some(scale) = spec.map(|r| r.scale).filter(|scale| *scale != 1.0) {
            line.push_str(&format!(", scale {scale}"));
        }
        if role.name == doc.align.reference {
            line.push_str(", reference");
        } else {
            line.push_str(&format!(
                " | at [{}, {}, {}] m yaw {}\u{b0}, {:.3} m from {}",
                role.offset[0],
                role.offset[1],
                role.offset[2],
                role.yaw_deg,
                role.distance,
                doc.align.reference
            ));
        }
        println!("{line}");
    }
    println!("align: {} ticks, moves {}", doc.align.ticks, mover(report));
    for event in &doc.events {
        println!("event {} @ {}", event.name, event.t);
    }
    print_contacts(report);
}

/// One line per measured contact, the same text `check-scene` and `render --paired` print.
pub(crate) fn print_contacts(report: &PairedReport) {
    for contact in &report.contacts {
        println!(
            "contact {} -> {} @ {} (t {}): gap {:.3} cells ({:.4} m), max {}",
            contact.a,
            contact.b,
            contact.at,
            contact.t,
            contact.gap_cells,
            contact.gap_m,
            contact.max_gap
        );
    }
}
