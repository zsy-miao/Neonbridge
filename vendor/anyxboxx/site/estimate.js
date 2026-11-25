// Rough "will my PC run it?" estimate for the Xbox Series build of GTA VI, relative to the Xbox Series X.
// Nothing here is measured: the game does not run yet. Hardware indexes are approximate (public benchmark
// averages, ±20 %), with the Series X GPU and CPU at 100. The model is deliberately simple and says so on the page.

export const ASSUMPTIONS = Object.freeze({
  consoleFps: 30,              // assumed frame rate of the console version
  overheadBest: 1.2,           // translated code and shaders cost 20 % more than on the console…
  overheadWorst: 1.6,          // …up to 60 % more: unknown until the game boots
  laptopFactor: 0.75,          // a laptop GPU of the same name is slower
  minThreads: 8,               // the console CPU runs 8 cores / 16 threads
  minRamGb: 16,
  comfortableRamGb: 32,
  minVramGb: 8,
  comfortableVramGb: 12,
});

// [match key, label, index (Series X GPU = 100), VRAM in GB]
export const GPUS = Object.freeze([
  ["rtx 5090", "GeForce RTX 5090", 330, 32], ["rtx 5080", "GeForce RTX 5080", 215, 16],
  ["rtx 5070 ti", "GeForce RTX 5070 Ti", 180, 16], ["rtx 5070", "GeForce RTX 5070", 145, 12],
  ["rtx 5060 ti", "GeForce RTX 5060 Ti", 100, 16], ["rtx 5060", "GeForce RTX 5060", 85, 8],
  ["rtx 4090", "GeForce RTX 4090", 290, 24], ["rtx 4080 super", "GeForce RTX 4080 Super", 200, 16],
  ["rtx 4080", "GeForce RTX 4080", 195, 16], ["rtx 4070 ti super", "GeForce RTX 4070 Ti Super", 165, 16],
  ["rtx 4070 ti", "GeForce RTX 4070 Ti", 152, 12], ["rtx 4070 super", "GeForce RTX 4070 Super", 140, 12],
  ["rtx 4070", "GeForce RTX 4070", 122, 12], ["rtx 4060 ti", "GeForce RTX 4060 Ti", 92, 8],
  ["rtx 4060", "GeForce RTX 4060", 75, 8], ["rtx 3090 ti", "GeForce RTX 3090 Ti", 145, 24],
  ["rtx 3090", "GeForce RTX 3090", 132, 24], ["rtx 3080 ti", "GeForce RTX 3080 Ti", 128, 12],
  ["rtx 3080", "GeForce RTX 3080", 118, 10], ["rtx 3070 ti", "GeForce RTX 3070 Ti", 100, 8],
  ["rtx 3070", "GeForce RTX 3070", 94, 8], ["rtx 3060 ti", "GeForce RTX 3060 Ti", 83, 8],
  ["rtx 3060", "GeForce RTX 3060", 63, 12], ["rtx 3050", "GeForce RTX 3050", 42, 8],
  ["rtx 2080 ti", "GeForce RTX 2080 Ti", 96, 11], ["rtx 2080 super", "GeForce RTX 2080 Super", 83, 8],
  ["rtx 2080", "GeForce RTX 2080", 79, 8], ["rtx 2070 super", "GeForce RTX 2070 Super", 73, 8],
  ["rtx 2070", "GeForce RTX 2070", 65, 8], ["rtx 2060 super", "GeForce RTX 2060 Super", 59, 8],
  ["rtx 2060", "GeForce RTX 2060", 51, 6], ["gtx 1080 ti", "GeForce GTX 1080 Ti", 70, 11],
  ["gtx 1080", "GeForce GTX 1080", 55, 8], ["gtx 1070", "GeForce GTX 1070", 45, 8],
  ["gtx 1660 super", "GeForce GTX 1660 Super", 39, 6], ["gtx 1650", "GeForce GTX 1650", 25, 4],
  ["rx 9070 xt", "Radeon RX 9070 XT", 190, 16], ["rx 9070", "Radeon RX 9070", 165, 16],
  ["rx 9060 xt", "Radeon RX 9060 XT", 98, 16], ["rx 7900 xtx", "Radeon RX 7900 XTX", 215, 24],
  ["rx 7900 xt", "Radeon RX 7900 XT", 185, 20], ["rx 7800 xt", "Radeon RX 7800 XT", 140, 16],
  ["rx 7700 xt", "Radeon RX 7700 XT", 120, 12], ["rx 7600 xt", "Radeon RX 7600 XT", 82, 16],
  ["rx 7600", "Radeon RX 7600", 78, 8], ["rx 6950 xt", "Radeon RX 6950 XT", 160, 16],
  ["rx 6900 xt", "Radeon RX 6900 XT", 150, 16], ["rx 6800 xt", "Radeon RX 6800 XT", 140, 16],
  ["rx 6800", "Radeon RX 6800", 120, 16], ["rx 6750 xt", "Radeon RX 6750 XT", 108, 12],
  ["rx 6700 xt", "Radeon RX 6700 XT", 103, 12], ["rx 6700", "Radeon RX 6700", 95, 10],
  ["rx 6650 xt", "Radeon RX 6650 XT", 82, 8], ["rx 6600 xt", "Radeon RX 6600 XT", 77, 8],
  ["rx 6600", "Radeon RX 6600", 67, 8], ["rx 5700 xt", "Radeon RX 5700 XT", 70, 8],
  ["rx 580", "Radeon RX 580", 32, 8], ["arc b580", "Intel Arc B580", 82, 12],
  ["arc a770", "Intel Arc A770", 72, 16], ["arc a750", "Intel Arc A750", 65, 8],
]);

