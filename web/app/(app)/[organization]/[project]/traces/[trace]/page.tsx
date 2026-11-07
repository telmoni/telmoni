import Link from "next/link";

import { AccessDenied } from "@/components/access-denied";
import { NoticeCard } from "@/components/notice-card";
import { PageHeader } from "@/components/page-header";
import { ServiceUnavailable } from "@/components/service-unavailable";
import { DeleteTrace } from "@/components/traces/delete-trace";
import { TraceBody, TraceMeta } from "@/components/traces/trace-view";
import { TraceScores, TraceScoresProvider } from "@/components/traces/trace-scores";
import { Card } from "@/components/ui/card";
import { TooltipProvider } from "@/components/ui/tooltip";
import { MediaProjectProvider, RunTurn } from "@/components/traces/span-content";
import { ownerContact } from "@/lib/identity";
import { administers } from "@/lib/roles";
import { activeOrganization, fetchRunIncidents, fetchTrace } from "@/lib/server/data";
import { fetchContentMode } from "@/lib/server/entities/content-mode";
import { ContentMode } from "@/lib/types/enums";
import { fetchScores } from "@/lib/server/entities/scores";
import { fieldsOf } from "@/lib/messages";
import { commentsOf } from "@/lib/scores";
import { projectPage } from "@/lib/server/project-page";
import { projectPath } from "@/lib/slug";
import { summarizeTrace } from "@/lib/trace-tree";
import { tracesHref } from "@/lib/traces";

export const metadata = { title: "Trace" };

// One trace: a run's record, named by the run's id, read as the list's peek
// reads it (`components/traces/peek.tsx`) with the page's room: how it ended
// and what it spent on one line, the run's own turn where the project keeps
// content, then its spans as the tree they ran in beside the span chosen, in
// full. A project admin deletes it from here.
export default async function TracePage({
  params,
}: PageProps<"/[organization]/[project]/traces/[trace]">) {
  const { organization, project: slug, trace: runId } = await params;
  const resolved = await projectPage(slug);
  if (resolved.kind === "unavailable") return <ServiceUnavailable />;
  const { gate, project } = resolved;

  const list = tracesHref(organization, slug, {});
  const [access, incidents, scores, mode] = await Promise.all([
    fetchTrace(project.id, runId, true),
    fetchRunIncidents(project.id, runId),
    fetchScores(project.id, runId),
    fetchContentMode(project.id),
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
        <PageHeader title="Trace" backHref={list} backLabel="Runs" />
        <NoticeCard
          title="No trace with this id in this project."
          body={
            <>
              It may have been deleted, its spans may have passed their retention, or the run belongs to another project.{" "}
              <Link href={list} className="underline underline-offset-4">
                Back to the traces
              </Link>
              .
            </>
          }
        />
      </>
    );
  }

  const { trace } = access;
  const { root } = summarizeTrace(trace.spans);
  return (
    <TooltipProvider>
      <PageHeader
        title={root.agent}
        backHref={list}
        backLabel="Runs"
        action={
          administers(project.role) && <DeleteTrace projectId={project.id} runId={trace.run_id} />
        }
      />
      {/* As tall as the window leaves under the header and the page's title,
          both of a set height, so the spans and the span chosen scroll each
          on its own, as they do in the peek; never so short that neither
          reads. The trace's line stands inside, where its wrapping on a
          narrow window takes the panes' room rather than the page's. */}
      <Card className="h-[calc(100svh-10rem)] min-h-[28rem] gap-0 overflow-hidden p-0">
        <div className="mx-gutter shrink-0 border-b border-border py-gutter">
          <TraceMeta
            trace={trace}
            organization={organization}
            slug={slug}
            incidents={incidents}
          />
          {/* Its scores, and a member's own to give: none drawn where they could not be read. */}
          {scores.kind === "ok" && (
            <div aria-label="Scores" role="group" className="mt-gutter flex flex-wrap items-center gap-1">
              <TraceScores
                projectId={project.id}
                runId={trace.run_id}
                scores={scores.scores}
                comments={commentsOf(trace.content)}
                spanNames={Object.fromEntries(trace.spans.map((span) => [span.span_id, span.name]))}
                me={gate.person.userId}
                administers={administers(project.role)}
                keepsContent={mode?.content_mode === ContentMode.On}
              />
            </div>
          )}
        </div>
        <MediaProjectProvider projectId={project.id}>
          <TraceScoresProvider
            scores={scores.kind === "ok" ? scores.scores : []}
            comments={commentsOf(trace.content)}
          >
            <div className="mx-gutter max-h-56 shrink-0 overflow-y-auto border-b border-border py-gutter">
              <RunTurn fields={fieldsOf(trace.content, root.span_id)} />
            </div>
            <TraceBody trace={trace} organization={organization} slug={slug} className="min-h-0 flex-1" />
          </TraceScoresProvider>
        </MediaProjectProvider>
      </Card>
    </TooltipProvider>
  );
}
