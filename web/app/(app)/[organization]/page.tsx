import Link from "next/link";
import { notFound } from "next/navigation";

import { CreateProjectAction } from "@/components/create-project";
import { LocalTime } from "@/components/local-time";
import { RoleBadge } from "@/components/role-badge";
import { Row, Rows } from "@/components/rows";
import { ServiceUnavailable } from "@/components/service-unavailable";
import { Badge } from "@/components/ui/badge";
import { Card } from "@/components/ui/card";
import { organizationLabel } from "@/lib/identity";
import { eventKindLabel } from "@/lib/notification-kinds";
import {
  activeOrganization,
  fetchNotifications,
  fetchProjectListing,
  getServerContext,
  type FeedItem,
  type NotificationFeed,
  type ProjectListing,
} from "@/lib/server/data";
import { projectPath } from "@/lib/slug";
import { cn } from "@/lib/utils";

import { Overview } from "../_overview";
import { LeaveOrganization } from "./_leave-organization";
import { MarkOrganizationRead } from "./_organization-notices";

export const metadata = { title: "Overview" };

export default async function OrganizationOverviewPage() {
  const gate = await getServerContext();
  // The path names the organization: not found unless auth answered with it.
  if (gate?.organizationNotFound) notFound();
  const organization = gate ? activeOrganization(gate) : null;
  if (!organization) return <ServiceUnavailable />;

  // The organization's own name — never the name or the address of the
  // person who holds it. See `organizationLabel`.
  const name = organizationLabel(organization);

  const isOwner = organization.role === "owner";
  const isAdmin = isOwner || organization.role === "admin";
  // The organization's own feed — the notices that name no project, such as
  // an `organization_alert` a service beside this one raises — answers its
  // owner and admins, so nobody else is sent to ask for it.
  const [notices, projects] = await Promise.all([
    isAdmin ? fetchNotifications() : null,
    fetchProjectListing(),
  ]);

  // Anybody but the owner may leave. The owner hands the organization to an
  // admin first — the organization cannot be left with nobody holding it.
  const canLeave = !isOwner;

  const markRead =
    notices && notices.unread > 0 ? (
      <MarkOrganizationRead organizationId={organization.organizationId} unread={notices.unread} />
    ) : null;

  return (
    <Overview
      action={
        markRead || isAdmin ? (
          <div className="flex items-center gap-2">
            {markRead}
            {isAdmin && <CreateProjectAction organizationId={organization.organizationId} />}
          </div>
        ) : undefined
      }
      sections={
        <>
          <OrganizationProjects
            organization={organization.slug}
            listing={projects}
            canCreate={isAdmin}
          />
          {notices && notices.items.length > 0 && <OrganizationNotices feed={notices} />}
          {canLeave && (
            <LeaveOrganization organizationId={organization.organizationId} name={name} />
          )}
        </>
      }
    >
      <h2 className="text-base font-semibold">{name}</h2>
    </Overview>
  );
}

// ⚠ **The overview is where the console opens, so it carries the projects**,
// as a Vercel team's overview does: each opens from here, and the first is made
// from here. The listing is auth's — every project for an owner or admin, the
// ones a member holds a seat in — so it is shown as answered, with no filter.
function OrganizationProjects({
  organization,
  listing,
  canCreate,
}: {
  organization: string;
  listing: ProjectListing;
  canCreate: boolean;
}) {
  return (
    <section className="grid gap-3" aria-label="Projects">
      <div className="flex items-center gap-2">
        <h3 className="text-sm font-medium">Projects</h3>
        {listing.kind === "ok" && (
          <Badge variant="secondary" className="text-xs">
            {listing.projects.length}
          </Badge>
        )}
      </div>
      {listing.kind === "unavailable" ? (
        <Card className="p-4 text-sm text-muted-foreground">
          The projects could not be loaded. Try again in a moment.
        </Card>
      ) : listing.projects.length === 0 ? (
        <Card className="overflow-hidden p-0 gap-0">
          <div className="grid gap-1 p-4">
            <p className="text-sm font-medium">No projects yet.</p>
            <p className="text-sm text-muted-foreground">
              {canCreate
                ? "Create the first one with New project, above."
                : "An owner or admin adds you to a project."}
            </p>
          </div>
        </Card>
      ) : (
        <Rows>
          {listing.projects.map((project) => (
            <Row
              key={project.id}
              label={
                <Link
                  href={projectPath(organization, project.slug)}
                  className="block min-w-0 truncate hover:underline"
                >
                  {project.name}
                </Link>
              }
            >
              <RoleBadge role={project.role} />
            </Row>
          ))}
        </Rows>
      )}
    </section>
  );
}

function OrganizationNotices({ feed }: { feed: NotificationFeed }) {
  return (
    <section className="grid gap-3" aria-label="Organization notices">
      <div className="flex items-center justify-between gap-2">
        <h3 className="text-sm font-medium">Organization notices</h3>
      </div>
      <Rows>
        {feed.items.map((item) => (
          <OrganizationNoticeRow key={item.id} item={item} />
        ))}
      </Rows>
    </section>
  );
}

function OrganizationNoticeRow({ item }: { item: FeedItem }) {
  const unread = item.read_at === null;
  return (
    <Row
      label={
        <span
          className={cn(
            "block min-w-0 truncate",
            unread ? "font-bold text-foreground" : "font-normal text-muted-foreground",
          )}
        >
          <span>{item.title}</span>
          {unread && <span className="sr-only"> Unread</span>}
        </span>
      }
      hint={
        <span className="block">
          <span className={cn(unread ? "text-foreground/80" : "text-muted-foreground")}>
            {item.body}
          </span>
          <span className="mt-0.5 block text-xs opacity-70">{eventKindLabel(item.kind)}</span>
        </span>
      }
    >
      <span className={cn(unread ? "font-medium text-foreground" : "text-muted-foreground")}>
        <LocalTime iso={item.created_at} mode="datetime" />
      </span>
    </Row>
  );
}
