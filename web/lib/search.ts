
export type SearchKind =
  | "recent"
  | "project"
  | "key"
  | "member"
  | "docs";

export interface SearchItem {
  id: string;
  kind: SearchKind;
  label: string;
  hint?: string;
  href: string;
  external?: boolean;
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
