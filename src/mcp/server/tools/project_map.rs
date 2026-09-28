//! `project_map` — modules / dependencies / entry points / hot functions.
//! 60s TTL cache; compact mode strips `languages`, keeps key_symbols for discoverability.

use super::super::*;

/// How many hot functions compact mode shows. The full envelope carries up to 15
/// (the query's LIMIT); anything past this cap is dropped and DISCLOSED via
/// `hot_functions_truncated` — see the trim site in `tool_project_map`.
const COMPACT_HOT_FUNCTIONS: usize = 10;

/// One module entry, trimmed for compact mode.
///
/// Split out of `tool_project_map` so the field set is unit-testable and so the
/// `project_map_compact_forwards_every_module_key` drift guard has a region to
/// scan — the same sibling-path drift this function exists to fix will otherwise
/// recur the next time the full builder grows a key.
///
/// The symbol-count buckets are forwarded when non-zero rather than dropped. The
/// full builder grew its `other` bucket precisely because a docs- or types-only
/// module read as `functions: 0, classes: 0`, i.e. empty, while `module_overview`
/// listed its symbols — and compact was still answering exactly that, for
/// `other` and for every module that is entirely classes or constants. Being
/// conditional, the cost lands only on modules that actually have them.
fn compact_project_module(m: &serde_json::Value) -> serde_json::Value {
    let mut obj = json!({
        "path": m["path"],
        "files": m["files"],
        "functions": m["functions"],
    });
    for key in ["classes", "interfaces_traits", "constants", "other"] {
        if m.get(key).and_then(|v| v.as_u64()).is_some_and(|n| n > 0) {
            obj[key] = m[key].clone();
        }
    }
    // Preserve key_symbols — essential for deciding what to explore next
    if let Some(ks) = m.get("key_symbols") {
        if ks.as_array().is_some_and(|a| !a.is_empty()) {
            obj["key_symbols"] = ks.clone();
        }
    }
    obj
}

