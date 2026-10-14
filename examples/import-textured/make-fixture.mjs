#!/usr/bin/env node
// Generates examples/import-textured/scene.gltf — a hand-made, two-cube glTF fixture that carries
// all five metallic-roughness texture slots on both cubes, one cube with an authored TANGENT
// accessor and the other without (Plan/textured-pbr.md, G7 fixtures). Regenerate with:
//
//   node examples/import-textured/make-fixture.mjs
//
// Zero npm dependencies — Node built-ins only (fs, zlib) — same convention as scripts/size-gate.mjs.
// Deterministic: no timestamps, no Math.random, stable object key order (object literals below are
// written in the exact order they serialize), JSON.stringify(..., null, 2) plus one trailing
// newline, LF line endings throughout. Running this script twice must produce byte-identical output.

import { deflateSync } from 'node:zlib';
import { writeFileAtomic } from '../../scripts/atomic-write.mjs';
import { fileURLToPath } from 'node:url';
import path from 'node:path';

const OUT_DIR = path.dirname(fileURLToPath(import.meta.url));
const OUT_FILE = path.join(OUT_DIR, 'scene.gltf');

// ── PNG encoder (IHDR / IDAT / IEND, zlib via deflateSync, CRC32 below) ─────────────────────────

const CRC_TABLE = (() => {
  const table = new Uint32Array(256);
  for (let n = 0; n < 256; n++) {
    let c = n;
    for (let k = 0; k < 8; k++) {
      c = c & 1 ? 0xedb88320 ^ (c >>> 1) : c >>> 1;
    }
    table[n] = c >>> 0;
  }
  return table;
})();

function crc32(buf) {
  let c = 0xffffffff;
  for (let i = 0; i < buf.length; i++) {
    c = CRC_TABLE[(c ^ buf[i]) & 0xff] ^ (c >>> 8);
  }
  return (c ^ 0xffffffff) >>> 0;
}

function u32be(n) {
  const b = Buffer.alloc(4);
  b.writeUInt32BE(n >>> 0, 0);
  return b;
}

function pngChunk(type, data) {
  const typeBuf = Buffer.from(type, 'ascii');
  const body = Buffer.concat([typeBuf, data]);
  return Buffer.concat([u32be(data.length), body, u32be(crc32(body))]);
}

const PNG_SIGNATURE = Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]);

// `pixels` is a flat RGBA8 buffer, row-major, top row first, length width*height*4.
function encodePng(width, height, pixels) {
  const stride = width * 4;
  const raw = Buffer.alloc(height * (1 + stride));
  for (let y = 0; y < height; y++) {
    const rowStart = y * (1 + stride);
    raw[rowStart] = 0; // filter type 0 (None) on every scanline
    pixels.copy(raw, rowStart + 1, y * stride, y * stride + stride);
  }
  const ihdr = Buffer.concat([
    u32be(width),
    u32be(height),
    Buffer.from([8, 6, 0, 0, 0]), // bit depth 8, color type 6 (RGBA), compression/filter/interlace 0
  ]);
  const idat = deflateSync(raw, { level: 9 });
  return Buffer.concat([
    PNG_SIGNATURE,
    pngChunk('IHDR', ihdr),
    pngChunk('IDAT', idat),
    pngChunk('IEND', Buffer.alloc(0)),
  ]);
}

function rgba(width, height, fn) {
  const buf = Buffer.alloc(width * height * 4);
  for (let y = 0; y < height; y++) {
    for (let x = 0; x < width; x++) {
      const [r, g, b, a] = fn(x, y);
      const i = (y * width + x) * 4;
      buf[i] = r;
      buf[i + 1] = g;
      buf[i + 2] = b;
      buf[i + 3] = a;
    }
  }
  return buf;
}

const IMG_SIZE = 16;

// image 0 — base color: a 4x4-cell checker of two saturated colours.
const BASE_A = [230, 120, 40];
const BASE_B = [40, 140, 160];
const pngBase = encodePng(
  IMG_SIZE,
  IMG_SIZE,
  rgba(IMG_SIZE, IMG_SIZE, (x, y) => {
    const cell = (Math.floor(x / 4) + Math.floor(y / 4)) % 2;
    const [r, g, b] = cell === 0 ? BASE_A : BASE_B;
    return [r, g, b, 255];
  })
);

