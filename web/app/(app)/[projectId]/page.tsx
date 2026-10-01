import { LocalTime } from "@/components/local-time";
import { Row, Rows } from "@/components/rows";
import { ServiceUnavailable } from "@/components/service-unavailable";
import { eventKindLabel } from "@/lib/notification-kinds";
import {
  fetchProject,
  fetchProjectNotifications,
  type FeedItem,
  type NotificationFeed,
} from "@/lib/server/data";
import { cn } from "@/lib/utils";

import { Overview } from "../_overview";
import { MarkProjectRead } from "./_activity";

export const metadata = { title: "Overview" };

export default async function OverviewPage({
  params,
}: {
  params: Promise<{ projectId?: string }>;
}) {
  const { projectId = "" } = await params;
  const [project, activity] = await Promise.all([
    fetchProject(projectId),
    fetchProjectNotifications(projectId),
  ]);
  if (!project) return <ServiceUnavailable />;

  return (
    <Overview sections={<ProjectActivity projectId={projectId} feed={activity} />}>
      <h2 className="text-base font-semibold">{project.name}</h2>
    </Overview>
  );
}

// ⚠ **This is the only place a project's notices are readable.** `member_added`
// and the two connector kinds are written with a project on them, and the service
// answers them only to a request that carries `x-project-id` — so they never
// appear among the organization's own notices, and the header bell has no
// project to ask with. Removing this section makes every kind this repository
// raises unreachable again.
//
// A null feed is the service being unset or unreachable, which renders nothing
// rather than an error: the page's subject is the project, not its activity.
function ProjectActivity({
  projectId,
  feed,
}: {
  projectId: string;
  feed: NotificationFeed | null;
}) {
  if (feed === null) return null;

  return (
    <section className="grid gap-3" data-testid="project-activity">
      <div className="flex items-center justify-between gap-2">
        <h2 className="text-sm font-medium">Recent activity</h2>
        {feed.unread > 0 && (
          <MarkProjectRead projectId={projectId} unread={feed.unread} />
        )}
      </div>
      {feed.items.length === 0 ? (
        <p
          className="text-sm text-muted-foreground"
          data-testid="project-activity-empty"
        >
          Nothing yet. Members joining and channels connecting show up here.
        </p>
      ) : (
        <Rows>
          {feed.items.map((item) => (
            <ActivityRow key={item.id} item={item} />
          ))}
        </Rows>
      )}
    </section>
  );
}

function ActivityRow({ item }: { item: FeedItem }) {
  const unread = item.read_at === null;
  return (
    <Row
      label={
        <span className={cn("block min-w-0 truncate", !unread && "font-normal text-muted-foreground")}>
          <span>{item.title}</span>
          {unread && <span className="sr-only"> Unread</span>}
        </span>
      }
      hint={
        <span className="block">
          {item.body}
          <span className="mt-0.5 block text-xs opacity-70">
            {eventKindLabel(item.kind)}
          </span>
        </span>
      }
    >
      <LocalTime iso={item.created_at} mode="datetime" />
    </Row>
  );
}
