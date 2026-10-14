import { describe, expect, it } from 'vitest';
import type { FieldPath } from '../../viewer/inspect-fields';
import {
  defaultForSchema,
  deriveDocForm,
  formLeaves,
  helpText,
  markFloats,
  subschemaAt,
  type DocListNode,
  type DocMapNode,
  type DocNode,
  type DocOptionalNode,
} from './docFields';
import { PATTERN_SCHEMA } from './patternSchema';
import { Float } from './yamlDoc';

/** A document exercising every container the schema can build: a bullet kind with a motion segment
 *  carrying BOTH phases, and a timeline entry with its optional speed range set. Every leaf of
 *  `PatternDoc` is reachable from it, which is what makes the coverage assertion below meaningful. */
const FULL = {
  format: 1,
  kind: 'pattern',
  name: 'boss-phase-1',
  bullets: {
    needle: {
      atlas: 'art/bullets.atlas.yaml',
      frame: 'needle',
      size: [0.3, 0.3],
      color: [1, 0.4, 0.8, 1],
      blend: 'additive',
      layer: 0,
      billboard: false,
      hit_radius: 0.12,
      life: 240,
      motion: [
        { ramp: { frames: 30, from: 6, to: 2 }, turn: { frames: 90, speed: 2, rate_deg: 45 } },
      ],
    },
  },
  timeline: [
    {
      at: 0,
      play: null,
      bullet: 'needle',
      repeat: 8,
      every: 12,
      shape: 'ring',
      center: 'absolute',
      spread_deg: 0,
      count: 24,
      speed: 4,
      speed_range: [4, 6],
      angle_deg: 0,
      angle_step_deg: 7.5,
      speed_step: 0,
    },
  ],
  loop: 240,
};

function nodeAt(nodes: readonly DocNode[], path: FieldPath): DocNode {
  const key = path.join('.');
  const found: DocNode[] = [];
  const walk = (node: DocNode): void => {
    if (node.path.join('.') === key) {
      found.push(node);
    }
    switch (node.kind) {
      case 'group':
        node.children.forEach(walk);
        return;
      case 'map':
        node.entries.forEach((entry) => walk(entry.node));
        return;
      case 'list':
        node.items.forEach(walk);
        return;
      case 'optional':
        walk(node.inner);
        return;
      default:
    }
  };
  nodes.forEach(walk);
  if (found.length === 0) {
    throw new Error(`no node at ${key}`);
  }
  return found[0];
}