// [label, index (Series X CPU = 100), threads]
export const CPUS = Object.freeze([
  ["Ryzen 7 9800X3D", 225, 16], ["Ryzen 9 9950X", 215, 32], ["Ryzen 7 9700X", 180, 16],
  ["Ryzen 5 9600X", 165, 12], ["Ryzen 7 7800X3D", 200, 16], ["Ryzen 9 7950X", 205, 32],
  ["Ryzen 7 7700X", 170, 16], ["Ryzen 5 7600X", 158, 12], ["Ryzen 5 7600", 150, 12],
  ["Ryzen 7 5800X3D", 160, 16], ["Ryzen 7 5800X", 135, 16], ["Ryzen 7 5700X", 130, 16],
  ["Ryzen 5 5600X", 125, 12], ["Ryzen 5 5600", 120, 12], ["Ryzen 7 3700X", 105, 16],
  ["Ryzen 5 3600", 95, 12], ["Ryzen 5 2600", 70, 12], ["Ryzen 5 1600", 55, 12],
  ["Core Ultra 9 285K", 205, 24], ["Core Ultra 7 265K", 190, 20], ["Core Ultra 5 245K", 165, 14],
  ["Core i9-14900K", 205, 32], ["Core i7-14700K", 195, 28], ["Core i5-14600K", 170, 20],
  ["Core i9-13900K", 200, 32], ["Core i7-13700K", 185, 24], ["Core i5-13600K", 165, 20],
  ["Core i5-13400F", 135, 16], ["Core i7-12700K", 165, 20], ["Core i5-12600K", 145, 16],
  ["Core i5-12400F", 125, 12], ["Core i3-12100F", 95, 8], ["Core i7-10700K", 105, 16],
  ["Core i5-10400F", 85, 12], ["Core i7-9700K", 90, 8], ["Core i7-8700K", 85, 12],
  ["Core i5-9600K", 75, 6], ["Core i5-8400", 65, 6],
]);

export const QUALITY_TIERS = Object.freeze([
  [0.5, "Unlikely to be playable"],
  [0.8, "Low settings, reduced resolution (upscaling), around 30 fps at best"],
  [1.2, "Close to console quality, around 30 fps"],
  [1.6, "Above console quality, or console quality at about 40 to 50 fps"],
  [Infinity, "High settings, about 50 to 60 fps or more"],
]);

const normalise = (texte) => String(texte).toLowerCase().replace(/\s+/g, " ");

/** Find the GPU named in a WebGL renderer string; « rtx 3070 » does not match « rtx 3070 ti ». */
export function detectGpu(renderer) {
  const texte = normalise(renderer);
  let trouve = null;
  for (const gpu of GPUS) {
    const motif = new RegExp(`\\b${gpu[0].replace(/ /g, "\\s*")}\\b(?!\\s*(ti|super|xt|xtx)\\b)`);
    if (motif.test(texte) && (!trouve || gpu[0].length > trouve[0].length)) trouve = gpu;
  }
  return trouve && { key: trouve[0], label: trouve[1], laptop: /laptop|mobile|max-q/.test(texte) };
}

