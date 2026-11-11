//! Deterministic data parallelism — the engine's only sanctioned way to put more than one thread
//! on the sim path.
//!
//! The hazard is completion order. Threads finish when the OS says they finish, so anything whose
//! result depends on *which chunk landed first* — a shared accumulator, a push into a shared `Vec`,
//! a float total combined as the partial sums arrive — makes the sim's output a function of machine
//! load. `WorldHash` then differs between two runs of the same seed, and every replay, golden, and
//! determinism gate built on it is void.
//!
//! The rule: **a result computed here is a pure function of the input and the chunk size, never of
//! the thread count or the interleaving.** Four properties make that true, and every function below
//! preserves them:
//!
//! 1. **Static partitioning.** Chunk boundaries come from `chunk_size` alone. *Which* worker picks
//!    up a chunk is dynamic; where the chunks start and end is not.
//! 2. **Disjoint writes.** [`par_chunks`] hands each chunk an exclusive `&mut` sub-slice, so no two
//!    workers can reach the same element. Anything a chunk closure writes *outside* its own slice is
//!    the caller's bug — the one property here that the types cannot check.
//! 3. **Ordered reduction.** Float addition is not associative: `(a + b) + c` and `a + (b + c)`
//!    differ in the last bits. [`par_map_chunks`] returns per-chunk results in ascending chunk index
//!    regardless of which finished first, and [`ordered_reduce`] folds them in that order.
//! 4. **Deterministic panics.** A panicking chunk does not race its siblings into the caller. Every
//!    chunk is run and joined, then the payload of the **lowest-index** panicking chunk is the one
//!    that propagates — and, inside a [`panic_capture`] scope, its panic site travels with it.
//!
//! **Chunk size is part of the determinism contract.** A float reduction's last bits may legitimately
//! depend on it, so changing `chunk_size` at a call site is a hash-moving edit like any other (the
//! `LUDION_BLESS=1` re-bless / re-record procedure). Thread count is the opposite: the same input at
//! 1, 2, or 8 threads is bit-identical, and that invariance is the whole point of this module.
//!
//! Worker count is a process-global default ([`set_threads`]) with a thread-local scoped override
//! ([`with_threads`]), so concurrent caller threads can hold different counts. The override covers a
//! region's workers too: a nested region inside a chunk resolves the same count. Effective parallelism
//! is `min(count, number of chunks)`; an effective count of `1` runs inline on the calling thread and
//! hands nothing to the pool, so the sequential path is not a second implementation to keep in step —
//! it is this one with no workers.
//!
//! **The default resolves itself, and it is not `1`.** The first time [`threads`] is asked on a
//! process where nothing called [`set_threads`], it reads `LUDION_THREADS` ([`THREADS_ENV`]) and
//! falls back to [`auto_threads`]. Every process therefore exercises the parallel path — the engine,
//! a served game, an example, and the test suite alike — which is the point: this module's
//! invariance claim is only worth what routinely runs against it, and a defect here is invisible on
//! a machine that never spawns a worker. A host with a `--threads` flag should call
//! [`set_startup_threads`] instead of resolving anything itself.
//!
//! Results are independent of the execution strategy, not just of the thread count: the same inputs
//! and chunk size produce the same bytes however the chunks are dispatched.
//!
//! # Example
//!
//! ```
//! use ludion_core::par;
//!
//! // Same input, same chunk size, three thread counts — one answer, bit for bit.
//! let values: Vec<f32> = (0..1000).map(|i| (i as f32) * 0.125 - 50.0).collect();
//! let sum = |threads| {
//!     par::with_threads(threads, || {
//!         par::ordered_reduce(
//!             &values,
//!             64,
//!             |_, chunk: &[f32]| chunk.iter().sum::<f32>(),
//!             0.0f32,
//!             |total, chunk_total| total + chunk_total,
//!         )
//!     })
//! };
//! assert_eq!(sum(1).to_bits(), sum(2).to_bits());
//! assert_eq!(sum(1).to_bits(), sum(8).to_bits());
//!
//! // Disjoint `&mut` chunks: every element is written exactly once, by exactly one worker.
//! let mut cells = vec![0u32; 10];
//! par::with_threads(4, || {
//!     par::par_chunks(&mut cells, 3, |chunk_index, chunk| {
//!         for (offset, cell) in chunk.iter_mut().enumerate() {
//!             *cell = (chunk_index * 3 + offset) as u32;
//!         }
//!     });
//! });
//! assert_eq!(cells, (0..10).collect::<Vec<u32>>());
//! ```

use crate::codes::code;
use crate::diagnostics::Diagnostic;
use crate::panic_capture::{self, PanicSite};
use std::any::Any;
use std::cell::Cell;
use std::fmt;
use std::panic::{self, AssertUnwindSafe};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

