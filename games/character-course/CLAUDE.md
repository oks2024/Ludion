# character-course

A greybox obstacle course for the character motor and the reference run/jump/crouch controller
(`ludion docs systems` for input). A public example game crate, shaped like `games/breakout`.

## Project layout

- `src/lib.rs` — `Runner` (tunables, scene data), `RunnerState` (hashed), `Spin`, `FollowCam`; the
  systems `spin` → `controller` → `follow_camera`; `game()`, `registry()`.
- `scenes/main.yaml` — the course: collision mesh entity, render meshes, the conveyor, path platform,
  turntable and press (kinematic scene entities), the hero, lights, camera.
- `scenes/blender/build.py` — the source of the static geometry and of `scenes/paths/*.path.yaml`;
  `course.glb` is its committed export, `scenes/meshes/*` the import of that glb. Never hand-edit
  any of them.
- `tests/route.rs` (the scripted route as a numeric trace, run twice), `tests/replay.rs` (`run.lrp`),
  `tests/golden.rs` (`goldens/<os>/main.png`, the whole course at rest), `tests/content_drift.rs`,
  `tests/drift.rs`; `tests/common/mod.rs` holds the one route script they share.

## The loop

```
blender -b --factory-startup --python scenes/blender/build.py -- scenes/blender/course.glb   # run twice, cmp the glbs
ludion import scenes/blender/course.glb --out target/tmp/course-import   # copy its meshes/ into scenes/meshes/
cargo test -p character-course
cargo run -p character-course -- --scene games/character-course/scenes/main.yaml --window
```

A change to the route timing or the course re-records `run.lrp` last
(`LUDION_BLESS=1 cargo test -p character-course --test replay`); the golden's bless belongs to the
pinned GPU of each OS.
