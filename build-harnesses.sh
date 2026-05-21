#!/usr/bin/env sh
set -eu

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
failures=""

for dir in "$SCRIPT_DIR"/harnesses/*/; do
    [ -d "$dir" ] || continue
    name="$(basename "$dir")"
    if ! docker build -t "harness-test/${name}:latest" "$dir"; then
        failures="${failures}
- harness:${name}"
    fi
done

for dir in "$SCRIPT_DIR"/tests/*/; do
    [ -d "$dir" ] || continue
    name="$(basename "$dir")"
    dockerfile="${dir%/}/evaluate.Dockerfile"
    if [ ! -f "$dockerfile" ]; then
        echo "missing evaluator dockerfile for test '${name}': $dockerfile" >&2
        failures="${failures}
- evaluator:${name} missing evaluate.Dockerfile"
        continue
    fi

    if ! docker build -t "harness-test-evaluator/${name}:latest" -f "$dockerfile" "$dir"; then
        failures="${failures}
- evaluator:${name}"
    fi
done

if [ -n "$failures" ]; then
    echo "Build failures:" >&2
    printf '%s\n' "$failures" >&2
    exit 1
fi
