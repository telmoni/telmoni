import Link from "next/link";

import { AccessDenied } from "@/components/access-denied";
import { DataTable, THead, Td } from "@/components/data-table";
import { LinkPending } from "@/components/link-pending";
import { LocalTime } from "@/components/local-time";
import { PageHeader } from "@/components/page-header";
import { ServiceUnavailable } from "@/components/service-unavailable";
import { SortHeading } from "@/components/sort-heading";
import { Card } from "@/components/ui/card";
import { WindowPicker } from "@/components/window-picker";
import { ownerContact } from "@/lib/identity";
import { LIST_DIMS_WHILE_PENDING } from "@/lib/list-pending";
import { MAX_LIMIT, type SearchParams } from "@/lib/list-query";
import {
  activeOrganization,
  fetchProjectBySlug,
  fetchSessions,
  getServerContext,
  type SessionQuery,
  type SessionRow,
} from "@/lib/server/data";
import {
  SESSION_COLUMNS,
  currentSessionSort,
  nextSessionSort,
  parseSessionQuery,
  sessionHref,
  sessionsHref,
} from "@/lib/sessions";
import { projectPath } from "@/lib/slug";
import { formatCount, tracesHref } from "@/lib/traces";

export const metadata = { title: "Sessions" };

// The sessions list: a row per session this project's agents took part in
// within a window of days, by the id the application gave it, with the user
// it was with and what its runs added up to. Every column is a sort, carried
// in the URL; a session opens its own page, a replay of its runs in metadata.
// Nothing here is a message: the server sums spans.
export default async function SessionsPage({
  params,
  searchParams,
}: {
  params: Promise<{ organization: string; project: string }>;
  searchParams?: Promise<SearchParams>;
}) {
  const { organization, project: slug } = await params;
  const query = parseSessionQuery((await searchParams) ?? {});
  const [gate, project] = await Promise.all([getServerContext(), fetchProjectBySlug(slug)]);
  if (!gate || !project) return <ServiceUnavailable />;

  const access = await fetchSessions(project.id, query);
  if (access.kind === "unavailable") return <ServiceUnavailable />;
  if (access.kind === "forbidden") {
    return (
      <AccessDenied
        what="this project's sessions"
        owner={ownerContact(activeOrganization(gate))}
        backHref={projectPath(organization, slug)}
      />
    );
  }

  const { list } = access;
  const { sort, dir } = currentSessionSort(query);
  const href = (patch: Partial<SessionQuery>) => sessionsHref(organization, slug, query, patch);
  const days = query.days ?? list.days;

  return (
    <>
      <PageHeader title="Sessions" count={list.more ? undefined : list.sessions.length} />
      <div className="grid gap-6">
        {/* The window, the headings and the link to more mark themselves
            pending while the list they asked for is on its way, and the
            list dims under them. */}
        <section className="group/list grid gap-3">
          <p className="text-sm text-muted-foreground">
            Every session this project&rsquo;s agents took part in, last heard from first, with the
            user it was with and its traces, errors, steps and tokens. Click a session to replay
            its runs; click a heading to sort by it.
          </p>

          <div>
            <WindowPicker days={days} />
          </div>

          {list.sessions.length === 0 ? (
            <Card className="overflow-hidden p-0 gap-0">
              <div className="grid gap-1 p-4">
                <p className="text-sm font-medium">Nothing to show yet.</p>
                <p className="text-sm text-muted-foreground">
                  No run from the last {days} {days === 1 ? "day" : "days"} named the session it
                  belonged to. Name one when you open a run — <code>session_id</code> in the{" "}
                  <Link href="/docs/api/sdks" className="underline underline-offset-4">
                    SDKs
                  </Link>
                  , <code>gen_ai.conversation.id</code> on the span — and the runs that share it
                  group here as a session.
                </p>
              </div>
            </Card>
          ) : (
            <DataTable className={LIST_DIMS_WHILE_PENDING}>
              <THead>
                {SESSION_COLUMNS.map((column) => (
                  <SortHeading
                    key={column.sort}
                    label={column.label}
                    column={column.sort}
                    sort={sort}
                    dir={dir}
                    href={href(nextSessionSort(query, column.sort))}
                  />
                ))}
              </THead>
              <tbody>
                {list.sessions.map((s) => (
                  <SessionTableRow
                    key={s.session_id}
                    session={s}
                    open={sessionHref(organization, slug, s.session_id, {
                      days: query.days,
                    })}
                    user={tracesHref(organization, slug, { user: [s.user_id], days: query.days })}
                  />
                ))}
              </tbody>
            </DataTable>
          )}

          {list.more && (
            <p className="text-sm text-muted-foreground">
              The first {formatCount(list.limit)} of more in this window.{" "}
              {list.limit < MAX_LIMIT ? (
                <>
                  <Link
                    href={href({ limit: MAX_LIMIT })}
                    scroll={false}
                    className="underline underline-offset-4"
                  >
                    Show up to {formatCount(MAX_LIMIT)}
                    <LinkPending />
                  </Link>
                  , or narrow the window.
                </>
              ) : (
                "Narrow the window to see the rest."
              )}
            </p>
          )}
        </section>
      </div>
    </>
  );
}

function SessionTableRow({
  session,
  open,
  user,
}: {
  session: SessionRow;
  open: string;
  user: string;
}) {
  return (
    <tr>
      <Td className="max-w-64 truncate font-mono text-xs" title={session.session_id}>
        <Link href={open} className="underline-offset-4 hover:underline">
          {session.session_id}
        </Link>
      </Td>
      <Td className="max-w-48 truncate font-mono text-xs" title={session.user_id || undefined}>
        {session.user_id ? (
          <Link href={user} className="underline-offset-4 hover:underline">
            {session.user_id}
          </Link>
        ) : (
          <span className="text-muted-foreground">—</span>
        )}
      </Td>
      <Td className="whitespace-nowrap text-right tabular-nums">{formatCount(session.traces)}</Td>
      <Td
        className={`whitespace-nowrap text-right tabular-nums ${session.errors > 0 ? "text-destructive" : "text-muted-foreground"}`}
      >
        {formatCount(session.errors)}
      </Td>
      <Td className="whitespace-nowrap text-right tabular-nums">{formatCount(session.steps)}</Td>
      <Td
        className="whitespace-nowrap text-right tabular-nums"
        title={`${formatCount(session.input_tokens)} in, ${formatCount(session.output_tokens)} out`}
      >
        {formatCount(session.input_tokens + session.output_tokens)}
      </Td>
      <Td className="whitespace-nowrap text-xs text-muted-foreground">
        <LocalTime iso={session.first_seen} />
      </Td>
      <Td className="whitespace-nowrap text-xs text-muted-foreground">
        <LocalTime iso={session.last_seen} />
      </Td>
    </tr>
  );
}
