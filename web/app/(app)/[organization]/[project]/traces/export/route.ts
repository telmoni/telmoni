import { NextResponse, type NextRequest } from "next/server";
import { z } from "zod";

import { listDownload } from "@/lib/api/list-download";
import { many } from "@/lib/list-query";
import { traceDownload } from "@/lib/server/data";
import { RUNS_MAX, parseTraceQuery } from "@/lib/traces";

// The trace list as a file: the filters and the sort its URL carries, or the
// runs chosen on its page, as CSV for a spreadsheet or the list's own JSON.
export async function GET(
  request: NextRequest,
  { params }: { params: Promise<{ organization: string; project: string }> },
) {
  return listDownload(request, params, "traces", async (projectId, searchParams, format) => {
    // A UUID of any version, as ingest takes one; dropped, a chosen run would widen the file.
    const runs = many(searchParams.run, RUNS_MAX + 1);
    if (runs.some((run) => !z.guid().safeParse(run).success)) {
      return NextResponse.json({ error: "bad request" }, { status: 400 });
    }
    if (runs.length > RUNS_MAX) {
      return NextResponse.json({ error: "too many runs" }, { status: 400 });
    }
    // The page and its rows are the table's; a download is every row.
    return traceDownload(projectId, parseTraceQuery(searchParams), format, runs);
  });
}
