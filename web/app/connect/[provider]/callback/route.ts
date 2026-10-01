import { type NextRequest, NextResponse } from "next/server";

import { tryFetchWithTimeout } from "@/lib/api/fetch";
import { CONNECT_COOKIE, getSession, unsealConnect } from "@/lib/auth/session";
import { connectorsPath, isOAuthProvider } from "@/lib/connect";
import { env } from "@/lib/env";
import { logger } from "@/lib/logger";
import { fetchProject, identityContext, projectHeaders } from "@/lib/server/data";

export const dynamic = "force-dynamic";

// The vendor sends the browser back here with `code` and `state`, or with
// `error` when the person pressed Cancel. The project it is all for rides the
// sealed cookie the start route set — the query string is the vendor's to
// fill — and the service re-checks the state against the person and project
// that started it, so a forged cookie cannot move an install either.
export async function GET(
  request: NextRequest,
  { params }: { params: Promise<{ provider: string }> },
) {
  const { provider } = await params;
  if (!isOAuthProvider(provider)) {
    return NextResponse.redirect(new URL("/console", env.AUTH_URL));
  }

  const cookie = request.cookies.get(CONNECT_COOKIE)?.value ?? "";
  const pending = await unsealConnect(cookie);
  if (!pending || pending.provider !== provider) {
    // No cookie means no project to land on: the handshake outlived its ten
    // minutes, or this is not the browser that started it.
    return NextResponse.redirect(new URL("/console", env.AUTH_URL));
  }

  const done = (query: Record<string, string>) => {
    const response = NextResponse.redirect(
      new URL(connectorsPath(pending.projectId, query), env.AUTH_URL),
    );
    response.cookies.delete(CONNECT_COOKIE);
    return response;
  };

  const query = request.nextUrl.searchParams;
  const vendorError = query.get("error");
  if (vendorError) {
    // `access_denied` is the person pressing Cancel. No service call: nothing
    // was granted, and the state is left to expire.
    return done({ error: vendorError === "access_denied" ? "denied" : "vendor" });
  }
  const state = query.get("state");
  if (!state || state !== pending.state) return done({ error: "callback" });
  const code = query.get("code");
  if (!code) return done({ error: "callback" });

  const session = await getSession();
  if (!session) {
    // Keep the cookie: login brings the browser back here with the query
    // intact, and the handshake can still finish inside its ten minutes.
    const login = new URL("/auth/login", env.AUTH_URL);
    login.searchParams.set("returnTo", request.nextUrl.pathname + request.nextUrl.search);
    return NextResponse.redirect(login);
  }

  const base = env.SERVER_URL;

  const [ctx, project] = await Promise.all([identityContext(), fetchProject(pending.projectId)]);
  if (!ctx || !project) return done({ error: "project" });

  const res = await tryFetchWithTimeout(
    `${base}/internal/connectors/${provider}/callback`,
    {
      method: "POST",
      headers: {
        ...projectHeaders(ctx, pending.projectId),
        "content-type": "application/json",
      },
      body: JSON.stringify({ code, state }),
    },
  );
  if (!res) return done({ error: "unavailable" });
  if (res.status === 403) return done({ error: "forbidden" });
  if (!res.ok) {
    logger.warn({ provider, status: res.status }, "connector callback refused");
    return done({ error: "callback" });
  }
  return done({ connected: provider });
}
