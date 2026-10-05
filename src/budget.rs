//! Budgeted output (P1 #2): `--budget <tokens>` on the CLI and `max_tokens` on
//! MCP for `map`/`project_map`, `overview`/`module_overview`,
//! `callgraph`/`get_call_graph` and `show`/`get_ast_node`.
//!
//! The caller names a size; the tool ranks what it would print and gives the
//! low-ranked part less room. Each surface splits its answer into *units* (a
//! module, a symbol line, a call-graph node, a definition) and says, per unit,
//! what it looks like at each [`Level`]. This module decides the levels:
//!
//! 1. every unit that has a shorter form goes to [`Level::Skeleton`], least
//!    important first;
//! 2. then units are dropped, least important first;
//!
//! and stops at the first point where the rendered answer fits (Aider's
//! repo-map idea: render, count, search — binary search over the number of
//! steps taken). A unit is always rendered whole at its level, so nothing is
//! cut in the middle of a member. Size is counted the way the rest of the
//! codebase counts it, `bytes / CHARS_PER_TOKEN` of the exact text the caller
//! receives; there is no tokenizer.
//!
//! Every surface that leaves something out ends with a runnable command that
//! returns it ([`NextCommand`]).

use std::collections::HashMap;

use anyhow::Result;
use rusqlite::Connection;

/// Smallest accepted budget, in tokens. Below this even the fixed part of most
/// answers (headers, notices, the next-step line) does not fit.
pub const MIN_BUDGET_TOKENS: u64 = 100;
/// Largest accepted budget, in tokens.
pub const MAX_BUDGET_TOKENS: u64 = 100_000;

/// Share of the budget one file may take where files are the unit (`overview`):
/// codegraph's explore-budget rule, so one huge file cannot crowd out the rest.
pub const FILE_SHARE_PERCENT: usize = 70;

/// Budget in bytes for a token budget, using the shared bytes/token ratio.
pub fn budget_bytes(tokens: usize) -> usize {
    tokens.saturating_mul(crate::domain::CHARS_PER_TOKEN)
}

/// How much of one unit is rendered. Ordered: a later variant is smaller.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Level {
    /// As the unbudgeted answer renders it.
    Full,
    /// The identifying line only (name, signature, `file:line`) — no body, no
    /// member list.
    Skeleton,
    /// Not rendered; counted in the omission notice.
    Dropped,
}

/// One degradation step: lower `unit` to `level`.
pub type Step = (usize, Level);

/// The standard step order: every unit that has a skeleton goes to
/// [`Level::Skeleton`] (in `order`, least important first), then every unit is
/// dropped (same order).
pub fn standard_steps(order: &[usize], has_skeleton: impl Fn(usize) -> bool) -> Vec<Step> {
    let mut steps: Vec<Step> = order
        .iter()
        .copied()
        .filter(|&u| has_skeleton(u))
        .map(|u| (u, Level::Skeleton))
        .collect();
    steps.extend(order.iter().map(|&u| (u, Level::Dropped)));
    steps
}

/// The levels after applying the first `n` steps to `initial`. A step never
/// raises a level (a unit already dropped stays dropped).
pub fn levels_after(initial: &[Level], steps: &[Step], n: usize) -> Vec<Level> {
    let mut levels = initial.to_vec();
    for &(unit, level) in &steps[..n.min(steps.len())] {
        if level > levels[unit] {
            levels[unit] = level;
        }
    }
    levels
}

/// Outcome of [`fit`].
pub struct Fitted<T> {
    pub output: T,
    pub levels: Vec<Level>,
    /// Size of `output` in bytes, as measured by the render callback.
    pub bytes: usize,
    /// Even with every step applied the answer is larger than the budget (the
    /// fixed part alone does not fit).
    pub over_budget: bool,
}

impl<T> Fitted<T> {
    /// Whether any unit is below [`Level::Full`].
    pub fn degraded(&self) -> bool {
        self.levels.iter().any(|l| *l != Level::Full)
    }
    pub fn count(&self, level: Level) -> usize {
        self.levels.iter().filter(|l| **l == level).count()
    }
}

