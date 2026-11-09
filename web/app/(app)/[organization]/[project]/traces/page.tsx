import Link from "next/link";
import { cookies } from "next/headers";

import { CopyBox } from "@/components/plaintext-token-dialog";
import { FirstRun, FirstRunStep, Listening } from "@/components/first-run";
import { NoticeCard } from "@/components/notice-card";
import { Button } from "@/components/ui/button";
import { ServiceUnavailable } from "@/components/service-unavailable";
import { TraceCharts, TraceHistogramStrip } from "@/components/traces/charts";
import { ListPage } from "@/components/traces/list-page";
import { NewTraces } from "@/components/traces/new-traces";
import { TracePeek } from "@/components/traces/peek";
import { TraceTable } from "@/components/traces/table";
import { TraceToolbar } from "@/components/traces/toolbar";
import { UtcOffsetCookie } from "@/components/traces/utc-offset";
import { administers } from "@/lib/roles";
import {
  fetchTraceFacets,
  fetchTraceHistogram,
  fetchTraces,
} from "@/lib/server/data";
import { fetchChanges } from "@/lib/server/entities/changes";
import { projectPage } from "@/lib/server/project-page";
import { projectPath } from "@/lib/slug";
import { AGENT_PROMPT } from "@/lib/first-run";
import { MAX_DAYS } from "@/lib/list-query";
import { DEFAULT_ROWS, activeFilters, marksOn, parseTraceQuery, parseTraceUi } from "@/lib/traces";
import { UTC_OFFSET_COOKIE, parseUtcOffset } from "@/lib/utc-offset";

export const metadata = { title: "Runs" };

// The trace list: a toolbar, the filters beside the list, the runs over the
// window above a page of rows, and the way through the pages below. A row
// per run of this project's agents in a window of days, every column a sort
// and most a filter, all of it carried in the URL so a link holds the view.
// Every pixel is rendered from metadata: the server sums each run's spans
// and never reads a prompt.
export default async function TracesPage({
  params,
  searchParams,
}: PageProps<"/[organization]/[project]/traces">) {
  const { organization, project: slug } = await params;
  const given = await searchParams;
  const query = parseTraceQuery(given);
  const ui = parseTraceUi(given);
  const utcOffset = parseUtcOffset((await cookies()).get(UTC_OFFSET_COOKIE)?.value);
  const resolved = await projectPage(slug);
  if (resolved.kind === "unavailable") return <ServiceUnavailable />;
  const { gate, project } = resolved;

  const [access, facets, histogram, changes] = await Promise.all([
    fetchTraces(project.id, { ...query, limit: query.limit ?? DEFAULT_ROWS }),
    fetchTraceFacets(project.id, query),
    fetchTraceHistogram(project.id, query, utcOffset),
    // By agent and environment alone: a change explains the bars whatever else narrows them.
    fetchChanges(project.id, query.days, query.agent ?? [], query.environment ?? []),
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
  // The rows this render drew, as one token: the table is keyed by it, and
  // the new-traces bar reads a change in it as the list drawn again.
  const drawn = access.kind === "ok" ? access.list.traces.map((t) => t.run_id).join() : "";
  const knownTags = facets.kind === "ok" ? facets.facets.tag.values.map((v) => v.value) : [];
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
        title="Runs"
        info="Every run of this project's agents, newest first, with its steps, model and tool calls. Click a value to narrow the list to it, a heading to sort by it."
        organization={organization}
        slug={slug}
        gate={gate}
        query={query}
        ui={ui}
        access={access}
        facets={facets.kind === "ok" ? facets.facets : null}
        rows={(list) => list.traces.length}
        toolbar={(list, days) => (
          <TraceToolbar
            days={days}
            organization={organization}
            slug={slug}
            query={query}
            ui={ui}
            total={list.total}
          />
        )}
        firstRun={
          firstRun ? (
            <FirstRun
              status={<Listening>Listening for this project&rsquo;s first run</Listening>}
              title="Send your first trace"
              lead="Three steps, a few minutes. The first run lists here by itself as it lands, with no reload."
            >
              <FirstRunStep
                n={1}
                title="Mint an API key"
                note={
                  canMint
                    ? "One with Send spans comes with a first run to paste, for each SDK."
                    : "The project's owners and admins mint its keys; ask one for a key with Send spans."
                }
              >
                {canMint && (
                  <div className="flex flex-wrap gap-gutter">
                    <Button asChild size="sm">
                      <Link href={`${base}/api-keys`}>Mint a key</Link>
                    </Button>
                  </div>
                )}
              </FirstRunStep>
              <FirstRunStep
                n={2}
                title="Have your coding agent add tracing"
                note="Paste this into Claude Code, Antigravity or another coding agent working in your application."
              >
                <CopyBox value={AGENT_PROMPT} label="the prompt" />
                <p className="text-sm text-muted-foreground">
                  Or by hand: an{" "}
                  <Link href="/docs/api/sdks" className="underline underline-offset-4">
                    SDK
                  </Link>
                  , a{" "}
                  <Link href="/docs/api/frameworks" className="underline underline-offset-4">
                    framework you already use
                  </Link>{" "}
                  or any OpenTelemetry exporter, and the{" "}
                  <Link href="/docs/api/coding-agents" className="underline underline-offset-4">
                    plugin
                  </Link>{" "}
                  for your coding agent&rsquo;s own sessions.
                </p>
              </FirstRunStep>
              <FirstRunStep
                n={3}
                title="Run your agent"
                note="Each run lists here within a minute of ending, with its steps, model calls, tokens and spend."
              />
            </FirstRun>
          ) : undefined
        }
        nothingYet={(days) => (
          <>
            Install an{" "}
            <Link href="/docs/api/sdks" className="underline underline-offset-4">
              SDK
            </Link>{" "}
            and wrap one function as a run, or add the{" "}
            <Link href="/docs/api/coding-agents" className="underline underline-offset-4">
              coding-agent plugin
            </Link>{" "}
            for Claude Code or Antigravity, and each run&rsquo;s trace lists here
            within a minute of ending. A key minted on{" "}
            <Link
              href={`${projectPath(organization, slug)}/api-keys`}
              className="underline underline-offset-4"
            >
              API keys
            </Link>{" "}
            to send spans comes with a first run to paste. Runs from the last {days}{" "}
            {days === 1 ? "day" : "days"} are shown.
          </>
        )}
        // What landed since this render, counted rather than drawn in: the
        // rows stay under the reader's hand until they ask.
        before={
          access.kind === "ok" && (
            <NewTraces
              key={project.id}
              projectId={project.id}
              drawn={drawn}
              empty={access.list.total === 0}
            />
          )
        }
        // The charts in place of the table, when the page asks for them.
        instead={
          ui.display === "chart" ? (
            histogram.kind === "ok" ? (
              <>
                <TraceCharts histogram={histogram.histogram} marks={marks} />
                {unmarked}
              </>
            ) : (
              <NoticeCard title="The charts could not be read." body="Try again in a moment." />
            )
          ) : undefined
        }
        table={(list) => (
          <>
            {histogram.kind === "ok" && (
              <>
                <TraceHistogramStrip histogram={histogram.histogram} marks={marks} />
                {unmarked}
              </>
            )}
            <TraceTable
              organization={organization}
              slug={slug}
              query={query}
              ui={ui}
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
            runs={access.list.traces.map((t) => t.run_id)}
          />
        )}
      </ListPage>
    </>
  );
}
