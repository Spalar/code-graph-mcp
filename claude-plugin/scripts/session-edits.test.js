'use strict';
const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const os = require('node:os');
const path = require('node:path');
const { spawnSync } = require('node:child_process');

// These tests redirect HOME for their children; CLAUDE_CONFIG_DIR would win
// over it (claude-config.js) and point them at the real config. Dropped at
// module load so every spawn below inherits the sandbox, not the live dir.
delete process.env.CLAUDE_CONFIG_DIR;

// session-edits.js resolves cgTmpDir() from os.tmpdir() at require time, so
// every assertion that touches disk runs in a child with TMPDIR redirected —
// never against the machine-wide dir the live hooks share.
function sandbox(t) {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), 'cg-sessedits-'));
  t.after(() => fs.rmSync(root, { recursive: true, force: true }));
  const tmp = path.join(root, 'tmp');
  const home = path.join(root, 'home');
  fs.mkdirSync(tmp);
  fs.mkdirSync(home);
  return { root, tmp, home, env: { ...process.env, HOME: home, USERPROFILE: home, TMPDIR: tmp, TMP: tmp, TEMP: tmp } };
}

function inChild(sb, body) {
  const r = spawnSync(process.execPath, ['-e', `
    const se = require(${JSON.stringify(path.join(__dirname, 'session-edits.js'))});
    const out = (() => { ${body} })();
    process.stdout.write(JSON.stringify(out));
  `], { env: sb.env, encoding: 'utf8' });
  assert.equal(r.status, 0, r.stderr);
  return JSON.parse(r.stdout);
}

test('sessionKey: a UUID is used as is; anything else is hashed, never a path', () => {
  const { sessionKey } = require('./session-edits');
  assert.equal(sessionKey('15042022-d26d-4f59-970d-980e70ac34b0'), '15042022-d26d-4f59-970d-980e70ac34b0');
  const k = sessionKey('../../etc/passwd');
  assert.match(k, /^[0-9a-f]{16}$/);
  assert.equal(sessionKey(''), null);
  assert.equal(sessionKey(undefined), null);
});

test('recordEdit/readEdits round trip; torn lines and foreign shapes are skipped', (t) => {
  const sb = sandbox(t);
  const got = inChild(sb, `
    se.recordEdit('/p', 'S1', { file: 'src/a.rs' }, 10);
    se.recordEdit('/p', 'S1', { file: 'src/a.rs', symbol: 'compute' }, 11);
    se.recordEdit('/p', 'S2', { file: 'src/z.rs' }, 12);
    require('fs').appendFileSync(se.editsPath('/p', 'S1'), '{"ts":1,"fi\\n["x"]\\n');
    return { s1: se.readEdits('/p', 'S1'), s2: se.readEdits('/p', 'S2'), none: se.readEdits('/p', 'S3'),
             noSession: se.recordEdit('/p', '', { file: 'a' }) };
  `);
  // '/p/src/a.rs' does not exist: a symbol record carries `sigs: null`.
  assert.deepEqual(got.s1, [
    { ts: 10, file: 'src/a.rs', symbol: null, sigs: null },
    { ts: 11, file: 'src/a.rs', symbol: 'compute', sigs: null },
  ]);
  assert.deepEqual(got.s2, [{ ts: 12, file: 'src/z.rs', symbol: null, sigs: null }]);
  assert.deepEqual(got.none, []);
  assert.equal(got.noSession, false, 'no session id → nothing written');
  // Everything landed in the redirected cgTmpDir.
  const names = fs.readdirSync(path.join(sb.tmp, 'code-graph-mcp')).sort();
  assert.equal(names.length, 2, names.join(','));
  assert.ok(names.every((n) => /^\.cg-edits-[0-9a-f]{12}-S[12]\.jsonl$/.test(n)), names.join(','));
});

