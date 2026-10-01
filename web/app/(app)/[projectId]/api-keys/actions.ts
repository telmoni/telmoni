"use server";

import { redirect } from "next/navigation";
import { z } from "zod";

import { extractProblem, tryFetchWithTimeout } from "@/lib/api/fetch";
import { rateLimit, sessionKey } from "@/lib/api/rate-limit";
import { env } from "@/lib/env";
import { Flag } from "@/lib/flags";
import { featureOff } from "@/lib/server/flags";
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

function isTtl(s: unknown): s is TtlOption {
  return typeof s === "string" && s in TTL_OPTIONS;
}

const MintedToken = z.object({ id: z.string(), token: z.string() });

export interface MintResult {
  status:  "ok" | "error";
  token?:  string;
  prefix?: string;
  id?:     string;
  error?:  string;
}

export async function mintTokenAction(
  _prev: MintResult | undefined,
  formData: FormData,
): Promise<MintResult> {
  const projectId = String(formData.get("projectId") ?? "");
  if (!projectId) {
    return { status: "error", error: "API keys are scoped by project, not organization." };
  }
  const session = await getServerSession();
  if (!session) redirect("/auth/login");

  const limited = await rateLimit(
    sessionKey(session, "tokens:mint"),
    { limit: 5, windowMs: 60_000 },
  );
  if (limited) return { status: "error", error: "Too many mint requests. Try again in a minute." };
  const off = await featureOff(Flag.ApiTokens);
  if (off) return { status: "error", error: off };

  const name = String(formData.get("name") ?? "").trim();
  if (!name || name.length > 100) {
    return { status: "error", error: "Name is required (≤100 chars)." };
  }
  const description = String(formData.get("description") ?? "").trim() || null;
  const ttl = formData.get("ttl");
  if (!isTtl(ttl)) return { status: "error", error: "Invalid TTL selection." };

  const resolved = await activeProjectForMutation(projectId);
  if (resolved.error !== null) return { status: "error", error: resolved.error };
  const expires_in_seconds = TTL_OPTIONS[ttl];

  const res = await tryFetchWithTimeout(`${env.SERVER_URL}/internal/tokens`, {
    method: "POST",
    headers: {
      ...sessionHeaders(session, resolved.organizationId, resolved.projectId),
      "content-type": "application/json",
    },
    body: JSON.stringify({
      name,
      description,
      created_by:         session.userId,
      expires_in_seconds,
    }),
  });
  if (!res) return { status: "error", error: "The key service is unreachable. Try again." };
  if (!res.ok) {
    const { message } = await extractProblem(res);
    return { status: "error", error: message };
  }

  const parsed = MintedToken.safeParse(await res.json().catch(() => null));
  if (!parsed.success) {
    return { status: "error", error: "Unexpected response from the key service." };
  }
  const body = parsed.data;
  const prefix = body.token.slice(0, 9);

  return { status: "ok", token: body.token, prefix, id: body.id };
}

export async function rotateTokenAction(
  projectId: string,
  tokenId: string,
): Promise<MintResult> {
  if (!projectId) {
    return { status: "error", error: "API keys are scoped by project, not organization." };
  }
  const session = await getServerSession();
  if (!session) redirect("/auth/login");

  const limited = await rateLimit(
    sessionKey(session, "tokens:rotate"),
    { limit: 10, windowMs: 60_000 },
  );
  if (limited) return { status: "error", error: "Too many rotate requests." };
  const off = await featureOff(Flag.ApiTokens);
  if (off) return { status: "error", error: off };
  if (!z.guid().safeParse(tokenId).success) return { status: "error", error: "Invalid key id." };

  const resolved = await activeProjectForMutation(projectId);
  if (resolved.error !== null) return { status: "error", error: resolved.error };
  const res = await tryFetchWithTimeout(
    `${env.SERVER_URL}/internal/tokens/${encodeURIComponent(tokenId)}/rotate`,
    {
      method: "POST",
      headers: {
        ...sessionHeaders(session, resolved.organizationId, resolved.projectId),
        "content-type": "application/json",
      },
      body: JSON.stringify({}),
    },
  );
  if (!res) return { status: "error", error: "The key service is unreachable. Try again." };
  if (!res.ok) {
    const { message } = await extractProblem(res);
    return { status: "error", error: message };
  }
  const parsed = MintedToken.safeParse(await res.json().catch(() => null));
  if (!parsed.success) {
    return { status: "error", error: "Unexpected response from the key service." };
  }
  const body = parsed.data;
  return { status: "ok", token: body.token, prefix: body.token.slice(0, 9), id: body.id };
}

export async function revokeTokenAction(
  projectId: string,
  tokenId: string,
): Promise<{ error?: string }> {
  if (!projectId) {
    return { error: "API keys are scoped by project, not organization." };
  }
  const session = await getServerSession();
  if (!session) redirect("/auth/login");

  const limited = await rateLimit(
    sessionKey(session, "tokens:revoke"),
    { limit: 20, windowMs: 60_000 },
  );
  if (limited) return { error: "Too many revoke requests." };
  if (!z.guid().safeParse(tokenId).success) return { error: "Invalid key id." };

  const resolved = await activeProjectForMutation(projectId);
  if (resolved.error !== null) return { error: resolved.error };
  const res = await tryFetchWithTimeout(
    `${env.SERVER_URL}/internal/tokens/${encodeURIComponent(tokenId)}`,
    {
      method: "DELETE",
      headers: sessionHeaders(session, resolved.organizationId, resolved.projectId),
    },
  );
  if (!res) return { error: "The key service is unreachable. Try again." };
  if (!res.ok && res.status !== 404) {
    const { message } = await extractProblem(res);
    return { error: message };
  }
  return {};
}
