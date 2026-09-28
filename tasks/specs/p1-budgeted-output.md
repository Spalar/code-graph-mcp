---
status: implemented
revision: 2
---

# P1 #2 — budgeted output and runnable next steps at every cut

Source: `docs/COMPETITIVE-ANALYSIS-2026-09-25.md` §4 建议 2 (local-only doc).
User authorized batch 2 (P1 #4 → #2 → #3) including L3 on 2026-09-28.

## Goal
Let the caller say how much output it wants: `--budget <tokens>` on the CLI
and `max_tokens` on the MCP tools for `project_map`, `overview`
(module_overview), `callgraph` (get_call_graph) and `show` (get_ast_node).
Within the budget, rank by relevance (in-degree until PageRank #5 exists),
render low-rank items as skeleton (signature + `file:line`) before dropping
them, never cut in the middle of a member, and end every truncation with a
runnable next-step command.

## Non-goals
- Default output unchanged when no budget is given (released-artifact default
  behavior stays; only the next-step command text is added to existing
  truncation notices, which is additive disclosure).
- No tokenizer: budget counts with the existing bytes/3 (`CHARS_PER_TOKEN`).
- No PageRank (#5 is P2).

## Constraints
- Truncation notices already exist (ARCHITECTURE "truncation is disclosed");
  extend them, don't add a parallel mechanism. Find the existing compression
  tiers first and reuse them.
- MCP tool schema change is additive (optional param). Tool descriptions are
  LLM-visible metadata: keep the added text to one short clause per tool and
  record the adoption token baseline (`claude plugin details` / description
  byte count) before and after.

## Success criteria
- `project_map --budget 1000` rendered size within ±15% of 1000 tokens
  (bytes/3) on this repo and on one outside corpus; same for 500 and 4000.
- Every truncated output (budgeted or default tier) carries a next-step
  command that runs and returns the omitted part (test executes it).
- Default (no budget) outputs byte-identical to before except the added
  next-step line(s) — pinned by a test.
- Tool-description byte delta reported.

## Open questions
- None blocking; ranking signal = in-degree (callers count) for now.

## Decisions taken in implementation (r2)
The spec left these open; each is the conservative reading.
- **Next-step form.** Every `next` is a `code-graph-mcp …` CLI command, on the
  MCP surface too (`next` key / `budget.next`). The CLI is the same binary as
  the server, and several omitted parts (inactive names, dead code, every
  dependency) have no MCP call that returns them whole. Same form as P1 #4's
  `boundaries.next`.
- **Which default cuts get a command.** Output-size cuts only: `map`'s
  `... and N more` lines, `hot_functions_truncated` (CLI JSON and MCP),
  `active_capped` / an inactive group's `more`, the `rollup_call_graph` mode,
  `compressed_node`, and the threshold tier's `_truncated` for these four tools.
  Where the threshold tier and a handler cap both fire (`project_map compact`
  on a large repo), the tier's command (the whole answer, `map --json`)
  replaces the handler's (`map`, whose text stops at 30 dependencies).
  Traversal bounds (`limit_hit`, `depth_capped`, `callers_truncated`) and
  filters (hidden tests, hidden ambiguous edges, export filter) keep their text
  as it was: no command returns more than the traversal can reach, and filters
  already name their flag. The index-time 4096-byte cap on stored
  `code_content` has no notice today and did not get one, as the spec's non-goal
  allows only adding a command to an existing notice.
- **Budget scope.** CLI `--budget` applies to the text answer and is refused
  with `--json` / `--compact` (clap, exit 2). MCP `max_tokens` starts from the
  full envelope; `compact` beside it is inert and reported in
  `ignored_arguments` (MODE_INERT_ARGS now accepts a numeric selector). Range
  100-100000, clamped and disclosed like every count argument.
- **Starting point.** A budgeted answer starts from the uncapped answer (every
  dependency, every export, the flat call graph), so a large budget can return
  more than the default. The threshold tier (`centralized_compress`) does not
  run on a budgeted call.
- **Ranking.** In-degree = count of `calls` edges targeting the node; for a
  `map` module, the summed import count of dependencies into it; for an
  `overview` file, the sum of its symbols' caller counts; call-graph nodes rank
  deepest-first, then by in-degree (a node goes before its parent). `map`'s
  sections lose items in proportion to their length. Entry points and
  references (`show --refs`, `called_by`/`calls`) rank by listing order.
- **Levels.** Standard order everywhere: every item that has a shorter form is
  shortened (least important first) before any item is dropped. Then a fill
  pass restores, most important first, whatever still fits (Aider's
  render-and-count, with a knapsack step: a smaller lower-ranked item may come
  back where a larger higher-ranked one could not). The pass is bounded at 256
  renders.
- **70% rule.** Applied where files are the unit: `overview` of a directory
  (CLI: a file's name list; MCP: a file's active exports) is held to 70% of the
  budget before the global fit.
- **Accounting.** Size is measured over the answer the handler returns; the
  post-handler `freshness` / `ignored_arguments` / `clamped_arguments` keys are
  added afterwards and are not counted (measured: ~280 B for `freshness`).
- **Tool descriptions** unchanged (zero clauses added); the `max_tokens`
  property description carries the one clause.

## Results (r2)
- Accuracy (bytes / budget×3), this repo: `overview src` 0.996 / 1.000 / 0.999
  at 500 / 1000 / 4000; `map` 0.989 / 0.994 / whole answer; MCP `project_map`
  0.993 / 0.999 / 1.022. hono `map` 0.984 / 1.000 / 1.000. Full tables in
  CHANGELOG. Below the band: express `show next --refs` at 1000 tokens came to
  1,273 B (ratio 0.42), because the body is larger than the budget and is left
  out whole.
- `tools/list` 9,120 → 9,720 B (tool descriptions +0 B, four schemas +150 B each).
- 31 mutations of the new guards (neutralize and invert), all red on a failing
  test (4 of them rewritten first because they did not compile; one survived
  until the MCP depth-rank check was added).

# Change log
- r1 2026-09-28: created; approved under the user's batch AUTH.
- r2 2026-09-28: implemented (`src/budget.rs`, the four CLI commands, the four
  MCP tools, `tests/budgeted_output.rs`, `tests/data/budget_base/`); decisions
  and results above.
