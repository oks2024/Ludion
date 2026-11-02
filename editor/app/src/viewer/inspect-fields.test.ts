// Tests for the schema-driven field derivation. The schema fixtures below are the *exact* output of
// Rust `schemars` v1 for the engine's built-in components (crates/ludion-engine/src/components.rs) —
// captured from `schema_for!` so the derivation is verified against what `engine.describe` really
// sends, not an idealised guess (fixed arrays as `items` + `minItems`/`maxItems`, `Option<String>`
// as `["string","null"]`, unit/tagged enums behind `$ref` into `$defs`).

import { describe, expect, it } from 'vitest';
import { parseWire, stringifyWire } from '../protocol/wireJson';
import { applyFieldEdit, deriveFields, parseNumberDraft, valueAtPath, type FieldDescriptor } from './inspect-fields';

// --- Real schemars v1 fixtures ------------------------------------------------------------------

const NAME_SCHEMA = { $schema: 'https://json-schema.org/draft/2020-12/schema', title: 'Name', type: 'string' };

const TRANSFORM_SCHEMA = {
  $schema: 'https://json-schema.org/draft/2020-12/schema',
  title: 'Transform',
  type: 'object',
  properties: {
    translation: { type: 'array', items: { type: 'number', format: 'float' }, minItems: 3, maxItems: 3 },
    rotation: { type: 'array', items: { type: 'number', format: 'float' }, minItems: 3, maxItems: 3 },
    scale: { type: 'array', items: { type: 'number', format: 'float' }, minItems: 3, maxItems: 3 },
  },
};

const SPRITE_SCHEMA = {
  $schema: 'https://json-schema.org/draft/2020-12/schema',
  title: 'Sprite',
  type: 'object',
  properties: {
    texture: { type: ['string', 'null'] },
    size: { type: 'array', items: { type: 'number', format: 'float' }, minItems: 2, maxItems: 2 },
    color: { type: 'array', items: { type: 'number', format: 'float' }, minItems: 4, maxItems: 4 },
    billboard: { type: 'boolean' },
  },
};

const MESH_SCHEMA = {
  $schema: 'https://json-schema.org/draft/2020-12/schema',
  title: 'Mesh',
  type: 'object',
  properties: {
    primitive: { $ref: '#/$defs/Primitive' },
    color: { type: 'array', items: { type: 'number', format: 'float' }, minItems: 4, maxItems: 4 },
  },
  $defs: { Primitive: { type: 'string', enum: ['cube', 'plane', 'sphere'] } },
};

// Phase 8 Wave 2: `Mesh.shape` becomes a `$ref` to `MeshSource`, an externally-tagged enum whose unit
// variants (`cube`/`plane`/`sphere`) serialize as bare strings and whose one data-carrying variant
// serializes as `{ asset: "…" }`. This is the exact fragment from the task's engine.describe output.
const MESH_SOURCE_SCHEMA = {
  $schema: 'https://json-schema.org/draft/2020-12/schema',
  title: 'Mesh',
  type: 'object',
  properties: {
    shape: { $ref: '#/$defs/MeshSource' },
    color: { type: 'array', items: { type: 'number', format: 'float' }, minItems: 4, maxItems: 4 },
  },
  $defs: {
    MeshSource: {
      oneOf: [
        { type: 'string', const: 'cube' },
        { type: 'string', const: 'plane' },
        { type: 'string', const: 'sphere' },
        {
          type: 'object',
          properties: { asset: { type: 'string' } },
          required: ['asset'],
          additionalProperties: false,
        },
      ],
    },
  },
};

// Phase 10 Wave 5: `Mesh.shape` gains a second data-carrying variant, `Voxel(String)` (`{ voxel: … }`),
// beside `Asset(String)` — `deriveVariantOptions` generalizes from one object variant to N. This is the
// shape `engine.describe` will send once the Rust `MeshSource` enum grows the `Voxel` variant.
const MESH_SHAPE_WITH_VOXEL_SCHEMA = {
  $schema: 'https://json-schema.org/draft/2020-12/schema',
  title: 'Mesh',
  type: 'object',
  properties: {
    shape: { $ref: '#/$defs/MeshSource' },
    color: { type: 'array', items: { type: 'number', format: 'float' }, minItems: 4, maxItems: 4 },
  },
  $defs: {
    MeshSource: {
      oneOf: [
        { type: 'string', const: 'cube' },
        { type: 'string', const: 'plane' },
        { type: 'string', const: 'sphere' },
        {
          type: 'object',
          properties: { asset: { type: 'string' } },
          required: ['asset'],
          additionalProperties: false,
        },
        {
          type: 'object',
          properties: { voxel: { type: 'string' } },
          required: ['voxel'],
          additionalProperties: false,
        },
      ],
    },
  },
};

// Phase 11 Wave 1: `Mesh` gains three plain `f32` PBR fields (`roughness`, `metallic`, `emissive`) —
// no new widget: schemars emits each as a `{ "type": "number", "format": "float" }` property, the
// exact number shape `near`/`far` already exercise on CAMERA_SCHEMA below, so they commit through the
// existing number-field path, same as any other f32. Their `minimum`/`maximum` keywords are read as
// the field's range: `roughness`/`metallic` are bounded on both sides and get a track, `emissive`
// (bounded below only) does not.
const MESH_SCHEMA_WITH_PBR = {
  $schema: 'https://json-schema.org/draft/2020-12/schema',
  title: 'Mesh',
  type: 'object',
  properties: {
    shape: { $ref: '#/$defs/MeshSource' },
    color: { type: 'array', items: { type: 'number', format: 'float' }, minItems: 4, maxItems: 4 },
    roughness: { type: 'number', format: 'float', minimum: 0, maximum: 1 },
    metallic: { type: 'number', format: 'float', minimum: 0, maximum: 1 },
    emissive: { type: 'number', format: 'float', minimum: 0 },
  },
  $defs: {
    MeshSource: {
      oneOf: [
        { type: 'string', const: 'cube' },
        { type: 'string', const: 'plane' },
        { type: 'string', const: 'sphere' },
        {
          type: 'object',
          properties: { asset: { type: 'string' } },
          required: ['asset'],
          additionalProperties: false,
        },
      ],
    },
  },
};

// Phase 12 Wave 5: `Collider` (from ludion-spatial, registered last in the builtin registry). The
// exact schemars v1 structure (descriptions/defaults trimmed like the other fixtures): `body` is a
// unit-only enum emitted as a oneOf of string consts (per-variant doc comments force the oneOf
// form) — a plain dropdown; `shape`'s variants carry an *object* payload (`circle`/`aabb`), which
// the variant widget deliberately does not model, so it stays on the raw-JSON fallback (the same
// honest fallback `Projection` uses — plan decision 6).
const COLLIDER_SCHEMA = {
  $schema: 'https://json-schema.org/draft/2020-12/schema',
  title: 'Collider',
  type: 'object',
  properties: {
    shape: { $ref: '#/$defs/Shape', default: { circle: { radius: 0.5 } } },
    body: { $ref: '#/$defs/Body', default: 'static' },
  },
  $defs: {
    Shape: {
      oneOf: [
        {
          type: 'object',
          properties: {
            circle: {
              type: 'object',
              properties: { radius: { type: 'number', format: 'float', minimum: 0, default: 0.5 } },
            },
          },
          required: ['circle'],
          additionalProperties: false,
        },
        {
          type: 'object',
          properties: {
            aabb: {
              type: 'object',
              properties: {
                half_extents: {
                  type: 'array',
                  items: { type: 'number', format: 'float' },
                  minItems: 2,
                  maxItems: 2,
                  default: [0.5, 0.5],
                },
              },
            },
          },
          required: ['aabb'],
          additionalProperties: false,
        },
      ],
    },
    Body: {
      oneOf: [
        { type: 'string', const: 'static' },
        { type: 'string', const: 'dynamic' },
      ],
    },
  },
};

