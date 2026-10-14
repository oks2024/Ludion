// A recursive schema-driven form tree, for editing a whole document rather than one component.
//
// WHY THIS IS NOT `viewer/inspect-fields.ts`. That module is deliberately ONE LEVEL DEEP: a
// component is a flat struct, and anything nested is a shape its widgets don't model, so it falls
// to a raw-JSON textarea. A `kind: pattern` document is the opposite — a map of bullet kinds, each
// with a list of motion segments, and a list of timeline entries — and a raw-JSON bucket over any
// of that is a text editor with extra steps. So this module walks the schema all the way down
// (`$ref` into `$defs`, `additionalProperties` maps, `items` lists, nullable wrappers) and hands
// only the genuine LEAVES to `inspect-fields.ts`, which keeps one implementation of number,
// boolean, string, vector and enum-dropdown rows for the whole editor.
//
// NOTHING HERE IS PATTERN-SPECIFIC. It takes a JSON Schema and a value; the pattern document is
// simply the first one. What a fresh list item or map entry holds comes from the schema's own
// `default`s, which schemars emits from the Rust `#[serde(default)]` — so the editor and
// `PatternDoc::default()` agree without a table to keep in step.
//
// A DEFAULT THAT IS NULL OR EMPTY IS OMITTED, not written. Every field this form builds is
// `#[serde(default)]` in Rust, so an absent key and an explicit `null` mean the same thing — and an
// empty collection would render in FLOW style, which the engine's content walk skips.
//
// Pure: no React, no Tauri, no DOM.

import { deriveFields, type FieldDescriptor, type FieldPath } from '../../viewer/inspect-fields';
import type { CommitCadence } from '../../viewer/useComponentCommit';
import { float } from './yamlDoc';

/** One edit a form row asks for: a whole-value write at a path, or a removal. */
export type DocEdit =
  | {
      readonly op: 'set';
      readonly path: FieldPath;
      readonly value: unknown;
      readonly cadence?: CommitCadence;
    }
  | { readonly op: 'remove'; readonly path: FieldPath };

/** What every node carries: where it writes, what it is called, and the schema's own prose. */
interface DocNodeBase {
  readonly path: FieldPath;
  readonly label: string;
  readonly help: string | null;
}

/** A leaf: one `inspect-fields` descriptor, rendered by the editor's one widget layer. */
export interface DocFieldNode extends DocNodeBase {
  readonly kind: 'field';
  readonly field: FieldDescriptor;
}

/** A nested struct: its properties, in schema order. */
export interface DocGroupNode extends DocNodeBase {
  readonly kind: 'group';
  readonly children: readonly DocNode[];
}

/** One entry of a {@link DocMapNode}. `value` is carried because a RENAME cannot be a single-path
 *  write: the surviving keys have to be re-emitted in order. */
export interface DocMapEntry {
  readonly key: string;
  readonly value: unknown;
  readonly node: DocNode;
}

/** An `additionalProperties` map: keyed entries a human adds, renames and removes. */
export interface DocMapNode extends DocNodeBase {
  readonly kind: 'map';
  readonly entries: readonly DocMapEntry[];
  /** What a freshly added entry holds. */
  readonly entryDefault: unknown;
}

/** An `items` list: ordered entries a human adds, reorders and removes. */
export interface DocListNode extends DocNodeBase {
  readonly kind: 'list';
  readonly items: readonly DocNode[];
  /** The raw items, aligned with {@link items} — a REORDER cannot be a single-path write. */
  readonly values: readonly unknown[];
  /** What a freshly appended item holds. */
  readonly itemDefault: unknown;
}

/** A nullable non-string field: a checkbox that writes the inner default or removes the key. */
export interface DocOptionalNode extends DocNodeBase {
  readonly kind: 'optional';
  readonly present: boolean;
  /** Rendered only while `present`. */
  readonly inner: DocNode;
  /** What turning it on writes. */
  readonly enabled: unknown;
}

export type DocNode = DocFieldNode | DocGroupNode | DocMapNode | DocListNode | DocOptionalNode;

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null && !Array.isArray(value);
}

