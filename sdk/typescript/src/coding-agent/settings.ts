/**
 * Where a coding-agent plugin sends: the key, the endpoint and the switches, always together from one place, never one
 * from each — the environment, else a key file read only where it is the person's own and shut to everyone else.
 */

import { createHmac } from 'node:crypto';
import { closeSync, constants, fstatSync, openSync, readFileSync } from 'node:fs';
import { isAbsolute } from 'node:path';

import { kindOf } from './log.js';

export interface Settings {
  apiKey: string | undefined;
  /** Unset, the SDK's own default. */
  endpoint: string | undefined;
  /** The person's switch, which goes with the key: the SDK sends content only with it and the project's mode `on`. */
  captureContent: boolean;
  /** The SDK's scrubbers, which go with the key too: each on unless the person turned it off. */
  scrubSecrets: boolean;
  scrubPii: boolean;
  /** Where they came from: an environment source by its prefix, a key file by its path (a refused one too), or nowhere. */
  source: SettingsSource;
  /** What kept a key file from being read, where one is there and was not: nothing is sent then. */
  keyFileProblem?: string;
}

export type SettingsSource = { kind: 'env'; prefix: string } | { kind: 'file'; path: string } | { kind: 'none' };

/** A set of variables named `<prefix>TELMONI_API_KEY`, `<prefix>TELMONI_ENDPOINT` and the switches'. */
export interface EnvSource {
  prefix?: string | undefined;
  /**
   * How its switches read: `sdk`, as the SDKs read `TELMONI_CAPTURE_CONTENT` (`1` alone turns content on, `0` alone a
   * scrubber off, White_Space off the ends); `boolean`, as a host hands a plugin's boolean option (`true` alone turns
   * content on, `false` alone a scrubber off).
   */
  switches?: 'sdk' | 'boolean' | undefined;
}

export interface SettingsOptions {
  env: Readonly<Record<string, string | undefined>>;
  /** The variables to read, in order: the first whose key is set gives everything. `[{}]` unless given. */
  envSources?: readonly EnvSource[] | undefined;
  /** The key files to read, in order, where no variable gives a key: the first that is there decides, even refused. */
  keyFiles?: readonly string[] | undefined;
}

/** A key file's fields, or why it was passed over. */
export interface KeyFile {
  apiKey?: string;
  endpoint?: string;
  captureContent?: boolean;
  scrubSecrets?: boolean;
  scrubPii?: boolean;
  problem?: string;
}

/** A file this size is not a plugin's few settings. */
const MAX_KEY_FILE_BYTES = 64 * 1024;

/** Unicode's White_Space, which the SDKs trim a switch's variable of: `trim` takes a byte-order mark too, and leaves U+0085. */
const SPACE = '[\\t\\n\\v\\f\\r \\u0085\\u00a0\\u1680\\u2000-\\u200a\\u2028\\u2029\\u202f\\u205f\\u3000]';
const ENDS = new RegExp(`^${SPACE}+|${SPACE}+$`, 'g');

/** `TELMONI_CAPTURE_CONTENT` as the SDKs read it: `1` alone, White_Space off its ends, so no spelling of "false" lets content out. */
export function contentSwitchOn(given: string | undefined): boolean {
  return (given ?? '').replace(ENDS, '') === '1';
}

/** A scrubber's variable, trimmed as the content switch's: `0` alone turns it off. */
export function scrubSwitchOn(given: string | undefined): boolean {
  return (given ?? '').replace(ENDS, '') !== '0';
}

function trimmed(given: unknown): string | undefined {
  return typeof given === 'string' && given.trim() !== '' ? given.trim() : undefined;
}

/** An absolute folder from a variable, or undefined: a relative one would put a plugin's files under the session's working folder. */
export function absoluteFrom(env: Readonly<Record<string, string | undefined>>, name: string): string | undefined {
  const given = trimmed(env[name]);
  return given !== undefined && isAbsolute(given) ? given : undefined;
}

/**
 * The key file at `path`, or undefined where there is none. Read as `ssh` reads a key: opened without blocking, so a
 * FIFO cannot hold the hook, and checked on the descriptor it is read from — a regular file, the person's own, no bit of
 * `077`, at most 64 KiB, a JSON object. A field there but empty or no string, or a switch neither true nor false, has
 * the whole file passed over, rather than send the key to the default endpoint in place of the one meant.
 */
