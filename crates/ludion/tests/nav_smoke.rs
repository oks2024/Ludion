//! Facade smoke for navigation: `.nav(..)` and `nav_update` build a grid from a static collider and
//! the flow cache answers, all reachable through `use ludion::prelude::*` alone (the grid types are `ludion::nav`).

use ludion::prelude::*;

const SCENE: &str = concat!(
    "format: 1\n",
    "physics:\n",
    "  space: free\n",
    "entities:\n",
    "  - name: floor\n",
    "    components:\n",
    "      Transform:\n",
    "        translation:\n",
    "          - 4.0\n",
    "          - -0.5\n",
    "          - 4.0\n",
    "      Collider:\n",
    "        shape:\n",
    "          box:\n",
    "            half_extents:\n",
    "              - 4.0\n",
    "              - 0.5\n",
    "              - 4.0\n",
    "        body: static\n",
);

#[test]
fn a_floor_becomes_a_flow_field_through_the_facade() {
    let mut engine = Engine::builder()
        .nav(NavConfig {
            nx: 8,
            nz: 8,
            ..NavConfig::default()
        })
        .system("nav_update", nav_update)
        .build()
        .expect("the game builds");
    engine.load_inline_scene(SCENE).expect("the scene loads");
    for _ in 0..3 {
        engine.advance_live();
    }
    let (grid, cache) = engine.world_mut().resource_mut::<Nav>().unwrap().parts();
    let grid = grid.expect("the grid is built");
    assert_eq!(grid.node_count(), 64, "an 8 x 8 floor, every cell walkable");
    let target = grid.node_at([0.5, 0.0, 0.5]).unwrap();
    let flow = cache
        .field(grid, target)
        .flow_at(grid, [6.5, 0.0, 6.5])
        .unwrap();
    assert!(
        flow[0] < 0.0 && flow[1] < 0.0,
        "toward the corner: {flow:?}"
    );
}
