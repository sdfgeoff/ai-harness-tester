# Decisions

## Benchmark Run Artifacts

- Each orchestrator invocation creates a batch result directory.
- Each batch gets a `batch_id` based on the orchestrator launch timestamp.
- Each benchmark execution gets a unique `run_id`.
- Results are stored under `results/<batch_id>/runs/<run_id>/`.
- The unpacked test initial state becomes the run's persistent `working_dir`.
- The `working_dir` is the primary artifact used for evaluation.
- The orchestrator writes a single `results.json` for each run.
- Metrics include upstream-reported token usage and request count.
- Model, harness, test, versions, and other run-identifying details should be stored as metadata rather than relying only on the directory path.
- Run IDs should be human-readable, using timestamp, harness, model, test, and a short random suffix.
- The timestamp in each run ID uses UTC.
- The timestamp is the orchestrator launch time, shared by all runs started by that orchestrator invocation so related runs group together naturally.
- The same launch timestamp forms the batch ID.

Example run ID:

```text
20260519T091530Z_codex_qwen_fix-python-bug_a1b2c3
```

Recommended baseline layout:

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

Batch `summary.json`:

- Contains batch-level fields such as `batch_id`, `started_at`, `finished_at`, and `duration_ms`.
- Avoids duplicating per-run metrics and metadata.
- Contains run references only, including each `run_id` and path to that run's `results.json`.
- Includes references for `setup_failed` runs when a per-run setup failure is recorded after the batch starts.

Batch config snapshot:

- The orchestrator writes a redacted copy of the launch config to `results/<batch_id>/config.json`.
- Secrets such as API keys are not written to the batch config snapshot.

## Data Models

These schemas are the working v0 data models. Update this section when changing persisted JSON shapes.

### `config.json`

```json
{
  "timeout_seconds": 1800,
  "results_dir": "results",
  "models": {
    "qwen-coder-32b": {
      "model_name": "qwen2.5-coder:32b",
      "base_url": "http://localhost:11434/v1",
      "api_key": "local"
    }
  },
  "harnesses": {
    "codex": {
      "image": "harness-test/codex:latest"
    }
  }
}
```

### Batch `summary.json`

```json
{
  "batch_id": "20260519T091530Z",
  "started_at": "2026-05-19T09:15:30Z",
  "finished_at": "2026-05-19T09:45:00Z",
  "duration_ms": 1770000,
  "config_path": "config.json",
  "runs": [
    {
      "run_id": "20260519T091530Z_codex_qwen-coder-32b_fix-python-bug_a1b2c3",
      "results_path": "runs/20260519T091530Z_codex_qwen-coder-32b_fix-python-bug_a1b2c3/results.json"
    }
  ]
}
```

### Run `results.json`

```json
{
  "run_id": "20260519T091530Z_codex_qwen-coder-32b_fix-python-bug_a1b2c3",
  "batch_id": "20260519T091530Z",
  "status": "completed",
  "harness_exit_code": 0,
  "error": null,
  "started_at": "2026-05-19T09:15:30Z",
  "finished_at": "2026-05-19T09:45:00Z",
  "duration_ms": 1770000,
  "selection": {
    "test": "fix-python-bug",
    "model": "qwen-coder-32b",
    "harness": "codex"
  },
  "inputs": {
    "initial_state_sha256": "...",
    "prompt_sha256": "..."
  },
  "resolved": {
    "model": {
      "model_name": "qwen2.5-coder:32b",
      "base_url": "http://localhost:11434/v1"
    },
    "harness": {
      "image": "harness-test/codex:latest",
      "image_id": "sha256:..."
    }
  },
  "metrics": {
    "request_count": 12,
    "input_tokens": 10000,
    "output_tokens": 3000,
    "total_tokens": 13000,
    "cache_read_tokens": null,
    "cache_write_tokens": null
  },
  "artifacts": {
    "working_dir": "working_dir",
    "prompt": "PROMPT.md",
    "harness_log": "logs/harness.log",
    "proxy_log": "logs/proxy.ndjson"
  }
}
```