export function readKeyFile(path: string): KeyFile | undefined {
  let fd: number;
  try {
    fd = openSync(path, constants.O_RDONLY | constants.O_NONBLOCK);
  } catch (error) {
    const code = (error as NodeJS.ErrnoException).code;
    if (code === 'ENOENT' || code === 'ENOTDIR') return undefined;
    if (code === 'EISDIR') return { problem: 'not a file' };
    return { problem: `unreadable (${kindOf(error)})` };
  }
  let parsed: unknown;
  try {
    const stat = fstatSync(fd);
    if (!stat.isFile()) return { problem: 'not a file' };
    if (process.platform !== 'win32') {
      // The owner first: another's file is not the person's to chmod.
      const uid = process.getuid?.();
      if (uid !== undefined && stat.uid !== uid) return { problem: 'owned by another user' };
      if ((stat.mode & 0o077) !== 0) return { problem: 'open to other users; chmod 600 it' };
    }
    if (stat.size > MAX_KEY_FILE_BYTES) return { problem: 'too large' };
    parsed = JSON.parse(readFileSync(fd, 'utf8'));
  } catch (error) {
    return { problem: error instanceof SyntaxError ? 'not JSON' : `unreadable (${kindOf(error)})` };
  } finally {
    closeSync(fd);
  }
  if (parsed === null || typeof parsed !== 'object' || Array.isArray(parsed)) return { problem: 'not a JSON object' };
  const fields = parsed as Record<string, unknown>;
  const file: KeyFile = {};
  for (const field of ['apiKey', 'endpoint'] as const) {
    if (fields[field] === undefined) continue;
    const value = trimmed(fields[field]);
    if (value === undefined) return { problem: `${field} empty or not a string` };
    file[field] = value;
  }
  for (const field of ['captureContent', 'scrubSecrets', 'scrubPii'] as const) {
    const given = fields[field];
    if (given === undefined) continue;
    if (typeof given !== 'boolean') return { problem: `${field} not true or false` };
    file[field] = given;
  }
  return file;
}

/**
 * The key, the endpoint and the switches, together from the first environment source with a key, else from the first
 * key file there; a key never goes to an endpoint, nor takes a switch, named anywhere but beside it.
 */
export function resolveSettings(options: SettingsOptions): Settings {
  const { env } = options;
  for (const { prefix = '', switches = 'sdk' } of options.envSources ?? [{}]) {
    const apiKey = trimmed(env[`${prefix}TELMONI_API_KEY`]);
    if (apiKey === undefined) continue;
    const capture = env[`${prefix}TELMONI_CAPTURE_CONTENT`];
    const secrets = env[`${prefix}TELMONI_SCRUB_SECRETS`];
    const pii = env[`${prefix}TELMONI_SCRUB_PII`];
    const sdk = switches === 'sdk';
    return {
      apiKey,
      endpoint: trimmed(env[`${prefix}TELMONI_ENDPOINT`]),
      captureContent: sdk ? contentSwitchOn(capture) : capture === 'true',
      scrubSecrets: sdk ? scrubSwitchOn(secrets) : secrets !== 'false',
      scrubPii: sdk ? scrubSwitchOn(pii) : pii !== 'false',
      source: { kind: 'env', prefix },
    };
  }
  const settings: Settings = { apiKey: undefined, endpoint: undefined, captureContent: false, scrubSecrets: true, scrubPii: true, source: { kind: 'none' } };
  for (const path of options.keyFiles ?? []) {
    const file = readKeyFile(path);
    if (file === undefined) continue;
    // The first file there decides: a second is no fallback for one refused, whose endpoint may have been the one meant.
    if (file.problem !== undefined) return { ...settings, source: { kind: 'file', path }, keyFileProblem: file.problem };
    return {
      apiKey: file.apiKey,
      endpoint: file.endpoint,
      captureContent: file.captureContent === true,
      scrubSecrets: file.scrubSecrets !== false,
      scrubPii: file.scrubPii !== false,
      source: { kind: 'file', path },
    };
  }
  return settings;
}

/**
 * Where a key sends, as the first 16 hex digits of an HMAC-SHA256 of the endpoint under the key: what turns a session
 * left unsent are held to, so another session's hook sends them nowhere they were not meant to go.
 */
export function destinationOf(settings: { apiKey: string | undefined; endpoint: string | undefined }): string {
  return createHmac('sha256', settings.apiKey ?? '').update(settings.endpoint ?? '').digest('hex').slice(0, 16);
}
