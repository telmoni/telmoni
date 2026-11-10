import type { Severity } from "@/lib/types/enums";
import { SEVERITY_MARK } from "@/lib/alerts";
import { cn } from "@/lib/utils";

/** Where an alert stands, as a square in its colour, the shape the Traces
 *  page's listening mark takes: its word is the square's title, and a screen
 *  reader's. */
export function SeverityMark({ severity, label }: { severity: Severity; label: string }) {
  return (
    <span title={label} data-slot="severity" className="inline-flex shrink-0 items-center">
      <span aria-hidden="true" className={cn("size-2 rounded-full", SEVERITY_MARK[severity])} />
      <span className="sr-only">{label}</span>
    </span>
  );
}
