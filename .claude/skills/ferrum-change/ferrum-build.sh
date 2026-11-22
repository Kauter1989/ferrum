#!/usr/bin/env bash
# Build wrapper: isolate each branch's build artifacts in a unique target dir
# (branch + date) so old builds never accumulate in one shared `target/` (L53).
# Sets CARGO_TARGET_DIR, then runs the preflight (which assesses and logs the
# build-drive free space before building). Clean up with ferrum-clean-builds.sh
# when the branch merges.
#
#   .claude/skills/ferrum-change/ferrum-build.sh            # full preflight
#   .claude/skills/ferrum-change/ferrum-build.sh --quick    # fmt/clippy/schema
#   .claude/skills/ferrum-change/ferrum-build.sh --coverage
set -uo pipefail

[[ -f Cargo.toml && -d crates/ferrum-agent ]] || { echo "run from the FERRUM repo root" >&2; exit 2; }

branch=$(git rev-parse --abbrev-ref HEAD 2>/dev/null || echo detached)
# sanitise the branch name for a directory (slashes, spaces, etc.)
safe_branch=$(printf '%s' "$branch" | tr '/\\ :*' '-----')
stamp=$(date +%Y%m%d)
export CARGO_TARGET_DIR="$PWD/.builds/${safe_branch}-${stamp}"
mkdir -p "$CARGO_TARGET_DIR"

# Keep the cargo registry off the system drive too, portably: honour a locally
# set CARGO_HOME, else default to a project-relative .cargo-home (gitignored).
# No absolute path is committed, so the repo clones cleanly on any machine.
export CARGO_HOME="${CARGO_HOME:-$PWD/.cargo-home}"
mkdir -p "$CARGO_HOME"

echo "==> build dir: $CARGO_TARGET_DIR"
echo "    CARGO_HOME: $CARGO_HOME"
echo "    (unique per branch+date; clean with ferrum-clean-builds.sh after merge)"

here=$(cd "$(dirname "$0")" && pwd)
exec "$here/preflight.sh" "$@"
