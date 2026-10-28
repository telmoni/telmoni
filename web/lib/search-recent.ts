import { z } from "zod";

import { sameOriginUrl } from "@/lib/same-origin";

const STORAGE_KEY = "telmoni-recent-visits";

export const RECENT_LIMIT = 5;

// Storage is the browser's to change, so a link read back is followed only if
// it stays on this origin.
const isOwnPath = (href: string): boolean => sameOriginUrl(href) !== null;

const VisitSchema = z.object({
  href: z.string().refine(isOwnPath),
  label: z.string(),
  hint: z.string().optional(),
  at: z.number(),
});

// Kept with the person who made it, so nobody signing in after them sees it.
const TrailSchema = z.object({
  owner: z.string(),
  visits: z.array(VisitSchema),
});

type Trail = z.infer<typeof TrailSchema>;

export type RecentVisit = z.infer<typeof VisitSchema>;

export function readRecent(owner: string): RecentVisit[] {
  let raw: string | null;
  try {
    raw = window.localStorage.getItem(STORAGE_KEY);
  } catch {
    return memory?.owner === owner ? memory.visits : [];
  }
  if (!raw) return [];
  let body: unknown;
  try {
    body = JSON.parse(raw);
  } catch {
    return [];
  }
  const parsed = TrailSchema.safeParse(body);
  if (!parsed.success || parsed.data.owner !== owner) return [];
  return parsed.data.visits.slice(0, RECENT_LIMIT);
}

export function recordVisit(owner: string, visit: Omit<RecentVisit, "at">): void {
  if (!isOwnPath(visit.href)) return;
  const visits = [
    { ...visit, at: Date.now() },
    ...readRecent(owner).filter((v) => v.href !== visit.href),
  ].slice(0, RECENT_LIMIT);
  memory = { owner, visits };
  try {
    window.localStorage.setItem(STORAGE_KEY, JSON.stringify(memory));
  } catch {
  }
}

let memory: Trail | null = null;

/** Forgets every visit: a person signed out leaves nothing for the next. */
export function forgetRecent(): void {
  memory = null;
  try {
    window.localStorage.removeItem(STORAGE_KEY);
  } catch {
  }
}
