#!/usr/bin/env sh
set -eu

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"

for dir in "$SCRIPT_DIR"/harnesses/*/; do
    name="$(basename "$dir")"
    docker build -t "harness-test/${name}:latest" "$dir"
done
