#!/bin/sh
set -eu

artifact="/run/working_dir/dist/index.html"
output="/output/evaluation.json"

exists_score="0.0"
chess_score="0.0"
three_score="0.0"

if [ -f "$artifact" ]; then
    exists_score="1.0"
fi

if [ -f "$artifact" ] && grep -Eiq "chess|checkmate|geodesic" "$artifact"; then
    chess_score="1.0"
fi

if [ -f "$artifact" ] && grep -Eiq "three|canvas|webgl" "$artifact"; then
    three_score="1.0"
fi

overall_score="0.0"
if [ "$exists_score" = "1.0" ]; then
    overall_score="0.4"
fi
if [ "$exists_score" = "1.0" ] && [ "$chess_score" = "1.0" ]; then
    overall_score="0.7"
fi
if [ "$exists_score" = "1.0" ] && [ "$chess_score" = "1.0" ] && [ "$three_score" = "1.0" ]; then
    overall_score="1.0"
fi

cat > "$output" <<EOF
{
  "score": ${overall_score},
  "breakdown": {
    "dist_index_exists": ${exists_score},
    "mentions_chess_concepts": ${chess_score},
    "mentions_3d_stack": ${three_score}
  }
}
EOF
