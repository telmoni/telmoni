import { type NextRequest } from "next/server";

import { listDownload } from "@/lib/api/list-download";
import { sessionDownload } from "@/lib/server/entities/sessions";
import { parseSessionQuery } from "@/lib/sessions";

// The sessions list as a file: the filters and the sort its URL carries, as
// CSV for a spreadsheet or the list's own JSON.
export async function GET(
  request: NextRequest,
  { params }: { params: Promise<{ organization: string; project: string }> },
) {
  return listDownload(request, params, "sessions", (projectId, searchParams, format) =>
    sessionDownload(projectId, parseSessionQuery(searchParams), format),
  );
}
