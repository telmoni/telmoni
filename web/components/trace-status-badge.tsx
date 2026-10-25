import { Badge } from "@/components/ui/badge";
import type { Trace } from "@/lib/server/entities/traces";
import { formatCount, STATUS_LABEL } from "@/lib/traces";
import { SpanStatus } from "@/lib/types/enums";

// A trace's status as a list shows it: the run's own, and beside any but
// Error how many of its spans failed, since a run that recovered from a
// failed call reads as clean otherwise. A run whose own span has not arrived
// is still running, whatever its spans so far say.
export function TraceStatusBadge({
  trace,
}: {
  trace: Pick<Trace, "ended" | "status" | "errors">;
}) {
  const badge = !trace.ended ? (
    <Badge variant="secondary" className="font-normal">
      Running
    </Badge>
  ) : trace.status === SpanStatus.Error ? (
    <Badge variant="destructive" className="font-normal">
      {STATUS_LABEL[trace.status]}
    </Badge>
  ) : (
    <Badge variant="outline" className="font-normal">
      {STATUS_LABEL[trace.status]}
    </Badge>
  );
  if (trace.errors === 0 || (trace.ended && trace.status === SpanStatus.Error)) return badge;
  return (
    <span className="inline-flex items-center gap-1.5">
      {badge}
      <span className="text-xs text-destructive">
        {formatCount(trace.errors)} failed {trace.errors === 1 ? "span" : "spans"}
      </span>
    </span>
  );
}
