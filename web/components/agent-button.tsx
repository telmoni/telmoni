"use client";

import { Bot } from "lucide-react";

import { useConsoleUi } from "@/components/console-ui-context";
import { cn } from "@/lib/utils";

/**
 * The agent entry point, beside the bell. It toggles the agent's window
 * via ConsoleUiContext, so a second press closes what the first opened, as ⌘J does.
 */
export function AgentButton() {
  const { agentOpen, toggleAgent } = useConsoleUi();
  return (
    <button
      type="button"
      data-testid="agent-button"
      aria-label="Agent"
      aria-controls="agent-panel"
      aria-expanded={agentOpen}
      title="Agent (⌘J)"
      onClick={toggleAgent}
      className={cn(
        "relative flex size-9 shrink-0 items-center justify-center",
        "rounded-md text-muted-foreground hover:text-foreground",
        "outline-none focus-visible:ring-2 focus-visible:ring-ring",
        // Half the accent, as the search, the bell and the account beside it.
        "hover:bg-sidebar-accent/50",
      )}
    >
      <Bot className="size-4" />
    </button>
  );
}