describe('deriveDocForm over the real PatternDoc schema', () => {
  const nodes = deriveDocForm(PATTERN_SCHEMA, FULL);

  it('offers the schema properties in order and never the document envelope', () => {
    expect(nodes.map((node) => node.label)).toEqual(['name', 'bullets', 'timeline', 'loop']);
  });

  it('widgets EVERY leaf — no raw-JSON bucket anywhere in the document', () => {
    const leaves = formLeaves(nodes);
    expect(leaves.filter((leaf) => leaf.kind === 'json')).toEqual([]);
    // One bullet kind (9 fields + a segment's ramp 3 and turn 3), one timeline entry (14), plus
    // `name` and `loop`. A field added in Rust moves this number, which is the point.
    expect(leaves).toHaveLength(31);
  });

  it('routes each kind of field to the widget that fits it', () => {
    const kindAt = (path: FieldPath): string => {
      const node = nodeAt(nodes, path);
      if (node.kind !== 'field') {
        throw new Error(`${path.join('.')} is a ${node.kind}, not a field`);
      }
      return node.field.kind;
    };
    expect(kindAt(['name'])).toBe('string');
    expect(kindAt(['loop'])).toBe('number');
    expect(kindAt(['bullets', 'needle', 'atlas'])).toBe('string');
    expect(kindAt(['bullets', 'needle', 'size'])).toBe('vector');
    expect(kindAt(['bullets', 'needle', 'blend'])).toBe('variant');
    expect(kindAt(['bullets', 'needle', 'billboard'])).toBe('boolean');
    expect(kindAt(['bullets', 'needle', 'motion', 0, 'ramp', 'to'])).toBe('number');
    expect(kindAt(['timeline', 0, 'shape'])).toBe('variant');
    expect(kindAt(['timeline', 0, 'center'])).toBe('variant');
  });

  it('gives the optional speed range a vector inside its toggle', () => {
    const range = nodeAt(nodes, ['timeline', 0, 'speed_range']) as DocOptionalNode;
    expect(range.kind).toBe('optional');
    expect(range.present).toBe(true);
    expect(range.inner.kind === 'field' && range.inner.field.kind).toBe('vector');
    expect(range.enabled).toEqual([0, 0]);
  });

  it('keeps the colour swatch, which depends on the field NAME surviving the walk', () => {
    const node = nodeAt(nodes, ['bullets', 'needle', 'color']);
    expect(node.kind === 'field' && node.field.kind === 'vector' && node.field.color).toBe(true);
  });

  it('makes `bullets` a keyed map and `timeline`/`motion` ordered lists', () => {
    const bullets = nodeAt(nodes, ['bullets']) as DocMapNode;
    expect(bullets.kind).toBe('map');
    expect(bullets.entries.map((entry) => entry.key)).toEqual(['needle']);

    const timeline = nodeAt(nodes, ['timeline']) as DocListNode;
    expect(timeline.kind).toBe('list');
    expect(timeline.items).toHaveLength(1);
    expect(timeline.values).toHaveLength(1);

    const motion = nodeAt(nodes, ['bullets', 'needle', 'motion']) as DocListNode;
    expect(motion.kind).toBe('list');
    expect(motion.items).toHaveLength(1);
  });

  it('makes a nullable struct or number an optional block, and reads its presence off the value', () => {
    const ramp = nodeAt(nodes, ['bullets', 'needle', 'motion', 0, 'ramp']) as DocOptionalNode;
    expect(ramp.kind).toBe('optional');
    expect(ramp.present).toBe(true);
    expect(ramp.inner.kind).toBe('group');

    const from = nodeAt(nodes, ['bullets', 'needle', 'motion', 0, 'ramp', 'from']) as DocOptionalNode;
    expect(from.kind).toBe('optional');
    expect(from.enabled).toBe(0);
  });

  it('reads an ABSENT optional as off rather than as a value', () => {
    const sparse = { bullets: { needle: { motion: [{}] } } };
    const ramp = nodeAt(
      deriveDocForm(PATTERN_SCHEMA, sparse),
      ['bullets', 'needle', 'motion', 0, 'ramp'],
    ) as DocOptionalNode;
    expect(ramp.present).toBe(false);
  });

  it('carries a nullable string as a plain nullable text field, not an optional block', () => {
    const play = nodeAt(nodes, ['timeline', 0, 'play']);
    expect(play.kind).toBe('field');
    expect(play.kind === 'field' && play.field.kind === 'string' && play.field.nullable).toBe(true);
  });

  it('yields nothing for a schema that declares no properties', () => {
    expect(deriveDocForm({ type: 'string' }, 'x')).toEqual([]);
  });
});

