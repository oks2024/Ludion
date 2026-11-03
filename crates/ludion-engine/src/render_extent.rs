//! The live render extent as world state: the [`RenderExtent`] resource a game system reads before
//! it maps a cursor pixel back into the world.
//!
//! The extent itself is host state on the [`Engine`](crate::Engine); how a machine chooses it is
//! [`resolve_startup_render_size`](crate::resolve_startup_render_size)'s half. This module is the
//! read side: a system holds only `&World`, so without a resource the one number every pointer
//! mapping needs — `ludion::math::list_hit`'s `viewport`, `ludion::math::camera_ray`'s — has to be a
//! constant in the game, which is wrong the moment a host runs at any other extent.

use crate::engine::DEFAULT_RENDER_SIZE;

/// The extent, in pixels, the current session renders and streams at — the space
/// `MouseState::position` reports a cursor in.
///
/// Reach it from a system as `world.resource::<RenderExtent>()`. The engine installs it on every
/// world it builds and rewrites it whenever the session's extent moves (a host's `--render-size` /
/// `LUDION_RENDER_SIZE` at startup, a client's `render.size` mid-session), so a system always reads
/// the live number rather than a guess.
///
/// **Never hashed.** Like [`Diagnostics`](crate::Diagnostics) it is engine-owned world state
/// outside the component registry, so it never reaches `WorldHash`; a game that never reads it
/// hashes identically at every extent. The snapshot document carries the extent beside the world
/// (`render_extent`), and a restore applies it.
///
/// **It is sim input, so a recording carries it:** the header holds the extent at arm time and an
/// applied `render.size` is a record, so a session recorded at extent E is replayed at E. Two
/// sessions *played* at different extents are still different input traces (mouse events are
/// render-target pixels), which is why committed `run.lrp`s are recorded at the default extent.
///
/// # Example
/// ```
/// use ludion_engine::{Engine, RenderExtent};
///
/// let mut engine = Engine::new();
/// engine.set_render_size(960, 540);
/// let extent = engine.world().resource::<RenderExtent>().copied().unwrap();
/// assert_eq!(extent.size(), (960, 540));
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RenderExtent {
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
}

impl RenderExtent {
    /// The extent as the `(width, height)` pair every viewport-parameterised helper takes
    /// (`ludion::math::list_hit`, `ludion::math::camera_ray`).
    pub fn size(&self) -> (u32, u32) {
        (self.width, self.height)
    }
}

impl Default for RenderExtent {
    /// The engine's own default extent — what a headless engine, and every test and replay that
    /// never names one, carries.
    fn default() -> Self {
        let (width, height) = DEFAULT_RENDER_SIZE;
        RenderExtent { width, height }
    }
}

#[cfg(test)]
mod tests {
    use super::RenderExtent;
    use crate::{Engine, SceneSwitch};
    use ludion_core::ids::EntityId;
    use ludion_ecs::{SystemCtx, World};
    use std::path::{Path, PathBuf};

    const SCENE: &str = "\
format: 1
entities:
  - name: player
    components:
      Transform:
        translation:
          - 1.0
          - 2.0
          - 3.0
";

    const NEXT: &str = "\
format: 1
entities:
  - name: gate-from-start
    components:
      Transform:
        translation:
          - 4.0
          - 0.0
          - 0.0
";

    /// Requests a district switch on frame 2 (the `tests/transition.rs` gate mold), carrying nothing.
    fn switch_on_frame_2(world: &mut World, ctx: &SystemCtx) {
        if ctx.frame.as_u64() != 2 {
            return;
        }
        if let Some(switch) = world.resource_mut::<SceneSwitch>() {
            switch.request(
                "next.yaml".to_string(),
                "gate-from-start".to_string(),
                Vec::<EntityId>::new(),
            );
        }
    }

    fn fresh_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "ludion-render-extent-{}-{name}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("next.yaml"), NEXT).unwrap();
        dir
    }

    fn extent(engine: &Engine) -> Option<(u32, u32)> {
        engine
            .world()
            .resource::<RenderExtent>()
            .map(RenderExtent::size)
    }

    /// A fresh engine carries the default extent, and `set_render_size` — the one field writer, and
    /// what both the `--render-size` host path and the `render.size` command go through — rewrites
    /// the live world's resource. A `scene.load` reinstalls the session's extent, not the default.
    #[test]
    fn the_render_extent_resource_follows_the_session_extent() {
        let mut engine = Engine::new();
        assert_eq!(
            engine.world().resource::<RenderExtent>().copied(),
            Some(RenderExtent::default()),
            "a headless engine carries the default extent"
        );

        engine.set_render_size(960, 540);
        assert_eq!(
            extent(&engine),
            Some((960, 540)),
            "the setter mirrors onto the live world"
        );

        engine
            .load_inline_scene("format: 1\nentities: []\n")
            .unwrap();
        assert_eq!(
            extent(&engine),
            Some((960, 540)),
            "a scene.load reinstalls the current extent, not the default"
        );
    }

    /// A district switch continues the session, so the arriving world carries the session's extent.
    #[test]
    fn a_scene_switch_carries_the_render_extent() {
        let dir = fresh_dir("switch");
        let mut engine = engine_at(&dir);
        engine.set_render_size(640, 360);
        // Frames 0, 1, then 2 — whose boundary performs the switch — and one in the new district.
        for _ in 0..4 {
            engine.advance_live();
        }
        assert!(
            engine.last_transition_error().is_none(),
            "the switch is clean: {:?}",
            engine.last_transition_error()
        );
        assert!(
            engine
                .world_mut()
                .query::<(&crate::Name,)>()
                .any(|(name,)| name.0 == "gate-from-start"),
            "the arriving district is loaded"
        );
        assert_eq!(
            extent(&engine),
            Some((640, 360)),
            "the arriving district reads the session's extent"
        );
    }

    fn engine_at(root: &Path) -> Engine {
        let mut engine = Engine::builder()
            .system("switch_on_frame_2", switch_on_frame_2)
            .project_root(root)
            .build()
            .expect("the test engine builds");
        engine.load_inline_scene(SCENE).expect("the scene loads");
        engine
    }
}