// Phase 14 Wave 4: `UiRect` (`crates/ludion-engine/src/components.rs`) — captured from
// `schemars::schema_for!(UiRect)` (descriptions/defaults trimmed like the other fixtures above).
// `anchor` is a `$ref` to `Anchor`, a `oneOf` of nine bare-string `const` branches (every variant
// doc-commented, decision 1) — no object branch at all, which still satisfies `deriveVariantOptions`
// (at least one unit branch) so it renders as a plain 9-option dropdown, the same shape as
// `Collider.body` above, just wider. `offset`/`size` are fixed 2-wide float arrays (vectors, not
// colors — the name doesn't read as one); `texture` is `Option<String>` (nullable string, the exact
// shape `Sprite.texture` already exercises); `color` is the 4-wide color vector; `order` is `i32`
// (`schemars` emits `{"type":"integer","format":"int32"}` for it, an integer number field).
const UI_RECT_SCHEMA = {
  $schema: 'https://json-schema.org/draft/2020-12/schema',
  title: 'UiRect',
  type: 'object',
  properties: {
    anchor: { $ref: '#/$defs/Anchor' },
    offset: { type: 'array', items: { type: 'number', format: 'float' }, minItems: 2, maxItems: 2 },
    size: { type: 'array', items: { type: 'number', format: 'float' }, minItems: 2, maxItems: 2 },
    texture: { type: ['string', 'null'] },
    color: { type: 'array', items: { type: 'number', format: 'float' }, minItems: 4, maxItems: 4 },
    order: { type: 'integer', format: 'int32' },
  },
  $defs: {
    Anchor: {
      oneOf: [
        { type: 'string', const: 'top_left' },
        { type: 'string', const: 'top_center' },
        { type: 'string', const: 'top_right' },
        { type: 'string', const: 'center_left' },
        { type: 'string', const: 'center' },
        { type: 'string', const: 'center_right' },
        { type: 'string', const: 'bottom_left' },
        { type: 'string', const: 'bottom_center' },
        { type: 'string', const: 'bottom_right' },
      ],
    },
  },
};

// Phase 14 Wave 4: `UiText` — same `Anchor` shape as `UiRect` above, plus a plain (non-nullable)
// `text: String` field and a scalar `size: f32` (unlike `UiRect.size`, this is one number, the
// glyph cell height — not a 2-wide array).
const UI_TEXT_SCHEMA = {
  $schema: 'https://json-schema.org/draft/2020-12/schema',
  title: 'UiText',
  type: 'object',
  properties: {
    anchor: { $ref: '#/$defs/Anchor' },
    offset: { type: 'array', items: { type: 'number', format: 'float' }, minItems: 2, maxItems: 2 },
    text: { type: 'string' },
    size: { type: 'number', format: 'float' },
    color: { type: 'array', items: { type: 'number', format: 'float' }, minItems: 4, maxItems: 4 },
    order: { type: 'integer', format: 'int32' },
  },
  $defs: {
    Anchor: {
      oneOf: [
        { type: 'string', const: 'top_left' },
        { type: 'string', const: 'top_center' },
        { type: 'string', const: 'top_right' },
        { type: 'string', const: 'center_left' },
        { type: 'string', const: 'center' },
        { type: 'string', const: 'center_right' },
        { type: 'string', const: 'bottom_left' },
        { type: 'string', const: 'bottom_center' },
        { type: 'string', const: 'bottom_right' },
      ],
    },
  },
};

// Environment/IBL Wave 1: `Environment` (the opt-in sky/lighting singleton) — captured from
// `schemars::schema_for!(Environment)` (descriptions/defaults trimmed like the other fixtures). Every
// field is either a fixed 3-wide float array (the sky/ambient colors — vectors, not swatch-flagged:
// the labels don't read as "color") or a plain `f32` scalar (the disc/IBL knobs, the exact shape
// `near`/`far` already exercise on CAMERA_SCHEMA). Their lone `minimum` bounds them below but states
// no ceiling, so no field here has a range: half a bound is not a track.
const ENVIRONMENT_SCHEMA = {
  $schema: 'https://json-schema.org/draft/2020-12/schema',
  title: 'Environment',
  type: 'object',
  properties: {
    sky_zenith: { type: 'array', items: { type: 'number', format: 'float' }, minItems: 3, maxItems: 3 },
    sky_horizon: { type: 'array', items: { type: 'number', format: 'float' }, minItems: 3, maxItems: 3 },
    sky_ground: { type: 'array', items: { type: 'number', format: 'float' }, minItems: 3, maxItems: 3 },
    sun_disc_size: { type: 'number', format: 'float', minimum: 0.0 },
    sun_disc_intensity: { type: 'number', format: 'float', minimum: 0.0 },
    ambient: { type: 'array', items: { type: 'number', format: 'float' }, minItems: 3, maxItems: 3 },
    ibl_intensity: { type: 'number', format: 'float', minimum: 0.0 },
  },
};

// Environment/IBL Wave 1: `Fade` — two plain `f32` fields, both plain number fields. `fade` carries
// a `[0, 1]` range (`minimum`/`maximum`, so it is both its legal bound and its slider travel);
// `height_min` is a signed world Y and carries none.
const FADE_SCHEMA = {
  $schema: 'https://json-schema.org/draft/2020-12/schema',
  title: 'Fade',
  type: 'object',
  properties: {
    fade: { type: 'number', format: 'float', minimum: 0.0, maximum: 1.0 },
    height_min: { type: 'number', format: 'float' },
  },
};

// `PostProcess`, the component that carries most of the engine's range annotations. `x-ui-range` is
// an inert extension keyword (`#[schemars(extend(…))]`) emitted on the field's own subschema beside
// the `minimum`/`maximum` its `range(…)` attribute emits, so a field can state a useful travel that
// is narrower than its legal bounds (`grain_size` ships at 0.002 inside a legal `[0, 1]`), or state
// travel where only half a legal bound exists (`bloom_strength`). `lift` is the fixed-array case: the
// keyword sits on the array node and is understood per element.
const POST_PROCESS_SCHEMA = {
  $schema: 'https://json-schema.org/draft/2020-12/schema',
  title: 'PostProcess',
  type: 'object',
  properties: {
    bloom_strength: { type: 'number', format: 'float', minimum: 0.0, 'x-ui-range': [0.0, 0.5] },
    grain_size: { type: 'number', format: 'float', minimum: 0.0, maximum: 1.0, 'x-ui-range': [0.0, 0.01] },
    vignette: { type: 'number', format: 'float', minimum: 0.0, maximum: 1.0 },
    fog_height: { type: 'number', format: 'float' },
    lift: {
      type: 'array',
      items: { type: 'number', format: 'float' },
      minItems: 3,
      maxItems: 3,
      'x-ui-range': [0.0, 0.2],
    },
  },
};

const CAMERA_SCHEMA = {
  $schema: 'https://json-schema.org/draft/2020-12/schema',
  title: 'Camera',
  type: 'object',
  properties: {
    projection: { $ref: '#/$defs/Projection' },
    near: { type: 'number', format: 'float' },
    far: { type: 'number', format: 'float' },
  },
  $defs: {
    Projection: {
      oneOf: [
        { type: 'object', properties: { type: { type: 'string', const: 'perspective' }, fov_y_deg: { type: 'number', format: 'float' } }, required: ['type', 'fov_y_deg'] },
        { type: 'object', properties: { type: { type: 'string', const: 'orthographic' }, height: { type: 'number', format: 'float' } }, required: ['type', 'height'] },
      ],
    },
  },
};

// ui-widgets: `UiList` — captured from `schemars::schema_for!(UiList)` (descriptions/defaults trimmed
// like the other fixtures above). `anchor` is the same `Anchor` `$ref` `UiRect`/`UiText` already
// exercise; `title` is a plain string; `items` is a variable-length array of `UiListItem` objects
// (no `minItems`/`maxItems` — a `Vec`, not a fixed array), which this widget set does not model, same
// as `Tilemap.rows`; `selected`/`max_visible` are `u32` (bounded below only, so no track, same shape
// as `emissive` above); `size`/`line_spacing`/`padding` are scalar numbers (`line_spacing`/`padding`
// bounded below only); `color`/`selected_color`/`disabled_color` are 4-wide color-swatched vectors,
// but `background` doesn't match the `colou?r` label heuristic and derives as a plain 4-wide vector;
// `order` is `i32`.
const UI_LIST_SCHEMA = {
  $schema: 'https://json-schema.org/draft/2020-12/schema',
  title: 'UiList',
  type: 'object',
  properties: {
    anchor: { $ref: '#/$defs/Anchor' },
    offset: { type: 'array', items: { type: 'number', format: 'float' }, minItems: 2, maxItems: 2 },
    title: { type: 'string' },
    items: { type: 'array', items: { $ref: '#/$defs/UiListItem' } },
    selected: { type: 'integer', format: 'uint32', minimum: 0 },
    size: { type: 'number', format: 'float' },
    line_spacing: { type: 'number', format: 'float', minimum: 0.0 },
    padding: { type: 'number', format: 'float', minimum: 0.0 },
    color: { type: 'array', items: { type: 'number', format: 'float' }, minItems: 4, maxItems: 4 },
    selected_color: { type: 'array', items: { type: 'number', format: 'float' }, minItems: 4, maxItems: 4 },
    disabled_color: { type: 'array', items: { type: 'number', format: 'float' }, minItems: 4, maxItems: 4 },
    background: { type: 'array', items: { type: 'number', format: 'float' }, minItems: 4, maxItems: 4 },
    order: { type: 'integer', format: 'int32' },
    max_visible: { type: 'integer', format: 'uint32', minimum: 0 },
  },
  $defs: {
    Anchor: {
      oneOf: [
        { type: 'string', const: 'top_left' },
        { type: 'string', const: 'top_center' },
        { type: 'string', const: 'top_right' },
        { type: 'string', const: 'center_left' },
        { type: 'string', const: 'center' },
        { type: 'string', const: 'center_right' },
        { type: 'string', const: 'bottom_left' },
        { type: 'string', const: 'bottom_center' },
        { type: 'string', const: 'bottom_right' },
      ],
    },
    UiListItem: {
      type: 'object',
      properties: {
        text: { type: 'string' },
        enabled: { type: 'boolean' },
      },
    },
  },
};

