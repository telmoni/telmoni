import type { Alert, Incident } from "@/lib/server/entities/alerts";
import { IncidentStatus } from "@/lib/types/enums";

/** The agents an alert is held to by its filter; none is every agent. */
function heldTo(alert: Alert): readonly string[] {
  return alert.filters.agent ?? [];
}

/** The alerts that read `agent`'s runs, paused ones aside: those held to it,
 *  and those over every agent. */
export function watchersOf(alerts: readonly Alert[], agent: string): number {
  return alerts.filter((alert) => {
    const agents = heldTo(alert);
    return alert.paused === null && (agents.length === 0 || agents.includes(agent));
  }).length;
}

/** The open incidents of the alerts held to `agent`, or, for `null`, of those
 *  over every agent, which no one agent's row counts. */
export function openIncidentsOf(
  alerts: readonly Alert[],
  incidents: readonly Incident[],
  agent: string | null,
): number {
  const ruled = new Map(alerts.map((alert) => [alert.id, heldTo(alert)]));
  return incidents.filter((incident) => {
    if (incident.status === IncidentStatus.Resolved) return false;
    const agents = incident.rule_id === null ? undefined : ruled.get(incident.rule_id);
    if (agents === undefined) return false;
    return agent === null ? agents.length === 0 : agents.includes(agent);
  }).length;
}
