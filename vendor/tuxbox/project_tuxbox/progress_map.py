"""Render the public runtime and Xbox library compatibility baseline."""
from collections import Counter
import json
from pathlib import Path
from xml.etree import ElementTree as ET

from .verify import homebrew_matrix


WIDTH, PAD, GAP = 1000, 14, 12
INNER = WIDTH - 2 * PAD
BG, BAR, CHROME = "#0b0b0f", "#090910", "#0d0d15"
PANEL, LINE = "#14141d", "#272733"
TEXT, MUTED, AQUA = "#d7d7de", "#a0a0b1", "#6fc7d4"
GREEN, AMBER, RED = "#2ecc71", "#e2bc8c", "#e74c3c"
PATH_COLORS = {"verified": GREEN, "partial": AMBER, "unverified": MUTED, "blocked": RED}
MONO = "'Terminess Nerd Font Mono', 'DejaVu Sans Mono', Consolas, monospace"


def element(parent, tag, **attrs):
    return ET.SubElement(parent, tag, {k.replace("_", "-"): str(v) for k, v in attrs.items()})


def text(parent, x, y, value, size=16, color=TEXT, *, bold=False, anchor="start"):
    node = element(parent, "text", x=x, y=y, fill=color, font_size=size, text_anchor=anchor,
                   font_weight="bold" if bold else "normal")
    node.text = str(value)
    return node


def rect(node, x, y, width, height, color, **attrs):
    return element(node, "rect", x=x, y=y, width=width, height=height, fill=color, **attrs)


def read_json(path):
    return json.loads(path.read_text(encoding="utf-8")) if path.is_file() else {}


def section(parent, x, y, heading):
    group = element(parent, "g", transform=f"translate({x} {y})")
    element(group, "title").text = heading
    return group


def window(group, width, height, path, *, focused=False):
    rect(group, 0, 0, width, height, PANEL, stroke=AMBER if focused else LINE,
         stroke_width=2 if focused else 1)
    rect(group, 1, 1, width-2, 32, CHROME)
    rect(group, 1, 33, width-2, 1, LINE)
    for i, color in enumerate((RED, "#f1c40f", GREEN)):
        element(group, "circle", cx=16+16*i, cy=17, r=4.5, fill=color)
    text(group, 68, 22, path, 14, AQUA)


# Labels for observations in a packaged-settings retail run.
FROSTY_GATES = {
    "load_world": "World loaded",
    "submit_frame": "Frame submitted",
    "use_menu": "Menu used",
    "load_wake_up_room": "Wake Up room loaded",
    "load_chase_intro": "Chase intro loaded",
    "play_chase": "Chase played",
    "explore_city": "City explored",
    "verify_visuals": "Visuals verified",
    "verify_audio": "Audio verified",
    "verify_frame_pacing": "Frame pacing verified",
}


def draw_overall(svg, y, paths, inventory, shader_rows):
    verified = sum(row["status"] == "verified" for row in paths)
    library_share = verified / len(paths) if paths else None
    archives = inventory.get("archives", {})
    totals = [archives.get(name, {}).get("entries") for name in ("global", "material")]
    shader_total = sum(totals) if all(type(n) is int and n > 0 for n in totals) else 0
    surveyed = sum(row[2] for row in shader_rows)
    translated = sum(row[1] for row in shader_rows)
    shader_share = translated / shader_total if surveyed == shader_total and shader_total else None

    group = section(svg, PAD, y, "Overall TuxBox coverage")
    height = 112
    rect(group, 0, 0, INNER, height, PANEL, stroke=AMBER)
    text(group, 18, 32, "Overall TuxBox coverage", 25, AMBER, bold=True)
    overall = None if library_share is None or shader_share is None else (library_share + shader_share) / 2
    text(group, INNER-18, 32, f"{overall:.1%}" if overall is not None else "Unmeasured",
         23, AQUA if overall is not None else MUTED, bold=True, anchor="end")
    rect(group, 18, 45, INNER-36, 8, LINE)
    if overall:
        rect(group, 18, 45, (INNER-36)*overall, 8, AQUA)
    library_label = f"{verified}/{len(paths)} verified ({library_share:.1%})" if library_share is not None else "Unmeasured"
    shader_label = f"{translated:,}/{shader_total:,} entries ({shader_share:.1%})" if shader_share is not None else "Unmeasured"
    text(group, 18, 77, f"Library behaviors · {library_label}", 15)
    text(group, INNER-18, 77, f"Shader translation · {shader_label}", 15, anchor="end")
    text(group, 18, 99, "Frosty sample · 50% required library behaviors, 50% shader translation.", 13, MUTED)
    return y+height+GAP


