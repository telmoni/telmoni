import { NextResponse, type NextRequest } from "next/server";
import { z } from "zod";

import { ProblemType } from "@/lib/api/problem";
import { rateLimit, sessionKey } from "@/lib/api/rate-limit";
import { problem } from "@/lib/api/respond";
import { activeOrganization, auditExportFile, getServerContext } from "@/lib/server/data";
import { getServerSession } from "@/lib/server/session";

/** Downloads a person may make in an hour. A person takes a file once or
 *  twice; a script looping on this route would pull a whole file each time —
 *  up to 24 MiB, held in auth's memory and the console's on its way out — so
 *  the cap is what bounds the bytes a loop can draw. */
const DOWNLOADS_AN_HOUR = { limit: 10, windowMs: 60 * 60_000 };

// One finished export of the organization's audit log, as a download. Auth
// hands it to the person who asked for it alone, while they still hold the
// role and while it is kept; this names the file and keeps it out of every
// cache. Same-origin only, as every authenticated route here is.
export async function GET(
  request: NextRequest,
  { params }: { params: Promise<{ id: string }> },
) {
  const site = request.headers.get("sec-fetch-site");
  if (site && site !== "same-origin" && site !== "none") {
    return problem(403, "forbidden");
  }

  const session = await getServerSession();
  if (!session) {
    return problem(401, "unauthenticated");
  }
  const limited = await rateLimit(sessionKey(session, "audit-export:download"), DOWNLOADS_AN_HOUR);
  if (limited) return limited;

  const gate = await getServerContext();
  const organization = gate && !gate.organizationNotFound ? activeOrganization(gate) : null;
  if (!organization) {
    return problem(503, "unavailable");
  }

  // An export's id is a UUID, and nothing else goes into auth's path: a
  // segment such as `..` would otherwise name another route there.
  const { id } = await params;
  if (!z.uuid().safeParse(id).success) {
    return problem(404, "gone");
  }
  const result = await auditExportFile(id);
  if (result.kind === "forbidden") {
    return problem(403, "forbidden");
  }
  if (result.kind === "missing") {
    return problem(404, "gone");
  }
  // Auth's own refusal, typed as it answered it, so the bell tells the
  // organization's spent day from this route's hourly ceiling.
  if (result.kind === "spent") {
    return problem(429, "spent", {
      type: ProblemType.TenantRateLimited,
      headers: result.retryAfter ? { "Retry-After": result.retryAfter } : {},
    });
  }
  if (result.kind === "unavailable") {
    return problem(503, "unavailable");
  }

  const extension = result.contentType.startsWith("text/csv") ? "csv" : "json";
  const day = new Date().toISOString().slice(0, 10);
  return new NextResponse(result.body, {
    headers: {
      "Content-Type": result.contentType,
      "Content-Disposition": `attachment; filename="${organization.slug}-audit-log-${day}.${extension}"`,
      "Cache-Control": "no-store, private",
    },
  });
}