/// Apply the shortest prefix of `steps` whose rendering is at most
/// `budget_bytes`. `render` returns the output and its size in bytes.
///
/// Binary search assumes size does not grow as steps are applied, which holds
/// up to the digits of an omission count; the result is re-checked and, if the
/// assumption failed, the search continues linearly, so the returned output
/// fits whenever any prefix does.
pub fn fit<T>(
    initial: &[Level],
    steps: &[Step],
    budget_bytes: usize,
    mut render: impl FnMut(&[Level]) -> (T, usize),
) -> Fitted<T> {
    let at = |n: usize, render: &mut dyn FnMut(&[Level]) -> (T, usize)| {
        let levels = levels_after(initial, steps, n);
        let (output, bytes) = render(&levels);
        Fitted {
            output,
            levels,
            bytes,
            over_budget: false,
        }
    };
    let first = at(0, &mut render);
    if first.bytes <= budget_bytes || steps.is_empty() {
        let over = first.bytes > budget_bytes;
        return Fitted {
            over_budget: over,
            ..first
        };
    }
    // Invariant: `lo` does not fit; find the smallest n in (lo, hi] that fits.
    let (mut lo, mut hi) = (0usize, steps.len());
    while hi - lo > 1 {
        let mid = lo + (hi - lo) / 2;
        let (_, bytes) = render(&levels_after(initial, steps, mid));
        if bytes <= budget_bytes {
            hi = mid;
        } else {
            lo = mid;
        }
    }
    let mut n = hi;
    let fitted = loop {
        let candidate = at(n, &mut render);
        if candidate.bytes <= budget_bytes {
            break candidate;
        }
        if n == steps.len() {
            return Fitted {
                over_budget: true,
                ..candidate
            };
        }
        n += 1;
    };
    fill(initial, &steps[..n], budget_bytes, fitted, &mut render)
}

/// Most renders [`fill`] spends. Each is a full render of the answer, so the
/// bound keeps a budgeted call's cost linear in the answer's size.
const FILL_TRIES: usize = 256;

/// Undo applied steps where the room allows, most important unit first (the
/// reverse of the order they were applied in): a dropped unit comes back at
/// its shorter form, then shorter forms come back whole. The prefix search
/// alone stops at the first fit, which undershoots whenever the last step
/// removed a large unit; this is the knapsack half of the fit. A unit is still
/// only ever rendered whole at some level.
fn fill<T>(
    initial: &[Level],
    applied: &[Step],
    budget_bytes: usize,
    mut best: Fitted<T>,
    render: &mut dyn FnMut(&[Level]) -> (T, usize),
) -> Fitted<T> {
    let has_skeleton_step = |u: usize| applied.iter().any(|&(v, l)| v == u && l == Level::Skeleton);
    let mut tries = 0usize;
    for &(unit, level) in applied.iter().rev() {
        if tries >= FILL_TRIES {
            break;
        }
        if best.levels[unit] != level {
            continue;
        }
        let up = match level {
            Level::Dropped if has_skeleton_step(unit) => Level::Skeleton,
            _ => Level::Full,
        };
        let up = up.max(initial[unit]);
        if up >= level {
            continue;
        }
        let mut levels = best.levels.clone();
        levels[unit] = up;
        tries += 1;
        let (output, bytes) = render(&levels);
        if bytes <= budget_bytes {
            best = Fitted {
                output,
                levels,
                bytes,
                over_budget: false,
            };
        }
    }
    best
}

