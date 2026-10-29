//! Cook → KTX2 → read round trips, mismatch and corruption handling, determinism.

use ludion_assets::Image;
use ludion_assets::cooked::{ColorSpace, CompressedFormat, CookSettings, CookTarget, read_ktx2};
use ludion_cook::{cook, write_ktx2};

const TARGETS: [CookTarget; 2] = [CookTarget::Desktop, CookTarget::Ios];
const SPACES: [ColorSpace; 2] = [ColorSpace::Srgb, ColorSpace::Linear];

fn settings(target: CookTarget, color_space: ColorSpace) -> CookSettings {
    CookSettings {
        target,
        color_space,
        max_texture_size: None,
    }
}

/// A smooth gradient with a little LCG noise; `alpha` makes the fourth channel vary.
fn image(width: u32, height: u32, alpha: bool) -> Image {
    let mut state = 0x2545_f491_u32;
    let mut rgba = Vec::new();
    for y in 0..height {
        for x in 0..width {
            state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            let noise = (state >> 29) as u8;
            let a = if alpha {
                (x * 255 / width.max(1)) as u8
            } else {
                255
            };
            rgba.extend_from_slice(&[
                (x * 200 / width) as u8 + noise,
                (y * 200 / height) as u8 + noise,
                60 + noise,
                a,
            ]);
        }
    }
    Image {
        width,
        height,
        rgba,
        hash: 1,
    }
}

