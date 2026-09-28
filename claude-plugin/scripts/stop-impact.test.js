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
  extractSignatures, signatureChanged, findCallSiteLine, computeStopReport, formatStopContext,
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

function world({ work = SIG_A, mtimes = {}, refs } = {}) {
  return {
    headText: (f) => (f === 'src/a.rs' ? HEAD_A : null),
    workText: (f) => (f === 'src/a.rs' ? work : null),
    callers: () => refs || [{ file: 'src/b.rs', line: 5, name: 'caller_b' }],
    mtimeMs: (f) => (f in mtimes ? mtimes[f] : 1000),
  };
}
const EMPTY = { lastStopAt: null, reported: [] };
const EDIT_A = [{ ts: 5000, file: 'src/a.rs', symbol: null }, { ts: 5001, file: 'src/a.rs', symbol: 'compute' }];

test('stop: signature change in a.rs with an untouched caller in b.rs → one b.rs:line line', () => {
  const r = computeStopReport({ edits: EDIT_A, state: EMPTY, now: 9000, ...world() });
  assert.deepEqual(r.lines, ['  compute() in src/a.rs: src/b.rs:5 (caller_b)']);
  assert.deepEqual(r.state, { lastStopAt: 9000, reported: ['src/a.rs#compute'] });
  const text = formatStopContext(r.lines);
  assert.match(text, /src\/b\.rs:5/);
  assert.doesNotMatch(text, /\b(?:must|should|you)\b/i, 'facts only');
});

test('stop: caller file also edited this turn → silent (logged edit, or mtime inside the turn)', () => {
  const logged = [...EDIT_A, { ts: 5002, file: 'src/b.rs', symbol: null }];
  assert.deepEqual(computeStopReport({ edits: logged, state: EMPTY, now: 9000, ...world() }).lines, []);
  // Not logged (Write, `sed -i`, a formatter) but modified after the turn began.
  const viaMtime = world({ mtimes: { 'src/b.rs': 6000 } });
  assert.deepEqual(computeStopReport({ edits: EDIT_A, state: EMPTY, now: 9000, ...viaMtime }).lines, []);
});

test('stop: second Stop in the same session → silent, even after a new edit of the symbol', () => {
  const first = computeStopReport({ edits: EDIT_A, state: EMPTY, now: 9000, ...world() });
  assert.equal(first.lines.length, 1);
  const again = computeStopReport({ edits: EDIT_A, state: first.state, now: 9500, ...world() });
  assert.deepEqual(again.lines, [], 'no edits since the last Stop');
  const reEdit = [...EDIT_A, { ts: 9600, file: 'src/a.rs', symbol: 'compute' }];
  assert.deepEqual(computeStopReport({ edits: reEdit, state: first.state, now: 9900, ...world() }).lines, [],
    'once per symbol per session');
});

test('stop: body-only change → silent; new or vanished definition → silent', () => {
  assert.deepEqual(computeStopReport({ edits: EDIT_A, state: EMPTY, now: 9000, ...world({ work: BODY_A }) }).lines, []);
  assert.deepEqual(computeStopReport({ edits: EDIT_A, state: EMPTY, now: 9000, ...world({ work: 'fn other() {}\n' }) }).lines, []);
  const newFile = { ...world(), headText: () => null };
  assert.deepEqual(computeStopReport({ edits: EDIT_A, state: EMPTY, now: 9000, ...newFile }).lines, []);
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

// --- Spawned end to end: real git, real binary, sandbox HOME/TMPDIR ---------

function realBinary() {
  try { return require('./find-binary').findBinary(); } catch { return null; }
}
const BIN = process.platform === 'win32' ? null : realBinary();
const e2eSkip = !BIN && 'needs a code-graph-mcp binary and a POSIX shell';

function sandboxRepo(t) {
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
  fs.mkdirSync(path.join(repo, 'src'), { recursive: true });
  fs.writeFileSync(path.join(repo, 'src', 'lib.rs'), 'pub mod a;\npub mod b;\n');
  fs.writeFileSync(path.join(repo, 'src', 'a.rs'), HEAD_A);
  fs.writeFileSync(path.join(repo, 'src', 'b.rs'),
    'use crate::a::compute;\n\npub fn caller_b() -> i32 {\n    let v = 2;\n    compute(v)\n}\n');
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
  for (const f of ['src/lib.rs', 'src/a.rs', 'src/b.rs']) fs.utimesSync(path.join(repo, f), past, past);
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
  // A different session keeps its own ledger. Its edit is body-only, but the
  // comparison is working tree vs HEAD, where the signature still differs and
  // b.rs is still untouched — so sess-2 is told once too.
  const header = 'pub fn compute(x: i32, y: i32) -> i32 {\n';
  edit(sb, 'sess-2', 'src/a.rs', header + '    x + 1', header + '    x + 1 + 0');
  const other = stop(sb, 'sess-2');
  assert.ok(other, 'sess-2 has not been told yet');
  assert.match(other.hookSpecificOutput.additionalContext, /src\/b\.rs:5/);
  assert.equal(stop(sb, 'sess-2'), null);
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
