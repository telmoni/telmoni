import { STATUS_LABEL } from "@/lib/traces";
import { SpanStatus } from "@/lib/types/enums";
import { cn } from "@/lib/utils";

const MARK: Record<SpanStatus, string> = {
  [SpanStatus.Ok]: "bg-brand-positive",
  [SpanStatus.Error]: "bg-brand-negative",
  [SpanStatus.Cancelled]: "bg-muted-foreground/50",
};

/** How a run or a span ended, as a square in its colour, as an alert's
 *  severity is (`SeverityMark`): a failure keeps its error's class in words
 *  beside it; `null`, still running, is a muted square that breathes. The
 *  word is the square's title and a screen reader's. */
export function StatusMark({
  status,
  detail,
}: {
  status: SpanStatus | null;
  detail?: string | null;
}) {
  const word = status === null ? "Running" : STATUS_LABEL[status];
  const failed = status === SpanStatus.Error && detail;
  return (
    <span
      title={failed ? `${word}: ${detail}` : word}
      data-slot="status"
      className="inline-flex shrink-0 items-center gap-1.5"
    >
      <span
        aria-hidden="true"
        className={cn(
          "size-2 rounded-full",
          status === null ? "bg-muted-foreground/50 motion-safe:animate-pulse" : MARK[status],
        )}
      />
      <span className="sr-only">{failed ? `${word}: ` : word}</span>
      {failed && <span className="text-brand-negative">{detail}</span>}
    </span>
  );
}
