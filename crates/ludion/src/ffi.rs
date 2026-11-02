//! The C ABI over the [embedding core](crate::embed): what a native host (the iOS app's
//! Objective-C) calls. The one module in the workspace that holds `unsafe` — it is the boundary
//! where raw pointers become Rust values — and a thin one: every call validates its arguments,
//! prints an `L0020` diagnostic to stderr for one it refuses, and forwards to [`Embedded`].
//!
//! A game exports the one symbol the host starts it through, handing the facade its builder:
//!
//! ```ignore
//! #[allow(unsafe_code)] // the one symbol the native host calls
//! #[unsafe(no_mangle)]
//! pub unsafe extern "C" fn ludion_game_start(params: *const ludion::ffi::LudionStartParams)
//!     -> *mut ludion::ffi::LudionEmbed {
//!     unsafe { ludion::ffi::start(game(), params) }
//! }
//! ```
//!
//! The C declarations of what the facade exports are `crates/ludion-cli/templates/ios/ludion.h`,
//! which `scripts/lists-in-step.mjs` checks against this module.
//!
//! Button indices follow `GamepadButton`'s declaration order — `South` 0, `East`, `West`, `North`,
//! `LeftShoulder`, `RightShoulder`, `LeftStick`, `RightStick`, `Start`, `Select`, `DpadUp`,
//! `DpadDown`, `DpadLeft`, `DpadRight` 13; the trigger buttons have no index: send the trigger
//! axes (4, 5) and the engine presses them — and axis indices `GamepadAxis`'s: `LeftStickX` 0,
//! `LeftStickY`, `RightStickX`, `RightStickY`, `LeftTrigger`, `RightTrigger` 5. Every forwarder
//! accepts a null handle and returns `false` (or does nothing) for it.
//!
//! Touches go through the game's touch layout ([`touch`](crate::touch)): the host forwards every
//! touch with its `UITouch` identity and draws the controls at the rects `ludion_touch_control`
//! reports, never hit-testing itself. `x` and `y` are physical pixels in the surface — the space
//! `ludion_resize`'s safe rect is in. Touches become pad events only. After `ludion_suspend` the
//! first touch to begin resumes the engine and is swallowed (`LUDION_TOUCH_RESUMED`), so the tap
//! that wakes the game presses nothing.
//!
//! The safe rect is the part of the surface the frame may occupy, in physical pixels; all four zero
//! means the whole surface. A start with a safe rect also sets the render extent from its aspect
//! ([`safe_render_extent`](crate::embed::safe_render_extent)), and `ludion_resize` follows it.
//!
//! A build with the `wire` feature hands the process argv to the boot thread: when it names one of
//! `run`'s modes (`--replay-verify FILE`, …) the engine runs that mode in place of hosting and prints
//! its verdict to the console; a foreign argument only warns. The same build binds the wire listener
//! on loopback `9411`, as [`launch`](crate::launch) does. An argv mode, a wire `engine.shutdown` or a
//! failed boot ends the engine while the app stays alive: `ludion_ended` is how a host learns that
//! and stops. A fatal render fault (a lost device, a dead surface) counts too: `ludion_ended`
//! prints it once, and `ludion_stop` then returns 1.
//!
//! Only UIKit is named as a surface kind: every other shell uses the Rust API.
//!
//! Threads: the forwarders take `&` and may run concurrently from any thread (UIKit's main thread,
//! the display link). `ludion_stop` consumes the handle, so the host guarantees no call is in
//! flight or follows it, and the `UIView*` given to `start` must outlive the engine. A Rust panic
//! inside a forwarder aborts the process (`extern "C"`).

#![deny(clippy::undocumented_unsafe_blocks)]

use std::ffi::{CStr, CString, c_char, c_void};
use std::path::PathBuf;
use std::process::ExitCode;
use std::ptr::NonNull;
use std::sync::atomic::{AtomicBool, Ordering};

use ludion_core::codes::code;
use ludion_core::diagnostics::Diagnostic;
use ludion_core::letterbox::HostRect;
use ludion_engine::EngineBuilder;
use ludion_sim::{GamepadAxis, GamepadButton, InputEvent};
use wgpu::rwh::{RawDisplayHandle, RawWindowHandle, UiKitDisplayHandle, UiKitWindowHandle};

use crate::LaunchRoots;
use crate::embed::{Embedded, StartOptions, Surface, TouchOutcome};
use crate::touch::{ControlKind, TouchPhase};

/// `surface_kind`: no surface — the engine runs headless.
pub const LUDION_SURFACE_NONE: u32 = 0;
/// `surface_kind`: `view` is a `UIView*` (a `CAMetalLayer`-backed view) the engine presents into.
pub const LUDION_SURFACE_UIKIT: u32 = 1;

/// `phase`: the finger went down.
pub const LUDION_TOUCH_BEGAN: u32 = 0;
/// `phase`: the finger moved.
pub const LUDION_TOUCH_MOVED: u32 = 1;
/// `phase`: the finger lifted.
pub const LUDION_TOUCH_ENDED: u32 = 2;
/// `phase`: the system cancelled the touch.
pub const LUDION_TOUCH_CANCELLED: u32 = 3;

/// `ludion_touch` result: the touch went through the layout.
pub const LUDION_TOUCH_MAPPED: i32 = 0;
/// `ludion_touch` result: the touch resumed a suspended engine and was swallowed.
pub const LUDION_TOUCH_RESUMED: i32 = 1;
/// `ludion_touch` result: a null handle, an unknown phase, or a game with no touch layout.
pub const LUDION_TOUCH_REFUSED: i32 = -1;