/** Resolves a local `$ref` (`#/$defs/…`) against `root`, bounded like `inspect-fields.ts`'s own. */
function resolve(node: unknown, root: unknown, depth = 0): unknown {
  if (depth >= 16 || !isRecord(node)) {
    return node;
  }
  const ref = node.$ref;
  if (typeof ref !== 'string' || !ref.startsWith('#/')) {
    return node;
  }
  let cur: unknown = root;
  for (const raw of ref.slice(2).split('/')) {
    const part = raw.replace(/~1/g, '/').replace(/~0/g, '~');
    if (!isRecord(cur)) {
      return node;
    }
    cur = cur[part];
  }
  return cur === undefined ? node : resolve(cur, root, depth + 1);
}

/** The JSON Schema `type` keyword as a list, or `null`. */
function typeNames(schema: Record<string, unknown>): readonly string[] | null {
  if (typeof schema.type === 'string') {
    return [schema.type];
  }
  if (Array.isArray(schema.type)) {
    return schema.type.filter((t): t is string => typeof t === 'string');
  }
  return null;
}

/** Whether `schema` says "this or nothing", and if so what the "this" is — either `anyOf: [T, null]`
 *  (how schemars writes an `Option<Struct>`) or a `["number", "null"]` type list (how it writes an
 *  `Option<f32>`). `null` for anything not nullable. */
function nullableBranch(schema: Record<string, unknown>, root: unknown): unknown {
  if (Array.isArray(schema.anyOf)) {
    const branches = schema.anyOf.map((branch) => resolve(branch, root));
    const nonNull = branches.filter((branch) => !(isRecord(branch) && typeNames(branch)?.join() === 'null'));
    return branches.length === 2 && nonNull.length === 1 ? nonNull[0] : null;
  }
  const types = typeNames(schema);
  if (types === null || types.length !== 2 || !types.includes('null')) {
    return null;
  }
  // The `default: null` the outer node carries is dropped with the `null` type: the inner schema
  // must describe the value turning the field ON writes, which is its own type's default.
  const { default: _null, description: _description, ...rest } = schema;
  return { ...rest, type: types.filter((name) => name !== 'null')[0] };
}

/** A nullable schema's inner branch, or the schema itself. */
function unwrapNullable(schema: unknown, root: unknown): unknown {
  return isRecord(schema) ? (nullableBranch(schema, root) ?? schema) : schema;
}

/** Whether the schema calls this a float (`f32`/`f64`) rather than an integer. */
function isFloat(schema: Record<string, unknown>): boolean {
  return typeNames(schema)?.includes('number') === true;
}

/**
 * Re-types a value for the YAML writer: every number the schema calls a `number` becomes a
 * {@link float} mark, so `1` is written `1.0` and a doc edited here matches one written from Rust.
 */
export function markFloats(value: unknown, schema: unknown, root: unknown): unknown {
  const node = unwrapNullable(resolve(schema, root), root);
  if (!isRecord(node)) {
    return value;
  }
  if (typeof value === 'number') {
    return isFloat(node) ? float(value) : value;
  }
  if (Array.isArray(value)) {
    return value.map((item) => markFloats(item, node.items, root));
  }
  if (isRecord(value)) {
    const properties = isRecord(node.properties) ? node.properties : {};
    return Object.fromEntries(
      Object.entries(value).map(([key, inner]) => [
        key,
        markFloats(inner, properties[key] ?? node.additionalProperties, root),
      ]),
    );
  }
  return value;
}

/** The subschema a path addresses, or `undefined` when the path leaves the schema. */
export function subschemaAt(root: unknown, path: FieldPath): unknown {
  let node: unknown = resolve(root, root);
  for (const step of path) {
    node = unwrapNullable(node, root);
    if (!isRecord(node)) {
      return undefined;
    }
    if (typeof step === 'number') {
      node = resolve(node.items, root);
      continue;
    }
    const properties = isRecord(node.properties) ? node.properties : null;
    if (properties !== null && step in properties) {
      node = resolve(properties[step], root);
      continue;
    }
    if (isRecord(node.additionalProperties)) {
      node = resolve(node.additionalProperties, root);
      continue;
    }
    return undefined;
  }
  return node;
}

