'use strict';
const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const os = require('node:os');
const path = require('node:path');
const { spawnSync, execFileSync } = require('node:child_process');

// These tests redirect HOME for their children; CLAUDE_CONFIG_DIR would win
// over it (claude-config.js) and point them at the real config. Dropped at
// module load so every spawn below inherits the sandbox, not the live dir.
delete process.env.CLAUDE_CONFIG_DIR;

const {
  extractSignatures, signatureChanged, findCallSiteLine, computeStopReport, formatStopContext, followUpOf,
  reportRecords, MAX_FOLLOWUP_FILES,
} = require('./stop-impact');

// --- extractSignatures ------------------------------------------------------

test('extractSignatures: Rust header up to the brace, whitespace-insensitive', () => {
  const a = extractSignatures('pub fn compute(x: i32) -> i32 {\n    x + 1\n}\n', 'compute', '.rs');
  const b = extractSignatures('pub fn compute(\n    x: i32,\n) -> i32 {\n    x + 2\n}\n', 'compute', '.rs');
  assert.deepEqual(a, ['pubfncompute(x:i32)->i32']);
  assert.deepEqual(a, b, 'a formatter re-wrapping the params is not a signature change');
  const c = extractSignatures('pub fn compute(x: i32, y: i32) -> i32 {\n}\n', 'compute', '.rs');
  assert.ok(signatureChanged(a, c));
  const ret = extractSignatures('pub fn compute(x: i32) -> std::io::Result<i32> {\n}\n', 'compute', '.rs');
  assert.ok(signatureChanged(a, ret), 'a return-type change counts, `::` included');
});

test('extractSignatures: Python stops at the depth-0 colon, annotations kept', () => {
  const a = extractSignatures('def load(path: str) -> dict:\n    return {}\n', 'load', '.py');
  assert.deepEqual(a, ['defload(path:str)->dict']);
  const b = extractSignatures('def load(path: str, strict: bool = False) -> dict:\n    pass\n', 'load', '.py');
  assert.ok(signatureChanged(a, b));
});

test('extractSignatures: JS function, arrow binding and class method; calls are not definitions', () => {
  const src = [
    'function parse(a) {', '  return a;', '}',
    'const build = (x, y) => x + y;',
    'class K {', '  render(props) {', '    return parse(props);', '  }', '}',
    'parse(1);', 'render(() => {', '});', 'return build(1, 2);',
  ].join('\n');
  assert.deepEqual(extractSignatures(src, 'parse', '.js'), ['functionparse(a)']);
  assert.deepEqual(extractSignatures(src, 'build', '.js'), ['constbuild=(x,y)']);
  assert.deepEqual(extractSignatures(src, 'render', '.js'), ['render(props)']);
});

test('extractSignatures: a match on a line over 2,000 characters gives no reading (pre-tag review H2)', () => {
  const head = 'function abc(t) {';
  const withLine = (n) => head + ' '.repeat(n - head.length) + '\n  return t;\n}\n';
  assert.equal(withLine(2000).indexOf('\n'), 2000);
  assert.deepEqual(extractSignatures(withLine(2000), 'abc', '.js'), ['functionabc(t)']);
  assert.equal(extractSignatures(withLine(2001), 'abc', '.js'), null);
  // A long line without a match does not matter.
  assert.deepEqual(extractSignatures('x'.repeat(5000) + '\n' + withLine(20), 'abc', '.js'), ['functionabc(t)']);
  // A one-line 1.5 MB bundle is refused at its first match instead of
  // re-finding its line per match (1ffc45c: about 9 s of CPU here).
  const unit = 'function abc(t){return t+1}var a=abc(1);';
  const bundle = unit.repeat(Math.ceil((1536 * 1024) / unit.length));
  const t0 = process.cpuUsage();
  const got = extractSignatures(bundle, 'abc', '.js');
  const d = process.cpuUsage(t0);
  assert.equal(got, null);
  const ms = (d.user + d.system) / 1e3;
  assert.ok(ms < 1000, `1.5 MB one-line bundle took ${ms.toFixed(0)} ms of CPU`);
});

test('extractSignatures: comments and absent symbols yield nothing', () => {
  assert.deepEqual(extractSignatures('// fn compute(x: i32) {\n', 'compute', '.rs'), []);
  assert.deepEqual(extractSignatures('fn other() {}\n', 'compute', '.rs'), []);
  assert.deepEqual(extractSignatures(null, 'compute', '.rs'), []);
});

test('findCallSiteLine: first mention at or after the caller start, else the start', () => {
  const text = 'use crate::a::compute;\n\npub fn caller_b() -> i32 {\n    let v = 2;\n    compute(v)\n}\n';
  assert.equal(findCallSiteLine(text, 'compute', 3), 5);
  assert.equal(findCallSiteLine(text, 'nothing_here', 3), 3);
  assert.equal(findCallSiteLine(null, 'compute', 7), 7);
});

// --- computeStopReport (IO injected) ----------------------------------------

const HEAD_A = 'pub fn compute(x: i32) -> i32 {\n    x + 1\n}\n';
const SIG_A = 'pub fn compute(x: i32, y: i32) -> i32 {\n    x + y\n}\n';
const BODY_A = 'pub fn compute(x: i32) -> i32 {\n    x + 2\n}\n';
// What pre-edit-guide records with a symbol: its signatures before that Edit.
const sigsOf = (text) => extractSignatures(text, 'compute', '.rs');

