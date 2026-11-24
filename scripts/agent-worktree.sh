#!/usr/bin/env bash
# Give one locally run agent its own checkout of an arena with a warm build.
#
#   scripts/agent-worktree.sh <arena clone> <project dir> <agent name>
#
# Creates <arena clone>/../hillclimb-agents/<agent name> as a git worktree on a
# new branch `agent/<agent name>` off origin/main. The dependency packages are
# shared with the main clone (read-only use); the project's own build output is
# copied, so the agent's first `lake build` only recompiles what it changes.
# The main clone must have been built once (`lake exe cache get && lake build`).
set -euo pipefail
arena="$(cd "$1" && pwd)"; dir="$2"; name="$3"
root="$(dirname "$arena")/hillclimb-agents"
tree="$root/$name"
[ -d "$arena/$dir/.lake/build" ] || { echo "build $arena/$dir first" >&2; exit 1; }
mkdir -p "$root"
git -C "$arena" fetch --quiet origin
git -C "$arena" worktree add --quiet -B "agent/$name" "$tree" origin/main
mkdir -p "$tree/$dir/.lake"
ln -sfn "$arena/$dir/.lake/packages" "$tree/$dir/.lake/packages"
cp -a --reflink=auto "$arena/$dir/.lake/build" "$tree/$dir/.lake/build"
echo "$tree"
