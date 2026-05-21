#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
interface_dir="${repo_root}/interface"

if [[ ! -d "${interface_dir}" ]]; then
    echo "error: interface/ directory not found" >&2
    exit 1
fi

cd "${interface_dir}"

if [[ ! -d node_modules ]]; then
    echo "Installing interface dependencies..."
    npm install
fi

exec npm run dev -- --host 127.0.0.1
