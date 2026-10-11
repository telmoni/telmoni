import Link from "next/link";

import { AccessDenied } from "@/components/access-denied";
import { DataTable, THead, Td } from "@/components/data-table";
import { LocalTime } from "@/components/local-time";
import { PageHeader } from "@/components/page-header";
import { ServiceUnavailable } from "@/components/service-unavailable";
import { SortHeading } from "@/components/sort-heading";
import { Card } from "@/components/ui/card";
import { WindowForm } from "@/components/window-form";
import {
  CONVERSATION_COLUMNS,
  conversationHref,
  conversationsHref,
  currentConversationSort,
  nextConversationSort,
  parseConversationQuery,
} from "@/lib/conversations";
import { ownerContact } from "@/lib/identity";
import { MAX_LIMIT, type SearchParams } from "@/lib/list-query";
import {
  activeOrganization,
  fetchConversations,
  fetchProjectBySlug,
  getServerContext,
  type ConversationQuery,
  type ConversationRow,
} from "@/lib/server/data";
import { projectPath } from "@/lib/slug";
import { formatCount, tracesHref } from "@/lib/traces";

export const metadata = { title: "Conversations" };

// The conversations list: a row per conversation this project's agents took
// part in within a window of days, by the id the application gave it, with
// the user it was with and what its runs added up to. Every column is a
// sort, carried in the URL; a conversation opens its own page, a replay of
// its runs in metadata. Nothing here is a message: the server sums spans.
export default async function ConversationsPage({
  params,
  searchParams,
}: {
  params: Promise<{ organization: string; project: string }>;
  searchParams?: Promise<SearchParams>;
}) {
  const { organization, project: slug } = await params;
  const query = parseConversationQuery((await searchParams) ?? {});
  const [gate, project] = await Promise.all([getServerContext(), fetchProjectBySlug(slug)]);
  if (!gate || !project) return <ServiceUnavailable />;

  const access = await fetchConversations(project.id, query);
  if (access.kind === "unavailable") return <ServiceUnavailable />;
  if (access.kind === "forbidden") {
    return (
      <AccessDenied
        what="this project's conversations"
        owner={ownerContact(activeOrganization(gate))}
        backHref={projectPath(organization, slug)}
      />
    );
  }

  const { list } = access;
  const { sort, dir } = currentConversationSort(query);
  const href = (patch: Partial<ConversationQuery>) =>
    conversationsHref(organization, slug, query, patch);
  const days = query.days ?? list.days;

  return (
    <>
      <PageHeader
        title="Conversations"
        count={list.more ? undefined : list.conversations.length}
      />
      <div className="grid gap-6">
        <section className="grid gap-3">
          <p className="text-sm text-muted-foreground">
            Every conversation this project&rsquo;s agents took part in, last heard from first,
            with the user it was with and its traces, errors, steps and tokens. Click a
            conversation to replay its runs; click a heading to sort by it.
          </p>

          <WindowForm
            action={projectPath(organization, slug, "/conversations")}
            days={days}
            carry={{ sort: query.sort, dir: query.dir }}
            label="Choose the window"
          />

          {list.conversations.length === 0 ? (
            <Card className="overflow-hidden p-0 gap-0">
              <div className="grid gap-1 p-4">
                <p className="text-sm font-medium">Nothing to show yet.</p>
                <p className="text-sm text-muted-foreground">
                  No run from the last {days} {days === 1 ? "day" : "days"} named the conversation
                  it belonged to. Name one when you open a run — <code>conversation_id</code> in
                  the{" "}
                  <Link href="/docs/api/sdks" className="underline underline-offset-4">
                    SDKs
                  </Link>
                  , <code>gen_ai.conversation.id</code> on the span — and the runs that share it
                  group here as a conversation.
                </p>
              </div>
            </Card>
          ) : (
            <DataTable>
              <THead>
                {CONVERSATION_COLUMNS.map((column) => (
                  <SortHeading
                    key={column.sort}
                    label={column.label}
                    column={column.sort}
                    sort={sort}
                    dir={dir}
                    href={href(nextConversationSort(query, column.sort))}
                  />
                ))}
              </THead>
              <tbody>
                {list.conversations.map((c) => (
                  <ConversationTableRow
                    key={c.conversation_id}
                    conversation={c}
                    open={conversationHref(organization, slug, c.conversation_id)}
                    user={tracesHref(organization, slug, { user: c.user_id })}
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
                  <Link href={href({ limit: MAX_LIMIT })} className="underline underline-offset-4">
                    Show up to {formatCount(MAX_LIMIT)}
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

function ConversationTableRow({
  conversation,
  open,
  user,
}: {
  conversation: ConversationRow;
  open: string;
  user: string;
}) {
  return (
    <tr>
      <Td className="max-w-64 truncate font-mono text-xs" title={conversation.conversation_id}>
        <Link href={open} className="underline-offset-4 hover:underline">
          {conversation.conversation_id}
        </Link>
      </Td>
      <Td className="max-w-48 truncate font-mono text-xs" title={conversation.user_id || undefined}>
        {conversation.user_id ? (
          <Link href={user} className="underline-offset-4 hover:underline">
            {conversation.user_id}
          </Link>
        ) : (
          <span className="text-muted-foreground">—</span>
        )}
      </Td>
      <Td className="whitespace-nowrap text-right tabular-nums">
        {formatCount(conversation.traces)}
      </Td>
      <Td
        className={`whitespace-nowrap text-right tabular-nums ${conversation.errors > 0 ? "text-destructive" : "text-muted-foreground"}`}
      >
        {formatCount(conversation.errors)}
      </Td>
      <Td className="whitespace-nowrap text-right tabular-nums">
        {formatCount(conversation.steps)}
      </Td>
      <Td
        className="whitespace-nowrap text-right tabular-nums"
        title={`${formatCount(conversation.input_tokens)} in, ${formatCount(conversation.output_tokens)} out`}
      >
        {formatCount(conversation.input_tokens + conversation.output_tokens)}
      </Td>
      <Td className="whitespace-nowrap text-xs text-muted-foreground">
        <LocalTime iso={conversation.first_seen} />
      </Td>
      <Td className="whitespace-nowrap text-xs text-muted-foreground">
        <LocalTime iso={conversation.last_seen} />
      </Td>
    </tr>
  );
}
