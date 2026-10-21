"use client";

import { createContext, useContext, useEffect, useSyncExternalStore } from "react";

import {
  LOCAL_TIME_COOKIE,
  type Reader,
  type TimeMode,
  browserReader,
  formatTime,
  formatUtc,
  readerCookie,
} from "@/lib/local-time";

/** The zone and locale the server wrote the page's times in, from the
 *  browser's cookie; none before the browser has left one. */
const ServerReader = createContext<Reader | null>(null);

/** A year: the cookie outlives a session, and is written again whenever the
 *  reader's zone or locale changes. */
const YEAR_SECONDS = 365 * 24 * 60 * 60;

// The zone and locale the server read from the cookie (`lib/local-time.ts`),
// for every time below; where they are not this browser's, the cookie is
// written for the next page the server draws.
export function LocalTimeReader({
  reader,
  children,
}: {
  reader: Reader | null;
  children: React.ReactNode;
}) {
  const timeZone = reader?.timeZone;
  const locale = reader?.locale;
  useEffect(() => {
    const browser = browserReader();
    if (browser.timeZone === timeZone && browser.locale === locale) return;
    const secure = window.location.protocol === "https:" ? "; secure" : "";
    document.cookie = `${LOCAL_TIME_COOKIE}=${readerCookie(browser)}; path=/; max-age=${YEAR_SECONDS}; samesite=lax${secure}`;
  }, [timeZone, locale]);
  return <ServerReader.Provider value={reader}>{children}</ServerReader.Provider>;
}

export function LocalTime({ iso, mode = "datetime" }: { iso: string; mode?: TimeMode }) {
  const reader = useContext(ServerReader);
  const text = useSyncExternalStore(
    subscribeNever,
    () => browserLabel(iso, mode),
    () => serverLabel(iso, mode, reader),
  );
  return (
    <time dateTime={iso} suppressHydrationWarning>
      {text}
    </time>
  );
}

const subscribeNever = () => () => {};

function browserLabel(iso: string, mode: TimeMode): string {
  const d = new Date(iso);
  return Number.isNaN(d.getTime()) ? iso : formatTime(d, mode);
}

/** Also the label the page hydrates with, so it must be the server's own. */
function serverLabel(iso: string, mode: TimeMode, reader: Reader | null): string {
  const d = new Date(iso);
  if (Number.isNaN(d.getTime())) return iso;
  return reader ? formatTime(d, mode, reader) : formatUtc(d, mode);
}
