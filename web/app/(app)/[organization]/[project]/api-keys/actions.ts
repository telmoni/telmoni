"use server";

import { refresh } from "next/cache";
import { z } from "zod";

import { type ActionResult, refused } from "@/lib/action-result";
import { isApiKeyScope } from "@/lib/api-key-scopes";
import { extractProblem, tryFetchWithTimeout } from "@/lib/api/fetch";
import { env } from "@/lib/env";
import { Flag } from "@/lib/flags";
import { checkApiKeyDescription, checkApiKeyName } from "@/lib/names";
import { admitProject } from "@/lib/server/admit";
import { featureOff } from "@/lib/server/flags";

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
  const gate = await admitProject({ call: "tokens:mint", limit: 5 }, projectId);
  if (!gate.ok) return refused(gate.error);
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

  const expires_in_seconds = TTL_OPTIONS[ttl];

  const res = await tryFetchWithTimeout(`${env.SERVER_URL}/internal/v1/tokens`, {
    method: "POST",
    headers: { ...gate.headers, "content-type": "application/json" },
    body: JSON.stringify({
      name:               name.value,
      description,
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
  const gate = await admitProject({ call: "tokens:rotate", limit: 10 }, projectId);
  if (!gate.ok) return refused(gate.error);
  const off = await featureOff(Flag.ApiTokens);
  if (off) return refused(off);
  if (!z.guid().safeParse(tokenId).success) return refused("Invalid key id.");

  const res = await tryFetchWithTimeout(
    `${env.SERVER_URL}/internal/v1/tokens/${encodeURIComponent(tokenId)}/rotate`,
    {
      method: "POST",
      headers: { ...gate.headers, "content-type": "application/json" },
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
  const gate = await admitProject({ call: "tokens:revoke", limit: 20 }, projectId);
  if (!gate.ok) return refused(gate.error);
  if (!z.guid().safeParse(tokenId).success) return refused("Invalid key id.");

  const res = await tryFetchWithTimeout(
    `${env.SERVER_URL}/internal/v1/tokens/${encodeURIComponent(tokenId)}`,
    { method: "DELETE", headers: gate.headers },
  );
  if (!res) return refused("The key service is unreachable. Try again.");
  if (!res.ok && res.status !== 404) {
    const { message } = await extractProblem(res);
    return refused(message);
  }
  refresh();
  return { ok: true };
}