def draw_frosty_target(svg, y, evidence):
    title = read_json(evidence / "matrix-awakens-launch.json")
    if not title:
        return y
    receipt = read_json(evidence / title["progress_receipt"]) if title.get("progress_receipt") else {}
    settings = receipt.get("settings", {})
    run = receipt.get("runs", {}).get("tuxbox", {})
    measured = bool(title.get("sha256") and title["sha256"] == receipt.get("title_sha256")
                    and run.get("log_sha256") and "verified_milestones" in run
                    and settings.get("graphics_profile") == "packaged"
                    and settings.get("loose_graphics_overrides_in_package") == 0
                    and settings.get("ray_tracing") == settings.get("lumen_hardware_ray_tracing") == 1)
    verified = set(run["verified_milestones"]) if measured else set()
    unknown = verified.difference(FROSTY_GATES)
    if unknown:
        raise ValueError(f"Unknown Frosty milestone(s): {sorted(unknown)}")
    failure = receipt.get("failure", {}) if measured else {}
    shader = failure.get("global_shader")
    blocker = f"Blocked at global shader {shader} · {failure['entry']}" if shader is not None else ""
    labels = [label for key, label in FROSTY_GATES.items() if key in verified]
    # Five or more observations wrap to a second line within the SVG.
    lines = [" · ".join(labels[pos:pos+4]) for pos in range(0, len(labels), 4)]
    if not lines:
        lines = ["No verified gameplay observations" if measured else "No matching retail run"]
    reduced = read_json(evidence / "matrix-awakens/wake-up-xg.json")
    executable = reduced.get("providers", {}).get("Frosty-XSX-Shipping.exe", {})
    reduced_settings = reduced.get("profile_cvars", {})
    reached_wake_up = (bool(title.get("sha256")) and executable.get("sha256") == title["sha256"]
                       and reduced.get("fixed_log_sha256") and reduced_settings.get("r.RayTracing") == "0"
                       and reduced_settings.get("r.Lumen.DiffuseIndirect.Allow") == "0"
                       and reduced_settings.get("r.Lumen.Reflections.Allow") == "0"
                       and any(event.get("event") == "Construct bullet time" for event in reduced.get("events", [])))
    height = 65 + 22*len(lines) + (30 if blocker else 0) + (30 if reached_wake_up else 0)
    group = section(svg, PAD, y, "Frosty retail test")
    rect(group, 0, 0, INNER, height, PANEL, stroke=LINE)
    text(group, 18, 32, "Frosty retail test", 23, AMBER, bold=True)
    for index, line in enumerate(lines):
        text(group, 18, 61+22*index, ("Packaged settings · " if index == 0 else "") + line, 15)
    if blocker:
        text(group, 18, 91+22*(len(lines)-1), blocker, 15, AMBER)
    if reached_wake_up:
        text(group, 18, height-13, "Reduced settings · Wake Up reached the bullet-time scene", 15, GREEN)
    return y+height+GAP


