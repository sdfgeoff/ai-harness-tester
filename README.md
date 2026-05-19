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
cargo run -p orchestrator-cli -- run-image harness-test/smoke:latest
```

At this stage the command runs the image, reports its exit status, and writes minimal run artifacts. Later tickets add working directory mounts, prompt handling, and proxy wiring.

The command writes a minimal run artifact:

```text
results/
  <run_id>/
    logs/
      harness.log
    results.json
```

At this stage `results.json` records run ID, status, timestamps, duration, harness exit code, and the relative harness log path. Harness stdout and stderr are captured together in `logs/harness.log` and streamed live to the console with the run ID as a prefix.
