"use client";

import { ConfirmDialog } from "@/components/confirm-dialog";
import { Button } from "@/components/ui/button";
import { ALERTS_HEADING, alertHeading, draftOf, type ConnectionChoice } from "@/lib/alerts";
import type { Alert } from "@/lib/server/data";

import { useAct } from "./_act";
import { AlertPanelHost, useOpening } from "./_alert-panel";
import { MuteButton } from "./_mute";
import { deleteAlertAction, hearAgainAction, pauseAlertAction } from "./actions";

const ROW_BUTTON = "h-9 gap-1.5 text-xs text-muted-foreground hover:text-foreground";

/** An alert's own controls: edit it, pause or resume it, mute it for a
 *  while, or delete it with its incidents. One its plan paused may be kept
 *  paused as a member's own, so room returning does not run it again. */
export function AlertActions({
  projectId,
  alert,
  connections,
}: {
  projectId: string;
  alert: Alert;
  connections: ConnectionChoice[] | null;
}) {
  const { opening, open, close } = useOpening();
  const { pending, error, act } = useAct(ALERTS_HEADING);
  const paused = alert.paused !== null;

  return (
    <div className="flex flex-wrap items-center justify-end gap-3">
      {error && (
        <span className="text-xs text-destructive" role="alert">
          {error}
        </span>
      )}
      <Button
        variant="outline"
        size="sm"
        className={ROW_BUTTON}
        disabled={pending}
        onClick={(e) => open(e.currentTarget)}
        aria-label={`Edit ${alert.name}`}
      >
        Edit
      </Button>
      <Button
        variant="outline"
        size="sm"
        className={ROW_BUTTON}
        disabled={pending}
        onClick={() => act(() => pauseAlertAction(projectId, alert.id, !paused))}
        aria-label={`${paused ? "Resume" : "Pause"} ${alert.name}`}
      >
        {paused ? "Resume" : "Pause"}
      </Button>
      {alert.paused === "plan" && (
        <Button
          variant="outline"
          size="sm"
          className={ROW_BUTTON}
          disabled={pending}
          onClick={() => act(() => pauseAlertAction(projectId, alert.id, true))}
          aria-label={`Keep ${alert.name} paused`}
        >
          Keep paused
        </Button>
      )}
      <MuteButton projectId={projectId} ruleId={alert.id} name={alert.name} disabled={pending} />
      <ConfirmDialog
        trigger={
          <Button
            variant="outline"
            size="sm"
            className="h-9 gap-1.5 text-xs text-destructive hover:bg-destructive/10 hover:text-destructive"
            disabled={pending}
            aria-label={`Delete ${alert.name}`}
          >
            Delete
          </Button>
        }
        title={`Delete "${alert.name}"?`}
        description="The alert stops at once, and its incidents and maintenance windows go with it; one still open is told resolved to the connectors that heard it."
        confirmLabel="Delete"
        destructive
        onConfirm={() => act(() => deleteAlertAction(projectId, alert.id), true)}
      />
      <AlertPanelHost
        opening={opening}
        onClose={close}
        projectId={projectId}
        ruleId={alert.id}
        initial={draftOf(alert)}
        connections={connections}
      />
    </div>
  );
}

/** A connector the alert muted after five failed deliveries in a row,
 *  hearing its notices again, its failures forgotten. */
export function HearAgain({
  projectId,
  ruleId,
  connectionId,
  name,
}: {
  projectId: string;
  ruleId: string;
  connectionId: string;
  name: string;
}) {
  const { pending, error, act } = useAct(alertHeading(ruleId));
  return (
    <span className="inline-flex flex-wrap items-center gap-3">
      <Button
        variant="outline"
        size="sm"
        className={ROW_BUTTON}
        disabled={pending}
        onClick={() => act(() => hearAgainAction(projectId, ruleId, connectionId), true)}
        aria-label={`Hear again: ${name}`}
      >
        Hear again
      </Button>
      {error && (
        <span className="text-xs text-destructive" role="alert">
          {error}
        </span>
      )}
    </span>
  );
}
