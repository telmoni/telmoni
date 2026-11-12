import type { LucideIcon } from "lucide-react";
import Link from "next/link";
import type { ReactNode } from "react";

import { LocalTime } from "@/components/local-time";
import { NoticeCard } from "@/components/notice-card";
import { OverviewCard } from "@/components/overview-card";
import { PageHeader } from "@/components/page-header";
import { StatusMark } from "@/components/status-badge";
import { ScoreChips, TagChips } from "@/components/traces/cells";
import { WindowPicker } from "@/components/window-picker";
import { agentHref } from "@/lib/agents";
import { windowLabel } from "@/lib/list-query";
import type { Summed } from "@/lib/server/entities/sessions";
import { formatCost, formatCount } from "@/lib/traces";
import { SpanStatus } from "@/lib/types/enums";
import type { ScoreSummary } from "@/lib/types/traces";
import { failedShare, formatShare } from "@/lib/versions";

// What a session's page and an end user's both open on, each read off the
// row its list sums: how it stands, its traces and what they cost, and then
// what it was — its agents, its tags and the scores given to it.

/** How a trace that has ended stands, said under its time. */
const ENDED: Record<SpanStatus, string> = {
  [SpanStatus.Ok]: "It ended well.",
  [SpanStatus.Error]: "It ended in error.",
  [SpanStatus.Cancelled]: "It was cancelled.",
};

/** The page of one the window holds no trace of: its title and way back, the
 *  window to widen, and no visit filed for a page that found nothing. */
export function SummedNone({
  title,
  list,
  icon,
  days,
  held,
  none,
}: {
  title: string;
  /** Its list, where the way back leads, by the name its page goes by. */
  list: { href: string; label: string; rows: string };
  icon: LucideIcon;
  /** The days asked for, and those the read answered, which the notice names. */
  days: number;
  held: number;
  /** What the window lacks: "No trace of this session". */
  none: string;
}) {
  return (
    <>
      <PageHeader
        title={title}
        backHref={list.href}
        backLabel={list.label}
        visitHref={null}
        action={<WindowPicker days={days} />}
      />
      <NoticeCard
        icon={icon}
        title={`${none} in the ${windowLabel(held).toLowerCase()}.`}
        body={
          <>
            Widen the window, or go back to{" "}
            <Link href={list.href} className="underline underline-offset-4">
              {list.label}
            </Link>
            , which lists every {list.rows} the window holds.
          </>
        }
      />
    </>
  );
}

/** The page's four cards: its last trace, its traces, the page's own
 *  (`children`) and its spend. */
export function SummedCards({
  row,
  lastTrace,
  children,
}: {
  row: Summed;
  /** Where its last trace opens; none where the page does not hold it. */
  lastTrace: string | null;
  children: ReactNode;
}) {
  const share = failedShare(row);
  return (
    <div className="grid auto-rows-fr gap-page sm:grid-cols-2 lg:grid-cols-4">
      <OverviewCard title="Last trace" links={lastTrace ? [{ href: lastTrace, label: "Open it" }] : []}>
        <p className="flex items-center gap-gutter text-xl font-medium">
          <StatusMark status={row.last_ended ? row.last_status : null} />
          <LocalTime iso={row.last_seen} />
        </p>
        <p className="text-muted-foreground">{row.last_ended ? ENDED[row.last_status] : "Still running."}</p>
      </OverviewCard>
      {/* Not "Traces", which the section below is headed. */}
      <OverviewCard title="All traces">
        <p className="text-xl font-medium tabular-nums">{formatCount(row.traces)}</p>
        <p className={row.failed_traces > 0 ? "text-destructive" : "text-muted-foreground"}>
          {row.failed_traces > 0 && share !== null
            ? `${formatCount(row.failed_traces)} ended in error, ${formatShare(share)}.`
            : "None ended in error."}
        </p>
      </OverviewCard>
      {children}
      <OverviewCard title="Spend">
        <p className="text-xl font-medium tabular-nums">{formatCost(row.spend)}</p>
        <p className="text-muted-foreground">
          {formatCount(row.input_tokens)} tokens in, {formatCount(row.output_tokens)} out.
        </p>
      </OverviewCard>
    </div>
  );
}

/** What it was, on one line under the cards: the facts the page puts first
 *  (`children`), its agents, each opening the agent's page, its tags, each
 *  opening the list of `tagHref` narrowed to it, and its own scores. */
export function SummedFacts({
  row,
  organization,
  slug,
  days,
  tagHref,
  scores,
  children,
}: {
  row: Summed;
  organization: string;
  slug: string;
  /** The window the page reads, which its links carry on. */
  days: number | undefined;
  tagHref: (tag: string) => string;
  /** Its own scores, where the page has any to show. */
  scores?: readonly ScoreSummary[];
  children?: ReactNode;
}) {
  return (
    <div className="flex min-w-0 flex-wrap items-center gap-x-page gap-y-gutter text-sm text-muted-foreground">
      {children}
      <span className="flex min-w-0 flex-wrap items-center gap-x-gutter">
        {row.agents.length === 1 ? "Agent" : "Agents"}
        {row.agents.map((agent) => (
          <Link
            key={agent}
            href={agentHref(organization, slug, agent, days)}
            className="font-medium text-foreground underline-offset-4 hover:underline"
          >
            {agent}
          </Link>
        ))}
      </span>
      {row.tags.length > 0 && (
        <span className="flex min-w-0 flex-wrap items-center gap-x-gutter text-xs">
          <span className="text-sm">Tags</span>
          <TagChips tags={row.tags} href={tagHref} />
        </span>
      )}
      {scores && scores.length > 0 && (
        <span className="flex min-w-0 flex-wrap items-center gap-x-gutter text-xs">
          <span className="text-sm">Scores</span>
          <ScoreChips scores={scores} />
        </span>
      )}
    </div>
  );
}
