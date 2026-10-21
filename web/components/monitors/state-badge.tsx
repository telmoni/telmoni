import { Badge } from "@/components/ui/badge";
import { STATE_LABEL } from "@/lib/monitors";
import { MonitorState } from "@/lib/types/enums";

// A monitor's state as the board shows it: the states that need someone in
// the destructive colour, late and running marked, the rest plain.
export function MonitorStateBadge({ state }: { state: MonitorState }) {
  const variant =
    state === MonitorState.Failed ||
    state === MonitorState.Stalled ||
    state === MonitorState.OverBound
      ? "destructive"
      : state === MonitorState.Late || state === MonitorState.Running
        ? "secondary"
        : "outline";
  return (
    <Badge variant={variant} className="font-normal">
      {STATE_LABEL[state]}
    </Badge>
  );
}
