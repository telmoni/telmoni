import { PageHeader } from "@/components/page-header";
import { ServiceUnavailable } from "@/components/service-unavailable";
import { Card } from "@/components/ui/card";
import { fetchProjectBySlug, getServerContext } from "@/lib/server/data";

export const metadata = { title: "Dashboards" };

// A placeholder: the rail's row stands before its page does, so the console
// shows the product's shape while telemetry is built. The dashboards page
// replaces this file; until then it resolves the project as every page under
// one does, and draws the one content wrapper every page keeps.
export default async function DashboardsPage({
  params,
}: {
  params: Promise<{ organization: string; project: string }>;
}) {
  const { project: slug } = await params;
  const gate = await getServerContext();
  if (!gate) return <ServiceUnavailable />;

  const project = await fetchProjectBySlug(slug);
  if (!project) return <ServiceUnavailable />;

  return (
    <>
      <PageHeader title="Dashboards" />
      <div className="grid gap-6">
        <Card className="overflow-hidden p-0 gap-0">
          <div className="grid gap-1 p-4">
            <p className="text-sm font-medium">Nothing to show yet.</p>
            <p className="text-sm text-muted-foreground">Charts over this project&rsquo;s telemetry will live here: spend, tokens, latency and volume by model, agent, user, session and tag over any window, with dashboards you compose and keep.</p>
          </div>
        </Card>
      </div>
    </>
  );
}