test('recordEdit: a symbol record carries the signatures the file has at record time', (t) => {
  const sb = sandbox(t);
  const project = path.join(sb.root, 'project');
  fs.mkdirSync(path.join(project, 'src'), { recursive: true });
  fs.writeFileSync(path.join(project, 'src', 'a.rs'), 'pub fn compute(x: i32) -> i32 {\n    x\n}\n');
  fs.writeFileSync(path.join(project, 'src', 'a.ex'), 'def compute(x), do: x\n');
  const many = Array.from({ length: 33 }, (_, i) => `fn compute(x: [u8; ${i}]) {}\n`).join('');
  fs.writeFileSync(path.join(project, 'src', 'many.rs'), many);
  const got = inChild(sb, `
    const p = ${JSON.stringify(project)};
    se.recordEdit(p, 'S', { file: 'src/a.rs', symbol: 'compute' }, 1);
    // The edit lands after the record: the record keeps the old header.
    require('fs').writeFileSync(require('path').join(p, 'src', 'a.rs'), 'pub fn compute(x: i64) {}\\n');
    se.recordEdit(p, 'S', { file: 'src/a.ex', symbol: 'compute' }, 2);
    se.recordEdit(p, 'S', { file: 'src/many.rs', symbol: 'compute' }, 3);
    return se.readEdits(p, 'S').map((r) => r.sigs);
  `);
  assert.deepEqual(got, [
    ['pubfncompute(x:i32)->i32'],
    null,  // no exact reading for Elixir
    null,  // more same-named definitions than the cap
  ]);
});

test('stop state: missing/corrupt reads as empty; write is atomic and leaves no temp file', (t) => {
  const sb = sandbox(t);
  const got = inChild(sb, `
    const empty = se.readStopState('/p', 'S');
    require('fs').writeFileSync(se.stopStatePath('/p', 'S'), '{nope');
    const corrupt = se.readStopState('/p', 'S');
    se.writeStopState('/p', 'S', { lastStopAt: 7, reported: ['a#b'] });
    return { empty, corrupt, after: se.readStopState('/p', 'S') };
  `);
  assert.deepEqual(got.empty, { lastStopAt: null, reported: [] });
  assert.deepEqual(got.corrupt, { lastStopAt: null, reported: [] });
  assert.deepEqual(got.after, { lastStopAt: 7, reported: ['a#b'] });
  const names = fs.readdirSync(path.join(sb.tmp, 'code-graph-mcp'));
  assert.deepEqual(names.filter((n) => n.endsWith('.tmp')), []);
});

test('stop state: a pending follow-up survives a round trip; a malformed one is dropped', (t) => {
  const sb = sandbox(t);
  const got = inChild(sb, `
    const read = (pending) => {
      se.writeStopState('/p', 'S', { lastStopAt: 7, reported: [], pending });
      return se.readStopState('/p', 'S');
    };
    return [
      read({ at: 9, files: ['src/b.rs', 3, 'src/c.rs'] }),
      read({ at: 'x', files: ['src/b.rs'] }),
      read({ at: 9, files: 'src/b.rs' }),
    ];
  `);
  assert.deepEqual(got[0], { lastStopAt: 7, reported: [], pending: { at: 9, files: ['src/b.rs', 'src/c.rs'] } });
  assert.deepEqual(got[1], { lastStopAt: 7, reported: [] }, 'a non-numeric time is no follow-up');
  assert.deepEqual(got[2], { lastStopAt: 7, reported: [] }, 'a non-list of files is no follow-up');
});

