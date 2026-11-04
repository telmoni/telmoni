import { ServiceUnavailable } from "@/components/service-unavailable";
import { administers } from "@/lib/roles";
import { fetchEvaluationRules, fetchEvaluators } from "@/lib/server/entities/evaluators";
import { fetchTraceViews } from "@/lib/server/entities/traces";
import { projectPage } from "@/lib/server/project-page";

import { Rules } from "./_rules";

export const metadata = { title: "Rules" };

// The project's rules, what its evaluators judge, and how long a session is
// quiet before one is judged; owners and admins change them.
export default async function RulesPage({ params }: PageProps<"/[organization]/[project]/rules">) {
  const { project: slug } = await params;
  const resolved = await projectPage(slug);
  if (resolved.kind === "unavailable") return <ServiceUnavailable />;
  const { project } = resolved;

  const [rules, listing, views] = await Promise.all([
    fetchEvaluationRules(project.id),
    fetchEvaluators(project.id),
    fetchTraceViews(project.id),
  ]);
  if (!rules || !listing) return <ServiceUnavailable />;

  return (
    <Rules
      projectId={project.id}
      rules={rules.rules}
      evaluators={listing.evaluators}
      views={views.kind === "ok" ? views.views : []}
      idleMinutes={rules.session_idle_minutes}
      canEdit={administers(project.role)}
      contentKept={listing.content_kept}
    />
  );
}
