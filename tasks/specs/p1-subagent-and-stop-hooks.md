---
status: approved
revision: 1
---

# P1 #3 — SubagentStart steering + Stop impact check (+ handler `if` filter)

Source: `docs/COMPETITIVE-ANALYSIS-2026-09-25.md` §4 建议 3 (local-only doc).
User authorized batch 2 (P1 #4 → #2 → #3) including L3 and hook registration
in `~/.claude/settings.json` via lifecycle.js on 2026-09-28.

## Goal
(a) SubagentStart: Explore and Plan subagents skip CLAUDE.md, so the adopt
steering never reaches them. Register a SubagentStart hook (matcher
`Explore|Plan|general-purpose`) that returns `additionalContext` of ≤400
chars stating facts: the repo has a code-graph index (file count, fresh or
not) and the three CLI commands that answer structural questions. No
imperative "you must" wording (prompt-injection defenses).
(b) Stop: turn impact from pre-edit advice into an end-of-turn check. The
PreToolUse Edit hook already computes "edited function + production callers";
record that per `session_id` in the plugin's existing tmp/state dir. On Stop,
for symbols whose signature actually changed in `git diff`, list callers
(`inferred` floor) whose files were not touched this turn, as `file:line`,
once per symbol per session, via `additionalContext` (not a block).
(c) Measure per-Bash-call hook overhead; if the harness `if` handler filter
(`"if": "Bash(grep *)"`) is supported by the installed Claude Code version,
add it to the Bash hooks whose parser is the real gate; otherwise record the
number and skip.

## Non-goals
- No asyncRewake test running; no PostToolBatch rewrite.
- No change to existing hooks' decisions.

## Constraints
- Both new hooks join the `HOOK_TIMEOUT_SECONDS` table and the "armed must
  spend" guard; lifecycle install/update/uninstall/doctor all handle them
  (enumerate every writer — memory feedback_a_repair_default_diagnostic_can_undo_the_teardown).
- Tests that exercise install/uninstall run against a sandbox HOME, never the
  real `~/.claude/settings.json` (spec §8.V3). Residue count after tests.
- Stop hook respects `stop_hook_active` (no loops) and stays silent when
  nothing is missing, when not a git repo, or when the index is absent.
- Injected text is LLM-visible metadata: record byte counts.

## Success criteria
- (a) Unit: SubagentStart payload for each matcher ≤400 chars, facts only,
  absent when no index. Verified the event name/output schema against the
  current Claude Code hooks docs (cite source).
- (b) Unit + fixture: signature change in `a.rs` with an untouched caller in
  `b.rs` → one `b.rs:line` line; caller also edited → silent; second Stop in
  the same session → silent; body-only change → silent.
- (c) Overhead baseline number recorded; filter added or skip reason recorded.
- Full plugin JS test suite + cargo suite green.

## Open questions
- Whether SubagentStart supports `additionalContext` in the installed CC
  version — verify against docs/changelog before building (a).

# Change log
- r1 2026-09-28: created; approved under the user's batch AUTH.
