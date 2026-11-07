"use client";

import { useId, useState } from "react";

import { CopyButton } from "@/components/copy-button";
import { cn } from "@/lib/utils";

export type InstallTab = { label: string; command: string };

// The install box: a row of tabs, one per way in, and under the one chosen
// its command, with a copy button. The arrow keys move between the tabs, as a
// tablist's do, and only the chosen one is in the tab order.
export function InstallTabs({
  tabs,
  name = "Install",
  compact = false,
}: {
  tabs: readonly InstallTab[];
  /** What the tabs are, for a screen reader. */
  name?: string;
  /** Small type and a gutter's padding, for a dialog rather than a page. */
  compact?: boolean;
}) {
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
    <div className={cn("rounded-md border border-border font-mono", compact ? "text-xs" : "text-sm lg:text-base")}>
      <div
        role="tablist"
        aria-label={name}
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
              "relative text-muted-foreground transition-colors hover:text-foreground",
              compact ? "p-gutter" : "px-5 py-gutter lg:px-page lg:py-4",
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
        className={cn(
          "flex justify-between gap-gutter",
          compact ? "items-start bg-muted/40 p-gutter" : "items-center px-5 py-5 lg:px-page lg:py-7",
        )}
      >
        <code
          className={cn(
            "block min-w-0 flex-1 text-foreground",
            compact ? "break-all whitespace-pre-wrap" : "overflow-x-auto whitespace-pre",
          )}
        >
          {tab.command}
        </code>
        <CopyButton value={tab.command} label={`the ${tab.label} command`} />
      </div>
    </div>
  );
}
