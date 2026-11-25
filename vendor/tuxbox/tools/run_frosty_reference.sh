#!/usr/bin/env bash
# Historical comparison layouts have their own launch entry point.
set -euo pipefail
repo=$(cd -- "$(dirname -- "$0")/.." && pwd)
launcher="$repo/tools/run_frosty_reference.sh"
pointer="$repo/build/frosty-native/latest-graphics-bringup"
verify_layout=no
replay_options=()
extra_variables=(TOMBOX_LOG)
export TOMBOX_LOG=${TOMBOX_LOG:-NUL}
# Historical layouts can select a different renderer and still need console CBV offsets.
export TUXBOX_REFERENCE_CBV=1
if [[ ${1:-} == --tuxbox-d3d12 ]]; then
    shift
    pointer="$repo/build/frosty-native/latest-tuxbox-d3d12"
    replay_options+=(--tuxbox-d3d12)
    export TUXBOX_SHADER_PREP=${TUXBOX_SHADER_PREP:-menu}
elif [[ ${1:-} == --prepare-only ]]; then
    exec python "$repo/tools/reference/prepare_comparison.py" "$pointer"
elif [[ ${1:-} != --desktop && ${1:-} != --no-shader-prep && ${TUXBOX_SHADER_PREP:-1} != 0 && " $* " != *" -nullrhi "* ]]; then
    python "$repo/tools/reference/prepare_comparison.py" "$pointer" || echo "Comparison shader preparation incomplete." >&2
fi
desktop_limits=(--property=MemoryHigh=8G --property=MemoryMax=11G --property=MemorySwapMax=2G)
source "$repo/tools/frosty_session.sh"