/// A caught panic's payload, in the shape [`std::panic::catch_unwind`] hands it over.
type PanicPayload = Box<dyn Any + Send + 'static>;

/// The environment variable naming a machine's worker count: a positive integer, or `auto` for
/// [`auto_threads`]. Unset (or empty) means `auto`.
///
/// This is the per-machine knob — thread count is a property of the box, not of the project, so it
/// lives in the environment rather than in any committed file. A host's `--threads` flag beats it.
pub const THREADS_ENV: &str = "LUDION_THREADS";

/// The largest worker count [`auto_threads`] will pick on its own.
///
/// The rule this encodes: **a default worker count must be one the identity gate proves.** That gate
/// runs the committed replay corpus at 1, 2, and 8 threads, so 8 is the top of the range anything
/// has ever been shown bit-identical at. A bigger machine can still ask for more explicitly — it
/// just will not silently land on a count no gate has run.
pub const AUTO_THREAD_CAP: usize = 8;

/// The [`DEFAULT_THREADS`] value meaning "nothing has resolved this yet". Never an effective count:
/// a worker count of zero is rejected everywhere it can be set.
const UNRESOLVED: usize = 0;

/// Process-wide worker count, used by any thread not currently inside a [`with_threads`] scope.
///
/// Starts [`UNRESOLVED`]; [`threads`] resolves it on first use (see the module docs).
static DEFAULT_THREADS: AtomicUsize = AtomicUsize::new(UNRESOLVED);

thread_local! {
    /// This thread's [`with_threads`] override; `0` means "none, use the process default".
    static THREAD_OVERRIDE: Cell<usize> = const { Cell::new(0) };
}

/// Sets the process-wide worker count, replacing whatever [`threads`] would otherwise resolve.
///
/// This is a throughput knob and nothing else — no result in this module changes when it moves.
/// Prefer [`with_threads`] where a scope will do, and [`set_startup_threads`] when the caller is a
/// host applying a `--threads` flag; this is the plain setter underneath both.
///
/// # Panics
/// Panics if `n` is zero.
///
/// ```
/// use ludion_core::par;
///
/// par::set_threads(4);
/// assert_eq!(par::threads(), 4);
/// par::set_threads(1);
/// ```
pub fn set_threads(n: usize) {
    assert!(n > 0, "set_threads: worker count must be at least 1");
    DEFAULT_THREADS.store(n, Ordering::Relaxed);
}

/// The worker count in effect on the calling thread: its [`with_threads`] override if it is inside
/// one, otherwise the process default — resolving that default from the environment on first use if
/// nothing has set it (see the module docs).
///
/// The resolution here is deliberately lenient: a malformed [`THREADS_ENV`] falls back to
/// [`auto_threads`] rather than derailing a sim query with an error it cannot report. Hosts call
/// [`set_startup_threads`], which is strict, so a typo is caught once at startup where diagnostics
/// belong.
pub fn threads() -> usize {
    let scoped = THREAD_OVERRIDE.with(Cell::get);
    if scoped > 0 {
        return scoped;
    }
    match DEFAULT_THREADS.load(Ordering::Relaxed) {
        // Two threads racing here compute the same value from the same environment, so whichever
        // store lands last stores what the other one already returned.
        UNRESOLVED => {
            let resolved = threads_from_env()
                .ok()
                .flatten()
                .unwrap_or_else(auto_threads);
            DEFAULT_THREADS.store(resolved, Ordering::Relaxed);
            resolved
        }
        resolved => resolved,
    }
}

/// The worker count this machine gets when nothing asked for one: its available parallelism, capped
/// at [`AUTO_THREAD_CAP`], and never zero.
///
/// ```
/// use ludion_core::par;
///
/// assert!((1..=par::AUTO_THREAD_CAP).contains(&par::auto_threads()));
/// ```
pub fn auto_threads() -> usize {
    std::thread::available_parallelism()
        .map(|available| available.get())
        .unwrap_or(1)
        .min(AUTO_THREAD_CAP)
}

/// A [`THREADS_ENV`] value that is neither `auto` nor a positive integer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InvalidThreadsEnv {
    /// The offending value, exactly as it was read.
    pub value: String,
}

impl fmt::Display for InvalidThreadsEnv {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{THREADS_ENV} must be a positive integer or `auto`, but it is `{}`",
            self.value
        )
    }
}

impl std::error::Error for InvalidThreadsEnv {}

