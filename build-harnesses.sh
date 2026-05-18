#!/usr/bin/env sh
set -eu

docker build -t harness-test/smoke:latest harnesses/smoke
