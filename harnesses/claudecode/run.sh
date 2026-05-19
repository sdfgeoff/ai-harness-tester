#!/usr/bin/env sh
set -eu

workdir="${WORKDIR:-/workdir}"
prompt_file="${INITIAL_PROMPT_FILE:-/prompt/PROMPT.md}"

echo "claude-code harness starting"
echo "workdir=${workdir}"
echo "prompt_file=${prompt_file}"
echo "llm_url=${LLM_URL:-}"
echo "llm_api_key_present=$([ -n "${LLM_API_KEY:-}" ] && echo yes || echo no)"

if [ ! -d "${workdir}" ]; then
  echo "missing workdir: ${workdir}" >&2
  exit 2
fi

# Set environment variables for claude-code to use the orchestrator proxy
export ANTHROPIC_AUTH_TOKEN="ollama"
export ANTHROPIC_API_KEY="${LLM_API_KEY}"
export ANTHROPIC_BASE_URL="${LLM_URL}"

echo "anthropic_auth_token=ollama"
echo "anthropic_api_key_present=yes"
echo "anthropic_base_url=${ANTHROPIC_BASE_URL}"

cd "${workdir}"
claude "$(cat "${prompt_file}")"
