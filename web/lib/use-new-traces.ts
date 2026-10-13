"use client";

import { useSyncExternalStore } from "react";

// ⚠ **The listener writes, the list reads, and there is no second copy.**
// Ingest's nudges name the runs that landed (`spans:ingested`); the Traces
// list, rather than drawing itself under the reader's hand, counts them in a
// bar until the reader asks for them, or until it was drawn with them on it.
// A run already on the list is not new, however many batches its spans
// still arrive in. Kept per project, as a nudge names one, so a list of
// another project reads nothing of it; module state, as the preferences
// are, since a browser tab holds one list at a time.

/** What a list has to show: how many traces landed since it was drawn, and
 *  whether some went uncounted, which makes the count a floor. */
export interface NewTraces {
  count: number;
  more: boolean;
}

const NONE: NewTraces = { count: 0, more: false };

/** How many run ids a list keeps between draws: past it the count is a
 *  floor anyway, and a list left open through a day's flood holds no more. */
export const LANDED_MAX = 1_000;

/** The run ids that landed and are not on the list, by project. */
const landed = new Map<string, Set<string>>();

/** The run ids on the list as last drawn, by project. */
const onPage = new Map<string, Set<string>>();

/** The projects whose count is a floor: a nudge named only some of a batch,
 *  or the list kept as many ids as it keeps. */
const floors = new Set<string>();

/** The projects whose list, as last drawn, had no rows: nothing on it can
 *  move, so a nudge may draw it rather than be counted. */
const empties = new Set<string>();

/** One object per project until its count changes: `useSyncExternalStore`
 *  tells a change by identity. */
const snapshots = new Map<string, NewTraces>();

let listeners: Array<() => void> = [];

function emit() {
  for (const l of listeners) l();
}

function subscribe(onChange: () => void) {
  listeners = [...listeners, onChange];
  return () => {
    listeners = listeners.filter((l) => l !== onChange);
  };
}

function changed(projectId: string) {
  const count = landed.get(projectId)?.size ?? 0;
  const more = floors.has(projectId);
  const before = snapshots.get(projectId) ?? NONE;
  if (before.count === count && before.more === more) return;
  if (count === 0 && !more) snapshots.delete(projectId);
  else snapshots.set(projectId, { count, more });
  emit();
}

/** Runs of `projectId` landed: `runIds` as the nudge named them, and `more`
 *  when it named only some of them. One on the list already is not new. */
export function tracesArrived(projectId: string, runIds: readonly string[], more: boolean): void {
  let runs = landed.get(projectId);
  if (!runs) {
    runs = new Set();
    landed.set(projectId, runs);
  }
  const shown = onPage.get(projectId);
  for (const id of runIds) {
    if (shown?.has(id)) continue;
    if (runs.size >= LANDED_MAX) {
      floors.add(projectId);
      break;
    }
    runs.add(id);
  }
  if (more) floors.add(projectId);
  changed(projectId);
}

/** The list of `projectId` was drawn with `rows` on it, which are no longer
 *  new, `empty` when it had none. A `first` draw stands before nothing
 *  counted; after any other, what was counted and not drawn stands: a run
 *  the filters keep off the list, or one that landed while the draw was on
 *  its way. */
export function listDrawn(
  projectId: string,
  { rows, empty, first }: { rows: readonly string[]; empty: boolean; first: boolean },
): void {
  onPage.set(projectId, new Set(rows));
  if (empty) empties.add(projectId);
  else empties.delete(projectId);
  if (first) {
    landed.delete(projectId);
  } else {
    const runs = landed.get(projectId);
    if (runs) for (const id of rows) runs.delete(id);
  }
  floors.delete(projectId);
  changed(projectId);
}

/** Whether the list of `projectId`, as last drawn, had no rows. */
export function listIsEmpty(projectId: string): boolean {
  return empties.has(projectId);
}

/** The reader asked for the list: everything counted is on its way. */
export function tracesShown(projectId: string): void {
  landed.delete(projectId);
  floors.delete(projectId);
  changed(projectId);
}

/** What has landed for `projectId` since its list was drawn, read once. */
export function newTraces(projectId: string): NewTraces {
  return snapshots.get(projectId) ?? NONE;
}

/** What has landed for `projectId` since its list was drawn, as it changes. */
export function useNewTraces(projectId: string): NewTraces {
  return useSyncExternalStore(
    subscribe,
    () => newTraces(projectId),
    () => NONE,
  );
}
