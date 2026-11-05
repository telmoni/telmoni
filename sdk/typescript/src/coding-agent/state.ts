/**
 * A coding-agent plugin's files: a private folder, JSON states written whole or not at all, markers listed oldest first,
 * and the lock that keeps two hooks of one session from sending the same turns at once.
 */

import { randomUUID } from 'node:crypto';
import {
  chmodSync,
  closeSync,
  fsyncSync,
  mkdirSync,
  openSync,
  readFileSync,
  readdirSync,
  renameSync,
  rmSync,
  statSync,
  utimesSync,
  writeFileSync,
} from 'node:fs';
import { join } from 'node:path';
import { setTimeout as sleep } from 'node:timers/promises';

/** An id plain enough to name a file: a session's, a thread's, a conversation's. */
export const FILE_ID = /^[A-Za-z0-9_-]{1,128}$/;

export function isFileId(value: unknown): value is string {
  return typeof value === 'string' && FILE_ID.test(value);
}

export interface PrivateDirOptions {
  /** Also shut a folder that is there already to `700`: one an earlier version made may be open to others. */
  tighten?: boolean | undefined;
}

/** Creates `dir`, and any folder above it, mode `700`; a folder there already keeps its own mode unless `tighten`. */
export function ensurePrivateDir(dir: string, options?: PrivateDirOptions): void {
  mkdirSync(dir, { recursive: true, mode: 0o700 });
  if (options?.tighten === true && process.platform !== 'win32') chmodSync(dir, 0o700);
}

/**
 * Writes `text` to `path`, mode `600`: to `<path>.<pid>.tmp`, synced, then renamed over, so a hook killed or a machine
 * stopped mid-write leaves the last copy whole, and two hooks that took a stale lock in the same instant never write
 * into one file.
 */
export function writeFileAtomic(path: string, text: string): void {
  const temporary = `${path}.${process.pid}.tmp`;
  const fd = openSync(temporary, 'w', 0o600);
  try {
    writeFileSync(fd, text);
    fsyncSync(fd);
  } finally {
    closeSync(fd);
  }
  renameSync(temporary, path);
}

/**
 * `<dir>/<name>.json` as a JSON object, or undefined where none was saved. One that cannot be read, or is no object,
 * throws: read as new, it would send the session again from the top.
 */
export function readStateFile(dir: string, name: string): Record<string, unknown> | undefined {
  let text: string;
  try {
    text = readFileSync(join(dir, `${name}.json`), 'utf8');
  } catch (error) {
    if ((error as NodeJS.ErrnoException).code === 'ENOENT') return undefined;
    throw error;
  }
  const value: unknown = JSON.parse(text);
  if (value === null || typeof value !== 'object' || Array.isArray(value)) throw new TypeError('state is not an object');
  return value as Record<string, unknown>;
}

/** Saves `value` as `<dir>/<name>.json`, its bytes `JSON.stringify(value)`, through `writeFileAtomic`, in a private folder. */
export function writeStateFile(dir: string, name: string, value: unknown): void {
  ensurePrivateDir(dir);
  writeFileAtomic(join(dir, `${name}.json`), JSON.stringify(value));
}

/**
 * The ids of the files in `dir` named `<id><suffix>` (`.left`, `.pending`), each a plain id and none `except`, the
 * longest untouched first, ties by name.
 */
export function markedIds(dir: string, suffix: string, except?: string): string[] {
  let names: string[] = [];
  try {
    names = readdirSync(dir);
  } catch {
    // Nothing kept yet.
  }
  const waited = (name: string): number => {
    try {
      return statSync(join(dir, name)).mtimeMs;
    } catch {
      return 0;
    }
  };
  return names
    .filter((name) => name.endsWith(suffix) && isFileId(name.slice(0, -suffix.length)) && name.slice(0, -suffix.length) !== except)
    .sort((a, b) => waited(a) - waited(b) || a.localeCompare(b))
    .map((name) => name.slice(0, -suffix.length));
}

/** A lock not freshened for this long was left by a hook that died holding it. */
export const LOCK_STALE_MS = 60_000;

export interface LockOptions {
  /** How long an untouched lock stands before it is taken as a dead hook's; it is freshened every sixth of this. */
  staleMs?: number | undefined;
  /** How often a hook waiting for the lock tries it again. */
  pollMs?: number | undefined;
}

/**
 * Holds `<dir>/<name>.lock` while `work` runs, waiting up to `waitMs` (`Infinity` for as long as a live hook holds it);
 * false, `work` not run, if it never comes free. The lock holds a token of this hook's own (`<pid>:<uuid>`), so a hook
 * lets go of its own and never of one another hook took from it after a sleep of the machine.
 */
export async function withLock(dir: string, name: string, waitMs: number, work: () => Promise<void>, options?: LockOptions): Promise<boolean> {
  const staleMs = options?.staleMs ?? LOCK_STALE_MS;
  const pollMs = options?.pollMs ?? 200;
  ensurePrivateDir(dir);
  const path = join(dir, `${name}.lock`);
  const token = `${process.pid}:${randomUUID()}`;
  const deadline = Date.now() + waitMs;
  for (;;) {
    try {
      writeFileSync(path, token, { flag: 'wx', mode: 0o600 });
      break;
    } catch (error) {
      if ((error as NodeJS.ErrnoException).code !== 'EEXIST') throw error;
      try {
        if (Date.now() - statSync(path).mtimeMs > staleMs) rmSync(path, { force: true });
      } catch {
        // Freed between the two calls.
      }
      if (Date.now() >= deadline) return false;
      await sleep(pollMs);
    }
  }
  // Kept fresh while held, so only a dead hook's lock goes stale however long the work takes.
  const freshen = setInterval(() => {
    try {
      const now = new Date();
      utimesSync(path, now, now);
    } catch {
      // Taken away: the release below leaves it alone.
    }
  }, staleMs / 6);
  freshen.unref();
  try {
    await work();
  } finally {
    clearInterval(freshen);
    try {
      if (readFileSync(path, 'utf8') === token) rmSync(path, { force: true });
    } catch {
      // Gone already.
    }
  }
  return true;
}
