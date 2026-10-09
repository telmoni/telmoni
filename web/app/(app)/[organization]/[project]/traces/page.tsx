import { PageHeader } from "@/components/page-header";
import { ServiceUnavailable } from "@/components/service-unavailable";
import { Card } from "@/components/ui/card";
import { fetchProjectBySlug, getServerContext } from "@/lib/server/data";

export const metadata = { title: "Traces" };

// A placeholder: the rail's row stands before its page does, so the console
// shows the product's shape while telemetry is built. The trace list replaces
// this file; until then it resolves the project as every page under one does,
// and draws the one content wrapper every page keeps.
export default async function TracesPage({
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
      <PageHeader title="Traces" />
      <div className="grid gap-6">
        <Card className="overflow-hidden p-0 gap-0">
          <div className="grid gap-1 p-4">
            <p className="text-sm font-medium">Nothing to show yet.</p>
            <p className="text-sm text-muted-foreground">Every run of this project&rsquo;s agents will list here as a trace, newest first, with its steps, model and tool calls, filtered and sorted by user and by session.</p>
          </div>
        </Card>
      </div>
    </>
  );
}
