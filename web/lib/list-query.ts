import { z } from "zod";

import { isOneOf } from "@/lib/one-of";
import { projectPath } from "@/lib/slug";
import { SortDirection } from "@/lib/types/enums";

// A list's URL is its state: the sort, the window and every filter are query
// parameters, so a link holds the view and a page sent to a colleague opens
// as it was left. The traces, users and sessions pages each read theirs with
// what is here — one reading of a parameter, one bound on the window, one way
// a column heading sorts — and keep to themselves only the columns they have.

/** The furthest back a list reads, in days: the longest retention sold. */
export const MAX_DAYS = 360;
/** The most rows one page asks for. */
export const MAX_LIMIT = 500;
/** The windows a page offers, in days. */
export const WINDOWS = [1, 7, 30, 90] as const;

/** A window of days as a page names it. */
export function windowLabel(days: number): string {
  return days === 1 ? "Last day" : `Last ${days} days`;
}

/** The windows a page offers, and the one in force among them: a window the
 *  URL asked for by hand is offered too, so the picker says what is shown. */
export function windowsOffered(days: number): number[] {
  return (WINDOWS as readonly number[]).includes(days)
    ? [...WINDOWS]
    : [...WINDOWS, days].sort((a, b) => a - b);
}
/** The week a list opens on when its URL names no window, as the server's. */
export const OPENING_DAYS = 7;

/** The narrowest window a page offers that reaches back to `iso`, or none
 *  where the week a list opens on already does: a link from a run to its
 *  user's traces or its session then finds the run it came from, and
 *  the runs beside it, however old it is. */
export function windowReaching(iso: string, now: number = Date.now()): number | undefined {
  const age = Math.ceil((now - Date.parse(iso)) / 86_400_000);
  if (age <= OPENING_DAYS) return undefined;
  return WINDOWS.find((w) => w >= age) ?? MAX_DAYS;
}

/** Next's `searchParams`, as a page receives them. */
export type SearchParams = Record<string, string | string[] | undefined>;

/** What bounds a page: a key left out asks for the server's default. */
export type WindowQuery = { days?: number; limit?: number };

/** A parameter's one value, trimmed: the first where the URL repeats it,
 *  none where it is empty. */
export function one(value: string | string[] | undefined): string | undefined {
  const text = Array.isArray(value) ? value[0] : value;
  const trimmed = text?.trim();
  return trimmed ? trimmed : undefined;
}

/** A parameter's one value, as the sign-in pages read `state`: a repeated
 *  parameter is nothing, and nothing is trimmed. */
export function sole(value: string | string[] | undefined): string | undefined {
  return typeof value === "string" && value.length > 0 ? value : undefined;
}

/** The most values one filter carries, as the server admits them. */
export const FILTER_VALUES_MAX = 50;

/** A filter's values as the URL repeats it, each trimmed and once, an empty
 *  one dropped; past `max`, the rest are let go rather than turning the page
 *  into a refusal. */
export function many(
  value: string | readonly string[] | undefined,
  max = FILTER_VALUES_MAX,
): string[] {
  const given: readonly string[] = Array.isArray(value) ? value : value === undefined ? [] : [value];
  const kept: string[] = [];
  for (const text of given) {
    const trimmed = text.trim();
    if (trimmed && !kept.includes(trimmed)) kept.push(trimmed);
  }
  return kept.slice(0, max);
}

/** Whether two lists hold the same values, in any order. */
export function sameValues(a: readonly string[] = [], b: readonly string[] = []): boolean {
  return [...a].sort().join("\u0000") === [...b].sort().join("\u0000");
}

/** Whether `value` is spelled as a UUID, whatever its version. */
export const isUuid = (value: string): boolean => z.guid().safeParse(value).success;

/** A whole number from 1 to `max`, or none: a day count or a page's limit. */
export function whole(value: string | string[] | undefined, max: number): number | undefined {
  const text = one(value);
  if (text === undefined || !/^\d+$/.test(text)) return undefined;
  const n = Number(text);
  return n >= 1 && n <= max ? n : undefined;
}

/** A parameter that must be a word of a closed vocabulary — a sort, a
 *  direction, a status — or none. */
export function oneOf<T extends string>(
  value: string | string[] | undefined,
  vocabulary: Record<string, T>,
): T | undefined {
  const text = one(value);
  return isOneOf(Object.values(vocabulary), text) ? text : undefined;
}

/** The window the URL carries, every value the server would refuse dropped. */
export function parseWindow(searchParams: SearchParams): WindowQuery {
  return compact({
    days: whole(searchParams.days, MAX_DAYS),
    limit: whole(searchParams.limit, MAX_LIMIT),
  });
}

/** `query` without the keys that are not set, so it reads as what the URL
 *  carried and nothing else. */
export function compact<Q extends object>(query: Q): Q {
  for (const key of Object.keys(query) as (keyof Q)[]) {
    if (query[key] === undefined) delete query[key];
  }
  return query;
}

/** One parameter's value as a URL or a lane takes it: a filter's values are
 *  the parameter given again, once each. */
export type QueryValue = string | number | readonly string[] | undefined;

/** The query string a lane or a path takes, with only the keys that are set. */
export function queryString(query: Record<string, QueryValue>): string {
  const params = new URLSearchParams();
  for (const [key, value] of Object.entries(query)) {
    if (value === undefined || value === "") continue;
    if (typeof value === "object") {
      for (const each of value) if (each !== "") params.append(key, each);
      continue;
    }
    params.set(key, String(value));
  }
  const text = params.toString();
  return text ? `?${text}` : "";
}

/** A list's path with `query` in its URL: `patch` changes some keys, and a
 *  key patched to `undefined` is dropped. */
export function listHref(
  organization: string,
  project: string,
  page: string,
  query: Record<string, QueryValue>,
  patch: Record<string, QueryValue> = {},
): string {
  return projectPath(organization, project, `${page}${queryString({ ...query, ...patch })}`);
}

/** A column a list sorts by: its heading, and the way it sorts when first
 *  clicked. */
export type Column<S extends string> = { sort: S; label: string; first: SortDirection };

/** The sort in force: the URL's, else `fallback`, each column its own way
 *  unless the URL says which. */
export function sortInForce<S extends string>(
  columns: readonly Column<S>[],
  fallback: S,
  query: { sort?: S; dir?: SortDirection },
): { sort: S; dir: SortDirection } {
  const sort = query.sort ?? fallback;
  const dir = query.dir ?? columns.find((c) => c.sort === sort)?.first ?? SortDirection.Desc;
  return { sort, dir };
}

/** What clicking a column heading asks for: the other way when the list is
 *  already sorted by it, its own first way otherwise. */
export function sortAfterClick<S extends string>(
  columns: readonly Column<S>[],
  fallback: S,
  query: { sort?: S; dir?: SortDirection },
  column: S,
): { sort: S; dir: SortDirection } {
  const current = sortInForce(columns, fallback, query);
  if (current.sort === column) {
    return {
      sort: column,
      dir: current.dir === SortDirection.Asc ? SortDirection.Desc : SortDirection.Asc,
    };
  }
  return {
    sort: column,
    dir: columns.find((c) => c.sort === column)?.first ?? SortDirection.Desc,
  };
}
