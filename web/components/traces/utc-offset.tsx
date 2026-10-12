"use client";

import { useEffect } from "react";
import { useRouter } from "next/navigation";

import { UTC_OFFSET_COOKIE, browserUtcOffset } from "@/lib/utc-offset";

/** A year: the cookie outlives a session, and is written again whenever the
 *  reader's clock moves, at a change of season or of country. */
const YEAR_SECONDS = 365 * 24 * 60 * 60;

/** Whether this page load has written the cookie already: a browser that
 *  keeps no cookies would otherwise be asked to draw the page again and
 *  again, for ever. */
let written = false;

// The reader's clock, left where the server reads it (`lib/utc-offset.ts`):
// where the offset the page was drawn with is not this browser's, the
// cookie is written and the page drawn again, once, with bars that start on
// the reader's own hours.
export function UtcOffsetCookie({ drawnWith }: { drawnWith: number }) {
  const router = useRouter();
  useEffect(() => {
    const offset = browserUtcOffset();
    if (offset === drawnWith || written) return;
    written = true;
    const secure = window.location.protocol === "https:" ? "; secure" : "";
    document.cookie = `${UTC_OFFSET_COOKIE}=${offset}; path=/; max-age=${YEAR_SECONDS}; samesite=lax${secure}`;
    router.refresh();
  }, [drawnWith, router]);
  return null;
}
