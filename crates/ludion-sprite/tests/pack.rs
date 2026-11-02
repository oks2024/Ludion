//! The atlas packer: its pinned layout, its geometric invariants, and the document it derives.

use ludion_scene::{
    AtlasFilter, AtlasSourceDocument, AtlasSourceInput, Flipbook, check_atlas, emit_atlas,
};
use ludion_sprite::{AtlasBake, AtlasInput, AtlasInputFrame, FrameImage, pack};

/// A frame filled with one opaque colour, so a gutter's provenance is readable from its bytes.
fn solid(width: u32, height: u32, color: [f32; 3]) -> FrameImage {
    let mut image = FrameImage::new(width, height);
    for texel in &mut image.texels {
        *texel = [color[0], color[1], color[2], 1.0];
    }
    image
}

/// A frame whose every texel differs, so an extruded gutter can only match the edge it copied.
fn ramp(width: u32, height: u32) -> FrameImage {
    let mut image = FrameImage::new(width, height);
    for y in 0..height {
        for x in 0..width {
            let index = (y * width + x) as usize;
            image.texels[index] = [
                (x as f32 + 1.0) / (width as f32 + 1.0),
                (y as f32 + 1.0) / (height as f32 + 1.0),
                0.25,
                1.0,
            ];
        }
    }
    image
}

fn source(flipbooks: Vec<Flipbook>) -> AtlasSourceDocument {
    AtlasSourceDocument {
        name: Some("fixture".to_string()),
        pixels_per_unit: 16.0,
        filter: AtlasFilter::Nearest,
        inputs: vec![
            AtlasSourceInput::plain("a.sprite.yaml"),
            AtlasSourceInput::plain("b.sprite.yaml"),
            AtlasSourceInput::plain("c.png"),
        ],
        flipbooks,
    }
}

/// Three inputs of three different sizes: two sprite documents (one frame each) and a loose PNG.
fn inputs() -> Vec<AtlasInput> {
    vec![
        AtlasInput {
            base: "a".to_string(),
            frames: vec![AtlasInputFrame {
                name: Some("idle".to_string()),
                image: ramp(8, 6),
                pivot: [4.0, 6.0],
            }],
        },
        AtlasInput {
            base: "b".to_string(),
            frames: vec![AtlasInputFrame {
                name: Some("idle".to_string()),
                image: solid(4, 4, [1.0, 0.0, 0.0]),
                pivot: [2.0, 2.0],
            }],
        },
        AtlasInput {
            base: "c".to_string(),
            frames: vec![AtlasInputFrame {
                name: None,
                image: solid(6, 2, [0.0, 0.0, 1.0]),
                pivot: [3.0, 1.0],
            }],
        },
    ]
}

fn packed() -> AtlasBake {
    pack(
        &source(Vec::new()),
        "fixture.atlas-source.yaml",
        "fixture",
        &inputs(),
    )
    .expect("the fixture packs")
}

#[test]
fn the_three_input_layout_is_pinned() {
    // A layout change re-bakes every committed atlas in the repository, so it has to be a
    // deliberate edit to this test — never a silent consequence of touching the packer.
    let bake = packed();
    assert_eq!((bake.image.width, bake.image.height), (16, 12));
    assert_eq!(
        bake.doc.region("a/idle").map(|r| r.rect),
        Some([1, 1, 8, 6])
    );
    assert_eq!(
        bake.doc.region("b/idle").map(|r| r.rect),
        Some([11, 1, 4, 4])
    );
    assert_eq!(bake.doc.region("c").map(|r| r.rect), Some([1, 9, 6, 2]));
}

#[test]
fn every_region_sits_inside_the_image_with_its_gutter() {
    let bake = packed();
    for (name, region) in &bake.doc.regions {
        let [x, y, w, h] = region.rect;
        assert!(x >= 1 && y >= 1, "region `{name}` has no top-left gutter");
        assert!(
            x + w < bake.image.width && y + h < bake.image.height,
            "region `{name}` overruns the image"
        );
    }
}