/// The handle a native host holds: a running [`Embedded`] and the control labels C reads, opaque to
/// C.
pub struct LudionEmbed {
    engine: Embedded,
    /// Each touch control's label, by control index; fixed at start, so a pointer handed to C stays
    /// valid until [`ludion_stop`].
    labels: Vec<Option<CString>>,
    /// Set once [`ludion_ended`] has taken the engine's fatal render fault, so it stays ended and
    /// [`ludion_stop`] reports the failure.
    faulted: AtomicBool,
}

// The host calls the forwarders from several threads through `&`; a non-`Sync` field would break
// here.
const _: fn() = || {
    fn is_sync<T: Sync>() {}
    is_sync::<LudionEmbed>();
};

/// One touch control to draw, laid out for C.
#[repr(C)]
pub struct LudionTouchControl {
    /// 0 left stick, 1 right stick, 2 button.
    pub kind: u32,
    /// Left edge in surface pixels.
    pub x: f32,
    /// Top edge in surface pixels.
    pub y: f32,
    /// Width in surface pixels.
    pub width: f32,
    /// Height in surface pixels.
    pub height: f32,
    /// A button's NUL-terminated label; null for a stick. Owned by the handle.
    pub label: *const c_char,
}

/// The arguments [`start`] reads, laid out for C. Strings are NUL-terminated UTF-8.
#[repr(C)]
pub struct LudionStartParams {
    /// Where content is read from (`LaunchRoots::content`).
    pub content_root: *const c_char,
    /// Where the player profile is written (`LaunchRoots::save`).
    pub save_root: *const c_char,
    /// [`LUDION_SURFACE_NONE`] or [`LUDION_SURFACE_UIKIT`].
    pub surface_kind: u32,
    /// The `UIView*` for a UIKit surface; ignored otherwise.
    pub view: *mut c_void,
    /// The surface's initial size in physical pixels; touches map against it until the first
    /// `ludion_resize`, with or without a surface.
    pub width: u32,
    /// See `width`.
    pub height: u32,
    /// Start the engine paused.
    pub paused: bool,
    /// The safe area inside the surface, in physical pixels; all four zero is the whole surface.
    pub safe_x: u32,
    /// See `safe_x`.
    pub safe_y: u32,
    /// See `safe_x`.
    pub safe_width: u32,
    /// See `safe_x`.
    pub safe_height: u32,
    /// The render quality the build ships with (`PRESET|FILE`), used when `LUDION_QUALITY` names
    /// none; null or empty is none.
    pub quality: *const c_char,
}

/// Every `GamepadButton`, by the index C names it with.
const BUTTONS: [GamepadButton; 14] = [
    GamepadButton::South,
    GamepadButton::East,
    GamepadButton::West,
    GamepadButton::North,
    GamepadButton::LeftShoulder,
    GamepadButton::RightShoulder,
    GamepadButton::LeftStick,
    GamepadButton::RightStick,
    GamepadButton::Start,
    GamepadButton::Select,
    GamepadButton::DpadUp,
    GamepadButton::DpadDown,
    GamepadButton::DpadLeft,
    GamepadButton::DpadRight,
];

/// Every `GamepadAxis`, by the index C names it with.
const AXES: [GamepadAxis; 6] = [
    GamepadAxis::LeftStickX,
    GamepadAxis::LeftStickY,
    GamepadAxis::RightStickX,
    GamepadAxis::RightStickY,
    GamepadAxis::LeftTrigger,
    GamepadAxis::RightTrigger,
];

/// Starts `builder` from C's `params` and returns the handle, or null after printing why to stderr.
/// The one function a game's exported symbol calls; it is not itself `extern`, since the builder is
/// the game's.
///
/// The UIKit surface is created here, on the calling thread — UIKit requires the main thread for the
/// view — and attached on the boot thread.
///
/// # Safety
/// `params` must be null or point to a valid [`LudionStartParams`] whose strings are NUL-terminated
/// and, for a UIKit surface, whose `view` is a live `UIView*` that outlives the returned handle.
pub unsafe fn start(builder: EngineBuilder, params: *const LudionStartParams) -> *mut LudionEmbed {
    #[cfg(feature = "wire")]
    let listen = Some(crate::LAUNCH_ADDR.to_string());
    #[cfg(not(feature = "wire"))]
    let listen = None;
    // SAFETY: the caller guarantees `params` is null or valid.
    unsafe { start_with(builder, params, listen) }
}

/// [`start`] with the wire listener's address named: `None` binds none.
///
/// # Safety
/// As [`start`].
unsafe fn start_with(
    builder: EngineBuilder,
    params: *const LudionStartParams,
    listen: Option<String>,
) -> *mut LudionEmbed {
    // SAFETY: the caller guarantees `params` is null or valid.
    match unsafe { try_start(builder, params, listen) } {
        Ok(engine) => {
            let labels = engine
                .touch_controls()
                .into_iter()
                .map(|control| {
                    control
                        .label
                        .and_then(|label| CString::new(label.replace('\0', "")).ok())
                })
                .collect();
            Box::into_raw(Box::new(LudionEmbed {
                engine,
                labels,
                faulted: AtomicBool::new(false),
            }))
        }
        Err(diagnostic) => {
            eprintln!("{diagnostic}");
            std::ptr::null_mut()
        }
    }
}

