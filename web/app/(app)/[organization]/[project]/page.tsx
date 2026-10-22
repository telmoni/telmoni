import type { Metadata } from "next";

import { ServiceUnavailable } from "@/components/service-unavailable";
import { fetchMonitors, fetchProjectBySlug, fetchProjectNotifications } from "@/lib/server/data";

import { Notices } from "../../_notices";
import { Overview } from "../../_overview";
import { MarkProjectRead } from "./_project-notices";
import { StatusBoard } from "./_status-board";

// The tab is titled as the page is, by the project's name.
export async function generateMetadata({
  params,
}: {
  params: Promise<{ project: string }>;
}): Promise<Metadata> {
  const { project: slug } = await params;
  const project = await fetchProjectBySlug(slug);
  return { title: project ? project.name : "Overview" };
}

// The project's home: the status board, its monitors as they stand, and
// under it the project's notices when there are any — a member joining or
// leaving, a channel connected or lost, a monitor its first ping made —
// which nothing else in the console reads. Anybody on the project reads
// both, as the service answers a project's monitors and its feed at any
// role.
export default async function OverviewPage({
  params,
}: {
  params: Promise<{ organization: string; project: string }>;
}) {
  const { organization, project: slug } = await params;
  const project = await fetchProjectBySlug(slug);
  if (!project) return <ServiceUnavailable />;
  const [monitors, notices] = await Promise.all([
    fetchMonitors(project.id),
    fetchProjectNotifications(project.id),
  ]);
  // Mark read goes up to the newest notice drawn, the first of the feed.
  const newest = notices?.items[0];

  return (
    <Overview
      title={project.name}
      sections={
        <>
          <StatusBoard monitors={monitors} organization={organization} project={slug} />
          {notices && notices.items.length > 0 && (
            <Notices
              label="Project notices"
              feed={notices}
              action={
                newest && notices.unread > 0 ? (
                  <MarkProjectRead
                    projectId={project.id}
                    unread={notices.unread}
                    before={newest.created_at}
                  />
                ) : undefined
              }
            />
          )}
        </>
      }
    />
  );
}
