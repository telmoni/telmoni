import type { Metadata } from "next";

import { ServiceUnavailable } from "@/components/service-unavailable";
import { fetchProjectBySlug } from "@/lib/server/data";

import { Overview } from "../../_overview";

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

// The project's name and nothing else, for now, as the organization's
// Overview is (asked 2026-10-08). ⚠ Its recent activity went with the rest,
// and that feed was the only place a project's notices — a member joining,
// a channel connected or disconnected — were readable in the console; they
// still reach every channel the project connects. The feed is in git, with
// its read-marking action, for when the page is rebuilt.
export default async function OverviewPage({
  params,
}: {
  params: Promise<{ project: string }>;
}) {
  const { project: slug } = await params;
  const project = await fetchProjectBySlug(slug);
  if (!project) return <ServiceUnavailable />;

  return <Overview title={project.name} />;
}