unsafe fn try_start(
    builder: EngineBuilder,
    params: *const LudionStartParams,
    listen: Option<String>,
) -> Result<Embedded, Diagnostic> {
    // SAFETY: the caller guarantees `params` is null or valid.
    let params = unsafe { params.as_ref() }.ok_or_else(|| invalid("`params` is null"))?;
    // SAFETY: the caller guarantees the strings are NUL-terminated.
    let content = unsafe { path_arg(params.content_root, "content_root") }?;
    // SAFETY: as above.
    let save = unsafe { path_arg(params.save_root, "save_root") }?;
    // SAFETY: as above.
    let quality = unsafe { optional_str_arg(params.quality, "quality") }?;
    let surface = match params.surface_kind {
        LUDION_SURFACE_NONE => None,
        // SAFETY: the caller guarantees the view is live.
        LUDION_SURFACE_UIKIT => Some(unsafe { uikit_surface(params) }?),
        other => {
            return Err(invalid(format!(
                "`surface_kind` {other} is unknown (0 = none, 1 = UIKit)"
            )));
        }
    };
    let options = StartOptions {
        paused: params.paused,
        size: (params.width != 0 && params.height != 0).then_some((params.width, params.height)),
        safe_rect: safe_rect(
            params.safe_x,
            params.safe_y,
            params.safe_width,
            params.safe_height,
        ),
        packaged_quality: quality,
        #[cfg(feature = "wire")]
        listen,
        #[cfg(feature = "wire")]
        argv: Some(
            std::env::args_os()
                .map(|arg| arg.to_string_lossy().into_owned())
                .collect(),
        ),
        ..StartOptions::default()
    };
    #[cfg(not(feature = "wire"))]
    let _ = listen;
    Embedded::start(builder, LaunchRoots { content, save }, surface, options)
}

/// The four C fields as a safe rect: all zero is the whole surface.
fn safe_rect(x: u32, y: u32, width: u32, height: u32) -> Option<HostRect> {
    (x | y | width | height != 0).then_some(HostRect {
        x,
        y,
        width,
        height,
    })
}

/// A NUL-terminated UTF-8 argument as a path. `OsStr` bytes are `cfg(unix)`; UTF-8 is the contract.
unsafe fn path_arg(raw: *const c_char, name: &str) -> Result<PathBuf, Diagnostic> {
    if raw.is_null() {
        return Err(invalid(format!("`{name}` is null")));
    }
    // SAFETY: non-null, and the caller guarantees NUL termination.
    let text = unsafe { CStr::from_ptr(raw) };
    text.to_str()
        .map(PathBuf::from)
        .map_err(|_| invalid(format!("`{name}` is not valid UTF-8")))
}

/// A nullable string argument: null is `None`.
unsafe fn optional_str_arg(raw: *const c_char, name: &str) -> Result<Option<String>, Diagnostic> {
    if raw.is_null() {
        return Ok(None);
    }
    // SAFETY: non-null, and the caller guarantees NUL termination.
    let text = unsafe { CStr::from_ptr(raw) };
    text.to_str()
        .map(|text| Some(text.to_string()))
        .map_err(|_| invalid(format!("`{name}` is not valid UTF-8")))
}

unsafe fn uikit_surface(params: &LudionStartParams) -> Result<Surface, Diagnostic> {
    let view =
        NonNull::new(params.view).ok_or_else(|| invalid("`view` is null for a UIKit surface"))?;
    let instance = ludion_render_graph::Gpu::instance();
    let target = wgpu::SurfaceTargetUnsafe::RawHandle {
        raw_display_handle: Some(RawDisplayHandle::UiKit(UiKitDisplayHandle::new())),
        raw_window_handle: RawWindowHandle::UiKit(UiKitWindowHandle::new(view)),
    };
    // SAFETY: the caller guarantees the view is a live `UIView*` that outlives the engine.
    let surface = unsafe { instance.create_surface_unsafe(target) }.map_err(|error| {
        Diagnostic::new(
            code::WINDOW_PRESENTATION_UNAVAILABLE,
            format!("could not create a GPU surface for the view: {error}"),
        )
    })?;
    Ok(Surface {
        instance,
        surface,
        width: params.width,
        height: params.height,
    })
}

fn invalid(detail: impl Into<String>) -> Diagnostic {
    Diagnostic::new(code::EMBED_ARGUMENT_INVALID, detail).with_help(
        "pass NUL-terminated UTF-8 roots, `LUDION_SURFACE_NONE` or `LUDION_SURFACE_UIKIT` with its \
         view, and a pad index or touch phase inside the tables in the `ludion::ffi` docs",
    )
}

/// The engine behind a handle, or `None` for a null one.
unsafe fn embedded<'a>(handle: *mut LudionEmbed) -> Option<&'a Embedded> {
    // SAFETY: the caller guarantees the handle is null or a live `start` result.
    unsafe { handle.as_ref() }.map(|embed| &embed.engine)
}

