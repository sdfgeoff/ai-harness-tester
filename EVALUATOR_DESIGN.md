# Evaluator Design

This document captures the agreed design for per-test evaluation artifacts and evaluator execution.

## Goals

- Keep execution outcome separate from correctness/scoring.
- Make evaluation test-specific.
- Preserve enough artifacts to support later aggregation and reruns.
- Keep v1 evaluation deterministic and local to preserved run artifacts.

## Test Contract

Each test directory under `tests/<name>/` must include:

- `initial_state.zip`
- `PROMPT.md`
- `evaluate.Dockerfile`

`evaluate.py` is not required by the orchestrator contract. It may be used by individual tests, but the evaluator image owns how evaluation is executed.

## Evaluator Images

- Evaluator images are built from `tests/<name>/evaluate.Dockerfile`.
- Build context is the full test directory `tests/<name>/`.
- Image tag convention: `harness-test-evaluator/<test>:latest`.
- Evaluator images are built only by the repo build script, not implicitly during orchestrator runs.
- The build script should build all harness and evaluator images, continue through failures, then fail with a summary.

## Preflight

Batch preflight should:

- validate selected tests as usual
- require `evaluate.Dockerfile` for every test
- verify the local evaluator image exists for every selected test
- inspect evaluator image IDs once up front, the same way harness image IDs are handled

Missing evaluator Dockerfiles or images are setup/configuration errors, not benchmark outcomes.

## Evaluation Timing

- Only `completed` runs are evaluated.
- Evaluation happens immediately after each run finishes.
- Evaluation failures count as batch failures, but do not change `results.json.status`.
- Non-completed runs still get an `evaluation.json` artifact with skipped status.

## Execution Boundary

Harness and evaluator containers should both use the `docker-runner` crate through a generic container-run API.

The orchestrator should not hardcode the evaluator command. The evaluator image defines its own entrypoint/cmd. The orchestrator contract is based on mounts, timeout, and expected outputs.

## Evaluator Container Contract

The evaluator container receives:

- `/run` mounted read-only with the full run artifact directory
- `/evaluator` mounted read-only with the full test directory
- `/output` mounted writable for evaluator outputs

Other constraints:

- no orchestrator-provided env vars in v1
- allow normal container-local scratch writes
- preserve the whole `/output` directory

Current design intent is to keep evaluators artifact-driven. Future LLM-as-judge support may require a different contract.

## Run Artifacts

Execution and evaluation artifacts remain separate.

Run root layout additions:

- `results.json` for execution outcome and run metrics
- `evaluation.json` for evaluator outcome and scored result
- `evaluation_output/` preserved from evaluator `/output`
- `logs/evaluator.log` when an evaluator container was actually started

`results.json` and `evaluation.json` should not cross-reference each other.

## `evaluation.json`

`evaluation.json` lives beside `results.json` at the run root.

For scored evaluations:

```json
{
  "status": "scored",
  "started_at": "2026-05-21T00:00:00Z",
  "finished_at": "2026-05-21T00:00:01Z",
  "duration_ms": 1000,
  "evaluator": {
    "image": "harness-test-evaluator/smoke:latest",
    "image_id": "sha256:..."
  },
  "result": {
    "score": 0.9,
    "breakdown": {}
  }
}
```

For failed evaluations:

```json
{
  "status": "failed",
  "started_at": "2026-05-21T00:00:00Z",
  "finished_at": "2026-05-21T00:00:01Z",
  "duration_ms": 1000,
  "evaluator": {
    "image": "harness-test-evaluator/smoke:latest",
    "image_id": "sha256:..."
  },
  "error": {
    "kind": "invalid_schema",
    "message": "score must be between 0 and 1"
  }
}
```

For skipped evaluations:

```json
{
  "status": "skipped",
  "reason": "run_not_completed"
}
```

## Evaluator Output Contract

The evaluator must write `/output/evaluation.json`.

The orchestrator copies that file to the run root as `evaluation.json` after validation and preserves the full output directory as `evaluation_output/`.

If the evaluator does not produce `/output/evaluation.json`, evaluation fails.

## Evaluation Result Validation

The orchestrator validates evaluator output strictly:

- `score` is required
- `score` must be a finite number in the inclusive range `[0.0, 1.0]`
- `breakdown` is optional
- if present, `breakdown` must be an object
- empty `breakdown` is valid
- every `breakdown` value must be a finite number in the inclusive range `[0.0, 1.0]`
- validation should report all discovered schema/range violations in one error message

## Evaluation Failure Kinds

Stable failure kinds:

- `container_failed`
- `timed_out`
- `missing_output`
- `invalid_json`
- `invalid_schema`

## Timeouts

Evaluation uses a separate top-level config value:

```json
{
  "timeout_seconds": 1800,
  "evaluation_timeout_seconds": 300
}
```

This is orchestrator behavior, not test metadata.

## Batch Summary

`summary.json` remains an index, not an aggregate. Each run entry should include:

```json
{
  "run_id": "20260519T020329Z_smoke_smoke-local_smoke_9341d8",
  "results_path": "runs/20260519T020329Z_smoke_smoke-local_smoke_9341d8/results.json",
  "evaluation_path": "runs/20260519T020329Z_smoke_smoke-local_smoke_9341d8/evaluation.json"
}
```

The summary should include the evaluation path, but not denormalize evaluation status or score.

## Future Direction

- Design current evaluation flow so rerunning evaluation against an existing run artifact is easy to add later.
- LLM-as-judge evaluators are explicitly out of scope for v1, but should remain a future extension point.
