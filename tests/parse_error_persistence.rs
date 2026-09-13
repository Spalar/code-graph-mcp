//! A degraded index must still say so after the process that built it is gone.
//!
//! Tree-sitter recovers from syntax errors by inserting ERROR nodes and still
//! returning a tree, so the file IS indexed — over a damaged parse, with symbols
//! silently missing. The indexer warns per file while it runs. Nothing carried
//! that past the run: `health-check` reported `healthy: true`, `stats`'
//! `parse_errors` is a different thing entirely (session metrics from
//! `usage.jsonl`), and the MCP `files_with_parse_errors` key only appears when
//! the SERVER process did the indexing, from in-memory state.
//!
//! # Why a path SET and not a count
//!
//! An incremental run parses only what changed. A count written by that run
//! describes the changed files alone, so one clean incremental after a full
//! index that found eight bad files would store `0` — "healthy", one edit to an
//! unrelated file later. The stored value is therefore the set of offending
//! paths, updated as `(stored - examined_this_run) + errored_this_run`, one rule
//! for both run kinds.
//!
//! EXAMINED, not parsed. An earlier version of this file said "a full index
//! parses everything, so the subtraction empties the set", and pre-ship review
//! showed that false: a full index parses everything it CAN, and a file set
//! aside as oversize / non-UTF-8 / unparseable kept its verdict through every
//! subsequent full rebuild. `a_file_the_run_examined_but_skipped_loses_its_verdict_too`
//! is the test for it.
//!
//! `incremental_run_over_other_files_keeps_the_verdict` is the test that
//! separates the set from the count — it is green for a count too, but only
//! because the count would be *wrong*, so it asserts the stored paths, not just
//! that something is stored.

use code_graph_mcp::indexer::pipeline::{run_full_index, run_incremental_index};
use code_graph_mcp::storage::db::Database;
use std::fs;
use tempfile::TempDir;

/// Unbalanced delimiters — invalid under any version of any Rust grammar.
///
/// Deliberately NOT the `&raw` shape (README "Known limitations"): that one is a
/// pinned-grammar quirk, so a tree-sitter upgrade would silently stop producing
/// the ERROR node and leave this file asserting nothing. Every test below
/// asserts the fixture really did error before asserting what was recorded.
const BROKEN: &str = "pub fn broken( { ] }\n";
const CLEAN_A: &str = "pub fn clean_a() -> i32 { 1 }\n";
const CLEAN_B: &str = "pub fn clean_b() -> i32 { 2 }\n";

struct Fixture {
    project: TempDir,
    _db_dir: TempDir,
    db: Database,
}

fn fixture() -> Fixture {
    let project = TempDir::new().unwrap();
    let db_dir = TempDir::new().unwrap();
    fs::create_dir_all(project.path().join("src")).unwrap();
    let db = Database::open(&db_dir.path().join("index.db")).unwrap();
    Fixture {
        project,
        _db_dir: db_dir,
        db,
    }
}

impl Fixture {
    fn write(&self, rel: &str, body: &str) {
        fs::write(self.project.path().join(rel), body).unwrap();
    }
    fn full(&self) -> usize {
        run_full_index(&self.db, self.project.path(), None, None)
            .unwrap()
            .stats
            .files_with_parse_errors
    }
    fn incremental(&self) -> usize {
        run_incremental_index(&self.db, self.project.path(), None, None)
            .unwrap()
            .stats
            .files_with_parse_errors
    }
    fn recorded(&self) -> Vec<String> {
        let mut v = self.db.parse_error_files().unwrap();
        v.sort();
        v
    }
}

#[test]
fn a_full_index_records_which_files_parsed_with_errors() {
    let f = fixture();
    f.write("src/broken.rs", BROKEN);
    f.write("src/clean_a.rs", CLEAN_A);

    assert_eq!(
        f.full(),
        1,
        "precondition: the fixture must actually trip tree-sitter error recovery, \
         or everything below asserts nothing"
    );
    assert_eq!(
        f.recorded(),
        vec!["src/broken.rs".to_string()],
        "the run's own warning is gone the moment the process exits; the index must keep it"
    );
}

#[test]
fn incremental_run_over_other_files_keeps_the_verdict() {
    let f = fixture();
    f.write("src/broken.rs", BROKEN);
    f.write("src/clean_a.rs", CLEAN_A);
    assert_eq!(f.full(), 1, "precondition");

    // The whole reason this is a set. This run parses clean_b.rs and nothing
    // else, so its own parse-error count is 0 — a stored COUNT would now read
    // "healthy" with a broken file still sitting in the index.
    f.write("src/clean_b.rs", CLEAN_B);
    assert_eq!(
        f.incremental(),
        0,
        "precondition: this run must NOT re-parse the broken file, or the test \
         cannot tell a set from a count"
    );
    assert_eq!(
        f.recorded(),
        vec!["src/broken.rs".to_string()],
        "a file nobody re-parsed cannot have been repaired"
    );
}

