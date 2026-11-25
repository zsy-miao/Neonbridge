"""Résumé des créations et rejets de PSO relevés dans le journal TomBox."""
import re
from collections import Counter, defaultdict


LOG_LINE = re.compile(r"^\d+ \[d3d12_xs\] (\w+) (.*)$")
GRAPHICS_COUNTS = re.compile(r"^counts: pso_ok=(\d+) pso_fail=(\d+) untranslatable=(\d+) \|")
EVENTS = {
    "CreateGraphicsPSO_X": ("Graphique", {
        "création": r"PSO #(?P<id>\d+) created from translated shaders$",
        "rejet": r"real CreateGraphicsPipelineState FAILED hr=|untranslatable shader -> E_FAIL\b",
    }),
    "CreateComputePSO_X": ("Calcul", {
        "création": r"compute PSO #(?P<id>\d+) created from translated shader$",
        "rejet": r"real CreateComputePipelineState FAILED hr=|untranslatable compute shader -> E_FAIL\b",
    }),
    "CreatePipelineState": ("Flux", {
        "flux": r"console-numbered stream PSO #(?P<id>\d+) raw subobjects ",
        "rejet": r"PC driver rejected stream hr=|.* -> E_INVALIDARG$",
    }),
    "DeserializeGraphicsPSO_X": ("Graphique désérialisé", {
        "création": r"PSO rebuilt from TomBox serialized parts$",
        "rejet": r"real CreateGraphicsPipelineState FAILED hr=|rejected: .* -> E_INVALIDARG$",
    }),
    "DeserializeComputePSO_X": ("Calcul désérialisé", {
        "création": r"compute PSO rebuilt from TomBox serialized parts$",
        "rejet": r"real CreateComputePipelineState FAILED hr=|rejected: .* -> E_INVALIDARG$",
    }),
}
COLUMNS = ("création", "rejet", "flux")
MAX_LISTED = 25


def summary(log, output):
    """Lignes Markdown pour un seul lancement, à partir des événements consignés."""
    lines = ["## Inventaire des PSO", ""]
    counts = Counter()
    identifiers = defaultdict(list)
    graphics = None
    try:
        with log.open(encoding="utf-8", errors="replace") as source:
            for line in source:
                match = LOG_LINE.match(line)
                if not match or match[1] not in EVENTS:
                    continue
                function, detail = match.groups()
                kind, outcomes = EVENTS[function]
                if function == "CreateGraphicsPSO_X" and (snapshot := GRAPHICS_COUNTS.match(detail)):
                    graphics = snapshot.groups()
                for outcome, pattern in outcomes.items():
                    if event := re.match(pattern, detail):
                        counts[kind, outcome] += 1
                        if (identifier := event.groupdict().get("id")) and len(identifiers[kind]) < MAX_LISTED:
                            identifiers[kind].append(f"#{identifier}")
                        break
    except OSError:
        return lines + ["Journal TomBox indisponible."]

    lines += [f"Journal local `{log.relative_to(output).as_posix()}`.", ""]
    if not counts and graphics is None:
        return lines + ["Aucun événement PSO exploitable dans ce journal."]
    if graphics is not None:
        ok, rejected, untranslated = graphics
        lines += [f"Dernier relevé cumulé des PSO graphiques par TomBox. Créations : **{ok}**, "
                  f"rejets du pilote : **{rejected}**, échecs de traduction : **{untranslated}**.", ""]
    if counts:
        lines += ["Événements échantillonnés. Un flux observé précède la création du PSO.", "",
                  "| Type | Créations confirmées | Rejets | Flux observés |", "|---|---:|---:|---:|"]
        for kind, _ in EVENTS.values():
            values = [counts[kind, outcome] for outcome in COLUMNS]
            if any(values):
                lines.append(f"| {kind} | " + " | ".join(map(str, values)) + " |")
    if identifiers:
        lines += ["", f"Identifiants journalisés (au plus {MAX_LISTED} par type).", ""]
        lines.extend(f"- {kind} — {', '.join(ids)}" for kind, ids in identifiers.items())
    return lines
