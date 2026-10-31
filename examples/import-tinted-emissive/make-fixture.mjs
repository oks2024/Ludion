#!/usr/bin/env node
// Generates examples/import-tinted-emissive/scene.gltf — a hand-made, three-quad glTF fixture whose
// materials carry an emissive tint that is not proportional to the base colour (Clockout request
// 0014): the importer bakes the tint into an emissive map. Regenerate with:
//
//   node examples/import-tinted-emissive/make-fixture.mjs
//
// Zero npm dependencies — Node built-ins only. Deterministic: no timestamps, no Math.random, stable
// key order, JSON.stringify(..., null, 2) plus one trailing newline, LF line endings.

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

// image 0 — base colour: a 4x4-cell checker of two muted colours.
const pngBase = encodePng(
  IMG_SIZE,
  IMG_SIZE,
  rgba(IMG_SIZE, IMG_SIZE, (x, y) => {
    const cell = (Math.floor(x / 4) + Math.floor(y / 4)) % 2;
    return cell === 0 ? [200, 190, 170, 255] : [90, 100, 120, 255];
  })
);

// image 1 — emissive mask: a horizontal grey ramp, dark to bright, with a dark border.
const pngEmissive = encodePng(
  IMG_SIZE,
  IMG_SIZE,
  rgba(IMG_SIZE, IMG_SIZE, (x, y) => {
    const border = x === 0 || y === 0 || x === IMG_SIZE - 1 || y === IMG_SIZE - 1;
    const v = border ? 0 : Math.round(40 + (215 * x) / (IMG_SIZE - 1));
    return [v, v, v, 255];
  })
);

function dataUri(buf) {
  return `data:image/png;base64,${buf.toString('base64')}`;
}

// ── one unit quad facing +Z (4 vertices, 6 indices), shared by the three meshes ─────────────────

const positions = [-0.5, -0.5, 0, 0.5, -0.5, 0, 0.5, 0.5, 0, -0.5, 0.5, 0];
const normals = [0, 0, 1, 0, 0, 1, 0, 0, 1, 0, 0, 1];
const uvs = [0, 1, 1, 1, 1, 0, 0, 0];
const indices = [0, 1, 2, 0, 2, 3];

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

const views = [f32Buffer(positions), f32Buffer(normals), f32Buffer(uvs), u16Buffer(indices)];
const bufferViews = [];
let offset = 0;
for (const data of views) {
  bufferViews.push({ buffer: 0, byteOffset: offset, byteLength: data.length });
  offset += data.length;
}
const binBuffer = Buffer.concat(views);

const accessors = [
  { bufferView: 0, componentType: 5126, count: 4, type: 'VEC3', min: [-0.5, -0.5, 0], max: [0.5, 0.5, 0] },
  { bufferView: 1, componentType: 5126, count: 4, type: 'VEC3', min: [0, 0, 1], max: [0, 0, 1] },
  { bufferView: 2, componentType: 5126, count: 4, type: 'VEC2', min: [0, 0], max: [1, 1] },
  { bufferView: 3, componentType: 5123, count: 6, type: 'SCALAR' },
];

const quad = (name, material) => ({
  name,
  primitives: [
    { attributes: { POSITION: 0, NORMAL: 1, TEXCOORD_0: 2 }, indices: 3, material },
  ],
});

// Every factor is exactly representable in f32, so the imported `emissive:` values read clean.
const gltf = {
  asset: { version: '2.0', generator: 'ludion hand-made tinted-emissive import fixture' },
  extensionsUsed: ['KHR_materials_emissive_strength'],
  scene: 0,
  scenes: [{ name: 'fixture', nodes: [0, 1, 2] }],
  nodes: [
    { name: 'card-textured', mesh: 0, translation: [-1.2, 0.6, 0] },
    { name: 'card-constant', mesh: 1, translation: [0, 0.6, 0] },
    { name: 'card-proportional', mesh: 2, translation: [1.2, 0.6, 0] },
  ],
  meshes: [
    quad('card-textured', 0),
    quad('card-constant', 1),
    quad('card-proportional', 2),
  ],
  materials: [
    {
      // White base, an emissive texture: the baked map is the ramp times the hue, intensity 3.
      name: 'card-textured-material',
      pbrMetallicRoughness: {
        baseColorFactor: [1, 1, 1, 1],
        baseColorTexture: { index: 0 },
        metallicFactor: 0,
        roughnessFactor: 0.75,
      },
      emissiveTexture: { index: 1 },
      emissiveFactor: [1, 0.5, 0.25],
      extensions: { KHR_materials_emissive_strength: { emissiveStrength: 3 } },
    },
    {
      // No emissive texture, a tinted base: a 1x1 map, intensity from dividing by the base.
      name: 'card-constant-material',
      pbrMetallicRoughness: {
        baseColorFactor: [0.5, 0.5, 1, 1],
        baseColorTexture: { index: 0 },
        metallicFactor: 0,
        roughnessFactor: 0.75,
      },
      emissiveFactor: [0.25, 0.5, 2],
    },
    {
      // Control: emissive is exactly twice the base, the scalar path with no baked map.
      name: 'card-proportional-material',
      pbrMetallicRoughness: {
        baseColorFactor: [0.5, 0.25, 1, 1],
        baseColorTexture: { index: 0 },
        metallicFactor: 0,
        roughnessFactor: 0.75,
      },
      emissiveFactor: [1, 0.5, 2],
    },
  ],
  textures: [
    { source: 0, sampler: 0 },
    { source: 1, sampler: 0 },
  ],
  samplers: [{ magFilter: 9729, minFilter: 9987, wrapS: 10497, wrapT: 10497 }],
  images: [
    { name: 'base', mimeType: 'image/png', uri: dataUri(pngBase) },
    { name: 'emissive-ramp', mimeType: 'image/png', uri: dataUri(pngEmissive) },
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

// A refused case, not imported into expected/: the base colour has no green to carry the emissive's.
const refused = {
  asset: gltf.asset,
  scene: 0,
  scenes: [{ name: 'refused', nodes: [0] }],
  nodes: [{ name: 'card-refused', mesh: 0 }],
  meshes: [quad('card-refused', 0)],
  materials: [
    {
      name: 'refused-zero-base-material',
      pbrMetallicRoughness: { baseColorFactor: [1, 0, 1, 1], metallicFactor: 0, roughnessFactor: 0.75 },
      emissiveFactor: [0.5, 0.5, 0],
    },
  ],
  accessors,
  bufferViews,
  buffers: gltf.buffers,
};
const refusedJson = `${JSON.stringify(refused, null, 2)}\n`;
const REFUSED_FILE = path.join(OUT_DIR, 'refused-zero-base.gltf');
writeFileAtomic(REFUSED_FILE, refusedJson, 'utf8');
console.log(`wrote ${REFUSED_FILE} (${Buffer.byteLength(refusedJson, 'utf8')} bytes)`);
