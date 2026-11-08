import type { Caps } from "@/lib/server/entities/limits";

/** Bytes in a megabyte, as the Limits page types and shows content. */
export const MB = 1_000_000;

/** The most each cap may hold, as the server's columns do: spans and content
 *  are 64-bit there and held to what a number here holds exactly; the rest
 *  are 32-bit counts. */
export const CAP_MOST: Record<keyof Caps, number> = {
  spans: Number.MAX_SAFE_INTEGER,
  content_bytes: Number.MAX_SAFE_INTEGER,
  alerts: 2_147_483_647,
  monitors: 2_147_483_647,
  objectives: 2_147_483_647,
  alert_deliveries_per_day: 2_147_483_647,
};

/** A cap as typed into its field: `null` for an empty one, which shares the
 *  pool; its number, in the server's units; or a sentence saying what is
 *  wrong. A count takes whole numbers alone; MB takes a fraction, to the
 *  byte. */
export function capTyped(
  field: { key: keyof Caps; label: string; unit?: number },
  typed: string,
): number | null | { refused: string } {
  const text = typed.trim();
  if (text === "") return null;
  const n = Number(text);
  if (!Number.isFinite(n) || n < 0) {
    return { refused: `${field.label}: a number, none or more, or empty to share the pool.` };
  }
  if (!field.unit && !Number.isInteger(n)) {
    return { refused: `${field.label}: a whole number.` };
  }
  const cap = field.unit ? Math.round(n * field.unit) : n;
  const most = CAP_MOST[field.key];
  if (cap > most) {
    const shown = field.unit ? Math.floor(most / field.unit) : most;
    return { refused: `${field.label}: at most ${shown.toLocaleString("en-US")}.` };
  }
  return cap;
}

/** Each cap a project may be held to, as the Limits page names it: content
 *  typed in MB. */
export const CAP_FIELDS: { key: keyof Caps; label: string; unit?: number }[] = [
  { key: "spans", label: "Spans a month" },
  { key: "content_bytes", label: "Content held, MB", unit: MB },
  { key: "alerts", label: "Alerts" },
  { key: "monitors", label: "Monitors" },
  { key: "objectives", label: "Objectives" },
  { key: "alert_deliveries_per_day", label: "Alert deliveries a day" },
];
