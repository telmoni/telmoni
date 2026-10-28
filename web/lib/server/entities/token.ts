import { cache } from "react";
import { z } from "zod";

import { env } from "@/lib/env";
import { ApiKeyScope } from "@/lib/types/enums";

import { identityContext, projectHeaders } from "./identity-context";
import { readJson } from "./read";

const ApiTokenSchema = z.object({
  id: z.string(),
  name: z.string(),
  token_hint: z.string(),
  scopes: z.array(z.enum(ApiKeyScope)),
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
    const read = await readJson(
      "fetchTokens",
      `${env.SERVER_URL}/internal/v1/tokens`,
      { headers: projectHeaders(ctx, projectId), cache: "no-store" },
      z.array(ApiTokenSchema),
    );
    return read.kind === "ok" ? { kind: "ok", tokens: read.value } : read;
  },
);
