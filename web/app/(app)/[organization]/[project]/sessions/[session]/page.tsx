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
import { ownerContact } from "@/lib/identity";
import { MAX_LIMIT, parseWindow, type SearchParams } from "@/lib/list-query";
import {
  activeOrganization,
  fetchProjectBySlug,
  fetchSession,
  getServerContext,
  type Trace,
} from "@/lib/server/data";
import { sessionHref, sessionsHref } from "@/lib/sessions";
import { projectPath } from "@/lib/slug";
import { formatCount, formatDuration, shortId, traceHref, tracesHref } from "@/lib/traces";

export const metadata = { title: "Session" };

// One session: its runs in the window, in the order they ran, each summed as
// the trace list sums it — a replay in metadata, never a message. The session
// id is the application's own text, up to 200 characters: the path carries it
// URL-encoded, and Next hands it here decoded, as the fetcher re-encodes it
// for the server.
export default async function SessionPage({
  params,
  searchParams,
}: {
  params: Promise<{ organization: string; project: string; session: string }>;
  searchParams?: Promise<SearchParams>;
}) {
  const { organization, project: slug, session: sessionId } = await params;
  const query = parseWindow((await searchParams) ?? {});
  const [gate, project] = await Promise.all([getServerContext(), fetchProjectBySlug(slug)]);
  if (!gate || !project) return <ServiceUnavailable />;

  const list = sessionsHref(organization, slug, { days: query.days });
  const access = await fetchSession(project.id, sessionId, query);
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
  if (access.kind === "missing") {
    return (
      <>
        <PageHeader title="Session" backHref={list} backLabel="Sessions" />
        <Card className="grid gap-1 text-sm">
          <p className="font-medium">No session with this id in this project&rsquo;s window.</p>
          <p className="text-muted-foreground">
            Its runs may lie outside the window or have passed their retention, or the session
            belongs to another project.{" "}
            <Link href={list} className="underline underline-offset-4">
              Back to the sessions
            </Link>
            .
          </p>
        </Card>
      </>
    );
  }

  const { session } = access;
  const traces = session.traces;
  const first = traces[0]!;
  const user = traces.find((t) => t.user_id)?.user_id ?? "";
  // As the list counts a session's errors: the runs with a span that failed,
  // a run that recovered from a failed call among them.
  const errors = traces.filter((t) => t.errors > 0).length;
  const steps = traces.reduce((sum, t) => sum + t.steps, 0);
  const inputTokens = traces.reduce((sum, t) => sum + t.input_tokens, 0);
  const outputTokens = traces.reduce((sum, t) => sum + t.output_tokens, 0);
  const lastEnded = traces.reduce(
    (latest, t) => (t.ended_at > latest ? t.ended_at : latest),
    first.ended_at,
  );
  const days = query.days ?? session.days;
  const here = (patch: { days?: number; limit?: number }) =>
    sessionHref(organization, slug, session.session_id, { ...query, ...patch });

  return (
    <>
      <PageHeader title="Session" backHref={list} backLabel="Sessions" />
      <div className="grid gap-6">
        <Section
          title="Session"
          description="Who it was with and what its runs added up to, from their spans alone."
        >
          <Rows>
            <Row label="Session" hint="As the application named it.">
              <code className="max-w-96 truncate text-xs" title={session.session_id}>
                {session.session_id}
              </code>
            </Row>
            <Row label="User" hint="The end user, as stored.">
              {user ? (
                <Link
                  href={tracesHref(organization, slug, { user, days: query.days })}
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
                {session.more ? " shown" : ""}
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
          description="Each run of the session in the order it started, with its agent, status, steps and tokens."
        >
          <WindowForm
            action={sessionHref(organization, slug, session.session_id)}
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
          {session.more && (
            <p className="text-sm text-muted-foreground">
              The first {formatCount(session.limit)} of more in this window.{" "}
              {session.limit < MAX_LIMIT ? (
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
