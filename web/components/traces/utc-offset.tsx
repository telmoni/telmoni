"use client";

import { useEffect } from "react";
import { useRouter } from "next/navigation";

import { writeCookie } from "@/lib/cookie";
import { UTC_OFFSET_COOKIE, browserUtcOffset } from "@/lib/utc-offset";

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
    writeCookie(UTC_OFFSET_COOKIE, String(offset));
    router.refresh();
  }, [drawnWith, router]);
  return null;
}
