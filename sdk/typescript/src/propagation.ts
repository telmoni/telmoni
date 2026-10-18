/**
 * A run across services: the W3C `traceparent` of the span current inside a
 * run, and the run's id and agent as the W3C `baggage` members
 * `telmoni.run_id` and `telmoni.agent`, written into an outgoing request's
 * headers and read back from an incoming one's. The SDK's own helpers, never
 * a propagator set globally: an application's propagators are its own.
 */

import { TraceFlags } from '@opentelemetry/api';
import type { SpanContext } from '@opentelemetry/api';

import * as contract from './contract.js';
import type { HeaderCarrier } from './types.js';

/** What headers name: the span to continue under, and the run. */
export interface Continued {
  spanContext: SpanContext;
  runId: string;
  agent: string;
}

const UUID = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i;
const HEX = /^[0-9a-f]+$/;

/** `span` and the run as the next service reads them, beside the members `headers` already hold. */
export function writeHeaders(headers: HeaderCarrier, span: SpanContext, runId: string, agent: string): void {
  const sampled = (span.traceFlags & TraceFlags.SAMPLED) !== 0 ? '01' : '00';
  setHeader(headers, contract.TRACEPARENT, `00-${span.traceId}-${span.spanId}-${sampled}`);
  const members = (getHeader(headers, contract.BAGGAGE) ?? '')
    .split(',')
    .map((member) => member.trim())
    .filter((member) => member !== '' && !isOurs(member));
  members.push(`${contract.RUN_ID}=${runId}`, `${contract.AGENT}=${encodeBaggageValue(agent)}`);
  setHeader(headers, contract.BAGGAGE, members.join(','));
}

/** The run `headers` name, or undefined for headers that name none. */
export function readHeaders(headers: HeaderCarrier): Continued | undefined {
  const traceparent = getHeader(headers, contract.TRACEPARENT);
  if (traceparent === undefined) return undefined;
  const spanContext = parseTraceparent(traceparent);
  if (spanContext === undefined) return undefined;
  const members = baggageMembers(getHeader(headers, contract.BAGGAGE) ?? '');
  const runId = members.get(contract.RUN_ID);
  const agent = members.get(contract.AGENT);
  if (runId === undefined || !UUID.test(runId) || agent === undefined || agent === '') return undefined;
  return { spanContext, runId: runId.toLowerCase(), agent };
}

/**
 * A `traceparent` as the W3C spells it: a version that is not `ff`, a trace
 * id and a span id that are not zeros, and the flags; a version other than
 * `00` may be followed by more, which is ignored.
 */
export function parseTraceparent(header: string): SpanContext | undefined {
  const parts = header.trim().split('-');
  if (parts.length < 4) return undefined;
  const [version, traceId, spanId, flags] = parts as [string, string, string, string];
  if (!isHex(version, 2) || version === 'ff') return undefined;
  if (version === '00' && parts.length !== 4) return undefined;
  if (!isHex(traceId, 32) || !/[1-9a-f]/.test(traceId)) return undefined;
  if (!isHex(spanId, 16) || !/[1-9a-f]/.test(spanId)) return undefined;
  if (!isHex(flags, 2)) return undefined;
  return { traceId, spanId, traceFlags: Number.parseInt(flags, 16) & TraceFlags.SAMPLED, isRemote: true };
}

/** The baggage's members by key, each value percent-decoded, its properties left off. */
function baggageMembers(header: string): Map<string, string> {
  const members = new Map<string, string>();
  for (const member of header.split(',')) {
    const equals = member.indexOf('=');
    if (equals < 0) continue;
    const key = member.slice(0, equals).trim();
    const value = member.slice(equals + 1).split(';', 1)[0]!.trim();
    if (key !== '') members.set(key, decodeBaggageValue(value));
  }
  return members;
}

function isOurs(member: string): boolean {
  const key = member.split('=', 1)[0]!.trim();
  return key === contract.RUN_ID || key === contract.AGENT;
}

function isHex(text: string, length: number): boolean {
  return text.length === length && HEX.test(text);
}

/**
 * A baggage value as the W3C writes it: the printable ASCII its grammar
 * allows as it is, and every other byte of the UTF-8 percent-encoded, so a
 * space, a comma or a name in another script travels whole.
 */
export function encodeBaggageValue(value: string): string {
  let out = '';
  for (const byte of new TextEncoder().encode(value)) {
    out += allowedInBaggage(byte) ? String.fromCharCode(byte) : `%${byte.toString(16).toUpperCase().padStart(2, '0')}`;
  }
  return out;
}

/** A baggage value read back: each `%XX` its byte, a `%` without two hex digits kept as it is. */
export function decodeBaggageValue(value: string): string {
  const bytes: number[] = [];
  for (let i = 0; i < value.length; i++) {
    const pair = value.slice(i + 1, i + 3);
    if (value[i] === '%' && pair.length === 2 && HEX.test(pair.toLowerCase())) {
      bytes.push(Number.parseInt(pair, 16));
      i += 2;
    } else {
      for (const byte of new TextEncoder().encode(value[i]!)) bytes.push(byte);
    }
  }
  return new TextDecoder().decode(Uint8Array.from(bytes));
}

/**
 * The W3C's `baggage-octet`: printable ASCII but space, `"`, `,`, `;` and
 * `\`; and but `%`, which the grammar allows and a reader would decode.
 */
function allowedInBaggage(byte: number): boolean {
  return (
    byte === 0x21 ||
    (byte >= 0x23 && byte <= 0x2b && byte !== 0x25) ||
    (byte >= 0x2d && byte <= 0x3a) ||
    (byte >= 0x3c && byte <= 0x5b) ||
    (byte >= 0x5d && byte <= 0x7e)
  );
}

type Methods = { get(name: string): string | null | undefined; set(name: string, value: string): void };

function hasGet(headers: HeaderCarrier): headers is Methods {
  return typeof (headers as Partial<Methods>).get === 'function';
}

/** The header named `name`, however its key is cased; several lines joined, as HTTP joins them. */
function getHeader(headers: HeaderCarrier, name: string): string | undefined {
  if (hasGet(headers)) {
    const value = headers.get(name);
    return typeof value === 'string' ? value : undefined;
  }
  for (const [key, value] of Object.entries(headers)) {
    if (key.toLowerCase() !== name) continue;
    if (typeof value === 'string') return value;
    if (Array.isArray(value)) return value.join(',');
  }
  return undefined;
}

/** The header named `name` set once, whatever case its key was in before. */
function setHeader(headers: HeaderCarrier, name: string, value: string): void {
  if (hasGet(headers)) {
    headers.set(name, value);
    return;
  }
  const record = headers as Record<string, string | readonly string[] | undefined>;
  for (const key of Object.keys(record)) {
    if (key.toLowerCase() === name) delete record[key];
  }
  record[name] = value;
}
