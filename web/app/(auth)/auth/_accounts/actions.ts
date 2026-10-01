"use server";

import { headers } from "next/headers";
import { z } from "zod";

import { extractProblem, tryFetchWithTimeout } from "@/lib/api/fetch";
import { clientKeyFromHeaders, rateLimit, sessionKey } from "@/lib/api/rate-limit";
import { env } from "@/lib/env";
import { personHeaders } from "@/lib/server/entities/identity-context";
import { getServerSession } from "@/lib/server/session";

// The console's half of accounts auth holds itself. Each action posts to one
// of auth's `/internal/auth/password/*` lanes, which exist only on a
// deployment whose provider is auth's own; under an external provider they
// answer 404, which reads here as "unavailable", and nothing links to these
// pages there. Every action is a public endpoint reached before any session
// exists, so each is metered by the caller's address.

type Result = { error: string | null };

const UNAVAILABLE = "Sign-in is unavailable right now. Try again in a minute.";

// Shape only; auth validates properly.
const email = z.email().max(254);
// Auth holds the same floor (`hashing::MIN_PASSWORD_CHARS`); this saves a
// round trip for a password that could never be accepted.
const newPassword = z.string().min(8, "Use at least 8 characters.").max(256);
const token = z.string().min(1).max(512);
const userId = z.string().min(1).max(256);

function serviceHeaders(): Record<string, string> {
  return {
    "content-type": "application/json",
    "x-service-secret": env.SERVICE_SECRET,
    "x-request-id": crypto.randomUUID(),
  };
}

async function limitedByAddress(
  scope: string,
  ceiling: { limit: number; windowMs: number },
): Promise<boolean> {
  const key = clientKeyFromHeaders(await headers(), scope);
  return (await rateLimit(key, ceiling)) !== null;
}

// The problem's own words when auth explained itself, else the generic line.
async function refusal(res: Response, fallback?: string): Promise<string> {
  const { message, problem } = await extractProblem(res);
  return problem?.detail || fallback || message;
}

async function post(path: string, body: unknown, extra: Record<string, string> = {}) {
  return tryFetchWithTimeout(`${env.SERVER_URL}${path}`, {
    method: "POST",
    headers: { ...serviceHeaders(), ...extra },
    body: JSON.stringify(body),
  });
}

// Check the password. A right one earns the code the callback spends, and
// the browser goes there next; the state is the one `/auth/login` sealed,
// and the callback is what checks it.
export async function signInAction(input: {
  email: string;
  password: string;
  state: string;
}): Promise<Result & { next?: string }> {
  const parsed = z
    .object({ email, password: z.string().min(1).max(256), state: z.string().min(1).max(200) })
    .safeParse(input);
  if (!parsed.success) return { error: "Enter your email address and password." };

  if (await limitedByAddress("accounts:sign-in", { limit: 10, windowMs: 60_000 })) {
    return { error: "Too many attempts. Try again in a minute." };
  }

  const res = await post("/internal/auth/password/sign-in", {
    email: parsed.data.email,
    password: parsed.data.password,
  });
  if (!res) return { error: UNAVAILABLE };
  if (res.status === 401) return { error: "Wrong email address or password." };
  if (res.status === 404) return { error: UNAVAILABLE };
  // Auth locks the account after repeated wrong passwords; its problem says
  // how long in seconds, which is not a sentence.
  if (res.status === 429) {
    return { error: "Too many wrong passwords. Try again in a few minutes." };
  }
  if (!res.ok) return { error: await refusal(res) };

  const body = z.object({ code: z.string().min(1) }).safeParse(await res.json().catch(() => null));
  if (!body.success) return { error: UNAVAILABLE };
  const next = new URLSearchParams({ code: body.data.code, state: parsed.data.state });
  return { error: null, next: `/auth/callback?${next.toString()}` };
}

