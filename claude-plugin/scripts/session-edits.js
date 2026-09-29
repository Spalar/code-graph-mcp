#!/usr/bin/env node
'use strict';
// Per-session edit log shared by the PreToolUse hooks (writers: pre-edit-guide
// for Edit/Write, pre-grep-guide for `sed -i`/`perl -pi`) and the
// Stop hook (reader) — P1 #3, Q1.
//
// Two files per (project, session), both in the shared cgTmpDir() so the
// SessionStart prune (24 h, mtime) and `uninstall`'s wholesale removal of that
// dir cover them without a new cleanup path:
//
//   .cg-edits-<cwdHash>-<sid>.jsonl  one line per Edit call: {ts, file, symbol[, sigs]}
//   .cg-stop-<cwdHash>-<sid>.json    written by the Stop hook only:
//                                    {lastStopAt, reported: ["file#symbol", …],
//                                     pending?: {at, files: […]}} — the last
//                                    report's caller files, until its follow-up
//
// The edit log is APPEND-only: parallel Edit calls each run their own hook
// process, and a read-modify-write JSON file would drop one of two concurrent
// records. A single small `appendFileSync` is one O_APPEND write. The Stop
// file has exactly one writer (Claude Code runs one Stop per turn), so plain
// write-then-rename is enough there.
const fs = require('fs');
const path = require('path');
const crypto = require('crypto');
const { cgTmpDir, cwdHash } = require('./tmp-dir');

// Claude Code's session_id is a UUID. Anything else is hashed rather than
// trusted as a path segment: it is stdin from the harness, and a `../` in it
// must not be able to name a file outside cgTmpDir().
function sessionKey(sessionId) {
  const s = String(sessionId || '');
  if (!s) return null;
  if (/^[A-Za-z0-9-]{1,64}$/.test(s)) return s;
  return crypto.createHash('sha1').update(s).digest('hex').slice(0, 16);
}

function editsPath(root, sessionId) {
  const key = sessionKey(sessionId);
  return key ? path.join(cgTmpDir(), `.cg-edits-${cwdHash(root)}-${key}.jsonl`) : null;
}

function stopStatePath(root, sessionId) {
  const key = sessionKey(sessionId);
  return key ? path.join(cgTmpDir(), `.cg-stop-${cwdHash(root)}-${key}.json`) : null;
}

// Baseline capture limits: a larger file, or more same-named definitions than
// this, records no baseline — and the Stop hook then says nothing about it.
const MAX_BASELINE_FILE_BYTES = 2 * 1024 * 1024;
const MAX_BASELINE_SIGS = 32;

/**
 * The symbol's definition headers in the file as it is NOW — called from
 * PreToolUse, i.e. before the Edit lands. The Stop hook compares the working
 * tree with the turn's first such record, so "changed this turn" means what it
 * says (review H1). null = no exact reading (stop-impact.js LANGS, size, IO).
 */
function baselineSignatures(root, file, symbol) {
  try {
    const abs = path.join(root, file);
    if (fs.statSync(abs).size > MAX_BASELINE_FILE_BYTES) return null;
    const { extractSignatures } = require('./stop-impact');
    const sigs = extractSignatures(fs.readFileSync(abs, 'utf8'), symbol, path.extname(file).toLowerCase());
    return sigs && sigs.length <= MAX_BASELINE_SIGS ? sigs : null;
  } catch { return null; }
}

/**
 * Append one Edit to the session log. Best-effort: a failed write costs the
 * Stop check one record, never the Edit it rides on. A record with a symbol
 * also carries `sigs`, that symbol's signatures before this Edit.
 * @param {string} root project root
 * @param {string} sessionId
 * @param {{file: string, symbol?: string|null}} rec file is root-relative
 * @returns {boolean} written
 */
function recordEdit(root, sessionId, { file, symbol = null }, now = Date.now()) {
  const p = editsPath(root, sessionId);
  if (!p || !file) return false;
  const rec = { ts: now, file, symbol: symbol || null };
  if (symbol) rec.sigs = baselineSignatures(root, file, symbol);
  try {
    fs.appendFileSync(p, JSON.stringify(rec) + '\n');
    return true;
  } catch { return false; }
}

