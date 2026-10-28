
import { z } from "zod";

import type { RecentVisit } from "@/lib/search-recent";
import { projectPath } from "@/lib/slug";

export const SEARCH_KINDS = ["recent", "project", "key", "member", "docs"] as const;
export type SearchKind = (typeof SEARCH_KINDS)[number];

export const SearchItemSchema = z.object({
  id: z.string(),
  kind: z.enum(SEARCH_KINDS),
  label: z.string(),
  hint: z.string().optional(),
  href: z.string(),
  external: z.boolean().optional(),
});
export type SearchItem = z.infer<typeof SearchItemSchema>;

/** One of a project's sections in the palette: its items, or why there are
 *  none — the reader may not see them, or they could not be read. */
export const SearchSectionSchema = z.object({
  items: z.array(SearchItemSchema),
  forbidden: z.boolean(),
  unavailable: z.boolean(),
});
export type SearchSection = z.infer<typeof SearchSectionSchema>;

/** What the palette lists of the project it is opened on, as
 *  `/api/v1/search/index` answers it. */
export const SearchIndexSchema = z.object({
  keys: SearchSectionSchema,
  members: SearchSectionSchema,
});
export type SearchIndex = z.infer<typeof SearchIndexSchema>;

/** What the palette lists, in the order its groups show: where the person
 *  was lately, each placed by `placeOf` where it names a place; the projects
 *  of `organization`; and the index's sections, read of the project the
 *  palette opened on. */
export function searchItems({
  recent,
  organization,
  projects,
  index,
  placeOf,
}: {
  recent: readonly RecentVisit[];
  organization: string | null;
  projects: readonly { id: string; slug: string; name: string }[];
  index: SearchIndex | null;
  placeOf: (href: string) => string | undefined;
}): SearchItem[] {
  const out: SearchItem[] = recent.map((visit) => ({
    id: `recent:${visit.href}`,
    kind: "recent",
    label: visit.label,
    hint: placeOf(visit.href) ?? visit.hint,
    href: visit.href,
  }));
  if (organization) {
    for (const project of projects) {
      out.push({
        id: `project:${project.id}`,
        kind: "project",
        label: project.name,
        hint: project.id,
        href: projectPath(organization, project.slug),
      });
    }
  }
  if (index) out.push(...index.keys.items, ...index.members.items);
  return out;
}

/** Names as a sentence lists them: "a", "a and b", "a, b and c". */
export function listNames(names: readonly string[]): string {
  if (names.length <= 1) return names[0] ?? "";
  return `${names.slice(0, -1).join(", ")} and ${names[names.length - 1]}`;
}

const SECTIONS: readonly (readonly [string, (index: SearchIndex) => SearchSection])[] = [
  ["API keys", (index) => index.keys],
  ["members", (index) => index.members],
];

/** What the palette could not search of the index, named: the sections the
 *  reader's role may not list, and the ones that could not be read; empty
 *  where every section was. */
export function unsearched(index: SearchIndex): { refused: string; broken: string } {
  const named = (of: (section: SearchSection) => boolean) =>
    listNames(SECTIONS.filter(([, pick]) => of(pick(index))).map(([name]) => name));
  return { refused: named((s) => s.forbidden), broken: named((s) => s.unavailable) };
}

export const GROUP_ORDER: readonly SearchKind[] = [
  "recent",
  "project",
  "key",
  "member",
  "docs",
];

export const GROUP_LABELS: Record<SearchKind, string> = {
  recent: "Recently visited",
  project: "Projects",
  key: "API keys",
  member: "Members",
  docs: "Documentation",
};

export function rankItem(item: SearchItem, query: string): number | null {
  const q = query.trim().toLowerCase();
  if (q === "") return 0;

  const label = item.label.toLowerCase();
  if (label.startsWith(q)) return 0;
  if (new RegExp(`[\\s\\-_./@]${escapeRegExp(q)}`).test(label)) return 1;
  if (label.includes(q)) return 2;
  if (item.hint?.toLowerCase().includes(q)) return 3;
  return null;
}

export function filterItems(
  items: readonly SearchItem[],
  query: string,
): SearchItem[] {
  const ranked: { item: SearchItem; rank: number }[] = [];
  for (const item of items) {
    const rank = rankItem(item, query);
    if (rank !== null) ranked.push({ item, rank });
  }
  ranked.sort((a, b) => a.rank - b.rank);
  return ranked.map((r) => r.item);
}

export function groupItems(
  items: readonly SearchItem[],
  query: string,
): { kind: SearchKind; label: string; items: SearchItem[] }[] {
  const hits = filterItems(items, query);
  const groups: { kind: SearchKind; label: string; items: SearchItem[] }[] = [];
  for (const kind of GROUP_ORDER) {
    const of = hits.filter((i) => i.kind === kind);
    if (of.length > 0) {
      groups.push({ kind, label: GROUP_LABELS[kind], items: of });
    }
  }
  return groups;
}

function escapeRegExp(value: string): string {
  return value.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
}
