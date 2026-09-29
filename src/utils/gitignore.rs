//! Keeping `.code-graph/` out of the user's commits.
//!
//! The index directory holds a multi-hundred-MB SQLite file that is a pure
//! cache — committing it is never what the user wants, and `git add -A` will do
//! exactly that unless git is told to ignore it. The write used to live inside
//! `McpServer::from_project_root`, so a pure-CLI install (hook-driven
//! `incremental-index`, never starting the MCP server) left a fresh repo with an
//! untracked `.code-graph/` and no ignore entry (audit 2026-08-02 DB-4).
//!
//! The entry goes to the repository's local exclude file, `info/exclude` in the
//! git dir, never to the tracked `.gitignore` (decision D3, 2026-09-28 usage
//! evaluation). Appending to `.gitignore` changed a file the user commits in
//! every repo the tool ran in; in 12 of 15 coding-eval runs Claude then stopped
//! to explain the unexpected `.gitignore` / `CLAUDE.md` changes to the user.

use std::path::{Path, PathBuf};

use crate::domain::CODE_GRAPH_DIR;

/// Ensure git ignores `<project_root>/.code-graph/`.
///
/// Nothing is written when `.gitignore` or `info/exclude` already names the
/// directory (either spelling), or when `project_root` is not a git work tree:
/// with no git there is nothing to commit it into.
///
/// Idempotent and best-effort: an unwritable exclude file is a warning, never an
/// error — indexing must not fail because the ignore rule could not be written.
/// Appends (rather than read-modify-write) so a concurrent writer's line cannot
/// be clobbered.
///
/// Shared by both index-creating entry points — the MCP server's
/// `from_project_root` and the CLI index commands — so the two cannot drift.
///
/// Set `CODE_GRAPH_NO_GITIGNORE=1` to disable this entirely — for a user whose
/// own ignore rules (e.g. a global `core.excludesFile`) already cover
/// `.code-graph/`.
pub(crate) fn ensure_code_graph_dir_ignored(project_root: &Path) {
    let disabled = std::env::var("CODE_GRAPH_NO_GITIGNORE").ok().as_deref() == Some("1");
    ensure_code_graph_dir_ignored_unless(project_root, disabled);
}

fn names_code_graph_dir(content: &str) -> bool {
    // Both `.code-graph` and `.code-graph/`, so a hand-written entry does not get
    // a duplicate appended on every run.
    content
        .lines()
        .any(|line| line.trim().trim_end_matches('/') == CODE_GRAPH_DIR)
}

/// The exclude file git reads for `project_root`: `<git-dir>/info/exclude`, where
/// a linked worktree's or submodule's `.git` FILE (`gitdir: …`) is followed, and
/// a worktree's `commondir` taken into account — git reads `info/exclude` from
/// the common dir, not from `.git/worktrees/<name>`. `None` when `project_root`
/// has no `.git`.
///
/// A `.git` that is not a plain directory — a `gitdir:` file or a symlink —
/// can point anywhere, so it is followed only to a git dir (one with a
/// `HEAD`): an unpacked project must not aim the write outside itself (pre-tag
/// review 2026-09-29).
fn exclude_path(project_root: &Path) -> Option<PathBuf> {
    let dot_git = project_root.join(".git");
    let meta = std::fs::symlink_metadata(&dot_git).ok()?;
    let is_git_dir = |d: &Path| d.join("HEAD").is_file();
    let git_dir = if meta.is_dir() {
        dot_git
    } else if meta.file_type().is_symlink() {
        Some(dot_git).filter(|d| d.is_dir() && is_git_dir(d))?
    } else {
        let raw = std::fs::read_to_string(&dot_git).ok()?;
        let target = raw
            .lines()
            .find_map(|l| l.strip_prefix("gitdir:"))?
            .trim()
            .to_string();
        let git_dir = Some(project_root.join(target)).filter(|d| is_git_dir(d))?;
        match std::fs::read_to_string(git_dir.join("commondir")) {
            Ok(common) => Some(git_dir.join(common.trim())).filter(|d| is_git_dir(d))?,
            Err(_) => git_dir,
        }
    };
    Some(git_dir.join("info").join("exclude"))
}

