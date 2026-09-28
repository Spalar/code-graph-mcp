#!/usr/bin/env node
'use strict';
// FIRST statement, before the other requires — same reason as pre-edit-guide.js.
if (require.main === module) require('./hook-fail-open').installHookFailOpen('Stop');

// Stop hook (P1 #3b): the end-of-turn half of the edit-impact check.
//
// pre-edit-guide.js pushes "this function has N callers" BEFORE an edit, when
// nobody yet knows whether the edit will change the signature. This runs when
// the turn ends and asks the question that can be answered then: of the
// symbols edited this turn, which ones' signatures did THIS TURN change, and
// which of their callers sit in files this turn did not touch? Those are
// listed as `file:line`, once per symbol per session, through the Stop
// event's `additionalContext` — documented as non-error feedback that lets
// the turn continue, as opposed to `decision: "block"`
// (https://code.claude.com/docs/en/hooks, "Stop decision control").
//
// "Changed this turn" compares the working tree with the signatures
// pre-edit-guide recorded just before the turn's FIRST Edit of the symbol
// (session-edits.js `sigs`) — not with HEAD. Both sides of the comparison are
// then scoped to the same turn: a signature changed in turn 1, with its
// callers fixed in turn 1, is not re-raised by a body-only edit in turn 2
// (review H1), and uncommitted work from before the session is not reported
// as this turn's.
//
// Silent when: `stop_hook_active` (a continuation this or another Stop hook
// caused — never loop), no session edit log, no index, not a git work tree,
// no signature changed, the language has no exact header reading (LANGS),
// every caller's file was touched this turn, or the symbol was already
// reported this session. CODE_GRAPH_QUIET_HOOKS=1 silences it like the other
// injecting hooks.
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
const { shellQuoteArg, formatCgCommand } = require('./cg-answer');

// Per-Stop work caps. The hook blocks the end of the turn, so its worst case
// is bounded by count as well as by the registered budget. The symbol cap
// bounds the `refs` queries, so it applies AFTER the text-only "did the
// signature change" filter (review M1): body-only edits do not use it up.
const MAX_SYMBOLS_CHECKED = 8;
const MAX_CALLERS_LISTED = 8;
// A header that has not ended within this many characters is not read at all
// (no verdict), rather than cut: a cut header can run into the body, and a
// body edit would then read as a signature change.
const MAX_SIGNATURE_CHARS = 600;

