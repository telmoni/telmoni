"use client";

import { Check, Copy } from "lucide-react";
import { toast } from "sonner";

import { BODY } from "@/components/paper-page";
import { useCopyToClipboard } from "@/lib/use-copy-to-clipboard";
import { cn } from "@/lib/utils";

// One prompt for a coding agent, the whole row its copy button, so a thumb
// has more to land on than an icon.
export function PromptRow({ label, prompt }: { label: string; prompt: string }) {
  const { copied, copy } = useCopyToClipboard();
  const name = `${label.toLowerCase()} prompt`;
  return (
    <button
      type="button"
      aria-label={`Copy the ${name}`}
      onClick={async () => {
        if (!(await copy(prompt))) toast.error(`Couldn't copy the ${name}.`);
      }}
      className="flex w-full min-h-11 items-start gap-4 px-4 py-gutter text-left transition-colors hover:bg-muted/50 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring"
    >
      {/* The label over the prompt on a phone, beside it on a wider page. */}
      <span className="flex min-w-0 flex-1 flex-col gap-1 sm:flex-row sm:gap-4">
        <span className="shrink-0 text-[15px] font-medium text-foreground sm:w-20">{label}</span>
        <span className={cn("min-w-0 flex-1", BODY)}>{prompt}</span>
      </span>
      <span aria-hidden="true" className="mt-1 shrink-0 text-muted-foreground">
        {copied ? <Check className="size-4 text-foreground" /> : <Copy className="size-4" />}
      </span>
      <span className="sr-only" aria-live="polite">
        {copied ? "Copied" : ""}
      </span>
    </button>
  );
}
