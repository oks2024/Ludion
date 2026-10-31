//! The [`RenderSettings`] resource: the options-menu switch a game system asks the engine for, the
//! [`Shutdown`](crate::Shutdown) mold.
//!
//! A system is handed `&mut World` and nothing else, so a game's "Anti-aliasing: Off / SMAA" row can
//! only leave a request the engine honors at the frame boundary. The boundary drain stores it on the
//! [`Engine`](crate::Engine), where it outlives world rebuilds, and the next render pushes it to the
//! renderer. It is render-only: never hashed, snapshotted, recorded or on the wire.
//!
//! **It is write-only on purpose.** The effective mode depends on a host flag that differs per
//! machine, so a system that could read it back would let a replay diverge between machines. The game
//! keeps its own option state, in its profile for instance.

use ludion_renderer::AntiAliasing;

/// An ECS resource: render settings a game system requests, honored at the next frame boundary.
///
/// The engine installs one (empty) into every world it builds, so a game system can always
/// `world.resource_mut::<RenderSettings>()` it. The latest request wins over the host's
/// [`set_anti_aliasing`](crate::Engine::set_anti_aliasing) and over the builder default (see
/// [`resolve_anti_aliasing`]), applies from the next frame, and survives a district switch for the
/// rest of the session. A plain resource: excluded from the `WorldHash` and from snapshots.
///
/// # Example
/// ```
/// use ludion_engine::{AntiAliasing, Engine, RenderSettings};
///
/// /// An options-menu row: the one line that turns SMAA on.
/// fn options(world: &mut ludion_ecs::World, _ctx: &ludion_ecs::SystemCtx) {
///     if let Some(settings) = world.resource_mut::<RenderSettings>() {
///         settings.request_anti_aliasing(AntiAliasing::Smaa);
///     }
/// }
///
/// let mut engine = Engine::builder().system("options", options).build().unwrap();
/// assert_eq!(engine.anti_aliasing(), AntiAliasing::Off);
/// engine.advance_live();
/// assert_eq!(engine.anti_aliasing(), AntiAliasing::Smaa);
/// ```
#[derive(Default, Debug)]
pub struct RenderSettings {
    anti_aliasing: Option<AntiAliasing>,
}

impl RenderSettings {
    /// Requests an anti-aliasing mode from the next frame on. The last request in a tick wins.
    pub fn request_anti_aliasing(&mut self, mode: AntiAliasing) {
        self.anti_aliasing = Some(mode);
    }

    /// Whether a request is waiting for the next frame boundary — false again once the engine has
    /// drained it.
    pub fn is_pending(&self) -> bool {
        self.anti_aliasing.is_some()
    }

    /// Takes the pending anti-aliasing request, leaving none (the engine's drain path).
    pub(crate) fn take(&mut self) -> Option<AntiAliasing> {
        self.anti_aliasing.take()
    }
}

