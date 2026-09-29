#!/usr/bin/env bash
# Build the workspaces for the coding cases (tag: coding).
#
# Each case is networkx at a pinned commit with one real upstream change
# reverted (source AND its tests), so the agent has to make that change again.
# The grader's tests live in evals/_coding/hidden/, outside the workspace, and
# evals/_coding/grade.py runs them on the workspaces that
# `claude plugin eval --keep-temp` leaves behind.
#
#   evals/_coding/build.sh          # once; evals/run.sh calls it when needed
#
# Checks every case both ways before it can be used: the hidden tests must
# FAIL on the workspace (the task is not already done) and PASS on the pinned
# commit (the tests are right).
set -euo pipefail

here="$(cd "$(dirname "$0")" && pwd)"
cache="${CG_EVAL_CODING_CACHE:-/var/tmp/code-graph-eval/coding}"
NX_COMMIT=c1ebe046ec672eca7cba4dc88d6e0145406d693a
src="$cache/networkx"

# case name -> upstream commit to revert
CASES=(
  "code-flow-capacity 0080011d83a3808ab2b3d206591e15d8f3ddb7b0"
  "code-betweenness-k a802a27f50623ba5669420bb2edeaafd30242323"
  "code-subclass-args 75bdd737ca8382d06eb76ff9018a81870c9f3443"
  "code-dead-helpers b1b678f4f768a2e51da83bd9e9de8bbaa1b30845"
  "code-ismags-empty c94928ed94899033126c9d47f797a1f698584b20"
)

mkdir -p "$cache/fixtures"
if [ ! -d "$src/.git" ]; then
  git clone -q https://github.com/networkx/networkx "$src"
fi
git -C "$src" cat-file -e "$NX_COMMIT^{commit}" 2>/dev/null || git -C "$src" fetch -q origin

base="$cache/base"
if [ ! -d "$base/networkx" ]; then
  mkdir -p "$base"
  git -C "$src" archive "$NX_COMMIT" | tar -x -C "$base"
fi

hidden_check() { # <case> <root>: exit 0 when the hidden grader passes on root
  local name="$1" root="$2"
  if [ "$name" = code-dead-helpers ]; then
    # "passes" = nothing left to delete
    python3 "$here/hidden/$name/dead_helpers.py" gold "$root" \
      | python3 -c 'import json,sys; sys.exit(1 if json.load(sys.stdin)["dead"] else 0)'
  else
    (cd "$root" && python3 -m pytest -q -p no:cacheprovider "$here/hidden/$name" >/dev/null 2>&1)
  fi
}

for entry in "${CASES[@]}"; do
  read -r name commit <<<"$entry"
  out="$cache/fixtures/$name.tar"
  [ -f "$out" ] && continue
  work="$(mktemp -d "$cache/build.XXXXXX")"
  # A 3-way revert: later upstream edits near the change make a plain
  # `git apply -R` of the old diff fail.
  git -C "$src" worktree add -q --detach "$work/tree" "$NX_COMMIT"
  git -C "$work/tree" revert --no-commit "$commit"
  git -C "$work/tree" archive "$(git -C "$work/tree" stash create)" | tar -x -C "$work"
  git -C "$src" worktree remove --force "$work/tree"
  if hidden_check "$name" "$work"; then
    echo "$name: hidden grader already passes on the workspace — the revert did not take" >&2
    exit 1
  fi
  if ! hidden_check "$name" "$base" && [ "$name" != code-dead-helpers ]; then
    echo "$name: hidden grader fails on the pinned commit — the test is wrong" >&2
    exit 1
  fi
  tar -C "$work" -cf "$out" .
  rm -rf "${work:?}"
  echo "built $name"
done