function world({ work = SIG_A, mtimes = {}, refs } = {}) {
  const calls = [];
  return {
    calls,
    workText: (f) => (f === 'src/a.rs' ? work : null),
    callers: (symbol, file) => { calls.push(symbol); return refs || [{ file: 'src/b.rs', line: 5, name: 'caller_b' }]; },
    mtimeMs: (f) => (f in mtimes ? mtimes[f] : 1000),
  };
}
const EMPTY = { lastStopAt: null, reported: [] };
const EDIT_A = [
  { ts: 5000, file: 'src/a.rs', symbol: null, sigs: null },
  { ts: 5001, file: 'src/a.rs', symbol: 'compute', sigs: sigsOf(HEAD_A) },
];

test('stop: signature change in a.rs with an untouched caller in b.rs → one b.rs:line line', () => {
  const r = computeStopReport({ edits: EDIT_A, state: EMPTY, now: 9000, ...world() });
  assert.deepEqual(r.lines, ['  compute() in src/a.rs: src/b.rs:5 (caller_b)']);
  assert.deepEqual(r.state, { lastStopAt: 9000, reported: ['src/a.rs#compute'] });
  const text = formatStopContext(r.lines);
  assert.match(text, /src\/b\.rs:5/);
  assert.doesNotMatch(text, /\b(?:must|should|you)\b/i, 'facts only');
});

test('stop: caller file also edited this turn → silent (logged edit, or mtime inside the turn)', () => {
  const logged = [...EDIT_A, { ts: 5002, file: 'src/b.rs', symbol: null, sigs: null }];
  assert.deepEqual(computeStopReport({ edits: logged, state: EMPTY, now: 9000, ...world() }).lines, []);
  // Not logged (Write, `sed -i`, a formatter) but modified after the turn began.
  const viaMtime = world({ mtimes: { 'src/b.rs': 6000 } });
  assert.deepEqual(computeStopReport({ edits: EDIT_A, state: EMPTY, now: 9000, ...viaMtime }).lines, []);
});

test('stop: mtime exactly at the turn start counts as touched', () => {
  const r = computeStopReport({ edits: EDIT_A, state: { lastStopAt: 4000, reported: [] }, now: 9000,
    ...world({ mtimes: { 'src/b.rs': 4000 } }) });
  assert.deepEqual(r.lines, []);
});

test('stop: the first turn starts at the session\'s FIRST logged edit, not its last', () => {
  // b.rs changed between the first and the last edit of the turn: touched.
  const edits = [...EDIT_A, { ts: 7000, file: 'src/c.rs', symbol: null, sigs: null }];
  const r = computeStopReport({ edits, state: EMPTY, now: 9000, ...world({ mtimes: { 'src/b.rs': 6000 } }) });
  assert.deepEqual(r.lines, []);
});

test('stop: second Stop in the same session → silent, even after a new edit of the symbol', () => {
  const first = computeStopReport({ edits: EDIT_A, state: EMPTY, now: 9000, ...world() });
  assert.equal(first.lines.length, 1);
  const again = computeStopReport({ edits: EDIT_A, state: first.state, now: 9500, ...world() });
  assert.deepEqual(again.lines, [], 'no edits since the last Stop');
  // A record whose baseline still differs: silent only because it was reported.
  const reEdit = [...EDIT_A, { ts: 9600, file: 'src/a.rs', symbol: 'compute', sigs: sigsOf(HEAD_A) }];
  assert.deepEqual(computeStopReport({ edits: reEdit, state: first.state, now: 9900, ...world() }).lines, [],
    'once per symbol per session');
});

test('stop H1: a later turn compares with its own baseline, not HEAD', () => {
  // Turn 1 changed the signature (and was silent, e.g. its caller was fixed).
  // Turn 2 edits only the body: its record's baseline is turn 1's result.
  const turn2 = [...EDIT_A, { ts: 9600, file: 'src/a.rs', symbol: 'compute', sigs: sigsOf(SIG_A) }];
  const body2 = SIG_A.replace('x + y', 'x + y + 1');
  const r = computeStopReport({ edits: turn2, state: { lastStopAt: 9000, reported: [] }, now: 9900, ...world({ work: body2 }) });
  assert.deepEqual(r.lines, []);
});

test('stop H1: the baseline is the turn\'s FIRST record of the symbol', () => {
  // Two edits of compute this turn; the second was recorded after the first
  // had already changed the signature.
  const edits = [...EDIT_A, { ts: 5003, file: 'src/a.rs', symbol: 'compute', sigs: sigsOf(SIG_A) }];
  const r = computeStopReport({ edits, state: EMPTY, now: 9000, ...world() });
  assert.equal(r.lines.length, 1, 'changed against the first record');
});

test('stop: body-only change → silent; new, vanished, or unreadable definition → silent', () => {
  assert.deepEqual(computeStopReport({ edits: EDIT_A, state: EMPTY, now: 9000, ...world({ work: BODY_A }) }).lines, []);
  assert.deepEqual(computeStopReport({ edits: EDIT_A, state: EMPTY, now: 9000, ...world({ work: 'fn other() {}\n' }) }).lines, []);
  const withSigs = (sigs) => [EDIT_A[0], { ...EDIT_A[1], sigs }];
  assert.deepEqual(computeStopReport({ edits: withSigs([]), state: EMPTY, now: 9000, ...world() }).lines, [],
    'added this turn');
  assert.deepEqual(computeStopReport({ edits: withSigs(null), state: EMPTY, now: 9000, ...world() }).lines, [],
    'no baseline (unsupported language, pre-repair log line)');
});

