import Link from "next/link";

import { AccessDenied } from "@/components/access-denied";
import { DataTable, THead, Th, Td } from "@/components/data-table";
import { LocalTime } from "@/components/local-time";
import { NoticeCard } from "@/components/notice-card";
import { PageHeader } from "@/components/page-header";
import { ServiceUnavailable } from "@/components/service-unavailable";
import { ownerContact } from "@/lib/identity";
import { SCORE_SOURCE, scoreSaid } from "@/lib/scores";
import { activeOrganization } from "@/lib/server/data";
import { fetchProjectScores, type Score } from "@/lib/server/entities/scores";
import { projectPage } from "@/lib/server/project-page";
import { sessionHref } from "@/lib/sessions";
import { projectPath } from "@/lib/slug";
import { traceHref } from "@/lib/traces";
import { ScoreTarget } from "@/lib/types/enums";

export const metadata = { title: "Scores" };

const TARGET: Record<Score["target"], string> = {
  [ScoreTarget.Run]: "Trace",
  [ScoreTarget.Span]: "Span",
  [ScoreTarget.Session]: "Session",
};

const one = (value: string | string[] | undefined) => (typeof value === "string" ? value : undefined);

/** The page asked for, from 0; anything but a whole number reads as the first. */
const pageOf = (value: string | undefined) => {
  const page = Number(value);
  return Number.isSafeInteger(page) && page > 0 ? page : 0;
};

export default async function ScoresPage({
  params,
  searchParams,
}: PageProps<"/[organization]/[project]/scores">) {
  const { organization, project: slug } = await params;
  const given = await searchParams;
  const name = one(given.name);
  const page = pageOf(one(given.page));
  const resolved = await projectPage(slug);
  if (resolved.kind === "unavailable") return <ServiceUnavailable />;
  const { gate, project } = resolved;

  const listing = await fetchProjectScores(project.id, name, page);
  if (listing.kind === "unavailable") return <ServiceUnavailable />;
  if (listing.kind === "forbidden") {
    return (
      <AccessDenied
        what="this project's scores"
        owner={ownerContact(activeOrganization(gate))}
        backHref={projectPath(organization, slug)}
      />
    );
  }

  const here = projectPath(organization, slug, "/scores");
  const at = (query: { name?: string; page?: number }) => {
    const kept = new URLSearchParams();
    if (query.name) kept.set("name", query.name);
    if (query.page) kept.set("page", String(query.page));
    return kept.size ? `${here}?${kept}` : here;
  };
  const { scores, more } = listing;
  const scoredHref = (score: Score) =>
    score.run_id
      ? traceHref(organization, slug, score.run_id)
      : score.session_id
        ? sessionHref(organization, slug, score.session_id)
        : null;

  return (
    <>
      <PageHeader
        title="Scores"
        info="Every score of this project's traces, spans and sessions, newest first: from your code, by a member on a trace's page, or by an evaluator. Click a name to see its scores alone."
      />
      <section className="grid gap-page">
        {name && (
          <p className="text-sm text-muted-foreground">
            Scores named <span className="font-medium text-foreground">{name}</span>.{" "}
            <Link className="underline" href={here}>
              All scores
            </Link>
          </p>
        )}
        {scores.length === 0 ? (
          <NoticeCard
            title={name || page ? "No scores here." : "No scores yet."}
            body="A score appears here once your code sends one, a member scores a trace, or an evaluator judges one."
          />
        ) : (
          <DataTable>
            <THead>
              <Th>Given</Th>
              <Th>Name</Th>
              <Th>Value</Th>
              <Th>Of</Th>
              <Th>Source</Th>
            </THead>
            <tbody>
              {scores.map((score) => {
                const scored = scoredHref(score);
                return (
                  <tr key={score.score_id}>
                    <Td className="text-xs text-muted-foreground whitespace-nowrap">
                      <LocalTime iso={score.given_at} />
                    </Td>
                    <Td className="font-medium whitespace-nowrap">
                      <Link className="hover:underline" href={at({ name: score.name })}>
                        {score.name}
                      </Link>
                    </Td>
                    <Td className="tabular-nums whitespace-nowrap">{scoreSaid(score)}</Td>
                    <Td className="whitespace-nowrap">
                      {scored ? (
                        <Link
                          className="underline"
                          href={scored}
                          aria-label={`The ${score.target === ScoreTarget.Session ? "session" : "trace"} ${score.name} scores`}
                        >
                          {TARGET[score.target]}
                        </Link>
                      ) : (
                        TARGET[score.target]
                      )}
                    </Td>
                    <Td className="text-xs text-muted-foreground whitespace-nowrap">
                      {SCORE_SOURCE[score.source]}
                    </Td>
                  </tr>
                );
              })}
            </tbody>
          </DataTable>
        )}
        {(page > 0 || more) && (
          <nav aria-label="Pages of scores" className="flex gap-gutter text-sm">
            {page > 0 && (
              <Link className="underline" href={at({ name, page: page - 1 })}>
                Newer scores
              </Link>
            )}
            {more && (
              <Link className="underline" href={at({ name, page: page + 1 })}>
                Older scores
              </Link>
            )}
          </nav>
        )}
      </section>
    </>
  );
}
