//! The numeric report `ludion check-scene` prints for a `kind: animation` clip: its own key counts,
//! key-value ranges and local loop seam, and — when the clip is derived from a glTF — a fresh
//! re-import of that source for the before/after key counts, the worst model-space error per joint
//! and the model-space seam. One [`ClipSummary`] is built once and rendered as text or JSON.

use std::f64::consts::PI;
use std::path::Path;
use std::process::ExitCode;

use ludion_core::math::f64::{acos, sqrt};
use ludion_import::{ClipReport, ExternalFiles, KeyCounts, SeamError};
use ludion_scene::{AnimationDocument, Keyframe, emit_animation, sample_channel, sample_rotation};
use serde_json::{Value, json};

use crate::check_scene::Reporter;

const CHANNELS: [&str; 3] = ["rotation", "translation", "scale"];

/// Runs the clip report over `source`, the text of the clip at the reporter's path.
pub(crate) fn run(mut reporter: Reporter, source: &str) -> ExitCode {
    let path = reporter.path;
    let clip = match ludion_scene::check_animation(source) {
        Ok(clip) => clip,
        Err(diagnostics) => return reporter.fail(diagnostics),
    };
    let summary = summarize(path, &clip);
    reporter.ok(summary.to_json(), || summary.print(path))
}

struct ChannelSummary {
    keys: usize,
    before: Option<usize>,
    range: Option<([f64; 3], [f64; 3])>,
}

struct TrackSummary {
    target: String,
    channels: [ChannelSummary; 3],
}

/// The worst difference of one channel and the target it came from (`None` when it is exactly 0).
#[derive(Clone, Default)]
struct Worst {
    value: f64,
    target: Option<String>,
}

#[derive(Default)]
struct LocalSeam {
    rotation_deg: Worst,
    translation: Worst,
    scale: Worst,
}

enum SourceOutcome {
    Ok,
    Missing,
    Failed(String),
    Absent(String),
    NotGltf,
}

impl SourceOutcome {
    fn status(&self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::Missing => "missing",
            Self::Failed(_) => "failed",
            Self::Absent(_) => "absent",
            Self::NotGltf => "not-gltf",
        }
    }
}

struct JointLine {
    joint: String,
    rotation_deg: f64,
    rotation_t: f64,
    translation: f64,
    translation_t: f64,
    scale: f64,
    scale_t: f64,
}

struct SourceSummary {
    path: String,
    outcome: SourceOutcome,
    stale: Option<bool>,
    frames: Option<usize>,
    joints: Vec<JointLine>,
    seam: Option<SeamError>,
}

impl SourceSummary {
    fn note(&self) -> Option<String> {
        let s = &self.path;
        match &self.outcome {
            SourceOutcome::Ok => None,
            SourceOutcome::Missing => Some(format!(
                "source `{s}` not found; the report above is the clip's own (no before/after counts or source error)"
            )),
            SourceOutcome::Failed(why) => Some(format!("re-importing `{s}` failed: {why}")),
            SourceOutcome::Absent(name) => Some(format!("`{s}` has no animation named `{name}`")),
            SourceOutcome::NotGltf => Some(format!("source `{s}` is not a glTF; no source report")),
        }
    }
}

struct ClipSummary {
    path: String,
    name: Option<String>,
    duration: f64,
    looped: bool,
    generated_from: Option<String>,
    tracks: Vec<TrackSummary>,
    seam: Option<LocalSeam>,
    source: Option<SourceSummary>,
}

fn summarize(path: &Path, clip: &AnimationDocument) -> ClipSummary {
    let source = derived_source(path, clip);
    let counts = |target: &str, channel: usize| -> Option<usize> {
        let report = source.as_ref()?.1.as_ref()?;
        let track = report.tracks.iter().find(|t| t.target == target)?;
        let KeyCounts { before, .. } = [track.rotation, track.translation, track.scale][channel];
        Some(before)
    };
    let tracks = clip
        .tracks
        .iter()
        .map(|track| {
            let keys = [&track.rotation, &track.translation, &track.scale];
            let channels = std::array::from_fn(|c| ChannelSummary {
                keys: keys[c].len(),
                before: counts(&track.target, c),
                range: key_range(keys[c]),
            });
            TrackSummary {
                target: track.target.clone(),
                channels,
            }
        })
        .collect();
    ClipSummary {
        path: path.display().to_string(),
        name: clip.name.clone(),
        duration: clip.duration,
        looped: clip.looped,
        generated_from: clip.generated_from.clone(),
        tracks,
        seam: clip.looped.then(|| local_seam(clip)),
        source: source.map(|(summary, _)| summary),
    }
}