test('stop: edits before the previous Stop belong to an earlier turn', () => {
  const r = computeStopReport({ edits: EDIT_A, state: { lastStopAt: 8000, reported: [] }, now: 9000, ...world() });
  assert.deepEqual(r.lines, []);
  assert.equal(r.state.lastStopAt, 9000);
});

test('stop: callers are capped, sorted, and the rest named with the command that lists them', () => {
  const refs = Array.from({ length: 11 }, (_, i) => ({ file: `src/c${String(i).padStart(2, '0')}.rs`, line: 1, name: `f${i}` }));
  const r = computeStopReport({ edits: EDIT_A, state: EMPTY, now: 9000, ...world({ refs }) });
  assert.equal(r.lines.length, 1);
  assert.equal((r.lines[0].match(/src\/c\d\d\.rs:1/g) || []).length, 8);
  assert.match(r.lines[0], /and 3 more \(code-graph-mcp refs compute --file src\/a\.rs\)$/);
});

test('stop: one caller line per file:line; a module-level caller has no name', () => {
  const refs = [
    { file: 'src/b.rs', line: 5, name: 'caller_b' },
    { file: 'src/b.rs', line: 5, name: 'caller_b' },
    { file: 'src/c.rs', line: 2, name: '<module>' },
  ];
  const r = computeStopReport({ edits: EDIT_A, state: EMPTY, now: 9000, ...world({ refs }) });
  assert.deepEqual(r.lines, ['  compute() in src/a.rs: src/b.rs:5 (caller_b), src/c.rs:2']);
});

test('stop M1: the symbol cap bounds refs queries over CHANGED symbols only; the rest are named', () => {
  const n = 10;
  const fns = (sig) => Array.from({ length: n }, (_, i) => `pub fn s${i}(x: i32${sig}) {\n}\n`).join('');
  const before = fns('');
  const after = fns(', y: i32');
  // Eight body-only edits first, then ten real changes.
  const edits = [];
  for (let i = 0; i < 8; i++) edits.push({ ts: 5000 + i, file: 'src/z.rs', symbol: `body${i}`, sigs: [`pubfnbody${i}()`] });
  for (let i = 0; i < n; i++) {
    edits.push({ ts: 6000 + i, file: 'src/a.rs', symbol: `s${i}`, sigs: extractSignatures(before, `s${i}`, '.rs') });
  }
  const w = world({ work: after });
  const bodies = Array.from({ length: 8 }, (_, i) => `pub fn body${i}() {\n  ${i}\n}\n`).join('');
  w.workText = (f) => (f === 'src/a.rs' ? after : f === 'src/z.rs' ? bodies : null);
  const r = computeStopReport({ edits, state: EMPTY, now: 9000, ...w });
  assert.equal(w.calls.length, 8, `refs queries: ${w.calls.join(',')}`);
  assert.deepEqual(w.calls, ['s0', 's1', 's2', 's3', 's4', 's5', 's6', 's7']);
  assert.equal(r.lines.length, 9);
  assert.equal(r.lines[8], '  2 more changed, callers not checked: s8() in src/a.rs, s9() in src/a.rs');
  assert.equal(r.state.reported.length, 8, 'the unchecked two are not marked reported');
});

test('stop M3: every repo-derived token in the text is shell-quoted', () => {
  const edited = 'src/a$(touch PWNED).rs';
  const refs = Array.from({ length: 9 }, (_, i) => ({ file: `src/m${i} x.rs`, line: 3, name: `c${i}` }));
  refs[0] = { file: 'src/b`id`.rs', line: 3, name: "it's" };
  const w = world({ refs });
  w.workText = (f) => (f === edited ? SIG_A : null);
  const edits = [{ ts: 5001, file: edited, symbol: 'compute', sigs: sigsOf(HEAD_A) }];
  const [line] = computeStopReport({ edits, state: EMPTY, now: 9000, ...w }).lines;
  assert.ok(line.startsWith("  compute() in 'src/a$(touch PWNED).rs': 'src/b`id`.rs':3"), line);
  assert.ok(line.includes("'src/b`id`.rs':3 ('it'\\''s')"), line);
  assert.ok(line.includes("'src/m1 x.rs':3 (c1)"), line);
  assert.ok(line.endsWith("(code-graph-mcp refs compute --file 'src/a$(touch PWNED).rs')"), line);
});

// --- Spawned end to end: real git, real binary, sandbox HOME/TMPDIR ---------

function realBinary() {
  try { return require('./find-binary').findBinary(); } catch { return null; }
}
const BIN = process.platform === 'win32' ? null : realBinary();
const e2eSkip = !BIN && 'needs a code-graph-mcp binary and a POSIX shell';

const DEFAULT_FILES = {
  'src/lib.rs': 'pub mod a;\npub mod b;\n',
  'src/a.rs': HEAD_A,
  'src/b.rs': 'use crate::a::compute;\n\npub fn caller_b() -> i32 {\n    let v = 2;\n    compute(v)\n}\n',
};