def load_library_paths(evidence):
    audit = read_json(evidence / "matrix-awakens/library-paths.json")
    title = read_json(evidence / "matrix-awakens-launch.json")
    if not title.get("sha256") or audit.get("title_sha256") != title["sha256"]:
        return {}
    names = set()
    for row in audit["paths"]:
        name = row["name"]
        key = name.strip().casefold()
        if not key or key in names:
            raise ValueError(f"Missing or duplicate library behavior {name!r}")
        names.add(key)
        libs = row.get("libraries")
        if (row.get("status") not in PATH_COLORS or not isinstance(libs, list) or not libs
                or not all(isinstance(lib, str) and lib.strip() for lib in libs)
                or not row.get("provider") or not row.get("scope")):
            raise ValueError(f"Incomplete library path record for {name}")
    for row in audit.get("optional", []):
        if not row.get("name") or not row.get("reason"):
            raise ValueError("Incomplete optional library record")
    for row in [*audit["paths"], *audit.get("optional", [])]:
        refs = row.get("evidence")
        if (not isinstance(refs, list) or not refs
                or not all(isinstance(ref, str) and not Path(ref).is_absolute() and ".." not in Path(ref).parts
                           and (evidence.parent / ref).is_file() for ref in refs)):
            raise ValueError(f"Missing evidence for {row['name']}")
    return audit


def library_table(evidence):
    audit = load_library_paths(evidence)
    if not audit.get("paths"):
        return "No matching Frosty library behavior audit."
    rows = audit["paths"]
    verified = sum(row["status"] == "verified" for row in rows)
    lines = [f"Frosty · {verified}/{len(rows)} required library behaviors verified"]
    for row in rows:
        lines.extend(["", f"{row['status'].capitalize()} · {row['name']} · {row['provider']}",
                      f"  {', '.join(row['libraries'])}", f"  {row['scope']}"])
    if audit.get("optional"):
        lines.extend(["", "Outside the current gameplay target"])
        lines.extend(f"  {row['name']} · {row['reason']}" for row in audit["optional"])
    return "\n".join(lines)


def draw_libraries(svg, y, rows):
    if not rows:
        return y
    height = 141 + 29*len(rows)
    group = section(svg, PAD, y, "Required library behaviors")
    window(group, INNER, height, "~/docs/LIBRARIES.md")
    text(group, 18, 70, "Required library behaviors", 25, AMBER, bold=True)
    text(group, 18, 96, "Verified in a game run or focused check. Partial means some behavior still needs verification.", 15)
    text(group, 18, 122, "Behavior", 13, MUTED)
    text(group, 420, 122, "Provided by", 13, MUTED)
    text(group, INNER-18, 122, "Status", 13, MUTED, anchor="end")
    for index, row in enumerate(rows):
        top = 149 + 29*index
        cell = element(group, "g")
        element(cell, "title").text = f"{', '.join(row['libraries'])} · {row['scope']}"
        rect(cell, 18, top-19, INNER-36, 1, LINE)
        text(cell, 18, top, row["name"], 15)
        text(cell, 420, top, row["provider"], 14)
        text(cell, INNER-18, top, row["status"].capitalize(), 14, PATH_COLORS[row["status"]], bold=True, anchor="end")
    return y+height+GAP


def shader_validation(data):
    checks = [row.get("standalone_dxil_validated") for row in data["requested"]]
    measured = sum(check is not None for check in checks)
    if not measured:
        return ""
    return (f"Standalone DXIL passed for {checks.count(True)}/{measured} tested programs "
            f"among {len(checks)} distinct title requests.")