#[test]
fn no_two_padded_regions_overlap() {
    let bake = packed();
    let boxes: Vec<(&String, [u32; 4])> = bake
        .doc
        .regions
        .iter()
        .map(|(name, region)| {
            let [x, y, w, h] = region.rect;
            (name, [x - 1, y - 1, w + 2, h + 2])
        })
        .collect();
    for (i, (left_name, left)) in boxes.iter().enumerate() {
        for (right_name, right) in &boxes[i + 1..] {
            let disjoint = left[0] + left[2] <= right[0]
                || right[0] + right[2] <= left[0]
                || left[1] + left[3] <= right[1]
                || right[1] + right[3] <= left[1];
            assert!(
                disjoint,
                "`{left_name}` and `{right_name}` overlap with gutters"
            );
        }
    }
}

#[test]
fn the_gutter_is_the_regions_own_extruded_edge() {
    let bake = packed();
    let texel = |x: u32, y: u32| {
        let offset = ((y * bake.image.width + x) as usize) * 4;
        bake.image.rgba[offset..offset + 4].to_vec()
    };
    for (name, region) in &bake.doc.regions {
        let [x, y, w, h] = region.rect;
        for column in 0..w {
            assert_eq!(
                texel(x + column, y - 1),
                texel(x + column, y),
                "`{name}` top gutter"
            );
            assert_eq!(
                texel(x + column, y + h),
                texel(x + column, y + h - 1),
                "`{name}` bottom gutter"
            );
        }
        for row in 0..h {
            assert_eq!(
                texel(x - 1, y + row),
                texel(x, y + row),
                "`{name}` left gutter"
            );
            assert_eq!(
                texel(x + w, y + row),
                texel(x + w - 1, y + row),
                "`{name}` right gutter"
            );
        }
        assert_eq!(texel(x - 1, y - 1), texel(x, y), "`{name}` top-left corner");
        assert_eq!(
            texel(x + w, y + h),
            texel(x + w - 1, y + h - 1),
            "`{name}` bottom-right corner"
        );
    }
}

#[test]
fn the_derived_document_round_trips_through_check_atlas() {
    let flipbook = Flipbook {
        name: "walk".to_string(),
        frames: vec!["a/idle".to_string(), "b/idle".to_string()],
        fps: 12.0,
        looped: true,
        hold: None,
    };
    let bake = pack(
        &source(vec![flipbook]),
        "fixture.atlas-source.yaml",
        "fixture",
        &inputs(),
    )
    .expect("the fixture packs");

    let text = emit_atlas(&bake.doc);
    let parsed = check_atlas(&text).expect("the derived atlas is valid");
    assert_eq!(parsed, bake.doc);
    assert_eq!(emit_atlas(&parsed), text, "canonical emit is a fixed point");

    assert_eq!(bake.doc.generated_from, "fixture.atlas-source.yaml");
    assert_eq!(bake.doc.color_map, "fixture.png");
    assert_eq!(bake.doc.pixels_per_unit, 16.0);
    assert_eq!(bake.doc.filter, AtlasFilter::Nearest);
    assert_eq!(bake.doc.region("a/idle").map(|r| r.pivot), Some([4.0, 6.0]));
}

#[test]
fn the_packed_png_hashes_the_bytes_that_get_committed() {
    let bake = packed();
    assert_eq!(bake.image.hash, ludion_assets::content_hash(&bake.png));
    let (width, height, rgba) =
        ludion_assets::decode_png(&bake.png, "fixture.png").expect("the packed PNG decodes");
    assert_eq!((width, height), (bake.image.width, bake.image.height));
    assert_eq!(rgba, bake.image.rgba);
}

#[test]
fn packing_the_same_inputs_twice_gives_the_same_png() {
    assert_eq!(packed().png, packed().png);
}

