import Link from "next/link";

import { AccessDenied } from "@/components/access-denied";
import { LocalTime } from "@/components/local-time";
import { PageHeader } from "@/components/page-header";
import { Row, Rows } from "@/components/rows";
import { Section } from "@/components/section";
import { ServiceUnavailable } from "@/components/service-unavailable";
import { LayoutControls } from "@/components/traces/layout-controls";
import { SessionRuns } from "@/components/traces/session-runs";
import { LayoutProvider } from "@/components/traces/use-layout";
import { Card } from "@/components/ui/card";
import { WindowPicker } from "@/components/window-picker";
import { ownerContact } from "@/lib/identity";
import { parseLayoutUi, resolveLayout } from "@/lib/layout";
import { MAX_LIMIT, parseWindow } from "@/lib/list-query";
import { administers } from "@/lib/roles";
import {
  activeOrganization,
  fetchSession,
  fetchSessionLayout,
  fetchTraceLayouts,
} from "@/lib/server/data";
import { projectPage } from "@/lib/server/project-page";
import { sessionHref, sessionSums, sessionsHref } from "@/lib/sessions";
import { projectPath } from "@/lib/slug";
import { formatCount, tracesHref } from "@/lib/traces";

export const metadata = { title: "Session" };

// One session: its runs in the window, in the order they ran, each summed as
// the trace list sums it — a replay in metadata, never a message — under the
// layout the page stands in: a turn of a conversation a line, or each
// trace's steps and calls, as Customize has it (`lib/layout.ts`). The list
// opens a session in the console's side panel (`components/traces/
// session-peek.tsx`); this page is the same session with the page's room.
// The session id is the application's own text, up to
// 200 characters: the path carries it URL-encoded, and Next hands it here
// decoded, as the fetcher re-encodes it for the server.
export default async function SessionPage({
  params,
  searchParams,
}: PageProps<"/[organization]/[project]/sessions/[session]">) {
  const { organization, project: slug, session: sessionId } = await params;
  const given = await searchParams;
  const query = parseWindow(given);
  const ui = parseLayoutUi(given);
  const resolved = await projectPage(slug);
  if (resolved.kind === "unavailable") return <ServiceUnavailable />;
  const { gate, project } = resolved;

  // The layout the server can know — a link's, or the project's — names the
  // spans to read with the session; the browser's own last choice is read
  // again in the page (`components/traces/session-runs.tsx`).
  const [setting, layouts] = await Promise.all([
    fetchSessionLayout(project.id),
    fetchTraceLayouts(project.id),
  ]);
  const known = layouts.kind === "ok" ? layouts.layouts : [];
  const { pinned } = resolveLayout(ui, null, setting, known).choice.options;

  const list = sessionsHref(organization, slug, { days: query.days });
  const access = await fetchSession(project.id, sessionId, query, pinned);
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
  const { user, errors, steps, inputTokens, outputTokens, lastEnded } = sessionSums(traces);
  const days = query.days ?? session.days;
  const here = (patch: { days?: number; limit?: number }) =>
    sessionHref(organization, slug, session.session_id, { ...query, ...patch });
  const page = { organization, slug, projectId: project.id };

  return (
    <LayoutProvider layouts={layouts.kind === "ok" ? layouts.layouts : null} project={setting}>
      <PageHeader
        title="Session"
        backHref={list}
        backLabel="Sessions"
        action={<LayoutControls projectId={project.id} canSave={administers(project.role)} />}
      />
      <div className="grid gap-6">
        <Section
          title="Session"
          description="Who it was with and what its runs added up to, from their spans alone."
        >
          <Rows>
            <Row label="Session" hint="As the application named it.">
              <span className="max-w-96 truncate text-xs" title={session.session_id}>
                {session.session_id}
              </span>
            </Row>
            <Row label="User" hint="The end user, as stored.">
              {user ? (
                <Link
                  href={tracesHref(organization, slug, { user: [user], days: query.days })}
                  className="text-xs underline-offset-4 hover:underline"
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
                    , {formatCount(errors)} with failed spans
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
          description="Each run of the session in the order it started, as the layout lays it out; j and k step through them, and o opens one where the layout opens its steps."
        >
          <div>
            <WindowPicker days={days} />
          </div>
          <SessionRuns
            {...page}
            sessionId={session.session_id}
            traces={traces}
            readWith={session.pinned}
            window={query}
          />
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
    </LayoutProvider>
  );
}
