use ludion_core::par::with_threads;
use ludion_core::rng::DeterministicRng;

use super::narrow;
use super::*;

fn h(i: u32) -> Handle {
    Handle::slot(i, 0)
}

fn sphere(i: u32, c: [f32; 3], r: f32, layers: u32) -> Item {
    Item {
        handle: h(i),
        volume: Volume::Sphere {
            center: c,
            radius: r,
        },
        layers,
    }
}

fn aabb(i: u32, min: [f32; 3], max: [f32; 3], layers: u32) -> Item {
    Item {
        handle: h(i),
        volume: Volume::Aabb { min, max },
        layers,
    }
}

fn built(items: &[Item]) -> Index3d {
    let mut index = Index3d::new();
    for item in items {
        index.push(*item);
    }
    index.build();
    index
}

fn rf(rng: &mut DeterministicRng, lo: f32, hi: f32) -> f32 {
    lo + rng.next_f32() * (hi - lo)
}

fn point(rng: &mut DeterministicRng) -> [f32; 3] {
    [rf(rng, 0.0, 100.0), rf(rng, 0.0, 12.0), rf(rng, 0.0, 100.0)]
}

fn scene(seed: u64) -> Vec<Item> {
    let mut rng = DeterministicRng::new(seed);
    let mut items = Vec::new();
    for i in 0..5_500u32 {
        let c = point(&mut rng);
        let layers = 1 << rng.gen_range_u32(0..3);
        items.push(if i < 500 {
            sphere(i, c, rf(&mut rng, 0.3, 1.5), layers)
        } else if i % 2 == 0 {
            sphere(i, c, rf(&mut rng, 0.02, 0.1), layers)
        } else {
            let e = [
                rf(&mut rng, 0.02, 0.15),
                rf(&mut rng, 0.02, 0.15),
                rf(&mut rng, 0.02, 0.15),
            ];
            aabb(
                i,
                [c[0] - e[0], c[1] - e[1], c[2] - e[2]],
                [c[0] + e[0], c[1] + e[1], c[2] + e[2]],
                layers,
            )
        });
    }
    items
}

fn shuffled(items: &[Item], seed: u64) -> Vec<Item> {
    let mut rng = DeterministicRng::new(seed);
    let mut v = items.to_vec();
    for i in (1..v.len()).rev() {
        v.swap(i, rng.gen_range_usize(0..i + 1));
    }
    v
}

// ── oracles ──────────────────────────────────────────────────────────────────────────────────

fn oracle_overlap(index: &Index3d, probe: &Volume, f: &Filter) -> Vec<Handle> {
    let mut v: Vec<Handle> = index
        .items()
        .iter()
        .filter(|i| f.passes(i) && narrow::overlap(probe, &i.volume))
        .map(|i| i.handle)
        .collect();
    v.sort_unstable();
    v
}

type KnnBits = (Handle, u32, [u32; 3]);

fn oracle_knn(index: &Index3d, p: [f32; 3], k: usize, max_r: f32, f: &Filter) -> Vec<KnnBits> {
    let mut v: Vec<(f32, Handle, [f32; 3])> = index
        .items()
        .iter()
        .filter(|i| f.passes(i))
        .map(|i| {
            (
                narrow::center_distance_sq(p, &i.volume),
                i.handle,
                i.volume.center(),
            )
        })
        .filter(|(d, _, _)| *d <= max_r * max_r)
        .collect();
    v.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
    v.truncate(k);
    v.into_iter()
        .map(|(d, h, c)| (h, d.to_bits(), c.map(f32::to_bits)))
        .collect()
}

fn oracle_cast(index: &Index3d, o: [f32; 3], d: [f32; 3], r: f32, f: &Filter) -> Option<CastHit> {
    let mut best: Option<CastHit> = None;
    for item in index.items().iter().filter(|i| f.passes(i)) {
        if let Some((t, point, normal)) = narrow::cast(o, d, r, &item.volume)
            && best.is_none_or(|b| {
                t.total_cmp(&b.t).then(item.handle.cmp(&b.handle)) == Ordering::Less
            })
        {
            best = Some(CastHit {
                handle: item.handle,
                t,
                point,
                normal,
            });
        }
    }
    best
}

use std::cmp::Ordering;

