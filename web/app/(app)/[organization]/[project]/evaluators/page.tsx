import { ServiceUnavailable } from "@/components/service-unavailable";
import { administers } from "@/lib/roles";
import {
  fetchEvaluationRules,
  fetchAgreement,
  fetchEvaluations,
  fetchEvaluatorTemplates,
  fetchEvaluatorVersions,
  fetchEvaluators,
} from "@/lib/server/entities/evaluators";
import { fetchTraceViews } from "@/lib/server/entities/traces";
import { fetchModelConnections } from "@/lib/server/entities/model-connections";
import { projectPage } from "@/lib/server/project-page";
import { projectPath } from "@/lib/slug";

import { EvaluatorDetails } from "./_details";
import { Evaluators } from "./_evaluators";
import { Rules } from "./_rules";

export const metadata = { title: "Evaluators" };

// The project's evaluators, each its newest version, and one's evaluations
// and every version where the URL names it; owners and admins change them.
export default async function EvaluatorsPage({
  params,
  searchParams,
}: PageProps<"/[organization]/[project]/evaluators">) {
  const { organization, project: slug } = await params;
  const query = await searchParams;
  const resolved = await projectPage(slug);
  if (resolved.kind === "unavailable") return <ServiceUnavailable />;
  const { project } = resolved;
  const named = typeof query.evaluator === "string" ? query.evaluator : null;

  const [listing, templates, connections, rules, views, versions, evaluations, agreement] = await Promise.all([
    fetchEvaluators(project.id),
    fetchEvaluatorTemplates(project.id),
    fetchModelConnections(project.id),
    fetchEvaluationRules(project.id),
    fetchTraceViews(project.id),
    named ? fetchEvaluatorVersions(project.id, named) : null,
    named ? fetchEvaluations(project.id, named) : null,
    named ? fetchAgreement(project.id, named) : null,
  ]);
  if (!listing || !templates || !connections || !rules) return <ServiceUnavailable />;
  const here = projectPath(organization, slug, "/evaluators");
  const rulesOf: Record<string, string[]> = {};
  for (const rule of rules.rules) {
    for (const id of rule.evaluators) (rulesOf[id] ??= []).push(rule.name);
  }

  return (
    <>
      <Evaluators
        projectId={project.id}
        evaluators={listing.evaluators}
        templates={templates.templates}
        typedTemplates={templates.typed}
        connections={connections}
        rulesOf={rulesOf}
        contentKept={listing.content_kept}
        canEdit={administers(project.role)}
        here={here}
        privacyHref={projectPath(organization, slug, "/privacy")}
        settingsHref={projectPath(organization, slug, "/settings")}
      />
      <div className="mt-gutter">
        <Rules
          projectId={project.id}
          rules={rules.rules}
          evaluators={listing.evaluators}
          views={views.kind === "ok" ? views.views : []}
          idleMinutes={rules.session_idle_minutes}
          canEdit={administers(project.role)}
          contentKept={listing.content_kept}
        />
      </div>
      {versions && versions.length > 0 && (
        <EvaluatorDetails
          versions={versions}
          evaluations={evaluations}
          agreement={agreement}
          here={here}
          tracesHref={projectPath(organization, slug, "/traces")}
          sessionsHref={projectPath(organization, slug, "/sessions")}
        />
      )}
    </>
  );
}
