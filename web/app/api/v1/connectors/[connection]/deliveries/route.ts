import { type NextRequest, NextResponse } from "next/server";
import { z } from "zod";

import { extractProblem, tryFetchWithTimeout } from "@/lib/api/fetch";
import { problem } from "@/lib/api/respond";
import {
  CONNECTORS_UNAVAILABLE,
  CONNECTORS_UNEXPECTED,
  admitConnectorsCall,
} from "@/lib/server/connectors-admit";
import { DeliveryLogPageSchema } from "@/lib/server/entities/connectors";

// A connection's delivery log, a page at a time, newest first. A read, so a
// Route Handler: Server Actions are for writes, and the client sends them one
// at a time, so paging back while a resend is out would wait on the resend.
// Every role on the project reads it; the service decides that from the
// bearer as it does for the listing. The ceiling is higher than the writes'
// because paging back through a busy log is several reads in a row.

const PAGE_SIZE = 25;

const PRIVATE = { "cache-control": "private, no-store" };

/** The session's own answer, so nothing in between keeps a copy. */
function reply(body: unknown) {
  return NextResponse.json(body, { headers: PRIVATE });
}

/** A read turned away, in the sentence the dialog shows. */
function refuse(sentence: string, status: number) {
  return problem(status, sentence, { headers: PRIVATE });
}

export async function GET(
  request: NextRequest,
  { params }: { params: Promise<{ connection: string }> },
) {
  const { connection } = await params;
  const search = request.nextUrl.searchParams;
  const gate = await admitConnectorsCall(
    "connectors:deliveries",
    60,
    search.get("project") ?? "",
    connection,
  );
  if (gate.error !== undefined) return refuse(gate.error, gate.status);
  const before = search.get("before");
  if (before !== null && !z.guid().safeParse(before).success) {
    return refuse("Invalid delivery.", 400);
  }
  const query = new URLSearchParams({ limit: String(PAGE_SIZE) });
  if (before !== null) query.set("before", before);
  const res = await tryFetchWithTimeout(
    `${gate.base}/internal/v1/connectors/${encodeURIComponent(connection)}/deliveries?${query}`,
    { method: "GET", headers: gate.headers },
  );
  if (!res) return refuse(CONNECTORS_UNAVAILABLE, 503);
  // The service's refusal keeps its status, so a bearer it no longer takes
  // reads as the ended session it is; its own failure is a bad gateway here.
  if (!res.ok) {
    const { message } = await extractProblem(res);
    return refuse(message, res.status >= 500 ? 502 : res.status);
  }
  const parsed = DeliveryLogPageSchema.safeParse(await res.json().catch(() => null));
  if (!parsed.success) return refuse(CONNECTORS_UNEXPECTED, 502);
  return reply(parsed.data);
}
