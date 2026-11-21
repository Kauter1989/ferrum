#!/usr/bin/env bash
# Clean per-branch build dirs (.builds/<branch>-<date>) for merged or old branches
# (L53), so regenerable artifacts never fill the build drive. PREVIEW by default;
# --apply to remove. Only .builds/ artifacts are removed — never source, patient
# data, or approved code; the full consequence is shown before any delete (L55).
#
#   .claude/skills/ferrum-change/ferrum-clean-builds.sh           # preview
#   .claude/skills/ferrum-change/ferrum-clean-builds.sh --apply   # remove
set -uo pipefail
APPLY=0
[[ "${1:-}" == "--apply" ]] && APPLY=1
[[ -f Cargo.toml && -d crates/ferrum-agent ]] || { echo "run from the FERRUM repo root" >&2; exit 2; }

# Branches already merged into develop: their build dirs are safe to remove.
merged=$(git branch --merged develop 2>/dev/null | sed 's/^[* ]*//' | tr '/\\ :*' '-----')

echo "==> .builds directories (branch-date  size  reason):"
candidates=()
shopt -s nullglob
for d in .builds/*/; do
  name=$(basename "$d")
  size_kb=$(du -sk "$d" 2>/dev/null | awk '{print $1}')
  size_gb=$(awk "BEGIN{printf \"%.1f\", ${size_kb:-0}/1024/1024}")
  branch_part=${name%-*}
  age_days=$(( ( $(date +%s) - $(stat -c %Y "$d" 2>/dev/null || date +%s) ) / 86400 ))
  reason=""
  if printf '%s\n' "$merged" | grep -qx "$branch_part"; then reason="merged"; fi
  if (( age_days > 14 )); then reason="${reason:+$reason,}old(${age_days}d)"; fi
  printf '  %-44s %7s GB  %s\n' "$name" "$size_gb" "${reason:-keep}"
  [[ -n "$reason" ]] && candidates+=("$d")
done
shopt -u nullglob

echo
if (( ${#candidates[@]} == 0 )); then echo "nothing to clean"; exit 0; fi
echo "candidates for removal:"
printf '  %s\n' "${candidates[@]}"
if (( APPLY )); then
  for d in "${candidates[@]}"; do
    case "$d" in
      .builds/*) rm -rf -- "$d" && echo "removed $d" ;;
      *) echo "SKIP (not under .builds/): $d" ;;
    esac
  done
else
  echo
  echo "preview only — re-run with --apply to remove."
  echo "(L55: assess consequences before deleting; only regenerable .builds artifacts)"
fi