impl InvalidThreadsEnv {
    /// This error as the `L0014` diagnostic a host prints before refusing to start.
    pub fn diagnostic(&self) -> Diagnostic {
        Diagnostic::new(code::THREADS_ENV_INVALID, self.to_string()).with_help(format!(
            "set {THREADS_ENV} to a positive integer (e.g. `4`) or to `auto`, or unset it; a \
             host's `--threads` flag overrides it either way"
        ))
    }
}

/// Reads [`THREADS_ENV`]: `Ok(None)` when it is unset or empty, `Ok(Some(n))` for a positive integer
/// or `auto`, and an error for anything else.
pub fn threads_from_env() -> Result<Option<usize>, InvalidThreadsEnv> {
    parse_threads_setting(std::env::var(THREADS_ENV).ok().as_deref())
}

/// The pure half of [`threads_from_env`], so the parse is testable without mutating the process
/// environment (which edition 2024 makes `unsafe`, and this crate forbids).
fn parse_threads_setting(raw: Option<&str>) -> Result<Option<usize>, InvalidThreadsEnv> {
    let Some(raw) = raw else { return Ok(None) };
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }
    if trimmed.eq_ignore_ascii_case("auto") {
        return Ok(Some(auto_threads()));
    }
    match trimmed.parse::<usize>() {
        Ok(n) if n > 0 => Ok(Some(n)),
        _ => Err(InvalidThreadsEnv {
            value: raw.to_string(),
        }),
    }
}

/// Resolves a host process's worker count once at startup, sets it as the process default, and
/// returns it so the host can report what it is running at.
///
/// Precedence is `explicit` (a `--threads` flag) → [`THREADS_ENV`] → [`auto_threads`]. An explicit
/// count wins outright and is not validated against the environment, so a flag still works on a box
/// whose `LUDION_THREADS` is malformed.
///
/// # Errors
/// Returns [`InvalidThreadsEnv`] when `explicit` is `None` and [`THREADS_ENV`] holds neither a
/// positive integer nor `auto`. Hosts should turn that into a diagnostic and refuse to start: the
/// alternative is a process silently running at a count nobody asked for.
///
/// # Panics
/// Panics if `explicit` is `Some(0)`; callers parse the flag with a `1..` range.
pub fn set_startup_threads(explicit: Option<usize>) -> Result<usize, InvalidThreadsEnv> {
    let resolved = match explicit {
        Some(n) => n,
        None => threads_from_env()?.unwrap_or_else(auto_threads),
    };
    set_threads(resolved);
    Ok(resolved)
}

/// Runs `f` with the worker count overridden **on the calling thread only**.
///
/// The override is thread-local, so two threads can hold different counts at the same time, and it
/// is restored when `f` returns or unwinds. Nesting is allowed: the innermost scope wins.
///
/// ```
/// use ludion_core::par;
///
/// par::with_threads(8, || {
///     assert_eq!(par::threads(), 8);
///     par::with_threads(2, || assert_eq!(par::threads(), 2));
///     assert_eq!(par::threads(), 8);
/// });
/// ```
///
/// # Panics
/// Panics if `n` is zero.
pub fn with_threads<R>(n: usize, f: impl FnOnce() -> R) -> R {
    assert!(n > 0, "with_threads: worker count must be at least 1");
    let previous = THREAD_OVERRIDE.with(|slot| slot.replace(n));
    let _restore = RestoreOverride(previous);
    f()
}

/// Puts the previous [`with_threads`] value back, on return and on unwind alike.
struct RestoreOverride(usize);

impl Drop for RestoreOverride {
    fn drop(&mut self) {
        THREAD_OVERRIDE.with(|slot| slot.set(self.0));
    }
}

