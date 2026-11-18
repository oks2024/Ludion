//! The plain-data rumble vocabulary shared across the sim→pad boundary.
//!
//! A [`RumbleCommand`] is a request to the window host's gamepad thread: play a force-feedback
//! effect, or stop everything. The engine's `Rumble` resource emits them, the pad thread
//! (`ludion-window`) receives them, and neither may depend on the other, so the type lives here, in
//! the dependency-light core, like the [`audio`](crate::audio) messages.
//!
//! The values are already validated by the emitter: both motors are in `0..=1` and `seconds` is in
//! `(0, 2]`. The receiver mixes and plays them; it never has to reject one.

/// One instruction for the gamepad's rumble motors.
///
/// # Example
///
/// ```
/// use ludion_core::rumble::RumbleCommand;
///
/// let cmd = RumbleCommand::Play { low: 0.8, high: 0.3, seconds: 0.18 };
/// assert_ne!(cmd, RumbleCommand::StopAll);
/// ```
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum RumbleCommand {
    /// Play an effect: `low` drives the strong motor and `high` the weak one, for `seconds` in
    /// `(0, 2]`.
    Play {
        /// The low-frequency motor strength, `0..=1`.
        low: f32,
        /// The high-frequency motor strength, `0..=1`.
        high: f32,
        /// How long it plays, in seconds.
        seconds: f32,
    },
    /// Stop every playing effect.
    StopAll,
}