const COMMENT_LINE = /^\s*(?:\/\/|#|\*|\/\*|--|;)/;
// First words that make a C-family `Type name(` line a statement, not a
// definition.
const NOT_A_TYPE = new Set([
  'return', 'new', 'throw', 'else', 'await', 'yield', 'case', 'delete', 'typeof',
  'echo', 'print', 'puts', 'if', 'while', 'for', 'switch', 'catch', 'when', 'not',
  'and', 'or', 'in', 'is', 'of', 'do', 'assert', 'raise', 'import', 'from', 'use',
  'let', 'const', 'var', 'mut', 'match', 'go', 'defer', 'sizeof', 'co_return',
  'co_await', 'co_yield', 'default', 'goto',
]);

// How each language's definition headers are found and where they end. Only
// languages whose headers these rules read exactly are listed; every other
// extension gets no verdict, and the Stop hook says nothing about its symbols
// (review M2: silent beats wrong).
//
//   keyword  a definition is `<keyword> name` (`receiver`: Go's `func (r T) name`;
//            `typeParams`: Kotlin's `fun <T> name`)
//   jsForms  also `const name = (…) =>` bindings and one-line class methods
//   typed    C-family `Type name(` lines — no keyword exists
//   ends     characters that end the header at bracket depth 0; `\n` included
//            only where a newline outside brackets cannot continue a header
//   arrowEnds  `=>` at depth 0 ends it (JS arrow bodies, C# expression bodies);
//            elsewhere `=>` is part of a type (Scala) and never an end
//   angles   `<…>` nests like a bracket (TS generics may hold `{`)
//   afterParams  a lone `:` after the parameter list ends it (C++ member
//            initializers, C# `: base(…)` — body, not signature)
//   closeParen  the parameter list's `)` ends it (Ruby, Lua: no return types,
//            and a one-line body may follow on the same line)
const LANGS = new Map();
function defineLang(exts, profile) {
  for (const e of exts) LANGS.set(e, profile);
}
defineLang(['.rs'], { keyword: 'fn', ends: '{;' });
defineLang(['.py', '.pyi'], { keyword: 'def', ends: ':' });
defineLang(['.go'], { keyword: 'func', receiver: true, ends: '{;\n' });
defineLang(['.js', '.jsx', '.mjs', '.cjs', '.ts', '.tsx', '.mts', '.cts'],
  { keyword: 'function\\*?', jsForms: true, ends: '{;', arrowEnds: true, angles: true });
defineLang(['.php'], { keyword: 'function', ends: '{;' });
defineLang(['.c', '.h', '.cc', '.cpp', '.cxx', '.hpp', '.hh', '.hxx', '.java', '.cs'],
  { typed: true, ends: '{;', arrowEnds: true, afterParams: true });
defineLang(['.kt', '.kts'], { keyword: 'fun', typeParams: true, ends: '{=\n' });
defineLang(['.scala', '.sc'], { keyword: 'def', ends: '{=\n' });
defineLang(['.rb'], { keyword: 'def', ends: ';=\n', closeParen: true });
defineLang(['.lua'], { keyword: 'function', ends: '\n', closeParen: true });

function escapeRe(s) {
  return String(s).replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
}

function definitionPatterns(S, lang) {
  const out = [];
  if (lang.keyword) {
    const receiver = lang.receiver ? '(?:\\([^)\\n]*\\)\\s*)?' : '';
    const typeParams = lang.typeParams ? '(?:<[^>\\n]*>\\s*)?' : '';
    out.push({ re: new RegExp(`\\b(?:${lang.keyword})\\s+${receiver}${typeParams}${S}\\b`, 'g') });
  }
  if (lang.jsForms) {
    out.push({ re: new RegExp(`\\b(?:const|let|var)\\s+${S}\\s*=\\s*(?:async\\s+)?(?:function\\b|\\([^)\\n]*\\)\\s*=>|[A-Za-z_$][\\w$]*\\s*=>)`, 'g') });
    out.push({ re: new RegExp(`^[ \\t]*(?:(?:async|static|get|set|public|private|protected|readonly|override)\\s+)*\\*?${S}\\s*\\([^()\\n]*\\)\\s*(?::\\s*[^{;=\\n]+)?\\{[ \\t]*$`, 'gm') });
  }
  if (lang.typed) {
    out.push({ re: new RegExp(`^[ \\t]*((?:[A-Za-z_][\\w<>\\[\\],.*&:?]*[ \\t]+)+)${S}[ \\t]*\\(`, 'gm'), typed: true });
  }
  return out;
}

/**
 * Normalized signatures of every definition of `symbol` in `text`, sorted —
 * or null when there is no exact reading: the language is not in LANGS, or a
 * header did not end within MAX_SIGNATURE_CHARS.
 *
 * A definition is located by the line it starts on (LANGS) and runs from that
 * line's start to the first end character at bracket depth 0. The key drops
 * all whitespace and a trailing comma, so a formatter re-wrapping the
 * parameter list is not a change.
 * @returns {string[]|null}
 */
function extractSignatures(text, symbol, ext = '') {
  const lang = LANGS.get(String(ext).toLowerCase());
  if (!lang) return null;
  if (typeof text !== 'string' || !symbol) return [];
  const lineStarts = new Set();
  for (const { re, typed } of definitionPatterns(escapeRe(symbol), lang)) {
    for (const m of text.matchAll(re)) {
      const at = text.lastIndexOf('\n', m.index) + 1;
      const lineEnd = text.indexOf('\n', at);
      const line = text.slice(at, lineEnd === -1 ? text.length : lineEnd);
      if (COMMENT_LINE.test(line)) continue;
      if (typed) {
        const first = (line.trim().match(/^[A-Za-z_]\w*/) || [''])[0];
        if (NOT_A_TYPE.has(first)) continue;
        // `label: f(`, `case X: f(`, `public: f(` — a token ending in a lone
        // `:` is a label or access specifier, never part of a return type.
        if (m[1].split(/[ \t]+/).some((tok) => /(?:^|[^:]):$/.test(tok))) continue;
      }
      lineStarts.add(at);
    }
  }
  const out = [];
  for (const at of lineStarts) {
    const header = readHeader(text, at, lang);
    if (header === null) return null;
    out.push(signatureKey(header));
  }
  return out.sort();
}

function readHeader(text, at, lang) {
  let depth = 0;
  let angle = 0;
  let sawParams = false;
  const limit = Math.min(text.length, at + MAX_SIGNATURE_CHARS);
  for (let i = at; i < limit; i++) {
    const c = text[i];
    if (c === '=' && text[i + 1] === '>') {
      if (lang.arrowEnds && depth === 0 && angle === 0) return text.slice(at, i);
      i++; // part of a type or a default value: step over both characters
      continue;
    }
    if (c === '(' || c === '[') { depth++; continue; }
    if (c === ')' || c === ']') {
      depth = Math.max(0, depth - 1);
      if (c === ')' && depth === 0) {
        sawParams = true;
        if (lang.closeParen) return text.slice(at, i + 1);
      }
      continue;
    }
    if (depth !== 0) continue;
    if (lang.angles && c === '<') { angle++; continue; }
    if (lang.angles && c === '>') { angle = Math.max(0, angle - 1); continue; }
    if (angle !== 0) continue;
    if (lang.ends.includes(c) && !(c === '\n' && i === at)) return text.slice(at, i);
    if (lang.afterParams && sawParams && c === ':' && text[i - 1] !== ':' && text[i + 1] !== ':') {
      return text.slice(at, i);
    }
  }
  // End of text ends a header; running out of the character budget does not.
  return limit === text.length ? text.slice(at, limit) : null;
}

function signatureKey(header) {
  return header.replace(/\s+/g, '').replace(/,\)/g, ')');
}