fn scratch(label: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("ludion-cook-{label}-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn decode_level0(image: &ludion_assets::cooked::CompressedImage) -> Vec<u8> {
    let (w, h) = (image.width as usize, image.height as usize);
    let mut out = vec![0u32; w * h];
    match image.format {
        CompressedFormat::Bc7 => {
            texture2ddecoder::decode_bc7(&image.levels[0], w, h, &mut out).unwrap()
        }
        CompressedFormat::Astc4x4 => {
            texture2ddecoder::decode_astc(&image.levels[0], w, h, 4, 4, &mut out).unwrap()
        }
    }
    // The decoder yields 0xAARRGGBB.
    out.iter()
        .flat_map(|v| [(v >> 16) as u8, (v >> 8) as u8, *v as u8, (v >> 24) as u8])
        .collect()
}

fn mean_abs_error(a: &[u8], b: &[u8]) -> [f64; 4] {
    let mut sum = [0u64; 4];
    for (i, (x, y)) in a.iter().zip(b).enumerate() {
        sum[i % 4] += u64::from(x.abs_diff(*y));
    }
    sum.map(|s| s as f64 / (a.len() / 4) as f64)
}

#[test]
fn round_trip_every_target_and_space() {
    let dir = scratch("roundtrip");
    for target in TARGETS {
        for space in SPACES {
            let source = image(64, 48, false);
            let cooked = cook(&source, &settings(target, space)).unwrap();
            let path = dir.join(format!("{}-{space:?}.ktx2", target.name()));
            std::fs::write(&path, write_ktx2(&cooked)).unwrap();
            let read = read_ktx2(&std::fs::read(&path).unwrap(), target, space).unwrap();
            assert_eq!(read, cooked);
            assert_eq!(read.format, target.format());
            assert_eq!(read.srgb, space == ColorSpace::Srgb);
            assert_eq!((read.width, read.height), (64, 48));
            assert_eq!(read.level_count(), 7); // 64 -> 1
            for level in 0..read.level_count() {
                let (bx, by) = read.blocks(level);
                assert_eq!(read.levels[level as usize].len(), (bx * by * 16) as usize);
            }
            let error = mean_abs_error(&source.rgba, &decode_level0(&read));
            assert!(
                error.iter().all(|e| *e <= 4.0),
                "{target:?} {space:?} error {error:?}"
            );
        }
    }
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn alpha_survives_both_encoders() {
    for target in TARGETS {
        let source = image(32, 32, true);
        let cooked = cook(&source, &settings(target, ColorSpace::Linear)).unwrap();
        let error = mean_abs_error(&source.rgba, &decode_level0(&cooked));
        assert!(error[3] <= 4.0, "{target:?} alpha error {error:?}");
    }
}

#[test]
fn unaligned_level_zero_is_resampled_up() {
    for target in TARGETS {
        let cooked = cook(&image(30, 18, false), &settings(target, ColorSpace::Srgb)).unwrap();
        assert_eq!((cooked.width, cooked.height), (32, 20));
        assert_eq!(cooked.level_count(), 6); // 32 -> 1
        let read = read_ktx2(&write_ktx2(&cooked), target, ColorSpace::Srgb).unwrap();
        assert_eq!(read, cooked);
    }
}

#[test]
fn one_texel_map_is_one_block() {
    for target in TARGETS {
        let cooked = cook(&image(1, 1, false), &settings(target, ColorSpace::Linear)).unwrap();
        assert_eq!(
            (cooked.width, cooked.height, cooked.level_count()),
            (4, 4, 3)
        );
        assert_eq!(
            read_ktx2(&write_ktx2(&cooked), target, ColorSpace::Linear).unwrap(),
            cooked
        );
    }
}

#[test]
fn texture_cap_halves_the_base() {
    let capped = CookSettings {
        max_texture_size: Some(16),
        ..settings(CookTarget::Desktop, ColorSpace::Srgb)
    };
    let cooked = cook(&image(64, 48, false), &capped).unwrap();
    assert_eq!((cooked.width, cooked.height), (16, 12));
}

#[test]
fn malformed_image_is_refused() {
    let mut bad = image(4, 4, false);
    bad.rgba.pop();
    assert!(cook(&bad, &settings(CookTarget::Desktop, ColorSpace::Srgb)).is_err());
    assert!(
        cook(
            &Image {
                width: 0,
                height: 4,
                rgba: vec![],
                hash: 0
            },
            &settings(CookTarget::Ios, ColorSpace::Srgb)
        )
        .is_err()
    );
}

#[test]
fn a_mismatched_file_is_an_error() {
    let file = write_ktx2(
        &cook(
            &image(16, 16, false),
            &settings(CookTarget::Desktop, ColorSpace::Srgb),
        )
        .unwrap(),
    );
    assert!(read_ktx2(&file, CookTarget::Desktop, ColorSpace::Srgb).is_ok());
    assert!(read_ktx2(&file, CookTarget::Desktop, ColorSpace::Linear).is_err());
    assert!(read_ktx2(&file, CookTarget::Ios, ColorSpace::Srgb).is_err());
}

#[test]
fn corrupt_files_never_panic() {
    let file = write_ktx2(
        &cook(
            &image(8, 8, false),
            &settings(CookTarget::Ios, ColorSpace::Srgb),
        )
        .unwrap(),
    );
    for end in 0..file.len() {
        let _ = read_ktx2(&file[..end], CookTarget::Ios, ColorSpace::Srgb);
    }
    for at in 0..200.min(file.len()) {
        let mut flipped = file.clone();
        flipped[at] ^= 0xff;
        let _ = read_ktx2(&flipped, CookTarget::Ios, ColorSpace::Srgb);
    }
}

#[test]
fn bytes_are_identical_across_runs_and_threads() {
    for target in TARGETS {
        let source = image(30, 18, true);
        let cook_bytes =
            || write_ktx2(&cook(&source, &settings(target, ColorSpace::Srgb)).unwrap());
        let first = cook_bytes();
        assert_eq!(first, cook_bytes());
        assert_eq!(first, cook_bytes());
        let threaded: Vec<Vec<u8>> = std::thread::scope(|scope| {
            let handles: Vec<_> = (0..4).map(|_| scope.spawn(cook_bytes)).collect();
            handles
                .into_iter()
                .map(|handle| handle.join().unwrap())
                .collect()
        });
        assert!(
            threaded.iter().all(|bytes| *bytes == first),
            "{target:?} differs across threads"
        );
    }
}

#[test]
fn the_mean_is_the_source_pixels_mean_even_when_capped() {
    let source = image(64, 48, true);
    let want = ludion_assets::srgb_mean(&source.rgba).map(f32::to_bits);
    let uncapped = cook(&source, &settings(CookTarget::Desktop, ColorSpace::Srgb)).unwrap();
    assert_eq!(uncapped.mean.map(f32::to_bits), want);
    let capped = cook(
        &source,
        &CookSettings {
            max_texture_size: Some(16),
            ..settings(CookTarget::Desktop, ColorSpace::Srgb)
        },
    )
    .unwrap();
    assert!(capped.width < source.width, "the cap must have resampled");
    assert_eq!(capped.mean.map(f32::to_bits), want);
}

#[test]
fn a_file_without_its_mean_entry_is_refused() {
    let cooked = cook(
        &image(16, 16, false),
        &settings(CookTarget::Desktop, ColorSpace::Srgb),
    )
    .unwrap();
    let mut bytes = write_ktx2(&cooked);
    // Header index: kvd length at byte 60.
    bytes[60..64].copy_from_slice(&0u32.to_le_bytes());
    assert!(read_ktx2(&bytes, CookTarget::Desktop, ColorSpace::Srgb).is_err());
}