def culling_readbacks(evidence, archive, compiler, name):
    census, probe, fields = {
        "InstanceCull": ("instance-cull-retail", "instance-cull", ("checked_words", "checked_texels")),
        "PersistentClusterCull": ("persistent-cluster-retail", "persistent-cull", ("verified_uav_words",)),
    }[name]
    retail = read_json(evidence / f"matrix-awakens/{census}.json")
    if not retail or not archive or not compiler or retail["global_archive_sha256"] != archive:
        return ""
    family = set(retail["family"]["archive_entries"])
    paired = []
    provider = None
    for suffix, adapter in (("amd", "1002:13c0"), ("nvidia", "10de:2705")):
        run = read_json(evidence / f"matrix-awakens/{probe}-{suffix}.json")
        if (run.get("adapter") != adapter or run.get("name") != name
                or run.get("compiler_sha256") != compiler or run.get("global_archive_sha256") != archive
                or not run.get("provider_sha256") or (provider and provider != run["provider_sha256"])):
            return ""
        provider = run["provider_sha256"]
        paired.append({(p["entry"], p["source_sha256"], p["dxil_sha256"]) for p in run["programs"]
                       if p["entry"] in family and p["cases"]
                       and all(c[field] > 0 for c in p["cases"].values() for field in fields)})
    verified = paired[0] & paired[1]
    # The trace identifies the original Xbox program. Both current GPUs must
    # agree on the tested DXIL, which can change after a compiler correction.
    if not any(entry == retail["entry"] and source == retail["source_sha256"]
               and (compiler != retail["compiler_sha256"] or dxil == retail["dxil_sha256"])
               for entry, source, dxil in verified):
        return ""
    count = len({entry for entry, _, _ in verified})
    return f"Nanite {name} · {count}/{len(family)} packaged variants passed GPU readback on both adapters"


def shader_archive_results(evidence, data):
    archives = {name: archive["member_sha256"] for name, archive in data.get("archives", {}).items()
                if archive.get("member_sha256")}
    raster = read_json(evidence / "matrix-awakens/raster-archive-survey.json")
    for name in ("global", "material"):
        report = raster.get("archives", {}).get(name, {})
        if not report.get("compiler_sha256") or report.get("archive_sha256") != archives.get(name):
            continue
        stages = [report.get("stages", {}).get(stage, {}) for stage in ("vertex", "pixel", "geometry")]
        if not all(type(s.get("translated")) is int and type(s.get("entries")) is int
                   and 0 <= s["translated"] <= s["entries"] for s in stages):
            continue
        total = sum(s["entries"] for s in stages)
        if total and name in archives:
            yield (name.capitalize() + " raster", sum(s["translated"] for s in stages),
                   total, "HLSL", report["compiler_sha256"])
    for name, filename in (("global", "compute-archive-survey.json"),
                           ("material", "material-compute-survey.json")):
        compute = read_json(evidence / "matrix-awakens" / filename)
        stage = compute.get("stage", {})
        total, translated = stage.get("entries"), stage.get("translated")
        if (name in archives and compute.get("archive_sha256") == archives[name]
                and compute.get("compiler_sha256") and type(total) is int and type(translated) is int
                and total > 0 and 0 <= translated <= total):
            yield name.capitalize() + " compute", translated, total, "HLSL", compute["compiler_sha256"]
    # One unique program per mesh archive entry lets us use the inventory's
    # HLSL survey directly, without double-counting shared program hashes.
    mesh = next((s for s in data.get("stages", []) if s["stage"] == "Mesh"), {})
    total, emitted = mesh.get("global"), mesh.get("hlsl_emitted")
    if ("global" in archives and data.get("compiler_sha256") and not mesh.get("material")
            and type(total) is int and total > 0 and total == mesh.get("unique_dxbc")
            and type(emitted) is int and 0 <= emitted <= total):
        yield "Global mesh", emitted, total, "HLSL", data["compiler_sha256"]
    census = read_json(evidence / "matrix-awakens/ray-state-census.json")
    sources = census.get("archives", {})
    if set(sources) == {"global", "material"} and census.get("xshader_sha256") and all(
            name in archives and info.get("archive_sha256") == archives[name]
            for name, info in sources.items()):
        compiled = total = 0
        for source in sources.values():
            count = source.get("desktop_translation", {}).get("compiled_entries")
            entries = list(source.get("entries", {}).values())
            if (type(count) is not int or not all(type(n) is int and n >= 0 for n in entries)
                    or not 0 <= count <= sum(entries)):
                return
            total += sum(entries)
            compiled += count
        if total:
            yield "Ray tracing", compiled, total, "DXIL", census.get("xshader_sha256", "")


