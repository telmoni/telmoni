"use client";

import { useId, useState } from "react";

import { DataTable, THead, Th, Td } from "@/components/data-table";
import { LocalTime } from "@/components/local-time";
import { Label } from "@/components/ui/label";
import { Switch } from "@/components/ui/switch";
import { resourceLabel } from "@/lib/audit-labels";
import type { AuditEvent } from "@/lib/server/data";

export function AuditTable({ events }: { events: AuditEvent[] }) {
  const [showDetails, setShowDetails] = useState(false);
  const switchId = useId();

  return (
    <div className="grid gap-2">
      <div className="flex items-center justify-end gap-2">
        <Switch
          id={switchId}
          checked={showDetails}
          onCheckedChange={setShowDetails}
        />
        <Label htmlFor={switchId} className="text-sm text-muted-foreground">
          Details
        </Label>
      </div>

      <DataTable>
        <THead>
          <Th>When</Th>
          <Th>Actor</Th>
          <Th className="text-center">Action</Th>
          <Th>Resource</Th>
        </THead>
        <tbody>
          {events.map((event) => (
            <tr key={event.id} className="align-top">
              <Td className="whitespace-nowrap text-muted-foreground">
                <LocalTime iso={event.created_at} />
              </Td>
              <Td className="font-mono text-xs whitespace-nowrap">
                {event.actor_id}
              </Td>
              <Td className="text-center whitespace-nowrap">{event.action}</Td>
              <Td className="text-xs text-muted-foreground">
                <span className="whitespace-nowrap">{resourceLabel(event.resource_kind)}</span>
                {event.resource_id ? (
                  <span className="font-mono whitespace-nowrap">
                    {showDetails
                      ? ` · ${event.resource_id}`
                      : ` · ${event.resource_id.slice(0, 18)}`}
                  </span>
                ) : null}
                {showDetails ? <EventDetails event={event} /> : null}
              </Td>
            </tr>
          ))}
        </tbody>
      </DataTable>
    </div>
  );
}

function EventDetails({ event }: { event: AuditEvent }) {
  const fields: [string, string][] = [];
  if (event.in_project) fields.push(["Project", event.in_project]);
  if (event.request_id) fields.push(["Request", event.request_id]);
  if (event.row_hash) fields.push(["Row hash", event.row_hash]);
  if (event.metadata !== null && event.metadata !== undefined) {
    fields.push(["Changed", stableJson(event.metadata)]);
  }
  if (fields.length === 0) return null;

  return (
    <dl className="mt-1.5 grid grid-cols-[auto_1fr] gap-x-2 gap-y-0.5">
      {fields.map(([label, value]) => (
        <div key={label} className="contents">
          <dt className="text-muted-foreground/70">{label}</dt>
          <dd className="font-mono break-all">{value}</dd>
        </div>
      ))}
    </dl>
  );
}

function stableJson(value: unknown): string {
  const sort = (v: unknown): unknown => {
    if (Array.isArray(v)) return v.map(sort);
    if (v !== null && typeof v === "object") {
      return Object.fromEntries(
        Object.entries(v as Record<string, unknown>)
          .sort(([a], [b]) => a.localeCompare(b))
          .map(([k, inner]) => [k, sort(inner)]),
      );
    }
    return v;
  };
  return JSON.stringify(sort(value));
}
