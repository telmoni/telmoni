import Link from "next/link";

import { AccessDenied } from "@/components/access-denied";
import { NoticeCard } from "@/components/notice-card";
import { PageHeader } from "@/components/page-header";
import { ServiceUnavailable } from "@/components/service-unavailable";
import { CompareRuns } from "@/components/traces/compare-runs";
import { MediaProjectProvider } from "@/components/traces/span-content";
import { TooltipProvider } from "@/components/ui/tooltip";
import { ownerContact } from "@/lib/identity";
import { isUuid, one } from "@/lib/list-query";
import { activeOrganization, fetchTrace } from "@/lib/server/data";
import { fetchContentMode } from "@/lib/server/entities/content-mode";
import { projectPage } from "@/lib/server/project-page";
import { projectPath } from "@/lib/slug";
import { tracesHref } from "@/lib/traces";
import { ContentMode } from "@/lib/types/enums";

export const metadata = { title: "Compare traces" };

const INFO =
  "Two runs of this project side by side: what each took and spent, and their spans in the order they ran, row by row, the first where they part marked. Their messages are opened only when you ask, each run's read filed in the audit log as its trace page's is.";

// Two runs, named in the link (`?a=` and `?b=`, the first chosen on the
// left), read as metadata alone: neither's content is opened, nor filed as
// a read, until the reader asks for it (`compare-runs.tsx`).
export default async function CompareTracesPage({
  params,
  searchParams,
}: PageProps<"/[organization]/[project]/traces/compare">) {
  const [{ organization, project: slug }, query] = await Promise.all([params, searchParams]);
  const resolved = await projectPage(slug);
  if (resolved.kind === "unavailable") return <ServiceUnavailable />;
  const { gate, project } = resolved;
  const list = tracesHref(organization, slug, {});
  const header = <PageHeader title="Compare traces" info={INFO} backHref={list} backLabel="Traces" />;
  const back = (
    <Link href={list} className="underline underline-offset-4">
      Back to the traces
    </Link>
  );

  const a = one(query.a) ?? "";
  const b = one(query.b) ?? "";
  if (!isUuid(a) || !isUuid(b) || a.toLowerCase() === b.toLowerCase()) {
    return (
      <>
        {header}
        <NoticeCard
          title={a && a.toLowerCase() === b.toLowerCase() ? "That is one trace twice." : "Choose two traces to compare."}
          body={<>Tick two different traces in the list, then Compare. {back}.</>}
        />
      </>
    );
  }

  const [left, right, mode] = await Promise.all([
    fetchTrace(project.id, a, false),
    fetchTrace(project.id, b, false),
    fetchContentMode(project.id),
  ]);
  if (left.kind === "unavailable" || right.kind === "unavailable") return <ServiceUnavailable />;
  if (left.kind === "forbidden" || right.kind === "forbidden") {
    return (
      <AccessDenied
        what="this project's traces"
        owner={ownerContact(activeOrganization(gate))}
        backHref={projectPath(organization, slug)}
      />
    );
  }
  if (left.kind === "missing" || right.kind === "missing") {
    const title =
      left.kind === "missing" && right.kind === "missing"
        ? "Neither trace is in this project."
        : `The ${left.kind === "missing" ? "first" : "second"} trace isn't in this project.`;
    return (
      <>
        {header}
        <NoticeCard
          title={title}
          body={
            <>
              It may have been deleted, its spans may have passed their retention, or the run belongs to another project. {back}.
            </>
          }
        />
      </>
    );
  }

  return (
    <>
      {header}
      <TooltipProvider>
        <MediaProjectProvider projectId={project.id}>
          {/* Keyed on the runs, so messages opened for one pair are never read for the next. */}
          <CompareRuns
            key={`${a}:${b}`}
            a={left.trace}
            b={right.trace}
            organization={organization}
            slug={slug}
            projectId={project.id}
            keepsContent={mode?.content_mode === ContentMode.On}
          />
        </MediaProjectProvider>
      </TooltipProvider>
    </>
  );
}
