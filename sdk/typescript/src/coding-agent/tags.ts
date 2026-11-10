/**
 * The tags a person sets for the turns started under a folder: those of the nearest `.telmoni-tags.json` at or above
 * it, read for its tags alone. The folder and the file's path serve only to find the file: never kept, sent or logged.
 */

import { type Stats, closeSync, constants, fstatSync, lstatSync, openSync, readFileSync } from 'node:fs';
import { dirname, isAbsolute, join, resolve } from 'node:path';

import type { TraceRecord } from '../types.js';

/** A folder's tags, and a line for each problem met reading them, naming its kind alone: no key, no value, no path. */
export interface FolderTags {
  tags: Readonly<Record<string, string>>;
  problems: readonly string[];
}

export const TAGS_FILE = '.telmoni-tags.json';
const MAX_BYTES = 64 * 1024;
/** Ingest's caps: the tags a trace carries, and a value's characters. */
export const TAGS_MAX = 32;
const VALUE_MAX = 256;
/** Never a `:`, which the console's `key:value` filter splits on. */
const KEY = /^[A-Za-z0-9_.-]{1,64}$/;
/** A control character, or half of a character: ingest cannot decode a lone surrogate, and refuses the whole export holding one. */
const NOT_TEXT = /[\u0000-\u001f\u007f-\u009f]|\p{Cs}/u;

const NOT_PLAIN = 'tags file not read: not a plain file';
const TOO_LARGE = 'tags file not read: past 64 KiB';
const NOT_OBJECT = 'tags file not read: not a JSON object';
const UNREADABLE = 'tags file not read: unreadable';
const BAD_KEY = 'tag dropped: its key is not 1 to 64 letters, digits, _ . or -';
const RESERVED = 'tag dropped: skills is reserved';
const BAD_VALUE = 'tag dropped: its value is not text of at most 256 characters';
const TOO_MANY = 'tags past 32 dropped';

/** Every line a problem is told in, so a plugin that keeps them (a spool, a state's warned list) keeps nothing else. */
export const TAG_PROBLEMS: ReadonlySet<string> = new Set([NOT_PLAIN, TOO_LARGE, NOT_OBJECT, UNREADABLE, BAD_KEY, RESERVED, BAD_VALUE, TOO_MANY]);

const NONE: FolderTags = { tags: {}, problems: [] };

function notRead(problem: string): FolderTags {
  return { tags: {}, problems: [problem] };
}

/** The tags of the nearest `.telmoni-tags.json` at or above `folder`: none where there is no such file, or it is not one to read. */
export function tagsAt(folder: string): FolderTags {
  if (!isAbsolute(folder)) return NONE;
  for (let place = resolve(folder); ; place = dirname(place)) {
    const path = join(place, TAGS_FILE);
    let entry: Stats;
    try {
      entry = lstatSync(path);
    } catch (error) {
      const code = (error as NodeJS.ErrnoException).code;
      if (code !== 'ENOENT' && code !== 'ENOTDIR') return notRead(UNREADABLE);
      if (dirname(place) === place) return NONE;
      continue;
    }
    // The nearest entry decides: one not read leaves the turn with none, never a parent's.
    return entry.isFile() ? read(path) : notRead(NOT_PLAIN);
  }
}

function read(path: string): FolderTags {
  let bytes: Buffer;
  try {
    // Never through a link swapped in since the look, and never waiting on a pipe.
    const fd = openSync(path, constants.O_RDONLY | constants.O_NOFOLLOW | constants.O_NONBLOCK);
    try {
      const stat = fstatSync(fd);
      if (!stat.isFile()) return notRead(NOT_PLAIN);
      if (stat.size > MAX_BYTES) return notRead(TOO_LARGE);
      bytes = readFileSync(fd);
    } finally {
      closeSync(fd);
    }
  } catch {
    return notRead(UNREADABLE);
  }
  if (bytes.length > MAX_BYTES) return notRead(TOO_LARGE);
  let parsed: unknown;
  try {
    parsed = JSON.parse(new TextDecoder('utf-8', { fatal: true }).decode(bytes));
  } catch {
    parsed = undefined;
  }
  if (parsed === null || typeof parsed !== 'object' || Array.isArray(parsed)) return notRead(NOT_OBJECT);
  const kept: Array<[string, string]> = [];
  const problems: string[] = [];
  for (const [key, value] of Object.entries(parsed as Record<string, unknown>)) {
    const problem = tagProblem(key, value);
    if (problem !== undefined) problems.push(problem);
    else if (kept.length === TAGS_MAX) problems.push(TOO_MANY);
    else kept.push([key, value as string]);
  }
  // From its entries, so a key such as `__proto__` is a tag like any other.
  return { tags: Object.fromEntries(kept), problems };
}

/** Why a member is no tag, in words naming no key and no value; undefined for one ingest keeps. */
export function tagProblem(key: string, value: unknown): string | undefined {
  if (!KEY.test(key)) return BAD_KEY;
  // Claude Code's own tag of the skills a turn ran.
  if (key === 'skills') return RESERVED;
  if (typeof value !== 'string' || [...value].length > VALUE_MAX || NOT_TEXT.test(value)) return BAD_VALUE;
  return undefined;
}

/** Tags read back from a plugin's own files, each held to the rules again: a line written by hand carries none that breaks one. */
export function checkedTags(value: unknown): Record<string, string> | undefined {
  if (value === null || typeof value !== 'object' || Array.isArray(value)) return undefined;
  const kept = Object.entries(value as Record<string, unknown>).filter(([key, tag]) => tagProblem(key, tag) === undefined);
  return kept.length > 0 ? (Object.fromEntries(kept.slice(0, TAGS_MAX)) as Record<string, string>) : undefined;
}

/** Puts on `record` the tags of `folder`, where its turn started, logging each problem the first time `warned` meets it. */
export function tagTrace(record: TraceRecord, folder: string | undefined, warned: Set<string>, log: (line: string) => void): void {
  if (folder === undefined) return;
  const { tags, problems } = tagsAt(folder);
  for (const problem of problems) {
    if (warned.has(problem)) continue;
    warned.add(problem);
    log(problem);
  }
  // Metadata, sent whatever the content switch.
  if (Object.keys(tags).length > 0) record.tags = tags;
}
