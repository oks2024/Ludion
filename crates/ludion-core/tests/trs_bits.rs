//! Cross-arch bit fixture for `ludion_core::math`'s rotation and `Trs`/`Affine3` composition.
//!
//! Every public function is run on seeded inputs and its output written as `f32::to_bits` hex. The
//! committed file must reproduce unblessed on both pinned machines (RTX 3090 box, M4).

use ludion_core::math::{Trs, euler_xyz_degrees_to_quat, quat_to_euler_xyz_degrees};
use ludion_core::rng::DeterministicRng;

const SEED: u64 = 0x7285_B175;
const RANDOM_CASES: usize = 56;

type Params = ([f32; 3], [f32; 3], [f32; 3]);

fn hex(values: &[f32]) -> String {
    values
        .iter()
        .map(|v| format!("{:08x}", v.to_bits()))
        .collect::<Vec<_>>()
        .join(" ")
}

fn trs_hex(t: &Trs) -> String {
    format!(
        "{} {} {}",
        hex(&t.translation),
        hex(&t.rotation),
        hex(&t.scale)
    )
}

fn trs_of((euler, translation, scale): Params) -> Trs {
    Trs {
        translation,
        rotation: euler_xyz_degrees_to_quat(euler),
        scale,
    }
}

fn random_params(rng: &mut DeterministicRng, uniform: bool) -> Params {
    let mut v = |lo: f32, hi: f32| lo + rng.next_f32() * (hi - lo);
    let euler = [v(-180.0, 180.0), v(-180.0, 180.0), v(-180.0, 180.0)];
    let translation = [v(-100.0, 100.0), v(-100.0, 100.0), v(-100.0, 100.0)];
    let s = v(0.25, 4.0);
    let scale = if uniform {
        [s; 3]
    } else {
        [s, v(0.25, 4.0), v(0.25, 4.0)]
    };
    (euler, translation, scale)
}

/// One case per line: `NN <inputs> => <outputs>`. The inputs are compared before the outputs.
fn cases() -> Vec<(String, String)> {
    let mut rng = DeterministicRng::new(SEED);
    let one = [1.0; 3];
    let specials: [Params; 8] = [
        ([0.0; 3], [0.0; 3], one),
        ([90.0, 0.0, 0.0], [1.0, 2.0, 3.0], one),
        ([0.0, 90.0, 0.0], [1.0, 2.0, 3.0], one),
        ([0.0, 0.0, 90.0], [1.0, 2.0, 3.0], one),
        ([0.0, 0.0, 0.0], [0.0; 3], [-1.0, 2.0, 1.0]),
        ([10.0, 90.0, 20.0], [0.0; 3], one),
        ([30.0, -90.0, -40.0], [5.0, 0.0, -5.0], [2.0; 3]),
        ([180.0, 0.0, -180.0], [0.0; 3], one),
    ];
    let mut out = Vec::new();
    for n in 0..specials.len() + RANDOM_CASES {
        let a = if n < specials.len() {
            specials[n]
        } else {
            random_params(&mut rng, n % 2 == 0)
        };
        let b = random_params(&mut rng, n % 3 != 0);
        let p = [
            rng.next_f32() * 20.0 - 10.0,
            rng.next_f32() * 20.0 - 10.0,
            rng.next_f32() * 20.0 - 10.0,
        ];
        let (ta, tb) = (trs_of(a), trs_of(b));
        let (aa, ab) = (ta.to_affine(), tb.to_affine());
        let product = aa.mul_affine(&ab);
        let flat = |c: &[[f32; 3]; 4]| c.iter().flatten().copied().collect::<Vec<f32>>();
        let inputs = format!(
            "{n:02} a {} {} {} | b {} {} {} | p {}",
            hex(&a.0),
            hex(&a.1),
            hex(&a.2),
            hex(&b.0),
            hex(&b.1),
            hex(&b.2),
            hex(&p)
        );
        let outputs = format!(
            "e2q {} | q2e {} | mul {} | inv {} | aff {} | maff {} | pt {} | lossy {}",
            hex(&ta.rotation),
            hex(&quat_to_euler_xyz_degrees(ta.rotation)),
            trs_hex(&ta.mul_trs(&tb)),
            trs_hex(&ta.inverse()),
            hex(&flat(&aa.cols)),
            hex(&flat(&product.cols)),
            hex(&aa.transform_point(p)),
            trs_hex(&product.to_trs_lossy()),
        );
        out.push((inputs, outputs));
    }
    out
}

#[test]
fn trs_math_matches_the_committed_bits() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/trs_bits.txt");
    let cases = cases();
    if ludion_core::bless::bless_requested() {
        let header = "\
# Bit fixture for ludion_core::math rotation and Trs/Affine3 (tests/trs_bits.rs).\n\
# Recorded with: LUDION_BLESS=1 cargo nextest run -p ludion-core --test trs_bits\n\
# Both pinned machines (RTX 3090 box, M4) must reproduce it unblessed. One case per line:\n\
# NN <inputs> => <outputs>, every float as 8-hex-digit f32::to_bits.\n";
        let body: Vec<String> = cases.iter().map(|(i, o)| format!("{i} => {o}")).collect();
        std::fs::write(&path, format!("{header}{}\n", body.join("\n")))
            .expect("the fixtures directory is committed");
        return;
    }
    let committed = std::fs::read_to_string(&path).expect("the committed trs_bits fixture");
    let lines: Vec<&str> = committed
        .lines()
        .filter(|l| !l.starts_with('#') && !l.is_empty())
        .collect();
    assert_eq!(lines.len(), cases.len(), "case count");
    for ((inputs, outputs), line) in cases.iter().zip(lines) {
        let (want_in, want_out) = line.split_once(" => ").expect("`inputs => outputs` line");
        assert_eq!(
            inputs,
            want_in,
            "inputs of case {} moved: the seeded generator changed, not the math",
            &inputs[..2]
        );
        for (got, want) in outputs.split(" | ").zip(want_out.split(" | ")) {
            let name = got.split(' ').next().unwrap_or("?");
            assert_eq!(got, want, "case {}: `{name}` differs", &inputs[..2]);
        }
    }
}

