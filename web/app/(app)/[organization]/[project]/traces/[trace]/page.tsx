import Link from "next/link";

import { AccessDenied } from "@/components/access-denied";
import { DataTable, THead, Th, Td } from "@/components/data-table";
import { LocalTime } from "@/components/local-time";
import { PageHeader } from "@/components/page-header";
import { Row, Rows } from "@/components/rows";
import { Section } from "@/components/section";
import { ServiceUnavailable } from "@/components/service-unavailable";
import { Badge } from "@/components/ui/badge";
import { Card } from "@/components/ui/card";
import { ownerContact } from "@/lib/identity";
import { windowReaching, type SearchParams } from "@/lib/list-query";
import {
  activeOrganization,
  fetchProjectBySlug,
  fetchTrace,
  getServerContext,
  type TraceSpan,
} from "@/lib/server/data";
import { sessionHref } from "@/lib/sessions";
import { projectPath } from "@/lib/slug";
import {
  KIND_LABEL,
  STATUS_LABEL,
  formatCount,
  formatDuration,
  groupSteps,
  spanMillis,
  timelineBars,
  tracesHref,
  type StepGroup,
} from "@/lib/traces";
import { SpanKind, SpanStatus } from "@/lib/types/enums";

export const metadata = { title: "Trace" };

type View = "steps" | "timeline";