fn hit_bits(h: Option<CastHit>) -> Option<(Handle, [u32; 10])> {
    h.map(|h| {
        let f = |v: [f32; 3]| v.map(f32::to_bits);
        let (p, n) = (f(h.point), f(h.normal));
        (
            h.handle,
            [h.t.to_bits(), p[0], p[1], p[2], n[0], n[1], n[2], 0, 0, 0],
        )
    })
}

fn knn_bits(v: &[Neighbor]) -> Vec<KnnBits> {
    v.iter()
        .map(|n| {
            (
                n.handle,
                n.distance_sq.to_bits(),
                n.center.map(f32::to_bits),
            )
        })
        .collect()
}

// ── (a) oracle equality ──────────────────────────────────────────────────────────────────────

#[test]
fn queries_match_the_brute_force_oracle() {
    let index = built(&scene(7));
    let mut rng = DeterministicRng::new(99);
    let mut handles = Vec::new();
    let mut neighbors = Vec::new();
    for n in 0..240 {
        let f = Filter::new([1, 2, 4, 7][n % 4]);
        let p = point(&mut rng);

        let r = rf(&mut rng, 0.0, 6.0);
        index.overlap_sphere(p, r, &f, &mut handles);
        assert_eq!(
            handles,
            oracle_overlap(
                &index,
                &Volume::Sphere {
                    center: p,
                    radius: r
                },
                &f
            )
        );

        let e = [
            rf(&mut rng, 0.0, 5.0),
            rf(&mut rng, 0.0, 5.0),
            rf(&mut rng, 0.0, 5.0),
        ];
        let (lo, hi) = (
            [p[0] - e[0], p[1] - e[1], p[2] - e[2]],
            [p[0] + e[0], p[1] + e[1], p[2] + e[2]],
        );
        index.overlap_aabb(lo, hi, &f, &mut handles);
        assert_eq!(
            handles,
            oracle_overlap(&index, &Volume::Aabb { min: lo, max: hi }, &f)
        );

        let (k, max_r) = (1 + n % 12, rf(&mut rng, 1.0, 40.0));
        index.k_nearest(p, k, max_r, &f, &mut neighbors);
        assert_eq!(knn_bits(&neighbors), oracle_knn(&index, p, k, max_r, &f));

        // Long diagonal casts, radius 0 and > 0.
        let d = [
            rf(&mut rng, -100.0, 100.0),
            rf(&mut rng, -12.0, 12.0),
            rf(&mut rng, -100.0, 100.0),
        ];
        let radius = if n % 2 == 0 {
            0.0
        } else {
            rf(&mut rng, 0.05, 0.6)
        };
        let got = index.cast(p, d, radius, &f);
        assert_eq!(
            hit_bits(got),
            hit_bits(oracle_cast(&index, p, d, radius, &f)),
            "cast #{n}"
        );
    }
}

// ── (b) push-order independence ──────────────────────────────────────────────────────────────

#[test]
fn tree_and_results_ignore_push_order() {
    let items = scene(11);
    let a = built(&shuffled(&items, 1));
    let b = built(&shuffled(&items, 2));
    assert_eq!(a.items(), b.items());
    assert_eq!(a.nodes(), b.nodes());
    let f = Filter::new(7);
    let (mut oa, mut ob) = (Vec::new(), Vec::new());
    a.overlap_sphere([50.0, 6.0, 50.0], 20.0, &f, &mut oa);
    b.overlap_sphere([50.0, 6.0, 50.0], 20.0, &f, &mut ob);
    assert_eq!(oa, ob);
    assert_eq!(
        hit_bits(a.cast([0.0, 6.0, 0.0], [100.0, 0.0, 100.0], 0.2, &f)),
        hit_bits(b.cast([0.0, 6.0, 0.0], [100.0, 0.0, 100.0], 0.2, &f))
    );
}

// ── (c) thread invariance ────────────────────────────────────────────────────────────────────

type Run = (
    Vec<Item>,
    Vec<Node>,
    Vec<KnnBits>,
    Option<(Handle, [u32; 10])>,
);

fn run_all(items: &[Item]) -> Run {
    let index = built(items);
    let mut nb = Vec::new();
    index.k_nearest([40.0, 5.0, 60.0], 9, 30.0, &Filter::new(7), &mut nb);
    let hit = index.cast([1.0, 6.0, 1.0], [98.0, 0.0, 97.0], 0.3, &Filter::new(7));
    (
        index.items().to_vec(),
        index.nodes().to_vec(),
        knn_bits(&nb),
        hit_bits(hit),
    )
}

