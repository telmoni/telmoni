import { type NextRequest, NextResponse } from "next/server";

import { rateLimit, sessionKey } from "@/lib/api/rate-limit";
import { getSession } from "@/lib/auth/session";
import { sessionEndKey } from "@/lib/auth/session-end-key";
import {
  isSessionBlacklisted,
  sessionRevokedChannel,
} from "@/lib/auth/session-blacklist";
import { organizationChannel, userChannel } from "@/lib/events/publisher";
import { subscribe } from "@/lib/events/subscriber";
import { RealtimeEventSchema, type RealtimeEvent } from "@/lib/events/types";
import { getServerContext } from "@/lib/server/entities/organization";
import { logger } from "@/lib/logger";

export const dynamic = "force-dynamic";
export const runtime = "nodejs";

const STREAM_OPENS = { limit: 60, windowMs: 60_000 };

export async function GET(request: NextRequest) {
  const secFetchSite = request.headers.get("sec-fetch-site");
  if (secFetchSite && secFetchSite !== "same-origin" && secFetchSite !== "none") {
    return NextResponse.json({ error: "forbidden" }, { status: 403 });
  }

  const session = await getSession();
  if (!session) {
    return NextResponse.json({ error: "unauthenticated" }, { status: 401 });
  }

  if (await isSessionBlacklisted(sessionEndKey(session))) {
    return NextResponse.json({ error: "unauthenticated" }, { status: 401 });
  }

  // The one authenticated door that allocates something long-lived per hit:
  // a stream, a keepalive timer and a share of the subscriber. Ceilinged
  // before the `/me` round trip below so a runaway client costs Redis one
  // script call and auth nothing. The listener reconnects at most twice a
  // minute, so sixty is a tab count nobody reaches by hand.
  const limited = await rateLimit(sessionKey(session, "events:open"), STREAM_OPENS);
  if (limited) return limited;

  const ctx = await getServerContext();
  if (!ctx) {
    return NextResponse.json(
      { error: "unavailable" },
      { status: 503, headers: { "Retry-After": "30" } },
    );
  }
  // The person's own channel, and one per organization they are in — their
  // own included, which is an entry like any other since organizations
  // stopped sharing their owner's id.
  const channels = [
    userChannel(session.email),
    ...ctx.organizations.map((o) => organizationChannel(o.organizationId)),
  ];

  const encoder = new TextEncoder();
  let controller: ReadableStreamDefaultController<Uint8Array> | null = null;
  let keepAliveTimer: NodeJS.Timeout | null = null;
  let unsubscribe: (() => void) | null = null;
  let unsubscribeRevocation: (() => void) | null = null;
  let isClosed = false;

  const cleanup = () => {
    if (isClosed) return;
    isClosed = true;
    request.signal.removeEventListener("abort", cleanup);
    if (keepAliveTimer) {
      clearInterval(keepAliveTimer);
      keepAliveTimer = null;
    }
    unsubscribe?.();
    unsubscribe = null;
    unsubscribeRevocation?.();
    unsubscribeRevocation = null;
    try {
      controller?.close();
    } catch {
    }
  };

  const send = (chunk: string) => {
    if (isClosed) return;
    try {
      controller?.enqueue(encoder.encode(chunk));
    } catch {
      cleanup();
    }
  };

  const closeRevoked = () => {
    send('event: close\ndata: {"reason":"session_revoked"}\n\n');
    cleanup();
  };

  const stream = new ReadableStream<Uint8Array>({
    start(c) {
      controller = c;
      request.signal.addEventListener("abort", cleanup);

      send(": connected\n\n");

      // Revocation arrives two ways. The channel is instant but only reaches a
      // stream that is subscribed when it fires; the poll below is the
      // backstop for a blacklist written while this subscriber was
      // reconnecting, and it doubles as the keepalive.
      const endKey = sessionEndKey(session);
      if (endKey) {
        unsubscribeRevocation = subscribe([sessionRevokedChannel(endKey)], closeRevoked);
      }

      keepAliveTimer = setInterval(async () => {
        if (await isSessionBlacklisted(sessionEndKey(session))) {
          closeRevoked();
          return;
        }
        send(": keepalive\n\n");
      }, 25_000);

      unsubscribe = subscribe(channels, (_channel, message) => {
        let body: unknown;
        try {
          body = JSON.parse(message);
        } catch (e) {
          logger.warn({ err: e }, "events: failed to parse incoming redis event");
          return;
        }
        const parsed = RealtimeEventSchema.safeParse(body);
        if (!parsed.success) {
          logger.warn(
            { issues: parsed.error.issues },
            "events: dropping an unrecognised redis event",
          );
          return;
        }
        const event: RealtimeEvent = parsed.data;
        send(`event: ${event.type}\ndata: ${JSON.stringify(event.data)}\n\n`);
      });
    },
    cancel() {
      cleanup();
    },
  });

  return new Response(stream, {
    headers: {
      "Content-Type": "text/event-stream; charset=utf-8",
      "Cache-Control": "no-cache, no-transform",
      Connection: "keep-alive",
      "X-Accel-Buffering": "no",
    },
  });
}