// image 1 — metallic-roughness: R unused, G (roughness) a horizontal gradient, B (metallic) a
// top/bottom split.
const pngMr = encodePng(
  IMG_SIZE,
  IMG_SIZE,
  rgba(IMG_SIZE, IMG_SIZE, (x, y) => {
    const g = Math.round(30 + ((230 - 30) * x) / (IMG_SIZE - 1));
    const b = y < IMG_SIZE / 2 ? 255 : 0;
    return [0, g, b, 255];
  })
);

// image 2 — tangent-space normal map: four diagonal ridges, one every 4 columns, each a valid unit
// normal tilted +/-0.45 in tangent x and, alternating every 4 rows, +/-0.45 in tangent y.
const pngNormal = encodePng(
  IMG_SIZE,
  IMG_SIZE,
  rgba(IMG_SIZE, IMG_SIZE, (x, y) => {
    const stripe = Math.floor(x / 4) % 2;
    const nx = stripe === 0 ? 0.45 : -0.45;
    const ny = Math.floor(y / 4) % 2 === 0 ? 0.45 : -0.45;
    const nz = Math.sqrt(1 - nx * nx - ny * ny);
    const r = Math.round((nx * 0.5 + 0.5) * 255);
    const g = Math.round((ny * 0.5 + 0.5) * 255);
    const b = Math.round((nz * 0.5 + 0.5) * 255);
    return [r, g, b, 255];
  })
);

// image 3 — occlusion: a radial darkening from the centre (255) to the corners (~60).
const pngOcclusion = encodePng(
  IMG_SIZE,
  IMG_SIZE,
  rgba(IMG_SIZE, IMG_SIZE, (x, y) => {
    const cx = (IMG_SIZE - 1) / 2;
    const cy = (IMG_SIZE - 1) / 2;
    const maxDist = Math.sqrt(cx * cx + cy * cy);
    const dist = Math.sqrt((x - cx) * (x - cx) + (y - cy) * (y - cy));
    const v = Math.round(255 - (255 - 60) * (dist / maxDist));
    return [v, v, v, 255];
  })
);

// image 4 — emissive: black except a 4x4 bright cyan square in one corner.
const pngEmissive = encodePng(
  IMG_SIZE,
  IMG_SIZE,
  rgba(IMG_SIZE, IMG_SIZE, (x, y) => {
    if (x < 4 && y < 4) return [0, 255, 255, 255];
    return [0, 0, 0, 255];
  })
);

// image 5 — pixel-identical duplicate of image 0 under a different name, to exercise content-hash
// dedup of identical bytes with different names.
const pngBaseDup = pngBase;

function dataUri(buf) {
  return `data:image/png;base64,${buf.toString('base64')}`;
}

// ── cube geometry (24 vertices, 36 indices, per-face normals/UVs/tangents) ─────────────────────

// Each face is (normal, right, up) with normal == right x up, so `right` is the face's +U
// (tangent) direction and (right, up, normal) form a right-handed basis — the winding order below
// is then counter-clockwise as seen from the normal side. glTF's rule: the bitangent
// `cross(normal, tangent) * w` points image-up, toward decreasing v. Here `+v` runs along `up` and
// `cross(normal, right) = up`, so w = -1 gives bitangent = -up = image-up.
const FACES = [
  { normal: [1, 0, 0], right: [0, 0, -1], up: [0, 1, 0] }, // +X
  { normal: [-1, 0, 0], right: [0, 0, 1], up: [0, 1, 0] }, // -X
  { normal: [0, 1, 0], right: [1, 0, 0], up: [0, 0, -1] }, // +Y
  { normal: [0, -1, 0], right: [1, 0, 0], up: [0, 0, 1] }, // -Y
  { normal: [0, 0, 1], right: [1, 0, 0], up: [0, 1, 0] }, // +Z
  { normal: [0, 0, -1], right: [-1, 0, 0], up: [0, 1, 0] }, // -Z
];

// Corners in (u, v) order 0,0 / 1,0 / 1,1 / 0,1 — CCW around the face as seen from `normal`.
const CORNERS = [
  { su: -0.5, sv: -0.5, u: 0, v: 0 },
  { su: 0.5, sv: -0.5, u: 1, v: 0 },
  { su: 0.5, sv: 0.5, u: 1, v: 1 },
  { su: -0.5, sv: 0.5, u: 0, v: 1 },
];

