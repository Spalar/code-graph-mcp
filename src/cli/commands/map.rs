use super::*;

/// CLI arguments for the `map` subcommand (audit #4 clap migration).
#[derive(Parser, Debug)]
#[command(
    name = "code-graph-mcp map",
    about = "Project architecture map (modules, deps, entry points)"
)]
pub struct MapArgs {
    /// JSON output
    #[arg(long)]
    pub json: bool,
    /// Compact output (top modules/deps/hot functions only)
    #[arg(long)]
    pub compact: bool,
    /// Token budget for the text answer (bytes/3, 100-100000): modules with the
    /// fewest incoming imports lose their key symbols first, then the
    /// lowest-ranked rows of every section are left out; ends with a command
    /// that prints them
    #[arg(long, conflicts_with_all = ["json", "compact"])]
    pub budget: Option<u64>,
}

/// Project map — aider repo-map style.
///
/// Output format:
/// ```text
/// src/mcp/server.rs (158KB, 98 symbols)
///   McpServer: handle_tool_call, process_message, flush_metrics
/// ```
pub fn cmd_map(project_root: &Path, args: MapArgs) -> Result<()> {
    let json_mode = args.json;
    let compact = args.compact;

    let ctx = CliContext::open(project_root)?;
    let conn = ctx.db.conn();

    let mut map = queries::get_project_map(conn)?;
    // Query-time freshness over the files this answer names — the same shared
    // resync every other read command runs. `map` was never swept when freshness
    // was wired command by command (audit 2026-08-29 CON-03): the architecture-
    // level commands were added before it and never revisited, so an edited file
    // kept its pre-edit hot-function and entry-point rows.
    {
        let mut files: Vec<String> = map.3.iter().map(|h| h.file.clone()).collect();
        files.extend(map.2.iter().map(|e| e.file.clone()));
        let outcome = refresh_files_if_stale(&ctx.db, &ctx.project_root, &files);
        if outcome.any_changed {
            map = queries::get_project_map(conn)?;
        }
        outcome.disclose();
    }
    let (modules, deps, entry_points, hot_functions) = map;

    let mut stdout = std::io::stdout().lock();

    if let Some(requested) = args.budget {
        let tokens = clamp_arg(
            "--budget",
            requested,
            crate::budget::MIN_BUDGET_TOKENS,
            crate::budget::MAX_BUDGET_TOKENS,
        ) as usize;
        let text = map_budget_text(&entry_points, &modules, &deps, &hot_functions, tokens);
        write!(stdout, "{}", text)?;
        return Ok(());
    }

    if json_mode {
        // Field names (`caller_count` / `test_caller_count`) and `--compact`
        // cap (top-10) match MCP `project_map`. CLI default returns top-15
        // (the DB LIMIT in get_project_map).
        let hot_cap = if compact { 10 } else { hot_functions.len() };
        let hot_json: Vec<serde_json::Value> = hot_functions
            .iter()
            .take(hot_cap)
            .map(|h| {
                let mut obj = serde_json::json!({
                    "name": h.name,
                    "type": h.node_type,
                    "file": h.file,
                    "caller_count": h.caller_count,
                });
                if h.test_caller_count > 0 {
                    obj["test_caller_count"] = serde_json::json!(h.test_caller_count);
                }
                obj
            })
            .collect();

        let mut result = serde_json::json!({
            "modules": modules.iter().map(|m| serde_json::json!({
                "path": m.path,
                "files": m.files,
                "functions": m.functions,
                "classes": m.classes,
                "interfaces_traits": m.interfaces_traits,
                "constants": m.constants,
                "other": m.other,
                "languages": m.languages,
                "key_symbols": m.key_symbols,
            })).collect::<Vec<_>>(),
            "module_dependencies": deps.iter().map(|d| serde_json::json!({
                "from": d.from,
                "to": d.to,
                "imports": d.import_count,
            })).collect::<Vec<_>>(),
            "entry_points": entry_points.iter().map(|e| serde_json::json!({
                "route": e.route,
                "handler": e.handler,
                "file": e.file,
                "kind": e.kind,
            })).collect::<Vec<_>>(),
            "hot_functions": hot_json,
        });
        // Text mode already prints "... and N more hot functions"; JSON mode cut
        // the same rows with no marker, so a `--compact --json` consumer read the
        // short list as the whole list. Same disclosure, same key names as MCP
        // `project_map` compact.
        if hot_functions.len() > hot_cap {
            result["hot_functions_truncated"] = serde_json::json!(true);
            result["hot_functions_total"] = serde_json::json!(hot_functions.len());
            result["next"] = serde_json::json!(MAP_ALL_JSON);
        }
        writeln!(stdout, "{}", serde_json::to_string(&result)?)?;
        return Ok(());
    }

    // Entry points
    if !entry_points.is_empty() {
        writeln!(stdout, "Entry Points:")?;
        for ep in &entry_points {
            writeln!(stdout, "{}", entry_line(ep))?;
        }
        writeln!(stdout)?;
    }

    // Modules
    if modules.is_empty() {
        if entry_points.is_empty() {
            writeln!(stdout, "(empty project — no indexed source files)")?;
        }
        return Ok(());
    }
    writeln!(stdout, "Modules:")?;
    let max_modules = if compact { 15 } else { modules.len() };
    for m in modules.iter().take(max_modules) {
        writeln!(stdout, "{}", module_header(m))?;
        if !m.key_symbols.is_empty() {
            writeln!(stdout, "{}", module_symbols_line(m))?;
        }
    }
    if compact && modules.len() > max_modules {
        writeln!(
            stdout,
            "  ... and {} more modules",
            modules.len() - max_modules
        )?;
        writeln!(stdout, "  next: {MAP_ALL_TEXT}")?;
    }

    // Dependencies (compact: top 10)
    if !deps.is_empty() {
        writeln!(stdout)?;
        writeln!(stdout, "Dependencies:")?;
        let max_deps = if compact { 10 } else { deps.len().min(30) };
        for d in deps.iter().take(max_deps) {
            writeln!(stdout, "{}", dep_line(d))?;
        }
        // Truncation marker (roadmap 2026-07-18 §1.7): the silent .min(30) cap
        // read as "that's every dependency" — same pattern as the modules cap.
        // The text answer caps dependencies at 30 in every mode, so the command
        // that returns the rest is the JSON one (no cap).
        if deps.len() > max_deps {
            writeln!(
                stdout,
                "  ... and {} more dependencies",
                deps.len() - max_deps
            )?;
            writeln!(stdout, "  next: {MAP_ALL_JSON}")?;
        }
    }

    // Hot functions (compact: top 5)
    if !hot_functions.is_empty() {
        writeln!(stdout)?;
        writeln!(stdout, "Hot Functions:")?;
        let max_hot = if compact { 5 } else { hot_functions.len() };
        for h in hot_functions.iter().take(max_hot) {
            writeln!(stdout, "{}", hot_line(h))?;
        }
        if hot_functions.len() > max_hot {
            writeln!(
                stdout,
                "  ... and {} more hot functions",
                hot_functions.len() - max_hot
            )?;
            writeln!(stdout, "  next: {MAP_ALL_TEXT}")?;
        }
    }

    Ok(())
}

