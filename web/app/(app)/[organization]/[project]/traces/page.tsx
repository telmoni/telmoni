import { ListTree } from "lucide-react";
import Link from "next/link";
import { cookies } from "next/headers";

import { TraceFirstRun } from "@/components/first-run";
import { ServiceUnavailable } from "@/components/service-unavailable";
import { TraceHistogramStrip } from "@/components/traces/charts";
import { ListPage } from "@/components/traces/list-page";
import { TracePeek } from "@/components/traces/peek";
import { TraceTable } from "@/components/traces/table";
import { ListToolbar } from "@/components/traces/toolbar";
import { UtcOffsetCookie } from "@/components/traces/utc-offset";
import { administers } from "@/lib/roles";
import {
  fetchTraceFacets,
  fetchTraceHistogram,
  fetchTraces,
} from "@/lib/server/data";
import { fetchChanges } from "@/lib/server/entities/changes";
import { projectPage } from "@/lib/server/project-page";
import { fetchContentMode } from "@/lib/server/entities/content-mode";
import { projectPath } from "@/lib/slug";
import { MAX_DAYS } from "@/lib/list-query";
import { DEFAULT_ROWS, activeFilters, marksOn, parseTraceQuery } from "@/lib/traces";
import { UTC_OFFSET_COOKIE, parseUtcOffset } from "@/lib/utc-offset";

export const metadata = { title: "Traces" };

// The trace list, where a trace is searched for: a toolbar, the filters
// beside the list, the traces over the window above a page of rows, and the
// way through the pages below. A row per trace of this project's agents in a
// window of days, every column a sort and most a filter, all of it carried in
// the URL so a link holds the view. Every pixel is rendered from metadata:
// the server sums each trace's spans and never reads a prompt.
export default async function TracesPage({
  params,
  searchParams,
}: PageProps<"/[organization]/[project]/traces">) {
  const { organization, project: slug } = await params;
  const query = parseTraceQuery(await searchParams);
  const utcOffset = parseUtcOffset((await cookies()).get(UTC_OFFSET_COOKIE)?.value);
  const resolved = await projectPage(slug);
  if (resolved.kind === "unavailable") return <ServiceUnavailable />;
  const { gate, project } = resolved;

  const [access, facets, histogram, changes, mode] = await Promise.all([
    fetchTraces(project.id, { ...query, limit: query.limit ?? DEFAULT_ROWS }),
    fetchTraceFacets(project.id, query),
    fetchTraceHistogram(project.id, query, utcOffset),
    // By agent and environment alone: a change explains the bars whatever else narrows them.
    fetchChanges(project.id, query.days, query.agent ?? [], query.environment ?? []),
    // Said on an opened trace's line, the peek's.
    fetchContentMode(project.id),
  ]);
  const marks =
    histogram.kind === "ok" && changes.kind === "ok"
      ? marksOn(histogram.histogram.bars, changes.value.changes)
      : [];
  const unmarked = changes.kind === "ok" && changes.value.more && (
    <p className="text-xs text-muted-foreground">
      Only the window&rsquo;s newest changes are marked; its older ones are not.
    </p>
  );
  const knownTags = facets.kind === "ok" ? facets.facets.tag.values.map((v) => v.value) : [];
  // The tokens chart's picker offers the window's models, with those its bars name.
  const models = facets.kind === "ok" ? facets.facets.model.values.map((v) => v.value) : [];
  // An empty, unfiltered window is a first run only where all the plan keeps is empty too.
  const ever =
    access.kind === "ok" && access.list.total === 0 && activeFilters(query).length === 0
      ? await fetchTraces(project.id, { ...parseTraceQuery({}), days: MAX_DAYS, limit: 1 })
      : null;
  const firstRun = ever?.kind === "ok" && ever.list.total === 0;
  const base = projectPath(organization, slug);
  const canMint = administers(project.role);

  return (
    <>
      <UtcOffsetCookie drawnWith={utcOffset} />
      <ListPage
        list="traces"
        title="Traces"
        info="Every trace of this project's agents, newest first, with its steps, model and tool calls. Click a value to narrow the list to it, a user or a session to open its own page, a heading to sort by it. An agent's own page charts its traces, as a session's and a user's chart theirs."
        icon={ListTree}
        organization={organization}
        slug={slug}
        projectId={project.id}
        gate={gate}
        query={query}
        access={access}
        facets={facets.kind === "ok" ? facets.facets : null}
        rows={(list) => list.traces.length}
        toolbar={(list, days) => (
          <ListToolbar
            list="traces"
            days={days}
            organization={organization}
            slug={slug}
            query={query}
            total={list.total}
          />
        )}
        firstRun={
          firstRun ? (
            <TraceFirstRun base={base} canMint={canMint} kind="traces" />
          ) : undefined
        }
        nothingYet={(days) => (
          <>
            Install an{" "}
            <Link href="/docs/api/sdks" className="underline underline-offset-4">
              SDK
            </Link>{" "}
            and wrap one function as a trace, or add the{" "}
            <Link href="/docs/api/coding-agents" className="underline underline-offset-4">
              coding-agent plugin
            </Link>{" "}
            for Claude Code or Antigravity, and each trace lists here
            within a minute of ending. A key minted on{" "}
            <Link
              href={`${projectPath(organization, slug)}/api-keys`}
              className="underline underline-offset-4"
            >
              API keys
            </Link>{" "}
            to send spans comes with a first trace to paste. Traces from the last {days}{" "}
            {days === 1 ? "day" : "days"} are shown.
          </>
        )}
        table={(list) => (
          <>
            {histogram.kind === "ok" && (
              <>
                <TraceHistogramStrip histogram={histogram.histogram} marks={marks} models={models} />
                {unmarked}
              </>
            )}
            <TraceTable
              organization={organization}
              slug={slug}
              query={query}
              traces={list.traces}
              knownTags={knownTags}
            />
          </>
        )}
      >
        {/* The trace a row opened, over the list rather than in its place: its
            spans are read as it opens, and its ↑ and ↓ buttons, and `j` and
            `k`, step through this page's rows. */}
        {access.kind === "ok" && (
          <TracePeek
            organization={organization}
            slug={slug}
            projectId={project.id}
            traces={access.list.traces.map((t) => t.trace_id)}
            contentMode={mode?.content_mode}
            me={gate.person.userId}
            administers={administers(project.role)}
          />
        )}
      </ListPage>
    </>
  );
}
