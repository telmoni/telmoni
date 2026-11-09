import { StatusMark } from "@/components/status-badge";
import type { Trace } from "@/lib/server/entities/traces";
import { formatCount } from "@/lib/traces";
import { SpanStatus } from "@/lib/types/enums";

// A trace's status as a list shows it: the run's own mark, and beside any but
// Error how many of its spans failed, since a run that recovered from a
// failed call reads as clean otherwise. A run whose own span has not arrived
// is still running, whatever its spans so far say.
export function TraceStatusBadge({
  trace,
}: {
  trace: Pick<Trace, "ended" | "status" | "errors" | "error_type">;
}) {
  const mark = <StatusMark status={trace.ended ? trace.status : null} detail={trace.error_type} />;
  if (trace.errors === 0 || (trace.ended && trace.status === SpanStatus.Error)) return mark;
  return (
    <span className="inline-flex items-center gap-1.5">
      {mark}
      <span className="text-xs text-destructive">
        {formatCount(trace.errors)} failed {trace.errors === 1 ? "span" : "spans"}
      </span>
    </span>
  );
}
