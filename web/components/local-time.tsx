"use client";

import { useSyncExternalStore } from "react";

/** `precise` is to the millisecond: a span's start, where two calls a few
 *  milliseconds apart must read apart. */
type Mode = "date" | "datetime" | "precise";

/** The fields a precise time is written in: `toLocaleString`'s own, so it
 *  reads as every other time in the console does, and the milliseconds. */
const PRECISE: Intl.DateTimeFormatOptions = {
  year: "numeric",
  month: "numeric",
  day: "numeric",
  hour: "numeric",
  minute: "2-digit",
  second: "2-digit",
  fractionalSecondDigits: 3,
};

export function LocalTime({
  iso,
  mode = "datetime",
  tz,
}: {
  iso: string;
  mode?: Mode;
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

function localLabel(iso: string, mode: Mode): string {
  const d = new Date(iso);
  if (Number.isNaN(d.getTime())) return iso;
  if (mode === "precise") return d.toLocaleString(undefined, PRECISE);
  return mode === "date" ? d.toLocaleDateString() : d.toLocaleString();
}

function serverLabel(
  iso: string,
  mode: Mode,
  tz = "UTC",
): string {
  const d = new Date(iso);
  if (Number.isNaN(d.getTime())) return iso;
  if (tz === "UTC") {
    const date = d.toISOString().slice(0, 10);
    if (mode === "date") return date;
    return `${date} ${d.toISOString().slice(11, mode === "precise" ? 23 : 16)} UTC`;
  }
  try {
    if (mode === "precise") {
      return new Intl.DateTimeFormat("en-CA", { ...PRECISE, timeZone: tz, timeZoneName: "short" }).format(d);
    }
    return new Intl.DateTimeFormat("en-CA", {
      timeZone: tz,
      dateStyle: "short",
      ...(mode === "datetime" ? { timeStyle: "short", timeZoneName: "short" } : {}),
    }).format(d);
  } catch {
    return serverLabel(iso, mode);
  }
}