For non-completed runs, `error` is a short object:

```json
{
  "kind": "timeout",
  "message": "Run exceeded timeout of 1800 seconds"
}
```

### Proxy `logs/proxy.ndjson`

Proxy logs are append-only NDJSON records. A proxied request may produce multiple records linked by `request_id`.

Request start records include the request body:

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

Request end records include response outcome and usage:

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
  "response_body": null,
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

For non-streaming responses, `response_body` contains the upstream JSON response. For streaming responses, `response_body` is `null` in v0 and streamed content is available only through `stream_event` records.

Streaming events are stored as separate records:

```json
{
  "record_type": "stream_event",
  "request_id": "...",
  "received_at": "2026-05-19T09:16:01Z",
  "event": "response.output_text.delta",
  "data_raw": "{\"type\":\"response.output_text.delta\"}"
}
```

Stream event `data` is stored as raw SSE `data:` text only in v0, not parsed JSON.

## Container Write Boundary

- The harness container may write anywhere inside its disposable filesystem.
- Only the run `working_dir` and explicitly captured logs/metrics are preserved after the run.
- Evaluation is based only on `working_dir`.

## Harness Container Contract

- Each harness lives in its own Dockerfile/image definition.
- Harness-specific installation, configuration, credentials wiring, and CLI invocation are handled inside that harness image.
- The orchestrator launches the harness container using the image's configured command/entrypoint; whether that is `run.sh` or something else is up to the Dockerfile.
- The orchestrator should treat harnesses uniformly by running their container with the agreed environment variables, rather than knowing harness-specific CLI details.
- The model name is not exposed to the harness container. The orchestrator proxy performs model routing.
- The run ID is not exposed to the harness container.

Minimum environment contract:

```text
WORKDIR              /workdir
INITIAL_PROMPT_FILE  /prompt/PROMPT.md
LLM_URL              base URL of the orchestrator proxy
LLM_API_KEY          API key accepted by the orchestrator proxy
```

Mount behavior:

- The run `working_dir` is mounted read-write at `WORKDIR`.
- The test `PROMPT.md` is mounted read-only at `INITIAL_PROMPT_FILE`.
- Only `PROMPT.md` and the run `working_dir` are mounted into the harness container for v0.
- The full test folder is not mounted into the harness container.
- The orchestrator sets the container process working directory to `/workdir`.
- v0 does not pass additional Docker run flags beyond the required mounts, environment variables, working directory, container name, network access, and removal/cleanup behavior.

## Proxy API Surface

- The orchestrator proxy initially supports the OpenAI-compatible `/v1/responses` endpoint.
- The proxy also supports `GET /v1/models` for harness model discovery.
- For v0, proxy `GET /v1/models` returns a minimal models list containing only the selected run model.
- Additional provider dialects or endpoints will be added only when required by a specific harness.
- Harness images should adapt their harness configuration to the currently supported proxy API surface where practical.

## Upstream Model Configuration

- The actual local LLM endpoint and model selection are configured in the orchestrator, not exposed directly to harness containers.
- The orchestrator is written in Rust.
- The default orchestrator config file is `config.json` at the repo root.
- The config path can be overridden with `--config`.
- The initial model config contains only:
  - `model_name`
  - `api_key`
  - `base_url`
- The proxy rewrites/routes harness requests to the configured upstream model.
- The proxy always rewrites the request model to the selected model profile's `model_name`.
- The original model name requested by the harness is preserved in `proxy.ndjson`.
- Other request fields are preserved and forwarded unchanged.
- Request overrides such as temperature or top-p are not part of the initial design.
- Model config values should be captured in run `results.json`.

## Rust Workspace Shape

- The project should use a Cargo workspace.
- v0 workspace crates:
  - `orchestrator-cli`
  - `orchestrator-core`
  - `llm-proxy`
  - `docker-runner`
