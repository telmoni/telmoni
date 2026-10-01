"use client";

import { useRouter } from "next/navigation";
import { useState, useTransition } from "react";
import { toast } from "sonner";

import { DataTable, THead, Th, Td } from "@/components/data-table";
import { LocalTime } from "@/components/local-time";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Card } from "@/components/ui/card";
import type { ActiveSession } from "@/lib/server/data";

import { revokeSessionAction } from "./actions";

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
  const router = useRouter();
  const [pending, startTransition] = useTransition();
  const [busyId, setBusyId] = useState<string | null>(null);

  const revoke = (id: string) => {
    setBusyId(id);
    startTransition(async () => {
      const { error } = await revokeSessionAction(id);
      setBusyId(null);
      if (error) {
        toast.error(error);
        return;
      }
      toast.success("Session ended. That device signs out within a few minutes.");
      router.refresh();
    });
  };

  if (sessions === null) {
    return (
      <Card>
        <p className="text-sm text-muted-foreground">
          Couldn&rsquo;t load your sessions. Reload to try again.
        </p>
      </Card>
    );
  }

  return (
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
                {deviceLabel(session.user_agent)}
                {isCurrent ? (
                  <Badge variant="secondary" className="ml-2">
                    Current
                  </Badge>
                ) : null}
              </Td>
              <Td className="whitespace-nowrap text-muted-foreground">
                <LocalTime iso={session.created_at} />
              </Td>
              <Td className="whitespace-nowrap text-muted-foreground">
                <LocalTime iso={session.last_seen_at} />
              </Td>
              <Td className="text-right whitespace-nowrap">
                {isCurrent ? null : (
                  <Button
                    variant="destructive"
                    size="sm"
                    aria-label={`End session ${deviceLabel(session.user_agent)}`}
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
  );
}