/**
 * True when a definition that existed before is gone: its header changed, or
 * it was removed while others of the same name remain. A definition ADDED
 * beside unchanged ones (a new overload, a new `impl From<B>`) changes no
 * existing caller's target, so it is not a change (review M2).
 */
function signatureChanged(oldSigs, newSigs) {
  const remaining = new Map();
  for (const s of newSigs) remaining.set(s, (remaining.get(s) || 0) + 1);
  for (const s of oldSigs) {
    const n = remaining.get(s) || 0;
    if (n === 0) return true;
    remaining.set(s, n - 1);
  }
  return false;
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
 * @param {{ts:number,file:string,symbol:string|null,sigs:string[]|null}[]} a.edits
 *   whole session log; `sigs` = the symbol's signatures just before that Edit
 * @param {{lastStopAt:number|null, reported:string[]}} a.state
 * @param {number} a.now
 * @param {(file:string)=>string|null} a.workText  file in the working tree
 * @param {(symbol:string,file:string)=>({file:string,line:number,name:string}[]|null)} a.callers
 * @param {(file:string)=>number|null} a.mtimeMs
 */
function computeStopReport({ edits, state, now, workText, callers, mtimeMs }) {
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

  // One candidate per (file, symbol). Its baseline is what pre-edit-guide
  // recorded before the turn's FIRST Edit of it — the signature as the turn
  // found it. Later records of the same symbol this turn were taken after
  // earlier edits had landed, so they are not the baseline.
  const seen = new Set();
  const changed = [];
  for (const e of turn) {
    if (!e.symbol) continue;
    const key = `${e.file}#${e.symbol}`;
    if (seen.has(key)) continue;
    seen.add(key);
    if (next.reported.includes(key)) continue;
    // null: no exact reading (language, unreadable file, a pre-repair log
    // line). [] (the definition was added this turn) needs no test of its
    // own: signatureChanged counts only definitions that went missing.
    if (!Array.isArray(e.sigs)) continue;
    const after = extractSignatures(workText(e.file), e.symbol, path.extname(e.file).toLowerCase());
    if (!after || after.length === 0) continue;     // removed, or no reading now
    if (!signatureChanged(e.sigs, after)) continue; // body-only this turn
    changed.push({ key, file: e.file, symbol: e.symbol });
  }

  const lines = [];
  for (const c of changed.slice(0, MAX_SYMBOLS_CHECKED)) {
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
    // Every repo-derived token is shell-quoted (review M3): a file name is
    // text from the repository, and the model may paste it into a command.
    const shown = untouched.slice(0, MAX_CALLERS_LISTED).map((r) => {
      const name = r.name && r.name !== '<module>' ? ' (' + shellQuoteArg(r.name) + ')' : '';
      return shellQuoteArg(r.file) + ':' + r.line + name;
    });
    let line = '  ' + shellQuoteArg(c.symbol) + '() in ' + shellQuoteArg(c.file) + ': ' + shown.join(', ');
    if (untouched.length > MAX_CALLERS_LISTED) {
      line += ', and ' + (untouched.length - MAX_CALLERS_LISTED) + ' more (' +
        formatCgCommand(['refs', c.symbol, '--file', c.file]) + ')';
    }
    lines.push(line);
    next.reported.push(c.key);
  }
  // Changed beyond the query cap: named, not dropped in silence (review M1).
  // Not marked reported, and a later turn does not see them as changed again
  // (their baseline moves on), so this line is the only mention they get.
  const unchecked = changed.slice(MAX_SYMBOLS_CHECKED);
  if (unchecked.length > 0) {
    lines.push('  ' + unchecked.length + ' more changed, callers not checked: ' +
      unchecked.map((c) => shellQuoteArg(c.symbol) + '() in ' + shellQuoteArg(c.file)).join(', '));
  }
  return { lines, state: next };
}

function formatStopContext(lines) {
  if (!lines.length) return null;
  // No apostrophe in the fixed text: file names are single-quoted in it.
  return '[code-graph] Signatures changed this turn (each compared with its file before ' +
    'the first edit of it this turn); their callers in files not edited this turn, from the code-graph index:\n' +
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

  // The comparison no longer reads HEAD (baselines come from the edit log),
  // but "silent outside a git work tree" is the spec's contract for this
  // hook, so the gate stays.
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
