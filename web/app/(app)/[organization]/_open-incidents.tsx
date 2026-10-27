import Link from "next/link";

import { LocalTime } from "@/components/local-time";
import { Row, Rows } from "@/components/rows";
import { Badge } from "@/components/ui/badge";
import { Card } from "@/components/ui/card";
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
  if (health.kind === "ok" && health.incidents.length === 0) return null;
  return (
    <section className="grid gap-3" aria-label="Open incidents">
      <div className="flex items-center justify-between gap-2">
        <h2 className="text-sm font-medium">Open incidents</h2>
      </div>
      {/* Saying nothing here would read as all being well. */}
      {health.kind === "unavailable" ? (
        <Card className="grid gap-1 text-sm">
          <p className="font-medium">The open incidents could not be read.</p>
          <p className="text-muted-foreground">
            Reload to try again. Each project&rsquo;s Alerts page lists its own.
          </p>
        </Card>
      ) : (
        <Rows>
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
    </section>
  );
}
