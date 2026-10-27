import { NextResponse, type NextRequest } from "next/server";

import { rateLimit, sessionKey } from "@/lib/api/rate-limit";
import { problem } from "@/lib/api/respond";
import { many, one, type SearchParams } from "@/lib/list-query";
import { fetchProjectBySlug, getServerContext } from "@/lib/server/data";
import type { DownloadAccess, DownloadFormat } from "@/lib/server/entities/traces";
import { getServerSession } from "@/lib/server/session";
import { FILTERS, parseFilters, type ListName } from "@/lib/traces";

/** Downloads a person may make of one list in an hour: a file or two a
 *  sitting, while a script looping here would have the server sum ten
 *  thousand rows a time. */
const DOWNLOADS_AN_HOUR = { limit: 30, windowMs: 60 * 60_000 };

/** A list's download route: same-origin, as every authenticated route here
 *  is, refusing a filter value the list would let go, since dropped it would
 *  widen the file; the file `read` has the server write, kept out of caches. */
export async function listDownload(
  request: NextRequest,
  params: Promise<{ organization: string; project: string }>,
  list: ListName,
  read: (
    projectId: string,
    searchParams: SearchParams,
    format: DownloadFormat,
  ) => Promise<DownloadAccess | NextResponse>,
): Promise<NextResponse> {
  const site = request.headers.get("sec-fetch-site");
  if (site && site !== "same-origin" && site !== "none") {
    return problem(403, "forbidden");
  }

  const session = await getServerSession();
  if (!session) {
    return problem(401, "unauthenticated");
  }
  const limited = await rateLimit(sessionKey(session, `${list}:download`), DOWNLOADS_AN_HOUR);
  if (limited) return limited;

  const { project: slug } = await params;
  const [gate, project] = await Promise.all([getServerContext(), fetchProjectBySlug(slug)]);
  if (!gate || !project) {
    return problem(503, "unavailable");
  }

  const searchParams: SearchParams = {};
  for (const key of new Set(request.nextUrl.searchParams.keys())) {
    searchParams[key] = request.nextUrl.searchParams.getAll(key);
  }
  const format = one(searchParams.format) === "json" ? "json" : "csv";
  const filters = parseFilters(searchParams);
  if (FILTERS.some((filter) => many(searchParams[filter]).length !== (filters[filter]?.length ?? 0))) {
    return problem(400, "bad request");
  }
  const result = await read(project.id, searchParams, format);
  if (result instanceof NextResponse) return result;
  if (result.kind === "forbidden") {
    return problem(403, "forbidden");
  }
  if (result.kind === "refused") {
    return problem(400, "bad request");
  }
  if (result.kind === "unavailable") {
    return problem(503, "unavailable");
  }

  const day = new Date().toISOString().slice(0, 10);
  return new NextResponse(result.body, {
    headers: {
      "Content-Type": result.contentType,
      "Content-Disposition": `attachment; filename="${project.slug}-${list}-${day}.${format}"`,
      "Cache-Control": "no-store, private",
    },
  });
}
