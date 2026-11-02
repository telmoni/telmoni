import Link from "next/link";

import { LocalTime } from "@/components/local-time";
import { ServiceUnavailable } from "@/components/service-unavailable";
import { Card } from "@/components/ui/card";
import { administers } from "@/lib/roles";
import {
  fetchEvaluatorTemplates,
  fetchEvaluatorVersions,
  fetchEvaluators,
} from "@/lib/server/entities/evaluators";
import { fetchModelConnections } from "@/lib/server/entities/model-connections";
import { projectPage } from "@/lib/server/project-page";
import { projectPath } from "@/lib/slug";

import { Evaluators } from "./_evaluators";

export const metadata = { title: "Evaluators" };

// The project's evaluators, each its newest version, and one's every version
// where the URL names it; owners and admins make and change them.
export default async function EvaluatorsPage({
  params,
  searchParams,
}: PageProps<"/[organization]/[project]/evaluators">) {
  const { organization, project: slug } = await params;
  const query = await searchParams;
  const resolved = await projectPage(slug);
  if (resolved.kind === "unavailable") return <ServiceUnavailable />;
  const { project } = resolved;
  const named = typeof query.versions === "string" ? query.versions : null;

  const [listing, templates, connections, versions] = await Promise.all([
    fetchEvaluators(project.id),
    fetchEvaluatorTemplates(project.id),
    fetchModelConnections(project.id),
    named ? fetchEvaluatorVersions(project.id, named) : null,
  ]);
  if (!listing || !templates || !connections) return <ServiceUnavailable />;
  const here = projectPath(organization, slug, "/evaluators");

  return (
    <>
      <Evaluators
        projectId={project.id}
        evaluators={listing.evaluators}
        templates={templates}
        connections={connections}
        contentKept={listing.content_kept}
        canEdit={administers(project.role)}
        here={here}
        privacyHref={projectPath(organization, slug, "/privacy")}
        settingsHref={projectPath(organization, slug, "/settings")}
      />
      {versions && versions.length > 0 && (
        <Card className="mt-gutter grid gap-gutter p-gutter text-sm" role="region" aria-label="Versions">
          <div className="flex items-center justify-between gap-gutter">
            <p className="font-medium">{versions[0]?.name}: every version</p>
            <Link className="underline" href={here}>
              Close
            </Link>
          </div>
          <ul className="grid gap-gutter">
            {versions.map((version) => (
              <li key={version.version} className="grid gap-1">
                <span>
                  <span className="font-medium">v{version.version}</span> ·{" "}
                  <LocalTime iso={version.created_at} /> ·{" "}
                  {version.score.name} · {version.model ?? "its connection's first model"}
                </span>
                <pre className="line-clamp-3 whitespace-pre-wrap font-mono text-xs text-muted-foreground">
                  {version.messages.map((m) => m.content).join("\n\n")}
                </pre>
              </li>
            ))}
          </ul>
        </Card>
      )}
    </>
  );
}
