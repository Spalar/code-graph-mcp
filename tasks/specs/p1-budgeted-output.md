---
status: approved
revision: 1
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

# Change log
- r1 2026-09-28: created; approved under the user's batch AUTH.