// The writer side, through the real hook: pre-edit-guide logs every Edit
// (file-only first, then with the symbol it extracted), before its cooldown
// and whether or not the impact query answers. The fake binary fails every
// call, so this proves the log does not depend on the impact push.
test('pre-edit-guide logs the edited file and the extracted symbol per session', { skip: process.platform === 'win32' && 'POSIX shell fixture' }, (t) => {
  const sb = sandbox(t);
  const cache = path.join(sb.home, '.cache', 'code-graph');
  fs.mkdirSync(path.join(cache, 'bin'), { recursive: true });
  fs.writeFileSync(path.join(cache, 'install-manifest.json'), '{"version":"9.9.9","config":{}}');
  const fake = path.join(cache, 'bin', 'code-graph-mcp');
  fs.writeFileSync(fake, '#!/bin/sh\nexit 1\n', { mode: 0o755 });
  fs.writeFileSync(path.join(cache, 'binary-path'), fake);
  const project = path.join(sb.root, 'project');
  fs.mkdirSync(path.join(project, '.code-graph'), { recursive: true });
  fs.writeFileSync(path.join(project, '.code-graph', 'index.db'), '');

  const runEdit = (session, oldString) => spawnSync(process.execPath, [path.join(__dirname, 'pre-edit-guide.js')], {
    input: JSON.stringify({
      session_id: session, tool_name: 'Edit',
      tool_input: { file_path: path.join(project, 'src', 'a.rs'), old_string: oldString, new_string: 'x' },
    }),
    cwd: project, env: sb.env, encoding: 'utf8', timeout: 15000,
  });
  for (const [session, old] of [['S1', 'pub fn compute(x: i32) -> i32 {'], ['S1', 'short'], ['S2', 'fn helper_one(a: u8) {']]) {
    const r = runEdit(session, old);
    assert.equal(r.status, 0, r.stderr);
  }
  const read = (s) => inChild({ ...sb, env: sb.env }, `return se.readEdits(${JSON.stringify(project)}, '${s}');`)
    .map(({ file, symbol }) => ({ file, symbol }));
  assert.deepEqual(read('S1'), [
    { file: 'src/a.rs', symbol: null },
    { file: 'src/a.rs', symbol: 'compute' },
    { file: 'src/a.rs', symbol: null },          // 'short' exits before extraction, file still logged
  ]);
  assert.deepEqual(read('S2'), [
    { file: 'src/a.rs', symbol: null },
    { file: 'src/a.rs', symbol: 'helper_one' },
  ]);
});

// Pre-tag review H2: the baseline capture runs inside pre-edit-guide, whose
// registered timeout is 4 s. On a 1.5 MB one-line bundle it took 9 s at
// 1ffc45c; a definition on a line over 2,000 characters now records no
// baseline (null), and the hook is done well inside its budget.
test('pre-edit-guide on a 1.5 MB one-line bundle records no baseline and stays inside its timeout', { skip: process.platform === 'win32' && 'POSIX shell fixture' }, (t) => {
  const sb = sandbox(t);
  const cache = path.join(sb.home, '.cache', 'code-graph');
  fs.mkdirSync(path.join(cache, 'bin'), { recursive: true });
  fs.writeFileSync(path.join(cache, 'install-manifest.json'), '{"version":"9.9.9","config":{}}');
  const fake = path.join(cache, 'bin', 'code-graph-mcp');
  fs.writeFileSync(fake, '#!/bin/sh\nexit 1\n', { mode: 0o755 });
  fs.writeFileSync(path.join(cache, 'binary-path'), fake);
  const project = path.join(sb.root, 'project');
  fs.mkdirSync(path.join(project, '.code-graph'), { recursive: true });
  fs.mkdirSync(path.join(project, 'dist'), { recursive: true });
  fs.writeFileSync(path.join(project, '.code-graph', 'index.db'), '');
  const unit = 'function abc(t){return t+1}var a=abc(1);';
  const bundle = unit.repeat(Math.ceil((1536 * 1024) / unit.length));
  assert.ok(bundle.length > 1.5 * 1024 * 1024 && !bundle.includes('\n'));
  fs.writeFileSync(path.join(project, 'dist', 'bundle.js'), bundle);

  const t0 = Date.now();
  const r = spawnSync(process.execPath, [path.join(__dirname, 'pre-edit-guide.js')], {
    input: JSON.stringify({
      session_id: 'S1', tool_name: 'Edit',
      tool_input: { file_path: path.join(project, 'dist', 'bundle.js'), old_string: 'function abc(t){return t+1}', new_string: 'x' },
    }),
    cwd: project, env: sb.env, encoding: 'utf8', timeout: 30000,
  });
  const ms = Date.now() - t0;
  assert.equal(r.status, 0, r.stderr);
  assert.ok(ms < 3000, `pre-edit-guide took ${ms} ms on the bundle (timeout 4000 ms)`);
  const recs = inChild(sb, `return se.readEdits(${JSON.stringify(project)}, 'S1');`)
    .map(({ file, symbol, sigs }) => ({ file, symbol, sigs }));
  assert.deepEqual(recs, [
    { file: 'dist/bundle.js', symbol: null, sigs: null },
    { file: 'dist/bundle.js', symbol: 'abc', sigs: null },
  ]);
});
