// The data sheet's readings of the chain and the stores: each project's
// content-mode changes, who read its content, and how many of its spans
// arrived scrubbed, drawn from what the server answers and never written by
// hand (`app/(app)/[organization]/data-sheet/`).

import { z } from "zod";

import { SCRUBBED } from "@/lib/messages";
import type { AuditEvent, PrivacyPosture } from "@/lib/server/data";

/** The days back a data sheet reads who read each project's content. */
export const READS_DAYS = 90;

/** Before every project: the start a read of a project's whole history asks for. */
export const EVERY_CHANGE_SINCE = "1970-01-01T00:00:00Z";

/** One change of a project's content mode, as the audit chain filed it. */
export type ModeChange = { at: string; from: string; to: string; by: string; deleted: boolean };

const ModeChangeSchema = z.object({
  content_mode: z.object({ from: z.string(), to: z.string() }),
  content_deleted: z.boolean().optional(),
});

/** The content-mode changes among a project's `events`, oldest first: the
 *  chain's other changes to it — its retention, its name — passed over. */
export function modeChanges(events: readonly AuditEvent[]): ModeChange[] {
  return events
    .flatMap((event) => {
      const parsed = ModeChangeSchema.safeParse(event.metadata);
      if (!parsed.success) return [];
      const { content_mode, content_deleted } = parsed.data;
      return [
        {
          at: event.createdAt,
          from: content_mode.from,
          to: content_mode.to,
          by: event.actorId,
          deleted: content_deleted === true,
        },
      ];
    })
    .sort((a, b) => (a.at < b.at ? -1 : a.at > b.at ? 1 : 0));
}

/** One reader of a project's content: who, how many reads, and the last. */
export type Reader = { who: string; reads: number; last: string };

/** Who read each project's content among `events`, by project, the latest
 *  reader first: each read of content the chain filed, a person's or a key's. */
export function readersByProject(events: readonly AuditEvent[]): Map<string, Reader[]> {
  const byProject = new Map<string, Map<string, Reader>>();
  for (const event of events) {
    if (event.action !== "viewed" || event.resourceKind !== "project" || !event.inProject) continue;
    const readers = byProject.get(event.inProject) ?? new Map<string, Reader>();
    const reader = readers.get(event.actorId) ?? { who: event.actorId, reads: 0, last: event.createdAt };
    reader.reads += 1;
    if (event.createdAt > reader.last) reader.last = event.createdAt;
    readers.set(event.actorId, reader);
    byProject.set(event.inProject, readers);
  }
  return new Map(
    [...byProject].map(([project, readers]) => [
      project,
      [...readers.values()].sort((a, b) => (a.last < b.last ? 1 : a.last > b.last ? -1 : 0)),
    ]),
  );
}

/** How many of the spans the posture counted arrived scrubbed by an SDK. */
export function scrubbedSpans(posture: PrivacyPosture): number {
  return posture.sent.find((row) => row.attribute === SCRUBBED)?.spans ?? 0;
}

/** What each answer to whether a field can carry a person says. */
export const PERSON_LABEL = {
  no: "No",
  if_sent: "Only if your code puts one there",
  hashed: "Only as the project's hash of it",
  content: "Yes: it is content",
} as const;

/** A reader as the sheet names them: a member by name, a key by its id. */
export function readerName(who: string, name: (id: string) => string): string {
  return who.startsWith("key:") ? `API key ${who.slice("key:".length)}` : name(who);
}
