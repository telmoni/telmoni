/**
 * Scores: a judgement of a run, one of its spans or a session, as a number, a
 * yes or no, or a category. On a span still open it rides as OpenTelemetry's
 * `gen_ai.evaluation.result` event; once ended, the client sends it to
 * `/v1/scores` through the export queue, so a flush and a shutdown hold it as
 * they hold spans (`sdk/fixtures/scores.json`).
 */

import { randomUUID } from 'node:crypto';

import type { Attributes } from '@opentelemetry/api';

import * as contract from './contract.js';
import { LONE_SURROGATE } from './exporter.js';
import type { ScoreOptions, ScoreTarget, Warning } from './types.js';
import { trimEnds } from './whitespace.js';

/** A score held to its rules, its texts trimmed. */
export type Judged = { name: string; version: string } & (
  | { type: 'number'; value: number; range: readonly [number, number] | undefined }
  | { type: 'boolean'; value: boolean }
  | { type: 'category'; label: string }
);

/** Why a score is not sent, as every SDK words it. */
export const WHY = {
  name: `its name is 1 to ${contract.SCORE_NAME_MAX} characters`,
  number: 'a number score is a finite number within its range',
  category: `its category is 1 to ${contract.SCORE_CATEGORY_MAX} characters`,
  version: `its version is up to ${contract.SCORE_VERSION_MAX} characters`,
  range: "a range is a number score's alone",
  value: 'its value is a number, a yes or no, or a category',
  target: `it names a run, a run and one of its spans by 16 hex digits, or a session of up to ${contract.SESSION_ID_MAX} characters`,
  ended: "its span had ended; send it with the client's score, naming the span",
  full: `its span holds ${contract.SCORES_PER_SPAN} scores already`,
} as const;

type WarnOnce = (key: string, text: string, warning: Warning) => void;

/** Warns once of a score not sent, by its why. */
export function refuse(warnOnce: WarnOnce, why: string): void {
  warnOnce(`score:${why}`, `telmoni: a score was not sent: ${why}`, { kind: 'input' });
}

/** `text` trimmed, or undefined where it is no string or past `most` characters. */
function trimmed(text: unknown, most: number): string | undefined {
  if (typeof text !== 'string') return undefined;
  const kept = trimEnds(text);
  return [...kept].length <= most ? kept : undefined;
}

/** The score held to its rules, or why it is not sent. */
export function judge(name: unknown, value: unknown, options: ScoreOptions | undefined): Judged | string {
  const named = trimmed(name, contract.SCORE_NAME_MAX);
  if (!named) return WHY.name;
  const given: unknown = options?.version;
  const version = given === undefined || given === null ? '' : trimmed(given, contract.SCORE_VERSION_MAX);
  if (version === undefined) return WHY.version;
  const range: unknown = options?.range;
  const ranged = range !== undefined && range !== null;
  if (typeof value === 'number') {
    if (!Number.isFinite(value)) return WHY.number;
    if (!ranged) return { name: named, version, type: 'number', value, range: undefined };
    const [min, max]: unknown[] = Array.isArray(range) && range.length === 2 ? range : [];
    if (typeof min !== 'number' || typeof max !== 'number' || !Number.isFinite(min) || !Number.isFinite(max)) {
      return WHY.number;
    }
    if (!(min < max && min <= value && value <= max)) return WHY.number;
    return { name: named, version, type: 'number', value, range: [min, max] };
  }
  if (typeof value !== 'boolean' && typeof value !== 'string') return WHY.value;
  if (ranged) return WHY.range;
  if (typeof value === 'boolean') return { name: named, version, type: 'boolean', value };
  const label = trimmed(value, contract.SCORE_CATEGORY_MAX);
  if (!label) return WHY.category;
  return { name: named, version, type: 'category', label };
}

/** The score as its span's evaluation event carries it. */
export function eventOf(score: Judged): Attributes {
  const attributes: Attributes = { [contract.EVALUATION_NAME]: score.name };
  if (score.type === 'number') {
    attributes[contract.EVALUATION_VALUE] = score.value;
    if (score.range !== undefined) {
      attributes[contract.SCORE_RANGE_MIN] = score.range[0];
      attributes[contract.SCORE_RANGE_MAX] = score.range[1];
    }
  } else if (score.type === 'boolean') {
    attributes[contract.EVALUATION_VALUE] = score.value ? 1 : 0;
    attributes[contract.SCORE_TYPE] = 'boolean';
  } else {
    attributes[contract.EVALUATION_LABEL] = score.label;
  }
  if (score.version !== '') attributes[contract.SCORE_VERSION] = score.version;
  return attributes;
}

const UUID = /^[0-9a-f]{8}-?[0-9a-f]{4}-?[0-9a-f]{4}-?[0-9a-f]{4}-?[0-9a-f]{12}$/i;
const SPAN = /^[0-9a-f]{16}$/i;

/** A UUID's 32 hex digits, lowercase, or undefined where it is none. */
export function uuidHexOf(id: unknown): string | undefined {
  if (typeof id !== 'string' || !UUID.test(id) || ![32, 36].includes(id.length)) return undefined;
  return id.replaceAll('-', '').toLowerCase();
}

/** 32 hex digits as a UUID is written. */
export function hyphenated(hex: string): string {
  return [hex.slice(0, 8), hex.slice(8, 12), hex.slice(12, 16), hex.slice(16, 20), hex.slice(20)].join('-');
}

/** What the target names as `/v1/scores` takes it, or undefined where it names none of the three. */
function targetOf(target: ScoreTarget): Record<string, string> | undefined {
  const set = (value: unknown): boolean => value !== undefined && value !== null;
  const { runId, spanId, sessionId } = target;
  if (set(sessionId)) {
    if (set(runId) || set(spanId)) return undefined;
    const session = trimmed(sessionId, contract.SESSION_ID_MAX);
    return session ? { session_id: session } : undefined;
  }
  const hex = uuidHexOf(runId);
  // The nil UUID is no run's id.
  if (hex === undefined || /^0+$/.test(hex)) return undefined;
  const ids: Record<string, string> = { run_id: hyphenated(hex) };
  if (!set(spanId)) return ids;
  if (typeof spanId !== 'string' || !SPAN.test(spanId)) return undefined;
  ids['span_id'] = spanId.toLowerCase();
  return ids;
}

/** One item of `/v1/scores`' body as JSON, its id fresh so a retry replaces itself; undefined where the target is none. */
export function lateItem(target: ScoreTarget, score: Judged): string | undefined {
  const ids = targetOf(target);
  if (ids === undefined) return undefined;
  const item: Record<string, unknown> = { id: randomUUID(), ...ids, name: score.name, data_type: score.type };
  if (score.type === 'number') {
    item['value'] = score.value;
    if (score.range !== undefined) [item['range_min'], item['range_max']] = score.range;
  } else if (score.type === 'boolean') {
    item['value'] = score.value ? 1 : 0;
  } else {
    item['categories'] = [score.label];
  }
  if (score.version !== '') item['version'] = score.version;
  // A lone surrogate is no text the server reads, and would lose the body.
  return JSON.stringify(item, (_key, value: unknown) =>
    typeof value === 'string' ? value.replace(LONE_SURROGATE, '\ufffd') : value,
  );
}
