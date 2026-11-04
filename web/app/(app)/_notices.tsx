import type { ComponentProps, ReactNode } from "react";
import type { LucideIcon } from "lucide-react";
import Link from "next/link";

import { OverviewCard } from "@/components/overview-card";
import { LocalTime } from "@/components/local-time";
import { Row, Rows } from "@/components/rows";
import { eventKindLabel } from "@/lib/notification-kinds";
import { noticeHref, type NoticePlace } from "@/lib/notifications";
import type { FeedItem, NotificationFeed } from "@/lib/server/data";
import { cn } from "@/lib/utils";

// A feed's notices, newest first as the server sends them, each unread one
// bold until somebody marks the feed read, and each a link to the page it is
// about (`noticeHref`). The organization's own notices and a project's are
// different sets the server answers by scope, so each page reads its own and
// hands it here, and the two read alike. `Notices` is an overview's card of
// them, saying so when the feed is empty or could not be read (`null`);
// `NoticeRows` the list alone, as the feeds' own pages draw it.
export function Notices({
  label,
  icon,
  feed,
  action,
  place,
  links,
}: {
  label: string;
  icon?: LucideIcon;
  feed: NotificationFeed | null;
  action?: ReactNode;
  place: NoticePlace;
  links?: ComponentProps<typeof OverviewCard>["links"];
}) {
  return (
    <OverviewCard title={label} icon={icon} action={action} links={links}>
      <NoticeRows feed={feed} place={place} flush />
    </OverviewCard>
  );
}

/** The feed's rows, or what stands in for them. `flush` in a card: rows ruled
 *  apart, never a card in a card. */
export function NoticeRows({
  feed,
  place,
  flush = false,
}: {
  feed: NotificationFeed | null;
  place: NoticePlace;
  flush?: boolean;
}) {
  if (feed === null) {
    return <p className="text-sm text-muted-foreground">Could not be read just now. Reload to try again.</p>;
  }
  if (feed.items.length === 0) return <p className="text-sm text-muted-foreground">Nothing new.</p>;
  return (
    <Rows className={cn(flush && "rounded-none p-0 ring-0")}>
      {feed.items.map((item) => (
        <NoticeRow key={item.id} item={item} place={place} />
      ))}
    </Rows>
  );
}

function NoticeRow({ item, place }: { item: FeedItem; place: NoticePlace }) {
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
          <Link href={noticeHref(item, place)} className="underline-offset-4 hover:underline">
            {item.title}
          </Link>
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