function sandboxRepo(t, files = DEFAULT_FILES) {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), 'cg-stop-'));
  t.after(() => fs.rmSync(root, { recursive: true, force: true }));
  const home = path.join(root, 'home');
  const cache = path.join(home, '.cache', 'code-graph');
  fs.mkdirSync(cache, { recursive: true });
  fs.writeFileSync(path.join(cache, 'install-manifest.json'), '{"version":"9.9.9","config":{}}');
  fs.writeFileSync(path.join(cache, 'binary-path'), BIN);
  const tmp = path.join(root, 'tmp');
  fs.mkdirSync(tmp);
  const repo = path.join(root, 'repo');
  for (const [f, c] of Object.entries(files)) {
    fs.mkdirSync(path.dirname(path.join(repo, f)), { recursive: true });
    fs.writeFileSync(path.join(repo, f), c);
  }
  fs.writeFileSync(path.join(repo, '.gitignore'), '.code-graph/\n');
  const env = {
    ...process.env,
    HOME: home, USERPROFILE: home,
    TMPDIR: tmp, TMP: tmp, TEMP: tmp,
    CODE_GRAPH_QUIET_HOOKS: '0',
    CODE_GRAPH_DISABLE_MODEL_DOWNLOAD: '1',
    GIT_CONFIG_NOSYSTEM: '1',
  };
  const git = (...args) => execFileSync('git', ['-c', 'user.email=t@t', '-c', 'user.name=t', ...args],
    { cwd: repo, env, stdio: 'pipe' });
  git('init', '-q');
  git('add', '-A');
  git('commit', '-qm', 'init');
  // Every fixture file predates the session by an hour, so "touched this
  // turn" is decided by the edit log and the edits the test makes — never by
  // a same-millisecond mtime.
  const past = new Date(Date.now() - 3600 * 1000);
  for (const f of Object.keys(files)) fs.utimesSync(path.join(repo, f), past, past);
  const index = () => execFileSync(BIN, ['incremental-index'], { cwd: repo, env, stdio: 'pipe' });
  index();
  return { root, repo, env, tmp, index };
}

function hook(sb, script, payload) {
  return spawnSync(process.execPath, [path.join(__dirname, script)], {
    input: JSON.stringify(payload), cwd: sb.repo, env: sb.env, encoding: 'utf8', timeout: 20000,
  });
}

// What Claude Code does for one Edit: PreToolUse (logs the edit), the edit
// itself, PostToolUse (the incremental index; run directly here).
function edit(sb, session, file, oldString, newString) {
  const abs = path.join(sb.repo, file);
  const pre = hook(sb, 'pre-edit-guide.js', {
    session_id: session, hook_event_name: 'PreToolUse', tool_name: 'Edit', cwd: sb.repo,
    tool_input: { file_path: abs, old_string: oldString, new_string: newString },
  });
  assert.equal(pre.status, 0, pre.stderr);
  const text = fs.readFileSync(abs, 'utf8');
  assert.ok(text.includes(oldString), `${file} does not contain the old_string`);
  fs.writeFileSync(abs, text.replace(oldString, newString));
  sb.index();
}

function stop(sb, session, extra = {}) {
  const r = hook(sb, 'stop-impact.js', {
    session_id: session, hook_event_name: 'Stop', cwd: sb.repo, stop_hook_active: false, ...extra,
  });
  assert.equal(r.status, 0, r.stderr);
  return r.stdout ? JSON.parse(r.stdout) : null;
}

test('e2e: signature change with an untouched caller → exactly one b.rs:line, then silent', { skip: e2eSkip }, (t) => {
  const sb = sandboxRepo(t);
  edit(sb, 'sess-1', 'src/a.rs', 'pub fn compute(x: i32) -> i32 {', 'pub fn compute(x: i32, y: i32) -> i32 {');

  // A continuation caused by a Stop hook is never re-checked.
  assert.equal(stop(sb, 'sess-1', { stop_hook_active: true }), null);

  const out = stop(sb, 'sess-1');
  assert.ok(out, 'expected a Stop envelope');
  assert.equal(out.hookSpecificOutput.hookEventName, 'Stop');
  assert.equal(out.decision, undefined, 'feedback, not a block');
  const ctx = out.hookSpecificOutput.additionalContext;
  assert.equal((ctx.match(/src\/b\.rs:\d+/g) || []).length, 1, ctx);
  assert.match(ctx, /src\/b\.rs:5 \(caller_b\)/);

  assert.equal(stop(sb, 'sess-1'), null, 'second Stop in the same session is silent');
  // A different session keeps its own ledger, and compares with the signature
  // ITS turn started from: a body-only edit there changed no signature, so it
  // is silent even though the working tree still differs from HEAD (review
  // H1: this used to report, from a HEAD comparison).
  const header = 'pub fn compute(x: i32, y: i32) -> i32 {\n';
  edit(sb, 'sess-2', 'src/a.rs', header + '    x + 1', header + '    x + 1 + 0');
  assert.equal(stop(sb, 'sess-2'), null);
  // sess-3 changes the signature again, and is told.
  edit(sb, 'sess-3', 'src/a.rs', header, 'pub fn compute(x: i32, y: i64) -> i32 {\n');
  const third = stop(sb, 'sess-3');
  assert.ok(third, 'sess-3 changed the signature this turn');
  assert.match(third.hookSpecificOutput.additionalContext, /src\/b\.rs:5/);
});