// particles: `ParticleEmitter` (`crates/ludion-engine/src/components.rs`) — captured from
// `schemars::schema_for!(ParticleEmitter)` (descriptions/defaults trimmed like the other fixtures
// above). `shape` and `blend` are `$ref`s into unit-only `oneOf`s — `EmitShape`'s four variants,
// `ParticleBlend`'s two, every variant doc-commented for the same reason `UiRect.anchor`/
// `Collider.body` are — so both render as plain dropdowns. `started`/`seed` are `u64`: schemars gives
// an unsigned integer type its `minimum: 0` for free, with no `#[schemars(…)]` attribute at all on the
// field, and neither carries a `#[schemars(extend(…))]` travel, so both stay plain number boxes (the
// `Mesh.emissive`/`UiList.selected` shape). Every other field — the thirteen "register" fields —
// carries `#[schemars(extend("x-ui-range" = …))]` beside whatever `minimum`/`maximum` it already has;
// on the three fixed arrays (`extent`, `life`, `speed`) the keyword sits on the array node itself, read
// per element, the `PostProcess.lift` shape — never on `items`. `burst` (`u32`) keeps its automatic
// `minimum: 0` alongside its own declared travel. `start_color`/`end_color` are 4-wide color-swatched
// vectors with no travel at all, exactly like `Sprite.color`.
const PARTICLE_EMITTER_SCHEMA = {
  $schema: 'https://json-schema.org/draft/2020-12/schema',
  title: 'ParticleEmitter',
  type: 'object',
  properties: {
    shape: { $ref: '#/$defs/EmitShape' },
    extent: {
      type: 'array',
      items: { type: 'number', format: 'float' },
      minItems: 3,
      maxItems: 3,
      'x-ui-range': [0.0, 4.0],
    },
    rate: { type: 'number', format: 'float', minimum: 0.0, 'x-ui-range': [0.0, 200.0] },
    burst: { type: 'integer', format: 'uint32', minimum: 0, 'x-ui-range': [0, 512] },
    started: { type: 'integer', format: 'uint64', minimum: 0 },
    duration: { type: 'number', format: 'float', minimum: 0.0, 'x-ui-range': [0.0, 10.0] },
    life: {
      type: 'array',
      items: { type: 'number', format: 'float' },
      minItems: 2,
      maxItems: 2,
      'x-ui-range': [0.0, 5.0],
    },
    speed: {
      type: 'array',
      items: { type: 'number', format: 'float' },
      minItems: 2,
      maxItems: 2,
      'x-ui-range': [0.0, 20.0],
    },
    spread_deg: { type: 'number', format: 'float', minimum: 0.0, maximum: 180.0, 'x-ui-range': [0.0, 180.0] },
    gravity: { type: 'number', format: 'float', 'x-ui-range': [-20.0, 20.0] },
    drag: { type: 'number', format: 'float', minimum: 0.0, 'x-ui-range': [0.0, 10.0] },
    floor: { type: 'number', format: 'float', 'x-ui-range': [-4.0, 4.0] },
    start_size: { type: 'number', format: 'float', minimum: 0.0, 'x-ui-range': [0.0, 4.0] },
    end_size: { type: 'number', format: 'float', minimum: 0.0, 'x-ui-range': [0.0, 4.0] },
    start_color: { type: 'array', items: { type: 'number', format: 'float' }, minItems: 4, maxItems: 4 },
    end_color: { type: 'array', items: { type: 'number', format: 'float' }, minItems: 4, maxItems: 4 },
    blend: { $ref: '#/$defs/ParticleBlend' },
    softness: { type: 'number', format: 'float', minimum: 0.0, maximum: 1.0, 'x-ui-range': [0.0, 1.0] },
    seed: { type: 'integer', format: 'uint64', minimum: 0 },
  },
  $defs: {
    EmitShape: {
      oneOf: [
        { type: 'string', const: 'point' },
        { type: 'string', const: 'sphere' },
        { type: 'string', const: 'box' },
        { type: 'string', const: 'disc' },
      ],
    },
    ParticleBlend: {
      oneOf: [
        { type: 'string', const: 'alpha' },
        { type: 'string', const: 'additive' },
      ],
    },
  },
};

// `ParticleEmitter::default()`, spelled out field-by-field so every ParticleEmitter test below drives
// `deriveFields` from one shared, valid value.
const PARTICLE_EMITTER_DEFAULT = {
  shape: 'point',
  extent: [0.5, 0.5, 0.5],
  rate: 40.0,
  burst: 0,
  started: 0,
  duration: 0.0,
  life: [0.8, 1.4],
  speed: [1.0, 2.5],
  spread_deg: 30.0,
  gravity: 0.0,
  drag: 0.0,
  floor: -3.4e38,
  start_size: 0.25,
  end_size: 0.0,
  start_color: [1, 1, 1, 1],
  end_color: [1, 1, 1, 0],
  blend: 'alpha',
  softness: 0.5,
  seed: 0,
};

/** Finds the descriptor at a dotted-ish label; asserts exactly one match. */
function field(fields: readonly FieldDescriptor[], label: string): FieldDescriptor {
  const found = fields.filter((f) => f.label === label);
  expect(found, `field '${label}'`).toHaveLength(1);
  return found[0];
}

// --- deriveFields: real components ---------------------------------------------------------------

