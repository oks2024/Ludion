---
description: ludion export: a game as an Xcode project for iOS or its simulator, the manifest keys
---

# Exporting to iOS

**Scope:** turning a game project into an Xcode project folder with `ludion export`. What the game
hosts and how it starts is `ludion docs running`; the manifest's other fields are
`ludion docs project-anatomy`.

## The loop

```
rustup target add aarch64-apple-ios-sim            # once (aarch64-apple-ios for a device)
ludion export --target ios-sim --out ../mygame-ios # from the project dir; --project <dir> elsewhere
ludion export --target ios-sim --out ../mygame-ios --debug --run   # + build, install, launch
```

Without `--run` the verb prints the `xcodebuild` line (Release) for the target it built; a device
build signs with your team: `DEVELOPMENT_TEAM=<id> -allowProvisioningUpdates` in place of
`CODE_SIGNING_ALLOWED=NO`. Device `--run` is refused (`L0024`): install a device build from Xcode.

`--run` (simulator only) builds with `xcodebuild`, uses the first booted simulator (else boots the
newest-iOS iPhone), prints `simulator: <udid> (<name>)`, installs, and launches with
`simctl launch --console-pty`, streaming the app's console until it exits. The app exits when its
engine ends, so the command returns.

**Verify a replay in the simulator** — the verdict is read on the console:

```
ludion export --target ios-sim --out ../mygame-ios --debug --replay run.lrp --run -- --replay-verify run.lrp
```

`--replay <FILE>` (repeatable, `--debug` only) copies a file into `content/` by name; arguments after
`--` (`--run` and `--debug` only) are the app's argv, read as `ludion run`'s, with paths relative to
`content/`. Success prints `<path> — keyframe check clean: …`; a divergence prints `L3024`.

**Look at it** (with `--debug --run` in another shell; `ludion profile --attach ws://127.0.0.1:9411`
attaches): `xcrun simctl io <udid> screenshot m.png` writes the portrait framebuffer, so
`sips -r 270 m.png` makes it upright. Touch input cannot be driven from `simctl`.

## Format

`ludion.toml`, read with unknown keys refused (`L0023`):

| Key | Meaning |
| --- | --- |
| `name` | The Xcode project and app name: letters, digits, `_`, `-`. |
| `default_scene` | The hosted scene, a relative path with a directory and no `.` or `..` (`scenes/main.yaml`). |
| `[[import]] max_texture_size` | Optional: cap on the side of every texture map the table cooks (halved until it fits; absent keeps the source size). |
| `[cook] targets`, `textures`, `meshes` | Optional: `targets` is the GPU families `ludion sync` cooks maps for (`desktop` BC7, `ios` ASTC; default `["desktop"]`), `textures = false` skips the cook, `meshes = true` is refused (no binary mesh cook). |
| `[audio] voices_per_sound`, `voices_total` | Optional voice caps (defaults 4 and 64; `ludion docs audio`). The manifest ships in the app's content, so the app honours them. |
| `[ios] bundle_id` | The app's identifier, `A-Z a-z 0-9 . -` only (`_` is refused). |
| `[ios] quality` | Optional: the render quality preset the app ships with (`low`, `medium`, `high`, `full`; absent is `full`). A preset name only — no override file. |

The export folder (`--out`; new, empty, or holding a previous export's `.ludion-export` marker,
and never inside the project):

| Path | Holds |
| --- | --- |
| `<name>.xcodeproj/` | The project, one target, deployment target 16.0, iPhone and iPad. |
| `Host/` | The Objective-C host and its `ludion.h`: a `CAMetalLayer` view that starts the game. |
| `Ludion/libgame.a` | The game's library, built `--release --no-default-features`. |
| `content/` | A copy of the default scene's directory plus every `[[import]]` tree, at its project-relative path, bundled as the content root. |

## Rules

- **The game crate exports `ludion_game_start`** and takes `wire`/`window` as its own features
  (`ludion new` writes both). The export builds without them; `--debug` adds `wire`, so a Ludion
  client can attach to the running app on loopback `9411`.
- **Bundle only what the scene reads.** `content/` is the default scene's directory plus each
  `[[import]]` tree (`ludion sync` runs first; the trees are copied under the sync lock, so a sync
  running beside the export finishes first, and the export says on stderr that it is waiting), so assets sit beside the scenes or under a tree's
  own path; a project's `src/`, `goldens/` and `target/` never enter the bundle.
- **The shipped quality is a default, not a lock.** `LUDION_QUALITY` in the Xcode scheme or a
  `devicectl … --environment-variables` launch still outranks `[ios] quality`, so a device test
  needs no re-export (`ludion docs quality`).
- **Pick your own `bundle_id` before the first device install and keep it.** A free Apple team
  registers only a few app IDs a week; `ludion new` writes `com.example.<name>` as a placeholder.
- **Refusals come before the build.** `L0024` for a flag combination, `L0023` for the manifest, an overlapping or occupied `--out`,
  `L0022` when Xcode's SDK for the target or the rustup target is missing.

## Gotchas

- **The host draws the game's touch controls and forwards touches** (a `touch.yaml` beside the scene defines them); it hides
  the home indicator and defers the edge gestures. Deactivating (Control Center, a call banner) or backgrounding suspends the engine behind a "Tap to
  resume" view; foregrounding does nothing until the next touch, which resumes and is swallowed.
  Closing the scene stops the engine.
- **A simulator app is a Mac process:** its loopback `9411` (`--debug`) contends with a desktop host
  on that port — stop one before starting the other.
- The simulator's Metal is not the Mac's: an engine that refuses to start there prints its diagnostic
  to the launch console; a failed start is logged once and the app stays blank.

## Next

`ludion docs project-anatomy` — the scaffold; `ludion docs running` — hosting and attaching.
