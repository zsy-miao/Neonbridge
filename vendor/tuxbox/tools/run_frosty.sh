#!/usr/bin/env bash
# Launch the standalone layout created by prepare_frosty.py.
set -euo pipefail
repo=$(cd -- "$(dirname -- "$0")/.." && pwd)
launcher="$repo/tools/run_frosty.sh"
pointer="$repo/build/frosty-native/latest-independent"
verify_layout=yes
replay_options=()
extra_variables=()
profile_pipelines=false
selected_work=
arguments=()
while (( $# )); do
    case $1 in
        --nanite) echo "The --nanite override has been retired. Using the game's graphics settings." >&2 ;;
        --profile-pipelines) profile_pipelines=true ;;
        --work)
            if (( $# < 2 )); then echo "--work requires a prepared working directory" >&2; exit 2; fi
            selected_work=$(realpath -e -- "$2")
            shift ;;
        *) arguments+=("$1") ;;
    esac
    shift
done
set -- "${arguments[@]}"
if [[ -n $selected_work ]]; then replay_options+=(--work "$selected_work"); fi
if [[ $profile_pipelines == true ]]; then replay_options+=(--profile-pipelines); fi
# Bridge destruction exceeded 12 GiB while both city and finale assets were live.
desktop_limits=(--property=MemoryHigh=13G --property=MemoryMax=15G --property=MemorySwapMax=2G)
source "$repo/tools/frosty_session.sh"