/// Applies `f` to every `chunk_size`-element chunk of `items`, in parallel, each chunk getting an
/// exclusive `&mut` view of its own elements.
///
/// `f` receives the chunk's index (`0`, `1`, …) and its slice. The final chunk is short when
/// `items.len()` is not a multiple of `chunk_size`. Chunks are dispatched to workers in whatever
/// order they become free, so `f` must write only through the slice it was handed — with that held,
/// the resulting `items` is identical at any worker count.
///
/// ```
/// use ludion_core::par;
///
/// let mut squares = vec![0usize; 7];
/// par::with_threads(3, || {
///     par::par_chunks(&mut squares, 2, |chunk_index, chunk| {
///         for (offset, cell) in chunk.iter_mut().enumerate() {
///             let i = chunk_index * 2 + offset;
///             *cell = i * i;
///         }
///     });
/// });
/// assert_eq!(squares, vec![0, 1, 4, 9, 16, 25, 36]);
/// ```
///
/// # Panics
/// Panics if `chunk_size` is zero. If chunk closures panic, every chunk still runs and is joined,
/// then the lowest-index panic is resumed on the calling thread — and, inside a capturing scope,
/// its panic site travels with it.
pub fn par_chunks<T, F>(items: &mut [T], chunk_size: usize, f: F)
where
    T: Send,
    F: Fn(usize, &mut [T]) + Sync,
{
    assert!(chunk_size > 0, "par_chunks: chunk_size must be non-zero");
    let chunk_count = items.len().div_ceil(chunk_size);
    let count = threads();
    let workers = count.min(chunk_count);

    if workers <= 1 {
        let mut lowest: Option<(PanicPayload, Option<PanicSite>)> = None;
        for (index, chunk) in items.chunks_mut(chunk_size).enumerate() {
            if let Err(payload) = panic::catch_unwind(AssertUnwindSafe(|| f(index, chunk))) {
                // Always taken, so a later chunk's site never overwrites the kept one. Ascending
                // iteration, so the first panic caught is already the lowest-index one.
                let site = panic_capture::take_site();
                lowest.get_or_insert((payload, site));
            }
        }
        if let Some((payload, site)) = lowest {
            panic_capture::put_site(site);
            panic::resume_unwind(payload);
        }
        return;
    }

    // The chunk iterator is the work queue: `ChunksMut` yields sub-slices borrowed from `items`, not
    // from the guard, so a worker holds the lock only long enough to claim its next chunk. Which
    // worker claims which chunk is nondeterministic and irrelevant — the boundaries are not.
    let queue = Mutex::new(items.chunks_mut(chunk_size).enumerate());
    let failures: Mutex<Vec<(usize, PanicPayload, Option<PanicSite>)>> = Mutex::new(Vec::new());
    let capturing = panic_capture::is_capturing();
    {
        let drain = || loop {
            let claimed = lock(&queue).next();
            let Some((index, chunk)) = claimed else { break };
            if let Err(payload) = panic::catch_unwind(AssertUnwindSafe(|| f(index, chunk))) {
                let site = panic_capture::take_site();
                lock(&failures).push((index, payload, site));
            }
        };
        // Helpers are tasks on `par`'s persistent pool (`ludion-par-N`, rapier's pool is separate).
        // The calling thread is one of the workers, so `workers` threads means `workers - 1`
        // spawned tasks and no idle caller.
        pool(count).in_place_scope(|scope| {
            for _ in 1..workers {
                scope.spawn(|_| with_threads(count, || panic_capture::scoped(capturing, drain)));
            }
            drain();
        });
    }

    let lowest = {
        let mut caught = lock(&failures);
        let position = (0..caught.len()).min_by_key(|&i| caught[i].0);
        position.map(|i| caught.swap_remove(i))
    };
    if let Some((_, payload, site)) = lowest {
        panic_capture::put_site(site);
        panic::resume_unwind(payload);
    }
}

/// Maps each `chunk_size`-element chunk of `items` to one value, in parallel, and returns the values
/// **in ascending chunk order** whatever order the chunks finished in.
///
/// `map` receives the chunk's index and its slice; the final chunk is short when `items.len()` is
/// not a multiple of `chunk_size`. An empty `items` yields an empty `Vec`.
///
/// ```
/// use ludion_core::par;
///
/// let items: Vec<u32> = (1..=7).collect();
/// let sums = par::with_threads(4, || {
///     par::par_map_chunks(&items, 3, |_, chunk: &[u32]| chunk.iter().sum::<u32>())
/// });
/// assert_eq!(sums, vec![6, 15, 7]);
/// ```
///
/// # Panics
/// Panics if `chunk_size` is zero, or — after every chunk has run and been joined — with the
/// lowest-index chunk's panic payload.
pub fn par_map_chunks<T, A, M>(items: &[T], chunk_size: usize, map: M) -> Vec<A>
where
    T: Sync,
    A: Send,
    M: Fn(usize, &[T]) -> A + Sync,
{
    assert!(
        chunk_size > 0,
        "par_map_chunks: chunk_size must be non-zero"
    );
    let chunk_count = items.len().div_ceil(chunk_size);
    let mut slots: Vec<Option<A>> = Vec::new();
    slots.resize_with(chunk_count, || None);

    // One slot per chunk, handed out as a one-element `&mut` chunk: the disjoint-write guarantee of
    // `par_chunks` is exactly what makes writing results by index safe, and the slot index *is* the
    // chunk index, so ordering costs nothing.
    par_chunks(&mut slots, 1, |index, slot| {
        let start = index * chunk_size;
        let end = (start + chunk_size).min(items.len());
        slot[0] = Some(map(index, &items[start..end]));
    });

    slots
        .into_iter()
        .map(|slot| slot.expect("par_chunks visits every slot exactly once"))
        .collect()
}

