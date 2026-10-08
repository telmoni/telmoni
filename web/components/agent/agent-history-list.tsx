"use client";

import { MessageSquare, Trash2 } from "lucide-react";
import { LocalTime } from "@/components/local-time";
import type { AgentConversationSummary } from "@/lib/types/agent";
import { cn } from "@/lib/utils";

export const ICON_BUTTON =
  "flex size-8 shrink-0 cursor-pointer items-center justify-center rounded-md text-muted-foreground hover:bg-accent hover:text-accent-foreground disabled:cursor-not-allowed disabled:opacity-50";

export type HistoryState =
  | { kind: "loading" }
  | { kind: "ok"; conversations: AgentConversationSummary[] }
  | { kind: "error"; message: string };

export function Empty({ children }: { children: React.ReactNode }) {
  return <p className="text-sm text-muted-foreground">{children}</p>;
}

export function HistoryList({
  history,
  activeId,
  onOpen,
  onDelete,
}: {
  history: HistoryState;
  activeId: string | null;
  onOpen: (id: string) => void;
  onDelete: (id: string) => void;
}) {
  if (history.kind === "loading") return <Empty>Loading your conversations…</Empty>;
  if (history.kind === "error") return <Empty>{history.message}</Empty>;
  if (history.conversations.length === 0) {
    return <Empty>No conversations in this project yet.</Empty>;
  }
  return (
    <ul aria-label="Your conversations" className="grid gap-1">
      {history.conversations.map((c) => (
        <li key={c.id} className="flex items-center justify-between gap-3">
          <button
            type="button"
            onClick={() => onOpen(c.id)}
            aria-current={c.id === activeId ? "true" : undefined}
            className={cn(
              "flex min-w-0 flex-1 cursor-pointer items-center rounded-md gap-2.5 px-3 py-2 text-left hover:bg-accent",
              c.id === activeId && "bg-accent",
            )}
          >
            <MessageSquare aria-hidden className="size-4 shrink-0 text-muted-foreground" />
            <span className="grid min-w-0 gap-0.5">
              <span className="truncate text-sm">{c.title || "Untitled conversation"}</span>
              <span className="truncate text-xs text-muted-foreground">
                <LocalTime iso={c.updatedAt} />
              </span>
            </span>
          </button>
          <button
            type="button"
            aria-label={`Delete “${c.title || "Untitled conversation"}”`}
            title="Delete"
            onClick={() => onDelete(c.id)}
            className={ICON_BUTTON}
          >
            <Trash2 className="size-4" />
          </button>
        </li>
      ))}
    </ul>
  );
}
