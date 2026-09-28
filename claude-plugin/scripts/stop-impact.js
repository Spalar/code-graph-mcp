#!/usr/bin/env node
'use strict';
// FIRST statement, before the other requires — same reason as pre-edit-guide.js.
if (require.main === module) require('./hook-fail-open').installHookFailOpen('Stop');

// Stop hook (P1 #3b): the end-of-turn half of the edit-impact check.
//
// pre-edit-guide.js pushes "this function has N callers" BEFORE an edit, when
// nobody yet knows whether the edit will change the signature. This runs when
// the turn ends and asks the question that can be answered then: of the
// symbols edited this turn, which ones' signatures differ from HEAD, and
// which of their callers sit in files this turn did not touch? Those are
// listed as `file:line`, once per symbol per session, through the Stop
// event's `additionalContext` — documented as non-error feedback that lets
// the turn continue, as opposed to `decision: "block"`
// (https://code.claude.com/docs/en/hooks, "Stop decision control").
//
// Silent when: `stop_hook_active` (a continuation this or another Stop hook
// caused — never loop), no session edit log, no index, not a git work tree,
// no signature changed, every caller's file was touched this turn, or the
// symbol was already reported this session. CODE_GRAPH_QUIET_HOOKS=1 silences
// it like the other injecting hooks.
//
// "Touched this turn" = an Edit to the file was logged since the previous Stop,
// OR the file's mtime is at or after the turn's start (covers Write, Bash
// `sed -i`, formatters). The turn starts at the previous Stop; the first turn
// of a session starts at its first logged Edit. Both only ever SUPPRESS a
// line, so an over-wide "touched" set costs a missed report, never a wrong one.
const { execFileSync } = require('child_process');
const fs = require('fs');
const path = require('path');
const { hidden } = require('./proc-opts');

// Per-Stop work caps. The hook blocks the end of the turn, so its worst case
// is bounded by count as well as by the registered budget.
const MAX_SYMBOLS_CHECKED = 8;
const MAX_CALLERS_LISTED = 8;
// A signature longer than this is cut: both sides are cut the same way, so
// the comparison stays symmetric.
const MAX_SIGNATURE_CHARS = 600;

const COMMENT_LINE = /^\s*(?:\/\/|#|\*|\/\*|--|;)/;
// First words that make a `Type name(` line a statement, not a definition.
const NOT_A_TYPE = new Set([
  'return', 'new', 'throw', 'else', 'await', 'yield', 'case', 'delete', 'typeof',
  'echo', 'print', 'puts', 'if', 'while', 'for', 'switch', 'catch', 'when', 'not',
  'and', 'or', 'in', 'is', 'of', 'do', 'assert', 'raise', 'import', 'from', 'use',
  'let', 'const', 'var', 'mut', 'match', 'go', 'defer', 'sizeof', 'co_return',
]);
// Files whose definitions end at a `:` (Python) or a newline (Ruby, Lua, …)
// rather than at `{` / `;`.
const COLON_TERMINATED = new Set(['.py', '.pyi']);
const NEWLINE_TERMINATED = new Set(['.rb', '.lua', '.ex', '.exs', '.jl']);

function escapeRe(s) {
  return String(s).replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
}

/**
 * Normalized signatures of every definition of `symbol` in `text`, sorted.
 *
 * Text heuristics, applied IDENTICALLY to the HEAD and the working-tree copy:
 * the question is only "did any definition's header change", so a shape the
 * patterns misread in both copies cancels out. A definition is located by the
 * line it starts on (keyword `fn`/`function`/`def`/`func`, a `const x = (…) =>`
 * binding, a class-method line, or a C-family `Type name(` line) and runs from
 * that line's start to the first depth-0 `{`, `;` or `=>` — `:` for Python, a
 * newline for Ruby-like files. The key drops all whitespace and a trailing
 * comma, so a formatter re-wrapping the parameter list is not a change.
 * @returns {string[]}
 */
function extractSignatures(text, symbol, ext = '') {
  if (typeof text !== 'string' || !symbol) return [];
  const S = escapeRe(symbol);
  const patterns = [
    new RegExp(`\\b(?:fn|function\\*?|def|func|sub|proc)\\s+(?:\\([^)\\n]*\\)\\s*)?${S}\\b`, 'g'),
    new RegExp(`\\b(?:const|let|var)\\s+${S}\\s*=\\s*(?:async\\s+)?(?:function\\b|\\([^)\\n]*\\)\\s*=>|[A-Za-z_$][\\w$]*\\s*=>)`, 'g'),
    new RegExp(`^[ \\t]*(?:(?:async|static|get|set|public|private|protected|readonly|override)\\s+)*\\*?${S}\\s*\\([^()\\n]*\\)\\s*(?::\\s*[^{;=\\n]+)?\\{[ \\t]*$`, 'gm'),
    new RegExp(`^[ \\t]*(?:[A-Za-z_][\\w<>\\[\\],.*&:?]*[ \\t]+)+${S}[ \\t]*\\(`, 'gm'),
  ];
  const lineStarts = new Set();
  for (const re of patterns) {
    for (const m of text.matchAll(re)) {
      const at = text.lastIndexOf('\n', m.index) + 1;
      const lineEnd = text.indexOf('\n', at);
      const line = text.slice(at, lineEnd === -1 ? text.length : lineEnd);
      if (COMMENT_LINE.test(line)) continue;
      const first = (line.trim().match(/^[A-Za-z_]\w*/) || [''])[0];
      if (re === patterns[3] && NOT_A_TYPE.has(first)) continue;
      lineStarts.add(at);
    }
  }
  const out = [];
  for (const at of lineStarts) out.push(signatureKey(readHeader(text, at, ext)));
  return out.sort();
}

function readHeader(text, at, ext) {
  const colon = COLON_TERMINATED.has(ext);
  const newline = NEWLINE_TERMINATED.has(ext);
  let depth = 0;
  let i = at;
  const end = Math.min(text.length, at + MAX_SIGNATURE_CHARS);
  for (; i < end; i++) {
    const c = text[i];
    if (c === '(' || c === '[') depth++;
    else if (c === ')' || c === ']') depth = Math.max(0, depth - 1);
    else if (depth === 0) {
      if (c === '{' || c === ';') break;
      if (c === '=' && text[i + 1] === '>') break;
      if (colon && c === ':') break;
      if (newline && c === '\n' && i > at) break;
    }
  }
  return text.slice(at, i);
}

function signatureKey(header) {
  return header.replace(/\s+/g, '').replace(/,\)/g, ')');
}

