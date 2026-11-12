import { STATUS_LABEL } from "@/lib/traces";
import { SpanStatus } from "@/lib/types/enums";
import { cn } from "@/lib/utils";

const MARK: Record<SpanStatus, string> = {
  [SpanStatus.Ok]: "bg-brand-positive",
  [SpanStatus.Error]: "bg-brand-negative",
  [SpanStatus.Cancelled]: "bg-muted-foreground/50",
};

/** Status colour and optional live pulse; the caller supplies its accessible label. */
export function StatusIndicator({ className, live = false }: { className: string; live?: boolean }) {
  return (
    <span aria-hidden="true" className="relative inline-flex size-2 shrink-0">
      {live && <span className={cn("absolute inline-flex size-full rounded-full opacity-75 motion-safe:animate-ping", className)} />}
      <span className={cn("relative inline-flex size-2 rounded-full", className)} />
    </span>
  );
}

/** A trace's outcome, or live activity, with failures named beside the mark. */
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
      <StatusIndicator live={status === null} className={status === null ? "bg-brand-positive" : MARK[status]} />
      <span className="sr-only">{failed ? `${word}: ` : word}</span>
      {failed && <span className="text-brand-negative">{detail}</span>}
    </span>
  );
}