#[test]
fn two_inputs_minting_one_region_name_are_refused() {
    let mut inputs = inputs();
    inputs[1].base = "a".to_string();
    let diagnostics = pack(
        &source(Vec::new()),
        "fixture.atlas-source.yaml",
        "fixture",
        &inputs,
    )
    .expect_err("a duplicate region name is a defect");
    assert_eq!(diagnostics.len(), 1);
    assert_eq!(diagnostics[0].code.as_str(), "L1153");
    assert_eq!(diagnostics[0].path.as_deref(), Some("/inputs/1"));
    assert!(diagnostics[0].message.contains("a/idle"));
}

#[test]
fn a_flipbook_naming_no_region_is_refused_with_a_suggestion() {
    let flipbook = Flipbook {
        name: "walk".to_string(),
        frames: vec!["a/idle".to_string(), "b/idel".to_string()],
        fps: 12.0,
        looped: true,
        hold: None,
    };
    let diagnostics = pack(
        &source(vec![flipbook]),
        "fixture.atlas-source.yaml",
        "fixture",
        &inputs(),
    )
    .expect_err("an unresolvable flipbook frame is a defect");
    assert_eq!(diagnostics.len(), 1);
    assert_eq!(diagnostics[0].code.as_str(), "L1153");
    assert_eq!(
        diagnostics[0].path.as_deref(),
        Some("/flipbooks/0/frames/1")
    );
    assert_eq!(
        diagnostics[0].help.as_deref(),
        Some("nearest region is `b/idle`")
    );
}

#[test]
fn an_atlas_with_no_inputs_is_refused() {
    let diagnostics = pack(
        &source(Vec::new()),
        "fixture.atlas-source.yaml",
        "fixture",
        &[],
    )
    .expect_err("an empty atlas can never be sampled");
    assert_eq!(diagnostics.len(), 1);
    assert_eq!(diagnostics[0].path.as_deref(), Some("/inputs"));
}

fn one_frame(width: u32, height: u32) -> Vec<AtlasInput> {
    vec![AtlasInput {
        base: "tall".to_string(),
        frames: vec![AtlasInputFrame {
            name: None,
            image: solid(width, height, [0.5, 0.5, 0.5]),
            pivot: [0.0, 0.0],
        }],
    }]
}

#[test]
fn an_atlas_over_the_png_limit_is_refused_at_inputs() {
    let diagnostics = pack(
        &source(Vec::new()),
        "fixture.atlas-source.yaml",
        "fixture",
        &one_frame(1, 9000),
    )
    .expect_err("a 9004-texel-tall atlas is not a loadable texture");
    assert_eq!(diagnostics.len(), 1);
    assert_eq!(diagnostics[0].code.as_str(), "L1153");
    assert_eq!(diagnostics[0].path.as_deref(), Some("/inputs"));
    assert!(diagnostics[0].message.contains("1 regions"));
    assert!(diagnostics[0].message.contains("8192"));
    assert!(!diagnostics[0].message.contains("  "));
}

#[test]
fn an_atlas_exactly_at_the_limit_packs() {
    let bake = pack(
        &source(Vec::new()),
        "fixture.atlas-source.yaml",
        "fixture",
        &one_frame(1, 8190),
    )
    .expect("an 8192-texel side is the largest the loader takes");
    assert_eq!(bake.image.height, 8192);
    ludion_assets::decode_png(&bake.png, "x").expect("the loader accepts what the packer emits");
}

#[test]
fn the_caller_supplied_stem_names_the_colour_map() {
    let bake = pack(
        &AtlasSourceDocument {
            name: None,
            ..source(Vec::new())
        },
        "ship-v1.2.atlas-source.yaml",
        "ship-v1.2",
        &inputs(),
    )
    .expect("packs");
    assert_eq!(bake.doc.color_map, "ship-v1.2.png");
}
