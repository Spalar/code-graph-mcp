//! Drift guards for the stack budget of the off-main-thread index pipeline.
//!
//! `walk_for_relations` recurses once per AST level up to `MAX_RELATION_DEPTH`,
//! so a sub-kilobyte source file can drive it to the cap. A stack overflow there
//! is an `abort`, not a panic, so it bypasses the serve loop's per-request
//! `catch_unwind` and takes the whole stdio session down — the one failure mode
//! the `panic = "abort"` note in Cargo.toml exists to prevent.
//!
//! The margin is build-profile dependent (unoptimized frames measured ~8x the
//! release ones), which is exactly why it is pinned by a constant and guarded
//! here instead of being left to whatever the release optimizer happens to buy.

use code_graph_mcp::domain::{INDEX_THREAD_STACK_SIZE, MAX_RELATION_DEPTH};
use code_graph_mcp::parser::relations::extract_relations;

/// A JS expression nested `depth` levels deep, each level a call so the walker
/// does real per-frame work rather than skipping through a thin arm.
fn nested_calls(depth: usize) -> String {
    format!("const y = {}1{};", "g(".repeat(depth), ")".repeat(depth))
}

/// The walk must survive its own recursion cap on a thread sized by
/// [`INDEX_THREAD_STACK_SIZE`] — the size `spawn_startup_indexing` uses.
///
/// Non-vacuous by construction: the input nests far past the cap, so the
/// assertion below only holds if the walker actually recursed all the way down
/// to `MAX_RELATION_DEPTH` and stopped there. A walk that bailed early (or an
/// input the grammar flattened) yields a different count and fails.
#[test]
fn relation_walk_survives_depth_cap_on_index_thread() {
    let src = nested_calls(MAX_RELATION_DEPTH * 2);

    let rels = std::thread::Builder::new()
        .stack_size(INDEX_THREAD_STACK_SIZE)
        .spawn(move || extract_relations(&src, "javascript").unwrap())
        .expect("spawn sized index thread")
        .join()
        .expect("walk must not unwind");

    // Each `g(...)` level costs two AST levels (call_expression + arguments),
    // so a cap of N levels admits ~N/2 calls. Tolerance absorbs the couple of
    // wrapper levels (program / statement / declarator) at the top of the tree.
    let expected = MAX_RELATION_DEPTH / 2;
    let calls = rels.iter().filter(|r| r.relation == "calls").count();
    assert!(
        calls.abs_diff(expected) <= 2,
        "expected ~{expected} call relations at the depth cap, got {calls} \
         (total relations {}) — the walk did not reach MAX_RELATION_DEPTH, so \
         this test is no longer measuring the deepest stack",
        rels.len()
    );
}

/// The startup index thread must keep an explicit stack size. `thread::spawn`'s
/// 2 MiB default is under the unoptimized peak of the walk above, and the
/// resulting abort is invisible to every panic-based defense in the server.
///
/// Scans by symbol, not by fixed path, so splitting `mcp/server` into more
/// files cannot silently retire the guard.
#[test]
fn startup_index_thread_declares_its_stack_size() {
    let server_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/mcp/server");
    let mut body = None;
    let mut scanned = 0usize;

    let mut stack = vec![server_dir.clone()];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).expect("read src/mcp/server") {
            let path = entry.expect("dir entry").path();
            if path.is_dir() {
                stack.push(path);
                continue;
            }
            if path.extension().is_none_or(|e| e != "rs") {
                continue;
            }
            scanned += 1;
            let src = std::fs::read_to_string(&path).expect("read source file");
            if let Some(start) = src.find("fn spawn_startup_indexing") {
                // Body ends at the next method declared at the same indentation.
                // Matching on the visibility spelling would miss `pub(super) fn`
                // and friends and run the region to EOF, which then trips the
                // no-`thread::spawn` assertion on an unrelated method — a false
                // red rather than a false green, but still noise.
                let rest = &src[start..];
                let end = rest[1..]
                    .match_indices("\n    ")
                    .map(|(i, _)| i + 1)
                    .find(|&i| {
                        let line = rest[i..].lines().nth(1).unwrap_or("").trim_start();
                        line.starts_with("fn ")
                            || (line.starts_with("pub") && line.contains(" fn "))
                    })
                    .unwrap_or(rest.len());
                body = Some(rest[..end].to_string());
            }
        }
    }

    assert!(scanned > 0, "guard scanned no files — src/mcp/server moved");
    let body = body.expect(
        "fn spawn_startup_indexing not found under src/mcp/server — \
         the guard must be repointed at wherever the index thread is now spawned",
    );
    // Match the CALL, not the constant's name: the spawn site is introduced by a
    // comment that names the constant, so asserting on the bare identifier passed
    // with `.stack_size(...)` deleted — the guard was vacuous until a mutation
    // test caught it.
    assert!(
        body.contains(".stack_size(crate::domain::INDEX_THREAD_STACK_SIZE)"),
        "spawn_startup_indexing must pass domain::INDEX_THREAD_STACK_SIZE to \
         Builder::stack_size (naming the constant in a comment is not sizing the thread)"
    );
    assert!(
        !body.contains("std::thread::spawn("),
        "spawn_startup_indexing must not use std::thread::spawn (2 MiB default \
         stack is below the unoptimized peak of the relation walk)"
    );
}