// D#163: the roadmap's Stop metric ("reported, and adopted") needs both halves
// in recommendations.jsonl.
function stopRecords(sb) {
  let raw = '';
  try { raw = fs.readFileSync(path.join(sb.repo, '.code-graph', 'recommendations.jsonl'), 'utf8'); } catch { /* none */ }
  return raw.split('\n').filter(Boolean).map((l) => JSON.parse(l)).filter((r) => r.hook === 'stop');
}

test('e2e: a report is recorded, and the next Stop records that a listed caller file was edited', { skip: e2eSkip }, (t) => {
  const sb = sandboxRepo(t);
  edit(sb, 's', 'src/a.rs', 'pub fn compute(x: i32) -> i32 {', 'pub fn compute(x: i32, y: i32) -> i32 {');
  assert.ok(stop(sb, 's'), 'the signature change is reported');
  assert.deepEqual(stopRecords(sb).map((r) => [r.action, r.symbols, r.callers]), [['stop_check', 1, 1]]);

  // The turn continues on the feedback, fixes the caller, and ends with
  // stop_hook_active: that Stop is not re-checked, but it is the follow-up.
  edit(sb, 's', 'src/b.rs', '    compute(v)', '    compute(v, 1)');
  assert.equal(stop(sb, 's', { stop_hook_active: true }), null, 'a continuation is never re-checked');
  const after = stopRecords(sb);
  assert.deepEqual(after.map((r) => [r.action, r.adopted, r.listed, r.edited]),
    [['stop_check', undefined, undefined, undefined], ['stop_followup', true, 1, 1]]);

  // One follow-up per report.
  assert.equal(stop(sb, 's', { stop_hook_active: true }), null);
  assert.equal(stopRecords(sb).length, 2);
});

test('e2e: a report whose callers stay untouched records adopted:false at the next Stop', { skip: e2eSkip }, (t) => {
  const sb = sandboxRepo(t);
  edit(sb, 's', 'src/a.rs', 'pub fn compute(x: i32) -> i32 {', 'pub fn compute(x: i32, y: i32) -> i32 {');
  assert.ok(stop(sb, 's'));
  assert.equal(stop(sb, 's'), null, 'already reported');
  assert.deepEqual(stopRecords(sb).map((r) => [r.action, r.adopted]),
    [['stop_check', undefined], ['stop_followup', false]]);
});

test('followUpOf: a listed file edited after the report is adoption; earlier or unlisted is not', () => {
  const pending = { at: 9000, files: ['src/b.rs', 'src/c.rs'] };
  const mtimes = (m) => (f) => (f in m ? m[f] : 1000);
  assert.deepEqual(followUpOf(pending, [{ ts: 9500, file: 'src/b.rs' }], mtimes({})),
    { adopted: true, listed: 2, edited: 1 });
  assert.deepEqual(followUpOf(pending, [{ ts: 8000, file: 'src/b.rs' }, { ts: 9500, file: 'src/z.rs' }], mtimes({})),
    { adopted: false, listed: 2, edited: 0 }, 'an edit before the report, or of a file it did not list');
  assert.deepEqual(followUpOf(pending, [], mtimes({ 'src/c.rs': 9000 })),
    { adopted: true, listed: 2, edited: 1 }, 'an mtime at the report counts (Write, sed -i)');
});

test('report symbols/files: newly reported symbols only, every untouched caller file, none touched this turn', () => {
  const refs = [{ file: 'src/b.rs', line: 5, name: 'x' }, { file: 'src/c.rs', line: 2, name: 'y' },
    { file: 'src/d.rs', line: 9, name: 'z' }];
  const w = world({ refs, mtimes: { 'src/d.rs': 6000 } }); // d.rs changed inside this turn
  const r = computeStopReport({ edits: EDIT_A, state: { lastStopAt: null, reported: ['src/q.rs#other'] }, now: 9000, ...w });
  assert.equal(r.lines.length, 1);
  assert.equal(r.symbols, 1, 'a symbol reported by an earlier Stop is not counted again');
  assert.deepEqual(r.files, ['src/b.rs', 'src/c.rs'], 'one entry per caller file; d.rs was touched this turn');
});

test('reportRecords: every shown report is recorded; only one naming caller files leaves a follow-up', () => {
  assert.deepEqual(reportRecords({ lines: [], state: { lastStopAt: 9 } }), { check: null, pending: null });
  const onlyUnchecked = reportRecords({ lines: ['  1 more changed, callers not checked: g() in a.rs'],
    state: { lastStopAt: 9 }, symbols: 0, files: [] });
  assert.deepEqual(onlyUnchecked, { check: { hook: 'stop', action: 'stop_check', symbols: 0, callers: 0 }, pending: null });
  const many = Array.from({ length: MAX_FOLLOWUP_FILES + 6 }, (_, i) => `src/m${i}.rs`);
  const r = reportRecords({ lines: ['x'], state: { lastStopAt: 9 }, symbols: 2, files: many });
  assert.deepEqual(r.check, { hook: 'stop', action: 'stop_check', symbols: 2, callers: MAX_FOLLOWUP_FILES + 6 });
  assert.equal(r.pending.at, 9);
  assert.equal(r.pending.files.length, MAX_FOLLOWUP_FILES, 'the follow-up keeps a bounded list');
});

