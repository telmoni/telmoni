import { cache } from "react";
import { z } from "zod";

import { fetchWithTimeout } from "@/lib/api/fetch";
import { env } from "@/lib/env";
import { logger } from "@/lib/logger";

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