// One trace: a run's record, named by the run's id. What it was for and how
// it ended at the top; its steps as a flat list first, each with the model
// and tool calls made inside it — tokens, latency, outcome — and the
// timeline of every span one view away. Every pixel is metadata: no prompt,
// completion or tool payload reaches this page, in any content mode.
export default async function TracePage({
  params,
  searchParams,
}: {
  params: Promise<{ organization: string; project: string; trace: string }>;
  searchParams?: Promise<SearchParams>;
}) {
  const { organization, project: slug, trace: runId } = await params;
  const wanted = (await searchParams)?.view;
  const view: View = (Array.isArray(wanted) ? wanted[0] : wanted) === "timeline" ? "timeline" : "steps";
  const [gate, project] = await Promise.all([getServerContext(), fetchProjectBySlug(slug)]);
  if (!gate || !project) return <ServiceUnavailable />;

  const list = tracesHref(organization, slug, {});
  const access = await fetchTrace(project.id, runId);
  if (access.kind === "unavailable") return <ServiceUnavailable />;
  if (access.kind === "forbidden") {
    return (
      <AccessDenied
        what="this project's traces"
        owner={ownerContact(activeOrganization(gate))}
        backHref={projectPath(organization, slug)}
      />
    );
  }
  if (access.kind === "missing") {
    return (
      <>
        <PageHeader title="Trace" backHref={list} backLabel="Traces" />
        <Card className="grid gap-1 text-sm">
          <p className="font-medium">No trace with this id in this project.</p>
          <p className="text-muted-foreground">
            Its spans may have passed their retention, or the run belongs to another project.{" "}
            <Link href={list} className="underline underline-offset-4">
              Back to the traces
            </Link>
            .
          </p>
        </Card>
      </>
    );
  }

  const { trace } = access;
  const { run, groups } = groupSteps(trace.spans);
  const first = trace.spans[0]!;
  const root = run ?? first;
  const startedAt = trace.spans.reduce(
    (earliest, s) => (s.started_at < earliest ? s.started_at : earliest),
    first.started_at,
  );
  const endedAt = trace.spans.reduce(
    (latest, s) => (s.ended_at > latest ? s.ended_at : latest),
    first.ended_at,
  );
  const duration = Math.max(0, new Date(endedAt).getTime() - new Date(startedAt).getTime());
  // The user's and the session's pages open on a week: one older run
  // links to a window that still holds it.
  const reaching = windowReaching(startedAt);
  const calls = trace.spans.filter(
    (s) => s.kind === SpanKind.ModelCall || s.kind === SpanKind.ToolCall,
  );
  // The model calls' tokens alone, as the list sums them: a span of another
  // kind may repeat its calls' usage, and would count it twice.
  const modelCalls = calls.filter((c) => c.kind === SpanKind.ModelCall);
  const inputTokens = modelCalls.reduce((sum, s) => sum + s.input_tokens, 0);
  const outputTokens = modelCalls.reduce((sum, s) => sum + s.output_tokens, 0);
  const tokens = inputTokens + outputTokens;
  const tags = Object.entries(root.tags);
  const bounds = [
    root.bound_stall_after_s !== null && `stall after ${formatDuration(root.bound_stall_after_s * 1000)}`,
    root.bound_max_steps !== null && `${formatCount(root.bound_max_steps)} steps at most`,
    root.bound_max_cost_usd !== null && `$${root.bound_max_cost_usd} at most`,
  ].filter((b): b is string => typeof b === "string");
  const here = projectPath(organization, slug, `/traces/${encodeURIComponent(runId)}`);

  return (
    <>
      <PageHeader title={root.agent} backHref={list} backLabel="Traces" />
      <div className="grid gap-6">
        <Section title="Run" description="What this run was for and how it ended, from its spans alone.">
          <Rows>
            <Row label="Status">{run ? <StatusBadge status={run.status} errorType={run.error_type} /> : <Badge variant="secondary" className="font-normal">Running</Badge>}</Row>
            <Row label="Started">
              <span className="text-sm text-muted-foreground">
                <LocalTime iso={startedAt} />
              </span>
            </Row>
            <Row label="Duration">
              <span className="tabular-nums">{formatDuration(duration)}</span>
            </Row>
            <Row label="User" hint="The end user, as stored.">
              <Value mono value={root.user_id} href={tracesHref(organization, slug, { user: [root.user_id], days: reaching })} />
            </Row>
            <Row label="Session" hint="The one this run was part of, as the application named it.">
              <Value mono value={root.session_id} href={sessionHref(organization, slug, root.session_id, { days: reaching })} />
            </Row>
            <Row label="Calls">
              <span className="text-sm">
                {formatCount(groups.filter((g) => g.step).length)} steps, {formatCount(calls.length)} calls:{" "}
                <span className="text-muted-foreground">
                  {formatCount(modelCalls.length)} to a model,{" "}
                  {formatCount(calls.filter((c) => c.kind === SpanKind.ToolCall).length)} to a tool
                </span>
              </span>
            </Row>
            <Row label="Tokens">
              <span className="text-sm tabular-nums">
                {formatCount(tokens)}{" "}
                <span className="text-muted-foreground">
                  ({formatCount(inputTokens)} in, {formatCount(outputTokens)} out)
                </span>
              </span>
            </Row>
            {bounds.length > 0 && <Row label="Bounds">{bounds.join("; ")}</Row>}
            {root.sample_rate !== null && root.sample_rate !== 1 && (
              <Row label="Sample rate">{root.sample_rate}</Row>
            )}
            {tags.length > 0 && (
              <Row label="Tags">
                <span className="flex flex-wrap justify-end gap-1">
                  {tags.map(([key, value]) => (
                    <Badge key={key} variant="secondary" className="font-mono text-xs font-normal">
                      {key}={value}
                    </Badge>
                  ))}
                </span>
              </Row>
            )}
            <Row label="Run id">
              <code className="text-xs">{trace.run_id}</code>
            </Row>
          </Rows>
        </Section>

        <Section
          title={view === "steps" ? "Steps" : "Timeline"}
          description={
            <span className="flex flex-wrap items-center gap-3">
              <span>
                {view === "steps"
                  ? "Each step with the model and tool calls made inside it, in the order they ran."
                  : "Every span of the run on one line of time, nested as it was recorded."}
              </span>
              <span className="inline-flex gap-2" role="group" aria-label="View">
                <ViewLink label="Steps" active={view === "steps"} href={here} />
                <ViewLink label="Timeline" active={view === "timeline"} href={`${here}?view=timeline`} />
              </span>
            </span>
          }
        >
          {view === "steps" ? <StepsTable groups={groups} /> : <Timeline spans={trace.spans} />}
          {trace.truncated && (
            <p className="text-xs text-muted-foreground">
              This run holds more spans than the page shows; the first {formatCount(trace.spans.length)} are here.
            </p>
          )}
        </Section>
      </div>
    </>
  );
}

function ViewLink({ label, active, href }: { label: string; active: boolean; href: string }) {
  if (active) {
    return (
      <span aria-current="page" className="font-medium text-foreground">
        {label}
      </span>
    );
  }
  return (
    <Link href={href} className="underline underline-offset-4">
      {label}
    </Link>
  );
}

function Value({ value, href, mono }: { value: string; href: string; mono?: boolean }) {
  if (!value) return <span className="text-sm text-muted-foreground">—</span>;
  return (
    <Link
      href={href}
      className={`text-sm underline-offset-4 hover:underline ${mono ? "font-mono text-xs" : ""}`}
    >
      {value}
    </Link>
  );
}

function StatusBadge({ status, errorType }: { status: SpanStatus; errorType: string }) {
  if (status === SpanStatus.Error) {
    return (
      <Badge variant="destructive" className="font-normal">
        {STATUS_LABEL[status]}
        {errorType ? `: ${errorType}` : ""}
      </Badge>
    );
  }
  if (status === SpanStatus.Cancelled) {
    return (
      <Badge variant="secondary" className="font-normal">
        {STATUS_LABEL[status]}
      </Badge>
    );
  }
  return (
    <Badge variant="outline" className="font-normal">
      {STATUS_LABEL[status]}
    </Badge>
  );
}

