---
name: code-graph-mcp plugin contract
description: When to use the code-graph CLI / MCP tools instead of rounds of Grep + Read, and what the graph cannot see
type: reference
---
# code-graph-mcp: when to reach for it

Written by `code-graph-mcp adopt` to `.claude/plugin_code_graph_mcp.md`, next to the
CLAUDE.md block; `code-graph-mcp unadopt` removes both. The CLI runs from Bash with no
tool loading; the MCP tools return the same data but are deferred behind ToolSearch in
Claude Code.

## Pick the command by the question

| Question | CLI | MCP tool |
|---|---|---|
| Who calls X / what does X call? | `code-graph-mcp callgraph X` | `get_call_graph` |
| What breaks if X's signature changes? | `code-graph-mcp impact X --change-type signature` | `get_ast_node include_impact=true` |
| Every use of X (rename / remove) | `code-graph-mcp refs X`, then `grep -w X` | `find_references` |
| Source and signature of X | `code-graph-mcp show X` | `get_ast_node` |
| What is in this dir or file? | `code-graph-mcp overview <path>` | `module_overview` |
| Find code by what it does | `code-graph-mcp search "words"` (full-text) | `semantic_code_search` (adds vector ranking once embeddings exist) |
| Text search, each hit with its enclosing fn | `code-graph-mcp grep "pat" [path]` | — |
| Tests that load these files | `code-graph-mcp affected <files>` | — |
| HTTP route to its handler chain | `code-graph-mcp trace "GET /api/x"` | `get_call_graph route_path="GET /api/x"` |
| Project layout | `code-graph-mcp map --compact` | `project_map` |
| Symbols by kind, return type or params | `code-graph-mcp ast-search "q" --type fn --returns T` | `ast_search` |

`grep` takes `-F -i -w -l -c`, `-t <lang>`, `-g <glob>`, `-A/-B/-C N` and exits like grep
(0 hits, 1 none, 2 error). Large answers: `callgraph` and `overview` accept `--budget N`.

## What the graph cannot see

- A call it could not resolve leaves no edge: dynamic dispatch, reflection, callbacks
  passed as values, unresolved imports. An empty caller list is not proof that nothing
  calls X — check with grep before deleting or renaming. `impact` reports such a
  function as `Risk: UNKNOWN`.
- `callgraph` and `impact` hide by-name `ambiguous` edges by default
  (`--min-confidence ambiguous` shows them); `refs` shows every tier. Never narrow a
  rename audit with `--min-confidence extracted`: that tier is same-file only and drops
  every cross-file caller.
- A JS function held only by a variable (`const f = function () {}`) or a computed
  member (`obj[k] = …`) is not a node. A call on a receiver with no written type
  (hono's `c.json()`) binds by name and is often `ambiguous`, hidden by default.
- `affected` follows resolved imports; with unresolved ones (`require('..')`) its test
  list is a lower bound.
- `dead-code` is dependable for Rust. Elsewhere read it as candidates: interface
  methods, re-exports and framework entry points show up in it.

## Still use the plain tools

- Grep for literal strings, config and non-code files; Read the file you are about to edit.
- Answers look empty or stale: `code-graph-mcp health-check`, then
  `code-graph-mcp incremental-index`.

## Switches (`env` in `~/.claude/settings.json`)

- `CODE_GRAPH_QUIET_HOOKS=1` — hooks stop injecting context.
- `CODE_GRAPH_NO_TEMPLATE_REFRESH=1` — no notice when this file drifts from the shipped one.

All commands: `code-graph-mcp --help`. All switches: README, "Environment variables".
