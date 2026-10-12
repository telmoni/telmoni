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

/// The request's own path and query, as a URL to change and spell back with
/// `pathAndQuery`: for a Server Action, the page it was posted from, since an
/// action is posted to its page's own address, which the proxy hands on.
export async function requestUrl(): Promise<URL> {
  const path = (await headers()).get(PATH_HEADER);
  return new URL(path ?? "/", "http://console.invalid");
}

/// A URL's path and query alone: a path on this console, never another host.
export function pathAndQuery(url: URL): string {
  return `${url.pathname}${url.search}`;
}