// The steps as a flat list, each followed by its calls, indented. Calls made
// straight under the run sit together under the run's own heading, placed
// where the first of them started.
function StepsTable({ groups }: { groups: StepGroup[] }) {
  if (groups.length === 0) {
    return (
      <Card className="grid gap-1 text-sm">
        <p className="font-medium">No steps or calls recorded.</p>
        <p className="text-muted-foreground">
          The run sent its own span and nothing inside it; the timeline shows what there is.
        </p>
      </Card>
    );
  }
  return (
    <DataTable>
      <THead>
        <Th>Step</Th>
        <Th>Kind</Th>
        <Th>Model or tool</Th>
        <Th className="text-right">Tokens</Th>
        <Th className="text-right">Latency</Th>
        <Th>Outcome</Th>
      </THead>
      <tbody>
        {groups.map((group) => (
          <StepRows key={group.step?.span_id ?? "run"} group={group} />
        ))}
      </tbody>
    </DataTable>
  );
}

function StepRows({ group }: { group: StepGroup }) {
  const step = group.step;
  return (
    <>
      <tr>
        <Td className="font-medium">{step ? step.name : "Under the run"}</Td>
        <Td className="text-xs text-muted-foreground">{step ? KIND_LABEL[step.kind] : "Run"}</Td>
        <Td className="text-xs text-muted-foreground">
          {group.calls.length === 0 ? "No calls" : `${formatCount(group.calls.length)} ${group.calls.length === 1 ? "call" : "calls"}`}
        </Td>
        <Td className="text-right tabular-nums text-xs text-muted-foreground">
          {step ? "" : ""}
        </Td>
        <Td className="text-right tabular-nums">{step ? formatDuration(spanMillis(step)) : ""}</Td>
        <Td>{step ? <StatusBadge status={step.status} errorType={step.error_type} /> : null}</Td>
      </tr>
      {group.calls.map((call) => (
        <CallRow key={call.span_id} call={call} />
      ))}
    </>
  );
}

function CallRow({ call }: { call: TraceSpan }) {
  const isModel = call.kind === SpanKind.ModelCall;
  return (
    <tr>
      <Td className="pl-8 text-sm text-muted-foreground">{call.name}</Td>
      <Td className="text-xs text-muted-foreground">{KIND_LABEL[call.kind]}</Td>
      <Td className="font-mono text-xs">
        {isModel ? (
          <>
            {call.model || "—"}
            {call.provider && <span className="text-muted-foreground"> · {call.provider}</span>}
          </>
        ) : (
          call.name
        )}
      </Td>
      <Td
        className="text-right tabular-nums"
        title={isModel ? `${formatCount(call.input_tokens)} in, ${formatCount(call.output_tokens)} out` : undefined}
      >
        {isModel ? formatCount(call.input_tokens + call.output_tokens) : ""}
      </Td>
      <Td className="text-right tabular-nums">{formatDuration(spanMillis(call))}</Td>
      <Td>
        <StatusBadge status={call.status} errorType={call.error_type} />
      </Td>
    </tr>
  );
}

// Every span as a bar on one line of time, from the run's first start to its
// last end, indented by its depth in the tree: drawn from the spans' own
// times and nothing else.
function Timeline({ spans }: { spans: TraceSpan[] }) {
  const bars = timelineBars(spans);
  return (
    <Card className="grid gap-2 p-4 text-sm">
      <ol className="grid gap-1.5">
        {bars.map(({ span, offset, width, depth }) => (
          <li key={span.span_id} className="grid grid-cols-[minmax(10rem,1fr)_3fr_auto] items-center gap-3">
            <span
              className="min-w-0 truncate text-xs"
              style={{ paddingLeft: `${Math.min(depth, 8) * 0.75}rem` }}
              title={`${KIND_LABEL[span.kind]}: ${span.name}`}
            >
              <span className="text-muted-foreground">{KIND_LABEL[span.kind]} </span>
              {span.name}
            </span>
            <span className="relative h-3 rounded-sm bg-muted" aria-hidden="true">
              <span
                className={`absolute top-0 h-3 rounded-sm ${span.status === SpanStatus.Error ? "bg-destructive" : "bg-foreground/60"}`}
                style={{ left: `${offset}%`, width: `${width}%` }}
              />
            </span>
            <span className="whitespace-nowrap text-xs tabular-nums text-muted-foreground">
              {formatDuration(spanMillis(span))}
            </span>
          </li>
        ))}
      </ol>
    </Card>
  );
}
