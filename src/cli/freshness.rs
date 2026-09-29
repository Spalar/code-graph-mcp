use super::*;

use crate::indexer::pipeline::RefreshScope;

/// Outcome of a query-time freshness resync (`refresh_files_if_stale`) over the
/// files a read command is about to print. Callers re-run their query when
/// `any_changed` and call `disclose()` to honestly surface a partial refresh.
#[derive(Default, Debug)]
pub(crate) struct FreshOutcome {
    /// At least one displayed file was dirty and successfully re-indexed → the
    /// query must re-run so line numbers reflect the post-edit index.
    pub(crate) any_changed: bool,
    /// Dirty files re-indexed within budget.
    refreshed: usize,
    /// Dirty files left stale because the reindex budget was exhausted.
    skipped_over_budget: usize,
    /// Dirty files whose reindex failed (write contention / parse error) — kept
    /// stale, never worse than before.
    failed: usize,
}

impl FreshOutcome {
    /// Some displayed files stayed stale (budget exhausted or reindex failed),
    /// so the printed line numbers for those files may be pre-edit.
    fn is_partial(&self) -> bool {
        self.skipped_over_budget > 0 || self.failed > 0
    }

    /// One-line honest disclosure when the refresh was only partial. stderr only
    /// — stdout carries the JSON/text contract and must not be polluted — and
    /// dual-written to `tracing` per the project's user-facing-warning rule
    /// (`feedback_tracing_invisible_in_cli`). No-op when everything was fresh or
    /// fully refreshed.
    pub(crate) fn disclose(&self) {
        if !self.is_partial() {
            return;
        }
        let stale = self.skipped_over_budget + self.failed;
        let msg = format!(
            "{} file(s) changed since indexing; refreshed {}, line numbers for the rest may be stale (rerun after 'code-graph-mcp incremental-index')",
            stale, self.refreshed
        );
        // ONE channel (audit 2026-08-29 CON-06): the CLI installs a subscriber,
        // so the pair printed the same fact twice, once as prose and once as a
        // log line.
        tracing::warn!("{msg}");
    }

    /// In-band partial-freshness marker for OBJECT-shaped `--json` outputs
    /// (roadmap 2026-07-18 §1.4): the stderr note above is invisible under
    /// `--json 2>/dev/null`, so envelope emitters attach `freshness_partial:
    /// true` when some displayed files stayed stale. Array-shaped outputs
    /// (search/show/overview/similar/dead-code) cannot carry a top-level field
    /// without breaking their success shape — for those the stderr note remains
    /// the only channel (documented boundary). No-op when fully fresh.
    pub(crate) fn attach_partial(&self, obj: &mut serde_json::Value) {
        if self.is_partial() {
            obj["freshness_partial"] = serde_json::json!(true);
        }
    }
}

/// Query-time freshness resync shared by the read commands that print
/// `start_line`/`end_line` straight from the index (`show`, `refs`, `overview`,
/// `search`, `ast-search`, `trace`, `similar`, `impact`, `dead-code`).
///
/// A thin adapter over [`crate::indexer::resync::resync_stale_files`], which
/// owns the predicate and the batching for every surface (CLI, `grep`, MCP).
/// `paths` must be the POST-limit result set (what the command will print), not
/// the whole index. Callers re-run their query when the outcome reports
/// `any_changed`.
pub(crate) fn refresh_files_if_stale(db: &Database, root: &Path, paths: &[String]) -> FreshOutcome {
    resync(db, root, paths, RefreshScope::IndexedOnly)
}

