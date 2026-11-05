import { type NextRequest } from "next/server";

import { listDownload } from "@/lib/api/list-download";
import { traceDownload } from "@/lib/server/data";
import { parseTraceQuery } from "@/lib/traces";

// The trace list as a file: the filters and the sort its URL carries, as CSV
// for a spreadsheet or the list's own JSON.
export async function GET(
  request: NextRequest,
  { params }: { params: Promise<{ organization: string; project: string }> },
) {
  return listDownload(request, params, "traces", async (projectId, searchParams, format) => {
    // The page and its rows are the table's; a download is every row.
    return traceDownload(projectId, parseTraceQuery(searchParams), format);
  });
}
