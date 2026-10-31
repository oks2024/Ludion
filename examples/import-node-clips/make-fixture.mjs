#!/usr/bin/env node
// Generates examples/import-node-clips/scene.gltf — a hand-made three-part hinge (base → lid →
// latch) with two animations on the nodes that become prefab parts (Clockout request 0016): `open`
// (LINEAR) and `snap` (STEP, keyed at 12 fps). The importer turns them into clips bound by the
// prefab's `clips:` map. Regenerate with:
//
//   node examples/import-node-clips/make-fixture.mjs
//
// Zero npm dependencies — Node built-ins only. Deterministic: no timestamps, no Math.random, stable
// key order, JSON.stringify(..., null, 2) plus one trailing newline, LF line endings.

import { writeFileAtomic } from '../../scripts/atomic-write.mjs';
import { fileURLToPath } from 'node:url';
import path from 'node:path';

const OUT_DIR = path.dirname(fileURLToPath(import.meta.url));
const OUT_FILE = path.join(OUT_DIR, 'scene.gltf');

// An axis-aligned box as 24 vertices (flat normals) and 36 indices.
function box([x0, y0, z0], [x1, y1, z1]) {
  const faces = [
    { n: [1, 0, 0], v: [[x1, y0, z0], [x1, y1, z0], [x1, y1, z1], [x1, y0, z1]] },
    { n: [-1, 0, 0], v: [[x0, y0, z1], [x0, y1, z1], [x0, y1, z0], [x0, y0, z0]] },
    { n: [0, 1, 0], v: [[x0, y1, z0], [x0, y1, z1], [x1, y1, z1], [x1, y1, z0]] },
    { n: [0, -1, 0], v: [[x0, y0, z1], [x0, y0, z0], [x1, y0, z0], [x1, y0, z1]] },
    { n: [0, 0, 1], v: [[x1, y0, z1], [x1, y1, z1], [x0, y1, z1], [x0, y0, z1]] },
    { n: [0, 0, -1], v: [[x0, y0, z0], [x0, y1, z0], [x1, y1, z0], [x1, y0, z0]] },
  ];
  const positions = [];
  const normals = [];
  const indices = [];
  faces.forEach((face, f) => {
    for (const v of face.v) {
      positions.push(...v);
      normals.push(...face.n);
    }
    const o = f * 4;
    indices.push(o, o + 1, o + 2, o, o + 2, o + 3);
  });
  return { positions, normals, indices, min: [x0, y0, z0], max: [x1, y1, z1] };
}

// The hinge: the lid's node origin sits on its back edge, so its vertices run forward from it.
const meshes = [
  { name: 'base', ...box([-0.6, 0, -0.4], [0.6, 0.1, 0.4]), color: [0.55, 0.5, 0.45, 1] },
  { name: 'lid', ...box([-0.6, 0, 0], [0.6, 0.05, 0.8]), color: [0.25, 0.45, 0.7, 1] },
  { name: 'latch', ...box([-0.05, 0, -0.05], [0.05, 0.1, 0.05]), color: [0.85, 0.7, 0.2, 1] },
];

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

const views = [];
const bufferViews = [];
const accessors = [];
let offset = 0;
function addView(data) {
  bufferViews.push({ buffer: 0, byteOffset: offset, byteLength: data.length });
  views.push(data);
  offset += data.length;
  return bufferViews.length - 1;
}
function addAccessor(accessor) {
  accessors.push(accessor);
  return accessors.length - 1;
}

const gltfMeshes = meshes.map((mesh) => {
  const position = addAccessor({
    bufferView: addView(f32Buffer(mesh.positions)),
    componentType: 5126,
    count: mesh.positions.length / 3,
    type: 'VEC3',
    min: mesh.min,
    max: mesh.max,
  });
  const normal = addAccessor({
    bufferView: addView(f32Buffer(mesh.normals)),
    componentType: 5126,
    count: mesh.normals.length / 3,
    type: 'VEC3',
  });
  const index = addAccessor({
    bufferView: addView(u16Buffer(mesh.indices)),
    componentType: 5123,
    count: mesh.indices.length,
    type: 'SCALAR',
  });
  return {
    name: mesh.name,
    primitives: [{ attributes: { POSITION: position, NORMAL: normal }, indices: index, material: meshes.indexOf(mesh) }],
  };
});

// Values rounded to seven decimals so the f32 output reads the same on every machine.
const round = (x) => Math.fround(Number(x.toFixed(7)));
const aboutX = (degrees) => {
  const half = (degrees * Math.PI) / 360;
  return [round(Math.sin(half)), 0, 0, round(Math.cos(half))];
};
const aboutZ = (degrees) => {
  const half = (degrees * Math.PI) / 360;
  return [0, 0, round(Math.sin(half)), round(Math.cos(half))];
};

const animations = [];
function animation(name, channels) {
  const samplers = [];
  const out = [];
  for (const channel of channels) {
    const input = addAccessor({
      bufferView: addView(f32Buffer(channel.times)),
      componentType: 5126,
      count: channel.times.length,
      type: 'SCALAR',
      min: [channel.times[0]],
      max: [channel.times[channel.times.length - 1]],
    });
    const output = addAccessor({
      bufferView: addView(f32Buffer(channel.values.flat())),
      componentType: 5126,
      count: channel.values.length,
      type: channel.path === 'rotation' ? 'VEC4' : 'VEC3',
    });
    samplers.push({ input, output, interpolation: channel.interpolation });
    out.push({ sampler: samplers.length - 1, target: { node: channel.node, path: channel.path } });
  }
  animations.push({ name, channels: out, samplers });
}

const BASE = 0;
const LID = 1;
const LATCH = 2;

// `open`: the lid swings up 100° about its hinge edge (no segment near a half turn) while the
// latch slides back; LINEAR.
animation('open', [
  {
    node: LID,
    path: 'rotation',
    interpolation: 'LINEAR',
    times: [0, 0.3, 0.6, 1],
    values: [0, -30, -70, -100].map(aboutX),
  },
  {
    node: LATCH,
    path: 'translation',
    interpolation: 'LINEAR',
    times: [0, 0.5, 1],
    values: [[0, 0.05, 0.8], [0, 0.05, 0.8], [0, 0.05, 0.7]],
  },
]);

// `snap`: the same motion keyed on twos at 12 fps and held between keys; STEP.
const snapTimes = [0, 1, 2, 3, 4, 5, 6].map((k) => round(k / 12));
animation('snap', [
  {
    node: LID,
    path: 'rotation',
    interpolation: 'STEP',
    times: snapTimes,
    values: [0, -15, -30, -50, -70, -85, -100].map(aboutX),
  },
  {
    node: LATCH,
    path: 'rotation',
    interpolation: 'STEP',
    times: snapTimes,
    values: [0, 20, 0, 20, 0, 20, 0].map(aboutZ),
  },
]);

const binBuffer = Buffer.concat(views);
const gltf = {
  asset: { version: '2.0', generator: 'ludion hand-made node-clips import fixture' },
  scene: 0,
  scenes: [{ name: 'hinge', nodes: [BASE] }],
  nodes: [
    { name: 'base', mesh: 0, children: [LID] },
    { name: 'lid', mesh: 1, translation: [0, 0.1, -0.4], children: [LATCH] },
    { name: 'latch', mesh: 2, translation: [0, 0.05, 0.8] },
  ],
  meshes: gltfMeshes,
  materials: meshes.map((mesh) => ({
    name: `${mesh.name}-material`,
    pbrMetallicRoughness: { baseColorFactor: mesh.color, metallicFactor: 0, roughnessFactor: 0.6 },
  })),
  animations,
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
