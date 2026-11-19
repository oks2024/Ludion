//! `check_paired_files` (`Plan/paired-animations.md` W4): the role-assembly checks and the contact
//! gaps measured on posed parts, over fixtures that name the gallery humanoid.

use std::path::{Path, PathBuf};

use ludion_assets::AssetRoot;
use ludion_core::diagnostics::Diagnostic;
use ludion_engine::{PairedReport, check_paired_files};

fn run(dir: &Path, file: &str) -> Result<PairedReport, Vec<Diagnostic>> {
    check_paired_files(AssetRoot::local_document_unconfined(dir), file)
}

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("paired")
}

fn fixture(name: &str) -> Result<PairedReport, Vec<Diagnostic>> {
    run(&fixtures(), &format!("{name}.paired.yaml"))
}

fn codes(errors: &[Diagnostic]) -> Vec<(String, String)> {
    errors
        .iter()
        .map(|d| (d.code.to_string(), d.path.clone().unwrap_or_default()))
        .collect()
}

#[test]
fn the_gallery_choke_is_clean_with_a_measured_contact() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/gallery/voxels");
    let report = run(&dir, "choke.paired.yaml").expect("the gallery document is valid");
    assert!(report.warnings.is_empty(), "{:?}", report.warnings);
    assert_eq!(report.roles.len(), 2);
    let target = &report.roles[1];
    assert_eq!(target.name, "target");
    assert_eq!(target.offset, [0.0, 0.0, -0.5]);
    assert!((target.distance - 0.5).abs() < 1e-6);
    assert_eq!(target.duration, Some(1.2));
    let contact = &report.contacts[0];
    assert_eq!(
        (contact.a.as_str(), contact.b.as_str()),
        ("instigator/lower-arm-r", "target/head")
    );
    assert!(contact.gap_cells <= contact.max_gap);
}

#[test]
fn a_rest_gap_is_the_offset_minus_the_part_width() {
    // The head is 2 cells wide; two heads 5 cells apart along X leave 5 − 2 = 3 cells.
    let report = fixture("rest-gap").expect("valid");
    let contact = &report.contacts[0];
    assert_eq!(contact.gap_cells, 3.0);
    assert_eq!(contact.gap_m, 3.0);
    assert!(report.warnings.is_empty());
}

#[test]
fn scale_converts_metre_offsets_to_cells() {
    // Offset 8 m at scale 2: the cell gap is 8 / 2 − 2 = 2 cells, i.e. 4 m.
    let report = fixture("rest-gap-scale2").expect("valid");
    let contact = &report.contacts[0];
    assert_eq!(contact.gap_cells, 2.0);
    assert_eq!(contact.gap_m, 4.0);
}

#[test]
fn a_posed_event_differs_from_rest() {
    let rest = fixture("posed-rest").expect("valid").contacts[0].gap_cells;
    let posed = fixture("posed-carry").expect("valid").contacts[0].gap_cells;
    assert!(
        (rest - posed).abs() > 1e-3,
        "the carry clip should move the forearm: rest {rest}, posed {posed}"
    );
}

#[test]
fn an_over_max_gap_contact_is_a_warning_not_a_failure() {
    let report = fixture("over-gap").expect("a warning does not fail the check");
    assert_eq!(report.warnings.len(), 1);
    assert_eq!(report.warnings[0].code.to_string(), "L1755");
    assert_eq!(report.warnings[0].path.as_deref(), Some("/contacts/0"));
}

#[test]
fn an_unknown_clip_is_l1754_with_the_nearest_name() {
    let errors = fixture("bad-clip").expect_err("unknown clip");
    assert!(codes(&errors).contains(&("L1754".to_string(), "/roles/a/clip".to_string())));
    let help = errors[0].help.clone().unwrap_or_default();
    assert!(help.contains("carry"), "{help}");
}

#[test]
fn an_event_past_the_clip_is_l1754() {
    let errors = fixture("bad-event").expect_err("event past the clip");
    assert!(codes(&errors).contains(&("L1754".to_string(), "/events/0/t".to_string())));
    assert!(
        errors[0].message.contains("role `a`"),
        "{}",
        errors[0].message
    );
}

#[test]
fn an_unknown_contact_part_is_l1754_with_the_nearest_name() {
    let errors = fixture("bad-part").expect_err("unknown part");
    assert!(codes(&errors).contains(&("L1754".to_string(), "/contacts/0/b".to_string())));
    assert!(errors[0].help.clone().unwrap_or_default().contains("head"));
}

#[test]
fn an_assembly_that_does_not_load_is_l1754() {
    let errors = fixture("bad-assembly").expect_err("missing assembly");
    assert!(codes(&errors).contains(&("L1754".to_string(), "/roles/b/assembly".to_string())));
}