fn key_range(keys: &[Keyframe]) -> Option<([f64; 3], [f64; 3])> {
    let first = keys.first()?;
    let (mut min, mut max) = (first.v, first.v);
    for key in keys {
        for axis in 0..3 {
            min[axis] = min[axis].min(key.v[axis]);
            max[axis] = max[axis].max(key.v[axis]);
        }
    }
    Some((min, max))
}

/// The clip's `t = 0` pose against its `t = duration` pose, per channel, worst over the tracks.
fn local_seam(clip: &AnimationDocument) -> LocalSeam {
    let mut seam = LocalSeam::default();
    let note = |worst: &mut Worst, value: f64, target: &str| {
        if value > worst.value {
            *worst = Worst {
                value,
                target: Some(target.to_string()),
            };
        }
    };
    for track in &clip.tracks {
        let end = clip.duration;
        if let (Some(a), Some(b)) = (
            sample_rotation(&track.rotation, 0.0),
            sample_rotation(&track.rotation, end),
        ) {
            let dot = (a[0] * b[0] + a[1] * b[1] + a[2] * b[2] + a[3] * b[3]).abs();
            note(
                &mut seam.rotation_deg,
                2.0 * acos(dot.min(1.0)) * 180.0 / PI,
                &track.target,
            );
        }
        if let (Some(a), Some(b)) = (
            sample_channel(&track.translation, 0.0),
            sample_channel(&track.translation, end),
        ) {
            let d: [f64; 3] = std::array::from_fn(|i| a[i] - b[i]);
            note(
                &mut seam.translation,
                sqrt(d[0] * d[0] + d[1] * d[1] + d[2] * d[2]),
                &track.target,
            );
        }
        if let (Some(a), Some(b)) = (
            sample_channel(&track.scale, 0.0),
            sample_channel(&track.scale, end),
        ) {
            let worst = (0..3).map(|i| (a[i] - b[i]).abs()).fold(0.0, f64::max);
            note(&mut seam.scale, worst, &track.target);
        }
    }
    seam
}

/// Re-imports the clip's `generated_from` glTF, when it has one. The second element is the matched
/// clip's report, for the per-track key counts.
fn derived_source(
    path: &Path,
    clip: &AnimationDocument,
) -> Option<(SourceSummary, Option<ClipReport>)> {
    let s = clip.generated_from.as_deref()?;
    let mut summary = SourceSummary {
        path: s.to_string(),
        outcome: SourceOutcome::NotGltf,
        stale: None,
        frames: None,
        joints: Vec::new(),
        seam: None,
    };
    let lower = s.to_ascii_lowercase();
    if !(lower.ends_with(".glb") || lower.ends_with(".gltf")) {
        return Some((summary, None));
    }
    let src = path.parent().unwrap_or(Path::new("")).join(s);
    let Ok(bytes) = std::fs::read(&src) else {
        summary.outcome = SourceOutcome::Missing;
        return Some((summary, None));
    };
    let import = match ludion_import::import(
        &bytes,
        ExternalFiles::Beside(src.parent().unwrap_or(Path::new(""))),
        &ludion_import::ImportOptions::lossy(true),
    ) {
        Ok(import) => import,
        Err(diagnostics) => {
            let first = diagnostics.first().map_or_else(
                || "no diagnostic".to_string(),
                |d| {
                    format!(
                        "{} {}",
                        d.code,
                        d.message.lines().next().unwrap_or_default()
                    )
                },
            );
            summary.outcome = SourceOutcome::Failed(first);
            return Some((summary, None));
        }
    };
    let Some(artifact) = import.clips.iter().find(|c| c.doc.name == clip.name) else {
        summary.outcome = SourceOutcome::Absent(clip.name.clone().unwrap_or_default());
        return Some((summary, None));
    };
    let mut fresh = artifact.doc.clone();
    fresh.generated_from = clip.generated_from.clone();
    let report = &artifact.report;
    let time = |frame: usize| report.frame_times.get(frame).copied().unwrap_or(0.0);
    summary.outcome = SourceOutcome::Ok;
    summary.stale = Some(emit_animation(&fresh) != emit_animation(clip));
    summary.frames = Some(report.source_frames);
    summary.seam = report.seam;
    summary.joints = report
        .joints
        .iter()
        .map(|j| JointLine {
            joint: j.joint.clone(),
            rotation_deg: j.rotation_deg,
            rotation_t: time(j.rotation_frame),
            translation: j.translation,
            translation_t: time(j.translation_frame),
            scale: j.scale,
            scale_t: time(j.scale_frame),
        })
        .collect();
    Some((summary, Some(report.clone())))
}

fn plural(n: usize) -> &'static str {
    if n == 1 { "" } else { "s" }
}