/// Maps each chunk in parallel, then folds the per-chunk results **sequentially in ascending chunk
/// order** — the deterministic way to aggregate.
///
/// This is [`par_map_chunks`] followed by an ordinary left fold, and the fold order is the whole
/// point: float addition is not associative, so combining partial sums as they complete would make
/// the total a function of thread scheduling. The result depends on `chunk_size` (a different
/// grouping is a different sum) and never on the worker count.
///
/// ```
/// use ludion_core::par;
///
/// let values: Vec<f32> = (0..512).map(|i| (i as f32) * 0.03125 - 8.0).collect();
/// let total = |threads| {
///     par::with_threads(threads, || {
///         par::ordered_reduce(
///             &values,
///             32,
///             |_, chunk: &[f32]| chunk.iter().sum::<f32>(),
///             0.0f32,
///             |total, part| total + part,
///         )
///     })
/// };
/// assert_eq!(total(1).to_bits(), total(8).to_bits());
/// ```
///
/// # Panics
/// Panics if `chunk_size` is zero, or with the lowest-index chunk's payload if any chunk panicked.
pub fn ordered_reduce<T, A, B, M, F>(items: &[T], chunk_size: usize, map: M, init: B, fold: F) -> B
where
    T: Sync,
    A: Send,
    M: Fn(usize, &[T]) -> A + Sync,
    F: Fn(B, A) -> B,
{
    par_map_chunks(items, chunk_size, map)
        .into_iter()
        .fold(init, fold)
}

/// The process-wide worker pool, rebuilt larger when a region asks for more threads than it has and
/// never shrunk. A region in flight keeps the pool it fetched alive through its own `Arc`.
static POOL: Mutex<Option<Arc<rayon_core::ThreadPool>>> = Mutex::new(None);

/// The pool for a region of `count` workers: the shared one, grown first if it is too small.
fn pool(count: usize) -> Arc<rayon_core::ThreadPool> {
    let mut slot = lock(&POOL);
    if let Some(existing) = slot.as_ref().filter(|p| p.current_num_threads() >= count) {
        return Arc::clone(existing);
    }
    let built = rayon_core::ThreadPoolBuilder::new()
        .num_threads(count)
        .thread_name(|i| format!("ludion-par-{i}"))
        .build()
        .unwrap_or_else(|e| {
            panic!("ludion_core::par: failed to build a {count}-thread worker pool: {e}")
        });
    let built = Arc::new(built);
    *slot = Some(Arc::clone(&built));
    built
}