#[test]
fn repairing_the_file_clears_it_on_the_next_run() {
    let f = fixture();
    f.write("src/broken.rs", BROKEN);
    assert_eq!(f.full(), 1, "precondition");

    f.write("src/broken.rs", CLEAN_A);
    assert_eq!(
        f.incremental(),
        0,
        "precondition: the repair parses cleanly"
    );
    assert!(
        f.recorded().is_empty(),
        "a re-parsed file that no longer errors must leave the set — otherwise the \
         verdict is a one-way latch and the user can never get back to green, got {:?}",
        f.recorded()
    );
}

#[test]
fn a_deleted_file_stops_counting_against_the_index() {
    let f = fixture();
    f.write("src/broken.rs", BROKEN);
    f.write("src/clean_a.rs", CLEAN_A);
    assert_eq!(f.full(), 1, "precondition");

    // Two arms, because the end-to-end one cannot say WHICH mechanism cleared
    // it. The docs credit the read-side intersection with `files` specifically —
    // "whatever removes the row removes the claim with it" — and an incremental
    // run also does its own bookkeeping, so a single arm proves only that the
    // pair of them together works (pre-ship review, reviewer-flagged).

    // Arm 1, the mechanism on its own: drop the `files` row directly and read,
    // with NO index run in between. Nothing but the intersection can be acting.
    f.db.conn()
        .execute("DELETE FROM files WHERE path = ?1", ["src/broken.rs"])
        .unwrap();
    assert!(
        f.recorded().is_empty(),
        "the stored row still names it; only the read-side intersection with `files` \
         can drop it here, and that is the claim the doc makes, got {:?}",
        f.recorded()
    );

    // Arm 2, the realistic path: the file leaves the tree and an ordinary run
    // notices. Same answer, reached the way a user reaches it.
    let g = fixture();
    g.write("src/broken.rs", BROKEN);
    g.write("src/clean_a.rs", CLEAN_A);
    assert_eq!(g.full(), 1, "precondition");
    fs::remove_file(g.project.path().join("src/broken.rs")).unwrap();
    g.incremental();
    assert!(
        g.recorded().is_empty(),
        "the offending file is gone; a verdict naming it is stale, got {:?}",
        g.recorded()
    );

    // Arm 3: the stored ROW, not only what the reader makes of it.
    //
    // Arms 1 and 2 are blind to this by construction. `record_parse_error_files`
    // opens with `let previous = self.parse_error_files()?` — the INTERSECTED
    // read — and that is the only thing that prunes dead paths out of the row.
    // Turn that line into a raw `get_meta` some day and both arms above stay
    // green while the row accumulates dead paths forever (pre-ship review,
    // reviewer-named; the mutation is verified red against this arm).
    //
    // DELIBERATELY the delete-PLUS-PARSE shape. Do not "simplify" it back into a
    // delete-only run — the two flavours have OPPOSITE expected values for this
    // very assertion, and that is the whole reason this comment is long:
    //
    //   delete-only          -> write skipped (`if !parsed_paths.is_empty()`),
    //                           the dead path is STILL in the row, and only the
    //                           reader's intersection hides it. Arm 2 pins this.
    //   delete + any parse   -> the write runs, `previous` is the intersected
    //                           read, the `files` row is already gone (deletions
    //                           land in Phase 0, the merge runs after the batch
    //                           loop), so the dead path is pruned OUT of the row
    //                           and never reaches the reader. This arm pins it.
    //
    // A first draft of this arm was delete-only and failed on correct code. That
    // was the test contradicting a decision, not finding a defect. Touching a
    // second file is what puts the run in the second flavour.
    //
    // (Arm 1's raw `DELETE FROM files` cascades to `nodes`/`edges` only under
    // `PRAGMA foreign_keys = ON`. Harmless while it asserts on `files` alone —
    // it would matter the day someone gives it a node-count assertion.)
    let h = fixture();
    h.write("src/broken.rs", BROKEN);
    h.write("src/clean_a.rs", CLEAN_A);
    assert_eq!(h.full(), 1, "precondition");
    fs::remove_file(h.project.path().join("src/broken.rs")).unwrap();
    h.write("src/clean_a.rs", CLEAN_B); // gives the run parse work to do
    h.incremental();

    let raw: Option<String> =
        h.db.conn()
            .query_row(
                "SELECT value FROM meta WHERE key = ?1",
                ["parse_error_files"],
                |r| r.get(0),
            )
            .ok();
    assert!(
        !raw.as_deref().unwrap_or("").contains("src/broken.rs"),
        "a run that did parse work must prune the departed file OUT OF THE ROW, \
         not merely hide it behind the reader's intersection — got {raw:?}"
    );
}

