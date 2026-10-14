---
description: For game designers — describe changes to the agent and watch them go live with ludion dev
---

# The designer's flow

You are a game designer working with Ludion. You do not write code, and you do not need to. **The
agent is your compiler.** You describe what the game should *do* — in plain language — and the agent
translates that into the Rust the engine runs, then shows you the result live. Your job is to
describe intent well and to judge what you see. This page is how that loop works.

## The whole loop in one picture

1. You describe a change ("the paddle should move faster", "make the win light pulse gold").
2. The agent edits the game and saves.
3. `ludion dev` — a command left running in a terminal — notices the save, rebuilds, and swaps the
   new behavior onto the *running* game **without losing your place**. Your score, positions, and
   progress are preserved.
4. You watch it change live, in a window or the viewer, and say what to adjust next.

That is the entire cycle. Most iterations never need you to touch anything but words.

## What to ask for, and how to phrase it

Describe the **behavior and the feel**, not the code. The agent knows the engine; you know the game.

Good asks — concrete, about intent:

- "The ball is too slow — make it about twice as fast."
- "When the player reaches the exit, flash the room green for half a second."
- "Boxes shouldn't slide through walls."
- "The room is too dark; light it warmly, like a lamp in the corner."
- "Add a second level that's the same but with two extra crates."

Less useful asks — vague, or trying to write the code for the agent:

- "Fix the game." (Fix *what*? Describe what's wrong and what it should do instead.)
- "Change the velocity variable to 12.0." (Maybe — but tell the agent the *goal* and let it pick
  the number; "make it noticeably faster" is a better brief.)

If you can name **what you see now** and **what you want to see instead**, that is a perfect brief.
Numbers and exact tuning are the agent's job — you react to the result.

## Running the live loop: `ludion dev`

The agent (or you) starts one command in the game's folder and leaves it running:

```
ludion dev
```

To watch in a native window at the same time:

```
ludion dev --window
```

You do not restart this between changes. It stays up and reacts every time the game is saved.

### The status lines, in plain terms

`ludion dev` prints a line for each thing it does. You will see:

| The line | What it means |
|---|---|
| `dev watching <folder>` | It's up and watching for changes. Nothing to do. |
| `listening on ws://…` | The address the window/viewer is attached to. Nothing to do. |
| `dev reload building` | It saw a change and is rebuilding. Wait a moment. |
| `dev reload restored frame=… in … ms` | The change is **live now**. Look at the game. |
| `dev reload build failed` | The new code didn't compile. **Your game keeps running on the last working version** — nothing broke. The reason is printed just above; hand it to the agent. |

The one number worth knowing: `restored frame=N in M ms` means the reload finished in `M`
milliseconds and the game is back exactly where it was (frame `N`). A few seconds is normal.

### The window (or viewer) during a reload

For the second or two while the game rebuilds and restarts, the window's title bar (or the viewer's
banner) shows that it is **reconnecting**, and it holds the last picture frozen. This is expected —
it is not a crash. When the reload finishes, the picture resumes on its own. If it says it gave up
after a long time, the rebuild probably failed; check the terminal or ask the agent.

**One thing to know:** the *running game* is your live state, and `ludion dev` preserves it across
code changes. Edits to the **level file** (the scene) are treated differently — the running world
isn't thrown away just because a level file was saved, so a scene edit prints a hint but doesn't
change what's on screen until it's deliberately loaded. If you changed a level and don't see it, tell
the agent "reload the level" — that's expected, not a bug.

## Tuning the look itself: shader edits

Most of what you ask for — speed, color, layout, behavior — the agent changes in the game's own
code, and `ludion dev` reloads it as described above. A smaller class of ask — "make the lighting
warmer across the board," "add a glow to emissive surfaces," "the metal highlights are too sharp"
— touches the engine's own shading, not the game. That still goes through `ludion dev`, but from
inside the engine's own project (the checkout the agent develops the engine in, not your game
folder), and it is even faster:

- The agent edits the shader file directly (a `.wgsl` file).
- `ludion dev`, running from inside the engine's checkout, notices the shader changed and pushes
  it live over the same connection — no rebuild, no restart. Pixels update in roughly **10
  milliseconds**, not the few seconds a code change takes.
- **A bad shader edit never breaks your session.** If the new shader doesn't compile or doesn't
  fit, the engine keeps showing you the last version that worked and prints a readable error —
  same "paste the red text to the agent" move as any other error.
- This fast path only exists when `ludion dev` is watching the engine's own shader folder, which
  is an engine-development setup, not something a game project has by default. A plain
  `ludion serve`/game run can still hot-reload shaders if it's explicitly started with a
  `--dev-shaders` flag — worth knowing if the agent mentions it, but not something you need to ask
  for; the agent picks the right mode for the job.

You never touch the shader file yourself — describe what the lighting/material should look like,
the same way you'd describe any other change, and the agent finds the right lever (an entity's
material, a light, or — on the rare occasion it's warranted — the shader itself).

## Lighting a scene with a sun

A "Sun" is one of the pieces the agent can drop into a level from the editor's entity palette. It
lights the whole scene from one direction, like daylight, and it casts one shadow — a block, a
character, a wall throws a shadow the way it would outdoors. This is different from the point
lights ("a lamp", "a torch") you may already know: a sun has no position of its own, only a
direction it shines from, and only one is ever active at a time.

Tuning it is an ordinary describe-and-watch ask, the same as any other change — it just happens to
be a property on the sun, not a shader edit, so it goes through the normal `ludion dev` reload, not
the faster shader path above:

- "Make the sun warmer / cooler." — a color change.
- "It's too bright / too dim." — an intensity change.
- "The shadows are too hard-edged" or "the shadow looks slightly detached from the box it belongs
  to" — both are the shadow's bias, a single small number the agent nudges; too little and edges
  look artificially sharp or speckled, too much and a shadow visibly lifts off its caster.

If your level is bigger than the area the shadow currently covers — the sun stops shadowing things
once you walk far enough — tell the agent "the shadow doesn't reach out here"; it will parent the
sun to whatever should keep the shadow centered (usually the player), so the shadow window follows
the action instead of staying fixed at one spot.

## When the game changes shape: the migration report

Sometimes the agent adds or removes a piece of the game (a new property on the player, a component it
no longer needs). When that happens, a reload prints a short **report** so nothing changes silently.
In plain terms, a line will say one of:

- **a value was filled in with a default** — the agent added something new; the game filled it with a
  sensible starting value. Usually fine.
- **a piece was dropped** — something the old game had is gone in the new version, so its data was
  discarded on purpose. Expected when the agent removed a feature; worth a glance if you didn't ask
  for that.

If a report line surprises you, just paste it to the agent and ask "was this meant to happen?"

## When something looks wrong

The engine is built so that **errors are readable**, and the agent is the one who reads them. You
almost never need to diagnose anything. The move that always works:

- **Copy the red error text from the terminal and paste it to the agent.** Every error says what is
  wrong, where, and usually the likely fix. The agent will take it from there.
- If the game *runs* but looks or feels wrong, **describe what you see** — "the paddle jumps back to
  the middle", "the light is way too bright", "the box passes through the wall". That description is
  exactly what the agent needs.

You never have to guess at code. Name the symptom; the agent finds the cause.

## The short version

- Describe **intent**, not code. Say what you see and what you want instead.
- Leave `ludion dev` running; it reloads your changes live and keeps your place.
- Watch the status lines: `restored` means it's live; `build failed` means the old version keeps
  running.
- A "reconnecting" title/banner during a reload is normal.
- Paste any red error, or describe any wrong-looking behavior, straight to the agent.