/// Locks a bookkeeping mutex, ignoring poisoning.
///
/// No worker holds either lock while running a chunk closure, so a poisoned lock can only mean a
/// panic inside this module's own queue/failure bookkeeping — the data behind it is intact either
/// way, and swallowing the poison keeps the lowest-index panic the one that propagates.
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rng::DeterministicRng;
    use std::sync::Barrier;

    #[test]
    fn workers_inherit_the_regions_thread_count() {
        let mut seen = vec![0usize; 12];
        with_threads(3, || {
            par_chunks(&mut seen, 1, |_, slot| {
                std::thread::sleep(std::time::Duration::from_millis(1));
                slot[0] = threads();
            });
        });
        assert!(seen.iter().all(|&n| n == 3), "{seen:?}");
    }

    #[test]
    fn nested_regions_on_workers_see_the_same_count() {
        let mut outer = vec![0usize; 8];
        with_threads(2, || {
            par_chunks(&mut outer, 1, |_, slot| {
                let mut inner = [0usize; 2];
                par_chunks(&mut inner, 1, |_, s| s[0] = threads());
                slot[0] = inner.iter().copied().min().unwrap_or(0).min(threads());
            });
        });
        assert!(outer.iter().all(|&n| n == 2), "{outer:?}");
    }

    /// The counts the whole module is contracted to agree on.
    const COUNTS: [usize; 3] = [1, 2, 8];

    /// Values with a wide enough dynamic range that regrouping the additions moves the result — so
    /// the invariance assertions below are testing something. `order_is_observable` guards that.
    fn order_sensitive_values(len: usize) -> Vec<f32> {
        let mut rng = DeterministicRng::new(0xf00d);
        (0..len)
            .map(|i| {
                let magnitude = match i % 4 {
                    0 => 1.0e6,
                    1 => 1.0e-3,
                    2 => 1.0e3,
                    _ => 1.0,
                };
                (rng.next_f32() - 0.5) * magnitude
            })
            .collect()
    }

    fn chunk_sum(_: usize, chunk: &[f32]) -> f32 {
        chunk.iter().sum::<f32>()
    }

    fn add(total: f32, part: f32) -> f32 {
        total + part
    }

    /// The reference implementation: chunk, sum each chunk, fold in ascending order — no threads.
    fn sequential_sum(values: &[f32], chunk_size: usize) -> f32 {
        values
            .chunks(chunk_size)
            .map(|chunk| chunk.iter().sum::<f32>())
            .fold(0.0f32, add)
    }

    #[test]
    fn order_is_observable() {
        let values = order_sensitive_values(4096);
        let flat = values.iter().sum::<f32>();
        assert_ne!(
            flat.to_bits(),
            sequential_sum(&values, 64).to_bits(),
            "the fixture must be order-sensitive or the invariance tests prove nothing",
        );
        let baseline = sequential_sum(&values, 64).to_bits();
        let regrouped = [7usize, 63, 100, 512]
            .into_iter()
            .any(|size| sequential_sum(&values, size).to_bits() != baseline);
        assert!(
            regrouped,
            "chunk size is part of the contract: some other grouping must give a different sum",
        );
    }

    #[test]
    fn ordered_reduce_is_thread_count_invariant() {
        let values = order_sensitive_values(4096);
        let expected = sequential_sum(&values, 64);
        for count in COUNTS {
            let total = with_threads(count, || {
                ordered_reduce(&values, 64, chunk_sum, 0.0f32, add)
            });
            assert_eq!(
                total.to_bits(),
                expected.to_bits(),
                "sum at {count} threads must equal the sequential fold bit for bit",
            );
        }
    }

    #[test]
    fn par_chunks_mutation_is_thread_count_invariant() {
        let source = order_sensitive_values(1000);
        let mut expected = source.clone();
        for (index, chunk) in expected.chunks_mut(33).enumerate() {
            for (offset, cell) in chunk.iter_mut().enumerate() {
                *cell = *cell * 2.0 + (index * 33 + offset) as f32;
            }
        }
        for count in COUNTS {
            let mut items = source.clone();
            with_threads(count, || {
                par_chunks(&mut items, 33, |index, chunk| {
                    for (offset, cell) in chunk.iter_mut().enumerate() {
                        *cell = *cell * 2.0 + (index * 33 + offset) as f32;
                    }
                });
            });
            let same = items
                .iter()
                .zip(&expected)
                .all(|(a, b)| a.to_bits() == b.to_bits());
            assert!(
                same,
                "par_chunks at {count} threads diverged from sequential"
            );
        }
    }

    #[test]
    fn par_map_chunks_returns_results_in_chunk_order() {
        let items: Vec<usize> = (0..1000).collect();
        for count in COUNTS {
            let indices = with_threads(count, || {
                par_map_chunks(&items, 64, |index, _: &[usize]| index)
            });
            assert_eq!(indices, (0..16).collect::<Vec<usize>>());

            // The chunk's own contents, not just its index, arrive in order.
            let firsts = with_threads(count, || {
                par_map_chunks(&items, 64, |_, chunk: &[usize]| chunk[0])
            });
            assert_eq!(firsts, (0..16).map(|i| i * 64).collect::<Vec<usize>>());
        }
    }

    #[test]
    fn chunk_boundaries_cover_the_last_partial_chunk() {
        let items: Vec<usize> = (0..10).collect();
        for count in COUNTS {
            let lens = with_threads(count, || {
                par_map_chunks(&items, 3, |_, chunk: &[usize]| chunk.len())
            });
            assert_eq!(lens, vec![3, 3, 3, 1]);

            let mut seen = vec![0usize; 10];
            with_threads(count, || {
                par_chunks(&mut seen, 3, |index, chunk| {
                    for cell in chunk.iter_mut() {
                        *cell = index + 1;
                    }
                });
            });
            assert_eq!(seen, vec![1, 1, 1, 2, 2, 2, 3, 3, 3, 4]);
        }
    }

    #[test]
    fn empty_input_calls_nothing() {
        let empty: [f32; 0] = [];
        for chunk_size in [1usize, 7, 1024] {
            let mut items = empty;
            with_threads(8, || {
                par_chunks(&mut items, chunk_size, |_, _| {
                    unreachable!("no chunks exist")
                });
            });
            let mapped = with_threads(8, || {
                par_map_chunks(&empty, chunk_size, |_, _: &[f32]| -> f32 {
                    unreachable!("no chunks exist")
                })
            });
            assert!(mapped.is_empty());
        }
    }

    #[test]
    #[should_panic(expected = "chunk_size must be non-zero")]
    fn par_chunks_rejects_a_zero_chunk_size() {
        let mut items = [1u32, 2, 3];
        par_chunks(&mut items, 0, |_, _| {});
    }

    #[test]
    #[should_panic(expected = "chunk_size must be non-zero")]
    fn par_map_chunks_rejects_a_zero_chunk_size() {
        let items = [1u32, 2, 3];
        let _ = par_map_chunks(&items, 0, |_, _: &[u32]| 0u32);
    }

    #[test]
    fn the_lowest_index_panic_is_the_one_that_propagates() {
        for count in COUNTS {
            let payload = with_threads(count, || {
                panic::catch_unwind(|| {
                    let mut items = vec![0u32; 5];
                    par_chunks(&mut items, 1, |index, _| {
                        assert!(index < 2, "chunk {index} refuses");
                    });
                })
                .expect_err("a panicking chunk must reach the caller")
            });
            let message = payload
                .downcast_ref::<String>()
                .expect("the payload is the panicking chunk's own message");
            assert_eq!(
                message, "chunk 2 refuses",
                "at {count} threads the lowest-index panic must win",
            );
        }
    }

    #[test]
    fn a_capturing_scope_gets_the_lowest_chunks_site_at_any_count() {
        panic_capture::install_hook();
        let first_line = line!() + 1;
        let first = |index: usize| panic!("chunk {index} fails here");
        let second_line = line!() + 1;
        let second = |index: usize| panic!("chunk {index} fails there");
        for count in COUNTS {
            let (payload, site) = panic_capture::scoped(true, || {
                with_threads(count, || {
                    let payload = panic::catch_unwind(|| {
                        let mut items = vec![0u32; 5];
                        par_chunks(&mut items, 1, |index, _| match index {
                            1 => first(index),
                            3 => second(index),
                            _ => {}
                        });
                    })
                    .expect_err("a panicking chunk must reach the caller");
                    (payload, panic_capture::take_site())
                })
            });
            let message = payload
                .downcast_ref::<String>()
                .expect("a formatted message");
            assert_eq!(message, "chunk 1 fails here", "{count} threads");
            let location = site
                .and_then(|site| site.location)
                .expect("the hook recorded the lowest chunk's site");
            assert!(
                location.contains(&format!("par.rs:{first_line}:")),
                "at {count} threads the site must be chunk 1's, got {location}",
            );
            assert!(!location.contains(&format!("par.rs:{second_line}:")));
        }
    }

    #[test]
    fn with_threads_nests_and_restores() {
        with_threads(8, || {
            assert_eq!(threads(), 8);
            with_threads(2, || {
                assert_eq!(threads(), 2);
                with_threads(4, || assert_eq!(threads(), 4));
                assert_eq!(threads(), 2);
            });
            assert_eq!(threads(), 8);
        });
    }

    #[test]
    fn with_threads_restores_on_unwind() {
        with_threads(6, || {
            let caught = panic::catch_unwind(|| with_threads(3, || panic!("inside the scope")));
            assert!(caught.is_err());
            assert_eq!(threads(), 6, "an unwind must not leave the override behind");
        });
    }

    #[test]
    fn concurrent_threads_hold_independent_overrides() {
        let values = order_sensitive_values(4096);
        let expected = sequential_sum(&values, 64);
        let barrier = Barrier::new(2);
        let observe = |count: usize| {
            with_threads(count, || {
                // Both threads are inside their own override before either computes.
                barrier.wait();
                (
                    threads(),
                    ordered_reduce(&values, 64, chunk_sum, 0.0f32, add),
                )
            })
        };
        let (slow, fast) = std::thread::scope(|scope| {
            let slow = scope.spawn(|| observe(1));
            let fast = scope.spawn(|| observe(8));
            (
                slow.join().expect("the 1-thread observer finishes"),
                fast.join().expect("the 8-thread observer finishes"),
            )
        });
        assert_eq!(slow.0, 1);
        assert_eq!(fast.0, 8);
        assert_eq!(slow.1.to_bits(), expected.to_bits());
        assert_eq!(fast.1.to_bits(), expected.to_bits());
    }

    #[test]
    fn set_threads_moves_the_process_default() {
        // The only test that touches the global. Every other test scopes its count with
        // `with_threads`, whose thread-local override wins, so this cannot perturb them — and it
        // restores the resolved default on the way out rather than assuming a value, since the
        // default now comes from the environment.
        let resolved = threads();
        set_threads(4);
        assert_eq!(threads(), 4);
        with_threads(2, || {
            assert_eq!(threads(), 2, "an override beats the default")
        });
        assert_eq!(threads(), 4);
        set_threads(resolved);
        assert_eq!(threads(), resolved);
    }

    #[test]
    fn the_default_resolves_itself_to_a_usable_count() {
        // Whatever this process resolved (env or auto), it is a real worker count: the parallel
        // path is reachable without a host having configured anything.
        assert!(threads() >= 1);
        assert!(auto_threads() >= 1);
        assert!(
            auto_threads() <= AUTO_THREAD_CAP,
            "auto must stay inside the range the identity gate proves"
        );
    }

    #[test]
    fn threads_env_parses_counts_auto_and_absence() {
        assert_eq!(parse_threads_setting(None), Ok(None), "unset means auto");
        assert_eq!(
            parse_threads_setting(Some("")),
            Ok(None),
            "empty means unset"
        );
        assert_eq!(parse_threads_setting(Some("   ")), Ok(None));
        assert_eq!(parse_threads_setting(Some("4")), Ok(Some(4)));
        assert_eq!(parse_threads_setting(Some(" 8 ")), Ok(Some(8)));
        assert_eq!(
            parse_threads_setting(Some("64")),
            Ok(Some(64)),
            "an explicit count is honoured above the auto cap — the cap only bounds auto"
        );
        assert_eq!(
            parse_threads_setting(Some("auto")),
            Ok(Some(auto_threads()))
        );
        assert_eq!(
            parse_threads_setting(Some("AUTO")),
            Ok(Some(auto_threads()))
        );
    }

    #[test]
    fn threads_env_rejects_what_it_cannot_mean() {
        for bad in ["0", "-2", "eight", "4.5", "4 8"] {
            let error = parse_threads_setting(Some(bad)).expect_err("must not silently pass");
            assert_eq!(error.value, bad);
            assert!(
                error.to_string().contains(THREADS_ENV),
                "the message must name the variable it is about: {error}"
            );
        }
    }

    #[test]
    fn an_explicit_count_beats_the_environment() {
        // `--threads` must work on a box whose LUDION_THREADS is malformed, so the explicit path
        // never consults (or validates) the environment at all.
        let resolved = threads();
        assert_eq!(set_startup_threads(Some(3)), Ok(3));
        assert_eq!(threads(), 3);
        set_threads(resolved);
    }

    #[test]
    #[should_panic(expected = "worker count must be at least 1")]
    fn zero_workers_is_a_panic() {
        with_threads(0, || ());
    }

    /// Thread names seen by the chunks of a sleeping region run at `count` workers.
    fn region_thread_names(count: usize, chunks: usize) -> Vec<Option<String>> {
        let names = Mutex::new(Vec::new());
        let mut items = vec![0u8; chunks];
        with_threads(count, || {
            par_chunks(&mut items, 1, |_, _| {
                std::thread::sleep(std::time::Duration::from_millis(2));
                lock(&names).push(std::thread::current().name().map(str::to_owned));
            });
        });
        names.into_inner().unwrap_or_else(|e| e.into_inner())
    }

    #[test]
    fn helpers_run_on_the_persistent_pool() {
        let caller = std::thread::current().name().map(str::to_owned);
        for _ in 0..2 {
            let names = region_thread_names(4, 16);
            assert_eq!(names.len(), 16);
            for name in names {
                assert!(
                    name == caller
                        || name
                            .as_deref()
                            .is_some_and(|n| n.starts_with("ludion-par-")),
                    "chunk ran on {name:?}, neither the caller {caller:?} nor a pool thread"
                );
            }
        }
    }

    fn nested_total(outer: &[f32]) -> f32 {
        ordered_reduce(
            outer,
            1,
            |_, o| {
                let middle = [o[0], o[0] * 0.5, o[0] * 0.25, o[0] * 0.125];
                ordered_reduce(
                    &middle,
                    1,
                    |_, m| {
                        let leaf = [m[0], m[0] * 0.1];
                        ordered_reduce(&leaf, 1, |_, l| l[0], 0.0f32, |acc, v| acc + v)
                    },
                    0.0f32,
                    |acc, v| acc + v,
                )
            },
            0.0f32,
            |acc, v| acc + v,
        )
    }

    #[test]
    fn nested_regions_three_deep_complete_at_every_count() {
        let data = order_sensitive_values(8);
        let reference = with_threads(1, || nested_total(&data));
        for count in COUNTS {
            let total = with_threads(count, || nested_total(&data));
            assert_eq!(total.to_bits(), reference.to_bits(), "count {count}");
        }
    }

    #[test]
    fn the_pool_grows_for_a_larger_count() {
        let data = order_sensitive_values(64);
        let reference = with_threads(1, || {
            ordered_reduce(&data, 4, chunk_sum, 0.0f32, |a, v| a + v)
        });
        for count in [2, 12, 2] {
            let total = with_threads(count, || {
                ordered_reduce(&data, 4, chunk_sum, 0.0f32, |a, v| a + v)
            });
            assert_eq!(total.to_bits(), reference.to_bits(), "count {count}");
        }
        let names = region_thread_names(12, 48);
        assert!(
            names.iter().flatten().any(|n| n.starts_with("ludion-par-")),
            "no chunk of a 12-worker region ran on a pool thread: {names:?}"
        );
    }
}
