/**
 * A coding-agent plugin's log: a line per thing that went wrong, naming its kind and never anything from the session —
 * no text, no path, no key — since a hook runs where nobody sees its output.
 */

import { appendFileSync, renameSync, statSync } from 'node:fs';
import { join } from 'node:path';

import { ensurePrivateDir } from './state.js';

/** Past this, the log is moved aside to `<file>.1` and begun again, the older one overwritten. */
export const LOG_MAX_BYTES = 1024 * 1024;

export interface LogOptions {
  /** The log's name in the plugin's folder: `hook.log` unless the host's is another. */
  file?: string | undefined;
}

/** Appends `<ISO time> <line>` to the log in `dir`; with no folder, or nowhere to write, the line is dropped. Never throws. */
export function writeLog(dir: string | undefined, line: string, options?: LogOptions): void {
  if (dir === undefined) return;
  try {
    ensurePrivateDir(dir);
    const path = join(dir, options?.file ?? 'hook.log');
    try {
      if (statSync(path).size > LOG_MAX_BYTES) renameSync(path, `${path}.1`);
    } catch {
      // No log yet.
    }
    appendFileSync(path, `${new Date().toISOString()} ${line}\n`, { mode: 0o600 });
  } catch {
    // Nowhere left to say so; the host must never see the plugin fail.
  }
}

/** An error's class and code, which is all of it a log keeps: its message may hold a path or the session's words. */
export function kindOf(error: unknown): string {
  if (error instanceof Error) {
    const code = (error as { code?: unknown }).code;
    return typeof code === 'string' ? `${error.name}:${code}` : error.name;
  }
  return typeof error;
}

/** The first characters of an id, enough to tell a session's lines apart in the log. */
export function shortId(id: string, length = 8): string {
  return id.slice(0, length);
}