describe('defaultForSchema', () => {
  it('reproduces BulletKind::default(), omitting the null and empty fields', () => {
    const value = defaultForSchema(subschemaAt(PATTERN_SCHEMA, ['bullets', 'any']), PATTERN_SCHEMA) as Record<
      string,
      unknown
    >;
    expect(Object.keys(value)).toEqual([
      'size',
      'color',
      'blend',
      'layer',
      'billboard',
      'hit_radius',
      'life',
    ]);
    expect(value.blend).toBe('alpha');
    expect(value.life).toBe(240);
    expect(value.size).toEqual([0.25, 0.25]);
  });

  it('reproduces TimelineEntry::default(), leaving `play`, `bullet` and `speed_range` out', () => {
    const value = defaultForSchema(subschemaAt(PATTERN_SCHEMA, ['timeline', 0]), PATTERN_SCHEMA) as Record<
      string,
      unknown
    >;
    expect(Object.keys(value)).toEqual([
      'at',
      'repeat',
      'every',
      'shape',
      'center',
      'spread_deg',
      'count',
      'speed',
      'angle_deg',
      'angle_step_deg',
      'speed_step',
    ]);
    expect(value.repeat).toBe(1);
    expect(value.shape).toBe('ring');
    expect(value.center).toBe('absolute');
  });

  it('builds a fixed-length array from its bound and omits an unbounded empty one', () => {
    expect(defaultForSchema(subschemaAt(PATTERN_SCHEMA, ['bullets', 'any', 'size']), PATTERN_SCHEMA)).toEqual([
      0.25, 0.25,
    ]);
    expect(defaultForSchema(subschemaAt(PATTERN_SCHEMA, ['bullets', 'any', 'motion']), PATTERN_SCHEMA)).toBeUndefined();
  });

  it('says ABSENT for a nullable field — turning one on writes the optional node enabled value', () => {
    expect(
      defaultForSchema(subschemaAt(PATTERN_SCHEMA, ['timeline', 0, 'speed_range']), PATTERN_SCHEMA),
    ).toBeUndefined();
    expect(defaultForSchema(subschemaAt(PATTERN_SCHEMA, ['bullets', 'a', 'atlas']), PATTERN_SCHEMA)).toBeUndefined();
  });

  it('takes the first variant of a unit enum', () => {
    expect(defaultForSchema({ oneOf: [{ const: 'ring' }, { const: 'arc' }] }, {})).toBe('ring');
  });
});

describe('subschemaAt', () => {
  it('walks properties, map entries and list indices, including into a vector element', () => {
    expect((subschemaAt(PATTERN_SCHEMA, ['bullets', 'needle', 'life']) as { format?: string }).format).toBe('uint32');
    expect((subschemaAt(PATTERN_SCHEMA, ['bullets', 'needle', 'size', 1]) as { format?: string }).format).toBe(
      'float',
    );
    expect(subschemaAt(PATTERN_SCHEMA, ['timeline', 3, 'rate_deg'])).toBeUndefined();
  });

  it('steps through a nullable wrapper to the value inside', () => {
    expect((subschemaAt(PATTERN_SCHEMA, ['bullets', 'n', 'motion', 0, 'turn', 'rate_deg']) as { format?: string })
      .format).toBe('float');
  });
});

describe('markFloats', () => {
  it('marks what the schema calls a number and leaves an integer alone', () => {
    expect(markFloats(4, subschemaAt(PATTERN_SCHEMA, ['timeline', 0, 'speed']), PATTERN_SCHEMA)).toBeInstanceOf(Float);
    expect(markFloats(240, subschemaAt(PATTERN_SCHEMA, ['bullets', 'n', 'life']), PATTERN_SCHEMA)).toBe(240);
  });

  it('walks a whole new entry, marking each float inside it', () => {
    const marked = markFloats(
      { size: [0.25, 0.25], life: 240, hit_radius: 0.1 },
      subschemaAt(PATTERN_SCHEMA, ['bullets', 'n']),
      PATTERN_SCHEMA,
    ) as Record<string, unknown>;
    expect(marked.life).toBe(240);
    expect(marked.hit_radius).toBeInstanceOf(Float);
    expect((marked.size as unknown[])[0]).toBeInstanceOf(Float);
  });

  it('marks through a nullable wrapper and leaves a string untouched', () => {
    expect(markFloats(2, subschemaAt(PATTERN_SCHEMA, ['bullets', 'n', 'motion', 0, 'ramp', 'from']), PATTERN_SCHEMA))
      .toBeInstanceOf(Float);
    expect(markFloats('needle', subschemaAt(PATTERN_SCHEMA, ['timeline', 0, 'bullet']), PATTERN_SCHEMA)).toBe('needle');
  });
});

describe('helpText', () => {
  it('flattens rustdoc links and code ticks onto one line', () => {
    expect(helpText('How the quad composites (default [`SpriteBlend::Alpha`](crate::SpriteBlend)) —\nthe flag.')).toBe(
      'How the quad composites (default SpriteBlend::Alpha) — the flag.',
    );
    expect(helpText('Frames between repeats (default `0`).')).toBe('Frames between repeats (default 0).');
  });

  it('has nothing to say about a missing or empty description', () => {
    expect(helpText(undefined)).toBeNull();
    expect(helpText('   ')).toBeNull();
  });
});
