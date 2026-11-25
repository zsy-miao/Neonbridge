"""Reproduce shader and pipeline preparation for the historical reference renderer."""
import json
from pathlib import Path
import sys

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT))
from project_tuxbox.host import Host
from project_tuxbox.inspect import digest, launch_target
from project_tuxbox.pipeline_prep import prepare_pipelines
from project_tuxbox.prep_progress import desktop_progress
from project_tuxbox.shader_prep import FROSTY_PLAN, GUEST_TEMP, prepare_shaders


def prepare(work):
    plan = json.loads(FROSTY_PLAN.read_text())
    plan.pop("source_cache")
    layout = Path((work / "stage").read_text().strip())
    executable = launch_target(layout, None)
    if digest(executable) != plan["executable_sha256"] or digest(executable.with_name(plan["provider"])) != plan["provider_sha256"]:
        raise ValueError("This comparison requires the recorded game and reference renderer hashes")
    prefix = work / "prefix"
    host = Host("proton", Path((work / "proton").read_text().strip()), prefix)
    cache = prefix / GUEST_TEMP / "tombox_shader_cache/by_hlsl"
    with desktop_progress(plan["title"], host.env) as progress:
        prepare_shaders(executable.parent / plan["shader_package"], plan, cache, progress)
        prepare_pipelines(executable, host, plan, cache, progress)


if __name__ == "__main__":
    try:
        prepare(Path(Path(sys.argv[1]).read_text().strip()))
    except (ValueError, OSError) as error:
        sys.exit(str(error))