def shader_checks(evidence, data):
    notes = [validation] if (validation := shader_validation(data)) else []
    for label, translated, total, output, compiler in shader_archive_results(evidence, data):
        if output == "HLSL":
            notes.append(f"{label} archive · {translated:,}/{total:,} entries emitted HLSL "
                         f"({translated/total:.1%}) · compiler {compiler[:8]}")
        else:
            notes.append(f"{label} · {translated:,} of {total:,} packaged entries compiled to DXIL "
                         f"({translated/total:.1%})")
    archive = data.get("archives", {}).get("global", {}).get("member_sha256")
    warmup = read_json(evidence / "matrix-awakens/chase-warmup.json")
    if (warmup and warmup["title_sha256"] == data.get("executable_sha256")
            and warmup["global_archive_sha256"] == archive
            and warmup["compiler_sha256"] == data.get("compiler_sha256")
            and warmup["warm"]["total_compiled"] == 0):
        chase = warmup["warm"]["chase"]
        notes.append(f"Gameplay shader warmup · {chase['programs']['checked']:,} programs and "
                     f"{chase['links']['checked']} linked pairs ready before playing")
    total = next((row["global"] for row in data["stages"] if row["stage"] == "Mesh"), 0)
    results = []
    for suffix, gpu in (("nvidia", "NVIDIA"), ("amd", "Radeon")):
        run = read_json(evidence / f"matrix-awakens/nanite-mesh-{suffix}.json")
        pipelines = run.get("mesh_pipelines", {})
        if (total and pipelines and run.get("global_archive_sha256") == archive
                and pipelines["compiler_sha256"] == data["compiler_sha256"]):
            results.append(f"{gpu} {len(set(pipelines['archive_entries']))}/{total}")
    if results:
        notes.append("Packaged mesh shaders with a GPU pipeline · " + " · ".join(results))
    for name in ("InstanceCull", "PersistentClusterCull"):
        if cull := culling_readbacks(evidence, archive, data.get("compiler_sha256"), name):
            notes.append(cull)
    micro = read_json(evidence / "matrix-awakens/micropoly-retail.json")
    if micro and micro["global_archive_sha256"] == archive:
        pipelines = []
        for suffix, adapter in (("amd", "1002:13c0"), ("nvidia", "10de:2705")):
            run = read_json(evidence / f"matrix-awakens/micropoly-{suffix}.json")
            suite = run.get("pipelines", {})
            if (run.get("adapter") == adapter and run.get("global_archive_sha256") == archive
                    and run.get("source_sha256") == micro["source_sha256"]
                    and run.get("compiler_sha256") == suite.get("compiler_sha256") == data["compiler_sha256"]
                    and run.get("cases") and all(c["checked_texels"] > 0 for c in run["cases"].values())):
                pipelines.append((suite["program_manifest_sha256"], run["provider_sha256"], run["dxil_sha256"],
                                  frozenset(suite["archive_entries"])))
        if len(pipelines) == 2 and pipelines[0] == pipelines[1]:
            family = set(micro["family"]["archive_entries"])
            count = len(family & pipelines[0][3])
            notes.append(f"Nanite MicropolyRasterize · {count}/{len(family)} pipelines on both adapters "
                         f"· retail shader {micro['entry']} passed pixel readback")
    for suite in ("classify-materials", "stencil-export", "tsr-filter", "tsr-history"):
        amd, nvidia = (read_json(evidence / f"matrix-awakens/{suite}-{vendor}.json")
                       for vendor in ("amd", "nvidia"))
        if not (amd.get("adapter") == "1002:13c0" and nvidia.get("adapter") == "10de:2705"
                and amd.get("provider_sha256") == nvidia.get("provider_sha256")
                and amd.get("compiler_sha256") == nvidia.get("compiler_sha256") == data["compiler_sha256"]
                and amd.get("global_archive_sha256") == nvidia.get("global_archive_sha256") == archive):
            continue
        if suite == "classify-materials":
            if (amd["source_sha256"] == nvidia["source_sha256"] and amd["dxil_sha256"] == nvidia["dxil_sha256"]
                    and amd["cases"] == nvidia["cases"] and amd["cases"]):
                notes.append(f"Nanite ClassifyMaterials · {len(amd['cases'])} material tile tests passed on both adapters")
        elif suite in ("tsr-filter", "tsr-history"):
            if (amd["source_sha256"] == nvidia["source_sha256"] and amd["dxil_sha256"] == nvidia["dxil_sha256"]
                    and amd["cases"] == nvidia["cases"] and amd["cases"]):
                texels = sum(output["verified_texels"] for case in amd["cases"].values() for output in case["outputs"])
                label = "temporal filter" if suite == "tsr-filter" else "temporal history"
                notes.append(f"City flyover {label} · {texels:,} output texels matched on both adapters")
        else:
            readbacks = [{(s["source_sha256"], s["readback_sha256"], s["matching_texels"])
                          for s in run["shaders"] if s["matching_texels"] > 0} for run in (amd, nvidia)]
            count = len(readbacks[0] & readbacks[1])
            if count:
                notes.append(f"Pixel shader stencil export · {count} retail programs matched depth and stencil on both adapters")
    return notes