describe('deriveFields', () => {
  it('treats a newtype scalar component (Name) as a single string field at the empty path', () => {
    const fields = deriveFields(NAME_SCHEMA, 'paddle');
    expect(fields).toHaveLength(1);
    expect(fields[0]).toEqual({ kind: 'string', path: [], label: '', nullable: false });
  });

  it('derives Transform as three 3-wide numeric vectors, in schema order', () => {
    const fields = deriveFields(TRANSFORM_SCHEMA, {
      translation: [1, 2, 3],
      rotation: [0, 0, 0],
      scale: [1, 1, 1],
    });
    expect(fields.map((f) => f.label)).toEqual(['translation', 'rotation', 'scale']);
    for (const f of fields) {
      expect(f).toMatchObject({ kind: 'vector', length: 3, integer: false, wide: false, color: false });
    }
    expect(fields[0].path).toEqual(['translation']);
  });

  it('derives Sprite: nullable string, vec2, vec4 color, boolean', () => {
    const fields = deriveFields(SPRITE_SCHEMA, {
      texture: null,
      size: [1, 1],
      color: [1, 1, 1, 1],
      billboard: false,
    });
    expect(field(fields, 'texture')).toEqual({ kind: 'string', path: ['texture'], label: 'texture', nullable: true });
    expect(field(fields, 'size')).toMatchObject({ kind: 'vector', length: 2, color: false });
    expect(field(fields, 'color')).toMatchObject({ kind: 'vector', length: 4, color: true });
    expect(field(fields, 'billboard')).toEqual({ kind: 'boolean', path: ['billboard'], label: 'billboard' });
  });

  it('sends a unit enum ($ref → string enum) to the JSON fallback', () => {
    const fields = deriveFields(MESH_SCHEMA, { primitive: 'cube', color: [1, 1, 1, 1] });
    expect(field(fields, 'primitive')).toEqual({ kind: 'json', path: ['primitive'], label: 'primitive' });
    expect(field(fields, 'color')).toMatchObject({ kind: 'vector', length: 4, color: true });
  });

  it('sends a tagged enum ($ref → oneOf) to the JSON fallback; plain numbers stay numbers', () => {
    const fields = deriveFields(CAMERA_SCHEMA, {
      projection: { type: 'perspective', fov_y_deg: 60 },
      near: 0.1,
      far: 1000,
    });
    expect(field(fields, 'projection')).toEqual({ kind: 'json', path: ['projection'], label: 'projection' });
    expect(field(fields, 'near')).toEqual({ kind: 'number', path: ['near'], label: 'near', integer: false, wide: false, range: null });
    expect(field(fields, 'far')).toMatchObject({ kind: 'number', integer: false });
  });

  // Phase 8 Wave 2: MeshSource ($ref → oneOf of 3 unit-string consts + 1 single-string-property object)
  // gets a dedicated `variant` widget instead of falling to raw JSON, unlike Projection above.
  it('derives Mesh.shape (MeshSource) as a variant field with all four branches, regardless of which value form is current', () => {
    const bareValue = deriveFields(MESH_SOURCE_SCHEMA, { shape: 'cube', color: [1, 1, 1, 1] });
    const objectValue = deriveFields(MESH_SOURCE_SCHEMA, {
      shape: { asset: 'meshes/rock.mesh.yaml' },
      color: [1, 1, 1, 1],
    });
    const expected = {
      kind: 'variant',
      path: ['shape'],
      label: 'shape',
      options: [
        { kind: 'unit', value: 'cube' },
        { kind: 'unit', value: 'plane' },
        { kind: 'unit', value: 'sphere' },
        { kind: 'asset', property: 'asset' },
      ],
    };
    // The descriptor is derived from the schema alone — it must not depend on which value form (bare
    // string vs. `{ asset }`) the component currently holds.
    expect(field(bareValue, 'shape')).toEqual(expected);
    expect(field(objectValue, 'shape')).toEqual(expected);
    expect(field(bareValue, 'color')).toMatchObject({ kind: 'vector', length: 4, color: true });
  });

  // Phase 10 Wave 5: a second object variant (`voxel`, beside `asset`) no longer collapses the field to
  // raw JSON (the old `sawAsset` singleton guard) — it becomes a fifth dropdown branch.
  it('derives Mesh.shape with two object variants (asset, voxel) as a variant field with all five branches, regardless of which value form is current', () => {
    const bareValue = deriveFields(MESH_SHAPE_WITH_VOXEL_SCHEMA, { shape: 'cube', color: [1, 1, 1, 1] });
    const assetValue = deriveFields(MESH_SHAPE_WITH_VOXEL_SCHEMA, {
      shape: { asset: 'meshes/rock.mesh.yaml' },
      color: [1, 1, 1, 1],
    });
    const voxelValue = deriveFields(MESH_SHAPE_WITH_VOXEL_SCHEMA, {
      shape: { voxel: 'voxels/lantern.voxel.yaml' },
      color: [1, 1, 1, 1],
    });
    const expected = {
      kind: 'variant',
      path: ['shape'],
      label: 'shape',
      options: [
        { kind: 'unit', value: 'cube' },
        { kind: 'unit', value: 'plane' },
        { kind: 'unit', value: 'sphere' },
        { kind: 'asset', property: 'asset' },
        { kind: 'asset', property: 'voxel' },
      ],
    };
    // The descriptor is derived from the schema alone — it must not depend on which value form (bare
    // string, `{ asset }`, or `{ voxel }`) the component currently holds.
    expect(field(bareValue, 'shape')).toEqual(expected);
    expect(field(assetValue, 'shape')).toEqual(expected);
    expect(field(voxelValue, 'shape')).toEqual(expected);
    expect(field(bareValue, 'color')).toMatchObject({ kind: 'vector', length: 4, color: true });
  });

  it('also recognises a unit-variant branch expressed as a single-entry enum (not just const)', () => {
    const schema = {
      type: 'object',
      properties: {
        shape: {
          oneOf: [
            { type: 'string', enum: ['cube'] },
            { type: 'string', enum: ['plane'] },
            { type: 'object', properties: { asset: { type: 'string' } }, required: ['asset'] },
          ],
        },
      },
    };
    expect(field(deriveFields(schema, { shape: 'cube' }), 'shape')).toEqual({
      kind: 'variant',
      path: ['shape'],
      label: 'shape',
      options: [
        { kind: 'unit', value: 'cube' },
        { kind: 'unit', value: 'plane' },
        { kind: 'asset', property: 'asset' },
      ],
    });
  });

  // Phase 11 Wave 1: verifies the plan's claim (decision 6) that `roughness`/`metallic`/`emissive`
  // need no new widget — they derive as three plain numeric fields, exactly like any other bare `f32`.
  // Their declared bounds ride along as the field's range: `[0, 1]` for the two the format bounds on
  // both sides, nothing for `emissive` (`minimum` alone leaves no ceiling to travel to).
  it('derives Mesh.roughness/metallic/emissive as plain non-integer number fields, alongside shape', () => {
    const fields = deriveFields(MESH_SCHEMA_WITH_PBR, {
      shape: 'cube',
      color: [1, 1, 1, 1],
      roughness: 1,
      metallic: 0,
      emissive: 0,
    });
    const unit = { min: 0, max: 1, hardMin: 0, hardMax: 1 };
    expect(field(fields, 'roughness')).toEqual({ kind: 'number', path: ['roughness'], label: 'roughness', integer: false, wide: false, range: unit });
    expect(field(fields, 'metallic')).toEqual({ kind: 'number', path: ['metallic'], label: 'metallic', integer: false, wide: false, range: unit });
    expect(field(fields, 'emissive')).toEqual({ kind: 'number', path: ['emissive'], label: 'emissive', integer: false, wide: false, range: null });
    expect(field(fields, 'shape')).toMatchObject({ kind: 'variant' });
  });

  // Phase 12 Wave 5 (plan decision 6): `Collider.body` — a unit-only enum (oneOf of string consts) —
  // is a plain dropdown with no text input; `Collider.shape` — object variants with number/array
  // payloads — is deliberately unsupported by the variant widget and lands on the raw-JSON textarea,
  // exactly like `Projection`. Both fields render; neither is dropped.
  it('derives Collider: body as a unit-only variant dropdown, shape as the raw-JSON fallback', () => {
    const circleValue = deriveFields(COLLIDER_SCHEMA, {
      shape: { circle: { radius: 0.5 } },
      body: 'static',
    });
    const aabbValue = deriveFields(COLLIDER_SCHEMA, {
      shape: { aabb: { half_extents: [0.5, 2.0] } },
      body: 'dynamic',
    });
    const expectedBody = {
      kind: 'variant',
      path: ['body'],
      label: 'body',
      options: [
        { kind: 'unit', value: 'static' },
        { kind: 'unit', value: 'dynamic' },
      ],
    };
    // Derived from the schema alone — independent of which value form the component holds.
    expect(field(circleValue, 'body')).toEqual(expectedBody);
    expect(field(aabbValue, 'body')).toEqual(expectedBody);
    expect(field(circleValue, 'shape')).toEqual({ kind: 'json', path: ['shape'], label: 'shape' });
    expect(field(aabbValue, 'shape')).toEqual({ kind: 'json', path: ['shape'], label: 'shape' });
  });

  // Phase 14 Wave 4 (plan decision 8, the editor "degrade-nowhere" check): `UiRect`'s `anchor` is a
  // unit-only `Anchor` oneOf — like `Collider.body` but with all nine variants, not two — so it
  // renders as a 9-option dropdown; `offset`/`size` are plain (non-color) vectors, `color` is a
  // color-swatched vector, `order` an integer number field, and `texture` (`Option<String>`) derives
  // the same nullable-string shape `Sprite.texture` already pins above. Pinning this now means a
  // future schemars/derive change that silently drops the dropdown or the swatch fails a test, not a
  // designer's inspector.
  it('derives UiRect: anchor as a nine-option unit variant dropdown, vectors, color swatch, nullable texture, integer order', () => {
    const fields = deriveFields(UI_RECT_SCHEMA, {
      anchor: 'top_left',
      offset: [0, 0],
      size: [100, 100],
      texture: null,
      color: [1, 1, 1, 1],
      order: 0,
    });
    expect(field(fields, 'anchor')).toEqual({
      kind: 'variant',
      path: ['anchor'],
      label: 'anchor',
      options: [
        { kind: 'unit', value: 'top_left' },
        { kind: 'unit', value: 'top_center' },
        { kind: 'unit', value: 'top_right' },
        { kind: 'unit', value: 'center_left' },
        { kind: 'unit', value: 'center' },
        { kind: 'unit', value: 'center_right' },
        { kind: 'unit', value: 'bottom_left' },
        { kind: 'unit', value: 'bottom_center' },
        { kind: 'unit', value: 'bottom_right' },
      ],
    });
    expect(field(fields, 'offset')).toEqual({ kind: 'vector', path: ['offset'], label: 'offset', length: 2, integer: false, wide: false, color: false, range: null });
    expect(field(fields, 'size')).toEqual({ kind: 'vector', path: ['size'], label: 'size', length: 2, integer: false, wide: false, color: false, range: null });
    expect(field(fields, 'texture')).toEqual({ kind: 'string', path: ['texture'], label: 'texture', nullable: true });
    expect(field(fields, 'color')).toEqual({ kind: 'vector', path: ['color'], label: 'color', length: 4, integer: false, wide: false, color: true, range: null });
    expect(field(fields, 'order')).toEqual({ kind: 'number', path: ['order'], label: 'order', integer: true, wide: false, range: null });
  });

  // `UiText` shares `UiRect`'s `anchor`/`offset`/`color`/`order` shapes; `text` is a plain
  // (non-nullable) string and `size` is a scalar number (the glyph cell height), not a vector like
  // `UiRect.size`.
  it('derives UiText: anchor dropdown, plain string text, scalar size, color swatch, integer order', () => {
    const fields = deriveFields(UI_TEXT_SCHEMA, {
      anchor: 'center',
      offset: [0, 0],
      text: 'HI',
      size: 24,
      color: [1, 1, 1, 1],
      order: 0,
    });
    expect(field(fields, 'anchor')).toEqual({
      kind: 'variant',
      path: ['anchor'],
      label: 'anchor',
      options: [
        { kind: 'unit', value: 'top_left' },
        { kind: 'unit', value: 'top_center' },
        { kind: 'unit', value: 'top_right' },
        { kind: 'unit', value: 'center_left' },
        { kind: 'unit', value: 'center' },
        { kind: 'unit', value: 'center_right' },
        { kind: 'unit', value: 'bottom_left' },
        { kind: 'unit', value: 'bottom_center' },
        { kind: 'unit', value: 'bottom_right' },
      ],
    });
    expect(field(fields, 'text')).toEqual({ kind: 'string', path: ['text'], label: 'text', nullable: false });
    expect(field(fields, 'size')).toEqual({ kind: 'number', path: ['size'], label: 'size', integer: false, wide: false, range: null });
    expect(field(fields, 'color')).toMatchObject({ kind: 'vector', length: 4, color: true });
    expect(field(fields, 'order')).toEqual({ kind: 'number', path: ['order'], label: 'order', integer: true, wide: false, range: null });
  });

  // Environment/IBL Wave 1: `Environment` renders as four 3-wide numeric vectors (the sky/ambient
  // colors) plus three plain number fields (the disc/IBL scalars), in schema order — every field
  // widgets cleanly (no raw-JSON fallback), so the inspector shows and commits the whole component.
  it('derives Environment: four color vectors and three scalar knobs, all widgetable', () => {
    const fields = deriveFields(ENVIRONMENT_SCHEMA, {
      sky_zenith: [0.02, 0.03, 0.06],
      sky_horizon: [0.05, 0.06, 0.1],
      sky_ground: [0.01, 0.01, 0.02],
      sun_disc_size: 0,
      sun_disc_intensity: 1,
      ambient: [0.05, 0.05, 0.06],
      ibl_intensity: 1,
    });
    expect(fields.map((f) => f.label)).toEqual([
      'sky_zenith',
      'sky_horizon',
      'sky_ground',
      'sun_disc_size',
      'sun_disc_intensity',
      'ambient',
      'ibl_intensity',
    ]);
    for (const label of ['sky_zenith', 'sky_horizon', 'sky_ground', 'ambient']) {
      expect(field(fields, label)).toMatchObject({ kind: 'vector', length: 3 });
    }
    for (const label of ['sun_disc_size', 'sun_disc_intensity', 'ibl_intensity']) {
      expect(field(fields, label)).toEqual({ kind: 'number', path: [label], label, integer: false, wide: false, range: null });
    }
    // None fell to the raw-JSON bucket — the whole component is editable.
    expect(fields.some((f) => f.kind === 'json')).toBe(false);
  });

  // Environment/IBL Wave 1: `Fade` — two plain number fields. `fade`'s `[0, 1]` is both its legal
  // bound and, in the absence of a declared travel, the track a slider gets; `height_min` is a signed
  // world Y with no bound at all and stays the number box it has always been.
  it('derives Fade: fade with a [0, 1] range, height_min with none', () => {
    const fields = deriveFields(FADE_SCHEMA, { fade: 1, height_min: -3.4e38 });
    expect(field(fields, 'fade')).toEqual({
      kind: 'number',
      path: ['fade'],
      label: 'fade',
      integer: false, wide: false,
      range: { min: 0, max: 1, hardMin: 0, hardMax: 1 },
    });
    expect(field(fields, 'height_min')).toEqual({
      kind: 'number',
      path: ['height_min'],
      label: 'height_min',
      integer: false, wide: false,
      range: null,
    });
  });

  // The sliders arc: the range keywords are read where they are declared — travel from `x-ui-range`
  // when it is there, from the legal pair when it is not, and nothing at all when the field states
  // neither. The legal bounds ride along independently of which of the two produced the travel.
  it('derives PostProcess ranges: declared travel, legal-pair travel, and none', () => {
    const fields = deriveFields(POST_PROCESS_SCHEMA, {
      bloom_strength: 0.1,
      grain_size: 0.002,
      vignette: 0.3,
      fog_height: 12,
      lift: [0, 0, 0],
    });
    // Travel declared, bounded below only: the track is the useful part, the clamp is what exists.
    expect(field(fields, 'bloom_strength')).toMatchObject({
      kind: 'number',
      range: { min: 0, max: 0.5, hardMin: 0, hardMax: null },
    });
    // Travel declared *narrower* than legal — 0.002 ships, 1.0 is absurd — and both survive.
    expect(field(fields, 'grain_size')).toMatchObject({
      kind: 'number',
      range: { min: 0, max: 0.01, hardMin: 0, hardMax: 1 },
    });
    // No travel declared: the legal pair is the track.
    expect(field(fields, 'vignette')).toMatchObject({
      kind: 'number',
      range: { min: 0, max: 1, hardMin: 0, hardMax: 1 },
    });
    // A signed world Y is genuinely unbounded — the correct answer is no slider.
    expect(field(fields, 'fog_height')).toMatchObject({ kind: 'number', range: null });
    // The array's keyword is the range of each element; the elements themselves are unbounded.
    expect(field(fields, 'lift')).toEqual({
      kind: 'vector',
      path: ['lift'],
      label: 'lift',
      length: 3,
      integer: false, wide: false,
      color: false,
      range: { min: 0, max: 0.2, hardMin: null, hardMax: null },
    });
  });

  it('falls back to JSON for a oneOf this widget cannot model (an arbitrary unsupported shape): never crashes, never drops the field', () => {
    // Two data-carrying object variants but NO unit variant to mix with: the widget models "unit
    // variants plus object variants", not "object variants alone" (Phase 10 Wave 5 lifted the one-
    // object-variant cap, but didn't lift this requirement) — a genuinely multi-branch "unsupported"
    // oneOf like Projection, same bucket as CAMERA_SCHEMA above.
    const twoObjectVariants = {
      type: 'object',
      properties: {
        target: {
          oneOf: [
            { type: 'object', properties: { asset: { type: 'string' } }, required: ['asset'] },
            { type: 'object', properties: { entity: { type: 'string' } }, required: ['entity'] },
          ],
        },
      },
    };
    expect(field(deriveFields(twoObjectVariants, { target: { asset: 'x' } }), 'target')).toEqual({
      kind: 'json',
      path: ['target'],
      label: 'target',
    });

    // A multi-property object variant: the widget only understands exactly one required string prop.
    const multiPropVariant = {
      type: 'object',
      properties: {
        shape: {
          oneOf: [
            { type: 'string', const: 'cube' },
            {
              type: 'object',
              properties: { asset: { type: 'string' }, offset: { type: 'number' } },
              required: ['asset', 'offset'],
            },
          ],
        },
      },
    };
    expect(field(deriveFields(multiPropVariant, { shape: 'cube' }), 'shape')).toMatchObject({ kind: 'json' });

    // A non-string, non-object branch (e.g. a number variant): also unsupported.
    const nonStringBranch = {
      type: 'object',
      properties: {
        value: { oneOf: [{ type: 'string', const: 'auto' }, { type: 'number' }] },
      },
    };
    expect(field(deriveFields(nonStringBranch, { value: 'auto' }), 'value')).toMatchObject({ kind: 'json' });

    // Never dropped from the form: the fallback field is always present, just not a `variant` widget.
    expect(deriveFields(twoObjectVariants, { target: { asset: 'x' } })).toHaveLength(1);
  });

  // ui-widgets: `UiList` shares `UiRect`/`UiText`'s `anchor`/`offset`/`order` shapes; `title` is a
  // plain string; `items` is variable-length and object-valued, so it falls to the raw-JSON widget —
  // the expected and accepted degradation the plan calls out (same bucket as `Tilemap.rows`), not a
  // crash and not a dropped field; `selected`/`max_visible` are bounded-below-only integers, so no
  // track (same shape as `Mesh.emissive`); `size` is an unbounded scalar number; `line_spacing` and
  // `padding` are bounded-below-only scalars too; three of the four RGBA fields are color-swatched
  // vectors, but `background` isn't (the label heuristic looks for `colou?r`, which it doesn't have).
  it('derives UiList: anchor dropdown, string title, items to raw-JSON, integer selected/max_visible, number size/line_spacing/padding, three color swatches plus a plain background vector, integer order', () => {
    const fields = deriveFields(UI_LIST_SCHEMA, {
      anchor: 'center',
      offset: [0, 0],
      title: 'MENU',
      items: [{ text: 'RESUME', enabled: true }],
      selected: 0,
      size: 24,
      line_spacing: 0,
      padding: 16,
      color: [1, 1, 1, 1],
      selected_color: [1, 0.9, 0.4, 1],
      disabled_color: [0.4, 0.4, 0.4, 1],
      background: [0, 0, 0, 0],
      order: 0,
      max_visible: 0,
    });
    expect(field(fields, 'anchor')).toEqual({
      kind: 'variant',
      path: ['anchor'],
      label: 'anchor',
      options: [
        { kind: 'unit', value: 'top_left' },
        { kind: 'unit', value: 'top_center' },
        { kind: 'unit', value: 'top_right' },
        { kind: 'unit', value: 'center_left' },
        { kind: 'unit', value: 'center' },
        { kind: 'unit', value: 'center_right' },
        { kind: 'unit', value: 'bottom_left' },
        { kind: 'unit', value: 'bottom_center' },
        { kind: 'unit', value: 'bottom_right' },
      ],
    });
    expect(field(fields, 'offset')).toEqual({ kind: 'vector', path: ['offset'], label: 'offset', length: 2, integer: false, wide: false, color: false, range: null });
    expect(field(fields, 'title')).toEqual({ kind: 'string', path: ['title'], label: 'title', nullable: false });
    expect(field(fields, 'items')).toEqual({ kind: 'json', path: ['items'], label: 'items' });
    expect(field(fields, 'selected')).toEqual({ kind: 'number', path: ['selected'], label: 'selected', integer: true, wide: false, range: null });
    expect(field(fields, 'size')).toEqual({ kind: 'number', path: ['size'], label: 'size', integer: false, wide: false, range: null });
    expect(field(fields, 'line_spacing')).toEqual({ kind: 'number', path: ['line_spacing'], label: 'line_spacing', integer: false, wide: false, range: null });
    expect(field(fields, 'padding')).toEqual({ kind: 'number', path: ['padding'], label: 'padding', integer: false, wide: false, range: null });
    for (const label of ['color', 'selected_color', 'disabled_color']) {
      expect(field(fields, label)).toMatchObject({ kind: 'vector', length: 4, color: true });
    }
    // `background` doesn't match the `colou?r` label heuristic, so it derives as an ordinary
    // (non-swatched) 4-wide vector, same degradation `Sprite.size`/`UiRect.offset` accept elsewhere.
    expect(field(fields, 'background')).toEqual({ kind: 'vector', path: ['background'], label: 'background', length: 4, integer: false, wide: false, color: false, range: null });
    expect(field(fields, 'order')).toEqual({ kind: 'number', path: ['order'], label: 'order', integer: true, wide: false, range: null });
    expect(field(fields, 'max_visible')).toEqual({ kind: 'number', path: ['max_visible'], label: 'max_visible', integer: true, wide: false, range: null });
  });

  // particles (`Plan/particles-engine-and-editor.md`): every one of `ParticleEmitter`'s 19 fields
  // must widget, none dropping to the raw-JSON fallback — the assertion that catches a future nested
  // field on this component.
  it('derives ParticleEmitter: every field gets a typed widget, none falls to JSON, in schema order', () => {
    const fields = deriveFields(PARTICLE_EMITTER_SCHEMA, PARTICLE_EMITTER_DEFAULT);
    expect(fields.map((f) => [f.label, f.kind])).toEqual([
      ['shape', 'variant'],
      ['extent', 'vector'],
      ['rate', 'number'],
      ['burst', 'number'],
      ['started', 'number'],
      ['duration', 'number'],
      ['life', 'vector'],
      ['speed', 'vector'],
      ['spread_deg', 'number'],
      ['gravity', 'number'],
      ['drag', 'number'],
      ['floor', 'number'],
      ['start_size', 'number'],
      ['end_size', 'number'],
      ['start_color', 'vector'],
      ['end_color', 'vector'],
      ['blend', 'variant'],
      ['softness', 'number'],
      ['seed', 'number'],
    ]);
    expect(fields.some((f) => f.kind === 'json')).toBe(false);
  });

  // The thirteen "register" fields each carry `#[schemars(extend("x-ui-range" = …))]`, so each gets
  // exactly that travel as its range — the three vectors (`extent`, `life`, `speed`) read it off the
  // array node and apply it per element, the `PostProcess.lift` shape. `started`/`seed` (`u64`) declare
  // no travel at all, so both stay plain integer number boxes with no track.
  it('derives ParticleEmitter ranges: x-ui-range travel on the 13 register fields, integer started/seed with none', () => {
    const fields = deriveFields(PARTICLE_EMITTER_SCHEMA, PARTICLE_EMITTER_DEFAULT);
    const ranged: Record<string, { min: number; max: number }> = {
      extent: { min: 0, max: 4 },
      rate: { min: 0, max: 200 },
      burst: { min: 0, max: 512 },
      duration: { min: 0, max: 10 },
      life: { min: 0, max: 5 },
      speed: { min: 0, max: 20 },
      spread_deg: { min: 0, max: 180 },
      gravity: { min: -20, max: 20 },
      drag: { min: 0, max: 10 },
      floor: { min: -4, max: 4 },
      start_size: { min: 0, max: 4 },
      end_size: { min: 0, max: 4 },
      softness: { min: 0, max: 1 },
    };
    for (const [label, { min, max }] of Object.entries(ranged)) {
      expect(field(fields, label), label).toMatchObject({ range: { min, max } });
    }
    expect(field(fields, 'started')).toEqual({ kind: 'number', path: ['started'], label: 'started', integer: true, wide: true, range: null });
    expect(field(fields, 'seed')).toEqual({ kind: 'number', path: ['seed'], label: 'seed', integer: true, wide: true, range: null });
    // burst is the integer among the thirteen — still gets a track like any other, its automatic
    // `minimum: 0` riding along as the range's hard lower bound.
    expect(field(fields, 'burst')).toMatchObject({
      kind: 'number',
      integer: true, wide: false,
      range: { min: 0, max: 512, hardMin: 0, hardMax: null },
    });
  });

  // `FieldWidgets.tsx`'s vector case draws a `.field-swatch` exactly when `field.color` is true — that
  // boolean, not a re-stated label heuristic, is what this test reads.
  it('derives ParticleEmitter.start_color/end_color as swatched 4-wide vectors', () => {
    const fields = deriveFields(PARTICLE_EMITTER_SCHEMA, PARTICLE_EMITTER_DEFAULT);
    for (const label of ['start_color', 'end_color']) {
      const swatched = field(fields, label);
      expect(swatched).toMatchObject({ kind: 'vector', length: 4, range: null });
      expect((swatched as { color: boolean }).color).toBe(true);
    }
  });

  it('derives ParticleEmitter.shape and .blend as unit-only variant dropdowns (4 and 2 options)', () => {
    const fields = deriveFields(PARTICLE_EMITTER_SCHEMA, PARTICLE_EMITTER_DEFAULT);
    expect(field(fields, 'shape')).toEqual({
      kind: 'variant',
      path: ['shape'],
      label: 'shape',
      options: [
        { kind: 'unit', value: 'point' },
        { kind: 'unit', value: 'sphere' },
        { kind: 'unit', value: 'box' },
        { kind: 'unit', value: 'disc' },
      ],
    });
    expect(field(fields, 'blend')).toEqual({
      kind: 'variant',
      path: ['blend'],
      label: 'blend',
      options: [
        { kind: 'unit', value: 'alpha' },
        { kind: 'unit', value: 'additive' },
      ],
    });
  });
});