mod mat4 {
    use super::hex;
    use ludion_core::math::glam::{Mat4, Vec3};
    use ludion_core::math::{mat4_determinant, mat4_inverse};

    /// Rigid, translate + non-uniform scale, perspective × view, and a singular matrix.
    fn fixtures() -> [(&'static str, Mat4); 4] {
        let rigid = Mat4::from_rotation_translation(
            ludion_core::math::glam::Quat::from_xyzw(0.5, 0.5, 0.5, 0.5),
            Vec3::new(1.5, -2.25, 3.0),
        );
        let scaled = Mat4::from_translation(Vec3::new(10.0, -3.0, 0.5))
            * Mat4::from_scale(Vec3::new(2.0, 0.5, 3.0));
        let view = Mat4::look_at_rh(Vec3::new(3.0, 4.0, 5.0), Vec3::ZERO, Vec3::Y);
        let persp = Mat4::perspective_rh(1.0, 16.0 / 9.0, 0.1, 100.0) * view;
        let singular = Mat4::from_scale(Vec3::new(1.0, 0.0, 2.0));
        [
            ("rigid", rigid),
            ("scaled", scaled),
            ("persp", persp),
            ("singular", singular),
        ]
    }

    fn line(name: &str, m: Mat4) -> String {
        format!(
            "{name} det {}{}",
            hex(&[mat4_determinant(m)]),
            if name == "singular" {
                String::new()
            } else {
                format!(" inv {}", hex(&mat4_inverse(m).to_cols_array()))
            }
        )
    }

    #[test]
    fn mat4_matches_the_pinned_bits() {
        let got: Vec<String> = fixtures().iter().map(|(n, m)| line(n, *m)).collect();
        let want: [&str; 4] = [PINNED_RIGID, PINNED_SCALED, PINNED_PERSP, PINNED_SINGULAR];
        for (g, w) in got.iter().zip(want) {
            assert_eq!(g, w);
        }
    }

    #[test]
    fn mat4_inverse_times_matrix_is_identity() {
        for (name, m) in fixtures().iter().take(3) {
            let id = mat4_inverse(*m) * *m;
            assert!(id.abs_diff_eq(Mat4::IDENTITY, 1e-4), "{name}: {id:?}");
        }
    }

    #[test]
    #[allow(clippy::disallowed_methods)] // test oracle: compares against glam's own inverse and determinant
    fn mat4_agrees_with_glam_within_tolerance() {
        for (name, m) in fixtures().iter().take(3) {
            // Relative, like the determinant below: glam's SIMD inverse on x86 rounds differently
            // from the scalar one, and the perspective inverse has entries near 50.
            let theirs = m.inverse().to_cols_array();
            for (i, (a, b)) in mat4_inverse(*m)
                .to_cols_array()
                .into_iter()
                .zip(theirs)
                .enumerate()
            {
                assert!(
                    (a - b).abs() <= 1e-4 * b.abs().max(1.0),
                    "{name} inverse[{i}]: {a} vs {b}"
                );
            }
            let (a, b) = (mat4_determinant(*m), m.determinant());
            assert!(
                (a - b).abs() <= 1e-4 * b.abs().max(1.0),
                "{name}: {a} vs {b}"
            );
        }
    }

    #[test]
    fn singular_inverse_is_non_finite_not_a_panic() {
        let (_, m) = fixtures()[3];
        assert_eq!(mat4_determinant(m), 0.0);
        assert!(!mat4_inverse(m).is_finite());
    }

    const PINNED_RIGID: &str = "rigid det 3f800000 inv 00000000 00000000 3f800000 00000000 3f800000 00000000 00000000 00000000 00000000 3f800000 00000000 00000000 40100000 c0400000 bfc00000 3f800000";
    const PINNED_SCALED: &str = "scaled det 40400000 inv 3f000000 00000000 00000000 00000000 00000000 40000000 00000000 00000000 00000000 00000000 3eaaaaab 00000000 c0a00000 40c00000 be2aaaab 3f800000";
    const PINNED_PERSP: &str = "persp det be413140 inv 3f55324d 80000000 beffd5ee 80000000 be22d027 3ee6a6f7 be87ad76 b4299d02 c1efc2a4 c21fd718 c247ccdf c11fd718 41ec9b30 421dbcca 42452bfe 4120000f";
    // The inverse of a singular matrix is NaN/inf, whose NaN sign bit is arch-defined: only the determinant is pinned.
    const PINNED_SINGULAR: &str = "singular det 00000000";
}
