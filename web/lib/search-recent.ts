import { z } from "zod";

const STORAGE_KEY = "telmoni-recent-visits";

export const RECENT_LIMIT = 5;

const VisitSchema = z.object({
  href: z.string(),
  label: z.string(),
  hint: z.string().optional(),
  at: z.number(),
});

const TrailSchema = z.array(VisitSchema);

export type RecentVisit = z.infer<typeof VisitSchema>;

export function readRecent(): RecentVisit[] {
  let raw: string | null;
  try {
    raw = window.localStorage.getItem(STORAGE_KEY);
  } catch {
    return memory;
  }
  if (!raw) return [];
  let body: unknown;
  try {
    body = JSON.parse(raw);
  } catch {
    return [];
  }
  const parsed = TrailSchema.safeParse(body);
  if (!parsed.success) return [];
  return parsed.data.slice(0, RECENT_LIMIT);
}

export function recordVisit(visit: Omit<RecentVisit, "at">): void {
  const trail = [
    { ...visit, at: Date.now() },
    ...readRecent().filter((v) => v.href !== visit.href),
  ].slice(0, RECENT_LIMIT);
  memory = trail;
  try {
    window.localStorage.setItem(STORAGE_KEY, JSON.stringify(trail));
  } catch {
  }
}

let memory: RecentVisit[] = [];

export function clearRecentForTest(): void {
  memory = [];
  try {
    window.localStorage.removeItem(STORAGE_KEY);
  } catch {
  }
}
