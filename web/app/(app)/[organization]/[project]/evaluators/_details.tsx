import Link from "next/link";

import { LocalTime } from "@/components/local-time";
import { Badge } from "@/components/ui/badge";
import { Card } from "@/components/ui/card";
import type { Evaluations, Evaluator } from "@/lib/server/entities/evaluators";
import { formatCost } from "@/lib/traces";
import { EvaluationStatus, EvaluationTarget } from "@/lib/types/enums";

const STATUS_LABEL: Record<EvaluationStatus, string> = {
  [EvaluationStatus.Pending]: "Pending",
  [EvaluationStatus.Delayed]: "Delayed",
  [EvaluationStatus.Done]: "Done",
  [EvaluationStatus.Failed]: "Failed",
};

/** One evaluator's evaluations — its counts, the week's spend, its newest
 *  each linked to what it judged and to its own trace — and every version. */
export function EvaluatorDetails({
  versions,
  evaluations,
  here,
  tracesHref,
  sessionsHref,
}: {
  versions: readonly Evaluator[];
  evaluations: Evaluations | null;
  here: string;
  tracesHref: string;
  sessionsHref: string;
}) {
  const newest = versions[0];
  const limit = newest?.weekly_limit ?? null;
  return (
    <Card className="mt-gutter grid gap-gutter p-gutter text-sm" role="region" aria-label="Evaluator details">
      <div className="flex items-center justify-between gap-gutter">
        <p className="font-medium">{newest?.name}</p>
        <Link className="underline" href={here}>
          Close
        </Link>
      </div>
      {evaluations && (
        <>
          <p className="text-muted-foreground">
            {evaluations.done} done · {evaluations.pending} pending · {evaluations.delayed} delayed ·{" "}
            {evaluations.failed} failed · {formatCost(evaluations.spent_this_week)} since Monday
            {limit !== null && ` of its ${formatCost(limit)} a week`}
          </p>
          {evaluations.evaluations.length > 0 && (
            <ul className="grid gap-gutter">
              {evaluations.evaluations.map((evaluation) => (
                <li key={evaluation.id} className="flex flex-wrap items-center justify-between gap-gutter">
                  <span className="flex min-w-0 flex-wrap items-center gap-gutter">
                    <Badge variant={evaluation.status === EvaluationStatus.Failed ? "destructive" : "secondary"}>
                      {STATUS_LABEL[evaluation.status]}
                    </Badge>
                    {evaluation.target === EvaluationTarget.Session && evaluation.session_id ? (
                      <Link className="underline" href={`${sessionsHref}/${encodeURIComponent(evaluation.session_id)}`}>
                        Its session
                      </Link>
                    ) : evaluation.run_id ? (
                      <Link className="underline" href={`${tracesHref}/${evaluation.run_id}`}>
                        {evaluation.target === EvaluationTarget.Span ? "Its span's trace" : "Its trace"}
                      </Link>
                    ) : null}
                    {evaluation.trace_run_id && (
                      <Link className="underline" href={`${tracesHref}/${evaluation.trace_run_id}`}>
                        The judgement
                      </Link>
                    )}
                    <span className="text-muted-foreground">
                      v{evaluation.evaluator_version}
                      {evaluation.model && ` · ${evaluation.model}`}
                      {evaluation.cost !== null && ` · ${formatCost(evaluation.cost)}`}
                      {evaluation.cut && " · judged as cut to fit an export"}
                      {evaluation.reason && ` · ${evaluation.reason}`}
                    </span>
                  </span>
                  <span className="text-muted-foreground">
                    <LocalTime iso={evaluation.created_at} />
                  </span>
                </li>
              ))}
            </ul>
          )}
        </>
      )}
      <p className="font-medium">Versions</p>
      <ul className="grid gap-gutter">
        {versions.map((version) => (
          <li key={version.version} className="grid gap-1">
            <span>
              <span className="font-medium">v{version.version}</span> · <LocalTime iso={version.created_at} /> ·{" "}
              {version.score.name} · {version.model ?? "its connection's first model"}
            </span>
            <pre className="line-clamp-3 whitespace-pre-wrap font-mono text-xs text-muted-foreground">
              {version.messages.map((m) => m.content).join("\n\n")}
            </pre>
          </li>
        ))}
      </ul>
    </Card>
  );
}
