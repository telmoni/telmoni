"use client";

import { ArrowRight, Bot, History, Plug, Users, type LucideIcon } from "lucide-react";

import type { AgentSuggestion, SuggestionTopic } from "@/lib/agent/suggestions";

// The rail's own icons for the pages each starter reads (`lib/console-nav.ts`).
const ICONS: Record<SuggestionTopic, LucideIcon> = {
  members: Users,
  deliveries: Plug,
  audit: History,
};

/** An empty conversation: what the agent is for, and a few questions to start with. */
export function Welcome({
  project,
  suggestions,
  disabled,
  onAsk,
}: {
  project: string;
  suggestions: AgentSuggestion[];
  disabled: boolean;
  onAsk: (question: string) => void;
}) {
  return (
    <div className="my-auto grid justify-items-center gap-6 py-4 text-center">
      <div className="grid justify-items-center gap-2">
        <Bot aria-hidden className="size-6 text-muted-foreground" />
        <h3 className="text-base font-semibold wrap-break-word">Ask about {project}</h3>
        <p className="max-w-sm text-sm text-muted-foreground">
          It looks things up as you, so it sees what you can see, and it changes nothing.
        </p>
      </div>
      <ul aria-label="Questions to start with" className="grid w-full max-w-md gap-2 text-left">
        {suggestions.map((s) => {
          const Icon = ICONS[s.topic];
          return (
            <li key={s.topic}>
              <button
                type="button"
                disabled={disabled}
                onClick={() => onAsk(s.question)}
                className="flex w-full cursor-pointer items-center gap-3 rounded-lg border border-border px-3 py-2.5 text-left hover:bg-accent disabled:cursor-not-allowed disabled:opacity-50"
              >
                <span className="flex size-8 shrink-0 items-center justify-center rounded-md bg-muted text-muted-foreground">
                  <Icon aria-hidden className="size-4" />
                </span>
                <span className="grid min-w-0 flex-1 gap-0.5">
                  <span className="text-sm font-medium">{s.title}</span>
                  <span className="text-xs text-muted-foreground">{s.description}</span>
                </span>
                <ArrowRight aria-hidden className="size-4 shrink-0 text-muted-foreground" />
              </button>
            </li>
          );
        })}
      </ul>
    </div>
  );
}
