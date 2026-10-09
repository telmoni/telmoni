import { cache } from "react";
import { z } from "zod";

import { fetchWithTimeout } from "@/lib/api/fetch";
import { env } from "@/lib/env";
import { logger } from "@/lib/logger";
import {
  AuditExportSchema,
  type AuditExport,
  type AuditExportFormat,
} from "@/lib/types/audit-export";

import { identityContext, organizationHeaders, projectHeaders } from "./identity-context";

const AuditEventSchema = z.object({
  id: z.string(),
  actor_id: z.string(),
  action: z.string(),
  resource_kind: z.string(),
  resource_id: z.string().nullable(),
  created_at: z.string(),
  request_id: z.string().nullish(),
  metadata: z.unknown().nullish(),
  in_project: z.string().nullish(),
  prev_hash: z.string().nullish(),
  row_hash: z.string().nullish(),
});
export type AuditEvent = z.infer<typeof AuditEventSchema>;

const ListSchema = z.object({ events: z.array(AuditEventSchema) });

const LIST_LIMIT = 100;

export type AuditAccess =
  | { kind: "ok"; events: AuditEvent[] }
  | { kind: "forbidden" }
  | { kind: "unavailable" };

export const fetchAuditEvents = cache(
  async (projectId: string, limit: number = LIST_LIMIT): Promise<AuditAccess> => {
    const ctx = await identityContext();
    if (!ctx) return { kind: "unavailable" };
    return listAudit(
      `${env.SERVER_URL}/internal/audit/projects/${encodeURIComponent(
        projectId,
      )}?limit=${limit}`,
      projectHeaders(ctx, projectId),
    );
  },
);

export const fetchOrganizationAudit = cache(
  async (limit: number = LIST_LIMIT): Promise<AuditAccess> => {
    const ctx = await identityContext();
    if (!ctx) return { kind: "unavailable" };
    return listAudit(
      `${env.SERVER_URL}/internal/audit/organizations/${encodeURIComponent(
        ctx.organizationId,
      )}?limit=${limit}`,
      organizationHeaders(ctx),
    );
  },
);

const ExportListSchema = z.object({ exports: z.array(AuditExportSchema) });

export type AuditExportsAccess =
  | { kind: "ok"; exports: AuditExport[] }
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
  | { kind: "gone" }
  | { kind: "unavailable" };

/** A file is read whole before it is handed on, so the whole of it must
 *  arrive inside this: up to auth's 24 MiB, from inside the cluster. */
const FILE_TIMEOUT_MS = 60_000;

function exportsUrl(organizationId: string, id?: string): string {
  const base = `${env.SERVER_URL}/internal/audit/organizations/${encodeURIComponent(
    organizationId,
  )}/exports`;
  return id ? `${base}/${encodeURIComponent(id)}` : base;
}

function fetchFailed(fetcher: string, err: unknown): void {
  logger.warn(
    { fetcher, error: err instanceof Error ? err.message : String(err) },
    "entities: fetch failed",
  );
}

/** The caller's latest exports of the active organization's audit log, newest
 *  first, as the bell and the export dialog show them. */
export async function listAuditExports(): Promise<AuditExportsAccess> {
  const ctx = await identityContext();
  if (!ctx) return { kind: "unavailable" };
  try {
    const res = await fetchWithTimeout(exportsUrl(ctx.organizationId), {
      headers: organizationHeaders(ctx),
    });
    if (res.status === 403) return { kind: "forbidden" };
    if (!res.ok) return { kind: "unavailable" };
    const parsed = ExportListSchema.safeParse(await res.json());
    if (!parsed.success) {
      logger.warn(
        { fetcher: "listAuditExports", issues: parsed.error.issues },
        "entities: upstream shape mismatch",
      );
      return { kind: "unavailable" };
    }
    return { kind: "ok", exports: parsed.data.exports };
  } catch (err) {
    fetchFailed("listAuditExports", err);
    return { kind: "unavailable" };
  }
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
  try {
    const res = await fetchWithTimeout(exportsUrl(ctx.organizationId), {
      method: "POST",
      headers: { ...organizationHeaders(ctx), "content-type": "application/json" },
      body: JSON.stringify({
        format: input.format,
        ...(input.from ? { from: input.from } : {}),
        ...(input.to ? { to: input.to } : {}),
      }),
    });
    if (res.status === 403) return { kind: "forbidden" };
    if (res.status === 409) return { kind: "busy" };
    if (res.status === 400) return { kind: "invalid" };
    if (!res.ok) return { kind: "unavailable" };
    const parsed = AuditExportSchema.safeParse(await res.json());
    if (!parsed.success) {
      logger.warn(
        { fetcher: "startAuditExport", issues: parsed.error.issues },
        "entities: upstream shape mismatch",
      );
      return { kind: "unavailable" };
    }
    return { kind: "ok", export: parsed.data };
  } catch (err) {
    fetchFailed("startAuditExport", err);
    return { kind: "unavailable" };
  }
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
    if (res.status === 404 || res.status === 400) return { kind: "gone" };
    if (!res.ok) return { kind: "unavailable" };
    return {
      kind: "ok",
      body: await res.arrayBuffer(),
      contentType: res.headers.get("content-type") ?? "application/octet-stream",
    };
  } catch (err) {
    fetchFailed("auditExportFile", err);
    return { kind: "unavailable" };
  }
}

async function listAudit(
  url: string,
  headers: Record<string, string>,
): Promise<AuditAccess> {
  try {
    const res = await fetchWithTimeout(url, { headers });
    if (res.status === 403) return { kind: "forbidden" };
    if (!res.ok) return { kind: "unavailable" };
    const parsed = ListSchema.safeParse(await res.json());
    if (!parsed.success) {
      logger.warn(
        { fetcher: "audit", issues: parsed.error.issues },
        "entities: upstream shape mismatch",
      );
      return { kind: "unavailable" };
    }
    return { kind: "ok", events: parsed.data.events };
  } catch (err) {
    logger.warn(
      { fetcher: "audit", error: err instanceof Error ? err.message : String(err) },
      "entities: fetch failed",
    );
    return { kind: "unavailable" };
  }
}
