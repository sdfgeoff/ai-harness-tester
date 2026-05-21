#!/usr/bin/env python3

import json
from pathlib import Path


ARTIFACT = Path("/run/working_dir/pelican.svg")
OUTPUT = Path("/output/evaluation.json")


def score(condition: bool) -> float:
    return 1.0 if condition else 0.0


exists_score = score(ARTIFACT.is_file())
svg_score = score(ARTIFACT.is_file() and "<svg" in ARTIFACT.read_text().lower())

overall_score = 0.0
if exists_score == 1.0 and svg_score == 1.0:
    overall_score = 1.0
elif exists_score == 1.0:
    overall_score = 0.5

OUTPUT.write_text(
    json.dumps(
        {
            "score": overall_score,
            "breakdown": {
                "file_exists": exists_score,
                "looks_like_svg": svg_score,
            },
        },
        indent=2,
    )
    + "\n"
)
