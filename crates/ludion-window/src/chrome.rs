//! Window chrome shared by every windowed host: the fullscreen chord, the idle-cursor timer, the
//! HiDPI-aware first window size, the `LUDION_FULLSCREEN` knob and window titles.
//!
//! Everything here is pure (time arrives as a parameter, the platform as a value) so the rules are
//! testable without a window. All of it is host state: nothing reaches the sim, the `WorldHash`, a
//! snapshot or the wire.

use std::time::{Duration, Instant};

use ludion_core::codes::code;
use ludion_core::diagnostics::Diagnostic;
use winit::dpi::LogicalSize;
use winit::event::ElementState;
use winit::event_loop::ActiveEventLoop;
use winit::keyboard::{KeyCode, ModifiersState};

/// How long the cursor may sit still before an auto-hiding window hides it.
pub const CURSOR_IDLE: Duration = Duration::from_secs(2);

/// The share of the monitor a first window may take. winit 0.30 exposes no work area; the margin
/// stands in for the taskbar, menu bar and title bar.
pub const WORK_AREA_FRACTION: f64 = 0.9;

/// The environment variable that opens a game window fullscreen (`1`) or windowed (`0`).
pub const FULLSCREEN_ENV: &str = "LUDION_FULLSCREEN";

/// Which platform's window conventions apply. A value rather than a `cfg`, so tests build each one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HostPlatform {
    /// Alt+Enter or F11.
    Windows,
    /// Ctrl+Cmd+F.
    MacOs,
    /// Behaves like [`HostPlatform::Windows`].
    Other,
}

impl HostPlatform {
    /// The platform this process runs on.
    pub fn current() -> Self {
        match std::env::consts::OS {
            "macos" => Self::MacOs,
            "windows" => Self::Windows,
            _ => Self::Other,
        }
    }
}

/// What a window host does with a key event after the fullscreen chord has seen it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeyRoute {
    /// Not part of the chord: map and inject as usual.
    Forward,
    /// The chord was pressed: toggle fullscreen, inject nothing.
    Toggle,
    /// A repeat or the release of the chord's key: inject nothing.
    Swallow,
}

/// Recognises the platform's fullscreen chord and swallows its key for the whole press, so the game
/// never sees half of it.
#[derive(Debug, Default)]
pub struct FullscreenChord {
    swallowed: Option<KeyCode>,
}

impl FullscreenChord {
    /// Routes one key event. Modifiers themselves are always forwarded: they were pressed before the
    /// chord was complete.
    pub fn on_key(
        &mut self,
        platform: HostPlatform,
        code: KeyCode,
        mods: ModifiersState,
        state: ElementState,
        repeat: bool,
    ) -> KeyRoute {
        if self.swallowed == Some(code) {
            if state == ElementState::Released {
                self.swallowed = None;
            }
            return KeyRoute::Swallow;
        }
        if state == ElementState::Pressed && !repeat && is_chord(platform, code, mods) {
            self.swallowed = Some(code);
            return KeyRoute::Toggle;
        }
        KeyRoute::Forward
    }
}

fn is_chord(platform: HostPlatform, code: KeyCode, mods: ModifiersState) -> bool {
    let (shift, ctrl, alt, logo) = (
        mods.shift_key(),
        mods.control_key(),
        mods.alt_key(),
        mods.super_key(),
    );
    match platform {
        HostPlatform::MacOs => code == KeyCode::KeyF && ctrl && logo && !alt,
        HostPlatform::Windows | HostPlatform::Other => match code {
            KeyCode::Enter | KeyCode::NumpadEnter => alt && !shift && !ctrl && !logo,
            KeyCode::F11 => !alt && !ctrl && !logo,
            _ => false,
        },
    }
}

/// Hides the cursor after [`CURSOR_IDLE`] without motion and shows it again on the next motion.
/// Disabled, it never hides.
#[derive(Debug)]
pub struct CursorAutoHide {
    enabled: bool,
    hidden: bool,
    last_motion: Instant,
}

