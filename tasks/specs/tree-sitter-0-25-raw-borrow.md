---
status: approved
revision: 2
---

# tree-sitter 0.25 + tree-sitter-rust 0.24: parse `&raw` borrows

## goal

A Rust binding named `raw` borrowed as `&raw`, `&raw[..]` or `&raw.field` parses
cleanly, so the enclosing expression — and sometimes the whole function — stops
being lost from the index. Measured 2026-09-13 (memory
`project_tree_sitter_raw_parse_gap`): 8 of 161 Rust files carried ERROR nodes
(29 nodes), 1 symbol lost (`cmd_affected`). README "Known limitations" says
9/163 at v0.150.0; `health-check` on 2026-09-25 names 2 files
(`src/mcp/server/mod.rs`, `src/storage/db.rs`).

## non-goals

- Upgrading any other grammar crate. Only `tree-sitter` (core, 0.24 → 0.25) and
  `tree-sitter-rust` (0.23 → 0.24) move.
- Changing any extractor logic beyond what an API break forces.

## constraints

- `tree-sitter-rust` 0.24 panics under core 0.24 (`LanguageError { version: 15 }`);
  both must move together. Under core 0.25 all 19 grammars runtime-load (memory).
- Prod dependency bump — authorized by the user 2026-09-25.
- The other 18 languages' extraction must not change: differential index of a
  multi-language corpus, old binary vs new, nodes + edges canonicalized without
  ids, must be byte-identical outside Rust.
- `INDEX_VERSION` 71 → 72: old indexes keep the lost symbols with no automatic
  route back (a file re-parses only when its content changes).
- Author ≠ reviewer: an independent fresh-subagent review before merging.

## success-criteria

1. `cargo test` (both feature legs as pre-commit runs them) green.
2. This repo indexed fresh: 0 Rust files with parse errors (baseline measured
   before the bump in the same way).
3. A test pinning the trigger shapes (`&raw`, `&raw[..]`, `&raw.field`) extracts
   the enclosing function, RED on the old grammar.
4. Differential corpus: in every file that parses without errors, every non-Rust
   language keeps identical symbols and edges (a `context_string` summary may
   change where it names a symbol in a damaged file); every other difference
   is in a file already parsed with errors. (r2 — r1 demanded byte-identical
   output for all non-Rust files, see change log.)
5. README "Known limitations" bullet removed/updated; CHANGELOG `## Unreleased`
   carries the rebuild notice and a pin-back path.

## open-questions

- None blocking. If core 0.25 changes any API the extractors use, the fix is
  mechanical; if it changes parse output for another grammar, criterion 4 fails
  and the change stops there for a decision.

# Change log

- r1 (2026-09-25): initial, from `docs/SESSION-HISTORY-ANALYSIS-2026-09-25.md` §2 item 2.6.
- r2 (2026-09-25): criterion 4 narrowed after measurement. Core 0.25 recovers
  already-damaged files differently: C++ +15 real symbols, 2 bogus nodes gone
  (C++, C#), 7 C++ renames (a name no longer cut short), and one Swift class
  lost (Alamofire `Protected`, 3 methods de-scoped, 12 inbound calls dropped).
  Clean files: identical symbols and edges in all 18 non-Rust languages.
  Independent pre-merge review reproduced all of it (HIGH-1). User accepted the
  Swift regression the same day. Also from that review: `PARSE_TIMEOUT_MS=0`
  kept as "no deadline" (MEDIUM-1), `for r in &raw {` added to the fixture
  (MEDIUM-2).
