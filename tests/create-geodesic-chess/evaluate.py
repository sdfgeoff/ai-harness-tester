#!/usr/bin/env python3

import json
import re
from pathlib import Path


ARTIFACT = Path("/run/working_dir/dist/index.html")
OUTPUT = Path("/output/evaluation.json")


def score(condition: bool) -> float:
    return 1.0 if condition else 0.0


def contains(pattern: str) -> bool:
    return ARTIFACT.is_file() and re.search(pattern, ARTIFACT.read_text(), re.IGNORECASE) is not None


exists_score = score(ARTIFACT.is_file())
chess_score = score(contains(r"chess|checkmate|geodesic"))
three_score = score(contains(r"three|canvas|webgl"))

overall_score = 0.0
if exists_score == 1.0:
    overall_score = 0.4
if exists_score == 1.0 and chess_score == 1.0:
    overall_score = 0.7
if exists_score == 1.0 and chess_score == 1.0 and three_score == 1.0:
    overall_score = 1.0

OUTPUT.write_text(
    json.dumps(
        {
            "score": overall_score,
            "breakdown": {
                "dist_index_exists": exists_score,
                "mentions_chess_concepts": chess_score,
                "mentions_3d_stack": three_score,
            },
        },
        indent=2,
    )
    + "\n"
)
