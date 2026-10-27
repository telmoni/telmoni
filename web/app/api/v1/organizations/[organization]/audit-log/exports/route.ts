import { NextResponse, type NextRequest } from "next/server";
import { z } from "zod";

import { rateLimit, sessionKey } from "@/lib/api/rate-limit";
import { problem } from "@/lib/api/respond";
import { listAuditExports, startAuditExport } from "@/lib/server/data";
import { getServerSession } from "@/lib/server/session";
import { ExportFormat } from "@/lib/types/enums";

const NO_STORE = { "Cache-Control": "no-store, private" };

/** Starts a person may make in an hour: a build reads up to a file's worth of
 *  the chain, and auth already keeps only their ten newest files. */
const STARTS_AN_HOUR = { limit: 20, windowMs: 60 * 60_000 };

const StartSchema = z
  .object({
    format: z.enum(ExportFormat),
    from: z.iso.datetime({ offset: true }).nullable().optional(),
    to: z.iso.datetime({ offset: true }).nullable().optional(),
  })
  .strict();

// Same-origin only, as every authenticated route here is: a page elsewhere
// must not be able to start an export onto somebody's chain, nor read which
// ones they hold.
function crossSite(request: NextRequest): boolean {
  const site = request.headers.get("sec-fetch-site");
  return site !== null && site !== "same-origin" && site !== "none";
}

// The caller's exports of the organization's audit log, which the bell polls
// while one is building and the export dialog lists. The organization is the
// path's, resolved once inside the call.
export async function GET(request: NextRequest) {
  if (crossSite(request)) {
    return problem(403, "forbidden");
  }
  const result = await listAuditExports();
  if (result.kind === "forbidden") {
    return problem(403, "forbidden");
  }
  if (result.kind === "unavailable") {
    return problem(503, "unavailable");
  }
  return NextResponse.json({ exports: result.exports, ...result.limits }, { headers: NO_STORE });
}

// Start one: auth queues it and builds it in the background, and the answer is
// the export as the list shows it, still queued.
export async function POST(request: NextRequest) {
  if (crossSite(request)) {
    return problem(403, "forbidden");
  }
  const session = await getServerSession();
  if (!session) {
    return problem(401, "unauthenticated");
  }
  const limited = await rateLimit(sessionKey(session, "audit-export:start"), STARTS_AN_HOUR);
  if (limited) return limited;

  const parsed = StartSchema.safeParse(await request.json().catch(() => null));
  if (!parsed.success) {
    return problem(400, "invalid");
  }
  const result = await startAuditExport({
    format: parsed.data.format,
    from: parsed.data.from ?? null,
    to: parsed.data.to ?? null,
  });
  switch (result.kind) {
    case "ok":
      return NextResponse.json({ export: result.export }, { status: 202, headers: NO_STORE });
    case "forbidden":
      return problem(403, "forbidden");
    case "busy":
      return problem(409, "busy");
    case "invalid":
      return problem(400, "invalid");
    case "unavailable":
      return problem(503, "unavailable");
  }
}
