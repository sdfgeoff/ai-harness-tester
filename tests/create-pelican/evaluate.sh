#!/bin/sh
set -eu

artifact="/run/working_dir/pelican.svg"
output="/output/evaluation.json"

exists_score="0.0"
svg_score="0.0"

if [ -f "$artifact" ]; then
    exists_score="1.0"
fi

if [ -f "$artifact" ] && grep -qi "<svg" "$artifact"; then
    svg_score="1.0"
fi

overall_score="0.0"
if [ "$exists_score" = "1.0" ] && [ "$svg_score" = "1.0" ]; then
    overall_score="1.0"
elif [ "$exists_score" = "1.0" ]; then
    overall_score="0.5"
fi

cat > "$output" <<EOF
{
  "score": ${overall_score},
  "breakdown": {
    "file_exists": ${exists_score},
    "looks_like_svg": ${svg_score}
  }
}
EOF