impl McpServer {
    pub(in crate::mcp::server) fn tool_project_map(
        &self,
        args: &serde_json::Value,
    ) -> Result<serde_json::Value> {
        // Bound at entry, not inside `if include_centrality` below — same rule as
        // `module_overview`'s `deps_direction` / `deps_depth` / `dead_min_lines`,
        // and pre-tag review caught this one sitting on the wrong side of it: a
        // `centrality_limit` sent without the flag was still swallowed, which is
        // precisely what the sibling comment says must not happen.
        let centrality_limit = arg_clamped(args, "centrality_limit", "project_map", 10)? as usize;
        // P1 #2: absent = the unbudgeted answer. Read at entry for the same reason.
        let max_tokens = match &args["max_tokens"] {
            serde_json::Value::Null => None,
            _ => Some(arg_clamped(args, "max_tokens", "project_map", 0)? as usize),
        };

        if !should_skip_indexing(args)? {
            self.ensure_indexed()?;
        }
        let compact = arg_bool(args, "compact", false)?;

        // Return cached result if fresh (< 60s) — project_map is expensive and rarely changes mid-session
        // Note: cache stores full result; compact is derived from it on the fly
        let full_result = {
            let cache = lock_or_recover(&self.cache.cached_project_map, "cached_pmap");
            if let Some((ts, ref val)) = *cache {
                if ts.elapsed().as_secs() < 60 {
                    Some(val.clone())
                } else {
                    None
                }
            } else {
                None
            }
        };

        let mut result = if let Some(cached) = full_result {
            cached
        } else {
            let (modules, deps, entry_points, hot_functions) =
                queries::get_project_map(self.db.conn())?;

            let modules_json: Vec<serde_json::Value> = modules
                .iter()
                .map(|m| {
                    let mut obj = json!({
                        "path": m.path,
                        "files": m.files,
                        "functions": m.functions,
                        "classes": m.classes,
                    });
                    if m.interfaces_traits > 0 {
                        obj["interfaces_traits"] = json!(m.interfaces_traits);
                    }
                    if m.constants > 0 {
                        obj["constants"] = json!(m.constants);
                    }
                    // Symbols outside the four named buckets (TS `type` aliases,
                    // markdown headings). Without this a docs- or types-only
                    // module read as `functions: 0, classes: 0` — i.e. empty —
                    // while module_overview listed its symbols.
                    if m.other > 0 {
                        obj["other"] = json!(m.other);
                    }
                    if !m.languages.is_empty() {
                        obj["languages"] = json!(m.languages);
                    }
                    if !m.key_symbols.is_empty() {
                        obj["key_symbols"] = json!(m.key_symbols);
                    }
                    obj
                })
                .collect();

            let deps_json: Vec<serde_json::Value> = deps
                .iter()
                .map(|d| {
                    json!({
                        "from": d.from,
                        "to": d.to,
                        "imports": d.import_count,
                    })
                })
                .collect();

            let routes_json: Vec<serde_json::Value> = entry_points
                .iter()
                .map(|e| {
                    json!({
                        "route": e.route,
                        "handler": e.handler,
                        "file": e.file,
                        "kind": e.kind,
                    })
                })
                .collect();

            let hot_json: Vec<serde_json::Value> = hot_functions
                .iter()
                .map(|h| {
                    let mut obj = json!({
                        "name": h.name,
                        "type": h.node_type,
                        "file": h.file,
                        "caller_count": h.caller_count,
                    });
                    if h.test_caller_count > 0 {
                        obj["test_caller_count"] = json!(h.test_caller_count);
                    }
                    obj
                })
                .collect();

            let r = json!({
                "modules": modules_json,
                "module_dependencies": deps_json,
                "entry_points": routes_json,
                "hot_functions": hot_json,
            });

            // Cache the full result
            *lock_or_recover(&self.cache.cached_project_map, "cached_pmap") =
                Some((std::time::Instant::now(), r.clone()));

            r
        };

        // include_centrality (roadmap 2026-07-18 §2.4 — CLI `centrality` had no MCP
        // surface): architectural chokepoints by betweenness centrality. Computed
        // per call and attached OUTSIDE the 60s cache (the flag/limit vary per
        // call; the cached envelope stays flag-free). Test callers excluded, same
        // default as the CLI.
        if arg_bool(args, "include_centrality", false)? {
            // Clamped, not just floored (audit 2026-08-29 CON-08): this was the
            // only numeric MCP parameter without an upper bound, while every
            // sibling has one (top_k/limit 1-100, depth 1-20, context_lines
            // 0-100). Betweenness is computed per call, so an unbounded limit is
            // an unbounded response and an unbounded render.
            let limit = centrality_limit;
            let ranked =
                crate::graph::centrality::betweenness_centrality(self.db.conn(), false, limit)?;
            let centrality_json: Vec<serde_json::Value> = ranked
                .iter()
                .map(|c| {
                    json!({
                        "name": c.name,
                        "type": c.node_type,
                        "file_path": c.file_path,
                        "betweenness": c.score,
                        "normalized": c.normalized,
                        "caller_count": c.caller_count,
                    })
                })
                .collect();
            result["centrality"] = json!(centrality_json);
        }

        if let Some(tokens) = max_tokens {
            return Ok(project_map_budgeted(&result, tokens));
        }

        if compact {
            let compact_modules: Vec<serde_json::Value> = result["modules"]
                .as_array()
                .map(|arr| arr.iter().map(compact_project_module).collect())
                .unwrap_or_default();

            let compact_deps: Vec<serde_json::Value> = result["module_dependencies"]
                .as_array()
                .map(|arr| {
                    arr.iter()
                        .map(|d| {
                            json!({
                                "from": d["from"],
                                "to": d["to"],
                                // One integer, and the only thing that ranks the
                                // list: `59 imports` and `1 import` are the same
                                // edge without it.
                                "imports": d["imports"],
                            })
                        })
                        .collect()
                })
                .unwrap_or_default();

            // Trim hot_functions: top 10, name+type+file+counts.
            // `type` retained so callers can distinguish function vs method
            // without a follow-up get_ast_node call (parity with non-compact
            // envelope and CLI `map --json`).
            //
            // The full list is capped at 15 by the query, so this cuts up to 5.
            // The cut itself is fine; an UNDISCLOSED cut is not — a short list
            // with no marker reads as "these are all the hot functions there
            // are", which is a wrong answer rather than a terse one. `truncated`
            // below is emitted only when something was actually dropped.
            let hot_total = result["hot_functions"]
                .as_array()
                .map(|a| a.len())
                .unwrap_or(0);
            let compact_hot: Vec<serde_json::Value> = result["hot_functions"]
                .as_array()
                .map(|arr| {
                    arr.iter()
                        .take(COMPACT_HOT_FUNCTIONS)
                        .map(|h| {
                            let mut obj = json!({
                                "name": h["name"],
                                "type": h["type"],
                                "file": h["file"],
                                "caller_count": h["caller_count"],
                            });
                            if h.get("test_caller_count")
                                .and_then(|v| v.as_i64())
                                .unwrap_or(0)
                                > 0
                            {
                                obj["test_caller_count"] = h["test_caller_count"].clone();
                            }
                            obj
                        })
                        .collect()
                })
                .unwrap_or_default();

            // entry_points: route+handler+file+kind — the full entry, nothing
            // trimmed. `kind` lets a caller skip `main` when scanning the HTTP
            // surface; `route` is the URL, i.e. the answer to the question the
            // HTTP surface is being scanned FOR. Compact used to drop it and
            // reply with handler names and no endpoints.
            let compact_entries: Vec<serde_json::Value> = result["entry_points"]
                .as_array()
                .map(|arr| {
                    arr.iter()
                        .map(|e| {
                            json!({
                                "route": e["route"],
                                "file": e["file"],
                                "handler": e["handler"],
                                "kind": e["kind"],
                            })
                        })
                        .collect()
                })
                .unwrap_or_default();

            let mut compact_result = json!({
                "modules": compact_modules,
                "module_dependencies": compact_deps,
                "entry_points": compact_entries,
                "hot_functions": compact_hot,
            });
            if hot_total > COMPACT_HOT_FUNCTIONS {
                compact_result["hot_functions_truncated"] = json!(true);
                compact_result["hot_functions_total"] = json!(hot_total);
                // The non-compact text map lists every hot function.
                compact_result["next"] = json!("code-graph-mcp map");
            }
            // Compact is a WHITELIST rebuild (feedback_compact_field_allowlist —
            // v0.90/v0.97.1 both dropped new fields here): forward centrality
            // explicitly, trimmed to name+file+score.
            if let Some(cent) = result.get("centrality").and_then(|c| c.as_array()) {
                let compact_cent: Vec<serde_json::Value> = cent
                    .iter()
                    .map(|c| {
                        json!({
                            "name": c["name"],
                            "file_path": c["file_path"],
                            "betweenness": c["betweenness"],
                        })
                    })
                    .collect();
                compact_result["centrality"] = json!(compact_cent);
            }
            return Ok(compact_result);
        }

        Ok(result)
    }
}