const positions = [];
const normals = [];
const uvs = [];
const tangents = [];
const indices = [];

for (const face of FACES) {
  const base = positions.length / 3;
  for (const c of CORNERS) {
    const pos = [0, 1, 2].map(
      (i) => face.normal[i] * 0.5 + face.right[i] * c.su + face.up[i] * c.sv
    );
    positions.push(...pos);
    normals.push(...face.normal);
    uvs.push(c.u, c.v);
    tangents.push(...face.right, -1);
  }
  indices.push(base, base + 1, base + 2, base, base + 2, base + 3);
}

const VERTEX_COUNT = positions.length / 3;
const INDEX_COUNT = indices.length;
if (VERTEX_COUNT !== 24) throw new Error(`expected 24 vertices, got ${VERTEX_COUNT}`);
if (INDEX_COUNT !== 36) throw new Error(`expected 36 indices, got ${INDEX_COUNT}`);

function round7(x) {
  return Math.round(x * 1e7) / 1e7;
}

function minMax(values, components) {
  const min = new Array(components).fill(Infinity);
  const max = new Array(components).fill(-Infinity);
  for (let i = 0; i < values.length; i += components) {
    for (let c = 0; c < components; c++) {
      min[c] = Math.min(min[c], values[i + c]);
      max[c] = Math.max(max[c], values[i + c]);
    }
  }
  return { min: min.map(round7), max: max.map(round7) };
}

// ── binary buffer (POSITION, NORMAL, TEXCOORD_0, TANGENT, indices) ─────────────────────────────

function f32Buffer(values) {
  const buf = Buffer.alloc(values.length * 4);
  for (let i = 0; i < values.length; i++) buf.writeFloatLE(values[i], i * 4);
  return buf;
}

function u16Buffer(values) {
  const buf = Buffer.alloc(values.length * 2);
  for (let i = 0; i < values.length; i++) buf.writeUInt16LE(values[i], i * 2);
  return buf;
}

const posBuf = f32Buffer(positions);
const normBuf = f32Buffer(normals);
const uvBuf = f32Buffer(uvs);
const tanBuf = f32Buffer(tangents);
const idxBuf = u16Buffer(indices);

const bufferViewSpecs = [
  { name: 'position', data: posBuf },
  { name: 'normal', data: normBuf },
  { name: 'uv', data: uvBuf },
  { name: 'tangent', data: tanBuf },
  { name: 'indices', data: idxBuf },
];

let offset = 0;
const bufferViewOffsets = {};
for (const spec of bufferViewSpecs) {
  bufferViewOffsets[spec.name] = offset;
  offset += spec.data.length;
}
const binBuffer = Buffer.concat(
  bufferViewSpecs.map((s) => s.data),
  offset
);

const bufferViews = bufferViewSpecs.map((spec) => ({
  buffer: 0,
  byteOffset: bufferViewOffsets[spec.name],
  byteLength: spec.data.length,
}));

const BV = { position: 0, normal: 1, uv: 2, tangent: 3, indices: 4 };

const posMinMax = minMax(positions, 3);

const accessors = [
  {
    bufferView: BV.position,
    componentType: 5126,
    count: VERTEX_COUNT,
    type: 'VEC3',
    min: posMinMax.min,
    max: posMinMax.max,
  },
  {
    bufferView: BV.normal,
    componentType: 5126,
    count: VERTEX_COUNT,
    type: 'VEC3',
    ...minMax(normals, 3),
  },
  {
    bufferView: BV.uv,
    componentType: 5126,
    count: VERTEX_COUNT,
    type: 'VEC2',
    ...minMax(uvs, 2),
  },
  {
    bufferView: BV.tangent,
    componentType: 5126,
    count: VERTEX_COUNT,
    type: 'VEC4',
    ...minMax(tangents, 4),
  },
  {
    bufferView: BV.indices,
    componentType: 5123,
    count: INDEX_COUNT,
    type: 'SCALAR',
  },
];

const ACC = { position: 0, normal: 1, uv: 2, tangent: 3, indices: 4 };

// ── structural self-check ───────────────────────────────────────────────────────────────────────

for (const [i, bv] of bufferViews.entries()) {
  if (bv.byteOffset + bv.byteLength > binBuffer.length) {
    throw new Error(`bufferView ${i} exceeds buffer length`);
  }
}
if (accessors[ACC.position].min.length !== 3 || accessors[ACC.position].max.length !== 3) {
  throw new Error('POSITION accessor missing min/max');
}

