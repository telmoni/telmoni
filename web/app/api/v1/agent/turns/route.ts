import { type NextRequest } from "next/server";
import { z } from "zod";

import { readBodyCapped } from "@/lib/api/body";
import { fetchWithTimeout } from "@/lib/api/fetch";
import { EVENT_STREAM_HEADERS, crossSite, privateProblem, problem } from "@/lib/api/respond";
import { isProjectId } from "@/lib/ids";
import { env } from "@/lib/env";
import { logger } from "@/lib/logger";
import { type Ceiling, admit } from "@/lib/server/admit";
import { agentHeaders } from "@/lib/server/entities/agent";
import { AGENT_MESSAGE_MAX, AGENT_STREAM_TIMEOUT_MS } from "@/lib/types/agent";

// Six bytes a character at the message ceiling — what JSON makes of a control
// character (`\u0001`) — and room for the ids around them. Anything past it
// is not a turn this lane would forward.
const BODY_CAP_BYTES = 32 * 1024;

// A turn holds a stream and a model call open for up to two minutes, so it is
// ceilinged here before it costs the server anything. The server keeps its own
// per-person budget and answers past it with a problem the panel words.
const TURNS: Ceiling = { call: "agent:turn", limit: 20 };

const TurnBody = z.object({
  projectId: z.string().refine(isProjectId),
  conversationId: z.uuid().nullable().optional(),
  // In characters, as the server counts: Zod's `max` counts UTF-16 units.
  message: z
    .string()
    .trim()
    .min(1)
    .refine((message) => [...message].length <= AGENT_MESSAGE_MAX),
});

const PROBLEM_JSON = "application/problem+json";

// ⚠ **Under `/api`, and a Route Handler rather than a Server Action.** The
// reply streams, and a Server Action answers once. And the proxy answers a
// lapsed session on `/api/*` with a JSON 401 the panel can word, where on any
// other path it redirects through the sign-in page — which a `fetch` would
// follow and hand back as a 200 of HTML.
export async function POST(request: NextRequest) {
  if (crossSite(request)) return problem(403, "forbidden");
  const gate = await admit(TURNS);
  if (!gate.ok) return privateProblem(gate.status, gate.error, gate.retryAfter);

  const raw = await readBodyCapped(request, BODY_CAP_BYTES);
  if (raw === null) {
    return problem(413, "payload too large");
  }
  let json: unknown;
  try {
    json = JSON.parse(new TextDecoder().decode(raw));
  } catch {
    return problem(400, "body is not JSON");
  }
  const body = TurnBody.safeParse(json);
  if (!body.success) {
    return problem(400, "invalid turn");
  }
  const { projectId, conversationId, message } = body.data;

  const headers = await agentHeaders(projectId);
  if (!headers) {
    return problem(404, "unknown project");
  }

  let res: Response;
  try {
    res = await fetchWithTimeout(
      `${env.SERVER_URL}/internal/v1/agent/turns`,
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
    return problem(503, "unavailable");
  }

  if (!res.ok) {
    return refusal(res);
  }
  if (!res.body) {
    logger.warn({ route: "agent:turns", status: res.status }, "agent: upstream sent no stream");
    return problem(502, "unavailable");
  }

  return new Response(res.body, { headers: EVENT_STREAM_HEADERS });
}

// The server's refusal as it answered it, so the panel reads the agent's own
// problem types. A body that is not JSON — a load balancer's error page — is
// not passed on; the panel words the status instead.
async function refusal(res: Response): Promise<Response> {
  const headers: Record<string, string> = { "cache-control": "no-store" };
  const retryAfter = res.headers.get("retry-after");
  if (retryAfter) headers["retry-after"] = retryAfter;

  const text = await res.text().catch(() => "");
  let answered: unknown = null;
  if ((res.headers.get("content-type") ?? "").includes("json")) {
    try {
      answered = JSON.parse(text);
    } catch {
      answered = null;
    }
  }
  if (answered === null || typeof answered !== "object") {
    return problem(res.status, "upstream error", { headers });
  }
  return new Response(JSON.stringify(answered), {
    status: res.status,
    headers: { ...headers, "content-type": PROBLEM_JSON },
  });
}