/// The command that returns every module and hot function (the text answer
/// without `--compact`/`--budget`).
const MAP_ALL_TEXT: &str = "code-graph-mcp map";
/// The command that returns everything `map` knows, dependencies included —
/// the text answer caps those at 30.
pub(crate) const MAP_ALL_JSON: &str = "code-graph-mcp map --json";
/// Dependencies the text answer prints before its "... and N more" line.
const MAP_TEXT_DEPS_CAP: usize = 30;

fn entry_line(ep: &queries::EntryPoint) -> String {
    format!("  {} → {} ({})", ep.route, ep.handler, ep.file)
}

fn module_header(m: &queries::ModuleStats) -> String {
    // Include constants: key_symbols can list exported consts (e.g. a TS
    // `export const db`), so leaving them out of the total made the header
    // claim fewer symbols than the names printed right under it. `other`
    // closes the same hole for every remaining type — a markdown-only module
    // (headings) or a types-only module (TS `type` aliases) reported
    // "0 symbols" here while `overview <path>` listed them.
    let total_symbols = m.functions + m.classes + m.interfaces_traits + m.constants + m.other;
    let mut s = format!(
        "{} ({}, {}",
        m.path,
        plural(m.files as i64, "file"),
        plural(total_symbols as i64, "symbol")
    );
    if !m.languages.is_empty() {
        s.push_str(", ");
        s.push_str(&m.languages.join("/"));
    }
    s.push(')');
    s
}

fn module_symbols_line(m: &queries::ModuleStats) -> String {
    format!("  {}", m.key_symbols.join(", "))
}

fn dep_line(d: &queries::ModuleDep) -> String {
    format!("  {} → {} ({} imports)", d.from, d.to, d.import_count)
}

fn hot_line(h: &queries::HotFunction) -> String {
    if h.test_caller_count > 0 {
        format!(
            "  {} ({}) — {} + {} test ({})",
            h.name,
            h.node_type,
            plural(h.caller_count as i64, "caller"),
            h.test_caller_count,
            h.file
        )
    } else {
        format!(
            "  {} ({}) — {} ({})",
            h.name,
            h.node_type,
            plural(h.caller_count as i64, "caller"),
            h.file
        )
    }
}

