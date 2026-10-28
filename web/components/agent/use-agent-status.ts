import { useEffect, useState } from "react";

import { readAgentStatus } from "@/components/agent/reads";
import type { AgentStatus } from "@/lib/types/agent";

/** Whether the agent answers for `projectId`, asked each time the window
 *  opens on a project, so a check that failed once is not the answer for
 *  good, and a closed window asks nothing; `null` until it answers. */
export function useAgentStatus(projectId: string | null, open: boolean) {
  const [answer, setAnswer] = useState<{ projectId: string; status: AgentStatus } | null>(null);
  const [asked, setAsked] = useState(0);
  useEffect(() => {
    if (!open || !projectId) return;
    let cancelled = false;
    void readAgentStatus(projectId).then((res) => {
      if (!cancelled) setAnswer({ projectId, status: "error" in res ? "unavailable" : res.status });
    });
    return () => {
      cancelled = true;
    };
  }, [open, projectId, asked]);
  return {
    status: projectId && answer?.projectId === projectId ? answer.status : null,
    /** Asked again, the last answer gone meanwhile. */
    retry: () => {
      setAnswer(null);
      setAsked((n) => n + 1);
    },
    /** A turn was refused because the agent is off: the window says so. */
    disabled: () => {
      if (projectId) setAnswer({ projectId, status: "disabled" });
    },
  };
}
