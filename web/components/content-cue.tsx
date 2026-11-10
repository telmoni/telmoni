import { Eye, EyeOff } from "lucide-react";

import { ContentMode } from "@/lib/types/enums";
import { cn } from "@/lib/utils";

/** Whether a project keeps its spans' content, at a glance: an eye when it
 *  does, a muted struck eye when it does not; `labelled` says it in words
 *  beside the eye, as a trace's summary line does. */
export function ContentCue({ mode, labelled = false }: { mode: ContentMode; labelled?: boolean }) {
  const on = mode === ContentMode.On;
  const label = on ? "Content on" : "Content off";
  const title = on
    ? "This project keeps prompts, replies and tool payloads, sealed under its own key"
    : "This project keeps metadata alone: prompts and replies stay in your process";
  return (
    <span
      title={title}
      className={cn(
        "flex shrink-0 items-center gap-gutter [&_svg]:size-icon",
        on ? "text-foreground" : "text-muted-foreground",
      )}
    >
      {on ? <Eye aria-hidden="true" /> : <EyeOff aria-hidden="true" />}
      <span className={labelled ? undefined : "sr-only"}>{label}</span>
    </span>
  );
}
