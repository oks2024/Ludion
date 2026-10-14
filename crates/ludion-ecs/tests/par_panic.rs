//! A panic inside a `par` region of a system reports like a sequential one: one `L4007`-bound
//! [`SystemPanic`] whose message and location are the **same** panic's — the lowest chunk's — at any
//! thread count, and nothing reaches the previous panic hook.
//!
//! One `#[test]` on purpose: the panic hook is process-global, so a second test in this binary would
//! share the counting hook installed below.

use std::panic::{self, AssertUnwindSafe};
use std::sync::atomic::{AtomicUsize, Ordering};

use ludion_core::par;
use ludion_core::time::{FrameIndex, TickRate};
use ludion_ecs::{Schedule, SystemCtx, SystemPanic, World};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Panics the previous (pre-install) hook was asked to report.
static REACHED_PREVIOUS_HOOK: AtomicUsize = AtomicUsize::new(0);

#[derive(Serialize, Deserialize, JsonSchema, Clone, Copy, Debug)]
struct Row(u32);

const CHUNK_ROWS: usize = 4;

const CHUNK_3_LINE: u32 = line!() + 2;
fn chunk_3_refuses() {
    panic!("chunk 3 refuses")
}

const CHUNK_9_LINE: u32 = line!() + 2;
fn chunk_9_refuses() {
    panic!("chunk 9 refuses")
}

/// Sweeps 64 rows in chunks of four; the first row of chunk 3 and of chunk 9 panic.
fn sweep(world: &mut World, _ctx: &SystemCtx) {
    world.par_for_each::<(&mut Row,), ()>(CHUNK_ROWS, |_ctx, _entity, (row,)| match row.0 {
        12 => chunk_3_refuses(),
        36 => chunk_9_refuses(),
        _ => row.0 += 1,
    });
}

fn world_of_rows() -> World {
    let mut world = World::new();
    for i in 0..64 {
        let entity = world.spawn();
        world.insert(entity, Row(i)).unwrap();
    }
    world
}

#[test]
fn a_parallel_panic_reports_the_lowest_chunks_message_and_site_at_any_count() {
    panic::set_hook(Box::new(|_| {
        REACHED_PREVIOUS_HOOK.fetch_add(1, Ordering::SeqCst);
    }));

    let mut schedule = Schedule::new();
    schedule.add_system("sweep", sweep);
    let ctx = SystemCtx::new(FrameIndex::ZERO, TickRate::DEFAULT);

    let mut reports = Vec::new();
    for count in [1, 2, 8] {
        let mut world = world_of_rows();
        par::with_threads(count, || schedule.run(&mut world, &ctx));
        let report = world
            .remove_resource::<SystemPanic>()
            .expect("the panic must be caught and recorded");
        assert_eq!(report.system, "sweep");
        assert_eq!(report.message, "chunk 3 refuses", "{count} threads");
        let location = report.location.expect("the site must be recorded");
        assert!(
            location.contains(&format!("par_panic.rs:{CHUNK_3_LINE}:")),
            "{count} threads: the site must be chunk 3's, got {location}"
        );
        assert!(!location.contains(&format!("par_panic.rs:{CHUNK_9_LINE}:")));
        reports.push((report.message, location));
    }
    assert!(
        reports.windows(2).all(|pair| pair[0] == pair[1]),
        "1, 2 and 8 threads must report the same panic: {reports:?}"
    );
    assert_eq!(
        REACHED_PREVIOUS_HOOK.load(Ordering::SeqCst),
        0,
        "an in-system panic must never reach the previous hook"
    );

    // Off the boundary nothing is captured: the hook delegates, as it did before.
    let caught = panic::catch_unwind(AssertUnwindSafe(|| {
        par::with_threads(2, || {
            let mut cells = vec![0u32; 8];
            par::par_chunks(&mut cells, 2, |index, _| {
                assert!(index != 1, "off the boundary")
            });
        });
    }));
    assert!(caught.is_err());
    assert!(
        REACHED_PREVIOUS_HOOK.load(Ordering::SeqCst) > 0,
        "outside a system the previous hook must still see the panic"
    );
}
