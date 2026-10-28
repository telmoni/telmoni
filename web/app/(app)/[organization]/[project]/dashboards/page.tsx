import { PageHeader } from "@/components/page-header";
import { NoticeCard } from "@/components/notice-card";
import { ServiceUnavailable } from "@/components/service-unavailable";
import { projectPage } from "@/lib/server/project-page";

export const metadata = { title: "Dashboards" };

// A placeholder: the rail's row stands before its page does, so the console
// shows the product's shape while telemetry is built. The dashboards page
// replaces this file; until then it resolves the project as every page under
// one does, and draws the one content wrapper every page keeps.
export default async function DashboardsPage({
  params,
}: PageProps<"/[organization]/[project]/dashboards">) {
  const { project: slug } = await params;
  if ((await projectPage(slug)).kind === "unavailable") return <ServiceUnavailable />;

  return (
    <>
      <PageHeader
        title="Dashboards"
        info="Charts over this project's telemetry, still to come: spend, tokens, latency and volume by model, agent, user, session and tag over any window, in dashboards you compose and keep."
      />
      <div className="grid gap-6">
        <NoticeCard
          title="Nothing to show yet."
          body={
            <>
              Dashboards are still to come. Until then, Chart on the Traces page charts a
              window&rsquo;s runs, tokens and durations.
            </>
          }
        />
      </div>
    </>
  );
}
