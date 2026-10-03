#!/usr/bin/env bash
# Local checks in CI order (.github/workflows/ci.yml). Run from the repo root.
#   .claude/skills/ferrum-change/preflight.sh              fmt, clippy, schema, tests
#   .claude/skills/ferrum-change/preflight.sh --quick      fmt, clippy, schema only
#   .claude/skills/ferrum-change/preflight.sh --coverage   also make coverage-ci
set -uo pipefail

QUICK=0
COVERAGE=0
for a in "$@"; do
  case "$a" in
    --quick) QUICK=1 ;;
    --coverage) COVERAGE=1 ;;
    *) echo "unknown option $a" >&2; exit 2 ;;
  esac
done

[[ -f Cargo.toml && -d crates/ferrum-agent ]] || { echo "run from the FERRUM repo root" >&2; exit 2; }

failed=()
step() {
  local name="$1"; shift
  echo "==> $name"
  if "$@"; then echo "    ok"; else echo "    FAILED: $name"; failed+=("$name"); fi
}

schema_fresh() {
  local tmp; tmp=$(mktemp)
  cargo run -q -p ferrum-cli -- schema >"$tmp" || return 1
  if ! diff -q "$tmp" skills/ferrum/schemas/commands.json >/dev/null; then
    echo "    skills/ferrum/schemas/commands.json is stale:"
    echo "    cargo run -q -p ferrum-cli -- schema > skills/ferrum/schemas/commands.json"
    rm -f "$tmp"; return 1
  fi
  rm -f "$tmp"
}

gpu_tests() {
  if ! command -v vulkaninfo >/dev/null 2>&1 && ! ls /usr/share/vulkan/icd.d/lvp_icd* >/dev/null 2>&1; then
    echo "    no Vulkan driver found; install lavapipe (mesa-vulkan-drivers libvulkan1)"
    echo "    or GPU tests are skipped instead of run, unlike CI"
    return 1
  fi
  FERRUM_REQUIRE_GPU=1 cargo test --workspace -- --test-threads=4
}

step "format" cargo fmt --all -- --check
step "clippy" cargo clippy --workspace --all-targets -- -D warnings
step "schema" schema_fresh
if (( ! QUICK )); then
  step "tests (CI mode, GPU required)" gpu_tests
fi
if (( COVERAGE )); then
  step "coverage floor" make coverage-ci
fi

echo
du -sh target 2>/dev/null | sed 's/^/target size: /'
if (( ${#failed[@]} )); then
  echo "FAILED: ${failed[*]}"
  exit 1
fi
echo "all checks passed"
