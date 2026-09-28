---
status: implemented
revision: 2
---

# P1 #4 — an empty call result says where the static graph stops

Source: `docs/COMPETITIVE-ANALYSIS-2026-09-25.md` §4 建议 4 (local-only doc).
User authorized batch 2 (P1 #4 → #2 → #3) including L3 on 2026-09-28.

## Goal
When `callgraph` / `impact` / `refs` (CLI and MCP) find 0 callers (or an empty
result) for a symbol, add a short, deterministic `boundaries` disclosure: the
places in the project where the symbol's name appears in a dynamic-dispatch
shape the static graph does not turn into an edge (string-keyed table/dict
entry, `getattr(x, "name")` / `obj["name"]` / `obj[name]`-style computed member,
reflection primitives, event-bus string keys, `send(:name)` / `method(:name)`,
function pointer / callback registration by name), each as `file:line` plus
the shape label. The agent can then tell "nobody calls it" from "called
through a shape we don't see".

## Non-goals
- No new edges. The graph is not changed ("silent beats wrong").
- No change to non-empty results.
- No whole-repo regex scan per query beyond what an FTS/grep over the name
  already costs; no new index tables, no INDEX_VERSION bump.

## Constraints
- Shape table + corpus test first (per language: JS/TS, Python, Ruby, Go, Rust,
  Java, C/C++ at least the reflection/string-key forms that exist there).
  Comments and strings that merely mention the name must not count, except
  where the string IS the key shape (`handlers["save"]`).
- Bounded output: at most N (≈5) sites + a count of the rest + a runnable
  next-step command (`code-graph-mcp grep …`).
- Text and JSON outputs both; MCP output field additive (`boundaries`).
- Latency: an empty-result query must stay within +50 ms p50 on this repo.

## Success criteria
- Corpus test: every accepted shape reported, every look-alike (comment,
  unrelated string, different identifier containing the name) not reported.
- On this repo and one JS corpus (hono or express under /var/tmp), a symbol
  that really is dispatched dynamically gets a boundary line; a truly dead
  symbol gets none (or an explicit "no dynamic-dispatch sites found").
- README "What the Graph Does Not See" and ARCHITECTURE disclosure principle
  stay consistent with the new output.

## Open questions
- Whether `refs` (floor-less by design) needs it too — decide by whether refs
  can return empty for a dynamically-dispatched symbol; default: yes, same helper.
  Resolved r2: yes. `refs` returns empty for a symbol reached only through
  `getattr(ctrl, "on_save")` (fixture in `tests/dispatch_boundaries.rs`), so
  both `refs` surfaces use the same helper, gated to `--relation` all /
  `calls` / `references`.

## Decisions taken in implementation (r2)
- Trigger: the caller list SHOWN is empty (after the test filter), the query
  asks for callers (not `--direction callees`), and some definition of the
  name is a function or method. Types and constants get no field.
- Scan source: the indexed file list, read from disk (not FTS: `module` nodes
  carry no `code_content`, so top-level registration tables are invisible to
  FTS; not `rg`: the MCP surface has no ripgrep dependency). Test files and
  languages without a shape table are skipped; files over 2 MB too.
- A file that declares a local / parameter / pattern of the name has its bare
  uses read as the local's (qualified uses still count).
- Wording: `N dynamic-dispatch site(s) name 'x' (not graph edges):` / `(no
  dynamic-dispatch site names 'x')`; JSON `boundaries {total, sites[≤5],
  note, next}` or `{sites: [], total: 0}`.

# Change log
- r1 2026-09-28: created from the analysis doc; approved under the user's batch AUTH.
- r2 2026-09-28: implemented (`src/graph/boundaries/`, `tests/dispatch_boundaries.rs`); open question resolved; implementation decisions recorded.
- r3 2026-09-28: pre-release review repairs. The scan is linear (a bracket
  index per file replaces a scan per occurrence; the 240 KB nested repro went
  22.6 s → 45 ms) and stops at a 1-second per-query limit. The "none" line is
  printed only for a complete scan; otherwise the answer names what was not
  read (`not_scanned`: a definition's language with no shape table such as
  bash, files over 2 MB or not UTF-8, files past the time limit). Rust shapes
  narrowed: no `a.b` member value, `Q::b` only through a qualifier that owns
  a definition, generic-fn and same-line parameters bind, attributes are not
  values (tokio: 42 names with a site, 5 real → 5, all real).
