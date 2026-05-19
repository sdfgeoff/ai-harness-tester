# Harness Test

Harness Test compares coding harnesses against the same local model by running each harness in Docker, routing LLM traffic through an orchestrator proxy, and preserving run artifacts.

The current implementation target is v0. v0 is complete when the smoke harness and smoke test can run through the final CLI and produce the expected batch/run artifacts.

## Smoke Target

Build the smoke harness:

```sh
./build-harnesses.sh
```

Intended final smoke command:

```sh
orchestrator run --tests smoke --harnesses smoke --models smoke-local --config config.json
```

Expected final artifact shape:

```text
results/
  <batch_id>/
    summary.json
    config.json
    runs/
      <run_id>/
        results.json
        working_dir/
        logs/
          harness.log
          proxy.ndjson
        PROMPT.md
```

## Smoke Test

The smoke test lives in `tests/smoke/` and contains:

- `initial_state.zip`
- `PROMPT.md`

The harness image is built from `harnesses/smoke/Dockerfile` and tagged as `harness-test/smoke:latest`.

## Development

Check the Rust workspace:

```sh
cargo check
cargo test
```

Show CLI help:

```sh
cargo run -p orchestrator-cli -- --help
```

Run a Docker image directly:

```sh
cargo run -p orchestrator-cli -- run-image --harnesses smoke --models smoke-local --tests smoke --config config.json
```

Run an image with a selected test:

```sh
cargo run -p orchestrator-cli -- run-image --harnesses smoke --models smoke-local --tests smoke --config config.json
```

At this stage the command runs the image, reports its exit status, and writes minimal run artifacts. Later tickets add working directory mounts, prompt handling, and proxy wiring.

The command writes a minimal run artifact:

```text
results/
  <batch_id>/
    config.json
    summary.json
    runs/
      <run_id>/
        logs/
          harness.log
        working_dir/
        PROMPT.md
        results.json
```

At this stage `results.json` records run ID, status, timestamps, duration, harness exit code, and the relative harness log path. Harness stdout and stderr are captured together in `logs/harness.log` and streamed live to the console with the run ID as a prefix.

When `--tests <name>` is provided, the CLI validates `tests/<name>/initial_state.zip` and `tests/<name>/PROMPT.md`, then records SHA-256 hashes for both files in `results.json`. The `--tests`, `--harnesses`, and `--models` flags accept comma-separated values.

Selected combinations run sequentially in test/model/harness order. A failed harness run is preserved and does not stop later selected combinations. The CLI exits non-zero after the batch if any run failed.

Before creating a batch directory, the CLI validates selected test folders, required test files, harness profile names, model profile names, local Docker image availability, and upstream model availability. For each selected model profile, the CLI calls `<base_url>/models` with the configured API key and requires `model_name` to appear in the response. These preflight failures are configuration errors and do not produce result artifacts.

The selected test archive is extracted into `working_dir` at the root of the run artifact. Archive entries with absolute paths, `..` path traversal, or symlinks are rejected before Docker starts.

When a test is selected, `working_dir` is mounted into the container read-write at `/workdir`, the container working directory is set to `/workdir`, and `WORKDIR=/workdir` is provided in the environment.

The selected prompt is copied to a temporary file outside `working_dir`, mounted read-only as `/prompt/PROMPT.md`, and exposed as `INITIAL_PROMPT_FILE=/prompt/PROMPT.md`. After the run, the same temporary prompt is copied into the run artifact as `PROMPT.md` and the temporary file is removed.

Each harness container receives this environment contract:

```text
WORKDIR=/workdir
INITIAL_PROMPT_FILE=/prompt/PROMPT.md
LLM_URL=<per-run proxy URL>
LLM_API_KEY=<per-run proxy API key>
```

The per-run proxy starts before Docker and shuts down after the harness run finishes.

Harnesses are selected by name from `config.json`:

```json
{
  "models": {
    "smoke-local": {
      "model_name": "smoke-local",
      "base_url": "http://localhost:11434/v1",
      "api_key": "local"
    }
  },
  "harnesses": {
    "smoke": {
      "image": "harness-test/smoke:latest"
    }
  }
}
```

The selected model profile is recorded in `results.json` with non-secret resolved values only. The model API key is not written to run artifacts.

Each CLI invocation creates a UTC batch directory under `results/`. Run IDs include the batch timestamp, harness, model, test, and a short suffix. A redacted copy of the launch config is written to `results/<batch_id>/config.json`.

After all selected runs finish, `summary.json` is written at the batch root. It contains batch timing, the config path, and run references only:

```json
{
  "batch_id": "20260519T020329Z",
  "started_at": "2026-05-19T02:03:29Z",
  "finished_at": "2026-05-19T02:03:32Z",
  "duration_ms": 3121,
  "config_path": "config.json",
  "runs": [
    {
      "run_id": "20260519T020329Z_smoke_smoke-local_smoke_9341d8",
      "results_path": "runs/20260519T020329Z_smoke_smoke-local_smoke_9341d8/results.json"
    }
  ]
}
```

## Proxy

The `llm-proxy` crate provides the in-process per-run proxy. It starts on a random local port, generates a per-run API key, requires bearer auth, serves `GET /v1/models` with a minimal response containing only the selected model, and forwards non-streaming `POST /v1/responses` requests upstream. For `/v1/responses`, the proxy rewrites `model` to the selected model profile's `model_name` and preserves other request fields.

### Proxy Log Format

Each run writes an append-only NDJSON log at `logs/proxy.ndjson`. Every proxied request produces linked `request_start` and `request_end` records identified by a shared `request_id`.

**`request_start`** — written when the proxy receives a request:

```json
{
  "record_type": "request_start",
  "request_id": "...",
  "started_at": "2026-05-19T09:16:00Z",
  "kind": "generation",
  "method": "POST",
  "path": "/v1/responses",
  "original_model": "gpt-4o",
  "upstream_model": "qwen2.5-coder:32b",
  "request_body": {}
}
```

- `kind` is `"generation"` for `/v1/responses` and `"discovery"` for `/v1/models`.
- `request_body` is present only for authorized requests. Auth failures omit it.
- `request_body` contains the payload *after* model rewrite (i.e., `model` is the upstream model).

**`request_end`** — written when the upstream response is received or an error occurs:

```json
{
  "record_type": "request_end",
  "request_id": "...",
  "finished_at": "2026-05-19T09:16:05Z",
  "duration_ms": 5000,
  "kind": "generation",
  "method": "POST",
  "path": "/v1/responses",
  "original_model": "gpt-4o",
  "upstream_model": "qwen2.5-coder:32b",
  "status_code": 200,
  "response_body": {},
  "usage": {
    "input_tokens": 100,
    "output_tokens": 50,
    "total_tokens": 150,
    "cache_read_tokens": null,
    "cache_write_tokens": null
  },
  "error": null
}
```

- `usage` is extracted from the upstream response's `usage` object when present.
- `error` is `null` on success, or a descriptive string on failure (e.g., connection error, auth failure).
- `/v1/models` discovery requests are logged but excluded from generation metrics.
