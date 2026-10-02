"use client";

import { Bot } from "lucide-react";

import { useConsoleUi } from "@/components/console-ui-context";
import { cn } from "@/lib/utils";

/**
 * The agent entry point, beside the bell. It toggles the agent panel
 * via ConsoleUiContext, so a second press closes what the first opened, as ⌘J does.
 */
export function AgentButton() {
  const { toggleAgent } = useConsoleUi();
  return (
    <button
      type="button"
      data-testid="agent-button"
      aria-label="Agent"
      aria-controls="agent-panel"
      title="Agent (⌘J)"
      onClick={toggleAgent}
      className={cn(
        "relative flex size-8 shrink-0 items-center justify-center",
        "rounded-menu text-muted-foreground hover:text-foreground",
        "outline-none focus-visible:ring-2 focus-visible:ring-ring",
        "hover:bg-sidebar-accent",
      )}
    >
      <Bot className="size-4" />
    </button>
  );
}
