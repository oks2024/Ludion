#!/usr/bin/env node
// Regenerates the audio-zones gallery sounds: rain.wav, hum.wav and chatter.wav, mono 16-bit
// 22050 Hz, one second or less. Run: `node examples/gallery/sounds/synth.mjs`.
// The output is a pure function of the seeds below, so a second run is byte-identical. Never
// hand-edit a .wav; edit the synthesis here.

import { writeFileAtomic } from "../../../scripts/atomic-write.mjs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const SR = 22050;
const OUT = dirname(fileURLToPath(import.meta.url));

// mulberry32, seeded per sound.
function rng(seed) {
  let a = seed >>> 0;
  return () => {
    a = (a + 0x6d2b79f5) >>> 0;
    let t = a;
    t = Math.imul(t ^ (t >>> 15), t | 1);
    t ^= t + Math.imul(t ^ (t >>> 7), t | 61);
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
}

function wavBytes(samples) {
  const data = Buffer.alloc(samples.length * 2);
  samples.forEach((s, i) => {
    const q = Math.round(Math.max(-1, Math.min(1, s)) * 32767);
    data.writeInt16LE(q, i * 2);
  });
  const header = Buffer.alloc(44);
  header.write("RIFF", 0);
  header.writeUInt32LE(36 + data.length, 4);
  header.write("WAVEfmt ", 8);
  header.writeUInt32LE(16, 16);
  header.writeUInt16LE(1, 20); // PCM
  header.writeUInt16LE(1, 22); // mono
  header.writeUInt32LE(SR, 24);
  header.writeUInt32LE(SR * 2, 28);
  header.writeUInt16LE(2, 32);
  header.writeUInt16LE(16, 34);
  header.write("data", 36);
  header.writeUInt32LE(data.length, 40);
  return Buffer.concat([header, data]);
}

// Rain: white noise through a one-pole low-pass, minus a slower one (a band).
function rain() {
  const rand = rng(0x5a1e);
  const n = SR;
  const raw = Array.from({ length: n }, () => rand() * 2 - 1);
  let lo = 0;
  let slow = 0;
  const out = raw.map((x) => {
    lo += 0.35 * (x - lo);
    slow += 0.02 * (lo - slow);
    return (lo - slow) * 0.9;
  });
  // Ramp both ends to silence so the loop seam does not click.
  const fade = 400;
  for (let i = 0; i < fade; i++) {
    out[i] *= i / fade;
    out[n - 1 - i] *= i / fade;
  }
  return out.map((s) => s * 0.5);
}

// Hum: 60 Hz and 120 Hz sines, whole cycles in one second so it loops exactly.
function hum() {
  return Array.from({ length: SR }, (_, i) => {
    const t = i / SR;
    return 0.35 * Math.sin(2 * Math.PI * 60 * t) + 0.18 * Math.sin(2 * Math.PI * 120 * t);
  });
}

// Chatter: three short tone bursts with gaps, 0.6 s.
function chatter() {
  const n = Math.floor(SR * 0.6);
  const out = new Array(n).fill(0);
  const bursts = [
    [0.0, 0.14, 420],
    [0.2, 0.12, 520],
    [0.38, 0.16, 380],
  ];
  for (const [start, len, hz] of bursts) {
    const s0 = Math.floor(start * SR);
    const count = Math.floor(len * SR);
    for (let i = 0; i < count; i++) {
      const env = Math.sin((Math.PI * i) / count);
      out[s0 + i] += 0.5 * env * Math.sin(2 * Math.PI * hz * (i / SR));
    }
  }
  return out;
}

for (const [name, make] of [
  ["rain", rain],
  ["hum", hum],
  ["chatter", chatter],
]) {
  writeFileAtomic(join(OUT, `${name}.wav`), wavBytes(make()));
}
