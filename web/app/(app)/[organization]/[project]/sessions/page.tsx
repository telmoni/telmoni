import { MessagesSquare } from "lucide-react";
import Link from "next/link";

import { TraceFirstRun } from "@/components/first-run";
import { ServiceUnavailable } from "@/components/service-unavailable";
import { ListPage } from "@/components/traces/list-page";
import { SessionTable } from "@/components/traces/session-table";
import { ListToolbar } from "@/components/traces/toolbar";
import { fetchSessionFacets, fetchSessions } from "@/lib/server/entities/sessions";
import { projectPage } from "@/lib/server/project-page";
import { parseSessionQuery } from "@/lib/sessions";
import { DEFAULT_ROWS, activeFilters, counted } from "@/lib/traces";
import { MAX_DAYS } from "@/lib/list-query";
import { administers } from "@/lib/roles";
import { projectPath } from "@/lib/slug";

export const metadata = { title: "Sessions" };

// The sessions list, laid out as the trace list is, all of it in the URL: a
// row per session in a window of days, admitted by any of its traces the
// filters admit and summed over all of them. A session opens its own page,
// and its user theirs. Nothing here is a message: the server sums spans.
export default async function SessionsPage({
  params,
  searchParams,
}: PageProps<"/[organization]/[project]/sessions">) {
  const { organization, project: slug } = await params;
  const query = parseSessionQuery(await searchParams);
  const resolved = await projectPage(slug);
  if (resolved.kind === "unavailable") return <ServiceUnavailable />;
  const { gate, project } = resolved;

  const [access, facets] = await Promise.all([
    fetchSessions(project.id, { ...query, limit: query.limit ?? DEFAULT_ROWS }),
    fetchSessionFacets(project.id, query),
  ]);
  const ever =
    access.kind === "ok" && access.list.total === 0 && activeFilters(query).length === 0
      ? await fetchSessions(project.id, { ...parseSessionQuery({}), days: MAX_DAYS, limit: 1 })
      : null;
  const firstRun = ever?.kind === "ok" && ever.list.total === 0;

  return (
    <ListPage
      list="sessions"
      title="Sessions"
      info="Every session this project's agents took part in, the one last heard from first: its agents, its traces and those that failed, how long they ran and what they cost. A session is the traces that share the id your application gave them, which is stored hashed. Click a session to open its page, its user to open theirs, a value to narrow the list to it, a heading to sort by it."
      icon={MessagesSquare}
      organization={organization}
      slug={slug}
      projectId={project.id}
      gate={gate}
      query={query}
      access={access}
      facets={facets.kind === "ok" ? facets.facets : null}
      rows={(list) => list.sessions.length}
      toolbar={(list, days) => (
        <ListToolbar
          list="sessions"
          days={days}
          organization={organization}
          slug={slug}
          query={query}
          total={list.total}
        />
      )}
      firstRun={firstRun ? <TraceFirstRun base={projectPath(organization, slug)} canMint={administers(project.role)} kind="sessions" /> : undefined}
      nothingYet={(days) => (
        <>
          No trace from the last {counted(days, "day", "days")} named the session it belongs to. Name one
          when you open a trace — <code>session_id</code> in the{" "}
          <Link href="/docs/api/sdks" className="underline underline-offset-4">
            SDKs
          </Link>
          , <code>gen_ai.conversation.id</code> on the span — and the traces that share it list here as one
          session. A{" "}
          <Link href="/docs/api/coding-agents" className="underline underline-offset-4">
            coding-agent plugin
          </Link>{" "}
          names each of its sessions itself.
        </>
      )}
      table={(list) => (
        <SessionTable organization={organization} slug={slug} query={query} sessions={list.sessions} />
      )}
    />
  );
}
