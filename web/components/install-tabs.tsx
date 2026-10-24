"use client";

import { useId, useState } from "react";

import { CopyButton } from "@/components/copy-button";
import { cn } from "@/lib/utils";

export type InstallTab = { label: string; command: string };

// The install box: a row of tabs, one per way in, and under the one chosen
// its command, with a copy button. The arrow keys move between the tabs, as a
// tablist's do, and only the chosen one is in the tab order.
export function InstallTabs({ tabs }: { tabs: readonly InstallTab[] }) {
  const [at, setAt] = useState(0);
  const id = useId();
  const tab = tabs[at];
  if (tab === undefined) return null;

  function onKeyDown(event: React.KeyboardEvent<HTMLDivElement>) {
    const to =
      event.key === "ArrowRight"
        ? at + 1
        : event.key === "ArrowLeft"
          ? at - 1
          : event.key === "Home"
            ? 0
            : event.key === "End"
              ? tabs.length - 1
              : null;
    if (to === null) return;
    event.preventDefault();
    const next = (to + tabs.length) % tabs.length;
    setAt(next);
    document.getElementById(`${id}-tab-${next}`)?.focus();
  }

  return (
    <div className="rounded-md border border-border font-mono text-sm lg:text-base">
      <div
        role="tablist"
        aria-label="Install"
        onKeyDown={onKeyDown}
        className="flex flex-wrap border-b border-border"
      >
        {tabs.map((candidate, i) => (
          <button
            key={candidate.label}
            id={`${id}-tab-${i}`}
            type="button"
            role="tab"
            aria-selected={i === at}
            aria-controls={`${id}-panel`}
            tabIndex={i === at ? 0 : -1}
            onClick={() => setAt(i)}
            className={cn(
              "relative px-5 py-3.5 text-muted-foreground transition-colors hover:text-foreground lg:px-6 lg:py-4",
              i === at &&
                "text-foreground after:absolute after:inset-x-0 after:-bottom-px after:h-px after:bg-foreground",
            )}
          >
            {candidate.label}
          </button>
        ))}
      </div>
      <div
        id={`${id}-panel`}
        role="tabpanel"
        aria-labelledby={`${id}-tab-${at}`}
        className="flex items-center justify-between gap-3 px-5 py-5 lg:px-6 lg:py-7"
      >
        <code className="block min-w-0 flex-1 overflow-x-auto whitespace-pre text-foreground">
          {tab.command}
        </code>
        <CopyButton value={tab.command} label="the command" />
      </div>
    </div>
  );
}
