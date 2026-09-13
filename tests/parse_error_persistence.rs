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
//! paths, updated as `(stored - parsed_this_run) + errored_this_run`, which is
//! the same rule for both run kinds: a full index parses everything, so the
//! subtraction empties the set and the result is exactly what this run saw.
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

    fs::remove_file(f.project.path().join("src/broken.rs")).unwrap();
    f.incremental();
    assert!(
        f.recorded().is_empty(),
        "the offending file is gone; a verdict naming it is stale, got {:?}",
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
