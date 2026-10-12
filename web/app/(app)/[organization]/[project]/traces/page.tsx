import Link from "next/link";

import { AccessDenied } from "@/components/access-denied";
import { DataTable, THead, Th, Td } from "@/components/data-table";
import { LocalTime } from "@/components/local-time";
import { PageHeader } from "@/components/page-header";
import { ServiceUnavailable } from "@/components/service-unavailable";
import { SortHeading } from "@/components/sort-heading";
import { TraceStatusBadge } from "@/components/trace-status-badge";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Card } from "@/components/ui/card";
import { Input } from "@/components/ui/input";
import { SelectField, WindowSelect } from "@/components/window-form";
import { ownerContact } from "@/lib/identity";
import { MAX_LIMIT, type SearchParams } from "@/lib/list-query";
import {
  activeOrganization,
  fetchProjectBySlug,
  fetchTraces,
  getServerContext,
  type Trace,
  type TraceQuery,
} from "@/lib/server/data";
import { projectPath } from "@/lib/slug";
import {
  STATUS_LABEL,
  TRACE_COLUMNS,
  activeFilters,
  currentSort,
  formatCount,
  formatDuration,
  nextSort,
  parseTraceQuery,
  shortId,
  traceHref,
  tracesHref,
} from "@/lib/traces";
import { SpanStatus } from "@/lib/types/enums";

export const metadata = { title: "Traces" };

