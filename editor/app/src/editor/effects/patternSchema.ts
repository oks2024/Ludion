// GENERATED from `crates/ludion-engine/tests/snapshots/pattern_schema.json` by
// `editor/protocol/scripts/generate-pattern-schema.js`. Do not edit by hand -- re-bless the Rust
// snapshot, then run `npm run generate` in `editor/protocol`. The `schema-drift` gate rejects a
// stale copy.
//
// The bundled `kind: pattern` JSON Schema, `schema_for!(PatternDoc)`: the document
// `engine.describe` reports under `documents.pattern`. The pattern tab prefers the live schema, but
// the effects stage is plain `ludion serve` -- no game crate registers the kind, so `documents` is
// empty -- and the tab still needs a schema to list, open, edit and save a file. The form's
// fallback and `docFields.test.ts`'s coverage assertion read this same constant.

/** `schema_for!(PatternDoc)`, draft 2020-12, exactly as schemars v1 emits it. */
export const PATTERN_SCHEMA: unknown =
  {
    "$schema": "https://json-schema.org/draft/2020-12/schema",
    "title": "PatternDoc",
    "description": "A `kind: pattern` document: a named set of bullet kinds and a timeline that fires them\n(`Plan/projectile-patterns.md` decision 4).\n\nOne level deep, no expressions and no conditions — the vocabulary is a fixed enumerated set, and\nanything beyond it is Rust. A game opts in with [`EngineBuilder::patterns`](crate::EngineBuilder::patterns),\nwhich registers both the kind and [`validate_pattern_docs`].\n\n```yaml\nkind: pattern\nname: boss-phase-1\nbullets:\n  needle:\n    atlas: art/bullets.atlas.yaml\n    frame: needle\n    size:\n      - 0.3\n      - 0.3\n    blend: additive\n    hit_radius: 0.12\n    life: 240\n    motion:\n      - ramp:\n          frames: 30\n          from: 6.0\n          to: 2.0\n      - turn:\n          frames: 90\n          rate_deg: 45.0\ntimeline:\n  - at: 0\n    repeat: 8\n    every: 12\n    shape: ring\n    count: 24\n    bullet: needle\n    speed: 4.0\n    angle_step_deg: 7.5\n  - at: 120\n    play: patterns/sweep-left.yaml\nloop: 240\n```",
    "type": "object",
    "properties": {
      "name": {
        "description": "The pattern's own name — identity for a designer and for a report; the engine resolves a\npattern by its [`DocStore`] key (its content-root-relative path), never by this.",
        "type": "string",
        "default": ""
      },
      "bullets": {
        "description": "The bullet kinds this document's timeline may name, keyed by the short name a timeline entry\nwrites under `bullet:`. The vocabulary is local: a name from another document is exactly as\nunknown as a typo (`L1148`).",
        "type": "object",
        "additionalProperties": {
          "$ref": "#/$defs/BulletKind"
        },
        "default": {}
      },
      "timeline": {
        "description": "What fires when, in frames after the [`PatternPlayer`]'s `started`.",
        "type": "array",
        "items": {
          "$ref": "#/$defs/TimelineEntry"
        },
        "default": []
      },
      "loop": {
        "description": "How many frames the timeline spans before repeating (default `0` — play once). A boss phase\nis a looping pattern; a phase change is the game setting a different `pattern` on the player.",
        "type": "integer",
        "format": "uint32",
        "minimum": 0,
        "default": 0
      }
    },
    "$defs": {
      "BulletKind": {
        "description": "One kind of bullet: how it looks, how big its hit circle is, how long it lives, and the ≤ 4\nmotion segments its trajectory is made of.",
        "type": "object",
        "properties": {
          "atlas": {
            "description": "A `kind: atlas` document path (relative to the content root), or `None` for an untextured\nquad tinted by [`color`](BulletKind::color). Paired with [`frame`](BulletKind::frame) exactly\nas a [`Sprite`](crate::Sprite)'s is — both or neither (`L1151`); the pair is resolved when the\ndocument loads and reloads (`L1151`/`L1152`).",
            "type": [
              "string",
              "null"
            ],
            "default": null
          },
          "frame": {
            "description": "The region name inside [`atlas`](BulletKind::atlas).",
            "type": [
              "string",
              "null"
            ],
            "default": null
          },
          "size": {
            "description": "Quad size `[width, height]` in world units (default `[0.25, 0.25]`).",
            "type": "array",
            "items": {
              "type": "number",
              "format": "float"
            },
            "minItems": 2,
            "maxItems": 2,
            "default": [
              0.25,
              0.25
            ]
          },
          "color": {
            "description": "RGBA tint multiplied into the sampled texel (default opaque white).",
            "type": "array",
            "items": {
              "type": "number",
              "format": "float"
            },
            "minItems": 4,
            "maxItems": 4,
            "default": [
              1.0,
              1.0,
              1.0,
              1.0
            ]
          },
          "blend": {
            "description": "How the quad composites (default [`SpriteBlend::Alpha`]) — the sprite path's own flag, so a\nbullet swarm never splits a batch.",
            "$ref": "#/$defs/SpriteBlend",
            "default": "alpha"
          },
          "layer": {
            "description": "The sprite sort key (default `0`).",
            "type": "integer",
            "format": "int16",
            "minimum": -32768,
            "maximum": 32767,
            "default": 0
          },
          "billboard": {
            "description": "Whether the quad faces the camera (default `false` — it lies in the game's\n[`Plane`], which is what a top-down or side-on field wants).",
            "type": "boolean",
            "default": false
          },
          "hit_radius": {
            "description": "The bullet's collision radius in world units (default `0.1`), added to whatever radius a\n[`volley_hits`] caller asks about.",
            "type": "number",
            "format": "float",
            "default": 0.10000000149011612
          },
          "life": {
            "description": "How many frames the bullet lives after its volley fires (default `240` — four seconds).",
            "type": "integer",
            "format": "uint32",
            "minimum": 0,
            "default": 240
          },
          "motion": {
            "description": "The trajectory's phases, in order (default empty — straight travel at the fire speed\nforever). At most four (`L1149`); past the last one the bullet continues straight at its end\nspeed until `life`.",
            "type": "array",
            "items": {
              "$ref": "#/$defs/MotionSegment"
            },
            "default": []
          }
        }
      },
      "SpriteBlend": {
        "description": "How a [`Sprite`] composites over what it draws atop. A per-instance flag, not a second pipeline:\nboth modes run through the one premultiplied `One, OneMinusSrcAlpha` blend, so a flag bit alone\nseparates them and blend never splits a batch (`Plan/sprite-engine.md` decision 4).",
        "oneOf": [
          {
            "description": "Ordinary `over` compositing — the premultiplied texel blends by its own alpha.",
            "type": "string",
            "const": "alpha"
          },
          {
            "description": "Additive: the shader withholds the output alpha, which under `One, OneMinusSrcAlpha` turns the\nsame `over` into a plain add — glows, muzzle flashes, energy bolts.",
            "type": "string",
            "const": "additive"
          }
        ]
      },
      "MotionSegment": {
        "description": "One phase of a bullet's trajectory: exactly one of `ramp` and `turn` (both, or neither, is\n`L1149` — a ramping turn has no elementary closed form).",
        "type": "object",
        "properties": {
          "ramp": {
            "description": "Straight travel with the speed sweeping linearly.",
            "anyOf": [
              {
                "$ref": "#/$defs/RampSpec"
              },
              {
                "type": "null"
              }
            ],
            "default": null
          },
          "turn": {
            "description": "A circular arc at constant speed.",
            "anyOf": [
              {
                "$ref": "#/$defs/TurnSpec"
              },
              {
                "type": "null"
              }
            ],
            "default": null
          }
        }
      },
      "RampSpec": {
        "description": "`ramp { frames, from, to }` — straight along the current heading, speed sweeping from `from` to\n`to`.",
        "type": "object",
        "properties": {
          "frames": {
            "description": "How long the phase lasts, in frames.",
            "type": "integer",
            "format": "uint32",
            "minimum": 0,
            "default": 0
          },
          "from": {
            "description": "Speed at the start, in units per second, or `None` to carry the speed in force when the\nphase begins.",
            "type": [
              "number",
              "null"
            ],
            "format": "float",
            "default": null
          },
          "to": {
            "description": "Speed at the end, in units per second.",
            "type": "number",
            "format": "float",
            "default": 0.0
          }
        }
      },
      "TurnSpec": {
        "description": "`turn { frames, speed, rate_deg }` — a circular arc at constant speed, the heading rotating at\n`rate_deg` degrees per second. A positive rate always turns toward increasing angle, on both\nplanes.",
        "type": "object",
        "properties": {
          "frames": {
            "description": "How long the phase lasts, in frames.",
            "type": "integer",
            "format": "uint32",
            "minimum": 0,
            "default": 0
          },
          "speed": {
            "description": "Speed along the arc, in units per second, or `None` to carry the speed in force when the\nphase begins.",
            "type": [
              "number",
              "null"
            ],
            "format": "float",
            "default": null
          },
          "rate_deg": {
            "description": "Turn rate in **degrees** per second; negative turns the other way.",
            "type": "number",
            "format": "float",
            "default": 0.0
          }
        }
      },
      "TimelineEntry": {
        "description": "One timeline entry: either a volley (`bullet` and its shape parameters) or a `play` of another\npattern's timeline, offset by this entry's `at`.\n\nOne flat struct rather than a tagged enum, because the doc form stays one level deep and a\nschema-driven form must be able to render it. `play` alongside any volley field, and neither set,\nare both `L1149`.",
        "type": "object",
        "properties": {
          "at": {
            "description": "The frame this entry fires on, counted from the [`PatternPlayer`]'s `started` (and, inside a\nplayed pattern, offset by the playing entry's own `at`).",
            "type": "integer",
            "format": "uint32",
            "minimum": 0,
            "default": 0
          },
          "play": {
            "description": "Another pattern document's [`DocStore`] key, whose timeline plays offset by\n[`at`](TimelineEntry::at). Nesting reaches exactly one level (`L1150`).",
            "type": [
              "string",
              "null"
            ],
            "default": null
          },
          "bullet": {
            "description": "Which of the document's own `bullets:` this volley fires. Required for a volley (`L1148`\nwhen it names no kind).",
            "type": [
              "string",
              "null"
            ],
            "default": null
          },
          "repeat": {
            "description": "How many times the volley repeats (default `1`).",
            "type": "integer",
            "format": "uint32",
            "minimum": 0,
            "default": 1
          },
          "every": {
            "description": "Frames between repeats (default `0`, which is only legal with `repeat: 1`).",
            "type": "integer",
            "format": "uint32",
            "minimum": 0,
            "default": 0
          },
          "shape": {
            "description": "How the bullets are laid out (default [`VolleyShape::Ring`]).",
            "$ref": "#/$defs/VolleyShape",
            "default": "ring"
          },
          "center": {
            "description": "What an `arc`'s angle is measured from (default [`VolleyCenter::Absolute`]).",
            "$ref": "#/$defs/VolleyCenter",
            "default": "absolute"
          },
          "spread_deg": {
            "description": "The cone width in degrees for `arc` and `random` (default `0`).",
            "type": "number",
            "format": "float",
            "default": 0.0
          },
          "count": {
            "description": "How many bullets the volley holds (default `1`, at most [`VOLLEY_MAX_BULLETS`]).",
            "type": "integer",
            "format": "uint32",
            "minimum": 0,
            "default": 1
          },
          "speed": {
            "description": "The fire speed in units per second, used when [`speed_range`](TimelineEntry::speed_range) is\nabsent.",
            "type": "number",
            "format": "float",
            "default": 0.0
          },
          "speed_range": {
            "description": "A per-bullet-index speed range that **replaces** [`speed`](TimelineEntry::speed): bullet `i`\nof `count` fires at `lerp(range[0], range[1], i / (count − 1))`, and a single bullet at\n`range[0]`.",
            "type": [
              "array",
              "null"
            ],
            "items": {
              "type": "number",
              "format": "float"
            },
            "minItems": 2,
            "maxItems": 2,
            "default": null
          },
          "angle_deg": {
            "description": "The volley's angle in degrees (default `0`), measured per [`center`](TimelineEntry::center).",
            "type": "number",
            "format": "float",
            "default": 0.0
          },
          "angle_step_deg": {
            "description": "Added to the angle once per repeat (default `0`) — the spiral.",
            "type": "number",
            "format": "float",
            "default": 0.0
          },
          "speed_step": {
            "description": "Added to both ends of the speed range once per repeat (default `0`).",
            "type": "number",
            "format": "float",
            "default": 0.0
          }
        }
      },
      "VolleyShape": {
        "description": "How a volley lays its bullets out around its angle.\n\n**Every variant carries a doc comment** for [`crate::EmitShape`]'s reason: it is what makes\n`schemars` emit the `oneOf`-of-`const` shape a schema-driven form renders as a dropdown.",
        "oneOf": [
          {
            "description": "`count` bullets evenly over the full turn, the first at the volley's angle.",
            "type": "string",
            "const": "ring"
          },
          {
            "description": "`count` bullets evenly over `spread_deg`, centred on the volley's angle (a single bullet\nsits at the centre).",
            "type": "string",
            "const": "arc"
          },
          {
            "description": "Each bullet's angle drawn uniformly inside the cone and its speed uniformly from the\nvolley's range, from a stream named by the bullet's own index.",
            "type": "string",
            "const": "random"
          }
        ]
      },
      "VolleyCenter": {
        "description": "What an `arc` volley's angle is measured from.\n\nDoc comments on both variants, for [`VolleyShape`]'s reason.",
        "oneOf": [
          {
            "description": "The authored `angle_deg`, as a world angle.",
            "type": "string",
            "const": "absolute"
          },
          {
            "description": "The direction from the player to its [`PatternPlayer::target`], captured at fire time, plus\nthe authored `angle_deg`. A missing or dead target degrades to `absolute` with one `L2108`.",
            "type": "string",
            "const": "aim"
          }
        ]
      }
    }
  };
