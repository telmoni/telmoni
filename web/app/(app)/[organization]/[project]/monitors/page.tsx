import Link from "next/link";

import { AccessDenied } from "@/components/access-denied";
import { DataTable, THead, Td, Th } from "@/components/data-table";
import { LocalTime } from "@/components/local-time";
import { MonitorFilters } from "@/components/monitors/monitor-filters";
import { NewMonitor } from "@/components/monitors/new-monitor";
import { PingKeyMenu } from "@/components/monitors/ping-key-menu";
import { MonitorStateBadge } from "@/components/monitors/state-badge";
import { PageHeader } from "@/components/page-header";
import { ServiceUnavailable } from "@/components/service-unavailable";
import { Card } from "@/components/ui/card";
import { ownerContact } from "@/lib/identity";
import type { SearchParams } from "@/lib/list-query";
import {
  boundsInBrief,
  filterMonitors,
  lastRunLine,
  monitorHref,
  monitorsHref,
  parseMonitorQuery,
  scheduleInWords,
} from "@/lib/monitors";
import { administersProject } from "@/lib/project-role";
import {
  activeOrganization,
  fetchMonitors,
  fetchPingKeys,
  fetchProjectBySlug,
  fetchTimezones,
  getServerContext,
} from "@/lib/server/data";
import { projectPath } from "@/lib/slug";
import type { MonitorState } from "@/lib/types/enums";

export const metadata = { title: "Monitors" };

// Every monitor of the project, the worst state first and then by name, as
// the lane answers them: read from Postgres alone, as the status board is,
// and drawn again on the ping lane's nudge (`realtime-listener.tsx`), never
// on a timer. Narrowed by state and searched by name in the URL. Owners and
// admins make monitors here and hold the project's ping key.
export default async function MonitorsPage({
  params,
  searchParams,
}: {
  params: Promise<{ organization: string; project: string }>;
  searchParams?: Promise<SearchParams>;
}) {
  const { organization, project: slug } = await params;
  const query = parseMonitorQuery((await searchParams) ?? {});
  const [gate, project] = await Promise.all([getServerContext(), fetchProjectBySlug(slug)]);
  if (!gate || !project) return <ServiceUnavailable />;
  const canManage = administersProject(project.role);
  const [access, zones, keys] = await Promise.all([
    fetchMonitors(project.id),
    canManage ? fetchTimezones(project.id) : null,
    canManage ? fetchPingKeys(project.id) : null,
  ]);
  if (access.kind === "unavailable") return <ServiceUnavailable />;
  if (access.kind === "forbidden") {
    return (
      <AccessDenied
        what="this project's monitors"
        owner={ownerContact(activeOrganization(gate))}
        backHref={projectPath(organization, slug)}
      />
    );
  }

  const all = access.monitors;
  const shown = filterMonitors(all, query);
  const counts: Partial<Record<MonitorState, number>> = {};
  for (const monitor of all) counts[monitor.state] = (counts[monitor.state] ?? 0) + 1;

  return (
    <>
      <PageHeader
        title="Monitors"
        count={all.length}
        info="Every monitor of this project, the worst state first: each an agent's contract — when its runs are expected, how long one may go quiet and how many steps it may take."
        action={
          canManage ? (
            <div className="flex items-center gap-2">
              {keys?.kind === "ok" && <PingKeyMenu projectId={project.id} keys={keys.keys} />}
              <NewMonitor
                organization={organization}
                slug={slug}
                projectId={project.id}
                zones={zones}
              />
            </div>
          ) : undefined
        }
      />
      <div className="grid gap-6">
        <section className="grid gap-3">
          {all.length > 0 && (
            <MonitorFilters
              key={query.q ?? ""}
              organization={organization}
              slug={slug}
              query={query}
              counts={counts}
            />
          )}
          {all.length === 0 ? (
            <Notice title="No monitors yet.">
              A monitor watches one agent&apos;s runs: when they are expected, how long one may go
              quiet and how many steps it may take.{" "}
              {canManage
                ? "Make one with New monitor; the first ping to a slug, or the first run of an agent an SDK reports, makes one too."
                : "The project's owners and admins make them; the first ping to a slug makes one too."}
            </Notice>
          ) : shown.length === 0 ? (
            <Notice title="No monitors match.">
              <Link
                href={monitorsHref(organization, slug, {})}
                className="underline underline-offset-4"
              >
                Clear the filters
              </Link>
              .
            </Notice>
          ) : (
            <DataTable aria-label="Monitors">
              <THead>
                <Th>Name</Th>
                <Th>Agent</Th>
                <Th>Schedule</Th>
                <Th>State</Th>
                <Th>Last run</Th>
                <Th>Next due</Th>
                <Th>Bounds</Th>
              </THead>
              <tbody>
                {shown.map((monitor) => (
                  <tr key={monitor.id}>
                    <Td className="font-medium">
                      <Link
                        href={monitorHref(organization, slug, monitor.slug)}
                        className="underline-offset-4 hover:underline"
                      >
                        {monitor.name}
                      </Link>
                    </Td>
                    <Td className="font-mono text-xs text-muted-foreground">{monitor.agent}</Td>
                    <Td className="text-muted-foreground">{scheduleInWords(monitor)}</Td>
                    <Td>
                      <MonitorStateBadge state={monitor.state} />
                    </Td>
                    <Td className="text-xs text-muted-foreground">
                      {monitor.last_run ? (
                        <span className="grid gap-0.5">
                          <LocalTime iso={monitor.last_run.ended_at} mode="datetime" />
                          <span>{lastRunLine(monitor)}</span>
                        </span>
                      ) : (
                        "None yet"
                      )}
                    </Td>
                    <Td className="text-xs whitespace-nowrap text-muted-foreground">
                      {monitor.next_due_at ? (
                        <LocalTime iso={monitor.next_due_at} mode="datetime" />
                      ) : (
                        "—"
                      )}
                    </Td>
                    <Td className="text-xs text-muted-foreground">
                      {boundsInBrief(monitor.bounds) ?? "—"}
                    </Td>
                  </tr>
                ))}
              </tbody>
            </DataTable>
          )}
        </section>
      </div>
    </>
  );
}

/** What the page says in the table's place. */
function Notice({ title, children }: { title: string; children: React.ReactNode }) {
  return (
    <Card className="gap-0 overflow-hidden p-0">
      <div className="grid gap-1 p-4">
        <p className="text-sm font-medium">{title}</p>
        <p className="text-sm text-muted-foreground">{children}</p>
      </div>
    </Card>
  );
}