/// Resolves the anti-aliasing mode an engine renders with.
///
/// Precedence is `game` (the game's runtime [`RenderSettings`] request) → `host`
/// ([`Engine::set_anti_aliasing`](crate::Engine::set_anti_aliasing)) → `builder` (the
/// game's packaged [`EngineBuilder::anti_aliasing`](crate::EngineBuilder::anti_aliasing), `Off` unless
/// set). Resolved on read from three separately held values, as
/// [`resolve_render_scale`](crate::resolve_render_scale) is, so the order they are set in never matters.
///
/// # Example
/// ```
/// use ludion_engine::{AntiAliasing::{Off, Smaa}, resolve_anti_aliasing};
///
/// assert_eq!(resolve_anti_aliasing(None, None, Smaa), Smaa);
/// // The host beats the packaged default, and the game's own choice beats the host.
/// assert_eq!(resolve_anti_aliasing(None, Some(Off), Smaa), Off);
/// assert_eq!(resolve_anti_aliasing(Some(Smaa), Some(Off), Off), Smaa);
/// ```
pub fn resolve_anti_aliasing(
    game: Option<AntiAliasing>,
    host: Option<AntiAliasing>,
    builder: AntiAliasing,
) -> AntiAliasing {
    game.or(host).unwrap_or(builder)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Engine;
    use ludion_core::time::FrameIndex;

    const MODES: [AntiAliasing; 2] = [AntiAliasing::Off, AntiAliasing::Smaa];

    #[test]
    fn precedence_holds_for_every_combination() {
        let optional = [None, Some(AntiAliasing::Off), Some(AntiAliasing::Smaa)];
        for game in optional {
            for host in optional {
                for builder in MODES {
                    let expected = game.or(host).unwrap_or(builder);
                    assert_eq!(resolve_anti_aliasing(game, host, builder), expected);
                    if let Some(game) = game {
                        assert_eq!(resolve_anti_aliasing(Some(game), host, builder), game);
                    } else if let Some(host) = host {
                        assert_eq!(resolve_anti_aliasing(None, Some(host), builder), host);
                    } else {
                        assert_eq!(resolve_anti_aliasing(None, None, builder), builder);
                    }
                }
            }
        }
    }

    #[test]
    fn the_last_request_wins_and_take_empties() {
        let mut settings = RenderSettings::default();
        assert!(!settings.is_pending());
        settings.request_anti_aliasing(AntiAliasing::Smaa);
        settings.request_anti_aliasing(AntiAliasing::Off);
        assert!(settings.is_pending());
        assert_eq!(settings.take(), Some(AntiAliasing::Off));
        assert!(!settings.is_pending());
        assert_eq!(settings.take(), None);
    }

    #[test]
    fn host_beats_builder_default() {
        let engine = Engine::new();
        assert_eq!(engine.anti_aliasing(), AntiAliasing::Off);
        let mut engine = Engine::builder()
            .anti_aliasing(AntiAliasing::Smaa)
            .build()
            .unwrap();
        assert_eq!(engine.anti_aliasing(), AntiAliasing::Smaa);
        engine.set_anti_aliasing(AntiAliasing::Off);
        assert_eq!(engine.anti_aliasing(), AntiAliasing::Off);
    }

    fn toggle(world: &mut ludion_ecs::World, ctx: &ludion_ecs::SystemCtx) {
        let mode = match ctx.frame {
            FrameIndex(1) => AntiAliasing::Smaa,
            FrameIndex(2) => AntiAliasing::Off,
            _ => return,
        };
        if let Some(settings) = world.resource_mut::<RenderSettings>() {
            settings.request_anti_aliasing(mode);
        }
    }

    #[test]
    fn the_game_beats_the_host_and_toggles_per_frame() {
        let mut engine = Engine::builder().system("toggle", toggle).build().unwrap();
        engine.set_anti_aliasing(AntiAliasing::Off);
        let mut seen = Vec::new();
        for _ in 0..3 {
            engine.advance_live();
            seen.push(engine.anti_aliasing());
        }
        assert_eq!(
            seen,
            [AntiAliasing::Off, AntiAliasing::Smaa, AntiAliasing::Off]
        );
    }

    fn request_smaa(world: &mut ludion_ecs::World, _ctx: &ludion_ecs::SystemCtx) {
        if let Some(settings) = world.resource_mut::<RenderSettings>() {
            settings.request_anti_aliasing(AntiAliasing::Smaa);
        }
    }

    #[test]
    fn the_choice_survives_a_world_rebuild() {
        let mut engine = Engine::builder()
            .system("request", request_smaa)
            .build()
            .unwrap();
        engine.advance_live();
        assert_eq!(engine.anti_aliasing(), AntiAliasing::Smaa);
        engine
            .load_inline_scene("format: 1\nentities: []\n")
            .expect("scene loads");
        assert_eq!(engine.anti_aliasing(), AntiAliasing::Smaa);
    }

    #[test]
    fn describe_reports_the_mode() {
        let engine = Engine::new();
        assert_eq!(engine.describe_document()["anti_aliasing"], "off");
        let engine = Engine::builder()
            .anti_aliasing(AntiAliasing::Smaa)
            .build()
            .unwrap();
        assert_eq!(engine.describe_document()["anti_aliasing"], "smaa");
    }
}
