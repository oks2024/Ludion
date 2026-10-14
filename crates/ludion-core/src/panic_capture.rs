//! Turning an in-boundary panic into a record instead of stderr.
//!
//! A boundary that catches panics (the schedule's unwind boundary in `ludion-ecs`) wants the panic
//! *site* — file, line, backtrace — which only the panic hook sees, and wants the default hook's
//! stderr output suppressed, since the boundary reports the same failure as a structured diagnostic.
//! The hook is process-global; a thread-local capture flag keeps its *effect* local. Inside a
//! [`scoped`]`(true, …)` region the hook prints nothing and stores a [`PanicSite`] for the caller to
//! [`take_site`]; everywhere else it delegates to the hook it replaced, so a panic on a server,
//! audio, or window thread prints exactly as it did before.
//!
//! The flag is inherited into [`par`](crate::par) workers, and `par` carries the site of the chunk
//! whose panic it re-raises back to the calling thread ([`put_site`]), so a panic inside a parallel
//! region reports the same way a sequential one does.

use std::backtrace::{Backtrace, BacktraceStatus};
use std::cell::{Cell, RefCell};
use std::panic;
use std::sync::Once;

/// Where a captured panic happened.
#[derive(Debug)]
pub struct PanicSite {
    /// `file:line:column` of the panic, if the hook was given one.
    pub location: Option<String>,
    /// The captured backtrace, present only when `RUST_BACKTRACE` asked for one.
    pub backtrace: Option<String>,
}

thread_local! {
    /// Whether this thread is inside a capturing scope.
    static CAPTURING: Cell<bool> = const { Cell::new(false) };
    /// The site the hook recorded for the panic now unwinding on this thread.
    static SITE: RefCell<Option<PanicSite>> = const { RefCell::new(None) };
}

/// Guards the one-time installation of the panic hook.
static HOOK: Once = Once::new();

/// Installs, once per process, the panic hook that records a capturing thread's panic instead of
/// printing it. Idempotent.
pub fn install_hook() {
    HOOK.call_once(|| {
        let previous = panic::take_hook();
        panic::set_hook(Box::new(move |info| {
            if !CAPTURING.get() {
                previous(info);
                return;
            }
            let location = info.location().map(ToString::to_string);
            let backtrace = Backtrace::capture();
            let backtrace =
                (backtrace.status() == BacktraceStatus::Captured).then(|| backtrace.to_string());
            SITE.with_borrow_mut(|site| {
                *site = Some(PanicSite {
                    location,
                    backtrace,
                });
            });
        }));
    });
}

/// Whether the calling thread is inside a capturing scope.
pub fn is_capturing() -> bool {
    CAPTURING.get()
}

/// Runs `f` with the capture flag set to `capturing`, restoring the previous value on return and on
/// unwind alike. The hook fires before unwinding starts, so the flag is still set when it reads it.
pub fn scoped<R>(capturing: bool, f: impl FnOnce() -> R) -> R {
    struct Restore(bool);
    impl Drop for Restore {
        fn drop(&mut self) {
            CAPTURING.set(self.0);
        }
    }
    let _restore = Restore(CAPTURING.replace(capturing));
    f()
}

/// Takes the site the hook recorded on this thread, leaving the slot empty.
pub fn take_site() -> Option<PanicSite> {
    SITE.with_borrow_mut(Option::take)
}

/// Puts `site` in this thread's slot — what `par` uses to hand a worker's site to the calling
/// thread just before it re-raises that worker's panic.
pub fn put_site(site: Option<PanicSite>) {
    SITE.with_borrow_mut(|slot| *slot = site);
}
