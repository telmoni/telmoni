import { LocalTime } from "@/components/local-time";
import { Row, Rows } from "@/components/rows";
import { eventKindLabel } from "@/lib/notification-kinds";
import type { FeedItem, NotificationFeed } from "@/lib/server/data";
import { cn } from "@/lib/utils";

// A feed's notices as an overview shows them, newest first as the server
// sends them, each unread one bold until somebody marks the feed read. The
// organization's own notices and a project's are different sets the server
// answers by scope, so each overview reads its own and hands it here, and
// the two read alike.
export function Notices({ label, feed }: { label: string; feed: NotificationFeed }) {
  return (
    <section className="grid gap-3" aria-label={label}>
      <div className="flex items-center justify-between gap-2">
        <h2 className="text-sm font-medium">{label}</h2>
      </div>
      <Rows>
        {feed.items.map((item) => (
          <NoticeRow key={item.id} item={item} />
        ))}
      </Rows>
    </section>
  );
}

function NoticeRow({ item }: { item: FeedItem }) {
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