// ── document assembly ────────────────────────────────────────────────────────────────────────────

const gltf = {
  asset: {
    version: '2.0',
    generator: 'ludion hand-made textured import fixture',
  },
  extensionsUsed: ['KHR_lights_punctual'],
  extensions: {
    KHR_lights_punctual: {
      lights: [
        {
          type: 'point',
          name: 'key',
          color: [1, 1, 1],
          intensity: 8,
          range: 20,
        },
      ],
    },
  },
  scene: 0,
  scenes: [
    {
      name: 'fixture',
      nodes: [0, 1, 2],
    },
  ],
  nodes: [
    {
      name: 'tangent-cube',
      mesh: 0,
      translation: [-0.9, 0.6, 0],
      rotation: [0, 0.258819, 0, 0.9659258],
    },
    {
      name: 'computed-cube',
      mesh: 1,
      translation: [0.9, 0.6, 0],
      rotation: [0, -0.258819, 0, 0.9659258],
    },
    {
      name: 'key-light',
      translation: [2, 3, 3],
      extensions: {
        KHR_lights_punctual: {
          light: 0,
        },
      },
    },
  ],
  meshes: [
    {
      name: 'tangent-cube',
      primitives: [
        {
          attributes: {
            POSITION: ACC.position,
            NORMAL: ACC.normal,
            TEXCOORD_0: ACC.uv,
            TANGENT: ACC.tangent,
          },
          indices: ACC.indices,
          material: 0,
        },
      ],
    },
    {
      name: 'computed-cube',
      primitives: [
        {
          attributes: {
            POSITION: ACC.position,
            NORMAL: ACC.normal,
            TEXCOORD_0: ACC.uv,
          },
          indices: ACC.indices,
          material: 1,
        },
      ],
    },
  ],
  materials: [
    {
      name: 'tangent-cube-material',
      pbrMetallicRoughness: {
        baseColorFactor: [1, 1, 1, 1],
        baseColorTexture: { index: 0 },
        metallicFactor: 1,
        roughnessFactor: 1,
        metallicRoughnessTexture: { index: 1 },
      },
      normalTexture: { index: 2, scale: 1 },
      occlusionTexture: { index: 3, strength: 1 },
      emissiveTexture: { index: 4 },
      emissiveFactor: [1, 1, 1],
    },
    {
      name: 'computed-cube-material',
      pbrMetallicRoughness: {
        baseColorFactor: [1, 1, 1, 1],
        baseColorTexture: { index: 5 },
        metallicFactor: 1,
        roughnessFactor: 1,
        metallicRoughnessTexture: { index: 1 },
      },
      normalTexture: { index: 2, scale: 1 },
      occlusionTexture: { index: 3, strength: 1 },
      emissiveTexture: { index: 4 },
      emissiveFactor: [1, 1, 1],
    },
  ],
  textures: [
    { source: 0, sampler: 0 },
    { source: 1, sampler: 0 },
    { source: 2, sampler: 0 },
    { source: 3, sampler: 0 },
    { source: 4, sampler: 0 },
    { source: 5, sampler: 0 },
  ],
  samplers: [
    {
      magFilter: 9729,
      minFilter: 9987,
      wrapS: 10497,
      wrapT: 10497,
    },
  ],
  images: [
    { name: 'base', mimeType: 'image/png', uri: dataUri(pngBase) },
    { name: 'mr', mimeType: 'image/png', uri: dataUri(pngMr) },
    { name: 'normal', mimeType: 'image/png', uri: dataUri(pngNormal) },
    { name: 'occlusion', mimeType: 'image/png', uri: dataUri(pngOcclusion) },
    { name: 'emissive', mimeType: 'image/png', uri: dataUri(pngEmissive) },
    { name: 'base-dup', mimeType: 'image/png', uri: dataUri(pngBaseDup) },
  ],
  accessors,
  bufferViews,
  buffers: [
    {
      byteLength: binBuffer.length,
      uri: `data:application/octet-stream;base64,${binBuffer.toString('base64')}`,
    },
  ],
};

const json = `${JSON.stringify(gltf, null, 2)}\n`;
writeFileAtomic(OUT_FILE, json, 'utf8');
console.log(`wrote ${OUT_FILE} (${Buffer.byteLength(json, 'utf8')} bytes)`);