/// `project_map` with `max_tokens`: the full envelope fitted to the budget.
///
/// Same units and ranking as CLI `map --budget`: modules by incoming imports
/// (shorter form: path + counts, no `key_symbols` / `languages`), dependencies
/// by import count, hot functions by caller count, entry points by order;
/// sections lose units in proportion to their length. `centrality` (bounded by
/// `centrality_limit`) is kept whole. When anything was shortened or left out,
/// `budget` says how much and names the command that returns it.
fn project_map_budgeted(full: &serde_json::Value, tokens: usize) -> serde_json::Value {
    use crate::budget::{self, Level};
    use std::cmp::Reverse;
    let arr = |k: &str| full[k].as_array().cloned().unwrap_or_default();
    let (eps, mods, deps, hot) = (
        arr("entry_points"),
        arr("modules"),
        arr("module_dependencies"),
        arr("hot_functions"),
    );
    let (ne, nm, nd, nh) = (eps.len(), mods.len(), deps.len(), hot.len());
    let (om, od, oh) = (ne, ne + nm, ne + nm + nd);
    let n = oh + nh;
    let u = |v: &serde_json::Value, k: &str| v[k].as_u64().unwrap_or(0);
    let mut in_imports: std::collections::HashMap<&str, u64> = std::collections::HashMap::new();
    for d in &deps {
        *in_imports
            .entry(d["to"].as_str().unwrap_or(""))
            .or_default() += u(d, "imports");
    }
    let symbols = |m: &serde_json::Value| {
        [
            "functions",
            "classes",
            "interfaces_traits",
            "constants",
            "other",
        ]
        .iter()
        .map(|k| u(m, k))
        .sum::<u64>()
    };
    let shift = |v: Vec<usize>, by: usize| v.into_iter().map(|x| x + by).collect::<Vec<_>>();
    let order = budget::interleave(&[
        budget::order_by_importance(ne, Reverse),
        shift(
            budget::order_by_importance(nm, |i| {
                let path = mods[i]["path"].as_str().unwrap_or("");
                (
                    in_imports.get(path).copied().unwrap_or(0),
                    symbols(&mods[i]),
                    Reverse(i),
                )
            }),
            om,
        ),
        shift(
            budget::order_by_importance(nd, |i| (u(&deps[i], "imports"), Reverse(i))),
            od,
        ),
        shift(
            budget::order_by_importance(nh, |i| (u(&hot[i], "caller_count"), Reverse(i))),
            oh,
        ),
    ]);
    let steps = budget::standard_steps(&order, |x| (om..od).contains(&x));
    let skeleton_module = |m: &serde_json::Value| {
        let mut s = compact_project_module(m);
        if let Some(o) = s.as_object_mut() {
            o.remove("key_symbols");
        }
        s
    };
    let render = |levels: &[Level]| -> serde_json::Value {
        let keep = |items: &[serde_json::Value], off: usize| -> Vec<serde_json::Value> {
            items
                .iter()
                .enumerate()
                .filter(|(i, _)| levels[off + i] != Level::Dropped)
                .map(|(i, v)| {
                    if levels[off + i] == Level::Skeleton {
                        skeleton_module(v)
                    } else {
                        v.clone()
                    }
                })
                .collect()
        };
        let mut out = full.clone();
        out["entry_points"] = json!(keep(&eps, 0));
        out["modules"] = json!(keep(&mods, om));
        out["module_dependencies"] = json!(keep(&deps, od));
        out["hot_functions"] = json!(keep(&hot, oh));
        let dropped =
            |r: std::ops::Range<usize>| r.filter(|&x| levels[x] == Level::Dropped).count();
        let mut omitted = serde_json::Map::new();
        for (k, c) in [
            ("entry_points", dropped(0..ne)),
            ("modules", dropped(om..od)),
            ("module_dependencies", dropped(od..oh)),
            ("hot_functions", dropped(oh..n)),
        ] {
            if c > 0 {
                omitted.insert(k.to_string(), json!(c));
            }
        }
        let skel = (om..od).filter(|&x| levels[x] == Level::Skeleton).count();
        if !omitted.is_empty() || skel > 0 {
            let mut b = json!({ "max_tokens": tokens });
            if !omitted.is_empty() {
                b["omitted"] = serde_json::Value::Object(omitted);
            }
            if skel > 0 {
                b["modules_without_key_symbols"] = json!(skel);
            }
            // The text map caps dependencies at 30; everything else it lists.
            let past_cap = (30..nd).any(|i| levels[od + i] == Level::Dropped);
            b["next"] = json!(if past_cap {
                "code-graph-mcp map --json"
            } else {
                "code-graph-mcp map"
            });
            out["budget"] = b;
        }
        out
    };
    budget::fit(
        &vec![Level::Full; n],
        &steps,
        budget::budget_bytes(tokens),
        |levels| {
            let v = render(levels);
            let len = serde_json::to_string(&v).map(|s| s.len()).unwrap_or(0);
            (v, len)
        },
    )
    .output
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A module whose symbols are all outside the `functions`/`classes` buckets
    /// (TS type aliases, markdown headings) must not compact down to
    /// `functions: 0` with nothing else — that reads as an empty module and
    /// steers the caller away from a directory that is in fact full of symbols.
    #[test]
    fn compact_module_keeps_nonzero_symbol_buckets() {
        let full = json!({
            "path": "docs",
            "files": 12,
            "functions": 0,
            "classes": 0,
            "other": 143,
            "languages": ["markdown"],
        });
        let compact = compact_project_module(&full);
        assert_eq!(
            compact["other"],
            json!(143),
            "a docs-only module must keep its `other` count in compact mode, got {compact}"
        );
        assert_eq!(compact["path"], json!("docs"));
        assert_eq!(compact["files"], json!(12));
    }

    #[test]
    fn compact_module_keeps_class_and_constant_counts() {
        let full = json!({
            "path": "src/models",
            "files": 8,
            "functions": 0,
            "classes": 31,
            "constants": 4,
            "interfaces_traits": 2,
        });
        let compact = compact_project_module(&full);
        for (key, want) in [("classes", 31), ("constants", 4), ("interfaces_traits", 2)] {
            assert_eq!(
                compact[key],
                json!(want),
                "compact must keep `{key}`, got {compact}"
            );
        }
    }

    /// The buckets are forwarded only when non-zero, so a plain code module pays
    /// nothing for the fix — otherwise compact mode stops being compact.
    #[test]
    fn compact_module_omits_zero_and_dropped_fields() {
        let full = json!({
            "path": "src/util",
            "files": 3,
            "functions": 9,
            "classes": 0,
            "languages": ["rust"],
        });
        let compact = compact_project_module(&full);
        assert!(
            compact.get("classes").is_none(),
            "a zero bucket must stay out of the compact payload, got {compact}"
        );
        assert!(
            compact.get("languages").is_none(),
            "`languages` is the one field compact deliberately drops, got {compact}"
        );
        assert_eq!(compact["functions"], json!(9));
    }
}