// --- deriveFields: shape edge cases --------------------------------------------------------------

describe('deriveFields edge cases', () => {
  it('marks JSON Schema integer fields as integer', () => {
    const fields = deriveFields({ type: 'object', properties: { count: { type: 'integer' } } }, { count: 3 });
    expect(field(fields, 'count')).toEqual({ kind: 'number', path: ['count'], label: 'count', integer: true, wide: false, range: null });
  });

  it('reads a bounded integer field’s range like any other number', () => {
    const schema = { type: 'object', properties: { order: { type: 'integer', minimum: -8, maximum: 8 } } };
    expect(field(deriveFields(schema, { order: 0 }), 'order')).toMatchObject({
      integer: true, wide: false,
      range: { min: -8, max: 8, hardMin: -8, hardMax: 8 },
    });
  });

  it('reads a vector’s legal bounds off items, per element', () => {
    // The shape `#[schemars(inner(range(…)))]` emits. The engine's own fixed arrays deliberately
    // carry element-wise semantic diagnostics instead, but items is where per-element bounds belong,
    // so that is where they are read from.
    const schema = {
      type: 'object',
      properties: {
        gain: { type: 'array', items: { type: 'number', minimum: 0, maximum: 4 }, minItems: 3, maxItems: 3 },
      },
    };
    expect(field(deriveFields(schema, {}), 'gain')).toMatchObject({
      kind: 'vector',
      range: { min: 0, max: 4, hardMin: 0, hardMax: 4 },
    });
  });

  it('ignores range keywords the array node carries in the scalar keys — travel is the array’s, bounds are the elements’', () => {
    // `minimum`/`maximum` on the array node itself would describe the array, not its elements; only
    // `x-ui-range` is understood there.
    const schema = {
      type: 'object',
      properties: {
        gain: {
          type: 'array',
          items: { type: 'number' },
          minItems: 3,
          maxItems: 3,
          minimum: 0,
          maximum: 4,
        },
      },
    };
    expect(field(deriveFields(schema, {}), 'gain')).toMatchObject({ kind: 'vector', range: null });
  });

  it('degrades a malformed x-ui-range to no range at all, never a broken slider', () => {
    const malformed: readonly unknown[] = [
      [0],
      [0, 1, 2],
      [1, 0], // lo >= hi: no track to draw
      [0, 0],
      ['0', '1'],
      [0, Number.NaN],
      [0, null],
      0.5,
      null,
      { min: 0, max: 1 },
    ];
    for (const value of malformed) {
      const schema = { type: 'object', properties: { knob: { type: 'number', 'x-ui-range': value } } };
      expect(field(deriveFields(schema, { knob: 0 }), 'knob'), JSON.stringify(value)).toMatchObject({
        kind: 'number',
        range: null,
      });
    }
  });

  it('falls back to the legal pair when x-ui-range is malformed, and ignores non-numeric bounds', () => {
    const withLegal = {
      type: 'object',
      properties: { knob: { type: 'number', minimum: 0, maximum: 1, 'x-ui-range': [2, 1] } },
    };
    expect(field(deriveFields(withLegal, { knob: 0 }), 'knob')).toMatchObject({
      range: { min: 0, max: 1, hardMin: 0, hardMax: 1 },
    });

    const badBounds = {
      type: 'object',
      properties: { knob: { type: 'number', minimum: '0', maximum: true, 'x-ui-range': [0, 1] } },
    };
    expect(field(deriveFields(badBounds, { knob: 0 }), 'knob')).toMatchObject({
      range: { min: 0, max: 1, hardMin: null, hardMax: null },
    });
  });

  it('detects a color by a 4-wide numeric array whose name reads as colour', () => {
    const schema = { type: 'object', properties: { colour: { type: 'array', items: { type: 'number' }, minItems: 4, maxItems: 4 } } };
    expect(field(deriveFields(schema, {}), 'colour')).toMatchObject({ kind: 'vector', color: true });
  });

  it('does not treat a same-width numeric array with a non-colour name as a color', () => {
    const schema = { type: 'object', properties: { size: { type: 'array', items: { type: 'number' }, minItems: 3, maxItems: 3 } } };
    expect(field(deriveFields(schema, {}), 'size')).toMatchObject({ kind: 'vector', color: false });
  });

  it('falls back to JSON for a variable-length array (min != max)', () => {
    const schema = { type: 'object', properties: { tags: { type: 'array', items: { type: 'number' }, minItems: 1, maxItems: 4 } } };
    expect(field(deriveFields(schema, {}), 'tags')).toMatchObject({ kind: 'json' });
  });

  it('falls back to JSON for a non-numeric array', () => {
    const schema = { type: 'object', properties: { names: { type: 'array', items: { type: 'string' }, minItems: 2, maxItems: 2 } } };
    expect(field(deriveFields(schema, {}), 'names')).toMatchObject({ kind: 'json' });
  });

  it('falls back to JSON for a nested struct property', () => {
    const schema = { type: 'object', properties: { inner: { type: 'object', properties: { a: { type: 'number' } } } } };
    expect(field(deriveFields(schema, {}), 'inner')).toMatchObject({ kind: 'json' });
  });

  it('falls back to JSON for a nullable non-string (e.g. ["number","null"])', () => {
    const schema = { type: 'object', properties: { maybe: { type: ['number', 'null'] } } };
    expect(field(deriveFields(schema, {}), 'maybe')).toMatchObject({ kind: 'json' });
  });

  it('falls back to JSON for an unresolvable $ref', () => {
    const schema = { type: 'object', properties: { x: { $ref: '#/$defs/Missing' } } };
    expect(field(deriveFields(schema, {}), 'x')).toMatchObject({ kind: 'json' });
  });

  it('surfaces a value key the schema does not declare as a JSON field', () => {
    const schema = { type: 'object', properties: { a: { type: 'number' } } };
    const fields = deriveFields(schema, { a: 1, extra: { nested: true } });
    expect(field(fields, 'extra')).toEqual({ kind: 'json', path: ['extra'], label: 'extra' });
  });

  it('falls back to a single JSON field for an entirely unrecognised schema', () => {
    const fields = deriveFields({ description: 'mystery' }, { anything: 1 });
    expect(fields).toEqual([{ kind: 'json', path: [], label: '' }]);
  });
});