impl CursorAutoHide {
    /// A timer started at `now`; `enabled == false` never hides.
    pub fn new(enabled: bool, now: Instant) -> Self {
        Self {
            enabled,
            hidden: false,
            last_motion: now,
        }
    }

    /// The cursor moved: `Some(true)` when it must be shown again (it was hidden).
    pub fn on_motion(&mut self, now: Instant) -> Option<bool> {
        self.last_motion = now;
        if self.hidden {
            self.hidden = false;
            return Some(true);
        }
        None
    }

    /// `Some(false)` once, when the cursor has been idle for [`CURSOR_IDLE`] and must be hidden.
    pub fn poll(&mut self, now: Instant) -> Option<bool> {
        if self.enabled && !self.hidden && now >= self.last_motion + CURSOR_IDLE {
            self.hidden = true;
            return Some(false);
        }
        None
    }

    /// When the next [`poll`](Self::poll) may hide the cursor; `None` when disabled or already hidden.
    pub fn deadline(&self) -> Option<Instant> {
        (self.enabled && !self.hidden).then(|| self.last_motion + CURSOR_IDLE)
    }
}

/// The first window's logical size: `requested` shrunk, aspect kept, to fit inside
/// [`WORK_AREA_FRACTION`] of the monitor; never upscaled. No monitor, no change.
pub fn first_window_size(
    requested: LogicalSize<f64>,
    monitor: Option<LogicalSize<f64>>,
) -> LogicalSize<f64> {
    let Some(monitor) = monitor else {
        return requested;
    };
    let scale = (monitor.width * WORK_AREA_FRACTION / requested.width)
        .min(monitor.height * WORK_AREA_FRACTION / requested.height)
        .min(1.0);
    if scale >= 1.0 {
        return requested;
    }
    LogicalSize::new(
        (requested.width * scale).round(),
        (requested.height * scale).round(),
    )
}

/// The primary monitor's size in logical pixels (else the first available monitor's).
pub fn monitor_logical_size(event_loop: &ActiveEventLoop) -> Option<LogicalSize<f64>> {
    let monitor = event_loop
        .primary_monitor()
        .or_else(|| event_loop.available_monitors().next())?;
    Some(monitor.size().to_logical(monitor.scale_factor()))
}

/// Resolves whether a window opens fullscreen: the `--fullscreen` flag beats the environment
/// outright (the environment is not even read), a trimmed `1`/`0` in `env` beats `default`, and an
/// empty or absent `env` defers to it.
///
/// # Errors
/// `L0037` for any other value.
pub fn resolve_fullscreen(
    flag: bool,
    env: Option<&str>,
    default: bool,
) -> Result<bool, Diagnostic> {
    if flag {
        return Ok(true);
    }
    match env.map(str::trim) {
        None | Some("") => Ok(default),
        Some("1") => Ok(true),
        Some("0") => Ok(false),
        Some(other) => Err(Diagnostic::new(
            code::FULLSCREEN_SETTING_INVALID,
            format!("{FULLSCREEN_ENV} is {other:?}, which is neither 1 nor 0"),
        )
        .with_help(format!(
            "set {FULLSCREEN_ENV}=1 to open fullscreen or {FULLSCREEN_ENV}=0 to open windowed, \
             or unset it; the --fullscreen flag overrides it"
        ))),
    }
}

/// [`resolve_fullscreen`] against the process environment.
///
/// # Errors
/// `L0037` for a malformed `LUDION_FULLSCREEN` (unless `flag` is set).
pub fn resolve_startup_fullscreen(flag: bool, default: bool) -> Result<bool, Diagnostic> {
    if flag {
        return Ok(true);
    }
    resolve_fullscreen(
        false,
        std::env::var(FULLSCREEN_ENV).ok().as_deref(),
        default,
    )
}

/// The game's window title: the builder's, else the manifest's `name`, else `Ludion`.
pub fn game_title(builder: Option<&str>, manifest_name: Option<&str>) -> String {
    builder.or(manifest_name).unwrap_or("Ludion").to_owned()
}

