# Trellis Fork Patches

This submodule is a **fork** of upstream `code-graph-mcp` (branch
`trellis-fork`). It exists so Trellis can carry security fixes that are not
yet (or may never be) accepted upstream, while still tracking upstream
releases via the gitlink in the main trellis repo.

**Do not merge this branch upstream.** Upstream stays clean; our deltas live
here as small, marked patches.

## Current patches

All patches are marked `TRELLIS FORK:` in the source so a conflict or a lost
hunk is visible in review, and so `git diff upstream-tag..HEAD` enumerates
them.

| Patch | Files | Why |
|-------|-------|-----|
| Refuse a non-regular file at the DB path before open/wipe | `src/storage/db.rs` (`open_impl`, `open_readonly`) | rusqlite follows a symlink at `.code-graph/index.db`; a repo shipping one as a link could have the link target initialized in place (serve startup) or deleted by corruption recovery (`rebuild-index`). Applies the same `refuse_non_regular` policy the owned-file module uses for every other write under `.code-graph/`. |
| Freshness path must not follow symlinks | `src/indexer/pipeline/mod.rs` (`plan_file_refresh`) | The index walk never follows symlinks and the read path canonicalizes under the project root, but the query-time freshness check used `Path::is_file()` (follows links). A malicious repo could ship `leak.py -> <sensitive file>` and get the target's contents indexed on the next tool call. Non-regular files are now treated like missing files. |
| Document the closed gap | `src/utils/owned.rs` | Notes that `index.db` does not come through the owned-file module and therefore needed the guards above. |

Tests: `open_refuses_symlinked_db_and_target_survives` and siblings in
`src/storage/db.rs`, plus the freshness refusal tests in
`src/indexer/pipeline/tests.rs`.

## How to update from upstream

1. `git fetch upstream && git merge <new-tag>` (or rebase) into `trellis-fork`.
2. Resolve conflicts by keeping the `TRELLIS FORK:` blocks.
3. Re-run: `cargo test storage::` and `cargo test indexer::pipeline` (must
   stay green; the patches add tests that fail if the guards regress).
4. Rebuild `bin/code-graph-mcp.exe` (`scripts/build_bridge.py` in the main
   repo) and bump `bin/version.txt`.
5. In the main repo, update the gitlink and commit with the upstream version
   in the message.

## Relationship to the audit fixes in Cargo.lock

`Cargo.lock` upgrades (e.g. rustls, crossbeam-epoch) track upstream release
tags that ship RUSTSEC fixes; they are not fork patches.
