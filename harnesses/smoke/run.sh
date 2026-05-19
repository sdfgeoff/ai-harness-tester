#!/usr/bin/env sh
set -eu

workdir="${WORKDIR:-/workdir}"
prompt_file="${INITIAL_PROMPT_FILE:-/prompt/PROMPT.md}"

echo "smoke harness starting"
echo "workdir=${workdir}"
echo "prompt_file=${prompt_file}"

if [ ! -d "${workdir}" ]; then
  echo "missing workdir: ${workdir}" >&2
  exit 2
fi

echo "smoke harness saw workdir" > "${workdir}/smoke-workdir-seen.txt"

if [ ! -f "${prompt_file}" ]; then
  echo "missing prompt file: ${prompt_file}" >&2
  exit 3
fi

{
  echo "smoke harness completed"
  echo "prompt_sha256_unavailable_in_v0_harness"
  echo "initial_files:"
  find "${workdir}" -maxdepth 2 -type f | sort
} > "${workdir}/smoke-harness-output.txt"

echo "smoke harness wrote ${workdir}/smoke-harness-output.txt"