// Create an account. A 202 is an address to confirm from its mail; a 200
// carries the code that opens the session at once, and the browser goes to
// the callback with it, as after a sign-in. Auth's refusals — the address
// has an account, sign-ups are closed to the uninvited — come back in its
// own words.
export async function signUpAction(input: {
  email: string;
  password: string;
  givenName?: string;
  familyName?: string;
  state: string;
}): Promise<Result & { next?: string }> {
  const parsed = z
    .object({
      email,
      password: newPassword,
      givenName: z.string().trim().max(120).optional(),
      familyName: z.string().trim().max(120).optional(),
      state: z.string().min(1).max(200),
    })
    .safeParse(input);
  if (!parsed.success) {
    return { error: parsed.error.issues[0]?.message ?? "Check the form and try again." };
  }

  if (await limitedByAddress("accounts:sign-up", { limit: 5, windowMs: 60 * 60_000 })) {
    return { error: "Too many sign-ups from here. Try again in an hour." };
  }

  const { givenName, familyName, state, ...rest } = parsed.data;
  const res = await post("/internal/auth/password/sign-up", {
    ...rest,
    ...(givenName ? { givenName } : {}),
    ...(familyName ? { familyName } : {}),
  });
  if (!res) return { error: UNAVAILABLE };
  if (res.status === 404) return { error: UNAVAILABLE };
  if (!res.ok) return { error: await refusal(res) };
  if (res.status === 202) return { error: null };

  const body = z.object({ code: z.string().min(1) }).safeParse(await res.json().catch(() => null));
  if (!body.success) return { error: UNAVAILABLE };
  const next = new URLSearchParams({ code: body.data.code, state });
  return { error: null, next: `/auth/callback?${next.toString()}` };
}

// Spend the verification link.
export async function verifyEmailAction(input: { userId: string; token: string }): Promise<Result> {
  const parsed = z.object({ userId, token }).safeParse(input);
  if (!parsed.success) return { error: "This link is incomplete. Open it from the email again." };

  if (await limitedByAddress("accounts:verify", { limit: 10, windowMs: 60 * 60_000 })) {
    return { error: "Too many attempts. Try again in an hour." };
  }

  const res = await post("/internal/auth/password/verify", parsed.data);
  if (!res) return { error: UNAVAILABLE };
  if (res.status === 404) return { error: UNAVAILABLE };
  if (!res.ok) return { error: await refusal(res) };
  return { error: null };
}

// Ask for a reset link. Auth mails one if the address has an account and
// says nothing either way, so neither does this.
export async function forgotPasswordAction(input: { email: string }): Promise<Result> {
  const parsed = z.object({ email }).safeParse(input);
  if (!parsed.success) return { error: "Enter a valid email address." };

  if (await limitedByAddress("accounts:forgot", { limit: 5, windowMs: 60 * 60_000 })) {
    return { error: "Too many requests. Try again in an hour." };
  }

  const res = await post("/internal/auth/password/forgot", parsed.data);
  if (!res) return { error: UNAVAILABLE };
  if (res.status === 404) return { error: UNAVAILABLE };
  if (!res.ok) return { error: await refusal(res) };
  return { error: null };
}

// Spend the reset link and set the password. Every session ends with it.
export async function resetPasswordAction(input: {
  userId: string;
  token: string;
  password: string;
}): Promise<Result> {
  const parsed = z.object({ userId, token, password: newPassword }).safeParse(input);
  if (!parsed.success) {
    return { error: parsed.error.issues[0]?.message ?? "Check the form and try again." };
  }

  if (await limitedByAddress("accounts:reset", { limit: 10, windowMs: 60 * 60_000 })) {
    return { error: "Too many attempts. Try again in an hour." };
  }

  const res = await post("/internal/auth/password/reset", parsed.data);
  if (!res) return { error: UNAVAILABLE };
  if (res.status === 404) return { error: UNAVAILABLE };
  if (!res.ok) return { error: await refusal(res) };
  return { error: null };
}

// The signed-in person approves or refuses the device showing the code.
export async function deviceDecisionAction(input: {
  userCode: string;
  decision: "approve" | "deny";
}): Promise<Result> {
  const parsed = z
    .object({ userCode: z.string().trim().min(1).max(20), decision: z.enum(["approve", "deny"]) })
    .safeParse(input);
  if (!parsed.success) return { error: "Enter the code the device shows." };

  const session = await getServerSession();
  if (!session) return { error: "Your session expired — sign in again." };

  const limited = await rateLimit(sessionKey(session, "device:decide"), {
    limit: 10,
    windowMs: 60_000,
  });
  if (limited) return { error: "Too many attempts. Try again in a minute." };

  const res = await tryFetchWithTimeout(
    `${env.SERVER_URL}/internal/auth/device/${parsed.data.decision}`,
    {
      method: "POST",
      headers: { ...personHeaders(session), "content-type": "application/json" },
      body: JSON.stringify({ userCode: parsed.data.userCode }),
    },
  );
  if (!res) return { error: UNAVAILABLE };
  if (!res.ok) {
    return { error: await refusal(res, "No device is waiting for that code.") };
  }
  return { error: null };
}
