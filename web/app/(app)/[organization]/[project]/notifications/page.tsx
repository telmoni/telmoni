import { MarkRead } from "@/components/mark-read";
import { PageHeader } from "@/components/page-header";
import { ServiceUnavailable } from "@/components/service-unavailable";
import { fetchProjectNotifications } from "@/lib/server/data";
import { projectPage } from "@/lib/server/project-page";

import { NoticeRows } from "../../../_notices";
import { markProjectReadAction } from "../notice-actions";

export const metadata = { title: "Notifications" };

// The project's notices whole, newest first: what its alerts, members and
// connectors raised, each leading to the page it is about, read by anyone on
// the project and marked read for all of them.
export default async function ProjectNotificationsPage({
  params,
}: PageProps<"/[organization]/[project]/notifications">) {
  const { organization, project: slug } = await params;
  const resolved = await projectPage(slug);
  if (resolved.kind === "unavailable") return <ServiceUnavailable />;
  const { project } = resolved;
  const feed = await fetchProjectNotifications(project.id);
  const newest = feed?.items[0];

  return (
    <>
      <PageHeader
        title="Notifications"
        info="What happened in this project — its alerts' incidents, members joining and leaving, connectors connected or lost — newest first. Marking them read marks them for everyone on the project."
        action={
          feed && newest && feed.unread > 0 ? (
            <MarkRead unread={feed.unread} mark={markProjectReadAction.bind(null, project.id, newest.created_at)} />
          ) : undefined
        }
      />
      <div className="grid gap-page">
        <section className="grid gap-gutter" aria-label="The project's notices">
          <NoticeRows feed={feed} place={{ organization, project: slug }} />
        </section>
      </div>
    </>
  );
}