def independent_counts(data):
    measurement = data.get("independent_emitter")
    if measurement is None:
        return {}
    counts = Counter(p["stage"] for p in measurement["programs"])
    return {row["stage"]: counts[row["stage"]] for row in data["stages"] if row["unique_dxbc"]}


def shader_table(evidence):
    data = read_json(evidence / "matrix-awakens/shader-inventory.json")
    if not data:
        return "No retail shader inventory recorded."
    independent = independent_counts(data)
    lines = ["RDNA 2 shader translation measured on Frosty",
             f"{'Shader stage':<16} {'Packaged':>9} {'HLSL / unique DXBC':>21} {'TuxBox only':>12} "
             f"{'In profile':>11} {'Profiled / requested':>23}"]
    for row in data["stages"]:
        emitted = row["hlsl_emitted"]
        hlsl = f"{emitted:,}/{row['unique_dxbc']:,}" if emitted is not None else "—"
        observed = f"{row['requested_in_profile']}/{row['requested_dxbc']}" if row["requested_dxbc"] else "—"
        lines.append(f"{row['stage']:<16} {row['global'] + row['material']:>9,} {hlsl:>21} "
                     f"{independent.get(row['stage'], '—'):>12} {row['profiled_dxbc']:>11} {observed:>23}")
    lines.append(f"The preparation profile includes {data['linked_variants']} additional linked variants.")
    lines.append(f"Stage table uses compiler snapshot {data['compiler_sha256'][:8]}; newer archive surveys follow.")
    lines.extend(shader_checks(evidence, data))
    for material in data.get("menu_materials", ()):
        requested = [f"{row['stage']} #{row['entry']}" for row in material["members"] if row["observed_in"]]
        matches = ", ".join(requested) or "none in these traces"
        lines.append(f"{Path(material['asset']).stem} (map {material['map']}, {len(material['members'])} entries) "
                     f"· request trace matches {matches}.")
    for shader in data["requested"]:
        if not shader["in_preparation_profile"]:
            where = ", ".join(shader["archive_entries"]) or "outside the indexed archives"
            frontend = {True: "HLSL emitted", False: "frontend failed", None: "frontend unmeasured"}[shader.get("hlsl_emitted")]
            if (validated := shader.get("standalone_dxil_validated")) is not None:
                frontend = "standalone DXIL passed" if validated else "standalone DXIL failed"
            lines.append(f"Unprofiled {shader['stage'].lower()} {shader['source_sha256']} ({where}, {frontend})")
    return "\n".join(lines)