// --- valueAtPath ---------------------------------------------------------------------------------

describe('valueAtPath', () => {
  it('reads object keys and array indices', () => {
    const value = { translation: [1, 2, 3], nested: { flag: true } };
    expect(valueAtPath(value, ['translation'])).toEqual([1, 2, 3]);
    expect(valueAtPath(value, ['translation', 1])).toBe(2);
    expect(valueAtPath(value, ['nested', 'flag'])).toBe(true);
  });

  it('reads the whole value at the empty path', () => {
    expect(valueAtPath('paddle', [])).toBe('paddle');
  });

  it('returns undefined when a step is missing', () => {
    expect(valueAtPath({ a: 1 }, ['b', 'c'])).toBeUndefined();
    expect(valueAtPath(null, ['a'])).toBeUndefined();
  });
});

// --- applyFieldEdit ------------------------------------------------------------------------------

describe('applyFieldEdit', () => {
  it('replaces the whole value at the empty path (scalar component)', () => {
    expect(applyFieldEdit('old', [], 'new')).toBe('new');
  });

  it('replaces one object field, leaving the rest', () => {
    const current = { translation: [0, 0, 0], scale: [1, 1, 1] };
    const next = applyFieldEdit(current, ['scale'], [2, 2, 2]);
    expect(next).toEqual({ translation: [0, 0, 0], scale: [2, 2, 2] });
  });

  it('replaces one array element within a field', () => {
    const current = { translation: [1, 2, 3] };
    const next = applyFieldEdit(current, ['translation', 1], 9);
    expect(next).toEqual({ translation: [1, 9, 3] });
  });

  // Mirrors what the `variant` widget actually sends through `commit(field.path, …)` — the whole
  // replacement value (bare string or `{ asset }` object) is handed straight to `applyFieldEdit`, same
  // as the JSON fallback widget. Both forms must assemble into a valid `component.set { value }` body.
  it('assembles a Mesh component with either MeshSource value form at the shape field', () => {
    const current = { shape: 'cube', color: [1, 1, 1, 1] };
    const toPlane = applyFieldEdit(current, ['shape'], 'plane');
    expect(toPlane).toEqual({ shape: 'plane', color: [1, 1, 1, 1] });
    const toAsset = applyFieldEdit(current, ['shape'], { asset: 'meshes/rock.mesh.yaml' });
    expect(toAsset).toEqual({ shape: { asset: 'meshes/rock.mesh.yaml' }, color: [1, 1, 1, 1] });
    // And switching back from the object form to a unit form drops the object cleanly.
    const backToUnit = applyFieldEdit(toAsset, ['shape'], 'sphere');
    expect(backToUnit).toEqual({ shape: 'sphere', color: [1, 1, 1, 1] });
  });

  // Phase 10 Wave 5: a second object variant commits through the exact same path — `applyFieldEdit`
  // just replaces the whole field value, so it never distinguishes which object variant it is.
  it('assembles a Mesh component with either object variant form (asset or voxel) at the shape field', () => {
    const current = { shape: 'cube', color: [1, 1, 1, 1] };
    const toAsset = applyFieldEdit(current, ['shape'], { asset: 'meshes/rock.mesh.yaml' });
    expect(toAsset).toEqual({ shape: { asset: 'meshes/rock.mesh.yaml' }, color: [1, 1, 1, 1] });
    const toVoxel = applyFieldEdit(toAsset, ['shape'], { voxel: 'voxels/lantern.voxel.yaml' });
    expect(toVoxel).toEqual({ shape: { voxel: 'voxels/lantern.voxel.yaml' }, color: [1, 1, 1, 1] });
    // And switching between the two object forms replaces cleanly, no leftover `asset` key.
    expect(toVoxel).not.toHaveProperty('shape.asset');
  });

  // Phase 11 Wave 1: a `roughness`/`metallic`/`emissive` edit commits through the exact same
  // `commit(field.path, value)` -> `applyFieldEdit` path as any other plain number field (e.g. `near`
  // on Camera) — the number-field widget's existing commit logic needs no changes for the new fields.
  it('commits an edited PBR field (roughness), leaving the other Mesh fields untouched', () => {
    const current = { shape: 'cube', color: [1, 1, 1, 1], roughness: 1, metallic: 0, emissive: 0 };
    const next = applyFieldEdit(current, ['roughness'], 0.4);
    expect(next).toEqual({ shape: 'cube', color: [1, 1, 1, 1], roughness: 0.4, metallic: 0, emissive: 0 });
  });

  // Phase 12 Wave 5: both Collider fields commit through the same `commit(field.path, value)` →
  // `applyFieldEdit` path as every other widget — the body dropdown sends the bare variant string,
  // the shape JSON textarea sends the whole parsed object; each assembles into a full
  // `component.set { value }` body without touching the other field.
  it('assembles a Collider from a body dropdown commit and from a shape JSON commit', () => {
    const current = { shape: { circle: { radius: 0.5 } }, body: 'static' };
    const toDynamic = applyFieldEdit(current, ['body'], 'dynamic');
    expect(toDynamic).toEqual({ shape: { circle: { radius: 0.5 } }, body: 'dynamic' });
    const toAabb = applyFieldEdit(toDynamic, ['shape'], { aabb: { half_extents: [0.5, 2.0] } });
    expect(toAabb).toEqual({ shape: { aabb: { half_extents: [0.5, 2.0] } }, body: 'dynamic' });
    // Switching the shape's object variant replaces cleanly — no leftover `circle` key.
    expect(toAabb).not.toHaveProperty('shape.circle');
  });

  // Environment/IBL Wave 1: an `Environment`/`Fade` edit commits through the exact same
  // `commit(field.path, value)` -> `applyFieldEdit` path as any other vector/number field — a sky
  // color element and a scalar knob each assemble into a full `component.set { value }` body without
  // touching the rest of the component.
  it('commits an Environment color element and a scalar, and a Fade field, leaving siblings untouched', () => {
    const env = { sky_zenith: [0.02, 0.03, 0.06], ambient: [0.05, 0.05, 0.06], ibl_intensity: 1, sun_disc_size: 5 };
    const zenithEdit = applyFieldEdit(env, ['sky_zenith', 2], 0.9);
    expect(zenithEdit).toEqual({ sky_zenith: [0.02, 0.03, 0.9], ambient: [0.05, 0.05, 0.06], ibl_intensity: 1, sun_disc_size: 5 });
    const iblEdit = applyFieldEdit(zenithEdit, ['ibl_intensity'], 1.5);
    expect(iblEdit).toEqual({ sky_zenith: [0.02, 0.03, 0.9], ambient: [0.05, 0.05, 0.06], ibl_intensity: 1.5, sun_disc_size: 5 });

    const fade = { fade: 1, height_min: -3.4e38 };
    expect(applyFieldEdit(fade, ['fade'], 0.25)).toEqual({ fade: 0.25, height_min: -3.4e38 });
  });

  it('does not mutate the input value (containers are cloned)', () => {
    const current = { translation: [1, 2, 3] };
    const next = applyFieldEdit(current, ['translation', 0], 5);
    expect(current).toEqual({ translation: [1, 2, 3] });
    expect(next).not.toBe(current);
    expect((next as { translation: number[] }).translation).not.toBe(current.translation);
  });

  it('creates containers from the path when the current value is partial', () => {
    expect(applyFieldEdit(undefined, ['a', 0], 7)).toEqual({ a: [7] });
  });

  // The inspector's stale-assembly guard rests on this: rapid sequential commits must each assemble
  // from the PREVIOUS assembled value (the form's pending base), not from the last polled value —
  // otherwise the full value sent for edit B silently reverts edit A (the poll is ~300 ms stale).
  it('composes sequential edits when each assembles from the previous result', () => {
    const polled = { translation: [0, 0, 0], rotation: [0, 0, 0], scale: [1, 1, 1] };
    const afterA = applyFieldEdit(polled, ['translation', 0], 5);
    const afterB = applyFieldEdit(afterA, ['translation', 1], 7);
    expect(afterB).toEqual({ translation: [5, 7, 0], rotation: [0, 0, 0], scale: [1, 1, 1] });
    // …whereas assembling B from the stale polled base loses A — the race the pending base prevents.
    const clobbered = applyFieldEdit(polled, ['translation', 1], 7) as { translation: number[] };
    expect(clobbered.translation).toEqual([0, 7, 0]);
  });
});

