"use client";

import Link from "next/link";
import type { ReactNode } from "react";

import { plainClick } from "@/components/traces/list-table";
import { openPeek } from "@/lib/peek";

/** A session's id in the list: a plain click opens the session beside the
 *  list (`session-peek.tsx`); one that asks for a new tab or window, or a
 *  copy of the link, gets the session's own page, as the link says — as a
 *  trace's row opens the trace list's peek (`table.tsx`). */
export function SessionLink({
  sessionId,
  href,
  children,
}: {
  sessionId: string;
  href: string;
  children: ReactNode;
}) {
  return (
    <Link
      href={href}
      data-peek-session={sessionId}
      onClick={(event) => {
        if (!plainClick(event)) return;
        event.preventDefault();
        openPeek(sessionId);
      }}
      className="underline-offset-4 hover:underline"
    >
      {children}
    </Link>
  );
}
