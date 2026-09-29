---
description: "Cleanup needing whole-repo reference checks: delete unreferenced private module-level functions (upstream networkx b1b678f, reverted: three dead helpers among ~150 private functions in scope)."
max_turns: 100
timeout_seconds: 1800
allowed_tools: [Read, Glob, Grep, Bash, Edit, Write, Agent]
tags: [coding]
---

Clean-up task in this networkx checkout: find module-level private functions (names starting with a single `_`) in the non-test modules under `networkx/algorithms/` and `networkx/generators/` that are never called or referenced anywhere in the repository (code, tests, docs or benchmarks), and delete them, together with any imports that only they used. Do not remove anything that is referenced somewhere. When you are done, list what you removed.