// --- 64-bit integers survive an inspector round trip ---------------------------------------------

describe('u64 round trip', () => {
  const VOLLEY =
    '{"pattern":"p","count":8,"angle_deg":3.5,"speed":[2.0,4.0],"seed":18446744073709551614,' +
    '"killed":[9223372036854775809,0,18446744073709551615,1]}';

  it('a Volley nudge keeps seed and killed digit-exact', () => {
    const edited = applyFieldEdit(parseWire(VOLLEY), ['angle_deg'], 12.5);
    expect(stringifyWire(edited)).toBe(
      '{"pattern":"p","count":8,"angle_deg":12.5,"speed":[2,4],"seed":18446744073709551614,' +
        '"killed":[9223372036854775809,0,18446744073709551615,1]}',
    );
  });

  it('a ParticleEmitter edit keeps a seed above 2^53', () => {
    const edited = applyFieldEdit(parseWire('{"rate":10.0,"seed":18446744073709551615}'), ['rate'], 12);
    expect(stringifyWire(edited)).toBe('{"rate":12,"seed":18446744073709551615}');
  });

  it('a typed wide draft becomes a bigint, a safe one a number', () => {
    expect(parseNumberDraft('18446744073709551614', true, true)).toBe(18446744073709551614n);
    expect(parseNumberDraft(' 42 ', true, true)).toBe(42);
    expect(parseNumberDraft('1.5', true, true)).toBeNull();
  });

  it('derives wide from the schema format', () => {
    const schema = {
      type: 'object',
      properties: {
        seed: { type: 'integer', format: 'uint64', minimum: 0 },
        count: { type: 'integer', format: 'uint32', minimum: 0 },
        killed: { type: 'array', items: { type: 'integer', format: 'uint64' }, minItems: 4, maxItems: 4 },
      },
    };
    const fields = deriveFields(schema, {});
    expect(fields.map((f) => (f.kind === 'number' || f.kind === 'vector' ? f.wide : null))).toEqual([true, false, true]);
  });
});
