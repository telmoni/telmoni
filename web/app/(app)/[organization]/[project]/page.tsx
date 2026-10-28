import type { Metadata } from "next";

import { MarkRead } from "@/components/mark-read";
import { ServiceUnavailable } from "@/components/service-unavailable";
import { fetchProjectBySlug, fetchProjectNotifications } from "@/lib/server/data";

import { Notices } from "../../_notices";
import { Overview } from "../../_overview";
import { markProjectReadAction } from "./notice-actions";

// The tab is titled as the page is, by the project's name.
export async function generateMetadata({
  params,
}: PageProps<"/[organization]/[project]">): Promise<Metadata> {
  const { project: slug } = await params;
  const project = await fetchProjectBySlug(slug);
  return { title: project ? project.name : "Overview" };
}

// The project's name, and under it its notices when there are any — a
// member joining or leaving, a channel connected or lost, an incident told,
// linking to it — which nothing else in the console reads. Anybody on the
// project reads them, as the service answers a project's feed at any role.
export default async function OverviewPage({
  params,
}: PageProps<"/[organization]/[project]">) {
  const { organization, project: slug } = await params;
  const project = await fetchProjectBySlug(slug);
  if (!project) return <ServiceUnavailable />;
  const notices = await fetchProjectNotifications(project.id);
  // Mark read goes up to the newest notice drawn, the first of the feed.
  const newest = notices?.items[0];

  return (
    <Overview
      title={project.name}
      sections={
        notices && notices.items.length > 0 ? (
          <Notices
            label="Project notices"
            feed={notices}
            project={{ organization, slug }}
            action={
              newest && notices.unread > 0 ? (
                <MarkRead
                  unread={notices.unread}
                  mark={markProjectReadAction.bind(null, project.id, newest.created_at)}
                />
              ) : undefined
            }
          />
        ) : undefined
      }
    />
  );
}
