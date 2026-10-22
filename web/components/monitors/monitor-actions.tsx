"use client";

import { useState, useTransition } from "react";
import { useRouter } from "next/navigation";
import { toast } from "sonner";

import { ConfirmDialog } from "@/components/confirm-dialog";
import { EditMonitor } from "@/components/monitors/new-monitor";
import { Button } from "@/components/ui/button";
import type { Monitor } from "@/lib/server/entities/monitors";
import { MonitorState } from "@/lib/types/enums";

import {
  deleteMonitorAction,
  pauseMonitorAction,
  resumeMonitorAction,
} from "@/app/(app)/[organization]/[project]/monitors/actions";

// A monitor's own changes, on its page for the project's owners and admins:
// Edit, its slug fixed; Pause, never late again until resumed, its runs still
// kept, or Resume; and Delete, which asks for the slug. A monitor its first
// ping or run made is paused until its settings are saved, so Edit is its
// way to start, and a pause would change nothing.
export function MonitorActions({
  organization,
  slug,
  projectId,
  zones,
  monitor,
  listHref,
}: {
  organization: string;
  slug: string;
  projectId: string;
  zones: string[] | null;
  monitor: Monitor;
  listHref: string;
}) {
  const router = useRouter();
  const [pending, start] = useTransition();
  const [deleting, setDeleting] = useState(false);

  const flip = (paused: boolean) =>
    start(async () => {
      const answer = paused
        ? await resumeMonitorAction(projectId, monitor.slug)
        : await pauseMonitorAction(projectId, monitor.slug);
      if (answer.status === "error") toast.error(answer.error);
    });

  const remove = () =>
    start(async () => {
      const answer = await deleteMonitorAction(projectId, monitor.slug);
      setDeleting(false);
      if (answer.error) {
        toast.error(answer.error);
        return;
      }
      toast.success(`${monitor.name} deleted. Its finished runs stay, as traces of its agent.`);
      router.push(listHref);
    });

  const paused = monitor.state === MonitorState.Paused;
  return (
    <div className="flex items-center gap-2">
      {monitor.state !== MonitorState.New && (
        <Button variant="outline" disabled={pending} onClick={() => flip(paused)}>
          {paused ? "Resume" : "Pause"}
        </Button>
      )}
      <Button variant="outline" disabled={pending} onClick={() => setDeleting(true)}>
        Delete
      </Button>
      <EditMonitor
        organization={organization}
        slug={slug}
        projectId={projectId}
        zones={zones}
        monitor={monitor}
      />
      <ConfirmDialog
        open={deleting}
        onOpenChange={setDeleting}
        title={`Delete ${monitor.name}?`}
        description={
          <>
            Its finished runs stay, as traces of the agent <code>{monitor.agent}</code>; a run
            still open goes with it, and pings to its slug make a new monitor, paused until it is
            set up.
          </>
        }
        confirmLabel="Delete monitor"
        destructive
        confirmPhrase={monitor.slug}
        onConfirm={remove}
      />
    </div>
  );
}
