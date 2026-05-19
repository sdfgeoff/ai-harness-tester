#!/usr/bin/env sh
set -eu

workdir="${WORKDIR:-/workdir}"
prompt_file="${INITIAL_PROMPT_FILE:-/prompt/PROMPT.md}"

echo "pi harness starting"
echo "workdir=${workdir}"
echo "prompt_file=${prompt_file}"
echo "llm_url=${LLM_URL:-}"
echo "llm_api_key_present=$([ -n "${LLM_API_KEY:-}" ] && echo yes || echo no)"

if [ ! -d "${workdir}" ]; then
  echo "missing workdir: ${workdir}" >&2
  exit 2
fi

# Generate pi agent config pointing at the orchestrator proxy
config_dir="$HOME/.pi/agent"
mkdir -p "${config_dir}"

cat > "${config_dir}/models.json" <<EOF
{
  "providers": {
    "orchestrator": {
      "baseUrl": "${LLM_URL}/v1",
      "api": "openai-completions",
      "apiKey": "${LLM_API_KEY}",
      "models": [
        { "id": "gpt-4o" }
      ]
    }
  }
}
EOF

echo "wrote pi config to ${config_dir}/models.json"

cd "${workdir}"
pi -p "$(cat "${prompt_file}")"