/// Presses (`down`) or releases a gamepad button; `false` for a null handle or an unknown index.
///
/// # Safety
/// `handle` must be null or a live handle from [`start`] that [`ludion_stop`] has not consumed.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ludion_pad_button(
    handle: *mut LudionEmbed,
    button: u32,
    down: bool,
) -> bool {
    // SAFETY: the caller guarantees the handle is null or a live `start` result.
    let Some(engine) = (unsafe { embedded(handle) }) else {
        return false;
    };
    let Some(&button) = BUTTONS.get(button as usize) else {
        eprintln!(
            "{}",
            invalid(format!("pad button {button} is out of range"))
        );
        return false;
    };
    engine.inject(if down {
        InputEvent::GamepadButtonDown { button }
    } else {
        InputEvent::GamepadButtonUp { button }
    });
    true
}

/// Sets a gamepad axis to `value`; `false` for a null handle or an unknown index.
///
/// # Safety
/// `handle` must be null or a live handle from [`start`] that [`ludion_stop`] has not consumed.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ludion_pad_axis(handle: *mut LudionEmbed, axis: u32, value: f32) -> bool {
    // SAFETY: the caller guarantees the handle is null or a live `start` result.
    let Some(engine) = (unsafe { embedded(handle) }) else {
        return false;
    };
    let Some(&axis) = AXES.get(axis as usize) else {
        eprintln!("{}", invalid(format!("pad axis {axis} is out of range")));
        return false;
    };
    engine.inject(InputEvent::GamepadAxis { axis, value });
    true
}

/// Tells the engine the surface's new size and safe rect in physical pixels; all four safe fields
/// zero is the whole surface.
///
/// # Safety
/// `handle` must be null or a live handle from [`start`] that [`ludion_stop`] has not consumed.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ludion_resize(
    handle: *mut LudionEmbed,
    width: u32,
    height: u32,
    safe_x: u32,
    safe_y: u32,
    safe_width: u32,
    safe_height: u32,
) {
    // SAFETY: the caller guarantees the handle is null or a live `start` result.
    if let Some(engine) = unsafe { embedded(handle) } {
        engine.resize(
            width,
            height,
            safe_rect(safe_x, safe_y, safe_width, safe_height),
        );
    }
}

/// Pauses the sim and returns once it has; `false` for a null handle or an ended host. A sim slow to
/// confirm is reported and still `true`: the pause is queued.
///
/// # Safety
/// `handle` must be null or a live handle from [`start`] that [`ludion_stop`] has not consumed.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ludion_suspend(handle: *mut LudionEmbed) -> bool {
    // SAFETY: the caller guarantees the handle is null or a live `start` result.
    let Some(engine) = (unsafe { embedded(handle) }) else {
        return false;
    };
    transition_queued(engine.suspend())
}

/// Resumes the sim and returns once it has; `false` for a null handle or an ended host. A sim slow
/// to confirm is reported and still `true`: the resume is queued.
///
/// # Safety
/// `handle` must be null or a live handle from [`start`] that [`ludion_stop`] has not consumed.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ludion_resume(handle: *mut LudionEmbed) -> bool {
    // SAFETY: the caller guarantees the handle is null or a live `start` result.
    let Some(engine) = (unsafe { embedded(handle) }) else {
        return false;
    };
    transition_queued(engine.resume())
}

/// Whether a suspend/resume took hold: a confirmation timeout (`L3023`) is printed and still counts,
/// because the transition stays queued.
fn transition_queued(result: Result<(), Diagnostic>) -> bool {
    match result {
        Ok(()) => true,
        Err(diag) if diag.code == code::PROTOCOL_SIM_STALLED => {
            eprintln!("{diag}");
            true
        }
        Err(_) => false,
    }
}

/// Whether the engine has ended on its own — an argv mode finished, a wire `engine.shutdown`, a
/// failed boot, a fatal render fault (a lost device, a dead surface; printed once and then
/// reported by [`ludion_stop`] as a failure); `false` for a null handle or a live engine. The
/// handle still needs [`ludion_stop`].
///
/// # Safety
/// `handle` must be null or a live handle from [`start`] that [`ludion_stop`] has not consumed.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ludion_ended(handle: *mut LudionEmbed) -> bool {
    // SAFETY: the caller guarantees the handle is null or a live `start` result.
    let Some(embed) = (unsafe { handle.as_ref() }) else {
        return false;
    };
    if embed.faulted.load(Ordering::Relaxed) {
        return true;
    }
    if let Ok(fault) = embed.engine.render_fault().try_recv() {
        eprintln!("{fault}");
        embed.faulted.store(true, Ordering::Relaxed);
        return true;
    }
    matches!(
        embed.engine.ended().try_recv(),
        Err(crossbeam_channel::TryRecvError::Disconnected)
    )
}

/// Feeds one touch (`phase` is one of the `LUDION_TOUCH_*` phases, `x`/`y` surface pixels) through the
/// touch layout. [`LUDION_TOUCH_RESUMED`] when it resumed a suspended engine, [`LUDION_TOUCH_REFUSED`]
/// for a null handle, an unknown phase or a game with no layout.
///
/// # Safety
/// `handle` must be null or a live handle from [`start`] that [`ludion_stop`] has not consumed.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ludion_touch(
    handle: *mut LudionEmbed,
    id: u64,
    phase: u32,
    x: f32,
    y: f32,
) -> i32 {
    // SAFETY: the caller guarantees the handle is null or a live `start` result.
    let Some(engine) = (unsafe { embedded(handle) }) else {
        return LUDION_TOUCH_REFUSED;
    };
    let phase = match phase {
        LUDION_TOUCH_BEGAN => TouchPhase::Began,
        LUDION_TOUCH_MOVED => TouchPhase::Moved,
        LUDION_TOUCH_ENDED => TouchPhase::Ended,
        LUDION_TOUCH_CANCELLED => TouchPhase::Cancelled,
        other => {
            eprintln!("{}", invalid(format!("touch phase {other} is unknown")));
            return LUDION_TOUCH_REFUSED;
        }
    };
    match engine.touch(id, phase, x, y) {
        TouchOutcome::Mapped => LUDION_TOUCH_MAPPED,
        TouchOutcome::Resumed => LUDION_TOUCH_RESUMED,
        TouchOutcome::NoLayout => LUDION_TOUCH_REFUSED,
    }
}

