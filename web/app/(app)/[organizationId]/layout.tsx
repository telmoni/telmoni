import { notFound, redirect } from "next/navigation";
import { activeOrganization, getServerContext } from "@/lib/server/entities/organization";
import { organizationMatches, organizationSegment } from "@/lib/slug";

export default async function OrgLayout({
  children,
  params,
}: {
  children: React.ReactNode;
  params: Promise<{ organizationId: string }>;
}) {
  const { organizationId } = await params;
  const ctx = await getServerContext();

  if (organizationId === "organization") {
    const activeOrg = ctx ? activeOrganization(ctx) : null;
    const target = activeOrg ? organizationSegment(activeOrg) : ctx?.activeOrganizationId;
    if (target) {
      redirect(`/${target}`);
    }
  }

  if (ctx && !ctx.organizations.some((o) => organizationMatches(o, organizationId))) {
    notFound();
  }

  return children;
}
