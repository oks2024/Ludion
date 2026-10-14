import { describe, expect, it } from 'vitest';
import { numberAt, parseRecipeDoc, textAt, type RecipeDoc } from './doc';
import {
  formatRecipeNumber,
  invertSplice,
  quoteText,
  sanitizeText,
  setNumber,
  setText,
  spliceSource,
  type RecipeEdit,
  type Splice,
} from './edit';

// Edited against a frozen, hand-authored recipe, like `sculpt/edit.test.ts` edits a real gallery
// model: the property under test is that a hand-authored file survives an edit byte-for-byte
// outside the one token touched, and only a real file has a long prompt to lose.
const FILES = import.meta.glob(
  '../../../fixtures/stage/generation/recipes/lantern-portrait.gen.yaml',
  { query: '?raw', import: 'default', eager: true },
) as Record<string, string>;

function portraitSource(): string {
  const keys = Object.keys(FILES);
  expect(keys, 'lantern-portrait.gen.yaml is missing from the frozen stage fixture').toHaveLength(1);
  return FILES[keys[0]];
}

const PORTRAIT = portraitSource();

/** Asserts `after` differs from `before` only inside the region `splice` describes. */
function assertOnlySpliceChanged(before: string, after: string, splice: Splice): void {
  expect(after.slice(0, splice.offset)).toBe(before.slice(0, splice.offset));
  expect(after.slice(splice.offset, splice.offset + splice.inserted.length)).toBe(splice.inserted);
  expect(after.slice(splice.offset + splice.inserted.length)).toBe(
    before.slice(splice.offset + splice.removed.length),
  );
}

function ok(result: RecipeEdit): Splice {
  expect(result.splice, 'expected an edit, got a no-op').not.toBeNull();
  return result.splice as Splice;
}

function doc(source: string): RecipeDoc {
  return parseRecipeDoc(source);
}

describe('reading a recipe positionally', () => {
  it('finds the run values, the node overrides and the prompt in the real recipe', () => {
    const parsed = doc(PORTRAIT);
    expect(parsed.name).toBe('lantern-portrait');
    expect(parsed.workflow).toBe('../workflows/txt2img.json');
    expect(parsed.seed?.value).toBe(300);
    expect(parsed.seed?.integer).toBe(true);
    expect(parsed.count?.value).toBe(8);
    expect(parsed.nodes.map((node) => node.title)).toEqual([
      'Empty Latent',
      'Positive Prompt',
      'Scheduler',
    ]);
    expect(numberAt(parsed, '/inputs/Empty Latent/width')?.value).toBe(1024);
    const prompt = textAt(parsed, '/inputs/Positive Prompt/text');
    expect(prompt?.value.startsWith('A studio product shot of a small brass oil lantern')).toBe(true);
    // The span covers the quotes, so a rewrite replaces the quoting too.
    expect(PORTRAIT[prompt?.offset ?? -1]).toBe('"');
    expect(PORTRAIT[(prompt?.offset ?? 0) + (prompt?.length ?? 0) - 1]).toBe('"');
  });

  it('reads an image reference as read-only, never as a value', () => {
    const parsed = doc(
      [
        'format: 1',
        'kind: generation',
        'images:',
        '  subject: ../references/lantern-reference.png',
        'inputs:',
        '  Load Subject:',
        '    image:',
        '      image: subject',
        '  Bake Texture:',
        '    texture_size: 1024',
        '',
      ].join('\n'),
    );
    expect(parsed.images).toEqual([{ name: 'subject', path: '../references/lantern-reference.png' }]);
    const load = parsed.nodes.find((node) => node.title === 'Load Subject');
    expect(load?.refs).toEqual([{ key: 'image', image: 'subject' }]);
    expect(load?.numbers).toEqual([]);
    expect(load?.texts).toEqual([]);
    // A reference has no pointer among the editable values, so no control can be bound to one.
    expect(numberAt(parsed, '/inputs/Load Subject/image')).toBeNull();
    expect(textAt(parsed, '/inputs/Load Subject/image')).toBeNull();
    expect(numberAt(parsed, '/inputs/Bake Texture/texture_size')?.value).toBe(1024);
  });

  it('keeps a quoted number a string and escapes a title into its pointer', () => {
    const parsed = doc(
      ['inputs:', '  "A/B Switch":', '    value: "1024"', '    steps: 20', ''].join('\n'),
    );
    expect(parsed.nodes[0].title).toBe('A/B Switch');
    // `"1024"` is a string to the graph, so it must not be offered a number control.
    expect(textAt(parsed, '/inputs/A~1B Switch/value')?.value).toBe('1024');
    expect(numberAt(parsed, '/inputs/A~1B Switch/value')).toBeNull();
    expect(numberAt(parsed, '/inputs/A~1B Switch/steps')?.value).toBe(20);
  });

  it('never throws on a file it cannot fully understand', () => {
    for (const source of ['', 'not: yaml: at all', 'inputs:', 'inputs:\n  Node:\n', '# comment\n']) {
      expect(() => parseRecipeDoc(source)).not.toThrow();
    }
    expect(parseRecipeDoc('inputs:\n  Node:\n').nodes[0].numbers).toEqual([]);
  });
});

