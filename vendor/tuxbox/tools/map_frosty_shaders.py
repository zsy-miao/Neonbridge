#!/usr/bin/env python3
"""Map shader stages, preparation coverage and observed runtime requests for retail Frosty."""
import argparse
import json
from pathlib import Path
import sys

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT))
from project_tuxbox.shader_inventory import inventory, use_frontend_survey
from project_tuxbox.shader_prep import FROSTY_PLAN, tuxbox_plan


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("mount", type=Path)
    parser.add_argument("--trace", metavar="LABEL=LOG", action="append", default=[],
                        help="capture from a retail run with TUXBOX_TRACE_SHADERS=1; repeatable")
    frontend = parser.add_mutually_exclusive_group()
    frontend.add_argument("--compiler", type=Path, help="survey the checkout compiler's HLSL output on each unique DXBC")
    frontend.add_argument("--reuse-survey", type=Path, metavar="JSON",
                          help="reuse per-stage frontend measurements from a matching archive inventory")
    parser.add_argument("--independent-compiler", type=Path,
                        help="check a build without legacy-hlsl against the regular compiler on every distinct program")
    parser.add_argument("--dxc", type=Path, help="with --compiler, compile observed sources and validate DXIL with the sibling dxv")
    parser.add_argument("--index", type=Path, help="write a CSV with every shipped entry and shader map membership")
    parser.add_argument("--output", type=Path, required=True, help="write a JSON summary and observed source list")
    args = parser.parse_args()
    if (args.dxc or args.independent_compiler) and not args.compiler:
        parser.error("--dxc and --independent-compiler require --compiler")
    traces = {}
    for value in args.trace:
        name, separator, path = value.partition("=")
        if not separator or not name or not path or name in traces:
            parser.error("Each --trace needs a unique LABEL=LOG")
        traces[name] = Path(path)
    try:
        plan, options = tuxbox_plan(FROSTY_PLAN, json.loads(FROSTY_PLAN.read_text()), True)
        material_map = json.loads((ROOT / "evidence/matrix-awakens/menu-materials.json").read_text())
        if material_map["title_sha256"] != plan["executable_sha256"]:
            raise ValueError("Menu materials belong to a different build")
        report = inventory(args.mount, plan, options.get("linked_shaders", []), traces, args.compiler,
                           args.index, material_map["materials"], args.dxc, args.independent_compiler)
        if args.reuse_survey:
            use_frontend_survey(report, json.loads(args.reuse_survey.read_text()))
        args.output.parent.mkdir(parents=True, exist_ok=True)
        args.output.write_text(json.dumps(report, indent=2) + "\n", encoding="utf-8")
    except (OSError, ValueError, RuntimeError) as error:
        parser.exit(1, f"Shader inventory failed: {error}\n")


if __name__ == "__main__":
    main()
