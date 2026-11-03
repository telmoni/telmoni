import Link from "next/link";

import { LocalTime } from "@/components/local-time";
import { Badge } from "@/components/ui/badge";
import { Card } from "@/components/ui/card";
import type { Agreement, Agreements, Evaluations, Evaluator } from "@/lib/server/entities/evaluators";
import { formatScoreValue } from "@/lib/scores";
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
  agreement,
  here,
  tracesHref,
  sessionsHref,
}: {
  versions: readonly Evaluator[];
  evaluations: Evaluations | null;
  /** How far it agrees with members' scores; none where it could not be read. */
  agreement: Agreements | null;
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
      {agreement && <AgreementSection agreements={agreement} />}
      <p className="font-medium">Versions</p>
      <ul className="grid gap-gutter">
        {versions.map((version) => (
          <li key={version.version} className="grid gap-1">
            <span>
              <span className="font-medium">v{version.version}</span> · <LocalTime iso={version.created_at} /> ·{" "}
              {version.score?.name ?? (version.questions ?? []).map((q) => q.name).join(", ")} ·{" "}
              {version.model ?? "its connection's first model"}
            </span>
            <pre className="line-clamp-3 whitespace-pre-wrap font-mono text-xs text-muted-foreground">
              {version.state ?? (version.messages ?? []).map((m) => m.content).join("\n\n")}
            </pre>
          </li>
        ))}
      </ul>
    </Card>
  );
}

const PERCENT = new Intl.NumberFormat("en-US", { style: "percent", maximumFractionDigits: 0 });

/** Its agreement with members' scores of the same name on the same targets,
 *  a score name at a time: the share agreeing, Cohen's kappa and the
 *  confusion matrix for a yes or no or a category, the mean difference for a
 *  number. */
function AgreementSection({ agreements }: { agreements: Agreements }) {
  const heading = `Agreement with members, the last ${agreements.days} days`;
  const scored = agreements.scores.filter((a) => a.pairs > 0);
  return (
    <section aria-label={heading} className="grid gap-2">
      <p className="font-medium">{heading}</p>
      {scored.length === 0 ? (
        <p className="text-muted-foreground">
          No member has scored what it scored, under its score&rsquo;s name, in this window.
        </p>
      ) : (
        scored.map((agreement) => <ScoreAgreement key={agreement.name} agreement={agreement} />)
      )}
    </section>
  );
}

/** One score name's agreement. */
function ScoreAgreement({ agreement }: { agreement: Agreement }) {
  const pairs = `${agreement.pairs} ${agreement.pairs === 1 ? "pair" : "pairs"}`;
  return (
    <div className="grid gap-2" role="group" aria-label={agreement.name}>
      <p className="text-xs font-medium text-muted-foreground">{agreement.name}</p>
      {agreement.data_type === "number" ? (
        <p className="text-muted-foreground">
          {pairs} ·{" "}
          {agreement.mean_difference === null
            ? "no mean difference to read"
            : `it scores ${formatScoreValue(Math.abs(agreement.mean_difference))} ${agreement.mean_difference >= 0 ? "above" : "below"} its members on average`}
        </p>
      ) : (
        <>
          <p className="text-muted-foreground">
            {pairs} · {agreement.share === null ? "—" : PERCENT.format(agreement.share)} agree · Cohen&rsquo;s kappa{" "}
            {agreement.kappa === null ? "—, every pair one answer" : agreement.kappa.toFixed(2)}
          </p>
          <table className="w-fit text-xs tabular-nums">
            <caption className="sr-only">Its answers by row, its members&rsquo; by column</caption>
            <thead>
              <tr>
                <th scope="col" className="px-2 py-1 text-left font-normal text-muted-foreground">
                  It, by members
                </th>
                {agreement.labels.map((label) => (
                  <th key={label} scope="col" className="px-2 py-1 text-right font-normal text-muted-foreground">
                    {label}
                  </th>
                ))}
              </tr>
            </thead>
            <tbody>
              {agreement.matrix.map((row, i) => (
                <tr key={agreement.labels[i]}>
                  <th scope="row" className="px-2 py-1 text-left font-normal text-muted-foreground">
                    {agreement.labels[i]}
                  </th>
                  {row.map((count, j) => (
                    <td key={j} className={`px-2 py-1 text-right ${i === j ? "font-medium" : ""}`}>
                      {count}
                    </td>
                  ))}
                </tr>
              ))}
            </tbody>
          </table>
        </>
      )}
    </div>
  );
}
