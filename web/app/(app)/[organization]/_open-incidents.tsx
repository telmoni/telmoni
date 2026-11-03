import { BellRing } from "lucide-react";
import Link from "next/link";

import { OverviewCard } from "@/components/overview-card";
import { LocalTime } from "@/components/local-time";
import { NoticeCard } from "@/components/notice-card";
import { Row, Rows } from "@/components/rows";
import { Badge } from "@/components/ui/badge";
import { SEVERITY_LABEL, incidentHref, severityVariant } from "@/lib/alerts";
import type { OrganizationHealth } from "@/lib/server/data";
import { IncidentStatus } from "@/lib/types/enums";

export function OpenIncidents({
  organization,
  health,
}: {
  /** The organization's slug, which each link starts with. */
  organization: string;
  health: OrganizationHealth;
}) {
  if (health.kind === "forbidden") return null;
  return (
    <OverviewCard title="Open incidents" icon={BellRing}>
      {/* Saying nothing here would read as all being well. */}
      {health.kind === "ok" && health.incidents.length === 0 ? (
        <p className="text-muted-foreground">None open on any project you can open.</p>
      ) : health.kind === "unavailable" ? (
        <NoticeCard
          title="The open incidents could not be read."
          body="Reload to try again. Each project’s Alerts page lists its own."
        />
      ) : (
        <Rows className="rounded-none p-0 ring-0">
          {health.incidents.map((incident) => (
            <Row
              key={incident.id}
              keep
              label={
                <Link
                  href={incidentHref(organization, incident.project.slug, incident.id)}
                  className="underline-offset-4 hover:underline"
                >
                  {incident.rule_name}
                  <span className="sr-only"> in {incident.project.name}</span>
                </Link>
              }
              hint={
                <>
                  {incident.project.name} · opened <LocalTime iso={incident.opened_at} />
                  {incident.status === IncidentStatus.Acknowledged && " · acknowledged"}
                </>
              }
            >
              <Badge variant={severityVariant(incident.severity)} className="font-normal">
                {SEVERITY_LABEL[incident.severity]}
              </Badge>
            </Row>
          ))}
        </Rows>
      )}
    </OverviewCard>
  );
}