#[test]
fn build_and_queries_are_thread_count_invariant() {
    let items = scene(5);
    let one = with_threads(1, || run_all(&items));
    let eight = with_threads(8, || run_all(&items));
    // PartialEq on floats equals bit equality here: no NaN is ever stored.
    assert_eq!(one.2, eight.2);
    assert_eq!(one.3, eight.3);
    assert_eq!(one.1, eight.1);
    let bits = |v: &[Item]| {
        v.iter()
            .map(|i| (i.handle, i.volume.bits(), i.layers))
            .collect::<Vec<_>>()
    };
    assert_eq!(bits(&one.0), bits(&eight.0));
}

// ── (d) ties ─────────────────────────────────────────────────────────────────────────────────

#[test]
fn equal_distance_neighbors_resolve_by_handle_regardless_of_push_order() {
    let a = sphere(1, [1.0, 0.0, 0.0], 0.1, 1);
    let b = sphere(2, [-1.0, 0.0, 0.0], 0.1, 1);
    let far = sphere(3, [9.0, 0.0, 0.0], 0.1, 1);
    let f = Filter::new(1);
    for order in [[a, b, far], [far, b, a]] {
        let index = built(&order);
        let mut out = Vec::new();
        index.k_nearest([0.0; 3], 1, 100.0, &f, &mut out);
        assert_eq!(out[0].handle, h(1));
    }
    // Many equal-distance items with k full: the lowest handles survive pruning.
    let ring: Vec<Item> = (0..64)
        .map(|i| {
            let s = if i % 2 == 0 { 1.0 } else { -1.0 };
            let (x, z) = if i % 4 < 2 {
                (s * 3.0, 0.0)
            } else {
                (0.0, s * 3.0)
            };
            sphere(i, [x, 0.0, z], 0.1, 1)
        })
        .chain((100..200).map(|i| sphere(i, [20.0 + i as f32, 0.0, 0.0], 0.1, 1)))
        .collect();
    let index = built(&shuffled(&ring, 3));
    let mut out = Vec::new();
    index.k_nearest([0.0; 3], 5, 100.0, &f, &mut out);
    assert_eq!(knn_bits(&out), oracle_knn(&index, [0.0; 3], 5, 100.0, &f));
    let handles: Vec<u32> = out.iter().map(|n| n.handle.index).collect();
    assert_eq!(handles, vec![0, 1, 2, 3, 4]);
}

#[test]
fn identical_boxes_hit_at_one_t_resolve_to_the_lower_handle() {
    let lo = aabb(4, [5.0, -1.0, -1.0], [6.0, 1.0, 1.0], 1);
    let hi = aabb(9, [5.0, -1.0, -1.0], [6.0, 1.0, 1.0], 1);
    for order in [[lo, hi], [hi, lo]] {
        let hit = built(&order)
            .cast([0.0; 3], [10.0, 0.0, 0.0], 0.0, &Filter::new(1))
            .unwrap();
        assert_eq!(hit.handle, h(4));
    }
}

// ── (e) filters ──────────────────────────────────────────────────────────────────────────────

#[test]
fn layers_exclusion_and_owner_filter() {
    let index = built(&[
        sphere(1, [0.0; 3], 1.0, 1),
        sphere(2, [0.0; 3], 1.0, 2),
        sphere(3, [0.0; 3], 1.0, 1),
    ]);
    let mut out = Vec::new();
    index.overlap_sphere([0.0; 3], 0.5, &Filter::new(1), &mut out);
    assert_eq!(out, vec![h(1), h(3)]);
    index.overlap_sphere([0.0; 3], 0.5, &Filter::new(1).excluding(h(1)), &mut out);
    assert_eq!(out, vec![h(3)]);
    index.overlap_sphere(
        [0.0; 3],
        0.5,
        &Filter::new(3).excluding(h(1)).owned_by(h(3)),
        &mut out,
    );
    assert_eq!(out, vec![h(2)]);
    let mut nb = Vec::new();
    index.k_nearest([0.0; 3], 5, 9.0, &Filter::new(2), &mut nb);
    assert_eq!(nb.len(), 1);
    assert!(
        index
            .cast([5.0, 0.0, 0.0], [-10.0, 0.0, 0.0], 0.0, &Filter::new(4))
            .is_none()
    );
    let hit = index
        .cast(
            [5.0, 0.0, 0.0],
            [-10.0, 0.0, 0.0],
            0.0,
            &Filter::new(1).excluding(h(1)),
        )
        .unwrap();
    assert_eq!(hit.handle, h(3));
}

