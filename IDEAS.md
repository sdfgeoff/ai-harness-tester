# Ideas

## Future Metrics

- Track tool calls emitted through LLM API protocols.
- Consider distinguishing model-declared tool calls from actual harness-local tool execution.
- Add aggregate proxy error counts to `results.json` if summary-level request failure reporting becomes useful.

## Harness Configuration

- Add optional per-harness environment variables in config.
- Reserve orchestrator-owned variables such as `WORKDIR`, `INITIAL_PROMPT_FILE`, `LLM_URL`, and `LLM_API_KEY` from being overridden.

## Proxy Compatibility

- Expand `GET /v1/models` compatibility if specific harnesses require additional fields or behavior.

## Artifact Analysis

- Generate a diff between the initial extracted state and final `working_dir`.
