import { PiggyBank } from "lucide-react";
import Link from "next/link";

import { OverviewCard } from "@/components/overview-card";
import { LocalTime } from "@/components/local-time";
import { NoticeCard } from "@/components/notice-card";
import { Row, Rows } from "@/components/rows";
import type { OrganizationSpend } from "@/lib/server/data";
import { formatCost, formatCount } from "@/lib/traces";
import { cn } from "@/lib/utils";

/** A budget's month so far against it, past it in red; none until it is read. */
function budgetWords(spent: number | null, budget: number | null): { text: string; over: boolean } | null {
  if (spent === null || budget === null || budget <= 0) return null;
  const share = Math.round((spent / budget) * 100);
  return { text: `${share}% of its ${formatCost(budget)} budget`, over: spent >= budget };
}

export function OrganizationSpendSection({
  organization,
  spend,
}: {
  /** The organization's slug, which each link starts with. */
  organization: string;
  spend: OrganizationSpend;
}) {
  if (spend.kind === "forbidden") return null;
  return (
    <OverviewCard title="Spend" icon={PiggyBank}>
      {spend.kind === "ok" && (
        <p className="tabular-nums">
          <span className="text-xl font-medium">{formatCost(spend.total)}</span>
          <span className="text-muted-foreground">
            {" "}
            since <LocalTime iso={spend.period_start} />
          </span>
        </p>
      )}
      {spend.kind === "ok" && spend.projects.length === 0 ? (
        <p className="text-muted-foreground">No project made a model call this period.</p>
      ) : spend.kind === "unavailable" ? (
        <NoticeCard
          title="The spend could not be read."
          body="Reload to try again. Each project’s traces show what its runs spent."
        />
      ) : (
        <Rows className="rounded-none p-0 ring-0">
          {spend.projects.map((row) => {
            const against = budgetWords(row.budget_spent, row.budget);
            return (
              <Row
                key={row.project.id}
                keep
                label={
                  <Link
                    href={`/${organization}/${row.project.slug}/dashboards`}
                    className="underline-offset-4 hover:underline"
                  >
                    {row.project.name}
                  </Link>
                }
                hint={
                  <>
                    {against && (
                      <span className={cn(against.over && "text-destructive")}>{against.text}</span>
                    )}
                    {against && row.unpriced_calls > 0 && " · "}
                    {row.unpriced_calls > 0 &&
                      `${formatCount(row.unpriced_calls)} unpriced ${row.unpriced_calls === 1 ? "call" : "calls"}`}
                  </>
                }
              >
                <span className="tabular-nums">{formatCost(row.spent)}</span>
              </Row>
            );
          })}
        </Rows>
      )}
    </OverviewCard>
  );
}