fn point(p: [f64; 3]) -> String {
    format!("({:.4}, {:.4}, {:.4})", p[0], p[1], p[2])
}

fn target_label(worst: &Worst) -> &str {
    worst.target.as_deref().unwrap_or("-")
}

impl ClipSummary {
    fn print(&self, path: &Path) {
        let name = self
            .name
            .as_ref()
            .map_or(String::new(), |name| format!("{name}, "));
        let from = self
            .generated_from
            .as_ref()
            .map_or(String::new(), |s| format!(", generated from {s}"));
        println!(
            "{}: OK ({name}duration {}, loop {}, {} track{}{from})",
            path.display(),
            self.duration,
            self.looped,
            self.tracks.len(),
            plural(self.tracks.len()),
        );
        for track in &self.tracks {
            let parts: Vec<String> = track
                .channels
                .iter()
                .zip(CHANNELS)
                .map(|(c, label)| {
                    let of = c.before.map_or(String::new(), |b| format!(" (of {b})"));
                    format!("{label} {} key{}{of}", c.keys, plural(c.keys))
                })
                .collect();
            println!("  {}: {}", track.target, parts.join(", "));
            for (c, label) in track.channels.iter().zip(CHANNELS) {
                if let Some((min, max)) = c.range {
                    println!("    {label} key values {}..{}", point(min), point(max));
                }
            }
        }
        if let Some(seam) = &self.seam {
            println!(
                "  seam (local, t=0 vs t=duration): rotation {:.4}° ({}), translation {:.5} ({}), scale {:.5} ({})",
                seam.rotation_deg.value,
                target_label(&seam.rotation_deg),
                seam.translation.value,
                target_label(&seam.translation),
                seam.scale.value,
                target_label(&seam.scale),
            );
        }
        let Some(source) = &self.source else { return };
        match source.frames {
            Some(frames) => println!("  source: {} ({frames} frames)", source.path),
            None => println!("  source: {}", source.path),
        }
        if let Some(note) = source.note() {
            println!("    note: {note}");
            return;
        }
        println!("    error vs source (model space, worst per joint):");
        for j in &source.joints {
            println!(
                "      {}: rotation {:.4}° @ {} s, translation {:.5} @ {} s, scale {:.5} @ {} s",
                j.joint,
                j.rotation_deg,
                j.rotation_t,
                j.translation,
                j.translation_t,
                j.scale,
                j.scale_t,
            );
        }
        if let Some(seam) = source.seam {
            println!(
                "    seam (model space): rotation {:.4}°, translation {:.5}, scale {:.5}",
                seam.rotation_deg, seam.translation, seam.scale,
            );
        }
        if source.stale == Some(true) {
            println!(
                "    note: this file differs from a fresh import of its source; re-run ludion import"
            );
        }
    }

    fn to_json(&self) -> Value {
        let channel = |c: &ChannelSummary| {
            json!({
                "keys": c.keys,
                "before": c.before,
                "min": c.range.map(|(min, _)| min),
                "max": c.range.map(|(_, max)| max),
            })
        };
        let tracks: Vec<Value> = self
            .tracks
            .iter()
            .map(|t| {
                json!({
                    "target": t.target,
                    "rotation": channel(&t.channels[0]),
                    "translation": channel(&t.channels[1]),
                    "scale": channel(&t.channels[2]),
                })
            })
            .collect();
        let worst = |w: &Worst| json!({ "value": w.value, "target": w.target });
        let seam = self.seam.as_ref().map(|s| {
            json!({
                "rotation_deg": worst(&s.rotation_deg),
                "translation": worst(&s.translation),
                "scale": worst(&s.scale),
            })
        });
        let source = self.source.as_ref().map(|s| {
            let joints: Vec<Value> = s
                .joints
                .iter()
                .map(|j| {
                    json!({
                        "joint": j.joint,
                        "rotation_deg": j.rotation_deg,
                        "rotation_t": j.rotation_t,
                        "translation": j.translation,
                        "translation_t": j.translation_t,
                        "scale": j.scale,
                        "scale_t": j.scale_t,
                    })
                })
                .collect();
            json!({
                "path": s.path,
                "status": s.outcome.status(),
                "note": s.note(),
                "stale": s.stale,
                "frames": s.frames,
                "joints": joints,
                "seam": s.seam.map(|m| json!({
                    "rotation_deg": m.rotation_deg,
                    "translation": m.translation,
                    "scale": m.scale,
                })),
            })
        });
        json!({
            "path": self.path,
            "name": self.name,
            "duration": self.duration,
            "loop": self.looped,
            "generated_from": self.generated_from,
            "tracks": tracks,
            "seam": seam,
            "source": source,
        })
    }
}