/** True when the definitions differ in number or in any header. */
function signatureChanged(oldSigs, newSigs) {
  if (oldSigs.length !== newSigs.length) return true;
  return oldSigs.some((s, i) => s !== newSigs[i]);
}

/**
 * The line of the first mention of `symbol` at or after the caller's own
 * start line — the call site, which is what a reader needs to open. Falls
 * back to the caller's start line.
 */
function findCallSiteLine(text, symbol, startLine) {
  const start = Math.max(1, Number(startLine) || 1);
  if (typeof text !== 'string') return start;
  const lines = text.split('\n');
  const re = new RegExp(`(?:^|[^\\w$])${escapeRe(symbol)}(?![\\w$])`);
  for (let i = start - 1; i < lines.length && i < start - 1 + 400; i++) {
    if (!COMMENT_LINE.test(lines[i]) && re.test(lines[i])) return i + 1;
  }
  return start;
}

/**
 * The decision, with every side effect injected (unit-testable without git or
 * a binary). Returns the lines to report and the next Stop state.
 *
 * @param {object} a
 * @param {{ts:number,file:string,symbol:string|null}[]} a.edits whole session log
 * @param {{lastStopAt:number|null, reported:string[]}} a.state
 * @param {number} a.now
 * @param {(file:string)=>string|null} a.headText  file at HEAD, null if absent
 * @param {(file:string)=>string|null} a.workText  file in the working tree
 * @param {(symbol:string,file:string)=>({file:string,line:number,name:string}[]|null)} a.callers
 * @param {(file:string)=>number|null} a.mtimeMs
 */
function computeStopReport({ edits, state, now, headText, workText, callers, mtimeMs }) {
  const lastStopAt = state.lastStopAt;
  const turn = edits.filter((e) => lastStopAt === null || e.ts > lastStopAt);
  const next = { lastStopAt: now, reported: [...state.reported] };
  if (turn.length === 0) return { lines: [], state: next };

  const turnStart = lastStopAt !== null ? lastStopAt : Math.min(...edits.map((e) => e.ts));
  const editedThisTurn = new Set(turn.map((e) => e.file));
  const touched = (file) => {
    if (editedThisTurn.has(file)) return true;
    const m = mtimeMs(file);
    return m !== null && m >= turnStart;
  };

  const seen = new Set();
  const candidates = [];
  for (const e of turn) {
    if (!e.symbol) continue;
    const key = `${e.file}#${e.symbol}`;
    if (seen.has(key) || next.reported.includes(key)) continue;
    seen.add(key);
    candidates.push({ key, file: e.file, symbol: e.symbol });
  }

  const lines = [];
  for (const c of candidates.slice(0, MAX_SYMBOLS_CHECKED)) {
    const ext = path.extname(c.file).toLowerCase();
    const before = headText(c.file);
    const after = workText(c.file);
    if (before === null || after === null) continue;          // new or deleted file
    const oldSigs = extractSignatures(before, c.symbol, ext);
    const newSigs = extractSignatures(after, c.symbol, ext);
    if (oldSigs.length === 0 || newSigs.length === 0) continue; // added / removed / not found
    if (!signatureChanged(oldSigs, newSigs)) continue;          // body-only edit

    const refs = callers(c.symbol, c.file);
    if (!refs || refs.length === 0) continue;
    const untouched = [];
    const dedup = new Set();
    for (const r of refs) {
      if (r.file === c.file || touched(r.file)) continue;
      const id = `${r.file}:${r.line}`;
      if (dedup.has(id)) continue;
      dedup.add(id);
      untouched.push(r);
    }
    if (untouched.length === 0) continue;
    untouched.sort((x, y) => (x.file < y.file ? -1 : x.file > y.file ? 1 : x.line - y.line));
    const shown = untouched.slice(0, MAX_CALLERS_LISTED)
      .map((r) => `${r.file}:${r.line}${r.name && r.name !== '<module>' ? ` (${r.name})` : ''}`);
    let line = `  ${c.symbol}() in ${c.file}: ${shown.join(', ')}`;
    if (untouched.length > MAX_CALLERS_LISTED) {
      line += `, and ${untouched.length - MAX_CALLERS_LISTED} more (code-graph-mcp refs ${c.symbol} --file ${c.file})`;
    }
    lines.push(line);
    next.reported.push(c.key);
  }
  return { lines, state: next };
}

