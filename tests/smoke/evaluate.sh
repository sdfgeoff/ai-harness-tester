#!/bin/sh
set -eu

workdir="/run/working_dir"
output="/output/evaluation.json"
marker="${workdir}/smoke-workdir-seen.txt"
report="${workdir}/smoke-harness-output.txt"

marker_score="0.0"
report_score="0.0"

if [ -f "$marker" ]; then
    marker_score="1.0"
fi

if [ -f "$report" ]; then
    report_score="1.0"
fi

overall_score="0.0"
if [ "$marker_score" = "1.0" ] && [ "$report_score" = "1.0" ]; then
    overall_score="1.0"
elif [ "$marker_score" = "1.0" ] || [ "$report_score" = "1.0" ]; then
    overall_score="0.5"
fi

cat > "$output" <<EOF
{
  "score": ${overall_score},
  "breakdown": {
    "marker_file": ${marker_score},
    "harness_report": ${report_score}
  }
}
EOF
