"use client";

import { useState } from "react";
import { SparklesIcon } from "lucide-react";
import { useRouter } from "next/navigation";
import { toast } from "sonner";

import { askTraceListAction } from "@/app/(app)/agent-actions";
import { Input } from "@/components/ui/input";
import { TRACE_QUESTION_MAX } from "@/lib/types/agent";
import { TRACES, askedNote, type TraceUi } from "@/lib/traces";
import { useToastAction } from "@/lib/use-action";

/** The trace list asked in words: the agent turns the sentence into the
 *  list's filters, sort and window, which the page opens on as its chips
 *  and its URL. One of the plan's agent questions; drawn where the agent is on. */
export function AskList({
  organization,
  slug,
  projectId,
  days,
  q,
  ui,
}: {
  organization: string;
  slug: string;
  projectId: string;
  /** The window the list shows now, which a sentence naming none keeps. */
  days: number;
  /** The search of kept content, which is the search box's and stays. */
  q: string | undefined;
  ui: TraceUi;
}) {
  const router = useRouter();
  const { pending, run } = useToastAction();
  const [value, setValue] = useState("");

  return (
    <form
      className="relative min-w-48 flex-1 sm:max-w-80"
      aria-busy={pending}
      onSubmit={(e) => {
        e.preventDefault();
        const sentence = value.trim();
        if (!sentence || pending) return;
        run(
          () => askTraceListAction(projectId, sentence, days),
          ({ answer }) => {
            const note = askedNote(answer);
            if (note) toast.message(note);
            if (answer.query === null) return;
            router.push(
              TRACES.href(organization, slug, { ...TRACES.queryOfView(answer.query), q }, {}, {
                display: ui.display,
              }),
              { scroll: false },
            );
          },
        );
      }}
    >
      <SparklesIcon
        className="pointer-events-none absolute top-1/2 left-1.5 size-4 -translate-y-1/2 text-muted-foreground"
        aria-hidden="true"
      />
      <Input
        aria-label="Ask the list in words"
        placeholder="Ask: errors from billing this week"
        maxLength={TRACE_QUESTION_MAX}
        value={value}
        disabled={pending}
        onChange={(e) => setValue(e.target.value)}
        className="pl-7"
      />
    </form>
  );
}
