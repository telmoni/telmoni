"use client";

import { useEffect, useMemo, useState, type ReactNode } from "react";
import Link from "next/link";

import { DataTable, THead, Td, Th } from "@/components/data-table";
import { StatusBadge } from "@/components/status-badge";
import { readTrace, startRead } from "@/components/traces/peek-read";
import { SpanContent } from "@/components/traces/span-content";
import { INDENT_MAX, KindIcon, SpanFacts, indentStyle } from "@/components/traces/span-parts";
import { Button } from "@/components/ui/button";
import { Card } from "@/components/ui/card";
import { spanTitle } from "@/lib/layout";
import { fieldsOf } from "@/lib/messages";
import type { RouteRead } from "@/lib/route-read";
import type { TraceDetail } from "@/lib/server/entities/traces";
import { compareRuns, figuresOf, type Comparison, type RunFigures } from "@/lib/trace-compare";
import { summarizeTrace, type SpanNode } from "@/lib/trace-tree";
import { KIND_LABEL, formatCost, formatCount, formatDuration, shortId, traceHref, traceReadHref } from "@/lib/traces";
import { cn } from "@/lib/utils";

// Two runs side by side (`lib/trace-compare.ts`), read as metadata: their
// figures, then their spans row by row, where they part marked. A project
// that keeps content offers each run's messages beside the other's, opened
// only when asked, through the trace's own route, so each run's read is
// filed as its trace page's is.

type Side = "a" | "b";

const FIGURES: ReadonlyArray<{ label: string; of: (f: RunFigures) => number | null; shown: (n: number) => string }> = [
  { label: "Duration", of: (f) => f.durationMs, shown: formatDuration },
  { label: "Input tokens", of: (f) => f.inputTokens, shown: formatCount },
  { label: "Output tokens", of: (f) => f.outputTokens, shown: formatCount },
  { label: "Cost", of: (f) => f.cost, shown: formatCost },
  { label: "Steps", of: (f) => f.steps, shown: formatCount },
  { label: "Model calls", of: (f) => f.modelCalls, shown: formatCount },
  { label: "Tool calls", of: (f) => f.toolCalls, shown: formatCount },
];

/** The second run against the first, signed; none where either is unknown. */
function difference(a: number | null, b: number | null, shown: (n: number) => string): string {
  if (a === null || b === null) return "—";
  if (a === b) return "Same";
  return `${b > a ? "+" : "−"}${shown(Math.abs(b - a))}`;
}

type Opened =
  | { kind: "shut" }
  | { kind: "reading" }
  | { kind: "open"; a: TraceDetail["content"]; b: TraceDetail["content"] }
  | { kind: "failed" };

export function CompareRuns({
  a,
  b,
  organization,
  slug,
  projectId,
  keepsContent,
}: {
  a: TraceDetail;
  b: TraceDetail;
  organization: string;
  slug: string;
  projectId: string;
  keepsContent: boolean;
}) {
  const comparison = useMemo(() => compareRuns(a, b), [a, b]);
  const figures = useMemo(() => ({ a: figuresOf(a), b: figuresOf(b) }), [a, b]);
  const [asked, setAsked] = useState(0);
  const [opened, setOpened] = useState<Opened>({ kind: "shut" });

  // Each run read again with its content, through the route the peek reads,
  // which files the read; both or neither are shown.
  useEffect(() => {
    if (asked === 0) return;
    const got: Partial<Record<Side, RouteRead<TraceDetail>>> = {};
    const settle = (side: Side) => (read: RouteRead<TraceDetail>) => {
      got[side] = read;
      if (!got.a || !got.b) return;
      // Content `null` is content the server could not open: neither is shown.
      setOpened(
        got.a.kind === "ok" && got.b.kind === "ok" && got.a.value.content && got.b.value.content
          ? { kind: "open", a: got.a.value.content, b: got.b.value.content }
          : { kind: "failed" },
      );
    };
    const stopA = startRead(traceReadHref(a.run_id, projectId), readTrace, settle("a"));
    const stopB = startRead(traceReadHref(b.run_id, projectId), readTrace, settle("b"));
    return () => {
      stopA();
      stopB();
    };
  }, [asked, a.run_id, b.run_id, projectId]);

  const open = () => {
    setOpened({ kind: "reading" });
    setAsked((n) => n + 1);
  };
  const runs = { a, b };
  const { rows, parted, partial: cutShort, hidden } = comparison;
  const at = parted?.at ?? null;
  // A run cut short by the server's cap is summed over the spans it sent.
  const partial = (["a", "b"] as const).filter((side) => runs[side].truncated);

  return (
    <div className="grid gap-page">
      <Card className="gap-0 p-0">
        <DataTable>
          <THead>
            <Th>
              <span className="sr-only">Figure</span>
            </Th>
            {(["a", "b"] as const).map((side) => (
              <Th key={side}>
                <RunHeading trace={runs[side]} side={side} href={traceHref(organization, slug, runs[side].run_id)} />
              </Th>
            ))}
            <Th className="text-right">Difference</Th>
          </THead>
          <tbody>
            {FIGURES.map(({ label, of, shown }) => {
              const x = of(figures.a);
              const y = of(figures.b);
              return (
                <tr key={label} className="border-t border-border">
                  <Td className="text-muted-foreground">{label}</Td>
                  <Td className="tabular-nums">{x === null ? "—" : shown(x)}</Td>
                  <Td className="tabular-nums">{y === null ? "—" : shown(y)}</Td>
                  <Td className="text-right tabular-nums">{difference(x, y, shown)}</Td>
                </tr>
              );
            })}
          </tbody>
        </DataTable>
        {partial.length > 0 && (
          <p className="border-t border-border px-gutter py-2 text-xs text-muted-foreground">
            {partial.length === 2 ? "Both runs hold" : `The ${partial[0] === "a" ? "first" : "second"} run holds`} more
            spans than the server sends at once: but for the cost, {partial.length === 2 ? "their" : "its"} figures
            count only the spans sent.
          </p>
        )}
      </Card>

      <section aria-labelledby="compared-spans" className="grid gap-gutter">
        <div className="flex flex-wrap items-center gap-gutter">
          <h2 id="compared-spans" className="text-sm font-medium">
            Spans
          </h2>
          <p className="text-sm text-muted-foreground" role="status">
            {partedWords(parted, cutShort, rows.length)}
          </p>
          {keepsContent && opened.kind !== "open" && (
            <Button variant="outline" size="sm" className="ml-auto" onClick={open} disabled={opened.kind === "reading"}>
              {opened.kind === "reading" ? "Opening…" : opened.kind === "failed" ? "Try opening again" : "Open messages"}
            </Button>
          )}
        </div>
        {keepsContent && opened.kind === "shut" && (
          <p className="text-xs text-muted-foreground">
            Opening the messages files a read of each run in the audit log, as its trace page does.
          </p>
        )}
        {opened.kind === "failed" && (
          <p className="text-xs text-destructive" role="alert">
            The messages could not be opened. The spans stand as they are.
          </p>
        )}
        <Card className="gap-0 overflow-hidden p-0">
          <div role="table" aria-label="The two runs' spans, row by row">
            <div role="row" className="grid grid-cols-2 border-b border-border text-xs font-medium text-muted-foreground">
              <span role="columnheader" className="px-gutter py-2">
                First
              </span>
              <span role="columnheader" className="border-l border-border px-gutter py-2">
                Second
              </span>
            </div>
            {rows.map((row, i) => (
              <ComparedRow
                key={i}
                a={row.a}
                b={row.b}
                parts={i === at}
                after={at !== null && i > at}
                fields={
                  opened.kind === "open"
                    ? { a: row.a && fieldsOf(opened.a, row.a.span.span_id), b: row.b && fieldsOf(opened.b, row.b.span.span_id) }
                    : null
                }
              />
            ))}
          </div>
          {hidden > 0 && (
            <p className="border-t border-border px-gutter py-2 text-xs text-muted-foreground">
              {formatCount(hidden)} more rows, not drawn.
            </p>
          )}
        </Card>
      </section>
    </div>
  );
}

