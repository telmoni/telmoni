"use client";

import { useRef, useState } from "react";
import { createPortal } from "react-dom";
import { Plus } from "lucide-react";

import { useConsoleShell } from "@/components/console-shell";
import { MonitorPanel } from "@/components/monitors/monitor-panel";
import { PageAction } from "@/components/page-action";
import type { Monitor } from "@/lib/server/entities/monitors";

/** One opening of the panel: counted, so asking again draws it afresh, and
 *  the control the focus goes back to as it shuts. */
type Opening = { at: number; returnTo: HTMLElement | null };

/** The panel a monitor is made or changed in, opened from `children`'s
 *  button and drawn over the content area. */
function useMonitorPanel(props: {
  organization: string;
  slug: string;
  projectId: string;
  zones: string[] | null;
  monitor?: Monitor;
}) {
  const [opening, setOpening] = useState<Opening | null>(null);
  const openings = useRef(0);
  const { element } = useConsoleShell();
  const panel = opening && (
    <MonitorPanel
      key={opening.at}
      {...props}
      onClose={() => setOpening(null)}
      returnFocus={() => opening.returnTo}
    />
  );
  return {
    open: (from: HTMLElement) => setOpening({ at: ++openings.current, returnTo: from }),
    drawn: panel && (element ? createPortal(panel, element) : panel),
  };
}

/** New monitor, the Monitors page's primary action (`c`), for its owners and
 *  admins. */
export function NewMonitor(props: {
  organization: string;
  slug: string;
  projectId: string;
  zones: string[] | null;
}) {
  const { open, drawn } = useMonitorPanel(props);
  return (
    <>
      <PageAction primary="New monitor" onClick={(e) => open(e.currentTarget)}>
        <Plus className="size-4" />
        New monitor
      </PageAction>
      {drawn}
    </>
  );
}

/** Edit, on a monitor's own page: the same panel, its slug fixed. */
export function EditMonitor(props: {
  organization: string;
  slug: string;
  projectId: string;
  zones: string[] | null;
  monitor: Monitor;
}) {
  const { open, drawn } = useMonitorPanel(props);
  return (
    <>
      <PageAction primary="Edit" onClick={(e) => open(e.currentTarget)}>
        Edit
      </PageAction>
      {drawn}
    </>
  );
}
