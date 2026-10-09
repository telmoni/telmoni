import { NextResponse, type NextRequest } from "next/server";
import { z } from "zod";

import { rateLimit, sessionKey } from "@/lib/api/rate-limit";
import { listAuditExports, startAuditExport } from "@/lib/server/data";
import { getServerSession } from "@/lib/server/session";

export const dynamic = "force-dynamic";
export const runtime = "nodejs";

const NO_STORE = { "Cache-Control": "no-store, private" };

/** Starts a person may make in an hour: a build reads up to a file's worth of
 *  the chain, and auth already keeps only their ten newest files. */
const STARTS_AN_HOUR = { limit: 20, windowMs: 60 * 60_000 };

const StartSchema = z
  .object({
    format: z.enum(["json", "csv"]),
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
    return NextResponse.json({ error: "forbidden" }, { status: 403 });
  }
  const result = await listAuditExports();
  if (result.kind === "forbidden") {
    return NextResponse.json({ error: "forbidden" }, { status: 403 });
  }
  if (result.kind === "unavailable") {
    return NextResponse.json({ error: "unavailable" }, { status: 503 });
  }
  return NextResponse.json({ exports: result.exports }, { headers: NO_STORE });
}

// Start one: auth queues it and builds it in the background, and the answer is
// the export as the list shows it, still queued.
export async function POST(request: NextRequest) {
  if (crossSite(request)) {
    return NextResponse.json({ error: "forbidden" }, { status: 403 });
  }
  const session = await getServerSession();
  if (!session) {
    return NextResponse.json({ error: "unauthenticated" }, { status: 401 });
  }
  const limited = await rateLimit(sessionKey(session, "audit-export:start"), STARTS_AN_HOUR);
  if (limited) return limited;

  const parsed = StartSchema.safeParse(await request.json().catch(() => null));
  if (!parsed.success) {
    return NextResponse.json({ error: "invalid" }, { status: 400 });
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
      return NextResponse.json({ error: "forbidden" }, { status: 403 });
    case "busy":
      return NextResponse.json({ error: "busy" }, { status: 409 });
    case "invalid":
      return NextResponse.json({ error: "invalid" }, { status: 400 });
    case "unavailable":
      return NextResponse.json({ error: "unavailable" }, { status: 503 });
  }
}
