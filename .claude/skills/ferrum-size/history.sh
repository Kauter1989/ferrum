#!/usr/bin/env bash
# Size of merged PRs on develop, for calibrating ferrum-size.
#   .claude/skills/ferrum-size/history.sh [N]     last N merges (default 25)
# Columns: PR, branch, files, lines added+deleted in code / tests / docs / other.
set -euo pipefail
N="${1:-25}"
git fetch -q origin develop 2>/dev/null || true
printf '%-5s %-38s %5s %7s %7s %7s %7s %7s\n' PR branch files total code tests docs other
git log origin/develop --first-parent --merges -n "$N" --format='%H %s' | while read -r sha subject; do
  pr=$(sed -n 's/.*#\([0-9]*\).*/\1/p' <<<"$subject")
  branch=$(sed -n 's/.* from [^/]*\/\(.*\)$/\1/p' <<<"$subject")
  git diff --numstat "$sha^1" "$sha" | awk -v pr="$pr" -v br="${branch:-?}" '
    $1 == "-" { next }                       # binary
    { n = $1 + $2; files++; total += n
      if ($3 ~ /\.md$/ || $3 ~ /^docs\//) docs += n
      else if ($3 ~ /\/tests\// || $3 ~ /^bridges\/tests\// || $3 ~ /_test\.py$/) tests += n
      else if ($3 ~ /\.(rs|wgsl|py)$/) code += n
      else other += n }
    END { printf "%-5s %-38s %5d %7d %7d %7d %7d %7d\n", "#" pr, substr(br, 1, 38), files, total, code, tests, docs, other }'
done
echo
echo "code = .rs/.wgsl/.py outside tests (includes #[cfg(test)] modules); tests = tests/ dirs; docs = *.md and docs/"