// ── (f) analytic casts ───────────────────────────────────────────────────────────────────────

fn unit_box() -> Index3d {
    built(&[aabb(0, [0.0; 3], [1.0; 3], 1)])
}

#[test]
fn face_hit_gives_an_exact_axis_normal() {
    let hit = unit_box()
        .cast([-3.0, 0.5, 0.5], [6.0, 0.0, 0.0], 0.5, &Filter::new(1))
        .unwrap();
    assert_eq!(hit.t.to_bits(), (2.5f32 / 6.0).to_bits());
    assert_eq!(
        hit.normal.map(f32::to_bits),
        [(-1.0f32).to_bits(), 0.0f32.to_bits(), 0.0f32.to_bits()]
    );
    assert_eq!(hit.point, [0.0, 0.5, 0.5]);
    let up = unit_box()
        .cast([0.5, 4.0, 0.5], [0.0, -8.0, 0.0], 0.0, &Filter::new(1))
        .unwrap();
    assert_eq!(up.normal, [0.0, 1.0, 0.0]);
    assert_eq!(up.t, 0.375);
}

#[test]
fn edge_hit_uses_the_capsule_normal() {
    // Sweeping along x at y = 1.3 (r = 0.5) clips the box's top-front edge at (0, 1, *).
    let hit = unit_box()
        .cast([-3.0, 1.3, 0.5], [6.0, 0.0, 0.0], 0.5, &Filter::new(1))
        .unwrap();
    let dy = 0.3f32;
    let dx = (0.25f32 - dy * dy).sqrt();
    assert!((hit.t * 6.0 - (3.0 - dx)).abs() < 1e-5);
    assert!((hit.normal[0] + dx / 0.5).abs() < 1e-5 && (hit.normal[1] - dy / 0.5).abs() < 1e-5);
    assert_eq!(hit.normal[2], 0.0);
    assert_eq!(hit.point, [0.0, 1.0, 0.5]);
}

#[test]
fn corner_hit_takes_the_nearest_edge_capsule() {
    let o = [-2.0, -2.0, -2.0];
    let hit = unit_box()
        .cast(o, [2.0, 2.0, 2.0], 0.5, &Filter::new(1))
        .unwrap();
    // Diagonal into the (0,0,0) corner: the sphere touches it at distance 0.5 from the corner.
    let s = (3.0f32).sqrt();
    assert!((hit.t * 2.0 * s - (2.0 * s - 0.5)).abs() < 1e-4);
    for n in hit.normal {
        assert!((n + 1.0 / s).abs() < 1e-4);
    }
    assert_eq!(hit.point, [0.0; 3]);
}

#[test]
fn start_overlap_miss_zero_delta_and_spheres() {
    let b = unit_box();
    let f = Filter::new(1);
    let inside = b.cast([0.2, 0.5, 0.5], [3.0, 0.0, 0.0], 0.0, &f).unwrap();
    assert_eq!((inside.t, inside.normal), (0.0, [-1.0, 0.0, 0.0]));
    let tie = b.cast([0.5; 3], [1.0, 0.0, 0.0], 0.0, &f).unwrap();
    assert_eq!(tie.normal, [1.0, 0.0, 0.0]);
    let touching = b.cast([-0.5, 0.5, 0.5], [0.0, 1.0, 0.0], 0.5, &f).unwrap();
    assert_eq!((touching.t, touching.normal), (0.0, [-1.0, 0.0, 0.0]));
    assert!(
        b.cast([-3.0, 0.5, 0.5], [2.0, 0.0, 0.0], 0.0, &f).is_none(),
        "ends short of t = 1"
    );
    assert!(b.cast([-3.0, 0.5, 0.5], [0.0; 3], 0.0, &f).is_none());
    assert!(b.cast([-3.0, 5.0, 0.5], [9.0, 0.0, 0.0], 0.5, &f).is_none());
    assert!(b.cast([f32::NAN, 0.0, 0.0], [1.0; 3], 0.0, &f).is_none());

    let s = built(&[sphere(0, [5.0, 0.0, 0.0], 1.0, 1)]);
    let hit = s.cast([0.0; 3], [10.0, 0.0, 0.0], 0.5, &f).unwrap();
    assert_eq!(
        (hit.t, hit.normal, hit.point),
        (0.35, [-1.0, 0.0, 0.0], [4.0, 0.0, 0.0])
    );
    let centre = s.cast([5.0, 0.0, 0.0], [1.0, 0.0, 0.0], 0.0, &f).unwrap();
    assert_eq!((centre.t, centre.normal), (0.0, [0.0, 1.0, 0.0]));
}

