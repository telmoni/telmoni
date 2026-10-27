import type { NextRequest } from "next/server";

import { listDownload } from "@/lib/api/list-download";
import { sessionDownload } from "@/lib/server/data";
import { parseSessionQuery } from "@/lib/sessions";

// The sessions list as a file: the filters and the sort its URL carries, as
// CSV for a spreadsheet or the list's own JSON. It stands beside the sessions
// page rather than under it, where a session's id would be read as one.
export async function GET(
  request: NextRequest,
  { params }: { params: Promise<{ organization: string; project: string }> },
) {
  return listDownload(request, params, "sessions", (projectId, searchParams, format) =>
    sessionDownload(projectId, parseSessionQuery(searchParams), format),
  );
}
