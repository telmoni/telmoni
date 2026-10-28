import Link from "next/link";

import { AccessDenied } from "@/components/access-denied";
import { PageHeader } from "@/components/page-header";
import { ServiceUnavailable } from "@/components/service-unavailable";
import { LayoutControls } from "@/components/traces/layout-controls";
import { TraceLayoutBody } from "@/components/traces/trace-layout";
import { TraceMeta } from "@/components/traces/trace-view";
import { Card } from "@/components/ui/card";
import { TooltipProvider } from "@/components/ui/tooltip";
import { ownerContact } from "@/lib/identity";
import { administers } from "@/lib/roles";
import {
  activeOrganization,
  fetchRunIncidents,
  fetchSessionLayout,
  fetchTrace,
  fetchTraceLayouts,
} from "@/lib/server/data";
import { projectPage } from "@/lib/server/project-page";
import { projectPath } from "@/lib/slug";
import { summarizeTrace } from "@/lib/trace-tree";
import { tracesHref } from "@/lib/traces";

export const metadata = { title: "Trace" };

// One trace: a run's record, named by the run's id, read as the list's peek
// reads it (`components/traces/peek.tsx`) with the page's room: how it ended
// and what it spent on one line, then its spans as the tree they ran in
// beside the span chosen, in full, opened as far as the page's layout goes
// (`lib/layout.ts`). Every pixel is metadata: no prompt, completion or tool
// payload reaches this page, in any content mode.
export default async function TracePage({
  params,
}: {
  params: Promise<{ organization: string; project: string; trace: string }>;
}) {
  const { organization, project: slug, trace: runId } = await params;
  const resolved = await projectPage(slug);
  if (resolved.kind === "unavailable") return <ServiceUnavailable />;
  const { gate, project } = resolved;

  const list = tracesHref(organization, slug, {});
  const [access, setting, layouts, incidents] = await Promise.all([
    fetchTrace(project.id, runId),
    fetchSessionLayout(project.id),
    fetchTraceLayouts(project.id),
    fetchRunIncidents(project.id, runId),
  ]);
  if (access.kind === "unavailable") return <ServiceUnavailable />;
  if (access.kind === "forbidden") {
    return (
      <AccessDenied
        what="this project's traces"
        owner={ownerContact(activeOrganization(gate))}
        backHref={projectPath(organization, slug)}
      />
    );
  }
  if (access.kind === "missing") {
    return (
      <>
        <PageHeader title="Trace" backHref={list} backLabel="Traces" />
        <Card className="grid gap-1 text-sm">
          <p className="font-medium">No trace with this id in this project.</p>
          <p className="text-muted-foreground">
            Its spans may have passed their retention, or the run belongs to another project.{" "}
            <Link href={list} className="underline underline-offset-4">
              Back to the traces
            </Link>
            .
          </p>
        </Card>
      </>
    );
  }

  const { trace } = access;
  const { root } = summarizeTrace(trace.spans);
  const page = {
    organization,
    slug,
    projectId: project.id,
    layouts: layouts.kind === "ok" ? layouts.layouts : null,
    project: setting,
  };
  return (
    <TooltipProvider>
      <PageHeader
        title={root.agent}
        backHref={list}
        backLabel="Traces"
        action={<LayoutControls {...page} canSave={administers(project.role)} />}
      />
      {/* As tall as the window leaves under the header and the page's title,
          both of a set height, so the spans and the span chosen scroll each
          on its own, as they do in the peek; never so short that neither
          reads. The trace's line stands inside, where its wrapping on a
          narrow window takes the panes' room rather than the page's. */}
      <Card className="h-[calc(100svh-10rem)] min-h-[28rem] gap-0 overflow-hidden p-0">
        <div className="shrink-0 border-b border-border p-3.5">
          <TraceMeta trace={trace} organization={organization} slug={slug} incidents={incidents} />
        </div>
        <TraceLayoutBody {...page} trace={trace} />
      </Card>
    </TooltipProvider>
  );
}