/// Phase 1a extracts relations on its worker threads, not on the index thread.
/// The index here runs on a thread sized like the startup one, so the parse
/// workers are the only threads this input can overflow. Measured need of this
/// walk in an unoptimized build: between 512 KiB and 1 MiB, so it would also
/// pass on rayon's 2 MiB default today; the budget itself is pinned by
/// `parse_workers_declare_their_stack_size` below.
#[test]
fn relation_walk_survives_depth_cap_in_the_parse_workers() {
    use code_graph_mcp::indexer::pipeline::run_full_index;
    use code_graph_mcp::storage::db::Database;

    let project = tempfile::TempDir::new().unwrap();
    std::fs::write(
        project.path().join("deep.js"),
        format!(
            "function g(x) {{ return x; }}\n{}\n",
            nested_calls(MAX_RELATION_DEPTH * 2)
        ),
    )
    .unwrap();
    let db_dir = tempfile::TempDir::new().unwrap();
    let db_path = db_dir.path().join("index.db");
    let root = project.path().to_path_buf();

    let calls = std::thread::Builder::new()
        .stack_size(INDEX_THREAD_STACK_SIZE)
        .spawn(move || {
            let db = Database::open(&db_path).unwrap();
            run_full_index(&db, &root, None, None).unwrap();
            db.conn()
                .query_row(
                    "SELECT COUNT(*) FROM edges e JOIN nodes t ON t.id = e.target_id
                     WHERE e.relation = 'calls' AND t.name = 'g'",
                    [],
                    |r| r.get::<_, i64>(0),
                )
                .unwrap()
        })
        .expect("spawn sized index thread")
        .join()
        .expect("index must not unwind");
    // The file was indexed: `<module>` calls `g`.
    assert_eq!(calls, 1);
}

/// Phase 1a's pool must keep an explicit stack size: rayon's default worker
/// stack is `thread::spawn`'s 2 MiB, and the relation walk runs there.
#[test]
fn parse_workers_declare_their_stack_size() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("src/indexer/pipeline/index_files.rs");
    let src = std::fs::read_to_string(&path).expect("read index_files.rs");
    let body = |name: &str| {
        let start = src
            .find(&format!("fn {name}("))
            .unwrap_or_else(|| panic!("fn {name} not found in {}", path.display()));
        let rest = &src[start..];
        rest[..rest.find("\n}\n").expect("function end")].to_string()
    };
    assert!(
        body("parse_pool").contains(".stack_size(crate::domain::INDEX_THREAD_STACK_SIZE)"),
        "parse_pool must pass domain::INDEX_THREAD_STACK_SIZE to ThreadPoolBuilder::stack_size"
    );
    let pre_parse = body("pre_parse_batch");
    assert!(
        pre_parse.contains("pool.install("),
        "pre_parse_batch must run its parallel iterator inside parse_pool's pool"
    );
    assert_eq!(
        pre_parse.matches("par_iter()").count(),
        1,
        "pre_parse_batch must have exactly one parallel iterator, the one inside pool.install"
    );
}