test('e2e: adoption is read from the edit log alone and from the mtime alone', { skip: e2eSkip }, (t) => {
  const past = new Date(Date.now() - 3600 * 1000);
  // Log only: an Edit is logged, then the file's mtime is put back before the report.
  const a = sandboxRepo(t);
  edit(a, 's', 'src/a.rs', 'pub fn compute(x: i32) -> i32 {', 'pub fn compute(x: i32, y: i32) -> i32 {');
  assert.ok(stop(a, 's'));
  edit(a, 's', 'src/b.rs', '    compute(v)', '    compute(v, 1)');
  fs.utimesSync(path.join(a.repo, 'src/b.rs'), past, past);
  stop(a, 's', { stop_hook_active: true });
  assert.deepEqual(stopRecords(a).map((r) => [r.action, r.adopted]), [['stop_check', undefined], ['stop_followup', true]]);
  // Mtime only: the file is written with no Edit (Write, sed -i).
  const b = sandboxRepo(t);
  edit(b, 's', 'src/a.rs', 'pub fn compute(x: i32) -> i32 {', 'pub fn compute(x: i32, y: i32) -> i32 {');
  assert.ok(stop(b, 's'));
  const bf = path.join(b.repo, 'src/b.rs');
  fs.writeFileSync(bf, fs.readFileSync(bf, 'utf8').replace('compute(v)', 'compute(v, 1)'));
  stop(b, 's', { stop_hook_active: true });
  assert.deepEqual(stopRecords(b).map((r) => [r.action, r.adopted]), [['stop_check', undefined], ['stop_followup', true]]);
});

test('e2e: caller also edited in the same turn → silent', { skip: e2eSkip }, (t) => {
  const sb = sandboxRepo(t);
  edit(sb, 's', 'src/a.rs', 'pub fn compute(x: i32) -> i32 {', 'pub fn compute(x: i32, y: i32) -> i32 {');
  edit(sb, 's', 'src/b.rs', '    compute(v)', '    compute(v, 1)');
  assert.equal(stop(sb, 's'), null);
});

test('e2e: body-only change → silent; not a git repo → silent; state stays in TMPDIR', { skip: e2eSkip }, (t) => {
  const sb = sandboxRepo(t);
  // old_string carries the header, as a real body edit usually does — that is
  // what lets pre-edit-guide name the symbol.
  edit(sb, 's', 'src/a.rs', 'pub fn compute(x: i32) -> i32 {\n    x + 1', 'pub fn compute(x: i32) -> i32 {\n    x + 2');
  assert.equal(stop(sb, 's'), null);

  const sb2 = sandboxRepo(t);
  fs.rmSync(path.join(sb2.repo, '.git'), { recursive: true, force: true });
  edit(sb2, 's', 'src/a.rs', 'pub fn compute(x: i32) -> i32 {', 'pub fn compute(x: i32, y: i32) -> i32 {');
  assert.equal(stop(sb2, 's'), null);

  // Both files this feature writes live in the redirected cgTmpDir, nowhere else.
  const cg = path.join(sb.tmp, 'code-graph-mcp');
  const names = fs.readdirSync(cg).filter((n) => /^\.cg-(edits|stop)-/.test(n)).sort();
  assert.equal(names.length, 2, names.join(', '));
  assert.match(names[0], /^\.cg-edits-[0-9a-f]{12}-s\.jsonl$/);
  assert.match(names[1], /^\.cg-stop-[0-9a-f]{12}-s\.json$/);
});

// --- Review repairs (P1 #3 review, 2026-09-28) ------------------------------

// M2: what the text heuristic concludes for one old → new copy of a file.
// 'changed' = a definition header of `symbol` differs; 'same' = found, none
// differs; 'silent' = no verdict (unsupported language or no definition).
function verdict(oldText, newText, symbol, ext) {
  const a = extractSignatures(oldText, symbol, ext);
  const b = extractSignatures(newText, symbol, ext);
  if (!a || !b || a.length === 0 || b.length === 0) return 'silent';
  return signatureChanged(a, b) ? 'changed' : 'same';
}

