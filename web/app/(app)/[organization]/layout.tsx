import { notFound, redirect } from "next/navigation";

import { getServerContext } from "@/lib/server/data";
import { canonicalPath } from "@/lib/server/path";
import { storeSeed } from "@/lib/server/store-seed";
import { StoreSeed } from "@/lib/store/provider";

// The path's first segment names the organization everything below acts in:
// the proxy hands it to `getServerContext`, which asks auth for it. Here a
// segment that is no slug of theirs is sent on to the slug it means, or
// answered "not found", and the store is handed the organization arrived in.
//
// ⚠ Not the check that keeps a page to the organization its path names. The
// router keeps this layout across a move between the pages under it, and a
// page renders beside it, not after it: every page asks for itself
// (`organizationNotFound`, `fetchProjectBySlug`).
export default async function OrganizationLayout({
  children,
  params,
}: {
  children: React.ReactNode;
  params: Promise<{ organization: string }>;
}) {
  const { organization: segment } = await params;
  const [ctx, seed] = await Promise.all([getServerContext(), storeSeed()]);
  // Auth did not answer: each page says so itself.
  if (!ctx || !seed) return children;

  const named = ctx.organizations.find((o) => o.slug === segment);
  if (!named) {
    // An id — the connect callback's redirect, or a path an API client spelled
    // with one — or a slug typed with a capital: on to the slug it goes by.
    const meant = ctx.organizations.find(
      (o) => o.organizationId === segment || o.slug === segment.toLowerCase(),
    );
    if (!meant) notFound();
    redirect(await canonicalPath([meant.slug]));
  }
  if (ctx.activeOrganizationId !== named.organizationId) notFound();

  return (
    <>
      <StoreSeed {...seed} />
      {children}
    </>
  );
}
