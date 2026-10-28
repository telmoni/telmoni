import type { NextRequest } from "next/server";
import { z } from "zod";

import { extractProblem, tryFetchWithTimeout } from "@/lib/api/fetch";
import { privateJson, privateProblem } from "@/lib/api/respond";
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

export async function GET(
  request: NextRequest,
  { params }: RouteContext<"/api/v1/connectors/[connection]/deliveries">,
) {
  const { connection } = await params;
  const search = request.nextUrl.searchParams;
  const gate = await admitConnectorsCall(
    "connectors:deliveries",
    60,
    search.get("project") ?? "",
    connection,
  );
  if (!gate.ok) return privateProblem(gate.status, gate.error, gate.retryAfter);
  const before = search.get("before");
  if (before !== null && !z.guid().safeParse(before).success) {
    return privateProblem(400, "Invalid delivery.");
  }
  const query = new URLSearchParams({ limit: String(PAGE_SIZE) });
  if (before !== null) query.set("before", before);
  const res = await tryFetchWithTimeout(
    `${gate.base}/internal/v1/connectors/${encodeURIComponent(connection)}/deliveries?${query}`,
    { method: "GET", headers: gate.headers },
  );
  if (!res) return privateProblem(503, CONNECTORS_UNAVAILABLE);
  // The service's refusal keeps its status, so a bearer it no longer takes
  // reads as the ended session it is; its own failure is a bad gateway here.
  if (!res.ok) {
    const { message } = await extractProblem(res);
    return privateProblem(res.status >= 500 ? 502 : res.status, message);
  }
  const parsed = DeliveryLogPageSchema.safeParse(await res.json().catch(() => null));
  if (!parsed.success) return privateProblem(502, CONNECTORS_UNEXPECTED);
  return privateJson(parsed.data);
}
