"use server";

import { refresh } from "next/cache";
import { z } from "zod";

import { type ActionResult, refused } from "@/lib/action-result";
import { isApiKeyScope } from "@/lib/api-key-scopes";
import { extractProblem, tryFetchWithTimeout } from "@/lib/api/fetch";
import { rateLimit, sessionKey } from "@/lib/api/rate-limit";
import { env } from "@/lib/env";
import { Flag } from "@/lib/flags";
import { featureOff } from "@/lib/server/flags";
import { checkApiKeyDescription, checkApiKeyName } from "@/lib/names";
import { activeProjectForMutation } from "@/lib/server/identity";
import { sessionHeaders } from "@/lib/server/entities/identity-context";
import { getServerSession } from "@/lib/server/session";

const ONE_DAY = 24 * 60 * 60;
const TTL_OPTIONS = {
  "30d":   30  * ONE_DAY,
  "90d":   90  * ONE_DAY,
  "1y":    365 * ONE_DAY,
  "never": null,
} as const satisfies Record<string, number | null>;
type TtlOption = keyof typeof TTL_OPTIONS;

// Its own keys alone: `in` would also let "toString" through, whose value is
// a function JSON drops, minting a key with no expiry asked for.
function isTtl(s: unknown): s is TtlOption {
  return typeof s === "string" && Object.hasOwn(TTL_OPTIONS, s);
}

const MintedToken = z.object({ id: z.string(), token: z.string() });

/** A key, shown this once, and its id. */
export type MintedKey = { token: string; id: string };
export type MintResult = ActionResult<MintedKey>;

/** What the mint dialog sends: what the browser could send, before each field
 *  is checked below. */
const MintInputSchema = z.object({
  name: z.string(),
  description: z.string(),
  ttl: z.string(),
  scopes: z.array(z.string()),
});

export async function mintTokenAction(projectId: string, input: unknown): Promise<MintResult> {
  if (!projectId) {
    return refused("API keys are scoped by project, not organization.");
  }
  const session = await getServerSession();
  if (!session) return refused("Your session expired — sign in again.");

  const limited = await rateLimit(
    sessionKey(session, "tokens:mint"),
    { limit: 5, windowMs: 60_000 },
  );
  if (limited) return refused("Too many mint requests. Try again in a minute.");
  const off = await featureOff(Flag.ApiTokens);
  if (off) return refused(off);

  const asked = MintInputSchema.safeParse(input);
  if (!asked.success) return refused("That key could not be read.");
  const name = checkApiKeyName(asked.data.name);
  if (!name.ok) return refused(name.error);
  const described = checkApiKeyDescription(asked.data.description);
  if (!described.ok) return refused(described.error);
  const description = described.value || null;
  const ttl = asked.data.ttl;
  if (!isTtl(ttl)) return refused("Invalid TTL selection.");
  const scopes = asked.data.scopes;
  if (scopes.length === 0 || !scopes.every(isApiKeyScope)) {
    return refused("Choose what the key opens: sending spans, reading the API, or both.");
  }

  const resolved = await activeProjectForMutation(projectId);
  if (resolved.error !== null) return refused(resolved.error);
  const expires_in_seconds = TTL_OPTIONS[ttl];

  const res = await tryFetchWithTimeout(`${env.SERVER_URL}/internal/v1/tokens`, {
    method: "POST",
    headers: {
      ...sessionHeaders(session, resolved.organizationId, resolved.projectId),
      "content-type": "application/json",
    },
    body: JSON.stringify({
      name:               name.value,
      description,
      created_by:         session.userId,
      scopes,
      expires_in_seconds,
    }),
  });
  if (!res) return refused("The key service is unreachable. Try again.");
  if (!res.ok) {
    const { message } = await extractProblem(res);
    return refused(message);
  }

  // The key exists from here on, whatever its answer reads as: the list is
  // drawn again with it.
  refresh();
  const parsed = MintedToken.safeParse(await res.json().catch(() => null));
  if (!parsed.success) {
    return refused("Unexpected response from the key service.");
  }
  const body = parsed.data;
  return { ok: true, token: body.token, id: body.id };
}

export async function rotateTokenAction(
  projectId: string,
  tokenId: string,
): Promise<MintResult> {
  if (!projectId) {
    return refused("API keys are scoped by project, not organization.");
  }
  const session = await getServerSession();
  if (!session) return refused("Your session expired — sign in again.");

  const limited = await rateLimit(
    sessionKey(session, "tokens:rotate"),
    { limit: 10, windowMs: 60_000 },
  );
  if (limited) return refused("Too many rotate requests.");
  const off = await featureOff(Flag.ApiTokens);
  if (off) return refused(off);
  if (!z.guid().safeParse(tokenId).success) return refused("Invalid key id.");

  const resolved = await activeProjectForMutation(projectId);
  if (resolved.error !== null) return refused(resolved.error);
  const res = await tryFetchWithTimeout(
    `${env.SERVER_URL}/internal/v1/tokens/${encodeURIComponent(tokenId)}/rotate`,
    {
      method: "POST",
      headers: {
        ...sessionHeaders(session, resolved.organizationId, resolved.projectId),
        "content-type": "application/json",
      },
      body: JSON.stringify({}),
    },
  );
  if (!res) return refused("The key service is unreachable. Try again.");
  if (!res.ok) {
    const { message } = await extractProblem(res);
    return refused(message);
  }
  refresh();
  const parsed = MintedToken.safeParse(await res.json().catch(() => null));
  if (!parsed.success) {
    return refused("Unexpected response from the key service.");
  }
  const body = parsed.data;
  return { ok: true, token: body.token, id: body.id };
}

export async function revokeTokenAction(
  projectId: string,
  tokenId: string,
): Promise<ActionResult> {
  if (!projectId) {
    return refused("API keys are scoped by project, not organization.");
  }
  const session = await getServerSession();
  if (!session) return refused("Your session expired — sign in again.");

  const limited = await rateLimit(
    sessionKey(session, "tokens:revoke"),
    { limit: 20, windowMs: 60_000 },
  );
  if (limited) return refused("Too many revoke requests.");
  if (!z.guid().safeParse(tokenId).success) return refused("Invalid key id.");

  const resolved = await activeProjectForMutation(projectId);
  if (resolved.error !== null) return refused(resolved.error);
  const res = await tryFetchWithTimeout(
    `${env.SERVER_URL}/internal/v1/tokens/${encodeURIComponent(tokenId)}`,
    {
      method: "DELETE",
      headers: sessionHeaders(session, resolved.organizationId, resolved.projectId),
    },
  );
  if (!res) return refused("The key service is unreachable. Try again.");
  if (!res.ok && res.status !== 404) {
    const { message } = await extractProblem(res);
    return refused(message);
  }
  refresh();
  return { ok: true };
}
