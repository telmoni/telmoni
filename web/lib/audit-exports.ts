import { organizationPath } from "@/lib/slug";
import { AUDIT_EXPORT_MAX_ROWS, type AuditExport } from "@/lib/types/audit-export";

/// The ranges the export dialog offers: the last week, month or quarter up to
/// now, the whole chain, or two days the person picks.
export type ExportRange = "7d" | "30d" | "90d" | "all" | "custom";

const DAY_MS = 24 * 60 * 60 * 1000;

const LAST_DAYS = { "7d": 7, "30d": 30, "90d": 90 } as const;

/// The instants a range asks auth for — `null` for the chain's start, and for
/// now — or `null` for a custom range not yet whole: both days given, the first
/// no later than the last and no later than today. A custom range runs from
/// the start of its first day to the end of its last, in the person's own
/// time zone, since those are the days they picked.
export function exportWindow(
  range: ExportRange,
  fromDay: string,
  toDay: string,
  now: Date,
): { from: string | null; to: string | null } | null {
  if (range === "all") return { from: null, to: null };
  if (range !== "custom") {
    return { from: new Date(now.getTime() - LAST_DAYS[range] * DAY_MS).toISOString(), to: null };
  }
  const first = localDay(fromDay);
  const last = localDay(toDay);
  if (!first || !last || first > last || first > now) return null;
  const end = new Date(last.getFullYear(), last.getMonth(), last.getDate() + 1);
  return { from: first.toISOString(), to: end > now ? null : end.toISOString() };
}

/// A date field's `yyyy-mm-dd`, as midnight in the person's own time zone.
function localDay(day: string): Date | null {
  const match = /^(\d{4})-(\d{2})-(\d{2})$/.exec(day);
  if (!match) return null;
  const date = new Date(Number(match[1]), Number(match[2]) - 1, Number(match[3]));
  return Number.isNaN(date.getTime()) ? null : date;
}

/// A day as a date field spells it, in the person's own time zone.
export function dayField(date: Date): string {
  const pad = (n: number) => String(n).padStart(2, "0");
  return `${date.getFullYear()}-${pad(date.getMonth() + 1)}-${pad(date.getDate())}`;
}

const DAY = new Intl.DateTimeFormat(undefined, { month: "short", day: "numeric" });
const DAY_AND_YEAR = new Intl.DateTimeFormat(undefined, {
  month: "short",
  day: "numeric",
  year: "numeric",
});

function dayLabel(date: Date, now: Date): string {
  return (date.getFullYear() === now.getFullYear() ? DAY : DAY_AND_YEAR).format(date);
}

/// What an export covers, as people say it: `Sep 8 – Oct 8`, or `Everything
/// to Oct 8`. The end is exclusive, so its last moment names the day: a
/// custom range through Oct 7 ends at midnight on the 8th.
export function rangeLabel(
  entry: Pick<AuditExport, "range_from" | "range_to">,
  now: Date,
): string {
  const to = dayLabel(new Date(new Date(entry.range_to).getTime() - 1), now);
  if (!entry.range_from) return `Everything to ${to}`;
  const from = dayLabel(new Date(entry.range_from), now);
  return from === to ? from : `${from} – ${to}`;
}

const COUNT = new Intl.NumberFormat();

export function eventsLabel(count: number): string {
  return count === 1 ? "1 event" : `${COUNT.format(count)} events`;
}

/// The line under an export's name: its format, its range and, once built,
/// how many events it holds.
export function exportSummary(entry: AuditExport, now: Date): string {
  const parts = [entry.format.toUpperCase(), rangeLabel(entry, now)];
  if (entry.status === "ready" && entry.row_count !== null) {
    parts.push(eventsLabel(entry.row_count));
  }
  return parts.join(" · ");
}

/// Why one failed, and what to do instead. `too_large` is either cap — the
/// events or the bytes their details come to — and a shorter range is the
/// answer to both.
export function failureText(entry: Pick<AuditExport, "failure">): string {
  return entry.failure === "too_large"
    ? `That range is too much for one file, which holds up to ${COUNT.format(
        AUDIT_EXPORT_MAX_ROWS,
      )} events and 24 MiB. Export a shorter one.`
    : "The audit log export failed. Try again.";
}

/// Where the export dialog starts one and the bell reads the list.
export function exportsHref(organizationSlug: string): string {
  return organizationPath(organizationSlug, "/audit-log/exports");
}

/// Where a finished file downloads from.
export function exportHref(organizationSlug: string, id: string): string {
  return organizationPath(organizationSlug, `/audit-log/exports/${encodeURIComponent(id)}`);
}
