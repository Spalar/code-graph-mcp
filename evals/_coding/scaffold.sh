#!/usr/bin/env bash
# Workspace for one coding case: the networkx fixture built by build.sh, as a
# one-commit git repo (no history, so the upstream fix is not one `git log`
# away). Sourced by each code-* case's scaffold.sh with the case name; runs in
# the empty workspace, as you, outside the agent's sandbox, in BOTH arms.
set -euo pipefail
source "$(dirname "${BASH_SOURCE[0]}")/../_fixture/env.sh"
fixture="${CG_EVAL_CODING_CACHE:-/var/tmp/code-graph-eval/coding}/fixtures/${1:?case name}.tar"
[ -f "$fixture" ] || { echo "no fixture $fixture — run evals/_coding/build.sh" >&2; exit 1; }
tar -xf "$fixture"
git init -q
git add -A
git -c user.email=eval@example.invalid -c user.name=eval commit -qm networkx

# Where a real install keeps the binary, in both arms (as _fixture/scaffold.sh).
mkdir -p "$HOME/.cache/code-graph/bin"
cp "${CG_EVAL_NATIVE:?run through evals/run.sh}" "$HOME/.cache/code-graph/bin/code-graph-mcp"