- `orchestrator-cli` owns argument parsing, command dispatch, and user-facing console output.
- `orchestrator-core` owns config loading, preflight, matrix expansion, run lifecycle, result paths, data models, and summary/results writing.
- `llm-proxy` owns the per-run HTTP proxy, auth, model rewrite, proxy logging, and usage extraction.
- `docker-runner` owns Docker CLI abstraction, image inspection, container execution/cleanup, timeout handling, and harness log capture.
- `llm-proxy` is a library crate started in-process by the orchestrator for each run, not a separate binary process in v0.
- v0 uses Tokio for async coordination across the in-process proxy, Docker process/log handling, and timeout handling.
- `llm-proxy` uses Axum for the HTTP server and Reqwest for upstream HTTP calls.
- The existing reference project at `/home/geoffrey/Reference/OffTopic/llm-proxy` should be used as an implementation reference for Axum/Reqwest proxying and streaming tee behavior.
- This project should not adopt the reference proxy's SQLite database, encrypted payload archive, dashboard, or missing-token estimation for v0.
- v0 uses `tracing` for internal structured logs, with simple human-readable CLI output.
- Dependency choices should be pragmatic: use established crates when they remove real complexity, but avoid unnecessary dependencies and over-engineering for v0.
- `anyhow` is acceptable for application-level error handling.
- Persisted config/results data models live in `orchestrator-core` for v0.
- Do not add a separate shared types crate for v0.

## Token Accounting

- Token usage should come from upstream LLM usage reporting.
- The orchestrator/proxy should not perform local token estimation in the initial design.
- Metrics should clearly represent upstream-reported usage rather than inferred usage.
- v0 run metrics are limited to request count and upstream-reported token/cache fields.
- `request_count` includes all `/v1/responses` generation requests logged by the proxy, regardless of whether the upstream request succeeds.
- `request_count` does not include `/v1/models` discovery requests.
- Aggregate token/cache fields are `null` unless all logged requests provide enough upstream usage data for that field.
- Cache token fields are normalized only when upstream usage fields have clearly equivalent meaning.
- Raw upstream usage is preserved in `proxy.ndjson`.

## Streaming Responses

- The proxy must support streaming responses from the start.
- Streaming chunks should be forwarded to the harness while the proxy records request timing and response metadata.
- For streaming `/v1/responses`, proxy logs store SSE events verbatim as separate NDJSON records.
- Stream event records are linked to request lifecycle records by `request_id`.
- v0 does not reconstruct streaming responses into a final `response_body`.
- Token usage for streamed requests should be captured only when upstream reports it.
- If upstream streaming does not report usage, token usage for that request should be recorded as missing rather than estimated.
- The proxy may parse raw stream data internally to extract upstream usage for metrics.
- Parsed stream event data is not stored in v0 proxy logs.

## Runtime Network Access

- Harness containers may use the network at runtime.
- The benchmark is intended to mimic real-world harness usage rather than complete isolation.
- The orchestrator still routes LLM calls through its proxy for metrics and model routing.
- The initial design does not include network tracking, network policy config, or egress metrics.

## Run Timeout

- The only initial execution limit is a wall-clock timeout for the whole harness run.
- The timeout is configured at the orchestrator level.
- When the timeout expires, the orchestrator stops/kills the harness container.
- Timed-out runs are explicitly recorded with a timeout status.
- Partial metrics, logs, and `working_dir` contents are preserved on timeout.

## Prompt Delivery

- The orchestrator passes the initial prompt as a file path only, via `INITIAL_PROMPT_FILE`.
- The orchestrator does not pass prompt content through stdin, environment variables, or CLI arguments.
- The harness image/entrypoint is responsible for reading and adapting the prompt file for that harness.

## Test Folder Structure

- Tests are self-contained folders.
- The required initial test structure is:

```text
tests/
  <test_name>/
    initial_state.zip
    PROMPT.md
```

- A per-test `metadata.json` may be added later, but is not part of the initial required contract.
- Evaluation instructions such as `EVALUATION.md` are deferred until after v0.

## Initial State Extraction

