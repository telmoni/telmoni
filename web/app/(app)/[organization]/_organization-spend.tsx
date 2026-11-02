import Link from "next/link";

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
  if (spend.kind === "ok" && spend.projects.length === 0) return null;
  return (
    <section className="grid gap-gutter" aria-label="Spend">
      <div className="flex items-center justify-between gap-gutter">
        <h2 className="text-sm font-medium">Spend</h2>
        {spend.kind === "ok" && (
          <p className="text-sm tabular-nums">
            {formatCost(spend.total)}
            <span className="text-muted-foreground">
              {" "}
              since <LocalTime iso={spend.period_start} />
            </span>
          </p>
        )}
      </div>
      {spend.kind === "unavailable" ? (
        <NoticeCard
          title="The spend could not be read."
          body="Reload to try again. Each project’s traces show what its runs spent."
        />
      ) : (
        <Rows>
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
    </section>
  );
}
