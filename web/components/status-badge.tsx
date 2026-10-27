import type { ComponentProps } from "react";

import { Badge } from "@/components/ui/badge";
import { STATUS_LABEL } from "@/lib/traces";
import { SpanStatus } from "@/lib/types/enums";

const VARIANT: Record<SpanStatus, ComponentProps<typeof Badge>["variant"]> = {
  [SpanStatus.Ok]: "outline",
  [SpanStatus.Error]: "destructive",
  [SpanStatus.Cancelled]: "secondary",
};

/** How a run or a span ended, the error's class beside a failure; `null`
 *  while a run's own span has not arrived. One badge, so the list and a
 *  run's page draw an ending alike. */
export function StatusBadge({
  status,
  detail,
}: {
  status: SpanStatus | null;
  detail?: string | null;
}) {
  if (status === null) {
    return (
      <Badge variant="secondary" className="font-normal">
        Running
      </Badge>
    );
  }
  return (
    <Badge variant={VARIANT[status]} className="font-normal">
      {STATUS_LABEL[status]}
      {status === SpanStatus.Error && detail ? `: ${detail}` : ""}
    </Badge>
  );
}