const SHAPES = [
  // The reviewer's false positives: a body-only change read as a signature change.
  ['py: `with f(...)` call is not a definition', '.py', 'load',
    'def load(p):\n    with load(1) as f:\n        pass\n',
    'def load(p):\n    with load(1, 2) as f:\n        pass\n', 'same'],
  ['py: `elif f(...)` call is not a definition', '.py', 'ok',
    'def ok(n):\n    if n:\n        pass\n    elif ok(1):\n        pass\n',
    'def ok(n):\n    if n:\n        pass\n    elif ok(2):\n        pass\n', 'same'],
  ['rust: a new same-named impl adds a definition, changes none', '.rs', 'from',
    'impl From<A> for X {\n    fn from(a: A) -> X { X }\n}\n',
    'impl From<A> for X {\n    fn from(a: A) -> X { X }\n}\nimpl From<B> for X {\n    fn from(b: B) -> X { X }\n}\n', 'same'],
  ['kotlin: expression-bodied fun, body edited', '.kt', 'area',
    'fun area(r: Double) =\n    r * r * 3.14\n\nfun other() {\n}\n',
    'fun area(r: Double) =\n    r * r * 3.1416\n\nfun other() {\n}\n', 'same'],
  ['kotlin: one-line expression body edited', '.kt', 'area',
    'fun area(r: Double) = r * r * 3.14\n', 'fun area(r: Double) = r * r * 3.1416\n', 'same'],
  ['scala: expression-bodied def, body edited', '.scala', 'f',
    'def f(): Int = 1\n\ndef g() = {\n}\n',
    'def f(): Int = 2\n\ndef g() = {\n}\n', 'same'],
  // The reviewer's false negative.
  ['ts: generic bound holding `{` — a new parameter is seen', '.ts', 'pick',
    'function pick<T extends {id:number}>(a: T) {\n  return a;\n}\n',
    'function pick<T extends {id:number}>(a: T, b: number) {\n  return a;\n}\n', 'changed'],
  // Real changes in the same languages stay visible.
  ['py: parameter added', '.py', 'load', 'def load(p):\n    pass\n', 'def load(p, q):\n    pass\n', 'changed'],
  ['kotlin: parameter added to an expression body', '.kt', 'area',
    'fun area(r: Double) =\n    r * r\n', 'fun area(r: Double, k: Double) =\n    r * r * k\n', 'changed'],
  ['kotlin: bodyless interface fun, the NEXT fun changes', '.kt', 'area',
    'interface S {\n    fun area(r: Double): Double\n}\nfun g() {\n  1\n}\n',
    'interface S {\n    fun area(r: Double): Double\n}\nfun g(x: Int) {\n  1\n}\n', 'same'],
  ['go: bodyless declaration, the NEXT func changes', '.go', 'Compute',
    'func Compute(x int) int\n\nfunc other() {\n}\n', 'func Compute(x int) int\n\nfunc other(y int) {\n}\n', 'same'],
  ['c#: expression-bodied member, body edited', '.cs', 'F',
    '  public int F(int x) => x + 1;\n', '  public int F(int x) => x + 2;\n', 'same'],
  ['header not ended within the character budget → silent', '.rs', 'compute',
    'fn compute(' + 'a: u8, '.repeat(120) + ') {}\n', 'fn compute(' + 'a: u8, '.repeat(121) + ') {}\n', 'silent'],
  ['scala: parameter added', '.scala', 'f', 'def f(): Int = 1\n', 'def f(x: Int): Int = x\n', 'changed'],
  ['rust: one impl of two changes its parameter', '.rs', 'from',
    'impl From<A> for X {\n    fn from(a: A) -> X { X }\n}\nimpl From<B> for X {\n    fn from(b: B) -> X { X }\n}\n',
    'impl From<A> for X {\n    fn from(a: &A) -> X { X }\n}\nimpl From<B> for X {\n    fn from(b: B) -> X { X }\n}\n', 'changed'],
  ['rust: a `match` guard calling the fn is not a definition', '.rs', 'compute',
    'fn compute(x: i32) -> i32 {\n    match x {\n        a if compute(a) > 0 => 1,\n        _ => 0,\n    }\n}\n',
    'fn compute(x: i32) -> i32 {\n    match x {\n        a if compute(a - 1) > 0 => 1,\n        _ => 0,\n    }\n}\n', 'same'],
  ['ruby: body-only change (newline ends the header)', '.rb', 'run',
    'def run(a, b)\n  a + b\nend\n', 'def run(a, b)\n  a - b\nend\n', 'same'],
  ['ruby: endless def, body edited', '.rb', 'run', 'def run(a) = a + 1\n', 'def run(a) = a + 2\n', 'same'],
  ['ruby: parameter added', '.rb', 'run', 'def run(a)\n  a\nend\n', 'def run(a, b)\n  a\nend\n', 'changed'],
  ['lua: one-line function, body edited', '.lua', 'run',
    'local function run(a) return a + 1 end\n', 'local function run(a) return a + 2 end\n', 'same'],
  ['c++: constructor initializer list edited', '.cpp', 'Foo',
    'explicit Foo(int x) : a(x), b{x} {\n}\n', 'explicit Foo(int x) : a(x + 1), b{x} {\n}\n', 'same'],
  ['c++: a label before a call is not a type', '.cpp', 'compute',
    'int compute(int x) {\nretry: compute(1);\n}\n', 'int compute(int x) {\nretry: compute(2);\n}\n', 'same'],
  ['c++: parameter added', '.cpp', 'compute', 'int compute(int x) {\n}\n', 'int compute(int x, int y) {\n}\n', 'changed'],
  ['java: parameter type changed', '.java', 'compute',
    '  public int compute(int x) {\n  }\n', '  public int compute(long x) {\n  }\n', 'changed'],
  ['go: parameter added', '.go', 'Compute',
    'func (s *S) Compute(x int) (int, error) {\n}\n', 'func (s *S) Compute(x int, y int) (int, error) {\n}\n', 'changed'],
  ['js: arrow binding parameter added', '.js', 'build',
    'const build = (x) => x;\n', 'const build = (x, y) => x + y;\n', 'changed'],
  // No exact reading for these: say nothing rather than guess.
  ['elixir: unsupported → silent', '.ex', 'run',
    'def run(a), do: a + 1\n', 'def run(a), do: a + 2\n', 'silent'],
  ['unknown extension → silent', '.txt', 'run', 'def run(a):\n', 'def run(a, b):\n', 'silent'],
];

for (const [name, ext, symbol, before, after, want] of SHAPES) {
  test(`M2 signature shapes: ${name}`, () => {
    assert.equal(verdict(before, after, symbol, ext), want);
  });
}

