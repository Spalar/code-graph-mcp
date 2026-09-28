---
status: implemented
revision: 3
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

## Decisions (r2)
- Open question answered: SubagentStart supports `additionalContext`. Docs
  (code.claude.com/docs/en/hooks, "SubagentStart"): "SubagentStart hooks can't
  block subagent creation, but they can inject context into the subagent";
  decision table row "SessionStart, SubagentStart, PostModelSwitch | Context
  only". Event added in CC 2.0.43; installed 2.1.283. A headless 2.1.283 probe
  (`claude -p --setting-sources project --settings …`) returned the injected
  token from the subagent.
- Stop uses `hookSpecificOutput.additionalContext` (CC 2.1.163+), documented as
  non-error feedback that continues the turn through the same loop protections
  as `decision: "block"`. Probe: the model answered with the injected token and
  the next Stop carried `stop_hook_active: true`.
- Registration follows the other six: lifecycle.js writes both into
  settings.json (hooks.json stays SessionStart-only). Descriptions keep the
  `[code-graph-mcp v0.32+]` settings-registration prefix, which
  lifecycle.test.js pins on every entry. Budgets: `subagent-start.js` 3 s,
  `stop-impact.js` 5 s in `HOOK_TIMEOUT_SECONDS`.
- Freshness for (a) = `health-check`'s `index_age` plus its
  `index_version_stale` flag; the three commands are `callgraph`, `show`,
  `overview`. Silent under `CODE_GRAPH_QUIET_HOOKS=1`, like the SessionStart
  project map.
- (b) records from pre-edit-guide.js into an append-only JSONL per
  (project, session) in cgTmpDir(): one file-only record per Edit, then one
  with the symbol when extracted (before the cooldown). The Stop hook keeps
  `{lastStopAt, reported}` in a second file. "Signature changed" = the sorted
  whitespace-free definition headers of the symbol differ between
  `git show HEAD:./file` and the working tree (text heuristics applied to both
  sides) — superseded in r3, see the change log. Callers = `refs --relation calls --min-confidence inferred`, `file:line`
  of the first mention at or after the caller's start line. "Touched this
  turn" = Edit logged since the previous Stop, or mtime ≥ turn start (the
  previous Stop; the first logged Edit for the session's first turn). Both
  only suppress output.
- (c) skipped after measuring: `if` is supported (CC 2.1.85+) and filtered a
  non-matching call in the probe, but it takes one rule; the Bash parsers
  accept grep/rg/ag/git grep/env (+ `sed -n` for PreToolUse), so each hook needs
  ~5 handlers, and the probe ran two same-command handlers twice for one
  `grep …; sed …` call (same tool_use_id). Double rewrites/answers would
  follow. A per-tool_use_id claim inside the hooks would make it safe; not in
  this scope.

## Results (r2)
- Byte counts: SubagentStart 321 bytes on this repo (418 files), 389 at the
  builder's worst accepted input; Stop 185 bytes for one caller, 3,314 for
  8 symbols × 8 callers.
- Latency (20 runs, median/p90): subagent-start 125/178 ms; stop-impact with
  no Edit log 33.6/44.2 ms. Bash hooks today (30 runs, `cargo build`
  payload): pre-grep-guide 34.0/45.1 ms, post-grep-inject 35.3/45.5 ms,
  bare `node -e 0` 22.3/30.4 ms.
- Tests: new subagent-start.test.js (6), stop-impact.test.js (14, 3 of them
  e2e with real git + binary), session-edits.test.js (4), one lifecycle e2e
  (install/doctor-repair/update/uninstall on a sandbox HOME); hooks.test.js,
  hook-emit.test.js, hook-fire.test.js pins updated. 19 mutations
  (neutralize + invert) all red.

# Change log
- r1 2026-09-28: created; approved under the user's batch AUTH.
- r2 2026-09-28: implemented (a) and (b); (c) measured and skipped with the
  reason above.
- r3 2026-09-28: review repairs. H1: the baseline is the symbol's headers
  recorded by pre-edit-guide before the turn's first Edit of it (log field
  `sigs`), not HEAD, so both sides of the check are this turn's; HEAD is no
  longer read (the git-work-tree gate stays). M1: the 8-symbol cap bounds
  `refs` queries over changed symbols only; the rest are named. M2: per-language
  header rules (LANGS) for 13 languages, others silent; an added same-named
  definition is not a change; an unterminated header (600 chars) is not read.
  M3: repo-derived tokens shell-quoted via cg-answer.js `shellQuoteArg` /
  `formatCgCommand`. Stop text 230 bytes for one caller. Tests: stop-impact 53
  (7 e2e), session-edits 5, subagent-start 8; 31 mutations all red.
