import type { NextRequest } from "next/server";

import { privateProblem } from "@/lib/api/respond";
import { isProjectId } from "@/lib/ids";
import { drawnAs } from "@/lib/messages";
import { type Ceiling, admit } from "@/lib/server/admit";
import { fetchMedia } from "@/lib/server/data";

/** Files a person may open in a minute: a conversation draws each image it
 *  holds once, and a page of them stays well under it. */
const READS: Ceiling = { call: "media:read", limit: 240 };

const HASH = /^[0-9a-f]{64}$/;

// A file a project keeps, for the trace page's conversations, each read
// filed by the server as its span's content is. Served so the browser runs
// nothing it holds: never sniffed, sandboxed, and anything but an image or
// audio a download.
export async function GET(request: NextRequest, { params }: RouteContext<"/api/v1/media/[sha256]">) {
  const gate = await admit(READS);
  if (!gate.ok) return privateProblem(gate.status, gate.error, gate.retryAfter);

  const projectId = request.nextUrl.searchParams.get("project") ?? "";
  if (!isProjectId(projectId)) return privateProblem(404, "Unknown project.");
  const { sha256 } = await params;
  if (!HASH.test(sha256)) return privateProblem(404, "No such file.");

  const access = await fetchMedia(projectId, sha256);
  switch (access.kind) {
    case "ok":
      return new Response(access.body, {
        headers: {
          "content-type": access.type,
          "x-content-type-options": "nosniff",
          "content-security-policy": "sandbox",
          "cache-control": "private, no-store",
          "content-disposition": drawnAs(access.type) === "download" ? "attachment" : "inline",
        },
      });
    case "missing":
      return privateProblem(404, "No such file.");
    case "forbidden":
      return privateProblem(403, "Not yours to read.");
    case "unavailable":
      return privateProblem(503, "The file could not be read.");
  }
}