/// A development window's title: `"<game> — <scene file>"`, or just the game.
pub fn dev_title(game: &str, scene_file: Option<&str>) -> String {
    match scene_file {
        Some(file) => format!("{game} — {file}"),
        None => game.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PRESSED: ElementState = ElementState::Pressed;
    const RELEASED: ElementState = ElementState::Released;

    fn route(
        chord: &mut FullscreenChord,
        platform: HostPlatform,
        code: KeyCode,
        mods: ModifiersState,
        state: ElementState,
        repeat: bool,
    ) -> KeyRoute {
        chord.on_key(platform, code, mods, state, repeat)
    }

    #[test]
    fn windows_chords_toggle() {
        for (code, mods) in [
            (KeyCode::Enter, ModifiersState::ALT),
            (KeyCode::NumpadEnter, ModifiersState::ALT),
            (KeyCode::F11, ModifiersState::empty()),
        ] {
            let mut chord = FullscreenChord::default();
            assert_eq!(
                route(
                    &mut chord,
                    HostPlatform::Windows,
                    code,
                    mods,
                    PRESSED,
                    false
                ),
                KeyRoute::Toggle,
                "{code:?}"
            );
        }
    }

    #[test]
    fn macos_chord_toggles_and_other_platforms_chords_do_not() {
        let mut chord = FullscreenChord::default();
        let ctrl_cmd = ModifiersState::CONTROL | ModifiersState::SUPER;
        assert_eq!(
            route(
                &mut chord,
                HostPlatform::MacOs,
                KeyCode::KeyF,
                ctrl_cmd,
                PRESSED,
                false
            ),
            KeyRoute::Toggle
        );
        let mut chord = FullscreenChord::default();
        assert_eq!(
            route(
                &mut chord,
                HostPlatform::MacOs,
                KeyCode::Enter,
                ModifiersState::ALT,
                PRESSED,
                false
            ),
            KeyRoute::Forward
        );
        assert_eq!(
            route(
                &mut chord,
                HostPlatform::Windows,
                KeyCode::KeyF,
                ctrl_cmd,
                PRESSED,
                false
            ),
            KeyRoute::Forward
        );
        assert_eq!(
            route(
                &mut chord,
                HostPlatform::Other,
                KeyCode::Enter,
                ModifiersState::ALT,
                PRESSED,
                false
            ),
            KeyRoute::Toggle
        );
    }

    #[test]
    fn near_misses_forward() {
        let mut chord = FullscreenChord::default();
        let alt_shift = ModifiersState::ALT | ModifiersState::SHIFT;
        assert_eq!(
            route(
                &mut chord,
                HostPlatform::Windows,
                KeyCode::Enter,
                alt_shift,
                PRESSED,
                false
            ),
            KeyRoute::Forward
        );
        assert_eq!(
            route(
                &mut chord,
                HostPlatform::Windows,
                KeyCode::F11,
                ModifiersState::CONTROL,
                PRESSED,
                false
            ),
            KeyRoute::Forward
        );
        assert_eq!(
            route(
                &mut chord,
                HostPlatform::Windows,
                KeyCode::Enter,
                ModifiersState::empty(),
                PRESSED,
                false
            ),
            KeyRoute::Forward
        );
    }

    #[test]
    fn the_chord_key_is_swallowed_until_released() {
        let mut chord = FullscreenChord::default();
        let p = HostPlatform::Windows;
        let alt = ModifiersState::ALT;
        assert_eq!(
            route(&mut chord, p, KeyCode::Enter, alt, PRESSED, false),
            KeyRoute::Toggle
        );
        assert_eq!(
            route(&mut chord, p, KeyCode::Enter, alt, PRESSED, true),
            KeyRoute::Swallow
        );
        // The release arrives after Alt was let go.
        assert_eq!(
            route(
                &mut chord,
                p,
                KeyCode::Enter,
                ModifiersState::empty(),
                RELEASED,
                false
            ),
            KeyRoute::Swallow
        );
        assert_eq!(
            route(
                &mut chord,
                p,
                KeyCode::Enter,
                ModifiersState::empty(),
                PRESSED,
                false
            ),
            KeyRoute::Forward,
            "after the release the key is the game's again"
        );
        assert_eq!(
            route(
                &mut chord,
                p,
                KeyCode::KeyA,
                ModifiersState::empty(),
                PRESSED,
                false
            ),
            KeyRoute::Forward
        );
    }

    #[test]
    fn the_cursor_hides_at_exactly_two_seconds_and_motion_rearms_it() {
        let t0 = Instant::now();
        let mut cursor = CursorAutoHide::new(true, t0);
        assert_eq!(cursor.deadline(), Some(t0 + CURSOR_IDLE));
        assert_eq!(
            cursor.poll(t0 + CURSOR_IDLE - Duration::from_millis(1)),
            None
        );
        assert_eq!(cursor.poll(t0 + CURSOR_IDLE), Some(false));
        assert_eq!(cursor.poll(t0 + CURSOR_IDLE * 2), None, "hides once");
        assert_eq!(cursor.deadline(), None);

        let t1 = t0 + Duration::from_secs(5);
        assert_eq!(cursor.on_motion(t1), Some(true));
        assert_eq!(cursor.on_motion(t1), None, "already shown");
        assert_eq!(cursor.deadline(), Some(t1 + CURSOR_IDLE));
        assert_eq!(cursor.poll(t1 + CURSOR_IDLE), Some(false));
    }

    #[test]
    fn a_disabled_cursor_never_hides() {
        let t0 = Instant::now();
        let mut cursor = CursorAutoHide::new(false, t0);
        assert_eq!(cursor.deadline(), None);
        assert_eq!(cursor.poll(t0 + Duration::from_secs(3600)), None);
    }

    #[test]
    fn the_first_size_is_kept_on_a_big_monitor_and_shrunk_on_a_small_one() {
        let requested = LogicalSize::new(1280.0, 720.0);
        // 2560x1440 at 2x is 1280x720 logical: 90% of it is smaller than the request.
        let retina = LogicalSize::new(1280.0, 720.0);
        let shrunk = first_window_size(requested, Some(retina));
        assert!(
            shrunk.width <= 1152.0 && shrunk.height <= 648.0,
            "{shrunk:?}"
        );

        let roomy = LogicalSize::new(2560.0, 1440.0);
        assert_eq!(first_window_size(requested, Some(roomy)), requested);

        let small = first_window_size(requested, Some(LogicalSize::new(1366.0, 768.0)));
        assert!(
            small.width <= 1366.0 * 0.9 && small.height <= 768.0 * 0.9,
            "{small:?}"
        );
        assert!(
            (small.width / small.height - 1280.0 / 720.0).abs() < 0.01,
            "aspect kept: {small:?}"
        );
        assert_eq!(first_window_size(requested, None), requested);
    }

    #[test]
    fn fullscreen_resolution() {
        assert_eq!(
            resolve_fullscreen(true, Some("yes"), false).ok(),
            Some(true)
        );
        assert_eq!(resolve_fullscreen(false, Some("1"), false).ok(), Some(true));
        assert_eq!(
            resolve_fullscreen(false, Some(" 1 "), false).ok(),
            Some(true)
        );
        assert_eq!(resolve_fullscreen(false, Some("0"), true).ok(), Some(false));
        assert_eq!(resolve_fullscreen(false, Some(""), true).ok(), Some(true));
        assert_eq!(resolve_fullscreen(false, None, false).ok(), Some(false));
        let error = resolve_fullscreen(false, Some("yes"), false).unwrap_err();
        assert_eq!(error.code, code::FULLSCREEN_SETTING_INVALID);
        assert!(error.message.contains("yes"), "{}", error.message);
    }

    #[test]
    fn titles() {
        assert_eq!(game_title(Some("A"), Some("B")), "A");
        assert_eq!(game_title(None, Some("B")), "B");
        assert_eq!(game_title(None, None), "Ludion");
        assert_eq!(dev_title("G", Some("main.yaml")), "G — main.yaml");
        assert_eq!(dev_title("G", None), "G");
    }
}