/// Lower units of one group (least important first, `group_order`) until the
/// group's summed cost is at most `cap_bytes`. Used for the per-file share
/// where files are the unit: costs are additive per line, so the group's size
/// is known without rendering the whole answer.
pub fn cap_group(
    levels: &mut [Level],
    group_order: &[usize],
    has_skeleton: impl Fn(usize) -> bool,
    cost: impl Fn(usize, Level) -> usize,
    cap_bytes: usize,
) {
    let total =
        |levels: &[Level]| -> usize { group_order.iter().map(|&u| cost(u, levels[u])).sum() };
    if total(levels) <= cap_bytes {
        return;
    }
    for (unit, level) in standard_steps(group_order, has_skeleton) {
        if level > levels[unit] {
            levels[unit] = level;
        }
        if total(levels) <= cap_bytes {
            return;
        }
    }
}

/// The least-important-first order of `n` units given a key where larger is
/// more important. Ties keep the later unit less important, so a list already
/// in priority order degrades from its tail.
pub fn order_by_importance<K: Ord>(n: usize, key: impl Fn(usize) -> K) -> Vec<usize> {
    let mut order: Vec<usize> = (0..n).collect();
    order.sort_by(|&a, &b| key(a).cmp(&key(b)).then(b.cmp(&a)));
    order
}

/// Interleave several sections' least-important-first orders so each loses
/// units in proportion to its size: a unit's position is its rank within its
/// own section, as a fraction of that section's length.
pub fn interleave(sections: &[Vec<usize>]) -> Vec<usize> {
    let mut all: Vec<(u64, usize, usize)> = Vec::new();
    for (s, order) in sections.iter().enumerate() {
        let len = order.len().max(1) as u64;
        for (i, &u) in order.iter().enumerate() {
            // Fraction of the way from least to most important, scaled to avoid floats.
            all.push((i as u64 * 1_000_000 / len, s, u));
        }
    }
    all.sort();
    all.into_iter().map(|(_, _, u)| u).collect()
}

/// Call-edge in-degree (number of `calls` edges targeting each id). Missing ids
/// count 0. The ranking signal until PageRank exists (P2 #5).
pub fn caller_counts(conn: &Connection, ids: &[i64]) -> Result<HashMap<i64, i64>> {
    let mut out: HashMap<i64, i64> = HashMap::new();
    for chunk in ids.chunks(500) {
        let placeholders = vec!["?"; chunk.len()].join(",");
        let sql = format!(
            "SELECT target_id, COUNT(*) FROM edges WHERE relation = '{}' AND target_id IN ({}) GROUP BY target_id",
            crate::domain::REL_CALLS,
            placeholders
        );
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map(rusqlite::params_from_iter(chunk.iter()), |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?))
        })?;
        for row in rows {
            let (id, n) = row?;
            out.insert(id, n);
        }
    }
    Ok(out)
}

/// A `code-graph-mcp …` command line, quoted for a POSIX shell.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NextCommand {
    words: Vec<String>,
}

impl NextCommand {
    pub fn new(subcommand: &str) -> Self {
        Self {
            words: vec!["code-graph-mcp".to_string(), subcommand.to_string()],
        }
    }
    pub fn arg(mut self, word: impl Into<String>) -> Self {
        self.words.push(word.into());
        self
    }
    pub fn flag_if(self, on: bool, flag: &str) -> Self {
        if on {
            self.arg(flag)
        } else {
            self
        }
    }
    pub fn opt(self, flag: &str, value: Option<impl Into<String>>) -> Self {
        match value {
            Some(v) => self.arg(flag).arg(v),
            None => self,
        }
    }
    /// A project-relative path argument. One starting with `-` would parse as
    /// a flag (`--json.js`, `-x/b.js`); `./` in front names the same file.
    pub fn path(self, p: impl Into<String>) -> Self {
        let p = p.into();
        if p.starts_with('-') {
            self.arg(format!("./{p}"))
        } else {
            self.arg(p)
        }
    }
    /// [`Self::opt`] for a path value (see [`Self::path`]).
    pub fn opt_path(self, flag: &str, value: Option<impl Into<String>>) -> Self {
        match value {
            Some(v) => self.arg(flag).path(v),
            None => self,
        }
    }
    /// The words, unquoted (what a shell passes to the program).
    pub fn words(&self) -> &[String] {
        &self.words
    }
}

