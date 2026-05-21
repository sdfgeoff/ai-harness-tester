#!/usr/bin/env python3

import json
import re
from pathlib import Path


ARTIFACT = Path("/run/working_dir/index.html")
OUTPUT = Path("/output/evaluation.json")


def score(condition: bool) -> float:
    return 1.0 if condition else 0.0


def contains(pattern: str) -> bool:
    return ARTIFACT.is_file() and re.search(pattern, ARTIFACT.read_text(), re.IGNORECASE) is not None


exists_score = score(ARTIFACT.is_file())
printer_score = score(contains(r"3d printer|printer"))
projects_score = score(contains(r"project|projects"))

overall_score = 0.0
if exists_score == 1.0:
    overall_score = 0.4
if printer_score == 1.0:
    overall_score = 0.7
if exists_score == 1.0 and printer_score == 1.0 and projects_score == 1.0:
    overall_score = 1.0

OUTPUT.write_text(
    json.dumps(
        {
            "score": overall_score,
            "breakdown": {
                "index_html_exists": exists_score,
                "mentions_3d_printers": printer_score,
                "mentions_projects": projects_score,
            },
        },
        indent=2,
    )
    + "\n"
)
