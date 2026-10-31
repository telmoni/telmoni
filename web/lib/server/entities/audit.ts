import { cache } from "react";
import { z } from "zod";

import { fetchWithTimeout } from "@/lib/api/fetch";
import { env } from "@/lib/env";
import { logger } from "@/lib/logger";
import {
  AuditExportListSchema,
  AuditExportSchema,
  type AuditExport,
  type AuditExportFormat,
  type AuditExportLimits,
} from "@/lib/types/audit-export";
import { ResourceKind } from "@/lib/types/enums";

import { identityContext, organizationHeaders, projectHeaders } from "./identity-context";
import { readFailed, readJson } from "./read";

const AuditEventSchema = z.object({
  id: z.string(),
  actorId: z.string(),
  action: z.string(),
  resourceKind: z.enum(ResourceKind),
  resourceId: z.string().nullable(),
  createdAt: z.string(),
  requestId: z.string().nullish(),
  metadata: z.unknown().nullish(),
  inProject: z.string().nullish(),
  prevHash: z.string().nullish(),
  rowHash: z.string().nullish(),
});
export type AuditEvent = z.infer<typeof AuditEventSchema>;

export const AuditPageSchema = z.object({ events: z.array(AuditEventSchema) });

const LIST_LIMIT = 100;

export type AuditAccess =
  | { kind: "ok"; events: AuditEvent[] }
  | { kind: "forbidden" }
  | { kind: "unavailable" };

/** One action on one kind of resource, since `from`: what a page that lists
 *  a single kind of event narrows the log to. */
export interface AuditFilter {
  action: string;
  resourceKind: ResourceKind;
  from: string;
}

export const fetchAuditEvents = cache(
  async (
    projectId: string,
    limit: number = LIST_LIMIT,
    filter?: AuditFilter,
  ): Promise<AuditAccess> => {
    const ctx = await identityContext();
    if (!ctx) return { kind: "unavailable" };
    const query = new URLSearchParams({ limit: String(limit) });
    if (filter) {
      query.set("action", filter.action);
      query.set("resource_kind", filter.resourceKind);
      query.set("from", filter.from);
    }
    return listAudit(
      "fetchAuditEvents",
      `${env.SERVER_URL}/internal/v1/audit/projects/${encodeURIComponent(projectId)}?${query}`,
      projectHeaders(ctx, projectId),
    );
  },
);

export const fetchOrganizationAudit = cache(
  async (limit: number = LIST_LIMIT): Promise<AuditAccess> => {
    const ctx = await identityContext();
    if (!ctx) return { kind: "unavailable" };
    return listAudit(
      "fetchOrganizationAudit",
      `${env.SERVER_URL}/internal/v1/audit/organizations/${encodeURIComponent(
        ctx.organizationId,
      )}?limit=${limit}`,
      organizationHeaders(ctx),
    );
  },
);

export type AuditExportsAccess =
  | { kind: "ok"; exports: AuditExport[]; limits: AuditExportLimits }
  | { kind: "forbidden" }
  | { kind: "unavailable" };

export type StartedAuditExport =
  | { kind: "ok"; export: AuditExport }
  | { kind: "forbidden" }
  /** Three of the person's exports are still being built. */
  | { kind: "busy" }
  /** A range that ends before it starts, or starts after now. */
  | { kind: "invalid" }
  | { kind: "unavailable" };

export type AuditExportFile =
  | { kind: "ok"; body: ArrayBuffer; contentType: string }
  | { kind: "forbidden" }
  /** Not the caller's, never finished, or past its week. */
  | { kind: "missing" }
  /** The organization's data out for the day is spent: until `retryAfter`. */
  | { kind: "spent"; retryAfter: string | null }
  | { kind: "unavailable" };

/** A file is read whole before it is handed on, so the whole of it must
 *  arrive inside this: up to auth's 24 MiB, from inside the cluster. */
const FILE_TIMEOUT_MS = 60_000;

function exportsUrl(organizationId: string, id?: string): string {
  const base = `${env.SERVER_URL}/internal/v1/audit/organizations/${encodeURIComponent(
    organizationId,
  )}/exports`;
  return id ? `${base}/${encodeURIComponent(id)}` : base;
}

/** The caller's latest exports of the active organization's audit log, newest
 *  first, as the bell and the export dialog show them. */
export async function listAuditExports(): Promise<AuditExportsAccess> {
  const ctx = await identityContext();
  if (!ctx) return { kind: "unavailable" };
  const read = await readJson(
    "listAuditExports",
    exportsUrl(ctx.organizationId),
    { headers: organizationHeaders(ctx) },
    AuditExportListSchema,
  );
  if (read.kind !== "ok") return read;
  const { exports, maxRows, maxBytes } = read.value;
  return { kind: "ok", exports, limits: { maxRows, maxBytes } };
}

/** Queue an export of the active organization's audit log; auth builds it in
 *  the background and records it on the chain once built. */
export async function startAuditExport(input: {
  format: AuditExportFormat;
  from: string | null;
  to: string | null;
}): Promise<StartedAuditExport> {
  const ctx = await identityContext();
  if (!ctx) return { kind: "unavailable" };
  const read = await readJson(
    "startAuditExport",
    exportsUrl(ctx.organizationId),
    {
      method: "POST",
      headers: { ...organizationHeaders(ctx), "content-type": "application/json" },
      body: JSON.stringify({
        format: input.format,
        ...(input.from ? { from: input.from } : {}),
        ...(input.to ? { to: input.to } : {}),
      }),
    },
    AuditExportSchema,
    { 409: "busy", 400: "invalid" },
  );
  return read.kind === "ok" ? { kind: "ok", export: read.value } : read;
}

/**
 * One finished file, in the format it was built in. Passed through as auth
 * wrote it, never parsed and re-serialized: a verifier recomputes the hashes
 * from these very bytes, and a round trip through JavaScript numbers could
 * change one.
 */
export async function auditExportFile(id: string): Promise<AuditExportFile> {
  const ctx = await identityContext();
  if (!ctx) return { kind: "unavailable" };
  try {
    const res = await fetchWithTimeout(
      exportsUrl(ctx.organizationId, id),
      { headers: organizationHeaders(ctx) },
      FILE_TIMEOUT_MS,
    );
    if (res.status === 403) return { kind: "forbidden" };
    if (res.status === 404 || res.status === 400) return { kind: "missing" };
    if (res.status === 429) return { kind: "spent", retryAfter: res.headers.get("retry-after") };
    if (!res.ok) {
      logger.warn({ fetcher: "auditExportFile", status: res.status }, "entities: upstream refused");
      return { kind: "unavailable" };
    }
    return {
      kind: "ok",
      body: await res.arrayBuffer(),
      contentType: res.headers.get("content-type") ?? "application/octet-stream",
    };
  } catch (err) {
    readFailed("auditExportFile", err);
    return { kind: "unavailable" };
  }
}

async function listAudit(
  fetcher: string,
  url: string,
  headers: Record<string, string>,
): Promise<AuditAccess> {
  const read = await readJson(fetcher, url, { headers }, AuditPageSchema);
  return read.kind === "ok" ? { kind: "ok", events: read.value.events } : read;
}
