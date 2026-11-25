#!/usr/bin/env bash
set -eu
exec python3 "$(dirname "$0")/../build.py" route "$@"