function RunHeading({ trace, side, href }: { trace: TraceDetail; side: Side; href: string }) {
  const { root } = summarizeTrace(trace.spans);
  return (
    <span className="flex flex-wrap items-center gap-2">
      <span className="text-muted-foreground">{side === "a" ? "First" : "Second"}</span>
      <Link href={href} className="font-medium underline-offset-4 hover:underline">
        {spanTitle(root)} <span className="font-mono text-xs text-muted-foreground">{shortId(trace.run_id)}</span>
      </Link>
      <StatusBadge status={root.status} detail={root.error_type} />
    </span>
  );
}

/** Where the runs part, in words. */
/** Where the runs part, in words; as far as both were sent where either
 *  holds more, since the spans the server leaves out may be any of a branch's. */
function partedWords(parted: Comparison["parted"], partial: boolean, drawn: number): string {
  const sent = partial ? " as far as both were sent" : "";
  if (parted === null) return partial ? `Alike by name and kind${sent}.` : "Alike by name and kind, span for span.";
  const named = (node: SpanNode | null) =>
    node ? `${KIND_LABEL[node.span.kind].toLowerCase()} ${spanTitle(node.span)}` : "nothing, the run having ended";
  const past = parted.at >= drawn ? ", past the rows drawn here" : "";
  return `They part${sent} at span ${formatCount(parted.at + 1)}${past}: ${named(parted.a)} against ${named(parted.b)}.`;
}

function ComparedRow({
  a,
  b,
  parts,
  after,
  fields,
}: {
  a: SpanNode | null;
  b: SpanNode | null;
  parts: boolean;
  after: boolean;
  fields: { a: Readonly<Record<string, string>> | null | undefined; b: Readonly<Record<string, string>> | null | undefined } | null;
}) {
  const kept = fields && (fields.a || fields.b);
  return (
    <div
      role="row"
      aria-current={parts ? "true" : undefined}
      className={cn(
        "grid grid-cols-2 border-b border-border last:border-b-0",
        parts && "bg-destructive/10",
        after && "text-muted-foreground",
      )}
    >
      {(["a", "b"] as const).map((side) => {
        const node = side === "a" ? a : b;
        const own = fields?.[side];
        return (
          <div key={side} role="cell" className={cn("min-w-0 px-gutter py-1.5", side === "b" && "border-l border-border")}>
            {parts && side === "a" && <span className="mb-1 block text-xs font-medium text-destructive">They part here</span>}
            {node ? <SpanLine node={node} /> : <span className="text-xs text-muted-foreground">No span here</span>}
            {kept && own && (
              <div className="mt-2">
                <SpanContent fields={own} />
              </div>
            )}
          </div>
        );
      })}
    </div>
  );
}

function SpanLine({ node }: { node: SpanNode }): ReactNode {
  const { span } = node;
  return (
    <span className="flex min-w-0 items-start gap-1.5" style={indentStyle(Math.min(node.depth, INDENT_MAX))}>
      <KindIcon span={span} className="mt-0.5" />
      <span className="flex min-w-0 flex-1 flex-wrap items-baseline gap-x-2 gap-y-0.5">
        <span className="truncate text-sm" title={span.name}>
          {spanTitle(span)}
        </span>
        <SpanFacts span={span} />
      </span>
    </span>
  );
}
