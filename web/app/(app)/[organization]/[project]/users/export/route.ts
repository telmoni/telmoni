import type { NextRequest } from "next/server";

import { listDownload } from "@/lib/api/list-download";
import { userDownload } from "@/lib/server/data";
import { parseUserQuery } from "@/lib/users";

// The users list as a file: the filters and the sort its URL carries, as CSV
// for a spreadsheet or the list's own JSON.
export async function GET(
  request: NextRequest,
  { params }: { params: Promise<{ organization: string; project: string }> },
) {
  return listDownload(request, params, "users", (projectId, searchParams, format) =>
    userDownload(projectId, parseUserQuery(searchParams), format),
  );
}
