# character-course

A greybox obstacle course for the character motor and the reference run/jump/crouch controller
(`ludion docs input` for input). A public example game crate, shaped like `games/breakout`.

## Project layout

- `src/lib.rs` — `Runner` (tunables, scene data), `RunnerState` (hashed), `Spin`, and the
  lane section's `LaneSwitch` (a sensor that engages or releases the hero's `LaneConstraint`) and
  `Checkpoint` (a sensor that sets the respawn feet) and `CameraZone` (a sensor that switches the engine `CameraRig`'s config); the systems `spin` → `triggers` → `controller`; `game()`, `registry()`.
- `scenes/main.yaml` — the course: collision mesh entity, render meshes, the conveyor, path platform,
  turntable and press (kinematic scene entities), the lane section's sensors and belt past the end
  pad (x 29..52: ramp, pit, drop, belt), the hero, lights, camera.
- `scenes/blender/build.py` — the source of the static geometry and of `scenes/paths/*.path.yaml` (the two moving-part paths, `lane.path.yaml`, the lane the hero
  is held to, and `camera-behind.path.yaml`, the behind rig's rail);
  `course.glb` is its committed export, `scenes/meshes/*` the import of that glb. Never hand-edit
  any of them.
- `tests/route.rs` (the scripted route as a numeric trace, run twice), `tests/replay.rs` (`run.lrp`),
  `tests/camera.rs` (the rig per tick: blends, the snap, the shake, `camera_relative`), `tests/hold.rs` (the controller freezes under a clock hold), `tests/golden.rs` (`goldens/<os>/main.png`, the whole course at rest; `behind.png`, the rig settled behind the hero), `tests/content_drift.rs`,
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
