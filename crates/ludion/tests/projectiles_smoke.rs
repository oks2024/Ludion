//! Facade smoke for the projectile pool: `.projectiles(..)`, `projectiles_step` after
//! `spatial3d_rebuild`, `spawn_projectile` and the hit events, all reachable through
//! `use ludion::prelude::*` alone.

use ludion::prelude::*;
use projectiles::{HitOutcome, ProjectileDesc, Response};
use spatial3d::Handle;

fn fire(world: &mut World, ctx: &SystemCtx) {
    if ctx.frame.as_u64() == 0 {
        let mut shot = ProjectileDesc::new([0.0; 3], [60.0, 0.0, 0.0]);
        shot.response = Response::Despawn;
        shot.tag = 7;
        assert!(spawn_projectile(world, shot).is_some());
    }
}

const SCENE: &str = concat!(
    "format: 1\n",
    "entities:\n",
    "  - name: target\n",
    "    components:\n",
    "      Transform:\n",
    "        translation:\n",
    "          - 5.0\n",
    "          - 0.0\n",
    "          - 0.0\n",
    "      SpatialVolume: {}\n",
);

#[test]
fn a_shot_hits_an_entity_through_the_facade() {
    let mut engine = Engine::builder()
        .projectiles(ProjectileConfig::default())
        .system(
            "spatial3d_rebuild",
            spatial3d::rebuild::<Transform, TransformHierarchy>,
        )
        .system("fire", fire)
        .system("projectiles_step", projectiles_step)
        .build()
        .expect("the game builds");
    engine.load_inline_scene(SCENE).expect("the scene loads");

    let mut seen = Vec::new();
    for _ in 0..10 {
        engine.advance_live();
        let events = engine.world().resource::<Projectiles>().unwrap().events();
        seen.extend(events.hits.iter().map(|h| (h.handle, h.outcome, h.tag)));
    }
    let target = engine
        .world()
        .entities()
        .find(|&e| {
            engine
                .world()
                .get::<Transform>(e)
                .is_ok_and(|t| t.is_some())
        })
        .expect("the target");
    assert_eq!(
        seen,
        [(Handle::entity(target), HitOutcome::Despawned, 7)],
        "one hit, on the target, tagged"
    );
}