// Q1: an edit that names no definition (Write over an existing file, `sed -i`,
// `perl -pi`) baselines the file's definitions at once. Bounded so the hook
// stays inside its budget: a larger file is logged as a file only, and at
// most this many definitions — the first in the file — get a record.
const MAX_FILE_SCAN_BYTES = 256 * 1024;
const MAX_FILE_DEFINITIONS = 64;

/**
 * Log an edit that names no definition: one file record, then one baselined
 * record per definition of the file as it is now (before the edit). With
 * `newText` (a Write's content) only definitions whose header the new text
 * changes are recorded — the Stop hook would find the rest unchanged. Without
 * it (`sed -i`: the result is unknown until the command runs) every
 * definition is, and the Stop hook sorts them out. One append, best-effort.
 * @param {string} root project root
 * @param {string} sessionId
 * @param {string} file root-relative
 * @param {{newText?: string|null}} [opts]
 * @returns {number} records written
 */
function recordFileEdit(root, sessionId, file, { newText = null } = {}, now = Date.now()) {
  const p = editsPath(root, sessionId);
  if (!p || !file) return 0;
  const recs = [{ ts: now, file, symbol: null }];
  for (const [symbol, sigs] of fileBaselines(root, file, newText)) recs.push({ ts: now, file, symbol, sigs });
  try {
    fs.appendFileSync(p, recs.map((r) => JSON.stringify(r) + '\n').join(''));
    return recs.length;
  } catch { return 0; }
}

function fileBaselines(root, file, newText) {
  try {
    const abs = path.join(root, file);
    if (fs.statSync(abs).size > MAX_FILE_SCAN_BYTES) return [];
    const { allSignatures, extractSignatures, signatureChanged } = require('./stop-impact');
    const ext = path.extname(file).toLowerCase();
    const all = allSignatures(fs.readFileSync(abs, 'utf8'), ext);
    if (!all) return [];
    const out = [];
    for (const [symbol, sigs] of all) {
      if (out.length === MAX_FILE_DEFINITIONS) break;
      if (sigs.length > MAX_BASELINE_SIGS) continue;
      if (typeof newText === 'string') {
        const after = extractSignatures(newText, symbol, ext);
        if (!after || after.length === 0 || !signatureChanged(sigs, after, symbol)) continue;
      }
      out.push([symbol, sigs]);
    }
    return out;
  } catch { return []; }
}

/** Every well-formed record in the session log; a torn or foreign line is skipped. */
function readEdits(root, sessionId) {
  const p = editsPath(root, sessionId);
  if (!p) return [];
  let raw;
  try { raw = fs.readFileSync(p, 'utf8'); } catch { return []; }
  const out = [];
  for (const line of raw.split('\n')) {
    if (!line) continue;
    try {
      const r = JSON.parse(line);
      if (r && typeof r.ts === 'number' && typeof r.file === 'string') {
        const sigs = Array.isArray(r.sigs) && r.sigs.every((x) => typeof x === 'string') ? r.sigs : null;
        out.push({ ts: r.ts, file: r.file, symbol: typeof r.symbol === 'string' ? r.symbol : null, sigs });
      }
    } catch { /* torn line from a killed writer */ }
  }
  return out;
}

function readStopState(root, sessionId) {
  const p = stopStatePath(root, sessionId);
  const empty = { lastStopAt: null, reported: [] };
  if (!p) return empty;
  try {
    const s = JSON.parse(fs.readFileSync(p, 'utf8'));
    const out = {
      lastStopAt: typeof s.lastStopAt === 'number' ? s.lastStopAt : null,
      reported: Array.isArray(s.reported) ? s.reported.filter((x) => typeof x === 'string') : [],
    };
    const p2 = s.pending;
    if (p2 && typeof p2.at === 'number' && Array.isArray(p2.files)) {
      out.pending = { at: p2.at, files: p2.files.filter((x) => typeof x === 'string') };
    }
    return out;
  } catch { return empty; }
}

function writeStopState(root, sessionId, state) {
  const p = stopStatePath(root, sessionId);
  if (!p) return false;
  const tmp = `${p}.${process.pid}.tmp`;
  try {
    fs.writeFileSync(tmp, JSON.stringify(state));
    fs.renameSync(tmp, p);
    return true;
  } catch {
    try { fs.unlinkSync(tmp); } catch { /* never created */ }
    return false;
  }
}

module.exports = {
  sessionKey, editsPath, stopStatePath, recordEdit, recordFileEdit, readEdits, readStopState, writeStopState,
  MAX_FILE_DEFINITIONS,
};
