/**
 * What an error is called, never what it says: its message is content and
 * stays home, as does its stack trace.
 */

interface Named {
  constructor?: { name?: unknown } | undefined;
  name?: unknown;
}

function names(error: unknown): { ctor: string; name: string } {
  if (error === null || error === undefined) return { ctor: '', name: '' };
  const named = error as Named;
  const ctor = typeof named.constructor?.name === 'string' ? named.constructor.name : '';
  const name = typeof named.name === 'string' ? named.name : '';
  return { ctor, name };
}

/** The error's class, for `error.type`: `class ProviderDown extends Error` is `ProviderDown`. */
export function errorType(error: unknown): string {
  const { ctor, name } = names(error);
  // One class stands for many errors there, told apart by name alone.
  if (ctor === 'DOMException') return name || ctor;
  return ctor || name || 'Error';
}

/** The error's name, for a transport failure: a timeout reads `TimeoutError`, not `DOMException`. */
export function errorName(error: unknown): string {
  const { ctor, name } = names(error);
  return name || ctor || 'Error';
}

/**
 * `recordRun` refused a record the export queue has no room for, queuing
 * nothing of it: flush, or wait for the queue to drain, and record it again.
 */
export class QueueFullError extends Error {
  constructor(spans: number) {
    super(`the export queue has no room for the record's ${spans} spans; flush or wait, then record it again`);
    this.name = 'QueueFullError';
  }
}

/** An abort is the agent stopping, which is neither ok nor an error. */
export function isAbort(error: unknown): boolean {
  return typeof error === 'object' && error !== null && (error as Named).name === 'AbortError';
}
