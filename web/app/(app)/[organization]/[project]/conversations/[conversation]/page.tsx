import Link from "next/link";

import { AccessDenied } from "@/components/access-denied";
import { DataTable, THead, Th, Td } from "@/components/data-table";
import { LocalTime } from "@/components/local-time";
import { PageHeader } from "@/components/page-header";
import { Row, Rows } from "@/components/rows";
import { Section } from "@/components/section";
import { ServiceUnavailable } from "@/components/service-unavailable";
import { TraceStatusBadge } from "@/components/trace-status-badge";
import { Card } from "@/components/ui/card";
import { WindowForm } from "@/components/window-form";
import { conversationHref, conversationsHref } from "@/lib/conversations";
import { ownerContact } from "@/lib/identity";
import { MAX_LIMIT, parseWindow, type SearchParams } from "@/lib/list-query";
import {
  activeOrganization,
  fetchConversation,
  fetchProjectBySlug,
  getServerContext,
  type Trace,
} from "@/lib/server/data";
import { projectPath } from "@/lib/slug";
import { formatCount, formatDuration, shortId, traceHref, tracesHref } from "@/lib/traces";
import { SpanStatus } from "@/lib/types/enums";

export const metadata = { title: "Conversation" };

// One conversation: its runs in the window, in the order they ran, each
// summed as the trace list sums it — a replay in metadata, never a message.
// The conversation id is the application's own text, up to 200 characters:
// the path carries it URL-encoded, and Next hands it here decoded, as the
// fetcher re-encodes it for the server.
export default async function ConversationPage({
  params,
  searchParams,
}: {
  params: Promise<{ organization: string; project: string; conversation: string }>;
  searchParams?: Promise<SearchParams>;
}) {
  const { organization, project: slug, conversation: conversationId } = await params;
  const query = parseWindow((await searchParams) ?? {});
  const [gate, project] = await Promise.all([getServerContext(), fetchProjectBySlug(slug)]);
  if (!gate || !project) return <ServiceUnavailable />;

  const list = conversationsHref(organization, slug, {});
  const access = await fetchConversation(project.id, conversationId, query);
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
  if (access.kind === "missing") {
    return (
      <>
        <PageHeader title="Conversation" backHref={list} backLabel="Conversations" />
        <Card className="grid gap-1 text-sm">
          <p className="font-medium">
            No conversation with this id in this project&rsquo;s window.
          </p>
          <p className="text-muted-foreground">
            Its runs may lie outside the window or have passed their retention, or the
            conversation belongs to another project.{" "}
            <Link href={list} className="underline underline-offset-4">
              Back to the conversations
            </Link>
            .
          </p>
        </Card>
      </>
    );
  }

  const { conversation } = access;
  const traces = conversation.traces;
  const first = traces[0]!;
  const user = traces.find((t) => t.user_id)?.user_id ?? "";
  const errors = traces.filter((t) => t.ended && t.status === SpanStatus.Error).length;
  const steps = traces.reduce((sum, t) => sum + t.steps, 0);
  const inputTokens = traces.reduce((sum, t) => sum + t.input_tokens, 0);
  const outputTokens = traces.reduce((sum, t) => sum + t.output_tokens, 0);
  const lastEnded = traces.reduce(
    (latest, t) => (t.ended_at > latest ? t.ended_at : latest),
    first.ended_at,
  );
  const days = query.days ?? conversation.days;
  const here = (patch: { days?: number; limit?: number }) =>
    conversationHref(organization, slug, conversation.conversation_id, { ...query, ...patch });

  return (
    <>
      <PageHeader title="Conversation" backHref={list} backLabel="Conversations" />
      <div className="grid gap-6">
        <Section
          title="Conversation"
          description="Who it was with and what its runs added up to, from their spans alone."
        >
          <Rows>
            <Row label="Conversation" hint="As the application named it.">
              <code className="max-w-96 truncate text-xs" title={conversation.conversation_id}>
                {conversation.conversation_id}
              </code>
            </Row>
            <Row label="User" hint="The end user, as stored.">
              {user ? (
                <Link
                  href={tracesHref(organization, slug, { user })}
                  className="font-mono text-xs underline-offset-4 hover:underline"
                >
                  {user}
                </Link>
              ) : (
                <span className="text-sm text-muted-foreground">—</span>
              )}
            </Row>
            <Row label="Traces">
              <span className="text-sm tabular-nums">
                {formatCount(traces.length)}
                {conversation.more ? " shown" : ""}
                {errors > 0 && (
                  <span className="text-destructive">
                    , {formatCount(errors)} in error
                  </span>
                )}
              </span>
            </Row>
            <Row label="Steps">
              <span className="text-sm tabular-nums">{formatCount(steps)}</span>
            </Row>
            <Row label="Tokens">
              <span className="text-sm tabular-nums">
                {formatCount(inputTokens + outputTokens)}{" "}
                <span className="text-muted-foreground">
                  ({formatCount(inputTokens)} in, {formatCount(outputTokens)} out)
                </span>
              </span>
            </Row>
            <Row label="First run">
              <span className="text-sm text-muted-foreground">
                <LocalTime iso={first.started_at} />
              </span>
            </Row>
            <Row label="Last run ended">
              <span className="text-sm text-muted-foreground">
                <LocalTime iso={lastEnded} />
              </span>
            </Row>
          </Rows>
        </Section>

        <Section
          title="Runs"
          description="Each run of the conversation in the order it started, with its agent, status, steps and tokens."
        >
          <WindowForm
            action={conversationHref(organization, slug, conversation.conversation_id)}
            days={days}
            label="Choose the window"
          />
          <DataTable>
            <THead>
              <Th className="whitespace-nowrap">Trace</Th>
              <Th className="whitespace-nowrap">Started</Th>
              <Th className="whitespace-nowrap">Agent</Th>
              <Th className="whitespace-nowrap">Status</Th>
              <Th className="whitespace-nowrap">Steps</Th>
              <Th className="whitespace-nowrap">Tokens</Th>
              <Th className="whitespace-nowrap">Duration</Th>
            </THead>
            <tbody>
              {traces.map((t) => (
                <RunRow key={t.run_id} trace={t} open={traceHref(organization, slug, t.run_id)} />
              ))}
            </tbody>
          </DataTable>
          {conversation.more && (
            <p className="text-sm text-muted-foreground">
              The first {formatCount(conversation.limit)} of more in this window.{" "}
              {conversation.limit < MAX_LIMIT ? (
                <>
                  <Link href={here({ limit: MAX_LIMIT })} className="underline underline-offset-4">
                    Show up to {formatCount(MAX_LIMIT)}
                  </Link>
                  , or narrow the window.
                </>
              ) : (
                "Narrow the window to see the rest."
              )}
            </p>
          )}
        </Section>
      </div>
    </>
  );
}

function RunRow({ trace, open }: { trace: Trace; open: string }) {
  return (
    <tr>
      <Td className="whitespace-nowrap">
        <Link href={open} className="font-mono text-xs underline-offset-4 hover:underline">
          {shortId(trace.run_id)}
        </Link>
      </Td>
      <Td className="whitespace-nowrap text-xs text-muted-foreground">
        <LocalTime iso={trace.started_at} />
      </Td>
      <Td className="whitespace-nowrap font-medium">{trace.agent}</Td>
      <Td className="whitespace-nowrap">
        <TraceStatusBadge trace={trace} />
      </Td>
      <Td
        className="whitespace-nowrap text-right tabular-nums"
        title={`${formatCount(trace.model_calls)} model calls, ${formatCount(trace.tool_calls)} tool calls`}
      >
        {formatCount(trace.steps)}
      </Td>
      <Td
        className="whitespace-nowrap text-right tabular-nums"
        title={`${formatCount(trace.input_tokens)} in, ${formatCount(trace.output_tokens)} out`}
      >
        {formatCount(trace.input_tokens + trace.output_tokens)}
      </Td>
      <Td className="whitespace-nowrap text-right tabular-nums">
        {formatDuration(trace.duration_ms)}
      </Td>
    </tr>
  );
}