/// Query-time freshness for paths the USER named, rather than paths a query
/// produced: `affected`'s changed-file list and `deps`' file argument.
///
/// Same machinery, one difference that matters — [`RefreshScope::IncludeNew`].
/// A result set must not pull an unknown path into the index (that would widen
/// the index on nothing but a query), but a path the caller typed is an
/// assertion that the file matters, exactly like an MCP tool's explicit
/// `file_path`. Without it, `affected` classified a file the branch had just
/// added as "not indexed", dropped it from the reverse closure, and printed
/// "0 test file(s) to re-run" — the one output a CI hook acts on
/// (audit 2026-08-29 CON-03).
pub(crate) fn refresh_input_files(db: &Database, root: &Path, paths: &[String]) -> FreshOutcome {
    resync(db, root, paths, RefreshScope::IncludeNew)
}

fn resync(db: &Database, root: &Path, paths: &[String], scope: RefreshScope) -> FreshOutcome {
    // Never let a concurrent writer (MCP watcher, another index run) stall an
    // interactive command for the default 5s busy_timeout — fail fast, keep stale.
    // Set and forgotten on purpose: unlike the MCP server's long-lived handle,
    // this is a short-lived CLI process.
    let _ = db.conn().execute_batch(&format!(
        "PRAGMA busy_timeout = {};",
        crate::indexer::resync::RESYNC_BUSY_TIMEOUT_MS
    ));

    let outcome = crate::indexer::resync::resync_stale_files(
        db,
        root,
        paths,
        crate::indexer::resync::resync_budget(),
        scope,
    );
    FreshOutcome {
        any_changed: outcome.any_changed(),
        refreshed: outcome.refreshed,
        skipped_over_budget: outcome.skipped_over_budget,
        failed: outcome.failed,
    }
}

/// Before a lookup: when no node carries the symbol's name at all, index the
/// unindexed files that mention it ([`index_new_files_naming`]), so the
/// command's own resolution — exact, qualified or fuzzy — sees them. One
/// indexed name query when the name exists; the walk only on a true absence.
pub(crate) fn index_new_files_if_absent(db: &Database, root: &Path, symbol: &str) {
    let name = symbol.rsplit(['.', ':']).next().unwrap_or(symbol);
    let present = crate::storage::queries::get_nodes_by_name(db.conn(), name)
        .map(|nodes| !nodes.is_empty())
        .unwrap_or(true);
    if !present {
        index_new_files_naming(db, root, symbol);
    }
}

/// On a symbol-name miss, index the files the index does not hold yet that
/// mention the name, so the lookup can try again (2026-09-28 usage evaluation
/// D2). A symbol ADDED since the last index lives in such a file, and no
/// result-set refresh reaches it: every lookup missed it until a manual
/// `incremental-index` (the hook audit reproduced it 4/4). Only files whose
/// bytes contain the name's last segment are read in, at most the resync
/// budget of them, so a miss does not turn into an index run over the tree.
/// Returns true when a file was indexed; the caller then resolves again.
pub(crate) fn index_new_files_naming(db: &Database, root: &Path, symbol: &str) -> bool {
    let name = symbol.rsplit(['.', ':']).next().unwrap_or(symbol);
    if name.len() < 2 {
        return false;
    }
    let Ok(walked) = crate::indexer::merkle::walk_indexable_files(root) else {
        return false;
    };
    let known: std::collections::HashSet<String> = match db
        .conn()
        .prepare("SELECT path FROM files")
        .and_then(|mut s| s.query_map([], |r| r.get::<_, String>(0))?.collect())
    {
        Ok(known) => known,
        Err(_) => return false,
    };
    let limit = crate::domain::max_file_size();
    let budget = crate::indexer::resync::resync_budget();
    let naming: Vec<String> = walked
        .into_iter()
        .filter(|(rel, _)| !known.contains(rel))
        .filter(|(_, abs)| {
            std::fs::metadata(abs).is_ok_and(|m| m.len() <= limit)
                && std::fs::read(abs)
                    .is_ok_and(|bytes| bytes.windows(name.len()).any(|w| w == name.as_bytes()))
        })
        .map(|(rel, _)| rel)
        .take(budget)
        .collect();
    !naming.is_empty() && refresh_input_files(db, root, &naming).any_changed
}
