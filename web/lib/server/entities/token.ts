import { cache } from "react";
import { z } from "zod";

import { fetchWithTimeout } from "@/lib/api/fetch";
import { env } from "@/lib/env";
import { logger } from "@/lib/logger";

import { identityContext, projectHeaders } from "./identity-context";

const ApiTokenSchema = z.object({
  id: z.string(),
  name: z.string(),
  expires_at: z.string().nullable(),
  last_used_at: z.string().nullable(),
  created_at: z.string(),
});
export type ApiToken = z.infer<typeof ApiTokenSchema>;

export type TokenAccess =
  | { kind: "ok"; tokens: ApiToken[] }
  | { kind: "forbidden" }
  | { kind: "unavailable" };

export const fetchTokens = cache(
  async (projectId: string): Promise<TokenAccess> => {
    if (!projectId || projectId === "organization" || projectId === "org") return { kind: "unavailable" };
    const ctx = await identityContext();
    if (!ctx) return { kind: "unavailable" };
    try {
      const res = await fetchWithTimeout(
        `${env.SERVER_URL}/internal/tokens`,
        {
          headers: projectHeaders(ctx, projectId),
          cache: "no-store",
        },
      );
      if (res.status === 403) return { kind: "forbidden" };
      if (!res.ok) return { kind: "unavailable" };
      const parsed = z.array(ApiTokenSchema).safeParse(await res.json());
      if (!parsed.success) {
        logger.warn(
          { fetcher: "fetchTokens", issues: parsed.error.issues },
          "entities: upstream shape mismatch",
        );
        return { kind: "unavailable" };
      }
      return { kind: "ok", tokens: parsed.data };
    } catch (err) {
      logger.warn(
        {
          fetcher: "fetchTokens",
          error: err instanceof Error ? err.message : String(err),
        },
        "entities: fetch failed",
      );
      return { kind: "unavailable" };
    }
  },
);
