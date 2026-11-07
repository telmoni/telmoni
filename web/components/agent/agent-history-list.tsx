import { MessageSquare, Trash2 } from "lucide-react";
import { ConfirmDialog } from "@/components/confirm-dialog";
import { LocalTime } from "@/components/local-time";
import type { AgentConversationSummary } from "@/lib/types/agent";
import { cn } from "@/lib/utils";

export const ICON_BUTTON =
  "flex size-control shrink-0 cursor-pointer items-center justify-center rounded-md text-muted-foreground hover:bg-accent hover:text-accent-foreground disabled:cursor-not-allowed disabled:opacity-50";

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
  if (history.conversations.length === 0) return <Empty>No conversations yet.</Empty>;

  return (
    <ul className="flex flex-col gap-0.5">
      {history.conversations.map((item) => (
        <HistoryItem
          key={item.id}
          item={item}
          isActive={item.id === activeId}
          onOpen={() => onOpen(item.id)}
          onDelete={() => onDelete(item.id)}
        />
      ))}
    </ul>
  );
}

function HistoryItem({
  item,
  isActive,
  onOpen,
  onDelete,
}: {
  item: AgentConversationSummary;
  isActive: boolean;
  onOpen: () => void;
  onDelete: () => void;
}) {
  return (
    <li
      className={cn(
        "group flex items-center justify-between gap-2 rounded-md px-2 py-1.5 text-sm transition-colors",
        isActive ? "bg-accent text-accent-foreground" : "hover:bg-accent/50",
      )}
    >
      <button
        type="button"
        onClick={onOpen}
        className="flex min-w-0 flex-1 items-center gap-2 text-left"
      >
        <MessageSquare className="size-4 shrink-0 text-muted-foreground" />
        <span className="truncate">{item.title || "Untitled conversation"}</span>
        <span className="shrink-0 text-xs text-muted-foreground">
          <LocalTime iso={item.updatedAt} />
        </span>
      </button>

      <ConfirmDialog
        title="Delete conversation?"
        description="This will permanently delete this conversation and its messages."
        confirmLabel="Delete"
        destructive
        onConfirm={onDelete}
        trigger={
          <button
            type="button"
            aria-label="Delete conversation"
            className={cn(ICON_BUTTON, "opacity-0 group-hover:opacity-100 focus-visible:opacity-100")}
          >
            <Trash2 className="size-3.5" />
          </button>
        }
      />
    </li>
  );
}