// e2e H1: a caller fixed in the same turn as the signature change must not
// come back when a LATER turn only edits the function body.
test('e2e H1: callers fixed in turn 1, body-only edit in turn 2 → silent both turns', { skip: e2eSkip }, (t) => {
  const sb = sandboxRepo(t);
  edit(sb, 's', 'src/a.rs', 'pub fn compute(x: i32) -> i32 {', 'pub fn compute(x: i32, y: i32) -> i32 {');
  edit(sb, 's', 'src/b.rs', '    compute(v)', '    compute(v, 1)');
  assert.equal(stop(sb, 's'), null, 'turn 1: the only caller was updated');
  const header = 'pub fn compute(x: i32, y: i32) -> i32 {\n';
  edit(sb, 's', 'src/a.rs', header + '    x + 1', header + '    x + y + 1');
  const t2 = stop(sb, 's');
  assert.equal(t2, null, t2 && t2.hookSpecificOutput.additionalContext);
});

// e2e M1: eight body-only edits before the one real change must not use up
// the per-Stop symbol budget.
test('e2e M1: the 9th edited symbol, the only signature change, is reported', { skip: e2eSkip }, (t) => {
  const fns = [];
  const callers = [];
  for (let i = 0; i < 9; i++) {
    fns.push(`pub fn f${i}xx(x: i32) -> i32 {\n    x + ${i}\n}\n`);
    callers.push(`use crate::a::f${i}xx;\npub fn c${i}() -> i32 {\n    f${i}xx(1)\n}\n`);
  }
  const sb = sandboxRepo(t, { 'src/lib.rs': 'pub mod a;\npub mod b;\n', 'src/a.rs': fns.join('\n'), 'src/b.rs': callers.join('') });
  for (let i = 0; i < 8; i++) {
    edit(sb, 's', 'src/a.rs', `pub fn f${i}xx(x: i32) -> i32 {\n    x + ${i}`, `pub fn f${i}xx(x: i32) -> i32 {\n    x + ${i} + 0`);
  }
  edit(sb, 's', 'src/a.rs', 'pub fn f8xx(x: i32) -> i32 {', 'pub fn f8xx(x: i32, y: i32) -> i32 {');
  const out = stop(sb, 's');
  assert.ok(out, 'expected the f8xx report');
  const ctx = out.hookSpecificOutput.additionalContext;
  assert.match(ctx, /f8xx\(\) in src\/a\.rs: src\/b\.rs:\d+ \(c8\)/);
  assert.doesNotMatch(ctx, /f[0-7]xx\(\)/, 'body-only edits are not reported');
});

// e2e M3: repo-derived tokens in the injected text are shell-quoted — the
// edited file in the suggested command and every caller file.
test('e2e M3: hostile file names are shell-quoted in the injected text', { skip: e2eSkip }, (t) => {
  const edited = 'src/a$(touch PWNED).rs';
  const files = { 'src/lib.rs': 'pub mod a;\n', [edited]: HEAD_A };
  const caller = (name, arg) => `use crate::a::compute;\npub fn ${name}() -> i32 {\n    compute(${arg})\n}\n`;
  for (let i = 0; i < 8; i++) files[`src/m${i}.rs`] = caller(`cm${i}`, i);
  files['src/m$(id).rs'] = caller('cmx', 9);
  const sb = sandboxRepo(t, files);
  edit(sb, 's', edited, 'pub fn compute(x: i32) -> i32 {', 'pub fn compute(x: i32, y: i32) -> i32 {');
  const out = stop(sb, 's');
  assert.ok(out, 'expected a report');
  const ctx = out.hookSpecificOutput.additionalContext;
  assert.ok(ctx.includes("code-graph-mcp refs compute --file 'src/a$(touch PWNED).rs'"), ctx);
  assert.ok(ctx.includes("'src/m$(id).rs':3"), ctx);
  // No `$(` outside single quotes anywhere in the text.
  assert.doesNotMatch(ctx.replace(/'[^']*'/g, "''"), /\$\(/, ctx);
  assert.equal(fs.existsSync(path.join(sb.repo, 'PWNED')), false);
});

// Review L7 survivors M08 and M15: two documented silences no test pinned.
test('e2e: CODE_GRAPH_QUIET_HOOKS=1 silences Stop; a session with no edit log writes nothing', { skip: e2eSkip }, (t) => {
  const sb = sandboxRepo(t);
  edit(sb, 's', 'src/a.rs', 'pub fn compute(x: i32) -> i32 {', 'pub fn compute(x: i32, y: i32) -> i32 {');
  const quiet = hook({ ...sb, env: { ...sb.env, CODE_GRAPH_QUIET_HOOKS: '1' } }, 'stop-impact.js',
    { session_id: 's', hook_event_name: 'Stop', cwd: sb.repo, stop_hook_active: false });
  assert.equal(quiet.status, 0, quiet.stderr);
  assert.equal(quiet.stdout, '');
  const cg = path.join(sb.tmp, 'code-graph-mcp');
  const stateFiles = () => fs.readdirSync(cg).filter((n) => n.startsWith('.cg-stop-'));
  assert.deepEqual(stateFiles(), [], 'a quiet Stop does not advance the state either');

  assert.equal(stop(sb, 'never-edited'), null);
  assert.deepEqual(stateFiles(), [], 'no edit log → no state file');
  assert.ok(stop(sb, 's'), 'the logged session is still reported once unquieted');
});