/// [`ensure_code_graph_dir_ignored`] with the switch already read.
///
/// The env read stays in the caller so tests can drive BOTH arms by argument.
/// Setting `CODE_GRAPH_NO_GITIGNORE` from a test instead would be process-global
/// while sibling tests in this module call the public entry point on other
/// threads — the same `env::set_var` race the embedding tests removed by
/// injection (`src/embedding/model.rs`, `record_download_state_at`).
fn ensure_code_graph_dir_ignored_unless(project_root: &Path, disabled: bool) {
    if disabled {
        return;
    }
    // An existing `.gitignore` entry (every repo this tool indexed before the
    // switch to `info/exclude`) already does the job; leave both files alone.
    let gitignore = std::fs::read_to_string(project_root.join(".gitignore")).unwrap_or_default();
    if names_code_graph_dir(&gitignore) {
        return;
    }
    let Some(exclude) = exclude_path(project_root) else {
        return;
    };
    let content = std::fs::read_to_string(&exclude).unwrap_or_default();
    if names_code_graph_dir(&content) {
        return;
    }
    if let Some(info) = exclude.parent() {
        if let Err(e) = std::fs::create_dir_all(info) {
            tracing::warn!("Could not create {}: {}", info.display(), e);
            return;
        }
    }
    use std::io::Write as _;
    // Through `owned::append_owned`: a repo-supplied path can be a symlink, and a
    // plain append would follow it into the target (audit 2026-08-29 SEC-03).
    // Best-effort — a refusal is a warning.
    match crate::utils::owned::append_owned(&exclude) {
        Ok(mut f) => {
            if !content.ends_with('\n') && !content.is_empty() {
                let _ = f.write_all(b"\n");
            }
            let _ = f.write_all(format!("{}/\n", CODE_GRAPH_DIR).as_bytes());
        }
        Err(e) => tracing::warn!("Could not update {}: {}", exclude.display(), e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A repo with a `.git` dir and no ignore entry anywhere.
    fn repo() -> tempfile::TempDir {
        let root = tempfile::TempDir::new().unwrap();
        std::fs::create_dir(root.path().join(".git")).unwrap();
        root
    }

    fn exclude_of(root: &Path) -> String {
        std::fs::read_to_string(root.join(".git/info/exclude")).unwrap_or_default()
    }

    #[test]
    fn writes_the_entry_to_info_exclude_and_never_to_gitignore() {
        let root = repo();
        ensure_code_graph_dir_ignored_unless(root.path(), false);
        assert_eq!(exclude_of(root.path()), ".code-graph/\n");
        assert!(
            !root.path().join(".gitignore").exists(),
            "the tracked .gitignore must not be created"
        );
    }

    #[test]
    fn leaves_an_existing_gitignore_untouched() {
        let root = repo();
        let gi = root.path().join(".gitignore");
        std::fs::write(&gi, "node_modules\n").unwrap();
        ensure_code_graph_dir_ignored_unless(root.path(), false);
        assert_eq!(std::fs::read_to_string(&gi).unwrap(), "node_modules\n");
        assert_eq!(exclude_of(root.path()), ".code-graph/\n");
    }

    #[test]
    fn appends_after_a_missing_trailing_newline_without_joining_lines() {
        let root = repo();
        std::fs::create_dir_all(root.path().join(".git/info")).unwrap();
        std::fs::write(root.path().join(".git/info/exclude"), "*.swp").unwrap();
        ensure_code_graph_dir_ignored_unless(root.path(), false);
        assert_eq!(exclude_of(root.path()), "*.swp\n.code-graph/\n");
    }

    /// Every repo indexed before the switch already has a `.gitignore` entry.
    /// That is enough; writing a second rule to `info/exclude` would be noise.
    #[test]
    fn an_existing_gitignore_entry_is_enough() {
        for existing in [".code-graph/\n", ".code-graph\n"] {
            let root = repo();
            std::fs::write(root.path().join(".gitignore"), existing).unwrap();
            ensure_code_graph_dir_ignored_unless(root.path(), false);
            assert!(
                !root.path().join(".git/info/exclude").exists(),
                "{existing:?} already ignores it; nothing more to write"
            );
        }
    }

    /// Idempotence across BOTH spellings in the exclude file itself.
    #[test]
    fn is_idempotent_for_both_slash_spellings() {
        for existing in [".code-graph/\n", ".code-graph\n"] {
            let root = repo();
            std::fs::create_dir_all(root.path().join(".git/info")).unwrap();
            std::fs::write(root.path().join(".git/info/exclude"), existing).unwrap();
            ensure_code_graph_dir_ignored_unless(root.path(), false);
            ensure_code_graph_dir_ignored_unless(root.path(), false);
            assert_eq!(exclude_of(root.path()), existing);
        }
    }

    /// Outside a git work tree there is nothing to commit the index into, and no
    /// exclude file to write: touch nothing.
    #[test]
    fn writes_nothing_outside_a_git_work_tree() {
        let root = tempfile::TempDir::new().unwrap();
        ensure_code_graph_dir_ignored_unless(root.path(), false);
        assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 0);
    }

    /// A linked worktree's `.git` is a FILE pointing at
    /// `<main>/.git/worktrees/<name>`, whose `commondir` leads back to
    /// `<main>/.git`. git reads `info/exclude` from the common dir.
    #[test]
    fn a_linked_worktree_writes_to_the_common_git_dir() {
        let dir = tempfile::TempDir::new().unwrap();
        let main_git = dir.path().join("main/.git");
        let wt_git = main_git.join("worktrees/feat");
        std::fs::create_dir_all(&wt_git).unwrap();
        std::fs::write(main_git.join("HEAD"), "ref: refs/heads/main\n").unwrap();
        std::fs::write(wt_git.join("HEAD"), "ref: refs/heads/feat\n").unwrap();
        std::fs::write(wt_git.join("commondir"), "../..\n").unwrap();
        let wt = dir.path().join("feat");
        std::fs::create_dir_all(&wt).unwrap();
        std::fs::write(wt.join(".git"), format!("gitdir: {}\n", wt_git.display())).unwrap();

        ensure_code_graph_dir_ignored_unless(&wt, false);

        assert_eq!(
            std::fs::read_to_string(main_git.join("info/exclude")).unwrap(),
            ".code-graph/\n"
        );
        assert!(!wt_git.join("info").exists(), "not the per-worktree dir");
        assert!(!wt.join(".gitignore").exists());
    }

    /// A `.git` FILE can point anywhere; only a git dir (one with a `HEAD`)
    /// gets the rule, so an unpacked project cannot aim the write outside
    /// itself (pre-tag review 2026-09-29: `gitdir: ../victim2` created
    /// `victim2/info/exclude`).
    #[test]
    fn a_gitdir_pointer_to_a_non_git_dir_writes_nothing() {
        let dir = tempfile::TempDir::new().unwrap();
        let victim = dir.path().join("victim");
        std::fs::create_dir_all(&victim).unwrap();
        let proj = dir.path().join("proj");
        std::fs::create_dir_all(&proj).unwrap();
        std::fs::write(proj.join(".git"), "gitdir: ../victim\n").unwrap();

        ensure_code_graph_dir_ignored_unless(&proj, false);

        assert!(!victim.join("info").exists(), "wrote outside the project");
        assert!(!proj.join(".gitignore").exists());
    }

    /// A `.git` symlinked to a git dir gets the rule in that dir, as git reads
    /// it; 0.163.0's `.gitignore` covered this layout and the switch to
    /// `info/exclude` had dropped it (pre-tag review 2026-09-29).
    #[cfg(unix)]
    #[test]
    fn a_symlinked_git_dir_gets_the_rule() {
        let dir = tempfile::TempDir::new().unwrap();
        let real = dir.path().join("real.git");
        std::fs::create_dir_all(&real).unwrap();
        std::fs::write(real.join("HEAD"), "ref: refs/heads/main\n").unwrap();
        let proj = dir.path().join("proj");
        std::fs::create_dir_all(&proj).unwrap();
        std::os::unix::fs::symlink(&real, proj.join(".git")).unwrap();

        ensure_code_graph_dir_ignored_unless(&proj, false);

        assert_eq!(
            std::fs::read_to_string(real.join("info/exclude")).unwrap_or_default(),
            ".code-graph/\n"
        );
    }

    /// A worktree's `commondir` is followed only to a git dir as well: a
    /// crafted `.git` dir with a `HEAD` could otherwise aim it anywhere
    /// (second review round: `commondir` = `../../victim-rel`).
    #[test]
    fn a_commondir_to_a_non_git_dir_writes_nothing() {
        let dir = tempfile::TempDir::new().unwrap();
        let proj = dir.path().join("proj");
        let fake = proj.join(".fakegit");
        std::fs::create_dir_all(&fake).unwrap();
        std::fs::write(fake.join("HEAD"), "ref: refs/heads/main\n").unwrap();
        std::fs::write(fake.join("commondir"), "../../victim\n").unwrap();
        std::fs::write(proj.join(".git"), "gitdir: .fakegit\n").unwrap();

        ensure_code_graph_dir_ignored_unless(&proj, false);

        assert!(!dir.path().join("victim").exists(), "created a dir outside");
    }

    /// The symlink arm of the same rule: a `.git` symlinked to a directory
    /// that is no git dir gets nothing written into it.
    #[cfg(unix)]
    #[test]
    fn a_git_symlink_to_a_non_git_dir_writes_nothing() {
        let dir = tempfile::TempDir::new().unwrap();
        let victim = dir.path().join("victim");
        std::fs::create_dir_all(&victim).unwrap();
        let proj = dir.path().join("proj");
        std::fs::create_dir_all(&proj).unwrap();
        std::os::unix::fs::symlink(&victim, proj.join(".git")).unwrap();

        ensure_code_graph_dir_ignored_unless(&proj, false);

        assert!(!victim.join("info").exists(), "wrote outside the project");
    }

    /// A repo can ship a symlink where the exclude file goes. The append must
    /// not follow it into the target (audit 2026-08-29 SEC-03).
    #[cfg(unix)]
    #[test]
    fn refuses_to_append_through_a_symlinked_exclude() {
        let dir = tempfile::TempDir::new().unwrap();
        let root = dir.path().join("repo");
        std::fs::create_dir_all(root.join(".git/info")).unwrap();
        let victim = dir.path().join("victim.conf");
        std::fs::write(&victim, "keep = 1\n").unwrap();
        std::os::unix::fs::symlink(&victim, root.join(".git/info/exclude")).unwrap();

        ensure_code_graph_dir_ignored_unless(&root, false);

        assert_eq!(
            std::fs::read_to_string(&victim).unwrap(),
            "keep = 1\n",
            "the link target must not be appended to"
        );

        // Positive control: a regular repo next to it still gets the entry.
        let ok = repo();
        ensure_code_graph_dir_ignored_unless(ok.path(), false);
        assert_eq!(exclude_of(ok.path()), ".code-graph/\n");
    }

    /// The switch disables the write entirely.
    #[test]
    fn the_switch_suppresses_the_write() {
        let root = repo();
        ensure_code_graph_dir_ignored_unless(root.path(), true);
        assert!(!root.path().join(".git/info/exclude").exists());
        assert!(!root.path().join(".gitignore").exists());

        // Positive control: the same call with the switch off still writes.
        let control = repo();
        ensure_code_graph_dir_ignored_unless(control.path(), false);
        assert_eq!(exclude_of(control.path()), ".code-graph/\n");
    }
}
