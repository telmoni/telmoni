import type { Metadata } from "next";

import { ServiceUnavailable } from "@/components/service-unavailable";
import { fetchProjectBySlug, fetchProjectNotifications } from "@/lib/server/data";

import { Notices } from "../../_notices";
import { Overview } from "../../_overview";
import { MarkProjectRead } from "./_project-notices";

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

// The project's name, and its notices when there are any: a member joining
// or leaving, a channel connected or lost, a monitor its first ping made.
// Nothing else in the console reads them — why the organization's own
// notices stayed on its Overview when both were cleared on 2026-10-08 — and
// a monitor a ping made is told nowhere else unless a channel is connected.
// Anybody on the project reads them, as the service answers a project's feed
// at any role; the status board (roadmap 2.6) goes above them.
export default async function OverviewPage({
  params,
}: {
  params: Promise<{ project: string }>;
}) {
  const { project: slug } = await params;
  const project = await fetchProjectBySlug(slug);
  if (!project) return <ServiceUnavailable />;
  const notices = await fetchProjectNotifications(project.id);

  return (
    <Overview
      title={project.name}
      action={
        notices && notices.unread > 0 ? (
          <MarkProjectRead projectId={project.id} unread={notices.unread} />
        ) : undefined
      }
      sections={
        notices && notices.items.length > 0 ? (
          <Notices label="Project notices" feed={notices} />
        ) : undefined
      }
    />
  );
}
