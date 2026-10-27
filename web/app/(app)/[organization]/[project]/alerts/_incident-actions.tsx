"use client";

import { useEffect, useRef } from "react";

import { ConfirmDialog } from "@/components/confirm-dialog";
import { LocalTime } from "@/components/local-time";
import { Button } from "@/components/ui/button";
import { alertHeading } from "@/lib/alerts";
import { IncidentStatus } from "@/lib/types/enums";

import { useAct } from "./_act";
import { incidentAction } from "./actions";

const ROW_BUTTON = "h-9 gap-1.5 text-xs text-muted-foreground hover:text-foreground";

/** An incident taken by a member, which stops its reminders, or resolved by
 *  hand, which its channels are told as a recovery is. */
export function IncidentActions({
  projectId,
  ruleId,
  incidentId,
  status,
  name,
  openedAt,
}: {
  projectId: string;
  /** Its alert's id, whose heading takes the focus once no control stays. */
  ruleId: string;
  incidentId: string;
  status: IncidentStatus;
  /** Its alert's name. */
  name: string;
  openedAt: string;
}) {
  const { pending, error, act, movedRef } = useAct();
  const resolve = useRef<HTMLButtonElement>(null);
  // A move takes away the button it was made with: the focus goes on to the
  // row's next control, or to its alert's heading once none stays.
  useEffect(() => {
    if (movedRef.current && document.activeElement === document.body) {
      (resolve.current ?? document.getElementById(alertHeading(ruleId)))?.focus({
        preventScroll: true,
      });
    }
    movedRef.current = false;
  }, [status, movedRef, ruleId]);
  const settled = status === IncidentStatus.Resolved;
  // Told apart, to a screen reader, from the same alert's other incidents.
  const which = (
    <>
      the incident of {name}, opened <LocalTime iso={openedAt} />
    </>
  );

  return (
    <div className="flex items-center justify-end gap-3">
      {error && (
        <span className="text-xs text-destructive" role="alert">
          {error}
        </span>
      )}
      {status === IncidentStatus.Open && (
        <Button
          variant="outline"
          size="sm"
          className={ROW_BUTTON}
          disabled={pending}
          onClick={() => act(() => incidentAction(projectId, incidentId, "acknowledge"), true)}
        >
          Acknowledge <span className="sr-only">{which}</span>
        </Button>
      )}
      {!settled && (
        <ConfirmDialog
          trigger={
            <Button
              ref={resolve}
              variant="outline"
              size="sm"
              className={ROW_BUTTON}
              disabled={pending}
            >
              Resolve <span className="sr-only">{which}</span>
            </Button>
          }
          title={`Resolve the incident of "${name}"?`}
          description="Its channels are told it is resolved. An alert still past its threshold opens no new incident until it recovers and breaches again."
          confirmLabel="Resolve"
          onConfirm={() => act(() => incidentAction(projectId, incidentId, "resolve"), true)}
        />
      )}
    </div>
  );
}