export function findGpu(key) { return GPUS.find((gpu) => gpu[0] === key) || null; }
export function findCpu(label) { return CPUS.find((cpu) => cpu[0] === label) || null; }

function hardwareNotes(gpu, ramGb, cpuKnown) {
  const a = ASSUMPTIONS;
  const notes = [];
  if (!cpuKnown) notes.push("Pick your CPU for a better estimate (browsers do not reveal its model).");
  if (ramGb !== null && ramGb < a.minRamGb) notes.push(`${ramGb} GB of RAM: plan for ${a.minRamGb} GB at least.`);
  else if (ramGb !== null && ramGb < a.comfortableRamGb) {
    notes.push(`${ramGb} GB of RAM should do; ${a.comfortableRamGb} GB is more comfortable.`);
  }
  if (gpu[3] < a.minVramGb) notes.push(`${gpu[3]} GB of video memory: textures will have to be lowered.`);
  else if (gpu[3] < a.comfortableVramGb) notes.push(`${gpu[3]} GB of video memory may limit texture quality.`);
  return notes;
}

/** Estimate from a GPU key, a CPU label (or only a thread count), the RAM in GB and a laptop flag. */
export function estimate({ gpuKey, cpuLabel = null, threads = null, ramGb = null, laptop = false }) {
  const a = ASSUMPTIONS;
  const gpu = findGpu(gpuKey);
  if (!gpu) throw new Error(`unknown GPU: ${gpuKey}`);
  const cpu = cpuLabel ? findCpu(cpuLabel) : null;
  if (cpuLabel && !cpu) throw new Error(`unknown CPU: ${cpuLabel}`);
  const gpuIndex = gpu[2] * (laptop ? a.laptopFactor : 1);
  const cpuIndex = cpu ? cpu[1] : null;
  const nbThreads = cpu ? cpu[2] : threads;
  const notes = hardwareNotes(gpu, ramGb, cpu !== null);
  let ratio = Math.min(gpuIndex, cpuIndex ?? gpuIndex) / 100;
  if (nbThreads !== null && nbThreads < a.minThreads) {
    ratio *= nbThreads / a.minThreads;
    notes.unshift(`Only ${nbThreads} CPU threads: the console runs 8 cores / 16 threads.`);
  }
  const milieu = ratio / ((a.overheadBest + a.overheadWorst) / 2);
  return {
    ratio,
    fpsLow: Math.round((a.consoleFps * ratio) / a.overheadWorst),
    fpsHigh: Math.round((a.consoleFps * ratio) / a.overheadBest),
    quality: QUALITY_TIERS.find(([plafond]) => milieu < plafond)[1],
    bottleneck: cpuIndex === null ? null : (cpuIndex < gpuIndex ? "CPU" : "GPU"),
    notes,
  };
}

const compact = (texte) => String(texte).toLowerCase().replace(/[^a-z0-9]/g, "");

/**
 * Find the entry a typed text names. An exact name wins; otherwise the only name containing the text, or, among
 * several, the one that ends with it (« 4070 » finds the RTX 4070, not the 4070 Ti). Returns { match, candidates }.
 */
function matchTyped(texte, entries, label) {
  const cherche = compact(texte);
  if (!cherche) return { match: null, candidates: [] };
  const exact = entries.find((entry) => compact(label(entry)) === cherche);
  if (exact) return { match: exact, candidates: [exact] };
  const candidates = entries.filter((entry) => compact(label(entry)).includes(cherche));
  const finissent = candidates.filter((entry) => compact(label(entry)).endsWith(cherche));
  if (candidates.length === 1) return { match: candidates[0], candidates };
  return { match: finissent.length === 1 ? finissent[0] : null, candidates };
}

export function matchGpu(texte) {
  const { match, candidates } = matchTyped(texte, GPUS, (gpu) => gpu[1]);
  return { key: match ? match[0] : null, label: match ? match[1] : null, candidates: candidates.length };
}

export function matchCpu(texte) {
  const { match, candidates } = matchTyped(texte, CPUS, (cpu) => cpu[0]);
  return { label: match ? match[0] : null, candidates: candidates.length };
}
