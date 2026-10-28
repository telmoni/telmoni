import type { NextRequest } from "next/server";
import { z } from "zod";

import { crossSite, privateJson, privateProblem, problem } from "@/lib/api/respond";
import { type Ceiling, admit } from "@/lib/server/admit";
import { listAuditExports, startAuditExport } from "@/lib/server/data";
import { ExportFormat } from "@/lib/types/enums";

/** Starts a person may make in an hour: a build reads up to a file's worth of
 *  the chain, and auth already keeps only their ten newest files. */
const STARTS: Ceiling = { call: "audit-export:start", limit: 20, per: "hour" };

const StartSchema = z
  .object({
    format: z.enum(ExportFormat),
    from: z.iso.datetime({ offset: true }).nullable().optional(),
    to: z.iso.datetime({ offset: true }).nullable().optional(),
  })
  .strict();

// The caller's exports of the organization's audit log, which the bell polls
// while one is building and the export dialog lists. The organization is the
// path's, resolved once inside the call. Same-origin only, as every
// authenticated route here is: a page elsewhere must not be able to start an
// export onto somebody's chain, nor read which ones they hold.
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
  return privateJson({ exports: result.exports, ...result.limits });
}

// Start one: auth queues it and builds it in the background, and the answer is
// the export as the list shows it, still queued.
export async function POST(request: NextRequest) {
  if (crossSite(request)) {
    return problem(403, "forbidden");
  }
  const gate = await admit(STARTS);
  if (!gate.ok) return privateProblem(gate.status, gate.error, gate.retryAfter);

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
      return privateJson({ export: result.export }, 202);
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
