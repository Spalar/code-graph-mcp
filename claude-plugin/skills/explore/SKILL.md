---
name: explore
description: |
  Understand code structure with the AST index before reading files one by
  one — when starting work in unfamiliar code, exploring a module before a
  change, or finding which file to edit. An overview lists a directory's
  symbols and their callers in one call.
---

# Explore Code (indexed project)

Use these BEFORE reading individual files:

| Need | Command | Replaces |
|------|---------|----------|
| Module structure | `code-graph-mcp overview <dir>` | 5+ Read calls |
| Project architecture | `code-graph-mcp map --compact` | ls + README |
| Who calls / what calls | `code-graph-mcp callgraph <symbol>` | Grep + manual trace |
| Find by concept | `code-graph-mcp search "concept"` (full-text) | 3+ Grep attempts |
| Impact before a signature change | `code-graph-mcp impact <symbol>` | Grep for callers |
| Every use (rename / remove) | `code-graph-mcp refs <symbol>`, then `grep -w` | Grep alone |

**Workflow**: overview first → Read only the file you will edit. Calls the graph could
not resolve (dynamic dispatch, reflection, unresolved imports) are not in it, so confirm
an empty answer with grep.