/** An empty list or map, which is written as an ABSENT key rather than flow-style `[]`/`{}`. */
function isEmptyCollection(value: unknown): boolean {
  if (Array.isArray(value)) {
    return value.length === 0;
  }
  return isRecord(value) && Object.keys(value).length === 0;
}

/** A schema's first `oneOf` constant — how schemars writes a unit enum's variants. */
function firstConst(schema: Record<string, unknown>): string | null {
  if (!Array.isArray(schema.oneOf)) {
    return null;
  }
  for (const branch of schema.oneOf) {
    if (isRecord(branch) && typeof branch.const === 'string') {
      return branch.const;
    }
  }
  return null;
}

/**
 * What a freshly created value of this schema holds, or `undefined` when it should simply be left
 * out (a `null` default, an empty map, an unbounded empty list).
 *
 * The schema's own `default` wins wherever schemars emitted one — which, for a Rust struct whose
 * every field is `#[serde(default)]`, reproduces `T::default()` field for field.
 */
export function defaultForSchema(schema: unknown, root: unknown): unknown {
  const node = resolve(schema, root);
  if (!isRecord(node)) {
    return undefined;
  }
  const declared = node.default;
  if (declared !== undefined && declared !== null && !isEmptyCollection(declared)) {
    return structuredClone(declared);
  }
  if (nullableBranch(node, root) !== null) {
    // A nullable field's default is ABSENCE. What turning one on writes is its inner schema's own
    // default, which the optional node asks for directly (see `deriveNode`).
    return undefined;
  }
  const constant = firstConst(node);
  if (constant !== null) {
    return constant;
  }
  switch (typeNames(node)?.filter((name) => name !== 'null')[0]) {
    case 'object': {
      if (!isRecord(node.properties)) {
        return undefined; // a map: an empty one is an absent key, never `{}`
      }
      const built: Record<string, unknown> = {};
      for (const [key, property] of Object.entries(node.properties)) {
        const value = defaultForSchema(property, root);
        if (value !== undefined) {
          built[key] = value;
        }
      }
      return Object.keys(built).length === 0 ? undefined : built;
    }
    case 'array': {
      const min = typeof node.minItems === 'number' ? node.minItems : 0;
      if (min === 0) {
        return undefined; // an empty list is an absent key, never `[]`
      }
      const item = defaultForSchema(node.items, root) ?? 0;
      return Array.from({ length: min }, () => structuredClone(item));
    }
    case 'string':
      return '';
    case 'integer':
    case 'number':
      return 0;
    case 'boolean':
      return false;
    default:
      return undefined;
  }
}

/** A schema `description` as one line of help: rustdoc intra-doc links flattened to their text, code
 *  ticks dropped, whitespace collapsed. */
