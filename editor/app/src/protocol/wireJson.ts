// Lossless JSON for component values on the wire. Components travel as `serde_json::Value`, so any
// u64 (a seed, a bitset word) can exceed 2^53, which `JSON.parse` rounds to a double. Here an
// integer literal outside the safe range parses to a `bigint`; every other value is what
// `JSON.parse` gives. Generated protocol types still say `number` for these fields: a `bigint`
// appears only where precision would otherwise be lost. `JSON.stringify` throws on a bigint, so
// every site that writes a component value back uses `stringifyWire`.

const LONG_DIGITS = /\d{16,}/;

class Cursor {
  pos = 0;
  readonly text: string;
  constructor(text: string) {
    this.text = text;
  }

  fail(what: string): never {
    throw new SyntaxError(`${what} at position ${this.pos} in JSON`);
  }

  skipSpace(): void {
    const t = this.text;
    while (this.pos < t.length) {
      const c = t.charCodeAt(this.pos);
      if (c === 0x20 || c === 0x0a || c === 0x0d || c === 0x09) this.pos++;
      else break;
    }
  }
}

function parseValue(cur: Cursor): unknown {
  cur.skipSpace();
  const t = cur.text;
  const c = t[cur.pos];
  if (c === '{') return parseObject(cur);
  if (c === '[') return parseArray(cur);
  if (c === '"') return JSON.parse(readString(cur)) as string;
  if (t.startsWith('true', cur.pos)) {
    cur.pos += 4;
    return true;
  }
  if (t.startsWith('false', cur.pos)) {
    cur.pos += 5;
    return false;
  }
  if (t.startsWith('null', cur.pos)) {
    cur.pos += 4;
    return null;
  }
  return parseNumber(cur);
}

function readString(cur: Cursor): string {
  const t = cur.text;
  const start = cur.pos;
  cur.pos++;
  while (cur.pos < t.length) {
    const c = t[cur.pos];
    if (c === '\\') cur.pos += 2;
    else if (c === '"') {
      cur.pos++;
      return t.slice(start, cur.pos);
    } else cur.pos++;
  }
  return cur.fail('unterminated string');
}

const NUMBER = /-?(?:0|[1-9]\d*)(\.\d+)?([eE][+-]?\d+)?/y;

function parseNumber(cur: Cursor): number | bigint {
  NUMBER.lastIndex = cur.pos;
  const m = NUMBER.exec(cur.text);
  if (m === null) return cur.fail('unexpected token');
  cur.pos += m[0].length;
  const asNumber = Number(m[0]);
  if (m[1] === undefined && m[2] === undefined && !Number.isSafeInteger(asNumber)) {
    return BigInt(m[0]);
  }
  return asNumber;
}

function parseArray(cur: Cursor): unknown[] {
  cur.pos++;
  const out: unknown[] = [];
  cur.skipSpace();
  if (cur.text[cur.pos] === ']') {
    cur.pos++;
    return out;
  }
  for (;;) {
    out.push(parseValue(cur));
    cur.skipSpace();
    const c = cur.text[cur.pos++];
    if (c === ']') return out;
    if (c !== ',') cur.pos--, cur.fail("expected ',' or ']'");
  }
}

function parseObject(cur: Cursor): Record<string, unknown> {
  cur.pos++;
  const out: Record<string, unknown> = {};
  cur.skipSpace();
  if (cur.text[cur.pos] === '}') {
    cur.pos++;
    return out;
  }
  for (;;) {
    cur.skipSpace();
    if (cur.text[cur.pos] !== '"') cur.fail('expected a string key');
    const key = JSON.parse(readString(cur)) as string;
    cur.skipSpace();
    if (cur.text[cur.pos++] !== ':') cur.pos--, cur.fail("expected ':'");
    Object.defineProperty(out, key, {
      value: parseValue(cur),
      enumerable: true,
      writable: true,
      configurable: true,
    });
    cur.skipSpace();
    const c = cur.text[cur.pos++];
    if (c === '}') return out;
    if (c !== ',') cur.pos--, cur.fail("expected ',' or '}'");
  }
}

/** `JSON.parse`, except an integer literal outside the safe range becomes a `bigint`. */
export function parseWire(text: string): unknown {
  if (!LONG_DIGITS.test(text)) return JSON.parse(text);
  const cur = new Cursor(text);
  const value = parseValue(cur);
  cur.skipSpace();
  if (cur.pos < text.length) cur.fail('unexpected trailing content');
  return value;
}

/** `JSON.stringify(value, null, indent)`, except a `bigint` is written as its exact digits. */
export function stringifyWire(value: unknown, indent?: number): string {
  return write(value, indent ?? 0, '') ?? 'undefined';
}

function write(value: unknown, indent: number, pad: string): string | undefined {
  if (typeof value === 'bigint') return value.toString();
  if (value === null || typeof value !== 'object') {
    return typeof value === 'function' || typeof value === 'symbol' ? undefined : JSON.stringify(value);
  }
  const toJSON = (value as { toJSON?: () => unknown }).toJSON;
  if (typeof toJSON === 'function') return write(toJSON.call(value), indent, pad);
  const inner = indent > 0 ? `${pad}${' '.repeat(Math.min(indent, 10))}` : '';
  const nl = indent > 0 ? '\n' : '';
  const colon = indent > 0 ? ': ' : ':';
  if (Array.isArray(value)) {
    if (value.length === 0) return '[]';
    const items = value.map((v) => write(v, indent, inner) ?? 'null');
    return `[${nl}${items.map((s) => inner + s).join(`,${nl}`)}${nl}${indent > 0 ? pad : ''}]`;
  }
  const members: string[] = [];
  for (const [k, v] of Object.entries(value)) {
    const s = write(v, indent, inner);
    if (s !== undefined) members.push(`${inner}${JSON.stringify(k)}${colon}${s}`);
  }
  if (members.length === 0) return '{}';
  return `{${nl}${members.join(`,${nl}`)}${nl}${indent > 0 ? pad : ''}}`;
}
