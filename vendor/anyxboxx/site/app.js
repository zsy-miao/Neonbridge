// « Will my PC run it? » : détecte ce que le navigateur révèle (GPU par WebGL, threads, RAM arrondie), laisse
// chercher sa carte et son processeur en tapant, puis affiche l'estimation. Rien n'est envoyé nulle part.
import { CPUS, GPUS, detectGpu, estimate, matchCpu, matchGpu } from "./estimate.js";

const RAM_CHOICES = [8, 16, 32, 64];
const $ = (id) => document.getElementById(id);
const item = (texte) => Object.assign(document.createElement("li"), { textContent: texte });
const option = (value, label = value) => Object.assign(document.createElement("option"), { value, textContent: label });

function webglRenderer() {
  try {
    const gl = document.createElement("canvas").getContext("webgl");
    const ext = gl && gl.getExtension("WEBGL_debug_renderer_info");
    return ext ? gl.getParameter(ext.UNMASKED_RENDERER_WEBGL) : "";
  } catch (erreur) {
    console.warn("WebGL unavailable:", erreur);
    return "";
  }
}

function hint(id, texte, found, nombre, vide) {
  const element = $(`${id}-hint`);
  element.classList.toggle("miss", Boolean(texte) && !found && nombre === 0);
  if (!texte) element.textContent = vide;
  else if (found) element.textContent = "";
  else if (nombre === 0) element.textContent = "Not in the list: try only the model number, or pick the closest one.";
  else element.textContent = `${nombre} matches: pick yours in the list.`;
}

function render(threads) {
  const gpu = matchGpu($("gpu").value);
  const cpu = matchCpu($("cpu").value);
  hint("gpu", $("gpu").value.trim(), gpu.key, gpu.candidates, "");
  const sansCpu = threads ? `Leave empty to use the ${threads} threads your browser reports.` : "";
  hint("cpu", $("cpu").value.trim(), cpu.label, cpu.candidates, sansCpu);
  $("empty").hidden = Boolean(gpu.key);
  $("answer").hidden = !gpu.key;
  if (!gpu.key) return;
  const r = estimate({
    gpuKey: gpu.key, cpuLabel: cpu.label, threads, ramGb: Number($("ram").value), laptop: $("laptop").checked,
  });
  $("fps").textContent = r.fpsLow === r.fpsHigh ? `~${r.fpsLow} fps` : `~${r.fpsLow}–${r.fpsHigh} fps`;
  $("quality").textContent = r.quality;
  $("bottleneck").textContent = r.bottleneck ? `Limited by your ${r.bottleneck}.` : "";
  $("notes").replaceChildren(...r.notes.map(item));
}

function main() {
  const renderer = webglRenderer();
  const detected = detectGpu(renderer);
  const threads = navigator.hardwareConcurrency || null;
  const ramGb = navigator.deviceMemory || null;
  $("detected").replaceChildren(...[
    `Graphics: ${detected ? detected.label + (detected.laptop ? " (laptop)" : "") : renderer || "not revealed by this browser"}`,
    `CPU threads: ${threads ?? "not revealed"}`,
    `RAM: ${ramGb ? `about ${ramGb} GB (browsers round this value, check below)` : "not revealed"}`,
  ].map(item));
  $("gpu-list").append(...GPUS.map((g) => option(g[1])));
  $("cpu-list").append(...CPUS.map((c) => option(c[0])));
  $("ram").append(...RAM_CHOICES.map((gb) => option(String(gb), `${gb} GB`)));
  if (detected) $("gpu").value = detected.label;
  $("laptop").checked = Boolean(detected && detected.laptop);
  $("ram").value = String(RAM_CHOICES.find((gb) => gb >= (ramGb || 16)) || 16);
  for (const id of ["gpu", "cpu"]) $(id).addEventListener("input", () => render(threads));
  for (const id of ["ram", "laptop"]) $(id).addEventListener("change", () => render(threads));
  render(threads);
}

main();