- `initial_state.zip` contents are extracted directly into the root of the run `working_dir`.
- The extracted project/repo root is therefore `WORKDIR`.
- The orchestrator should reject unsafe archive entries, including absolute paths and paths that escape `working_dir` via `..`.
- The orchestrator does not initialize a git repository or otherwise add git state to `working_dir`.

## v0 Outputs

- v0 focuses on preserving run artifacts and metrics.
- v0 does not perform LLM-as-judge evaluation.
- v0 does not perform deterministic test/check execution as part of evaluation.
- v0 does not track tool calls.
- v0 should include one minimal example harness and one minimal example test as smoke coverage.
- Result artifact schemas do not need backward compatibility during v0 development.
- v0 targets this development machine only; do not spend effort on cross-platform or Docker Desktop compatibility yet.
- Evaluation and metrics analysis will be added later.

## v0 Run Results

- Token counts come from upstream usage reporting.
- Cache token fields are `null` when upstream does not report them.
- `harness_exit_code` is recorded when available and is `null` for statuses where no harness exit code exists.
- `results.json` includes a short structured `error` object for failed, timed-out, and setup-failed runs.
- Detailed harness output remains in `logs/harness.log`.
- `results.json` should include the selected test, model profile, and harness profile names.
- `results.json` should include a snapshot of resolved non-secret config values for the run, such as upstream `model_name`, upstream `base_url`, and harness image.
- `results.json` should include the resolved harness Docker image ID/digest, not only the configured image tag.
- `results.json` should include SHA-256 hashes of `initial_state.zip` and `PROMPT.md`.
- `initial_state.zip` is not copied into each run artifact for v0.
- The run artifact should include a copy of the test `PROMPT.md`, but only after the harness run finishes.
- The copied artifact `PROMPT.md` is not mounted into the harness container.
- During the run, the orchestrator creates a temporary prompt file outside both `working_dir` and the final artifact location.
- The temporary prompt file is mounted read-only at `/prompt/PROMPT.md`.
- After the run, the final artifact `PROMPT.md` is copied from the same temporary prompt file the harness saw.
- The temporary prompt file is deleted after the run.
- Secret values such as upstream API keys and generated proxy API keys are not written to `results.json`.

## Proxy Logs

- v0 stores full proxy request/response traffic for debugging and later analysis.
- Proxy logs are written under each run's `logs/` directory, for example `logs/proxy.ndjson`.
- Logs should include request timing, path, status, request body, response body, streamed events, and upstream usage where available.
- `/v1/models` traffic is logged in `proxy.ndjson` but excluded from generation metrics.
- Proxy log records include a `kind` field, such as `generation` for `/v1/responses` and `discovery` for `/v1/models`.
- Proxy auth failures are logged without request bodies.
- Unauthenticated requests are excluded from generation metrics.
- Full proxy logs may contain sensitive prompt, code, or response data.

## Harness Logs

- Harness stdout and stderr are captured together.
- Combined harness output is stored at `logs/harness.log` for each run.
- Harness output is also streamed live to the orchestrator console with a run-identifying prefix.

## Metrics Generation

- `logs/proxy.ndjson` is the source of truth for LLM request metrics.
- The proxy writes append-only log records as requests complete.
- The orchestrator writes run status and timing information.
- At the end of a run, the orchestrator aggregates raw logs and run status into the run's `results.json`.
- If aggregation logic changes later, the metrics portion of `results.json` should be regenerable from `proxy.ndjson` plus the non-metric run fields in `results.json`.

## Batch Execution

- The orchestrator should support launching runs across multiple harnesses, tests, and models.
- v0 executes those runs sequentially rather than in parallel.
- Parallel execution and scheduling are deferred.
- A failed or timed-out run does not stop the rest of the batch.
- The orchestrator continues to the next selected run after preserving the failed/timed-out run's artifacts.
- Sequential matrix order is test outermost, then model, then harness.

Example order:

```text
test A / model 1 / harness 1
test A / model 1 / harness 2
test A / model 2 / harness 1
test A / model 2 / harness 2
test B / model 1 / harness 1
```

## Orchestrator Exit Codes

