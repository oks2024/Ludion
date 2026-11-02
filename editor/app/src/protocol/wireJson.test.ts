import { describe, expect, it } from 'vitest';
import { parseWire, stringifyWire } from './wireJson';

describe('wireJson', () => {
  it.each([
    '18446744073709551615',
    '18446744073709551614',
    '-9223372036854775808',
    '9007199254740993',
  ])('round-trips %s digit-exact', (digits) => {
    const v = parseWire(`{"a":[${digits}],"b":{"c":${digits}}}`) as any;
    expect(typeof v.a[0]).toBe('bigint');
    expect(stringifyWire(v)).toBe(`{"a":[${digits}],"b":{"c":${digits}}}`);
  });

  it('keeps safe integers and floats as numbers', () => {
    const v = parseWire('[9007199254740991,0.30000000000000004,1e300,-0,12345678901234567890.5]') as any[];
    expect(v.map((x) => typeof x)).toEqual(['number', 'number', 'number', 'number', 'number']);
    expect(v[1]).toBe(0.30000000000000004);
    expect(v[2]).toBe(1e300);
  });

  it('keeps escapes and unicode in strings', () => {
    const text = '{"k\\u00e9y":"a\\"b\\\\c\\n\\ud83d\\ude00 é 123456789012345678901","n":18446744073709551615}';
    const v = parseWire(text) as any;
    expect(v['kéy']).toBe('a"b\\c\n😀 é 123456789012345678901');
    expect(v.n).toBe(18446744073709551615n);
  });

  it('matches JSON for bigint-free documents, indented or not', () => {
    const doc = { a: [1, 2.5, 'x', null, true, [], {}], b: { c: { d: [{ e: 'é' }] } }, u: undefined, f: () => 1 };
    for (const indent of [undefined, 2, 4]) {
      expect(stringifyWire(doc, indent)).toBe(JSON.stringify(doc, null, indent));
    }
    const text = '{"a":[1,2,{"b":[]}],"c":"d","big":1234567890123456789012}';
    const parsedSafe = '{"a":[1,2,{"b":[]}],"c":"d","big":1.2345678901234568e+21}';
    expect(parseWire(parsedSafe)).toEqual(JSON.parse(parsedSafe));
    expect(stringifyWire(parseWire(text), 2)).toBe(
      '{\n  "a": [\n    1,\n    2,\n    {\n      "b": []\n    }\n  ],\n  "c": "d",\n  "big": 1234567890123456789012\n}',
    );
  });

  it('indents bigints like numbers', () => {
    expect(stringifyWire({ a: [18446744073709551615n] }, 2)).toBe('{\n  "a": [\n    18446744073709551615\n  ]\n}');
  });

  it('throws SyntaxError on malformed input', () => {
    expect(() => parseWire('{"a":18446744073709551615,}')).toThrow(SyntaxError);
    expect(() => parseWire('[18446744073709551615')).toThrow(SyntaxError);
    expect(() => parseWire('[18446744073709551615] x')).toThrow(SyntaxError);
  });
});
