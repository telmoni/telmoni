import { NextResponse, type NextRequest } from "next/server";
import { z } from "zod";

import { rateLimit, sessionKey } from "@/lib/api/rate-limit";
import { fetchProjectBySlug, getServerContext, traceDownload } from "@/lib/server/data";
import { getServerSession } from "@/lib/server/session";
import { many, one, type SearchParams } from "@/lib/list-query";
import { parseTraceQuery } from "@/lib/traces";

/** Downloads a person may make in an hour: a file or two a sitting, while a
 *  script looping here would have the server sum ten thousand runs a time. */
const DOWNLOADS_AN_HOUR = { limit: 30, windowMs: 60 * 60_000 };

/** The most runs a download names: a page's chosen rows, as the server
 *  takes them. */
const RUNS_MAX = 100;

// The trace list as a file: the filters and the sort its URL carries, or the
// runs chosen on its page, as CSV for a spreadsheet or the list's own JSON.
// Telemetry decides what the person may read and writes the file; this names
// it and keeps it out of every cache. Same-origin only, as every
// authenticated route here is.
export async function GET(
  request: NextRequest,
  { params }: { params: Promise<{ organization: string; project: string }> },
) {
  const site = request.headers.get("sec-fetch-site");
  if (site && site !== "same-origin" && site !== "none") {
    return NextResponse.json({ error: "forbidden" }, { status: 403 });
  }

  const session = await getServerSession();
  if (!session) {
    return NextResponse.json({ error: "unauthenticated" }, { status: 401 });
  }
  const limited = await rateLimit(sessionKey(session, "traces:download"), DOWNLOADS_AN_HOUR);
  if (limited) return limited;

  const { project: slug } = await params;
  const [gate, project] = await Promise.all([getServerContext(), fetchProjectBySlug(slug)]);
  if (!gate || !project) {
    return NextResponse.json({ error: "unavailable" }, { status: 503 });
  }

  const searchParams: SearchParams = {};
  for (const key of new Set(request.nextUrl.searchParams.keys())) {
    searchParams[key] = request.nextUrl.searchParams.getAll(key);
  }
  const format = one(searchParams.format) === "json" ? "json" : "csv";
  // A run is named by its UUID, and nothing else goes on to the server.
  const runs = many(searchParams.run).filter((run) => z.uuid().safeParse(run).success);
  if (runs.length > RUNS_MAX) {
    return NextResponse.json({ error: "too many runs" }, { status: 400 });
  }
  // The page and its rows are the table's; a download is every row.
  const result = await traceDownload(project.id, parseTraceQuery(searchParams), format, runs);
  if (result.kind === "forbidden") {
    return NextResponse.json({ error: "forbidden" }, { status: 403 });
  }
  if (result.kind === "refused") {
    return NextResponse.json({ error: "bad request" }, { status: 400 });
  }
  if (result.kind === "unavailable") {
    return NextResponse.json({ error: "unavailable" }, { status: 503 });
  }

  const day = new Date().toISOString().slice(0, 10);
  return new NextResponse(result.body, {
    headers: {
      "Content-Type": result.contentType,
      "Content-Disposition": `attachment; filename="${project.slug}-traces-${day}.${format}"`,
      "Cache-Control": "no-store, private",
    },
  });
}
