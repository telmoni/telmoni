import type { Caps } from "@/lib/server/entities/limits";

/** Bytes in a megabyte, as the Limits page types and shows content. */
export const MB = 1_000_000;

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