def draw_shaders(svg, y, data, rows, material_hits):
    if not data:
        return y
    archive = data.get("archives", {}).get("material", {}).get("member_sha256")
    stages = material_hits.get("stages", {})
    hit_groups = [stages.get(kind, {}) for kind in ("opaque", "paired")]
    show_hits = (archive and material_hits.get("material_archive_sha256") == archive
                 and material_hits.get("result_set_sha256") and all(
                     type(stage.get("translated")) is int and type(stage.get("entries")) is int
                     and 0 <= stage["translated"] <= stage["entries"] for stage in hit_groups))
    height = 146 + 28*len(rows) + (28 if show_hits else 0)
    group = section(svg, PAD, y, "RDNA 2 shader translation")
    window(group, INNER, height, "~/docs/SHADERS.md")
    text(group, 18, 70, "RDNA 2 shader translation", 25, AMBER, bold=True)
    text(group, 18, 96, "Measured on Frosty’s shader archives", 15)
    text(group, 301, 121, "Compiler output", 13, MUTED)
    text(group, INNER-102, 121, "Archive entries", 13, MUTED, anchor="end")
    for index, (label, count, total, output, compiler) in enumerate(rows):
        top = 145 + 28*index
        cell = element(group, "g")
        element(cell, "title").text = f"{label} · compiler snapshot {compiler}"
        rect(cell, 18, top-18, INNER-36, 1, LINE)
        text(cell, 18, top, label, 15)
        text(cell, 301, top, "HLSL emitted" if output == "HLSL" else "DXIL compiled", 14)
        rect(cell, 510, top-10, 125, 6, LINE)
        rect(cell, 510, top-10, 125*count/total, 6, AQUA)
        text(cell, INNER-102, top, f"{count:,}/{total:,}", 14, TEXT, anchor="end")
        text(cell, INNER-18, top, f"{count/total:.1%}", 14, TEXT, anchor="end")
    if show_hits:
        opaque, paired = (f"{row['translated']:,}/{row['entries']:,}" for row in hit_groups)
        text(group, 18, height-17, f"Material hit groups emitting HLSL · {opaque} opaque · {paired} with any-hit", 14, MUTED)
    return y + height + GAP


def draw_projectscarlett_shaders(svg, y, evidence):
    run = read_json(evidence / "projectscarlett-shader-preparation.json")
    title = read_json(evidence / "projectscarlett-startup.json")
    if (not title.get("executable_sha256") or run.get("executable_sha256") != title["executable_sha256"]
            or not run.get("compiler_sha256") or not run.get("log_sha256")):
        return y
    counts = run.get("requests", {})
    total, compiled, unsupported, failed = (counts.get(key) for key in ("total", "compiled", "unsupported", "failed"))
    if (not all(type(n) is int and n >= 0 for n in (total, compiled, unsupported, failed))
            or not total or compiled + unsupported + failed != total):
        return y

    group = section(svg, PAD, y, "Project Scarlett shader preparation")
    share = compiled / total
    pipelines = run.get("mesh_pipelines", {})
    tested, created = pipelines.get("tested"), pipelines.get("created")
    gpu_result = (type(tested) is int and type(created) is int and 0 <= created <= tested <= compiled
                  and tested and pipelines.get("gpu") and pipelines.get("log_sha256"))
    play = read_json(evidence / "projectscarlett-gameplay.json")
    report = play.get("title_crash_report", {})
    frame, code = report.get("frame"), play.get("result", {}).get("exit_code")
    play_result = (play.get("executable_sha256") == title["executable_sha256"] and report.get("sha256")
                   and report.get("game_state") == "Game Running" and type(frame) is int and type(code) is int)
    height = 88 + 22*bool(gpu_result) + 22*bool(play_result)
    rect(group, 0, 0, INNER, height, PANEL, stroke=LINE)
    text(group, 18, 30, "Project Scarlett shader preparation", 23, AMBER, bold=True)
    text(group, INNER-18, 30, f"{share:.1%}", 23, AQUA, bold=True, anchor="end")
    rect(group, 18, 42, INNER-36, 7, LINE)
    rect(group, 18, 42, (INNER-36)*share, 7, AQUA)
    text(group, 18, 73, f"{compiled:,}/{total:,} captured requests compiled", 15)
    text(group, INNER-18, 73, f"{unsupported:,} unsupported · {failed:,} failed", 14, MUTED, anchor="end")
    if gpu_result:
        text(group, 18, 96, f"{created:,}/{tested:,} new mesh pipelines created on {pipelines['gpu']}", 14, MUTED)
    if play_result:
        outcome = f"then exited with code {code}" if code else "then exited"
        text(group, 18, height-14, f"Furthest run reached gameplay at frame {frame:,}, {outcome} · not yet playable",
             14, MUTED)
    return y+height+GAP


