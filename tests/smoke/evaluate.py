#!/usr/bin/env python3

import json
from pathlib import Path


WORKDIR = Path("/run/working_dir")
OUTPUT = Path("/output/evaluation.json")


def score(condition: bool) -> float:
    return 1.0 if condition else 0.0


marker_score = score((WORKDIR / "smoke-workdir-seen.txt").is_file())
report_score = score((WORKDIR / "smoke-harness-output.txt").is_file())

overall_score = 0.0
if marker_score == 1.0 and report_score == 1.0:
    overall_score = 1.0
elif marker_score == 1.0 or report_score == 1.0:
    overall_score = 0.5

OUTPUT.write_text(
    json.dumps(
        {
            "score": overall_score,
            "breakdown": {
                "marker_file": marker_score,
                "harness_report": report_score,
            },
        },
        indent=2,
    )
    + "\n"
)
