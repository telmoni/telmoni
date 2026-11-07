"use client";

import { startTransition, useState } from "react";

import { DataTable, THead, Th, Td } from "@/components/data-table";
import { LocalTime } from "@/components/local-time";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import type { ActiveSession } from "@/lib/server/data";
import { useToastAction } from "@/lib/use-action";

import { revokeOtherSessionsAction, revokeSessionAction } from "./actions";

export function deviceLabel(userAgent: string | null): string {
  if (!userAgent) return "Unknown device";
  const ua = userAgent;
  // The CLI names itself `telmoni-cli/<version> (<os>; <arch>)`, with the
  // os as Rust spells it; a session it recorded is a terminal, not a browser.
  const cli = /^telmoni-cli\/\S+(?:\s+\((\w+)[;)])?/.exec(ua);
  if (cli) {
    const os = { macos: "macOS", linux: "Linux", windows: "Windows" }[cli[1] ?? ""];
    return os ? `Telmoni CLI (${os})` : "Telmoni CLI";
  }
  const browser = /Edg\//.test(ua)
    ? "Edge"
    : /OPR\//.test(ua)
      ? "Opera"
      : /Chrome\//.test(ua)
        ? "Chrome"
        : /Firefox\//.test(ua)
          ? "Firefox"
          : /Safari\//.test(ua)
            ? "Safari"
            : null;
  const os = /Android/.test(ua)
    ? "Android"
    : /(iPhone|iPad|iPod)/.test(ua)
      ? "iOS"
      : /Mac OS X|Macintosh/.test(ua)
        ? "macOS"
        : /Windows/.test(ua)
          ? "Windows"
          : /Linux/.test(ua)
            ? "Linux"
            : null;
  if (!browser && !os) return "Unknown device";
  if (!browser) return os ?? "Unknown device";
  return os ? `${browser} (${os})` : browser;
}

export function ActiveSessions({
  sessions,
  currentId,
}: {
  sessions: ActiveSession[] | null;
  currentId: string | null;
}) {
  const { pending, run } = useToastAction();
  const [busyId, setBusyId] = useState<string | null>(null);

  // Not busy once the action settles, however: in the transition, with the
  // list the action drew again, so the row goes as it stops being busy.
  const settle = <T,>(action: () => Promise<T>) => async () => {
    try {
      return await action();
    } finally {
      startTransition(() => setBusyId(null));
    }
  };

  const revoke = (id: string) => {
    setBusyId(id);
    run(
      settle(() => revokeSessionAction(id)),
      "Session ended. That device signs out within a few minutes.",
    );
  };

  const revokeOthers = () => {
    setBusyId("others");
    run(
      settle(() => revokeOtherSessionsAction()),
      "Every other session ended. Those devices sign out within a few minutes.",
    );
  };

  if (sessions === null) {
    return (
      <p className="text-muted-foreground">
        Couldn&rsquo;t load your sessions. Reload to try again.
      </p>
    );
  }

  const others = sessions.some((session) => session.id !== currentId);

  return (
    <div className="grid gap-gutter">
      {others && (
        <div>
          <Button
            variant="destructive"
            size="sm"
            disabled={pending && busyId === "others"}
            onClick={revokeOthers}
          >
            Sign out everywhere else
          </Button>
        </div>
      )}
      <DataTable>
        <THead>
          <Th>Device</Th>
          <Th>Started</Th>
          <Th>Last seen</Th>
          <Th className="w-10" />
        </THead>
        <tbody>
          {sessions.map((session) => {
            const isCurrent = session.id === currentId;
            return (
              <tr key={session.id}>
                <Td className="whitespace-nowrap">
                  {deviceLabel(session.userAgent)}
                  {isCurrent ? (
                    <Badge variant="secondary" className="ml-2">
                      Current
                    </Badge>
                  ) : null}
                </Td>
                <Td className="whitespace-nowrap text-muted-foreground">
                  <LocalTime iso={session.createdAt} />
                </Td>
                <Td className="whitespace-nowrap text-muted-foreground">
                  <LocalTime iso={session.lastSeenAt} />
                </Td>
                <Td className="text-right whitespace-nowrap">
                  {isCurrent ? null : (
                    <Button
                      variant="destructive"
                      size="sm"
                      aria-label={`End session ${deviceLabel(session.userAgent)}`}
                      disabled={pending && busyId === session.id}
                      onClick={() => revoke(session.id)}
                    >
                      End session
                    </Button>
                  )}
                </Td>
              </tr>
            );
          })}
        </tbody>
      </DataTable>
    </div>
  );
}
