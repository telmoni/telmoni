"use client";

import { useSyncExternalStore } from "react";

export function LocalTime({
  iso,
  mode = "datetime",
  tz,
}: {
  iso: string;
  mode?: "date" | "datetime";
  tz?: string;
}) {
  const text = useSyncExternalStore(
    subscribeNever,
    () => localLabel(iso, mode),
    () => serverLabel(iso, mode, tz),
  );
  return (
    <time dateTime={iso} suppressHydrationWarning>
      {text}
    </time>
  );
}

const subscribeNever = () => () => {};

function localLabel(iso: string, mode: "date" | "datetime"): string {
  const d = new Date(iso);
  if (Number.isNaN(d.getTime())) return iso;
  return mode === "date" ? d.toLocaleDateString() : d.toLocaleString();
}

function serverLabel(
  iso: string,
  mode: "date" | "datetime",
  tz = "UTC",
): string {
  const d = new Date(iso);
  if (Number.isNaN(d.getTime())) return iso;
  if (tz === "UTC") {
    const date = d.toISOString().slice(0, 10);
    if (mode === "date") return date;
    return `${date} ${d.toISOString().slice(11, 16)} UTC`;
  }
  try {
    return new Intl.DateTimeFormat("en-CA", {
      timeZone: tz,
      dateStyle: "short",
      ...(mode === "datetime" ? { timeStyle: "short", timeZoneName: "short" } : {}),
    }).format(d);
  } catch {
    return serverLabel(iso, mode);
  }
}