describe('writing a number', () => {
  it('preserves an integer spelling, because a ComfyUI INT input refuses 1024.0', () => {
    const parsed = doc(PORTRAIT);
    const result = setNumber(parsed, '/inputs/Empty Latent/width', 768);
    const splice = ok(result);
    expect(splice.removed).toBe('1024');
    expect(splice.inserted).toBe('768');
    assertOnlySpliceChanged(PORTRAIT, result.source, splice);
    // A whole value from a float slider still lands as an integer on an integer-spelled field.
    expect(ok(setNumber(parsed, '/count', 12)).inserted).toBe('12');
    expect(formatRecipeNumber(7.0, true)).toBe('7');
  });

  it('keeps a decimal point a field already had', () => {
    const parsed = doc(['inputs:', '  Sampler:', '    cfg: 3.5', ''].join('\n'));
    expect(ok(setNumber(parsed, '/inputs/Sampler/cfg', 4)).inserted).toBe('4.0');
    expect(ok(setNumber(parsed, '/inputs/Sampler/cfg', 4.25)).inserted).toBe('4.25');
    expect(formatRecipeNumber(4, false)).toBe('4.0');
  });

  it('is a no-op for an unknown pointer, a non-finite value and a value already there', () => {
    const parsed = doc(PORTRAIT);
    for (const [pointer, value] of [
      ['/nope', 8],
      ['/count', Number.NaN],
      ['/count', Number.POSITIVE_INFINITY],
      ['/count', 8],
    ] as const) {
      const result = setNumber(parsed, pointer, value);
      expect(result.splice, `${pointer} ${String(value)}`).toBeNull();
      expect(result.source).toBe(PORTRAIT);
    }
  });

  it('re-parses correctly after a length-changing write, so the next edit still lands', () => {
    // The whole reason the session re-parses: `8` → `16` moves every byte after it.
    let parsed = doc(PORTRAIT);
    parsed = doc(setNumber(parsed, '/count', 16).source);
    const result = setNumber(parsed, '/inputs/Scheduler/height', 512);
    const splice = ok(result);
    expect(splice.removed).toBe('1024');
    assertOnlySpliceChanged(parsed.source, result.source, splice);
    expect(doc(result.source).count?.value).toBe(16);
    expect(doc(result.source).nodes.find((n) => n.title === 'Scheduler')?.numbers).toEqual([
      expect.objectContaining({ key: 'height', value: 512 }),
      expect.objectContaining({ key: 'width', value: 1024 }),
    ]);
  });
});

describe('writing a prompt', () => {
  it('escapes a backslash and a quote, and round-trips through the reader', () => {
    const parsed = doc(PORTRAIT);
    const written = 'A "weathered" lantern, lit at 45\\60 degrees, no text.';
    const result = setText(parsed, '/inputs/Positive Prompt/text', written);
    const splice = ok(result);
    expect(splice.inserted).toBe(
      '"A \\"weathered\\" lantern, lit at 45\\\\60 degrees, no text."',
    );
    assertOnlySpliceChanged(PORTRAIT, result.source, splice);
    // The escape survives a round trip: what the file holds decodes back to what was typed.
    expect(textAt(doc(result.source), '/inputs/Positive Prompt/text')?.value).toBe(written);
    // And the rest of the document is untouched — the prompt line is the only one that moved.
    expect(doc(result.source).seed?.value).toBe(300);
  });

  it('folds every line break to one space, because the strict subset has nowhere to put one', () => {
    expect(sanitizeText('one\ntwo')).toBe('one two');
    expect(sanitizeText('one\r\ntwo')).toBe('one two');
    expect(sanitizeText('one\n\n\ntwo')).toBe('one two');
    expect(sanitizeText('  trailing\n')).toBe('trailing');
    expect(sanitizeText('a\tb\vc\fd')).toBe('a b c d');
    expect(quoteText('one\ntwo')).toBe('"one two"');
    // A pasted multi-line prompt must not produce a block scalar (`L1006`) or a stray newline.
    const parsed = doc(PORTRAIT);
    const pasted = setText(parsed, '/inputs/Positive Prompt/text', 'line one\nline two\n');
    expect(ok(pasted).inserted).toBe('"line one line two"');
    expect(pasted.source.split('\n')).toHaveLength(PORTRAIT.split('\n').length);
  });

  it('normalizes a bare or single-quoted scalar into one double-quoted line', () => {
    const bare = doc(['inputs:', '  Sampler:', '    name: euler', ''].join('\n'));
    expect(ok(setText(bare, '/inputs/Sampler/name', 'dpmpp_2m')).inserted).toBe('"dpmpp_2m"');
    const single = doc(['inputs:', '  Sampler:', "    name: 'euler'", ''].join('\n'));
    const splice = ok(setText(single, '/inputs/Sampler/name', 'dpmpp_2m'));
    expect(splice.removed).toBe("'euler'");
    expect(splice.inserted).toBe('"dpmpp_2m"');
  });

  it('writes an emptied prompt, and refuses an unknown pointer', () => {
    const parsed = doc(PORTRAIT);
    expect(ok(setText(parsed, '/inputs/Positive Prompt/text', '')).inserted).toBe('""');
    expect(setText(parsed, '/inputs/No Such Node/text', 'x').splice).toBeNull();
    // Writing back exactly what is there is not an edit (and so costs no disk write).
    const current = textAt(parsed, '/inputs/Positive Prompt/text')?.value ?? '';
    expect(setText(parsed, '/inputs/Positive Prompt/text', current).splice).toBeNull();
  });
});

describe('splices', () => {
  it('invert exactly, so an edit and its undo are the same shape', () => {
    const parsed = doc(PORTRAIT);
    const result = setNumber(parsed, '/count', 4);
    const splice = ok(result);
    expect(spliceSource(result.source, invertSplice(splice))).toBe(PORTRAIT);
    expect(invertSplice(null)).toBeNull();
    expect(spliceSource(PORTRAIT, null)).toBe(PORTRAIT);
  });
});