def draw_verification(svg, y, evidence):
    rows, _ = homebrew_matrix(evidence)
    if not rows:
        return y
    checks = [check for _, data in rows for check in data["checks"]]
    passed = sum(bool(check["passed"]) for check in checks)
    group = section(svg, PAD, y, "Guest regression checks")
    text(group, 0, 21, "Guest regression checks", 15, MUTED)
    text(group, INNER, 21, f"{passed} of {len(checks)} passed across {len(rows)} host configurations",
         15, MUTED, anchor="end")
    return y+28


def render_map(evidence):
    svg = ET.Element("svg", {"xmlns": "http://www.w3.org/2000/svg", "role": "img", "font-family": MONO,
                             "aria-labelledby": "map-title map-description"})
    element(svg, "title", id="map-title").text = "TuxBox runtime compatibility"
    element(svg, "desc", id="map-description").text = (
        "Overall coverage gives equal weight to verified Frosty library behaviors and shader translation. "
        "Project Scarlett shader preparation is measured on captured requests. "
        "Gameplay results identify the graphics settings used.")
    bg = rect(svg, 0, 0, WIDTH, 1, BG)
    rect(svg, 0, 0, WIDTH, 48, BAR)
    rect(svg, 0, 47, WIDTH, 1, LINE)
    rect(svg, PAD, 10, 210, 27, AMBER)
    text(svg, PAD+105, 29, "Runtime compatibility", 15, BG, bold=True, anchor="middle")
    brand = text(svg, WIDTH//2, 32, "TUXBOX", 22, AMBER, bold=True, anchor="middle")
    brand.set("letter-spacing", "4")
    text(svg, WIDTH-PAD, 30, "Xbox Series X|S", 16, AQUA, anchor="end")
    paths = load_library_paths(evidence).get("paths", [])
    inventory = read_json(evidence / "matrix-awakens/shader-inventory.json")
    shader_rows = list(shader_archive_results(evidence, inventory))
    y = draw_overall(svg, 48+GAP, paths, inventory, shader_rows)
    material_hits = read_json(evidence / "matrix-awakens/material-hit-hlsl-survey.json")
    y = draw_shaders(svg, y, inventory, shader_rows, material_hits)
    y = draw_projectscarlett_shaders(svg, y, evidence)
    y = draw_libraries(svg, y, paths)
    y = draw_frosty_target(svg, y, evidence)
    height = draw_verification(svg, y, evidence) + 6
    bg.set("height", str(height))
    svg.set("viewBox", f"0 0 {WIDTH} {height}")
    svg.set("width", str(WIDTH))
    svg.set("height", str(height))
    ET.indent(svg)
    return ET.tostring(svg, encoding="unicode") + "\n"


def write_map(evidence, path):
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(render_map(evidence), encoding="utf-8")
