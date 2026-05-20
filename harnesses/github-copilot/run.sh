#!/usr/bin/env sh
set -eu

workdir="${WORKDIR:-/workdir}"
prompt_file="${INITIAL_PROMPT_FILE:-/prompt/PROMPT.md}"

echo "github-copilot harness starting"
echo "workdir=${workdir}"
echo "prompt_file=${prompt_file}"
echo "llm_url=${LLM_URL:-}"
echo "llm_api_key_present=$([ -n "${LLM_API_KEY:-}" ] && echo yes || echo no)"

if [ ! -d "${workdir}" ]; then
  echo "missing workdir: ${workdir}" >&2
  exit 2
fi

# Configure copilot to use the orchestrator proxy
export COPILOT_OFFLINE=true
export COPILOT_PROVIDER_BASE_URL="${LLM_URL}"
export COPILOT_PROVIDER_TYPE="anthropic"
export COPILOT_PROVIDER_API_KEY="${LLM_API_KEY}"
export COPILOT_MODEL="gpt-4o"

echo "copilot_provider_base_url=${COPILOT_PROVIDER_BASE_URL}"
echo "copilot_provider_type=${COPILOT_PROVIDER_TYPE}"
echo "copilot_model=${COPILOT_MODEL}"

cd "${workdir}"
copilot --yolo --prompt "$(cat "${prompt_file}")"