#[test]
fn a_file_the_run_examined_but_skipped_loses_its_verdict_too() {
    // Pre-ship review, reviewer-reproduced. The fold's stated invariant — "a full
    // index parses everything, so the subtraction empties the set" — was false: a
    // full index parses everything it CAN. `pre_parse_batch` has three outcomes
    // and the drain read only `parsed`, so a file that errored and then became
    // unparseable-but-known (oversize here; non-UTF-8 and outright parse failure
    // take the same exit) kept its verdict through any number of FULL rebuilds.
    //
    // That is worse than a stale count: `health-check` names a file as damaged
    // when it is syntactically fine, and the remedy it implies — re-index —
    // provably does not clear it.
    //
    // `Skipped` means the bytes were read and identified and the file's nodes
    // were purged, so the old verdict is spent. `Nothing` (read failure, unknown
    // language) stays excluded: no identity was established, the file re-diffs
    // next run, and dropping a verdict on that basis would be guessing.
    let f = fixture();
    f.write("src/broken.rs", BROKEN);
    assert_eq!(f.full(), 1, "precondition");
    assert_eq!(
        f.recorded(),
        vec!["src/broken.rs".to_string()],
        "precondition"
    );

    // Repaired AND pushed past CODE_GRAPH_MAX_FILE_SIZE, so this run reads it,
    // identifies it, and skips it rather than parsing it.
    let mut grown = String::from(CLEAN_A);
    while grown.len() <= 1024 * 1024 {
        grown.push_str("// pad pad pad pad pad pad pad pad pad pad pad pad pad\n");
    }
    f.write("src/broken.rs", &grown);
    assert_eq!(
        f.full(),
        0,
        "precondition: the rebuild parsed nothing broken"
    );
    assert!(
        f.recorded().is_empty(),
        "a FULL rebuild must not keep a verdict about a file it re-examined; the \
         file is valid now and re-indexing is the remedy health-check implies, \
         got {:?}",
        f.recorded()
    );
}

#[test]
fn the_same_holds_for_the_other_skipped_arm_non_utf8() {
    // `Skipped` is reached from three conditions, and the test above exercises
    // exactly one of them (oversize). The reviewer who found the defect said so
    // plainly — "my repro used arm 2 only, so arms 4 and 5 are now fixed but
    // untested" — and a fix verified on one arm of three is the shape that
    // leaves the other two to rot.
    //
    // This is arm 4: bytes read, identity known, `String::from_utf8` refuses.
    //
    // Arm 5 (`parse_tree` returns Err, i.e. the parse TIMES OUT) is deliberately
    // not here. Its trigger is `CODE_GRAPH_PARSE_TIMEOUT_MS`, read through a
    // process-global `OnceLock` that latches on first use, so it is only
    // settable in a test binary that contains exactly one test — which is what
    // `tests/parse_failure_recording.rs` is, and that file already pins arm 5's
    // membership in `Skipped`. From the drain's point of view arms 4 and 5 are
    // the same statement (`for s in &pre_parsed.skipped`), so pinning one arm's
    // drain plus the other arm's membership covers the pair.
    let f = fixture();
    f.write("src/broken.rs", BROKEN);
    assert_eq!(f.full(), 1, "precondition");
    assert_eq!(
        f.recorded(),
        vec!["src/broken.rs".to_string()],
        "precondition"
    );

    // Valid Rust, then a lone 0xFF — readable bytes, not UTF-8.
    let mut bytes = CLEAN_A.as_bytes().to_vec();
    bytes.push(0xFF);
    fs::write(f.project.path().join("src/broken.rs"), bytes).unwrap();

    assert_eq!(
        f.full(),
        0,
        "precondition: the rebuild parsed nothing broken"
    );
    assert!(
        f.recorded().is_empty(),
        "a file the run read and identified but could not decode was still \
         re-examined, so its old verdict is spent — got {:?}",
        f.recorded()
    );
}

#[test]
fn a_clean_project_records_nothing() {
    // Negative control for all four above: the mechanism must be inert when
    // there is nothing to report, or "the set is non-empty" proves only that
    // something writes to it unconditionally.
    let f = fixture();
    f.write("src/clean_a.rs", CLEAN_A);
    assert_eq!(f.full(), 0, "precondition: nothing errors");
    assert!(f.recorded().is_empty(), "got {:?}", f.recorded());
}