// The trace list: a row per run of this project's agents in a window of days,
// newest first, every column a sort and most a filter, all of it carried in
// the URL so a link holds the view. Every pixel is rendered from metadata:
// the server sums each run's spans and never reads a prompt.
export default async function TracesPage({
  params,
  searchParams,
}: {
  params: Promise<{ organization: string; project: string }>;
  searchParams?: Promise<SearchParams>;
}) {
  const { organization, project: slug } = await params;
  const query = parseTraceQuery((await searchParams) ?? {});
  const [gate, project] = await Promise.all([getServerContext(), fetchProjectBySlug(slug)]);
  if (!gate || !project) return <ServiceUnavailable />;

  const access = await fetchTraces(project.id, query);
  if (access.kind === "unavailable") return <ServiceUnavailable />;
  const overview = projectPath(organization, slug);
  if (access.kind === "forbidden") {
    return (
      <AccessDenied
        what="this project's traces"
        owner={ownerContact(activeOrganization(gate))}
        backHref={overview}
      />
    );
  }

  const { list } = access;
  const { sort, dir } = currentSort(query);
  const filters = activeFilters(query);
  const href = (patch: Partial<TraceQuery>) => tracesHref(organization, slug, query, patch);
  const days = query.days ?? list.days;

  return (
    <>
      <PageHeader title="Traces" count={list.more ? undefined : list.traces.length} />
      <div className="grid gap-6">
        <section className="grid gap-3">
          <p className="text-sm text-muted-foreground">
            Every run of this project&rsquo;s agents, newest first, with its steps, model and
            tool calls. Click a user or a session to see its runs together; click a heading to sort
            by it.
          </p>

          <FilterForm organization={organization} slug={slug} query={query} days={days} />

          {filters.length > 0 && (
            <p className="flex flex-wrap items-center gap-2 text-sm text-muted-foreground">
              <span>Showing</span>
              {filters.map((f) => (
                <Badge key={f.key} variant="secondary" className="gap-1 font-normal">
                  <span>
                    {f.label}: {f.key === "status" ? STATUS_LABEL[f.value as SpanStatus] : f.value}
                  </span>
                  <Link
                    href={href({ [f.key]: undefined })}
                    aria-label={`Clear the ${f.label.toLowerCase()} filter`}
                    className="underline underline-offset-2"
                  >
                    ×
                  </Link>
                </Badge>
              ))}
              <Link href={tracesHref(organization, slug, { days: query.days })} className="underline underline-offset-4">
                Clear all
              </Link>
            </p>
          )}

          {list.traces.length === 0 ? (
            <Card className="overflow-hidden p-0 gap-0">
              <div className="grid gap-1 p-4">
                {filters.length > 0 ? (
                  <>
                    <p className="text-sm font-medium">No traces match these filters.</p>
                    <p className="text-sm text-muted-foreground">
                      Widen the window, or{" "}
                      <Link
                        href={tracesHref(organization, slug, { days: query.days })}
                        className="underline underline-offset-4"
                      >
                        clear the filters
                      </Link>
                      .
                    </p>
                  </>
                ) : (
                  <>
                    <p className="text-sm font-medium">Nothing to show yet.</p>
                    <p className="text-sm text-muted-foreground">
                      Install an{" "}
                      <Link href="/docs/api/sdks" className="underline underline-offset-4">
                        SDK
                      </Link>
                      , wrap one function as a run, and its trace lists here within a minute of
                      ending. Runs from the last {days} {days === 1 ? "day" : "days"} are shown.
                    </p>
                  </>
                )}
              </div>
            </Card>
          ) : (
            <DataTable>
              <THead>
                <Th className="whitespace-nowrap">Trace</Th>
                {TRACE_COLUMNS.map((column) => (
                  <SortHeading
                    key={column.sort}
                    label={column.label}
                    column={column.sort}
                    sort={sort}
                    dir={dir}
                    href={href(nextSort(query, column.sort))}
                  />
                ))}
              </THead>
              <tbody>
                {list.traces.map((t) => (
                  <TraceRow
                    key={t.run_id}
                    trace={t}
                    href={href}
                    query={query}
                    open={traceHref(organization, slug, t.run_id)}
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
                  , or narrow the window or the filters.
                </>
              ) : (
                "Narrow the window or the filters to see the rest."
              )}
            </p>
          )}
        </section>
      </div>
    </>
  );
}

function TraceRow({
  trace,
  href,
  query,
  open,
}: {
  trace: Trace;
  href: (patch: Partial<TraceQuery>) => string;
  query: TraceQuery;
  open: string;
}) {
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
      <Td className="whitespace-nowrap font-medium">
        <FilterLink value={trace.agent} current={query.agent} href={href({ agent: trace.agent })} />
      </Td>
      <Td className="max-w-48 truncate font-mono text-xs" title={trace.user_id || undefined}>
        <FilterLink
          value={trace.user_id}
          current={query.user}
          href={href({ user: trace.user_id })}
        />
      </Td>
      <Td className="max-w-48 truncate font-mono text-xs" title={trace.session_id || undefined}>
        <FilterLink
          value={trace.session_id}
          current={query.session}
          href={href({ session: trace.session_id })}
        />
      </Td>
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

// A cell whose value is a filter: a link that narrows the list to it, plain
// once the list is narrowed to it already, a dash when the run sent none.
function FilterLink({
  value,
  current,
  href,
}: {
  value: string;
  current: string | undefined;
  href: string;
}) {
  if (!value) return <span className="text-muted-foreground">—</span>;
  if (current === value) return <span>{value}</span>;
  return (
    <Link href={href} className="underline-offset-4 hover:underline">
      {value}
    </Link>
  );
}

// A plain GET form: the browser writes the URL, the server renders the list,
// and nothing runs on the client. The sort in force rides along unchanged.
function FilterForm({
  organization,
  slug,
  query,
  days,
}: {
  organization: string;
  slug: string;
  query: TraceQuery;
  days: number;
}) {
  return (
    <form
      method="get"
      action={projectPath(organization, slug, "/traces")}
      className="flex flex-wrap items-end gap-2"
      aria-label="Filter traces"
    >
      {(
        [
          ["agent", "Agent"],
          ["user", "User"],
          ["session", "Session"],
          ["model", "Model"],
          ["tool", "Tool"],
        ] as const
      ).map(([name, label]) => (
        <label key={name} className="grid gap-1 text-xs text-muted-foreground">
          {label}
          <Input
            name={name}
            defaultValue={query[name] ?? ""}
            className="h-8 w-36 text-xs"
            autoComplete="off"
          />
        </label>
      ))}
      <SelectField label="Status" name="status" defaultValue={query.status ?? ""}>
        <option value="">Any</option>
        {Object.values(SpanStatus).map((status) => (
          <option key={status} value={status}>
            {STATUS_LABEL[status]}
          </option>
        ))}
      </SelectField>
      <WindowSelect days={days} />
      {query.sort && <input type="hidden" name="sort" value={query.sort} />}
      {query.dir && <input type="hidden" name="dir" value={query.dir} />}
      <Button type="submit" size="sm" variant="outline">
        Filter
      </Button>
    </form>
  );
}
