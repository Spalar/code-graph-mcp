'use strict';
// Single home for the `~/.cache/code-graph` file names.
//
// JS-05 (audit 2026-09-05): `update-state.json` was spelled in five modules,
// `install.lock` in two, `install-manifest.json` in three — and two of those
// rebuilt the whole path out of `os.homedir()` rather than joining `CACHE_DIR`,
// so the modules that would break first if the cache layout moved were also the
// two that never load the module where it was defined. A rename had to be found
// by grep. `tests/hardening.rs::cache_file_names_have_exactly_one_spelling`
// keeps this the only file that may spell them.
//
// Deliberately tiny and dependency-free beyond node builtins, for ONE consumer:
// `user-prompt-context.js` runs on the UserPromptSubmit hook path and does not
// load `lifecycle.js`, so taking a path string from there would cost it the
// whole module. Measured at 30.1 ms as shipped versus 34.5 ms if it also loaded
// `lifecycle.js` (+14%).
//
// `statusline.js` was the other hardcoded spelling and is NOT a reason for this
// file — it already does `require('./lifecycle')`, so its copy of the three path
// segments bought nothing. An earlier version of this comment claimed both
// modules needed the split (pre-ship review 2026-09-07).
const fs = require('fs');
const os = require('os');
const path = require('path');

const CACHE_DIR = path.join(os.homedir(), '.cache', 'code-graph');

/** Auto-update bookkeeping: available version, attempt counters, last error. */
const UPDATE_STATE_FILE = path.join(CACHE_DIR, 'update-state.json');
/** What the last install wrote, and where. Absent = never installed. */
const MANIFEST_FILE = path.join(CACHE_DIR, 'install-manifest.json');
/** Inter-process install lock (see install-lock.js). */
const INSTALL_LOCK_FILE = path.join(CACHE_DIR, 'install.lock');

/**
 * Teardown tombstone — a SIBLING of CACHE_DIR, deliberately not a file inside
 * it.
 *
 * A teardown that runs while a SessionStart-spawned `auto-update` is in flight
 * used to get CACHE_DIR re-created under it: measured at 42,847,128 B of fresh
 * binary plus three JSON files, 2 of 2 runs, in the arm where the teardown
 * beats the download.
 *
 * INSTALL_LOCK_FILE above is the token the updater already respects, and taking
 * it here was prototyped for exactly this job and REFUTED: it lives at
 * CACHE_DIR/install.lock, `removeCacheResidue()` deletes CACHE_DIR, so the lock
 * goes with the directory and the updater acquires freely seconds later. A
 * mutual-exclusion token stored inside the resource being destroyed cannot
 * guard that destruction — which is why this one is a sibling.
 */
const UNINSTALL_TOMBSTONE_FILE = path.join(os.homedir(), '.cache', 'code-graph.uninstalled');

/**
 * How long a tombstone suppresses writes. Longer than any teardown, shorter
 * than any interval over which a stale one could matter: a reinstall inside the
 * window merely skips one update check, and the install itself supplies the
 * binary. The TTL is what keeps this from becoming a permanent kill switch —
 * a tombstone that never expired would disable auto-update for the life of the
 * machine, a worse failure than the residue it prevents.
 */
const UNINSTALL_TOMBSTONE_TTL_MS = 5 * 60 * 1000;

/** Record that a teardown is in flight. Best-effort: a cache we cannot write
 *  to is one the updater's own writes will fail against too. */
function writeUninstallTombstone({ file = UNINSTALL_TOMBSTONE_FILE, now = Date.now() } = {}) {
  try {
    fs.mkdirSync(path.dirname(file), { recursive: true });
    fs.writeFileSync(file, JSON.stringify({ at: new Date(now).toISOString() }));
    return true;
  } catch {
    return false;
  }
}

/**
 * Is a teardown in flight right now?
 *
 * Fail-OPEN on every unreadable shape — absent, unparseable, no timestamp. The
 * alternative is a corrupt file that can never expire, and the spec rates a
 * permanently suppressed updater worse than the 41 MB this exists to stop.
 */
function uninstallTombstoneActive({
  file = UNINSTALL_TOMBSTONE_FILE,
  now = Date.now(),
  ttlMs = UNINSTALL_TOMBSTONE_TTL_MS,
} = {}) {
  let at;
  try {
    at = Date.parse(JSON.parse(fs.readFileSync(file, 'utf8')).at);
  } catch {
    return false;
  }
  if (!Number.isFinite(at)) return false;
  // `age >= 0` is not redundant with the upper bound: a NEGATIVE age — a
  // timestamp in the future — is always less than the TTL, so a bare
  // `age < ttlMs` would treat a tombstone stamped a year ahead as active for a
  // year, and a legitimate one plus an 8h clock rollback (NTP correction, VM
  // restore, RTC drift) as active for 8h05m. That is the never-expiring
  // kill switch this design's TTL exists to prevent, in bounded form, and it is
  // reachable without any corruption because the file carries OUR OWN
  // Date.now(). A future stamp means the clock moved, not that a teardown is in
  // flight, so it fails open like every other shape we cannot trust.
  const age = now - at;
  return age >= 0 && age < ttlMs;
}

/** Drop the tombstone. The TTL is the backstop; an install clears it eagerly so
 *  a reinstall inside the window does not skip its first update check. */
function clearUninstallTombstone({ file = UNINSTALL_TOMBSTONE_FILE } = {}) {
  try {
    fs.rmSync(file, { force: true });
    return true;
  } catch {
    return false;
  }
}

module.exports = {
  CACHE_DIR,
  UPDATE_STATE_FILE,
  MANIFEST_FILE,
  INSTALL_LOCK_FILE,
  UNINSTALL_TOMBSTONE_FILE,
  UNINSTALL_TOMBSTONE_TTL_MS,
  writeUninstallTombstone,
  uninstallTombstoneActive,
  clearUninstallTombstone,
};
