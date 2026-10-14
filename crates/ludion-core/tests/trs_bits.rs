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