impl std::fmt::Display for NextCommand {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let quoted: Vec<String> = self.words.iter().map(|w| shell_word(w)).collect();
        f.write_str(&quoted.join(" "))
    }
}

/// Quote one word for a POSIX shell; words made of safe characters stay bare.
pub fn shell_word(w: &str) -> String {
    let safe = !w.is_empty()
        && w.bytes().all(|b| {
            b.is_ascii_alphanumeric()
                || matches!(
                    b,
                    b'_' | b'-' | b'.' | b'/' | b':' | b'=' | b'@' | b'+' | b','
                )
        });
    if safe {
        w.to_string()
    } else {
        format!("'{}'", w.replace('\'', "'\\''"))
    }
}

/// `… budget N tokens: <parts>` — the one-line notice a budgeted text answer
/// prints where it left something out. `parts` are `(count, singular,
/// plural)`; zero counts are skipped.
pub fn notice(indent: &str, tokens: usize, parts: &[(usize, &str, &str)]) -> Option<String> {
    let said: Vec<String> = parts
        .iter()
        .filter(|(n, _, _)| *n > 0)
        .map(|(n, one, many)| format!("{n} {}", if *n == 1 { one } else { many }))
        .collect();
    if said.is_empty() {
        return None;
    }
    Some(format!(
        "{indent}… budget {tokens} tokens: {}",
        said.join(", ")
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sizes(levels: &[Level], full: &[usize], skel: &[usize]) -> usize {
        levels
            .iter()
            .enumerate()
            .map(|(i, l)| match l {
                Level::Full => full[i],
                Level::Skeleton => skel[i],
                Level::Dropped => 0,
            })
            .sum()
    }

    #[test]
    fn fits_without_steps_when_small() {
        let init = vec![Level::Full; 3];
        let steps = standard_steps(&[2, 1, 0], |_| true);
        let f = fit(&init, &steps, 100, |l| {
            ((), sizes(l, &[10, 10, 10], &[5, 5, 5]))
        });
        assert!(!f.degraded());
        assert_eq!(f.bytes, 30);
    }

    #[test]
    fn skeletons_every_unit_before_dropping_any() {
        let init = vec![Level::Full; 3];
        // unit 0 most important, 2 least.
        let steps = standard_steps(&[2, 1, 0], |_| true);
        // Full 30 each, skeleton 10 each. Budget 50: skeleton 2 and 1 -> 30+10+10=50.
        let f = fit(&init, &steps, 50, |l| {
            ((), sizes(l, &[30, 30, 30], &[10, 10, 10]))
        });
        assert_eq!(
            f.levels,
            vec![Level::Full, Level::Skeleton, Level::Skeleton]
        );
        // Budget 25: all skeleton = 30 > 25 -> drop 2 -> 20.
        let f = fit(&init, &steps, 25, |l| {
            ((), sizes(l, &[30, 30, 30], &[10, 10, 10]))
        });
        assert_eq!(
            f.levels,
            vec![Level::Skeleton, Level::Skeleton, Level::Dropped]
        );
        assert!(!f.over_budget);
    }

    #[test]
    fn fill_restores_smaller_units_the_prefix_search_left_out() {
        let init = vec![Level::Full; 3];
        // Most important first: 0 (big), 1, 2. No skeletons.
        let steps = standard_steps(&[2, 1, 0], |_| false);
        // Full sizes 100, 10, 10; budget 50: prefix drops 2, 1, then 0 (100 > 50);
        // fill brings back 1 and 2.
        let f = fit(&init, &steps, 50, |l| {
            ((), sizes(l, &[100, 10, 10], &[0, 0, 0]))
        });
        assert_eq!(f.levels, vec![Level::Dropped, Level::Full, Level::Full]);
        assert_eq!(f.bytes, 20);
    }

    #[test]
    fn reports_over_budget_when_the_fixed_part_does_not_fit() {
        let init = vec![Level::Full; 2];
        let steps = standard_steps(&[1, 0], |_| false);
        let f = fit(&init, &steps, 5, |l| ((), 10 + sizes(l, &[3, 3], &[3, 3])));
        assert!(f.over_budget);
        assert_eq!(f.levels, vec![Level::Dropped, Level::Dropped]);
    }

    #[test]
    fn interleave_takes_from_each_section_in_proportion() {
        // Section A: 4 units (least-first 3,2,1,0); section B: 2 units (5,4).
        let order = interleave(&[vec![3, 2, 1, 0], vec![5, 4]]);
        assert_eq!(order.len(), 6);
        assert_eq!(
            &order[..2],
            &[3, 5],
            "both sections lose their least unit first: {order:?}"
        );
    }

    #[test]
    fn order_by_importance_breaks_ties_from_the_tail() {
        let key = [5, 1, 1, 9];
        assert_eq!(order_by_importance(4, |i| key[i]), vec![2, 1, 0, 3]);
    }

    #[test]
    fn cap_group_lowers_one_group_to_its_share() {
        let mut levels = vec![Level::Full; 4];
        // group = units 0..3 (least first 2,1,0); costs full 40, dropped 0.
        cap_group(
            &mut levels,
            &[2, 1, 0],
            |_| false,
            |_, l| if l == Level::Full { 40 } else { 0 },
            85,
        );
        assert_eq!(
            levels,
            vec![Level::Full, Level::Full, Level::Dropped, Level::Full]
        );
    }

    #[test]
    fn next_command_quotes_only_what_needs_it() {
        let c = NextCommand::new("callgraph")
            .arg("Foo.bar")
            .opt("--file", Some("src/a b.rs"))
            .flag_if(true, "--include-tests")
            .arg("it's");
        assert_eq!(
            c.to_string(),
            "code-graph-mcp callgraph Foo.bar --file 'src/a b.rs' --include-tests 'it'\\''s'"
        );
    }

    /// A path that starts with `-` keeps naming a file: `./` in front, which
    /// every command that takes a path accepts (review F-L1: `show foo3
    /// --file --json.js` exited 2).
    #[test]
    fn a_path_starting_with_a_dash_is_not_a_flag() {
        let c = NextCommand::new("show")
            .arg("foo3")
            .opt_path("--file", Some("--json.js"))
            .path("-x/b.js")
            .path("src/a.rs")
            .opt_path("--file", None::<String>);
        assert_eq!(
            c.to_string(),
            "code-graph-mcp show foo3 --file ./--json.js ./-x/b.js src/a.rs"
        );
    }

    /// Every byte a POSIX shell treats specially is quoted, so a path or name
    /// carrying one cannot run anything when the command is pasted: `$(…)`,
    /// backticks, `;`, `|`, `&`, redirections, globs, `!`, `~`, `#`, quotes,
    /// backslash, whitespace, braces and brackets.
    #[test]
    fn shell_word_quotes_every_metacharacter() {
        for c in "$()`;|&<>*?!~#\"'\\ \t\n{}[]".chars() {
            let w = format!("a{c}b");
            let q = shell_word(&w);
            assert!(
                q.starts_with('\'') && q.ends_with('\''),
                "{c:?} left unquoted: {q}"
            );
        }
        for w in ["src/a-b_c.rs", "Foo::bar", "x=1", "a@b+c,d"] {
            assert_eq!(shell_word(w), w, "{w} needs no quotes");
        }
        assert_eq!(shell_word(""), "''");
    }

    #[test]
    fn notice_skips_zero_parts() {
        assert_eq!(notice("  ", 500, &[(0, "x", "xs"), (0, "y", "ys")]), None);
        assert_eq!(
            notice(
                "  ",
                500,
                &[
                    (3, "file omitted", "files omitted"),
                    (0, "y", "ys"),
                    (1, "file cut", "files cut")
                ]
            )
            .unwrap(),
            "  … budget 500 tokens: 3 files omitted, 1 file cut"
        );
    }
}
