import { Badge } from "@/components/ui/badge";
import type { Trace } from "@/lib/server/entities/traces";
import { STATUS_LABEL } from "@/lib/traces";
import { SpanStatus } from "@/lib/types/enums";

// A trace's status as a list shows it. A run whose own span has not arrived
// is still running: its status is not known yet, whatever its spans so far
// say.
export function TraceStatusBadge({ trace }: { trace: Pick<Trace, "ended" | "status"> }) {
  if (!trace.ended) {
    return (
      <Badge variant="secondary" className="font-normal">
        Running
      </Badge>
    );
  }
  if (trace.status === SpanStatus.Error) {
    return (
      <Badge variant="destructive" className="font-normal">
        {STATUS_LABEL[trace.status]}
      </Badge>
    );
  }
  return (
    <Badge variant="outline" className="font-normal">
      {STATUS_LABEL[trace.status]}
    </Badge>
  );
}
