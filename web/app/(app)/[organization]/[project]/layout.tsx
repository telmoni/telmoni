import { notFound, redirect } from "next/navigation";

import {
  fetchProjectListing,
  fetchProjectsEverywhere,
  getServerContext,
} from "@/lib/server/data";
import { canonicalPath } from "@/lib/server/path";

// The path's second segment names a project by its slug in the organization
// the first one names, whose listing this is.
export default async function ProjectLayout({
  children,
  params,
}: {
  children: React.ReactNode;
  params: Promise<{ organization: string; project: string }>;
}) {
  const { organization, project: segment } = await params;

  const listing = await fetchProjectListing();
  // Auth did not answer: each page says so itself.
  if (listing.kind !== "ok") return children;
  if (listing.projects.some((p) => p.slug === segment)) return children;

  const here = listing.projects.find(
    (p) => p.id === segment || p.slug === segment.toLowerCase(),
  );
  if (here) redirect(await canonicalPath([organization, here.slug]));

  // An id is how anything kept longer than a page names a project, and the
  // project may have been handed to another organization since: on to where
  // it is now, when that is an organization the person is in.
  const [everywhere, ctx] = await Promise.all([fetchProjectsEverywhere(), getServerContext()]);
  const moved =
    everywhere.kind === "ok" ? everywhere.projects.find((p) => p.id === segment) : undefined;
  if (!moved || !ctx?.organizations.some((o) => o.organizationId === moved.organizationId)) {
    notFound();
  }
  redirect(await canonicalPath([moved.organizationSlug, moved.slug]));
}
