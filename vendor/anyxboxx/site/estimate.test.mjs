// node --test site/estimate.test.mjs : logique de l'estimation du site (sans navigateur).
import { test } from "node:test";
import assert from "node:assert/strict";
import { CPUS, GPUS, QUALITY_TIERS, detectGpu, estimate } from "./estimate.js";

test("les clés et libellés sont uniques, les indices positifs", () => {
  assert.equal(new Set(GPUS.map((g) => g[0])).size, GPUS.length);
  assert.equal(new Set(CPUS.map((c) => c[0])).size, CPUS.length);
  for (const [, , indice, vram] of GPUS) assert.ok(indice > 0 && vram > 0);
  for (const [, indice, threads] of CPUS) assert.ok(indice > 0 && threads > 0);
});

test("détection WebGL : Ti, Super et XT ne sont pas confondus avec le modèle de base", () => {
  const angle = (nom) => `ANGLE (NVIDIA, NVIDIA GeForce ${nom} (0x00002484) Direct3D11 vs_5_0 ps_5_0, D3D11)`;
  assert.equal(detectGpu(angle("RTX 3070")).key, "rtx 3070");
  assert.equal(detectGpu(angle("RTX 3070 Ti")).key, "rtx 3070 ti");
  assert.equal(detectGpu(angle("RTX 4070 Ti SUPER")).key, "rtx 4070 ti super");
  assert.equal(detectGpu("ANGLE (AMD, AMD Radeon RX 6700 XT Direct3D11)").key, "rx 6700 xt");
  assert.equal(detectGpu("ANGLE (AMD, AMD Radeon RX 7900 XTX Direct3D11)").key, "rx 7900 xtx");
  assert.equal(detectGpu(angle("RTX 4060 Laptop GPU")).laptop, true);
  assert.equal(detectGpu("Apple M2"), null);
});

test("une machine au niveau de la Series X reste sous la qualité console (surcoût de traduction)", () => {
  const r = estimate({ gpuKey: "rx 6700", cpuLabel: "Ryzen 7 3700X", ramGb: 16 });
  assert.equal(r.bottleneck, "GPU");
  assert.ok(r.fpsLow < r.fpsHigh && r.fpsHigh <= 30);
  assert.equal(r.quality, QUALITY_TIERS[1][1]);
});

test("une machine haut de gamme vise 60 fps, limitée par le processeur", () => {
  const r = estimate({ gpuKey: "rtx 4090", cpuLabel: "Ryzen 7 9800X3D", ramGb: 32 });
  assert.equal(r.bottleneck, "CPU");
  assert.ok(r.fpsLow >= 40);
  assert.equal(r.quality, QUALITY_TIERS.at(-1)[1]);
  assert.deepEqual(r.notes, []);
});

test("peu de threads, peu de RAM ou de VRAM, GPU portable : pénalités et avertissements", () => {
  const base = estimate({ gpuKey: "rtx 3060 ti", threads: 16, ramGb: 32 });
  const r = estimate({ gpuKey: "rtx 3060 ti", threads: 4, ramGb: 8, laptop: true });
  assert.ok(r.ratio < base.ratio * 0.5);
  assert.match(r.notes.join(" "), /4 CPU threads/);
  assert.match(r.notes.join(" "), /16 GB at least/);
  assert.match(base.notes.join(" "), /Pick your CPU/);
  assert.match(estimate({ gpuKey: "gtx 1650" }).notes.join(" "), /textures will have to be lowered/);
});

test("un GPU ou un CPU inconnu est une erreur explicite", () => {
  assert.throws(() => estimate({ gpuKey: "voodoo 2" }), /unknown GPU/);
  assert.throws(() => estimate({ gpuKey: "rtx 3070", cpuLabel: "Pentium 4" }), /unknown CPU/);
});

test("recherche tapée : nom exact, fragment unique, fragment qui termine le nom", async () => {
  const { matchCpu, matchGpu } = await import("./estimate.js");
  assert.equal(matchCpu("13700k").label, "Core i7-13700K");
  assert.equal(matchCpu("i7 13700K").label, "Core i7-13700K");
  assert.equal(matchCpu("7800x3d").label, "Ryzen 7 7800X3D");
  assert.equal(matchCpu("Ryzen 7").label, null);
  assert.ok(matchCpu("Ryzen 7").candidates > 1);
  assert.equal(matchGpu("GeForce RTX 4070 Super").key, "rtx 4070 super");
  assert.equal(matchGpu("4070").key, "rtx 4070");
  assert.equal(matchGpu("4070 ti").key, "rtx 4070 ti");
  assert.equal(matchGpu("6700 xt").key, "rx 6700 xt");
  assert.equal(matchGpu("b580").key, "arc b580");
  assert.deepEqual(matchGpu(""), { key: null, label: null, candidates: 0 });
  assert.equal(matchGpu("voodoo").candidates, 0);
});
