import { type NextRequest, NextResponse } from "next/server";
import { z } from "zod";

import { tryFetchWithTimeout } from "@/lib/api/fetch";
import { clientKey, rateLimitRetryAfter } from "@/lib/api/rate-limit";
import { CONNECT_COOKIE, cookieOpts, getSession, sealConnect } from "@/lib/auth/session";
import { connectorsPath, isOAuthProvider, isProjectId } from "@/lib/connect";
import { env } from "@/lib/env";
import { logger } from "@/lib/logger";
import { fetchProjectAnywhere, identityContext, projectHeaders } from "@/lib/server/data";
import { FeatureOffProblemSchema } from "@/lib/server/flags";

export const dynamic = "force-dynamic";

const Authorized = z.object({ url: z.string().url(), state: z.string().min(1) });

// Every press past the session check mints a state row in the service and a
// round trip to it, so the same per-source ceiling the sign-in door carries.
const CONNECT_START_CEILING = { limit: 30, windowMs: 60_000 };

// ⚠ **A GET Route Handler, not a Server Action, and not under `/api`.** The
// proxy answers a bare 401 to `/api/*` without a session but redirects any
// other protected path through login with `returnTo`, which is what brings a
// lapsed session back here with its query intact. And the CSP's
// `form-action` would block a Server Action's redirect to the vendor: a link
// to a GET route is a navigation, which it does not govern. The sign-in flow
// is a GET route for the same two reasons.
export async function GET(
  request: NextRequest,
  { params }: { params: Promise<{ provider: string }> },
) {
  const { provider } = await params;
  // The OAuth two only: the webhook is connected in a dialog on the page and
  // has no vendor to be sent to.
  if (!isOAuthProvider(provider)) {
    return NextResponse.redirect(new URL("/console", env.AUTH_URL));
  }
  const projectId = request.nextUrl.searchParams.get("project") ?? "";
  if (!isProjectId(projectId)) {
    return NextResponse.redirect(new URL("/console", env.AUTH_URL));
  }

  const session = await getSession();
  if (!session) {
    const login = new URL("/auth/login", env.AUTH_URL);
    login.searchParams.set("returnTo", request.nextUrl.pathname + request.nextUrl.search);
    return NextResponse.redirect(login);
  }

  const retryAfter = await rateLimitRetryAfter(
    clientKey(request, "connect:start"),
    CONNECT_START_CEILING,
  );
  if (retryAfter !== null) {
    return new NextResponse("Too many connection attempts. Try again in a minute.", {
      status: 429,
      headers: {
        "content-type": "text/plain; charset=utf-8",
        "retry-after": String(retryAfter),
        "cache-control": "no-store, max-age=0",
      },
    });
  }

  const base = env.SERVER_URL;

  // The project's own organization, not the one this request stands in: the
  // path names none, and the cookie follows whichever tab opened a page last.
  const [ctx, project] = await Promise.all([identityContext(), fetchProjectAnywhere(projectId)]);
  if (!ctx || !project) return NextResponse.redirect(new URL("/console", env.AUTH_URL));

  const back = (error: string) =>
    NextResponse.redirect(
      new URL(connectorsPath(project.organizationSlug, project.slug, { error }), env.AUTH_URL),
    );

  // The service decides: a non-owner is refused there, and so is a
  // switched-off flag. This route only relays the bearer and the project;
  // the role is auth's to derive, and nothing here asserts it.
  const res = await tryFetchWithTimeout(
    `${base}/internal/connectors/${provider}/authorize`,
    {
      method: "POST",
      headers: projectHeaders({ ...ctx, organizationId: project.organizationId }, projectId),
    },
  );
  if (!res) return back("unavailable");
  if (res.status === 403) return back("forbidden");
  if (!res.ok) {
    const problem = FeatureOffProblemSchema.safeParse(await res.json().catch(() => null));
    if (problem.success) return back("off");
    logger.warn({ provider, status: res.status }, "connector authorize refused");
    return back("authorize");
  }
  const parsed = Authorized.safeParse(await res.json().catch(() => null));
  if (!parsed.success) return back("authorize");

  const sealed = await sealConnect({
    state: parsed.data.state,
    organizationId: project.organizationId,
    projectId,
    provider,
  });
  const response = NextResponse.redirect(parsed.data.url);
  response.headers.set("cache-control", "no-store, max-age=0");
  response.cookies.set({
    name: CONNECT_COOKIE,
    value: sealed,
    ...cookieOpts(600),
  });
  return response;
}
