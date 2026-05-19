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
    runs/
      <run_id>/
        logs/
          harness.log
        working_dir/
        PROMPT.md
        results.json
```

At this stage `results.json` records run ID, status, timestamps, duration, harness exit code, and the relative harness log path. Harness stdout and stderr are captured together in `logs/harness.log` and streamed live to the console with the run ID as a prefix.

When `--tests <name>` is provided, the CLI validates `tests/<name>/initial_state.zip` and `tests/<name>/PROMPT.md`, then records SHA-256 hashes for both files in `results.json`. The `--tests`, `--harnesses`, and `--models` flags accept comma-separated values; full matrix execution is added in a later ticket.

The selected test archive is extracted into `working_dir` at the root of the run artifact. Archive entries with absolute paths, `..` path traversal, or symlinks are rejected before Docker starts.

When a test is selected, `working_dir` is mounted into the container read-write at `/workdir`, the container working directory is set to `/workdir`, and `WORKDIR=/workdir` is provided in the environment.

The selected prompt is copied to a temporary file outside `working_dir`, mounted read-only as `/prompt/PROMPT.md`, and exposed as `INITIAL_PROMPT_FILE=/prompt/PROMPT.md`. After the run, the same temporary prompt is copied into the run artifact as `PROMPT.md` and the temporary file is removed.

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
