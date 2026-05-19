#!/usr/bin/env sh
set -eu

workdir="${WORKDIR:-/workdir}"
prompt_file="${INITIAL_PROMPT_FILE:-/prompt/PROMPT.md}"

echo "opencode harness starting"
echo "workdir=${workdir}"
echo "prompt_file=${prompt_file}"
echo "llm_url=${LLM_URL:-}"
echo "llm_api_key_present=$([ -n "${LLM_API_KEY:-}" ] && echo yes || echo no)"

if [ ! -d "${workdir}" ]; then
  echo "missing workdir: ${workdir}" >&2
  exit 2
fi

# Generate opencode config pointing at the orchestrator proxy
config_dir="$HOME/.config/opencode"
mkdir -p "${config_dir}"

cat > "${config_dir}/opencode.json" <<EOF
{
  "\$schema": "https://opencode.ai/config.json",
  "provider": {
    "orchestrator": {
      "npm": "@ai-sdk/openai-compatible",
      "name": "Orchestrator Proxy",
      "options": {
        "baseURL": "${LLM_URL}/v1",
        "apiKey": "${LLM_API_KEY}"
      },
      "models": {
        "gpt-4o": {
          "name": "gpt-4o"
        }
      }
    }
  },
  "model": "orchestrator/gpt-4o"
}
EOF

echo "wrote opencode config to ${config_dir}/opencode.json"

cd "${workdir}"
opencode run "$(cat "${prompt_file}")"