function formatStopContext(lines) {
  if (!lines.length) return null;
  return '[code-graph] Signatures changed this turn (working tree vs HEAD); ' +
    'their callers in files not edited this turn, from the code-graph index:\n' +
    lines.join('\n') + '\n';
}

// --- Side-effecting shell ---------------------------------------------------

function readStdinJson() {
  try {
    // fd 0, not '/dev/stdin' (ENXIO on socketpair stdin) — as the other hooks.
    return JSON.parse(fs.readFileSync(0, 'utf8'));
  } catch { return null; }
}

function runMain() {
  if (process.env.CODE_GRAPH_QUIET_HOOKS === '1') return;
  const input = readStdinJson();
  if (!input || input.stop_hook_active === true) return;
  if (!input.session_id) return;

  const { resolveProjectRoot } = require('./project-root');
  const root = resolveProjectRoot(typeof input.cwd === 'string' && input.cwd ? input.cwd : process.cwd());
  if (root === null) return;

  const sessionEdits = require('./session-edits');
  const edits = sessionEdits.readEdits(root, input.session_id);
  if (edits.length === 0) return; // no log → nothing to check, and nothing written
  const state = sessionEdits.readStopState(root, input.session_id);

  const { remainingMs } = require('./hook-fail-open');
  const run = (cmd, args, defaultMs) => {
    const budget = remainingMs(defaultMs);
    if (budget === null) return null;
    try {
      return execFileSync(cmd, args, hidden({
        cwd: root,
        timeout: budget,
        encoding: 'utf8',
        stdio: ['ignore', 'pipe', 'ignore'],
        env: { ...process.env, CODE_GRAPH_INTERNAL: '1' },
        maxBuffer: 16 * 1024 * 1024,
      }));
    } catch { return null; }
  };

  const inGit = run('git', ['rev-parse', '--is-inside-work-tree'], 1000);
  let binary = null;
  let report = { lines: [], state: { lastStopAt: Date.now(), reported: state.reported } };
  if (inGit !== null && inGit.trim() === 'true') {
    binary = require('./find-binary').findBinary();
  }
  if (binary) {
    report = computeStopReport({
      edits,
      state,
      now: Date.now(),
      // `HEAD:./<path>` is resolved against cwd (= root), which need not be
      // the top of the git work tree.
      headText: (file) => run('git', ['show', `HEAD:./${file}`], 1000),
      workText: (file) => {
        try { return fs.readFileSync(path.join(root, file), 'utf8'); } catch { return null; }
      },
      callers: (symbol, file) => {
        const raw = run(binary, ['refs', symbol, '--file', file, '--relation', 'calls',
          '--min-confidence', 'inferred', '--json'], 1500);
        if (raw === null) return null;
        let parsed;
        try { parsed = JSON.parse(raw); } catch { return null; }
        if (!parsed || !Array.isArray(parsed.references)) return null;
        return parsed.references
          .filter((r) => r && typeof r.file_path === 'string')
          .map((r) => {
            let text = null;
            try { text = fs.readFileSync(path.join(root, r.file_path), 'utf8'); } catch { /* gone */ }
            return { file: r.file_path, name: r.name, line: findCallSiteLine(text, symbol, r.start_line) };
          });
      },
      mtimeMs: (file) => {
        try { return fs.statSync(path.join(root, file)).mtimeMs; } catch { return null; }
      },
    });
  }
  sessionEdits.writeStopState(root, input.session_id, report.state);

  const text = formatStopContext(report.lines);
  if (!text) return;
  const { emitEventContext } = require('./hook-emit');
  process.stdout.write(emitEventContext('Stop', text) + '\n');
}

if (require.main === module) runMain();

module.exports = {
  extractSignatures, signatureChanged, findCallSiteLine, computeStopReport, formatStopContext,
  MAX_SYMBOLS_CHECKED, MAX_CALLERS_LISTED,
};
