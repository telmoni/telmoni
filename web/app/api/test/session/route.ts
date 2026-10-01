import { type NextRequest, NextResponse } from "next/server";
import { z } from "zod";
import {
  SESSION_COOKIE,
  cookieOpts,
  sealSession,
  type SessionData,
} from "@/lib/auth/session";
import { fetchWithTimeout } from "@/lib/api/fetch";
import { env } from "@/lib/env";

const ALLOWED =
  process.env.ALLOW_TEST_SESSION === "true" &&
  process.env.NODE_ENV !== "production";

// Who to sign in, and what the cookie should say about how. Tokens are never
// the caller's to offer: auth mints the session for the person named, and the
// bearer sealed here is the one it answered with.
const BodySchema = z.object({
  userId: z.string().optional(),
  email: z.string().optional(),
  firstName: z.string().nullable().optional(),
  lastName: z.string().nullable().optional(),
  idToken: z.string().nullable().optional(),
  authMethod: z.string().nullable().optional(),
});

// What auth's own test door answers: the tokens a code exchange would.
const MintedSchema = z.object({
  accessToken: z.string().min(1),
  sessionId: z.string().min(1),
  refreshToken: z.string().nullable(),
  expiresIn: z.number(),
});

export async function POST(request: NextRequest) {
  if (!ALLOWED)
    return NextResponse.json({ error: "forbidden" }, { status: 403 });

  const parsed = BodySchema.safeParse(await request.json().catch(() => null));
  if (!parsed.success) {
    return NextResponse.json({ error: "invalid JSON" }, { status: 400 });
  }
  const body = parsed.data;
  const userId = body.userId ?? "test-user";
  const email = body.email || `${userId}@example.test`;

  // ⚠ **Auth mints the session; this door only seals it.** The server's door,
  // mounted only under ALLOW_TEST_SESSION and refused in a pod, records the
  // person as an exchange would and opens a session for them, so every
  // injected session is a real one: its bearer resolves, its refresh works,
  // and a sign-out ends it. Nothing in the console holds a key to mint with.
  const minted = await fetchWithTimeout(`${env.SERVER_URL}/test/session`, {
    method: "POST",
    headers: {
      "content-type": "application/json",
      "x-service-secret": env.SERVICE_SECRET,
    },
    body: JSON.stringify({
      userId,
      email,
      firstName: body.firstName ?? null,
      lastName: body.lastName ?? null,
    }),
  }).catch(() => null);
  const tokens = minted?.ok
    ? MintedSchema.safeParse(await minted.json().catch(() => null))
    : null;
  if (!tokens?.success) {
    return NextResponse.json(
      {
        error: "session_not_minted",
        message:
          "auth did not mint a test session — is the server started with ALLOW_TEST_SESSION=true?",
      },
      { status: 502 },
    );
  }

  const sessionData: SessionData = {
    userId,
    email,
    firstName: body.firstName ?? null,
    lastName: body.lastName ?? null,
    sessionRowId: null,
    accessToken: tokens.data.accessToken,
    sessionId: tokens.data.sessionId,
    refreshToken: tokens.data.refreshToken,
    expiresAt: Date.now() + tokens.data.expiresIn * 1000,
    idToken: body.idToken ?? null,
    authMethod: body.authMethod ?? null,
  };

  const sealed = await sealSession(sessionData);
  const response = NextResponse.json({ ok: true });
  response.cookies.set({
    name: SESSION_COOKIE,
    value: sealed,
    ...cookieOpts(60 * 60),
  });
  return response;
}

export async function DELETE() {
  if (!ALLOWED)
    return NextResponse.json({ error: "forbidden" }, { status: 403 });
  const response = NextResponse.json({ ok: true });
  response.cookies.delete(SESSION_COOKIE);
  return response;
}
