#!/bin/sh
set -eu

artifact="/run/working_dir/index.html"
output="/output/evaluation.json"

exists_score="0.0"
printer_score="0.0"
projects_score="0.0"

if [ -f "$artifact" ]; then
    exists_score="1.0"
fi

if [ -f "$artifact" ] && grep -Eiq "3d printer|printer" "$artifact"; then
    printer_score="1.0"
fi

if [ -f "$artifact" ] && grep -Eiq "project|projects" "$artifact"; then
    projects_score="1.0"
fi

overall_score="0.0"
if [ "$exists_score" = "1.0" ]; then
    overall_score="0.4"
fi
if [ "$printer_score" = "1.0" ]; then
    overall_score="0.7"
fi
if [ "$printer_score" = "1.0" ] && [ "$projects_score" = "1.0" ] && [ "$exists_score" = "1.0" ]; then
    overall_score="1.0"
fi

cat > "$output" <<EOF
{
  "score": ${overall_score},
  "breakdown": {
    "index_html_exists": ${exists_score},
    "mentions_3d_printers": ${printer_score},
    "mentions_projects": ${projects_score}
  }
}
EOF
