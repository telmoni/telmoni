import { headers } from "next/headers";

import { PATH_HEADER } from "@/lib/proxy/organization";
import { withLeadingSegments } from "@/lib/slug";

/// The request's own path with its leading segments replaced by `segments`:
/// the redirect target for a path spelled with an id or a stray capital. The
/// proxy hands the path on, since a layout is given only its params; without
/// it, the bare path `segments` spell.
export async function canonicalPath(segments: readonly string[]): Promise<string> {
  const path = (await headers()).get(PATH_HEADER);
  return withLeadingSegments(path ?? "/", segments);
}