- The orchestrator exits non-zero for preflight/configuration errors.
- The orchestrator exits non-zero if any run ends with a status other than `completed`.
- The orchestrator exits zero only when all selected runs complete successfully.

## Run Status Semantics

- Run status describes execution outcome, not task correctness or evaluation score.
- Harness container exit code `0` maps to `completed`.
- Harness container non-zero exit code maps to `failed`.
- Wall-clock timeout maps to `timed_out`.
- Pre-harness per-run setup error maps to `setup_failed`.
- Proxy HTTP errors do not directly determine run status.
- If the harness exits `0`, the run status is `completed` even if proxy HTTP errors occurred.
- Proxy errors are recorded in proxy logs.
- Future evaluation/scoring should be represented independently from execution status.

## Preflight Validation

- The orchestrator validates selected tests, model profiles, harness profiles, and required files before starting the batch.
- Missing or invalid `initial_state.zip` is a test configuration error, not a benchmark result.
- The orchestrator verifies selected harness Docker images exist locally before starting the batch.
- Missing harness images are setup/configuration errors, not benchmark results.
- The orchestrator checks each selected model profile by calling the upstream `/models` endpoint.
- The configured `model_name` must be available in the upstream `/models` response.
- Missing or unreachable models are setup/configuration errors, not benchmark results.
- Configuration errors abort before any runs start.
- Configuration errors should not produce run results or metrics.

## Per-Run Setup Failures

- If a per-run setup error occurs after the batch has started but before the harness starts, the run is recorded with status `setup_failed`.
- Partial artifacts for `setup_failed` runs are preserved where available.
- `setup_failed` is distinct from harness `failed` and `timed_out` statuses.

## Model Profiles

- The orchestrator config format is JSON.
- The orchestrator config supports named model profiles.
- A model profile contains:
  - `model_name`
  - `base_url`
  - `api_key`
- The profile key is used for run selection and result identity.
- The `model_name` value is forwarded upstream by the proxy.

Example:

```json
{
  "models": {
    "qwen-coder-32b": {
      "model_name": "qwen2.5-coder:32b",
      "base_url": "http://localhost:11434/v1",
      "api_key": "local"
    }
  }
}
```

## Harness Profiles

- The orchestrator config supports named harness profiles.
- A harness profile points to a Docker image, not a Dockerfile.
- Harness Dockerfiles live under `harnesses/<harness_name>/Dockerfile`.
- Benchmark runs execute the configured image for each harness.
- A helper script such as `build-harnesses.sh` should build all harness Dockerfiles into their expected image tags.
- v0 uses the Docker CLI for image inspection and container execution.
- Docker CLI usage should be isolated behind a small runner abstraction so the Docker Engine API can replace it later if needed.
- Harness containers are disposable and removed after each run finishes.
- Anything interesting from a run must be captured outside the container in the run artifacts.
- The orchestrator does not override container UID/GID.
- Harness containers run with whatever user configuration their image defines.
- The orchestrator does not attempt to fix ownership of files written by the harness.

Example:

```json
{
  "harnesses": {
    "codex": {
      "image": "harness-test/codex:latest"
    }
  }
}
```

## Run Selection CLI

- Runs are selected using command-line flags.
- Tests are referenced by test folder name.
- Harnesses are referenced by harness profile name.
- Models are referenced by model profile name.
- Comma-separated values are acceptable for selecting multiple items.

Example:

```bash
orchestrator run --tests fix-python-bug,add-cli-option --harnesses codex,opencode --models qwen-coder-32b
```

## Proxy Lifecycle

- The orchestrator starts a fresh proxy instance for each run.
- Each proxy uses an available port selected at run time.
- The selected proxy address is passed to the harness container via `LLM_URL`.
- Per-run proxies reduce state leakage between runs and leave a clear path to future parallel execution.
- For v0, the proxy binds to `0.0.0.0` so harness containers can reach it.
- Each proxy requires `LLM_API_KEY`.
- `LLM_API_KEY` is generated uniquely for each run.
- The generated `LLM_API_KEY` is not stored in run results.
- The proxy is shut down after its run completes.
