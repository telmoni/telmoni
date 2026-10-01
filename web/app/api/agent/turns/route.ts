import { type NextRequest, NextResponse } from "next/server";
import { z } from "zod";

import { readBodyCapped } from "@/lib/api/body";
import { fetchWithTimeout } from "@/lib/api/fetch";
import { rateLimit, sessionKey } from "@/lib/api/rate-limit";
import { getSession } from "@/lib/auth/session";
import { sessionEndKey } from "@/lib/auth/session-end-key";
import { isSessionBlacklisted } from "@/lib/auth/session-blacklist";
import { isProjectId } from "@/lib/connect";
import { env } from "@/lib/env";
import { logger } from "@/lib/logger";
import { AGENT_STREAM_TIMEOUT_MS, agentHeaders } from "@/lib/server/entities/agent";
import { AGENT_MESSAGE_MAX } from "@/lib/types/agent";

export const dynamic = "force-dynamic";
export const runtime = "nodejs";

// Four bytes a character at the message ceiling, and room for the ids and the
// JSON around them. Anything past it is not a turn this lane would forward.
const BODY_CAP_BYTES = 20 * 1024;

// A turn holds a stream and a model call open for up to two minutes, so it is
// ceilinged here before it costs the server anything. The server keeps its own
// per-person budget and answers past it with a problem the panel words.
const TURNS = { limit: 20, windowMs: 60_000 };

const TurnBody = z.object({
  projectId: z.string().refine(isProjectId),
  conversationId: z.uuid().nullable().optional(),
  message: z.string().trim().min(1).max(AGENT_MESSAGE_MAX),
});

const PROBLEM_JSON = "application/problem+json";

// ⚠ **Under `/api`, and a Route Handler rather than a Server Action.** The
// reply streams, and a Server Action answers once. And the proxy answers a
// lapsed session on `/api/*` with a JSON 401 the panel can word, where on any
// other path it redirects through the sign-in page — which a `fetch` would
// follow and hand back as a 200 of HTML.
export async function POST(request: NextRequest) {
  const secFetchSite = request.headers.get("sec-fetch-site");
  if (secFetchSite && secFetchSite !== "same-origin" && secFetchSite !== "none") {
    return NextResponse.json({ error: "forbidden" }, { status: 403 });
  }

  const session = await getSession();
  if (!session || (await isSessionBlacklisted(sessionEndKey(session)))) {
    return NextResponse.json({ error: "unauthenticated" }, { status: 401 });
  }

  const limited = await rateLimit(sessionKey(session, "agent:turn"), TURNS);
  if (limited) return limited;

  const raw = await readBodyCapped(request, BODY_CAP_BYTES);
  if (raw === null) {
    return NextResponse.json({ error: "payload too large" }, { status: 413 });
  }
  let json: unknown;
  try {
    json = JSON.parse(new TextDecoder().decode(raw));
  } catch {
    return NextResponse.json({ error: "body is not JSON" }, { status: 400 });
  }
  const body = TurnBody.safeParse(json);
  if (!body.success) {
    return NextResponse.json({ error: "invalid turn" }, { status: 400 });
  }
  const { projectId, conversationId, message } = body.data;

  const headers = await agentHeaders(projectId);
  if (!headers) {
    return NextResponse.json({ error: "unknown project" }, { status: 404 });
  }

  let res: Response;
  try {
    res = await fetchWithTimeout(
      `${env.SERVER_URL}/internal/agent/turns`,
      {
        method: "POST",
        headers: {
          ...headers,
          "content-type": "application/json",
          accept: "text/event-stream",
        },
        body: JSON.stringify({ conversation_id: conversationId ?? null, message }),
        // A panel that closes, or a tab that goes away, stops the model call
        // instead of leaving it to run to completion for nobody.
        signal: request.signal,
      },
      AGENT_STREAM_TIMEOUT_MS,
    );
  } catch (err) {
    logger.warn(
      { route: "agent:turns", error: err instanceof Error ? err.name : "unknown" },
      "agent: upstream unreachable",
    );
    return NextResponse.json({ error: "unavailable" }, { status: 503 });
  }

  if (!res.ok) {
    return refusal(res);
  }
  if (!res.body) {
    logger.warn({ route: "agent:turns", status: res.status }, "agent: upstream sent no stream");
    return NextResponse.json({ error: "unavailable" }, { status: 502 });
  }

  return new Response(res.body, {
    headers: {
      "Content-Type": "text/event-stream; charset=utf-8",
      "Cache-Control": "no-cache, no-transform",
      Connection: "keep-alive",
      "X-Accel-Buffering": "no",
    },
  });
}

// The server's refusal as it answered it, so the panel reads the agent's own
// problem types. A body that is not JSON — a load balancer's error page — is
// not passed on; the panel words the status instead.
async function refusal(res: Response): Promise<Response> {
  const headers: Record<string, string> = { "cache-control": "no-store" };
  const retryAfter = res.headers.get("retry-after");
  if (retryAfter) headers["retry-after"] = retryAfter;

  const text = await res.text().catch(() => "");
  let problem: unknown = null;
  if ((res.headers.get("content-type") ?? "").includes("json")) {
    try {
      problem = JSON.parse(text);
    } catch {
      problem = null;
    }
  }
  if (problem === null || typeof problem !== "object") {
    problem = { title: "upstream error", status: res.status };
  }
  return new Response(JSON.stringify(problem), {
    status: res.status,
    headers: { ...headers, "content-type": PROBLEM_JSON },
  });
}
