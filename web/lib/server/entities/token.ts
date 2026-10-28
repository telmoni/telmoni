import { cache } from "react";
import { z } from "zod";

import { env } from "@/lib/env";
import { ApiKeyScope } from "@/lib/types/enums";

import { identityContext, projectHeaders } from "./identity-context";
import { readJson } from "./read";

const ApiTokenSchema = z.object({
  id: z.string(),
  name: z.string(),
  tokenHint: z.string(),
  scopes: z.array(z.enum(ApiKeyScope)),
  expiresAt: z.string().nullable(),
  lastUsedAt: z.string().nullable(),
  createdAt: z.string(),
});
export type ApiToken = z.infer<typeof ApiTokenSchema>;

export type TokenAccess =
  | { kind: "ok"; tokens: ApiToken[] }
  | { kind: "forbidden" }
  | { kind: "unavailable" };

export const ApiTokenListSchema = z.array(ApiTokenSchema);

// A key minted: its id, and the key itself, shown this once.
export const MintedTokenSchema = z.object({ id: z.string(), token: z.string() });

export const fetchTokens = cache(
  async (projectId: string): Promise<TokenAccess> => {
    const ctx = await identityContext();
    if (!ctx) return { kind: "unavailable" };
    const read = await readJson(
      "fetchTokens",
      `${env.SERVER_URL}/internal/v1/tokens`,
      { headers: projectHeaders(ctx, projectId), cache: "no-store" },
      ApiTokenListSchema,
    );
    return read.kind === "ok" ? { kind: "ok", tokens: read.value } : read;
  },
);