/// `map --budget`: the text answer fitted to `tokens`.
///
/// Units, least important first within each section: modules by incoming
/// imports (then symbol count), dependencies by import count, hot functions by
/// caller count, entry points by listing order. Sections lose units in
/// proportion to their length ([`crate::budget::interleave`]). A module's
/// shorter form is its header line without the key-symbol line; every other
/// row is one line and is either printed or left out. Dependencies are not
/// capped at 30 here — the budget is the cap.
pub(crate) fn map_budget_text(
    entry_points: &[queries::EntryPoint],
    modules: &[queries::ModuleStats],
    deps: &[queries::ModuleDep],
    hot: &[queries::HotFunction],
    tokens: usize,
) -> String {
    use crate::budget::{self, Level};
    use std::cmp::Reverse;
    let (ne, nm, nd, nh) = (entry_points.len(), modules.len(), deps.len(), hot.len());
    let (om, od, oh) = (ne, ne + nm, ne + nm + nd);
    let n = oh + nh;
    let mut in_imports: std::collections::HashMap<&str, usize> = std::collections::HashMap::new();
    for d in deps {
        *in_imports.entry(d.to.as_str()).or_default() += d.import_count;
    }
    let symbols = |m: &queries::ModuleStats| {
        m.functions + m.classes + m.interfaces_traits + m.constants + m.other
    };
    let shift = |v: Vec<usize>, by: usize| v.into_iter().map(|u| u + by).collect::<Vec<_>>();
    let order = budget::interleave(&[
        budget::order_by_importance(ne, Reverse),
        shift(
            budget::order_by_importance(nm, |i| {
                (
                    in_imports
                        .get(modules[i].path.as_str())
                        .copied()
                        .unwrap_or(0),
                    symbols(&modules[i]),
                    Reverse(i),
                )
            }),
            om,
        ),
        shift(
            budget::order_by_importance(nd, |i| (deps[i].import_count, Reverse(i))),
            od,
        ),
        shift(
            budget::order_by_importance(nh, |i| (hot[i].caller_count, Reverse(i))),
            oh,
        ),
    ]);
    let steps = budget::standard_steps(&order, |u| {
        (om..od).contains(&u) && !modules[u - om].key_symbols.is_empty()
    });

    let render = |levels: &[Level]| -> String {
        let mut out = String::new();
        let mut line = |s: &str| {
            out.push_str(s);
            out.push('\n');
        };
        let dropped =
            |r: std::ops::Range<usize>| r.filter(|&u| levels[u] == Level::Dropped).count();
        if ne > 0 {
            line("Entry Points:");
            for (i, ep) in entry_points.iter().enumerate() {
                if levels[i] != Level::Dropped {
                    line(&entry_line(ep));
                }
            }
            if let Some(n) = budget::notice(
                "  ",
                tokens,
                &[(
                    dropped(0..ne),
                    "entry point omitted",
                    "entry points omitted",
                )],
            ) {
                line(&n);
                line(&format!("  next: {MAP_ALL_TEXT}"));
            }
            line("");
        }
        if nm == 0 {
            if ne == 0 {
                line("(empty project — no indexed source files)");
            }
            return out;
        }
        line("Modules:");
        let mut skel = 0usize;
        for (i, m) in modules.iter().enumerate() {
            match levels[om + i] {
                Level::Dropped => {}
                Level::Skeleton => {
                    skel += 1;
                    line(&module_header(m));
                }
                Level::Full => {
                    line(&module_header(m));
                    if !m.key_symbols.is_empty() {
                        line(&module_symbols_line(m));
                    }
                }
            }
        }
        if let Some(n) = budget::notice(
            "  ",
            tokens,
            &[
                (dropped(om..od), "module omitted", "modules omitted"),
                (skel, "without key symbols", "without key symbols"),
            ],
        ) {
            line(&n);
            line(&format!("  next: {MAP_ALL_TEXT}"));
        }
        if nd > 0 {
            line("");
            line("Dependencies:");
            for (i, d) in deps.iter().enumerate() {
                if levels[od + i] != Level::Dropped {
                    line(&dep_line(d));
                }
            }
            if let Some(n) = budget::notice(
                "  ",
                tokens,
                &[(
                    dropped(od..oh),
                    "dependency omitted",
                    "dependencies omitted",
                )],
            ) {
                line(&n);
                // The unbudgeted text stops at 30 dependencies; name the JSON
                // answer when a dropped one sits past that cap.
                let past_cap = (MAP_TEXT_DEPS_CAP..nd).any(|i| levels[od + i] == Level::Dropped);
                let cmd = if past_cap { MAP_ALL_JSON } else { MAP_ALL_TEXT };
                line(&format!("  next: {cmd}"));
            }
        }
        if nh > 0 {
            line("");
            line("Hot Functions:");
            for (i, h) in hot.iter().enumerate() {
                if levels[oh + i] != Level::Dropped {
                    line(&hot_line(h));
                }
            }
            if let Some(n) = budget::notice(
                "  ",
                tokens,
                &[(
                    dropped(oh..n),
                    "hot function omitted",
                    "hot functions omitted",
                )],
            ) {
                line(&n);
                line(&format!("  next: {MAP_ALL_TEXT}"));
            }
        }
        out
    };
    budget::fit(
        &vec![Level::Full; n],
        &steps,
        budget::budget_bytes(tokens),
        |levels| {
            let s = render(levels);
            let len = s.len();
            (s, len)
        },
    )
    .output
}

// --- tour subcommand ---
