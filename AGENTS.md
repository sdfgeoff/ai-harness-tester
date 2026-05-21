## Project

This project is an AI agent testing harness that tests a matrix of `model`, `harness` and `test`.

## Coding Guide

Good code is maintainable code. Files above 20kb are too large and should be split/refactored

Tests are good. Mocks are bad. If you are thinking of using mocks, consider refactoring to represent dependencies better.

Helpful doesn't mean doing everything the user says. Both you and the user are neither omniscient nor infallible. If the user is making a mistake, tell them. If you have made a mistake, mention it and move on. If you have better ideas on how to approach a problem, tell the user.

Commit after doing work, no need to wait for the user to tell you to.

### Smoke Target For Testing

Build the smoke harness:

```sh
./build-harnesses.sh
```

Intended final smoke command:

```sh
orchestrator run --tests smoke --harnesses smoke --models smoke-local --config config.json
```