export function helpText(description: unknown): string | null {
  if (typeof description !== 'string') {
    return null;
  }
  const flat = description
    .replace(/\[([^\]]+)\]\([^)]*\)/g, '$1')
    .replace(/\[([^\]\s]+)\]/g, '$1')
    .replace(/`/g, '')
    .replace(/\s+/g, ' ')
    .trim();
  return flat === '' ? null : flat;
}

/** Re-paths a derived descriptor onto its absolute location in the document. Exhaustive over the
 *  descriptor union, so a widget kind added there is a compile error here rather than a lost row. */
function repath(field: FieldDescriptor, path: FieldPath, label: string): FieldDescriptor {
  switch (field.kind) {
    case 'number':
      return { ...field, path, label };
    case 'boolean':
      return { ...field, path, label };
    case 'string':
      return { ...field, path, label };
    case 'vector':
      return { ...field, path, label };
    case 'variant':
      return { ...field, path, label };
    case 'json':
      return { ...field, path, label };
  }
}

/** One leaf, widgeted by `inspect-fields.ts`. The subschema is handed over wrapped in a synthetic
 *  one-property object so the derivation sees the field's real NAME (which is what decides whether a
 *  4-wide numeric array is a colour) and can still chase `$ref`s into the document's `$defs`. */
function leafNode(
  schema: unknown,
  root: unknown,
  path: FieldPath,
  label: string,
  help: string | null,
): DocFieldNode {
  const key = label === '' ? 'value' : label;
  const wrapper = {
    type: 'object',
    properties: { [key]: schema },
    $defs: isRecord(root) ? root.$defs : undefined,
  };
  const derived = deriveFields(wrapper, undefined)[0];
  return { kind: 'field', path, label, help, field: repath(derived, path, label) };
}

/** Whether an array is the fixed numeric vector `inspect-fields.ts` already widgets (a `[f32; N]`, a
 *  colour) rather than a variable-length list. */
function isFixedNumericArray(node: Record<string, unknown>, root: unknown): boolean {
  const min = node.minItems;
  const max = node.maxItems;
  if (typeof min !== 'number' || typeof max !== 'number' || min !== max || min < 1) {
    return false;
  }
  const items = resolve(node.items, root);
  const itemType = isRecord(items) ? typeNames(items)?.join() : null;
  return itemType === 'number' || itemType === 'integer';
}

/** Derives one node of the form tree. */
function deriveNode(
  schema: unknown,
  root: unknown,
  value: unknown,
  path: FieldPath,
  label: string,
): DocNode {
  const outerHelp = isRecord(schema) ? helpText(schema.description) : null;
  const node = resolve(schema, root);
  if (!isRecord(node)) {
    return leafNode(schema, root, path, label, outerHelp);
  }
  const help = outerHelp ?? helpText(node.description);

  // A nullable NON-string field: a checkbox plus, while it is on, whatever it wraps. (A nullable
  // string needs none of this — an empty box already commits as `null`, which is exactly what
  // `inspect-fields.ts`'s own `nullable` flag means.)
  const inner = nullableBranch(node, root);
  if (inner !== null && (!isRecord(inner) || typeNames(inner)?.join() !== 'string')) {
    return {
      kind: 'optional',
      path,
      label,
      help,
      present: value !== undefined && value !== null,
      inner: deriveNode(inner, root, value, path, label),
      enabled: defaultForSchema(inner, root) ?? 0,
    };
  }

  const type = typeNames(node)?.filter((name) => name !== 'null')[0];

  if (type === 'object' && isRecord(node.properties)) {
    const properties = node.properties;
    return {
      kind: 'group',
      path,
      label,
      help,
      children: Object.keys(properties).map((key) =>
        deriveNode(properties[key], root, isRecord(value) ? value[key] : undefined, [...path, key], key),
      ),
    };
  }

  if (type === 'object' && isRecord(node.additionalProperties)) {
    const entrySchema = node.additionalProperties;
    const record = isRecord(value) ? value : {};
    return {
      kind: 'map',
      path,
      label,
      help,
      entries: Object.keys(record).map((key) => ({
        key,
        value: record[key],
        node: deriveNode(entrySchema, root, record[key], [...path, key], key),
      })),
      entryDefault: defaultForSchema(entrySchema, root),
    };
  }

  if (type === 'array' && !isFixedNumericArray(node, root)) {
    const items = Array.isArray(value) ? value : [];
    return {
      kind: 'list',
      path,
      label,
      help,
      items: items.map((item, i) => deriveNode(node.items, root, item, [...path, i], `${i + 1}`)),
      values: items,
      itemDefault: defaultForSchema(node.items, root),
    };
  }

  return leafNode(node, root, path, label, help);
}

/**
 * The document form: one node per property the schema declares, in schema order.
 *
 * Keys the value carries but the schema does not are NOT surfaced — unlike a component's inspector,
 * where a stray key means an uneditable field. A document's stray keys are its envelope (`format:`,
 * `kind:`), which validation owns and a form must never offer to edit.
 */
export function deriveDocForm(schema: unknown, value: unknown): readonly DocNode[] {
  const root = resolve(schema, schema);
  if (!isRecord(root) || !isRecord(root.properties)) {
    return [];
  }
  const properties = root.properties;
  return Object.keys(properties).map((key) =>
    deriveNode(properties[key], schema, isRecord(value) ? value[key] : undefined, [key], key),
  );
}

/** Every leaf descriptor in a form tree, depth-first — what a coverage test walks. */
export function formLeaves(nodes: readonly DocNode[]): readonly FieldDescriptor[] {
  const leaves: FieldDescriptor[] = [];
  const walk = (node: DocNode): void => {
    switch (node.kind) {
      case 'field':
        leaves.push(node.field);
        return;
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
    }
  };
  nodes.forEach(walk);
  return leaves;
}