/// How many touch controls the game's layout has (0 for none or a null handle).
///
/// # Safety
/// `handle` must be null or a live handle from [`start`] that [`ludion_stop`] has not consumed.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ludion_touch_control_count(handle: *mut LudionEmbed) -> u32 {
    // SAFETY: the caller guarantees the handle is null or live.
    unsafe { handle.as_ref() }.map_or(0, |embed| embed.labels.len() as u32)
}

/// Writes control `index` (layout order: sticks, then buttons) to `out`, its rect in the current safe
/// rect; `false` for a null handle or `out`, or an index past the count.
///
/// # Safety
/// `handle` must be null or a live handle from [`start`] that [`ludion_stop`] has not consumed, and
/// `out` null or valid for a write.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ludion_touch_control(
    handle: *mut LudionEmbed,
    index: u32,
    out: *mut LudionTouchControl,
) -> bool {
    // SAFETY: the caller guarantees the handle is null or live.
    let Some(embed) = (unsafe { handle.as_ref() }) else {
        return false;
    };
    let Some(control) = embed
        .engine
        .touch_controls()
        .into_iter()
        .nth(index as usize)
    else {
        return false;
    };
    // SAFETY: the caller guarantees `out` is null or valid for a write.
    let Some(out) = (unsafe { out.as_mut() }) else {
        return false;
    };
    let [x, y, width, height] = control.rect;
    *out = LudionTouchControl {
        kind: match control.kind {
            ControlKind::LeftStick => 0,
            ControlKind::RightStick => 1,
            ControlKind::Button => 2,
        },
        x,
        y,
        width,
        height,
        label: embed
            .labels
            .get(index as usize)
            .and_then(Option::as_ref)
            .map_or(std::ptr::null(), |label| label.as_ptr()),
    };
    true
}

/// Stops the host, waits for it, and frees the handle: 0 on success, 1 on a failed boot, a fatal
/// render fault [`ludion_ended`] reported, or a null handle.
///
/// # Safety
/// `handle` must be null or a live handle from [`start`]; it is consumed and must not be used again.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ludion_stop(handle: *mut LudionEmbed) -> i32 {
    if handle.is_null() {
        return 1;
    }
    // SAFETY: non-null, and the caller hands over the one owner `start` boxed.
    let LudionEmbed {
        engine, faulted, ..
    } = *unsafe { Box::from_raw(handle) };
    if engine.stop() == ExitCode::SUCCESS && !faulted.into_inner() {
        0
    } else {
        1
    }
}

#[cfg(test)]
#[allow(clippy::undocumented_unsafe_blocks)]
mod tests {
    use super::*;
    use crate::embed::test_game::{builder, content_root};
    use ludion_protocol::command::{InputInjectParams, QueryEntitiesParams, SimStepParams};
    use ludion_protocol::{Command, ResponseBody, WireHash};
    use std::ffi::CString;

    /// What a game's exported symbol looks like: the builder is the game's, the params are C's.
    /// It binds no listener: a unit test must not contend for the launch address.
    extern "C" fn game_start(params: *const LudionStartParams) -> *mut LudionEmbed {
        unsafe { start_with(builder(), params, None) }
    }

    fn params(content: &CString, save: &CString) -> LudionStartParams {
        LudionStartParams {
            content_root: content.as_ptr(),
            save_root: save.as_ptr(),
            surface_kind: LUDION_SURFACE_NONE,
            view: std::ptr::null_mut(),
            width: 0,
            height: 0,
            paused: true,
            safe_x: 0,
            safe_y: 0,
            safe_width: 0,
            safe_height: 0,
            quality: std::ptr::null(),
        }
    }

