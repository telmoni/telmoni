import type { ReactNode } from "react";
import type { LucideIcon } from "lucide-react";
import Link from "next/link";

import { OverviewCard } from "@/components/overview-card";
import { LocalTime } from "@/components/local-time";
import { Row, Rows } from "@/components/rows";
import { incidentHref } from "@/lib/alerts";
import { eventKindLabel } from "@/lib/notification-kinds";
import type { FeedItem, NotificationFeed } from "@/lib/server/data";
import { cn } from "@/lib/utils";

/** The project a feed is of, by the slugs an incident's link is made of. */
type ProjectAt = { organization: string; slug: string };

// A feed's notices as an overview shows them, newest first as the server
// sends them, each unread one bold until somebody marks the feed read. The
// organization's own notices and a project's are different sets the server
// answers by scope, so each overview reads its own and hands it here, and
// the two read alike. `action` is the feed's own, Mark read, in its heading
// row beside what it clears. A project's feed names its project too, so an
// incident's notice links to the incident. A card of its own, saying so when
// the feed is empty or could not be read (`null`).
export function Notices({
  label,
  icon,
  feed,
  action,
  project,
}: {
  label: string;
  icon?: LucideIcon;
  feed: NotificationFeed | null;
  action?: ReactNode;
  project?: ProjectAt;
}) {
  return (
    <OverviewCard title={label} icon={icon} action={action}>
      {feed === null ? (
        <p className="text-muted-foreground">Could not be read just now. Reload to try again.</p>
      ) : feed.items.length === 0 ? (
        <p className="text-muted-foreground">Nothing new.</p>
      ) : (
        // Flush in the card: rows ruled apart, never a card in a card.
        <Rows className="rounded-none p-0 ring-0">
          {feed.items.map((item) => (
            <NoticeRow key={item.id} item={item} project={project} />
          ))}
        </Rows>
      )}
    </OverviewCard>
  );
}

function NoticeRow({ item, project }: { item: FeedItem; project?: ProjectAt }) {
  const unread = item.read_at === null;
  const incident =
    project && item.metadata
      ? incidentHref(project.organization, project.slug, item.metadata.incident_id)
      : null;
  return (
    <Row
      label={
        <span
          className={cn(
            "block min-w-0 truncate",
            unread ? "font-bold text-foreground" : "font-normal text-muted-foreground",
          )}
        >
          {incident ? (
            <Link href={incident} className="underline-offset-4 hover:underline">
              {item.title}
            </Link>
          ) : (
            <span>{item.title}</span>
          )}
          {unread && <span className="sr-only"> Unread</span>}
        </span>
      }
      hint={
        <span className="block">
          {/* Kept as lines, and an address, one long word, broken at the row's edge. */}
          <span
            className={cn(
              "whitespace-pre-line wrap-anywhere",
              unread ? "text-foreground/80" : "text-muted-foreground",
            )}
          >
            {item.body}
          </span>
          {/* The muted colour alone: dimmed further, 12px text falls under 4.5:1. */}
          <span className="mt-0.5 block text-xs">{eventKindLabel(item.kind)}</span>
        </span>
      }
    >
      <span className={cn(unread ? "font-medium text-foreground" : "text-muted-foreground")}>
        <LocalTime iso={item.created_at} mode="datetime" />
      </span>
    </Row>
  );
}