// ── (g) validation ───────────────────────────────────────────────────────────────────────────

#[test]
fn bad_items_are_skipped_and_counted() {
    let mut index = Index3d::new();
    index.push(sphere(1, [f32::NAN, 0.0, 0.0], 1.0, 1));
    index.push(sphere(2, [0.0; 3], -1.0, 1));
    index.push(sphere(3, [0.0; 3], f32::INFINITY, 1));
    index.push(aabb(4, [1.0; 3], [0.0; 3], 1));
    index.push(sphere(5, [0.0; 3], 1.0, 1));
    index.push(sphere(6, [1.0, 0.0, 0.0], 1.0, 1));
    index.push(sphere(6, [3.0, 0.0, 0.0], 1.0, 1));
    index.push(sphere(6, [9.0, 0.0, 0.0], 1.0, 1));
    assert_eq!(index.skipped(), 4);
    index.build();
    assert_eq!(index.skipped(), 6);
    assert_eq!(index.len(), 2);
    let kept = index.items().iter().find(|i| i.handle == h(6)).unwrap();
    // The duplicate with the smallest sort key survives, whatever the push order was.
    let mut reversed = Index3d::new();
    reversed.push(sphere(6, [9.0, 0.0, 0.0], 1.0, 1));
    reversed.push(sphere(6, [3.0, 0.0, 0.0], 1.0, 1));
    reversed.push(sphere(6, [1.0, 0.0, 0.0], 1.0, 1));
    reversed.build();
    assert_eq!(reversed.items(), &[*kept]);
}

#[test]
fn negative_zero_is_canonicalized() {
    let index = built(&[
        sphere(0, [-0.0, -0.0, -0.0], 1.0, 1),
        aabb(1, [-0.0; 3], [1.0; 3], 1),
    ]);
    for item in index.items() {
        assert!(item.volume.bits().iter().all(|b| *b != (-0.0f32).to_bits()));
    }
}

// ── (h) steady state ─────────────────────────────────────────────────────────────────────────

#[test]
fn a_repeated_query_keeps_its_buffer() {
    let index = built(&scene(3));
    let f = Filter::new(7);
    let (mut handles, mut nb) = (Vec::new(), Vec::new());
    index.overlap_sphere([50.0, 6.0, 50.0], 8.0, &f, &mut handles);
    index.k_nearest([50.0, 6.0, 50.0], 16, 30.0, &f, &mut nb);
    let (hc, nc) = (handles.capacity(), nb.capacity());
    index.overlap_sphere([50.0, 6.0, 50.0], 8.0, &f, &mut handles);
    index.k_nearest([50.0, 6.0, 50.0], 16, 30.0, &f, &mut nb);
    assert_eq!((handles.capacity(), nb.capacity()), (hc, nc));
}

#[test]
fn a_second_build_reuses_every_buffer() {
    let items = scene(13);
    let mut index = built(&items);
    let caps = |i: &Index3d| {
        [
            i.staging.capacity(),
            i.keyed.capacity(),
            i.items.capacity(),
            i.nodes.capacity(),
            i.scratch_handles.capacity(),
            i.scratch_dups.capacity(),
            i.scratch_seen.capacity(),
        ]
    };
    // Include a duplicate so the dedup scratch is exercised on both builds.
    let dup = items[0];
    for item in items.iter().chain([&dup]) {
        index.push(*item);
    }
    index.build();
    let before = caps(&index);
    for item in items.iter().chain([&dup]) {
        index.push(*item);
    }
    index.build();
    assert_eq!(caps(&index), before);
}