    /// A fatal render fault ends the engine for a native host: `ludion_ended` turns true and
    /// `ludion_stop` reports failure.
    #[cfg(feature = "wire")]
    #[test]
    fn a_render_fault_ends_the_engine_for_a_native_host() {
        let handle = Box::into_raw(Box::new(LudionEmbed {
            engine: crate::embed::test_game::start_faulted("ffi-faulted"),
            labels: vec![],
            faulted: AtomicBool::new(false),
        }));
        let mut polls = 0;
        while !unsafe { ludion_ended(handle) } {
            polls += 1;
            assert!(polls < 1000, "the fault never ended the engine");
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert_eq!(unsafe { ludion_stop(handle) }, 1);
    }

    fn step_hash(embed: &Embedded) -> WireHash {
        let body = embed
            .request(Command::SimStep(SimStepParams {
                ticks: 10,
                hash: true,
            }))
            .expect("the sim thread answers the step");
        let ResponseBody::Stepped { hash: Some(hash) } = body else {
            panic!("a hashed sim.step replies Stepped with a hash");
        };
        hash
    }

    /// The same builder, scene and step, driven with the two `input.inject`s a native host's pad
    /// forwarders emit, written as wire JSON by hand.
    fn reference_hash(content: &std::path::Path, inject: bool) -> WireHash {
        let mut engine = builder()
            .load_scene_file(&content.join("scenes/main.yaml"))
            .unwrap_or_else(|d| panic!("{d}"));
        engine.execute(Command::SimPause).unwrap();
        if inject {
            for event in [
                serde_json::json!({"kind": "gamepad_axis", "axis": "left_stick_x", "value_bits": 1.0f32.to_bits()}),
                serde_json::json!({"kind": "gamepad_button_down", "button": "south"}),
            ] {
                engine
                    .execute(Command::InputInject(InputInjectParams {
                        frame: None,
                        event,
                    }))
                    .unwrap();
            }
        }
        let dispatched = engine
            .execute(Command::SimStep(SimStepParams {
                ticks: 10,
                hash: true,
            }))
            .unwrap();
        let ResponseBody::Stepped { hash: Some(hash) } = dispatched.body else {
            panic!("a hashed sim.step replies Stepped with a hash");
        };
        hash
    }

    /// The exit smoke: start through the C entry, drive pad input through the C forwarders, step,
    /// suspend, resume, stop — and land on the hash the same inputs give a directly built engine.
    #[test]
    fn the_c_abi_drives_the_engine_to_the_reference_hash() {
        let content = content_root("ffi-smoke");
        let content_c = CString::new(content.to_str().unwrap()).unwrap();
        let save_c = CString::new(content.join("save-root").to_str().unwrap()).unwrap();
        let start_params = params(&content_c, &save_c);

        let handle = game_start(&start_params);
        assert!(!handle.is_null(), "start returns a handle");
        unsafe {
            assert!(ludion_pad_axis(handle, 0, 1.0));
            assert!(ludion_pad_button(handle, 0, true));
            ludion_resize(handle, 640, 480, 0, 0, 0, 0);
        }
        let hash = step_hash(unsafe { &(*handle).engine });
        unsafe {
            assert!(ludion_suspend(handle));
            assert!(ludion_resume(handle));
            assert_eq!(ludion_stop(handle), 0);
        }

        assert_eq!(
            hash,
            reference_hash(&content, true),
            "input reached the sim"
        );
        assert_ne!(
            hash,
            reference_hash(&content, false),
            "the reference is sensitive to the input, so equality proves it arrived"
        );
    }

    const TOUCH_LAYOUT: &str = "\
format: 1
kind: touch-layout
sticks:
  - stick: left
    anchor: left
    x: 0.1
    y: 0.4
    width: 0.4
    height: 0.4
    deadzone: 0.2
buttons:
  - button: south
    label: A
    anchor: right
    x: 0.1
    y: 0.1
    width: 0.2
    height: 0.2
";

    /// On a 2000×1000 surface: the stick's centre and a point twice its radius to the right of it,
    /// and a point on the button.
    const STICK_CENTRE: (f32, f32) = (300.0, 600.0);
    const STICK_FULL: (f32, f32) = (700.0, 600.0);
    const BUTTON: (f32, f32) = (1800.0, 200.0);

    fn touch_content(name: &str) -> (std::path::PathBuf, CString, CString) {
        let content = content_root(name);
        std::fs::write(content.join("scenes/touch.yaml"), TOUCH_LAYOUT).unwrap();
        let content_c = CString::new(content.to_str().unwrap()).unwrap();
        let save_c = CString::new(content.join("save-root").to_str().unwrap()).unwrap();
        (content, content_c, save_c)
    }

    fn blank_control() -> LudionTouchControl {
        LudionTouchControl {
            kind: 0,
            x: 0.0,
            y: 0.0,
            width: 0.0,
            height: 0.0,
            label: std::ptr::null(),
        }
    }

    fn mover_x(embed: &Embedded) -> f64 {
        let body = embed
            .request(Command::QueryEntities(QueryEntitiesParams {
                filter: vec!["Mover".into()],
                include: vec!["Mover".into()],
                world: false,
                entities: None,
            }))
            .expect("the sim thread answers the query");
        let ResponseBody::QueryResult { entities } = body else {
            panic!("query.entities replies QueryResult");
        };
        entities[0].components["Mover"]["x"]
            .as_f64()
            .expect("x is a number")
    }

    fn step(embed: &Embedded, ticks: u64) {
        embed
            .request(Command::SimStep(SimStepParams { ticks, hash: false }))
            .expect("the sim thread answers the step");
    }

    /// The exit smoke for touch: a finger on the stick and one on the button reach the sim as the
    /// same two pad events `input.inject` carries, and the layout's controls read back for drawing.
    #[test]
    fn touches_reach_the_sim_as_the_reference_pad_events() {
        let (content, content_c, save_c) = touch_content("ffi-touch");
        let handle = game_start(&params(&content_c, &save_c));
        assert!(!handle.is_null());
        unsafe {
            ludion_resize(handle, 2000, 1000, 0, 0, 0, 0);
            assert_eq!(ludion_touch_control_count(handle), 2);
            let mut control = blank_control();
            assert!(ludion_touch_control(handle, 0, &mut control));
            let stick = &control;
            assert_eq!(
                (stick.kind, stick.x, stick.y, stick.width),
                (0, 100.0, 400.0, 400.0)
            );
            assert!(stick.label.is_null());
            assert!(ludion_touch_control(handle, 1, &mut control));
            let button = &control;
            assert_eq!((button.kind, button.x, button.y), (2, 1700.0, 100.0));
            assert_eq!(CStr::from_ptr(button.label).to_str(), Ok("A"));
            assert!(
                !ludion_touch_control(handle, 2, &mut control),
                "past the count"
            );

            assert_eq!(
                ludion_touch(
                    handle,
                    1,
                    LUDION_TOUCH_BEGAN,
                    STICK_CENTRE.0,
                    STICK_CENTRE.1
                ),
                LUDION_TOUCH_MAPPED
            );
            assert_eq!(
                ludion_touch(handle, 1, LUDION_TOUCH_MOVED, STICK_FULL.0, STICK_FULL.1),
                LUDION_TOUCH_MAPPED
            );
            assert_eq!(
                ludion_touch(handle, 2, LUDION_TOUCH_BEGAN, BUTTON.0, BUTTON.1),
                LUDION_TOUCH_MAPPED
            );
        }
        let hash = step_hash(unsafe { &(*handle).engine });
        unsafe { assert_eq!(ludion_stop(handle), 0) };

        assert_eq!(
            hash,
            reference_hash(&content, true),
            "the touches emitted the reference's stick deflection and South press"
        );
    }

    /// The start's own geometry serves touches with no `ludion_resize`: the params' size, and no
    /// surface to carry it.
    #[test]
    fn touches_map_against_the_start_size_before_any_resize() {
        let (content, content_c, save_c) = touch_content("ffi-touch-seeded");
        let handle = game_start(&LudionStartParams {
            width: 2000,
            height: 1000,
            ..params(&content_c, &save_c)
        });
        assert!(!handle.is_null());
        unsafe {
            for (id, phase, at) in [
                (1, LUDION_TOUCH_BEGAN, STICK_CENTRE),
                (1, LUDION_TOUCH_MOVED, STICK_FULL),
                (2, LUDION_TOUCH_BEGAN, BUTTON),
            ] {
                assert_eq!(
                    ludion_touch(handle, id, phase, at.0, at.1),
                    LUDION_TOUCH_MAPPED
                );
            }
        }
        let hash = step_hash(unsafe { &(*handle).engine });
        unsafe { assert_eq!(ludion_stop(handle), 0) };

        assert_eq!(
            hash,
            reference_hash(&content, true),
            "touches on the start size emitted the reference's stick deflection and South press"
        );
    }

    /// A start safe rect alone, with no size and no resize, is what the controls resolve against.
    #[test]
    fn the_start_safe_rect_resolves_the_controls_before_any_resize() {
        let (_content, content_c, save_c) = touch_content("ffi-touch-safe");
        let handle = game_start(&LudionStartParams {
            safe_x: 100,
            safe_y: 50,
            safe_width: 2000,
            safe_height: 1000,
            ..params(&content_c, &save_c)
        });
        assert!(!handle.is_null());
        unsafe {
            let mut control = blank_control();
            assert!(ludion_touch_control(handle, 0, &mut control));
            assert_eq!(
                (control.x, control.y, control.width),
                (200.0, 450.0, 400.0),
                "the stick sits at the safe rect's offset"
            );
            assert!(ludion_touch_control(handle, 1, &mut control));
            assert_eq!((control.x, control.y), (1800.0, 150.0));
            assert_eq!(ludion_stop(handle), 0);
        }
    }

    /// After a suspend the first touch resumes the engine and is swallowed: the finger that woke the
    /// game neither deflects the stick nor presses the button for the rest of its life.
    #[test]
    fn the_resume_tap_is_swallowed() {
        let (_content, content_c, save_c) = touch_content("ffi-resume");
        let handle = game_start(&params(&content_c, &save_c));
        assert!(!handle.is_null());
        let engine = unsafe { &(*handle).engine };
        unsafe {
            ludion_resize(handle, 2000, 1000, 0, 0, 0, 0);
            // A legitimate stick + button drift, so the reading below is sensitive to input.
            ludion_touch(
                handle,
                1,
                LUDION_TOUCH_BEGAN,
                STICK_CENTRE.0,
                STICK_CENTRE.1,
            );
            ludion_touch(handle, 1, LUDION_TOUCH_MOVED, STICK_FULL.0, STICK_FULL.1);
            ludion_touch(handle, 2, LUDION_TOUCH_BEGAN, BUTTON.0, BUTTON.1);
        }
        step(engine, 5);
        let driven = mover_x(engine);
        assert!(driven > 0.0, "a stick and a button move the mover");

        unsafe {
            assert!(ludion_suspend(handle));
            // The wake-up tap lands on the stick and slides right; a live finger presses the button.
            assert_eq!(
                ludion_touch(
                    handle,
                    9,
                    LUDION_TOUCH_BEGAN,
                    STICK_CENTRE.0,
                    STICK_CENTRE.1
                ),
                LUDION_TOUCH_RESUMED
            );
            assert_eq!(
                ludion_touch(handle, 9, LUDION_TOUCH_MOVED, STICK_FULL.0, STICK_FULL.1),
                LUDION_TOUCH_MAPPED
            );
            assert_eq!(
                ludion_touch(handle, 10, LUDION_TOUCH_BEGAN, BUTTON.0, BUTTON.1),
                LUDION_TOUCH_MAPPED
            );
        }
        step(engine, 5);
        assert_eq!(
            mover_x(engine),
            driven,
            "the swallowed finger deflected nothing (the suspend released the earlier touches)"
        );

        unsafe {
            assert_eq!(
                ludion_touch(handle, 9, LUDION_TOUCH_ENDED, STICK_FULL.0, STICK_FULL.1),
                LUDION_TOUCH_MAPPED
            );
            // Another finger on the stick does deflect it.
            ludion_touch(
                handle,
                11,
                LUDION_TOUCH_BEGAN,
                STICK_CENTRE.0,
                STICK_CENTRE.1,
            );
            ludion_touch(handle, 11, LUDION_TOUCH_MOVED, STICK_FULL.0, STICK_FULL.1);
        }
        step(engine, 5);
        assert!(mover_x(engine) > driven, "a live finger moves the mover");
        unsafe { assert_eq!(ludion_stop(handle), 0) };
    }

    /// A host learns the engine ended without stopping it: a healthy surface-less start is not
    /// ended, one whose boot fails after `start` returned is, and a null handle never is.
    #[test]
    fn ended_reports_a_boot_that_failed_after_the_start_returned() {
        assert!(!unsafe { ludion_ended(std::ptr::null_mut()) });

        let content = content_root("ffi-ended-live");
        let content_c = CString::new(content.to_str().unwrap()).unwrap();
        let save_c = CString::new(content.join("save-root").to_str().unwrap()).unwrap();
        let live = game_start(&params(&content_c, &save_c));
        assert!(!live.is_null());
        assert!(!unsafe { ludion_ended(live) }, "a healthy host is running");
        assert_eq!(unsafe { ludion_stop(live) }, 0);

        let content = content_root("ffi-ended-failed");
        std::fs::remove_file(content.join("scenes/main.yaml")).unwrap();
        let content_c = CString::new(content.to_str().unwrap()).unwrap();
        let save_c = CString::new(content.join("save-root").to_str().unwrap()).unwrap();
        let failed = game_start(&params(&content_c, &save_c));
        assert!(
            !failed.is_null(),
            "the missing scene fails the boot, not the start"
        );
        let mut polls = 0;
        while !unsafe { ludion_ended(failed) } {
            polls += 1;
            assert!(polls < 3000, "the failed boot never reported ended");
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert_eq!(unsafe { ludion_stop(failed) }, 1);
    }

    #[test]
    fn a_null_handle_an_unknown_index_or_a_bad_argument_refuses_without_crashing() {
        unsafe {
            assert!(!ludion_pad_button(std::ptr::null_mut(), 0, true));
            assert!(!ludion_pad_axis(std::ptr::null_mut(), 0, 0.0));
            assert!(!ludion_suspend(std::ptr::null_mut()));
            assert!(!ludion_resume(std::ptr::null_mut()));
            assert_eq!(
                ludion_touch(std::ptr::null_mut(), 1, LUDION_TOUCH_BEGAN, 0.0, 0.0),
                LUDION_TOUCH_REFUSED
            );
            assert_eq!(ludion_touch_control_count(std::ptr::null_mut()), 0);
            assert!(!ludion_touch_control(
                std::ptr::null_mut(),
                0,
                std::ptr::null_mut()
            ));
            ludion_resize(std::ptr::null_mut(), 1, 1, 0, 0, 0, 0);
            assert_eq!(ludion_stop(std::ptr::null_mut()), 1);
        }

        let content = content_root("ffi-refusals");
        let content_c = CString::new(content.to_str().unwrap()).unwrap();
        let save_c = CString::new(content.join("save-root").to_str().unwrap()).unwrap();
        let handle = game_start(&params(&content_c, &save_c));
        assert!(!handle.is_null());
        unsafe {
            assert!(!ludion_pad_button(handle, BUTTONS.len() as u32, true));
            assert!(!ludion_pad_axis(handle, AXES.len() as u32, 0.0));
            assert_eq!(
                ludion_touch(handle, 1, 9, 0.0, 0.0),
                LUDION_TOUCH_REFUSED,
                "an unknown phase"
            );
            assert_eq!(
                ludion_touch(handle, 1, LUDION_TOUCH_BEGAN, 0.0, 0.0),
                LUDION_TOUCH_REFUSED,
                "no layout"
            );
            assert_eq!(ludion_touch_control_count(handle), 0);
            let mut control = blank_control();
            assert!(!ludion_touch_control(handle, 0, &mut control));
            assert_eq!(ludion_stop(handle), 0);
        }

        assert!(game_start(std::ptr::null()).is_null(), "null params");
        let mut null_root = params(&content_c, &save_c);
        null_root.content_root = std::ptr::null();
        assert!(game_start(&null_root).is_null(), "null content root");
        let mut unknown_kind = params(&content_c, &save_c);
        unknown_kind.surface_kind = 9;
        assert!(game_start(&unknown_kind).is_null(), "unknown surface kind");
        let mut null_view = params(&content_c, &save_c);
        null_view.surface_kind = LUDION_SURFACE_UIKIT;
        assert!(game_start(&null_view).is_null(), "null UIKit view");
        // The packaged quality reaches the resolver, unless this box's environment outranks it.
        if std::env::var_os(ludion_engine::QUALITY_ENV).is_none() {
            let typo = CString::new("medum").unwrap();
            let mut bad_quality = params(&content_c, &save_c);
            bad_quality.quality = typo.as_ptr();
            assert!(
                game_start(&bad_quality).is_null(),
                "an unresolvable quality"
            );
        }
    }
}
