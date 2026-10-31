import { z } from "zod";

import type { AuditEvent } from "@/lib/server/entities/audit";
import { ResourceKind } from "@/lib/types/enums";

/** The audit rows a read of a project's content writes: `viewed` on the
 *  project, naming the traces read. */
export const CONTENT_READ = { action: "viewed", resourceKind: ResourceKind.Project } as const;

/** The most reads one page of the audit log answers. */
export const READS_MAX = 1_000;

const ReadDetails = z.object({ trace_ids: z.array(z.string()) });

/** One person's reads of a project's content. */
export interface ContentReader {
  actorId: string;
  reads: number;
  traces: number;
  last: string;
}

/** Each reader of `events`, newest first as the log answers them: how often they read,
 *  how many traces, and when they last did. */
export function contentReaders(events: readonly AuditEvent[]): ContentReader[] {
  const readers = new Map<string, { reads: number; traces: Set<string>; last: string }>();
  for (const event of events) {
    const reader = readers.get(event.actorId) ?? { reads: 0, traces: new Set(), last: "" };
    reader.reads += 1;
    for (const trace of ReadDetails.safeParse(event.metadata).data?.trace_ids ?? []) {
      reader.traces.add(trace);
    }
    // The log answers newest first, so a reader's first row is their last read.
    reader.last ||= event.createdAt;
    readers.set(event.actorId, reader);
  }
  return [...readers].map(([actorId, { reads, traces, last }]) => ({
    actorId,
    reads,
    traces: traces.size,
    last,
  }));
}
