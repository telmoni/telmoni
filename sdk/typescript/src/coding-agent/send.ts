/**
 * Sends a coding agent's finished turns through the SDK, each from a client of its own, and reads that client's
 * warnings into a verdict: a turn sent again, after a hook that died before saving that it had, is the same bytes,
 * which ingest stores once within the hour.
 */

import { Telmoni } from '../client.js';
import type { Hooks, RunRecord, Warning } from '../types.js';

export interface Destination {
  endpoint: string | undefined;
  apiKey: string;
  /** The person's switch, which travels with the key: content leaves only with it and the project's mode `on`. */
  captureContent: boolean;
  /** The SDK's scrubbers, which travel with the key too. */
  scrubSecrets: boolean;
  scrubPii: boolean;
}

/**
 * What became of a turn. Done with, never sent again: `sent`; `partial`, taken less a span refused or a field dropped;
 * `refused`, for good, which the same bytes would meet on every try; `invalid`, a record the SDK itself refused. Not:
 * `kept`, which a later try may get through. Each `warnings` is every line the turn's client said, which names
 * statuses, problem types, span ids and attribute keys, never anything of the session.
 */
export type Verdict =
  | { outcome: 'sent' }
  | { outcome: 'partial'; warnings: string[] }
  | { outcome: 'refused'; warnings: string[] }
  | { outcome: 'invalid'; error: string }
  | { outcome: 'kept'; warnings: string[]; undelivered: string[]; timedOut: boolean };

/** Whether a turn is done with: anything but `kept`. */
export function isDone(verdict: Verdict): boolean {
  return verdict.outcome !== 'kept';
}

/** Refused for what it holds (`400`, `413`), for a fault of ingest's own (`500`), for an organization paused (`402`). */
export const FINAL_STATUSES: ReadonlySet<number> = new Set([400, 402, 413, 500]);
/** The same, and a key refused (`401`, `403`), for a host that reads its key once a load, so no later try holds another. */
const FINAL_WITH_KEY: ReadonlySet<number> = new Set([...FINAL_STATUSES, 401, 403]);
/** Ingest's `500` for a fault of its database, which a later try may find mended, as it would none of its other `500`s. */
const DATABASE_FAILED = '/errors/internal/database';

/**
 * Whether a warning tells of spans the SDK could not deliver, which a later try may yet get through: Telmoni out of
 * reach or busy past its attempts, an export dropped, or refused with a status not in `final` or for its database.
 */
export function undelivered(warning: Warning, final: ReadonlySet<number> = FINAL_STATUSES): boolean {
  const { kind, status, problemType } = warning;
  if (kind === 'failed' || kind === 'export_error' || kind === 'queue_full') return true;
  return kind === 'refused' && (status === undefined || !final.has(status) || (status === 500 && problemType === DATABASE_FAILED));
}

/** What a flush and the warnings heard during it say became of a turn. */
export function judge(heard: ReadonlyArray<{ message: string; warning: Warning }>, flushed: boolean, final: ReadonlySet<number> = FINAL_STATUSES): Verdict {
  const warnings = heard.map(({ message }) => message);
  const lost = heard.filter(({ warning }) => undelivered(warning, final)).map(({ message }) => message);
  if (!flushed || lost.length > 0) return { outcome: 'kept', warnings, undelivered: lost, timedOut: !flushed };
  if (heard.some(({ warning }) => warning.kind === 'refused')) return { outcome: 'refused', warnings };
  return warnings.length > 0 ? { outcome: 'partial', warnings } : { outcome: 'sent' };
}

/** The log line for a verdict, or undefined for a turn taken whole. */
export function verdictLine(verdict: Verdict): string | undefined {
  switch (verdict.outcome) {
    case 'sent':
      return undefined;
    case 'partial':
      return `taken in part: ${verdict.warnings.join('; ')}`;
    case 'refused':
      return `refused, not sent again: ${verdict.warnings.join('; ')}`;
    case 'invalid':
      return `turn refused by the SDK: ${verdict.error}`;
    case 'kept':
      return verdict.timedOut ? 'not sent: the export did not finish in time' : `not sent: ${verdict.undelivered.join('; ')}`;
  }
}

export interface SenderOptions {
  /**
   * Whether a key refused (`401`, `403`) is for good: true for a host that reads its key once a load, where every later
   * try would meet the same; false, the default, where each hook reads it afresh and a mended key sends the turn.
   */
  keyReadOnce?: boolean | undefined;
  /** The SDK's transport and wait between attempts, for a test to inject. */
  hooks?: Pick<Hooks, 'transport' | 'sleep'> | undefined;
}

interface Client {
  telmoni: Telmoni;
  heard: Array<{ message: string; warning: Warning }>;
}

/**
 * One SDK client per turn, since the SDK warns of each kind once a client and a turn refused as the one before it,
 * unwarned, would read as taken. The first is made with the sender, so an endpoint the SDK refuses throws its
 * `TypeError` from the constructor, before any turn is read; its message names at most the endpoint's host.
 */
export class Sender {
  readonly #destination: Destination;
  readonly #final: ReadonlySet<number>;
  readonly #hooks: Pick<Hooks, 'transport' | 'sleep'> | undefined;
  #first: Client | undefined;

  constructor(destination: Destination, options?: SenderOptions) {
    this.#destination = destination;
    this.#final = options?.keyReadOnce === true ? FINAL_WITH_KEY : FINAL_STATUSES;
    this.#hooks = options?.hooks;
    this.#first = this.#client(undefined);
  }

  #client(release: string | undefined): Client {
    const heard: Client['heard'] = [];
    const { endpoint, apiKey, captureContent, scrubSecrets, scrubPii } = this.#destination;
    const telmoni = new Telmoni(
      { endpoint, apiKey, captureContent, scrubSecrets, scrubPii, ...(release === undefined ? {} : { release }) },
      {
        ...this.#hooks,
        warn: (message, warning) => void heard.push({ message, warning }),
        // Unbounded: the SDK queues a record whole, and a turn past its own 2,048 spans would be refused on every try.
        processor: { maxQueueSize: Number.POSITIVE_INFINITY },
      },
    );
    return { telmoni, heard };
  }

  /** Sends one turn from a client of its own, `release` naming the host's version, and says what became of it. */
  async send(record: RunRecord, options?: { release?: string | undefined }): Promise<Verdict> {
    const release = options?.release;
    const first = this.#first;
    this.#first = undefined;
    // The first client names no release: a turn that has one gets a client of its own.
    if (first !== undefined && release !== undefined) await first.telmoni.shutdown();
    const { telmoni, heard } = first !== undefined && release === undefined ? first : this.#client(release);
    try {
      try {
        telmoni.recordRun(record);
      } catch (error) {
        return { outcome: 'invalid', error: error instanceof Error ? error.name : typeof error };
      }
      // As long as the SDK's own attempts take, however many batches: a turn the platform took late is never kept and stored again.
      const flushed = await telmoni.flush(Number.POSITIVE_INFINITY);
      return judge(heard, flushed, this.#final);
    } finally {
      await telmoni.shutdown();
    }
  }

  async close(): Promise<void> {
    const first = this.#first;
    this.#first = undefined;
    await first?.telmoni.shutdown();
  }
}

/** The SDK's reason for refusing `destination`'s endpoint, which names at most its host, or undefined where it takes it. */
export async function endpointProblem(destination: Destination): Promise<string | undefined> {
  let sender: Sender;
  try {
    sender = new Sender(destination);
  } catch (error) {
    if (error instanceof TypeError) return error.message;
    throw error;
  }
  await sender.close();
  return undefined;
}
